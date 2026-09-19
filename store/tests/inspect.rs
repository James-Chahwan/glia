//! LC.4 — the self-describing header and the domain-free reader.
//!
//! `Header::for_code` fills its registries from code-domain's `ALL` tables, and
//! `inspect_path` names a file's ids from that file's header alone. The toy
//! tests reuse id 1 on purpose: in code-domain it is MODULE / DEFINES / CODE,
//! so any accidental code-domain lookup inside `inspect` names the wrong thing.

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{
    Cell, CellPayload, CellTypeId, Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId,
    RepoId,
};
use repo_graph_store::{
    Container, Header, NamedCount, RegistryEntry, StoreError, inspect_path, write_container,
};

fn entries(pairs: impl Iterator<Item = (u32, &'static str)>) -> Vec<RegistryEntry> {
    let mut out: Vec<RegistryEntry> =
        pairs.map(|(id, name)| RegistryEntry { id, name: name.to_string() }).collect();
    out.sort_by_key(|e| e.id);
    out
}

#[test]
fn header_registries_match_code_domain() {
    let h = Header::for_code();
    let kinds = entries(node_kind::ALL.iter().map(|(id, n)| (id.0, *n)));
    let cats = entries(edge_category::ALL.iter().map(|(id, n)| (id.0, *n)));
    let cells = entries(cell_type::ALL.iter().map(|(id, n)| (id.0, *n)));
    assert_eq!(h.node_kind_registry.len(), node_kind::ALL.len(), "node kinds");
    assert_eq!(h.edge_category_registry.len(), edge_category::ALL.len(), "edge categories");
    assert_eq!(h.cell_registry.len(), cell_type::ALL.len(), "cell types");
    assert_eq!(h.node_kind_registry, kinds);
    assert_eq!(h.edge_category_registry, cats);
    assert_eq!(h.cell_registry, cells);
    assert_eq!(h.graph_type, "code");
}

fn repo() -> RepoId {
    RepoId::from_canonical("test://lc4-toy")
}

fn node(id: u64, cells: Vec<Cell>) -> Node {
    Node { id: NodeId(id), repo: repo(), confidence: Confidence::Strong, cells }
}

fn cell(kind: u32) -> Cell {
    Cell { kind: CellTypeId(kind), payload: CellPayload::Text("x".into()) }
}

/// Write a toy core (no sections) and return its path inside `dir`.
fn write_toy(dir: &std::path::Path, header: Header, kinds: &[(u64, u32)], edge_cells: Vec<Cell>) -> std::path::PathBuf {
    let path = dir.join("toy.gmap");
    let mut core = Container {
        header,
        repo: repo(),
        nodes: vec![node(10, vec![cell(1)]), node(20, vec![])],
        edges: vec![Edge {
            from: NodeId(10),
            to: NodeId(20),
            category: EdgeCategoryId(1),
            confidence: Confidence::Strong,
            cells: edge_cells,
        }],
        node_kinds: kinds.iter().map(|(n, k)| (NodeId(*n), NodeKindId(*k))).collect(),
        sections: Vec::new(),
    };
    write_container(&path, &mut core, &[]).unwrap();
    path
}

fn nc(id: u32, name: &str, count: u64) -> (u32, String, u64) {
    (id, name.to_string(), count)
}

fn flat(v: &[NamedCount]) -> Vec<(u32, String, u64)> {
    v.iter().map(|c| (c.id, c.name.clone(), c.count)).collect()
}

#[test]
fn inspect_uses_header_names_not_code_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let header =
        Header::for_domain("toy", &[(1, "ATOM")], &[(1, "BOND")], &[(1, "CHARGE")]).unwrap();
    let path = write_toy(tmp.path(), header, &[(10, 1), (20, 1)], vec![]);

    let ins = inspect_path(&path).unwrap();
    assert_eq!(ins.manifest_schema, None);
    assert_eq!(ins.build_stamp, None);
    assert_eq!(ins.shards.len(), 1);
    let s = &ins.shards[0];
    assert_eq!(s.name, "toy");
    assert_eq!(s.graph_type, "toy");
    assert_eq!((s.nodes, s.edges), (2, 1));
    assert_eq!(flat(&s.kinds), vec![nc(1, "ATOM", 2)]);
    assert_eq!(flat(&s.categories), vec![nc(1, "BOND", 1)]);
    assert_eq!(flat(&s.node_cells), vec![nc(1, "CHARGE", 1)]);
    assert!(s.edge_cells.is_empty());
    assert!(s.sections.is_empty());
    assert_eq!(s.unregistered, 0);
    assert_eq!(flat(&ins.totals.kinds), vec![nc(1, "ATOM", 2)]);
    assert_eq!(ins.totals.unregistered, 0);
    assert_eq!(
        ins.marker(),
        format!(
            "[inspect] {}: shards=1 graph_types=toy format={} kinds=1 categories=1 node_cells=1 \
             edge_cells=0 unregistered=0",
            path.display(),
            repo_graph_store::FORMAT_VERSION
        )
    );
}

#[test]
fn unregistered_ids_are_counted() {
    let tmp = tempfile::tempdir().unwrap();
    let header =
        Header::for_domain("toy", &[(1, "ATOM")], &[(1, "BOND")], &[(1, "CHARGE")]).unwrap();
    // Node 20 is kind 77, which the header does not name; an edge cell of a
    // registered type is named from the same cell registry.
    let path = write_toy(tmp.path(), header, &[(10, 1), (20, 77)], vec![cell(1)]);

    let ins = inspect_path(&path).unwrap();
    let s = &ins.shards[0];
    assert_eq!(flat(&s.kinds), vec![nc(1, "ATOM", 1), nc(77, "#77", 1)]);
    assert_eq!(flat(&s.edge_cells), vec![nc(1, "CHARGE", 1)]);
    assert_eq!(s.unregistered, 1);
    assert_eq!(ins.totals.unregistered, 1);
    assert!(ins.marker().ends_with("edge_cells=1 unregistered=1"), "{}", ins.marker());

    // A header with empty registries (`Header::new`) names nothing: every id
    // is `#<id>`, never a code-domain name.
    let bare = tempfile::tempdir().unwrap();
    let path = write_toy(bare.path(), Header::new("toy"), &[(10, 1), (20, 1)], vec![]);
    let ins = inspect_path(&path).unwrap();
    assert_eq!(flat(&ins.shards[0].kinds), vec![nc(1, "#1", 2)]);
    assert_eq!(flat(&ins.shards[0].categories), vec![nc(1, "#1", 1)]);
    assert_eq!(ins.shards[0].unregistered, 3);
}

#[test]
fn inspect_reports_what_it_cannot_read() {
    let tmp = tempfile::tempdir().unwrap();
    let old = tmp.path().join("old.gmap");
    std::fs::write(&old, b"pre-0.5.0 bytes, no preamble").unwrap();
    let err = inspect_path(&old).unwrap_err();
    assert!(matches!(err, StoreError::OldFormat { found: None }), "{err:?}");
    assert!(err.to_string().contains("rebuild the graph"), "{err}");

    let missing = inspect_path(&tmp.path().join("nope.gmap")).unwrap_err();
    assert!(missing.needs_rebuild(), "{missing:?}");
    // A directory without a manifest is not a layout.
    assert!(inspect_path(tmp.path()).is_err());
}
