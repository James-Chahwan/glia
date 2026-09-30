//! CE.1d: a `.glia/scip-snapshot/` (CE.1a's format, written here with
//! `code_domain::snapshots::write_scip`, no indexer needed) binds each
//! definition to the innermost glia node whose span holds it and whose name
//! matches, and turns the references glia's static resolver cannot bind into
//! FACT CALLS / USES edges under the `scip:<tool>` EVIDENCE stage.
//!
//! The sources are the substrate-gap fixture `scip-python-dict-dispatch`
//! (`repo` is typed only by the value of a dict literal behind an unannotated
//! factory), copied into temp trees. `fixture_snapshot_is_current` pins the
//! fixture's committed snapshot to its sources; regenerate it with
//! `cargo test -p glia-engine --test scip_ingest -- --ignored write_dict_dispatch_fixture`.
//!
//! The fired_on markers are read from a child process: `child_for_stderr`
//! re-runs this binary on one tree with `--nocapture` and the parent reads its
//! stderr (`grep '^\[scip\] ingest'`).

use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::edge_category;
use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::snapshots::{
    ScipDefRow, ScipDocumentRecord, ScipMeta, ScipRefRow, ScipSymbolRecord, read_scip, source_hash, write_scip,
};
use glia_core::{Confidence, Edge, EdgeCategoryId, NodeId};
use glia_engine::generate_one;
use glia_engine::persist::{default_layout_dir, persist_result};
use glia_engine::why::why_edge;
use glia_graph::MergedGraph;
use glia_store::{is_gmap_stale, write_merged_sharded};

const FIXTURE: &str = "../bench/substrate-gap/fixtures/scip-python-dict-dispatch";
const REPOS_PY: &str = include_str!("../../bench/substrate-gap/fixtures/scip-python-dict-dispatch/svc/repos.py");
const HANDLERS_PY: &str =
    include_str!("../../bench/substrate-gap/fixtures/scip-python-dict-dispatch/svc/handlers.py");

/// `handle` keeps the method as a value (row 5) and calls the value (row 6).
const HANDLERS_VALUE_PY: &str =
    "from svc.repos import get_repo\n\n\ndef handle(row):\n    repo = get_repo(\"users\")\n    cb = repo.save\n    return cb(row)\n";

const ORDER_SAVE: &str = "scip-python python svc 0.1 `svc.repos`/OrderRepo#save().";
const USER_SAVE: &str = "scip-python python svc 0.1 `svc.repos`/UserRepo#save().";
/// Symbol ids: row order of the ascending symbol strings.
const ORDER: u32 = 0;
const USER: u32 = 1;

const HANDLE: &str = "svc::handlers::handle";
const USER_SAVE_Q: &str = "svc::repos::UserRepo::save";
const ORDER_SAVE_Q: &str = "svc::repos::OrderRepo::save";

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
}

fn def(s: u32, line: u32, name: &str) -> ScipDefRow {
    ScipDefRow { s, line, name: name.to_string() }
}

fn call(s: u32, line: u32) -> ScipRefRow {
    ScipRefRow { s, line, call: true, write: false, import: false }
}

fn value(s: u32, line: u32) -> ScipRefRow {
    ScipRefRow { call: false, ..call(s, line) }
}

/// The fixture's definitions: `UserRepo.save` on row 1, `OrderRepo.save` on
/// row 6 of svc/repos.py.
fn fixture_defs() -> Vec<ScipDefRow> {
    vec![def(USER, 1, "save"), def(ORDER, 6, "save")]
}

/// The two documents of a snapshot over `root`'s current sources, each hashed
/// as it is on disk now.
fn documents(root: &Path, repos_defs: Vec<ScipDefRow>, handler_refs: Vec<ScipRefRow>) -> Vec<ScipDocumentRecord> {
    let doc = |path: &str, defs, refs| ScipDocumentRecord {
        path: path.to_string(),
        language: "python".to_string(),
        source_hash: source_hash(&std::fs::read(root.join(path)).expect("fixture source")),
        defs,
        refs,
    };
    vec![doc("svc/handlers.py", Vec::new(), handler_refs), doc("svc/repos.py", repos_defs, Vec::new())]
}

