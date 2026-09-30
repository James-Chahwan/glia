//! CC.10a: hotspots rank each module / symbol by git churn and by PageRank
//! centrality, and order rows by the two ranks together. The tree: `helper`
//! in core/util.py is called by five functions across three files
//! (core/util.py `wrap`, api/a.py, api/b.py); scripts/once.py calls and is
//! called by nothing. History is written with
//! `code_domain::snapshots::write_history` before the build, no git needed.

use std::path::Path;

use glia_code_domain::snapshots::{BlameFile, HistoryCommit, HistoryFile, HistoryMeta, write_history};
use glia_engine::generate_one;
use glia_engine::hotspots::{HotspotArgs, Hotspots, hotspots};
use glia_graph::MergedGraph;

const UTIL_PY: &str = "\"\"\"Shared helpers.\"\"\"\n\n\ndef helper(x):\n    total = x + 1\n    total = total * 2\n    return total\n\n\ndef wrap(x):\n    return helper(x) + 1\n";
const A_PY: &str = "from core.util import helper\n\n\ndef get_a(x):\n    return helper(x)\n\n\ndef list_a(xs):\n    return [helper(x) for x in xs]\n";
const B_PY: &str = "from core.util import helper\n\n\ndef get_b(x):\n    return helper(x) * 3\n\n\ndef list_b(xs):\n    return [helper(x) - 1 for x in xs]\n";
const ONCE_PY: &str = "def run_once(n):\n    acc = 0\n    for i in range(n):\n        acc += i\n    return acc\n";
const UTIL_TEST_PY: &str = "from core.util import helper\n\n\ndef test_helper():\n    assert helper(1) == 4\n";

const UTIL: &str = "core/util.py";
const A: &str = "api/a.py";
const B: &str = "api/b.py";
const ONCE: &str = "scripts/once.py";
const UTIL_TEST: &str = "core/util_test.py";

/// Commit `i`'s time: one day apart, commit 8 the newest.
fn t(i: i64) -> i64 {
    1_767_225_600 + i * 86_400
}

fn commit(i: i64, paths: &[&str]) -> HistoryCommit {
    HistoryCommit {
        c: format!("{i:02}{}", "0".repeat(38)),
        t: t(i),
        files: paths.iter().map(|p| HistoryFile { p: p.to_string(), a: Some(2), d: Some(1), from: None }).collect(),
    }
}

/// The acceptance history, newest first: once.py in 8 commits, util.py in 6,
/// a.py in 4, b.py in 1.
fn commits() -> Vec<HistoryCommit> {
    vec![
        commit(8, &[ONCE]),
        commit(7, &[ONCE]),
        commit(6, &[ONCE, UTIL]),
        commit(5, &[ONCE, UTIL, A]),
        commit(4, &[ONCE, UTIL, A]),
        commit(3, &[ONCE, UTIL, A]),
        commit(2, &[ONCE, UTIL]),
        commit(1, &[ONCE, UTIL, A, B]),
    ]
}

/// helper's lines (4..=7) were last changed at three times, wrap's at one;
/// get_a's (4..=5) at two, list_a's at one.
fn blame() -> Vec<BlameFile> {
    vec![
        BlameFile { p: UTIL.into(), runs: vec![[1, 4, t(1)], [5, 5, t(3)], [6, 7, t(6)], [8, 11, t(1)]] },
        BlameFile { p: A.into(), runs: vec![[1, 4, t(1)], [5, 5, t(4)], [6, 9, t(1)]] },
    ]
}

/// A fresh tree with the five sources, plus `extra` files.
fn tree(extra: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    for (p, text) in [(UTIL, UTIL_PY), (A, A_PY), (B, B_PY), (ONCE, ONCE_PY)].iter().chain(extra) {
        let path = d.path().join(p);
        std::fs::create_dir_all(path.parent().expect("parent")).unwrap();
        std::fs::write(path, text).unwrap();
    }
    d
}

fn snapshot(root: &Path, commits: &[HistoryCommit], blame: &[BlameFile]) {
    let head = commits.first().map_or_else(|| "0".repeat(40), |c| c.c.clone());
    write_history(root, HistoryMeta::new(head, 2000, None, String::new()), commits, blame).expect("write snapshot");
}

fn build(root: &Path) -> MergedGraph {
    generate_one(&root.to_string_lossy()).expect("build").merged
}

/// The acceptance tree with its history, built.
fn acceptance() -> MergedGraph {
    let d = tree(&[]);
    snapshot(d.path(), &commits(), &blame());
    build(d.path())
}

fn qnames(rows: &[glia_engine::hotspots::Hotspot]) -> Vec<&str> {
    rows.iter().map(|h| h.qname.as_str()).collect()
}

