//! LE.3a: the TEST cell (7), filled at build time from TESTS edges. Every
//! node a TESTS edge points at lists its DIRECT tests
//! `{"tests":[{"test":"<qname>","kind":"FUNCTION"}],"total":N}`, sorted by
//! test qname, capped at 50 with `total` the real count, once per node (its
//! first copy in graph order). Built on tempdir copies of the
//! `py-test-cells` fixture.
//!
//! Measured before LE.3a (HEAD 0b2b391, installed leap wheel): the fixture's
//! 4 TESTS edges (2 module pairings, 2 function-level) and no node carrying
//! cell 7 — grade.py `TEST 0/3 (0.00)`.

use std::path::Path;

use glia_code_domain::cell_type;
use glia_core::{Cell, CellPayload};
use glia_engine::generate_one;
use glia_graph::MergedGraph;
use glia_store::write_merged_sharded;
use serde_json::Value;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/py-test-cells";

/// Copy the fixture's sources (not its key.json) into `dst`.
fn copy_fixture(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            std::fs::create_dir_all(&to).expect("mkdir");
            copy_fixture(&from, &to);
        } else if entry.file_name() != "key.json" {
            std::fs::copy(&from, &to).expect("copy fixture file");
        }
    }
}

/// A tempdir holding `repo/` = the fixture's sources.
fn fixture_copy() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    copy_fixture(&Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE), &repo);
    tmp
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("file has a parent")).expect("mkdir");
    std::fs::write(path, text).expect("write source");
}

fn build(repo: &Path) -> MergedGraph {
    generate_one(repo.to_str().expect("utf-8 tempdir")).expect("build").merged
}

/// Every TEST cell of every copy of the node named `qname`, one entry per
/// copy (a copy with no TEST cell is an empty vec).
fn test_cells<'m>(m: &'m MergedGraph, qname: &str) -> Vec<Vec<&'m Cell>> {
    let mut out = Vec::new();
    for g in &m.graphs {
        for n in &g.nodes {
            if g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname) {
                out.push(n.cells.iter().filter(|c| c.kind == cell_type::TEST).collect());
            }
        }
    }
    assert!(!out.is_empty(), "no node {qname}");
    out
}

/// The one TEST cell `qname` carries across its copies, parsed.
fn test_cell(m: &MergedGraph, qname: &str) -> Value {
    let cells: Vec<&Cell> = test_cells(m, qname).into_iter().flatten().collect();
    assert_eq!(cells.len(), 1, "{qname}: exactly one TEST cell across its copies");
    match &cells[0].payload {
        CellPayload::Json(j) => serde_json::from_str(j).expect("TEST payload is JSON"),
        other => panic!("{qname}: TEST payload is not Json: {other:?}"),
    }
}

/// `(test, kind)` of a parsed TEST cell's entries, in order.
fn entries(cell: &Value) -> Vec<(String, String)> {
    cell["tests"]
        .as_array()
        .expect("tests is an array")
        .iter()
        .map(|e| {
            assert_eq!(e.as_object().map(|o| o.len()), Some(2), "entry is {{test, kind}}: {e}");
            (
                e["test"].as_str().expect("test is a string").to_string(),
                e["kind"].as_str().expect("kind is a string").to_string(),
            )
        })
        .collect()
}

fn all_test_cells(m: &MergedGraph) -> usize {
    m.graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .map(|n| n.cells.iter().filter(|c| c.kind == cell_type::TEST).count())
        .sum()
}

#[test]
fn test_cell_lists_direct_tests_sorted() {
    let tmp = fixture_copy();
    let m = build(&tmp.path().join("repo"));

    let price = test_cell(&m, "shop::orders::service::price");
    assert_eq!(
        entries(&price),
        [("shop::tests::test_service::test_price".to_string(), "FUNCTION".to_string())]
    );
    assert_eq!(price["total"], 1);
    assert_eq!(price.as_object().map(|o| o.len()), Some(2), "{{tests, total}} only: {price}");

    let service = test_cell(&m, "shop::orders::service");
    assert_eq!(
        entries(&service),
        [("shop::tests::test_service".to_string(), "MODULE".to_string())]
    );
    assert_eq!(service["total"], 1);

    let audited = test_cell(&m, "shop::orders::audit::audited_place");
    assert_eq!(
        entries(&audited),
        [("shop::tests::test_audit::test_audited_place".to_string(), "FUNCTION".to_string())]
    );

    // Precision: test_service pairs service, never audit.
    let audit = test_cell(&m, "shop::orders::audit");
    assert_eq!(entries(&audit), [("shop::tests::test_audit".to_string(), "MODULE".to_string())]);

    // The compact payload, byte for byte (key order = the documented shape).
    let raw: Vec<&Cell> = test_cells(&m, "shop::orders::service::price").into_iter().flatten().collect();
    assert_eq!(
        raw[0].payload,
        CellPayload::Json(
            r#"{"tests":[{"test":"shop::tests::test_service::test_price","kind":"FUNCTION"}],"total":1}"#
                .to_string()
        )
    );
    // Four tested nodes, one cell each.
    assert_eq!(all_test_cells(&m), 4);
}

