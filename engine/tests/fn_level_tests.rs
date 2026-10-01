//! CL.5b: function-level TESTS for every language. A ROOT function of a
//! name-paired test module (no function of that test module calls it) that
//! CALLS into the module it tests gets `TESTS test_fn -> callee` (Medium,
//! EVIDENCE `pass:tests` / `calls_into_tested_module`, the call's line),
//! derived by `passes::emit_tests_edges` from the module pairing and the
//! CALLS edges. tests-for keeps its own tiering: it skips these edges, so a
//! case reached over its CALLS edge stays `derived` / `reaches`.
//!
//! Measured before CL.5b (HEAD adbc635, debug CLI): matrix/go/tests held
//! `CALLS calc_test::TestAdd -> calc::Add` and `TESTS calc_test -> calc`
//! only; java/tests, csharp/tests, rust/tests and php/tests the same shape
//! (module TESTS + the case's CALLS, no fn-level TESTS). Every test here
//! fails on that HEAD except `tests_for_does_not_promote_derived_fn_tests`,
//! a guard that holds there and fails if the tests_for.rs half is left out.

use std::path::Path;

use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{cell_type, edge_category};
use glia_core::{CellPayload, Confidence, Edge};
use glia_engine::merge::{MergeMember, merge_layouts, read_workspace};
use glia_engine::persist::persist_result;
use glia_engine::tests_for::{DERIVED, TestsForArgs, tests_for};
use glia_engine::{GenerateResult, generate_one};
use glia_graph::MergedGraph;

fn write_all(root: &Path, files: &[(&str, &str)]) {
    for (rel, text) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("fixture dir");
        std::fs::write(&p, text).expect("fixture write");
    }
}

/// A tempdir holding `files`, and its graph.
fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, GenerateResult) {
    let dir = tempfile::tempdir().expect("temp dir");
    write_all(dir.path(), files);
    let g = generate_one(dir.path().to_str().expect("utf-8 temp path")).expect("build");
    (dir, g)
}

fn qname_of(m: &MergedGraph, id: glia_core::NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_else(|| format!("#{}", id.0))
}

/// Every TESTS edge whose source is a FUNCTION / METHOD, as
/// `(from qname, to qname)`, sorted.
fn fn_tests(m: &MergedGraph) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = fn_tests_edges(m)
        .into_iter()
        .map(|e| (qname_of(m, e.from), qname_of(m, e.to)))
        .collect();
    out.sort();
    out
}

fn fn_tests_edges(m: &MergedGraph) -> Vec<&Edge> {
    let callable = |id: glia_core::NodeId| {
        m.graphs.iter().any(|g| {
            matches!(
                g.nav.kind_by_id.get(&id),
                Some(&glia_code_domain::node_kind::FUNCTION | &glia_code_domain::node_kind::METHOD)
            )
        })
    };
    m.all_edges()
        .filter(|e| e.category == edge_category::TESTS && callable(e.from))
        .collect()
}

fn pair(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

/// The one edge `from -> to` of TESTS.
fn tests_edge<'m>(m: &'m MergedGraph, from: &str, to: &str) -> &'m Edge {
    let hits: Vec<&Edge> = m
        .all_edges()
        .filter(|e| {
            e.category == edge_category::TESTS
                && qname_of(m, e.from) == from
                && qname_of(m, e.to) == to
        })
        .collect();
    assert_eq!(hits.len(), 1, "exactly one TESTS {from} -> {to}");
    hits[0]
}

