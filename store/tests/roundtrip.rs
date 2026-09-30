//! End-to-end roundtrip: build a real `RepoGraph` with `build_go` against the
//! `http_stack_smoke/backend` fixture, write it to a temp `.gmap` file, mmap
//! it back, and assert the archived form matches the owned form for every
//! surface v0.4.5a exposes.
//!
//! This is the v0.4.5a acceptance test — if this stays green, the store
//! crate's write + mmap + zero-copy access contract is working.

use std::path::{Path, PathBuf};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use glia_core::{CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use glia_graph::{MergedGraph, RepoGraph, SymbolTable, build_go};
use glia_parser_go::parse_file;
use glia_store::{
    CODE_SECTION, CROSS_STACK_NAME, FORMAT_VERSION, MANIFEST_NAME, MmapContainer, STRINGS_SECTION,
    StoreError, code_section_of, decode_repo_graph, qname_of, read_merged_sharded,
    upsert_cell_sharded, write_merged_sharded, write_repo_graph,
};

const MODULE_PREFIX: &str = "example.com/backend";

fn backend_root() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent()
        .unwrap()
        .join("tests/fixtures/http_stack_smoke/backend")
}

fn repo() -> RepoId {
    RepoId::from_canonical("test://http_stack_smoke/backend")
}

fn build() -> glia_graph::RepoGraph {
    let files = [
        ("users/users.go", "users"),
        ("server/server.go", "server"),
    ];
    let parses: Vec<_> = files
        .iter()
        .map(|(rel, pkg)| {
            let src = std::fs::read_to_string(backend_root().join(rel)).unwrap();
            parse_file(&src, rel, pkg, MODULE_PREFIX, repo()).unwrap()
        })
        .collect();
    build_go(repo(), parses).unwrap()
}

#[test]
fn repo_graph_roundtrips_through_gmap_file() {
    let g = build();
    let expected_node_count = g.nodes.len();
    let expected_edge_count = g.edges.len();
    let expected_nav_size = g.nav.qname_by_id.len();

    // Sanity: the backend fixture should produce a non-trivial graph —
    // otherwise a silently-empty write would pass this test meaninglessly.
    assert!(expected_node_count > 0, "backend build produced no nodes");
    assert!(expected_edge_count > 0, "backend build produced no edges");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backend.gmap");
    write_repo_graph(&g, &path).unwrap();
    assert!(path.exists(), "written .gmap file should exist at {path:?}");
    let raw = std::fs::read(&path).unwrap();
    assert!(raw.starts_with(b"GLIAGMAP"), "written .gmap has no GLIAGMAP preamble");

    let container = MmapContainer::open(&path).unwrap();
    let archived = container.archived().unwrap();

    // Header + repo round-trip.
    assert_eq!(archived.header.magic, *b"GMAP");
    assert_eq!(archived.header.version.to_native(), FORMAT_VERSION);
    assert_eq!(archived.header.graph_type.as_str(), "code");
    assert_eq!(archived.repo.0.to_native(), repo().0);

    // Node and edge counts match.
    assert_eq!(archived.nodes.len(), expected_node_count);
    assert_eq!(archived.edges.len(), expected_edge_count);

    // Nav index count matches (pre/post-flatten sizes are identical: one pair
    // per key with no dedup since the source is already a HashMap with unique
    // keys). LC.5b: the nav maps live in the "code" section, the kinds in the
    // core.
    let code = code_section_of(&container)
        .unwrap()
        .expect("a built graph writes a code section");
    assert_eq!(code.nav.qname_by_id.len(), expected_nav_size);
    assert_eq!(archived.node_kinds.len(), g.nav.kind_by_id.len());
    assert!(container.section_bytes(CODE_SECTION).unwrap().is_some());

    // Point lookup via binary search: one known node from the fixture is the
    // `/health` route — v0.4.4a tests assert its presence.
    // Find a Route node id by its KIND in the owned graph (LB.11a: the check
    // names no qname shape), then look up the same id via the code section
    // (qname) and the core (kind).
    let route_id = g
        .nav
        .qname_by_id
        .keys()
        .find(|id| g.nav.kind_by_id.get(id) == Some(&node_kind::ROUTE))
        .copied()
        .expect("backend fixture has at least one Route");
    let qn_owned = g.nav.qname_by_id.get(&route_id).unwrap();
    let qn_archived = code.qname(route_id).expect("archived qname lookup");
    assert_eq!(qn_archived, qn_owned);
    assert_eq!(qname_of(&container, route_id).unwrap().as_ref(), Some(qn_owned));
    assert_eq!(archived.kind(route_id), Some(node_kind::ROUTE));

    // Unknown id returns None (binary search miss, no panic).
    assert_eq!(code.qname(NodeId(0xDEAD_BEEF)), None);
    assert_eq!(qname_of(&container, NodeId(0xDEAD_BEEF)).unwrap(), None);
    assert_eq!(archived.kind(NodeId(0xDEAD_BEEF)), None);

    // Edge iterator yields the same (from, to, category) triples.
    let mut from_owned: Vec<_> = g
        .edges
        .iter()
        .map(|e| (e.from.0, e.to.0, e.category.0))
        .collect();
    let mut from_archived: Vec<_> = archived
        .edges_iter()
        .map(|(f, t, c)| (f.0, t.0, c.0))
        .collect();
    from_owned.sort();
    from_archived.sort();
    assert_eq!(from_archived, from_owned);

    // File size is non-zero and reasonable — a .gmap of this fixture should
    // be hundreds of bytes to a few kB, not MB.
    assert!(container.len() > 64, "gmap file suspiciously small");
    assert!(container.len() < 1_000_000, "gmap file suspiciously large");

    // tempdir cleans up on drop; container holds an mmap of a file that's
    // about to be unlinked but the kernel keeps the inode alive until we
    // drop the mmap.
    drop(container);
    drop(dir);
}

