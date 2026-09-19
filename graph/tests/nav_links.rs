//! LA.6a — NAVIGATES_TO resolution: frontend navigation links resolved against
//! the nav-route table, dead links kept as unresolved refs, and link / page
//! endpoints lifted from the file MODULE to its page component.
//!
//! Its own test binary (like `http_nav_route.rs`), so no sibling packet's test
//! file is shared. Every assertion compares route PATHS through
//! `nav::nav_route_path`, never a ROUTE qname literal, so the test survives a
//! change of the nav qname shape.

use std::collections::HashSet;

use repo_graph_code_domain::{
    CallQualifier, CodeNav, FileParse, GRAPH_TYPE, UnresolvedRef, cell_type, edge_category,
    node_kind,
};
use repo_graph_core::{
    Cell, CellPayload, Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId,
};
use repo_graph_graph::nav::nav_route_path;
use repo_graph_graph::{RepoGraph, build_typescript};

fn repo() -> RepoId {
    RepoId::from_canonical("test://nav_links")
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

fn edge(from: NodeId, to: NodeId, category: EdgeCategoryId) -> Edge {
    Edge {
        from,
        to,
        category,
        confidence: Confidence::Medium,
        cells: Vec::new(),
    }
}

fn parse(nodes: Vec<Node>, edges: Vec<Edge>, refs: Vec<UnresolvedRef>, nav: CodeNav) -> FileParse {
    FileParse {
        nodes,
        edges,
        imports: vec![],
        calls: vec![],
        refs,
        nav,
        properties: HashSet::new(),
    }
}

/// The ORIGIN cell the client-router extractors stamp on a nav ROUTE (A3.4),
/// spelled as a literal so a drift in the extractors' payload fails here.
fn nav_origin() -> Cell {
    Cell {
        kind: cell_type::ORIGIN,
        payload: CellPayload::Json(r#"{"provenance":"nav_route"}"#.into()),
    }
}

fn route_method() -> Cell {
    Cell {
        kind: cell_type::ROUTE_METHOD,
        payload: CellPayload::Text("GET".into()),
    }
}

/// LB.3's ROLE cell naming the COMPONENT stereotype.
fn component_role() -> Cell {
    Cell {
        kind: cell_type::ROLE,
        payload: CellPayload::Json(r#"{"roles":["COMPONENT"]}"#.into()),
    }
}

/// A nav ROUTE in the shape `react.rs` emits since LB.4c: `page:<path>`,
/// display name `<path>`, no nav parent.
fn page_id(path: &str) -> NodeId {
    id(node_kind::ROUTE, &format!("page:{path}"))
}

fn push_page(nodes: &mut Vec<Node>, nav: &mut CodeNav, path: &str) -> NodeId {
    let pid = page_id(path);
    nodes.push(node(pid, vec![route_method(), nav_origin()]));
    nav.record(pid, path, &format!("page:{path}"), node_kind::ROUTE, None);
    pid
}

fn link(from: NodeId, from_module: NodeId, target: &str) -> UnresolvedRef {
    UnresolvedRef {
        from,
        from_module,
        qualifier: CallQualifier::Bare(target.into()),
        category: edge_category::NAVIGATES_TO,
        line: 0 ,
    }
}

const LINKS: &[&str] = &[
    "/dashboard",
    "/users/${...}",
    "/users/42",
    "/docs/intro/start",
    "/billing",
    "/auth/google",
    "href:/logout",
];

fn app_module() -> NodeId {
    id(node_kind::MODULE, "src::App")
}
fn nav_fn() -> NodeId {
    id(node_kind::FUNCTION, "src::App::Nav")
}
fn dashboard_module() -> NodeId {
    id(node_kind::MODULE, "src::Dashboard")
}
fn dashboard_fn() -> NodeId {
    id(node_kind::FUNCTION, "src::Dashboard::Dashboard")
}
fn orphan() -> NodeId {
    id(node_kind::MODULE, "src::Gone")
}

/// `src/App.tsx`: the router table, a server ROUTE, and `Nav` — a FUNCTION
/// plus the COMPONENT overlay `react.rs` mints over it (LB.3 folds it into a
/// ROLE cell), recorded twice under the module the way two scans record it.
fn app_file(with_router: bool) -> FileParse {
    let (m, nav_id, overlay) = (
        app_module(),
        nav_fn(),
        id(node_kind::COMPONENT, "src::App::Nav"),
    );
    let mut nav = CodeNav::default();
    nav.record(m, "App", "src::App", node_kind::MODULE, None);
    nav.record(nav_id, "Nav", "src::App::Nav", node_kind::FUNCTION, Some(m));
    nav.record(
        overlay,
        "Nav",
        "src::App::Nav",
        node_kind::COMPONENT,
        Some(m),
    );
    nav.children_of.entry(m).or_default().push(nav_id);
    let mut nodes = vec![node(m, vec![]), node(nav_id, vec![]), node(overlay, vec![])];
    if with_router {
        for path in ["/dashboard", "/users/:id", "/docs/:slug*", "/**"] {
            push_page(&mut nodes, &mut nav, path);
        }
    }
    // A same-repo server route (ts_routes' `route:<path>` shape, no ORIGIN).
    let server = id(node_kind::ROUTE, "route:/auth/google");
    nodes.push(node(server, vec![route_method()]));
    nav.record(
        server,
        "/auth/google",
        "route:/auth/google",
        node_kind::ROUTE,
        None,
    );

    let mut refs: Vec<UnresolvedRef> = LINKS.iter().map(|l| link(m, m, l)).collect();
    refs.push(link(orphan(), orphan(), "/dashboard"));
    parse(
        nodes,
        vec![edge(m, nav_id, edge_category::DEFINES)],
        refs,
        nav,
    )
}

/// `src/Dashboard.tsx`: one page component, and the route table's
/// `/dashboard -> Dashboard` binding at file granularity (HANDLED_BY to the
/// MODULE) that the lift must move onto the component.
fn dashboard_file(with_router: bool) -> FileParse {
    let (m, f) = (dashboard_module(), dashboard_fn());
    let mut nav = CodeNav::default();
    nav.record(m, "Dashboard", "src::Dashboard", node_kind::MODULE, None);
    nav.record(
        f,
        "Dashboard",
        "src::Dashboard::Dashboard",
        node_kind::FUNCTION,
        Some(m),
    );
    let mut edges = vec![edge(m, f, edge_category::DEFINES)];
    if with_router {
        edges.push(edge(page_id("/dashboard"), m, edge_category::HANDLED_BY));
    }
    parse(
        vec![node(m, vec![]), node(f, vec![component_role()])],
        edges,
        vec![],
        nav,
    )
}

fn build(parses: Vec<FileParse>) -> RepoGraph {
    build_typescript(repo(), parses, |_, _| None).expect("build")
}

fn app() -> RepoGraph {
    build(vec![app_file(true), dashboard_file(true)])
}

fn nav_edges(g: &RepoGraph) -> Vec<&Edge> {
    g.edges
        .iter()
        .filter(|e| e.category == edge_category::NAVIGATES_TO)
        .collect()
}

fn nav_refs(g: &RepoGraph) -> Vec<&UnresolvedRef> {
    g.unresolved_refs
        .iter()
        .filter(|r| r.category == edge_category::NAVIGATES_TO)
        .collect()
}

/// The request path of a ROUTE node, read back through `nav_route_path`.
fn path_of(g: &RepoGraph, route: NodeId) -> Option<String> {
    g.nav
        .qname_by_id
        .get(&route)
        .and_then(|q| nav_route_path(q))
        .map(String::from)
}

#[test]
fn links_resolve_by_tier() {
    let g = app();
    let got: Vec<(NodeId, Option<String>, Confidence)> = nav_edges(&g)
        .iter()
        .map(|e| (e.from, path_of(&g, e.to), e.confidence))
        .collect();
    let want = vec![
        (nav_fn(), Some("/dashboard".to_string()), Confidence::Strong),
        (nav_fn(), Some("/users/:id".to_string()), Confidence::Medium),
        (
            nav_fn(),
            Some("/docs/:slug*".to_string()),
            Confidence::Medium,
        ),
    ];
    assert_eq!(
        got, want,
        "exactly the exact, param and scoped hits, from the page component"
    );
}

#[test]
fn unmatched_router_link_is_kept_as_dead() {
    let g = app();
    let dead: Vec<(NodeId, &CallQualifier)> = nav_refs(&g)
        .iter()
        .map(|r| (r.from, &r.qualifier))
        .collect();
    assert_eq!(
        dead,
        vec![(nav_fn(), &CallQualifier::Bare("/billing".into()))],
        "one dead link, lifted to the page component"
    );
    let root = page_id("/**");
    assert!(
        g.edges.iter().all(|e| e.to != root),
        "the root catch-all must absorb nothing"
    );
}

#[test]
fn server_route_and_plain_href_are_never_dead() {
    let g = app();
    let server = id(node_kind::ROUTE, "route:/auth/google");
    assert!(nav_edges(&g).iter().all(|e| e.to != server));
    for gone in ["/auth/google", "href:/logout"] {
        let q = CallQualifier::Bare(gone.into());
        assert!(
            nav_refs(&g).iter().all(|r| r.qualifier != q),
            "{gone} must be neither an edge nor a dead ref"
        );
    }
}

#[test]
fn no_router_drops_links() {
    let g = build(vec![app_file(false), dashboard_file(false)]);
    assert!(nav_edges(&g).is_empty(), "no nav ROUTE, no link edge");
    assert!(
        nav_refs(&g).is_empty(),
        "a repo with no router has no judgeable dead link"
    );
}

#[test]
fn orphan_ref_is_dropped() {
    let g = app();
    assert!(g.edges.iter().all(|e| e.from != orphan()));
    assert!(g.unresolved_refs.iter().all(|r| r.from != orphan()));
}

#[test]
fn nav_route_handled_by_module_lifts_to_component() {
    let g = app();
    let dashboard = page_id("/dashboard");
    let handled: Vec<NodeId> = g
        .edges
        .iter()
        .filter(|e| e.category == edge_category::HANDLED_BY && e.from == dashboard)
        .map(|e| e.to)
        .collect();
    assert_eq!(handled, vec![dashboard_fn()], "the page, not the file");

    // A file with TWO page components is ambiguous: its module stays.
    let (m, a, b) = (
        id(node_kind::MODULE, "src::Pages"),
        id(node_kind::FUNCTION, "src::Pages::A"),
        id(node_kind::FUNCTION, "src::Pages::B"),
    );
    let mut nav = CodeNav::default();
    nav.record(m, "Pages", "src::Pages", node_kind::MODULE, None);
    nav.record(a, "A", "src::Pages::A", node_kind::FUNCTION, Some(m));
    nav.record(b, "B", "src::Pages::B", node_kind::FUNCTION, Some(m));
    let mut nodes = vec![
        node(m, vec![]),
        node(a, vec![component_role()]),
        node(b, vec![component_role()]),
    ];
    let users = push_page(&mut nodes, &mut nav, "/users");
    let pages = parse(
        nodes,
        vec![edge(users, m, edge_category::HANDLED_BY)],
        vec![link(m, m, "/users")],
        nav,
    );
    let g = build(vec![pages]);
    let handled: Vec<NodeId> = g
        .edges
        .iter()
        .filter(|e| e.category == edge_category::HANDLED_BY)
        .map(|e| e.to)
        .collect();
    assert_eq!(handled, vec![m]);
    let froms: Vec<NodeId> = nav_edges(&g).iter().map(|e| e.from).collect();
    assert_eq!(
        froms,
        vec![m],
        "an ambiguous file keeps the link at the module"
    );
}

#[test]
fn navigates_to_carries_blast_radius() {
    assert!(repo_graph_code_domain::profile::CODE_TABLES.carries(edge_category::NAVIGATES_TO));
}

#[test]
fn owner_prefers_the_linking_files_project() {
    // Two SPAs in one repo (LB.4a owner segments). A link from a file under
    // `apps/web` binds to web's `/users`, not admin's.
    let (m, f) = (
        id(node_kind::MODULE, "apps::web::src::App"),
        id(node_kind::FUNCTION, "apps::web::src::App::App"),
    );
    let mut nav = CodeNav::default();
    nav.record(m, "App", "apps::web::src::App", node_kind::MODULE, None);
    nav.record(
        f,
        "App",
        "apps::web::src::App::App",
        node_kind::FUNCTION,
        Some(m),
    );
    let position = Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(
            r#"{"file":"apps/web/src/App.tsx","start_line":0,"end_line":9}"#.into(),
        ),
    };
    let mut nodes = vec![node(m, vec![position]), node(f, vec![component_role()])];
    let admin = push_page(&mut nodes, &mut nav, "/users @apps/admin");
    let web = push_page(&mut nodes, &mut nav, "/users @apps/web");
    let g = build(vec![parse(nodes, vec![], vec![link(m, m, "/users")], nav)]);
    let to: Vec<NodeId> = nav_edges(&g).iter().map(|e| e.to).collect();
    assert_eq!(to, vec![web]);
    assert_ne!(web, admin);
}

#[test]
fn build_is_deterministic() {
    let a = app();
    let b = app();
    assert_eq!(a.edges, b.edges);
    assert_eq!(a.unresolved_refs, b.unresolved_refs);
}
