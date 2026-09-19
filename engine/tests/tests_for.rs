//! LE.3b — `tests_for`: the tests to run for a change, by reverse reachability
//! from the changed nodes to test cases, tiered fact / derived / heuristic.
//!
//! The base tree is the substrate fixture `py-test-cells`: `price` <-CALLS-
//! `place` <-CALLS- `audited_place` <-CALLS+TESTS- `test_audited_place`, and
//! `test_price` -CALLS+TESTS-> `price`, with the module TESTS pairings
//! `test_service -> service` and `test_audit -> audit`. The engine prints
//! `[tests-for] seeds=.. tests=..` per answer; `cli/tests/tests_for_cli.rs`
//! asserts that line.

mod git_fixture;

use std::path::Path;

use git_fixture::GitRepo;
use glia_engine::generate_one;
use glia_engine::tests_for::{
    MAX_SEEDS, TestHit, TestsFor, TestsForArgs, tests_for, tests_for_diff, tests_for_rev,
};

const SERVICE_PY: &str = "def price(order):\n    return sum(i[\"p\"] for i in order[\"items\"])\n\n\n\
def place(order):\n    total = price(order)\n    return {\"total\": total}\n";
/// `SERVICE_PY` with `price`'s body edited (line 2 only).
const SERVICE_PY_EDITED: &str = "def price(order):\n    return sum(i[\"p\"] * 1 for i in order[\"items\"])\n\n\n\
def place(order):\n    total = price(order)\n    return {\"total\": total}\n";
/// `SERVICE_PY` plus a `discount` nothing calls.
const SERVICE_PY_DISCOUNT: &str = "def price(order):\n    return sum(i[\"p\"] for i in order[\"items\"])\n\n\n\
def place(order):\n    total = price(order)\n    return {\"total\": total}\n\n\n\
def discount(order):\n    return 0\n";
const AUDIT_PY: &str = "from shop.orders.service import place\n\n\ndef audited_place(order):\n    return place(order)\n";
const TEST_SERVICE_PY: &str = "from shop.orders.service import price\n\n\n\
def test_price():\n    assert price({\"items\": [{\"p\": 2}]}) == 2\n";
const TEST_AUDIT_PY: &str = "from shop.orders.audit import audited_place\n\n\n\
def test_audited_place():\n    assert audited_place({\"items\": []})[\"total\"] == 0\n";

const PRICE: &str = "shop::orders::service::price";
const PLACE: &str = "shop::orders::service::place";
const AUDITED_PLACE: &str = "shop::orders::audit::audited_place";
const TEST_PRICE: &str = "shop::tests::test_service::test_price";
const TEST_AUDITED_PLACE: &str = "shop::tests::test_audit::test_audited_place";
const TEST_SERVICE: &str = "shop::tests::test_service";
const TEST_FILES: [&str; 2] = ["shop/tests/test_audit.py", "shop/tests/test_service.py"];

/// Write `files` under `root`, creating parent dirs.
fn write_all(root: &Path, files: &[(&str, &str)]) {
    for (rel, text) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("fixture dir");
        std::fs::write(&p, text).expect("fixture write");
    }
}

/// The py-test-cells tree.
fn shop_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("shop/orders/service.py", SERVICE_PY),
        ("shop/orders/audit.py", AUDIT_PY),
        ("shop/tests/test_service.py", TEST_SERVICE_PY),
        ("shop/tests/test_audit.py", TEST_AUDIT_PY),
    ]
}

/// A tempdir holding `files`, and its graph.
fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, glia_engine::GenerateResult) {
    let dir = tempfile::tempdir().expect("temp dir");
    write_all(dir.path(), files);
    let g = generate_one(dir.path().to_str().expect("utf-8 temp path")).expect("build");
    (dir, g)
}

fn row<'a>(a: &'a TestsFor, qname: &str) -> &'a TestHit {
    a.tests
        .iter()
        .find(|t| t.qname == qname)
        .unwrap_or_else(|| panic!("no row {qname} in {:#?}", a.tests))
}

