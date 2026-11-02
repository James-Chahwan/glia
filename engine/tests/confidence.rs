//! HC.4 - calibrated confidence: every answer row that rests on edges carries
//! `precision`, the audited precision of its stratum (the table generated from
//! bench/edge-audit/calibration.json after the confidence gate read PASS),
//! beside its tier. Before this a row carried a tier and nothing measured how
//! often that tier's edges are right: `why`, `check`, `query`, `implementors`
//! and `review` rows had no `precision` key at all.
//!
//! The number never moves anything: with it dropped (`--no-precision`,
//! `precision=False`, `Annotated::drop_precision`) every row, order, tier and
//! gate is byte-identical to the answer before HC.4.

use glia_code_domain::edge_category;
use glia_engine::check::check;
use glia_engine::confidence::{self, AUDIT, Annotated, Precision, Table};
use glia_engine::implementors::{HierarchyDirection, implementors};
use glia_engine::rules::{QueryOptions, query};
use glia_engine::why::why_edge;
use glia_engine::{Locator, generate_one};
use glia_graph::MergedGraph;

fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (rel, src) in files {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let merged = generate_one(tmp.path().to_str().expect("utf-8 temp path"))
        .expect("generate_one")
        .merged;
    (tmp, merged)
}

const SHOP: &str = "def price(o):\n    return o\n\n\ndef place(o):\n    return price(o)\n";

