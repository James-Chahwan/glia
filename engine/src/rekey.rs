//! The NodeId rekey pass: move one node of a [`FileParse`] to a new id, with
//! every reference to it inside that parse. Filled by LB.4a; LC.2 edits it.
//!
//! Two post-cache passes re-key nodes, so the bookkeeping lives here once:
//! the A11.2 endpoint fold ([`crate::endpoint_fold`], which splits one id
//! across several call sites and so drives [`rewrite_node_id`] itself) and the
//! LB.4a HTTP owner pass ([`crate::http_owner`], one id to one id through
//! [`rekey_node`]).
//!
//! A rekey is file-local: it rewrites the ids one `FileParse` holds. A
//! reference to the node from ANOTHER parse would dangle, which is why the
//! owner pass counts such references (`foreign`) before it moves anything.
//!
//! Module slot declared by L0.2 so its owner edits only this file.
//! Crate-private: cross-module items are `pub(crate)`.

use repo_graph_code_domain::{CodeNav, FileParse};
use repo_graph_core::NodeId;

/// Move `old`'s place in the nav to `new`: drop its name/qname/kind, hand
/// over its parent slot (keeping its position among the siblings) and its
/// children. If `new` is already in the nav (another call site in the file
/// was already on that path), `new` keeps its own parent and `old` is only
/// removed from its parent's list. Re-keying a node inside a FileParse
/// happens only through here, so the `children_of` bookkeeping has to be
/// exact: a nav-derived traversal that loses a child does so silently.
pub(crate) fn rewrite_node_id(nav: &mut CodeNav, old: NodeId, new: NodeId) {
    nav.name_by_id.remove(&old);
    nav.qname_by_id.remove(&old);
    nav.kind_by_id.remove(&old);
    if let Some(p) = nav.parent_of.remove(&old) {
        let adopt = !nav.parent_of.contains_key(&new);
        if adopt {
            nav.parent_of.insert(new, p);
        }
        if let Some(kids) = nav.children_of.get_mut(&p) {
            let at = kids.iter().position(|k| *k == old);
            kids.retain(|k| *k != old);
            if adopt && !kids.contains(&new) {
                // `at` is at most the new length: only `old` entries were
                // removed, and none of them came before it.
                let at = at.unwrap_or(kids.len()).min(kids.len());
                kids.insert(at, new);
            }
        }
    }
    if let Some(kids) = nav.children_of.remove(&old) {
        for k in &kids {
            if let Some(p) = nav.parent_of.get_mut(k)
                && *p == old
            {
                *p = new;
            }
        }
        let slot = nav.children_of.entry(new).or_default();
        for k in kids {
            if !slot.contains(&k) {
                slot.push(k);
            }
        }
    }
}

