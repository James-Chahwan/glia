//! LC.1: `.gmap` FORMAT_VERSION 2 / manifest schema 2, and what an old,
//! future or foreign file reports.
//!
//! Every `.gmap` now opens with a fixed 32-byte preamble (`GLIAGMAP`, format
//! version, flags, core offset, core length) read BEFORE rkyv touches the
//! archive, so a 0.4.x file reports `OldFormat` ("rebuild the graph") instead of
//! an rkyv validation error. The pre-leap bytes are LG.6b's committed capture
//! (`tests/fixtures/gmap_pre_leap/`), materialised into a tempdir exactly as its
//! README says and never written in place. The preamble offsets the tests patch
//! are the layout documented on `repo_graph_store::FORMAT_VERSION`:
//!
//! ```text
//! [0..8)   b"GLIAGMAP"      [8..12)  format_version u32 LE
//! [12..16) flags u32 LE     [16..24) core_offset u64 LE   [24..32) core_len u64 LE
//! ```

use std::path::{Path, PathBuf};

use repo_graph_core::{Confidence, Node, NodeId, RepoId};
use repo_graph_graph::{MergedGraph, RepoGraph};
use repo_graph_store::{
    FORMAT_VERSION, MANIFEST_NAME, MANIFEST_VERSION, MmapContainer, ShardedMmap, StoreError,
    is_gmap_stale, read_merged_sharded, write_merged_sharded, write_repo_graph,
};

const PREAMBLE_MAGIC: &[u8; 8] = b"GLIAGMAP";
const VERSION_AT: usize = 8;
const CORE_LEN_AT: usize = 24;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/gmap_pre_leap")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let dst = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dst);
        } else {
            std::fs::copy(entry.path(), &dst).unwrap();
        }
    }
}

/// LG.6b's materialisation: `repo/*` -> `D/`, `layout-ai-repo-graph/*` ->
/// `D/.ai/repo-graph/`, `layout-glia-build/*` -> `D/.glia/`. The committed
/// fixture is only ever read.
fn materialise_pre_leap() -> tempfile::TempDir {
    let root = fixture_root();
    let d = tempfile::tempdir().unwrap();
    copy_tree(&root.join("repo"), d.path());
    copy_tree(&root.join("layout-ai-repo-graph"), &d.path().join(".ai/repo-graph"));
    copy_tree(&root.join("layout-glia-build"), &d.path().join(".glia"));
    d
}

/// Every `*.gmap` directly in `dir`, sorted by file name.
fn gmaps_in(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "gmap"))
        .collect();
    out.sort();
    out
}

fn node(id: u64, repo: RepoId) -> Node {
    Node {
        id: NodeId(id),
        repo,
        confidence: Confidence::Strong,
        cells: vec![],
    }
}

fn graph(canonical: &str, ids: &[u64]) -> RepoGraph {
    let repo = RepoId::from_canonical(canonical);
    RepoGraph {
        repo,
        nodes: ids.iter().map(|&i| node(i, repo)).collect(),
        edges: vec![],
        nav: Default::default(),
        symbols: Default::default(),
        unresolved_calls: vec![],
        unresolved_refs: vec![],
        properties: Default::default(),
    }
}

/// A freshly written v2 single-file `.gmap`.
fn write_v2_file(dir: &Path) -> PathBuf {
    let path = dir.join("v2.gmap");
    write_repo_graph(&graph("test://lc1-file", &[1, 2, 3]), &path).unwrap();
    path
}

fn patch(path: &Path, at: usize, bytes: &[u8]) {
    let mut b = std::fs::read(path).unwrap();
    b[at..at + bytes.len()].copy_from_slice(bytes);
    std::fs::write(path, b).unwrap();
}

fn assert_readable(e: &StoreError) {
    assert!(e.needs_rebuild(), "{e:?} must ask for a rebuild");
    assert!(e.rebuild_reason().is_some(), "{e:?} has no rebuild reason");
    let shown = e.to_string();
    assert!(!shown.contains("rkyv"), "error leaks rkyv internals: {shown}");
    assert!(!shown.contains("rancor"), "error leaks rancor internals: {shown}");
}

