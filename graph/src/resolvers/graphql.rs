//! GraphQL stack resolver — operation → resolver by name.

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::Edge;

use super::{CrossGraphResolver, build_kind_index, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// GraphQLStackResolver — matches operation → resolver by name
// ============================================================================

pub struct GraphQLStackResolver;

impl CrossGraphResolver for GraphQLStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let resolver_index = build_kind_index(&merged.graphs, node_kind::GRAPHQL_RESOLVER, "graphql_resolver:");
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::GRAPHQL_OPERATION) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(op_name) = qname.strip_prefix("graphql_op:") else { continue };
                for (resolver_key, targets) in &resolver_index {
                    if names_match_graphql(op_name, resolver_key) {
                        for t in targets {
                            merged.cross_edges.push(Edge {
                                from: n.id,
                                to: t.id,
                                category: edge_category::GRAPHQL_CALLS,
                                confidence: weakest(n.confidence, t.confidence),
                            });
                        }
                    }
                }
            }
        }
    }
}

fn names_match_graphql(operation: &str, resolver: &str) -> bool {
    let op_lower = operation.to_lowercase();
    let res_lower = resolver.to_lowercase();
    op_lower == res_lower
        || op_lower.contains(&res_lower)
        || res_lower.contains(&op_lower)
}
