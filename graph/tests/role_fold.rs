//! LB.3a — the build-time role fold, end to end through `build_typescript`.
//!
//! Hand-built parses in the exact shape the engine hands the builder for an
//! Angular pair: the parser's CLASS, then the extractor overlays sharing its
//! qname (SERVICE recorded twice — `angular.rs` and `services.rs` both mint
//! it), `services.rs`' CONTAINS duplicates of the class's DEFINES, and an
//! INJECTS ref naming the service by its bare name.

use std::collections::HashSet;

use repo_graph_code_domain::{
    CallQualifier, CodeNav, FileParse, GRAPH_TYPE, UnresolvedRef, cell_type, edge_category,
    node_kind,
};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, NodeKindId, RepoId};
use repo_graph_graph::roles::{ROLE_KINDS, roles_in};
use repo_graph_graph::{MergedGraph, RepoGraph, build_typescript};

fn repo() -> RepoId {
    RepoId::from_canonical("test://role_fold")
}

fn id(kind: NodeKindId, qname: &str) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
}

fn imports_cell() -> Cell {
    Cell {
        kind: cell_type::IMPORTS,
        payload: CellPayload::Json(r#"["@angular/core"]"#.into()),
    }
}

fn node(id: NodeId, cells: Vec<Cell>) -> Node {
    Node {
        id,
        repo: repo(),
        confidence: Confidence::Strong,
        cells,
    }
}

fn edge(from: NodeId, to: NodeId, category: repo_graph_core::EdgeCategoryId) -> Edge {
    Edge {
        from,
        to,
        category,
        confidence: Confidence::Strong,
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

/// `m.ts`: `@Injectable() class UserService { load() {} }`.
fn service_file() -> FileParse {
    let (m, class, load, svc) = (
        id(node_kind::MODULE, "m"),
        id(node_kind::CLASS, "m::UserService"),
        id(node_kind::METHOD, "m::UserService::load"),
        id(node_kind::SERVICE, "m::UserService"),
    );
    let mut nav = CodeNav::default();
    nav.record(m, "m", "m", node_kind::MODULE, None);
    nav.record(
        class,
        "UserService",
        "m::UserService",
        node_kind::CLASS,
        Some(m),
    );
    nav.record(
        load,
        "load",
        "m::UserService::load",
        node_kind::METHOD,
        Some(class),
    );
    nav.record(
        svc,
        "UserService",
        "m::UserService",
        node_kind::SERVICE,
        Some(m),
    );
    nav.children_of.entry(m).or_default().push(svc);
    parse(
        vec![
            node(m, vec![]),
            node(class, vec![imports_cell()]),
            node(load, vec![imports_cell()]),
            node(svc, vec![imports_cell()]),
        ],
        vec![
            edge(m, class, edge_category::DEFINES),
            edge(class, load, edge_category::DEFINES),
            edge(svc, load, edge_category::CONTAINS),
        ],
        vec![],
        nav,
    )
}

/// `n.ts`: `@Component() class UsersComponent { constructor(s: UserService) }`.
fn component_file() -> FileParse {
    let (n, class, comp) = (
        id(node_kind::MODULE, "n"),
        id(node_kind::CLASS, "n::UsersComponent"),
        id(node_kind::COMPONENT, "n::UsersComponent"),
    );
    let mut nav = CodeNav::default();
    nav.record(n, "n", "n", node_kind::MODULE, None);
    nav.record(
        class,
        "UsersComponent",
        "n::UsersComponent",
        node_kind::CLASS,
        Some(n),
    );
    nav.record(
        comp,
        "UsersComponent",
        "n::UsersComponent",
        node_kind::COMPONENT,
        Some(n),
    );
    parse(
        vec![node(n, vec![]), node(class, vec![]), node(comp, vec![])],
        vec![edge(n, class, edge_category::DEFINES)],
        vec![UnresolvedRef {
            from: class,
            from_module: n,
            qualifier: CallQualifier::Bare("UserService".into()),
            category: edge_category::INJECTS,
        }],
        nav,
    )
}

fn build(parses: Vec<FileParse>) -> RepoGraph {
    build_typescript(repo(), parses, |_, _| None).expect("build")
}

fn role_payload(g: &RepoGraph, of: NodeId) -> Vec<&CellPayload> {
    g.nodes
        .iter()
        .filter(|n| n.id == of)
        .flat_map(|n| {
            n.cells
                .iter()
                .filter(|c| c.kind == cell_type::ROLE)
                .map(|c| &c.payload)
        })
        .collect()
}

#[test]
fn overlay_folds_into_its_declaration() {
    let g = build(vec![service_file(), component_file()]);
    let module_m = id(node_kind::MODULE, "m");
    let service_class = id(node_kind::CLASS, "m::UserService");
    let component_class = id(node_kind::CLASS, "n::UsersComponent");

    // One node per declaration: the CLASS survives with its NodeId.
    let merged = MergedGraph::new(vec![build(vec![service_file(), component_file()])]);
    assert_eq!(merged.qnames_exact("m::UserService"), vec![service_class]);
    assert_eq!(
        merged.qnames_exact("n::UsersComponent"),
        vec![component_class]
    );
    for n in &g.nodes {
        let kind = g.nav.kind_by_id.get(&n.id).copied();
        assert!(
            kind.is_some_and(|k| !ROLE_KINDS.contains(&k)),
            "no role-kind node may survive the fold: {:?}",
            g.nav.qname_by_id.get(&n.id)
        );
    }

    // The overlay kind rides on the declaration as ONE ROLE cell.
    assert_eq!(
        role_payload(&g, service_class),
        vec![&CellPayload::Json(r#"{"roles":["SERVICE"]}"#.into())]
    );
    assert_eq!(
        role_payload(&g, component_class),
        vec![&CellPayload::Json(r#"{"roles":["COMPONENT"]}"#.into())]
    );
    let class_node = g
        .nodes
        .iter()
        .find(|n| n.id == service_class)
        .expect("class node");
    assert_eq!(
        roles_in(Some(node_kind::CLASS), &class_node.cells),
        vec![node_kind::SERVICE]
    );
    assert_eq!(
        class_node
            .cells
            .iter()
            .filter(|c| c.kind == cell_type::IMPORTS)
            .count(),
        1,
        "an overlay cell equal to one the class carries is not duplicated"
    );

    // INJECTS binds the located CLASS, not an edgeless marker.
    let injects: Vec<(NodeId, NodeId)> = g
        .edges
        .iter()
        .filter(|e| e.category == edge_category::INJECTS)
        .map(|e| (e.from, e.to))
        .collect();
    assert_eq!(injects, vec![(component_class, service_class)]);

    // services.rs' CONTAINS duplicated DEFINES: gone, not moved.
    assert!(
        !g.edges
            .iter()
            .any(|e| e.category == edge_category::CONTAINS),
        "no CONTAINS survives: {:?}",
        g.edges
    );

    // rC1 / A6.2a: the survivor keys the symbol tables.
    let load = id(node_kind::METHOD, "m::UserService::load");
    assert_eq!(
        g.symbols.class_methods[&service_class].get("load"),
        Some(&load)
    );
    assert_eq!(
        g.symbols.module_symbols[&module_m].get("UserService"),
        Some(&service_class)
    );
    assert_eq!(g.nav.children_of[&module_m], vec![service_class]);
}

#[test]
fn standalone_overlay_is_kept() {
    // A Vue SFC component: the file names it, no parser declaration shares it.
    let (m, comp) = (
        id(node_kind::MODULE, "components::UserCard"),
        id(node_kind::COMPONENT, "components::UserCard::UserCard"),
    );
    let mut nav = CodeNav::default();
    nav.record(
        m,
        "UserCard",
        "components::UserCard",
        node_kind::MODULE,
        None,
    );
    nav.record(
        comp,
        "UserCard",
        "components::UserCard::UserCard",
        node_kind::COMPONENT,
        Some(m),
    );
    let g = build(vec![parse(
        vec![node(m, vec![]), node(comp, vec![])],
        vec![],
        vec![],
        nav,
    )]);

    assert_eq!(
        g.nodes.iter().map(|n| n.id).collect::<Vec<_>>(),
        vec![m, comp]
    );
    assert_eq!(g.nav.kind_by_id[&comp], node_kind::COMPONENT);
    assert!(
        role_payload(&g, comp).is_empty(),
        "a standalone role node carries no ROLE cell"
    );
    let n = g.nodes.iter().find(|n| n.id == comp).expect("component");
    assert_eq!(
        roles_in(Some(node_kind::COMPONENT), &n.cells),
        vec![node_kind::COMPONENT]
    );
}

#[test]
fn fold_is_deterministic() {
    let a = build(vec![service_file(), component_file()]);
    let b = build(vec![service_file(), component_file()]);
    assert_eq!(a.nodes, b.nodes);
    assert_eq!(a.edges, b.edges);
    assert!(!a.nodes.is_empty() && !a.edges.is_empty());
}
