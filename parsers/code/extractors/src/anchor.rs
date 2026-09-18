//! A5.8 — anchor RPC-family marker nodes to the code that owns them.
//!
//! The cross-cutting extractors mint marker nodes (GRPC_CLIENT, WS_*, EVENT_*,
//! GRAPHQL_*) from a needle in the file text. `CodeNav::record` only writes
//! the nav parent, so until this pass those markers were graph islands: no
//! structural edge touched them and no POSITION cell located them. A trace from
//! the function that builds a gRPC stub could not cross into the gRPC hop, and
//! `locate_node` / `glia arch` could not place the marker in a file.
//!
//! Each extractor now reports an [`Anchor`] (marker id + 0-indexed line) for
//! the needle that minted the node. [`attach`] then:
//!
//! - gives the marker a one-line POSITION cell at its first anchor;
//! - emits an owner edge to the innermost METHOD / FUNCTION whose POSITION span
//!   holds an anchor ([`owner_edge`]): the method USES an outbound marker, and
//!   an inbound marker is HANDLED_BY the method. Both are blast carry edges, so
//!   `caller -CALLS-> FetchUser -USES-> grpc_client:X -GRPC_CALLS-> grpc:X`
//!   is one traversable chain;
//! - when no anchor of a marker lies inside a function (module-level
//!   `const ws = new WebSocket(...)`), falls back to a CONTAINS edge from the
//!   module. CONTAINS is structural, not a carry edge, so blast results do not
//!   widen.
//!
//! Anchor density per extractor: the gRPC client passes and the type-keyed
//! event needles anchor EVERY site (their name is read from that site); the
//! string-keyed needles anchor only the site that minted the node, because the
//! name they read is not per-site (e.g. a GraphQL operation name is the file's
//! first `gql` tag, whichever `useQuery(` line matched).
//!
//! Parsers extract, the graph crate resolves: this pass reads only the file's
//! own parse (spans the language parser already attached), so its output is a
//! function of the file's content and is safe to cache with the `FileParse`.
//! Queue markers (A2.8) should reuse this module rather than grow a parallel
//! helper, as GRPC_SERVER (A5.3) does: it finds its implementing type through
//! [`build_span_index`] and anchors through [`attach`].

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{CodeNav, FileParse, cell_type, edge_category, node_kind};
use repo_graph_core::{
    Cell, CellPayload, Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId,
};

/// Where a marker node's needle actually fired, 0-indexed like every other
/// POSITION in the repo (`repo_graph_doc::position_json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchor {
    pub node: NodeId,
    pub line: u32,
}

/// Marker kinds a method reaches OUT through: the method USES the marker.
const OUTBOUND: &[NodeKindId] = &[
    node_kind::GRPC_CLIENT,
    node_kind::WS_CLIENT,
    node_kind::EVENT_EMITTER,
    node_kind::GRAPHQL_OPERATION,
];

/// Marker kinds that are an INBOUND contract: the marker is HANDLED_BY the
/// method, mirroring the HTTP side (`ROUTE --HANDLED_BY--> handler`).
const INBOUND: &[NodeKindId] = &[
    node_kind::WS_HANDLER,
    node_kind::EVENT_HANDLER,
    node_kind::GRAPHQL_RESOLVER,
    node_kind::GRPC_SERVER,
];

/// True for every kind [`attach`] anchors.
pub fn is_marker_kind(kind: NodeKindId) -> bool {
    OUTBOUND.contains(&kind) || INBOUND.contains(&kind)
}

/// 0-indexed line of `byte_offset` in `source`: the number of `\n` before it,
/// which is the index `source.lines().enumerate()` and tree-sitter rows both
/// use. An offset past the end clamps to the last line.
pub fn line_of(source: &str, byte_offset: usize) -> u32 {
    let end = byte_offset.min(source.len());
    let newlines = source.as_bytes()[..end]
        .iter()
        .filter(|&&b| b == b'\n')
        .count();
    u32::try_from(newlines).unwrap_or(u32::MAX)
}