#[test]
fn opening_a_non_gmap_file_fails_with_old_format() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"NOTA gmap file, just some junk bytes").unwrap();
    // No GLIAGMAP preamble: rejected before rkyv reads a byte, and reported as
    // a file to rebuild rather than an rkyv validation error.
    match MmapContainer::open(tmp.path()) {
        Err(ref e @ StoreError::OldFormat { found: None }) => {
            assert!(e.needs_rebuild());
            assert!(!e.to_string().contains("rkyv"), "{e}");
        }
        Err(e) => panic!("garbage file: expected OldFormat{{None}}, got {e:?}"),
        Ok(_) => panic!("garbage file should not open as a gmap"),
    }
}

// ----------------------------------------------------------------------------
// CD.7b: EVIDENCE is stored interned (format 3) and read back as its JSON
// ----------------------------------------------------------------------------

/// The on-disk JSON form of an EVIDENCE payload: no format-3 file carries one.
const EVIDENCE_JSON: &[u8] = br#"{"emitter":"#;

fn has_evidence_json(bytes: &[u8]) -> bool {
    bytes.windows(EVIDENCE_JSON.len()).any(|w| w == EVIDENCE_JSON)
}

fn client_repo() -> RepoId {
    RepoId::from_canonical("test://http_stack_smoke/frontend")
}

/// A client repo (one ENDPOINT calling a helper, both with canonical
/// EVIDENCE, edge_cells.rs's shape) and one cross HTTP edge from its endpoint
/// to the backend's first ROUTE, carrying `resolver:http` EVIDENCE.
fn with_client(backend: RepoGraph) -> MergedGraph {
    let route = backend
        .nav
        .qname_by_id
        .keys()
        .copied()
        .filter(|id| backend.nav.kind_by_id.get(id) == Some(&node_kind::ROUTE))
        .min_by_key(|id| id.0)
        .expect("backend fixture has a ROUTE");
    let repo = client_repo();
    let mut nav = CodeNav::default();
    let mut node = |kind, qname: &str| {
        let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, qname);
        nav.record(id, qname.rsplit("::").next().unwrap_or(qname), qname, kind, None);
        Node { id, repo, confidence: Confidence::Strong, cells: Vec::new() }
    };
    let caller = node(node_kind::FUNCTION, "web::api::loadUsers");
    let ep = node(node_kind::ENDPOINT, "endpoint:GET:/health");
    let (c, e) = (caller.id, ep.id);
    let client = RepoGraph {
        repo,
        nodes: vec![caller, ep],
        edges: vec![
            Edge::new(c, e, edge_category::HTTP_CALLS, Confidence::Strong).with_cell(
                Evidence::emitter("extractor:http").rule("fetch").at("web/api.ts", 3).to_cell(),
            ),
            Edge::new(c, e, edge_category::CALLS, Confidence::Strong)
                .with_cell(Evidence::emitter("parser:typescript").rule("intra_file").at("web/api.ts", 9).to_cell()),
        ],
        nav,
        symbols: SymbolTable::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    };
    let mut merged = MergedGraph::new(vec![backend, client]);
    merged.cross_edges = vec![
        Edge::new(e, route, edge_category::HTTP_CALLS, Confidence::Strong)
            .with_cell(Evidence::emitter("resolver:http").rule("exact").at("web/api.ts", 3).to_cell()),
    ];
    merged
}

