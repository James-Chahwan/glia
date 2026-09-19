//! LF.6b: a `.glia/test-snapshot/` becomes FAIL cells (9) on the failing test
//! (mapped by file_line, qname or an unambiguous bare name) and on the non-test
//! frames its trace implicates, capped per node, deterministic, and with no
//! secret from a report reaching the graph or the `.gmap`.
//!
//! Snapshots are written directly: through `code_domain::snapshots::write_tests`,
//! or, for the leak gate, as hand-edited bytes whose `data_hash` is computed
//! with `snapshots::data_hash`. `fixture_graph_matches_the_key` builds the
//! committed substrate-gap fixture `test-reports-fail` itself.

use std::path::Path;

use repo_graph_code_domain::snapshots::{
    META_FILE, SOURCE_JUNIT, SOURCE_LOG, STATUS_FAILED, TESTS_CASES_FILE, TESTS_LCOV_FILE, TestCaseRecord,
    TestsMeta, data_hash, tests_dir, write_tests,
};
use repo_graph_code_domain::{cell_type, node_kind};
use repo_graph_core::{CellPayload, NodeId};
use repo_graph_engine::{BuildOptions, generate_one, generate_one_opts};
use repo_graph_graph::MergedGraph;
use repo_graph_store::write_merged_sharded;
use serde_json::Value;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/test-reports-fail";
const APP_PY: &str = include_str!("../../bench/substrate-gap/fixtures/test-reports-fail/api/app.py");
const TEST_APP_PY: &str = include_str!("../../bench/substrate-gap/fixtures/test-reports-fail/tests/test_app.py");

/// The pytest long traceback of the fixture's failure (tests/test_app.py:4
/// -> api/app.py:2 -> api/app.py:5).
const PYTEST_TRACE: &str = "def test_list_orders():\n>       assert list_orders() == []\n\ntests/test_app.py:4: \n\
_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ \napi/app.py:2: in list_orders\n    return helper()\n\
_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ \n\n    def helper():\n>       raise ValueError(\"boom\")\n\
E       ValueError: boom\n\napi/app.py:5: ValueError";

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

/// The fixture's two source files.
fn pytest_tree() -> tempfile::TempDir {
    tree(&[("api/app.py", APP_PY), ("tests/test_app.py", TEST_APP_PY)])
}

fn failed(classname: Option<&str>, name: &str) -> TestCaseRecord {
    TestCaseRecord {
        source: SOURCE_JUNIT.into(),
        report: "reports/junit.xml".into(),
        suite: Some("pytest".into()),
        classname: classname.map(str::to_string),
        name: name.into(),
        file: None,
        line: None,
        status: STATUS_FAILED.into(),
        message: None,
        trace: None,
        redacted: false,
    }
}

/// The fixture's one case: tests.test_app::test_list_orders at line 4.
fn list_orders_case() -> TestCaseRecord {
    TestCaseRecord {
        file: Some("tests/test_app.py".into()),
        line: Some(4),
        message: Some("ValueError: boom".into()),
        trace: Some(PYTEST_TRACE.into()),
        ..failed(Some("tests.test_app"), "test_list_orders")
    }
}

fn snapshot(root: &Path, run: Option<&str>, cases: &[TestCaseRecord]) {
    let meta = TestsMeta::new(run.map(str::to_string), vec!["reports/junit.xml".into()], 0, 3);
    write_tests(root, meta, cases, &[]).expect("write snapshot");
}

fn build(root: &Path) -> MergedGraph {
    generate_one(&root.to_string_lossy()).expect("build").merged
}

/// The node(s) whose qname is `qname`, sorted.
fn ids_of(m: &MergedGraph, qname: &str) -> Vec<NodeId> {
    m.qnames_exact(qname)
}

/// The FAIL entries of the node named `qname` (its first copy), or empty.
fn fail(m: &MergedGraph, qname: &str) -> Vec<Value> {
    let ids = ids_of(m, qname);
    let cell = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter(|n| ids.contains(&n.id)))
        .find_map(|n| n.cells.iter().find(|c| c.kind == cell_type::FAIL));
    let Some(cell) = cell else { return Vec::new() };
    let CellPayload::Json(s) = &cell.payload else { panic!("FAIL is JSON: {:?}", cell.payload) };
    serde_json::from_str::<Vec<Value>>(s).expect("FAIL is an entry array")
}

/// Every node carrying a FAIL cell, by qname, sorted.
fn failing(m: &MergedGraph) -> Vec<String> {
    let mut out: Vec<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(|n| n.cells.iter().any(|c| c.kind == cell_type::FAIL))
                .map(|n| g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default())
        })
        .collect();
    out.sort();
    out
}