#[test]
fn v1_layout_reports_old_format() {
    let d = materialise_pre_leap();
    let layout = d.path().join(".ai/repo-graph");

    // The fixture must really be the pre-leap schema, or this test proves nothing.
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(layout.join(MANIFEST_NAME)).unwrap()).unwrap();
    assert_eq!(manifest["schema_version"], 1, "fixture manifest is not schema 1");

    match ShardedMmap::open(&layout) {
        Err(ref e @ StoreError::ManifestSchemaVersion { got: 1, supported: 2 }) => {
            assert_readable(e)
        }
        Err(e) => panic!("v1 layout: expected ManifestSchemaVersion{{1, 2}}, got {e:?}"),
        Ok(_) => panic!("v1 layout opened under manifest schema {MANIFEST_VERSION}"),
    }

    // Every shard, cross_stack.gmap and the flat `glia build` output.
    let mut files = gmaps_in(&layout);
    files.extend(gmaps_in(&d.path().join(".glia")));
    assert_eq!(files.len(), 7, "expected 4 sharded + 3 flat .gmap files: {files:?}");
    assert!(files.iter().any(|p| p.ends_with(".ai/repo-graph/cross_stack.gmap")));
    for p in &files {
        match MmapContainer::open(p) {
            Err(ref e @ StoreError::OldFormat { found: None }) => assert_readable(e),
            Err(e) => panic!("{}: expected OldFormat{{None}}, got {e:?}", p.display()),
            Ok(_) => panic!("{}: a 0.4.x file opened as format {FORMAT_VERSION}", p.display()),
        }
    }

    // The loader every consumer calls reports the same, and fires the marker.
    match read_merged_sharded(&layout) {
        Err(e) => {
            assert_readable(&e);
            assert!(
                matches!(e, StoreError::ManifestSchemaVersion { got: 1, .. }),
                "read_merged_sharded: {e:?}"
            );
        }
        Ok(_) => panic!("read_merged_sharded loaded a v1 layout"),
    }

    assert!(is_gmap_stale(&layout, d.path()), "a v1 layout must be stale");
}

/// A schema-2 manifest over 0.4.x shards (a half-copied or hand-edited
/// layout): the manifest passes, its hashes still match the old files, and
/// the first shard reports `OldFormat` through the loader every consumer calls.
#[test]
fn v2_manifest_over_v1_shards_reports_old_format() {
    let d = materialise_pre_leap();
    let layout = d.path().join(".ai/repo-graph");
    let manifest_path = layout.join(MANIFEST_NAME);
    let mut v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    v["schema_version"] = serde_json::json!(MANIFEST_VERSION);
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();

    match read_merged_sharded(&layout) {
        Err(ref e @ StoreError::OldFormat { found: None }) => {
            assert_readable(e);
            assert_eq!(
                e.rebuild_reason().as_deref(),
                Some("old format (no preamble, written by glia < 0.5.0)")
            );
        }
        Err(e) => panic!("expected OldFormat{{None}}, got {e:?}"),
        Ok(_) => panic!("0.4.x shards loaded under a schema-2 manifest"),
    }
}

#[test]
fn old_format_reason_names_the_old_writer() {
    let d = materialise_pre_leap();
    let first = gmaps_in(&d.path().join(".glia")).remove(0);
    let e = MmapContainer::open(&first).err().expect("0.4.x file opened");
    assert_eq!(
        e.rebuild_reason().as_deref(),
        Some("old format (no preamble, written by glia < 0.5.0)")
    );
    assert!(e.to_string().ends_with("- rebuild the graph"), "{e}");
}

#[test]
fn preamble_v1_is_old_format() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_v2_file(tmp.path());
    // Control: the unpatched file opens.
    MmapContainer::open(&path).expect("fresh v2 file");

    patch(&path, VERSION_AT, &1u32.to_le_bytes());
    match MmapContainer::open(&path) {
        Err(ref e @ StoreError::OldFormat { found: Some(1) }) => assert_readable(e),
        other => panic!("preamble v1: expected OldFormat{{Some(1)}}, got {:?}", other.err()),
    }

    patch(&path, VERSION_AT, &3u32.to_le_bytes());
    match MmapContainer::open(&path) {
        Err(ref e @ StoreError::FutureFormat { found: 3 }) => assert_readable(e),
        other => panic!("preamble v3: expected FutureFormat{{3}}, got {:?}", other.err()),
    }
}

