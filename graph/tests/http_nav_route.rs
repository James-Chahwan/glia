//! A3.4 — client-side NAV routes are marked at extraction and excluded from
//! the HTTP route index.
//!
//! Lives in its own test binary rather than `stack_resolvers.rs` because that
//! file was being edited by a sibling packet in the same working tree, and a
//! `git commit --only` on it would have swept their tests into this commit.

use repo_graph_code_domain::{CodeNav, cell_type, edge_category, node_kind, GRAPH_TYPE};
use repo_graph_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};
use repo_graph_graph::*;

fn repo_a() -> RepoId {
    RepoId::from_canonical("test://nav-route/a")
}
fn repo_b() -> RepoId {
    RepoId::from_canonical("test://nav-route/b")
}

fn make_graph(repo: RepoId, nodes: Vec<Node>, nav: CodeNav) -> RepoGraph {
    RepoGraph {
        repo,
        nodes,
        edges: vec![],
        nav,
        symbols: Default::default(),
        unresolved_calls: vec![],
        unresolved_refs: vec![],
        properties: Default::default(),
    }
}

fn make_node(
    repo: RepoId,
    kind: repo_graph_core::NodeKindId,
    qname: &str,
    confidence: Confidence,
) -> (Node, NodeId) {
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, qname);
    let node = Node {
        id,
        repo,
        confidence,
        cells: vec![],
    };
    (node, id)
}

fn record(nav: &mut CodeNav, id: NodeId, name: &str, qname: &str, kind: repo_graph_core::NodeKindId) {
    nav.record(id, name, qname, kind, None);
}

/// The ORIGIN cell a client-router extractor stamps on a browser navigation
/// ROUTE (A3.4). Spelled here as a literal rather than imported, so this test
/// fails loudly if the payload the extractors write ever drifts away from the
/// one `graph::nav::is_nav_route` matches.
fn nav_route_cell() -> Cell {
    Cell {
        kind: cell_type::ORIGIN,
        payload: CellPayload::Json(r#"{"provenance":"nav_route"}"#.to_string()),
    }
}

// ============================================================================
// HttpStackResolver — nav-route precision (A3.4)
// ============================================================================

#[test]
fn nav_marked_route_is_not_indexed() {
    // A3.4 — THE false-pairing case. react-router / Angular Router / vue-router
    // / go_router all mint a ROUTE with the legacy `<METHOD> <path>` qname, so
    // `index_route_node` cannot tell an SPA's navigation table from a Spring or
    // Express server route. A same-repo `fetch('/dashboard')` then pairs to the
    // app's own nav entry and HttpStackResolver emits a phantom HTTP_CALLS edge
    // — a blast_radius carry edge and a weight-5.0 PPR edge.
    //
    // Two ROUTE nodes with the SAME qname, one marked. Exactly one edge must
    // survive, and it must target the unmarked (server) route.
    let mut nav_a = CodeNav::default();
    let (mut nav_route, nav_route_id) = make_node(
        repo_a(),
        node_kind::ROUTE,
        "GET /dashboard",
        Confidence::Medium,
    );
    nav_route.cells.push(nav_route_cell());
    record(
        &mut nav_a,
        nav_route_id,
        "GET /dashboard",
        "GET /dashboard",
        node_kind::ROUTE,
    );
    let (endpoint_node, endpoint_id) = make_node(
        repo_a(),
        node_kind::ENDPOINT,
        "endpoint:GET:/dashboard",
        Confidence::Medium,
    );
    record(
        &mut nav_a,
        endpoint_id,
        "GET /dashboard",
        "endpoint:GET:/dashboard",
        node_kind::ENDPOINT,
    );
    let ga = make_graph(repo_a(), vec![nav_route, endpoint_node], nav_a);

    // A real server route, same method + path, in another repo. Unmarked.
    let mut nav_b = CodeNav::default();
    let (server_route, server_route_id) = make_node(
        repo_b(),
        node_kind::ROUTE,
        "GET /dashboard",
        Confidence::Strong,
    );
    record(
        &mut nav_b,
        server_route_id,
        "GET /dashboard",
        "GET /dashboard",
        node_kind::ROUTE,
    );
    let gb = make_graph(repo_b(), vec![server_route], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    HttpStackResolver.resolve(&mut merged);

    let http_edges: Vec<_> = merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::HTTP_CALLS)
        .collect();
    assert_eq!(
        http_edges.len(),
        1,
        "the nav-marked ROUTE must not be an HTTP_CALLS target, got {:?}",
        merged.cross_edges
    );
    assert_eq!(http_edges[0].from, endpoint_id);
    assert_eq!(
        http_edges[0].to, server_route_id,
        "the surviving edge must target the unmarked server route"
    );
    // The nav ROUTE is MARKED, never removed — `where is /dashboard rendered?`
    // stays answerable.
    assert!(
        merged.graphs[0].nodes.iter().any(|n| n.id == nav_route_id),
        "the nav ROUTE node itself must survive"
    );
}