#[test]
fn pytest_case_maps_by_file_line() {
    let d = pytest_tree();
    snapshot(d.path(), None, &[list_orders_case()]);
    let m = build(d.path());
    let entries = fail(&m, "tests::test_app::test_list_orders");
    assert_eq!(entries.len(), 1, "{entries:?}");
    let e = &entries[0];
    assert_eq!(e["via"], "file_line");
    assert_eq!(e["role"], "test");
    assert_eq!(e["message"], "ValueError: boom");
    assert_eq!(e["id"], "latest:tests.test_app::test_list_orders");
    assert_eq!(e["test"], "tests.test_app::test_list_orders");
    assert_eq!((&e["source"], &e["status"], &e["report"]), (&"junit".into(), &"failed".into(), &"reports/junit.xml".into()));
    assert!(e.get("run").is_none() && e.get("redacted").is_none() && e.get("trace").is_none(), "{e}");

    // A fact input: `--no-overlay` keeps it.
    let no_overlay = generate_one_opts(&d.path().to_string_lossy(), false, &BuildOptions::default().with_overlay(false))
        .expect("build")
        .merged;
    assert_eq!(fail(&no_overlay, "tests::test_app::test_list_orders"), entries);

    // A named run keys the ids by it.
    snapshot(d.path(), Some("ci-812"), &[list_orders_case()]);
    let e = &fail(&build(d.path()), "tests::test_app::test_list_orders")[0];
    assert_eq!((&e["id"], &e["run"]), (&"ci-812:tests.test_app::test_list_orders".into(), &"ci-812".into()));
}

#[test]
fn junit_java_suffix_ladder() {
    let d = tree(&[
        (
            "src/main/java/com/example/UserService.java",
            "package com.example;\n\npublic class UserService {\n    public String getUser(int id) {\n        return null;\n    }\n}\n",
        ),
        (
            "src/test/java/com/example/UserServiceTest.java",
            "package com.example;\n\nimport org.junit.jupiter.api.Test;\n\nclass UserServiceTest {\n    @Test\n    void testGetUser() {\n        new UserService().getUser(1);\n    }\n}\n",
        ),
    ]);
    // JUnit gives no file and no line for a Java case: the qname rung maps it.
    snapshot(d.path(), None, &[failed(Some("com.example.UserServiceTest"), "testGetUser")]);
    let m = build(d.path());
    let [method] = failing(&m).try_into().unwrap_or_else(|v: Vec<String>| panic!("one FAIL node, got {v:?}"));
    assert!(method.ends_with("::com::example::UserServiceTest::testGetUser"), "{method}");
    assert_eq!(m.graphs.iter().find_map(|g| g.nav.kind_by_id.get(&ids_of(&m, &method)[0])), Some(&node_kind::METHOD));
    let e = &fail(&m, &method)[0];
    assert_eq!((&e["via"], &e["test"]), (&"qname".into(), &"com.example.UserServiceTest::testGetUser".into()));

    // A package the repo does not declare: the ladder walks down to the
    // class-qualified suffix, never to resolve's bare-name fallback alone.
    snapshot(d.path(), None, &[failed(Some("org.other.UserServiceTest"), "testGetUser()")]);
    let m = build(d.path());
    let e = &fail(&m, &method)[0];
    assert_eq!(e["via"], "qname", "{e}");
    assert_eq!(e["test"], "org.other.UserServiceTest::testGetUser()");
}

#[test]
fn ambiguous_bare_name_is_not_guessed() {
    let d = tree(&[
        ("tests/test_a.py", "def test_x():\n    assert False\n"),
        ("tests/test_b.py", "def test_x():\n    assert False\n"),
        ("tests/test_c.py", "def test_only():\n    assert False\n"),
    ]);
    let log = |classname: Option<&str>, name: &str| TestCaseRecord {
        source: SOURCE_LOG.into(),
        report: "ci.log".into(),
        suite: None,
        ..failed(classname, name)
    };
    snapshot(
        d.path(),
        None,
        &[
            // No file, no classname: two FUNCTIONs are named test_x.
            log(None, "test_x"),
            // A classname no qname carries: resolve's bare-name fallback
            // finds a test_x, which is no qname match.
            log(Some("nowhere.mod"), "test_x"),
            // One FUNCTION carries the name: the HEURISTIC rung maps it.
            log(None, "test_only"),
        ],
    );
    let m = build(d.path());
    assert_eq!(failing(&m), ["tests::test_c::test_only"]);
    let e = &fail(&m, "tests::test_c::test_only")[0];
    assert_eq!((&e["via"], &e["source"]), (&"name".into(), &"log".into()));
}

