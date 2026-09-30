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
