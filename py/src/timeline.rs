//! **timeline** (CD.5d): pyo3 surface for `glia_engine::timeline` (CD.5c),
//! the time-travel graph, as three module functions, each a native answer
//! (LD.2). They take a repo path, not a `PyGraph`: the engine builds the revs
//! itself, and the answers read the sidecar it wrote
//! (`<repo>/.glia/graph/timeline.gmap`).
//!
//! - `timeline_build(repo_path, revs=20, head="HEAD")` builds the window and
//!   writes the sidecar (not under `GLIA_NO_PERSIST=1`): the engine's
//!   `TimelineBuilt`.
//! - `timeline_history(repo_path, qname, category=None)` lists the edge spans
//!   that ever touched a node: the engine's `Answer` of `EdgeHistoryRow`s. The
//!   current graph (the default layout through `persist::load_or_rebuild`)
//!   feeds its absences only.
//! - `timeline_as_of(repo_path, rev)` is the graph at one rev: the engine's
//!   `AsOfSummary`.
//!
//! Errors: `ValueError` for an argument that names nothing (a window outside
//! `1..=200`, a rev no rev of the timeline matches), `RuntimeError` with the
//! engine's text for a failed build (git missing, not a git work tree, an
//! unknown head, no rev of the window built), a missing or unreadable
//! sidecar, or a current graph that cannot be loaded.
//!
//! Transport only. The helpers the pyfunctions delegate to are pyo3-free, so
//! `cargo test -p glia-py` covers them (see the crate doc). Fired-on markers:
//! the engine's `[timeline] repo=...` / `history ...` / `as_of ...` lines,
//! then this surface's `[timeline] surface=pyo3 <build|history|as_of>
//! rows=<n>` (`n`: revs, history rows, or the view's edges).

use std::path::Path;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

use glia_engine::absence::Answer;
use glia_engine::persist::{default_layout_dir, load_or_rebuild};
use glia_engine::timeline::{
    AsOfSummary, EdgeHistoryRow, MAX_REVS, TimelineArgs, TimelineBuilt, as_of, build_timeline, edge_history,
    load_timeline,
};

use crate::convert::to_py;
use crate::registry::ModuleFns;

/// Why a helper failed, as the Python exception it becomes.
#[derive(Debug, PartialEq, Eq)]
enum Fail {
    /// An argument that names nothing: `ValueError`.
    Value(String),
    /// A build, the sidecar or the current graph failed: `RuntimeError`.
    Runtime(String),
}

impl From<Fail> for PyErr {
    fn from(f: Fail) -> PyErr {
        match f {
            Fail::Value(m) => PyValueError::new_err(m),
            Fail::Runtime(m) => PyRuntimeError::new_err(m),
        }
    }
}

/// The body of [`timeline_build`], minus pyo3. `persist` overrides the
/// default (write unless `GLIA_NO_PERSIST=1`) when given; the pyfunction
/// passes `None`.
fn build_answer(repo_path: &str, revs: usize, head: &str, persist: Option<bool>) -> Result<TimelineBuilt, Fail> {
    if revs == 0 || revs > MAX_REVS {
        return Err(Fail::Value(format!("timeline_build: revs must be 1..={MAX_REVS}, not {revs}")));
    }
    let mut a = TimelineArgs::default();
    a.revs = revs;
    a.head = head.to_string();
    if let Some(p) = persist {
        a.persist = p;
    }
    let built = build_timeline(repo_path, &a).map_err(|e| Fail::Runtime(format!("timeline_build: {e}")))?;
    eprintln!("[timeline] surface=pyo3 build rows={}", built.revs.len());
    Ok(built)
}

/// The body of [`timeline_history`], minus pyo3.
fn history_answer(repo_path: &str, qname: &str, category: Option<&str>) -> Result<Answer<EdgeHistoryRow>, Fail> {
    let store = load_timeline(repo_path).map_err(|e| Fail::Runtime(format!("timeline_history: {e}")))?;
    let root = Path::new(repo_path);
    let (current, _outcome) = load_or_rebuild(&default_layout_dir(root), Some(root), true)
        .map_err(|e| Fail::Runtime(format!("timeline_history: the current graph of {repo_path}: {e}")))?;
    let mut answer = edge_history(&current.merged, &store, qname, category);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = current.parse_errors.len();
    }
    eprintln!("[timeline] surface=pyo3 history rows={}", answer.results.len());
    Ok(answer)
}

