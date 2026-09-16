//! Queue stack resolver — producer → consumer by topic name.

use std::collections::HashMap;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_code_extractors::queues::UNRESOLVED_PREFIX;
use repo_graph_core::{Edge, NodeKindId};

use super::{CrossGraphResolver, ServiceTarget, weakest};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

// ============================================================================
// QueueStackResolver — matches producer → consumer by topic name
// ============================================================================

pub struct QueueStackResolver;

impl CrossGraphResolver for QueueStackResolver {
    /// BREAKING (A2.3): a node whose topic is the framework-tag fallback
    /// (`queue_producer:unresolved:kafka`) is NOT joinable.
    ///
    /// The tag is minted by the extractor when no occurrence of a needle named a
    /// topic, so it carries no identity at all — it only says "this file talks to
    /// Kafka". Joining it like a real topic meant every repo whose topics failed
    /// to parse acquired a QUEUE_FLOWS edge to every OTHER such repo: an
    /// all-to-all false cross-service dependency, which `blast_radius` then
    /// traverses (QUEUE_FLOWS is a carry category) and `cross_stack_trace` labels
    /// as a real mechanism. The node is kept — it is a genuine coverage signal —
    /// but it is now structurally unpairable.
    ///
    /// Both sides are guarded, not just the producer: an unresolved CONSUMER
    /// never enters the index either, so a real producer topic that happens to
    /// spell itself `unresolved:...` cannot reach one.
    fn resolve(&self, merged: &mut MergedGraph) {
        let (consumer_index, mut skipped_unresolved) =
            build_queue_index(&merged.graphs, node_kind::QUEUE_CONSUMER, "queue_consumer:");
        let mut paired = 0usize;
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::QUEUE_PRODUCER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(topic) = qname.strip_prefix("queue_producer:") else { continue };
                if topic.starts_with(UNRESOLVED_PREFIX) {
                    skipped_unresolved += 1;
                    continue;
                }
                if let Some(targets) = consumer_index.get(topic) {
                    for t in targets {
                        merged.cross_edges.push(Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::QUEUE_FLOWS,
                            confidence: weakest(n.confidence, t.confidence),
                        });
                        paired += 1;
                    }
                }
            }
        }
        // One line per BUILD (not per file), and only when this resolver had
        // anything to say — the `[ws-resolve]` house style, so the ~80 fixtures
        // with no queue nodes at all stay silent.
        if paired > 0 || skipped_unresolved > 0 {
            eprintln!("[queues] resolver paired={paired} skipped_unresolved={skipped_unresolved}");
        }
    }
}

/// Topic → consumers, plus the number of unresolved sentinels left out of it.
fn build_queue_index(
    graphs: &[RepoGraph],
    kind: NodeKindId,
    prefix: &str,
) -> (HashMap<String, Vec<ServiceTarget>>, usize) {
    let mut index: HashMap<String, Vec<ServiceTarget>> = HashMap::new();
    let mut skipped = 0usize;
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&kind) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(topic) = qname.strip_prefix(prefix) else { continue };
            if topic.starts_with(UNRESOLVED_PREFIX) {
                skipped += 1;
                continue;
            }
            index
                .entry(topic.to_string())
                .or_default()
                .push(ServiceTarget {
                    id: n.id,
                    confidence: n.confidence,
                });
        }
    }
    (index, skipped)
}
