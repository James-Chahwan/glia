//! LF.1d — `.glia` is glia's control directory: the build's walk never
//! descends into it (no parse, no JSON contract sniff, no REGION), its readers
//! open their files directly, and the store tracks its inputs by content.
//!
//! Before LF.1d a repo that gitignores `.glia/docs-snapshot/` (the documented
//! default) grew a `region:.glia/docs-snapshot` node, `.glia/notes.py` was
//! parsed as `.glia::notes`, and an OpenAPI-shaped `.glia/scratch/spec.json`
//! became a contract. A repo that gitignores `.glia/` whole grew a
//! `region:.glia` only after its first persisted build, so its graph depended
//! on build history. Run with `-- --nocapture` and grep
//! `^\[gmap\] stale: external input changed` for the fired_on marker.

use std::collections::HashMap;
use std::path::Path;

use repo_graph_code_domain::node_kind;
use repo_graph_core::NodeId;
use repo_graph_engine::GenerateResult;
use repo_graph_engine::generate_one;
use repo_graph_engine::persist::{default_layout_dir, persist_result};
use repo_graph_store::{MANIFEST_NAME, is_gmap_stale};

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// One `DocRecord` line, the shape doc-sources `write_snapshot` writes.
const SNAPSHOT_LINE: &str = concat!(
    r##"{"rel_path":"confluence/ENG/runbook.md","text":"# Runbook\n\nCall `f` to start.\n","##,
    r##""provenance":{"kind":"Confluence","url":"https://wiki.example/x","container":"ENG","version":"3"}}"##,
    "\n"
);

/// Ordinary source beside a populated control dir: a source file, an
/// OpenAPI-shaped JSON and a docs snapshot under `.glia`, plus the same names
/// under a nested `pkg/.glia`.
fn control_dir_repo(root: &Path, gitignore: &str) {
    write(&root.join("m.py"), "def f():\n    return 1\n");
    write(&root.join(".gitignore"), gitignore);
    write(&root.join(".glia/notes.py"), "def g():\n    return 2\n");
    write(&root.join(".glia/notes.md"), "# Notes\n\nNever a DOC_SECTION.\n");
    write(
        &root.join(".glia/scratch/spec.json"),
        r#"{"openapi":"3.0.0","info":{"title":"t","version":"1"},"paths":{"/x":{"get":{"responses":{"200":{"description":"ok"}}}}}}"#,
    );
    write(&root.join(".glia/docs-snapshot/manifest.jsonl"), SNAPSHOT_LINE);
    write(&root.join("pkg/.glia/nested.py"), "def h():\n    return 3\n");
}

/// `(kind, qname)` of every node the build produced, sorted.
fn nodes(r: &GenerateResult) -> Vec<(u32, String)> {
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    let mut out = Vec::new();
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
        for (id, k) in &g.nav.kind_by_id {
            out.push((k.0, qname.get(id).cloned().unwrap_or_default()));
        }
    }
    for (k, q) in out.iter_mut() {
        if q.is_empty() {
            *q = format!("<kind {k}>");
        }
    }
    out.sort();
    out
}

fn assert_no_control_dir_nodes(r: &GenerateResult, context: &str) {
    let all = nodes(r);
    let leaked: Vec<&(u32, String)> = all
        .iter()
        .filter(|(_, q)| q.starts_with(".glia") || q.contains("::.glia") || q.contains(".glia/"))
        .collect();
    assert!(leaked.is_empty(), "{context}: nodes from under .glia: {leaked:?}");
    let regions: Vec<&String> =
        all.iter().filter(|(k, _)| *k == node_kind::REGION.0).map(|(_, q)| q).collect();
    assert!(regions.iter().all(|q| !q.contains(".glia")), "{context}: {regions:?}");
    assert!(
        // LB.12: the qname a sniffed .glia/scratch/spec.json op would mint.
        !all.iter().any(|(k, q)| *k == node_kind::DOC_SECTION.0 && q == "contract::.glia::scratch::spec::GET:/x"),
        "{context}: .glia/scratch/spec.json was contract-sniffed"
    );
    // The control: ordinary source beside `.glia` is still parsed.
    assert!(
        all.iter().any(|(k, q)| *k == node_kind::FUNCTION.0 && q == "m::f"),
        "{context}: m::f missing from {all:?}"
    );
}

