//! `MergedGraph` traversal (LD.3a): neighbours in either direction, BFS,
//! predecessors, reachable_by and shortest_path over the intra-repo edges AND
//! `cross_edges`, so a walk from a client function crosses HTTP_CALLS into the
//! repo that serves it.
//!
//! The graph: repo A `client -CALLS-> endpoint`, repo B
//! `route -HANDLED_BY-> handler -CALLS-> helper`, and the cross edge
//! `endpoint -HTTP_CALLS-> route` an HTTP resolver would emit.

use std::collections::HashSet;

use repo_graph_activation::algo::reach::Reached;
use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId};
use repo_graph_graph::{MergedGraph, Reach, RepoGraph, SymbolTable};

const CALLS: EdgeCategoryId = edge_category::CALLS;
const HTTP_CALLS: EdgeCategoryId = edge_category::HTTP_CALLS;
const HANDLED_BY: EdgeCategoryId = edge_category::HANDLED_BY;

fn repo_a() -> RepoId {
    RepoId::from_canonical("test://merged-traversal/a")
}

fn repo_b() -> RepoId {
    RepoId::from_canonical("test://merged-traversal/b")
}

fn id(repo: RepoId, kind: NodeKindId, qname: &str) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, repo, kind, qname)
}

fn edge(from: NodeId, to: NodeId, category: EdgeCategoryId) -> Edge {
    Edge { from, to, category, confidence: Confidence::Strong, cells: Vec::new() }
}

/// A repo graph of `nodes` (qname, kind) and `edges`, nav recorded.
fn graph(repo: RepoId, nodes: &[(&str, NodeKindId)], edges: Vec<Edge>) -> RepoGraph {
    let mut nav = CodeNav::default();
    let nodes = nodes
        .iter()
        .map(|&(qname, kind)| {
            let nid = id(repo, kind, qname);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            nav.record(nid, name, qname, kind, None);
            Node { id: nid, repo, confidence: Confidence::Strong, cells: vec![] }
        })
        .collect();
    RepoGraph {
        repo,
        nodes,
        edges,
        nav,
        symbols: SymbolTable::default(),
        unresolved_calls: vec![],
        unresolved_refs: vec![],
        properties: HashSet::new(),
    }
}

struct Stack {
    m: MergedGraph,
    client: NodeId,
    endpoint: NodeId,
    route: NodeId,
    handler: NodeId,
    helper: NodeId,
}

fn stack() -> Stack {
    let client = id(repo_a(), node_kind::FUNCTION, "web::api::load_orders");
    let endpoint = id(repo_a(), node_kind::ENDPOINT, "endpoint:GET:/orders");
    let route = id(repo_b(), node_kind::ROUTE, "route:GET:/orders");
    let handler = id(repo_b(), node_kind::FUNCTION, "api::orders::list");
    let helper = id(repo_b(), node_kind::FUNCTION, "api::orders::fetch_rows");
    let a = graph(
        repo_a(),
        &[("web::api::load_orders", node_kind::FUNCTION), ("endpoint:GET:/orders", node_kind::ENDPOINT)],
        vec![edge(client, endpoint, CALLS)],
    );
    let b = graph(
        repo_b(),
        &[
            ("route:GET:/orders", node_kind::ROUTE),
            ("api::orders::list", node_kind::FUNCTION),
            ("api::orders::fetch_rows", node_kind::FUNCTION),
        ],
        vec![edge(route, handler, HANDLED_BY), edge(handler, helper, CALLS)],
    );
    let mut m = MergedGraph::new(vec![a, b]);
    m.cross_edges.push(edge(endpoint, route, HTTP_CALLS));
    Stack { m, client, endpoint, route, handler, helper }
}

fn rows(r: &[Reached]) -> Vec<(NodeId, usize, EdgeCategoryId, NodeId)> {
    r.iter().map(|r| (r.id, r.depth, r.via, r.parent)).collect()
}