/// The body of [`timeline_as_of`], minus pyo3.
fn as_of_answer(repo_path: &str, rev: &str) -> Result<AsOfSummary, Fail> {
    let store = load_timeline(repo_path).map_err(|e| Fail::Runtime(format!("timeline_as_of: {e}")))?;
    let summary = as_of(&store, rev).map_err(|e| Fail::Value(format!("timeline_as_of: {e}")))?.summary();
    eprintln!("[timeline] surface=pyo3 as_of rows={}", summary.edges);
    Ok(summary)
}

/// **timeline_build** (CD.5d): build the time-travel graph of the repo at
/// `repo_path` over the last `revs` commits (1..=200) of `head`'s
/// first-parent chain, one incremental build per rev on the shared parse
/// cache, and write the sidecar `<repo>/.glia/graph/timeline.gmap`
/// (self-gitignored) unless `GLIA_NO_PERSIST=1`.
///
/// Returns a dict `{revs, skipped, nodes, edges, edge_spans, closed, moves,
/// written}`: `revs` the built revs oldest first, each `{index, sha, time,
/// subject}` (`time` unix seconds, `index` the rev a span names); `skipped`
/// `[sha, reason]` pairs of the revs that did not build; `nodes` node spans,
/// `edges` distinct edges, `edge_spans` their spans, `closed` the spans that
/// closed inside the window, `moves` the node moves chained; `written` the
/// sidecar's path, or None when nothing was written.
///
/// Raises `ValueError` when `revs` is outside 1..=200, `RuntimeError` with
/// the engine's message when git is missing, `repo_path` is not in a git
/// work tree, `head` names no commit, no rev of the window builds, or the
/// sidecar cannot be written.
#[pyfunction]
#[pyo3(signature = (repo_path, revs=20, head="HEAD"))]
fn timeline_build(py: Python<'_>, repo_path: &str, revs: usize, head: &str) -> PyResult<Py<PyAny>> {
    let built = build_answer(repo_path, revs, head, None)?;
    to_py(py, serde_json::to_string(&built))
}

/// **timeline_history** (CD.5d): every edge span of the timeline that ever
/// touched the node `qname` names (its exact qname, else the one qname
/// ending `::<qname>`), in every id it had over the window (a file move
/// followed), optionally of one edge `category` (case-insensitive).
///
/// Returns a dict `{results, absence}`. Each result is `{category,
/// direction, other_qname, other_kind, since, since_window_start, until,
/// file, line, tier}`: `direction` `out` (the node is the edge's `from`) or
/// `in`; `since` / `until` rev dicts `{index, sha, time, subject}`, `until`
/// None while the edge is present at the window's last rev;
/// `since_window_start` True when the edge was already there at the first
/// rev (its true start is earlier); `file` / `line` (1-based) the OTHER end's
/// location as last seen, not the edge site; `tier` always `derived`. Rows
/// are sorted by since, category, other end. An empty answer's `absence`
/// says why (an unknown or ambiguous name with suggestions, or no such edge
/// with its coverage caveats).
///
/// Reads the sidecar and the current graph: the default layout, served as
/// is when fresh, else rebuilt (and written back unless
/// `GLIA_NO_PERSIST=1`). Raises `RuntimeError` when there is no sidecar (run
/// `timeline_build` first) or it cannot be read, or when the current graph
/// cannot be loaded.
#[pyfunction]
#[pyo3(signature = (repo_path, qname, category=None))]
fn timeline_history(py: Python<'_>, repo_path: &str, qname: &str, category: Option<&str>) -> PyResult<Py<PyAny>> {
    let answer = history_answer(repo_path, qname, category)?;
    to_py(py, serde_json::to_string(&answer))
}

