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
//! CE.1e (the tests after `write_dict_dispatch_fixture`): a name-only glia
//! edge the index confirms is re-stamped `scip:<tool>` / `confirms:<old>`, a
//! located fact is left alone, an `is_implementation` relationship adds the
//! heritage edge glia missed, and a heuristic edge the index binds elsewhere is
//! counted, never changed. Their sources are the spec's `nameonly` /
//! `heritage` probes, inline.
//!
//! The fired_on markers are read from a child process: `child_for_stderr`
//! re-runs this binary on one tree with `--nocapture` and the parent reads its
//! stderr (`grep '^\[scip\] ingest'`, `grep '^\[scip\] confirm'`).

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

// ---- CE.1e: confirmation, heritage and contradictions ----

/// `Admin` inherits `Base` through a star import: glia binds the base class by
/// name alone (`graph:refs` `global_unique`, tier heuristic).
const NAMEONLY: &[(&str, &str)] = &[
    ("app/admin.py", "from app.star import *\n\n\nclass Admin(Base):\n    def go(self):\n        return 2\n"),
    ("app/base.py", "class Base:\n    def run(self):\n        return 1\n"),
    ("app/star.py", "from app.base import Base\n"),
];

/// `Admin`'s base is an attribute of a module object: glia binds no base at
/// all, and two classes are named `Base`.
const HERITAGE: &[(&str, &str)] = &[
    (
        "app/admin.py",
        "import importlib\n\nbase_mod = importlib.import_module(\"app.base\")\n\n\nclass Admin(base_mod.Base):\n    def go(self):\n        return 2\n",
    ),
    ("app/base.py", "class Base:\n    def run(self):\n        return 1\n"),
    ("app/other.py", "class Base:\n    def other(self):\n        return 3\n"),
];

/// A same-file call (`parser:python` `intra_file`) and an imported one
/// (`graph:calls` `import_binding`): both located facts.
const FACTS: &[(&str, &str)] = &[
    ("app/m.py", "def helper():\n    return 1\n\n\ndef run():\n    return helper()\n"),
    ("app/n.py", "from app.m import helper\n\n\ndef go():\n    return helper()\n"),
];

const ADMIN: &str = "app::admin::Admin";
const BASE: &str = "app::base::Base";

/// A temp tree holding `files`.
fn tree_of(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    for (p, text) in files {
        let path = d.path().join(p);
        std::fs::create_dir_all(path.parent().expect("a file in a directory")).unwrap();
        std::fs::write(path, text).unwrap();
    }
    d
}

/// A scip-python symbol string of package `app`.
fn sym(descriptor: &str) -> String {
    format!("scip-python python app 0.1 {descriptor}")
}

/// Write a snapshot over `root`'s current sources. `symbols` are
/// `(descriptor, implements)` in ascending descriptor order (the ids are
/// their row numbers); `docs` are `(path, defs, refs)` in path order.
fn snapshot_of(root: &Path, symbols: &[(&str, &[u32])], docs: Vec<(&str, Vec<ScipDefRow>, Vec<ScipRefRow>)>) {
    let symbols: Vec<ScipSymbolRecord> = symbols
        .iter()
        .enumerate()
        .map(|(id, (d, implements))| ScipSymbolRecord { id: id as u32, symbol: sym(d), implements: implements.to_vec() })
        .collect();
    let documents: Vec<ScipDocumentRecord> = docs
        .into_iter()
        .map(|(path, defs, refs)| ScipDocumentRecord {
            path: path.to_string(),
            language: "python".to_string(),
            source_hash: source_hash(&std::fs::read(root.join(path)).expect("source")),
            defs,
            refs,
        })
        .collect();
    write_scip(root, meta(), &documents, &symbols).expect("write snapshot");
}

fn import(s: u32, line: u32) -> ScipRefRow {
    ScipRefRow { import: true, ..value(s, line) }
}

/// Every `[scip]` stderr line of one build of `root`, `repo=<label> ` cut.
fn scip_lines(root: &Path) -> Vec<String> {
    let label = root.to_string_lossy().to_string();
    let head = format!("repo={label} ");
    child_stderr(&["build", &label], "[scip]").into_iter().map(|l| l.replacen(&head, "", 1)).collect()
}

/// The one line of `lines` starting with `prefix`, the prefix cut.
fn line_after<'a>(lines: &'a [String], prefix: &str) -> &'a str {
    let hits: Vec<&String> = lines.iter().filter(|l| l.starts_with(prefix)).collect();
    assert_eq!(hits.len(), 1, "{prefix}: {lines:?}");
    &hits[0][prefix.len()..]
}

fn evidence_cells(e: &Edge) -> usize {
    e.cells.iter().filter(|c| c.kind == glia_code_domain::cell_type::EVIDENCE).count()
}

