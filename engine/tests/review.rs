//! CC.6a — `review`: one `RevDelta` against a git rev -> the changed nodes,
//! their ranked impact, the tests to run, every added / removed edge with its
//! `why` tier, and the working tree's rules checked on both sides (new vs
//! resolved violations).
//!
//! The tree: two manifest projects (web/, services/api/), `services/api/
//! internal.py` `charge`, `web/app.py` `pay` (no import of the api),
//! `tests/test_pay.py` calling `pay`, and a `.glia/overlay.toml` forbidding
//! web -> services/api over IMPORTS + CALLS (its stanza header on line 3),
//! committed through the LE.1b git fixture. The engine prints
//! `[review] base=.. changed=.. seeds=.. impact=.. tests=.. untested=.. edges +a -r (..) violations new=.. resolved=.. blocking=..`
//! once per review.

mod git_fixture;

use std::collections::BTreeMap;

use git_fixture::GitRepo;
use glia_engine::check::Violation;
use glia_engine::delta::graph_delta_vs_rev;
use glia_engine::review::{Review, ReviewArgs, review, review_vs_rev};

const MANIFESTS: [(&str, &str); 2] = [
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
];
const INTERNAL: &str = "def charge(o):\n    return o\n";
/// `INTERNAL` with `charge`'s body edited: unrelated to the rule.
const INTERNAL_EDITED: &str = "def charge(o):\n    return o or 0\n";
const WEB_APP_CLEAN: &str = "def pay(o):\n    return o\n";
/// `pay` imports and calls the api's internals: IMPORTS on line 1, CALLS on
/// line 5.
const WEB_APP: &str =
    "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n";
const TEST_PAY: &str = "from web.app import pay\n\n\ndef test_pay():\n    assert pay(1) == 1\n";
/// The forbid_edge rule; `[[constraint]]` is on line 3.
const RULES: &str = "version = 1\n\n[[constraint]]\nid = \"web-no-api-internals\"\nkind = \"forbid_edge\"\nfrom = \"web\"\nto = \"services/api\"\ncategories = [\"IMPORTS\", \"CALLS\"]\n";
/// `RULES` plus a no_cycle rule over the api's module imports (line 10).
const RULES_ACYCLIC: &str = "version = 1\n\n[[constraint]]\nid = \"web-no-api-internals\"\nkind = \"forbid_edge\"\nfrom = \"web\"\nto = \"services/api\"\ncategories = [\"IMPORTS\", \"CALLS\"]\n\n[[constraint]]\nid = \"api-acyclic\"\nkind = \"no_cycle\"\nscope = \"services/api\"\n";

const RULE: &str = "web-no-api-internals";
const PAY: &str = "web::app::pay";
const TEST_CASE: &str = "tests::test_pay::test_pay";

/// The clean tree, committed on `main`, with `web_app` as `web/app.py`.
fn committed(web_app: &str) -> GitRepo {
    let repo = GitRepo::init();
    for (rel, src) in MANIFESTS {
        repo.write(rel, src);
    }
    repo.write("services/api/internal.py", INTERNAL);
    repo.write("web/app.py", web_app);
    repo.write("tests/test_pay.py", TEST_PAY);
    repo.write(".glia/overlay.toml", RULES);
    repo.commit("clean");
    repo
}

fn run(repo: &GitRepo) -> Review {
    review_vs_rev(repo.path(), "HEAD", &ReviewArgs::default()).expect("review")
}

/// `(category, file:line, tier)` of each evidence row, in report order.
fn rows(v: &Violation) -> Vec<(&'static str, String, &'static str)> {
    v.evidence
        .iter()
        .map(|e| {
            let at = format!(
                "{}:{}",
                e.file.as_deref().unwrap_or("-"),
                e.line.map_or("-".to_string(), |l| l.to_string())
            );
            (e.category, at, e.tier)
        })
        .collect()
}

/// `(change, category, from, to, tier, emitter)` of one review edge.
type EdgeRow<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str, Option<&'a str>);

fn json(r: &Review) -> String {
    serde_json::to_string(r).expect("serialise the review")
}

