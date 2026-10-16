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
//!
//! CC.6b — `render_markdown`, the PR report the CLI and pyo3 print: the
//! `markdown_*` tests pin the whole document of the new-violation change,
//! the cell escaping, the per-table row cap with its `_(shown of total)_`
//! footer, the resolved-only and empty reviews, and determinism.

mod git_fixture;

use std::collections::BTreeMap;

use git_fixture::GitRepo;
use glia_engine::check::Violation;
use glia_engine::delta::graph_delta_vs_rev;
use glia_engine::identity::Collapse;
use glia_engine::review::{
    MarkdownOptions, Review, ReviewArgs, render_markdown, review, review_vs_rev,
};

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

// ============================================================================
// CC.6b: the markdown PR report
// ============================================================================

fn md(r: &Review) -> String {
    render_markdown(r, &MarkdownOptions::default())
}

/// The whole report of the new-violation change (`markdown_golden`).
const GOLDEN_NEW: &str = "## glia review vs `HEAD`
**1 new violation(s)** | 0 resolved | 3 changed nodes | impact 1 | tests 1 (1 seeds untested) | edges +2 -0

### New violations (blocking)

#### web-no-api-internals (forbid_edge, .glia/overlay.toml:3)

| category | from | to | at | tier |
|---|---|---|---|---|
| IMPORTS | `web::app` | `services::api::internal` | web/app.py:1 | fact |
| CALLS | `web::app::pay` | `services::api::internal::charge` | web/app.py:5 | fact |

### Tests to run

| test | tier | reason | at | covers |
|---|---|---|---|---|
| `tests::test_pay::test_pay` | fact | tests_edge | tests/test_pay.py:4 | `services::api::internal::charge`, `web::app::pay` |

### Impact

| node | depth | via | seed | at | live |
|---|---|---|---|---|---|
| `tests::test_pay::test_pay` | 1 | CALLS | `web::app::pay` | tests/test_pay.py:4 | yes |

### Edge changes

_by tier: fact 2_

#### fact

| +/- | category | from | to | at | emitter |
|---|---|---|---|---|---|
| + | CALLS | `web::app::pay` | `services::api::internal::charge` | web/app.py:5 | graph:calls |
| + | IMPORTS | `web::app` | `services::api::internal` | web/app.py:1 | graph:imports |

### Changed nodes

| change | node | kind | at | seed |
|---|---|---|---|---|
| modified | `web::app` | MODULE | web/app.py:1 | no |
| modified | `web::app::pay` | FUNCTION | web/app.py:4 | yes |
| edge_endpoint | `services::api::internal::charge` | FUNCTION | services/api/internal.py:1 | yes |
";

/// The `#`-headings of a report, in order.
fn headings(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.starts_with('#')).collect()
}

