//! Build-pass composition (LD.13): a domain declares the passes that turn its
//! assembled graph into the stored one as a const [`PassRegistry`], and one
//! runner executes them in a fixed, checkable order.
//!
//! Domain-free on purpose: the registry is generic over the domain's graph
//! type `G` and its build context `C`, so the code domain (the engine's
//! `CODE_PASSES`, over `MergedGraph`) and any later domain share the runner
//! without this crate knowing either graph.
//!
//! The order is data, never solved:
//! - a pass belongs to a [`Stage`], and every `Resolve` pass runs before every
//!   `Post` pass, which runs before every `Finalize` pass (canonicalisation:
//!   the determinism sort). A pass appended later can therefore never run
//!   after the sort by accident;
//! - within a stage, passes run in declaration order;
//! - `after` names the passes a pass depends on. [`PassRegistry::validate`]
//!   checks each one exists and runs earlier; it never reorders anything, so
//!   a mis-declared list fails the domain's test instead of silently moving a
//!   pass.
//!
//! A pass is a plain `fn` pointer over `(&mut G, &C)`: passes hold no state,
//! the registry is a `&'static` slice (no allocation, no hash order) and a
//! domain declares it in a `const`.

use std::fmt;

use glia_core::CellTypeId;

/// When a pass runs. Stages run in declaration order (`Resolve` < `Post` <
/// `Finalize`), which is also their `Ord`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// Cross-graph resolution: pairs nodes across the assembled graphs.
    Resolve,
    /// Passes over the resolved graph: demotions, derived edges, tagging.
    Post,
    /// Canonicalisation, after every pass that can add or change anything:
    /// the steps that make the stored bytes deterministic.
    Finalize,
}

impl Stage {
    /// Lowercase name, as the stage counters of a pass marker print it.
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Resolve => "resolve",
            Stage::Post => "post",
            Stage::Finalize => "finalize",
        }
    }
}

/// One build pass of a domain.
pub struct PassSpec<G: 'static, C: 'static = ()> {
    /// Unique, non-empty within its registry.
    pub name: &'static str,
    pub stage: Stage,
    /// Passes that must run before this one. Checked by
    /// [`PassRegistry::validate`], never used to reorder.
    pub after: &'static [&'static str],
    /// The node cell types this pass writes, and only those. The domain's
    /// test proves the list both ways over its fixtures (a pass writes no
    /// undeclared type; every declared type is observed).
    pub populates: &'static [CellTypeId],
    /// The pass itself.
    pub run: fn(&mut G, &C),
}

// Manual impls: a derive would demand `G: Clone` / `C: Clone`, and the spec
// holds only references and a fn pointer.
impl<G: 'static, C: 'static> Clone for PassSpec<G, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<G: 'static, C: 'static> Copy for PassSpec<G, C> {}

impl<G: 'static, C: 'static> fmt::Debug for PassSpec<G, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PassSpec")
            .field("name", &self.name)
            .field("stage", &self.stage)
            .field("after", &self.after)
            .field("populates", &self.populates)
            .finish_non_exhaustive()
    }
}

/// A domain's build passes, in declaration order.
pub struct PassRegistry<G: 'static, C: 'static = ()> {
    specs: &'static [PassSpec<G, C>],
}

impl<G: 'static, C: 'static> Clone for PassRegistry<G, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<G: 'static, C: 'static> Copy for PassRegistry<G, C> {}

impl<G: 'static, C: 'static> fmt::Debug for PassRegistry<G, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PassRegistry").field("specs", &self.specs).finish()
    }
}

impl<G: 'static, C: 'static> PassRegistry<G, C> {
    pub const fn new(specs: &'static [PassSpec<G, C>]) -> Self {
        Self { specs }
    }

    /// A registry with no passes: a domain whose assembled graph is already
    /// its stored graph.
    pub const fn empty() -> Self {
        Self { specs: &[] }
    }

