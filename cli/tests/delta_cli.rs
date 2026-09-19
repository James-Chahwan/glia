//! LE.1c — `glia delta`, the CLI surface of the engine's graph delta
//! (`glia_engine::delta::graph_delta_vs_rev`), driving the real binary
//! over temporary git repos. These tests need a `git` binary: without one
//! they FAIL with a message saying so, never skip.
//!
//! Every git call — the fixture's and the binary's — runs hermetically: a
//! fixed identity, no signing, `main` as the initial branch, no system or
//! global config and `HOME` inside the scratch dir, the isolation of
//! `engine/tests/git_fixture` (whose module the cli crate cannot see).
//!
//! The `[delta] surface=cli rows=<n>` stderr line is the fired_on marker;
//! asserting it here (`marker_rows`) makes it a tested contract. The tests
//! capture the binary's stderr, so grep it from a run of the binary:
//! `glia delta <repo> 2>&1 >/dev/null | grep '^\[delta\] surface=cli rows='`.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

/// `place` calls `price`; `price` sits below it so its HEAD line (5) is not
/// one the working tree could also report.
const A_PY: &str = "def place(o):\n    return price(o)\n\n\ndef price(o):\n    return o\n";
const B_PY: &str = "from shop.a import place\n\n\ndef checkout(o):\n    return place(o)\n";
/// `A_PY` with a new `audit` (line 10) that `place` calls on line 2.
const A_PY_AUDIT: &str = "def place(o):\n    audit(o)\n    return price(o)\n\n\ndef price(o):\n    return o\n\n\ndef audit(o):\n    return o\n";

/// A scratch dir holding a git work tree (`repo/`), an empty global git
/// config and a `HOME`; removed on drop (`cli` has no dev-dependencies, so no
/// `tempfile`).
struct Scratch {
    root: PathBuf,
    top: PathBuf,
    home: PathBuf,
    gitconfig: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Scratch {
    /// A fresh `git init` (branch `main`) with no commits.
    fn git_repo(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-le1c-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&top).expect("scratch repo dir");
        std::fs::create_dir_all(&home).expect("scratch HOME dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        let s = Scratch { root, top, home, gitconfig };
        s.git(&["init", "-q"]);
        s
    }

    /// The committed two-file python repo, `shop/a.py` + `shop/b.py`.
    fn shop(name: &str) -> Self {
        let s = Scratch::git_repo(name);
        s.write("shop/a.py", A_PY);
        s.write("shop/b.py", B_PY);
        s.git(&["add", "-A"]);
        s.git(&["commit", "-q", "-m", "shop"]);
        s
    }

    fn path(&self) -> &str {
        self.top.to_str().expect("utf-8 scratch path")
    }

    /// Write `text` to the repo-relative `rel`. Not staged.
    fn write(&self, rel: &str, text: &str) {
        let p = self.top.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent dir");
        }
        std::fs::write(&p, text).expect("fixture write");
    }

    /// `cmd` with this scratch dir's hermetic git environment.
    fn hermetic(&self, mut cmd: Command) -> Command {
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("HOME", &self.home)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        cmd
    }