/// `(qname, tier, depth)` per row, in answer order.
fn shape(a: &TestsFor) -> Vec<(String, &'static str, usize)> {
    a.tests
        .iter()
        .map(|t| (t.qname.clone(), t.tier, t.depth))
        .collect()
}

/// The answer (1) asserts, for the modes that must reproduce it.
fn assert_price_answer(a: &TestsFor) {
    assert_eq!(a.seeds, [PRICE]);
    assert_eq!(
        shape(a),
        [
            (TEST_PRICE.to_string(), "fact", 1),
            (TEST_AUDITED_PLACE.to_string(), "derived", 3),
            (TEST_SERVICE.to_string(), "heuristic", 2),
        ],
        "{:#?}",
        a.tests
    );
    assert_eq!(a.test_files, TEST_FILES);
    assert!(
        a.untested.is_empty() && a.unresolved.is_empty() && a.absence.is_none(),
        "{a:#?}"
    );
}

#[test]
fn seed_leaf_finds_direct_and_transitive() {
    let (_dir, g) = build(&shop_files());
    let a = tests_for(&g.merged, &[PRICE], &TestsForArgs::default()).expect("answer");
    assert_price_answer(&a);

    let direct = row(&a, TEST_PRICE);
    assert_eq!((direct.reason, direct.kind), ("tests_edge", "FUNCTION"));
    assert_eq!(
        (direct.file.as_deref(), direct.line),
        (Some("shop/tests/test_service.py"), Some(4))
    );
    assert_eq!(direct.path, [(PRICE.to_string(), "TESTS")]);
    assert_eq!(direct.covers, [PRICE]);

    // test_audited_place both CALLS and TESTS audited_place: which one the
    // walk records first depends on edge order, so the node sequence is the
    // contract, and every hop is one of the two.
    let chain = row(&a, TEST_AUDITED_PLACE);
    assert_eq!(chain.reason, "reaches");
    let nodes: Vec<&str> = chain.path.iter().map(|(q, _)| q.as_str()).collect();
    assert_eq!(nodes, [AUDITED_PLACE, PLACE, PRICE]);
    assert!(
        chain
            .path
            .iter()
            .all(|(_, c)| ["CALLS", "TESTS"].contains(c)),
        "{:?}",
        chain.path
    );
    assert_eq!(
        (chain.file.as_deref(), chain.line),
        (Some("shop/tests/test_audit.py"), Some(4))
    );

    let module = row(&a, TEST_SERVICE);
    assert_eq!(
        (module.kind, module.reason),
        ("MODULE", "module_tests_edge")
    );
    assert_eq!(
        module.path,
        [
            ("shop::orders::service".to_string(), "TESTS"),
            (PRICE.to_string(), "DEFINES")
        ]
    );

    // --no-module-level drops the heuristic row and its file stays (the
    // fact row lives in it).
    let mut args = TestsForArgs::default();
    args.module_level = false;
    let cases = tests_for(&g.merged, &[PRICE], &args).expect("answer");
    assert_eq!(
        shape(&cases),
        [
            (TEST_PRICE.to_string(), "fact", 1),
            (TEST_AUDITED_PLACE.to_string(), "derived", 3)
        ]
    );
    assert_eq!(cases.test_files, TEST_FILES);

    // A depth bound below the chain keeps the fact, loses the transitive case.
    let mut args = TestsForArgs::default();
    args.max_depth = 2;
    let short = tests_for(&g.merged, &[PRICE], &args).expect("answer");
    assert!(
        short.tests.iter().all(|t| t.qname != TEST_AUDITED_PLACE),
        "{:#?}",
        short.tests
    );
    assert_eq!(row(&short, TEST_PRICE).tier, "fact");
}

