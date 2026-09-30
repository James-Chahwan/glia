//! CC.10b — `glia hotspots`, driving the real binary over CC.10a's acceptance
//! tree (engine/tests/hotspots.rs): `helper` in core/util.py is called by five
//! functions across three files; scripts/once.py calls and is called by
//! nothing. The history snapshot is written with
//! `code_domain::snapshots::write_history` before the build, no git needed:
//! once.py in 8 commits, util.py in 6, a.py in 4, b.py in 1; helper's span
//! blamed at three times, get_a's at two.
//!
//! The engine's `[hotspots] modules=<m>/<M> symbols=<s>/<S>
//! pagerank_iterations=<i> head=<t>` stderr line is the fired_on marker;
//! asserting it here makes it a tested contract.

use std::path::PathBuf;
use std::process::{Command, Output};

use glia_code_domain::snapshots::{BlameFile, HistoryCommit, HistoryFile, HistoryMeta, write_history};

const UTIL_PY: &str = "\"\"\"Shared helpers.\"\"\"\n\n\ndef helper(x):\n    total = x + 1\n    total = total * 2\n    return total\n\n\ndef wrap(x):\n    return helper(x) + 1\n";
const A_PY: &str = "from core.util import helper\n\n\ndef get_a(x):\n    return helper(x)\n\n\ndef list_a(xs):\n    return [helper(x) for x in xs]\n";
const B_PY: &str = "from core.util import helper\n\n\ndef get_b(x):\n    return helper(x) * 3\n\n\ndef list_b(xs):\n    return [helper(x) - 1 for x in xs]\n";
const ONCE_PY: &str = "def run_once(n):\n    acc = 0\n    for i in range(n):\n        acc += i\n    return acc\n";

const UTIL: &str = "core/util.py";
const A: &str = "api/a.py";
const B: &str = "api/b.py";
const ONCE: &str = "scripts/once.py";

/// Commit `i`'s time: one day apart from 2026-01-01, commit 8 the newest.
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

fn blame() -> Vec<BlameFile> {
    vec![
        BlameFile { p: UTIL.into(), runs: vec![[1, 4, t(1)], [5, 5, t(3)], [6, 7, t(6)], [8, 11, t(1)]] },
        BlameFile { p: A.into(), runs: vec![[1, 4, t(1)], [5, 5, t(4)], [6, 9, t(1)]] },
    ]
}

/// A fresh temp root holding the four sources, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(tag: &str, with_history: bool) -> Self {
        let p = std::env::temp_dir().join(format!("glia-cc10b-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        for (rel, text) in [(UTIL, UTIL_PY), (A, A_PY), (B, B_PY), (ONCE, ONCE_PY)] {
            let path = p.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, text).expect("write source");
        }
        if with_history {
            let c = commits();
            let meta = HistoryMeta::new(c[0].c.clone(), 2000, None, String::new());
            write_history(&p, meta, &c, &blame()).expect("write snapshot");
        }
        Root(p)
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("utf-8 temp path")
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `glia hotspots <args>`, relaying the fired_on marker, and check the
/// exit code.
fn glia(args: &[&str], want: i32) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .arg("hotspots")
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(want), "glia hotspots {args:?}\nstdout:\n{}\nstderr:\n{stderr}", stdout(&out));
    // Relay the marker so `-- --nocapture | grep '^\[hotspots\] '` sees it.
    for line in stderr.lines().filter(|l| l.starts_with("[hotspots] ")) {
        eprintln!("{line}");
    }
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn markers(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.starts_with("[hotspots] ")).map(str::to_string).collect()
}

/// The table rows (`| 1 | ...`) under `## <title>`, up to the next section.
fn rows<'a>(text: &'a str, title: &str) -> Vec<&'a str> {
    let head = format!("## {title}");
    text.lines()
        .skip_while(|l| *l != head)
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .filter(|l| l.starts_with("| ") && !l.starts_with("| #"))
        .collect()
}

/// Both tables, both ranks: util.py (churn rank 2, centrality rank 1) heads
/// the modules, not the uncalled once.py with the most commits.
#[test]
fn tables_show_both_ranks() {
    let root = Root::new("tables", true);
    let out = glia(&[root.path()], 0);
    let text = stdout(&out);
    assert!(text.contains("- history to 2026-01-09 (3 modules, 2 symbols ranked)"), "{text}");
    assert!(text.contains("| # | node | churn | churn rank | centrality rank | of | last change | at |"), "{text}");
    assert_eq!(
        rows(&text, "modules"),
        [
            "| 1 | `core::util` | 6 | 2 | 1 | 3 | 2026-01-07 | core/util.py:1 |",
            "| 2 | `scripts::once` | 8 | 1 | 3 | 3 | 2026-01-09 | scripts/once.py:1 |",
            "| 3 | `api::a` | 4 | 3 | 2 | 3 | 2026-01-06 | api/a.py:1 |",
        ],
        "{text}"
    );
    assert_eq!(
        rows(&text, "symbols"),
        [
            "| 1 | `core::util::helper` | 3 | 1 | 1 | 2 | 2026-01-07 | core/util.py:4 |",
            "| 2 | `api::a::get_a` | 2 | 2 | 2 | 2 | 2026-01-05 | api/a.py:4 |",
        ],
        "{text}"
    );
    let m = markers(&out);
    assert_eq!(m.len(), 1, "one marker per call: {m:?}");
    assert!(m[0].starts_with("[hotspots] modules=3/3 symbols=2/2 pagerank_iterations="), "{m:?}");
    assert!(m[0].ends_with(&format!(" head={}", t(8))), "{m:?}");
}

