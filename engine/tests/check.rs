//! LE.8 — `check`: the `[[constraint]]` rules LF.4a stores as CONSTRAINT
//! cells, evaluated to VIOLATION with located evidence.
//!
//! One python tree with two manifest projects (web/, services/api/) so LF.4a
//! anchors every rule on a PROJECT: web/app.py imports and calls
//! services/api/internal.py, services/api/a.py and b.py import each other,
//! and web/x.py and y.py import each other too (a cycle OUTSIDE the api
//! scope, which the api-acyclic rule must not report).
//!
//! Scope membership is strict: a node is in `services/api` only when its
//! located file sits under that path (or it is a PROJECT whose path does). A
//! SQL table (`data_entity:sql:orders`) has no file, so web code reaching it
//! never counts as reaching `services/api`.

use glia_code_domain::{cell_type, edge_category};
use glia_core::{Cell, CellPayload};
use glia_engine::check::{CheckReport, Violation, check};
use glia_engine::{generate_one, locate_node};
use glia_graph::MergedGraph;

const WEB_APP: &str =
    "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n";
const WEB_APP_CLEAN: &str = "def pay(o):\n    return o\n";
const INTERNAL: &str = "def charge(o):\n    return o\n";
const API_A: &str = "from services.api.b import g\n\n\ndef f():\n    return g()\n";
const API_B: &str = "from services.api.a import f\n\n\ndef g():\n    return 1\n";
const WEB_X: &str = "from web.y import h\n\n\ndef k():\n    return 1\n";
const WEB_Y: &str = "from web.x import k\n\n\ndef h():\n    return 2\n";
const WEB_STORE: &str =
    "def list_orders(cur):\n    return cur.execute(\"SELECT id FROM orders\")\n";

/// The three rules of the acceptance: a forbid_edge (line 3), a no_cycle
/// (line 10) and an invariant (line 15), every one scoped so it anchors on a
/// PROJECT.
const RULES: &str = "version = 1

[[constraint]]
id = \"web-no-api-internals\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
categories = [\"IMPORTS\", \"CALLS\"]

[[constraint]]
id = \"api-acyclic\"
kind = \"no_cycle\"
scope = \"services/api\"

[[constraint]]
id = \"prose\"
kind = \"invariant\"
scope = \"services/api\"
text = \"charges are idempotent\"
";

const MANIFESTS: [(&str, &str); 2] = [
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
];

/// Build one repo from `(relative path, source)` pairs plus the two
/// manifests; the tempdir is returned so it outlives the graph's use.
fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (rel, src) in MANIFESTS.iter().chain(files) {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let r = generate_one(tmp.path().to_str().expect("utf-8 temp path")).expect("generate_one");
    (tmp, r.merged)
}

fn violating() -> (tempfile::TempDir, MergedGraph) {
    build(&[
        ("web/app.py", WEB_APP),
        ("web/x.py", WEB_X),
        ("web/y.py", WEB_Y),
        ("services/api/internal.py", INTERNAL),
        ("services/api/a.py", API_A),
        ("services/api/b.py", API_B),
        (".glia/overlay.toml", RULES),
    ])
}

fn rule<'a>(r: &'a CheckReport, id: &str) -> Vec<&'a Violation> {
    r.violations.iter().filter(|v| v.rule_id == id).collect()
}

/// `(category, file:line)` of each evidence row, in report order.
fn sites(v: &Violation) -> Vec<(&'static str, String)> {
    v.evidence
        .iter()
        .map(|e| {
            (
                e.category,
                format!(
                    "{}:{}",
                    e.file.as_deref().unwrap_or("-"),
                    e.line.map_or("-".to_string(), |l| l.to_string())
                ),
            )
        })
        .collect()
}