#[test]
fn seed_caller_excludes_unrelated_tests() {
    let (_dir, g) = build(&shop_files());
    let a = tests_for(&g.merged, &[AUDITED_PLACE], &TestsForArgs::default()).expect("answer");
    let cases: Vec<(String, &str, usize)> = shape(&a)
        .into_iter()
        .filter(|(_, tier, _)| *tier != "heuristic")
        .collect();
    assert_eq!(cases, [(TEST_AUDITED_PLACE.to_string(), "fact", 1)]);
    assert!(
        a.tests
            .iter()
            .all(|t| t.qname != TEST_PRICE && t.qname != TEST_SERVICE),
        "{a:#?}"
    );
    // The heuristic row is audit's own test module.
    assert_eq!(row(&a, "shop::tests::test_audit").tier, "heuristic");
    assert_eq!(a.test_files, ["shop/tests/test_audit.py"]);
}

#[test]
fn several_seeds_merge_into_one_row_per_test() {
    let (_dir, g) = build(&shop_files());
    // By bare name and by qname; the seeds come back ordered by qname.
    let a = tests_for(&g.merged, &["price", PLACE], &TestsForArgs::default()).expect("answer");
    assert_eq!(a.seeds, [PLACE, PRICE]);
    let chain = row(&a, TEST_AUDITED_PLACE);
    assert_eq!(chain.covers, [PLACE, PRICE]);
    // Its best hit is the nearer seed: place, two hops.
    assert_eq!((chain.tier, chain.depth), ("derived", 2));
    assert_eq!(chain.path.last().map(|(q, _)| q.as_str()), Some(PLACE));
    let direct = row(&a, TEST_PRICE);
    assert_eq!(
        (direct.tier, direct.covers.as_slice()),
        ("fact", [PRICE.to_string()].as_slice())
    );
    // One row per test, whatever the number of seeds reaching it.
    let mut names: Vec<&str> = a.tests.iter().map(|t| t.qname.as_str()).collect();
    names.dedup();
    assert_eq!(names.len(), a.tests.len());
}

#[test]
fn helper_is_not_a_case() {
    let mut files = shop_files();
    files.retain(|(rel, _)| *rel != "shop/tests/test_service.py");
    files.push((
        "shop/tests/helpers.py",
        "from shop.orders.service import price\n\n\n\
def make_order():\n    order = {\"items\": [{\"p\": 2}]}\n    price(order)\n    return order\n",
    ));
    files.push((
        "shop/tests/test_service.py",
        "from shop.orders.service import price\nfrom shop.tests.helpers import make_order\n\n\n\
def test_price():\n    assert price(make_order()) == 2\n",
    ));
    let (_dir, g) = build(&files);
    const MAKE_ORDER: &str = "shop::tests::helpers::make_order";
    // make_order calls price, but a test calls make_order: it is a helper,
    // walked through, never a row.
    let a = tests_for(&g.merged, &[PRICE], &TestsForArgs::default()).expect("answer");
    assert!(
        a.tests.iter().all(|t| t.qname != MAKE_ORDER),
        "{:#?}",
        a.tests
    );
    assert_eq!(row(&a, TEST_PRICE).tier, "fact");
    assert!(
        !a.test_files.contains(&"shop/tests/helpers.py".to_string()),
        "{:?}",
        a.test_files
    );
    // Seeded itself, the helper is not a changed test: its callers are the rows.
    let h = tests_for(&g.merged, &[MAKE_ORDER], &TestsForArgs::default()).expect("answer");
    assert!(
        h.tests.iter().all(|t| t.qname != MAKE_ORDER),
        "{:#?}",
        h.tests
    );
    assert!(
        h.tests.iter().any(|t| t.qname == TEST_PRICE),
        "{:#?}",
        h.tests
    );
}

