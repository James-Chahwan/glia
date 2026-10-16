//! DB.2c — the predicate prelude: `engine/src/rules/prelude.dl`, structural
//! predicates over the code domain merged into every program (pruned to what the
//! program reads), with their engine relations built from `CODE_TABLES`, liveness
//! and the module import graph, each only when a rule reads it.
//!
//! The fixture is DB.2a's fx1 (engine/tests/rules_facts.rs, engine/tests/rules_query.rs:
//! three manifest projects web/, shared/, services/api/; web/app.py `page()` calls
//! shared.util.helper and `other()` calls services.api.public.facade.get; helper
//! calls services.api.internal.db.query; facade `get()` calls db.write; one overlay
//! `[[edge]]` page -> db::write CALLS) plus services/api/client.py, whose `fetch()`
//! calls an external HTTP endpoint.

use std::collections::{BTreeSet, HashSet};

use glia_code_domain::profile::CODE_TABLES;
use glia_code_domain::{edge_category, node_kind};
use glia_engine::rules::{QueryAnswer, QueryOptions, QueryRow, QueryValue, query, validate};
use glia_engine::{CoverageNote, Locator, coverage_report, entrypoint_reachable, generate_one};
use glia_graph::MergedGraph;

const WEB_APP: &str = "from shared.util import helper\n\
from services.api.public.facade import get\n\
\n\
\n\
def page():\n    return helper()\n\
\n\
\n\
def other():\n    return get()\n";
const SHARED_UTIL: &str =
    "from services.api.internal.db import query\n\n\ndef helper():\n    return query()\n";
const FACADE: &str =
    "from services.api.internal.db import write\n\n\ndef get():\n    return write()\n";
const DB: &str = "def query():\n    return 1\n\n\ndef write():\n    return 2\n";
const CLIENT: &str = "import requests\n\n\ndef fetch():\n    return requests.get(\"https://api.example.com/v1/items\")\n";
const OVERLAY: &str = "version = 1\n\n[[edge]]\nfrom = \"web::app::page\"\nto = \"services::api::internal::db::write\"\ncategory = \"CALLS\"\n";

const FILES: [(&str, &str); 9] = [
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("shared/pyproject.toml", "[project]\nname = \"shared\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
    ("web/app.py", WEB_APP),
    ("shared/util.py", SHARED_UTIL),
    ("services/api/public/facade.py", FACADE),
    ("services/api/internal/db.py", DB),
    ("services/api/client.py", CLIENT),
    (".glia/overlay.toml", OVERLAY),
];

/// fx1 + client.py, built; the tempdir is returned so it outlives the graph.
fn fx() -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (rel, src) in FILES {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let r = generate_one(tmp.path().to_str().expect("utf-8 temp path")).expect("generate_one");
    (tmp, r.merged)
}

fn run(merged: &MergedGraph, text: &str, o: &QueryOptions) -> QueryAnswer {
    query(merged, text, o).unwrap_or_else(|e| panic!("{}", e.rendered))
}

fn ask(merged: &MergedGraph, text: &str) -> QueryAnswer {
    run(merged, text, &QueryOptions::default())
}

/// A value as compared: a node's qname, a string or an int as text.
fn shown(v: &QueryValue) -> String {
    match v {
        QueryValue::Node { qname, .. } => qname.clone(),
        QueryValue::Str(s) => s.clone(),
        QueryValue::Int(i) => i.to_string(),
        other => panic!("an unexpected value: {other:?}"),
    }
}

fn values(r: &QueryRow) -> Vec<String> {
    r.values.iter().map(shown).collect()
}

/// Every row as (values, tier).
fn rows(a: &QueryAnswer) -> Vec<(Vec<String>, &'static str)> {
    a.results.iter().map(|r| (values(r), r.tier)).collect()
}

/// The notes as compared: their JSON.
fn notes(rows: &[CoverageNote]) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|c| serde_json::to_value(c).expect("a note serialises"))
        .collect()
}

/// The coverage rows of every present language whose category is one of `cats`
/// or `*`: what the query layer's `caveats_for` gives.
fn caveats_for(merged: &MergedGraph, cats: &[&str]) -> Vec<serde_json::Value> {
    let rows: Vec<CoverageNote> = coverage_report(merged)
        .into_iter()
        .filter(|c| c.edge_category == "*" || cats.contains(&c.edge_category))
        .collect();
    notes(&rows)
}

