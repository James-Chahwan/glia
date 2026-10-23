//! **snapshots** (LF.5d, LF.6d): the snapshot steps, as module functions.
//! External, run-dependent inputs enter a build the way Confluence docs do: a
//! separate step writes files under `<repo>/.glia/`, and the next
//! `generate()` reads them back offline. The build never runs these steps
//! itself; a consumer (the repo-graph wrapper, when HEAD moves or when it is
//! handed a CI run's reports) calls one before `generate()`.
//!
//! `history_sync` (LF.5d) reads the local git and writes
//! `<repo>/.glia/history-snapshot/` (docs/overlay.md, "History snapshot").
//! `tests_ingest` (LF.6d) reads one CI run's test reports (JUnit XML, CI logs,
//! lcov) and adds them as the newest run of `<repo>/.glia/test-snapshot/`,
//! which keeps the last `window` runs (CC.9b).
//! `history_calibrate` (EC.6) builds the repo's graph and replays the history
//! snapshot over it, writing `<repo>/.glia/calibration-snapshot/`: the
//! per-repo ranking each ranked answer picks (docs/overlay.md, "Calibration
//! snapshot"). It never syncs; run `history_sync` first.
//!
//! Transport only: the capture, the parsing, the redaction, the snapshot
//! formats and the `[history] sync` / `[tests] ingest` markers live in
//! `glia_snapshots`, the replay, the record and the `[calibrate] done` marker
//! in `glia_engine::calibrate`. The helper each pyfunction delegates to is pyo3-free, so
//! `cargo test -p glia-py` covers it (see the crate doc).

use std::path::{Path, PathBuf};

use glia_engine::calibrate::{CalibrateOptions, DEFAULT_GROUPS, ReplayParams, calibrate, specs};
use glia_snapshots::{
    HistoryOptions, HistorySummary, TestsIngestOptions, TestsSummary, history_sync, tests_ingest,
};
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
    messages: bool,
) -> HistoryOptions {
    HistoryOptions {
        max_commits,
        since,
        blame,
        blame_max_files,
        messages,
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
/// (slower). `messages`: keep each commit's message (trailers dropped,
/// redacted), author time and merged-branch label in the snapshot's
/// `messages.jsonl`, which only query-time answers read (EB.1); `False` keeps
/// none. Returns `{head, commits, files, renames, binary, blame_files, runs,
/// messages, redacted, trailers_dropped, branch_labels}` (`head` the full
/// sha). Raises `ValueError` when the sync fails, and
/// writes nothing then. Prints `[history] sync ... surface=pyo3` on stderr.
#[pyfunction]
#[pyo3(
    name = "history_sync",
    signature = (repo_path, max_commits=2000, since=None, blame=false, blame_max_files=300, messages=true)
)]
fn history_sync_py(
    py: Python<'_>,
    repo_path: &str,
    max_commits: usize,
    since: Option<String>,
    blame: bool,
    blame_max_files: usize,
    messages: bool,
) -> PyResult<Py<PyAny>> {
    let opts = history_options(max_commits, since, blame, blame_max_files, messages);
    let summary = history_of(repo_path, &opts).map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&summary))
}

/// The options [`tests_ingest_py`] passes, tagged `surface=pyo3` for the
/// marker. An omitted list is an empty one.
fn tests_options(
    junit: Option<Vec<PathBuf>>,
    logs: Option<Vec<PathBuf>>,
    lcov: Option<Vec<PathBuf>>,
    run: Option<String>,
    window: usize,
    reset: bool,
) -> TestsIngestOptions {
    TestsIngestOptions {
        junit: junit.unwrap_or_default(),
        logs: logs.unwrap_or_default(),
        lcov: lcov.unwrap_or_default(),
        run,
        window,
        reset,
        surface: "pyo3",
    }
}

/// The ingest behind [`tests_ingest_py`], minus pyo3. `Err` is the library's
/// plain message (not a directory, no reports given, no report readable, a
/// window of 0, the write failed); nothing is written then. Reports skipped while others
/// were read come back in `report_errors`, not as an `Err`.
fn tests_of(repo_path: &str, opts: &TestsIngestOptions) -> Result<TestsSummary, String> {
    tests_ingest(Path::new(repo_path), opts)
}

