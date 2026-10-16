//! DT.5d — members a framework or the runtime invokes on an instance of
//! their owner follow it, and generated code is no dead_symbol row.
//!
//! Angular calls a lifecycle hook on every component / service it creates,
//! the forms API calls a ControlValueAccessor's members, and DI or `new`
//! runs a constructor: no source line spells any of those calls, so no edge
//! can be read for them. Liveness takes a walk-local hop from a live owner to
//! each such member (`answers::invoked_members`, over the code domain's
//! `profile::INVOKED_MEMBERS`), and `glia gaps` withholds such a member's
//! dead_symbol row while its owner has an incoming carry edge: the member is
//! a row exactly when its owner is. Nothing is stored as an edge.
//!
//! The fixture is `bench/substrate-gap/fixtures/framework-invoked-members`
//! (web, mobile, jobs), copied into a tempdir and built with
//! `generate_many`. At HEAD 5dfe4c72 it had 22 dead_symbol rows and
//! `[live] ... owners=0 implementers=0 reached=5/43`.
//!
//! One test per constructor row builds a one-file graph in that language,
//! declares the class an entrypoint (an ENTRYPOINT cell, LF.3b) and asserts
//! the constructor METHOD its parser mints is live while a plain sibling
//! method stays dead.

use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, NodeId};
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, GapsReport, gaps_report};
use glia_engine::{entrypoint_reachable, generate_many, generate_one};
use glia_graph::MergedGraph;

const FIXTURE: &str = "framework-invoked-members";
const DIRS: [&str; 3] = ["web", "mobile", "jobs"];

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/substrate-gap/fixtures").join(FIXTURE)
}

/// Copy a fixture's sources (never its key.json or a stray `.ai/` / `.glia/`).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for e in std::fs::read_dir(from).expect("read fixture dir").flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            if e.file_name() != ".ai" && e.file_name() != ".glia" {
                copy_tree(&src, &dst);
            }
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).expect("copy");
        }
    }
}

/// The fixture copied under `tmp` and built as one merge (web, mobile, jobs).
fn build_fixture(tmp: &Path) -> MergedGraph {
    copy_tree(&fixture_dir(), tmp);
    let roots: Vec<String> = DIRS.iter().map(|d| tmp.join(d).display().to_string()).collect();
    generate_many(&roots).unwrap_or_else(|e| panic!("generate_many: {e}")).merged
}

fn report(m: &MergedGraph) -> GapsReport {
    gaps_report(m, &[], &GapsOptions::default()).expect("known categories")
}

/// The dead_symbol rows' qnames, sorted.
fn dead(rep: &GapsReport) -> Vec<String> {
    let mut rows: Vec<String> =
        rep.rows.iter().filter(|r| r.category == DEAD_SYMBOL).map(|r| r.qname.clone()).collect();
    rows.sort();
    rows
}

/// The one node whose qname is exactly `qname`.
fn node(m: &MergedGraph, qname: &str) -> NodeId {
    let ids = m.qnames_exact(qname);
    assert_eq!(ids.len(), 1, "one node `{qname}`: {ids:?}");
    ids[0]
}

const ORDERS_PAGE: &str = "src::app::orders::orders.page::OrdersPage";
const ORDER_SERVICE: &str = "src::app::core::order.service::OrderService";
const AMOUNT_INPUT: &str = "src::app::core::amount-input.component::AmountInputComponent";
const REPORT_SERVICE: &str = "src::app::core::report.service::ReportService";

#[test]
fn dead_rows_are_exactly_the_unowned_members() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build_fixture(tmp.path());
    let rows = dead(&report(&m));
    let mut want = vec![
        "lib::models::friend::Friend::fromJson".to_string(),
        "lib::models::friend::Orphan".to_string(),
        "lib::models::friend::Orphan::Orphan".to_string(),
        REPORT_SERVICE.to_string(),
        format!("{REPORT_SERVICE}::constructor"),
        format!("{REPORT_SERVICE}::ngOnDestroy"),
        format!("{ORDERS_PAGE}::ngOnDestroyLater"),
    ];
    want.sort();
    assert_eq!(rows, want, "7 of HEAD's 22 rows stay");
}

#[test]
fn members_of_a_live_owner_are_live() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build_fixture(tmp.path());
    let live = entrypoint_reachable(&m);
    let mut reached: Vec<String> = ["ngOnInit", "ngOnDestroy", "constructor"]
        .iter()
        .map(|s| format!("{ORDERS_PAGE}::{s}"))
        .collect();
    reached.extend(
        ["constructor", "ngOnDestroy", "list"].iter().map(|s| format!("{ORDER_SERVICE}::{s}")),
    );
    reached.extend(
        ["writeValue", "registerOnChange", "registerOnTouched", "setDisabledState"]
            .iter()
            .map(|s| format!("{AMOUNT_INPUT}::{s}")),
    );
    reached.push("worker::Worker::__init__".to_string());
    for q in &reached {
        assert!(live.contains(&node(&m, q)), "{q} is live");
    }
    for q in [
        format!("{REPORT_SERVICE}::constructor"),
        format!("{REPORT_SERVICE}::ngOnDestroy"),
        format!("{ORDERS_PAGE}::ngOnDestroyLater"),
        "lib::models::friend::Friend::Friend".to_string(),
        "lib::models::friend::Orphan::Orphan".to_string(),
    ] {
        assert!(!live.contains(&node(&m, &q)), "{q} stays outside the live set");
    }
}

