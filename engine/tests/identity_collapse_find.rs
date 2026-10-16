//! DD.9: `find` collapses identity classes. A FACT / DERIVED SAME_AS class
//! (DD.5's resolver, read through DD.8's `IdentityView`) is ONE row: the
//! best-ranked matching member, carrying `identity {tier, rules, diverged,
//! also_in}`; the fold runs on the ranked candidates BEFORE the top_k cut, so
//! a class never spends two slots. A HEURISTIC candidate class (vendored
//! copies) never folds, and a node in no class serialises byte for byte as it
//! did before the fold. `Collapse::off()` is the raw per-member answer.
//!
//! Real `generate_many` builds of DD.5's substrate-gap identity fixtures.
//!
//! Baseline before DD.9 (HEAD d263996c): this file did not compile
//! (`FindOptions` had no `collapse`, `FoundNode` no `identity`), and
//! `glia find <identity-npm-package/web> package:npm:react --with <admin>`
//! printed two `exact_qname` PACKAGE_DEP rows, both `file: null`;
//! `glia find <identity-contract-ops> GET:/orders` two `qname_suffix`
//! DOC_SECTION rows (docs/swagger.yaml:7, main.go:14).

use std::path::{Path, PathBuf};

use glia_code_domain::edge_category;
use glia_core::NodeId;
use glia_engine::find::{FindOptions, FoundNode, find_nodes};
use glia_engine::identity::Collapse;
use glia_engine::pack::{PackArgs, pack};
use glia_engine::{GenerateResult, generate_many};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
}

fn build(name: &str, dirs: &[&str]) -> GenerateResult {
    let root = fixture(name);
    let paths: Vec<String> = dirs
        .iter()
        .map(|d| root.join(d).to_string_lossy().into_owned())
        .collect();
    generate_many(&paths).unwrap_or_else(|e| panic!("{paths:?} builds: {e}"))
}

fn same_as_edges(r: &GenerateResult) -> usize {
    r.merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::SAME_AS)
        .count()
}

/// `find` with `top_k` and the build's labels, collapse on (the default).
fn collapsed(r: &GenerateResult, q: &str, top_k: usize) -> Vec<FoundNode> {
    let mut o = FindOptions::default();
    o.top_k = top_k;
    o.collapse = Collapse::with_labels(&r.repo_labels);
    find_nodes(&r.merged, q, &o).results
}

/// The raw, per-member answer.
fn raw(r: &GenerateResult, q: &str, top_k: usize) -> Vec<FoundNode> {
    let mut o = FindOptions::default();
    o.top_k = top_k;
    o.collapse = Collapse::off();
    find_nodes(&r.merged, q, &o).results
}

/// The label of the repo whose graph holds `id`.
fn repo_of(r: &GenerateResult, id: u64) -> String {
    r.merged
        .graphs
        .iter()
        .find(|g| g.nav.qname_by_id.contains_key(&NodeId(id)))
        .and_then(|g| r.repo_labels.get(&g.repo.0).cloned())
        .unwrap_or_else(|| panic!("no graph holds {id}"))
}

fn json(rows: &[FoundNode]) -> String {
    serde_json::to_string(rows).expect("rows serialise")
}