/// `(start_line, end_line, id)` for every METHOD / FUNCTION in a file, read
/// from the POSITION cells the language parser already attached. Innermost
/// wins: sorted by (start desc, end asc, id asc), so the first span that holds
/// a line is the most deeply nested one, and ties never depend on node order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OwnerIndex(Vec<(u32, u32, NodeId)>);

#[derive(serde::Deserialize)]
struct Span {
    start_line: u32,
    end_line: u32,
}

/// The first POSITION span on `node`, if it parses.
pub fn position_span(node: &Node) -> Option<(u32, u32)> {
    node.cells
        .iter()
        .filter(|c| c.kind == cell_type::POSITION)
        .find_map(|c| {
            let raw = match &c.payload {
                CellPayload::Json(s) | CellPayload::Text(s) => s,
                CellPayload::Bytes(_) => return None,
            };
            let span: Span = serde_json::from_str(raw).ok()?;
            (span.start_line <= span.end_line).then_some((span.start_line, span.end_line))
        })
}

pub fn build_owner_index(nodes: &[Node], nav: &CodeNav) -> OwnerIndex {
    build_span_index(nodes, nav, &[node_kind::METHOD, node_kind::FUNCTION])
}

/// [`build_owner_index`] over any set of node kinds, with the same
/// innermost-wins order, so [`owner_of_line`] answers "which `kinds` node
/// encloses this line". A5.3 asks it for the CLASS / STRUCT that declares a
/// gRPC base type, rather than growing a second span helper.
pub fn build_span_index(nodes: &[Node], nav: &CodeNav, kinds: &[NodeKindId]) -> OwnerIndex {
    let mut spans: Vec<(u32, u32, NodeId)> = nodes
        .iter()
        .filter(|n| nav.kind_by_id.get(&n.id).is_some_and(|k| kinds.contains(k)))
        .filter_map(|n| position_span(n).map(|(s, e)| (s, e, n.id)))
        .collect();
    spans.sort_unstable_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.0.cmp(&b.2.0))
    });
    spans.dedup();
    OwnerIndex(spans)
}

/// The innermost METHOD / FUNCTION whose span holds `line`.
pub fn owner_of_line(idx: &OwnerIndex, line: u32) -> Option<NodeId> {
    idx.0
        .iter()
        .find(|(start, end, _)| *start <= line && line <= *end)
        .map(|(_, _, id)| *id)
}

/// POSITION cell for a marker node: a one-line span at the needle, in the
/// canonical `{"file","start_line","end_line"}` key order.
pub fn position_cell(file_rel: &str, line: u32) -> Cell {
    let file = serde_json::to_string(file_rel).unwrap_or_else(|_| "\"\"".to_string());
    Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(format!(
            r#"{{"file":{file},"start_line":{line},"end_line":{line}}}"#
        )),
    }
}

/// The edge that ties `marker` to the method that owns it, by marker kind:
/// outbound kinds give `owner --USES--> marker`, inbound kinds give
/// `marker --HANDLED_BY--> owner`. `None` for any other kind.
pub fn owner_edge(marker_kind: NodeKindId, marker: NodeId, owner: NodeId) -> Option<Edge> {
    let (from, to, category) = if OUTBOUND.contains(&marker_kind) {
        (owner, marker, edge_category::USES)
    } else if INBOUND.contains(&marker_kind) {
        (marker, owner, edge_category::HANDLED_BY)
    } else {
        return None;
    };
    Some(Edge {
        from,
        to,
        category,
        confidence: Confidence::Medium,
    })
}

fn is_owner_edge(e: &Edge, marker: NodeId) -> bool {
    (e.category == edge_category::USES && e.to == marker)
        || (e.category == edge_category::HANDLED_BY && e.from == marker)
}