/// (1) The working tree adds the forbidden import and call: one NEW
/// violation carrying exactly those two fact rows, and the review blocks.
#[test]
fn new_violation_blocks() {
    let repo = committed(WEB_APP_CLEAN);
    repo.write("web/app.py", WEB_APP);
    let r = run(&repo);
    assert_eq!(r.base, "HEAD");
    assert!(r.blocking, "{r:#?}");
    assert_eq!(r.new_violations.len(), 1, "{:#?}", r.new_violations);
    let v = &r.new_violations[0];
    assert_eq!(
        (v.rule_id.as_str(), v.rule_kind, v.tier),
        (RULE, "forbid_edge", "fact")
    );
    assert_eq!(v.decl.as_deref(), Some(".glia/overlay.toml:3"));
    assert_eq!(v.count, 2, "{v:#?}");
    assert_eq!(
        rows(v),
        [
            ("IMPORTS", "web/app.py:1".to_string(), "fact"),
            ("CALLS", "web/app.py:5".to_string(), "fact"),
        ]
    );
    assert!(
        r.resolved_violations.is_empty(),
        "{:#?}",
        r.resolved_violations
    );
    assert!(r.check_errors.is_empty(), "{:#?}", r.check_errors);

    let pay = r
        .changed
        .iter()
        .find(|c| c.qname == PAY)
        .expect("pay is changed");
    assert_eq!((pay.change, pay.seed), ("modified", true), "{pay:#?}");
    assert_eq!(r.tests.tests[0].qname, TEST_CASE, "{:#?}", r.tests);

    let edges: Vec<EdgeRow<'_>> = r
        .edges
        .iter()
        .map(|e| {
            (
                e.change,
                e.category,
                e.from_qname.as_str(),
                e.to_qname.as_str(),
                e.tier,
                e.emitter.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        edges,
        [
            (
                "added",
                "CALLS",
                PAY,
                "services::api::internal::charge",
                "fact",
                Some("graph:calls")
            ),
            (
                "added",
                "IMPORTS",
                "web::app",
                "services::api::internal",
                "fact",
                Some("graph:imports")
            ),
        ],
        "{:#?}",
        r.edges
    );
    let call = &r.edges[0];
    assert_eq!(
        (call.site_file.as_deref(), call.site_line),
        (Some("web/app.py"), Some(5))
    );

    assert_eq!(r.counts.edges_by_tier, BTreeMap::from([("fact", 2)]));
    assert_eq!((r.counts.edges_added, r.counts.edges_removed), (2, 0));
    assert_eq!(r.counts.untested_seeds, 1, "{:#?}", r.tests.untested);
    assert_eq!(r.counts.tests, r.tests.tests.len());
    assert_eq!(
        (r.counts.new_violations, r.counts.resolved_violations),
        (1, 0)
    );
    assert_eq!(r.counts.nodes_changed, r.changed.len());
    assert_eq!(r.counts.seeds, r.impact.seeds.len());
    assert_eq!(r.counts.impact, r.impact.results.len());
}

/// (2) The base holds the violation and the working tree reverts it: the
/// violation is RESOLVED with the same two rows, located in the base graph,
/// and the review does not block.
#[test]
fn resolved_violation_is_reported() {
    let repo = committed(WEB_APP);
    repo.write("web/app.py", WEB_APP_CLEAN);
    let r = run(&repo);
    assert!(!r.blocking, "{r:#?}");
    assert!(r.new_violations.is_empty(), "{:#?}", r.new_violations);
    assert_eq!(
        r.resolved_violations.len(),
        1,
        "{:#?}",
        r.resolved_violations
    );
    let v = &r.resolved_violations[0];
    assert_eq!((v.rule_id.as_str(), v.count), (RULE, 2));
    assert_eq!(
        rows(v),
        [
            ("IMPORTS", "web/app.py:1".to_string(), "fact"),
            ("CALLS", "web/app.py:5".to_string(), "fact"),
        ]
    );
    assert_eq!(
        (r.counts.new_violations, r.counts.resolved_violations),
        (0, 1)
    );
    let removed: Vec<&str> = r
        .edges
        .iter()
        .filter(|e| e.change == "removed")
        .map(|e| e.category)
        .collect();
    assert_eq!(removed, ["CALLS", "IMPORTS"], "{:#?}", r.edges);
}

/// (3) The violation is on both sides and the change edits an unrelated
/// function: neither new nor resolved.
#[test]
fn pre_existing_violation_is_neither() {
    let repo = committed(WEB_APP);
    repo.write("services/api/internal.py", INTERNAL_EDITED);
    let r = run(&repo);
    assert!(!r.blocking, "{r:#?}");
    assert!(r.new_violations.is_empty(), "{:#?}", r.new_violations);
    assert!(
        r.resolved_violations.is_empty(),
        "{:#?}",
        r.resolved_violations
    );
    assert!(
        r.changed
            .iter()
            .any(|c| c.qname == "services::api::internal::charge"),
        "{:#?}",
        r.changed
    );
}

/// (4) `review` over a delta the caller computed is `review_vs_rev`.
#[test]
fn one_delta_for_everything() {
    let repo = committed(WEB_APP_CLEAN);
    repo.write("web/app.py", WEB_APP);
    let rev = graph_delta_vs_rev(repo.path(), "HEAD").expect("delta");
    let from_delta = review(&rev, &ReviewArgs::default()).expect("review of the delta");
    assert_eq!(json(&from_delta), json(&run(&repo)));
}

/// (5) Two reviews of one change serialise identically.
#[test]
fn deterministic() {
    let repo = committed(WEB_APP_CLEAN);
    repo.write("web/app.py", WEB_APP);
    assert_eq!(json(&run(&repo)), json(&run(&repo)));
}

/// `web/many.py`: `n` functions `f0..` each calling `charge` (a forbidden
/// CALLS edge apiece, plus the module's forbidden IMPORTS), then `extra`.
fn many(n: usize, extra: &str) -> String {
    let mut s = String::from("from services.api.internal import charge\n");
    for i in 0..n {
        s.push_str(&format!("\n\ndef f{i}(o):\n    return charge(o)\n"));
    }
    s.push_str(extra);
    s
}

/// More forbidden edges than a Violation lists (check::MAX_EVIDENCE): the
/// base holds 121, the change adds one at the end of the file. Exactly that
/// edge is new, with its row, and nothing is resolved.
#[test]
fn a_new_edge_beyond_the_evidence_cap_is_new_alone() {
    let repo = committed(WEB_APP_CLEAN);
    repo.write("web/many.py", &many(120, ""));
    repo.commit("many");
    repo.write(
        "web/many.py",
        &many(120, "\n\ndef extra(o):\n    return charge(o)\n"),
    );
    let r = run(&repo);
    assert!(r.blocking, "{r:#?}");
    assert_eq!(r.new_violations.len(), 1, "{:#?}", r.new_violations);
    let v = &r.new_violations[0];
    assert_eq!(v.count, 1, "{v:#?}");
    let got: Vec<(&str, &str, &str)> = v
        .evidence
        .iter()
        .map(|e| (e.category, e.from_qname.as_str(), e.to_qname.as_str()))
        .collect();
    assert_eq!(
        got,
        [(
            "CALLS",
            "web::many::extra",
            "services::api::internal::charge"
        )]
    );
    assert_eq!(v.evidence[0].line, Some(4 * 120 + 5), "{v:#?}");
    assert!(
        r.resolved_violations.is_empty(),
        "{:#?}",
        r.resolved_violations
    );
    assert!(r.check_errors.is_empty(), "{:#?}", r.check_errors);
}

/// The same cap the other way: removing one of 121 forbidden edges resolves
/// exactly that one and blocks nothing.
#[test]
fn a_removed_edge_beyond_the_evidence_cap_is_resolved_alone() {
    let repo = committed(WEB_APP_CLEAN);
    repo.write(
        "web/many.py",
        &many(120, "\n\ndef extra(o):\n    return charge(o)\n"),
    );
    repo.commit("many");
    repo.write("web/many.py", &many(120, ""));
    let r = run(&repo);
    assert!(!r.blocking, "{r:#?}");
    assert!(r.new_violations.is_empty(), "{:#?}", r.new_violations);
    assert_eq!(
        r.resolved_violations.len(),
        1,
        "{:#?}",
        r.resolved_violations
    );
    let v = &r.resolved_violations[0];
    assert_eq!(v.count, 1, "{v:#?}");
    assert_eq!(v.evidence.len(), 1, "{v:#?}");
    assert_eq!(v.evidence[0].from_qname, "web::many::extra");
}

const API_A: &str = "from services.api.b import g\n\n\ndef f():\n    return g()\n";
const API_B: &str = "def g():\n    return 1\n";
const API_B_CYCLE: &str = "from services.api.a import f\n\n\ndef g():\n    return 1\n";
const API_B_VIA_C: &str =
    "from services.api.a import f\nfrom services.api.c import h\n\n\ndef g():\n    return 1\n";
const API_C: &str = "from services.api.a import f\n\n\ndef h():\n    return 1\n";

fn committed_api(b: &str) -> GitRepo {
    let repo = committed(WEB_APP_CLEAN);
    repo.write(".glia/overlay.toml", RULES_ACYCLIC);
    repo.write("services/api/a.py", API_A);
    repo.write("services/api/b.py", b);
    repo.commit("api");
    repo
}

fn cycles(v: &[Violation]) -> Vec<(&str, usize)> {
    v.iter()
        .filter(|v| v.rule_kind == "no_cycle")
        .map(|v| (v.rule_id.as_str(), v.count))
        .collect()
}

/// A change that closes an import cycle is a new no_cycle violation.
#[test]
fn a_new_cycle_blocks() {
    let repo = committed_api(API_B);
    repo.write("services/api/b.py", API_B_CYCLE);
    let r = run(&repo);
    assert!(r.blocking, "{r:#?}");
    assert_eq!(cycles(&r.new_violations), [("api-acyclic", 2)]);
    assert!(
        r.resolved_violations.is_empty(),
        "{:#?}",
        r.resolved_violations
    );
}

/// A cycle's identity is its members, not its witness: a module joining an
/// existing cycle (whose shortest witness, a <-> b, is unchanged) reads as
/// the grown cycle new and the old one resolved.
#[test]
fn a_grown_cycle_is_new_and_the_old_one_resolved() {
    let repo = committed_api(API_B_CYCLE);
    repo.write("services/api/b.py", API_B_VIA_C);
    repo.write("services/api/c.py", API_C);
    let r = run(&repo);
    assert!(r.blocking, "{r:#?}");
    assert_eq!(cycles(&r.new_violations), [("api-acyclic", 3)]);
    assert_eq!(cycles(&r.resolved_violations), [("api-acyclic", 2)]);
}

/// An unchanged cycle beside an unrelated edit is neither new nor resolved.
#[test]
fn a_pre_existing_cycle_is_neither() {
    let repo = committed_api(API_B_CYCLE);
    repo.write("services/api/internal.py", INTERNAL_EDITED);
    let r = run(&repo);
    assert!(!r.blocking, "{r:#?}");
    assert!(
        r.new_violations.is_empty() && r.resolved_violations.is_empty(),
        "{r:#?}"
    );
}

/// `max_impact` / `max_tests` cut the lists, never the counts.
#[test]
fn caps_cut_rows_not_counts() {
    let repo = committed(WEB_APP_CLEAN);
    repo.write("web/app.py", WEB_APP);
    let full = run(&repo);
    let mut args = ReviewArgs::default();
    args.max_impact = 0;
    args.max_tests = 0;
    let cut = review_vs_rev(repo.path(), "HEAD", &args).expect("review");
    assert_eq!((full.counts.impact, full.counts.tests), (1, 1), "{full:#?}");
    assert_eq!(full.tests.test_files, ["tests/test_pay.py"]);
    assert!(cut.impact.results.is_empty(), "{:#?}", cut.impact);
    assert!(
        cut.tests.tests.is_empty() && cut.tests.test_files.is_empty(),
        "{:#?}",
        cut.tests
    );
    assert_eq!(json_counts(&cut), json_counts(&full));
}

/// A violating file moved: its qnames change, so the violation reads as the
/// old rows resolved and the new rows new (identity is by qname).
#[test]
fn a_moved_violating_file_is_resolved_and_new() {
    let repo = committed(WEB_APP);
    repo.git_mv("web/app.py", "web/checkout.py");
    assert!(repo.root().join("web/checkout.py").is_file());
    let r = run(&repo);
    assert!(r.blocking, "{r:#?}");
    let from = |v: &[Violation]| -> Vec<String> {
        v.iter()
            .flat_map(|v| v.evidence.iter().map(|e| e.from_qname.clone()))
            .collect()
    };
    assert_eq!(
        from(&r.new_violations),
        ["web::checkout", "web::checkout::pay"]
    );
    assert_eq!(from(&r.resolved_violations), ["web::app", PAY]);
}

/// A violating file deleted: its violation is resolved and nothing blocks.
#[test]
fn a_deleted_violating_file_resolves() {
    let repo = committed(WEB_APP);
    repo.remove("web/app.py");
    let r = run(&repo);
    assert!(!r.blocking, "{r:#?}");
    assert!(r.check_errors.is_empty(), "{:#?}", r.check_errors);
    assert_eq!(
        r.resolved_violations.len(),
        1,
        "{:#?}",
        r.resolved_violations
    );
    assert_eq!(r.resolved_violations[0].count, 2);
}

fn json_counts(r: &Review) -> String {
    serde_json::to_string(&r.counts).expect("serialise the counts")
}