/// Move node `old` of `fp` to `new`, whose qualified name is `qname`. The
/// display name and kind are kept.
///
/// Rewritten, in place and in their existing order:
/// - every node entry with id `old` (a TypeScript file pushes one per call
///   site). If `new` already had an entry in `fp`, the `old` entries' cells
///   are appended to the first `new` entry and the `old` entries dropped —
///   the rule `merge_parses` applies to a duplicate id, applied early;
/// - edges, both ends;
/// - `refs[].from` and `refs[].from_module` (go and ts_routes emit a ROUTE's
///   HANDLED_BY as an `UnresolvedRef` from the ROUTE id: forgetting refs
///   would drop every such handler edge);
/// - `calls[].from`, the `properties` set;
/// - the nav through [`rewrite_node_id`], then `new`'s qname, name and kind.
///
/// A no-op when `old == new` or `old` is not a node of `fp`.
pub(crate) fn rekey_node(fp: &mut FileParse, old: NodeId, new: NodeId, qname: &str) {
    if old == new || !fp.nodes.iter().any(|n| n.id == old) {
        return;
    }
    let name = fp.nav.name_by_id.get(&old).cloned();
    let kind = fp.nav.kind_by_id.get(&old).copied();

    match fp.nodes.iter().position(|n| n.id == new) {
        Some(at) => {
            let mut moved = Vec::new();
            for n in fp.nodes.iter_mut().filter(|n| n.id == old) {
                moved.append(&mut n.cells);
            }
            if let Some(target) = fp.nodes.get_mut(at) {
                target.cells.extend(moved);
            }
            fp.nodes.retain(|n| n.id != old);
        }
        None => {
            for n in fp.nodes.iter_mut().filter(|n| n.id == old) {
                n.id = new;
            }
        }
    }
    let swap = |id: &mut NodeId| {
        if *id == old {
            *id = new;
        }
    };
    for e in fp.edges.iter_mut() {
        swap(&mut e.from);
        swap(&mut e.to);
    }
    for r in fp.refs.iter_mut() {
        swap(&mut r.from);
        swap(&mut r.from_module);
    }
    for c in fp.calls.iter_mut() {
        swap(&mut c.from);
    }
    if fp.properties.remove(&old) {
        fp.properties.insert(new);
    }

    rewrite_node_id(&mut fp.nav, old, new);
    fp.nav.qname_by_id.insert(new, qname.to_string());
    if let Some(name) = name {
        fp.nav.name_by_id.entry(new).or_insert(name);
    }
    if let Some(kind) = kind {
        fp.nav.kind_by_id.entry(new).or_insert(kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{CallQualifier, CallSite, GRAPH_TYPE, UnresolvedRef, cell_type};
    use repo_graph_code_domain::{edge_category, node_kind};
    use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, RepoId};

    const REPO: RepoId = RepoId(11);

    fn id(kind: repo_graph_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, REPO, kind, qname)
    }

    fn node(id: NodeId, cell: &str) -> Node {
        Node {
            id,
            repo: REPO,
            confidence: Confidence::Strong,
            cells: vec![Cell { kind: cell_type::ROUTE_METHOD, payload: CellPayload::Text(cell.into()) }],
        }
    }

    fn edge(from: NodeId, to: NodeId, category: repo_graph_core::EdgeCategoryId) -> Edge {
        Edge { from, to, category, confidence: Confidence::Strong, cells: Vec::new() }
    }

    /// A module holding a handler, a ROUTE between two siblings, the ROUTE's
    /// HANDLED_BY as both an edge and an UnresolvedRef, and a call from the
    /// handler. Returns (fp, module, route, handler, sibling_a, sibling_b).
    fn file() -> (FileParse, [NodeId; 5]) {
        let module = id(node_kind::MODULE, "svc::app");
        let handler = id(node_kind::FUNCTION, "svc::app::health");
        let route = id(node_kind::ROUTE, "route:/health");
        let (a, b) = (id(node_kind::FUNCTION, "svc::app::a"), id(node_kind::FUNCTION, "svc::app::b"));
        let mut fp = FileParse::default();
        for (n, name, q, k) in [
            (module, "app", "svc::app", node_kind::MODULE),
            (a, "a", "svc::app::a", node_kind::FUNCTION),
            (route, "/health", "route:/health", node_kind::ROUTE),
            (b, "b", "svc::app::b", node_kind::FUNCTION),
            (handler, "health", "svc::app::health", node_kind::FUNCTION),
        ] {
            fp.nodes.push(node(n, "GET"));
            fp.nav.record(n, name, q, k, (n != module).then_some(module));
        }
        fp.edges.push(edge(module, route, edge_category::CONTAINS));
        fp.edges.push(edge(route, handler, edge_category::HANDLED_BY));
        fp.refs.push(UnresolvedRef {
            from: route,
            from_module: module,
            qualifier: CallQualifier::Bare("health".into()),
            category: edge_category::HANDLED_BY,
        });
        fp.calls.push(CallSite { from: handler, qualifier: CallQualifier::Bare("a".into()) });
        (fp, [module, route, handler, a, b])
    }

    #[test]
    fn rekey_moves_edges_refs_nav_and_keeps_sibling_order() {
        let (mut fp, [module, route, handler, a, b]) = file();
        let new = id(node_kind::ROUTE, "route:/health @svc");
        rekey_node(&mut fp, route, new, "route:/health @svc");

        let ids: Vec<NodeId> = fp.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, [module, a, new, b, handler], "node order kept, id swapped in place");
        assert_eq!(fp.edges[0], edge(module, new, edge_category::CONTAINS));
        assert_eq!(fp.edges[1], edge(new, handler, edge_category::HANDLED_BY));
        assert_eq!((fp.refs[0].from, fp.refs[0].from_module), (new, module));
        assert_eq!(fp.calls[0].from, handler, "an unrelated call is untouched");
        assert_eq!(fp.nav.qname_by_id.get(&new).map(String::as_str), Some("route:/health @svc"));
        assert_eq!(fp.nav.name_by_id.get(&new).map(String::as_str), Some("/health"), "display name kept");
        assert_eq!(fp.nav.kind_by_id.get(&new), Some(&node_kind::ROUTE));
        assert_eq!(fp.nav.parent_of.get(&new), Some(&module));
        for map_has_old in [
            fp.nav.qname_by_id.contains_key(&route),
            fp.nav.name_by_id.contains_key(&route),
            fp.nav.kind_by_id.contains_key(&route),
            fp.nav.parent_of.contains_key(&route),
        ] {
            assert!(!map_has_old, "old id retired from the nav");
        }
        assert_eq!(
            fp.nav.children_of.get(&module).map(Vec::as_slice),
            Some([a, new, b, handler].as_slice()),
            "the rekeyed child keeps its slot among its siblings"
        );
    }

    #[test]
    fn rekey_moves_calls_and_properties() {
        let (mut fp, [_, _, handler, a, _]) = file();
        fp.properties.insert(handler);
        let new = id(node_kind::FUNCTION, "svc::app::health @svc");
        rekey_node(&mut fp, handler, new, "svc::app::health @svc");
        assert_eq!(fp.calls[0].from, new);
        assert!(fp.properties.contains(&new) && !fp.properties.contains(&handler));
        assert_eq!(fp.edges[1].to, new);
        assert_ne!(fp.calls[0].from, a);
    }

    /// Repeated entries (one per TypeScript call site) all move; a `new` that
    /// already had an entry absorbs the old entries' cells, in order, and the
    /// old entries are dropped.
    #[test]
    fn rekey_merges_into_an_existing_id() {
        let (mut fp, [module, route, handler, a, b]) = file();
        let new = id(node_kind::ROUTE, "route:/health @svc");
        fp.nodes.insert(0, node(new, "POST"));
        fp.nav.record(new, "/health", "route:/health @svc", node_kind::ROUTE, Some(module));
        fp.nodes.push(node(route, "PUT"));
        rekey_node(&mut fp, route, new, "route:/health @svc");

        let ids: Vec<NodeId> = fp.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, [new, module, a, b, handler]);
        let methods: Vec<&CellPayload> = fp.nodes[0].cells.iter().map(|c| &c.payload).collect();
        assert_eq!(
            methods,
            [
                &CellPayload::Text("POST".into()),
                &CellPayload::Text("GET".into()),
                &CellPayload::Text("PUT".into())
            ]
        );
        assert_eq!(fp.edges[1].from, new);
        assert_eq!(fp.nav.parent_of.get(&new), Some(&module), "`new` keeps its own slot");
        assert_eq!(
            fp.nav.children_of.get(&module).map(Vec::as_slice),
            Some([a, b, handler, new].as_slice()),
            "`old` leaves its parent's list; `new` is not listed twice"
        );
    }

    #[test]
    fn rekey_of_an_absent_or_same_id_is_a_no_op() {
        let (mut fp, [_, route, ..]) = file();
        let before = (fp.nodes.clone(), fp.edges.clone(), fp.refs.clone());
        rekey_node(&mut fp, route, route, "route:/health");
        rekey_node(&mut fp, id(node_kind::ROUTE, "route:/nope"), route, "route:/health");
        assert_eq!((fp.nodes.clone(), fp.edges.clone(), fp.refs.clone()), before);
        assert_eq!(fp.nav.qname_by_id.get(&route).map(String::as_str), Some("route:/health"));
    }
}