#[test]
fn incoming_neighbours_include_cross_edges() {
    let s = stack();
    assert_eq!(s.m.neighbours(s.route, Reach::Backward, None), vec![(s.endpoint, HTTP_CALLS, Reach::Backward)]);
    // Forward from the endpoint crosses into repo B.
    assert_eq!(s.m.neighbours(s.endpoint, Reach::Forward, None), vec![(s.route, HTTP_CALLS, Reach::Forward)]);
    // Both: global edge order (graphs in Vec order, then cross_edges), each
    // row tagged with the way it was walked.
    assert_eq!(
        s.m.neighbours(s.route, Reach::Both, None),
        vec![(s.handler, HANDLED_BY, Reach::Forward), (s.endpoint, HTTP_CALLS, Reach::Backward)]
    );
    // A category filter drops the cross edge.
    assert_eq!(
        s.m.neighbours(s.route, Reach::Both, Some(&[HANDLED_BY])),
        vec![(s.handler, HANDLED_BY, Reach::Forward)]
    );
    assert!(s.m.neighbours(s.route, Reach::Both, Some(&[])).is_empty());
}

#[test]
fn a_self_loop_is_one_forward_neighbour_under_both() {
    let mut s = stack();
    s.m.cross_edges.push(edge(s.helper, s.helper, CALLS));
    assert_eq!(
        s.m.neighbours(s.helper, Reach::Both, None),
        vec![(s.handler, CALLS, Reach::Backward), (s.helper, CALLS, Reach::Forward)]
    );
    assert_eq!(
        s.m.neighbours(s.helper, Reach::Backward, None),
        vec![(s.handler, CALLS, Reach::Backward), (s.helper, CALLS, Reach::Backward)]
    );
}

#[test]
fn bfs_crosses_repos() {
    let s = stack();
    let got = s.m.bfs(&[s.client], Reach::Forward, None, 10);
    assert_eq!(
        rows(&got),
        vec![
            (s.endpoint, 1, CALLS, s.client),
            (s.route, 2, HTTP_CALLS, s.endpoint),
            (s.handler, 3, HANDLED_BY, s.route),
            (s.helper, 4, CALLS, s.handler),
        ]
    );
    // max_depth bounds the walk; the seed is never reached.
    let two: Vec<NodeId> = s.m.bfs(&[s.client], Reach::Forward, None, 2).iter().map(|r| r.id).collect();
    assert_eq!(two, vec![s.endpoint, s.route]);
    assert!(s.m.bfs(&[s.client], Reach::Forward, None, 0).is_empty());
    // Backward from the helper walks the same chain the other way.
    let back: Vec<NodeId> = s.m.bfs(&[s.helper], Reach::Backward, None, 10).iter().map(|r| r.id).collect();
    assert_eq!(back, vec![s.handler, s.route, s.endpoint, s.client]);
}

#[test]
fn predecessors_cross_repos() {
    let s = stack();
    assert_eq!(s.m.predecessors(s.helper, None, 10), vec![s.handler, s.route, s.endpoint, s.client]);
    assert_eq!(s.m.predecessors(s.helper, Some(&[CALLS, HANDLED_BY]), 10), vec![s.handler, s.route]);
}

#[test]
fn reachable_by_keeps_sources_order() {
    let s = stack();
    let stray = id(repo_a(), node_kind::FUNCTION, "web::api::never_built");
    assert_eq!(s.m.reachable_by(s.helper, &[s.client, stray], None, 10), vec![s.client]);
    assert_eq!(s.m.reachable_by(s.helper, &[s.route, s.client], None, 10), vec![s.route, s.client]);
    // Out of reach within 2 hops; the HTTP hop is filtered out.
    assert!(s.m.reachable_by(s.helper, &[s.client], None, 2).is_empty());
    assert!(s.m.reachable_by(s.helper, &[s.client], Some(&[CALLS, HANDLED_BY]), 10).is_empty());
    assert!(s.m.reachable_by(s.helper, &[], None, 10).is_empty());
}

