//! DB.3b — `check` evaluates a transitive forbid_edge (0.5.2 Q3).
//!
//! The fixture is DB.2a's fx1 without its overlay edge: three manifest
//! projects (web/, shared/, services/api/). web/app.py imports shared.util
//! and services.api.public.facade; `page()` calls shared.util.helper and
//! `other()` calls the facade's `get`. shared/util.py imports
//! services.api.internal.db and `helper()` calls its `query`; the facade
//! imports db and `get()` calls `write`.
//!
//! A version-2 `forbid_edge` with `transitive = true` reports every node of
//! `to` that a path from `from` reaches through any third scope, stopping at
//! the first node of `to`; a `via` facade is allowed and never walked
//! through. IMPORTS chains over the module import graph apart from the
//! symbol categories unless the rule names both IMPORTS and CALLS. One
//! `forbid_path` Violation per hit, with its best-tier shortest witness.
//!
//! The marker tests re-run this binary on one child test and read its
//! stderr; run with `-- --nocapture` to see the `[check]` and `[rules]
//! check` lines.

use std::path::Path;
use std::process::Command;

use glia_engine::check::{CheckReport, Violation, check};
use glia_engine::generate_one;
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

const SOURCES: [(&str, &str); 7] = [
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("shared/pyproject.toml", "[project]\nname = \"shared\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
    ("web/app.py", WEB_APP),
    ("shared/util.py", SHARED_UTIL),
    ("services/api/public/facade.py", FACADE),
    ("services/api/internal/db.py", DB),
];

/// The acceptance rule: web never reaches services/api except through its
/// public facade, over the default categories.
const VIA: &str = "version = 2

[[constraint]]
id = \"web-no-api\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
transitive = true
via = [\"services/api/public\"]
";

/// [`VIA`] without the facade.
const NO_VIA: &str = "version = 2

[[constraint]]
id = \"web-no-api\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
transitive = true
";

/// [`VIA`] naming IMPORTS and CALLS: one mixed chain.
const MIXED: &str = "version = 2

[[constraint]]
id = \"web-no-api\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
categories = [\"IMPORTS\", \"CALLS\"]
transitive = true
via = [\"services/api/public\"]
";

/// [`VIA`] plus a declared (heuristic) one-hop shortcut to `query`.
const SHORTCUT: &str = "version = 2

[[edge]]
from = \"web::app::page\"
to = \"services::api::internal::db::query\"
category = \"CALLS\"

[[constraint]]
id = \"web-no-api\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
transitive = true
via = [\"services/api/public\"]
";

/// [`SHORTCUT`] plus a declared edge to `write`, which nothing else reaches.
const SHORTCUTS: &str = "version = 2

[[edge]]
from = \"web::app::page\"
to = \"services::api::internal::db::query\"
category = \"CALLS\"

[[edge]]
from = \"web::app::page\"
to = \"services::api::internal::db::write\"
category = \"CALLS\"

[[constraint]]
id = \"web-no-api\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
transitive = true
via = [\"services/api/public\"]
";

/// A facade no node sits in.
const UNKNOWN_VIA: &str = "version = 2

[[constraint]]
id = \"web-no-api\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
transitive = true
via = [\"services/api/nope\"]
";

/// The HEAD-probe rule: a direct (version-1) forbid_edge.
const DIRECT: &str = "version = 1

[[constraint]]
id = \"web-no-api\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
";

/// The [`DIRECT`] Violation as `check` serialised it before DB.3b (HEAD
/// 333ec252): a direct rule's output does not move.
const DIRECT_AT_HEAD: &str = concat!(
    r#"{"rule_id":"web-no-api","rule_kind":"forbid_edge","decl":".glia/overlay.toml:3","#,
    r#""severity":"VIOLATION","tier":"fact","blocking":true,"count":2,"evidence":["#,
    r#"{"from_qname":"web::app","to_qname":"services::api::public::facade","category":"IMPORTS","#,
    r#""file":"web/app.py","line":2,"emitter":"graph:imports","tier":"fact","note":null},"#,
    r#"{"from_qname":"web::app::other","to_qname":"services::api::public::facade::get","#,
    r#""category":"CALLS","file":"web/app.py","line":10,"emitter":"graph:calls","tier":"fact","#,
    r#""note":null}]}"#
);

/// fx1 with `overlay` as `.glia/overlay.toml`, built in a fresh tempdir
/// (returned so it outlives the graph).
fn build(overlay: &str) -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (rel, src) in SOURCES
        .iter()
        .chain([(".glia/overlay.toml", overlay)].iter())
    {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let r = generate_one(tmp.path().to_str().expect("utf-8 temp path")).expect("generate_one");
    (tmp, r.merged)
}

fn report(overlay: &str) -> CheckReport {
    let (_tmp, merged) = build(overlay);
    check(&merged)
}

/// A hop as compared: (category, from, to, `file:line`).
type Hop = (&'static str, String, String, String);

fn hops(v: &Violation) -> Vec<Hop> {
    v.evidence
        .iter()
        .map(|e| {
            (
                e.category,
                e.from_qname.clone(),
                e.to_qname.clone(),
                format!(
                    "{}:{}",
                    e.file.as_deref().unwrap_or("-"),
                    e.line.map_or("-".to_string(), |l| l.to_string())
                ),
            )
        })
        .collect()
}

fn hop(category: &'static str, from: &str, to: &str, at: &str) -> Hop {
    (category, from.to_string(), to.to_string(), at.to_string())
}

/// Each violation's first subject, in report order.
fn subjects(r: &CheckReport) -> Vec<&str> {
    r.violations
        .iter()
        .map(|v| v.subject.first().map_or("", String::as_str))
        .collect()
}

/// The violation whose subject is `[q]`.
fn hit<'a>(r: &'a CheckReport, q: &str) -> &'a Violation {
    let found: Vec<&Violation> = r
        .violations
        .iter()
        .filter(|v| v.subject == [q.to_string()])
        .collect();
    assert_eq!(found.len(), 1, "{q}: {r:#?}");
    found[0]
}

/// The two fact paths into the api's internals that skip the facade.
fn assert_the_two_breaches(r: &CheckReport) {
    assert_eq!(
        subjects(r),
        [
            "services::api::internal::db",
            "services::api::internal::db::query"
        ],
        "{r:#?}"
    );
    let db = hit(r, "services::api::internal::db");
    assert_eq!(
        (
            db.rule_id.as_str(),
            db.rule_kind,
            db.tier,
            db.count,
            db.blocking
        ),
        ("web-no-api", "forbid_path", "fact", 2, true),
        "{db:#?}"
    );
    assert_eq!(
        hops(db),
        [
            hop("IMPORTS", "web::app", "shared::util", "web/app.py:1"),
            hop(
                "IMPORTS",
                "shared::util",
                "services::api::internal::db",
                "shared/util.py:1"
            ),
        ],
        "{db:#?}"
    );
    assert_eq!(
        db.file.as_deref(),
        Some("services/api/internal/db.py"),
        "{db:#?}"
    );
    assert!(
        db.decl
            .as_deref()
            .is_some_and(|d| d.starts_with(".glia/overlay.toml:")),
        "{db:#?}"
    );
    let query = hit(r, "services::api::internal::db::query");
    assert_eq!(
        (query.rule_kind, query.tier, query.count, query.blocking),
        ("forbid_path", "fact", 2, true),
        "{query:#?}"
    );
    assert_eq!(
        hops(query),
        [
            hop(
                "CALLS",
                "web::app::page",
                "shared::util::helper",
                "web/app.py:6"
            ),
            hop(
                "CALLS",
                "shared::util::helper",
                "services::api::internal::db::query",
                "shared/util.py:5"
            ),
        ],
        "{query:#?}"
    );
    assert_eq!(
        (query.file.as_deref(), query.line),
        (Some("services/api/internal/db.py"), Some(1)),
        "{query:#?}"
    );
    for v in &r.violations {
        assert!(
            v.evidence
                .iter()
                .all(|e| e.tier == "fact" && e.emitter.is_some()),
            "every hop is a located fact: {v:#?}"
        );
    }
}

/// (1) The facade is allowed and never walked through; a path through the
/// third scope `shared` counts, once per node of `to` it reaches first.
#[test]
fn via_facade_is_allowed_third_scope_counts() {
    let r = report(VIA);
    assert!(r.errors.is_empty(), "{r:#?}");
    assert!(r.unchecked.is_empty(), "the rule is evaluated: {r:#?}");
    assert_eq!((r.rules, r.checked), (1, 1), "{r:#?}");
    assert_the_two_breaches(&r);
    assert!(
        r.violations
            .iter()
            .all(|v| v.decl.as_deref() == Some(".glia/overlay.toml:3")),
        "{r:#?}"
    );
    assert!(
        r.violations
            .iter()
            .all(|v| !v.subject[0].starts_with("services::api::public")
                && v.subject[0] != "services::api::internal::db::write"),
        "nothing under the facade, and write is reached only through it: {r:#?}"
    );
}

/// (2) Without a facade the entry points are hits, and the walk stops at
/// the first node of `to`: `write`, reached only past `get`, is no hit.
#[test]
fn without_via_the_entry_points_are_hits() {
    let r = report(NO_VIA);
    assert!(r.errors.is_empty() && r.unchecked.is_empty(), "{r:#?}");
    assert_eq!(
        subjects(&r),
        [
            "services::api::internal::db",
            "services::api::internal::db::query",
            "services::api::public::facade",
            "services::api::public::facade::get",
        ],
        "{r:#?}"
    );
    assert_eq!(
        hops(hit(&r, "services::api::public::facade")),
        [hop(
            "IMPORTS",
            "web::app",
            "services::api::public::facade",
            "web/app.py:2"
        )]
    );
    assert_eq!(
        hops(hit(&r, "services::api::public::facade::get")),
        [hop(
            "CALLS",
            "web::app::other",
            "services::api::public::facade::get",
            "web/app.py:10"
        )]
    );
    assert_eq!(hit(&r, "services::api::public::facade::get").count, 1);
}

/// (3) A rule naming IMPORTS and CALLS walks one mixed chain: the same two
/// hits, with the same witnesses.
#[test]
fn naming_imports_and_calls_mixes_one_chain() {
    let r = report(MIXED);
    assert!(r.errors.is_empty() && r.unchecked.is_empty(), "{r:#?}");
    assert_the_two_breaches(&r);
}

/// (4) The tier of a hit is the first of three passes (FACT edges, then
/// DERIVED, then all) to reach it: a declared one-hop shortcut does not mask
/// the two-hop fact path, and a node only a declared edge reaches is a
/// heuristic, advisory hit.
#[test]
fn best_tier_path_wins() {
    let r = report(SHORTCUT);
    assert!(r.errors.is_empty(), "{r:#?}");
    assert_the_two_breaches(&r);

    let r = report(SHORTCUTS);
    assert!(r.errors.is_empty(), "{r:#?}");
    assert_eq!(
        subjects(&r),
        [
            "services::api::internal::db",
            "services::api::internal::db::query",
            "services::api::internal::db::write",
        ],
        "{r:#?}"
    );
    let query = hit(&r, "services::api::internal::db::query");
    assert_eq!((query.tier, query.count), ("fact", 2), "{query:#?}");
    let write = hit(&r, "services::api::internal::db::write");
    assert_eq!(
        (write.tier, write.count, write.blocking),
        ("heuristic", 1, false),
        "{write:#?}"
    );
    assert_eq!(write.evidence.len(), 1, "{write:#?}");
    let e = &write.evidence[0];
    assert_eq!(
        (
            e.category,
            e.from_qname.as_str(),
            e.to_qname.as_str(),
            e.tier
        ),
        (
            "CALLS",
            "web::app::page",
            "services::api::internal::db::write",
            "heuristic"
        ),
        "{e:#?}"
    );
}

/// (5) A facade no node sits in is a rule error (glia check exits 2), and
/// the rule is not counted checked.
#[test]
fn unknown_via_is_an_error() {
    let r = report(UNKNOWN_VIA);
    assert_eq!((r.rules, r.checked), (1, 0), "{r:#?}");
    assert!(r.violations.is_empty() && r.unchecked.is_empty(), "{r:#?}");
    assert_eq!(r.errors.len(), 1, "{r:#?}");
    let (id, msg) = &r.errors[0];
    assert_eq!(id, "web-no-api");
    assert!(msg.contains("via scope `services/api/nope`"), "{msg}");
}

/// (6) A direct rule keeps its function and output, byte for byte.
#[test]
fn direct_forbid_is_unchanged() {
    let r = report(DIRECT);
    assert_eq!(r.violations.len(), 1, "{r:#?}");
    let got = serde_json::to_string(&r.violations[0]).expect("serialises");
    assert_eq!(got, DIRECT_AT_HEAD);
}

/// (7) Two builds in two tempdirs, one report.
#[test]
fn report_is_deterministic_across_checkouts() {
    for overlay in [VIA, NO_VIA, MIXED, SHORTCUTS] {
        let one = serde_json::to_string(&report(overlay)).expect("serialises");
        let two = serde_json::to_string(&report(overlay)).expect("serialises");
        assert_eq!(one, two, "two tempdirs, one report");
    }
}

/// The env var that makes [`markers_child`] run; its value names the overlay.
const CHILD: &str = "GLIA_DB3B_MARKER_CHILD";

/// Runs only inside a [`markers`] child.
#[test]
fn markers_child() {
    let overlay = match std::env::var(CHILD).as_deref() {
        Ok("via") => VIA,
        Ok("mixed") => MIXED,
        _ => return,
    };
    report(overlay);
}

/// The `[check]` and `[rules] check` lines a child prints for `which`.
fn markers(which: &str) -> Vec<String> {
    let exe = std::env::current_exe().expect("the test binary");
    let out = Command::new(Path::new(&exe))
        .args([
            "markers_child",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, which)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("the test binary runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let lines: Vec<String> = stderr
        .lines()
        .filter(|l| l.starts_with("[check]") || l.starts_with("[rules] check "))
        .map(String::from)
        .collect();
    for l in &lines {
        eprintln!("{l}");
    }
    lines
}

#[test]
fn marker_counts_hits_per_chain() {
    assert_eq!(
        markers("via"),
        [
            "[check] rules=1 checked=1 violations=2 (forbid_edge=0 no_cycle=0) unchecked=0 errors=0",
            "[check] tiers fact=2 derived=0 heuristic=0",
            "[check] gate blocking=2 advisory=0",
            "[rules] check invariant=0 rows=0 transitive=1 hits=2 (imports=1 symbols=1 mixed=0)",
        ]
    );
    assert_eq!(
        markers("mixed").last().map(String::as_str),
        Some("[rules] check invariant=0 rows=0 transitive=1 hits=2 (imports=0 symbols=0 mixed=2)")
    );
}