/// The `tests` entries of the TEST cell on `qname`, as `(test, kind)`.
fn test_cell(m: &MergedGraph, qname: &str) -> Vec<(String, String)> {
    let cell = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(move |n| (g, n)))
        .filter(|(g, n)| g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname))
        .flat_map(|(_, n)| n.cells.iter())
        .find(|c| c.kind == cell_type::TEST)
        .unwrap_or_else(|| panic!("{qname} carries no TEST cell"));
    let CellPayload::Json(j) = &cell.payload else {
        panic!("TEST payload is JSON")
    };
    let v: serde_json::Value = serde_json::from_str(j).expect("TEST payload parses");
    v["tests"]
        .as_array()
        .expect("tests is an array")
        .iter()
        .map(|e| {
            (
                e["test"].as_str().unwrap_or_default().to_string(),
                e["kind"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

const CALC_GO: &str = "package calc\n\nfunc Add(a, b int) int {\n\treturn a + b\n}\n";
/// `TestAdd` calls `Add` directly (line 5, 0-based); `TestAddTwice` goes
/// through the helper `mustAdd`, which the case calls, so it is no root.
const CALC_TEST_GO: &str = "package calc\n\nimport \"testing\"\n\n\
func TestAdd(t *testing.T) {\n\tif Add(2, 3) != 5 {\n\t\tt.Fatal(\"Add(2,3) != 5\")\n\t}\n}\n\n\
func mustAdd(t *testing.T, a, b int) int {\n\treturn Add(a, b)\n}\n\n\
func TestAddTwice(t *testing.T) {\n\tif mustAdd(t, 1, 1) != 2 {\n\t\tt.Fatal(\"1+1\")\n\t}\n}\n";

fn go_files() -> Vec<(&'static str, &'static str)> {
    vec![("calc.go", CALC_GO), ("calc_test.go", CALC_TEST_GO)]
}

#[test]
fn go_test_function_tests_its_unit() {
    let (_dir, g) = build(&go_files());
    let m = &g.merged;
    assert_eq!(
        fn_tests(m),
        [pair("calc_test::TestAdd", "calc::Add")],
        "the helper mints nothing"
    );

    let e = tests_edge(m, "calc_test::TestAdd", "calc::Add");
    assert_eq!(e.confidence, Confidence::Medium);
    let ev = Evidence::of(e).expect("the edge carries EVIDENCE");
    assert_eq!(ev.emitter, "pass:tests");
    assert_eq!(ev.rule.as_deref(), Some("calls_into_tested_module"));
    assert_eq!(ev.file.as_deref(), Some("calc_test.go"));
    assert_eq!(ev.line, Some(5), "the call's own 0-based line");
    assert_eq!(ev.basis, Basis::Site);

    // The module pairing is still there, beside it.
    tests_edge(m, "calc_test", "calc");
    // LE.3a lists the case on the unit's TEST cell; the helper is no test.
    assert_eq!(
        test_cell(m, "calc::Add"),
        [pair("calc_test::TestAdd", "FUNCTION")]
    );
}

const CALC_JAVA: &str = "package com.example;\n\npublic class Calc {\n    \
public int add(int a, int b) {\n        return a + b;\n    }\n}\n";
const CALC_TEST_JAVA: &str = "package com.example;\n\nimport org.junit.jupiter.api.Test;\n\n\
class CalcTest {\n    @Test\n    void testAdd() {\n        new Calc().add(2, 3);\n    }\n}\n";
const CALC_CS: &str = "namespace Shop;\n\npublic class Calc\n{\n    \
public int Add(int a, int b) => a + b;\n}\n";
const CALC_TESTS_CS: &str = "using Xunit;\n\nnamespace Shop.Tests;\n\npublic class CalcTests\n{\n    \
[Fact]\n    public void Add_ReturnsSum()\n    {\n        Assert.Equal(5, new Calc().Add(2, 3));\n    }\n}\n";

/// CL.5a binds `new Calc().add(..)` through the constructed type; this pass
/// turns that CALLS edge of the JUnit / xUnit case into a TESTS edge. The
/// xUnit name carries no `test` word: the root rule, not the name, makes it
/// a case.
#[test]
fn java_and_csharp_tests_bind_through_constructed_receivers() {
    let (_j, java) = build(&[("Calc.java", CALC_JAVA), ("CalcTest.java", CALC_TEST_JAVA)]);
    assert_eq!(
        fn_tests(&java.merged),
        [pair("CalcTest::testAdd", "Calc::add")]
    );
    assert_eq!(
        test_cell(&java.merged, "Calc::add"),
        [pair("CalcTest::testAdd", "METHOD")]
    );

    let (_c, cs) = build(&[("Calc.cs", CALC_CS), ("CalcTests.cs", CALC_TESTS_CS)]);
    assert_eq!(
        fn_tests(&cs.merged),
        [pair(
            "Shop::Tests::CalcTests::Add_ReturnsSum",
            "Shop::Calc::Add"
        )]
    );
}

const ORDER_CS: &str = "namespace Shop;\n\npublic class Order\n{\n    \
public int Place(int n) => n;\n}\n";
/// Block namespaces: one `Shop::Tests` PACKAGE node both test files open,
/// each CLASS under it (a file-scoped `namespace X;` hangs its classes off
/// the file MODULE instead).
const CALC_TESTS_BLOCK_CS: &str = "using Xunit;\n\nnamespace Shop.Tests\n{\n    \
public class CalcTests\n    {\n        [Fact]\n        public void Add_ReturnsSum()\n        {\n            \
Assert.Equal(5, new Calc().Add(2, 3));\n        }\n    }\n}\n";
const ORDER_TESTS_BLOCK_CS: &str = "using Xunit;\n\nnamespace Shop.Tests\n{\n    \
public class OrderTests\n    {\n        [Fact]\n        public void Place_Works()\n        {\n            \
Assert.Equal(1, new Order().Place(1));\n        }\n    }\n}\n";

fn shared_namespace_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Calc.cs", CALC_CS),
        ("CalcTests.cs", CALC_TESTS_BLOCK_CS),
        ("Order.cs", ORDER_CS),
        ("OrderTests.cs", ORDER_TESTS_BLOCK_CS),
    ]
}

const SHARED_NAMESPACE_WANT: [(&str, &str); 2] = [
    ("Shop::Tests::CalcTests::Add_ReturnsSum", "Shop::Calc::Add"),
    ("Shop::Tests::OrderTests::Place_Works", "Shop::Order::Place"),
];

/// CB.15: two test files open `namespace Shop.Tests`, one PACKAGE node whose
/// nav parent is the first file only. Each case still belongs to its own
/// file's test module, so each pairs with its own unit.
#[test]
fn csharp_shared_test_namespace_keeps_each_file() {
    let (_d, g) = build(&shared_namespace_files());
    let want: Vec<(String, String)> = SHARED_NAMESPACE_WANT
        .iter()
        .map(|(f, t)| pair(f, t))
        .collect();
    assert_eq!(fn_tests(&g.merged), want);
}

/// LC.10b re-runs the passes over loaded graphs, which carry no
/// `SymbolTable::home_module` (build-time only): the merge of the C#
/// shared-namespace layout mints the joint build's fn-level TESTS edges.
#[test]
fn layout_merge_mints_the_joint_builds_fn_tests() {
    let (dir, g) = build(&shared_namespace_files());
    let out = dir.path().join("layout");
    persist_result(&g, &out, "test").expect("persist");
    let ws = dir.path().join("glia.workspace.json");
    std::fs::write(
        &ws,
        format!(
            r#"{{"version":1,"members":[{{"name":"shop","gmap":"{}"}}]}}"#,
            out.display()
        ),
    )
    .expect("workspace");
    let members: Vec<MergeMember> = read_workspace(&ws).expect("workspace reads");
    let merged = merge_layouts(&members).expect("merge");
    assert_eq!(fn_tests(&merged.result.merged), fn_tests(&g.merged));
    let want: Vec<(String, String)> = SHARED_NAMESPACE_WANT
        .iter()
        .map(|(f, t)| pair(f, t))
        .collect();
    assert_eq!(fn_tests(&merged.result.merged), want);
}

const CALC_PY: &str = "def add(a, b):\n    return a + b\n";
/// `test_add`'s bare call is the Python parser's own TESTS ref; the
/// attribute call `calc.add(..)` is one the parser's refs miss.
const TEST_CALC_PY: &str = "import calc\nfrom calc import add\n\n\n\
def test_add():\n    assert add(1, 2) == 3\n\n\n\
def test_add_attr():\n    assert calc.add(2, 2) == 4\n";

#[test]
fn python_parser_edges_are_not_doubled() {
    let (_d, g) = build(&[("calc.py", CALC_PY), ("test_calc.py", TEST_CALC_PY)]);
    let m = &g.merged;
    assert_eq!(
        fn_tests(m),
        [
            pair("test_calc::test_add", "calc::add"),
            pair("test_calc::test_add_attr", "calc::add")
        ]
    );
    // The parser's edge keeps its own evidence; the attribute call's is ours.
    let parser = Evidence::of(tests_edge(m, "test_calc::test_add", "calc::add")).expect("evidence");
    assert_ne!(parser.rule.as_deref(), Some("calls_into_tested_module"));
    let ours =
        Evidence::of(tests_edge(m, "test_calc::test_add_attr", "calc::add")).expect("evidence");
    assert_eq!(
        (ours.emitter.as_str(), ours.rule.as_deref()),
        ("pass:tests", Some("calls_into_tested_module"))
    );
    assert_eq!(ours.line, Some(9));
}

/// The guard on tests-for's tiering: the derived fn-level edge is a CALLS
/// edge plus a name pairing, so it never makes a row `fact` / `tests_edge`.
#[test]
fn tests_for_does_not_promote_derived_fn_tests() {
    let (_d, g) = build(&go_files());
    let a = tests_for(&g.merged, &["calc::Add"], &TestsForArgs::default()).expect("answer");
    let row = a
        .tests
        .iter()
        .find(|t| t.qname == "calc_test::TestAdd")
        .unwrap_or_else(|| panic!("TestAdd is a row: {:?}", a.tests));
    assert_eq!((row.tier, row.reason, row.depth), (DERIVED, "reaches", 1));
    assert_eq!(row.path, [("calc::Add".to_string(), "CALLS")]);
    // The helper's case is a row too (through mustAdd), and still derived.
    let twice = a
        .tests
        .iter()
        .find(|t| t.qname == "calc_test::TestAddTwice")
        .unwrap_or_else(|| panic!("TestAddTwice is a row: {:?}", a.tests));
    assert_eq!(
        (twice.tier, twice.reason, twice.depth),
        (DERIVED, "reaches", 2)
    );
    assert!(
        a.tests.iter().all(|t| t.reason != "tests_edge"),
        "{:?}",
        a.tests
    );
}
