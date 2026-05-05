//! T5 — pass-composition framework for the synth pipeline.
//!
//! Joern's `CpgPass` model adapted to glia: the pipeline is
//! `Pipeline { passes: Vec<Box<dyn Pass>> }`, each pass has a stable name and
//! declared input/output artifact keys, and they share state through
//! `PassContext` instead of round-tripping JSON to disk.
//!
//! Status: scaffolding + one exemplar (`NodeSummariesPass`). The 9 standalone
//! `bin/synth_*.rs` driver binaries remain the live pipeline; passes are
//! ported one at a time. No changes to existing bins until each port has
//! parity tests.

pub mod context;
pub mod node_summaries;
pub mod pipeline;
pub mod traits;

pub use context::PassContext;
pub use node_summaries::{NodeSummariesPass, SummaryEntry};
pub use pipeline::Pipeline;
pub use traits::Pass;