/// The spec probe: a name-only INHERITS_FROM the index confirms becomes a
/// located SCIP fact that still names its first emitter; it stays one edge
/// with one EVIDENCE cell.
#[test]
fn name_only_inherits_is_confirmed() {
    let d = tree_of(NAMEONLY);
    let before = build(d.path());
    let old = edges(&before, ADMIN, BASE, edge_category::INHERITS_FROM);
    assert_eq!(old.len(), 1, "{old:?}");
    let old_ev = Evidence::of(old[0]).expect("evidence");
    assert_eq!((old_ev.emitter.as_str(), old_ev.rule.as_deref()), ("graph:refs", Some("global_unique")));

    // Admin (id 0, row 3) implements Base (id 1, row 0); the base-class
    // expression is a reference on Admin's row, and star.py imports Base.
    snapshot_of(
        d.path(),
        &[("`app.admin`/Admin#", &[1]), ("`app.base`/Base#", &[])],
        vec![
            ("app/admin.py", vec![def(0, 3, "Admin")], vec![value(1, 3)]),
            ("app/base.py", vec![def(1, 0, "Base")], vec![]),
            ("app/star.py", vec![], vec![import(1, 0)]),
        ],
    );
    let m = build(d.path());
    let inherits = edges(&m, ADMIN, BASE, edge_category::INHERITS_FROM);
    assert_eq!(inherits.len(), 1, "{inherits:?}");
    let e = inherits[0];
    assert_eq!(evidence_cells(e), 1);
    assert_eq!(e.confidence, Confidence::Strong);
    assert_eq!(
        Evidence::of(e),
        Some(Evidence {
            emitter: "scip:scip-python".to_string(),
            rule: Some("confirms:graph:refs/global_unique".to_string()),
            file: Some("app/admin.py".to_string()),
            line: Some(3),
            basis: Basis::Site,
        })
    );
    // The class-header reference is the heritage itself: no USES besides it.
    assert!(edges(&m, ADMIN, BASE, edge_category::USES).is_empty());

    let why = why_edge(&m, ADMIN, BASE, Some("INHERITS_FROM")).expect("why");
    assert_eq!(why.edges.len(), 1);
    let row = &why.edges[0];
    assert_eq!(row.tier, "fact");
    let note = row.note.as_deref().unwrap_or_default();
    assert_eq!(
        note,
        "confirmed by a SCIP index (scip-python) at app/admin.py:4; first emitted by graph:refs (global_unique)"
    );
    assert!(note.contains("first emitted by graph:refs (global_unique)"), "{note}");

    let lines = scip_lines(d.path());
    assert_eq!(
        line_after(&lines, "[scip] ingest "),
        "tool=scip-python documents=3 stale=0 defs=2 bound=2 unbound=0 ambiguous=0 refs=2 imports=1 unowned=0 added=0 (calls=0 uses=0) confirmed=0 category_differs=1 self_refs=0"
    );
    assert_eq!(
        line_after(&lines, "[scip] confirm "),
        "upgraded=1 confirmed_fact=0 confirmed_other=0 relationships=1 added_implements=0 added_inherits=0 contradicted=0 other_kinds=0"
    );
}

/// The index's relationship adds the base glia never bound, to the one of the
/// two same-named classes it names, and the class-header reference adds no
/// USES beside it.
#[test]
fn missing_heritage_is_added() {
    let d = tree_of(HERITAGE);
    let other = "app::other::Base";
    assert!(edges(&build(d.path()), ADMIN, BASE, edge_category::INHERITS_FROM).is_empty());

    snapshot_of(
        d.path(),
        &[("`app.admin`/Admin#", &[1]), ("`app.base`/Base#", &[]), ("`app.other`/Base#", &[])],
        vec![
            ("app/admin.py", vec![def(0, 5, "Admin")], vec![value(1, 5)]),
            ("app/base.py", vec![def(1, 0, "Base")], vec![]),
            ("app/other.py", vec![def(2, 0, "Base")], vec![]),
        ],
    );
    let m = build(d.path());
    let inherits = edges(&m, ADMIN, BASE, edge_category::INHERITS_FROM);
    assert_eq!(inherits.len(), 1, "{inherits:?}");
    let e = inherits[0];
    assert_eq!(e.confidence, Confidence::Strong);
    assert_eq!(evidence_cells(e), 1);
    assert_eq!(
        Evidence::of(e),
        Some(Evidence::emitter("scip:scip-python").rule("implementation").at("app/admin.py", 5))
    );
    assert!(edges(&m, ADMIN, other, edge_category::INHERITS_FROM).is_empty());
    let (admin, other_id) = (id_of(&m, ADMIN), id_of(&m, other));
    assert!(!m.all_edges().any(|e| e.from == admin && e.to == other_id), "an edge to app::other::Base");
    assert!(edges(&m, ADMIN, BASE, edge_category::USES).is_empty());

    let why = why_edge(&m, ADMIN, BASE, None).expect("why");
    assert!(why.found);
    assert_eq!(why.edges.len(), 1);
    assert_eq!((why.edges[0].tier, why.edges[0].rule.as_deref()), ("fact", Some("implementation")));

    let lines = scip_lines(d.path());
    assert!(line_after(&lines, "[scip] ingest ").contains(" added=0 (calls=0 uses=0) confirmed=0 category_differs=1 "));
    assert_eq!(
        line_after(&lines, "[scip] confirm "),
        "upgraded=0 confirmed_fact=0 confirmed_other=0 relationships=1 added_implements=0 added_inherits=1 contradicted=0 other_kinds=0"
    );
}

