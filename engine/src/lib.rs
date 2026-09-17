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

pub mod arch;
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

pub use arch::{
    ServiceKeying, ServiceLink, ServiceMap, ServiceSummary, default_keying, node_file,
    repo_label_for, repo_label_map, service_map, service_map_with, service_of,
};
pub use cache::{CacheStats, ParseCache};
/// Build identity of THIS binary: `<release>+p<parser stamp>`. Re-exported so
/// `cli` and `py` can report which code they contain without taking a direct
/// dependency on the `stamp` crate.
pub use repo_graph_stamp::{BUILD_STAMP, PARSER_STAMP, RELEASE, VERSION_LINE};

pub use answers::{
    BlastAnswer, LocatedNode, MessageContractRow, MessageContractSide, TraceHop,
    blast_radius_by_qname, cross_stack_trace, entrypoint_reachable, governing_docs, locate_node,
    message_contracts, node_in_scope, resolve_signal_located,
};
pub use build::{
    GenerateResult, generate_many, generate_many_incremental, generate_one,
    generate_one_incremental, generate_one_with_cache,
};
pub use coverage::{CoverageCaveat, CoverageNote, coverage_report};
pub use extract::{parse_one, parse_one_with};