fn symbols() -> Vec<ScipSymbolRecord> {
    [ORDER_SAVE, USER_SAVE]
        .iter()
        .enumerate()
        .map(|(id, s)| ScipSymbolRecord { id: id as u32, symbol: s.to_string(), implements: Vec::new() })
        .collect()
}

fn meta() -> ScipMeta {
    ScipMeta::new("scip-python".to_string(), "0.6.0".to_string(), String::new(), 0)
}

fn snapshot(root: &Path, repos_defs: Vec<ScipDefRow>, handler_refs: Vec<ScipRefRow>) {
    write_scip(root, meta(), &documents(root, repos_defs, handler_refs), &symbols()).expect("write snapshot");
}

/// `<tmp>/svc/{__init__,repos,handlers}.py`, `handlers.py` being `handlers`.
fn tree(handlers: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(d.path().join("svc")).unwrap();
    for (p, text) in [("svc/__init__.py", ""), ("svc/repos.py", REPOS_PY), ("svc/handlers.py", handlers)] {
        std::fs::write(d.path().join(p), text).unwrap();
    }
    d
}

fn build(root: &Path) -> MergedGraph {
    generate_one(&root.to_string_lossy()).expect("build").merged
}

fn id_of(m: &MergedGraph, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.iter())
        .filter(|(_, q)| q.as_str() == qname)
        .map(|(id, _)| *id)
        .min_by_key(|id| id.0)
        .unwrap_or_else(|| panic!("no node {qname}"))
}

fn edges<'a>(m: &'a MergedGraph, from: &str, to: &str, category: EdgeCategoryId) -> Vec<&'a Edge> {
    let (from, to) = (id_of(m, from), id_of(m, to));
    m.all_edges().filter(|e| e.from == from && e.to == to && e.category == category).collect()
}

fn scip_edges(m: &MergedGraph) -> Vec<&Edge> {
    m.all_edges().filter(|e| Evidence::of(e).is_some_and(|ev| ev.emitter.starts_with("scip:"))).collect()
}

const CHILD_ENV: &str = "GLIA_CE1D_CHILD";

/// The child half of [`child_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`] to `build\t<repo>` (build it) or `stale\t<layout>\t<repo>`
/// (ask `is_gmap_stale`), so its stderr (with `--nocapture`) carries the
/// markers.
#[test]
fn child_for_stderr() {
    let Ok(job) = std::env::var(CHILD_ENV) else { return };
    let parts: Vec<&str> = job.split('\t').collect();
    match parts.as_slice() {
        ["build", repo] => {
            build(Path::new(repo));
        }
        ["stale", layout, repo] => {
            is_gmap_stale(Path::new(layout), Path::new(repo));
        }
        other => panic!("unknown child job {other:?}"),
    }
}

/// The stderr lines starting with `prefix` of one child `job` (tab-separated).
fn child_stderr(job: &[&str], prefix: &str) -> Vec<String> {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_for_stderr", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, job.join("\t"))
        .output()
        .expect("re-run the test binary");
    assert!(out.status.success(), "child failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.starts_with(prefix)).map(String::from).collect()
}

/// The `[scip] ingest` marker of a build of `root`, after its `repo=<label> `.
fn ingest_marker(root: &Path) -> String {
    let label = root.to_string_lossy().to_string();
    let lines = child_stderr(&["build", &label], "[scip] ingest");
    assert_eq!(lines.len(), 1, "{lines:?}");
    let head = format!("[scip] ingest repo={label} ");
    lines[0].strip_prefix(&head).unwrap_or_else(|| panic!("{:?} does not start {head:?}", lines[0])).to_string()
}

/// Every file of a sharded store written from `m`, by name.
fn store_bytes(m: &MergedGraph) -> Vec<(String, Vec<u8>)> {
    let out = tempfile::tempdir().expect("tempdir");
    write_merged_sharded(m, out.path()).expect("write store");
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(out.path())
        .unwrap()
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read(e.path()).unwrap()))
        .collect();
    files.sort();
    files
}