/// How the markers of one file (or one `attach` call) ended up.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AnchorStats {
    /// Markers with at least one owner edge to a METHOD / FUNCTION.
    pub to_method: usize,
    /// Markers no function encloses, tied to the module by CONTAINS.
    pub to_module: usize,
    /// Markers left floating: no owner edge, no module edge.
    pub unanchored: usize,
}

impl AnchorStats {
    pub fn add(&mut self, other: AnchorStats) {
        self.to_method += other.to_method;
        self.to_module += other.to_module;
        self.unanchored += other.unanchored;
    }

    pub fn total(&self) -> usize {
        self.to_method + self.to_module + self.unanchored
    }
}

/// Attach POSITION to every anchored marker and emit its owner edge, or the
/// module CONTAINS fallback when none of its anchors lies inside a function.
///
/// Sorts `anchors` by `(line, node id)` first: the emitted edge order must not
/// depend on extractor iteration order, or the byte-identical store gate sees
/// flapping bytes. May be called more than once on one parse (the engine's
/// post-cache gRPC client pass does); a marker that already carries a
/// POSITION keeps it, and an edge already present is not emitted twice.
///
/// The returned stats cover the markers named by `anchors`; an anchor whose
/// node is not a marker in `fp` counts as `unanchored`.
pub fn attach(
    fp: &mut FileParse,
    path: &str,
    module_id: NodeId,
    anchors: &mut Vec<Anchor>,
) -> AnchorStats {
    let mut stats = AnchorStats::default();
    if anchors.is_empty() {
        return stats;
    }
    anchors.sort_unstable_by_key(|a| (a.line, a.node.0));
    anchors.dedup();

    let idx = build_owner_index(&fp.nodes, &fp.nav);
    let mut existing: HashSet<(NodeId, NodeId, EdgeCategoryId)> = fp
        .edges
        .iter()
        .map(|e| (e.from, e.to, e.category))
        .collect();
    let mut push_edge = |fp: &mut FileParse, e: Edge| {
        if existing.insert((e.from, e.to, e.category)) {
            fp.edges.push(e);
        }
    };

    // Markers in first-anchor order (the anchors are line-sorted, so this is
    // also each marker's lowest line).
    let mut order: Vec<(NodeId, u32)> = Vec::new();
    let mut owned: HashMap<NodeId, bool> = HashMap::new();
    let mut bogus: HashSet<NodeId> = HashSet::new();
    for a in anchors.iter() {
        let Some(kind) = fp.nav.kind_by_id.get(&a.node).copied() else {
            bogus.insert(a.node);
            continue;
        };
        if !is_marker_kind(kind) {
            bogus.insert(a.node);
            continue;
        }
        let has_owner = owned.entry(a.node).or_insert_with(|| {
            order.push((a.node, a.line));
            false
        });
        if let Some(owner) = owner_of_line(&idx, a.line)
            && owner != a.node
            && let Some(edge) = owner_edge(kind, a.node, owner)
        {
            *has_owner = true;
            push_edge(fp, edge);
        }
    }

    for (marker, line) in order {
        if let Some(node) = fp.nodes.iter_mut().find(|n| n.id == marker)
            && !node.cells.iter().any(|c| c.kind == cell_type::POSITION)
        {
            node.cells.push(position_cell(path, line));
        }
        let has_owner = owned.get(&marker).copied().unwrap_or(false)
            || fp.edges.iter().any(|e| is_owner_edge(e, marker));
        if has_owner {
            stats.to_method += 1;
        } else {
            push_edge(
                fp,
                Edge {
                    from: module_id,
                    to: marker,
                    category: edge_category::CONTAINS,
                    confidence: Confidence::Medium,
                },
            );
            stats.to_module += 1;
        }
    }
    stats.unanchored += bogus.len();
    stats
}