#[test]
fn forbid_edge_reports_both_edges() {
    let (_tmp, merged) = violating();
    let report = check(&merged);
    assert_eq!(report.rules, 3, "{report:#?}");
    assert_eq!(report.checked, 2, "{report:#?}");
    assert!(report.errors.is_empty(), "{report:#?}");
    let v = rule(&report, "web-no-api-internals");
    assert_eq!(v.len(), 1, "{report:#?}");
    let v = v[0];
    assert_eq!(v.rule_kind, "forbid_edge");
    assert_eq!(v.severity, "VIOLATION");
    assert_eq!(v.tier, "fact");
    assert_eq!(v.decl.as_deref(), Some(".glia/overlay.toml:3"));
    assert_eq!(v.count, 2, "{v:#?}");
    // Sorted by (file, line, category): the import on line 1, the call on 5.
    assert_eq!(
        sites(v),
        vec![
            ("IMPORTS", "web/app.py:1".to_string()),
            ("CALLS", "web/app.py:5".to_string()),
        ],
        "{v:#?}"
    );
    let e = &v.evidence;
    assert_eq!(
        (e[0].from_qname.as_str(), e[0].to_qname.as_str()),
        ("web::app", "services::api::internal")
    );
    assert_eq!(
        (e[1].from_qname.as_str(), e[1].to_qname.as_str()),
        ("web::app::pay", "services::api::internal::charge")
    );
    assert!(
        e.iter().all(|x| x
            .emitter
            .as_deref()
            .is_some_and(|m| m.starts_with("graph:"))),
        "each edge names the stage that asserted it: {e:#?}"
    );
}

#[test]
fn no_cycle_reports_the_import_cycle() {
    let (_tmp, merged) = violating();
    let report = check(&merged);
    let v = rule(&report, "api-acyclic");
    // web/x <-> web/y is a cycle too, but outside services/api.
    assert_eq!(v.len(), 1, "{report:#?}");
    let v = v[0];
    assert_eq!(v.rule_kind, "no_cycle");
    assert_eq!(v.tier, "derived");
    assert_eq!(v.severity, "VIOLATION");
    assert_eq!(v.decl.as_deref(), Some(".glia/overlay.toml:10"));
    assert_eq!(v.count, 2, "two modules in the component: {v:#?}");
    let hops: Vec<(&str, &str, &str)> = v
        .evidence
        .iter()
        .map(|e| (e.from_qname.as_str(), e.category, e.to_qname.as_str()))
        .collect();
    assert_eq!(
        hops,
        vec![
            ("services::api::a", "IMPORTS", "services::api::b"),
            ("services::api::b", "IMPORTS", "services::api::a"),
        ]
    );
    assert_eq!(
        sites(v),
        vec![
            ("IMPORTS", "services/api/a.py:1".to_string()),
            ("IMPORTS", "services/api/b.py:1".to_string()),
        ]
    );
    // Violations come sorted by rule id.
    let ids: Vec<&str> = report
        .violations
        .iter()
        .map(|v| v.rule_id.as_str())
        .collect();
    assert_eq!(ids, vec!["api-acyclic", "web-no-api-internals"]);
}

#[test]
fn no_cycle_over_calls_is_node_level() {
    let calls_rule = "version = 1\n\n[[constraint]]\nid = \"api-no-recursion\"\nkind = \"no_cycle\"\nscope = \"services/api\"\ncategories = [\"CALLS\"]\n";
    let api_c = "from services.api.d import q\n\n\ndef p(n):\n    return q(n)\n";
    let api_d = "from services.api.c import p\n\n\ndef q(n):\n    return p(n - 1)\n";
    let (_tmp, merged) = build(&[
        ("services/api/internal.py", INTERNAL),
        ("services/api/c.py", api_c),
        ("services/api/d.py", api_d),
        (".glia/overlay.toml", calls_rule),
    ]);
    let report = check(&merged);
    assert!(report.errors.is_empty(), "{report:#?}");
    let v = rule(&report, "api-no-recursion");
    assert_eq!(v.len(), 1, "{report:#?}");
    let hops: Vec<(&str, &str, &str)> = v[0]
        .evidence
        .iter()
        .map(|e| (e.from_qname.as_str(), e.category, e.to_qname.as_str()))
        .collect();
    assert_eq!(
        hops,
        vec![
            ("services::api::c::p", "CALLS", "services::api::d::q"),
            ("services::api::d::q", "CALLS", "services::api::c::p"),
        ],
        "{v:#?}"
    );
    assert_eq!(v[0].tier, "derived");
}

#[test]
fn invariant_is_unchecked() {
    let (_tmp, merged) = violating();
    let report = check(&merged);
    assert_eq!(report.unchecked, vec!["prose".to_string()], "{report:#?}");
    assert!(
        report.violations.iter().all(|v| v.rule_id != "prose"),
        "an invariant is listed, never evaluated: {report:#?}"
    );
}

