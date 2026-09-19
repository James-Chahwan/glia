//! WebSocket stack resolver — WS client → handler by path.
//!
//! LA.18b (programme A5.9, resolver half) and LA.18c. Four rules on top of the
//! segment-aware Exact / Suffix pairing:
//!
//! - **Generic is a raw-key property.** The WS extractor names a site it could
//!   not read a path off `ws` (`default` for the path-less NestJS row). A real
//!   path always starts with `/`, so the generic test runs on the raw key
//!   BEFORE segmenting: `/ws` is a client path, `ws` is a fallback name. They
//!   used to reduce to the same segment list and pair Exact by coincidence.
//! - **A generic handler pairs only through the routes that reach it.** Which
//!   route serves an upgrade site is a cross-file join (the gorilla chat
//!   layout registers `/ws` in main.go and upgrades in client.go), so it lives
//!   here, not in the extractor: the upgrade site is HANDLED_BY its owning
//!   function F (LA.18a), and every ROUTE HANDLED_BY F — or HANDLED_BY a
//!   function that CALLS F, one hop — lends F's generic handler its path. A
//!   generic handler no route reaches, and every generic client, pairs
//!   nothing.
//! - **Parameter segments.** `{x}`, `:x` and `<x>` on the handler side (`{…}`
//!   on the client side) match exactly one non-empty segment, and a path
//!   holding any parameter pairs only at equal length — never through the
//!   suffix tier — so `/chat/lobby/feed` never reaches `/chat/{room}`.
//! - **Topic wildcards (LA.18c).** Channel-keyed frameworks name the socket by
//!   a channel or topic, not a path (`ws:ChatChannel`, `ws:room:*`), and pair
//!   through the same segment logic. A handler whose last segment ends in `*`
//!   (Phoenix's `channel "room:*"`) matches a client with the same number of
//!   segments, equal leading segments and a last segment starting with the
//!   pattern minus its `*` — `room:*` reaches `room:lobby`, never
//!   `lobby:room`. A bare `*` catch-all asserts nothing and never pairs.
//!
//! fired_on marker, once per build that pairs or drops anything:
//!   `[ws-resolve] 2 pairs (exact=1 suffix=0 param=0 inherited=0 wildcard=1) dropped-generic=0`
//! A pair with a generic handler counts `inherited` whatever tier its
//! inherited path matched at. `dropped-generic` counts (client, handler)
//! combinations that a generic side was part of and that did not pair.

use std::collections::{BTreeSet, HashMap, HashSet};

use repo_graph_code_domain::endpoint::split_owner;
use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, NodeId};

use super::http::route_path;
use super::{CrossGraphResolver, weakest};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

// ============================================================================
// WebSocketStackResolver — matches WS client → handler by path
// ============================================================================

pub struct WebSocketStackResolver;

impl CrossGraphResolver for WebSocketStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let (edges, stats) = pair_all(&merged.graphs);
        merged.cross_edges.extend(edges);
        let pairs = stats.pairs();
        if pairs > 0 || stats.dropped_generic > 0 {
            eprintln!(
                "[ws-resolve] {pairs} pairs (exact={} suffix={} param={} inherited={} \
                 wildcard={}) dropped-generic={}",
                stats.exact,
                stats.suffix,
                stats.param,
                stats.inherited,
                stats.wildcard,
                stats.dropped_generic
            );
        }
    }
}

/// What one resolve run paired, per tier, and what it dropped.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct WsStats {
    exact: usize,
    suffix: usize,
    param: usize,
    inherited: usize,
    wildcard: usize,
    dropped_generic: usize,
}

impl WsStats {
    fn pairs(&self) -> usize {
        self.exact + self.suffix + self.param + self.inherited + self.wildcard
    }
}

/// One WS_HANDLER node, with the path(s) clients are paired against.
struct Handler {
    id: NodeId,
    confidence: Confidence,
    /// The key was a fallback name: `paths` are the inherited route paths
    /// (possibly none), never the name itself. A bare `*` catch-all topic is
    /// generic with no paths at all: it has no mount to inherit.
    generic: bool,
    /// Segmented paths, in [`BTreeSet`] order of their raw text for a generic
    /// handler; exactly one for a handler that names its own path.
    paths: Vec<Vec<String>>,
}