#[test]
fn integration_test_through_http() {
    let (_dir, g) = build(&[
        (
            "app/api.py",
            "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.get('/users')\ndef get_users():\n    return []\n",
        ),
        (
            "tests/test_api.py",
            "import requests\n\n\ndef test_users():\n    requests.get('http://localhost:5000/users')\n",
        ),
    ]);
    let a = tests_for(
        &g.merged,
        &["app::api::get_users"],
        &TestsForArgs::default(),
    )
    .expect("answer");
    let hit = row(&a, "tests::test_api::test_users");
    assert_eq!((hit.tier, hit.reason, hit.depth), ("derived", "reaches", 3));
    let cats: Vec<&str> = hit.path.iter().map(|(_, c)| *c).collect();
    assert_eq!(
        cats,
        ["CALLS", "HTTP_CALLS", "HANDLED_BY"],
        "{:?}",
        hit.path
    );
    assert_eq!(
        hit.path.last().map(|(q, _)| q.as_str()),
        Some("app::api::get_users")
    );
    assert_eq!(a.test_files, ["tests/test_api.py"]);
}

#[test]
fn jest_module_level_calls_make_the_file_a_case() {
    // Jest's describe / it callbacks are anonymous: the TypeScript parser
    // attributes their calls to the test file's MODULE.
    let (_dir, g) = build(&[
        (
            "math.ts",
            "export function add(a: number, b: number): number {\n  return a + b;\n}\n",
        ),
        (
            "math.test.ts",
            "import { add } from \"./math\";\n\ndescribe(\"add\", () => {\n  it(\"adds\", () => {\n    expect(add(1, 2)).toBe(3);\n  });\n});\n",
        ),
    ]);
    let a = tests_for(&g.merged, &["math::add"], &TestsForArgs::default()).expect("answer");
    // Reached through its CALLS (derived), which outranks the module TESTS
    // pairing the same node also has: one row.
    assert_eq!(
        shape(&a),
        [("math.test".to_string(), "derived", 1)],
        "{:#?}",
        a.tests
    );
    let hit = row(&a, "math.test");
    assert_eq!(hit.kind, "MODULE");
    assert_eq!(hit.path, [("math::add".to_string(), "CALLS")]);
    assert_eq!(a.test_files, ["math.test.ts"]);
}

#[test]
fn changed_test_is_its_own_row() {
    let (_dir, g) = build(&shop_files());
    let a = tests_for(&g.merged, &[TEST_PRICE], &TestsForArgs::default()).expect("answer");
    let hit = row(&a, TEST_PRICE);
    assert_eq!(
        (hit.tier, hit.reason, hit.depth),
        ("fact", "changed_test", 0)
    );
    assert!(hit.path.is_empty());
    assert!(a.untested.is_empty());
    assert_eq!(a.test_files, ["shop/tests/test_service.py"]);
}

#[test]
fn untested_seed_is_listed_and_absence_set() {
    let mut files = shop_files();
    files.retain(|(rel, _)| *rel != "shop/orders/service.py");
    files.push(("shop/orders/service.py", SERVICE_PY_DISCOUNT));
    files.push((
        "shop/orders/refund.py",
        "def refund(order):\n    return 0\n",
    ));
    let (_dir, g) = build(&files);
    const REFUND: &str = "shop::orders::refund::refund";
    let a = tests_for(&g.merged, &[REFUND], &TestsForArgs::default()).expect("answer");
    assert!(a.tests.is_empty() && a.test_files.is_empty(), "{a:#?}");
    assert_eq!(a.untested, [REFUND]);
    let absence = a
        .absence
        .as_ref()
        .expect("an empty answer carries its absence");
    assert_eq!((absence.tier, absence.reason), ("FACT", "no_edges"));
    assert_eq!(absence.mechanisms, ["TESTS", "CALLS", "HTTP_CALLS"]);
    assert!(absence.note.contains(REFUND), "{}", absence.note);

    // `discount` sits in service.py, which test_service pairs with by name,
    // but no test case reaches it: the heuristic row is listed and the seed
    // is still untested.
    const DISCOUNT: &str = "shop::orders::service::discount";
    let b = tests_for(&g.merged, &[DISCOUNT, PRICE], &TestsForArgs::default()).expect("answer");
    assert_eq!(b.untested, [DISCOUNT]);
    assert!(b.absence.is_none());
    let module = row(&b, TEST_SERVICE);
    assert_eq!(module.tier, "heuristic");
    assert_eq!(module.covers, [DISCOUNT, PRICE]);
}

