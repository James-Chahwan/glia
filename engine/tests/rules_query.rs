//! DB.2b — `glia_engine::rules::{query, validate}`: Datalog text parsed and
//! analysed against the code schema, evaluated over `CodeFacts` within a row
//! budget, rows located through one Locator and sorted by rendered values,
//! tiered, with the Q5 negation caveats, optional witnesses, and an LD.8a
//! absence when nothing matches.
//!
//! The fixture is DB.2a's fx1 (engine/tests/rules_facts.rs): three manifest
//! projects (web/, shared/, services/api/); web/app.py `page()` calls
//! shared.util.helper and `other()` calls services.api.public.facade.get;
//! shared/util.py `helper()` calls services.api.internal.db.query; facade
//! `get()` calls db.write; plus one overlay `[[edge]]` web::app::page ->
//! services::api::internal::db::write CALLS (heuristic).

use glia_engine::rules::{
    QueryAnswer, QueryOptions, QueryRow, QueryValue, RuleError, query, validate,
};
use glia_engine::{CoverageNote, Locator, coverage_report, generate_one};
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
const OVERLAY: &str = "version = 1\n\n[[edge]]\nfrom = \"web::app::page\"\nto = \"services::api::internal::db::write\"\ncategory = \"CALLS\"\n";

const FILES: [(&str, &str); 8] = [
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("shared/pyproject.toml", "[project]\nname = \"shared\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
    ("web/app.py", WEB_APP),
    ("shared/util.py", SHARED_UTIL),
    ("services/api/public/facade.py", FACADE),
    ("services/api/internal/db.py", DB),
    (".glia/overlay.toml", OVERLAY),
];

/// The case-1 program: web's CALLS, both ends rendered.
const WEB_CALLS: &str = "x(a, b) :- edge(a, b, \"CALLS\"), in_scope(a, \"web\").";

/// One repo of `files`, built; the tempdir is returned so it outlives the graph.
fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (rel, src) in files {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let r = generate_one(tmp.path().to_str().expect("utf-8 temp path")).expect("generate_one");
    (tmp, r.merged)
}

fn fx1() -> (tempfile::TempDir, MergedGraph) {
    build(&FILES)
}

fn opts() -> QueryOptions {
    QueryOptions::default()
}

fn run(merged: &MergedGraph, text: &str, o: &QueryOptions) -> QueryAnswer {
    query(merged, text, o).unwrap_or_else(|e| panic!("{}", e.rendered))
}

fn fail(merged: &MergedGraph, text: &str) -> RuleError {
    match query(merged, text, &opts()) {
        Ok(a) => panic!("expected an error, got {} rows", a.results.len()),
        Err(e) => e,
    }
}

/// A Node value's qname; panics on any other value.
fn qname(v: &QueryValue) -> &str {
    match v {
        QueryValue::Node { qname, .. } => qname,
        other => panic!("not a node: {other:?}"),
    }
}

fn qnames(row: &QueryRow) -> Vec<&str> {
    row.values.iter().map(qname).collect()
}

/// The node of `merged` whose qname is `q`.
fn node(merged: &MergedGraph, q: &str) -> glia_core::NodeId {
    merged
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.iter())
        .filter(|(_, qn)| qn.as_str() == q)
        .map(|(id, _)| *id)
        .min_by_key(|id| id.0)
        .unwrap_or_else(|| panic!("no node {q}"))
}

/// A caveat row as compared: its JSON.
fn notes(rows: &[CoverageNote]) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|c| serde_json::to_value(c).expect("a note serialises"))
        .collect()
}

/// `caveats_for(merged, cats, None)`, from the public table: the rows of every
/// present language whose category is one of `cats` or `*`.
fn caveats_for(merged: &MergedGraph, cats: &[&str]) -> Vec<serde_json::Value> {
    let rows: Vec<CoverageNote> = coverage_report(merged)
        .into_iter()
        .filter(|c| c.edge_category == "*" || cats.contains(&c.edge_category))
        .collect();
    notes(&rows)
}