/// **timeline_as_of** (CD.5d): the graph at one rev of the timeline, `rev`
/// an index of the window (0 is the oldest) or a commit id prefix of at
/// least 7 hex chars.
///
/// Returns a dict `{rev, nodes, edges, by_category}`: `rev` the rev dict
/// `{index, sha, time, subject}`, `nodes` / `edges` the counts at that rev,
/// `by_category` edges per category name. Reads the sidecar only.
///
/// Raises `ValueError` when `rev` names no rev of the window (or several),
/// `RuntimeError` when there is no sidecar (run `timeline_build` first) or
/// it cannot be read.
#[pyfunction]
#[pyo3(signature = (repo_path, rev))]
fn timeline_as_of(py: Python<'_>, repo_path: &str, rev: &str) -> PyResult<Py<PyAny>> {
    let summary = as_of_answer(repo_path, rev)?;
    to_py(py, serde_json::to_string(&summary))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(timeline_as_of, m)?)?;
    m.add_function(wrap_pyfunction!(timeline_build, m)?)?;
    m.add_function(wrap_pyfunction!(timeline_history, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "timeline", add: register } }

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use glia_engine::timeline::DEFAULT_REVS;

    use super::*;

    /// A scratch dir under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// CD.5c's four-commit history: `f` calls `g` (1), `h` added and called
    /// (2), `git mv app/a.py app/b.py` (3), `f` drops `g` (4). Every git call
    /// is hermetic (fixed identity, empty global config); the engine's own
    /// git calls inherit only the process environment, which carries no
    /// signing or hook setting a commit here would need. Panics without a
    /// `git` binary.
    fn four_commits(name: &str) -> (Scratch, PathBuf) {
        let root = std::env::temp_dir().join(format!("glia-cd5d-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        std::fs::create_dir_all(top.join("app")).expect("scratch dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(["-c", "user.name=glia", "-c", "user.email=glia@example.invalid"])
                .args(["-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
                .arg("-C")
                .arg(&top)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", &gitconfig)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .output()
                .unwrap_or_else(|e| panic!("CD.5d timeline test needs a `git` binary: {e}"));
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let write = |rel: &str, text: &str| std::fs::write(top.join(rel), text).expect("fixture write");
        git(&["init", "-q"]);
        write("app/a.py", "def f():\n    return g()\n\n\ndef g():\n    return 1\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "f calls g"]);
        write("app/a.py", "def f():\n    g()\n    return h()\n\n\ndef g():\n    return 1\n\n\ndef h():\n    return 2\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "f calls h"]);
        git(&["mv", "app/a.py", "app/b.py"]);
        git(&["commit", "-q", "-m", "move a to b"]);
        write("app/b.py", "def f():\n    return h()\n\n\ndef g():\n    return 1\n\n\ndef h():\n    return 2\n");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "f drops g"]);
        (Scratch(root), top)
    }

    /// An answer's JSON text, as `to_py` hands it to `json.loads`.
    macro_rules! text {
        ($t:expr) => {
            serde_json::to_string($t).expect("serialises")
        };
    }

    fn value(text: &str) -> serde_json::Value {
        serde_json::from_str(text).expect("valid JSON")
    }

    /// The object keys of `text` in the order `json.loads` builds them.
    fn keys_in_order(text: &str) -> Vec<String> {
        let v = value(text);
        let mut keys: Vec<(usize, String)> = v
            .as_object()
            .expect("an object")
            .keys()
            .map(|k| (text.find(&format!("\"{k}\":")).unwrap_or(usize::MAX), k.clone()))
            .collect();
        keys.sort();
        keys.into_iter().map(|(_, k)| k).collect()
    }

    /// The pyo3 signature's literal default is the engine's.
    #[test]
    fn default_revs_matches_the_engine() {
        assert_eq!(DEFAULT_REVS, 20, "update `revs=20` in timeline_build's signature");
    }

    /// CD.5d: the three helpers return the documented objects over CD.5c's
    /// four commits — build (4 revs, written), history (f -> g closes at rev
    /// 3, f -> h still present), as-of (the counts at rev 1) — and each
    /// failure is the documented exception kind.
    #[test]
    fn helpers_return_the_documented_objects() {
        let (_scratch, top) = four_commits("objects");
        let repo = top.to_str().expect("utf-8 temp path");

        // Before any build: no sidecar is a RuntimeError naming the build.
        match history_answer(repo, "f", None) {
            Err(Fail::Runtime(m)) => assert!(m.contains("glia timeline build"), "{m}"),
            other => panic!("history before a build: {other:?}"),
        }
        assert!(matches!(as_of_answer(repo, "0"), Err(Fail::Runtime(_))));

        assert!(matches!(build_answer(repo, 0, "HEAD", Some(true)), Err(Fail::Value(_))));
        assert!(matches!(build_answer(repo, MAX_REVS + 1, "HEAD", Some(true)), Err(Fail::Value(_))));
        match build_answer(repo, 4, "no-such-rev", Some(true)) {
            Err(Fail::Runtime(m)) => assert!(m.contains("unknown rev no-such-rev"), "{m}"),
            other => panic!("unknown head: {other:?}"),
        }

        let built = build_answer(repo, 4, "HEAD", Some(true)).expect("build 4 revs");
        assert_eq!(
            keys_in_order(&text!(&built)),
            ["revs", "skipped", "nodes", "edges", "edge_spans", "closed", "moves", "written"]
        );
        let b = value(&text!(&built));
        assert_eq!(b["revs"].as_array().map(Vec::len), Some(4));
        assert_eq!(b["revs"][0]["subject"], "f calls g");
        assert_eq!(b["revs"][3]["index"], 3);
        let written = top.join(".glia/graph/timeline.gmap");
        assert_eq!(b["written"].as_str(), written.to_str());

        let history = history_answer(repo, "f", None).expect("history of f");
        assert_eq!(keys_in_order(&text!(&history)), ["results", "absence"]);
        let rows = &history.results;
        let g = rows
            .iter()
            .find(|r| r.category == "CALLS" && r.direction == "out" && r.other_qname.ends_with("::g"))
            .unwrap_or_else(|| panic!("f -> g row: {}", text!(&history)));
        assert_eq!(g.until.as_ref().map(|u| u.index), Some(3));
        assert!(g.since_window_start);
        let h = rows
            .iter()
            .find(|r| r.category == "CALLS" && r.direction == "out" && r.other_qname.ends_with("::h"))
            .unwrap_or_else(|| panic!("f -> h row: {}", text!(&history)));
        assert!(h.until.is_none(), "f -> h still present");
        assert_eq!(
            keys_in_order(&text!(g)),
            [
                "category",
                "direction",
                "other_qname",
                "other_kind",
                "since",
                "since_window_start",
                "until",
                "file",
                "line",
                "tier"
            ]
        );
        let none = history_answer(repo, "f", Some("HTTP_CALLS")).expect("filtered history");
        assert!(none.results.is_empty());
        assert_eq!(none.absence.as_ref().map(|a| a.reason), Some("no_edges"));

        let summary = as_of_answer(repo, "1").expect("as of rev 1");
        assert_eq!(keys_in_order(&text!(&summary)), ["rev", "nodes", "edges", "by_category"]);
        assert_eq!(summary.rev.index, 1);
        assert!(summary.by_category.get("CALLS").is_some_and(|n| *n >= 2), "{}", text!(&summary));
        let by_sha = as_of_answer(repo, &built.revs[1].sha[..7]).expect("as of rev 1 by sha");
        assert_eq!(by_sha.rev.index, 1);
        match as_of_answer(repo, "9") {
            Err(Fail::Value(m)) => assert!(m.contains("no rev `9`"), "{m}"),
            other => panic!("unknown rev: {other:?}"),
        }
    }

    /// Outside a git work tree the build is a RuntimeError, never a panic.
    #[test]
    fn build_outside_git_is_a_runtime_error() {
        let root = std::env::temp_dir().join(format!("glia-cd5d-py-{}-plain", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch dir");
        let _scratch = Scratch(root.clone());
        std::fs::write(root.join("app.py"), "def f():\n    return 1\n").expect("write app.py");
        let repo = root.to_str().expect("utf-8 temp path");
        match build_answer(repo, 2, "HEAD", Some(true)) {
            Err(Fail::Runtime(m)) => assert!(m.contains("not a git work tree"), "{m}"),
            other => panic!("outside git: {other:?}"),
        }
    }
}