#[test]
fn trace_implicates_non_test_frames() {
    // The test calls a helper in its own file (not a test-shaped name, not
    // under tests/: only the same-file rule leaves it out), which calls a
    // conftest helper (ORIGIN test_fixture), which calls into the app.
    let test_py = "from tests.conftest import make_orders\n\ndef build_orders():\n    return make_orders()\n\ndef test_list_orders():\n    assert build_orders() == []\n";
    let conftest = "def make_orders():\n    return list_orders()\n";
    let d = tree(&[("api/app.py", APP_PY), ("checks/test_app.py", test_py), ("tests/conftest.py", conftest)]);
    let trace = "checks/test_app.py:7: in test_list_orders\n    assert build_orders() == []\n\
                 checks/test_app.py:4: in build_orders\n    return make_orders()\n\
                 tests/conftest.py:2: in make_orders\n    return list_orders()\n\
                 /ci/work/repo/api/app.py:2: in list_orders\n    return helper()\n\
                 api/app.py:5: in helper\n    raise ValueError(\"boom\")\nE   ValueError: boom";
    let case = TestCaseRecord {
        file: Some("checks/test_app.py".into()),
        line: Some(6),
        message: Some("ValueError: boom".into()),
        trace: Some(trace.into()),
        ..failed(Some("checks.test_app"), "test_list_orders")
    };
    snapshot(d.path(), None, &[case]);
    let m = build(d.path());
    // Frames 1 (build_orders, the test's file) and 2 (make_orders, a test
    // fixture) are not implicated.
    assert_eq!(failing(&m), ["api::app::helper", "api::app::list_orders", "checks::test_app::test_list_orders"]);
    for (qname, frame) in [("api::app::list_orders", 3), ("api::app::helper", 4)] {
        let entries = fail(&m, qname);
        assert_eq!(entries.len(), 1, "{qname}: {entries:?}");
        let e = &entries[0];
        assert_eq!(e["role"], "implicated", "{qname}");
        assert_eq!(e["frame"], frame, "{qname}");
        assert_eq!(e["id"], format!("latest:checks.test_app::test_list_orders#{frame}"));
        assert_eq!(e["message"], "ValueError: boom");
        assert!(e.get("via").is_none(), "{e}");
    }
    let t = &fail(&m, "checks::test_app::test_list_orders")[0];
    assert_eq!((&t["role"], &t["via"]), (&"test".into(), &"file_line".into()));
}

#[test]
fn per_node_cap() {
    let d = pytest_tree();
    // 25 failures whose traces all end in helper; none of the tests exists.
    let cases: Vec<TestCaseRecord> = (0..25)
        .map(|i| TestCaseRecord {
            trace: Some("api/app.py:5: ValueError".into()),
            ..failed(Some("tests.test_gone"), &format!("test_{i:02}"))
        })
        .collect();
    snapshot(d.path(), None, &cases);
    let m = build(d.path());
    let entries = fail(&m, "api::app::helper");
    assert_eq!(entries.len(), 20);
    let ids: Vec<&str> = entries.iter().map(|e| e["id"].as_str().expect("id")).collect();
    let want: Vec<String> = (0..20).map(|i| format!("latest:tests.test_gone::test_{i:02}#0")).collect();
    assert_eq!(ids, want, "the lowest ids stay");
    assert!(entries.iter().all(|e| e["role"] == "implicated"));
}

#[test]
fn junit_and_log_copies_fold() {
    let d = pytest_tree();
    let from_log = TestCaseRecord {
        source: SOURCE_LOG.into(),
        report: "ci.log".into(),
        suite: None,
        message: Some("AssertionError".into()),
        trace: None,
        ..list_orders_case()
    };
    snapshot(d.path(), None, &[from_log.clone(), list_orders_case()]);
    let m = build(d.path());
    let entries = fail(&m, "tests::test_app::test_list_orders");
    assert_eq!(entries.len(), 1, "one failure, one entry: {entries:?}");
    assert_eq!((&entries[0]["source"], &entries[0]["message"]), (&"junit".into(), &"ValueError: boom".into()));

    // A log copy with no classname folds onto the same test too.
    let bare = TestCaseRecord { classname: None, ..from_log };
    snapshot(d.path(), None, &[bare, list_orders_case()]);
    let entries = fail(&build(d.path()), "tests::test_app::test_list_orders");
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["source"], "junit");
}