/// The uncalled file with the most churn is not the top hotspot: util.py
/// (churn rank 2, centrality rank 1) is, and b.py (1 commit) ranks nowhere.
#[test]
fn module_rows_join_churn_and_centrality() {
    let m = acceptance();
    let h = hotspots(&m, &HotspotArgs::default());
    assert_eq!(qnames(&h.modules), ["core::util", "scripts::once", "api::a"], "{h:#?}");
    let ranks: Vec<(usize, usize, usize)> =
        h.modules.iter().map(|r| (r.churn_rank, r.centrality_rank, r.ranked)).collect();
    assert_eq!(ranks, [(2, 1, 3), (1, 3, 3), (3, 2, 3)]);
    let util = &h.modules[0];
    assert_eq!((util.level, util.kind, util.churn), ("module", "MODULE", 6));
    assert_eq!((util.lines_changed, util.last_change), (Some(18), t(6)));
    assert_eq!(util.file.as_deref(), Some(UTIL));
    assert!(h.modules.iter().all(|r| r.tier == "heuristic"));
    assert_eq!(h.history_head, Some(t(8)));
    assert!(h.absence.is_none());

    // `level` and `top` cut the answer, never the ranks.
    let mut args = HotspotArgs::default();
    args.level = "module";
    args.top = 1;
    let h = hotspots(&m, &args);
    assert!(h.symbols.is_empty());
    assert_eq!(qnames(&h.modules), ["core::util"]);
    assert_eq!((h.modules[0].churn_rank, h.modules[0].centrality_rank, h.modules[0].ranked), (2, 1, 3));

    // `scope` narrows the population the ranks range over.
    let mut args = HotspotArgs::default();
    args.scope = Some("api".into());
    let h = hotspots(&m, &args);
    assert_eq!(qnames(&h.modules), ["api::a"]);
    assert_eq!((h.modules[0].churn_rank, h.modules[0].centrality_rank, h.modules[0].ranked), (1, 1, 1));
    assert_eq!(qnames(&h.symbols), ["api::a::get_a"]);
}

/// helper's span holds three blame times; get_a's two; wrap and list_a one
/// each, below `min_churn`.
#[test]
fn symbol_rows() {
    let m = acceptance();
    let h = hotspots(&m, &HotspotArgs::default());
    assert_eq!(qnames(&h.symbols), ["core::util::helper", "api::a::get_a"], "{h:#?}");
    let helper = &h.symbols[0];
    assert_eq!((helper.level, helper.kind, helper.churn), ("symbol", "FUNCTION", 3));
    assert_eq!((helper.churn_rank, helper.centrality_rank, helper.ranked), (1, 1, 2));
    assert_eq!((helper.lines_changed, helper.last_change), (None, t(6)));
    assert_eq!((helper.file.as_deref(), helper.line), (Some(UTIL), Some(4)));

    let mut args = HotspotArgs::default();
    args.level = "symbol";
    args.min_churn = 3;
    let h = hotspots(&m, &args);
    assert!(h.modules.is_empty());
    assert_eq!(qnames(&h.symbols), ["core::util::helper"]);
    assert_eq!(h.symbols[0].ranked, 1);
}

/// No snapshot: no ATTN, so no population, and the absence says how to get one.
#[test]
fn no_history_absence() {
    let d = tree(&[]);
    let m = build(d.path());
    let h = hotspots(&m, &HotspotArgs::default());
    assert!(h.modules.is_empty() && h.symbols.is_empty());
    assert_eq!(h.history_head, None);
    let a = h.absence.expect("absence");
    assert_eq!((a.tier, a.reason), ("FACT", "no_history"));
    assert!(a.note.contains("glia history sync"), "{}", a.note);

    // History that leaves every row below min_churn is a different absence.
    snapshot(d.path(), &[commit(1, &[UTIL, A, B, ONCE])], &[]);
    let h = hotspots(&build(d.path()), &HotspotArgs::default());
    assert!(h.modules.is_empty() && h.symbols.is_empty());
    assert_eq!(h.history_head, Some(t(1)));
    assert_eq!(h.absence.expect("absence").reason, "no_match");
}

/// Competition ranking: util.py and a.py change in the same commits (equal
/// commits and lines), so they share churn rank 2 and b.py is 4, not 3.
#[test]
fn ties_share_rank() {
    let d = tree(&[]);
    let history = vec![
        commit(8, &[ONCE]),
        commit(7, &[ONCE]),
        commit(6, &[ONCE, UTIL, A]),
        commit(5, &[ONCE, UTIL, A, B]),
        commit(4, &[ONCE, UTIL, A]),
        commit(3, &[ONCE, UTIL, A, B]),
    ];
    snapshot(d.path(), &history, &[]);
    let h = hotspots(&build(d.path()), &HotspotArgs::default());
    let churn: Vec<(&str, usize)> = h.modules.iter().map(|r| (r.qname.as_str(), r.churn_rank)).collect();
    let mut sorted = churn.clone();
    sorted.sort();
    assert_eq!(sorted, [("api::a", 2), ("api::b", 4), ("core::util", 2), ("scripts::once", 1)], "{h:#?}");
    assert!(h.modules.iter().all(|r| r.ranked == 4));
}

/// Test code churns with the code it tests: dropped unless asked for.
#[test]
fn tests_excluded_unless_asked() {
    let d = tree(&[(UTIL_TEST, UTIL_TEST_PY)]);
    let mut history = commits();
    history[0].files.push(HistoryFile { p: UTIL_TEST.into(), a: Some(1), d: Some(0), from: None });
    history[1].files.push(HistoryFile { p: UTIL_TEST.into(), a: Some(1), d: Some(0), from: None });
    snapshot(d.path(), &history, &blame());
    let m = build(d.path());
    let h = hotspots(&m, &HotspotArgs::default());
    assert!(!qnames(&h.modules).contains(&"core::util_test"), "{h:#?}");
    assert_eq!(h.modules[0].ranked, 3);
    let mut args = HotspotArgs::default();
    args.include_tests = true;
    let h = hotspots(&m, &args);
    assert!(qnames(&h.modules).contains(&"core::util_test"), "{h:#?}");
    assert_eq!(h.modules[0].ranked, 4);
}

/// Two fresh builds of the same tree and history serialise byte-identically.
#[test]
fn deterministic() {
    let json = || serde_json::to_string(&acceptance_hotspots()).expect("serialise");
    assert_eq!(json(), json());
}

fn acceptance_hotspots() -> Hotspots {
    hotspots(&acceptance(), &HotspotArgs::default())
}