/// The hop is walk-local: no class gets a CALLS / USES edge to its own
/// METHOD, and the merged graph's edges are those the build made.
#[test]
fn no_hop_is_stored() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build_fixture(tmp.path());
    let before = m.all_edges().count();
    let _ = entrypoint_reachable(&m);
    let _ = report(&m);
    assert_eq!(m.all_edges().count(), before);
    for g in &m.graphs {
        for e in &g.edges {
            if e.category != edge_category::CALLS && e.category != edge_category::USES {
                continue;
            }
            let class = g.nav.kind_by_id.get(&e.from) == Some(&node_kind::CLASS);
            let own = g.nav.parent_of.get(&e.to) == Some(&e.from);
            assert!(
                !(class && own),
                "a stored {} edge from a class to its own member: {:?} -> {:?}",
                edge_category::name(e.category),
                g.nav.qname_by_id.get(&e.from),
                g.nav.qname_by_id.get(&e.to)
            );
        }
    }
}

#[test]
fn two_reports_over_one_graph_agree() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build_fixture(tmp.path());
    let (a, b) = (report(&m), report(&m));
    assert_eq!(a, b, "whole report");
    assert_eq!(entrypoint_reachable(&m), entrypoint_reachable(&m));
}

const CHILD_ENV: &str = "GLIA_DT5D_CHILD_REPORT";

/// The child half of [`report_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`], in which case it builds the fixture under that dir and
/// reports its dead_symbol rows, so its stderr (with `--nocapture`) carries
/// the markers.
#[test]
fn child_report_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        let m = build_fixture(Path::new(&dir));
        let mut opts = GapsOptions::default();
        opts.category = Some(DEAD_SYMBOL.to_string());
        gaps_report(&m, &[], &opts).expect("report");
    }
}

/// The stderr lines of one child report that start with `prefix`.
fn report_stderr(prefix: &str) -> Vec<String> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_report_for_stderr", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, tmp.path())
        .output()
        .expect("re-run the test binary");
    assert!(out.status.success(), "child failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with(prefix))
        .map(String::from)
        .collect()
}

#[test]
fn markers_count_the_hops_and_the_withheld_rows() {
    assert_eq!(
        report_stderr("[dead-framework]"),
        ["[dead-framework] dead_symbol rows withheld: invoked=1 generated=4"],
        "Friend::Friend follows Friend (called by fromJson); ChatMessage and three members"
    );
    let live = report_stderr("[live] profile=");
    assert_eq!(live.len(), 1, "one liveness walk per report: {live:?}");
    assert!(
        live[0].contains(" owners=0 implementers=0 invoked=10 reached="),
        "OrdersPage x3, OrderService x2, AmountInputComponent x4, Worker::__init__: {live:?}"
    );
}

// ---------------------------------------------------------------------------
// One graph per constructor row: the parser mints the METHOD the row names.
// ---------------------------------------------------------------------------

/// Build `files` as one repo, make the CLASS / STRUCT named `owner` an entry
/// (an ENTRYPOINT cell, as `[entrypoints]` writes one), and return whether
/// its METHOD `ctor` and its METHOD `other` are live. Panics when the parser
/// minted no such METHOD under that owner.
fn ctor_live(files: &[(&str, &str)], owner: &str, ctor: &str, other: &str) -> (bool, bool) {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, body) in files {
        let path = dir.path().join(rel);
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).expect("mkdir");
        }
        std::fs::write(path, body).expect("write");
    }
    let root = dir.path().to_str().expect("utf-8 temp path").to_string();
    let mut m = generate_one(&root).unwrap_or_else(|e| panic!("generate_one: {e}")).merged;
    let member = |m: &MergedGraph, name: &str| -> NodeId {
        let found: Vec<NodeId> = m
            .graphs
            .iter()
            .flat_map(|g| {
                g.nodes.iter().map(|n| n.id).filter(move |id| {
                    g.nav.kind_by_id.get(id) == Some(&node_kind::METHOD)
                        && g.nav.name_by_id.get(id).map(String::as_str) == Some(name)
                        && g.nav.parent_of.get(id).is_some_and(|p| {
                            g.nav.name_by_id.get(p).map(String::as_str) == Some(owner)
                        })
                })
            })
            .collect();
        assert_eq!(found.len(), 1, "one METHOD `{owner}::{name}` in {files:?}");
        found[0]
    };
    let (c, o) = (member(&m, ctor), member(&m, other));
    let owner_id = m
        .graphs
        .iter()
        .find_map(|g| g.nav.parent_of.get(&c).copied())
        .expect("the constructor's owner");
    let mut marked = 0;
    for g in &mut m.graphs {
        for n in g.nodes.iter_mut().filter(|n| n.id == owner_id) {
            n.cells.push(Cell {
                kind: cell_type::ENTRYPOINT,
                payload: CellPayload::Json(r#"{"pattern":"test","source":"config"}"#.into()),
            });
            marked += 1;
        }
    }
    assert!(marked > 0, "the owner node of `{owner}::{ctor}`");
    let live = entrypoint_reachable(&m);
    (live.contains(&c), live.contains(&o))
}