/// **tests_ingest** (LF.6d, CC.9b): read the test reports one CI run
/// produced for the repo at `repo_path` and add them as the newest run of
/// `<repo>/.glia/test-snapshot/` (cases.jsonl, lcov.jsonl, meta.json), which
/// keeps the last `window` runs. The next `generate()` ingests it: FAIL
/// cells on failing tests and the code their traces implicate, each counting
/// the runs of the window it failed in, and COVERAGE cells from the newest
/// run's lcov. Never called by `generate()` itself.
///
/// `junit`: JUnit XML report paths. `logs`: CI log paths, read for their
/// failure summary lines (pytest, go test, cargo test, jest). `lcov`: lcov
/// tracefile paths. Each is a list of `str` or `os.PathLike`; at least one
/// report is required. `run`: a label for the run (a CI run id), stored
/// verbatim. `window`: the most runs the snapshot keeps, this one included
/// (1 replaces the snapshot; 0 raises). `reset`: drop every earlier run
/// first, so this run is seq 0. Failure messages and traces are redacted
/// before they are stored.
///
/// Returns `{reports, junit_files, log_files, lcov_files, cases, failed,
/// errors, skipped, passed, stored, redacted, covered_files, runs, seq,
/// report_errors}` (the counts are this run's, `runs` the runs the snapshot
/// holds now, `seq` this run's); a report that could not be read or parsed
/// is skipped and listed in `report_errors` as `{report, reason}`. Raises
/// `ValueError` when no report is given, none could be read or `window` is
/// 0, and writes nothing then. Prints `[tests] ingest ... runs=<R>
/// window=<W> seq=<S> surface=pyo3` on stderr.
#[pyfunction]
#[pyo3(
    name = "tests_ingest",
    signature = (repo_path, junit=None, logs=None, lcov=None, run=None, window=10, reset=false)
)]
#[allow(clippy::too_many_arguments)]
fn tests_ingest_py(
    py: Python<'_>,
    repo_path: &str,
    junit: Option<Vec<PathBuf>>,
    logs: Option<Vec<PathBuf>>,
    lcov: Option<Vec<PathBuf>>,
    run: Option<String>,
    window: usize,
    reset: bool,
) -> PyResult<Py<PyAny>> {
    let opts = tests_options(junit, logs, lcov, run, window, reset);
    let summary = tests_of(repo_path, &opts).map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&summary))
}

/// The registered group each `sets` name names; `None` (or an empty list) is
/// every registered group, an unknown name the engine's error listing the
/// known ones.
fn calibrate_groups(sets: Option<&[String]>) -> Result<Vec<&'static str>, String> {
    let names = sets.unwrap_or_default();
    if names.is_empty() {
        return Ok(DEFAULT_GROUPS.to_vec());
    }
    names
        .iter()
        .map(
            |name| match DEFAULT_GROUPS.iter().find(|g| **g == name.as_str()) {
                Some(g) => Ok(*g),
                // DEFAULT_GROUPS is every group `specs` registers, so `specs`
                // refuses this name with its list of the known ones.
                None => match specs(&[name.as_str()]) {
                    Err(e) => Err(e),
                    Ok(_) => Err(format!(
                        "unknown candidate group `{name}`: expected one of {}",
                        DEFAULT_GROUPS.join(", ")
                    )),
                },
            },
        )
        .collect()
}

/// The calibration behind [`history_calibrate_py`], minus pyo3: build the
/// repo at `repo_path` (`generate_one`), run `calibrate::calibrate` with
/// surface `pyo3` and return the summary as JSON. `Err` is a bad group name,
/// the build's error or the step's (no snapshot names `glia history sync`);
/// nothing is written then.
fn calibrate_json(
    repo_path: &str,
    max_commits: usize,
    max_seeds: usize,
    sets: Option<&[String]>,
) -> Result<String, String> {
    let groups = calibrate_groups(sets)?;
    let mut params = ReplayParams::default();
    params.max_commits = max_commits;
    params.max_seeds = max_seeds;
    let mut opts = CalibrateOptions::default();
    opts.params = params;
    opts.groups = groups;
    opts.surface = "pyo3";
    let built = glia_engine::generate_one(repo_path)?;
    let summary = calibrate(Path::new(repo_path), &built.merged, &opts)?;
    serde_json::to_string(&summary).map_err(|e| e.to_string())
}

