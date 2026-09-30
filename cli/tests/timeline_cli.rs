//! CD.5d — `glia timeline build|history|as-of`, the CLI surface of the
//! time-travel graph (`glia_engine::timeline`, CD.5c), driving the real
//! binary over CD.5c's four-commit git history. These tests need a `git`
//! binary: without one they FAIL with a message saying so, never skip.
//!
//! Every git call — the fixture's and the binary's — runs hermetically: a
//! fixed identity, no signing, `main` as the initial branch, no system or
//! global config and `HOME` inside the scratch dir, the isolation of
//! `engine/tests/git_fixture` (whose module the cli crate cannot see).
//!
//! The fired_on markers are the engine's `[timeline] repo=...`,
//! `[timeline] history ...` and `[timeline] as_of ...` lines and this
//! surface's `[timeline] surface=cli <action> rows=<n>`; asserting them here
//! makes them a tested contract. Grep them from a run of the binary:
//! `glia timeline build <repo> 2>&1 >/dev/null | grep '^\[timeline\] '`.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

/// Commit 1: `f` calls `g` (`g` on line 5).
const C1: &str = "def f():\n    return g()\n\n\ndef g():\n    return 1\n";
/// Commit 2: `h` added, `f` calls both.
const C2: &str = "def f():\n    g()\n    return h()\n\n\ndef g():\n    return 1\n\n\ndef h():\n    return 2\n";
/// Commit 4 (in the moved file): `f` calls `h` only.
const C4: &str = "def f():\n    return h()\n\n\ndef g():\n    return 1\n\n\ndef h():\n    return 2\n";

/// A scratch dir holding a work tree (`repo/`), an empty global git config
/// and a `HOME`; removed on drop (`cli` has no dev-dependencies, so no
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
    /// An empty work tree dir, not yet a git repo.
    fn plain(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-cd5d-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&top).expect("scratch repo dir");
        std::fs::create_dir_all(&home).expect("scratch HOME dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        Scratch { root, top, home, gitconfig }
    }

    /// CD.5c's history: f -> g (1), + h and f -> h (2), `git mv app/a.py
    /// app/b.py` (3), f -> g removed (4).
    fn four_commits(name: &str) -> Self {
        let s = Scratch::plain(name);
        s.git(&["init", "-q"]);
        s.write("app/a.py", C1);
        s.commit("f calls g");
        s.write("app/a.py", C2);
        s.commit("f calls h");
        s.git(&["mv", "app/a.py", "app/b.py"]);
        s.commit("move a to b");
        s.write("app/b.py", C4);
        s.commit("f drops g");
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

    fn commit(&self, msg: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
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
            panic!("CD.5d timeline tests need a `git` binary on PATH; running it failed: {e}")
        });
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            self.top.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// `glia timeline <args>` with persisting allowed.
    fn timeline(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
        cmd.arg("timeline").args(args).env_remove("GLIA_NO_PERSIST");
        self.hermetic(cmd).output().expect("run glia")
    }

    /// `glia timeline <args>` under `GLIA_NO_PERSIST=1`.
    fn timeline_no_persist(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
        cmd.arg("timeline").args(args).env("GLIA_NO_PERSIST", "1");
        self.hermetic(cmd).output().expect("run glia")
    }

    fn sidecar(&self) -> PathBuf {
        self.top.join(".glia").join("graph").join("timeline.gmap")
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn both(o: &Output) -> String {
    format!("stdout:\n{}\nstderr:\n{}", stdout(o), stderr(o))
}

fn json(o: &Output) -> Value {
    serde_json::from_str(&stdout(o)).unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{}", both(o)))
}

/// The `n` of this surface's `[timeline] surface=cli <action> rows=<n>`.
fn marker_rows(o: &Output, action: &str) -> Option<usize> {
    let prefix = format!("[timeline] surface=cli {action} rows=");
    stderr(o).lines().find_map(|l| l.strip_prefix(&prefix)).and_then(|n| n.trim().parse().ok())
}

/// The rev table's data rows: `| <index> | <sha> | <date> | <subject> |`.
fn rev_rows(out: &str) -> Vec<&str> {
    out.lines()
        .filter(|l| l.strip_prefix("| ").is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit())))
        .collect()
}

