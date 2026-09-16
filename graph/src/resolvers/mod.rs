//! Cross-graph resolvers — one module per mechanism. Each owns its own
//! matching rule and emits the cross-repo edges for that mechanism; the
//! shared index builder, target type, and confidence helper live here.

use std::collections::HashMap;

use repo_graph_core::{Confidence, NodeId, NodeKindId};

use crate::merged::MergedGraph;
use crate::types::RepoGraph;

mod http;
mod grpc;
mod queue;
mod graphql;
mod websocket;
mod eventbus;
mod shared_schema;
mod db;
mod cron;
mod config;
mod iac;
mod package;
mod cli;

pub use http::HttpStackResolver;
pub use grpc::GrpcStackResolver;
pub use queue::QueueStackResolver;
pub use graphql::GraphQLStackResolver;
pub use websocket::WebSocketStackResolver;
pub use eventbus::EventBusResolver;
pub use shared_schema::SharedSchemaResolver;
pub use db::DbResolver;
pub use cron::CronResolver;
pub use config::ConfigResolver;
pub use iac::IacResolver;
pub use package::PackageResolver;
pub use cli::CliInvocationResolver;
pub use http::normalise_http_path;

/// Emits edges that cross `RepoGraph` boundaries. v0.4.10 will add
/// `GraphQLResolver`, `GrpcResolver`, `QueueResolver`, etc. against the same
/// trait. Each resolver owns its own matching rule — path normalisation,
/// schema-name matching, queue-topic matching, etc.
pub trait CrossGraphResolver {
    fn resolve(&self, merged: &mut MergedGraph);
}

/// Index entry for the name-keyed resolvers (gRPC services, queue topics,
/// GraphQL resolvers, WS handlers, event handlers, CLI commands).
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

fn weakest(a: Confidence, b: Confidence) -> Confidence {
    fn rank(c: Confidence) -> u8 {
        match c {
            Confidence::Strong => 2,
            Confidence::Medium => 1,
            Confidence::Weak => 0,
        }
    }
    if rank(a) <= rank(b) { a } else { b }
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