/// **history_calibrate** (EC.6): build the repo at `repo_path`, replay its
/// history snapshot (`history_sync`) over the graph, leak-free and online,
/// across the fixed candidate rankings, and write
/// `<repo>/.glia/calibration-snapshot/`: the per-repo pick each ranked answer
/// reads. Never syncs; `generate()` never calibrates.
///
/// `max_commits`: the newest this many test commits are scored.
/// `max_seeds`: seed queries per test commit. `sets`: the candidate groups to
/// replay (`["structure"]`); `None` is every registered group.
///
/// Returns `{dir, history_head, commits, test_commits, messages, ms, sets}`,
/// `sets` one `{set, shape, pick, last_online_pick, fallback, test_commits,
/// queries, selection_recall10, fallback_recall10, delta_recall10, ci_low,
/// ci_high}` per (set, shape). Raises `ValueError` with no history snapshot
/// (naming `glia history sync`), an unknown group or a build error. Prints
/// `[calibrate] done surface=pyo3 sets=<n> test_commits=<t> ms=<ms>` on
/// stderr.
#[pyfunction]
#[pyo3(
    name = "history_calibrate",
    signature = (repo_path, max_commits=300, max_seeds=5, sets=None)
)]
fn history_calibrate_py(
    py: Python<'_>,
    repo_path: &str,
    max_commits: usize,
    max_seeds: usize,
    sets: Option<Vec<String>>,
) -> PyResult<Py<PyAny>> {
    let text = py
        .detach(|| calibrate_json(repo_path, max_commits, max_seeds, sets.as_deref()))
        .map_err(PyValueError::new_err)?;
    to_py(py, Ok::<String, serde_json::Error>(text))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(history_sync_py, m)?)?;
    m.add_function(wrap_pyfunction!(history_calibrate_py, m)?)?;
    m.add_function(wrap_pyfunction!(tests_ingest_py, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "snapshots", add: register } }

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use glia_code_domain::snapshots::TESTS_DEFAULT_WINDOW;

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
            history_options(2000, None, false, 300, true),
            HistoryOptions {
                surface: "pyo3",
                ..lib
            },
            "the #[pyo3(signature)] literals must track HistoryOptions::default()"
        );
        let opts = history_options(7, Some("2 weeks ago".into()), true, 9, false);
        assert_eq!(
            (
                opts.max_commits,
                opts.since.as_deref(),
                opts.blame,
                opts.blame_max_files,
                opts.messages,
                opts.surface
            ),
            (7, Some("2 weeks ago"), true, 9, false, "pyo3")
        );
    }

    /// LF.5d: `history_sync` is transport — pin the wiring: a real repo's
    /// summary reaches the JSON in field order and the snapshot is written; a
    /// directory outside git is the library's plain error, with nothing written.
    #[test]
    fn history_sync_returns_the_summary_json_or_the_error() {
        let (_scratch, top) = git_repo("ok");
        let repo = top.to_str().expect("utf-8 scratch path");
        let opts = history_options(2000, None, false, 300, true);
        let summary = history_of(repo, &opts).expect("sync");
        let json = serde_json::to_string(&summary).expect("json");
        assert!(
            json.starts_with(r#"{"head":""#)
                && json.ends_with(
                    r#"","commits":2,"files":1,"renames":0,"binary":0,"blame_files":0,"runs":0,"messages":2,"redacted":0,"trailers_dropped":0,"branch_labels":0}"#
                ),
            "{json}"
        );
        assert_eq!(
            summary.head.len(),
            40,
            "head is the full sha: {}",
            summary.head
        );
        let dir = glia_code_domain::snapshots::history_dir(&top);
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
    const PYTEST_JUNIT: &str = r#"<testsuites><testsuite name="pytest" tests="3" failures="1"><testcase classname="tests.test_app" name="test_boom" file="tests/test_app.py" line="3"><failure message="ValueError: boom">E   ValueError: boom</failure></testcase><testcase classname="tests.test_app" name="test_ok"/><testcase classname="tests.test_app" name="test_add"/></testsuite></testsuites>"#;
    const LCOV: &str = "TN:\nSF:app.py\nDA:1,1\nDA:2,0\nend_of_record\n";

    /// A scratch dir holding `repo/` (one python file) and `reports/`.
    fn reports_repo(name: &str) -> (Scratch, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("glia-lf6d-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        let reports = root.join("reports");
        std::fs::create_dir_all(&top).expect("scratch dir");
        std::fs::create_dir_all(&reports).expect("reports dir");
        std::fs::write(top.join("app.py"), "def f():\n    return 1\n").expect("write");
        (Scratch(root), top, reports)
    }

    /// Omitted lists are empty ones, the pyo3 signature's literal defaults
    /// (`window=10, reset=False`) are the library's, and the options name
    /// the pyo3 surface.
    #[test]
    fn tests_options_default_to_no_reports_and_name_pyo3() {
        assert_eq!(
            tests_options(None, None, None, None, 10, false),
            TestsIngestOptions {
                surface: "pyo3",
                ..TestsIngestOptions::default()
            },
            "the #[pyo3(signature)] literals must track TestsIngestOptions::default()"
        );
        assert_eq!(TESTS_DEFAULT_WINDOW, 10);
        let opts = tests_options(
            Some(vec![PathBuf::from("a.xml")]),
            Some(vec![PathBuf::from("ci.log")]),
            Some(vec![PathBuf::from("c.lcov"), PathBuf::from("d.lcov")]),
            Some("ci-42".into()),
            3,
            true,
        );
        assert_eq!(
            (
                opts.junit.len(),
                opts.logs.len(),
                opts.lcov.len(),
                opts.run.as_deref(),
                opts.window,
                opts.reset,
                opts.surface
            ),
            (1, 1, 2, Some("ci-42"), 3, true, "pyo3")
        );
    }

    /// LF.6d: `tests_ingest` is transport — pin the wiring: the summary
    /// reaches the JSON in field order with the skipped report listed, the
    /// snapshot is written with the run label; no readable report, or none
    /// given, is the library's plain error with nothing written.
    #[test]
    fn tests_ingest_returns_the_summary_json_or_the_error() {
        let (_scratch, top, reports) = reports_repo("ok");
        let repo = top.to_str().expect("utf-8 scratch path");
        let junit = reports.join("junit.xml");
        let lcov = reports.join("coverage.lcov");
        let cut = reports.join("cut.xml");
        std::fs::write(&junit, PYTEST_JUNIT).expect("write");
        std::fs::write(&lcov, LCOV).expect("write");
        std::fs::write(&cut, "<testsuite><testcase name=\"x\">").expect("write");

        let opts = tests_options(
            Some(vec![junit.clone(), cut.clone()]),
            None,
            Some(vec![lcov.clone()]),
            Some("ci-42".into()),
            3,
            false,
        );
        let summary = tests_of(repo, &opts).expect("ingest");
        let json = serde_json::to_string(&summary).expect("json");
        let (junit_s, lcov_s, cut_s) = (junit.display(), lcov.display(), cut.display());
        assert_eq!(
            json,
            format!(
                r#"{{"reports":["{lcov_s}","{junit_s}"],"junit_files":1,"log_files":0,"lcov_files":1,"cases":3,"failed":1,"errors":0,"skipped":0,"passed":2,"stored":1,"redacted":0,"covered_files":1,"runs":1,"seq":0,"report_errors":[{{"report":"{cut_s}","reason":"document ends with 2 element(s) still open"}}]}}"#
            )
        );
        let dir = glia_code_domain::snapshots::tests_dir(&top);
        let meta = std::fs::read_to_string(dir.join("meta.json")).expect("meta.json written");
        assert!(
            meta.contains(r#""run": "ci-42""#) && meta.contains(r#""window": 3"#),
            "{meta}"
        );
        // CC.9b: the next ingest is run seq 1 of the window.
        let again = tests_options(Some(vec![junit.clone()]), None, None, None, 3, false);
        let summary = tests_of(repo, &again).expect("second ingest");
        assert_eq!((summary.runs, summary.seq), (2, 1));

        let (_scratch, top, reports) = reports_repo("bad");
        let repo = top.to_str().expect("utf-8 scratch path");
        let cut = reports.join("cut.xml");
        std::fs::write(&cut, "<testsuite>").expect("write");
        let err = tests_of(
            repo,
            &tests_options(Some(vec![cut]), None, None, None, 10, false),
        )
        .expect_err("nothing readable");
        assert!(err.starts_with("no report could be read ("), "{err}");
        let err = tests_of(
            repo,
            &tests_options(None, Some(Vec::new()), None, None, 10, false),
        )
        .expect_err("no reports");
        assert!(err.starts_with("no test reports given"), "{err}");
        assert!(
            !top.join(".glia").exists(),
            "a failed ingest writes nothing"
        );
    }

    /// EC.5's cluster repo (smaller hub) with a synthetic 30-commit history
    /// snapshot, written by `write_history`: no git.
    fn calibrate_repo(name: &str, with_history: bool) -> (Scratch, PathBuf) {
        use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};
        const N: usize = 8;
        const COMMITS: usize = 30;
        let root = std::env::temp_dir().join(format!("glia-ec6-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        let write = |rel: &str, body: &str| {
            let p = top.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            std::fs::write(p, body).expect("write");
        };
        let mut hub = String::from("def h():\n    return 0\n\n");
        for i in 0..N {
            hub.push_str(&format!("def g{i:02}():\n    return h()\n\n"));
        }
        write("hub.py", &hub);
        write("zeta/__init__.py", "");
        let file = |i: usize| format!("zeta/zeta{i:02}.py");
        for i in 0..N {
            write(
                &file(i),
                &format!(
                    "from hub import h, g{i:02}\n\ndef zeta{i:02}_f():\n    h()\n    return g{i:02}()\n"
                ),
            );
        }
        if with_history {
            let mut commits: Vec<HistoryCommit> = (0..COMMITS)
                .map(|i| HistoryCommit {
                    c: format!("{:040x}", i + 1),
                    t: 1_700_000_000 + i as i64 * 3600,
                    files: (0..2 + i % 3)
                        .map(|j| HistoryFile {
                            p: file(((i * 3) % N + j) % N),
                            a: Some(1),
                            d: Some(0),
                            from: None,
                        })
                        .collect(),
                })
                .collect();
            commits.reverse();
            let meta = HistoryMeta::new(format!("{COMMITS:040x}"), COMMITS, None, String::new());
            write_history(&top, meta, &commits, &[]).expect("write history");
        }
        (Scratch(root), top)
    }

    /// EC.6: the summary as JSON with the options applied; no snapshot, an
    /// unknown group and a missing repo are the plain errors, nothing written.
    #[test]
    fn calibrate_json_returns_the_summary_or_the_error() {
        let lib = ReplayParams::default();
        assert_eq!(
            (lib.max_commits, lib.max_seeds),
            (300, 5),
            "the #[pyo3(signature)] literals must track ReplayParams::default()"
        );
        let (_scratch, top) = calibrate_repo("ok", true);
        let repo = top.to_str().expect("utf-8 scratch path");
        let json = calibrate_json(repo, 10, 2, None).expect("calibrate");
        let v: serde_json::Value = serde_json::from_str(&json).expect("json");
        let keys: Vec<&str> = v
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        let mut want = vec![
            "dir",
            "history_head",
            "commits",
            "test_commits",
            "messages",
            "ms",
            "sets",
        ];
        want.sort_unstable();
        let mut got = keys.clone();
        got.sort_unstable();
        assert_eq!(got, want, "{json}");
        assert_eq!(v["commits"], 30);
        assert_eq!(v["test_commits"], 10, "max_commits reaches the replay");
        let sets = v["sets"].as_array().expect("sets");
        assert_eq!(sets.len(), 8);
        assert_eq!(
            (sets[0]["set"].as_str(), sets[0]["shape"].as_str()),
            (Some("SELh0"), Some("k1"))
        );
        assert_eq!(
            sets[0]["queries"], 20,
            "max_seeds reaches the replay: 2 per test commit"
        );
        assert!(glia_engine::calibrate::record::calibration_dir(&top).is_dir());

        let structure = vec!["structure".to_string()];
        assert!(calibrate_json(repo, 10, 2, Some(&structure)).is_ok());
        let nope = vec!["nope".to_string()];
        let err = calibrate_json(repo, 10, 2, Some(&nope)).expect_err("unknown group");
        assert!(err.contains("nope") && err.contains("structure"), "{err}");

        let (_scratch, top) = calibrate_repo("nosnap", false);
        let err =
            calibrate_json(top.to_str().expect("utf-8"), 300, 5, None).expect_err("no snapshot");
        assert!(err.contains("glia history sync"), "{err}");
        assert!(!glia_engine::calibrate::record::calibration_dir(&top).exists());
    }
}
