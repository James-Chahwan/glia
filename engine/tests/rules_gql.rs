//! DB.7c — `glia_engine::rules::query_gql`: a GQL read query compiled by
//! `glia_rules::gql::compile`, evaluated over the code graph with the prelude, its
//! rows rendered and tiered as Datalog's, then ORDER BY (by rendered values), LIMIT
//! and the path column applied. A row of properties is located at the first
//! property's node, rows equal in every shown column staying one row (DT.8b).
//!
//! The fixture is DB.2a's fx1 (engine/tests/rules_facts.rs, engine/tests/rules_query.rs):
//! three manifest projects (web/, shared/, services/api/); web/app.py `page()` calls
//! shared.util.helper and `other()` calls services.api.public.facade.get;
//! shared/util.py `helper()` calls services.api.internal.db.query; facade `get()`
//! calls db.write; plus one overlay `[[edge]]` web::app::page ->
//! services::api::internal::db::write CALLS (heuristic).

use glia_engine::rules::{
    QueryAnswer, QueryOptions, QueryRow, QueryValue, RuleError, query, query_gql,
};
use glia_engine::{Locator, generate_one};
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

/// Case 1: web's FUNCTION -> FUNCTION calls, both ends' qnames.
const WEB_CALLS: &str = "MATCH (a:FUNCTION)-[:CALLS]->(b:FUNCTION) \
                         WHERE a.qname STARTS WITH 'web::' \
                         RETURN a.qname, b.qname ORDER BY a.qname";

/// Case 6: the FACT path from page to db::query.
const PATH: &str = "MATCH p = ANY SHORTEST (a {qname: 'web::app::page'})-[:CALLS*]->\
                    (b {qname: 'services::api::internal::db::query'}) RETURN p";

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

fn gql(merged: &MergedGraph, text: &str, o: &QueryOptions) -> QueryAnswer {
    query_gql(merged, text, o).unwrap_or_else(|e| panic!("{}", e.rendered))
}

fn fail(merged: &MergedGraph, text: &str) -> RuleError {
    match query_gql(merged, text, &opts()) {
        Ok(a) => panic!("expected an error, got {} rows", a.results.len()),
        Err(e) => e,
    }
}

/// A value as text: a node's qname, a string as is, an int in decimal.
fn text(v: &QueryValue) -> String {
    match v {
        QueryValue::Node { qname, .. } => qname.clone(),
        QueryValue::Str(s) => s.clone(),
        QueryValue::Int(i) => i.to_string(),
        other => panic!("an unknown value {other:?}"),
    }
}

fn texts(row: &QueryRow) -> Vec<String> {
    row.values.iter().map(text).collect()
}

/// Every row's values as text, with its tier.
fn rows(a: &QueryAnswer) -> Vec<(Vec<String>, &'static str)> {
    a.results.iter().map(|r| (texts(r), r.tier)).collect()
}