#[test]
fn deterministic() {
    let d = pytest_tree();
    let mut cases = vec![list_orders_case()];
    cases.extend((0..6).map(|i| TestCaseRecord {
        trace: Some(PYTEST_TRACE.into()),
        ..failed(Some("tests.test_app"), &format!("test_list_orders[{i}]"))
    }));
    snapshot(d.path(), Some("r1"), &cases);
    let cells = |m: &MergedGraph| -> Vec<Vec<Vec<repo_graph_core::Cell>>> {
        m.graphs.iter().map(|g| g.nodes.iter().map(|n| n.cells.clone()).collect()).collect()
    };
    let first = build(d.path());
    for _ in 0..3 {
        assert_eq!(cells(&build(d.path())), cells(&first));
    }
    // The parametrised copies map onto the test by qname, beside its own entry.
    let entries = fail(&first, "tests::test_app::test_list_orders");
    assert_eq!(entries.len(), 7);
    assert_eq!(entries.iter().filter(|e| e["via"] == "qname").count(), 6);
}

#[test]
fn secrets_in_a_report_never_reach_the_graph_or_the_gmap() {
    // Assembled at run time so no scanner mistakes this source for a leak.
    let stripe = format!("sk_{}_{}", "live", "4eC39HqLyjWDarjtT1zdp7dc");
    let jwt = ["eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9", "eyJzdWIiOiIxMjM0NTY3ODkwIn0", "dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"]
        .join(".");
    let secrets = [stripe.as_str(), jwt.as_str()];

    let d = pytest_tree();
    // A hand-edited snapshot: the secrets sit in the stored bytes, and only
    // the reader's re-sanitising stands between them and the build.
    let case = TestCaseRecord {
        message: Some(format!("charge failed for {stripe}")),
        trace: Some(format!("{PYTEST_TRACE}\nsession {jwt}")),
        ..list_orders_case()
    };
    let cases = format!("{}\n", serde_json::to_string(&case).unwrap()).into_bytes();
    for secret in secrets {
        assert!(String::from_utf8_lossy(&cases).contains(secret), "control: the snapshot carries {secret}");
    }
    let dir = tests_dir(d.path());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(TESTS_CASES_FILE), &cases).unwrap();
    std::fs::write(dir.join(TESTS_LCOV_FILE), b"").unwrap();
    let meta = TestsMeta {
        cases_total: 1,
        failed: 1,
        data_hash: data_hash(&[&cases, b""]),
        ..TestsMeta::new(None, vec!["reports/junit.xml".into()], 0, 0)
    };
    std::fs::write(dir.join(META_FILE), serde_json::to_vec_pretty(&meta).unwrap()).unwrap();

    let m = build(d.path());
    // Positive control: the failure landed, redacted and flagged.
    let e = &fail(&m, "tests::test_app::test_list_orders")[0];
    assert_eq!(e["message"], "charge failed for ***");
    assert_eq!((&e["redacted"], &e["via"]), (&true.into(), &"file_line".into()));
    assert_eq!(fail(&m, "api::app::helper")[0]["redacted"], true);

    let graph = format!("{m:?}");
    assert!(graph.contains("charge failed for ***"), "control: the dump holds the FAIL cell");
    let layout = tempfile::tempdir().unwrap();
    write_merged_sharded(&m, layout.path()).expect("write layout");
    let mut gmap = Vec::new();
    let mut dirs = vec![layout.path().to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                dirs.push(p);
            } else {
                gmap.extend(std::fs::read(p).unwrap());
            }
        }
    }
    assert!(!gmap.is_empty(), "control: a layout was written");
    for secret in secrets {
        assert!(!graph.contains(secret), "{secret} reached the graph");
        assert!(!String::from_utf8_lossy(&gmap).contains(secret), "{secret} reached the .gmap layout");
    }
}

#[test]
fn no_snapshot_writes_nothing() {
    let d = pytest_tree();
    assert!(failing(&build(d.path())).is_empty());
}

#[test]
fn fixture_graph_matches_the_key() {
    let m = build(Path::new(FIXTURE));
    assert_eq!(failing(&m), ["api::app::helper", "api::app::list_orders", "tests::test_app::test_list_orders"]);
    let t = &fail(&m, "tests::test_app::test_list_orders")[0];
    assert_eq!((&t["via"], &t["message"]), (&"file_line".into(), &"ValueError: boom".into()));
    for (qname, frame) in [("api::app::list_orders", 1), ("api::app::helper", 2)] {
        let e = &fail(&m, qname)[0];
        assert_eq!((&e["role"], &e["frame"]), (&"implicated".into(), &frame.into()), "{qname}");
    }
}