#[test]
fn rows_are_located_sorted_and_tiered() {
    let (_tmp, merged) = fx1();
    let a = run(&merged, WEB_CALLS, &opts());
    assert_eq!(a.language, "datalog");
    assert_eq!(a.outputs.len(), 1);
    let out = &a.outputs[0];
    assert_eq!(
        (
            out.predicate.as_str(),
            out.arity,
            &out.columns,
            out.total,
            out.shown
        ),
        ("x", 2, &vec!["a".to_string(), "b".to_string()], 3, 3)
    );
    let got: Vec<(Vec<&str>, &str)> = a.results.iter().map(|r| (qnames(r), r.tier)).collect();
    assert_eq!(
        got,
        [
            (
                vec!["web::app::other", "services::api::public::facade::get"],
                "fact"
            ),
            (
                vec!["web::app::page", "services::api::internal::db::write"],
                "heuristic"
            ),
            (vec!["web::app::page", "shared::util::helper"], "fact"),
        ]
    );
    let loc = Locator::new(&merged);
    for r in &a.results {
        assert_eq!(r.predicate, "x");
        assert_eq!(r.file.as_deref(), Some("web/app.py"));
        let first = loc.locate(node(&merged, qname(&r.values[0])));
        assert_eq!(r.line, first.line, "{:?}", r.values);
        assert!(r.line.is_some());
        // Every node value is located through the same Locator.
        for v in &r.values {
            let QueryValue::Node {
                qname,
                kind,
                file,
                line,
                ..
            } = v
            else {
                panic!("a node")
            };
            let at = loc.locate(node(&merged, qname));
            assert_eq!(
                (*kind, file, line),
                (at.kind, &at.file, &at.line),
                "{qname}"
            );
            assert_eq!(*kind, "FUNCTION");
        }
        assert!(r.caveats.is_empty() && r.witness.is_empty());
    }
    assert!(a.absence.is_none());

    // `limit` cuts after the sort; `total` stays honest.
    let mut cut = opts();
    cut.limit = Some(1);
    let c = run(&merged, WEB_CALLS, &cut);
    assert_eq!((c.outputs[0].total, c.outputs[0].shown), (3, 1));
    assert_eq!(qnames(&c.results[0]), qnames(&a.results[0]));
}

#[test]
fn negation_carries_caveats() {
    let (_tmp, merged) = fx1();
    let text = "uncalled(f) :- node(f, \"FUNCTION\"), !called(f).\n\
                called(f) :- edge(_, f, \"CALLS\").";
    let mut o = opts();
    o.outputs = vec!["uncalled".to_string()];
    let a = run(&merged, text, &o);
    assert_eq!(a.outputs.len(), 1, "`called` is not returned");
    assert_eq!(a.outputs[0].predicate, "uncalled");
    let got: Vec<&str> = a.results.iter().map(|r| qname(&r.values[0])).collect();
    assert_eq!(got, ["web::app::other", "web::app::page"]);
    let expected = caveats_for(&merged, &["CALLS"]);
    assert!(!expected.is_empty(), "the `*` CALLS row applies to fx1");
    for r in &a.results {
        assert_eq!(r.tier, "fact");
        assert_eq!(notes(&r.caveats), expected);
    }

    // A positive read of the same categories carries no caveat (Q5: only a
    // negation adds them), and the default outputs are the program's sinks.
    let both = run(&merged, text, &opts());
    assert_eq!(both.outputs.len(), 1);
    assert_eq!(both.outputs[0].predicate, "uncalled");
    let pos = run(&merged, "called(f) :- edge(_, f, \"CALLS\").", &opts());
    assert!(pos.results.iter().all(|r| r.caveats.is_empty()));

    // Naming both outputs returns both, in the order asked.
    let mut two = opts();
    two.outputs = vec!["called".to_string(), "uncalled/1".to_string()];
    let t = run(&merged, text, &two);
    let preds: Vec<&str> = t.outputs.iter().map(|o| o.predicate.as_str()).collect();
    assert_eq!(preds, ["called", "uncalled"]);
    assert_eq!(
        t.results.first().map(|r| r.predicate.as_str()),
        Some("called")
    );
    assert_eq!(
        t.results.last().map(|r| r.predicate.as_str()),
        Some("uncalled")
    );
    // The default sinks come in first-definition order, not by name.
    let sinks = run(
        &merged,
        "zz(x) :- node(x, \"FUNCTION\").\naa(x) :- node(x, \"MODULE\").",
        &opts(),
    );
    let preds: Vec<&str> = sinks.outputs.iter().map(|o| o.predicate.as_str()).collect();
    assert_eq!(preds, ["zz", "aa"]);

    // A negation over a category read through a variable caveats every category.
    let mut o = opts();
    o.outputs = vec!["v".to_string()];
    let any = run(
        &merged,
        "v(f) :- node(f, \"FUNCTION\"), !e(f).\ne(f) :- edge(f, f, k), category(k).",
        &o,
    );
    let every: Vec<&str> = glia_code_domain::edge_category::ALL
        .iter()
        .map(|(_, n)| *n)
        .collect();
    assert!(!any.results.is_empty());
    for r in &any.results {
        assert_eq!(notes(&r.caveats), caveats_for(&merged, &every));
    }
}

