//! `overlay try` judges rules by the check delta (DB.9b): every build that
//! declares a rule is checked, its violations keyed by identity, and a
//! `[[constraint]]` stanza is judged on WITH alone (holds / violated / error /
//! orphaned / duplicate, plus the scopes its rule names that hold no node),
//! with no leave-one-out build; a graph stanza that adds or hides a declared
//! rule's violation is review, and the candidate's verdict combines its graph
//! and rule parts.
//!
//! The tree is a small Python workspace: a PROJECT at the root and at web/,
//! store/, scripts/; `web::app::show` calls `store::db::load`, and the base
//! overlay forbids scripts -> store (`scripts-no-store`).
//!
//! Pre-fix baseline (HEAD bda10135, `GLIA_NO_PERSIST=1 ./target/debug/glia
//! overlay try`): these tests do not compile (`TryReport` has no `rules`,
//! `StanzaTrial` no `rule` / `rules`). On the CLI the seven-stanza candidate
//! reads `builds: 9` and `[overlay] try repo=<py> stanzas=7 builds=9 verdicts
//! keep=1 review=0 drop=6 gaps 4→3 closed=0 surface=cli`: every
//! `[[constraint]]` stanza `drop` with an empty marginal, and `edge#1` `keep`
//! although it breaks `scripts-no-store`. The rule-only candidate reads
//! `drop`, `builds: 3`. The capped tree's one [[edge]] reads `| edge#1 | keep
//! | — | — | CALLS +1 | dead_symbol -1 |`, `builds: 2`, while web-no-store
//! already has 104 rows (100 listed).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_engine::gaps::{DROP, KEEP, REVIEW};
use glia_engine::overlay_loop::{
    MAX_RULE_HITS, RULE_DUPLICATE, RULE_ERROR, RULE_HOLDS, RULE_ORPHANED, RULE_VIOLATED,
    StanzaTrial, TryOptions, TryReport, try_candidate,
};

/// The base overlay: one forbid_edge, scripts -> store.
const BASE_OVERLAY: &str = r#"version = 1

[[constraint]]
id = "scripts-no-store"
kind = "forbid_edge"
from = "scripts"
to = "store"
origin = "human"
"#;

const C1: &str = r#"[[constraint]]
id = "web-no-save"
kind = "invariant"
text = "web never saves"
rule = "violation(a) :- edge(a, b, \"CALLS\"), in_scope(a, \"web\"), qname(b, \"store::db::save\")."
"#;

const C2: &str = r#"[[constraint]]
id = "web-no-store-calls"
kind = "invariant"
text = "web calls nothing in store"
rule = "violation(a, b) :- edge(a, b, \"CALLS\"), in_scope(a, \"web\"), in_scope(b, \"store\")."
"#;

const C3: &str = r#"[[constraint]]
id = "webs-no-store"
kind = "invariant"
text = "webs calls nothing in store"
rule = "violation(a, b) :- edge(a, b, \"CALLS\"), in_scope(a, \"webs\"), in_scope(b, \"store\")."
"#;

const C4: &str = r#"[[constraint]]
id = "rotten"
kind = "invariant"
text = "no routee"
rule = "violation(r) :- node(r, \"ROUTEE\")."
"#;

const C5: &str = r#"[[constraint]]
id = "view-no-store"
kind = "forbid_edge"
from = "web/view.py"
to = "store"
anchor = "web::view::render"
"#;

const C6: &str = r#"[[constraint]]
id = "orphan"
kind = "forbid_edge"
from = "webs"
to = "store"
"#;

const EDGE: &str = r#"[[edge]]
from = "scripts::tool::run"
to = "store::db::save"
category = "CALLS"
"#;

/// c1..c6 then the [[edge]], version-less.
fn candidate() -> String {
    [C1, C2, C3, C4, C5, C6, EDGE].join("\n")
}

/// A fresh, empty temp dir for one test.
fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("glia_db9b_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(p, text).expect("write");
}