#[test]
fn transitive_tests_are_not_in_the_cell() {
    let tmp = fixture_copy();
    let m = build(&tmp.path().join("repo"));
    // `place` is reached from test_audited_place only through audited_place:
    // nothing TESTS it directly.
    assert_eq!(test_cells(&m, "shop::orders::service::place"), vec![Vec::<&Cell>::new()]);
    // audited_place lists its own test, not the tests of what calls it, and
    // price lists test_price only, not test_audited_place (which reaches it
    // through place).
    let price = test_cell(&m, "shop::orders::service::price");
    assert!(
        entries(&price).iter().all(|(t, _)| !t.contains("test_audited_place")),
        "{price}"
    );
    // Tests themselves carry no TEST cell.
    assert_eq!(test_cells(&m, "shop::tests::test_service::test_price"), vec![Vec::<&Cell>::new()]);
}

/// An ENDPOINT a Python and a TypeScript client both call is one NodeId in
/// two per-language graphs; an overlay-declared TESTS edge into it gives it
/// ONE TEST cell, on the first copy.
#[test]
fn one_cell_per_node_across_graph_copies() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    write(&repo, "client.py", "import requests\n\n\ndef load_users():\n    return requests.get(\"/users\").json()\n");
    write(
        &repo,
        "client.ts",
        "export async function loadUsers() {\n  const res = await fetch(\"/users\");\n  return res.json();\n}\n",
    );
    write(
        &repo,
        "tests/test_client.py",
        "from client import load_users\n\n\ndef test_users():\n    assert load_users() == []\n",
    );
    write(
        &repo,
        ".glia/overlay.toml",
        "version = 1\n\n[[edge]]\nfrom = \"tests::test_client::test_users\"\n\
         to = \"endpoint:GET:/users\"\ncategory = \"TESTS\"\n",
    );
    let m = build(&repo);

    let copies = test_cells(&m, "endpoint:GET:/users");
    assert_eq!(copies.len(), 2, "the endpoint sits in both per-language graphs");
    assert_eq!(
        copies.iter().map(Vec::len).collect::<Vec<_>>(),
        [1, 0],
        "one TEST cell, on the first copy in graph order"
    );
    let cell = test_cell(&m, "endpoint:GET:/users");
    assert_eq!(entries(&cell), [("tests::test_client::test_users".to_string(), "FUNCTION".to_string())]);
    assert_eq!(cell["total"], 1);
}

#[test]
fn cap_at_50_keeps_total() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    write(&repo, "shop/pricing.py", "def price(order):\n    return len(order)\n");
    let mut tests = String::from("from shop.pricing import price\n");
    for i in 0..60 {
        tests.push_str(&format!("\n\ndef test_price_{i:02}():\n    assert price([{i}]) == 1\n"));
    }
    write(&repo, "shop/tests/test_pricing.py", &tests);
    let m = build(&repo);

    let cell = test_cell(&m, "shop::pricing::price");
    let got = entries(&cell);
    assert_eq!(cell["total"], 60, "total is the real count");
    assert_eq!(got.len(), 50, "the list is capped");
    let want: Vec<(String, String)> = (0..50)
        .map(|i| (format!("shop::tests::test_pricing::test_price_{i:02}"), "FUNCTION".to_string()))
        .collect();
    assert_eq!(got, want, "sorted by test qname, the first 50 kept");
}

#[test]
fn no_tests_edges_no_cells_no_marker() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    write(&repo, "shop/pricing.py", "def price(order):\n    return len(order)\n\n\ndef place(order):\n    return price(order)\n");
    let m = build(&repo);
    assert!(
        !m.all_edges().any(|e| e.category == glia_code_domain::edge_category::TESTS),
        "no TESTS edge in a tree without tests"
    );
    // The marker is gated on the same count (TestCellStats::marker is None
    // with no TESTS edge; passes_tests::test_cell_marker_needs_a_tests_edge).
    assert_eq!(all_test_cells(&m), 0);
}

#[test]
fn two_builds_serialise_identically() {
    let tmp = fixture_copy();
    let repo = tmp.path().join("repo");
    let (out1, out2) = (tmp.path().join("out1"), tmp.path().join("out2"));
    let (m1, m2) = (build(&repo), build(&repo));
    assert_eq!(all_test_cells(&m1), 4, "the builds carry TEST cells");
    write_merged_sharded(&m1, &out1).expect("write 1");
    write_merged_sharded(&m2, &out2).expect("write 2");
    let bytes = |dir: &Path| {
        let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
            .expect("read out dir")
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).expect("read")))
            .collect();
        files.sort();
        files
    };
    let (b1, b2) = (bytes(&out1), bytes(&out2));
    assert!(!b1.is_empty());
    assert_eq!(b1, b2, "two builds write identical bytes");
}