#[test]
fn unknown_names_are_analysis_errors() {
    let (_tmp, merged) = fx1();
    let cases: [(&str, &str); 6] = [
        ("x(a) :- node(a, \"ROUTEE\").", "unknown node kind `ROUTEE`"),
        (
            "x(a, b) :- edge(a, b, \"CALL\").",
            "unknown edge category `CALL`",
        ),
        ("x(a) :- nope(a).", "nope/1"),
        (
            "edge(a, b) :- node(a, _), node(b, _).",
            "`edge/2` is a built-in predicate and cannot be defined by a rule",
        ),
        (
            "qname(a, q) :- node(a, q).",
            "`qname/2` is a built-in predicate and cannot be defined by a rule",
        ),
        (
            "in_scope(a, s) :- node(a, s).",
            "`in_scope/2` is a built-in predicate and cannot be defined by a rule",
        ),
    ];
    for (text, needle) in cases {
        let e = fail(&merged, text);
        assert_eq!(e.kind, "analysis", "{text}: {}", e.rendered);
        let d = e
            .diagnostics
            .iter()
            .find(|d| d.message.contains(needle))
            .unwrap_or_else(|| panic!("{text}: no `{needle}` in {:?}", e.diagnostics));
        assert_eq!(d.line, 1, "{text}");
        assert!(d.column >= 1, "{text}");
        assert!(e.rendered.starts_with("<query>:1:"), "{}", e.rendered);
        assert!(e.rendered.contains(needle), "{}", e.rendered);
        // validate reads the same schema with no graph.
        let v = validate(text);
        let same: Vec<(u32, u32, &str)> = e
            .diagnostics
            .iter()
            .map(|d| (d.line, d.column, d.message.as_str()))
            .collect();
        let theirs: Vec<(u32, u32, &str)> = v
            .iter()
            .map(|d| (d.line, d.column, d.message.as_str()))
            .collect();
        assert_eq!(same, theirs, "{text}");
    }
    // A suggestion rides DA.5b's wording.
    let e = fail(&merged, "x(a) :- node(a, \"ROUTEE\").");
    assert!(
        e.diagnostics[0].message.ends_with("did you mean `ROUTE`?"),
        "{:?}",
        e.diagnostics
    );
    // A diagnostic on a later line points there.
    let e = fail(
        &merged,
        "x(a) :- node(a, \"ROUTE\").\ny(a) :- node(a, \"ROUTEE\").",
    );
    assert_eq!(e.diagnostics[0].line, 2);

    // A parse error is kind parse; an output no rule defines is analysis.
    let e = fail(&merged, "x(a) :- node(a, \"ROUTE\"");
    assert_eq!(e.kind, "parse", "{}", e.rendered);
    assert!(!e.diagnostics.is_empty() && !validate("x(a) :- node(a").is_empty());
    let mut o = opts();
    o.outputs = vec!["missing".to_string()];
    let e = query(&merged, "x(a) :- node(a, \"ROUTE\").", &o).expect_err("an unknown output");
    assert_eq!(e.kind, "analysis");
    assert!(
        e.rendered.contains("unknown output `missing`"),
        "{}",
        e.rendered
    );

    // Valid programs validate clean, warnings aside.
    assert!(validate(WEB_CALLS).is_empty());
    assert!(validate("x(a) :- node(a, \"ROUTE\"), qname(a, q).").is_empty());
}

#[test]
fn empty_is_an_absence() {
    let (_tmp, merged) = fx1();
    let text = "x(a) :- node(a, \"ROUTE\").";
    let a = run(&merged, text, &opts());
    assert!(a.results.is_empty());
    assert_eq!((a.outputs[0].total, a.outputs[0].shown), (0, 0));
    let absence = a.absence.expect("an empty answer is an absence");
    assert_eq!((absence.reason, absence.tier), ("no_match", "FACT"));
    assert_eq!(absence.note, "no row of x/1 in this graph");
    assert_eq!(absence.query, text);
    assert!(absence.mechanisms.is_empty(), "node/2 reads no edge");

    // The mechanisms are the categories the outputs' rules read, positive or
    // negated, and the caveats theirs.
    let text = "y(a) :- node(a, \"ROUTE\"), edge(a, _, \"HANDLED_BY\").\n\
                z(a) :- node(a, \"ROUTE\"), !edge(_, a, \"NAVIGATES_TO\").";
    let a = run(&merged, text, &opts());
    let absence = a.absence.expect("no route in fx1");
    assert_eq!(absence.mechanisms, ["HANDLED_BY", "NAVIGATES_TO"]);
    assert_eq!(absence.note, "no row of y/1, z/1 in this graph");
    assert_eq!(
        notes(&absence.caveats),
        caveats_for(&merged, &["HANDLED_BY", "NAVIGATES_TO"])
    );
}

