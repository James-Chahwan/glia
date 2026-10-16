//! DT.1h acceptance: the `models` Resolve pass over the committed
//! `models-http-shop` fixture (an Angular web client and a gin API, built with
//! `generate_many` over the dirs its key.json names). Every confirmed client
//! <-> server model pair DT.1m finds becomes an `http_model` SAME_AS class,
//! client model -> server model (the center), EVIDENCE `resolver:models`,
//! DERIVED (Medium), located at the first joining call site; `/diverged` when
//! an exchange showed drift at that model's level.
//!
//! Of DT.1m's eight model pairs, five are classes: TS `Listing` pairs both
//! `models.Listing` and `models.ListingWithSeller`, and ListingWithSeller
//! embeds Listing, so the embedded base is kept; TS `Summary` pairs both
//! `DailySummary` and `WeeklySummary`, neither embedding the other, so it
//! joins nothing. The identity report, `why` and `find` read the classes as
//! every other DERIVED identity class.
//!
//! Baseline before DT.1h (HEAD ad52b2eb): the fixture graded SAME_AS 0/5
//! (forbid 0), `identity_classes` was empty (`no SAME_AS edge`).
//!
//! fired_on: `[models] classes=5 edges=5 diverged=3 ambiguous_client=1
//! ambiguous_project=0 embed_reduced=1` and `[evidence-rules] resolver=models
//! http_model=5` on stderr of every build of the fixture.

use std::path::{Path, PathBuf};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{edge_category, same_as};
use glia_core::Confidence;
use glia_engine::find::{FindOptions, find_nodes};
use glia_engine::identity::{Collapse, IdentityQuery, identity_classes};
use glia_engine::merge::{MergeMember, merge_layouts, persist_merge, read_workspace};
use glia_engine::persist::persist_result;
use glia_engine::why::why_edge;
use glia_engine::{GenerateResult, generate_many};
use glia_graph::MergedGraph;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
}

/// The repo paths a fixture's key.json `dirs` names, in order.
fn fixture_dirs(name: &str) -> Vec<String> {
    let root = fixture(name);
    let key: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("key.json")).unwrap()).unwrap();
    key["dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| root.join(d.as_str().unwrap()).to_str().unwrap().to_string())
        .collect()
}

fn shop() -> GenerateResult {
    generate_many(&fixture_dirs("models-http-shop")).expect("models-http-shop builds")
}

fn qname(m: &MergedGraph, id: glia_core::NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// `(from, to, emitter, rule, file, 0-based line)` of every SAME_AS edge,
/// with its confidence, sorted.
type Row = (String, String, String, String, String, u32, Confidence);

fn same_as_rows(m: &MergedGraph) -> Vec<Row> {
    let mut rows: Vec<Row> = m
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::SAME_AS)
        .map(|e| {
            let ev = Evidence::of(e).expect("every SAME_AS edge carries EVIDENCE");
            (
                qname(m, e.from),
                qname(m, e.to),
                ev.emitter,
                ev.rule.unwrap_or_default(),
                ev.file.unwrap_or_default(),
                ev.line.unwrap_or(u32::MAX),
                e.confidence,
            )
        })
        .collect();
    rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    rows
}

fn row(from: &str, to: &str, rule: &str, file: &str, line: u32) -> Row {
    (
        from.to_string(),
        to.to_string(),
        "resolver:models".to_string(),
        rule.to_string(),
        file.to_string(),
        line,
        Confidence::Medium,
    )
}

const INVOICES: &str = "src/app/services/invoice.service.ts";
const CATALOG: &str = "src/app/services/catalog.service.ts";