/// `--level symbol` prints only the symbols; `--top` cuts rows, never ranks;
/// `--scope` narrows the population the ranks range over.
#[test]
fn level_top_and_scope() {
    let root = Root::new("level", true);
    let text = stdout(&glia(&[root.path(), "--level", "symbol"], 0));
    assert!(!text.contains("## modules"), "{text}");
    assert!(text.contains("- history to 2026-01-09 (2 symbols ranked)"), "{text}");
    assert_eq!(rows(&text, "symbols").len(), 2, "{text}");

    let text = stdout(&glia(&[root.path(), "--level", "module", "--top", "1"], 0));
    assert!(!text.contains("## symbols"), "{text}");
    assert_eq!(rows(&text, "modules"), ["| 1 | `core::util` | 6 | 2 | 1 | 3 | 2026-01-07 | core/util.py:1 |"]);

    let text = stdout(&glia(&[root.path(), "--scope", "api"], 0));
    assert_eq!(rows(&text, "modules"), ["| 1 | `api::a` | 4 | 1 | 1 | 1 | 2026-01-06 | api/a.py:1 |"], "{text}");

    // Past every row's churn: symbols print their empty table, modules rank.
    let text = stdout(&glia(&[root.path(), "--min-churn", "4"], 0));
    assert_eq!(rows(&text, "modules").len(), 3, "{text}");
    assert!(text.contains("## symbols\n\n_(none at --min-churn 4)_"), "{text}");
}

/// `--json` is the whole answer, in the engine's field order.
#[test]
fn json_is_the_answer() {
    let root = Root::new("json", true);
    let text = stdout(&glia(&[root.path(), "--json"], 0));
    let v: serde_json::Value = serde_json::from_str(text.trim()).expect("json");
    let keys: Vec<&str> = v.as_object().expect("object").keys().map(String::as_str).collect();
    let mut want = ["absence", "history_head", "modules", "symbols"];
    want.sort_unstable();
    assert_eq!(keys, want, "{text}");
    assert!(text.starts_with("{\"modules\":"), "{text}");
    assert_eq!(v["history_head"], t(8));
    assert!(v["absence"].is_null());
    let util = &v["modules"][0];
    assert_eq!(util["qname"], "core::util");
    assert_eq!((util["churn_rank"].as_u64(), util["centrality_rank"].as_u64(), util["ranked"].as_u64()), (Some(2), Some(1), Some(3)));
    assert_eq!((util["file"].as_str(), util["line"].as_i64(), util["last_change"].as_i64()), (Some(UTIL), Some(1), Some(t(6))));
    assert_eq!(v["symbols"][0]["qname"], "core::util::helper");
}

/// No snapshot: exit 1, and the absence says to run `glia history sync`.
#[test]
fn no_history_exits_one() {
    let root = Root::new("nohist", false);
    let out = glia(&[root.path()], 1);
    let text = stdout(&out);
    assert!(text.contains("- no history"), "{text}");
    assert!(text.contains("_(no hotspots)_"), "{text}");
    assert!(text.contains("> FACT: ") && text.contains("glia history sync"), "{text}");
    assert!(!text.contains("## modules"), "{text}");
    assert!(markers(&out)[0].ends_with(" head=-"), "{:?}", markers(&out));

    let text = stdout(&glia(&[root.path(), "--json"], 1));
    let v: serde_json::Value = serde_json::from_str(text.trim()).expect("json");
    assert_eq!(v["absence"]["reason"], "no_history", "{text}");
    assert!(v["history_head"].is_null() && v["modules"] == serde_json::json!([]));
}

/// History that ranks nothing past the filters is the other absence.
#[test]
fn no_match_exits_one() {
    let root = Root::new("nomatch", true);
    let text = stdout(&glia(&[root.path(), "--min-churn", "99", "--json"], 1));
    let v: serde_json::Value = serde_json::from_str(text.trim()).expect("json");
    assert_eq!(v["absence"]["reason"], "no_match", "{text}");
    assert!(v["absence"]["note"].as_str().is_some_and(|n| n.contains(">= 99")), "{text}");
}

/// A level outside `module | symbol | both` is a usage error.
#[test]
fn unknown_level_exits_two() {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["hotspots", ".", "--level", "modules"])
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("module") && err.contains("symbol") && err.contains("both"), "{err}");
}