#[test]
fn explain_locates_the_witness() {
    let (_tmp, merged) = fx1();
    let mut o = opts();
    o.explain = true;
    let a = run(&merged, WEB_CALLS, &o);
    assert_eq!(a.results.len(), 3);
    for r in &a.results {
        assert_eq!(r.witness.len(), 1, "{:?}", r.values);
        let w = &r.witness[0];
        let names = qnames(r);
        assert_eq!(
            (
                w.from_qname.as_str(),
                w.to_qname.as_str(),
                w.category,
                w.tier
            ),
            (names[0], names[1], "CALLS", r.tier)
        );
    }
    // The parsed calls are located at their call sites in web/app.py; the
    // overlay edge at its stanza, heuristic, its note naming the stanza.
    let site = |i: usize| {
        let w = &a.results[i].witness[0];
        (w.file.as_deref(), w.line)
    };
    assert_eq!(site(0), (Some("web/app.py"), Some(10)));
    assert_eq!(site(2), (Some("web/app.py"), Some(6)));
    let overlay = &a.results[1].witness[0];
    assert_eq!(
        (overlay.file.as_deref(), overlay.line, overlay.tier),
        (Some(".glia/overlay.toml"), Some(3), "heuristic")
    );
    let note = overlay.note.as_deref().unwrap_or_default();
    assert!(note.contains("overlay.toml"), "{note}");

    // Without explain no witness is built.
    let plain = run(&merged, WEB_CALLS, &opts());
    assert!(plain.results.iter().all(|r| r.witness.is_empty()));
}

#[test]
fn budget_stops_unseeded_recursion() {
    let mut chain = String::new();
    for i in 0..40 {
        if i + 1 < 40 {
            chain.push_str(&format!("def f{i}():\n    return f{}()\n\n\n", i + 1));
        } else {
            chain.push_str(&format!("def f{i}():\n    return 0\n"));
        }
    }
    let (_tmp, merged) = build(&[("chain.py", chain.as_str())]);
    let text = "r(a, b) :- edge(a, b, \"CALLS\").\nr(a, c) :- r(a, b), edge(b, c, \"CALLS\").";
    let mut o = opts();
    o.row_budget = Some(100);
    let e = query(&merged, text, &o).expect_err("the unseeded closure passes 100 rows");
    assert_eq!(e.kind, "budget", "{}", e.rendered);
    assert!(e.rendered.contains("`r/2`"), "{}", e.rendered);
    assert!(
        e.rendered.contains("QueryOptions::row_budget"),
        "the engine surface's hint: {}",
        e.rendered
    );
    let mut cli = o.clone();
    cli.surface = "cli";
    let e = query(&merged, text, &cli).expect_err("the same stop");
    assert!(e.rendered.contains("--max-rows"), "{}", e.rendered);
    // Under the default budget the 780-row closure is answered whole.
    let a = run(&merged, text, &opts());
    assert_eq!(a.outputs[0].total, 40 * 39 / 2);
}

#[test]
fn deterministic() {
    let (_t1, m1) = fx1();
    let (_t2, m2) = fx1();
    let mut o = opts();
    o.explain = true;
    let a = serde_json::to_string(&run(&m1, WEB_CALLS, &o)).expect("serialises");
    let b = serde_json::to_string(&run(&m2, WEB_CALLS, &o)).expect("serialises");
    assert_eq!(a, b);
    // The JSON shape a surface prints.
    assert!(
        a.starts_with("{\"language\":\"datalog\",\"outputs\":["),
        "{a}"
    );
    let v: serde_json::Value = serde_json::from_str(&a).expect("json");
    let mut keys: Vec<&str> = v
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["absence", "language", "outputs", "results"]);
    assert_eq!(
        v["results"][0]["values"][0]["node"]["qname"],
        "web::app::other"
    );
}