fn strs(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

/// Every row's `(file, line)`.
fn located(a: &QueryAnswer) -> Vec<(Option<String>, Option<i64>)> {
    a.results.iter().map(|r| (r.file.clone(), r.line)).collect()
}

/// A row as [`shown`] renders it: (values, tier, file, line).
type ShownRow = (Vec<String>, &'static str, Option<String>, Option<i64>);

/// Every row's values as text, tier and location: what a GQL property row and its
/// Datalog twin (node values, rendered as qnames) must agree on.
fn shown(a: &QueryAnswer) -> Vec<ShownRow> {
    a.results
        .iter()
        .map(|r| (texts(r), r.tier, r.file.clone(), r.line))
        .collect()
}

#[test]
fn match_where_return_order() {
    let (_tmp, merged) = fx1();
    let a = gql(&merged, WEB_CALLS, &opts());
    assert_eq!(a.language, "gql");
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
        ("result", 2, &strs(&["a.qname", "b.qname"]), 3, 3)
    );
    assert_eq!(
        rows(&a),
        [
            (
                strs(&["web::app::other", "services::api::public::facade::get"]),
                "fact"
            ),
            (
                strs(&["web::app::page", "services::api::internal::db::write"]),
                "heuristic"
            ),
            (strs(&["web::app::page", "shared::util::helper"]), "fact"),
        ]
    );
    for r in &a.results {
        assert_eq!(r.predicate, "result");
        assert!(
            matches!(r.values[0], QueryValue::Str(_)),
            "an attribute is a string"
        );
        assert!(r.caveats.is_empty() && r.witness.is_empty());
    }
    // No node value, but every item is a property: the row is located at `a`, the
    // first property's node (DT.8b): other() at web/app.py:9, page() at :5.
    assert_eq!(
        located(&a),
        [
            (Some("web/app.py".to_string()), Some(9)),
            (Some("web/app.py".to_string()), Some(5)),
            (Some("web/app.py".to_string()), Some(5)),
        ]
    );
    assert!(a.absence.is_none());

    // DESC flips the key; the default rendered order still breaks ties.
    let desc = gql(
        &merged,
        "MATCH (a:FUNCTION)-[:CALLS]->(b:FUNCTION) WHERE a.qname STARTS WITH 'web::' \
         RETURN a.qname, b.qname ORDER BY a.qname DESC",
        &opts(),
    );
    let got: Vec<Vec<String>> = desc.results.iter().map(texts).collect();
    assert_eq!(
        got,
        [
            strs(&["web::app::page", "services::api::internal::db::write"]),
            strs(&["web::app::page", "shared::util::helper"]),
            strs(&["web::app::other", "services::api::public::facade::get"]),
        ]
    );
    // An ORDER BY over a later column reorders the rows by that column alone.
    let by_b = gql(
        &merged,
        "MATCH (a:FUNCTION)-[:CALLS]->(b:FUNCTION) WHERE a.qname STARTS WITH 'web::' \
         RETURN a.qname AS caller, b.qname AS callee ORDER BY callee DESC",
        &opts(),
    );
    assert_eq!(by_b.outputs[0].columns, strs(&["caller", "callee"]));
    let callees: Vec<String> = by_b.results.iter().map(|r| text(&r.values[1])).collect();
    assert_eq!(
        callees,
        [
            "shared::util::helper",
            "services::api::public::facade::get",
            "services::api::internal::db::write",
        ]
    );

    // A node value renders located, as Datalog's do.
    let nodes = gql(
        &merged,
        "MATCH (a:FUNCTION)-[:CALLS]->(b:FUNCTION) WHERE a.qname STARTS WITH 'web::' \
         RETURN a, b ORDER BY a",
        &opts(),
    );
    let loc = Locator::new(&merged);
    assert_eq!(nodes.results.len(), 3);
    for r in &nodes.results {
        let QueryValue::Node {
            qname,
            kind,
            file,
            line,
            ..
        } = &r.values[0]
        else {
            panic!("a node: {:?}", r.values)
        };
        assert_eq!((*kind, file.as_deref()), ("FUNCTION", Some("web/app.py")));
        assert_eq!((r.file.as_ref(), r.line), (file.as_ref(), *line), "{qname}");
        assert!(line.is_some());
        let _ = &loc;
    }
}

#[test]
fn datalog_parity() {
    let (_tmp, merged) = fx1();
    let g = gql(&merged, WEB_CALLS, &opts());
    let d = query(
        &merged,
        "result(x, y) :- edge(a, b, \"CALLS\"), node(a, \"FUNCTION\"), node(b, \"FUNCTION\"), \
         qname(a, x), qname(b, y), starts_with(x, \"web::\").",
        &opts(),
    )
    .unwrap_or_else(|e| panic!("{}", e.rendered));
    assert_eq!(d.language, "datalog");
    assert_eq!(rows(&g), rows(&d));
    assert_eq!(g.results.len(), 3);
    assert_eq!(g.outputs[0].total, d.outputs[0].total);
}

/// DT.8b: a GQL row of properties is located where the Datalog form that returns the
/// first property's node locates it: `RETURN a.qname, b.qname` as `x(a, b)`, `RETURN
/// b.qname, a.qname` as `x(b, a)`; values, tiers and order equal too.
#[test]
fn property_rows_are_located_like_datalog() {
    let (_tmp, merged) = fx1();
    let twin = |head: &str| {
        let text = format!(
            "x({head}) :- edge(a, b, \"CALLS\"), node(a, \"FUNCTION\"), node(b, \"FUNCTION\"), \
             qname(a, q), starts_with(q, \"web::\")."
        );
        query(&merged, &text, &opts()).unwrap_or_else(|e| panic!("{}", e.rendered))
    };
    let g = gql(&merged, WEB_CALLS, &opts());
    let d = twin("a, b");
    assert_eq!(shown(&g), shown(&d));
    assert_eq!(g.results.len(), 3);

    // The anchor is the FIRST property's node, whichever pattern end it reads.
    let g = gql(
        &merged,
        "MATCH (a:FUNCTION)-[:CALLS]->(b:FUNCTION) WHERE a.qname STARTS WITH 'web::' \
         RETURN b.qname, a.qname",
        &opts(),
    );
    let d = twin("b, a");
    assert_eq!(shown(&g), shown(&d));
    let files: Vec<Option<&str>> = g.results.iter().map(|r| r.file.as_deref()).collect();
    assert_eq!(
        files,
        [
            Some("services/api/internal/db.py"),
            Some("services/api/public/facade.py"),
            Some("shared/util.py"),
        ]
    );

    // A RETURN that already holds a node keeps Datalog's rule, its first node value:
    // `a` here, not the property's `b`.
    let n = gql(
        &merged,
        "MATCH (a:FUNCTION)-[:CALLS]->(b:FUNCTION) WHERE a.qname STARTS WITH 'web::' \
         RETURN b.qname, a",
        &opts(),
    );
    let files: Vec<Option<&str>> = n.results.iter().map(|r| r.file.as_deref()).collect();
    assert_eq!(files, [Some("web/app.py"); 3]);
}

/// DT.8b: rows stay DISTINCT on the shown columns. Two nodes with one shown value
/// fold back into one row, its tier the strongest of theirs and its location the
/// first `(file, line)` among the strongest.
#[test]
fn equal_property_rows_stay_one_row() {
    let (_tmp, merged) = build(&[
        (
            "alpha/x.py",
            "def run():\n    return 0\n\n\ndef other():\n    return 0\n",
        ),
        (
            "beta/y.py",
            "def helper():\n    return 1\n\n\ndef run():\n    return helper()\n",
        ),
        (
            "gamma/z.py",
            "class B:\n    def go(self):\n        return 1\n\n\n\
             class A:\n    def go(self):\n        return 2\n",
        ),
        (
            ".glia/overlay.toml",
            "version = 1\n\n[[edge]]\nfrom = \"alpha::x::run\"\nto = \"alpha::x::other\"\n\
             category = \"CALLS\"\n",
        ),
    ]);
    let one = |a: &QueryAnswer| (a.outputs[0].total, a.outputs[0].shown, a.results.len());

    // alpha::x::run and beta::y::run: one row, at the earlier file.
    let run = gql(
        &merged,
        "MATCH (f:FUNCTION) WHERE f.name = 'run' RETURN f.name",
        &opts(),
    );
    assert_eq!(one(&run), (1, 1, 1));
    assert_eq!(
        shown(&run),
        [(
            strs(&["run"]),
            "fact",
            Some("alpha/x.py".to_string()),
            Some(1)
        )]
    );

    // gamma::z::A::go sorts first by qname, but B.go is the earlier line.
    let go = gql(&merged, "MATCH (m:METHOD) RETURN m.name", &opts());
    assert_eq!(one(&go), (1, 1, 1));
    assert_eq!(
        shown(&go),
        [(
            strs(&["go"]),
            "fact",
            Some("gamma/z.py".to_string()),
            Some(2)
        )]
    );

    // alpha's run calls only through the overlay (heuristic), beta's through a parsed
    // call (fact): the row is FACT, as the unanchored answer's, and located at the
    // FACT caller although alpha/x.py is the earlier file.
    let callers = gql(
        &merged,
        "MATCH (f:FUNCTION)-[:CALLS]->(g:FUNCTION) RETURN f.name",
        &opts(),
    );
    assert_eq!(one(&callers), (1, 1, 1));
    assert_eq!(
        shown(&callers),
        [(
            strs(&["run"]),
            "fact",
            Some("beta/y.py".to_string()),
            Some(5)
        )]
    );
    let d = query(
        &merged,
        "x(n) :- edge(f, g, \"CALLS\"), node(f, \"FUNCTION\"), node(g, \"FUNCTION\"), \
         name(f, n).",
        &opts(),
    )
    .unwrap_or_else(|e| panic!("{}", e.rendered));
    assert_eq!(
        rows(&callers),
        rows(&d),
        "the shown values and tier are Datalog's"
    );
}

#[test]
fn reachability_across_scopes() {
    let (_tmp, merged) = fx1();
    let a = gql(
        &merged,
        "MATCH (a)-[:CALLS*]->(b) WHERE in_scope(a, 'web') AND in_scope(b, 'services/api') \
         RETURN DISTINCT b.qname ORDER BY b.qname",
        &opts(),
    );
    assert_eq!(a.outputs[0].columns, strs(&["b.qname"]));
    let got: Vec<String> = a.results.iter().map(|r| text(&r.values[0])).collect();
    assert_eq!(
        got,
        [
            "services::api::internal::db::query",
            "services::api::internal::db::write",
            "services::api::public::facade::get",
        ]
    );
    // db::write is reached by FACT calls (other -> get -> write) as well as the
    // overlay edge, so every row is FACT: a reaches row takes its best path's tier.
    assert!(a.results.iter().all(|r| r.tier == "fact"), "{:?}", rows(&a));
}

#[test]
fn aggregate_and_limit() {
    let (_tmp, merged) = fx1();
    let text_ = "MATCH (m:MODULE)-[:IMPORTS]->(n) RETURN m.qname AS module, count(n) AS imports \
                 ORDER BY imports DESC, module LIMIT 2";
    let a = gql(&merged, text_, &opts());
    let out = &a.outputs[0];
    assert_eq!(out.columns, strs(&["module", "imports"]));
    assert_eq!((out.arity, out.total, out.shown), (2, 3, 2));
    let got: Vec<Vec<String>> = a.results.iter().map(texts).collect();
    assert_eq!(
        got,
        [
            strs(&["web::app", "2"]),
            strs(&["services::api::public::facade", "1"]),
        ]
    );
    assert!(matches!(a.results[0].values[1], QueryValue::Int(2)));
    // A row of an aggregate is a group: not located (DT.8b anchors no aggregate).
    assert!(
        located(&a).iter().all(|l| *l == (None, None)),
        "{:?}",
        located(&a)
    );

    // The caller's limit cuts after the query's own; total stays the uncut count.
    let mut one = opts();
    one.limit = Some(1);
    let b = gql(&merged, text_, &one);
    assert_eq!((b.outputs[0].total, b.outputs[0].shown), (3, 1));
    assert_eq!(texts(&b.results[0]), strs(&["web::app", "2"]));
    // Without LIMIT the third group shows: shared::util imports one name.
    let whole = gql(
        &merged,
        "MATCH (m:MODULE)-[:IMPORTS]->(n) RETURN m.qname AS module, count(n) AS imports \
         ORDER BY imports DESC, module",
        &opts(),
    );
    let got: Vec<Vec<String>> = whole.results.iter().map(texts).collect();
    assert_eq!(
        got,
        [
            strs(&["web::app", "2"]),
            strs(&["services::api::public::facade", "1"]),
            strs(&["shared::util", "1"]),
        ]
    );
    // A caller's limit above the query's does not lift it.
    let mut ten = opts();
    ten.limit = Some(10);
    let c = gql(&merged, text_, &ten);
    assert_eq!((c.outputs[0].total, c.outputs[0].shown), (3, 2));
    // A limit of 0 shows no row and carries no absence: the total says why.
    let mut zero = opts();
    zero.limit = Some(0);
    let z = gql(&merged, text_, &zero);
    assert_eq!((z.outputs[0].total, z.outputs[0].shown), (3, 0));
    assert!(z.results.is_empty() && z.absence.is_none());
}

#[test]
fn not_pattern_has_caveats() {
    let (_tmp, merged) = fx1();
    let a = gql(
        &merged,
        "MATCH (f:FUNCTION) WHERE NOT ()-[:CALLS]->(f) RETURN f.qname",
        &opts(),
    );
    let got: Vec<String> = a.results.iter().map(|r| text(&r.values[0])).collect();
    assert_eq!(got, ["web::app::other", "web::app::page"]);
    for r in &a.results {
        assert_eq!(r.tier, "fact");
        assert!(!r.caveats.is_empty(), "a NOT pattern reads an absence");
        assert!(
            r.caveats
                .iter()
                .all(|c| c.edge_category == "CALLS" || c.edge_category == "*"),
            "{:?}",
            r.caveats
        );
    }

    // An empty answer is an absence naming what it read.
    let e = gql(
        &merged,
        "MATCH (r:ROUTE)-[:HANDLED_BY]->(f) RETURN r.qname",
        &opts(),
    );
    assert!(e.results.is_empty());
    let absence = e.absence.expect("no route in fx1");
    assert_eq!((absence.reason, absence.tier), ("no_match", "FACT"));
    assert_eq!(absence.note, "no row of result/1 in this graph");
    assert_eq!(absence.mechanisms, ["HANDLED_BY"]);

    // A function call may name an engine relation of the prelude: `live/1`, whose
    // negation carries the caveats of every carry category it is computed from.
    let dead = gql(
        &merged,
        "MATCH (f:FUNCTION) WHERE NOT live(f) RETURN f.qname",
        &opts(),
    );
    let live = gql(
        &merged,
        "MATCH (f:FUNCTION) WHERE live(f) RETURN f.qname",
        &opts(),
    );
    let functions = gql(&merged, "MATCH (f:FUNCTION) RETURN f.qname", &opts());
    // fx1 has no entry point, so no function is live.
    assert_eq!(functions.results.len(), 6);
    assert_eq!(rows(&dead), rows(&functions));
    assert!(dead.results.iter().all(|r| !r.caveats.is_empty()));
    assert!(live.results.is_empty() && live.absence.is_some());
}

#[test]
fn path_column() {
    let (_tmp, merged) = fx1();
    let a = gql(&merged, PATH, &opts());
    let out = &a.outputs[0];
    assert_eq!(
        (out.columns.clone(), out.arity, out.total, out.shown),
        (strs(&["p"]), 1, 1, 1)
    );
    assert_eq!(a.results.len(), 1);
    let r = &a.results[0];
    assert_eq!(
        r.values,
        [QueryValue::Str(
            "web::app::page -CALLS-> shared::util::helper -CALLS-> \
             services::api::internal::db::query"
                .to_string()
        )]
    );
    assert_eq!(r.tier, "fact", "the FACT path, not the overlay edge");
    // The witness rows stay, without `explain`: the path's two hops, in order.
    let hops: Vec<(&str, &str, &str, &str)> = r
        .witness
        .iter()
        .map(|w| {
            (
                w.from_qname.as_str(),
                w.category,
                w.to_qname.as_str(),
                w.tier,
            )
        })
        .collect();
    assert_eq!(
        hops,
        [
            ("web::app::page", "CALLS", "shared::util::helper", "fact"),
            (
                "shared::util::helper",
                "CALLS",
                "services::api::internal::db::query",
                "fact"
            ),
        ]
    );
    // A path row is located at its start node.
    let loc = Locator::new(&merged);
    let start = merged
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.iter())
        .filter(|(_, q)| q.as_str() == "web::app::page")
        .map(|(id, _)| *id)
        .min_by_key(|id| id.0)
        .expect("page");
    let at = loc.locate(start);
    assert_eq!((r.file.as_deref(), r.line), (Some("web/app.py"), at.line));
    assert!(at.line.is_some());

    // Different endpoints are different rows; the path column replaces the start
    // node and the end node is not shown.
    let all = gql(
        &merged,
        "MATCH p = (a {qname: 'web::app::page'})-[:CALLS*]->(b) RETURN p, b.qname AS to \
         ORDER BY to",
        &opts(),
    );
    assert_eq!(all.outputs[0].columns, strs(&["p", "to"]));
    let got: Vec<Vec<String>> = all.results.iter().map(texts).collect();
    assert_eq!(
        got,
        [
            strs(&[
                "web::app::page -CALLS-> shared::util::helper -CALLS-> \
                 services::api::internal::db::query",
                "services::api::internal::db::query",
            ]),
            strs(&[
                "web::app::page -CALLS-> services::api::internal::db::write",
                "services::api::internal::db::write",
            ]),
            strs(&[
                "web::app::page -CALLS-> shared::util::helper",
                "shared::util::helper",
            ]),
        ]
    );
    // page -> write exists only through the overlay edge.
    let tiers: Vec<&str> = all.results.iter().map(|r| r.tier).collect();
    assert_eq!(tiers, ["fact", "heuristic", "fact"]);

    // A backward path renders in pattern order, against the edges.
    let back = gql(
        &merged,
        "MATCH p = (b {qname: 'services::api::internal::db::query'})<-[:CALLS*]-\
         (a {qname: 'web::app::page'}) RETURN p",
        &opts(),
    );
    assert_eq!(
        back.results.iter().map(texts).collect::<Vec<_>>(),
        [strs(&[
            "services::api::internal::db::query <-CALLS- shared::util::helper <-CALLS- \
             web::app::page"
        ])]
    );
    // ORDER BY a path column orders by the rendered path.
    let ordered = gql(
        &merged,
        "MATCH p = (a {qname: 'web::app::page'})-[:CALLS*]->(b) RETURN p ORDER BY p DESC",
        &opts(),
    );
    let paths: Vec<String> = ordered.results.iter().map(|r| text(&r.values[0])).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    sorted.reverse();
    assert_eq!(paths, sorted);
    assert_eq!(paths.len(), 3);

    // An edge of another pattern that touches the path's start is not the path:
    // the walk takes the first run of witness leaves that reaches the end.
    let other = gql(
        &merged,
        "MATCH (m:MODULE)-[:DEFINES]->(a {qname: 'web::app::page'}), \
         p = (a)-[:CALLS*]->(b {qname: 'services::api::internal::db::query'}) RETURN p",
        &opts(),
    );
    assert_eq!(
        other.results.iter().map(texts).collect::<Vec<_>>(),
        [strs(&[
            "web::app::page -CALLS-> shared::util::helper -CALLS-> \
             services::api::internal::db::query"
        ])]
    );
    let cats: Vec<&str> = other.results[0]
        .witness
        .iter()
        .map(|w| w.category)
        .collect();
    assert_eq!(
        cats,
        ["DEFINES", "CALLS", "CALLS"],
        "the DEFINES leaf comes first"
    );

    // `{0,}` adds the zero-hop path, rendered as its one node.
    let zero = gql(
        &merged,
        "MATCH p = (a {qname: 'web::app::page'})-[:CALLS]->{0,}(b) RETURN p",
        &opts(),
    );
    let got: Vec<String> = zero.results.iter().map(|r| text(&r.values[0])).collect();
    assert_eq!(
        got,
        [
            "web::app::page",
            "web::app::page -CALLS-> services::api::internal::db::write",
            "web::app::page -CALLS-> shared::util::helper",
            "web::app::page -CALLS-> shared::util::helper -CALLS-> \
             services::api::internal::db::query",
        ]
    );
    assert!(zero.results[0].witness.is_empty());
}

