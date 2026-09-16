//! Event-bus resolver — emitter → handler by event name.

use std::collections::HashMap;

use repo_graph_code_domain::{CodeNav, edge_category, node_kind};
use repo_graph_core::{Edge, NodeId};

use super::{CrossGraphResolver, ServiceTarget, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// EventBusResolver — matches event emitter → handler by event name
// ============================================================================

pub struct EventBusResolver;

/// EVENT_* nodes come from two places: the cross-cutting extractor, whose
/// qnames carry an `event_emit:` / `event_handle:` prefix, and language parsers
/// (Solidity today) whose qnames are ordinary code qnames like
/// `Auction::Auction::BidPlaced`. Key on the prefix when it is there and on the
/// node's simple NAME when it is not, so a Solidity `BidPlaced` can reach a
/// Java `@EventListener(BidPlaced)`. `build_kind_index` is deliberately left
/// alone — GraphQL, WebSocket and CLI all share it and none of them has a
/// parser-side producer of the same kind.
fn event_key(nav: &CodeNav, id: NodeId, qname: &str, prefix: &str) -> Option<String> {
    match qname.strip_prefix(prefix) {
        Some(rest) => Some(rest.to_string()),
        None => nav.name_by_id.get(&id).cloned(),
    }
}

/// Type-named keys fold: `OrderPlacedEvent` and `OrderPlaced` are one event.
/// Applied ONLY to keys that look like a TYPE — a plain identifier starting
/// uppercase — so string topics (`user.created`) and the extractor's tag
/// fallbacks (`emit`, `on`, `@OnEvent`, `Subject.next`) keep matching
/// byte-exactly. There is deliberately NO separator folding: `user.created`
/// must not become `usercreated`, which is the all-to-all shape the queue side
/// just closed.
fn normalise_event_key(raw: &str) -> String {
    let is_type = raw.chars().next().is_some_and(char::is_uppercase)
        && raw.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !is_type {
        return raw.to_string();
    }
    let low = raw.to_lowercase();
    let stripped = low
        .strip_suffix("events")
        .or_else(|| low.strip_suffix("event"));
    match stripped {
        // `Event` alone folds to nothing; keep a stem worth matching on.
        Some(stem) if stem.len() >= 3 => stem.to_string(),
        _ => low,
    }
}

impl CrossGraphResolver for EventBusResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut handler_index: HashMap<String, Vec<ServiceTarget>> = HashMap::new();
        let mut prefixed = 0usize;
        let mut by_name = 0usize;
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::EVENT_HANDLER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                let Some(key) = event_key(&g.nav, n.id, qname, "event_handle:") else {
                    continue;
                };
                if qname.starts_with("event_handle:") {
                    prefixed += 1;
                } else {
                    by_name += 1;
                }
                handler_index
                    .entry(normalise_event_key(&key))
                    .or_default()
                    .push(ServiceTarget {
                        id: n.id,
                        confidence: n.confidence,
                    });
            }
        }

        let mut exact = 0usize;
        let mut folded = 0usize;
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::EVENT_EMITTER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                let Some(raw) = event_key(&g.nav, n.id, qname, "event_emit:") else {
                    continue;
                };
                let key = normalise_event_key(&raw);
                let Some(targets) = handler_index.get(&key) else {
                    continue;
                };
                if key == raw {
                    exact += targets.len();
                } else {
                    folded += targets.len();
                }
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

        // One line per BUILD, and only when this resolver had anything to say —
        // the `[ws-resolve]` house style.
        let pairs = exact + folded;
        if pairs > 0 || by_name > 0 {
            eprintln!(
                "[eventbus] {pairs} pairs (exact={exact} type-folded={folded}); \
                 handlers indexed: prefixed={prefixed} by-name={by_name}"
            );
        }
    }
}