#[test]
fn build_then_history() {
    let s = Scratch::four_commits("flow");

    // Before any build: history is the note, exit 0; as-of is an error.
    let o = s.timeline(&["history", s.path(), "f"]);
    assert_eq!(o.status.code(), Some(0), "{}", both(&o));
    assert!(stdout(&o).contains("run `glia timeline build"), "{}", both(&o));
    assert_eq!(marker_rows(&o, "history"), Some(0), "{}", both(&o));
    let o = s.timeline(&["history", s.path(), "f", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", both(&o));
    let v = json(&o);
    assert_eq!(v["results"], Value::Array(Vec::new()));
    assert!(v["absence"].is_null());
    assert!(v["note"].as_str().is_some_and(|n| n.contains("glia timeline build")), "{v:#}");
    let o = s.timeline(&["as-of", s.path(), "0"]);
    assert_eq!(o.status.code(), Some(2), "{}", both(&o));
    assert!(stderr(&o).contains("run `glia timeline build"), "{}", both(&o));

    // Build: 4 rows oldest first, the counts, the sidecar written.
    let o = s.timeline(&["build", s.path(), "--revs", "4"]);
    assert_eq!(o.status.code(), Some(0), "{}", both(&o));
    let out = stdout(&o);
    assert!(out.starts_with(&format!("# glia timeline build `{}` (4 revs of HEAD)\n", s.path())), "{out}");
    assert!(out.contains("| rev | sha | date | subject |"), "{out}");
    let rows = rev_rows(&out);
    assert_eq!(rows.len(), 4, "{out}");
    assert!(rows[0].starts_with("| 0 | ") && rows[0].ends_with(" | f calls g |"), "{}", rows[0]);
    assert!(rows[3].starts_with("| 3 | ") && rows[3].ends_with(" | f drops g |"), "{}", rows[3]);
    let date = rows[0].split(" | ").nth(2).expect("a date cell");
    assert!(date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-', "{date}");
    assert!(out.contains("- nodes: ") && out.contains(" closed in the window"), "{out}");
    let written = s.sidecar();
    assert!(out.contains(&format!("- written: {}", written.display())), "{out}");
    assert!(written.is_file());
    let err = stderr(&o);
    assert!(err.lines().any(|l| l.starts_with("[timeline] repo=") && l.contains(" built=4 ")), "{err}");
    assert_eq!(marker_rows(&o, "build"), Some(4), "{err}");

    // History: the f -> g span closes at rev 3, f -> h is still present.
    let o = s.timeline(&["history", s.path(), "f"]);
    assert_eq!(o.status.code(), Some(0), "{}", both(&o));
    let out = stdout(&o);
    let g = out
        .lines()
        .find(|l| l.starts_with("CALLS -> ") && l.contains("::g "))
        .unwrap_or_else(|| panic!("the CALLS -> g span:\n{}", both(&o)));
    assert!(g.contains("since ") && g.contains("'f calls g' (before the window)"), "{g}");
    assert!(g.contains("until ") && g.contains("'f drops g'"), "{g}");
    assert!(g.ends_with("[FUNCTION at app/b.py:5]"), "g's def, 1-based, as last seen: {g}");
    let h = out
        .lines()
        .find(|l| l.starts_with("CALLS -> ") && l.contains("::h "))
        .unwrap_or_else(|| panic!("the CALLS -> h span:\n{}", both(&o)));
    assert!(h.contains("'f calls h'") && h.contains("still present") && !h.contains("before the window"), "{h}");
    let err = stderr(&o);
    assert!(err.lines().any(|l| l.starts_with("[timeline] history f rows=")), "{err}");
    assert!(marker_rows(&o, "history").is_some_and(|n| n >= 2), "{err}");

    // History --json is the engine's answer; --category filters it.
    let o = s.timeline(&["history", s.path(), "f", "--category", "calls", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", both(&o));
    let v = json(&o);
    let rows = v["results"].as_array().expect("results");
    assert_eq!(rows.len(), 2, "{v:#}");
    assert!(rows.iter().all(|r| r["category"] == "CALLS" && r["tier"] == "derived"), "{v:#}");
    let g = rows.iter().find(|r| r["other_qname"] == "app::b::g").expect("f -> g row");
    assert_eq!(g["until"]["index"], 3);
    assert_eq!(g["since_window_start"], true);
    assert!(v["absence"].is_null());

    // An empty answer prints its absence and exits 0.
    let o = s.timeline(&["history", s.path(), "f", "--category", "HTTP_CALLS"]);
    assert_eq!(o.status.code(), Some(0), "{}", both(&o));
    let out = stdout(&o);
    assert!(out.contains("_(no edge history)_") && out.contains("> FACT: no HTTP_CALLS edge touches"), "{out}");

    // As-of by index (--json parses) and by sha prefix.
    let o = s.timeline(&["as-of", s.path(), "0", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", both(&o));
    let v = json(&o);
    assert_eq!(v["rev"]["index"], 0);
    assert_eq!(v["rev"]["subject"], "f calls g");
    assert!(v["by_category"]["CALLS"].as_u64().is_some_and(|n| n >= 1), "{v:#}");
    assert_eq!(marker_rows(&o, "as_of"), v["edges"].as_u64().map(|n| n as usize), "{}", both(&o));
    let head = s.git(&["rev-parse", "HEAD"]);
    let sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let o = s.timeline(&["as-of", s.path(), &sha[..7]]);
    assert_eq!(o.status.code(), Some(0), "{}", both(&o));
    let out = stdout(&o);
    assert!(out.contains(&format!(" rev 3 ({} ", &sha[..7])) && out.contains("'f drops g'"), "{out}");
    assert!(out.contains("| category | edges |") && out.contains("| CALLS | "), "{out}");
    let o = s.timeline(&["as-of", s.path(), "9"]);
    assert_eq!(o.status.code(), Some(2), "{}", both(&o));
    assert!(stderr(&o).contains("no rev `9`"), "{}", both(&o));
}

#[test]
fn build_json_and_no_persist() {
    let s = Scratch::four_commits("nopersist");
    let o = s.timeline_no_persist(&["build", s.path(), "--revs", "2"]);
    assert_eq!(o.status.code(), Some(0), "{}", both(&o));
    let out = stdout(&o);
    assert_eq!(rev_rows(&out).len(), 2, "{out}");
    assert!(out.contains("- not written (GLIA_NO_PERSIST=1)"), "{out}");
    assert!(!s.sidecar().exists(), "nothing written");
    assert!(stderr(&o).contains(" wrote=none"), "{}", both(&o));

    let o = s.timeline_no_persist(&["build", s.path(), "--revs", "2", "--head", "HEAD~1", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", both(&o));
    let v = json(&o);
    let revs = v["revs"].as_array().expect("revs");
    let subjects: Vec<&str> = revs.iter().filter_map(|r| r["subject"].as_str()).collect();
    assert_eq!(subjects, ["f calls h", "move a to b"], "the window ends at --head: {v:#}");
    assert!(v["written"].is_null(), "{v:#}");
    for k in ["skipped", "nodes", "edges", "edge_spans", "closed", "moves"] {
        assert!(v.get(k).is_some(), "key {k}: {v:#}");
    }
}

#[test]
fn build_errors_exit_2() {
    // Outside a git work tree.
    let s = Scratch::plain("nogit");
    s.write("app.py", "def f():\n    return 1\n");
    let o = s.timeline(&["build", s.path()]);
    assert_eq!(o.status.code(), Some(2), "{}", both(&o));
    assert!(stderr(&o).contains("not a git work tree"), "{}", both(&o));
    assert!(stdout(&o).is_empty(), "{}", both(&o));

    // An unknown --head, and a window outside 1..=200 (clap's usage error).
    let s = Scratch::four_commits("errors");
    let o = s.timeline(&["build", s.path(), "--head", "no-such-rev"]);
    assert_eq!(o.status.code(), Some(2), "{}", both(&o));
    assert!(stderr(&o).contains("unknown rev no-such-rev"), "{}", both(&o));
    for bad in ["0", "201"] {
        let o = s.timeline(&["build", s.path(), "--revs", bad]);
        assert_eq!(o.status.code(), Some(2), "--revs {bad}: {}", both(&o));
        assert!(!s.sidecar().exists(), "--revs {bad}: nothing built");
    }
}