    /// The specs as declared.
    pub fn specs(&self) -> &'static [PassSpec<G, C>] {
        self.specs
    }

    /// The run order: by stage, then declaration order within a stage (a
    /// stable sort).
    pub fn order(&self) -> Vec<&'static PassSpec<G, C>> {
        let mut order: Vec<&'static PassSpec<G, C>> = self.specs.iter().collect();
        order.sort_by_key(|s| s.stage);
        order
    }

    /// Every problem with the declaration, or `Ok`: names are non-empty and
    /// unique, and every `after` name exists and runs before the pass naming
    /// it in [`Self::order`].
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let order = self.order();
        let mut errors = Vec::new();
        for (i, spec) in order.iter().enumerate() {
            if spec.name.is_empty() {
                errors.push(format!("pass #{i} ({:?} stage) has an empty name", spec.stage));
            }
            if order[..i].iter().any(|s| s.name == spec.name) {
                errors.push(format!("pass name {:?} is declared more than once", spec.name));
            }
            for dep in spec.after {
                match order.iter().position(|s| s.name == *dep) {
                    None => errors.push(format!(
                        "pass {:?} runs after {dep:?}, which is not registered",
                        spec.name
                    )),
                    Some(j) if j == i => {
                        errors.push(format!("pass {:?} names itself in `after`", spec.name))
                    }
                    Some(j) if j > i => errors.push(format!(
                        "pass {:?} runs after {dep:?}, which runs later ({:?} stage)",
                        spec.name, order[j].stage
                    )),
                    Some(_) => {}
                }
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    /// Run every pass over `g` in [`Self::order`], handing each the build
    /// context `ctx`. Prints nothing: the caller owns the marker.
    pub fn run(&self, g: &mut G, ctx: &C) -> PassReport {
        let mut report = PassReport::default();
        for spec in self.order() {
            (spec.run)(g, ctx);
            report.ran.push(spec.name);
            match spec.stage {
                Stage::Resolve => report.resolve += 1,
                Stage::Post => report.post += 1,
                Stage::Finalize => report.finalize += 1,
            }
        }
        report
    }
}

/// What [`PassRegistry::run`] ran: the pass names in run order and the count
/// per stage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PassReport {
    pub ran: Vec<&'static str>,
    pub resolve: usize,
    pub post: usize,
    pub finalize: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toy graph: the names of the passes that touched it, in order.
    type Log = Vec<&'static str>;

    const fn spec(
        name: &'static str,
        stage: Stage,
        after: &'static [&'static str],
    ) -> PassSpec<Log> {
        PassSpec { name, stage, after, populates: &[], run: |_, _| {} }
    }

    fn names<G, C>(r: &PassRegistry<G, C>) -> Vec<&'static str> {
        r.order().iter().map(|s| s.name).collect()
    }

    #[test]
    fn order_is_stage_then_declaration() {
        const R: PassRegistry<Log> = PassRegistry::new(&[
            spec("sort", Stage::Finalize, &[]),
            spec("tag", Stage::Post, &[]),
            spec("pair_a", Stage::Resolve, &[]),
            spec("demote", Stage::Post, &[]),
            spec("pair_b", Stage::Resolve, &[]),
        ]);
        assert_eq!(names(&R), ["pair_a", "pair_b", "tag", "demote", "sort"]);
        assert_eq!(R.validate(), Ok(()));
        assert!(names(&PassRegistry::<Log>::empty()).is_empty());
        assert_eq!(PassRegistry::<Log>::empty().validate(), Ok(()));
    }

    #[test]
    fn validate_rejects_duplicate_names() {
        const R: PassRegistry<Log> = PassRegistry::new(&[
            spec("pair", Stage::Resolve, &[]),
            spec("pair", Stage::Post, &[]),
            spec("", Stage::Post, &[]),
        ]);
        let errors = R.validate().unwrap_err();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("\"pair\" is declared more than once"), "{errors:?}");
        assert!(errors[1].contains("empty name"), "{errors:?}");
    }

    #[test]
    fn validate_rejects_unknown_after() {
        const R: PassRegistry<Log> = PassRegistry::new(&[
            spec("pair", Stage::Resolve, &[]),
            spec("demote", Stage::Post, &["pair", "downgrade"]),
        ]);
        let errors = R.validate().unwrap_err();
        assert_eq!(
            errors,
            ["pass \"demote\" runs after \"downgrade\", which is not registered"]
        );
    }

    #[test]
    fn validate_rejects_after_that_runs_later() {
        // Declared first, but a Finalize pass runs after every Post pass.
        const R: PassRegistry<Log> = PassRegistry::new(&[
            spec("sort", Stage::Finalize, &[]),
            spec("tag", Stage::Post, &["sort"]),
            spec("self_dep", Stage::Post, &["self_dep"]),
        ]);
        let errors = R.validate().unwrap_err();
        assert_eq!(
            errors,
            [
                "pass \"tag\" runs after \"sort\", which runs later (Finalize stage)",
                "pass \"self_dep\" names itself in `after`",
            ]
        );
    }

    #[test]
    fn run_reports_stage_counts_and_threads_ctx() {
        // G = what the passes recorded; C = what they read.
        type Ctx = Vec<&'static str>;
        const R: PassRegistry<Log, Ctx> = PassRegistry::new(&[
            PassSpec {
                name: "finalize",
                stage: Stage::Finalize,
                after: &["copy_first"],
                populates: &[],
                run: |g, _| g.sort_unstable(),
            },
            PassSpec {
                name: "copy_first",
                stage: Stage::Resolve,
                after: &[],
                populates: &[],
                run: |g, c| g.extend(c.first()),
            },
            PassSpec {
                name: "copy_rest",
                stage: Stage::Post,
                after: &["copy_first"],
                populates: &[CellTypeId(7)],
                run: |g, c| g.extend(c.iter().skip(1)),
            },
        ]);
        assert_eq!(R.validate(), Ok(()));
        let ctx: Ctx = vec!["zeta", "alpha", "mid"];
        let mut g: Log = Vec::new();
        let report = R.run(&mut g, &ctx);
        assert_eq!(g, ["alpha", "mid", "zeta"], "every pass read the ctx; the finalize ran last");
        assert_eq!(report.ran, ["copy_first", "copy_rest", "finalize"]);
        assert_eq!((report.resolve, report.post, report.finalize), (1, 1, 1));
        assert_eq!(R.specs()[2].populates, [CellTypeId(7)]);
        assert_eq!(Stage::Finalize.as_str(), "finalize");
    }
}
