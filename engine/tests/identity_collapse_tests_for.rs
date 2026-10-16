//! DD.12: `tests_for` walks the identity quotient. A walk class (messages,
//! contract ops, SCIP symbols, a package and its project; DD.3's `walk`
//! rules, read through DD.8's `IdentityView`) is ONE node of the backward
//! walk, so a change to one member finds the tests that exercise any member:
//! a change to billing's `format_cents` finds shop's `test_checkout`, which
//! reaches shop's SCIP-identical copy. A walk class is a test case when any
//! member is, and test rows of one FACT / DERIVED class fold to one row
//! carrying `identity`. `Collapse::off()` is the HEAD walk; a graph with no
//! SAME_AS edge answers byte for byte as before.
//!
//! The repos are two tempdirs (`billing`, `shop`) whose
//! `.glia/scip-snapshot/`s are written the DD.7 way (`identity_scip.rs`,
//! `identity_collapse_blast.rs`): one package `money`, so each repo's
//! `format_cents` is one FACT `scip_symbol` class (`/diverged`: shop's appends
//! ` EUR`).
//!
//! - billing `lib/money.py`: `invoice` -> `format_cents`;
//!   `tests/test_invoice.py`: `test_invoice` -> `invoice`.
//! - shop `app/money.py`: `checkout` -> `format_cents`, `main` -> `checkout`;
//!   `tests/test_money.py`: `test_checkout` -> `checkout`.
//! - with `fmt_tests`, each repo also has `tests/test_fmt.py::test_format`
//!   calling its own `format_cents`, the two defined under one SCIP symbol:
//!   one class of two test cases.
//!
//! Baseline before DD.12 (HEAD 446ff17a, the same repos through the HEAD
//! API): `tests_for(["lib::money::format_cents"])` listed two rows,
//! `tests::test_invoice::test_invoice` (derived, depth 2, path
//! `[lib::money::invoice CALLS, lib::money::format_cents CALLS]`) and the
//! heuristic module pairing `tests::test_money` (module_tests_edge, depth 2)
//! — no `test_checkout`, TEST_REACH has no identity hop — and this file did
//! not compile (`TestsForArgs` had no `collapse`, `TestHit` no `identity`).

use std::path::Path;

use glia_code_domain::edge_category;
use glia_code_domain::snapshots::{
    ScipDefRow, ScipDocumentRecord, ScipMeta, ScipSymbolRecord, source_hash, write_scip,
};
use glia_engine::identity::Collapse;
use glia_engine::tests_for::{TestHit, TestsFor, TestsForArgs, tests_for, tests_for_diff};
use glia_engine::{GenerateResult, generate_many};

const BILLING_MONEY: &str = "def format_cents(cents):\n    return \"%.2f\" % (cents / 100)\n\n\ndef invoice(cents):\n    return format_cents(cents)\n";
const SHOP_MONEY: &str = "def format_cents(cents):\n    return \"%.2f EUR\" % (cents / 100)\n\n\ndef checkout(cents):\n    return format_cents(cents)\n\n\ndef main():\n    checkout(100)\n";
const BILLING_TEST: &str = "from lib.money import invoice\n\n\ndef test_invoice():\n    invoice(100)\n";
const SHOP_TEST: &str = "from app.money import checkout\n\n\ndef test_checkout():\n    checkout(100)\n";
const BILLING_FMT: &str = "from lib.money import format_cents\n\n\ndef test_format():\n    assert format_cents(100) == \"1.00\"\n";
const SHOP_FMT: &str = "from app.money import format_cents\n\n\ndef test_format():\n    assert format_cents(100) == \"1.00 EUR\"\n";

const SEED: &str = "lib::money::format_cents";
const TEST_INVOICE: &str = "tests::test_invoice::test_invoice";
const TEST_CHECKOUT: &str = "tests::test_money::test_checkout";
const TEST_FORMAT: &str = "tests::test_fmt::test_format";

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, text).expect("write");
}

