//! Event-bus resolver — emitter → handler by event name.

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::Edge;

use super::{CrossGraphResolver, build_kind_index, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// EventBusResolver — matches event emitter → handler by event name
// ============================================================================

pub struct EventBusResolver;

impl CrossGraphResolver for EventBusResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let handler_index = build_kind_index(&merged.graphs, node_kind::EVENT_HANDLER, "event_handle:");
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::EVENT_EMITTER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(event_name) = qname.strip_prefix("event_emit:") else { continue };
                if let Some(targets) = handler_index.get(event_name) {
                    for t in targets {
                        merged.cross_edges.push(Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::EVENT_FLOWS,
                            confidence: weakest(n.confidence, t.confidence),
                        });
                    }
                }
            }
        }
    }
}
