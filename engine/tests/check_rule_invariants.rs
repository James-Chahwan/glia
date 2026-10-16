//! DB.3a — `check` evaluates rule invariants and gates every rule by
//! `min_tier`.
//!
//! One python tree with five manifest projects (the root, web/,
//! services/api/, store/, scripts/), so the unscoped invariants anchor on the
//! root PROJECT and every forbid_edge on its `from` project:
//! web/admin.py calls store/db.py's `load` (a FACT CALLS, line 5),
//! web/app.py calls services/api/handlers.py's `get_order`, web/client.py
//! `requests.get`s the `/orders` route services/api/server.py serves (a
//! resolver-paired, DERIVED HTTP_CALLS), handlers.py's `unused` is called by
//! nothing, and the overlay declares the only edge from scripts/ into store/
//! (a HEURISTIC CALLS).
//!
//! An invariant's `rule` runs through the rules layer: one `rule` Violation
//! per `violation` row with its subject, location, tier, witness edges and
//! Q5 caveats; a rule that does not analyse is an `errors` row. Every
//! violation is `blocking` when its tier is at least its rule's `min_tier`
//! (default derived) and never when it is heuristic (0.5.2 Q2).
//!
//! The marker test re-runs this binary on one test and reads its stderr; run
//! with `-- --nocapture` to see the `[check]` and `[rules] check` lines.

use std::path::Path;
use std::process::Command;

use glia_engine::check::{CheckReport, Violation, check};
use glia_engine::{coverage_report, generate_one};
use glia_graph::MergedGraph;

const OVERLAY: &str = r#"version = 2

[[edge]]
from = "scripts::tool::run"
to = "store::db::load"
category = "CALLS"

[[constraint]]
id = "web-no-store"
kind = "invariant"
text = "web never calls the store directly"
rule = "violation(a, b) :- calls(a, b), in_scope(a, \"web\"), in_scope(b, \"store\")."

[[constraint]]
id = "api-called"
kind = "invariant"
text = "every api handler is called"
rule = "violation(f) :- node(f, \"FUNCTION\"), in_scope(f, \"services/api/handlers.py\"), !called(f). called(f) :- edge(_, f, \"CALLS\")."

[[constraint]]
id = "rotten"
kind = "invariant"
text = "a rule naming an unknown kind"
rule = "violation(r) :- node(r, \"ROUTEE\")."

[[constraint]]
id = "scripts-no-store"
kind = "invariant"
text = "scripts never call the store"
rule = "violation(a, b) :- calls(a, b), in_scope(a, \"scripts\"), in_scope(b, \"store\")."

[[constraint]]
id = "web-no-api-http"
kind = "forbid_edge"
from = "web"
to = "services/api"
categories = ["HTTP_CALLS"]

[[constraint]]
id = "web-no-api-http-fact"
kind = "forbid_edge"
from = "web"
to = "services/api"
categories = ["HTTP_CALLS"]
min_tier = "fact"

[[constraint]]
id = "scripts-direct"
kind = "forbid_edge"
from = "scripts"
to = "store"

[[constraint]]
id = "prose"
kind = "invariant"
text = "charges are idempotent"
"#;

const FILES: &[(&str, &str)] = &[
    ("pyproject.toml", "[project]\nname = \"root\"\n"),
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
    ("store/pyproject.toml", "[project]\nname = \"store\"\n"),
    ("scripts/pyproject.toml", "[project]\nname = \"scripts\"\n"),
    (
        "web/app.py",
        "from services.api.handlers import get_order\n\n\ndef show(o):\n    return get_order(o)\n",
    ),
    (
        "web/admin.py",
        "from store.db import load\n\n\ndef audit(k):\n    return load(k)\n",
    ),
    (
        "web/client.py",
        "import requests\n\n\ndef orders():\n    return requests.get(\"http://api/orders\")\n",
    ),
    (
        "services/api/handlers.py",
        "from store.db import load\n\n\ndef get_order(o):\n    return load(o)\n\n\ndef unused():\n    return 1\n",
    ),
    (
        "services/api/server.py",
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.get(\"/orders\")\ndef list_orders():\n    return []\n",
    ),
    ("store/db.py", "def load(k):\n    return k\n"),
    ("scripts/tool.py", "def run():\n    return 0\n"),
    (".glia/overlay.toml", OVERLAY),
];

/// Build the fixture in a fresh tempdir, returned so it outlives the graph.
fn build() -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (rel, src) in FILES {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let r = generate_one(tmp.path().to_str().expect("utf-8 temp path")).expect("generate_one");
    (tmp, r.merged)
}

/// The one violation of rule `id`.
fn one<'a>(r: &'a CheckReport, id: &str) -> &'a Violation {
    let found: Vec<&Violation> = r.violations.iter().filter(|v| v.rule_id == id).collect();
    assert_eq!(found.len(), 1, "{id}: {r:#?}");
    found[0]
}