/// The new-violation change renders its headline, the blocking violation
/// with its located fact rows, and the four answer sections, byte for byte;
/// two renders are identical.
#[test]
fn markdown_golden() {
    let repo = committed(WEB_APP_CLEAN);
    repo.write("web/app.py", WEB_APP);
    let r = run(&repo);
    let text = md(&r);
    assert!(
        text.starts_with("## glia review vs `HEAD`\n**1 new violation(s)**"),
        "{text}"
    );
    for want in [
        "### New violations (blocking)",
        "#### web-no-api-internals (forbid_edge, .glia/overlay.toml:3)",
        "| IMPORTS | `web::app` | `services::api::internal` | web/app.py:1 | fact |",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    assert_eq!(text, GOLDEN_NEW);
    assert_eq!(text, md(&r), "two renders differ");
}

/// A `|` in a qname (a Rust closure path) is escaped inside its code span,
/// a backtick widens the span's fence, text escapes `|`, backticks and a
/// `_` outside a word, and a newline in a note is a space.
#[test]
fn markdown_escapes_cells() {
    let repo = committed(WEB_APP_CLEAN);
    repo.write("web/app.py", WEB_APP);
    let mut r = run(&repo);
    r.new_violations[0].evidence[0].from_qname = "web::app::{closure|x|}".to_string();
    r.new_violations[0].evidence[1].to_qname = "a`b".to_string();
    r.new_violations[0].evidence[1].file = Some("web/__init__.py".to_string());
    r.check_errors
        .push(("r|1".to_string(), "line one\nline `two`".to_string()));
    let text = md(&r);
    for want in [
        "| IMPORTS | `web::app::{closure\\|x\\|}` | `services::api::internal` | web/app.py:1 | fact |",
        "| CALLS | `web::app::pay` | `` a`b `` | web/\\_\\_init\\_\\_.py:5 | fact |",
        "### Check errors\n\n| rule | message |\n|---|---|\n| r\\|1 | line one line \\`two\\` |\n\n### Tests to run",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    assert!(
        text.contains("| tests_edge |"),
        "a `_` inside a word stays:\n{text}"
    );
}

/// `max_rows` cuts every table, each cut one followed by its
/// `_(<shown> of <total>)_`; a violation's total is its `count`, and the
/// edge section's the whole edge list.
#[test]
fn markdown_rows_are_capped_with_the_total() {
    let repo = committed(WEB_APP_CLEAN);
    repo.write("web/app.py", WEB_APP);
    let r = run(&repo);
    let mut one = MarkdownOptions::default();
    one.max_rows = 1;
    let text = render_markdown(&r, &one);
    let footers: Vec<&str> = text.lines().filter(|l| l.starts_with("_(")).collect();
    assert_eq!(
        footers,
        ["_(1 of 2)_", "_(1 of 2)_", "_(1 of 3)_"],
        "{text}"
    );
    assert!(
        text.contains("| IMPORTS | `web::app` | `services::api::internal` | web/app.py:1 | fact |\n\n_(1 of 2)_\n\n### Tests to run"),
        "{text}"
    );
    assert!(
        !text.contains(
            "| CALLS | `web::app::pay` | `services::api::internal::charge` | web/app.py:5 | fact |"
        ),
        "{text}"
    );
    assert_eq!(
        headings(&text),
        headings(&md(&r)),
        "a cut keeps every section"
    );

    let mut zero = MarkdownOptions::default();
    zero.max_rows = 0;
    let text = render_markdown(&r, &zero);
    assert!(!text.contains("|---|"), "no table at 0 rows:\n{text}");
    let footers: Vec<&str> = text.lines().filter(|l| l.starts_with("_(")).collect();
    assert_eq!(
        footers,
        [
            "_(0 of 2)_",
            "_(0 of 1)_",
            "_(0 of 1)_",
            "_(0 of 2)_",
            "_(0 of 3)_"
        ],
        "{text}"
    );

    // The engine's own caps: the counts stay the totals.
    let mut args = ReviewArgs::default();
    args.max_tests = 0;
    args.max_impact = 0;
    let cut = review_vs_rev(repo.path(), "HEAD", &args).expect("review");
    let text = md(&cut);
    assert!(
        text.contains(
            "### Tests to run\n\n_(0 of 1)_\n\n### Impact\n\n_(0 of 1)_\n\n### Edge changes"
        ),
        "{text}"
    );
}

/// A resolved-only change: no blocking section, the resolved violation last,
/// its rows and the removed edges marked with the base they are located in.
#[test]
fn markdown_resolved_only() {
    let repo = committed(WEB_APP);
    repo.write("web/app.py", WEB_APP_CLEAN);
    let text = md(&run(&repo));
    assert!(
        text.starts_with("## glia review vs `HEAD`\n**0 new violation(s)** | 1 resolved |"),
        "{text}"
    );
    assert_eq!(
        headings(&text),
        [
            "## glia review vs `HEAD`",
            "### Tests to run",
            "### Impact",
            "### Edge changes",
            "#### fact",
            "### Changed nodes",
            "### Resolved violations",
            "#### web-no-api-internals (forbid_edge, .glia/overlay.toml:3)",
        ],
        "{text}"
    );
    for want in [
        "| - | CALLS | `web::app::pay` | `services::api::internal::charge` | web/app.py:5 (at HEAD) | graph:calls |",
        "| IMPORTS | `web::app` | `services::api::internal` | web/app.py:1 (at HEAD) | fact |\n| CALLS | `web::app::pay` | `services::api::internal::charge` | web/app.py:5 (at HEAD) | fact |\n",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    assert!(text.ends_with("fact |\n"), "{text}");
}

/// A new no_cycle violation names its component's size above its witness.
#[test]
fn markdown_new_cycle() {
    let repo = committed_api(API_B);
    repo.write("services/api/b.py", API_B_CYCLE);
    let text = md(&run(&repo));
    assert!(
        text.contains("#### api-acyclic (no_cycle, .glia/overlay.toml:10)\n\n_(one shortest cycle through the 2 members of its strongly-connected component)_\n\n| category | from | to | at | tier |\n|---|---|---|---|---|\n| IMPORTS | `services::api::a` | `services::api::b` | services/api/a.py:1 | fact |\n| IMPORTS | `services::api::b` | `services::api::a` | services/api/b.py:1 | fact |\n"),
        "{text}"
    );
}

/// No graph change: the headline and the impact absence's note, as a
/// sentence.
#[test]
fn markdown_empty_review() {
    let repo = committed(WEB_APP_CLEAN);
    let r = run(&repo);
    assert!(r.changed.is_empty() && r.edges.is_empty(), "{r:#?}");
    assert_eq!(
        md(&r),
        "## glia review vs `HEAD`\n**0 new violation(s)** | 0 resolved | 0 changed nodes | impact 0 | tests 0 (0 seeds untested) | edges +0 -0\n\nNo graph change vs HEAD.\n"
    );
}

// ============================================================================
// DB.3c: rule and forbid_path violations by subject; the gate is min_tier
// ============================================================================

/// DB.3a's tree (engine/tests/check_rule_invariants.rs) under git: five
/// manifest projects, the root's included, so an unscoped invariant anchors
/// on the root PROJECT. web/app.py calls only the api; the api's
/// `get_order` loads from the store and its `unused` is called by nothing.
const RULE_TREE: [(&str, &str); 9] = [
    ("pyproject.toml", "[project]\nname = \"root\"\n"),
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
    ("store/pyproject.toml", "[project]\nname = \"store\"\n"),
    ("scripts/pyproject.toml", "[project]\nname = \"scripts\"\n"),
    ("web/app.py", SHOW),
    ("services/api/handlers.py", HANDLERS),
    ("store/db.py", "def load(k):\n    return k\n"),
    ("scripts/tool.py", TOOL),
];
const SHOW: &str =
    "from services.api.handlers import get_order\n\n\ndef show(o):\n    return get_order(o)\n";
/// [`SHOW`] plus `f` (line 9), calling the store directly (line 10).
const SHOW_AND_F: &str = "from services.api.handlers import get_order\nfrom store.db import load\n\n\ndef show(o):\n    return get_order(o)\n\n\ndef f(k):\n    return load(k)\n";
const HANDLERS: &str = "from store.db import load\n\n\ndef get_order(o):\n    return load(o)\n\n\ndef unused():\n    return 1\n";
/// [`HANDLERS`] plus a second uncalled handler, `also_unused` (line 12).
const HANDLERS_PLUS: &str = "from store.db import load\n\n\ndef get_order(o):\n    return load(o)\n\n\ndef unused():\n    return 1\n\n\ndef also_unused():\n    return 2\n";
/// scripts/ never touches the store.
const TOOL: &str = "def run():\n    return 0\n\n\ndef other():\n    return 1\n";
/// `run` loads from the store (an IMPORTS and a CALLS, both facts).
const TOOL_LOADS: &str = "from store.db import load\n\n\ndef run():\n    return load(0)\n\n\ndef other():\n    return 1\n";

/// DB.3a's `web-no-store` invariant (`[[constraint]]` on line 3).
const WEB_NO_STORE: &str = r#"version = 2

[[constraint]]
id = "web-no-store"
kind = "invariant"
text = "web never calls the store directly"
rule = "violation(a, b) :- calls(a, b), in_scope(a, \"web\"), in_scope(b, \"store\")."
"#;
/// DB.3a's negation-only `api-called` invariant: its rows derive from no edge.
const API_CALLED: &str = r#"version = 2

[[constraint]]
id = "api-called"
kind = "invariant"
text = "every api handler is called"
rule = "violation(f) :- node(f, \"FUNCTION\"), in_scope(f, \"services/api/handlers.py\"), !called(f). called(f) :- edge(_, f, \"CALLS\")."
"#;
/// DB.3a's `scripts-no-store` invariant.
const SCRIPTS_NO_STORE: &str = r#"version = 2

[[constraint]]
id = "scripts-no-store"
kind = "invariant"
text = "scripts never call the store"
rule = "violation(a, b) :- calls(a, b), in_scope(a, \"scripts\"), in_scope(b, \"store\")."
"#;
/// A version-1 direct forbid_edge, scripts -> store, default categories.
const SCRIPTS_DIRECT: &str = r#"version = 1

[[constraint]]
id = "scripts-direct"
kind = "forbid_edge"
from = "scripts"
to = "store"
"#;
/// An overlay `[[edge]]` (a HEURISTIC CALLS) from `from` to the store's load.
fn declared_call(from: &str) -> String {
    format!("\n[[edge]]\nfrom = \"{from}\"\nto = \"store::db::load\"\ncategory = \"CALLS\"\n")
}

/// [`RULE_TREE`] committed on `main`, with `overlay` (none when empty) and
/// each `(path, text)` of `with` over the tree.
fn committed_rules(overlay: &str, with: &[(&str, &str)]) -> GitRepo {
    let repo = GitRepo::init();
    for (rel, src) in RULE_TREE.iter().chain(with) {
        repo.write(rel, src);
    }
    if !overlay.is_empty() {
        repo.write(".glia/overlay.toml", overlay);
    }
    repo.commit("rules");
    repo
}

/// `(rule id, kind, subject, tier, blocking)` of each violation.
type Classed<'a> = (&'a str, &'a str, Vec<&'a str>, &'a str, bool);

fn classed(v: &[Violation]) -> Vec<Classed<'_>> {
    v.iter()
        .map(|v| {
            (
                v.rule_id.as_str(),
                v.rule_kind,
                v.subject.iter().map(String::as_str).collect(),
                v.tier,
                v.blocking,
            )
        })
        .collect()
}

/// (1) The working tree adds a call web -> store and the version-2 overlay
/// declaring `web-no-store` (the rules are the working tree's, so the base is
/// checked against it too): one new RULE violation, by subject, that blocks.
#[test]
fn rule_violation_added_is_new_and_blocking() {
    let repo = committed_rules("", &[]);
    repo.write(".glia/overlay.toml", WEB_NO_STORE);
    repo.write("web/app.py", SHOW_AND_F);
    let r = run(&repo);
    assert!(r.check_errors.is_empty(), "{:#?}", r.check_errors);
    assert_eq!(
        classed(&r.new_violations),
        [(
            "web-no-store",
            "rule",
            vec!["web::app::f", "store::db::load"],
            "fact",
            true
        )],
        "{:#?}",
        r.new_violations
    );
    assert!(r.blocking, "{r:#?}");
    assert!(
        r.resolved_violations.is_empty(),
        "{:#?}",
        r.resolved_violations
    );
    let v = &r.new_violations[0];
    assert_eq!(rows(v), [("CALLS", "web/app.py:10".to_string(), "fact")]);
    let text = md(&r);
    assert!(
        text.starts_with("## glia review vs `HEAD`\n**1 new violation(s)** | 0 resolved |"),
        "no advisory, no suffix:\n{text}"
    );
    let want = "### New violations (blocking)\n\n#### web-no-store (rule, .glia/overlay.toml:3)\n\n_subject: `web::app::f`, `store::db::load` - tier fact at web/app.py:9_\n\n| category | from | to | at | tier |\n|---|---|---|---|---|\n| CALLS | `web::app::f` | `store::db::load` | web/app.py:10 | fact |\n";
    assert!(text.contains(want), "missing {want:?} in:\n{text}");
    assert!(!text.contains("### New violations (advisory"), "{text}");
}

/// (2) A negation-only rule row derives from no edge, so it has no evidence
/// row to compare: its subject is its identity. The change adds a second
/// uncalled handler: exactly that subject is new, the base's `unused` is
/// neither; the change the other way resolves it.
#[test]
fn negation_only_violation_is_classed() {
    let repo = committed_rules(API_CALLED, &[]);
    repo.write("services/api/handlers.py", HANDLERS_PLUS);
    let r = run(&repo);
    assert!(r.check_errors.is_empty(), "{:#?}", r.check_errors);
    assert_eq!(
        classed(&r.new_violations),
        [(
            "api-called",
            "rule",
            vec!["services::api::handlers::also_unused"],
            "fact",
            true
        )],
        "{:#?}",
        r.new_violations
    );
    let v = &r.new_violations[0];
    assert!(v.evidence.is_empty() && v.count == 0, "{v:#?}");
    assert!(!v.caveats.is_empty(), "Q5: the negation read CALLS: {v:#?}");
    assert!(r.blocking, "{r:#?}");
    assert!(
        r.resolved_violations.is_empty(),
        "{:#?}",
        r.resolved_violations
    );
    let text = md(&r);
    assert!(
        text.contains("#### api-called (rule, .glia/overlay.toml:3)\n\n_subject: `services::api::handlers::also_unused` - tier fact at services/api/handlers.py:12_\n\n⚠ "),
        "{text}"
    );
    let caveats = text.lines().filter(|l| l.starts_with("⚠ ")).count();
    assert_eq!(caveats, v.caveats.len().min(5), "{text}");

    let back = committed_rules(API_CALLED, &[("services/api/handlers.py", HANDLERS_PLUS)]);
    back.write("services/api/handlers.py", HANDLERS);
    let r = run(&back);
    assert!(r.new_violations.is_empty(), "{:#?}", r.new_violations);
    assert!(!r.blocking, "{r:#?}");
    assert_eq!(
        classed(&r.resolved_violations),
        [(
            "api-called",
            "rule",
            vec!["services::api::handlers::also_unused"],
            "fact",
            true
        )],
        "{:#?}",
        r.resolved_violations
    );
}

/// (3) A new violation below its rule's min_tier (here heuristic: an overlay
/// edge) is reported, under its own heading, and does not block (0.5.2 Q2).
#[test]
fn advisory_new_violation_does_not_block() {
    let repo = committed_rules(SCRIPTS_NO_STORE, &[]);
    repo.write(
        ".glia/overlay.toml",
        &format!("{SCRIPTS_NO_STORE}{}", declared_call("scripts::tool::run")),
    );
    let r = run(&repo);
    assert!(r.check_errors.is_empty(), "{:#?}", r.check_errors);
    assert_eq!(
        classed(&r.new_violations),
        [(
            "scripts-no-store",
            "rule",
            vec!["scripts::tool::run", "store::db::load"],
            "heuristic",
            false
        )],
        "{:#?}",
        r.new_violations
    );
    assert!(!r.blocking, "{r:#?}");
    assert_eq!(r.counts.new_violations, 1);
    let text = md(&r);
    assert!(
        text.starts_with(
            "## glia review vs `HEAD`\n**1 new violation(s)** (1 advisory) | 0 resolved |"
        ),
        "{text}"
    );
    assert!(
        text.contains("### New violations (advisory: below the rule's min_tier)\n\n#### scripts-no-store (rule, .glia/overlay.toml:3)\n\n_subject: `scripts::tool::run`, `store::db::load` - tier heuristic at scripts/tool.py:1_\n\n| category | from | to | at | tier |\n|---|---|---|---|---|\n| CALLS | `scripts::tool::run` | `store::db::load` | .glia/overlay.toml:"),
        "{text}"
    );
    assert!(!text.contains("### New violations (blocking)"), "{text}");
}

/// fx1 (engine/tests/check_transitive.rs): web/ reaches the api's internals
/// through shared/, and its public facade through the allowed `via`.
const FX1: [(&str, &str); 7] = [
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("shared/pyproject.toml", "[project]\nname = \"shared\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
    ("web/app.py", FX1_WEB),
    (
        "shared/util.py",
        "from services.api.internal.db import query\n\n\ndef helper():\n    return query()\n",
    ),
    (
        "services/api/public/facade.py",
        "from services.api.internal.db import write\n\n\ndef get():\n    return write()\n",
    ),
    (
        "services/api/internal/db.py",
        "def query():\n    return 1\n\n\ndef write():\n    return 2\n",
    ),
];
const FX1_WEB: &str = "from shared.util import helper\nfrom services.api.public.facade import get\n\n\ndef page():\n    return helper()\n\n\ndef other():\n    return get()\n";
/// [`FX1_WEB`] with `page` also calling `query` directly: a one-hop FACT
/// path (and a one-hop import of the db module) to the two existing hits.
const FX1_WEB_DIRECT: &str = "from shared.util import helper\nfrom services.api.public.facade import get\nfrom services.api.internal.db import query\n\n\ndef page():\n    return helper() + query()\n\n\ndef other():\n    return get()\n";
/// [`FX1_WEB`] plus `third`, calling the internal `write` past the facade.
const FX1_WEB_WRITE: &str = "from shared.util import helper\nfrom services.api.public.facade import get\nfrom services.api.internal.db import write\n\n\ndef page():\n    return helper()\n\n\ndef other():\n    return get()\n\n\ndef third():\n    return write()\n";
/// DB.3b's acceptance rule: web never reaches services/api except through
/// its public facade.
const VIA: &str = r#"version = 2

[[constraint]]
id = "web-no-api"
kind = "forbid_edge"
from = "web"
to = "services/api"
transitive = true
via = ["services/api/public"]
"#;

fn committed_fx1() -> GitRepo {
    let repo = GitRepo::init();
    for (rel, src) in FX1 {
        repo.write(rel, src);
    }
    repo.write(".glia/overlay.toml", VIA);
    repo.commit("fx1");
    repo
}

/// (4) A forbid_path violation is one hit, identified by the node reached: a
/// shorter path into an existing hit changes its witness, not its identity
/// (neither new nor resolved); a path into a node the base did not reach is
/// that one hit, new.
#[test]
fn forbid_path_witness_change_is_not_new() {
    let repo = committed_fx1();
    repo.write("web/app.py", FX1_WEB_DIRECT);
    let r = run(&repo);
    assert!(r.check_errors.is_empty(), "{:#?}", r.check_errors);
    assert!(
        r.new_violations.is_empty() && r.resolved_violations.is_empty(),
        "new {:#?}\nresolved {:#?}",
        r.new_violations,
        r.resolved_violations
    );
    assert!(!r.blocking, "{r:#?}");
    assert!(
        r.edges.iter().any(|e| e.change == "added"
            && e.from_qname == "web::app::page"
            && e.to_qname == "services::api::internal::db::query"),
        "the shorter path is in the graph: {:#?}",
        r.edges
    );

    let repo = committed_fx1();
    repo.write("web/app.py", FX1_WEB_WRITE);
    let r = run(&repo);
    assert!(r.check_errors.is_empty(), "{:#?}", r.check_errors);
    assert_eq!(
        classed(&r.new_violations),
        [(
            "web-no-api",
            "forbid_path",
            vec!["services::api::internal::db::write"],
            "fact",
            true
        )],
        "{:#?}",
        r.new_violations
    );
    assert!(r.blocking, "{r:#?}");
    assert!(
        r.resolved_violations.is_empty(),
        "{:#?}",
        r.resolved_violations
    );
    let text = md(&r);
    assert!(
        text.contains("#### web-no-api (forbid_path, .glia/overlay.toml:3)\n\n_subject: `services::api::internal::db::write` - tier fact at services/api/internal/db.py:5_\n\n| category | from | to | at | tier |\n|---|---|---|---|---|\n| CALLS | `web::app::third` | `services::api::internal::db::write` | web/app.py:15 | fact |\n"),
        "{text}"
    );
}

/// (6) A direct forbid_edge the base already broke with facts, to which the
/// change adds only a declared (heuristic) edge: the new violation is that
/// one row, its tier the row's, and its gate recomputed from it, so it does
/// not block.
#[test]
fn reduced_violation_reads_its_own_tier() {
    let repo = committed_rules(SCRIPTS_DIRECT, &[("scripts/tool.py", TOOL_LOADS)]);
    repo.write(
        ".glia/overlay.toml",
        &format!("{SCRIPTS_DIRECT}{}", declared_call("scripts::tool::other")),
    );
    let r = run(&repo);
    assert!(r.check_errors.is_empty(), "{:#?}", r.check_errors);
    assert_eq!(
        classed(&r.new_violations),
        [("scripts-direct", "forbid_edge", vec![], "heuristic", false)],
        "{:#?}",
        r.new_violations
    );
    let v = &r.new_violations[0];
    assert_eq!(v.count, 1, "{v:#?}");
    let got: Vec<(&str, &str, &str, &str)> = v
        .evidence
        .iter()
        .map(|e| {
            (
                e.category,
                e.from_qname.as_str(),
                e.to_qname.as_str(),
                e.tier,
            )
        })
        .collect();
    assert_eq!(
        got,
        [(
            "CALLS",
            "scripts::tool::other",
            "store::db::load",
            "heuristic"
        )]
    );
    assert!(!r.blocking, "{r:#?}");
    assert!(
        r.resolved_violations.is_empty(),
        "{:#?}",
        r.resolved_violations
    );
    let text = md(&r);
    assert!(
        text.contains("### New violations (advisory: below the rule's min_tier)\n\n#### scripts-direct (forbid_edge, .glia/overlay.toml:3)\n\n| category"),
        "a forbid_edge violation renders no subject line:\n{text}"
    );
}

/// DD.10 handoff: one repo whose web/ manifest starts depending on the api
/// project (`manifest_name`, a FACT identity class of the dependency and the
/// PROJECT it names). The added dependency is an impact seed that stands for
/// the class; its `also_in` names the repo by the working tree's label, as
/// `tests_for_delta` does, not `repo#<id % 1000>` (HEAD before DB.3c:
/// `repo#873` beside the label `repo`).
#[test]
fn impact_identity_names_the_repo_by_its_label() {
    let repo = GitRepo::init();
    repo.write("web/pyproject.toml", "[project]\nname = \"web\"\n");
    repo.write("services/api/pyproject.toml", "[project]\nname = \"api\"\n");
    repo.write("web/app.py", WEB_APP_CLEAN);
    repo.write("services/api/internal.py", INTERNAL);
    repo.commit("clean");
    repo.write(
        "web/pyproject.toml",
        "[project]\nname = \"web\"\ndependencies = [\"api\"]\n",
    );
    let rev = graph_delta_vs_rev(repo.path(), "HEAD").expect("delta");
    let label = rev
        .after
        .repo_labels
        .values()
        .next()
        .cloned()
        .expect("the working tree's label");
    let r = review(&rev, &ReviewArgs::default()).expect("review");
    let seed = r
        .impact
        .seeds
        .iter()
        .find(|s| s.qname == "package:pypi:api")
        .expect("the added dependency is a seed");
    let v = serde_json::to_value(seed).expect("serialises");
    let also = v["identity"]["also_in"]
        .as_array()
        .expect("the seed stands for its class");
    let repos: Vec<&str> = also.iter().filter_map(|a| a["repo"].as_str()).collect();
    assert_eq!(repos, [label.as_str()], "{v:#}");
    assert_eq!(also[0]["qname"], "project:services/api", "{v:#}");

    // A caller's own labels win; `Collapse::off()` is the raw answer.
    let mut args = ReviewArgs::default();
    args.blast.collapse = Collapse::off();
    let raw = review(&rev, &args).expect("review");
    assert!(
        !serde_json::to_string(&raw.impact)
            .expect("serialises")
            .contains("also_in"),
        "{:#?}",
        raw.impact
    );
    let mut args = ReviewArgs::default();
    args.blast.collapse = Collapse::with_labels(&BTreeMap::from([(
        rev.after.merged.graphs[0].repo.0,
        "mine".to_string(),
    )]));
    let own = review(&rev, &args).expect("review");
    assert!(
        serde_json::to_string(&own.impact)
            .expect("serialises")
            .contains("\"repo\":\"mine\""),
        "{:#?}",
        own.impact
    );
}
