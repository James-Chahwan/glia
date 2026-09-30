//! glia-graph — per-repo graph construction + resolver + traversal.
//!
//! Consumes `FileParse` outputs from the language parsers, merges them into a
//! single `RepoGraph` for the repo, resolves cross-file imports and calls
//! using a symbol table, and exposes BFS / neighbours / parent-chain.
//!
//! One entry point per parser — `build_python`, `build_go`, `build_typescript`
//! — because their import semantics differ (dotted qnames vs. stripped go.mod
//! paths vs. relative/bare module sources). The symbol-table + traversal
//! infrastructure is shared.
//!
//! v0.4.4b adds `MergedGraph` + `CrossGraphResolver` for cross-repo resolution.
//! The first resolver, `HttpStackResolver`, pairs frontend Endpoints with
//! backend Routes by (method, normalised path) and emits `HTTP_CALLS` edges.
//! Other stack resolvers (GraphQL, gRPC, queues, shared-schema) land at v0.4.10
//! against the same trait.

mod blast;
mod build;
mod calls;
mod imports;
mod merged;
mod resolvers;
mod signal;
mod traversal;
mod types;

// 0.5.0 leap primitives: one public module each, reached by module path
// (`glia_graph::roles::roles_in`), never flattened into the root.
pub mod cells;
pub mod identity;
pub mod nav;
pub mod roles;
pub mod rust_paths;

// 0.5.1 internals: crate-private slots, items pub(crate).
mod cpp_scope;
mod go_mounts;
mod swift_scope;

#[cfg(test)]
mod test_support;

// Facade: each module's `pub` items are the crate's public surface. A helper
// that must stay crate-internal is `pub(crate)`, never `pub`. `calls`,
// `imports`, `signal` and `traversal` declare no free `pub` items (their public
// methods hang off `RepoGraph` / `MergedGraph`), so they are not globbed.
pub use blast::*;
pub use build::*;
pub use merged::*;
pub use resolvers::*;
pub use types::*;