/// Probe s2's shape (`.gitignore` holds `.glia/docs-snapshot/`, that dir
/// present): no node under `.glia`, no REGION for it, and the snapshot is
/// still ingested, because its reader opens the file directly.
#[test]
fn glia_dir_is_never_walked() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    control_dir_repo(&repo, ".glia/docs-snapshot/\n");
    let r = generate_one(repo.to_str().unwrap()).unwrap();
    assert_no_control_dir_nodes(&r, "fresh build");

    let all = nodes(&r);
    assert!(
        all.iter().any(|(k, q)| *k == node_kind::DOC_SECTION.0 && q.contains("runbook")),
        "the docs snapshot must still be ingested: {all:?}"
    );
    assert!(
        !all.iter().any(|(_, q)| q.contains("nested") || q.contains("pkg::.glia")),
        "a nested .glia is the control dir too: {all:?}"
    );
}

/// Every file of a layout dir, name -> bytes, sorted by name.
fn layout_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read(e.path()).unwrap()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Two clean builds persisted by the one writer to fresh default layout dirs
/// are byte-identical, manifest (and its `external_inputs`) included. The
/// repo gitignores `/.glia/` whole (bench/doc-link/fixture-repo's shape): the
/// second build runs with the first layout on disk, the case that used to
/// grow a `region:.glia` only after the first persisted build.
#[test]
fn control_dir_builds_are_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    control_dir_repo(&repo, "/.glia/\n");
    let layout = default_layout_dir(&repo);

    let first = generate_one(repo.to_str().unwrap()).unwrap();
    assert_no_control_dir_nodes(&first, "first build");
    persist_result(&first, &layout, "test").unwrap();
    let bytes1 = layout_bytes(&layout);

    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(layout.join(MANIFEST_NAME)).unwrap()).unwrap();
    let inputs = manifest["external_inputs"].as_object().expect("the writer records the fingerprint");
    let keys: Vec<&str> = inputs.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            ".glia/docs-snapshot/manifest.jsonl",
            ".glia/notes.md",
            ".glia/notes.py",
            ".glia/scratch/spec.json",
        ],
        "every .glia input and none of the layout's own files"
    );

    // Build history present: the first layout sits under `.glia/graph`.
    let second = generate_one(repo.to_str().unwrap()).unwrap();
    assert_no_control_dir_nodes(&second, "build with a persisted layout on disk");
    std::fs::remove_dir_all(&layout).unwrap();
    persist_result(&second, &layout, "test").unwrap();
    let bytes2 = layout_bytes(&layout);

    let names = |b: &[(String, Vec<u8>)]| b.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(names(&bytes1), names(&bytes2), "layout file sets differ");
    for ((name, a), (_, b)) in bytes1.iter().zip(bytes2.iter()) {
        assert_eq!(a, b, "{name} bytes differ between two clean builds");
    }
}

/// The engine's writer (the one `glia build`, pyo3 `generate` and
/// `save_to_default` share) records the fingerprint with no call-site change,
/// so an in-place snapshot rewrite (what `glia docs sync` does) marks the
/// layout stale.
#[test]
fn in_place_snapshot_rewrite_after_persist_is_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    control_dir_repo(&repo, ".glia/docs-snapshot/\n");
    let layout = default_layout_dir(&repo);
    persist_result(&generate_one(repo.to_str().unwrap()).unwrap(), &layout, "test").unwrap();
    assert!(!is_gmap_stale(&layout, &repo), "fresh right after the persist");

    let snap = repo.join(".glia/docs-snapshot/manifest.jsonl");
    std::fs::write(&snap, SNAPSHOT_LINE.replace("\"3\"", "\"4\"")).unwrap();
    assert!(is_gmap_stale(&layout, &repo), "an in-place snapshot rewrite must mark stale");
}