/// The table's row of `stratum`, read back from the shipped JSON.
fn table_row(stratum: &str) -> serde_json::Value {
    let text = include_str!("../src/confidence/table.json");
    let t: serde_json::Value = serde_json::from_str(text).expect("table parses");
    t["strata"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(t["sub_strata"].as_array().into_iter().flatten())
        .find(|r| r["stratum"] == stratum)
        .cloned()
        .unwrap_or_default()
}

/// `v` with every `precision` / `precision_by_tier` key removed, recursively.
fn strip(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(m) => {
            m.remove("precision");
            m.remove("precision_by_tier");
            m.values_mut().for_each(strip);
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
        _ => {}
    }
}

/// The answer's JSON with its keys stripped equals the answer with its
/// precision dropped: the key is the only difference.
fn only_the_key_differs<T: serde::Serialize + Annotated + Clone>(answer: &T) {
    let mut kept = serde_json::to_value(answer).expect("serialises");
    let mut dropped = answer.clone();
    dropped.drop_precision();
    assert_eq!(dropped.precision_tally().rows, 0);
    strip(&mut kept);
    assert_eq!(
        kept,
        serde_json::to_value(&dropped).expect("serialises"),
        "precision moved something beside its own key"
    );
}

/// A CALLS|fact edge reads the table's CALLS|fact interval in `why`, named.
#[test]
fn why_row_reads_its_stratum() {
    let (_tmp, m) = build(&[("shop/a.py", SHOP)]);
    let a = why_edge(&m, "shop::a::place", "shop::a::price", None).expect("why");
    let r = &a.edges[0];
    assert_eq!((r.category, r.tier), ("CALLS", "fact"));
    let p = r.precision.as_ref().expect("a precision beside the tier");
    let want = table_row(&p.stratum);
    assert!(p.stratum.starts_with("CALLS|fact"), "{p:?}");
    assert_eq!(p.lo, want["lo"].as_f64(), "{p:?} vs {want}");
    assert_eq!(p.p, want["p"].as_f64());
    assert_eq!(Some(p.n), want["n"].as_u64());
    assert_eq!(p.audit, AUDIT);
    assert!(p.measured() && !p.under_floor, "{p:?}");
    let json = serde_json::to_value(r).expect("serialises");
    assert_eq!(json["precision"]["stratum"], p.stratum.as_str(), "{json}");
    assert_eq!(json["precision"]["audit"], AUDIT, "{json}");
    assert!(json["precision"].get("unmeasured").is_none(), "{json}");
    assert_eq!(a.precision_tally().measured, 1);
    only_the_key_differs(&a);
}

/// A DOCUMENTS|derived row (the doc linker's) is flagged under its floor.
#[test]
fn documents_derived_is_under_floor() {
    let (_tmp, m) = build(&[
        ("shop/a.py", SHOP),
        (
            "README.md",
            "# Shop\n\nCall `place()` to price an order.\n",
        ),
    ]);
    let doc = m
        .all_edges()
        .find(|e| e.category == edge_category::DOCUMENTS)
        .expect("the doc linker binds a DOCUMENTS edge");
    let p = confidence::of_edge(doc);
    assert!(p.stratum.starts_with("DOCUMENTS|derived"), "{p:?}");
    assert!(p.under_floor && p.measured(), "{p:?}");
    let loc = Locator::new(&m);
    let a = why_edge(
        &m,
        &loc.locate(doc.from).qname,
        &loc.locate(doc.to).qname,
        Some("DOCUMENTS"),
    )
    .expect("why");
    let row = a.edges.first().expect("one DOCUMENTS row");
    assert_eq!(row.tier, "derived");
    let text = confidence::tier_text(row.tier, row.precision.as_ref());
    assert!(
        text.starts_with("derived (>= ") && text.ends_with(", under floor)"),
        "{text}"
    );
    assert_eq!(a.precision_tally().under_floor, 1);
}

/// A heuristic overlay edge reads its (category, heuristic) stratum.
#[test]
fn overlay_edge_reads_its_heuristic_stratum() {
    let overlay = "version = 1\n\n[[edge]]\nfrom = \"shop::a::price\"\nto = \"shop::b::tax\"\ncategory = \"IMPORTS\"\nnote = \"declared\"\norigin = \"human\"\n";
    let (_tmp, m) = build(&[
        ("shop/a.py", SHOP),
        ("shop/b.py", "def tax(o):\n    return o\n"),
        (".glia/overlay.toml", overlay),
    ]);
    let a = why_edge(&m, "shop::a::price", "shop::b::tax", Some("IMPORTS")).expect("why");
    let r = a.edges.first().expect("the overlay edge");
    assert_eq!(r.tier, "heuristic", "{r:?}");
    let p = r.precision.as_ref().expect("precision");
    assert_eq!(p.stratum, "IMPORTS|heuristic", "{p:?}");
    assert_eq!(p.lo, table_row("IMPORTS|heuristic")["lo"].as_f64());
}

/// An empty table leaves every edge unmeasured, and the rows, order, tiers
/// and gates of check, query and implementors are unchanged by the key.
#[test]
fn precision_moves_no_row_order_tier_or_gate() {
    let rules = "version = 1\n\n[[constraint]]\nid = \"api-acyclic\"\nkind = \"no_cycle\"\nscope = \"services/api\"\n\n[[constraint]]\nid = \"web-no-api\"\nkind = \"forbid_edge\"\nfrom = \"web\"\nto = \"services/api\"\ncategories = [\"IMPORTS\", \"CALLS\"]\n";
    let (_tmp, m) = build(&[
        ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
        ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
        (
            "web/app.py",
            "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n",
        ),
        ("services/api/internal.py", "def charge(o):\n    return o\n"),
        (
            "services/api/a.py",
            "from services.api.b import g\n\n\ndef f():\n    return g()\n",
        ),
        (
            "services/api/b.py",
            "from services.api.a import f\n\n\ndef g():\n    return 1\n",
        ),
        (".glia/overlay.toml", rules),
    ]);
    let report = check(&m);
    assert!(!report.violations.is_empty(), "{report:?}");
    for v in &report.violations {
        let p = v.precision.as_ref().expect("a violation's precision");
        // forbid_edge: the strongest row of its tier; no_cycle: the weakest hop.
        let rows: Vec<&Precision> = v
            .evidence
            .iter()
            .filter_map(|e| e.precision.as_ref())
            .collect();
        assert_eq!(rows.len(), v.evidence.len(), "every row has one");
        let want = if v.rule_kind == "forbid_edge" {
            confidence::strongest(
                v.evidence
                    .iter()
                    .filter(|e| e.tier == v.tier)
                    .filter_map(|e| e.precision.as_ref()),
            )
        } else {
            confidence::weakest(rows)
        };
        assert_eq!(Some(p), want.as_ref(), "{v:?}");
    }
    let blocking: Vec<bool> = report.violations.iter().map(|v| v.blocking).collect();
    let mut dropped = report.clone();
    dropped.drop_precision();
    assert_eq!(
        dropped.violations.iter().map(|v| v.blocking).collect::<Vec<_>>(),
        blocking,
        "the gate reads no precision"
    );
    only_the_key_differs(&report);

    let mut opts = QueryOptions::default();
    opts.explain = true;
    let q = query(&m, "out(A, B) :- calls(A, B).", &opts).expect("query");
    assert!(!q.results.is_empty());
    for r in &q.results {
        let w = confidence::weakest(r.witness.iter().filter_map(|e| e.precision.as_ref()));
        assert_eq!(r.precision, w, "{r:?}");
    }
    only_the_key_differs(&q);

    let imp = implementors(&m, "shop", HierarchyDirection::Down, true);
    only_the_key_differs(&imp);

    let empty = Table::empty();
    let e = m.all_edges().next().expect("an edge");
    assert!(!empty.of_edge(e).measured());
    assert!(empty.of_tier("fact").is_none());
}

/// Implementors: a row's precision is its path's weakest link.
#[test]
fn implementors_row_reads_its_path() {
    let ts = "export interface Repo {\n  get(id: string): string;\n}\n\nexport class PgRepo implements Repo {\n  get(id: string): string { return id; }\n}\n\nexport class CachedRepo extends PgRepo {\n  get(id: string): string { return super.get(id); }\n}\n";
    let (_tmp, m) = build(&[("src/repo.ts", ts)]);
    let a = implementors(&m, "Repo", HierarchyDirection::Down, true);
    assert!(a.results.len() >= 2, "{a:?}");
    for r in &a.results {
        let p = r.precision.as_ref().expect("a row on edges has one");
        assert!(
            p.stratum.starts_with("IMPLEMENTS|") || p.stratum.starts_with("INHERITS_FROM|"),
            "{p:?}"
        );
    }
    let deep = a.results.iter().find(|r| r.depth == 2).expect("depth 2");
    let direct = a.results.iter().find(|r| r.depth == 1).expect("depth 1");
    let (dp, rp) = (
        deep.precision.as_ref().expect("p"),
        direct.precision.as_ref().expect("p"),
    );
    assert!(dp.lo <= rp.lo, "the deeper row is no stronger: {dp:?} {rp:?}");
    only_the_key_differs(&a);
}
