//! Cross-graph resolvers — one module per mechanism. Each owns its own
//! matching rule and emits the cross-repo edges for that mechanism; the
//! shared index builder, target type, confidence helper and cross-repo pair
//! emitter live here.

use std::collections::HashMap;

use repo_graph_core::{Confidence, Edge, EdgeCategoryId, NodeId, NodeKindId, RepoId};

use crate::merged::MergedGraph;
use crate::types::RepoGraph;

mod http;
mod grpc;
mod rpc;
mod queue;
mod graphql;
mod websocket;
mod eventbus;
mod shared_schema;
mod db;
mod message_schema;
mod cron;
mod config;
mod iac;
mod package;
mod cli;

pub use http::HttpStackResolver;
pub use grpc::GrpcStackResolver;
pub use rpc::RpcStackResolver;
pub use queue::QueueStackResolver;
pub use graphql::GraphQLStackResolver;
pub use websocket::WebSocketStackResolver;
pub use eventbus::EventBusResolver;
pub use shared_schema::SharedSchemaResolver;
pub use db::DbResolver;
pub use message_schema::MessageSchemaResolver;
pub use cron::CronResolver;
pub use config::ConfigResolver;
pub use iac::IacResolver;
pub use package::PackageResolver;
pub use cli::CliInvocationResolver;
pub use http::normalise_http_path;
// A10.2 — the route index + tiers 1-4 for passes that pair a DECLARED path
// (a contract operation) with the ROUTE serving it.
pub use http::{HttpRouteMatcher, RouteMatch};
// A9.1 — `cross_links` reads the HTTP channel label off the same parse the
// resolver uses; crate-internal only, the public surface is the facade.
pub(crate) use http::parse_endpoint_qname;

/// Emits edges that cross `RepoGraph` boundaries. v0.4.10 will add
/// `GraphQLResolver`, `GrpcResolver`, `QueueResolver`, etc. against the same
/// trait. Each resolver owns its own matching rule — path normalisation,
/// schema-name matching, queue-topic matching, etc.
pub trait CrossGraphResolver {
    fn resolve(&self, merged: &mut MergedGraph);
}

/// Index entry for the name-keyed resolvers (gRPC services, queue topics,
/// GraphQL resolvers, WS handlers, event handlers, CLI commands).
#[derive(Clone, Copy)]
struct ServiceTarget {
    id: NodeId,
    confidence: Confidence,
}

// ============================================================================
// Shared index builder
// ============================================================================

fn build_kind_index(
    graphs: &[RepoGraph],
    kind: NodeKindId,
    prefix: &str,
) -> HashMap<String, Vec<ServiceTarget>> {
    let mut index: HashMap<String, Vec<ServiceTarget>> = HashMap::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&kind) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(key) = qname.strip_prefix(prefix) else { continue };
            index
                .entry(key.to_string())
                .or_default()
                .push(ServiceTarget {
                    id: n.id,
                    confidence: n.confidence,
                });
        }
    }
    index
}

pub(crate) fn weakest(a: Confidence, b: Confidence) -> Confidence {
    fn rank(c: Confidence) -> u8 {
        match c {
            Confidence::Strong => 2,
            Confidence::Medium => 1,
            Confidence::Weak => 0,
        }
    }
    if rank(a) <= rank(b) { a } else { b }
}

/// Emit one edge per cross-repo pair in `refs`, returning how many were added.
/// Same-repo pairs are skipped: a repo's own duplicates are not a cross-service
/// join. `confidence: None` means `weakest(a, b)`; `Some(c)` forces `c` (the
/// DB provider pass forces `Weak` — see [`DbResolver::resolve`]). Shared by
/// the exact-qname pairwise resolvers (`DbResolver`, `MessageSchemaResolver`).
fn emit_cross_repo_pairs(
    refs: &[(NodeId, RepoId, Confidence)],
    category: EdgeCategoryId,
    confidence: Option<Confidence>,
    out: &mut Vec<Edge>,
) -> usize {
    let mut emitted = 0;
    for i in 0..refs.len() {
        for j in (i + 1)..refs.len() {
            if refs[i].1 == refs[j].1 {
                continue;
            }
            out.push(Edge {
                from: refs[i].0,
                to: refs[j].0,
                category,
                confidence: confidence.unwrap_or_else(|| weakest(refs[i].2, refs[j].2)),
            });
            emitted += 1;
        }
    }
    emitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_core::Confidence;

    #[test]
    fn weakest_confidence_is_min_rank() {
        assert_eq!(weakest(Confidence::Strong, Confidence::Strong), Confidence::Strong);
        assert_eq!(weakest(Confidence::Strong, Confidence::Medium), Confidence::Medium);
        assert_eq!(weakest(Confidence::Medium, Confidence::Weak), Confidence::Weak);
        assert_eq!(weakest(Confidence::Weak, Confidence::Strong), Confidence::Weak);
    }
}