/// `repo`'s snapshot: `<dir>/money.py` defines `format_cents` (row 0) and,
/// with `fmt`, `tests/test_fmt.py` defines `test_format` (row 3), each under
/// the package `money` whatever the directory, so each name is one class
/// across the repos.
fn snapshot(repo: &Path, dir: &str, fmt: bool) {
    let doc = |path: String, s: u32, line: u32, name: &str| {
        let bytes = std::fs::read(repo.join(&path)).expect("source");
        ScipDocumentRecord {
            path,
            language: "python".to_string(),
            source_hash: source_hash(&bytes),
            defs: vec![ScipDefRow { s, line, name: name.to_string() }],
            refs: Vec::new(),
        }
    };
    let symbol = |id: u32, descriptor: &str| ScipSymbolRecord {
        id,
        symbol: format!("scip-python python money 1.0.0 {descriptor}"),
        implements: Vec::new(),
    };
    let mut documents = vec![doc(format!("{dir}/money.py"), 0, 0, "format_cents")];
    let mut symbols = vec![symbol(0, "`money`/format_cents().")];
    if fmt {
        documents.push(doc("tests/test_fmt.py".to_string(), 1, 3, "test_format"));
        symbols.push(symbol(1, "`tests.test_fmt`/test_format()."));
    }
    let meta = ScipMeta::new("scip-python".to_string(), "0.6.0".to_string(), String::new(), 0);
    write_scip(repo, meta, &documents, &symbols).expect("write snapshot");
}

/// The two repos in a fresh tempdir (removed on drop), snapshots written.
fn repos(fmt_tests: bool) -> tempfile::TempDir {
    let td = tempfile::tempdir().expect("tempdir");
    write(td.path(), "billing/lib/money.py", BILLING_MONEY);
    write(td.path(), "billing/tests/test_invoice.py", BILLING_TEST);
    write(td.path(), "shop/app/money.py", SHOP_MONEY);
    write(td.path(), "shop/tests/test_money.py", SHOP_TEST);
    if fmt_tests {
        write(td.path(), "billing/tests/test_fmt.py", BILLING_FMT);
        write(td.path(), "shop/tests/test_fmt.py", SHOP_FMT);
    }
    snapshot(&td.path().join("billing"), "lib", fmt_tests);
    snapshot(&td.path().join("shop"), "app", fmt_tests);
    td
}

fn build_paths(roots: &[&Path]) -> GenerateResult {
    let paths: Vec<String> = roots.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    generate_many(&paths).unwrap_or_else(|e| panic!("{paths:?} builds: {e}"))
}

/// billing then shop: billing is the first graph, so its member of each
/// class is the display member.
fn build_both(td: &tempfile::TempDir) -> GenerateResult {
    build_paths(&[&td.path().join("billing"), &td.path().join("shop")])
}

fn same_as_edges(r: &GenerateResult) -> usize {
    r.merged.cross_edges.iter().filter(|e| e.category == edge_category::SAME_AS).count()
}

/// Options: collapse on with the build's labels (`on`) or off.
fn args(r: &GenerateResult, on: bool) -> TestsForArgs {
    let mut a = TestsForArgs::default();
    a.collapse = if on { Collapse::with_labels(&r.repo_labels) } else { Collapse::off() };
    a
}

fn answer(r: &GenerateResult, seeds: &[&str], a: &TestsForArgs) -> TestsFor {
    tests_for(&r.merged, seeds, a).expect("an answer")
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("answer serialises")
}

/// The FUNCTION / METHOD rows (the test cases), qnames sorted.
fn cases(a: &TestsFor) -> Vec<&str> {
    let mut q: Vec<&str> = a.tests.iter().filter(|t| t.kind != "MODULE").map(|t| t.qname.as_str()).collect();
    q.sort_unstable();
    q
}

fn row<'a>(a: &'a TestsFor, qname: &str) -> &'a TestHit {
    a.tests
        .iter()
        .find(|t| t.qname == qname)
        .unwrap_or_else(|| panic!("no `{qname}` row in {a:#?}"))
}

fn path_qnames(t: &TestHit) -> Vec<&str> {
    t.path.iter().map(|(q, _)| q.as_str()).collect()
}