fn assert_ctor(files: &[(&str, &str)], owner: &str, ctor: &str) {
    assert_eq!(
        ctor_live(files, owner, ctor, "other"),
        (true, false),
        "(constructor live, plain sibling dead) for {files:?}"
    );
}

#[test]
fn ts_family_constructor() {
    let ts = "export class Repo {\n  constructor(private n: number) {}\n  other(): number { return this.n; }\n}\n";
    assert_ctor(&[("src/repo.ts", ts)], "Repo", "constructor");
    assert_ctor(&[("src/view.tsx", ts)], "Repo", "constructor");
    let js = "class Repo {\n  constructor(n) { this.n = n; }\n  other() { return this.n; }\n}\nmodule.exports = { Repo };\n";
    assert_ctor(&[("src/repo.js", js)], "Repo", "constructor");
    assert_ctor(&[("src/repo.jsx", js)], "Repo", "constructor");
}

#[test]
fn python_constructor() {
    let py = "class Worker:\n    def __init__(self, name):\n        self.name = name\n\n    def other(self):\n        return self.name\n";
    assert_ctor(&[("worker.py", py)], "Worker", "__init__");
}

#[test]
fn ruby_constructor() {
    let rb = "class Worker\n  def initialize(name)\n    @name = name\n  end\n\n  def other\n    @name\n  end\nend\n";
    assert_ctor(&[("worker.rb", rb)], "Worker", "initialize");
}

#[test]
fn php_constructor() {
    let php = "<?php\nclass Worker {\n    public function __construct($name) { $this->name = $name; }\n    public function other() { return $this->name; }\n}\n";
    assert_ctor(&[("Worker.php", php)], "Worker", "__construct");
}

#[test]
fn swift_constructor() {
    let swift = "class Worker {\n    let name: String\n    init(name: String) { self.name = name }\n    func other() -> String { return name }\n}\n";
    assert_ctor(&[("Worker.swift", swift)], "Worker", "init");
}

#[test]
fn named_like_owner_constructors() {
    let dart = "class Worker {\n  Worker(this.name);\n  final String name;\n  String other() => name;\n}\n";
    assert_ctor(&[("lib/worker.dart", dart)], "Worker", "Worker");
    let java = "package shop;\n\npublic class Worker {\n    private final String name;\n\n    public Worker(String name) {\n        this.name = name;\n    }\n\n    public String other() {\n        return name;\n    }\n}\n";
    assert_ctor(&[("src/main/java/shop/Worker.java", java)], "Worker", "Worker");
    let cs = "namespace Shop {\n    public class Worker {\n        private string name;\n        public Worker(string name) { this.name = name; }\n        public string other() { return name; }\n    }\n}\n";
    assert_ctor(&[("Worker.cs", cs)], "Worker", "Worker");
    let cpp = "class Worker {\npublic:\n    Worker(int n) : n_(n) {}\n    int other() { return n_; }\nprivate:\n    int n_;\n};\n";
    assert_ctor(&[("src/worker.cpp", cpp)], "Worker", "Worker");
    // A header-only class.
    assert_ctor(&[("include/worker.h", cpp)], "Worker", "Worker");
    assert_ctor(&[("include/worker.hpp", cpp)], "Worker", "Worker");
    // Out of line: declared in the header, defined in the source.
    let hpp = "class Worker {\npublic:\n    Worker(int n);\n    int other();\nprivate:\n    int n_;\n};\n";
    let out = "#include \"worker.hpp\"\n\nWorker::Worker(int n) : n_(n) {}\n\nint Worker::other() { return n_; }\n";
    assert_ctor(&[("src/worker.hpp", hpp), ("src/worker.cpp", out)], "Worker", "Worker");
}

/// The gates: a member named like a hook on a class with no Angular role,
/// and a `constructor` METHOD in a language whose constructor is spelled
/// otherwise, are not invoked members.
#[test]
fn look_alikes_stay_dead() {
    let ts = "export class Plain {\n  ngOnInit(): void {}\n  constructor() {}\n}\n";
    assert_eq!(
        ctor_live(&[("src/plain.ts", ts)], "Plain", "constructor", "ngOnInit"),
        (true, false),
        "a plain class's constructor runs, its ngOnInit look-alike is no hook"
    );
    let py = "class Worker:\n    def constructor(self):\n        pass\n\n    def other(self):\n        pass\n";
    assert_eq!(
        ctor_live(&[("worker.py", py)], "Worker", "constructor", "other"),
        (false, false),
        "Python's constructor is __init__"
    );
}
