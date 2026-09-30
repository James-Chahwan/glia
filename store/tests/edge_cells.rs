//! LC.2: edge cells survive the sharded `.gmap` layout, and the cross-stack
//! file's bytes do not depend on the order the resolvers pushed edges in once
//! same-key edges carry different cells.
//!
//! CD.7b: the fixture's EVIDENCE is deliberately NOT canonical (`line` before
//! `emitter`, a `Text` payload), so every cell here is kept as written; the
//! interned path is `non_canonical_evidence_is_kept`'s control and
//! `corrupt_index_is_corrupt`'s subject, and roundtrip.rs's
//! `evidence_interned_round_trip` covers it end to end.

use std::hash::Hasher;
use std::path::Path;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use glia_graph::{MergedGraph, RepoGraph, SymbolTable};
use glia_store::{
    CROSS_STACK_NAME, MANIFEST_NAME, MmapContainer, StoreError, StringTable, decode_repo_graph,
    intern_evidence, read_merged_sharded, write_merged_sharded, write_repo_graph,
};

fn repo_a() -> RepoId {
    RepoId::from_canonical("test://edge_cells/a")
}

fn repo_b() -> RepoId {
    RepoId::from_canonical("test://edge_cells/b")
}

fn node(repo: RepoId, kind: glia_core::NodeKindId, qname: &str, nav: &mut CodeNav) -> Node {
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

// ----------------------------------------------------------------------------
// CD.7b: what is interned, what is kept, and a payload that does not decode
// ----------------------------------------------------------------------------

/// Canonical evidence: exactly what `Evidence::to_cell` writes.
const CANONICAL: &str = r#"{"emitter":"graph:calls","rule":"module_symbol","file":"srv/users.py","line":41,"basis":"site"}"#;
/// The same evidence with one extra space: it parses, but `to_cell` would not
/// write these bytes back, so interning it would not be lossless.
const SPACED: &str = r#"{"emitter":"graph:calls", "rule":"module_symbol","file":"srv/users.py","line":41,"basis":"site"}"#;

fn evidence_edge(json: &str) -> Edge {
    let (a, b) = (NodeId(1), NodeId(2));
    Edge::new(a, b, edge_category::CALLS, Confidence::Strong).with_cell(evidence(json))
}

#[test]
fn non_canonical_evidence_is_kept() {
    // The control is canonical, or this proves nothing.
    let parsed: Evidence = serde_json::from_str(SPACED).expect("the spaced form still parses");
    assert_eq!(parsed.to_cell().payload, CellPayload::Json(CANONICAL.into()));

    let mut edges = vec![evidence_edge(SPACED), evidence_edge(CANONICAL)];
    let mut table = StringTable::default();
    let stats = intern_evidence(&mut edges, &mut table);
    assert_eq!((stats.interned, stats.kept_json), (1, 1));
    assert_eq!(edges[0].cells[0].payload, CellPayload::Json(SPACED.into()), "kept byte for byte");
    assert!(matches!(edges[1].cells[0].payload, CellPayload::Bytes(_)), "the canonical one is interned");

    // Through a layout: the spaced JSON is stored as written and read back
    // unchanged; the canonical twin comes back as its JSON too.
    let (mut a, b, _, _) = fixture();
    let (h, v) = (a.nodes[0].id, a.nodes[1].id);
    a.edges = vec![
        Edge::new(h, v, edge_category::CALLS, Confidence::Strong).with_cell(evidence(SPACED)),
        Edge::new(h, v, edge_category::CALLS, Confidence::Weak).with_cell(evidence(CANONICAL)),
    ];
    let before = a.edges.clone();
    let dir = tempfile::tempdir().unwrap();
    write_merged_sharded(&MergedGraph::new(vec![a, b]), dir.path()).unwrap();
    let back = read_merged_sharded(dir.path()).unwrap();
    let a_back = back.graphs.iter().find(|g| g.repo == repo_a()).expect("repo a shard");
    assert_eq!(a_back.edges, before);
    let shard = std::fs::read(shard_path(dir.path(), repo_a())).unwrap();
    assert!(contains(&shard, SPACED.as_bytes()), "the kept payload is on disk as written");
    assert!(!contains(&shard, CANONICAL.as_bytes()), "the canonical payload was stored as JSON");
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// The file of `repo`'s shard in a layout written by `write_merged_sharded`.
fn shard_path(dir: &Path, repo: RepoId) -> std::path::PathBuf {
    let prefix = format!("repo-{}-", repo.0);
    let m: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(MANIFEST_NAME)).unwrap()).unwrap();
    let entry = m["shards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"].as_str().unwrap().starts_with(&prefix))
        .expect("the repo's shard");
    dir.join(entry["path"].as_str().unwrap())
}