/// Classify every marker node in `fp` by the edges the parse now carries:
/// owner edge → `to_method`, CONTAINS from a MODULE → `to_module`, neither →
/// `unanchored`. Reads the finished parse rather than `attach`'s return value,
/// so a cache-served `FileParse` counts exactly like a fresh one.
pub fn census(fp: &FileParse) -> AnchorStats {
    let mut stats = AnchorStats::default();
    let mut owner_linked: HashSet<NodeId> = HashSet::new();
    let mut module_linked: HashSet<NodeId> = HashSet::new();
    for e in &fp.edges {
        match e.category {
            c if c == edge_category::USES => {
                owner_linked.insert(e.to);
            }
            c if c == edge_category::HANDLED_BY => {
                owner_linked.insert(e.from);
            }
            c if c == edge_category::CONTAINS
                && fp.nav.kind_by_id.get(&e.from) == Some(&node_kind::MODULE) =>
            {
                module_linked.insert(e.to);
            }
            _ => {}
        }
    }
    let mut seen: HashSet<NodeId> = HashSet::new();
    for n in &fp.nodes {
        let is_marker = fp
            .nav
            .kind_by_id
            .get(&n.id)
            .is_some_and(|k| is_marker_kind(*k));
        if !is_marker || !seen.insert(n.id) {
            continue;
        }
        if owner_linked.contains(&n.id) {
            stats.to_method += 1;
        } else if module_linked.contains(&n.id) {
            stats.to_module += 1;
        } else {
            stats.unanchored += 1;
        }
    }
    stats
}

