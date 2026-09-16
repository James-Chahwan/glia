//! gRPC stack resolver — client → service by service name.

use std::collections::HashMap;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::Edge;

use super::{CrossGraphResolver, ServiceTarget, weakest};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

// ============================================================================
// GrpcStackResolver — matches gRPC client → service by service name
// ============================================================================

pub struct GrpcStackResolver;

impl CrossGraphResolver for GrpcStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let index = build_grpc_service_index(&merged.graphs);
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::GRPC_CLIENT) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(svc_name) = qname.strip_prefix("grpc_client:") else { continue };
                let key = svc_name.split('.').next().unwrap_or(svc_name);
                if let Some(targets) = index.get(key) {
                    for t in targets {
                        merged.cross_edges.push(Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::GRPC_CALLS,
                            confidence: weakest(n.confidence, t.confidence),
                        });
                    }
                }
            }
        }
    }
}

fn build_grpc_service_index(graphs: &[RepoGraph]) -> HashMap<String, Vec<ServiceTarget>> {
    let mut index: HashMap<String, Vec<ServiceTarget>> = HashMap::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::GRPC_SERVICE) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(svc_name) = qname.strip_prefix("grpc:") else { continue };
            let target = ServiceTarget {
                id: n.id,
                confidence: n.confidence,
            };
            index.entry(svc_name.to_string()).or_default().push(target);
            // A5.1: proto service qnames are package-qualified
            // (`grpc:user.UserService`), but a client stub reconstructed from
            // generated code only ever knows the bare last segment
            // (`grpc_client:UserService`). Index both so the pairing survives
            // the qualification.
            if let Some(bare) = svc_name.rsplit('.').next()
                && bare != svc_name
            {
                index.entry(bare.to_string()).or_default().push(target);
            }
        }
    }
    index
}
