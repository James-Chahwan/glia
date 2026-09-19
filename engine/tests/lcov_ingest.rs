//! LF.6c: the lcov rows of a `.glia/test-snapshot/` become COVERAGE cells
//! (`{"hit":H,"lines":N,"source":"lcov"}`, integers only) on the covered
//! file's MODULE (every DA record) and on each CLASS / FUNCTION / METHOD
//! whose POSITION span holds a DA record. A row maps to a repo file by its
//! `rel`, else by a unique boundary-aligned path tail; an ambiguous row maps
//! nothing, and a symbol that takes no record gets no cell.
//!
//! Snapshots are written through `code_domain::snapshots::write_tests`;
//! `fixture_graph_matches_the_key` builds the committed substrate-gap fixture
//! `test-reports-coverage` itself.

use std::path::Path;

use glia_code_domain::snapshots::{
    LcovFileRecord, META_FILE, TESTS_CASES_FILE, TESTS_LCOV_FILE, TestsMeta, data_hash, tests_dir, write_tests,
};
use glia_code_domain::{cell_type, node_kind};
use glia_core::{Cell, CellPayload, NodeId};
use glia_engine::generate_one;
use glia_graph::MergedGraph;
use serde_json::Value;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/test-reports-coverage";
const APP_PY: &str = include_str!("../../bench/substrate-gap/fixtures/test-reports-coverage/api/app.py");

/// A temp repo holding `files`.
fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    for (p, text) in files {
        let path = d.path().join(p);
        std::fs::create_dir_all(path.parent().expect("a parent")).unwrap();
        std::fs::write(path, text).unwrap();
    }
    d
}

fn row(sf: &str, rel: Option<&str>, lines: &[[u32; 2]]) -> LcovFileRecord {
    LcovFileRecord { sf: sf.into(), rel: rel.map(str::to_string), lines: lines.to_vec() }
}

/// The fixture's row: lines 1, 2 and 4 hit, line 5 not.
fn app_row() -> LcovFileRecord {
    row("api/app.py", Some("api/app.py"), &[[1, 1], [2, 1], [4, 1], [5, 0]])
}

fn snapshot(root: &Path, run: Option<&str>, lcov: &[LcovFileRecord]) {
    let meta = TestsMeta::new(run.map(str::to_string), vec!["coverage/lcov.info".into()], 0, 4);
    write_tests(root, meta, &[], lcov).expect("write snapshot");
}

fn build(root: &Path) -> MergedGraph {
    generate_one(&root.to_string_lossy()).expect("build").merged
}

/// The COVERAGE payload text of the node named `qname` (its first copy).
fn coverage_text(m: &MergedGraph, qname: &str) -> Option<String> {
    let ids: Vec<NodeId> = m.qnames_exact(qname);
    assert!(!ids.is_empty(), "no node {qname}");
    let cell = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter(|n| ids.contains(&n.id)))
        .find_map(|n| n.cells.iter().find(|c| c.kind == cell_type::COVERAGE))?;
    let CellPayload::Json(s) = &cell.payload else { panic!("COVERAGE is JSON: {:?}", cell.payload) };
    Some(s.clone())
}

/// `(lines, hit)` of the node named `qname`, or `None` without a cell.
fn coverage(m: &MergedGraph, qname: &str) -> Option<(u64, u64)> {
    let v: Value = serde_json::from_str(&coverage_text(m, qname)?).expect("COVERAGE parses");
    assert_eq!(v["source"], "lcov", "{v}");
    Some((v["lines"].as_u64().expect("lines is an integer"), v["hit"].as_u64().expect("hit is an integer")))
}

/// Every node carrying a COVERAGE cell, by qname, sorted.
fn covered(m: &MergedGraph) -> Vec<String> {
    let mut out: Vec<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(|n| n.cells.iter().any(|c| c.kind == cell_type::COVERAGE))
                .map(|n| g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default())
        })
        .collect();
    out.sort();
    out
}