/// Located fact edges the index agrees with keep their own evidence: the
/// build is the no-snapshot build, byte for byte.
#[test]
fn fact_edges_are_untouched() {
    let d = tree_of(FACTS);
    let bare = build(d.path());
    for (from, emitter) in [("app::m::run", "parser:python"), ("app::n::go", "graph:calls")] {
        let calls = edges(&bare, from, "app::m::helper", edge_category::CALLS);
        assert_eq!(calls.len(), 1, "{calls:?}");
        let ev = Evidence::of(calls[0]).expect("evidence");
        assert_eq!((ev.emitter.as_str(), ev.basis), (emitter, Basis::Site));
    }
    snapshot_of(
        d.path(),
        &[("`app.m`/helper().", &[]), ("`app.m`/run().", &[]), ("`app.n`/go().", &[])],
        vec![
            ("app/m.py", vec![def(0, 0, "helper"), def(1, 4, "run")], vec![call(0, 5)]),
            ("app/n.py", vec![def(2, 3, "go")], vec![import(0, 0), call(0, 4)]),
        ],
    );
    let m = build(d.path());
    assert_eq!(store_bytes(&bare), store_bytes(&m), "a confirmed fact changed the store");
    let lines = scip_lines(d.path());
    assert!(line_after(&lines, "[scip] ingest ").contains(" added=0 (calls=0 uses=0) confirmed=2 "));
    assert_eq!(
        line_after(&lines, "[scip] confirm "),
        "upgraded=0 confirmed_fact=2 confirmed_other=0 relationships=0 added_implements=0 added_inherits=0 contradicted=0 other_kinds=0"
    );
}

/// The index names another class at the site of a name-only edge: counted and
/// printed, and the glia edge stays as it was.
#[test]
fn contradiction_is_counted_not_applied() {
    let mut files = NAMEONLY.to_vec();
    files.push(("app/other.py", "class Other:\n    pass\n"));
    let d = tree_of(&files);
    let before = build(d.path());
    let old: Vec<Vec<u8>> = edges(&before, ADMIN, BASE, edge_category::INHERITS_FROM)
        .iter()
        .map(|e| serde_json::to_vec(&Evidence::of(e)).unwrap())
        .collect();
    assert_eq!(old.len(), 1);

    snapshot_of(
        d.path(),
        &[("`app.admin`/Admin#", &[2]), ("`app.base`/Base#", &[]), ("`app.other`/Other#", &[])],
        vec![
            ("app/admin.py", vec![def(0, 3, "Admin")], vec![]),
            ("app/base.py", vec![def(1, 0, "Base")], vec![]),
            ("app/other.py", vec![def(2, 0, "Other")], vec![]),
        ],
    );
    let m = build(d.path());
    let kept: Vec<Vec<u8>> = edges(&m, ADMIN, BASE, edge_category::INHERITS_FROM)
        .iter()
        .map(|e| serde_json::to_vec(&Evidence::of(e)).unwrap())
        .collect();
    assert_eq!(kept, old, "a contradicted edge is never changed");
    // The index's own relationship is still added.
    assert_eq!(edges(&m, ADMIN, "app::other::Other", edge_category::INHERITS_FROM).len(), 1);

    let lines = scip_lines(d.path());
    assert_eq!(
        line_after(&lines, "[scip] confirm "),
        "upgraded=0 confirmed_fact=0 confirmed_other=0 relationships=1 added_implements=0 added_inherits=1 contradicted=1 other_kinds=0"
    );
    assert_eq!(
        line_after(&lines, "[scip] contradicts "),
        "app::admin::Admin -[INHERITS_FROM graph:refs/global_unique]-> app::base::Base at app/admin.py:4; index says app::other::Other"
    );
}