#[test]
fn shortest_path_crosses_repos() {
    let s = stack();
    let forward = vec![
        (s.client, None),
        (s.endpoint, Some(CALLS)),
        (s.route, Some(HTTP_CALLS)),
        (s.handler, Some(HANDLED_BY)),
        (s.helper, Some(CALLS)),
    ];
    assert_eq!(s.m.shortest_path(s.client, s.helper, Reach::Forward, None, 10), Some(forward.clone()));
    assert_eq!(s.m.shortest_path(s.helper, s.client, Reach::Forward, None, 10), None);
    // Both: the reversed node sequence; each step carries the category of
    // the edge it was entered by.
    assert_eq!(
        s.m.shortest_path(s.helper, s.client, Reach::Both, None, 10),
        Some(vec![
            (s.helper, None),
            (s.handler, Some(CALLS)),
            (s.route, Some(HANDLED_BY)),
            (s.endpoint, Some(HTTP_CALLS)),
            (s.client, Some(CALLS)),
        ])
    );
    assert_eq!(s.m.shortest_path(s.helper, s.client, Reach::Backward, None, 10).map(|p| p.len()), Some(5));
    // Four hops do not fit in three.
    assert_eq!(s.m.shortest_path(s.client, s.helper, Reach::Forward, None, 3), None);
    assert_eq!(s.m.shortest_path(s.client, s.helper, Reach::Forward, None, 4), Some(forward));
    // A category filter that drops the HTTP hop disconnects the repos.
    assert_eq!(s.m.shortest_path(s.client, s.helper, Reach::Both, Some(&[CALLS, HANDLED_BY]), 10), None);
    // from == to is the zero-hop path.
    assert_eq!(s.m.shortest_path(s.route, s.route, Reach::Forward, None, 0), Some(vec![(s.route, None)]));
}

#[test]
fn shortest_path_takes_the_fewer_hops() {
    let mut s = stack();
    // A direct client -> helper shortcut, cross-repo, added last in edge order.
    s.m.cross_edges.push(edge(s.client, s.helper, CALLS));
    assert_eq!(
        s.m.shortest_path(s.client, s.helper, Reach::Forward, None, 10),
        Some(vec![(s.client, None), (s.helper, Some(CALLS))])
    );
}

#[test]
fn a_cross_edge_to_no_node_is_still_followed() {
    let mut s = stack();
    // An endpoint of an edge need not be a node (LD.15a parity rule 1).
    let ghost = id(repo_b(), node_kind::FUNCTION, "api::orders::unbuilt");
    s.m.cross_edges.push(edge(s.helper, ghost, CALLS));
    let ids: Vec<NodeId> = s.m.bfs(&[s.client], Reach::Forward, None, 10).iter().map(|r| r.id).collect();
    assert_eq!(ids.last(), Some(&ghost));
    assert_eq!(s.m.shortest_path(s.client, ghost, Reach::Forward, None, 10).map(|p| p.len()), Some(6));
    assert_eq!(s.m.neighbours(ghost, Reach::Backward, None), vec![(s.helper, CALLS, Reach::Backward)]);
}

#[test]
fn category_filter_limits_bfs() {
    let s = stack();
    let ids: Vec<NodeId> = s.m.bfs(&[s.client], Reach::Forward, Some(&[CALLS]), 10).iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![s.endpoint]);
}

/// What HEAD offered: per-repo walks only. They cannot see `cross_edges`, so
/// none of them crosses from the client's repo into the server's.
#[test]
fn head_contrast() {
    let s = stack();
    let b = &s.m.graphs[1];
    let outgoing = b.neighbours(s.route, Reach::Forward);
    assert_eq!(outgoing, vec![(s.handler, HANDLED_BY, Reach::Forward)]);
    assert!(outgoing.iter().all(|&(other, _, _)| other != s.endpoint));
    // The incoming HTTP_CALLS edge lives in cross_edges, not in repo B.
    assert!(b.neighbours(s.route, Reach::Backward).is_empty());
    assert_eq!(
        b.neighbours(s.handler, Reach::Both),
        vec![(s.route, HANDLED_BY, Reach::Backward), (s.helper, CALLS, Reach::Forward)]
    );
    let a = &s.m.graphs[0];
    assert_eq!(a.bfs(s.client, &[CALLS, HTTP_CALLS, HANDLED_BY], 10), vec![s.endpoint]);
}