/// The Python tree at `root`, with `overlay` as its `.glia/overlay.toml`
/// (none when `None`).
fn py_tree(root: &Path, overlay: Option<&str>) -> String {
    write(root, "pyproject.toml", "[project]\nname = \"py\"\n");
    for d in ["web", "store", "scripts"] {
        write(
            root,
            &format!("{d}/pyproject.toml"),
            &format!("[project]\nname = \"{d}\"\n"),
        );
    }
    write(
        root,
        "web/app.py",
        "from store.db import load\n\n\ndef show(k):\n    return load(k)\n",
    );
    write(root, "web/view.py", "def render(x):\n    return x\n");
    write(
        root,
        "store/db.py",
        "def load(k):\n    return k\n\n\ndef save(k, v):\n    return v\n",
    );
    write(root, "scripts/tool.py", "def run():\n    return 1\n");
    if let Some(text) = overlay {
        write(root, ".glia/overlay.toml", text);
    }
    root.to_str().expect("utf-8 temp path").to_string()
}

fn verdicts(r: &TryReport) -> Vec<(String, &'static str)> {
    r.stanzas
        .iter()
        .map(|s| (s.stanza.clone(), s.verdict))
        .collect()
}

fn stanza<'r>(r: &'r TryReport, handle: &str) -> &'r StanzaTrial {
    r.stanzas
        .iter()
        .find(|s| s.stanza == handle)
        .unwrap_or_else(|| panic!("no stanza {handle}: {r:#?}"))
}

fn statuses(r: &TryReport) -> Vec<&'static str> {
    r.stanzas
        .iter()
        .filter_map(|s| s.rule.as_ref().map(|t| t.status))
        .collect()
}