#[test]
fn errors() {
    let (_tmp, merged) = fx1();
    let e = fail(&merged, "OPTIONAL MATCH (a) RETURN a");
    assert_eq!(e.kind, "parse", "{}", e.rendered);
    assert!(e.rendered.contains("OPTIONAL MATCH"), "{}", e.rendered);
    assert!(
        e.rendered.starts_with("<gql>:1:1: error: "),
        "{}",
        e.rendered
    );
    assert_eq!((e.diagnostics[0].line, e.diagnostics[0].column), (1, 1));

    let e = fail(&merged, "MATCH (a:ROUTEE) RETURN a");
    assert_eq!(e.kind, "analysis", "{}", e.rendered);
    let d = e
        .diagnostics
        .iter()
        .find(|d| d.message.contains("unknown node kind `ROUTEE`"))
        .unwrap_or_else(|| panic!("{:?}", e.diagnostics));
    assert_eq!(d.line, 1);
    assert!(e.rendered.starts_with("<gql>:1:"), "{}", e.rendered);

    // A caller's source name labels the diagnostics.
    let mut named = opts();
    named.source_name = "q.gql".to_string();
    let e = query_gql(&merged, "MATCH (a:ROUTEE) RETURN a", &named).expect_err("analysis");
    assert!(e.rendered.starts_with("q.gql:1:"), "{}", e.rendered);
    let e = query_gql(&merged, "OPTIONAL MATCH (a) RETURN a", &named).expect_err("parse");
    assert!(
        e.rendered.starts_with("q.gql:1:1: error: "),
        "{}",
        e.rendered
    );

    // A refused lowering is a parse error too, located.
    let e = fail(&merged, "MATCH (a)-[:CALLS*1..9]->(b) RETURN b");
    assert_eq!(e.kind, "parse", "{}", e.rendered);
}

#[test]
fn deterministic() {
    let (_t1, m1) = fx1();
    let (_t2, m2) = fx1();
    let mut o = opts();
    o.explain = true;
    for text_ in [WEB_CALLS, PATH] {
        let a = serde_json::to_string(&gql(&m1, text_, &o)).expect("serialises");
        let b = serde_json::to_string(&gql(&m2, text_, &o)).expect("serialises");
        assert_eq!(a, b, "{text_}");
        assert!(a.starts_with("{\"language\":\"gql\",\"outputs\":["), "{a}");
    }
    let a = serde_json::to_string(&gql(&m1, PATH, &opts())).expect("serialises");
    let v: serde_json::Value = serde_json::from_str(&a).expect("json");
    assert_eq!(
        v["results"][0]["values"][0]["str"],
        "web::app::page -CALLS-> shared::util::helper -CALLS-> services::api::internal::db::query"
    );
    assert_eq!(v["outputs"][0]["predicate"], "result");
}