#[test]
fn model_pairs_become_http_model_stars() {
    let r = shop();
    let rows = same_as_rows(&r.merged);
    assert_eq!(
        rows,
        [
            // GET /api/articles writes []models.CatalogEntry: never the
            // stored models.Article of the same name.
            row(
                "src::app::models::catalog::Article",
                "models::catalog::CatalogEntry",
                "http_model",
                CATALOG,
                10
            ),
            // The embedded base wins over ListingWithSeller; sellerName is
            // optional and never written.
            row(
                "src::app::models::catalog::Listing",
                "models::catalog::Listing",
                "http_model/diverged",
                CATALOG,
                14
            ),
            // customerName against customer_name, couponCode never written;
            // the first join is the invoice-pages envelope.
            row(
                "src::app::models::invoice::Invoice",
                "models::invoice::Invoice",
                "http_model/diverged",
                INVOICES,
                24
            ),
            row(
                "src::app::models::invoice::InvoiceItem",
                "models::invoice::InvoiceItem",
                "http_model",
                INVOICES,
                24
            ),
            // The POST request: couponCode is dropped.
            row(
                "src::app::models::invoice::NewInvoice",
                "models::invoice::CreateInvoiceRequest",
                "http_model/diverged",
                INVOICES,
                16
            ),
        ]
    );
    for e in r
        .merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::SAME_AS)
    {
        assert_eq!(e.cells.len(), 1, "one EVIDENCE cell: {e:?}");
    }
}

#[test]
fn identity_report_lists_five_derived_classes() {
    let r = shop();
    let mut q = IdentityQuery::default();
    q.repo_labels = r.repo_labels.clone();
    let report = identity_classes(&r.merged, &q).expect("no filter: a report");
    let c = &report.counts;
    assert_eq!(
        (
            c.classes,
            c.members,
            c.fact,
            c.derived,
            c.heuristic,
            c.diverged
        ),
        (5, 10, 0, 5, 0, 3)
    );
    assert_eq!(
        c.by_rule.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        same_as::ALL_RULES,
        "every table rule, in table order"
    );
    assert_eq!(c.by_rule.last(), Some(&("http_model", 5)));
    assert!(report.absence.is_none());
    for class in &report.classes {
        assert_eq!(
            (
                class.tier,
                class.rules.as_slice(),
                class.collapses,
                class.walks
            ),
            ("derived", ["http_model"].as_slice(), true, true),
            "{class:?}"
        );
        assert_eq!(
            class.members.len(),
            2,
            "one client and one server: {class:?}"
        );
    }
    let listing = report
        .classes
        .iter()
        .find(|k| {
            k.members
                .iter()
                .any(|m| m.qname == "models::catalog::Listing")
        })
        .expect("a Listing class");
    let mut members: Vec<(&str, &str)> = listing
        .members
        .iter()
        .map(|m| (m.repo.as_str(), m.qname.as_str()))
        .collect();
    members.sort();
    assert_eq!(
        members,
        [
            ("api", "models::catalog::Listing"),
            ("web", "src::app::models::catalog::Listing"),
        ]
    );
    assert!(listing.diverged);

    let mut by_rule = IdentityQuery::default();
    by_rule.rule = Some("http_model".to_string());
    by_rule.diverged_only = true;
    let diverged = identity_classes(&r.merged, &by_rule).expect("a known rule");
    assert_eq!(diverged.counts.classes, 3);
}

#[test]
fn why_explains_the_identity_edge() {
    let r = shop();
    let a = why_edge(
        &r.merged,
        "src::app::models::invoice::Invoice",
        "models::invoice::Invoice",
        Some("SAME_AS"),
    )
    .expect("both sides resolve");
    assert!(a.found, "{a:?}");
    assert_eq!(a.edges.len(), 1, "{a:?}");
    let e = &a.edges[0];
    assert_eq!(
        (
            e.category,
            e.tier,
            e.emitter.as_deref(),
            e.rule.as_deref(),
            e.note.as_deref(),
            e.cross_repo
        ),
        (
            "SAME_AS",
            "derived",
            Some("resolver:models"),
            Some("http_model/diverged"),
            Some("identity class (http_model, diverged)"),
            true
        )
    );
    let site = e.site.as_ref().expect("located at the call site");
    assert_eq!((site.file.as_str(), site.line), (INVOICES, Some(25)));
}