/// The stage's payoff: glia binds `repo.save(row)` to nothing, the index binds
/// it to `UserRepo.save`, and the same-named `OrderRepo.save` stays unbound.
#[test]
fn dict_dispatch_call_is_added_from_scip() {
    let d = tree(HANDLERS_PY);
    snapshot(d.path(), fixture_defs(), vec![call(USER, 5)]);
    let m = build(d.path());

    let calls = edges(&m, HANDLE, USER_SAVE_Q, edge_category::CALLS);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let e = calls[0];
    assert_eq!(e.confidence, Confidence::Strong);
    let ev = Evidence::of(e).expect("one EVIDENCE cell");
    assert_eq!(
        ev,
        Evidence {
            emitter: "scip:scip-python".to_string(),
            rule: Some("call_site".to_string()),
            file: Some("svc/handlers.py".to_string()),
            line: Some(5),
            basis: Basis::Site,
        }
    );
    assert!(edges(&m, HANDLE, ORDER_SAVE_Q, edge_category::CALLS).is_empty());
    assert_eq!(scip_edges(&m).len(), 1);

    let why = why_edge(&m, HANDLE, USER_SAVE_Q, Some("CALLS")).expect("why");
    assert!(why.found);
    assert_eq!(why.edges.len(), 1);
    let row = &why.edges[0];
    assert_eq!((row.tier, row.emitter.as_deref(), row.rule.as_deref()), ("fact", Some("scip:scip-python"), Some("call_site")));
    assert_eq!(row.note.as_deref(), Some("resolved by a SCIP index (scip-python)"));
    // The editor line of `return repo.save(row)`.
    assert_eq!(row.site.as_ref().map(|s| (s.file.as_str(), s.line)), Some(("svc/handlers.py", Some(6))));

    assert_eq!(
        ingest_marker(d.path()),
        "tool=scip-python documents=2 stale=0 defs=2 bound=2 unbound=0 ambiguous=0 refs=1 imports=0 unowned=0 added=1 (calls=1 uses=0) confirmed=0 category_differs=0 self_refs=0"
    );
}

/// A document edited after the index is skipped whole: its references add
/// nothing, while the other document still binds its definitions.
#[test]
fn stale_document_is_skipped() {
    let d = tree(HANDLERS_PY);
    snapshot(d.path(), fixture_defs(), vec![call(USER, 5)]);
    let handlers = d.path().join("svc/handlers.py");
    std::fs::write(&handlers, format!("{HANDLERS_PY}# edited after the index\n")).unwrap();
    let m = build(d.path());
    assert!(scip_edges(&m).is_empty(), "{:?}", scip_edges(&m));
    assert!(edges(&m, HANDLE, USER_SAVE_Q, edge_category::CALLS).is_empty());
    assert_eq!(
        ingest_marker(d.path()),
        "tool=scip-python documents=2 stale=1 defs=2 bound=2 unbound=0 ambiguous=0 refs=0 imports=0 unowned=0 added=0 (calls=0 uses=0) confirmed=0 category_differs=0 self_refs=0"
    );
}

/// A definition whose identifier is not the node's name binds nothing, so the
/// reference to it adds nothing.
#[test]
fn def_name_mismatch_is_unbound() {
    let d = tree(HANDLERS_PY);
    snapshot(d.path(), vec![def(USER, 1, "sav"), def(ORDER, 6, "save")], vec![call(USER, 5)]);
    let m = build(d.path());
    assert!(scip_edges(&m).is_empty(), "{:?}", scip_edges(&m));
    let marker = ingest_marker(d.path());
    assert!(marker.contains(" defs=2 bound=1 unbound=1 ambiguous=0 refs=0 "), "{marker}");
    assert!(marker.contains(" added=0 "), "{marker}");
}

/// A method kept as a value is a USES `callable_ref`, never a CALLS.
#[test]
fn method_value_is_uses_callable_ref() {
    let d = tree(HANDLERS_VALUE_PY);
    snapshot(d.path(), fixture_defs(), vec![value(USER, 5)]);
    let m = build(d.path());
    let uses = edges(&m, HANDLE, USER_SAVE_Q, edge_category::USES);
    assert_eq!(uses.len(), 1, "{uses:?}");
    let ev = Evidence::of(uses[0]).expect("evidence");
    assert_eq!((ev.emitter.as_str(), ev.rule.as_deref(), ev.line), ("scip:scip-python", Some("callable_ref"), Some(5)));
    assert!(edges(&m, HANDLE, USER_SAVE_Q, edge_category::CALLS).is_empty());
    let marker = ingest_marker(d.path());
    assert!(marker.contains(" added=1 (calls=0 uses=1) "), "{marker}");
}

