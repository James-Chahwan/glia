//! `Pass` trait — single unit of work in the synth pipeline.
//!
//! Each pass declares the named artifacts it reads from `PassContext` and the
//! ones it writes back. The `Pipeline` runner uses these declarations to
//! validate dependency order before invoking `run`.
//!
//! Joern parallel: `io.shiftleft.passes.CpgPass` — a pass takes a context (CPG
//! there, `PassContext` here) and mutates it in place; the pipeline composes
//! passes into a deterministic order.

use anyhow::Result;

use crate::passes::context::PassContext;

pub trait Pass: Send + Sync {
    /// Stable identifier — used for `--only` / `--skip` selection and in logs.
    fn name(&self) -> &str;

    /// Artifact keys this pass reads from `PassContext`. Empty for source
    /// passes that only consume the graph + issue text.
    fn inputs(&self) -> &[&'static str] {
        &[]
    }

    /// Artifact keys this pass writes into `PassContext`. The pipeline runner
    /// uses this to verify that downstream passes' inputs are actually
    /// produced upstream.
    fn outputs(&self) -> &[&'static str] {
        &[]
    }

    /// Mutate `ctx` — read declared inputs, write declared outputs.
    fn run(&self, ctx: &mut PassContext) -> Result<()>;
}
