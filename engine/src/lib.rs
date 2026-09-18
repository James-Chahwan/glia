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
mod endpoint_fold;
mod extract;
mod passes;
mod route;
mod walk;

// 0.5.0 leap primitives: one public module each, reached by module path
// (`repo_graph_engine::delta::graph_delta_vs_rev`), never flattened into the
// root. Each slot's owner fills its file; no later packet edits this list.
pub mod absence;
pub mod check;
pub mod contract_fields;
pub mod cycles;
pub mod delta;
pub mod diff_impact;
pub mod effects;
pub mod feature_flows;
pub mod find;
pub mod gaps;
pub mod implementors;
pub mod merge;
pub mod pages;
pub mod patterns;
pub mod persist;
pub mod profile;
pub mod serves;
pub mod spec_status;
pub mod tests_for;
pub mod trace;
pub mod why;

// 0.5.0 leap internals: crate-private slots, cross-module items `pub(crate)`.
mod adr;
mod external;
mod git_rev;
mod http_owner;
mod parallel;
mod rekey;

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

// Facade: every `pub` item in these four modules is the crate's flat public
// surface. A helper another module needs is `pub(crate)`, never `pub`. `arch`
// and `cache` are public modules with a partial flat list (kept explicit);
// `docs`, `endpoint_fold`, `passes`, `route` and `walk` declare no free `pub`
// items, so they are not globbed.
pub use answers::*;
pub use build::*;
pub use coverage::*;
pub use extract::*;
