//! T5 — pass-composition framework for the synth pipeline.
//!
//! Joern's `CpgPass` model adapted to glia: the pipeline is
//! `Pipeline { passes: Vec<Box<dyn Pass>> }`, each pass has a stable name and
//! declared input/output artifact keys, and they share state through
//! `PassContext` instead of round-tripping JSON to disk.
//!
//! Status: scaffolding + one exemplar (`NodeSummariesPass`); no bin uses it.
//! The standalone `bin/synth_*.rs` bins are research drivers behind the
//! `research` feature and remain the live pipeline. For synth composition
//! this scaffold is superseded: since LD.12b the access-path and
//! callsite-argflow passes run as `activation::plan::SynthHook`s in one
//! `ActivationPlan` over one `ActivatedView` (`crate::hooks`:
//! `AccessPathSynth`, `CallsiteArgflowSynth`), with bin parity pinned by
//! `tests/synth_hooks.rs`. `Pipeline` / `Pass` / `PassContext` are left as
//! they are, for James to keep (as the artifact-keyed file pipeline) or
//! delete.

pub mod context;
pub mod node_summaries;
pub mod pipeline;
pub mod traits;

pub use context::PassContext;
pub use node_summaries::{NodeSummariesPass, SummaryEntry};
pub use pipeline::Pipeline;
pub use traits::Pass;