#[test]
fn same_version_other_layout_is_corrupt_not_rkyv() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_v2_file(tmp.path());
    let mut bytes = std::fs::read(&path).unwrap();
    let mut len = [0u8; 8];
    len.copy_from_slice(&bytes[CORE_LEN_AT..CORE_LEN_AT + 8]);
    let core_len = u64::from_le_bytes(len);
    assert_eq!(core_len as usize + 32, bytes.len(), "core is the rest of the file");

    // Same format version, different archive: the dev-build drift case.
    bytes.extend_from_slice(&[0u8; 8]);
    bytes[CORE_LEN_AT..CORE_LEN_AT + 8].copy_from_slice(&(core_len + 8).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    match MmapContainer::open(&path) {
        Err(ref e @ StoreError::Corrupt { .. }) => assert_readable(e),
        other => panic!("grown core: expected Corrupt, got {:?}", other.err()),
    }

    // A core range outside the file is Corrupt too, before rkyv runs.
    bytes[CORE_LEN_AT..CORE_LEN_AT + 8].copy_from_slice(&(core_len + 4096).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    match MmapContainer::open(&path) {
        Err(ref e @ StoreError::Corrupt { .. }) => assert_readable(e),
        other => panic!("overlong core: expected Corrupt, got {:?}", other.err()),
    }
}

#[test]
fn misaligned_core_offset_is_corrupt() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_v2_file(tmp.path());
    patch(&path, 16, &40u64.to_le_bytes());
    match MmapContainer::open(&path) {
        Err(ref e @ StoreError::Corrupt { .. }) => assert_readable(e),
        other => panic!("core_offset 40: expected Corrupt, got {:?}", other.err()),
    }
}

#[test]
fn v2_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("layout");
    let merged = MergedGraph {
        graphs: vec![graph("test://lc1-a", &[10, 11]), graph("test://lc1-b", &[20])],
        cross_edges: vec![repo_graph_core::Edge {
            from: NodeId(10),
            to: NodeId(20),
            category: repo_graph_code_domain::edge_category::HTTP_CALLS,
            confidence: Confidence::Strong,
        }],
        pass_undo: vec![],
    };
    let manifest = write_merged_sharded(&merged, &dir).unwrap();
    assert_eq!(manifest.schema_version, 2);
    assert_eq!(MANIFEST_VERSION, 2);
    assert_eq!(FORMAT_VERSION, 2);

    let on_disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(MANIFEST_NAME)).unwrap()).unwrap();
    assert_eq!(on_disk["schema_version"], 2);

    let files = gmaps_in(&dir);
    assert_eq!(files.len(), 3, "two shards + cross_stack: {files:?}");
    for p in &files {
        let b = std::fs::read(p).unwrap();
        assert!(b.starts_with(PREAMBLE_MAGIC), "{}: no GLIAGMAP preamble", p.display());
        assert_eq!(&b[VERSION_AT..VERSION_AT + 4], &2u32.to_le_bytes());
        assert_eq!(&b[16..24], &32u64.to_le_bytes(), "core_offset");
    }

    let loaded = read_merged_sharded(&dir).unwrap();
    assert_eq!(loaded.graphs.len(), merged.graphs.len());
    for (a, b) in loaded.graphs.iter().zip(&merged.graphs) {
        assert_eq!(a.nodes.len(), b.nodes.len());
        assert_eq!(a.edges.len(), b.edges.len());
    }
    assert_eq!(loaded.cross_edges, merged.cross_edges);

    // The archived header agrees with the preamble.
    let opened = MmapContainer::open(&files[0]).unwrap();
    assert_eq!(opened.archived().unwrap().header.version.to_native(), FORMAT_VERSION);
}

#[test]
fn stale_on_old_manifest_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let gmap_dir = tmp.path().join("gmap");
    let repo_dir = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    write_merged_sharded(
        &MergedGraph {
            graphs: vec![graph("test://lc1-stale", &[1])],
            cross_edges: vec![],
            pass_undo: vec![],
        },
        &gmap_dir,
    )
    .unwrap();
    // Control: this build's fresh layout with no newer sources is not stale.
    assert!(!is_gmap_stale(&gmap_dir, &repo_dir));

    // Only the schema number moves: same build stamp, same shard hashes.
    let manifest_path = gmap_dir.join(MANIFEST_NAME);
    let mut v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    v["schema_version"] = serde_json::json!(1);
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    assert!(is_gmap_stale(&gmap_dir, &repo_dir), "schema 1 manifest must be stale");
    assert!(matches!(
        ShardedMmap::open(&gmap_dir).err(),
        Some(StoreError::ManifestSchemaVersion { got: 1, supported: 2 })
    ));
}

#[test]
fn needs_rebuild_is_false_for_caller_errors() {
    let not_found = StoreError::NodeNotFound(NodeId(7));
    assert!(!not_found.needs_rebuild());
    assert_eq!(not_found.rebuild_reason(), None);
    let denied = StoreError::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    assert!(!denied.needs_rebuild());
    let missing = StoreError::Io(std::io::Error::from(std::io::ErrorKind::NotFound));
    assert!(missing.needs_rebuild());
}