#[test]
fn package_rows_fold() {
    let r = build("identity-npm-package", &["web", "admin"]);
    assert!(same_as_edges(&r) > 0, "control: both repos declare react");
    let q = "package:npm:react";

    // Control: the HEAD answer, two members of one package.
    let off = raw(&r, q, 20);
    assert_eq!(off.len(), 2, "{off:#?}");
    assert!(
        off.iter()
            .all(|f| f.qname == q && f.r#match == "exact_qname")
    );
    assert!(off.iter().all(|f| f.identity.is_none()), "{off:#?}");
    assert!(!json(&off).contains("\"identity\""), "{}", json(&off));

    // The default options collapse.
    let mut o = FindOptions::default();
    assert!(o.collapse.enabled, "collapse is on by default");
    o.collapse = Collapse::with_labels(&r.repo_labels);
    let rows = find_nodes(&r.merged, q, &o).results;
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let row = &rows[0];
    assert_eq!((row.qname.as_str(), row.r#match), (q, "exact_qname"));
    assert_eq!(row.id, off[0].id, "the best-ranked member is the row");
    let id = row
        .identity
        .as_ref()
        .expect("a collapsed row carries its identity");
    assert_eq!(id.tier, "fact");
    assert_eq!(id.rules, ["package_name"]);
    assert!(!id.diverged);
    assert_eq!(id.also_in.len(), 1, "{:?}", id.also_in);
    let other = &id.also_in[0];
    let shown = repo_of(&r, row.id);
    assert_ne!(other.repo, shown, "also_in names the OTHER repo");
    assert_eq!(other.repo, repo_of(&r, off[1].id));
    assert!(["web", "admin"].contains(&other.repo.as_str()), "{other:?}");
    assert_eq!(
        (other.qname.as_str(), other.file.as_deref(), other.line),
        (q, None, None)
    );

    let text = json(&rows);
    assert!(text.contains("\"identity\":{\"tier\":\"fact\""), "{text}");
}

#[test]
fn contract_rows_fold() {
    let r = build("identity-contract-ops", &["."]);
    let q = "GET:/orders";
    let off = raw(&r, q, 20);
    let mut qnames: Vec<&str> = off.iter().map(|f| f.qname.as_str()).collect();
    qnames.sort_unstable();
    assert_eq!(
        qnames,
        [
            "contract::docs::swagger::GET:/orders",
            "contract::main::GET:/orders"
        ],
        "control: the swagger and swaggo ops both match"
    );

    let rows = collapsed(&r, q, 20);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let row = &rows[0];
    assert_eq!(
        row.id, off[0].id,
        "the best-ranked matching member is the row"
    );
    let id = row.identity.as_ref().expect("identity");
    assert_eq!(id.tier, "derived");
    assert_eq!(id.rules, ["contract_op", "contract_route"]);
    assert!(!id.diverged);
    let mut also: Vec<&str> = id.also_in.iter().map(|a| a.qname.as_str()).collect();
    also.sort_unstable();
    let other = off[1].qname.as_str();
    let mut want = vec![
        "contract::features::orders::feature::GET:/api/orders",
        other,
    ];
    want.sort_unstable();
    assert_eq!(
        also, want,
        "the other op, and the feature.yaml op through contract_route"
    );
    assert!(
        id.also_in
            .iter()
            .all(|a| a.file.is_some() && a.line.is_some_and(|l| l >= 1)),
        "contract ops are located, 1-based: {:?}",
        id.also_in
    );
}

#[test]
fn top_k_after_fold() {
    let r = build("identity-npm-package", &["web", "admin"]);
    // `package:npm:` is a qname substring of every npm dep: react and lodash
    // in both repos, left-pad and typescript in one each.
    let q = "package:npm:";
    let all_raw = raw(&r, q, 0);
    let all = collapsed(&r, q, 0);
    let distinct = |rows: &[FoundNode]| {
        let mut q: Vec<&str> = rows.iter().map(|f| f.qname.as_str()).collect();
        q.sort_unstable();
        q.dedup();
        q.len()
    };
    assert_eq!(all_raw.len(), 6, "{all_raw:#?}");
    assert_eq!(distinct(&all_raw), 4);
    assert_eq!(all.len(), 4, "one row per package: {all:#?}");
    assert_eq!(distinct(&all), 4);
    assert_eq!(
        all.iter().filter(|f| f.identity.is_some()).count(),
        2,
        "react, lodash"
    );

    // A cut AFTER the fold: top_k=1 is one row, and top_k=k is the first k
    // folded rows, never fewer while distinct matches remain.
    for k in 1..=4 {
        let top = collapsed(&r, q, k);
        assert_eq!(top.len(), k, "top_k={k}: {top:#?}");
        assert_eq!(json(&top), json(&all[..k]), "top_k={k}");
        assert_eq!(distinct(&top), k, "top_k={k} spends no slot twice");
    }
    // Control: uncollapsed, the two react members take both slots of top_k=2.
    let two = raw(&r, q, 2);
    assert_eq!(distinct(&two), 1, "{two:#?}");
    assert_eq!(
        collapsed(&r, q, 5).len(),
        4,
        "top_k above the matches keeps them all"
    );
}

#[test]
fn heuristic_never_folds() {
    let r = build("identity-vendored-subtree", &["money", "shop"]);
    assert!(
        same_as_edges(&r) > 0,
        "control: the vendored copies are candidates"
    );
    let rows = collapsed(&r, "formatCents", 20);
    assert_eq!(
        rows.len(),
        2,
        "a HEURISTIC class is never one row (Q18): {rows:#?}"
    );
    assert!(rows.iter().all(|f| f.identity.is_none()), "{rows:#?}");
    assert_eq!(json(&rows), json(&raw(&r, "formatCents", 20)));
    assert!(!json(&rows).contains("\"identity\""));
}

#[test]
fn no_class_byte_identical() {
    let r = build("identity-npm-package", &["web"]);
    assert_eq!(same_as_edges(&r), 0, "control: one repo, no class");
    for q in ["react", "package:npm:", "index", "lodash", "zzqqxx"] {
        let mut on = FindOptions::default();
        on.top_k = 0;
        let mut off = on.clone();
        off.collapse = Collapse::off();
        let a = serde_json::to_string(&find_nodes(&r.merged, q, &on)).expect("json");
        let b = serde_json::to_string(&find_nodes(&r.merged, q, &off)).expect("json");
        assert_eq!(a, b, "query {q}");
        assert!(!a.contains("\"identity\""), "{a}");
    }
}

/// pack seeds through the public find but is not a collapsing answer: its
/// seeds stay every matching member.
#[test]
fn pack_keeps_raw_seeds() {
    let r = build("identity-npm-package", &["web", "admin"]);
    let p = pack(&r.merged, "package:npm:react", &PackArgs::default());
    let seeds: Vec<u64> = p
        .nodes
        .iter()
        .filter(|n| n.reason == "seed")
        .map(|n| n.id)
        .collect();
    let members: Vec<u64> = raw(&r, "package:npm:react", 20)
        .iter()
        .map(|f| f.id)
        .collect();
    assert_eq!(seeds, members, "{:#?}", p.nodes);
}