/// Every WS_CONNECTS edge of the build, in client-node order then
/// handler-node order (graph order both), each (client, handler) once.
fn pair_all(graphs: &[RepoGraph]) -> (Vec<Edge>, WsStats) {
    let mut stats = WsStats::default();
    let handlers = collect_handlers(graphs);
    let mut edges = Vec::new();
    if handlers.is_empty() {
        return (edges, stats);
    }
    let mut seen_clients: HashSet<NodeId> = HashSet::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::WS_CLIENT)
                || !seen_clients.insert(n.id)
            {
                continue;
            }
            // LB.8: the owner segment names the connecting project, not the
            // path; a client pairs every same-path handler, whoever owns it.
            let Some(key) = g
                .nav
                .qname_by_id
                .get(&n.id)
                .and_then(|q| split_owner(q).0.strip_prefix("ws_client:"))
            else {
                continue;
            };
            if is_generic_ws_key(key) {
                // The client's URL was unreadable: it asserts no path, so it
                // pairs nothing — not even another fallback name.
                stats.dropped_generic += handlers.len();
                continue;
            }
            let c = ws_segments(key);
            for h in &handlers {
                let Some(tier) = h
                    .paths
                    .iter()
                    .map(|p| ws_pair(&c, p))
                    .find(|t| *t != WsPair::No)
                else {
                    if h.generic {
                        stats.dropped_generic += 1;
                    }
                    continue;
                };
                match (h.generic, tier) {
                    (true, _) => stats.inherited += 1,
                    (false, WsPair::Exact) => stats.exact += 1,
                    (false, WsPair::Suffix) => stats.suffix += 1,
                    (false, WsPair::Param) => stats.param += 1,
                    (false, WsPair::Wildcard) => stats.wildcard += 1,
                    (false, WsPair::No) => continue,
                }
                edges.push(Edge {
                    from: n.id,
                    to: h.id,
                    category: edge_category::WS_CONNECTS,
                    confidence: weakest(n.confidence, h.confidence),
                });
            }
        }
    }
    (edges, stats)
}

/// Every WS_HANDLER node once, in graph order. A handler that names its own
/// path pairs by it; a generic one gets its inherited route paths (the join
/// is only built when a generic handler exists).
fn collect_handlers(graphs: &[RepoGraph]) -> Vec<Handler> {
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut found: Vec<(NodeId, Confidence, &str)> = Vec::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::WS_HANDLER) || !seen.insert(n.id) {
                continue;
            }
            if let Some(key) = g
                .nav
                .qname_by_id
                .get(&n.id)
                .and_then(|q| split_owner(q).0.strip_prefix("ws:"))
            {
                found.push((n.id, n.confidence, key));
            }
        }
    }
    let inherits = |key: &str| is_generic_ws_key(key) && !is_catch_all_key(key);
    let reach = found
        .iter()
        .any(|(_, _, key)| inherits(key))
        .then(|| RouteReach::new(graphs));
    found
        .into_iter()
        .map(|(id, confidence, key)| match &reach {
            _ if is_catch_all_key(key) => Handler {
                id,
                confidence,
                generic: true,
                paths: Vec::new(),
            },
            Some(reach) if inherits(key) => Handler {
                id,
                confidence,
                generic: true,
                paths: reach.paths_of(id).iter().map(|p| ws_segments(p)).collect(),
            },
            _ => Handler {
                id,
                confidence,
                generic: false,
                paths: vec![ws_segments(key)],
            },
        })
        .collect()
}

/// The route join for generic handlers, built from each graph's own edges
/// (a function node lives in one graph, so its HANDLED_BY and CALLS edges do
/// too). Only looked up, never iterated: output order comes from graph order
/// and the [`BTreeSet`] in [`RouteReach::paths_of`].
struct RouteReach {
    /// WS_HANDLER -> the functions its upgrade sites sit in (LA.18a anchors).
    ws_owner: HashMap<NodeId, Vec<NodeId>>,
    /// Function -> the raw paths of the ROUTEs HANDLED_BY it.
    route_paths: HashMap<NodeId, Vec<String>>,
    /// Upgrading function -> its direct callers (CALLS, one hop).
    callers: HashMap<NodeId, Vec<NodeId>>,
}

