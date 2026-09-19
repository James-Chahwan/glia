//! **snapshots** (LF.5d, LF.6d): the snapshot steps, as module functions.
//! External, run-dependent inputs enter a build the way Confluence docs do: a
//! separate step writes files under `<repo>/.glia/`, and the next
//! `generate()` reads them back offline. The build never runs these steps
//! itself; a consumer (the repo-graph wrapper, when HEAD moves) calls one
//! before `generate()`.
//!
//! `history_sync` (LF.5d) reads the local git and writes
//! `<repo>/.glia/history-snapshot/` (docs/overlay.md, "History snapshot").
//! LF.6d's `tests_ingest` joins it here.
//!
//! Transport only: the capture, the snapshot format and the `[history] sync`
//! marker live in `glia_snapshots`. The helper each pyfunction delegates to is
//! pyo3-free, so `cargo test -p repo-graph-py` covers it (see the crate doc).

use std::path::Path;

use glia_snapshots::{HistoryOptions, HistorySummary, history_sync};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::convert::to_py;
use crate::registry::ModuleFns;

/// The options [`history_sync_py`] passes, tagged `surface=pyo3` for the
/// marker.
fn history_options(
    max_commits: usize,
    since: Option<String>,
    blame: bool,
    blame_max_files: usize,
) -> HistoryOptions {
    HistoryOptions {
        max_commits,
        since,
        blame,
        blame_max_files,
        surface: "pyo3",
    }
}

/// The sync behind [`history_sync_py`], minus pyo3. `Err` is the library's
/// plain message (not a git work tree, no commits, not a directory, an empty
/// window, git missing); nothing is written then.
fn history_of(repo_path: &str, opts: &HistoryOptions) -> Result<HistorySummary, String> {
    history_sync(Path::new(repo_path), opts)
}

/// **history_sync** (LF.5d): read the git history of the repo at `repo_path`
/// (local only: no fetch, no remote, no author or committer identity) and
/// write `<repo>/.glia/history-snapshot/` (commits.jsonl, blame.jsonl,
/// meta.json), replacing any earlier snapshot. The next `generate()` ingests
/// it: churn ATTN on modules, blame recency on symbols, CO_CHANGES edges
/// between modules that change together.
///
/// `max_commits`: the newest this many non-merge commits (`git log -n`).
/// `since`: only commits newer than this date, passed to `git log --since`
/// verbatim. `blame`: also blame the `blame_max_files` most-changed files
/// (slower). Returns `{head, commits, files, renames, binary, blame_files,
/// runs}` (`head` the full sha). Raises `ValueError` when the sync fails, and
/// writes nothing then. Prints `[history] sync ... surface=pyo3` on stderr.
#[pyfunction]
#[pyo3(
    name = "history_sync",
    signature = (repo_path, max_commits=2000, since=None, blame=false, blame_max_files=300)
)]
fn history_sync_py(
    py: Python<'_>,
    repo_path: &str,
    max_commits: usize,
    since: Option<String>,
    blame: bool,
    blame_max_files: usize,
) -> PyResult<Py<PyAny>> {
    let opts = history_options(max_commits, since, blame, blame_max_files);
    let summary = history_of(repo_path, &opts).map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&summary))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(history_sync_py, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "snapshots", add: register } }

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;

    /// A scratch dir under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A one-file git repo with two commits, made with a fixed identity and
    /// dates under an empty global git config. Panics without a `git` binary.
    fn git_repo(name: &str) -> (Scratch, PathBuf) {
        let root = std::env::temp_dir().join(format!("glia-lf5d-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        std::fs::create_dir_all(&top).expect("scratch dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        let git = |args: &[&str], t: i64| {
            let date = format!("@{t} +0000");
            let out = Command::new("git")
                .arg("-C")
                .arg(&top)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", &gitconfig)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "Quillon Identitymarker")
                .env("GIT_AUTHOR_EMAIL", "quillon.author@identity.invalid")
                .env("GIT_COMMITTER_NAME", "Pemberly Committertoken")
                .env("GIT_COMMITTER_EMAIL", "pemberly.committer@identity.invalid")
                .env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .output()
                .unwrap_or_else(|e| panic!("LF.5d needs a `git` binary on PATH: {e}"));
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"], 0);
        std::fs::write(top.join("app.py"), "def f():\n    return 1\n").expect("write");
        git(&["add", "-A"], 1_767_225_600);
        git(&["commit", "-q", "-m", "init"], 1_767_225_600);
        std::fs::write(top.join("app.py"), "def f():\n    return 2\n").expect("write");
        git(&["add", "-A"], 1_767_312_000);
        git(&["commit", "-q", "-m", "edit"], 1_767_312_000);
        (Scratch(root), top)
    }

    /// The pyo3 signature's literal defaults are the library's defaults, and
    /// the options name the pyo3 surface.
    #[test]
    fn history_options_default_like_the_library_and_name_pyo3() {
        let lib = HistoryOptions::default();
        assert_eq!(
            history_options(2000, None, false, 300),
            HistoryOptions {
                surface: "pyo3",
                ..lib
            },
            "the #[pyo3(signature)] literals must track HistoryOptions::default()"
        );
        let opts = history_options(7, Some("2 weeks ago".into()), true, 9);
        assert_eq!(
            (
                opts.max_commits,
                opts.since.as_deref(),
                opts.blame,
                opts.blame_max_files,
                opts.surface
            ),
            (7, Some("2 weeks ago"), true, 9, "pyo3")
        );
    }

    /// LF.5d: `history_sync` is transport — pin the wiring: a real repo's
    /// summary reaches the JSON in field order and the snapshot is written; a
    /// directory outside git is the library's plain error, with nothing written.
    #[test]
    fn history_sync_returns_the_summary_json_or_the_error() {
        let (_scratch, top) = git_repo("ok");
        let repo = top.to_str().expect("utf-8 scratch path");
        let opts = history_options(2000, None, false, 300);
        let summary = history_of(repo, &opts).expect("sync");
        let json = serde_json::to_string(&summary).expect("json");
        assert!(
            json.starts_with(r#"{"head":""#)
                && json.ends_with(
                    r#"","commits":2,"files":1,"renames":0,"binary":0,"blame_files":0,"runs":0}"#
                ),
            "{json}"
        );
        assert_eq!(
            summary.head.len(),
            40,
            "head is the full sha: {}",
            summary.head
        );
        let dir = repo_graph_code_domain::snapshots::history_dir(&top);
        assert!(
            dir.join("meta.json").is_file(),
            "snapshot written under {}",
            dir.display()
        );

        let plain = std::env::temp_dir().join(format!("glia-lf5d-py-{}-plain", std::process::id()));
        let _ = std::fs::remove_dir_all(&plain);
        std::fs::create_dir_all(&plain).expect("plain dir");
        let _plain_guard = Scratch(plain.clone());
        let err = history_of(plain.to_str().expect("utf-8"), &opts).expect_err("not a git repo");
        assert!(err.contains("is not inside a git work tree"), "{err}");
        assert!(
            !plain.join(".glia").exists(),
            "a failed sync writes nothing"
        );
    }
}
