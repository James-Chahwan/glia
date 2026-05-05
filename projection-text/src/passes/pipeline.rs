//! `Pipeline` — ordered composition of `Pass`es with declared dataflow.
//!
//! Three execution modes match the three things you typically want in a
//! research-iteration loop:
//!
//!   * `run` — execute every pass in order; fail fast if any pass's declared
//!     inputs aren't yet produced. The default for a full synth run.
//!   * `run_only` — execute only the named passes (whitelist). Used to re-run
//!     a single pass against a context staged by a prior full run.
//!   * `run_skipping` — execute every pass except the named ones (blacklist).
//!     Used for ablation: skip pass N, measure delta in downstream output.
//!
//! Inputs are checked against artifacts already in `PassContext` plus any
//! produced earlier in the same run, so a partial pipeline (`run_only` after
//! loading a prior `seeds` artifact) still validates correctly.

use std::collections::HashSet;

use anyhow::{Result, anyhow};

use crate::passes::context::PassContext;
use crate::passes::traits::Pass;

pub struct Pipeline {
    passes: Vec<Box<dyn Pass>>,
}

impl Pipeline {
    pub fn new() -> Self {
        Self { passes: Vec::new() }
    }

    pub fn add<P: Pass + 'static>(mut self, pass: P) -> Self {
        self.passes.push(Box::new(pass));
        self
    }

    /// Names of all registered passes, in declaration order.
    pub fn names(&self) -> Vec<&str> {
        self.passes.iter().map(|p| p.name()).collect()
    }

    /// Run every pass. Errors on first input-validation failure or pass error.
    pub fn run(&self, ctx: &mut PassContext) -> Result<()> {
        let mut produced: HashSet<String> = HashSet::new();
        for pass in &self.passes {
            self.check_inputs(pass.as_ref(), ctx, &produced)?;
            eprintln!("[pipeline] run pass: {}", pass.name());
            pass.run(ctx)?;
            for o in pass.outputs() {
                produced.insert((*o).to_string());
            }
        }
        Ok(())
    }

    /// Run only the named passes. Useful for re-running a single pass on a
    /// pre-staged context (e.g. seeds artifact loaded from disk).
    pub fn run_only(&self, ctx: &mut PassContext, only: &[&str]) -> Result<()> {
        let allow: HashSet<&str> = only.iter().copied().collect();
        let mut produced: HashSet<String> = HashSet::new();
        for pass in &self.passes {
            if !allow.contains(pass.name()) {
                continue;
            }
            self.check_inputs(pass.as_ref(), ctx, &produced)?;
            eprintln!("[pipeline] run pass: {}", pass.name());
            pass.run(ctx)?;
            for o in pass.outputs() {
                produced.insert((*o).to_string());
            }
        }
        Ok(())
    }

    /// Run all passes except the named ones. Used for ablation runs.
    pub fn run_skipping(&self, ctx: &mut PassContext, skip: &[&str]) -> Result<()> {
        let deny: HashSet<&str> = skip.iter().copied().collect();
        let mut produced: HashSet<String> = HashSet::new();
        for pass in &self.passes {
            if deny.contains(pass.name()) {
                eprintln!("[pipeline] skip pass: {}", pass.name());
                continue;
            }
            self.check_inputs(pass.as_ref(), ctx, &produced)?;
            eprintln!("[pipeline] run pass: {}", pass.name());
            pass.run(ctx)?;
            for o in pass.outputs() {
                produced.insert((*o).to_string());
            }
        }
        Ok(())
    }

    fn check_inputs(
        &self,
        pass: &dyn Pass,
        ctx: &PassContext,
        produced: &HashSet<String>,
    ) -> Result<()> {
        for input in pass.inputs() {
            if ctx.has(input) {
                continue;
            }
            if produced.contains(*input) {
                continue;
            }
            return Err(anyhow!(
                "pass {} requires input artifact {input}, but no upstream pass produced it",
                pass.name()
            ));
        }
        Ok(())
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use repo_graph_core::RepoId;
    use repo_graph_graph::build_python;

    fn empty_ctx() -> PassContext {
        let graph = build_python(RepoId::from_canonical("test"), vec![]).unwrap();
        PassContext::new(graph, String::new())
    }

    /// A pass that records the order it ran in, optionally produces an artifact,
    /// optionally requires one. Used only for orchestration tests.
    struct RecordingPass {
        name: &'static str,
        inputs: Vec<&'static str>,
        outputs: Vec<&'static str>,
        log: Rc<RefCell<Vec<&'static str>>>,
    }

    // The Pipeline runner we test here is single-threaded, so `unsafe impl
    // Send/Sync` for an Rc-holding pass is fine in the test context.
    unsafe impl Send for RecordingPass {}
    unsafe impl Sync for RecordingPass {}

    impl Pass for RecordingPass {
        fn name(&self) -> &str {
            self.name
        }
        fn inputs(&self) -> &[&'static str] {
            &self.inputs
        }
        fn outputs(&self) -> &[&'static str] {
            &self.outputs
        }
        fn run(&self, ctx: &mut PassContext) -> Result<()> {
            self.log.borrow_mut().push(self.name);
            for o in &self.outputs {
                ctx.put(o, &serde_json::json!({"by": self.name}))?;
            }
            Ok(())
        }
    }

    #[test]
    fn run_executes_passes_in_declaration_order() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let pipeline = Pipeline::new()
            .add(RecordingPass {
                name: "a",
                inputs: vec![],
                outputs: vec!["a_out"],
                log: log.clone(),
            })
            .add(RecordingPass {
                name: "b",
                inputs: vec!["a_out"],
                outputs: vec!["b_out"],
                log: log.clone(),
            });

        let mut ctx = empty_ctx();
        pipeline.run(&mut ctx).unwrap();
        assert_eq!(*log.borrow(), vec!["a", "b"]);
        assert!(ctx.has("a_out"));
        assert!(ctx.has("b_out"));
    }

    #[test]
    fn run_errors_when_upstream_artifact_missing() {
        let log = Rc::new(RefCell::new(Vec::new()));
        // b declares a dependency on "missing" that nothing produces.
        let pipeline = Pipeline::new().add(RecordingPass {
            name: "b",
            inputs: vec!["missing"],
            outputs: vec![],
            log: log.clone(),
        });
        let mut ctx = empty_ctx();
        let err = pipeline.run(&mut ctx).unwrap_err();
        assert!(err.to_string().contains("missing"));
        assert!(log.borrow().is_empty(), "pass b should not have executed");
    }

    #[test]
    fn run_only_executes_whitelist_subset() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let pipeline = Pipeline::new()
            .add(RecordingPass {
                name: "a",
                inputs: vec![],
                outputs: vec!["a_out"],
                log: log.clone(),
            })
            .add(RecordingPass {
                name: "b",
                inputs: vec![],
                outputs: vec!["b_out"],
                log: log.clone(),
            })
            .add(RecordingPass {
                name: "c",
                inputs: vec![],
                outputs: vec!["c_out"],
                log: log.clone(),
            });

        let mut ctx = empty_ctx();
        pipeline.run_only(&mut ctx, &["a", "c"]).unwrap();
        assert_eq!(*log.borrow(), vec!["a", "c"]);
    }

    #[test]
    fn run_skipping_executes_complement() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let pipeline = Pipeline::new()
            .add(RecordingPass {
                name: "a",
                inputs: vec![],
                outputs: vec!["a_out"],
                log: log.clone(),
            })
            .add(RecordingPass {
                name: "b",
                inputs: vec![],
                outputs: vec!["b_out"],
                log: log.clone(),
            })
            .add(RecordingPass {
                name: "c",
                inputs: vec![],
                outputs: vec!["c_out"],
                log: log.clone(),
            });

        let mut ctx = empty_ctx();
        pipeline.run_skipping(&mut ctx, &["b"]).unwrap();
        assert_eq!(*log.borrow(), vec!["a", "c"]);
    }

    #[test]
    fn run_only_uses_pre_staged_artifact_to_satisfy_inputs() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let pipeline = Pipeline::new()
            .add(RecordingPass {
                name: "producer",
                inputs: vec![],
                outputs: vec!["x"],
                log: log.clone(),
            })
            .add(RecordingPass {
                name: "consumer",
                inputs: vec!["x"],
                outputs: vec![],
                log: log.clone(),
            });

        let mut ctx = empty_ctx();
        // Pre-stage "x" as if loaded from disk; run only the consumer.
        ctx.put("x", &serde_json::json!({"loaded": true})).unwrap();
        pipeline.run_only(&mut ctx, &["consumer"]).unwrap();
        assert_eq!(*log.borrow(), vec!["consumer"]);
    }

    #[test]
    fn names_returns_declaration_order() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let pipeline = Pipeline::new()
            .add(RecordingPass {
                name: "first",
                inputs: vec![],
                outputs: vec![],
                log: log.clone(),
            })
            .add(RecordingPass {
                name: "second",
                inputs: vec![],
                outputs: vec![],
                log: log.clone(),
            });
        assert_eq!(pipeline.names(), vec!["first", "second"]);
    }
}