/// A rule stored before an edge category was renamed keeps the stale name
/// (`parse_constraints` keeps it for the checker); the overlay loader would
/// reject it today, so the rule is written straight onto the PROJECT node, as
/// an older store holds it.
#[test]
fn unknown_category_is_an_error_not_a_violation() {
    let (_tmp, mut merged) = build(&[
        ("web/app.py", WEB_APP),
        ("services/api/internal.py", INTERNAL),
    ]);
    let entry = r#"[{"categories":["RENAMED_SINCE"],"from":"web","id":"old-rule","kind":"forbid_edge","source":"api","to":"services/api"}]"#;
    let mut placed = 0;
    for g in &mut merged.graphs {
        let Some(id) = g
            .nav
            .qname_by_id
            .iter()
            .find(|(_, q)| q.as_str() == "project:web")
            .map(|(id, _)| *id)
        else {
            continue;
        };
        for n in g.nodes.iter_mut().filter(|n| n.id == id) {
            n.cells.push(Cell {
                kind: cell_type::CONSTRAINT,
                payload: CellPayload::Json(entry.to_string()),
            });
            placed += 1;
        }
    }
    assert!(placed > 0, "project:web is in the graph");
    let report = check(&merged);
    assert_eq!(report.rules, 1, "{report:#?}");
    assert_eq!(
        report.checked, 0,
        "a rule with an unknown category is skipped"
    );
    assert!(report.violations.is_empty(), "{report:#?}");
    assert_eq!(report.errors.len(), 1, "{report:#?}");
    let (id, msg) = &report.errors[0];
    assert_eq!(id, "old-rule");
    assert!(msg.contains("RENAMED_SINCE"), "{msg}");
}

#[test]
fn scope_matching_no_node_is_an_error() {
    let ghost = "version = 1\n\n[[constraint]]\nid = \"web-no-ghost\"\nkind = \"forbid_edge\"\nfrom = \"web\"\nto = \"ghost\"\n";
    let (_tmp, merged) = build(&[
        ("web/app.py", WEB_APP),
        ("services/api/internal.py", INTERNAL),
        (".glia/overlay.toml", ghost),
    ]);
    let report = check(&merged);
    assert_eq!((report.rules, report.checked), (1, 0), "{report:#?}");
    assert!(report.violations.is_empty());
    assert_eq!(report.errors.len(), 1, "{report:#?}");
    assert_eq!(report.errors[0].0, "web-no-ghost");
    assert!(report.errors[0].1.contains("ghost"), "{report:#?}");
}

#[test]
fn unlocatable_nodes_never_match() {
    // Default categories (no `categories`): ACCESSES_DATA is one of them.
    let rules = "version = 1\n\n[[constraint]]\nid = \"web-off-api\"\nkind = \"forbid_edge\"\nfrom = \"web\"\nto = \"services/api\"\n";
    let (_tmp, merged) = build(&[
        ("web/store.py", WEB_STORE),
        ("services/api/internal.py", INTERNAL),
        (".glia/overlay.toml", rules),
    ]);
    // The precondition that makes this test bite: web code reaches a table
    // that no file places.
    let table = merged
        .all_edges()
        .find(|e| e.category == edge_category::ACCESSES_DATA)
        .map(|e| e.to)
        .expect("web/store.py reaches a data entity");
    let at = locate_node(&merged, table);
    assert_eq!(at.qname, "data_entity:sql:orders");
    assert!(at.file.is_none(), "{at:?}");

    let report = check(&merged);
    assert_eq!((report.rules, report.checked), (1, 1), "{report:#?}");
    assert!(report.errors.is_empty(), "{report:#?}");
    assert!(report.violations.is_empty(), "{report:#?}");
}

#[test]
fn clean_repo_zero_violations() {
    let (_tmp, merged) = build(&[
        ("web/app.py", WEB_APP_CLEAN),
        ("services/api/internal.py", INTERNAL),
        (".glia/overlay.toml", RULES),
    ]);
    let report = check(&merged);
    assert_eq!(report.rules, 3, "{report:#?}");
    assert_eq!(report.checked, 2, "{report:#?}");
    assert_eq!(report.unchecked, vec!["prose".to_string()]);
    assert!(report.errors.is_empty(), "{report:#?}");
    assert!(report.violations.is_empty(), "{report:#?}");
}

#[test]
fn no_rules_is_an_empty_report() {
    let (_tmp, merged) = build(&[("web/app.py", WEB_APP)]);
    let report = check(&merged);
    assert_eq!(
        (report.rules, report.checked, report.violations.len()),
        (0, 0, 0)
    );
    assert!(report.unchecked.is_empty() && report.errors.is_empty());
    let json = serde_json::to_string(&report).expect("serialises");
    assert_eq!(
        json,
        r#"{"rules":0,"checked":0,"unchecked":[],"errors":[],"violations":[]}"#
    );
}
