//! repo-graph-graph — per-repo graph construction + resolver + traversal.
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

mod activation;
mod blast;
mod build;
mod calls;
mod imports;
mod merged;
mod resolvers;
mod signal;
mod traversal;
mod types;

#[cfg(test)]
mod test_support;

pub use activation::{code_activation_defaults, code_activation_profile};
pub use blast::{BlastHit, Reach, blast_carry_edges};
pub use build::{build_dotted, build_go, build_python, build_ruby, build_typescript};
pub use merged::{CrossLink, MergedGraph, channel_of, cluster_key_for, cross_links};
pub use resolvers::{
    CliInvocationResolver, ConfigResolver, CronResolver, CrossGraphResolver, DbResolver,
    EventBusResolver, GraphQLStackResolver, GrpcStackResolver, HttpStackResolver, IacResolver,
    PackageResolver, QueueStackResolver, RpcStackResolver, SharedSchemaResolver,
    WebSocketStackResolver, normalise_http_path,
};
// A10.2 — the HTTP route index + match ladder, for passes that pair a declared
// path (a contract operation) with the ROUTE that serves it.
pub use resolvers::{HttpRouteMatcher, RouteMatch};
pub use types::{GraphError, RepoGraph, SymbolTable};