#[test]
fn unknown_seed_is_unresolved() {
    let (_dir, g) = build(&shop_files());
    let a = tests_for(&g.merged, &["no_such_symbol"], &TestsForArgs::default()).expect("answer");
    assert!(a.seeds.is_empty() && a.tests.is_empty());
    assert_eq!(a.unresolved, ["no_such_symbol"]);
    assert_eq!(a.absence.as_ref().map(|x| x.reason), Some("unknown_symbol"));

    // A dotted qname finds its `::` node (find's exact tiers); the unknown
    // name is reported beside a normal answer.
    let b = tests_for(
        &g.merged,
        &["shop.orders.service.price", "nope"],
        &TestsForArgs::default(),
    )
    .expect("answer");
    assert_eq!(b.unresolved, ["nope"]);
    let mut resolved = b.clone();
    resolved.unresolved.clear();
    assert_price_answer(&resolved);

    assert!(tests_for(&g.merged, &[], &TestsForArgs::default()).is_err());
}

#[test]
fn scope_filters_rows_and_empties_with_absence() {
    let (_dir, g) = build(&shop_files());
    let mut args = TestsForArgs::default();
    args.scope = Some("shop/tests/test_audit.py".to_string());
    let a = tests_for(&g.merged, &[PRICE], &args).expect("answer");
    assert_eq!(shape(&a), [(TEST_AUDITED_PLACE.to_string(), "derived", 3)]);
    assert_eq!(a.test_files, ["shop/tests/test_audit.py"]);

    args.scope = Some("elsewhere".to_string());
    let b = tests_for(&g.merged, &[PRICE], &args).expect("answer");
    assert!(b.tests.is_empty());
    // Untested is the graph's fact, computed before the scope.
    assert!(b.untested.is_empty());
    let absence = b.absence.as_ref().expect("absence");
    assert_eq!(absence.reason, "no_match");
    assert!(
        absence.note.contains("3 results outside scope `elsewhere`"),
        "{}",
        absence.note
    );
}

#[test]
fn diff_mode_equals_qname_mode() {
    let (_dir, g) = build(&shop_files());
    let diff = "--- a/shop/orders/service.py\n+++ b/shop/orders/service.py\n@@ -1,2 +1,2 @@\n \
def price(order):\n-    return sum(i[\"p\"] for i in order[\"items\"])\n\
+    return sum(i[\"p\"] * 1 for i in order[\"items\"])\n";
    let a = tests_for_diff(&g.merged, diff, &TestsForArgs::default()).expect("answer");
    assert_price_answer(&a);

    let none = tests_for_diff(
        &g.merged,
        "--- a/x.py\n+++ b/x.py\n@@ -1 +1 @@\n+x = 1\n",
        &TestsForArgs::default(),
    )
    .expect("answer");
    assert!(none.seeds.is_empty() && none.tests.is_empty());
    assert_eq!(
        none.absence.as_ref().map(|x| (x.reason, x.query.as_str())),
        Some(("no_signal_match", "diff: x.py"))
    );
}

/// The py-test-cells tree, committed.
fn committed_shop() -> GitRepo {
    let repo = GitRepo::init();
    for (rel, text) in shop_files() {
        repo.write(rel, text);
    }
    repo.commit("shop");
    repo
}

