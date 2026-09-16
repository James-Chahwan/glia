//! Pure-Rust orchestration for glia.
//!
//! Walk a repo → run language parsers + cross-cutting extractors → build per-
//! lang graphs → merge → run cross-graph resolvers. Returns a `MergedGraph`.
//!
//! Two entry points:
//!   - [`generate_one`] for a single repo path (each path's own RepoId).
//!   - [`generate_many`] for N repo paths under a single MergedGraph so
//!     cross-graph resolvers (HTTP, gRPC, DbResolver, etc.) fire across the
//!     boundary. The pyo3 wrapper and the `glia` CLI both call into here.

pub mod cache;

mod answers;
mod build;
mod coverage;
mod docs;
mod extract;
mod passes;
mod route;
mod walk;

pub use repo_graph_graph::MergedGraph as ReExportedMergedGraph;

pub use cache::{CacheStats, ParseCache};

pub use answers::{
    BlastAnswer, LocatedNode, TraceHop, blast_radius_by_qname, cross_stack_trace,
    entrypoint_reachable, governing_docs, locate_node, resolve_signal_located,
};
pub use build::{
    GenerateResult, generate_many, generate_one, generate_one_incremental,
    generate_one_with_cache,
};
pub use coverage::{CoverageCaveat, CoverageNote, coverage_report};
pub use extract::{parse_one, parse_one_with};