impl RouteReach {
    fn new(graphs: &[RepoGraph]) -> Self {
        let mut ws_owner: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut route_paths: HashMap<NodeId, Vec<String>> = HashMap::new();
        for g in graphs {
            for e in &g.edges {
                if e.category != edge_category::HANDLED_BY {
                    continue;
                }
                match g.nav.kind_by_id.get(&e.from) {
                    Some(&k) if k == node_kind::WS_HANDLER => {
                        ws_owner.entry(e.from).or_default().push(e.to);
                    }
                    Some(&k) if k == node_kind::ROUTE => {
                        if let Some(path) =
                            g.nav.qname_by_id.get(&e.from).and_then(|q| route_path(q))
                        {
                            route_paths.entry(e.to).or_default().push(path.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        let owners: HashSet<NodeId> = ws_owner.values().flatten().copied().collect();
        let mut callers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for g in graphs {
            for e in &g.edges {
                if e.category == edge_category::CALLS && owners.contains(&e.to) && e.from != e.to {
                    callers.entry(e.to).or_default().push(e.from);
                }
            }
        }
        Self {
            ws_owner,
            route_paths,
            callers,
        }
    }

    /// The effective paths of generic handler `ws`: for every function F an
    /// upgrade site of it sits in, the paths of the routes HANDLED_BY F and of
    /// the routes HANDLED_BY a direct caller of F. Deduplicated and ordered.
    fn paths_of(&self, ws: NodeId) -> BTreeSet<&str> {
        let mut out = BTreeSet::new();
        let paths = |f: &NodeId| {
            self.route_paths
                .get(f)
                .into_iter()
                .flatten()
                .map(String::as_str)
        };
        for f in self.ws_owner.get(&ws).into_iter().flatten() {
            out.extend(paths(f));
            for g in self.callers.get(f).into_iter().flatten() {
                out.extend(paths(g));
            }
        }
        out
    }
}

/// How a client path pairs with a handler path. `No` is a dropped pair.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WsPair {
    Exact,
    Suffix,
    /// Equal length, every segment equal or a parameter on either side.
    Param,
    /// A trailing-`*` topic pattern and a concrete topic it covers (LA.18c).
    Wildcard,
    No,
}

/// Path segments, lower-cased, empties dropped. `"/ws"` and `"ws"` both reduce
/// to `["ws"]` — which is why the generic test ([`is_generic_ws_key`]) runs on
/// the raw key, before this. A `wss://host/a/b` URL has already been stripped
/// to `/a/b` by the extractor's `normalise_ws_path`.
fn ws_segments(s: &str) -> Vec<String> {
    s.trim_matches('/')
        .to_lowercase()
        .split('/')
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

/// True for a raw WS key that is only an extractor fallback name — `ws` when
/// the WS extractor could not read a URL off the source, `default` for
/// `@WebSocketGateway`, which carries no path at all — or a bare `*` topic
/// catch-all ([`is_catch_all_key`]). Such a name asserts nothing about where
/// the socket is mounted. A key with a leading `/` (`/ws`, `/default`) is a
/// path that was read.
fn is_generic_ws_key(key: &str) -> bool {
    key == "ws" || key == "default" || is_catch_all_key(key)
}

/// Phoenix's `channel "*", CatchAllChannel`: every topic, so no join key.
/// Generic, but unlike a fallback name it inherits no route paths either — a
/// topic pattern is not an upgrade site a route reaches.
fn is_catch_all_key(key: &str) -> bool {
    key == "*"
}

/// The stem of a trailing-`*` topic pattern (`room:*` -> `room:`): the last
/// segment of `h` minus its `*`, when that segment ends in `*` and is more
/// than a bare `*`.
fn topic_stem(h: &[String]) -> Option<&str> {
    h.last()?.strip_suffix('*').filter(|stem| !stem.is_empty())
}

/// A handler-side parameter segment: `{room}` (JSR-356, Spring, ASP.NET, Go
/// 1.22), `:room` (gin, Express), `<room>` / `<int:room>` (Flask, Django).
fn is_handler_param(seg: &str) -> bool {
    (seg.len() >= 2 && seg.starts_with('{') && seg.ends_with('}'))
        || (seg.len() >= 2 && seg.starts_with(':'))
        || (seg.len() >= 3 && seg.starts_with('<') && seg.ends_with('>'))
}

/// A client-side parameter segment: a `{…}` the client URL still carries (a
/// template the extractor could not fold). `:x` / `<x>` in a client URL are
/// literal text.
fn is_client_param(seg: &str) -> bool {
    seg.len() >= 2 && seg.starts_with('{') && seg.ends_with('}')
}

/// Segment-aware pairing. Equal paths pair exactly; a trailing-`*` topic
/// pattern pairs a concrete topic under it at equal length ([`topic_stem`]); a
/// path holding a parameter pairs only at equal length with each parameter
/// standing for one segment; otherwise one literal path may be a
/// segment-boundary suffix of the other, so a client mounted at `/api/v1/chat`
/// still reaches a handler registered as `/chat` — but `/news` never "ends
/// with" a handler named `ws`. A bare `*` on either side never pairs.
fn ws_pair(c: &[String], h: &[String]) -> WsPair {
    let bare_star = |p: &[String]| matches!(p, [only] if only == "*");
    if c.is_empty() || h.is_empty() || bare_star(c) || bare_star(h) {
        return WsPair::No;
    }
    if c == h {
        return WsPair::Exact;
    }
    if let Some(stem) = topic_stem(h) {
        // Equal leading segments imply equal length.
        let fits = match (c.split_last(), h.split_last()) {
            (Some((last, lead)), Some((_, h_lead))) => lead == h_lead && last.starts_with(stem),
            _ => false,
        };
        return if fits { WsPair::Wildcard } else { WsPair::No };
    }
    let templated = h.iter().any(|s| is_handler_param(s)) || c.iter().any(|s| is_client_param(s));
    if templated {
        let fits = c.len() == h.len()
            && c.iter()
                .zip(h)
                .all(|(a, b)| a == b || is_handler_param(b) || is_client_param(a));
        return if fits { WsPair::Param } else { WsPair::No };
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
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE};
    use repo_graph_core::{EdgeCategoryId, Node, NodeKindId, RepoId};

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
        assert!(is_generic_ws_key("ws"));
        assert!(is_generic_ws_key("default"));
        assert!(!is_generic_ws_key("/api/ws"));
        // A leading slash means a real path was read.
        assert!(!is_generic_ws_key("/default"));
    }

    #[test]
    fn empty_path_never_pairs() {
        assert_eq!(pair("", "/chat"), WsPair::No);
        assert_eq!(pair("/chat", "//"), WsPair::No);
    }

    #[test]
    fn param_segment_matches_one_segment_only() {
        assert_eq!(pair("/chat/lobby", "/chat/{room}"), WsPair::Param);
        assert_eq!(pair("/chat/lobby", "/chat/:room"), WsPair::Param);
        assert_eq!(pair("/chat/lobby", "/chat/<room>"), WsPair::Param);
        assert_eq!(pair("/chat/lobby/feed", "/chat/{room}"), WsPair::No);
        assert_eq!(pair("/chat", "/chat/{room}"), WsPair::No);
        assert_eq!(pair("/news/lobby", "/chat/{room}"), WsPair::No);
        // A client template the extractor could not fold.
        assert_eq!(pair("/chat/{}", "/chat/lobby"), WsPair::Param);
        // `:x` in a client URL is literal text, not a parameter.
        assert_eq!(pair("/chat/:room", "/chat/lobby"), WsPair::No);
    }

    #[test]
    fn templated_path_has_no_suffix_tier() {
        // Literal: suffix pairs. Templated: only equal length does.
        assert_eq!(pair("/api/chat/lobby", "/chat/lobby"), WsPair::Suffix);
        assert_eq!(pair("/api/chat/lobby", "/chat/{room}"), WsPair::No);
        assert_eq!(pair("/chat/{}", "/api/chat/lobby"), WsPair::No);
    }

    // ---- hand-built graphs for the route join ----------------------------

    fn repo(tag: &str) -> RepoId {
        RepoId::from_canonical(&format!("test://ws/{tag}"))
    }

    /// Builds one RepoGraph: nodes by (kind, qname), edges by qname.
    struct G {
        repo: RepoId,
        nodes: Vec<Node>,
        nav: CodeNav,
        edges: Vec<Edge>,
    }

    impl G {
        fn new(tag: &str) -> Self {
            Self {
                repo: repo(tag),
                nodes: vec![],
                nav: CodeNav::default(),
                edges: vec![],
            }
        }

        fn node(&mut self, kind: NodeKindId, qname: &str) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, self.repo, kind, qname);
            self.nodes.push(Node {
                id,
                repo: self.repo,
                confidence: Confidence::Medium,
                cells: vec![],
            });
            self.nav.record(id, qname, qname, kind, None);
            id
        }

        fn edge(&mut self, from: NodeId, to: NodeId, category: EdgeCategoryId) {
            self.edges.push(Edge {
                from,
                to,
                category,
                confidence: Confidence::Strong,
            });
        }

        fn build(self) -> RepoGraph {
            RepoGraph {
                repo: self.repo,
                nodes: self.nodes,
                edges: self.edges,
                nav: self.nav,
                symbols: Default::default(),
                unresolved_calls: vec![],
                unresolved_refs: vec![],
                properties: Default::default(),
            }
        }
    }

    /// A Go server: `route` HANDLED_BY `ServeWs`, which holds a gorilla
    /// upgrade (`ws:ws` HANDLED_BY `ServeWs`). Returns (graph, ws handler id).
    fn gorilla_server(route: &str) -> (RepoGraph, NodeId) {
        let mut s = G::new("server");
        let serve = s.node(node_kind::FUNCTION, "hub::ServeWs");
        let r = s.node(node_kind::ROUTE, route);
        let ws = s.node(node_kind::WS_HANDLER, "ws:ws");
        s.edge(r, serve, edge_category::HANDLED_BY);
        s.edge(ws, serve, edge_category::HANDLED_BY);
        (s.build(), ws)
    }

    /// A client graph with one WS_CLIENT per key. Returns (graph, ids).
    fn clients(keys: &[&str]) -> (RepoGraph, Vec<NodeId>) {
        let mut c = G::new("client");
        let ids = keys
            .iter()
            .map(|k| c.node(node_kind::WS_CLIENT, &format!("ws_client:{k}")))
            .collect();
        (c.build(), ids)
    }

    fn connects(edges: &[Edge], from: NodeId, to: NodeId) -> bool {
        edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.category == edge_category::WS_CONNECTS)
    }

    #[test]
    fn generic_handler_pairs_only_through_route_paths() {
        // The route serves /chat: the /ws client paired by name coincidence
        // before LA.18b, and must not now.
        let (server, ws) = gorilla_server("route:/chat");
        let (client, ids) = clients(&["/ws"]);
        let (edges, stats) = pair_all(&[server, client]);
        assert!(!connects(&edges, ids[0], ws), "{edges:?}");
        assert!(edges.is_empty(), "{edges:?}");
        assert_eq!(stats.dropped_generic, 1);
        // The same upgrade reached by route /ws: one edge, counted inherited.
        let (server, ws) = gorilla_server("route:/ws");
        let (client, ids) = clients(&["/ws"]);
        let (edges, stats) = pair_all(&[server, client]);
        assert_eq!(edges.len(), 1, "{edges:?}");
        assert!(connects(&edges, ids[0], ws));
        assert_eq!(
            stats,
            WsStats {
                inherited: 1,
                ..WsStats::default()
            }
        );
        // The legacy `<METHOD> <path>` shape and an LB.4a owner suffix read too.
        for route in ["GET /ws", "route:/ws @services/chat"] {
            let (server, ws) = gorilla_server(route);
            let (client, ids) = clients(&["/ws"]);
            let (edges, _) = pair_all(&[server, client]);
            assert!(connects(&edges, ids[0], ws), "{route}: {edges:?}");
        }
    }

    #[test]
    fn generic_client_never_pairs() {
        // Not with a generic handler, not with a handler literally at /ws.
        let (server, _) = gorilla_server("route:/ws");
        let mut literal = G::new("literal");
        literal.node(node_kind::WS_HANDLER, "ws:/ws");
        let (client, _) = clients(&["ws"]);
        let (edges, stats) = pair_all(&[server, literal.build(), client]);
        assert!(edges.is_empty(), "{edges:?}");
        assert_eq!(stats.pairs(), 0);
        assert_eq!(stats.dropped_generic, 2);
    }

    #[test]
    fn generic_handler_without_routes_is_dropped() {
        // An upgrade whose owner no route reaches, and a `default` handler
        // with no owner at all.
        let mut s = G::new("server");
        let serve = s.node(node_kind::FUNCTION, "hub::ServeWs");
        let ws = s.node(node_kind::WS_HANDLER, "ws:ws");
        s.edge(ws, serve, edge_category::HANDLED_BY);
        s.node(node_kind::WS_HANDLER, "ws:default");
        let (client, _) = clients(&["/ws", "/default"]);
        let (edges, stats) = pair_all(&[s.build(), client]);
        assert!(edges.is_empty(), "{edges:?}");
        assert_eq!(stats.dropped_generic, 4);
    }

    #[test]
    fn inherits_one_call_hop_not_two() {
        // route:/one -> H1 -CALLS-> upgrade ; route:/two -> H2 -CALLS-> H3
        // -CALLS-> upgrade. Only /one is inherited.
        let mut s = G::new("server");
        let upgrade = s.node(node_kind::FUNCTION, "ws::upgrade");
        let h1 = s.node(node_kind::FUNCTION, "api::one");
        let h2 = s.node(node_kind::FUNCTION, "api::two");
        let h3 = s.node(node_kind::FUNCTION, "api::mid");
        let one = s.node(node_kind::ROUTE, "route:/one");
        let two = s.node(node_kind::ROUTE, "route:/two");
        let ws = s.node(node_kind::WS_HANDLER, "ws:ws");
        s.edge(ws, upgrade, edge_category::HANDLED_BY);
        s.edge(one, h1, edge_category::HANDLED_BY);
        s.edge(two, h2, edge_category::HANDLED_BY);
        s.edge(h1, upgrade, edge_category::CALLS);
        s.edge(h2, h3, edge_category::CALLS);
        s.edge(h3, upgrade, edge_category::CALLS);
        let (client, ids) = clients(&["/one", "/two"]);
        let (edges, stats) = pair_all(&[s.build(), client]);
        assert!(connects(&edges, ids[0], ws), "{edges:?}");
        assert!(!connects(&edges, ids[1], ws), "{edges:?}");
        assert_eq!(stats.inherited, 1);
        assert_eq!(stats.dropped_generic, 1);
    }

    #[test]
    fn raw_slash_ws_is_not_generic() {
        // A client at `/ws` is a path; a handler at `/ws` is a path. They
        // pair Exact. The fallback `ws` pairs neither.
        assert!(!is_generic_ws_key("/ws"));
        assert!(is_generic_ws_key("ws"));
        let mut s = G::new("server");
        let literal = s.node(node_kind::WS_HANDLER, "ws:/ws");
        let generic = s.node(node_kind::WS_HANDLER, "ws:ws");
        let (client, ids) = clients(&["/ws"]);
        let (edges, stats) = pair_all(&[s.build(), client]);
        assert!(connects(&edges, ids[0], literal), "{edges:?}");
        assert!(!connects(&edges, ids[0], generic), "{edges:?}");
        assert_eq!(stats.exact, 1);
        assert_eq!(stats.dropped_generic, 1);
    }

    #[test]
    fn param_handler_pairs_counted_param() {
        let mut s = G::new("server");
        let room = s.node(node_kind::WS_HANDLER, "ws:/chat/{room}");
        let (client, ids) = clients(&["/chat/lobby", "/chat/lobby/feed"]);
        let (edges, stats) = pair_all(&[s.build(), client]);
        assert!(connects(&edges, ids[0], room));
        assert!(!connects(&edges, ids[1], room));
        assert_eq!(
            stats,
            WsStats {
                param: 1,
                ..WsStats::default()
            }
        );
    }

    #[test]
    fn each_client_handler_pair_is_emitted_once() {
        // A generic handler with two inherited paths that both reach the
        // client (/ws exact, /api/ws suffix) still yields one edge.
        let mut s = G::new("server");
        let serve = s.node(node_kind::FUNCTION, "hub::ServeWs");
        let a = s.node(node_kind::ROUTE, "route:/ws");
        let b = s.node(node_kind::ROUTE, "GET /api/ws");
        let ws = s.node(node_kind::WS_HANDLER, "ws:ws");
        s.edge(a, serve, edge_category::HANDLED_BY);
        s.edge(b, serve, edge_category::HANDLED_BY);
        s.edge(ws, serve, edge_category::HANDLED_BY);
        let (client, ids) = clients(&["/api/ws"]);
        let (edges, stats) = pair_all(&[s.build(), client]);
        assert_eq!(edges.len(), 1, "{edges:?}");
        assert!(connects(&edges, ids[0], ws));
        assert_eq!(stats.inherited, 1);
    }

    // ---- LA.18c: channel-keyed frameworks --------------------------------

    #[test]
    fn phoenix_topic_wildcard_pairs_suffix() {
        assert_eq!(pair("room:lobby", "room:*"), WsPair::Wildcard);
        assert_eq!(pair("room:42:admin", "room:*"), WsPair::Wildcard);
        // Phoenix matches `"room:" <> _`, so the empty suffix pairs too.
        assert_eq!(pair("room:", "room:*"), WsPair::Wildcard);
        assert_eq!(pair("lobby:room", "room:*"), WsPair::No);
        assert_eq!(pair("roomy", "room:*"), WsPair::No);
        // The pattern is the handler's: a client `*` covers nothing.
        assert_eq!(pair("room:*", "room:lobby"), WsPair::No);
        // Class-named channels (ActionCable) pair exactly, and apart.
        assert_eq!(pair("ChatChannel", "ChatChannel"), WsPair::Exact);
        assert_eq!(pair("ChatChannel", "PresenceChannel"), WsPair::No);
        // A topic never reaches a mount path, nor a mount a topic pattern.
        assert_eq!(pair("room:lobby", "/socket"), WsPair::No);
        assert_eq!(pair("/socket", "room:*"), WsPair::No);
    }

    #[test]
    fn bare_star_never_pairs() {
        assert_eq!(pair("room:lobby", "*"), WsPair::No);
        assert_eq!(pair("*", "*"), WsPair::No);
        assert_eq!(pair("*", "room:*"), WsPair::No);
        assert!(is_generic_ws_key("*"));
        // Through pair_all: a `*` catch-all counts dropped-generic, pairs
        // nothing and inherits no route path even when a route reaches the
        // function its site sits in.
        let mut s = G::new("server");
        let serve = s.node(node_kind::FUNCTION, "sock::serve");
        let r = s.node(node_kind::ROUTE, "route:/room:lobby");
        let star = s.node(node_kind::WS_HANDLER, "ws:*");
        s.edge(r, serve, edge_category::HANDLED_BY);
        s.edge(star, serve, edge_category::HANDLED_BY);
        let (client, _) = clients(&["room:lobby", "/room:lobby"]);
        let (edges, stats) = pair_all(&[s.build(), client]);
        assert!(edges.is_empty(), "{edges:?}");
        assert_eq!(stats.pairs(), 0);
        assert_eq!(stats.dropped_generic, 2);
    }

    #[test]
    fn wildcard_needs_equal_segment_count() {
        assert_eq!(pair("/chat/room:lobby", "room:*"), WsPair::No);
        assert_eq!(pair("room:lobby", "/chat/room:*"), WsPair::No);
        assert_eq!(pair("/chat/room:lobby", "/chat/room:*"), WsPair::Wildcard);
        // Leading segments must be equal, not merely the same count.
        assert_eq!(pair("/news/room:lobby", "/chat/room:*"), WsPair::No);
        // A path whose last segment is a bare `*` is not a topic pattern and
        // keeps the path tiers (the literal suffix one, here).
        assert_eq!(pair("/api/files/*", "/files/*"), WsPair::Suffix);
    }

    #[test]
    fn wildcard_pairs_counted_wildcard() {
        // The Phoenix fixture shape: an endpoint mount and a topic pattern on
        // the server, the phoenix.js socket and its joined topic on the client.
        let mut s = G::new("server");
        let mount = s.node(node_kind::WS_HANDLER, "ws:/socket");
        let room = s.node(node_kind::WS_HANDLER, "ws:room:*");
        let (client, ids) = clients(&["/socket", "room:lobby"]);
        let (edges, stats) = pair_all(&[s.build(), client]);
        assert_eq!(edges.len(), 2, "{edges:?}");
        assert!(connects(&edges, ids[0], mount));
        assert!(connects(&edges, ids[1], room));
        assert!(!connects(&edges, ids[1], mount));
        assert_eq!(
            stats,
            WsStats {
                exact: 1,
                wildcard: 1,
                ..WsStats::default()
            }
        );
    }
}
