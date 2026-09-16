//! WebSocket stack resolver — WS client → handler by path.

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::Edge;

use super::{CrossGraphResolver, build_kind_index, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// WebSocketStackResolver — matches WS client → handler by path
// ============================================================================

pub struct WebSocketStackResolver;

impl CrossGraphResolver for WebSocketStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let handler_index = build_kind_index(&merged.graphs, node_kind::WS_HANDLER, "ws:");
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::WS_CLIENT) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(client_path) = qname.strip_prefix("ws_client:") else { continue };
                for (handler_key, targets) in &handler_index {
                    if ws_paths_match(client_path, handler_key) {
                        for t in targets {
                            merged.cross_edges.push(Edge {
                                from: n.id,
                                to: t.id,
                                category: edge_category::WS_CONNECTS,
                                confidence: weakest(n.confidence, t.confidence),
                            });
                        }
                    }
                }
            }
        }
    }
}

fn ws_paths_match(client: &str, handler: &str) -> bool {
    let norm_c = client.trim_matches('/').to_lowercase();
    let norm_h = handler.trim_matches('/').to_lowercase();
    norm_c == norm_h
        || norm_c.ends_with(&norm_h)
        || norm_h.ends_with(&norm_c)
        || (norm_c == "ws" || norm_h == "ws" || norm_h == "default")
}