// ---- tests ------------------------------------------------------------------

/// (1) A change to billing's `format_cents` finds shop's `test_checkout` by
/// default: over the quotient the two copies are one node, so `checkout`
/// (shop) is one hop from the seed and `test_checkout` two, derived, its path
/// running through the class onto the seed. `Collapse::off()` is the HEAD
/// answer: `test_invoice` only.
#[test]
fn shared_copy_finds_other_repo_tests() {
    let td = repos(false);
    let r = build_both(&td);
    assert!(same_as_edges(&r) > 0, "control: the format_cents copies are one SCIP class");

    let off = answer(&r, &[SEED], &args(&r, false));
    assert_eq!(cases(&off), [TEST_INVOICE], "HEAD: billing's test only: {off:#?}");
    assert!(!json(&off).contains("\"identity\""), "{}", json(&off));

    assert!(TestsForArgs::default().collapse.enabled, "collapse is on by default");
    let on = answer(&r, &[SEED], &args(&r, true));
    assert_eq!(cases(&on), [TEST_INVOICE, TEST_CHECKOUT], "{on:#?}");
    assert_eq!(on.seeds, [SEED]);
    assert!(on.untested.is_empty(), "{:?}", on.untested);

    let checkout = row(&on, TEST_CHECKOUT);
    assert_eq!((checkout.tier, checkout.reason, checkout.depth), ("derived", "reaches", 2), "{checkout:#?}");
    assert_eq!(path_qnames(checkout), ["app::money::checkout", SEED], "through the class onto the seed");
    assert_eq!(checkout.path.len(), checkout.depth);
    assert_eq!(checkout.covers, [SEED]);
    assert_eq!(checkout.file.as_deref(), Some("tests/test_money.py"));
    assert_eq!(checkout.line, Some(4), "1-based");
    assert!(checkout.identity.is_none(), "test_checkout is in no class");

    // billing's own row is the HEAD row.
    let invoice = row(&on, TEST_INVOICE);
    let head = row(&off, TEST_INVOICE);
    assert_eq!(json(invoice), json(head));
    assert!(on.test_files.contains(&"tests/test_money.py".to_string()), "{:?}", on.test_files);

    // Diff mode walks the same quotient.
    let diff = "--- a/lib/money.py\n+++ b/lib/money.py\n@@ -1,2 +1,2 @@\n def format_cents(cents):\n\
-    return \"%.2f\" % (cents / 100)\n+    return \"%.2f\" % (cents / 100.0)\n";
    let d = tests_for_diff(&r.merged, diff, &args(&r, true)).expect("a diff answer");
    assert!(cases(&d).contains(&TEST_CHECKOUT), "{d:#?}");
    let d_off = tests_for_diff(&r.merged, diff, &args(&r, false)).expect("a diff answer");
    assert!(!cases(&d_off).contains(&TEST_CHECKOUT), "{d_off:#?}");
}

/// (2) Two seeds in one walk class are one seed (the first by qname); off,
/// they are two.
#[test]
fn class_seeds_are_one_seed() {
    let td = repos(false);
    let r = build_both(&td);
    let both = ["lib::money::format_cents", "app::money::format_cents"];
    let on = answer(&r, &both, &args(&r, true));
    assert_eq!(on.seeds, ["app::money::format_cents"], "the first by (qname, id) kept");
    assert_eq!(cases(&on), [TEST_INVOICE, TEST_CHECKOUT], "{on:#?}");
    let off = answer(&r, &both, &args(&r, false));
    assert_eq!(off.seeds.len(), 2, "{:?}", off.seeds);
    assert_eq!(cases(&off), [TEST_INVOICE, TEST_CHECKOUT], "each seed's own test: {off:#?}");
}