fn map(pairs: &[(&'static str, i64)]) -> BTreeMap<&'static str, i64> {
    pairs.iter().copied().collect()
}

fn counts(pairs: &[(&'static str, usize)]) -> BTreeMap<&'static str, usize> {
    pairs.iter().copied().collect()
}

/// The asserts of (1), shared with the no-leave-one-out run where they hold.
fn assert_report_rules(report: &TryReport) {
    let rules = report.rules.as_ref().expect("a rule part");
    assert_eq!(rules.delta.added_total, 2, "{rules:#?}");
    assert_eq!(rules.delta.blocking, 1, "{rules:#?}");
    assert_eq!(rules.delta.resolved_total, 0, "{rules:#?}");
    assert_eq!(rules.delta.errors.len(), 1, "{rules:#?}");
    assert_eq!(rules.delta.errors[0].0, "rotten");
    assert!(rules.delta.errors[0].1.contains("ROUTEE"), "{rules:#?}");
    assert_eq!(
        rules.statuses,
        counts(&[
            (RULE_HOLDS, 3),
            (RULE_VIOLATED, 1),
            (RULE_ERROR, 1),
            (RULE_ORPHANED, 1)
        ])
    );
    assert_eq!(rules.graph_verdict, REVIEW);
    assert_eq!(rules.rule_verdict, Some(REVIEW));
    assert_eq!(report.verdict, REVIEW);
    assert_eq!(
        report.delta.edges,
        map(&[("CALLS", 1)]),
        "{:#?}",
        report.delta
    );
    assert_eq!(
        report.delta.gaps,
        map(&[("dead_symbol", -1)]),
        "{:#?}",
        report.delta
    );
    assert!(report.delta.nodes.is_empty(), "{:#?}", report.delta);
}

#[test]
fn rule_stanzas_are_judged_by_the_check_delta() {
    let root = tmp("judged");
    let repo = py_tree(&root, Some(BASE_OVERLAY));
    let report = try_candidate(
        std::slice::from_ref(&repo),
        &candidate(),
        &TryOptions::default(),
    )
    .expect("try");

    // BASE, WITH, WITHOUT(edge#1): no build for a rule stanza.
    assert_eq!(report.builds, 3, "{report:#?}");
    let want: Vec<(String, &str)> = [
        ("constraint#1", KEEP),
        ("constraint#2", REVIEW),
        ("constraint#3", REVIEW),
        ("constraint#4", DROP),
        ("constraint#5", KEEP),
        ("constraint#6", DROP),
        ("edge#1", REVIEW),
    ]
    .into_iter()
    .map(|(s, v)| (s.to_string(), v))
    .collect();
    assert_eq!(verdicts(&report), want, "{report:#?}");
    assert_eq!(
        statuses(&report),
        [
            RULE_HOLDS,
            RULE_VIOLATED,
            RULE_HOLDS,
            RULE_ERROR,
            RULE_HOLDS,
            RULE_ORPHANED
        ]
    );

    let c2 = stanza(&report, "constraint#2")
        .rule
        .as_ref()
        .expect("a rule trial");
    assert_eq!(c2.rule_id, "web-no-store-calls");
    assert_eq!(c2.violations, 1);
    assert_eq!(c2.tiers, counts(&[("fact", 1)]));
    assert_eq!(c2.blocking, 1);
    assert_eq!(c2.hits.len(), 1, "{c2:#?}");
    let hit = &c2.hits[0];
    assert_eq!(hit.rule_id, "web-no-store-calls");
    assert_eq!(hit.rule_kind, "rule");
    assert_eq!(hit.key, "web::app::show, store::db::load");
    assert_eq!(hit.tier, "fact");
    assert!(hit.blocking);
    assert_eq!(hit.file.as_deref(), Some("web/app.py"));
    assert_eq!(hit.line, Some(4));
    assert!(c2.empty_scopes.is_empty(), "{c2:#?}");
    assert!(c2.error.is_none(), "{c2:#?}");

    let c3 = stanza(&report, "constraint#3")
        .rule
        .as_ref()
        .expect("trial");
    assert_eq!(c3.status, RULE_HOLDS);
    assert_eq!(c3.empty_scopes, ["webs"]);
    let c4 = stanza(&report, "constraint#4")
        .rule
        .as_ref()
        .expect("trial");
    assert!(
        c4.error
            .as_deref()
            .is_some_and(|e| e.contains("unknown node kind `ROUTEE`") && !e.contains('\n')),
        "{c4:#?}"
    );
    let c1 = stanza(&report, "constraint#1")
        .rule
        .as_ref()
        .expect("trial");
    assert_eq!((c1.violations, c1.blocking), (0, 0));
    assert!(c1.hits.is_empty() && c1.empty_scopes.is_empty(), "{c1:#?}");

    for i in 1..=6 {
        let s = stanza(&report, &format!("constraint#{i}"));
        assert!(s.marginal.is_empty(), "{s:#?}");
        assert!(s.closes.is_empty(), "{s:#?}");
        assert!(s.rules.is_empty(), "{s:#?}");
    }

    let edge = stanza(&report, "edge#1");
    assert!(edge.rule.is_none());
    assert_eq!(edge.marginal.edges, map(&[("CALLS", 1)]), "{edge:#?}");
    assert_eq!(edge.marginal.gaps, map(&[("dead_symbol", -1)]), "{edge:#?}");
    assert_eq!(edge.rules.added_total, 1, "{edge:#?}");
    assert_eq!(edge.rules.resolved_total, 0, "{edge:#?}");
    assert_eq!(edge.rules.blocking, 0, "{edge:#?}");
    assert!(edge.rules.errors.is_empty(), "{edge:#?}");
    let added = &edge.rules.added[0];
    assert_eq!(added.rule_id, "scripts-no-store");
    assert_eq!(added.rule_kind, "forbid_edge");
    assert_eq!(added.key, "scripts::tool::run -[CALLS]-> store::db::save");
    assert_eq!(added.tier, "heuristic");
    assert!(!added.blocking);
    assert_eq!(added.file.as_deref(), Some(".glia/overlay.toml"));
    assert!(added.line.is_some(), "{added:#?}");

    assert_report_rules(&report);
    std::fs::remove_dir_all(root).ok();
}

const CHILD_ENV: &str = "GLIA_DB9B_CHILD_TRY";

/// The child half of [`markers`]: a no-op unless the parent set
/// [`CHILD_ENV`], in which case it tries [`candidate`] on that repo so its
/// stderr (with `--nocapture`) carries the markers.
#[test]
fn child_try_rules_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        try_candidate(&[dir], &candidate(), &TryOptions::default()).expect("try");
    }
}

#[test]
fn markers() {
    let root = tmp("markers");
    let repo = py_tree(&root, Some(BASE_OVERLAY));
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "--exact",
            "child_try_rules_for_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, &repo)
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "child try failed: {stderr}");
    let lines: Vec<&str> = stderr.lines().collect();
    let try_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.starts_with("[overlay] try "))
        .collect();
    assert_eq!(
        try_lines,
        [format!(
            "[overlay] try repo={repo} stanzas=7 builds=3 verdicts keep=2 review=3 drop=2 gaps 4→3 closed=0 surface=engine"
        )]
    );
    let rules_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.starts_with("[overlay] try-rules"))
        .collect();
    assert_eq!(
        rules_lines,
        [format!(
            "[overlay] try-rules repo={repo} rules=6 (holds=3 violated=1 error=1 orphaned=1 duplicate=0) empty_scopes=1 violations +2 -0 blocking=1 errors=1 surface=engine"
        )]
    );
    // After the try line; each of the three builds runs check once.
    let at = |prefix: &str| lines.iter().position(|l| l.starts_with(prefix));
    assert!(at("[overlay] try ") < at("[overlay] try-rules"), "{stderr}");
    let checks = lines
        .iter()
        .filter(|l| l.starts_with("[check] rules="))
        .count();
    assert_eq!(checks, 3, "{stderr}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn rule_only_candidate_builds_twice() {
    let root = tmp("only");
    let repo = py_tree(&root, Some(BASE_OVERLAY));
    let report =
        try_candidate(std::slice::from_ref(&repo), C1, &TryOptions::default()).expect("try");
    assert_eq!(report.builds, 2, "{report:#?}");
    assert_eq!(report.verdict, KEEP);
    assert_eq!(
        verdicts(&report),
        [("constraint#1".to_string(), KEEP)],
        "{report:#?}"
    );
    let rules = report.rules.as_ref().expect("a rule part");
    assert!(rules.delta.is_empty(), "{rules:#?}");
    assert_eq!(rules.graph_verdict, DROP);
    assert_eq!(rules.rule_verdict, Some(KEEP));
    assert_eq!(rules.statuses, counts(&[(RULE_HOLDS, 1)]));
    let json = serde_json::to_value(&report.stanzas[0]).expect("json");
    assert_eq!(json["rule"]["status"], "holds", "{json}");
    assert!(json.get("rules").is_none(), "{json}");
    assert!(json["rule"].get("error").is_none(), "{json}");
    assert!(json["rule"].get("empty_scopes").is_none(), "{json}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn duplicate_rule_is_dropped() {
    let root = tmp("dup");
    let repo = py_tree(&root, Some(BASE_OVERLAY));
    // The base rule again, re-ordered: the same body by value.
    let dup = "[[constraint]]\nid = \"scripts-no-store\"\nkind = \"forbid_edge\"\nto = \"store\"\nfrom = \"scripts\"\norigin = \"human\"\n";
    let report =
        try_candidate(std::slice::from_ref(&repo), dup, &TryOptions::default()).expect("try");
    // WITH's text is BASE's: one build.
    assert_eq!(report.builds, 1, "{report:#?}");
    assert_eq!(statuses(&report), [RULE_DUPLICATE]);
    assert_eq!(report.stanzas[0].verdict, DROP);
    assert_eq!(report.verdict, DROP);
    let rules = report.rules.as_ref().expect("a rule part");
    assert_eq!(rules.statuses, counts(&[(RULE_DUPLICATE, 1)]));
    assert_eq!(rules.rule_verdict, Some(DROP));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn no_leave_one_out_still_judges_rules() {
    let root = tmp("nolo");
    let repo = py_tree(&root, Some(BASE_OVERLAY));
    let mut opts = TryOptions::default();
    opts.leave_one_out = false;
    let report = try_candidate(std::slice::from_ref(&repo), &candidate(), &opts).expect("try");
    assert_eq!(report.builds, 2, "{report:#?}");
    assert!(report.stanzas.is_empty(), "{report:#?}");
    assert_report_rules(&report);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn rule_free_try_is_unchanged() {
    let root = tmp("free");
    let repo = py_tree(&root, None);
    let report =
        try_candidate(std::slice::from_ref(&repo), EDGE, &TryOptions::default()).expect("try");
    assert_eq!(report.builds, 2, "{report:#?}");
    assert_eq!(report.verdict, KEEP);
    assert!(report.rules.is_none(), "{report:#?}");
    let json = serde_json::to_value(&report).expect("json");
    assert!(json.get("rules").is_none(), "{json}");
    let s = &json["stanzas"][0];
    assert!(s.get("rule").is_none() && s.get("rules").is_none(), "{s}");
    // The key order a rule-free try has always had.
    let keys: Vec<&str> = json
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        [
            "base", "builds", "closed", "delta", "stanzas", "verdict", "with"
        ]
    );
    let stanza_keys: Vec<&str> = s
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    let mut sorted = stanza_keys.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, ["closes", "gaps", "marginal", "stanza", "verdict"]);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn deterministic() {
    let root = tmp("det");
    let repo = py_tree(&root, Some(BASE_OVERLAY));
    let run = || {
        let r = try_candidate(
            std::slice::from_ref(&repo),
            &candidate(),
            &TryOptions::default(),
        )
        .expect("try");
        serde_json::to_string(&r).expect("json")
    };
    let (a, b) = (run(), run());
    assert_eq!(a, b);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn capped_rule_moves_still_count() {
    let root = tmp("capped");
    let base = BASE_OVERLAY
        .replace("scripts-no-store", "web-no-store")
        .replace("from = \"scripts\"", "from = \"web\"");
    assert_ne!(base, BASE_OVERLAY);
    let repo = py_tree(&root, Some(&base));
    let mut many = String::from("from store.db import load\n\n");
    for i in 0..=100 {
        many.push_str(&format!("\ndef f{i}(k):\n    return load(k)\n"));
    }
    write(&root, "web/many.py", &many);
    // A heuristic overlay edge sorts past check's 100 listed (fact) rows.
    let cand =
        "[[edge]]\nfrom = \"web::many::f0\"\nto = \"store::db::save\"\ncategory = \"CALLS\"\n";
    let report =
        try_candidate(std::slice::from_ref(&repo), cand, &TryOptions::default()).expect("try");
    let edge = stanza(&report, "edge#1");
    assert_eq!(edge.rules.added_total, 1, "{edge:#?}");
    assert_eq!(edge.rules.resolved_total, 1, "{edge:#?}");
    for hit in edge.rules.added.iter().chain(&edge.rules.resolved) {
        assert_eq!(hit.rule_id, "web-no-store");
        assert!(
            hit.key.ends_with("row(s) past check::MAX_EVIDENCE"),
            "{hit:#?}"
        );
        assert!(hit.file.is_none() && hit.line.is_none(), "{hit:#?}");
    }
    assert_eq!(edge.rules.added[0].key, "5 row(s) past check::MAX_EVIDENCE");
    assert_eq!(
        edge.rules.resolved[0].key,
        "4 row(s) past check::MAX_EVIDENCE"
    );
    assert_eq!(edge.verdict, REVIEW);
    assert_eq!(report.verdict, REVIEW);
    let rules = report.rules.as_ref().expect("a rule part");
    assert_eq!(rules.rule_verdict, None);
    assert_eq!(rules.graph_verdict, REVIEW);
    assert!(rules.statuses.is_empty());
    assert!(edge.rules.added.len() <= MAX_RULE_HITS);
    std::fs::remove_dir_all(root).ok();
}