#[test]
fn rev_mode_via_git_fixture() {
    let repo = committed_shop();
    repo.write("shop/orders/service.py", SERVICE_PY_EDITED);
    let a = tests_for_rev(repo.path(), "HEAD", &TestsForArgs::default()).expect("answer");
    // The module's CODE span holds price's, so the delta marks it modified
    // too; its own text (price's cut out) did not change, so it is no seed.
    assert_price_answer(&a);

    // A module-level statement beside the edit changes the module's own
    // text (an import is no child node of it).
    repo.write(
        "shop/orders/service.py",
        &format!("import os\n\n\n{SERVICE_PY_EDITED}"),
    );
    let b = tests_for_rev(repo.path(), "HEAD", &TestsForArgs::default()).expect("answer");
    assert_eq!(b.seeds, ["shop::orders::service", PRICE]);
    assert_eq!(b.untested, ["shop::orders::service"]);
    assert_eq!(
        row(&b, TEST_SERVICE).covers,
        ["shop::orders::service", PRICE]
    );
    repo.write("shop/orders/service.py", SERVICE_PY_EDITED);

    // A clean tree changes nothing: no seed, an absence that says so.
    repo.commit("edit price");
    let clean = tests_for_rev(repo.path(), "HEAD", &TestsForArgs::default()).expect("answer");
    assert!(clean.seeds.is_empty() && clean.tests.is_empty());
    assert_eq!(clean.absence.as_ref().map(|x| x.reason), Some("no_match"));
    assert!(tests_for_rev(repo.path(), "no-such-rev", &TestsForArgs::default()).is_err());
}

#[test]
fn rev_mode_added_function_seeds_it_and_its_caller_not_the_module() {
    let repo = committed_shop();
    repo.write(
        "shop/orders/service.py",
        "def price(order):\n    return sum(i[\"p\"] for i in order[\"items\"])\n\n\n\
def place(order):\n    total = price(order) + tax(order)\n    return {\"total\": total}\n\n\n\
def tax(order):\n    return 0\n",
    );
    let a = tests_for_rev(repo.path(), "HEAD", &TestsForArgs::default()).expect("answer");
    const TAX: &str = "shop::orders::service::tax";
    // tax is added, place modified and the added CALLS' source; the module
    // gained only tax and blank lines, so it is no seed.
    assert_eq!(a.seeds, [PLACE, TAX]);
    let chain = row(&a, TEST_AUDITED_PLACE);
    assert_eq!((chain.tier, chain.depth), ("derived", 2));
    assert_eq!(chain.covers, [PLACE, TAX]);
    assert!(
        a.tests.iter().all(|t| t.qname != TEST_PRICE),
        "{:#?}",
        a.tests
    );
    assert!(a.untested.is_empty(), "{:?}", a.untested);
}

#[test]
fn rev_mode_deleted_test_seeds_its_target_and_moved_test_is_a_row() {
    let repo = committed_shop();
    repo.remove("shop/tests/test_service.py");
    repo.git_mv("shop/tests/test_audit.py", "shop/tests/test_audit_flow.py");
    assert!(!repo.root().join("shop/tests/test_service.py").exists());
    let a = tests_for_rev(repo.path(), "HEAD", &TestsForArgs::default()).expect("answer");
    // The deleted test's CALLS / TESTS into price are removed edges: price,
    // their surviving end, is a seed, and the test left reaching it is listed.
    assert!(a.seeds.iter().any(|s| s == PRICE), "{:?}", a.seeds);
    let moved = "shop::tests::test_audit_flow::test_audited_place";
    let hit = row(&a, moved);
    assert!(hit.covers.iter().any(|c| c == PRICE), "{hit:#?}");
    assert!(
        a.tests.iter().all(|t| t.qname != TEST_PRICE),
        "{:#?}",
        a.tests
    );
    assert_eq!(a.test_files, ["shop/tests/test_audit_flow.py"]);
}

#[test]
fn more_seeds_than_the_cap_is_an_error() {
    let body: String = (0..=MAX_SEEDS)
        .map(|i| format!("def f{i}():\n    return {i}\n\n\n"))
        .collect();
    let (_dir, g) = build(&[("many.py", body.as_str())]);
    let names: Vec<String> = (0..=MAX_SEEDS).map(|i| format!("many::f{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let err = tests_for(&g.merged, &refs, &TestsForArgs::default()).expect_err("over the cap");
    assert!(err.contains(&format!("{} seeds", MAX_SEEDS + 1)), "{err}");
}