/// The fired_on marker, once per repo that holds a marker node:
///   `[marker-anchor] {a} anchored to methods, {m} to module, {u} unanchored repo=<label>`
pub fn report(stats: AnchorStats, repo_label: &str) {
    if stats.total() > 0 {
        eprintln!(
            "[marker-anchor] {} anchored to methods, {} to module, {} unanchored repo={repo_label}",
            stats.to_method, stats.to_module, stats.unanchored
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::GRAPH_TYPE;
    use repo_graph_core::RepoId;

    fn repo() -> RepoId {
        RepoId(7)
    }

    fn id(kind: NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn node(id: NodeId, cells: Vec<Cell>) -> Node {
        Node {
            id,
            repo: repo(),
            confidence: Confidence::Strong,
            cells,
        }
    }

    fn span_cell(start: u32, end: u32) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"a.ts","start_line":{start},"end_line":{end}}}"#
            )),
        }
    }

    /// `(kind, qname, POSITION span)` for one node of a test parse.
    type Entry<'a> = (NodeKindId, &'a str, Option<(u32, u32)>);

    /// A parse with a MODULE plus the given entries.
    fn parse_with(entries: &[Entry<'_>]) -> (FileParse, NodeId) {
        let mut fp = FileParse::default();
        let module = id(node_kind::MODULE, "a");
        fp.nodes.push(node(module, vec![span_cell(0, 100)]));
        fp.nav.record(module, "a", "a", node_kind::MODULE, None);
        for (kind, qname, span) in entries {
            let nid = id(*kind, qname);
            let cells = span.map(|(s, e)| vec![span_cell(s, e)]).unwrap_or_default();
            fp.nodes.push(node(nid, cells));
            fp.nav.record(nid, qname, qname, *kind, Some(module));
        }
        (fp, module)
    }

    fn position_of(fp: &FileParse, nid: NodeId) -> Option<String> {
        fp.nodes
            .iter()
            .find(|n| n.id == nid)?
            .cells
            .iter()
            .find(|c| c.kind == cell_type::POSITION)
            .and_then(|c| match &c.payload {
                CellPayload::Json(s) => Some(s.clone()),
                _ => None,
            })
    }

    #[test]
    fn line_of_counts_newlines_before_the_offset() {
        let src = "a\nbb\nccc";
        assert_eq!(line_of(src, 0), 0);
        assert_eq!(line_of(src, 2), 1);
        assert_eq!(line_of(src, 5), 2);
        assert_eq!(line_of(src, 999), 2, "past the end clamps to the last line");
        let lines: Vec<_> = src.lines().collect();
        assert_eq!(lines[line_of(src, 6) as usize], "ccc");
    }

    #[test]
    fn owner_index_picks_innermost_span() {
        let (fp, _) = parse_with(&[
            (node_kind::FUNCTION, "outer", Some((2, 20))),
            (node_kind::FUNCTION, "inner", Some((5, 9))),
            (node_kind::METHOD, "sibling", Some((12, 14))),
            // Not an owner kind: a CLASS spanning everything never wins.
            (node_kind::CLASS, "Klass", Some((1, 30))),
            // No POSITION: cannot own anything.
            (node_kind::FUNCTION, "unlocated", None),
        ]);
        let idx = build_owner_index(&fp.nodes, &fp.nav);
        assert_eq!(
            owner_of_line(&idx, 6),
            Some(id(node_kind::FUNCTION, "inner"))
        );
        assert_eq!(
            owner_of_line(&idx, 5),
            Some(id(node_kind::FUNCTION, "inner"))
        );
        assert_eq!(
            owner_of_line(&idx, 3),
            Some(id(node_kind::FUNCTION, "outer"))
        );
        assert_eq!(
            owner_of_line(&idx, 13),
            Some(id(node_kind::METHOD, "sibling"))
        );
        assert_eq!(
            owner_of_line(&idx, 20),
            Some(id(node_kind::FUNCTION, "outer"))
        );
        assert_eq!(
            owner_of_line(&idx, 25),
            None,
            "a class body line has no owner"
        );
        assert_eq!(owner_of_line(&idx, 0), None);
    }

    #[test]
    fn owner_index_breaks_identical_span_ties_by_id() {
        let (mut fp, _) = parse_with(&[
            (node_kind::FUNCTION, "b", Some((3, 3))),
            (node_kind::FUNCTION, "a", Some((3, 3))),
        ]);
        let first = owner_of_line(&build_owner_index(&fp.nodes, &fp.nav), 3);
        fp.nodes.reverse();
        let second = owner_of_line(&build_owner_index(&fp.nodes, &fp.nav), 3);
        assert_eq!(first, second, "node order must not change the owner");
    }

    #[test]
    fn owner_edge_direction_by_kind() {
        let owner = id(node_kind::METHOD, "m");
        for kind in [
            node_kind::GRPC_CLIENT,
            node_kind::WS_CLIENT,
            node_kind::EVENT_EMITTER,
            node_kind::GRAPHQL_OPERATION,
        ] {
            let marker = id(kind, "x");
            let e = owner_edge(kind, marker, owner).expect("outbound kinds anchor");
            assert_eq!(
                (e.from, e.to, e.category),
                (owner, marker, edge_category::USES)
            );
            assert_eq!(e.confidence, Confidence::Medium);
        }
        for kind in [
            node_kind::WS_HANDLER,
            node_kind::EVENT_HANDLER,
            node_kind::GRAPHQL_RESOLVER,
            node_kind::GRPC_SERVER,
        ] {
            let marker = id(kind, "x");
            let e = owner_edge(kind, marker, owner).expect("inbound kinds anchor");
            assert_eq!(
                (e.from, e.to, e.category),
                (marker, owner, edge_category::HANDLED_BY)
            );
        }
        for kind in [
            node_kind::QUEUE_PRODUCER,
            node_kind::ROUTE,
            node_kind::METHOD,
            node_kind::GRPC_SERVICE,
        ] {
            assert!(owner_edge(kind, id(kind, "x"), owner).is_none(), "{kind:?}");
        }
    }

    #[test]
    fn marker_inside_a_function_gets_position_and_owner_edge() {
        let (mut fp, module) = parse_with(&[
            (node_kind::FUNCTION, "FetchUser", Some((10, 23))),
            (node_kind::GRPC_CLIENT, "grpc_client:UserService", None),
        ]);
        let client = id(node_kind::GRPC_CLIENT, "grpc_client:UserService");
        let mut anchors = vec![Anchor {
            node: client,
            line: 17,
        }];
        let stats = attach(&mut fp, "client/main.go", module, &mut anchors);
        assert_eq!(
            stats,
            AnchorStats {
                to_method: 1,
                to_module: 0,
                unanchored: 0
            }
        );
        assert_eq!(
            position_of(&fp, client).as_deref(),
            Some(r#"{"file":"client/main.go","start_line":17,"end_line":17}"#)
        );
        assert_eq!(
            fp.edges,
            vec![Edge {
                from: id(node_kind::FUNCTION, "FetchUser"),
                to: client,
                category: edge_category::USES,
                confidence: Confidence::Medium,
            }]
        );
        assert_eq!(census(&fp), stats);
    }

    #[test]
    fn module_level_marker_falls_back_to_contains() {
        let (mut fp, module) = parse_with(&[
            (node_kind::FUNCTION, "ServeWs", Some((14, 27))),
            (node_kind::WS_HANDLER, "ws:ws", None),
        ]);
        let handler = id(node_kind::WS_HANDLER, "ws:ws");
        let mut anchors = vec![Anchor {
            node: handler,
            line: 8,
        }];
        let stats = attach(&mut fp, "hub.go", module, &mut anchors);
        assert_eq!(
            stats,
            AnchorStats {
                to_method: 0,
                to_module: 1,
                unanchored: 0
            }
        );
        assert_eq!(
            fp.edges,
            vec![Edge {
                from: module,
                to: handler,
                category: edge_category::CONTAINS,
                confidence: Confidence::Medium,
            }]
        );
        assert!(position_of(&fp, handler).is_some_and(|p| p.contains("\"start_line\":8")));
        assert_eq!(census(&fp), stats);
    }

    #[test]
    fn module_fallback_only_when_no_anchor_has_an_owner() {
        let (mut fp, module) = parse_with(&[
            (node_kind::FUNCTION, "a", Some((10, 12))),
            (node_kind::FUNCTION, "b", Some((20, 22))),
            (node_kind::GRPC_CLIENT, "grpc_client:X", None),
        ]);
        let client = id(node_kind::GRPC_CLIENT, "grpc_client:X");
        // A module-level site at line 2, plus a site in each function.
        let mut anchors = vec![
            Anchor {
                node: client,
                line: 21,
            },
            Anchor {
                node: client,
                line: 2,
            },
            Anchor {
                node: client,
                line: 11,
            },
        ];
        let stats = attach(&mut fp, "x.go", module, &mut anchors);
        assert_eq!(stats.to_method, 1);
        assert_eq!(stats.to_module, 0);
        let uses: Vec<NodeId> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::USES)
            .map(|e| e.from)
            .collect();
        assert_eq!(
            uses,
            vec![id(node_kind::FUNCTION, "a"), id(node_kind::FUNCTION, "b")],
            "one USES per owning function, in line order"
        );
        assert!(
            !fp.edges
                .iter()
                .any(|e| e.category == edge_category::CONTAINS)
        );
        // POSITION is the lowest anchored line.
        assert!(position_of(&fp, client).is_some_and(|p| p.contains("\"start_line\":2,")));
    }

    #[test]
    fn attach_is_idempotent_and_keeps_an_existing_position() {
        let (mut fp, module) = parse_with(&[
            (node_kind::FUNCTION, "f", Some((1, 5))),
            (node_kind::EVENT_EMITTER, "event_emit:x", Some((3, 3))),
        ]);
        let marker = id(node_kind::EVENT_EMITTER, "event_emit:x");
        let mut anchors = vec![Anchor {
            node: marker,
            line: 4,
        }];
        attach(&mut fp, "a.ts", module, &mut anchors.clone());
        attach(&mut fp, "a.ts", module, &mut anchors);
        assert_eq!(
            fp.edges.len(),
            1,
            "no duplicate owner edge on a second call"
        );
        let positions = fp.nodes.iter().find(|n| n.id == marker).map(|n| {
            n.cells
                .iter()
                .filter(|c| c.kind == cell_type::POSITION)
                .count()
        });
        assert_eq!(positions, Some(1));
        assert!(position_of(&fp, marker).is_some_and(|p| p.contains("\"start_line\":3")));
    }

    #[test]
    fn non_marker_anchor_is_ignored_and_counted() {
        let (mut fp, module) = parse_with(&[(node_kind::FUNCTION, "f", Some((1, 5)))]);
        let f = id(node_kind::FUNCTION, "f");
        let ghost = id(node_kind::WS_CLIENT, "ws_client:nowhere");
        let mut anchors = vec![
            Anchor { node: f, line: 2 },
            Anchor {
                node: ghost,
                line: 2,
            },
        ];
        let stats = attach(&mut fp, "a.ts", module, &mut anchors);
        assert_eq!(
            stats,
            AnchorStats {
                to_method: 0,
                to_module: 0,
                unanchored: 2
            }
        );
        assert!(fp.edges.is_empty());
        assert!(
            fp.nodes
                .iter()
                .find(|n| n.id == f)
                .is_some_and(|n| n.cells.len() == 1)
        );
    }

    #[test]
    fn census_counts_a_marker_nobody_anchored() {
        let (fp, _) = parse_with(&[(node_kind::GRAPHQL_RESOLVER, "graphql_resolver:Q", None)]);
        assert_eq!(
            census(&fp),
            AnchorStats {
                to_method: 0,
                to_module: 0,
                unanchored: 1
            }
        );
    }

    #[test]
    fn attach_edge_order_is_independent_of_anchor_order() {
        let build = |order: &[usize]| {
            let (mut fp, module) = parse_with(&[
                (node_kind::FUNCTION, "f1", Some((1, 3))),
                (node_kind::FUNCTION, "f2", Some((5, 7))),
                (node_kind::METHOD, "m3", Some((9, 11))),
                (node_kind::WS_CLIENT, "ws_client:/a", None),
                (node_kind::EVENT_HANDLER, "event_handle:b", None),
                (node_kind::GRAPHQL_OPERATION, "graphql_op:c", None),
                (node_kind::EVENT_EMITTER, "event_emit:d", None),
            ]);
            let all = [
                Anchor {
                    node: id(node_kind::WS_CLIENT, "ws_client:/a"),
                    line: 2,
                },
                Anchor {
                    node: id(node_kind::EVENT_HANDLER, "event_handle:b"),
                    line: 6,
                },
                Anchor {
                    node: id(node_kind::GRAPHQL_OPERATION, "graphql_op:c"),
                    line: 10,
                },
                Anchor {
                    node: id(node_kind::EVENT_EMITTER, "event_emit:d"),
                    line: 20,
                },
                Anchor {
                    node: id(node_kind::WS_CLIENT, "ws_client:/a"),
                    line: 6,
                },
            ];
            let mut anchors: Vec<Anchor> = order.iter().map(|&i| all[i]).collect();
            attach(&mut fp, "a.ts", module, &mut anchors);
            fp
        };
        let a = build(&[0, 1, 2, 3, 4]);
        let b = build(&[4, 3, 2, 1, 0]);
        let c = build(&[2, 0, 4, 1, 3]);
        assert_eq!(a.edges, b.edges);
        assert_eq!(a.edges, c.edges);
        assert_eq!(a.nodes, b.nodes);
        assert_eq!(a.nodes, c.nodes);
        assert_eq!(a.edges.len(), 5, "4 owner edges + 1 module fallback");
    }
}