/// The `.gmap` files `dir`'s manifest names, shards then cross.
fn layout_files(dir: &Path) -> Vec<PathBuf> {
    let m: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(MANIFEST_NAME)).unwrap()).unwrap();
    m["shards"]
        .as_array()
        .unwrap()
        .iter()
        .chain(m.get("cross"))
        .map(|e| dir.join(e["path"].as_str().unwrap()))
        .collect()
}

#[test]
fn evidence_interned_round_trip() {
    // (a) One file: the Go backend through write_repo_graph / decode_repo_graph.
    let g = build();
    let with_evidence = g.edges.iter().filter(|e| e.cell(cell_type::EVIDENCE).is_some()).count();
    assert!(with_evidence > 0, "the backend fixture's edges carry no EVIDENCE");
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("backend.gmap");
    write_repo_graph(&g, &path).unwrap();
    let raw = std::fs::read(&path).unwrap();
    assert!(!has_evidence_json(&raw), "backend.gmap stores EVIDENCE as JSON");
    let m = MmapContainer::open(&path).unwrap();
    assert!(m.section_bytes(STRINGS_SECTION).unwrap().is_some(), "no strings section");
    // The archived core holds every EVIDENCE cell in the compact form ...
    let archived = m.archived().unwrap();
    let interned = archived
        .edges
        .iter()
        .flat_map(|e| e.cells.iter())
        .filter(|c| c.kind.0.to_native() == cell_type::EVIDENCE.0)
        .filter(|c| matches!(c.payload, glia_core::ArchivedCellPayload::Bytes(_)))
        .count();
    assert_eq!(interned, with_evidence, "an EVIDENCE cell was not interned");
    // ... and the decoded graph is the one written, every EVIDENCE JSON byte-identical.
    let back = decode_repo_graph(&m).unwrap();
    assert_eq!(back.edges.len(), g.edges.len());
    for (a, b) in back.edges.iter().zip(&g.edges) {
        assert_eq!(a.cells, b.cells, "edge {} -> {}: cells differ after the round trip", b.from.0, b.to.0);
    }
    assert_eq!(back.edges, g.edges);

    // (b) A layout: the backend and a client repo, and one cross edge.
    let merged = with_client(g);
    let dir = tmp.path().join("layout");
    write_merged_sharded(&merged, &dir).unwrap();
    let files = layout_files(&dir);
    assert_eq!(files.len(), 3, "two shards and cross_stack: {files:?}");
    for p in &files {
        let bytes = std::fs::read(p).unwrap();
        assert!(!has_evidence_json(&bytes), "{}: stores EVIDENCE as JSON", p.display());
        let names = MmapContainer::open(p).unwrap().section_names().unwrap();
        assert!(names.iter().any(|(n, _)| n == STRINGS_SECTION), "{}: {names:?}", p.display());
    }
    assert!(files.iter().any(|p| p.ends_with(CROSS_STACK_NAME)));
    let loaded = read_merged_sharded(&dir).unwrap();
    for (a, b) in loaded.graphs.iter().zip(&merged.graphs) {
        assert_eq!(a.edges, b.edges, "repo {}: edges, cells included", b.repo.0);
    }
    assert_eq!(loaded.cross_edges, merged.cross_edges, "cross edges, cells included");
    let json = |e: &Edge| match &e.cell(cell_type::EVIDENCE).unwrap().payload {
        CellPayload::Json(s) => s.clone(),
        other => panic!("EVIDENCE read back as {other:?}"),
    };
    assert_eq!(
        json(&loaded.cross_edges[0]),
        r#"{"emitter":"resolver:http","rule":"exact","file":"web/api.ts","line":3,"basis":"site"}"#
    );

    // (c) A cell upsert rewrites the shard raw: the interned payloads stay valid
    // against the strings section copied back beside them.
    let target = merged.graphs[0].nodes[0].id;
    upsert_cell_sharded(&dir, target, cell_type::INTENT, CellPayload::Text("cd7b".into())).unwrap();
    let again = read_merged_sharded(&dir).unwrap();
    for (a, b) in again.graphs.iter().zip(&merged.graphs) {
        assert_eq!(a.edges, b.edges, "repo {}: edges after an upsert", b.repo.0);
    }
    assert_eq!(again.cross_edges, merged.cross_edges);
}