/// One symbol whose definition rows land in both classes binds neither.
#[test]
fn ambiguous_symbol_is_not_bound() {
    let d = tree(HANDLERS_PY);
    snapshot(d.path(), vec![def(USER, 1, "save"), def(USER, 6, "save")], vec![call(USER, 5)]);
    let m = build(d.path());
    assert!(scip_edges(&m).is_empty(), "{:?}", scip_edges(&m));
    let marker = ingest_marker(d.path());
    assert!(marker.contains(" defs=2 bound=0 unbound=0 ambiguous=1 refs=0 "), "{marker}");
    assert!(marker.contains(" added=0 "), "{marker}");
}

/// No snapshot, no change: a repo with an empty `.glia` builds the same store
/// bytes as without one, and prints no `[scip]` line.
#[test]
fn no_snapshot_is_byte_identical() {
    let d = tree(HANDLERS_PY);
    let bare = store_bytes(&build(d.path()));
    std::fs::create_dir_all(d.path().join(".glia")).unwrap();
    let with_dir = store_bytes(&build(d.path()));
    assert_eq!(bare, with_dir, "an empty .glia changed the store");
    let label = d.path().to_string_lossy().to_string();
    let scip = child_stderr(&["build", &label], "[scip]");
    assert!(scip.is_empty(), "{scip:?}");
}

/// The committed fixture builds the same store on one thread as on the
/// engine's pool, and carries exactly what its `key.json` expects and forbids.
#[test]
fn deterministic_at_any_thread_count() {
    let root = fixture_root();
    let pooled = build(&root);
    assert_eq!(edges(&pooled, HANDLE, USER_SAVE_Q, edge_category::CALLS).len(), 1);
    assert!(edges(&pooled, HANDLE, ORDER_SAVE_Q, edge_category::CALLS).is_empty());
    let single = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .stack_size(16 << 20)
        .build()
        .expect("one-thread pool")
        .install(|| build(&root));
    assert_eq!(store_bytes(&pooled), store_bytes(&single), "the thread count reached the graph");
}

/// The snapshot is a build input: rewriting it after a persist marks the
/// layout stale, naming the file.
#[test]
fn snapshot_is_a_build_input() {
    let d = tree(HANDLERS_PY);
    snapshot(d.path(), fixture_defs(), vec![call(USER, 5)]);
    let layout = default_layout_dir(d.path());
    persist_result(&generate_one(&d.path().to_string_lossy()).expect("build"), &layout, "test").expect("persist");
    assert!(!is_gmap_stale(&layout, d.path()), "fresh right after the persist");

    snapshot(d.path(), fixture_defs(), vec![value(USER, 5)]);
    assert!(is_gmap_stale(&layout, d.path()), "a rewritten snapshot must mark the layout stale");
    let lines = child_stderr(
        &["stale", &layout.to_string_lossy(), &d.path().to_string_lossy()],
        "[gmap] stale: external input changed",
    );
    assert_eq!(
        lines,
        ["[gmap] stale: external input changed (.glia/scip-snapshot/documents.jsonl) - regenerating"]
    );
}

/// The committed snapshot is the one [`write_dict_dispatch_fixture`] writes
/// from the fixture's current sources: editing a source without regenerating
/// fails here.
#[test]
fn fixture_snapshot_is_current() {
    let root = fixture_root();
    let snap = read_scip(&root).expect("the fixture's committed snapshot reads back complete");
    assert_eq!(snap.documents, documents(&root, fixture_defs(), vec![call(USER, 5)]));
    assert_eq!(snap.symbols, symbols());
    assert_eq!((snap.meta.tool.as_str(), snap.meta.tool_version.as_str()), ("scip-python", "0.6.0"));
    for doc in &snap.documents {
        let bytes = std::fs::read(root.join(&doc.path)).expect("fixture source");
        assert_eq!(doc.source_hash, source_hash(&bytes), "{} changed since the snapshot", doc.path);
    }
    assert!(!root.join(".glia/scip-snapshot/.gitignore").exists(), "write_scip writes no .gitignore");
}

/// Regenerates the committed fixture snapshot (see the module doc).
#[test]
#[ignore]
fn write_dict_dispatch_fixture() {
    let root = fixture_root();
    snapshot(&root, fixture_defs(), vec![call(USER, 5)]);
}
