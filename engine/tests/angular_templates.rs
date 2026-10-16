//! DT.5a — Angular inline `template:` and `host:` bindings bind the
//! component's own METHODs: a binding that calls a METHOD of its component is
//! a CALLS from the component CLASS (rule `template_call`, `host_call` from a
//! host entry), one that only names it a USES, EVIDENCE
//! `extractor:angular_template` at the binding's row (the lowest row when two
//! bindings name one method). Template locals are scoped: a `@for` loop
//! variable shadows a same-named method in its header (the track expression
//! included) and its block only, and template text binds nothing.
//!
//! The fixture is `bench/substrate-gap/fixtures/angular-inline-template-bindings`,
//! copied to a tempdir. It has no constructor, no receiver call and no
//! decorator argument, so the dead_symbol rows it keeps (`entry`, `reload`,
//! `exportCsv`) hold whatever DT.5c / DT.5d / DT.5g land.

use std::path::Path;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{edge_category, node_kind};
use glia_core::EdgeCategoryId;
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, gaps_report};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/angular-inline-template-bindings";
const FILES: [&str; 2] = ["package.json", "src/app/offers/offer-list.component.ts"];
const CLASS: &str = "src::app::offers::offer-list.component::OfferListComponent";

fn build() -> MergedGraph {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let dir = tempfile::tempdir().expect("tempdir");
    for f in FILES {
        let body = std::fs::read_to_string(src.join(f)).unwrap_or_else(|e| panic!("read {f}: {e}"));
        let path = dir.path().join(f);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }
    generate_one(dir.path().to_str().expect("utf-8 temp path"))
        .unwrap_or_else(|e| panic!("generate_one: {e}"))
        .merged
}

/// Every edge out of the component CLASS of `cat`, as (method simple name,
/// evidence emitter, rule, 0-based line), sorted.
fn class_edges(m: &MergedGraph, cat: EdgeCategoryId) -> Vec<(String, String, String, u32)> {
    let mut out = Vec::new();
    for g in &m.graphs {
        for e in &g.edges {
            if e.category != cat {
                continue;
            }
            let from_is_class = g.nav.qname_by_id.get(&e.from).map(String::as_str) == Some(CLASS)
                && g.nav.kind_by_id.get(&e.from) == Some(&node_kind::CLASS);
            if !from_is_class {
                continue;
            }
            let to = g.nav.qname_by_id.get(&e.to).expect("target qname");
            let ev = Evidence::read(&e.cells).expect("every edge carries EVIDENCE");
            out.push((
                to.rsplit("::").next().unwrap_or(to).to_string(),
                ev.emitter,
                ev.rule.unwrap_or_default(),
                ev.line.expect("located at the binding's row"),
            ));
        }
    }
    out.sort();
    out
}

#[test]
fn template_and_host_bindings_call_the_components_methods() {
    let m = build();
    let calls = class_edges(&m, edge_category::CALLS);
    let at = |name: &str, rule: &str, line: u32| {
        (
            name.to_string(),
            "extractor:angular_template".to_string(),
            rule.to_string(),
            line,
        )
    };
    // 0-based rows: the file's 1-based line minus one.
    assert_eq!(
        calls,
        vec![
            at("canEdit", "template_call", 14),
            at("isBusy", "host_call", 7),
            at("onEscape", "host_call", 7),
            at("refresh", "template_call", 11),
            at("remove", "template_call", 18),
            at("row", "template_call", 12),
            at("summary", "template_call", 15),
            at("trackById", "template_call", 17),
        ]
    );
    // `offers()` is a signal (STATE_VAR), `rows` a plain field: no METHOD, no edge.
    assert!(
        class_edges(&m, edge_category::USES).is_empty(),
        "no USES expected"
    );
}

#[test]
fn shadowed_locals_and_template_text_stay_dead() {
    let m = build();
    let rep = gaps_report(&m, &[], &GapsOptions::default()).expect("known categories");
    let mut dead: Vec<String> = rep
        .rows
        .iter()
        .filter(|r| r.category == DEAD_SYMBOL)
        .map(|r| r.qname.rsplit("::").next().unwrap_or(&r.qname).to_string())
        .collect();
    dead.sort();
    assert_eq!(dead, ["entry", "exportCsv", "reload"]);
    for cat in [edge_category::CALLS, edge_category::USES] {
        let names: Vec<String> = class_edges(&m, cat).into_iter().map(|e| e.0).collect();
        for gone in ["entry", "reload", "exportCsv"] {
            assert!(
                !names.iter().any(|n| n == gone),
                "{gone} must not be bound ({cat:?})"
            );
        }
    }
}

#[test]
fn two_builds_give_identical_template_edges() {
    let a = build();
    let b = build();
    for cat in [edge_category::CALLS, edge_category::USES] {
        assert_eq!(class_edges(&a, cat), class_edges(&b, cat));
    }
}