#[test]
fn module_and_symbol_counts() {
    let d = tree(&[("api/app.py", APP_PY)]);
    snapshot(d.path(), None, &[app_row()]);
    let m = build(d.path());
    assert_eq!(covered(&m), ["api::app", "api::app::helper", "api::app::list_orders"]);
    // list_orders spans lines 1-2 (both hit); helper 4-5 (5 not hit); the
    // module takes all four DA records.
    assert_eq!(coverage(&m, "api::app::list_orders"), Some((2, 2)));
    assert_eq!(coverage(&m, "api::app::helper"), Some((2, 1)));
    assert_eq!(coverage(&m, "api::app"), Some((4, 3)));
    // Compact, sorted keys, integers: the fixture's `contains` reads this.
    assert_eq!(coverage_text(&m, "api::app::list_orders").as_deref(), Some(r#"{"hit":2,"lines":2,"source":"lcov"}"#));
    let kind = m.graphs.iter().find_map(|g| g.nav.kind_by_id.get(&m.qnames_exact("api::app")[0]).copied());
    assert_eq!(kind, Some(node_kind::MODULE));

    // A named run is carried beside the counts.
    snapshot(d.path(), Some("ci-812"), &[app_row()]);
    let m = build(d.path());
    assert_eq!(
        coverage_text(&m, "api::app::helper").as_deref(),
        Some(r#"{"hit":1,"lines":2,"run":"ci-812","source":"lcov"}"#)
    );
}

#[test]
fn class_spans_hold_their_methods() {
    let src = "class Cart:\n    def add(self, x):\n        return x\n\n    def drop(self):\n        return None\n";
    let d = tree(&[("shop/cart.py", src)]);
    snapshot(d.path(), None, &[row("shop/cart.py", Some("shop/cart.py"), &[[1, 1], [2, 1], [3, 4], [5, 1], [6, 0]])]);
    let m = build(d.path());
    assert_eq!(coverage(&m, "shop::cart::Cart"), Some((5, 4)));
    assert_eq!(coverage(&m, "shop::cart::Cart::add"), Some((2, 2)));
    assert_eq!(coverage(&m, "shop::cart::Cart::drop"), Some((2, 1)));
}

#[test]
fn zero_line_span_gets_no_cell() {
    let d = tree(&[("api/app.py", APP_PY)]);
    // No DA record inside helper (lines 4-5): unknown is not zero.
    snapshot(d.path(), None, &[row("api/app.py", Some("api/app.py"), &[[1, 1], [2, 0]])]);
    let m = build(d.path());
    assert_eq!(covered(&m), ["api::app", "api::app::list_orders"]);
    assert_eq!(coverage(&m, "api::app::helper"), None);
    assert_eq!(coverage(&m, "api::app::list_orders"), Some((2, 1)));
    assert_eq!(coverage(&m, "api::app"), Some((2, 1)));

    // A row with no DA record at all: not even the module takes a cell.
    snapshot(d.path(), None, &[row("api/app.py", Some("api/app.py"), &[])]);
    assert!(covered(&build(d.path())).is_empty());
}

#[test]
fn ambiguous_tail_is_unmatched() {
    let d = tree(&[("a/app.py", APP_PY), ("b/app.py", APP_PY), ("c/tool/run.py", "def main():\n    return 0\n")]);
    // `SF:app.py` from a coverage run in some subdirectory: a/app.py and
    // b/app.py both end in it, so neither is guessed.
    snapshot(d.path(), None, &[row("app.py", Some("app.py"), &[[1, 1], [2, 1]])]);
    assert!(covered(&build(d.path())).is_empty());

    // Positive controls: a unique tail (`SF:tool/run.py`, run in c/), and the
    // same row naming its file exactly.
    snapshot(
        d.path(),
        None,
        &[
            row("tool/run.py", Some("tool/run.py"), &[[1, 1], [2, 1]]),
            row("a/app.py", Some("a/app.py"), &[[1, 1], [2, 1]]),
        ],
    );
    let m = build(d.path());
    assert_eq!(covered(&m), ["a::app", "a::app::list_orders", "c::tool::run", "c::tool::run::main"]);
    assert_eq!(coverage(&m, "c::tool::run::main"), Some((2, 2)));
}

#[test]
fn absolute_sf_matches_via_rel() {
    let d = tree(&[("api/app.py", APP_PY), ("app.py", "def boot():\n    return 1\n")]);
    // An absolute SF under the repo: LF.6a stored its repo-relative `rel`.
    let abs = d.path().join("api/app.py").to_string_lossy().replace('\\', "/");
    snapshot(d.path(), None, &[row(&abs, Some("api/app.py"), &[[1, 1], [2, 1], [4, 1], [5, 0]])]);
    let m = build(d.path());
    assert_eq!(coverage(&m, "api::app::helper"), Some((2, 1)));
    assert_eq!(coverage(&m, "app::boot"), None, "the root app.py is not the row's file");

    // A CI checkout's absolute path (no `rel`): the longest repo-file tail
    // wins, api/app.py over the root app.py.
    snapshot(d.path(), None, &[row("/ci/work/repo/api/app.py", None, &[[4, 2], [5, 1]])]);
    let m = build(d.path());
    assert_eq!(covered(&m), ["api::app", "api::app::helper"]);
    assert_eq!(coverage(&m, "api::app::helper"), Some((2, 2)));

    // Rows no repo file ends: unmatched, nothing written.
    snapshot(d.path(), None, &[row("/usr/lib/python3/json/decoder.py", None, &[[1, 1]])]);
    assert!(covered(&build(d.path())).is_empty());
}

#[test]
fn partial_class_sums_its_parts() {
    // A C# partial class in two files is one CLASS node carrying a POSITION
    // per file: it takes the DA records inside both spans. Each file keeps
    // its own MODULE.
    let part = |method: &str| {
        format!("namespace Shop\n{{\n    public partial class Cart\n    {{\n        public int {method}() {{ return 1; }}\n    }}\n}}\n")
    };
    let d = tree(&[("Shop/CartAdd.cs", &part("Add")), ("Shop/CartDrop.cs", &part("Drop"))]);
    snapshot(
        d.path(),
        None,
        &[
            row("Shop/CartAdd.cs", Some("Shop/CartAdd.cs"), &[[5, 3], [7, 1]]),
            row("Shop/CartDrop.cs", Some("Shop/CartDrop.cs"), &[[5, 0]]),
        ],
    );
    let m = build(d.path());
    let of_kind = |kind| -> Vec<String> {
        covered(&m)
            .into_iter()
            .filter(|q| {
                let id = m.qnames_exact(q)[0];
                m.graphs.iter().find_map(|g| g.nav.kind_by_id.get(&id).copied()) == Some(kind)
            })
            .collect()
    };
    let [class] = of_kind(node_kind::CLASS).try_into().unwrap_or_else(|v: Vec<String>| panic!("one covered CLASS, got {v:?}"));
    // Line 5 of each part lies in the class span; line 7 (the namespace's
    // closing brace) does not.
    assert_eq!(coverage(&m, &class), Some((2, 1)), "{class}");
    let methods: Vec<(u64, u64)> = of_kind(node_kind::METHOD).iter().filter_map(|q| coverage(&m, q)).collect();
    assert_eq!(methods, [(1, 1), (1, 0)], "Add, then Drop");
}

#[test]
fn deterministic() {
    let d = tree(&[("api/app.py", APP_PY), ("a/app.py", APP_PY), ("b/app.py", APP_PY)]);
    snapshot(
        d.path(),
        Some("r1"),
        &[
            app_row(),
            row("/ci/w/a/app.py", None, &[[1, 3], [2, 0]]),
            row("a/app.py", Some("a/app.py"), &[[1, 1], [4, 1]]),
            row("app.py", Some("app.py"), &[[1, 1]]),
        ],
    );
    let cells = |m: &MergedGraph| -> Vec<Vec<Vec<Cell>>> {
        m.graphs.iter().map(|g| g.nodes.iter().map(|n| n.cells.clone()).collect()).collect()
    };
    let first = build(d.path());
    for _ in 0..3 {
        assert_eq!(cells(&build(d.path())), cells(&first));
    }
    // Two rows name a/app.py (one by rel, one by tail): summed per line
    // (line 1: 3 + 1 hits, line 2: 0, line 4: 1); `SF:app.py` is ambiguous.
    assert_eq!(coverage(&first, "a::app::list_orders"), Some((2, 1)));
    assert_eq!(coverage(&first, "a::app::helper"), Some((1, 1)));
    assert_eq!(coverage(&first, "a::app"), Some((3, 2)));
    assert_eq!(coverage(&first, "b::app"), None);
}

#[test]
fn no_lcov_writes_nothing() {
    let d = tree(&[("api/app.py", APP_PY)]);
    snapshot(d.path(), None, &[]);
    assert!(covered(&build(d.path())).is_empty());
}

#[test]
fn fixture_graph_matches_the_key() {
    // The committed meta's data_hash is snapshots::data_hash over the rows.
    let dir = tests_dir(Path::new(FIXTURE));
    let read = |f: &str| std::fs::read(dir.join(f)).expect("fixture snapshot file");
    let meta: TestsMeta = serde_json::from_slice(&read(META_FILE)).expect("meta parses");
    assert_eq!(meta.data_hash, data_hash(&[&read(TESTS_CASES_FILE), &read(TESTS_LCOV_FILE)]));

    let m = build(Path::new(FIXTURE));
    assert_eq!(covered(&m), ["api::app", "api::app::helper", "api::app::list_orders"]);
    assert_eq!(coverage(&m, "api::app::list_orders"), Some((2, 2)));
    assert_eq!(coverage(&m, "api::app::helper"), Some((2, 1)));
    assert_eq!(coverage(&m, "api::app"), Some((4, 3)));
    for (qname, want) in [("api::app::list_orders", r#""hit":2"#), ("api::app::helper", r#""hit":1"#)] {
        let text = coverage_text(&m, qname).expect("a COVERAGE cell");
        assert!(text.contains(want), "{qname}: {text}");
    }
}