/// (3) Test rows of one class fold: each repo's `test_format` is one SCIP
/// class of two test cases, so the change lists ONE `test_format` row, the
/// display member (billing's, the first graph) carrying `identity` with
/// shop's copy in `also_in`. Off, billing's alone reaches billing's seed.
#[test]
fn test_copies_fold_with_identity() {
    let td = repos(true);
    let r = build_both(&td);

    let off = answer(&r, &[SEED], &args(&r, false));
    let off_fmt: Vec<&TestHit> = off.tests.iter().filter(|t| t.qname == TEST_FORMAT).collect();
    assert_eq!(off_fmt.len(), 1, "HEAD: billing's test_format: {off:#?}");
    assert_eq!(off_fmt[0].file.as_deref(), Some("tests/test_fmt.py"));
    assert!(off_fmt[0].identity.is_none());

    let on = answer(&r, &[SEED], &args(&r, true));
    let fmt: Vec<&TestHit> = on.tests.iter().filter(|t| t.qname == TEST_FORMAT).collect();
    assert_eq!(fmt.len(), 1, "one row for the class: {on:#?}");
    let t = fmt[0];
    assert_eq!(t.depth, 1, "{t:#?}");
    assert_eq!(t.tier, off_fmt[0].tier, "billing's own best hit: {t:#?}");
    assert_eq!(t.line, off_fmt[0].line, "billing's copy is the one shown");
    let id = t.identity.as_ref().expect("the class row carries its identity");
    assert_eq!(id.tier, "fact");
    assert_eq!(id.rules, ["scip_symbol"]);
    assert!(id.diverged, "the two bodies differ");
    let also: Vec<(&str, &str)> = id.also_in.iter().map(|a| (a.repo.as_str(), a.qname.as_str())).collect();
    assert_eq!(also, [("shop", TEST_FORMAT)]);
    assert!(json(&on).contains("\"identity\":{\"tier\":\"fact\""), "{}", json(&on));
    // The other repo's tests are found too.
    assert!(cases(&on).contains(&TEST_CHECKOUT), "{on:#?}");

    // A limit counts the class once.
    let mut one = args(&r, true);
    one.limit = Some(on.tests.len());
    let cut = answer(&r, &[SEED], &one);
    assert_eq!((cut.tests.len(), cut.omitted), (on.tests.len(), 0));

    // shop first: shop's test_format is the class's display member, so the
    // walk's row for the class shows it while billing's TESTS edge straight
    // to the seed is a fact row of billing's copy. Two rows of one class:
    // the fold keeps the better-ranked (fact) one and drops the other.
    let rev = build_paths(&[&td.path().join("shop"), &td.path().join("billing")]);
    let on = answer(&rev, &[SEED], &args(&rev, true));
    let fmt: Vec<&TestHit> = on.tests.iter().filter(|t| t.qname == TEST_FORMAT).collect();
    assert_eq!(fmt.len(), 1, "folded to one row: {on:#?}");
    assert_eq!((fmt[0].tier, fmt[0].reason), ("fact", "tests_edge"), "{:#?}", fmt[0]);
    assert_eq!(fmt[0].line, off_fmt[0].line);
    let id = fmt[0].identity.as_ref().expect("the kept row carries the class");
    let also: Vec<(&str, &str)> = id.also_in.iter().map(|a| (a.repo.as_str(), a.qname.as_str())).collect();
    assert_eq!(also, [("shop", TEST_FORMAT)]);
    let raw = answer(&rev, &[SEED], &args(&rev, false));
    assert_eq!(raw.tests.iter().filter(|t| t.qname == TEST_FORMAT).count(), 1, "{raw:#?}");
}

/// (4) No SAME_AS edge: the default answer is the `Collapse::off()` answer,
/// byte for byte, with no identity key.
#[test]
fn class_free_identical() {
    let td = repos(false);
    let billing = build_paths(&[&td.path().join("billing")]);
    assert_eq!(same_as_edges(&billing), 0, "control: one SCIP repo, no class");
    for seeds in [&[SEED][..], &["invoice"][..], &[TEST_INVOICE][..], &["no_such_symbol"][..]] {
        let a = json(&answer(&billing, seeds, &TestsForArgs::default()));
        let b = json(&answer(&billing, seeds, &args(&billing, false)));
        assert_eq!(a, b, "{seeds:?}");
        assert!(!a.contains("\"identity\""), "{a}");
    }
}
