//! LC.2: edge cells survive the sharded `.gmap` layout, and the cross-stack
//! file's bytes do not depend on the order the resolvers pushed edges in once
//! same-key edges carry different cells.

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use repo_graph_graph::{MergedGraph, RepoGraph, SymbolTable};
use repo_graph_store::{CROSS_STACK_NAME, read_merged_sharded, write_merged_sharded};

fn repo_a() -> RepoId {
    RepoId::from_canonical("test://edge_cells/a")
}

fn repo_b() -> RepoId {
    RepoId::from_canonical("test://edge_cells/b")
}

fn node(repo: RepoId, kind: repo_graph_core::NodeKindId, qname: &str, nav: &mut CodeNav) -> Node {
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, qname);
    let name = qname.rsplit("::").next().unwrap_or(qname);
    nav.record(id, name, qname, kind, None);
    Node { id, repo, confidence: Confidence::Strong, cells: Vec::new() }
}

fn graph(repo: RepoId, nodes: Vec<Node>, edges: Vec<Edge>, nav: CodeNav) -> RepoGraph {
    RepoGraph {
        repo,
        nodes,
        edges,
        nav,
        symbols: SymbolTable::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    }
}

fn evidence(json: &str) -> Cell {
    Cell { kind: cell_type::EVIDENCE, payload: CellPayload::Json(json.into()) }
}

/// Server repo `a` (handler calls a helper at two call sites, the edges told
/// apart only by their cells) and client repo `b` (one ENDPOINT).
fn fixture() -> (RepoGraph, RepoGraph, NodeId, NodeId) {
    let mut nav_a = CodeNav::default();
    let handler = node(repo_a(), node_kind::FUNCTION, "srv::users::create", &mut nav_a);
    let helper = node(repo_a(), node_kind::FUNCTION, "srv::users::validate", &mut nav_a);
    let route = node(repo_a(), node_kind::ROUTE, "POST /users", &mut nav_a);
    let (h, v, r) = (handler.id, helper.id, route.id);
    let intra = vec![
        Edge::new(h, v, edge_category::CALLS, Confidence::Strong)
            .with_cell(evidence(r#"{"line":12,"emitter":"python"}"#)),
        Edge::new(h, v, edge_category::CALLS, Confidence::Strong)
            .with_cell(evidence(r#"{"line":31,"emitter":"python"}"#)),
        Edge::new(r, h, edge_category::HANDLED_BY, Confidence::Strong),
    ];
    let a = graph(repo_a(), vec![handler, helper, route], intra, nav_a);

    let mut nav_b = CodeNav::default();
    let ep = node(repo_b(), node_kind::ENDPOINT, "endpoint:POST:/users", &mut nav_b);
    let e = ep.id;
    let b = graph(repo_b(), vec![ep], Vec::new(), nav_b);
    (a, b, e, r)
}

/// Same key, cells tell them apart: three call sites of one client ENDPOINT,
/// plus a cell-less twin and a Weak one.
fn cross_edges(ep: NodeId, route: NodeId) -> Vec<Edge> {
    let base = || Edge::new(ep, route, edge_category::HTTP_CALLS, Confidence::Strong);
    vec![
        base().with_cell(Cell {
            kind: cell_type::EVIDENCE,
            payload: CellPayload::Text("web/api.ts:4".into()),
        }),
        base().with_cell(Cell {
            kind: cell_type::EVIDENCE,
            payload: CellPayload::Text("web/api.ts:40".into()),
        }),
        base().with_cell(evidence(r#"{"line":7}"#)),
        base(),
        Edge::new(ep, route, edge_category::HTTP_CALLS, Confidence::Weak),
    ]
}

#[test]
fn edge_cells_survive_sharded_round_trip() {
    let (a, b, ep, route) = fixture();
    let intra_before = a.edges.clone();
    let mut merged = MergedGraph::new(vec![a, b]);
    merged.cross_edges = cross_edges(ep, route);
    merged.sort_cross_edges();
    let cross_before = merged.cross_edges.clone();
    assert!(cross_before.iter().any(|e| !e.cells.is_empty()));

    let dir = tempfile::tempdir().unwrap();
    write_merged_sharded(&merged, dir.path()).unwrap();
    let back = read_merged_sharded(dir.path()).unwrap();

    assert_eq!(back.graphs.len(), 2);
    let a_back = back.graphs.iter().find(|g| g.repo == repo_a()).expect("repo a shard");
    assert_eq!(a_back.edges, intra_before, "intra edges, cells included");
    assert_eq!(
        a_back.edges.iter().filter(|e| e.cell(cell_type::EVIDENCE).is_some()).count(),
        2
    );
    assert_eq!(back.cross_edges, cross_before, "cross edges, cells included");
    assert_eq!(back.cross_edges.len(), 5, "same-key edges are never merged");
}

#[test]
fn cross_stack_bytes_independent_of_input_order() {
    let write = |reverse: bool, sort: bool| -> Vec<u8> {
        let (a, b, ep, route) = fixture();
        let mut merged = MergedGraph::new(vec![a, b]);
        let mut edges = cross_edges(ep, route);
        if reverse {
            edges.reverse();
        }
        merged.cross_edges = edges;
        if sort {
            merged.sort_cross_edges();
        }
        let dir = tempfile::tempdir().unwrap();
        write_merged_sharded(&merged, dir.path()).unwrap();
        std::fs::read(dir.path().join(CROSS_STACK_NAME)).unwrap()
    };
    // Control: unsorted, the two input orders write different bytes, so the
    // assertion below is not vacuous.
    assert_ne!(write(false, false), write(true, false));
    assert_eq!(write(false, true), write(true, true));
}