/// The code profile's carry categories, by name.
fn carry_names() -> Vec<&'static str> {
    CODE_TABLES
        .carry_edges
        .iter()
        .map(|c| edge_category::name(*c))
        .collect()
}

#[test]
fn carry_names_the_profile() {
    let (_tmp, merged) = fx();
    let carry = ask(&merged, "x(a, b) :- carry(a, b), in_scope(a, \"web\").");
    let calls = ask(
        &merged,
        "x(a, b) :- edge(a, b, \"CALLS\"), in_scope(a, \"web\").",
    );
    assert_eq!(carry.outputs.len(), 1, "the prelude's `carry` is no output");
    assert_eq!(carry.outputs[0].predicate, "x");
    assert_eq!(rows(&carry).len(), 3, "{:?}", rows(&carry));
    assert_eq!(rows(&carry), rows(&calls));
    assert!(carry.results.iter().all(|r| r.caveats.is_empty()));
}

#[test]
fn effect_classifies_by_the_profile() {
    let (_tmp, merged) = fx();
    let a = ask(&merged, "x(f, c) :- effect(f, _, c).");
    let class = CODE_TABLES
        .effect_sink(node_kind::ENDPOINT, edge_category::CALLS)
        .map(|(_, s)| s.class)
        .expect("an ENDPOINT reached over CALLS is an effect");
    let got: Vec<Vec<String>> = a.results.iter().map(values).collect();
    assert_eq!(
        got,
        [vec![
            "services::api::client::fetch".to_string(),
            class.to_string()
        ]]
    );
    assert_eq!(class, "http_call");
    assert_eq!(a.results[0].file.as_deref(), Some("services/api/client.py"));
    // The sink is the endpoint the call reaches.
    let sink = ask(&merged, "x(s) :- effect(_, s, \"http_call\").");
    let sinks: Vec<&str> = sink
        .results
        .iter()
        .map(|r| match &r.values[0] {
            QueryValue::Node { kind, .. } => *kind,
            other => panic!("not a node: {other:?}"),
        })
        .collect();
    assert_eq!(sinks, ["ENDPOINT"]);
}

#[test]
fn unreached_uses_liveness() {
    let (_tmp, merged) = fx();
    let a = ask(&merged, "x(n) :- unreached(n), in_scope(n, \"shared\").");
    let live = entrypoint_reachable(&merged);
    let loc = Locator::new(&merged);
    let mut seen = HashSet::new();
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if !seen.insert(n.id) || live.contains(&n.id) {
                continue;
            }
            let at = loc.locate(n.id);
            let symbol = matches!(at.kind, "FUNCTION" | "CLASS" | "METHOD");
            let shared = at.file.as_deref().is_some_and(|f| f.starts_with("shared/"));
            if symbol && shared {
                expected.insert(at.qname);
            }
        }
    }
    assert!(
        !expected.is_empty(),
        "fx1's shared helper is reached by no entry"
    );
    let got: BTreeSet<String> = a.results.iter().map(|r| shown(&r.values[0])).collect();
    assert_eq!(got, expected);
    // `!live` reads every carry category: each row carries their caveats.
    let caveats = caveats_for(&merged, &carry_names());
    assert!(!caveats.is_empty());
    for r in &a.results {
        assert_eq!(notes(&r.caveats), caveats, "{:?}", values(r));
    }
    // An empty answer through liveness names the carry categories it read.
    let none = ask(&merged, "x(n) :- unreached(n), in_scope(n, \"nowhere\").");
    let absence = none.absence.expect("nothing sits in `nowhere`");
    let mut want = carry_names();
    let mut got = absence.mechanisms.clone();
    want.sort_unstable();
    got.sort_unstable();
    assert_eq!(got, want);
}