#[test]
fn find_folds_the_client_and_server_model_into_one_row() {
    let r = shop();
    let mut o = FindOptions::default();
    o.collapse = Collapse::with_labels(&r.repo_labels);
    let rows = find_nodes(&r.merged, "CreateInvoiceRequest", &o).results;
    let shown: Vec<&str> = rows.iter().map(|f| f.qname.as_str()).collect();
    assert!(
        !(shown.contains(&"models::invoice::CreateInvoiceRequest")
            && shown.contains(&"src::app::models::invoice::NewInvoice")),
        "one row for the class: {shown:?}"
    );
    let row = rows
        .iter()
        .find(|f| f.identity.is_some())
        .unwrap_or_else(|| panic!("a collapsed row: {rows:?}"));
    let id = row.identity.as_ref().expect("identity");
    assert_eq!(
        (id.tier, id.rules.as_slice(), id.diverged),
        ("derived", ["http_model"].as_slice(), true)
    );
    let also: Vec<&str> = id.also_in.iter().map(|a| a.qname.as_str()).collect();
    let pair = [row.qname.as_str(), also[0]];
    assert!(
        pair.contains(&"models::invoice::CreateInvoiceRequest")
            && pair.contains(&"src::app::models::invoice::NewInvoice"),
        "{pair:?}"
    );

    let mut raw = FindOptions::default();
    raw.collapse = Collapse::off();
    let raw_rows = find_nodes(&r.merged, "CreateInvoiceRequest", &raw).results;
    assert!(raw_rows.iter().all(|f| f.identity.is_none()));
}

#[test]
fn model_identity_is_deterministic() {
    assert_eq!(same_as_rows(&shop().merged), same_as_rows(&shop().merged));
}

/// A layout member through the workspace reader, as a caller outside the
/// crate builds one.
fn gmap_member(name: &str, dir: &Path) -> MergeMember {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("glia.workspace.json");
    let member = format!(r#"{{"name":"{name}","gmap":"{}"}}"#, dir.display());
    std::fs::write(&path, format!(r#"{{"version":1,"members":[{member}]}}"#)).unwrap();
    read_workspace(&path).unwrap().remove(0)
}

/// Every `.gmap` file directly in `dir`, by name.
fn gmaps(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".gmap"))
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

/// merge == joint build with http_model stars: neither member alone pairs
/// an exchange, the merge re-runs `http` and then `models` over the union
/// (both `resolver:` emitters), from persisted cells and symbol tables only,
/// and its shards are the joint build's bytes.
#[test]
fn merge_equals_the_joint_build() {
    let dirs = fixture_dirs("models-http-shop");
    let tmp = tempfile::tempdir().unwrap();
    let (a, b, t) = (
        tmp.path().join("web"),
        tmp.path().join("api"),
        tmp.path().join("joint"),
    );
    let web = generate_many(&dirs[..1]).unwrap();
    assert!(
        same_as_rows(&web.merged).is_empty(),
        "one repo pairs no exchange"
    );
    persist_result(&web, &a, "test").unwrap();
    persist_result(&generate_many(&dirs[1..]).unwrap(), &b, "test").unwrap();
    let joint = generate_many(&dirs).unwrap();
    persist_result(&joint, &t, "test").unwrap();

    let m = merge_layouts(&[gmap_member("web", &a), gmap_member("api", &b)]).unwrap();
    assert_eq!(same_as_rows(&m.result.merged), same_as_rows(&joint.merged));
    assert_eq!(same_as_rows(&m.result.merged).len(), 5);
    let out = tmp.path().join("merged");
    persist_merge(&m, &out, "test").unwrap();
    let (gm, gt) = (gmaps(&out), gmaps(&t));
    assert_eq!(
        gm.iter().map(|g| &g.0).collect::<Vec<_>>(),
        gt.iter().map(|g| &g.0).collect::<Vec<_>>()
    );
    for ((name, bm), (_, bt)) in gm.iter().zip(&gt) {
        assert!(
            bm == bt,
            "{name}: merged bytes differ from the joint build's"
        );
    }
}