/// The interned form of [`CANONICAL`] alone in a file: emitter 0, rule 1 + 1,
/// file 2 + 1, line 41 + 1, basis site (1).
const INTERNED: [u8; 6] = [0x01, 0, 2, 3, 42, 1];

/// Point the emitter index of the one interned payload in `bytes` at entry
/// 0x7f of a 3-entry table.
fn patch_index(bytes: &mut [u8]) {
    let at: Vec<usize> =
        bytes.windows(INTERNED.len()).enumerate().filter(|(_, w)| *w == INTERNED).map(|(i, _)| i).collect();
    assert_eq!(at.len(), 1, "the interned payload is not unique in the file");
    bytes[at[0] + 1] = 0x7f;
}

fn xxh64(bytes: &[u8]) -> String {
    let mut h = twox_hash::XxHash64::with_seed(0);
    h.write(bytes);
    format!("{:016x}", h.finish())
}

/// The fixture with repo `a` cut down to one CALLS edge carrying
/// [`CANONICAL`], and repo `b`; the handler and helper ids.
fn one_edge() -> (RepoGraph, RepoGraph, NodeId, NodeId) {
    let (mut a, b, _, _) = fixture();
    let (h, v) = (a.nodes[0].id, a.nodes[1].id);
    a.edges = vec![Edge::new(h, v, edge_category::CALLS, Confidence::Strong).with_cell(evidence(CANONICAL))];
    (a, b, h, v)
}

#[test]
fn corrupt_index_is_corrupt() {
    // One file: decode_repo_graph names the edge and the index.
    let (one, _, h, v) = one_edge();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("one.gmap");
    write_repo_graph(&one, &path).unwrap();
    // Control: the unpatched file decodes to the graph written.
    assert_eq!(decode_repo_graph(&MmapContainer::open(&path).unwrap()).unwrap().edges, one.edges);
    let mut bytes = std::fs::read(&path).unwrap();
    patch_index(&mut bytes);
    std::fs::write(&path, &bytes).unwrap();
    let m = MmapContainer::open(&path).expect("the archive still validates: the payload is opaque bytes");
    match decode_repo_graph(&m) {
        Err(ref e @ StoreError::Corrupt { ref detail }) => {
            assert!(detail.contains("interned EVIDENCE of edge 0"), "{detail}");
            assert!(detail.contains("emitter index 127 outside the 3-entry strings table"), "{detail}");
            assert!(e.needs_rebuild(), "{e:?}");
        }
        other => panic!("expected Corrupt, got {:?}", other.map(|g| g.edges.len())),
    }

    // A layout: the same patch in a shard and in cross_stack.gmap, with the
    // manifest hash moved along so the damage reaches the decoder, names the
    // shard it is in.
    let mut cross = Edge::new(v, h, edge_category::HTTP_CALLS, Confidence::Strong);
    cross = cross.with_cell(evidence(CANONICAL));
    for (what, target) in [("shard", "a"), ("cross", "cross_stack")] {
        let dir = tmp.path().join(format!("layout-{what}"));
        let (a, b, _, _) = one_edge();
        let mut merged = MergedGraph::new(vec![a, b]);
        merged.cross_edges = vec![cross.clone()];
        write_merged_sharded(&merged, &dir).unwrap();
        assert_eq!(read_merged_sharded(&dir).unwrap().cross_edges, merged.cross_edges, "control");
        let file = if target == "a" { shard_path(&dir, repo_a()) } else { dir.join(CROSS_STACK_NAME) };
        let mut bytes = std::fs::read(&file).unwrap();
        patch_index(&mut bytes);
        std::fs::write(&file, &bytes).unwrap();
        let manifest_path = dir.join(MANIFEST_NAME);
        let mut m: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        let name = file.file_stem().unwrap().to_string_lossy().into_owned();
        let entry = if target == "a" {
            m["shards"].as_array_mut().unwrap().iter_mut().find(|e| e["name"] == name.as_str()).unwrap()
        } else {
            &mut m["cross"]
        };
        entry["content_hash"] = serde_json::json!(xxh64(&bytes));
        std::fs::write(&manifest_path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
        match read_merged_sharded(&dir) {
            Err(StoreError::Corrupt { detail }) => {
                assert!(detail.starts_with(&format!("shard {name}: interned EVIDENCE of edge 0")), "{what}: {detail}");
            }
            other => panic!("{what}: expected Corrupt, got {:?}", other.map(|g| g.graphs.len())),
        }
    }
}
