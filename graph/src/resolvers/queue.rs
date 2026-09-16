//! Queue stack resolver — producer → consumer by topic name.

use std::collections::HashMap;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Edge, NodeKindId};

use super::{CrossGraphResolver, ServiceTarget, weakest};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

// ============================================================================
// QueueStackResolver — matches producer → consumer by topic name
// ============================================================================

pub struct QueueStackResolver;

impl CrossGraphResolver for QueueStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let consumer_index = build_queue_index(&merged.graphs, node_kind::QUEUE_CONSUMER, "queue_consumer:");
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::QUEUE_PRODUCER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(topic) = qname.strip_prefix("queue_producer:") else { continue };
                if let Some(targets) = consumer_index.get(topic) {
                    for t in targets {
                        merged.cross_edges.push(Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::QUEUE_FLOWS,
                            confidence: weakest(n.confidence, t.confidence),
                        });
                    }
                }
            }
        }
    }
}

fn build_queue_index(
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
            let Some(topic) = qname.strip_prefix(prefix) else { continue };
            index
                .entry(topic.to_string())
                .or_default()
                .push(ServiceTarget {
                    id: n.id,
                    confidence: n.confidence,
                });
        }
    }
    index
}
