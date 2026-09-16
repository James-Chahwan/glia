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
        let (mut exact, mut suffix, mut dropped_generic) = (0usize, 0usize, 0usize);
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::WS_CLIENT) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(client_path) = qname.strip_prefix("ws_client:") else { continue };
                let c = ws_segments(client_path);
                for (handler_key, targets) in &handler_index {
                    let h = ws_segments(handler_key);
                    match ws_pair(&c, &h) {
                        WsPair::No => {
                            // A pair the old unconditional wildcard would have
                            // linked: one side carries only an extractor
                            // fallback name, so the substrate does not in fact
                            // know its path. Counted, not edged.
                            if is_generic_ws_path(&c) || is_generic_ws_path(&h) {
                                dropped_generic += targets.len();
                            }
                        }
                        kind => {
                            if kind == WsPair::Exact {
                                exact += targets.len();
                            } else {
                                suffix += targets.len();
                            }
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
        let pairs = exact + suffix;
        if pairs > 0 || dropped_generic > 0 {
            eprintln!(
                "[ws-resolve] {pairs} pairs (exact={exact} suffix={suffix}) \
                 dropped-generic={dropped_generic}"
            );
        }
    }
}

/// How a client path pairs with a handler path. `No` is a dropped pair.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WsPair {
    Exact,
    Suffix,
    No,
}

/// Path segments, lower-cased, empties dropped. `"/ws"` and `"ws"` both reduce
/// to `["ws"]`; a `wss://host/a/b` URL has already been stripped to `/a/b` by
/// the extractor's `normalise_ws_path`.
fn ws_segments(s: &str) -> Vec<String> {
    s.trim_matches('/')
        .to_lowercase()
        .split('/')
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

/// True for a path that is only an extractor fallback name — `ws` when the WS
/// extractor could not read a URL off the source, `default` for
/// `@WebSocketGateway` / `Phoenix.Channel` / `ActionCable`, which carry no path
/// at all. Such a name asserts nothing about where the socket is mounted.
fn is_generic_ws_path(segs: &[String]) -> bool {
    segs.len() == 1 && (segs[0] == "ws" || segs[0] == "default")
}

/// Segment-aware pairing. Equal paths pair exactly; otherwise one path may be a
/// segment-boundary suffix of the other, so a client mounted at `/api/v1/chat`
/// still reaches a handler registered as `/chat` — but `/news` no longer "ends
/// with" a handler named `ws`, and nothing pairs merely because one side's name
/// is a fallback.
fn ws_pair(c: &[String], h: &[String]) -> WsPair {
    if c.is_empty() || h.is_empty() {
        return WsPair::No;
    }
    if c == h {
        return WsPair::Exact;
    }
    let (long, short) = if c.len() >= h.len() { (c, h) } else { (h, c) };
    if long.ends_with(short) {
        WsPair::Suffix
    } else {
        WsPair::No
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(c: &str, h: &str) -> WsPair {
        ws_pair(&ws_segments(c), &ws_segments(h))
    }

    #[test]
    fn exact_match_survives_slash_and_case() {
        assert_eq!(pair("/ws", "ws"), WsPair::Exact);
        assert_eq!(pair("/Chat", "/chat"), WsPair::Exact);
    }

    #[test]
    fn suffix_is_segment_aware_not_byte_wise() {
        assert_eq!(pair("/api/v1/chat", "/chat"), WsPair::Suffix);
        assert_eq!(pair("/news", "ws"), WsPair::No);
        assert_eq!(pair("/notifications", "ws"), WsPair::No);
    }

    #[test]
    fn generic_names_no_longer_wildcard() {
        assert_eq!(pair("/notifications", "default"), WsPair::No);
        assert!(is_generic_ws_path(&ws_segments("ws")));
        assert!(is_generic_ws_path(&ws_segments("/default")));
        assert!(!is_generic_ws_path(&ws_segments("/api/ws")));
    }

    #[test]
    fn empty_path_never_pairs() {
        assert_eq!(pair("", "/chat"), WsPair::No);
        assert_eq!(pair("/chat", "//"), WsPair::No);
    }
}