    /// One hermetic git command in the work tree; panics on failure.
    fn git(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new("git");
        cmd.args([
            "-c",
            "user.name=glia",
            "-c",
            "user.email=glia@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .arg("-C")
        .arg(&self.top)
        .args(args);
        let out = self.hermetic(cmd).output().unwrap_or_else(|e| {
            panic!("LE.1c delta tests need a `git` binary on PATH; running it failed: {e}")
        });
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            self.top.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// `glia <args>` under the same hermetic git environment.
    fn glia(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
        cmd.args(args);
        self.hermetic(cmd).output().expect("run glia")
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The `[delta] surface=cli rows=<n>` marker's `n`.
fn marker_rows(o: &Output) -> Option<usize> {
    stderr(o)
        .lines()
        .find_map(|l| l.strip_prefix("[delta] surface=cli rows="))
        .and_then(|n| n.trim().parse().ok())
}

fn json(o: &Output) -> Value {
    serde_json::from_str(&stdout(o))
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{}\nstderr:\n{}", stdout(o), stderr(o)))
}

fn has_edge(v: &Value, change: &str, category: &str) -> bool {
    v["edges"]
        .as_array()
        .is_some_and(|es| es.iter().any(|e| e["change"] == change && e["category"] == category))
}

#[test]
fn delta_json_reports_added_call() {
    let s = Scratch::shop("json");
    s.write("shop/a.py", A_PY_AUDIT);
    let o = s.glia(&["delta", s.path(), "--json"]);
    assert_eq!(o.status.code(), Some(0), "stderr:\n{}", stderr(&o));
    let v = json(&o);
    let keys: Vec<&str> = v.as_object().expect("an object").keys().map(String::as_str).collect();
    for k in ["base", "files", "counts", "nodes", "edges"] {
        assert!(keys.contains(&k), "key {k} missing: {keys:?}");
    }
    assert_eq!(v["base"], "HEAD");
    assert!(has_edge(&v, "added", "CALLS"), "an added CALLS edge:\n{v:#}");
    let call = v["edges"]
        .as_array()
        .and_then(|es| {
            es.iter().find(|e| {
                e["change"] == "added"
                    && e["category"] == "CALLS"
                    && e["to_qname"].as_str().is_some_and(|q| q.ends_with("::audit"))
            })
        })
        .unwrap_or_else(|| panic!("place -> audit CALLS added:\n{v:#}"));
    assert_eq!(call["site_file"], "shop/a.py");
    assert_eq!(call["site_line"], 2, "1-based line of the audit(o) call");
    let err = stderr(&o);
    assert!(
        err.lines().any(|l| l.starts_with("[delta] base=HEAD")),
        "the engine's marker:\n{err}"
    );
    let rows = v["nodes"].as_array().map_or(0, Vec::len) + v["edges"].as_array().map_or(0, Vec::len);
    assert_eq!(marker_rows(&o), Some(rows), "surface marker counts the rows printed:\n{err}");
    // The engine line precedes the surface line.
    let engine_at = err.find("[delta] base=").expect("engine marker");
    let surface_at = err.find("[delta] surface=cli").expect("surface marker");
    assert!(engine_at < surface_at, "{err}");
    // A delta saves the parse-cache sidecar, never a layout.
    let gmap = s.top.join(".glia").join("graph");
    assert!(gmap.join("parse_cache.bin").is_file(), "parse cache saved");
    assert!(!gmap.join("manifest.json").exists(), "no layout written");
}

#[test]
fn delta_table_rows_are_located_one_based() {
    let s = Scratch::shop("table");
    s.write("shop/a.py", A_PY_AUDIT);
    let o = s.glia(&["delta", s.path()]);
    assert_eq!(o.status.code(), Some(0), "stderr:\n{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.starts_with(&format!("# glia delta `{}` vs HEAD\n", s.path())), "{out}");
    assert!(out.contains("| change | kind | qname | location |"), "{out}");
    assert!(out.contains("| change | category | from | to | site |"), "{out}");
    let audit = out
        .lines()
        .find(|l| l.starts_with("| added | FUNCTION |") && l.contains("::audit`"))
        .unwrap_or_else(|| panic!("audit added row:\n{out}"));
    assert!(audit.ends_with("| shop/a.py:10 |"), "the record's own 1-based line: {audit}");
    let call = out
        .lines()
        .find(|l| l.starts_with("| added | CALLS |") && l.contains("::audit`"))
        .unwrap_or_else(|| panic!("place -> audit CALLS row:\n{out}"));
    assert!(call.ends_with("| shop/a.py:2 |"), "the call site, 1-based: {call}");
    assert!(marker_rows(&o).is_some_and(|n| n >= 2), "{}", stderr(&o));
}

#[test]
fn delta_removed_rows_are_located_in_the_base() {
    let s = Scratch::shop("removed");
    s.write("shop/a.py", "def place(o):\n    return o\n");
    let o = s.glia(&["delta", s.path()]);
    assert_eq!(o.status.code(), Some(0), "stderr:\n{}", stderr(&o));
    let out = stdout(&o);
    let price = out
        .lines()
        .find(|l| l.starts_with("| removed | FUNCTION |") && l.contains("::price`"))
        .unwrap_or_else(|| panic!("price removed row:\n{out}"));
    assert!(price.ends_with("| shop/a.py:5 (at HEAD) |"), "its def line at HEAD: {price}");
}

#[test]
fn delta_filters_edges_only_and_category() {
    let s = Scratch::shop("filters");
    s.write("shop/a.py", A_PY_AUDIT);
    let o = s.glia(&["delta", s.path(), "--json", "--edges-only", "--category", "calls"]);
    assert_eq!(o.status.code(), Some(0), "stderr:\n{}", stderr(&o));
    let v = json(&o);
    assert_eq!(v["nodes"].as_array().map(Vec::len), Some(0), "--edges-only:\n{v:#}");
    let edges = v["edges"].as_array().expect("edges");
    assert!(!edges.is_empty() && edges.iter().all(|e| e["category"] == "CALLS"), "{v:#}");
    assert!(v["counts"]["nodes_added"].as_u64().is_some_and(|n| n >= 1), "counts stay whole: {v:#}");
    assert_eq!(marker_rows(&o), Some(edges.len()), "{}", stderr(&o));
}

#[test]
fn delta_unknown_category_exits_2() {
    let s = Scratch::shop("category");
    let o = s.glia(&["delta", s.path(), "--category", "CALL"]);
    assert_eq!(o.status.code(), Some(2), "stdout:\n{}\nstderr:\n{}", stdout(&o), stderr(&o));
    let err = stderr(&o);
    assert!(err.contains("unknown edge category `CALL`"), "{err}");
    assert!(err.contains("CALLS") && err.contains("HTTP_CALLS") && err.contains("CO_CHANGES"), "{err}");
    assert!(stdout(&o).is_empty(), "nothing printed on stdout");
    assert!(!s.top.join(".glia").exists(), "rejected before anything is built or saved");
}

#[test]
fn delta_clean_tree_table_says_no_change() {
    let s = Scratch::shop("clean");
    let o = s.glia(&["delta", s.path()]);
    assert_eq!(o.status.code(), Some(0), "stderr:\n{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("_(no graph change)_"), "{out}");
    assert!(!out.contains("| change |"), "no table for an empty delta:\n{out}");
    assert!(out.contains("- nodes: +0 added, -0 removed, ~0 modified, >0 moved"), "{out}");
    assert_eq!(marker_rows(&o), Some(0), "{}", stderr(&o));
}

#[test]
fn delta_git_errors_exit_2() {
    let s = Scratch::shop("errors");
    let o = s.glia(&["delta", s.path(), "--base", "no-such-rev"]);
    assert_eq!(o.status.code(), Some(2), "stderr:\n{}", stderr(&o));
    assert!(stderr(&o).contains("error: not a git work tree or unknown rev no-such-rev"), "{}", stderr(&o));
    assert_eq!(marker_rows(&o), None, "no surface marker on an error");

    let o = s.glia(&["--no-overlay", "delta", s.path()]);
    assert_eq!(o.status.code(), Some(2), "stderr:\n{}", stderr(&o));
    assert!(stderr(&o).contains("error: --no-overlay does not apply to delta"), "{}", stderr(&o));
}