#[test]
fn module_dep_is_lifted() {
    let (_tmp, merged) = fx();
    let a = ask(&merged, "x(a, b) :- module_dep(a, b).");
    let got: BTreeSet<(String, String)> = a
        .results
        .iter()
        .map(|r| (shown(&r.values[0]), shown(&r.values[1])))
        .collect();
    let want: BTreeSet<(String, String)> = [
        ("web::app", "shared::util"),
        ("web::app", "services::api::public::facade"),
        ("shared::util", "services::api::internal::db"),
        (
            "services::api::public::facade",
            "services::api::internal::db",
        ),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(got, want);
    assert_eq!(a.results.len(), want.len());
    for r in &a.results {
        assert_eq!(r.tier, "fact", "a parsed import is read at its site");
        for v in &r.values {
            let QueryValue::Node { kind, .. } = v else {
                panic!("a node")
            };
            assert_eq!(*kind, "MODULE");
        }
    }
    // Under negation the import graph is caveated by IMPORTS.
    let mut o = QueryOptions::default();
    o.outputs = vec!["x".to_string()];
    let neg = run(
        &merged,
        "x(a) :- node(a, \"MODULE\"), !imports_any(a).\nimports_any(a) :- module_dep(a, _).",
        &o,
    );
    assert!(!neg.results.is_empty());
    for r in &neg.results {
        assert_eq!(notes(&r.caveats), caveats_for(&merged, &["IMPORTS"]));
    }
}

#[test]
fn prelude_names_are_reserved() {
    let (_tmp, merged) = fx();
    for (text, name) in [
        ("carry(a, b) :- edge(a, b, \"USES\").", "carry"),
        (
            "unhandled_route(r) :- node(r, \"ROUTE\").",
            "unhandled_route",
        ),
        ("live(n) :- node(n, \"FUNCTION\").", "live"),
    ] {
        let e = query(&merged, text, &QueryOptions::default()).expect_err(text);
        assert_eq!(e.kind, "analysis", "{text}: {}", e.rendered);
        let needle = format!("`{name}` is a prelude");
        let d = e
            .diagnostics
            .iter()
            .find(|d| d.message.contains(&needle))
            .unwrap_or_else(|| panic!("{text}: no `{needle}` in {:?}", e.diagnostics));
        assert!(
            d.message
                .ends_with("(docs/rules.md); name yours differently")
        );
        assert_eq!((d.line, d.column), (1, 1), "{text}");
        assert!(
            e.rendered.starts_with("<query>:1:1: error: "),
            "{}",
            e.rendered
        );
        // validate refuses it the same way, with no graph.
        let v = validate(text);
        assert_eq!(v.len(), e.diagnostics.len(), "{text}");
        assert_eq!(v[0].message, d.message);
    }
    // A name the prelude does not define is the user's.
    assert!(validate("callers(a, b) :- calls(a, b).").is_empty());
}

#[test]
fn prelude_predicates_are_no_default_output() {
    let (_tmp, merged) = fx();
    // fx1 has no route: the answer is empty, and its absence names the
    // category the prelude's `handled` reads.
    let a = ask(&merged, "x(r) :- unhandled_route(r).");
    let names: Vec<&str> = a.outputs.iter().map(|o| o.predicate.as_str()).collect();
    assert_eq!(names, ["x"]);
    let absence = a.absence.expect("no route in fx1");
    assert_eq!(absence.mechanisms, ["HANDLED_BY"]);
    // Asked for by name, a prelude predicate is answered.
    let mut o = QueryOptions::default();
    o.outputs = vec!["calls".to_string()];
    let calls = run(&merged, "", &o);
    let direct = ask(&merged, "x(a, b) :- edge(a, b, \"CALLS\").");
    assert_eq!(calls.outputs[0].predicate, "calls");
    assert_eq!(rows(&calls), rows(&direct));
    assert!(validate("x(a) :- tested(a), unreached(a).").is_empty());
}

#[test]
fn the_prelude_is_documented() {
    let doc = include_str!("../../docs/rules.md");
    let section = doc
        .split("\n## The prelude\n")
        .nth(1)
        .expect("docs/rules.md has a `## The prelude` section");
    let section = section.split("\n## ").next().unwrap_or(section);
    for (name, arity) in glia_engine::rules::prelude_predicates() {
        let at = format!("`{name}(");
        assert!(
            section.contains(&at),
            "docs/rules.md does not list `{name}/{arity}`"
        );
    }
    for name in [
        "dependency_category",
        "symbol_kind",
        "effect_sink",
        "live",
        "module_import",
    ] {
        let at = format!("`{name}(");
        assert!(
            section.contains(&at),
            "docs/rules.md does not list `{name}`"
        );
    }
    // The prelude text is what PRELUDE holds.
    assert!(glia_engine::rules::PRELUDE.contains("carry(A, B) :-"));
    // SECURITY.md states the query surface's rule (Q6).
    let security = include_str!("../../SECURITY.md");
    assert!(security.contains("\n## The query language reads structure only\n"));
}