/// A JSON object's top-level keys in document order (`serde_json::Value`
/// sorts them).
struct Keys(Vec<String>);

impl<'de> serde::Deserialize<'de> for Keys {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Keys;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut m: A) -> Result<Keys, A::Error> {
                let mut out = Vec::new();
                while let Some((k, _)) = m.next_entry::<String, serde::de::IgnoredAny>()? {
                    out.push(k);
                }
                Ok(Keys(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// A violation's JSON object keys, in serialised order.
fn keys(v: &Violation) -> Vec<String> {
    let json = serde_json::to_string(v).expect("serialises");
    serde_json::from_str::<Keys>(&json).expect("an object").0
}

#[test]
fn rules_counts_and_the_rule_error() {
    let (_tmp, merged) = build();
    let report = check(&merged);
    assert_eq!((report.rules, report.checked), (8, 6), "{report:#?}");
    assert_eq!(report.unchecked, ["prose"], "{report:#?}");
    assert_eq!(report.errors.len(), 1, "{report:#?}");
    let (id, msg) = &report.errors[0];
    assert_eq!(id, "rotten");
    assert!(msg.contains("unknown node kind `ROUTEE`"), "{msg}");
    assert!(
        msg.contains("1:"),
        "a 1-based line:column in the rule: {msg}"
    );
    assert!(
        msg.starts_with("rotten (.glia/overlay.toml:"),
        "the source names the rule and its stanza: {msg}"
    );
    let ids: Vec<&str> = report
        .violations
        .iter()
        .map(|v| v.rule_id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "api-called",
            "scripts-direct",
            "scripts-no-store",
            "web-no-api-http",
            "web-no-api-http-fact",
            "web-no-store",
        ],
        "sorted by rule id; the rule in error has none"
    );
}

#[test]
fn rule_row_is_a_located_violation_with_its_witness() {
    let (_tmp, merged) = build();
    let report = check(&merged);
    let v = one(&report, "web-no-store");
    assert_eq!(v.rule_kind, "rule");
    assert_eq!(v.severity, "VIOLATION");
    assert!(
        v.decl
            .as_deref()
            .is_some_and(|d| d.starts_with(".glia/overlay.toml:")),
        "{v:#?}"
    );
    assert_eq!(
        v.subject,
        ["web::admin::audit", "store::db::load"],
        "{v:#?}"
    );
    assert_eq!((v.tier, v.blocking), ("fact", true), "{v:#?}");
    assert_eq!(v.count, 1, "{v:#?}");
    assert_eq!(v.evidence.len(), 1, "{v:#?}");
    let e = &v.evidence[0];
    assert_eq!(
        (e.category, e.file.as_deref(), e.line),
        ("CALLS", Some("web/admin.py"), Some(5)),
        "{e:#?}"
    );
    assert_eq!(
        (e.from_qname.as_str(), e.to_qname.as_str(), e.tier),
        ("web::admin::audit", "store::db::load", "fact")
    );
    assert_eq!(v.file.as_deref(), Some("web/admin.py"), "{v:#?}");
    assert!(
        v.line.is_some(),
        "the subject's first node is located: {v:#?}"
    );
    assert!(v.caveats.is_empty(), "no negation, no caveat: {v:#?}");
    assert_eq!(
        keys(v),
        [
            "rule_id",
            "rule_kind",
            "decl",
            "severity",
            "tier",
            "blocking",
            "count",
            "evidence",
            "subject",
            "file",
            "line",
        ],
        "JSON key order; empty caveats are not serialised"
    );
}

#[test]
fn negated_rule_row_carries_the_coverage_caveats() {
    let (_tmp, merged) = build();
    let report = check(&merged);
    let v = one(&report, "api-called");
    assert_eq!(v.rule_kind, "rule");
    assert_eq!(v.subject, ["services::api::handlers::unused"], "{v:#?}");
    assert_eq!(
        v.file.as_deref(),
        Some("services/api/handlers.py"),
        "{v:#?}"
    );
    assert!(
        v.evidence.is_empty() && v.count == 0,
        "no edge derives it: {v:#?}"
    );
    assert_eq!((v.tier, v.blocking), ("fact", true), "{v:#?}");
    // `caveats_for(merged, ["CALLS"], None)`: the coverage rows of the
    // languages present whose category is CALLS (or every category).
    let want: Vec<_> = coverage_report(&merged)
        .into_iter()
        .filter(|c| c.edge_category == "CALLS" || c.edge_category == "*")
        .collect();
    assert!(
        !want.is_empty(),
        "the `*` CALLS row (dynamic dispatch) applies"
    );
    assert_eq!(
        serde_json::to_value(&v.caveats).expect("serialises"),
        serde_json::to_value(&want).expect("serialises"),
        "Q5: the negation read CALLS"
    );
}

#[test]
fn heuristic_rule_row_is_advisory() {
    let (_tmp, merged) = build();
    let report = check(&merged);
    let v = one(&report, "scripts-no-store");
    assert_eq!(
        v.subject,
        ["scripts::tool::run", "store::db::load"],
        "{v:#?}"
    );
    assert_eq!((v.tier, v.blocking), ("heuristic", false), "{v:#?}");
    assert_eq!(v.evidence.len(), 1, "{v:#?}");
    assert_eq!(v.evidence[0].emitter.as_deref(), Some("overlay:edge"));
}

#[test]
fn min_tier_fact_makes_a_derived_violation_advisory() {
    let (_tmp, merged) = build();
    let report = check(&merged);
    let v = one(&report, "web-no-api-http");
    assert_eq!(v.rule_kind, "forbid_edge");
    assert_eq!(
        (v.tier, v.blocking),
        ("derived", true),
        "default min_tier: {v:#?}"
    );
    assert_eq!(
        v.evidence[0].emitter.as_deref(),
        Some("resolver:http"),
        "{v:#?}"
    );
    let f = one(&report, "web-no-api-http-fact");
    assert_eq!(
        (f.tier, f.blocking),
        ("derived", false),
        "min_tier = fact: {f:#?}"
    );
    assert!(
        f.subject.is_empty() && f.file.is_none() && f.line.is_none() && f.caveats.is_empty(),
        "{f:#?}"
    );
    assert_eq!(
        keys(f),
        [
            "rule_id",
            "rule_kind",
            "decl",
            "severity",
            "tier",
            "blocking",
            "count",
            "evidence"
        ],
        "a forbid_edge violation serialises no subject"
    );
}

/// HEAD (before DB.3a): the same rule failed CI. Q2: a violation broken only
/// by a declared edge is reported, never blocking.
#[test]
fn forbid_edge_heuristic_only_is_advisory() {
    let (_tmp, merged) = build();
    let report = check(&merged);
    let v = one(&report, "scripts-direct");
    assert_eq!(v.rule_kind, "forbid_edge");
    assert_eq!((v.tier, v.blocking), ("heuristic", false), "{v:#?}");
    assert_eq!(
        (
            v.evidence[0].from_qname.as_str(),
            v.evidence[0].to_qname.as_str()
        ),
        ("scripts::tool::run", "store::db::load")
    );
}

#[test]
fn report_is_deterministic_across_checkouts() {
    let (_a, first) = build();
    let (_b, second) = build();
    let one = serde_json::to_string(&check(&first)).expect("serialises");
    let two = serde_json::to_string(&check(&second)).expect("serialises");
    assert_eq!(one, two, "two tempdirs, one report");
}

/// The env var that makes [`markers_child`] run.
const CHILD: &str = "GLIA_DB3A_MARKER_CHILD";

/// Runs only inside [`markers_print_the_gate_and_rule_lines`]'s child.
#[test]
fn markers_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let (_tmp, merged) = build();
    check(&merged);
}

#[test]
fn markers_print_the_gate_and_rule_lines() {
    let exe = std::env::current_exe().expect("the test binary");
    let out = Command::new(Path::new(&exe))
        .args([
            "markers_child",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("the test binary runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[check]") || l.starts_with("[rules] check "))
        .collect();
    for l in &lines {
        eprintln!("{l}");
    }
    assert_eq!(
        lines,
        [
            "[check] rules=8 checked=6 violations=6 (forbid_edge=3 no_cycle=0) unchecked=1 errors=1",
            "[check] tiers fact=2 derived=2 heuristic=2",
            "[check] gate blocking=3 advisory=3",
            "[rules] check invariant=3 rows=3 transitive=0 hits=0 (imports=0 symbols=0 mixed=0)",
        ],
        "{stderr}"
    );
}
