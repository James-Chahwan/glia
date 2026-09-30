//! LF.6d — `glia tests ingest`, the CLI surface of the test-report snapshot
//! step (`glia_snapshots::tests_ingest`), driving the real binary over
//! temporary repos. The reports are written by the tests themselves, outside
//! the repo, so a report's label is the path as given.
//!
//! The `[tests] ingest ... surface=cli` stderr line is the fired_on marker;
//! asserting it here makes it a tested contract. Grep it with
//! `cargo test -p glia-cli --test tests_cli -- --nocapture 2>&1 | grep -o '\[tests\] ingest .*surface=cli'`
//! (since CC.9b the line ends `stored=N runs=<R> window=<W> seq=<S> surface=cli`).

use std::path::PathBuf;
use std::process::{Command, Output};

use glia_code_domain::snapshots::{META_FILE, TESTS_CASES_FILE, TESTS_LCOV_FILE, tests_dir};
use serde_json::Value;

/// A pytest-style JUnit report: 3 cases, `test_boom` failed, 2 passed.
const PYTEST_JUNIT: &str = r#"<?xml version="1.0" encoding="utf-8"?><testsuites name="pytest tests"><testsuite name="pytest" errors="0" failures="1" skipped="0" tests="3" time="0.022" hostname="ci"><testcase classname="tests.test_app" name="test_boom" file="tests/test_app.py" line="3" time="0.000"><failure message="ValueError: boom">def test_boom():
&gt;       raise ValueError("boom")
E       ValueError: boom

tests/test_app.py:5: ValueError</failure></testcase><testcase classname="tests.test_app" name="test_ok" file="tests/test_app.py" line="7" time="0.000" /><testcase classname="tests.test_app" name="test_add" file="tests/test_app.py" line="10" time="0.000" /></testsuite></testsuites>
"#;

/// A second JUnit report: one errored case, one skipped.
const GO_JUNIT: &str = r#"<testsuites><testsuite name="api" tests="2" failures="0" errors="1" skipped="1"><testcase classname="api" name="TestCreate"><error message="panic: nil map">panic: assignment to entry in nil map</error></testcase><testcase classname="api" name="TestSkip"><skipped/></testcase></testsuite></testsuites>
"#;

/// lcov for the repo's two source files, repo-relative `SF:` paths.
const LCOV: &str = "TN:\nSF:api/app.py\nDA:1,1\nDA:2,1\nDA:4,0\nend_of_record\nTN:\nSF:tests/test_app.py\nDA:1,1\nDA:3,1\nend_of_record\n";

/// A CI log with one pytest failure summary line.
const CI_LOG: &str = "tests/test_app.py F..                                    [100%]\n\
=========================== short test summary info ============================\n\
FAILED tests/test_app.py::test_boom - ValueError: boom\n\
========================= 1 failed, 2 passed in 0.01s ==========================\n";

/// A JUnit report cut off mid-document.
const CUT_XML: &str = "<testsuite name=\"s\"><testcase name=\"x\">";

/// A scratch dir holding a small python repo (`repo/`) and the reports
/// (`reports/`, outside the repo); removed on drop (`cli` has no
/// dev-dependencies, so no `tempfile`).
struct Scratch {
    root: PathBuf,
    top: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-lf6d-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        std::fs::create_dir_all(top.join("api")).expect("scratch dir");
        std::fs::create_dir_all(top.join("tests")).expect("scratch dir");
        std::fs::create_dir_all(root.join("reports")).expect("reports dir");
        std::fs::write(
            top.join("api/app.py"),
            "def handler():\n    return add(1, 2)\n\n\ndef add(a, b):\n    return a + b\n",
        )
        .expect("write");
        std::fs::write(
            top.join("tests/test_app.py"),
            "from api.app import add, handler\n\n\ndef test_boom():\n    raise ValueError(\"boom\")\n\n\n\
             def test_ok():\n    assert handler() == 3\n\n\ndef test_add():\n    assert add(1, 1) == 2\n",
        )
        .expect("write");
        Scratch { root, top }
    }

    fn path(&self) -> &str {
        self.top.to_str().expect("utf-8 scratch path")
    }

    /// Write `reports/<name>` and return its path.
    fn report(&self, name: &str, body: &str) -> String {
        let path = self.root.join("reports").join(name);
        std::fs::write(&path, body).expect("write report");
        path.to_str().expect("utf-8 report path").to_string()
    }

    /// `<repo>/.glia/test-snapshot/<file>`, or "" when absent.
    fn snapshot_file(&self, file: &str) -> String {
        std::fs::read_to_string(tests_dir(&self.top).join(file)).unwrap_or_default()
    }

    /// Run the real binary. Relays the markers so `-- --nocapture | grep
    /// '^\[tests\] '` sees them.
    fn glia(&self, args: &[&str]) -> Output {
        let out = Command::new(env!("CARGO_BIN_EXE_glia"))
            .args(args)
            .env("GLIA_NO_PERSIST", "1")
            .output()
            .expect("glia runs");
        for line in text(&out.stderr).lines() {
            if line.starts_with("[tests] ") {
                eprintln!("{line}");
            }
        }
        out
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The `[tests] ingest` line of `stderr`.
fn ingest_marker(stderr: &str) -> &str {
    stderr
        .lines()
        .find(|l| l.starts_with("[tests] ingest "))
        .unwrap_or_else(|| panic!("no `[tests] ingest` marker in stderr:\n{stderr}"))
}

fn warnings(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|l| l.starts_with("warning: ")).collect()
}

#[test]
fn ingest_writes_the_snapshot_and_names_the_cli_surface() {
    let r = Scratch::new("ingest");
    let junit = r.report("pytest-junit.xml", PYTEST_JUNIT);
    let lcov = r.report("coverage.lcov", LCOV);
    let out = r.glia(&["tests", "ingest", r.path(), "--junit", &junit, "--lcov", &lcov, "--run", "ci-42"]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");

    let snapshot = tests_dir(&r.top);
    let mut reports = [junit.clone(), lcov.clone()];
    reports.sort();
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            format!("read {}", reports[0]),
            format!("read {}", reports[1]),
            format!("ingested 1 failing case(s) and 1 lcov file(s) -> {}", snapshot.display()),
            "this run is seq 0; the snapshot holds 1 run(s) (window 10)".to_string(),
            format!("run `glia build {}` to ingest.", r.path()),
        ],
    );
    let marker = ingest_marker(&stderr);
    assert!(
        marker.ends_with(
            " junit_files=1 log_files=0 lcov_files=1 cases=3 failed=1 errors=0 skipped=0 passed=2 stored=1 \
             runs=1 window=10 seq=0 surface=cli"
        ),
        "{marker}"
    );
    assert!(warnings(&stderr).is_empty(), "{stderr}");

    for file in [TESTS_CASES_FILE, TESTS_LCOV_FILE, META_FILE] {
        assert!(snapshot.join(file).is_file(), "{file} not written under {}", snapshot.display());
    }
    let cases = r.snapshot_file(TESTS_CASES_FILE);
    assert_eq!(cases.lines().count(), 1, "{cases}");
    let case: Value = serde_json::from_str(cases.lines().next().unwrap_or("")).expect("case row parses");
    assert_eq!((case["name"].as_str(), case["status"].as_str()), (Some("test_boom"), Some("failed")));
    assert_eq!(r.snapshot_file(TESTS_LCOV_FILE).lines().count(), 2);
    let meta: Value = serde_json::from_str(&r.snapshot_file(META_FILE)).expect("meta.json parses");
    assert_eq!(meta["run"], "ci-42", "{meta}");
    assert_eq!(meta["reports"], serde_json::json!(reports), "{meta}");
    assert_eq!((meta["cases_total"].as_u64(), meta["passed"].as_u64()), (Some(3), Some(2)), "{meta}");
    // CC.9b: a version 2 snapshot, one run in a window of 10.
    assert_eq!((meta["version"].as_u64(), meta["window"].as_u64()), (Some(2), Some(10)), "{meta}");
    assert_eq!(meta["runs"].as_array().map(|r| r.len()), Some(1), "{meta}");
    assert_eq!((meta["runs"][0]["seq"].as_u64(), meta["runs"][0]["run"].as_str()), (Some(0), Some("ci-42")), "{meta}");
    assert_eq!(case["seq"], 0, "{case}");
}

/// CC.9b: each ingest is the next run of the snapshot's window; `--window N`
/// keeps the last N runs, `--reset` starts over at seq 0, and `--window 0`
/// is a usage error that writes nothing.
#[test]
fn window_keeps_the_last_runs_and_reset_starts_over() {
    let r = Scratch::new("window");
    let junit = r.report("pytest-junit.xml", PYTEST_JUNIT);
    let out = r.glia(&["tests", "ingest", r.path(), "--junit", &junit, "--window", "0"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(!r.top.join(".glia").exists(), "a usage error writes nothing");

    let seqs = |r: &Scratch| -> Vec<u64> {
        let meta: Value = serde_json::from_str(&r.snapshot_file(META_FILE)).expect("meta.json parses");
        meta["runs"].as_array().into_iter().flatten().filter_map(|run| run["seq"].as_u64()).collect()
    };
    for (i, window) in ["3", "3", "3", "3"].into_iter().enumerate() {
        let label = format!("ci-{i}");
        let out = r.glia(&["tests", "ingest", r.path(), "--junit", &junit, "--run", &label, "--window", window]);
        let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
        assert_eq!(out.status.code(), Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
        let runs = (i + 1).min(3);
        assert!(
            ingest_marker(&stderr).ends_with(&format!(" stored=1 runs={runs} window=3 seq={i} surface=cli")),
            "{stderr}"
        );
        assert!(
            stdout.contains(&format!("this run is seq {i}; the snapshot holds {runs} run(s) (window 3)")),
            "{stdout}"
        );
    }
    assert_eq!(seqs(&r), [1, 2, 3], "run 0 aged out of a window of 3");
    assert_eq!(r.snapshot_file(TESTS_CASES_FILE).lines().count(), 3, "one failing case per kept run");

    let out = r.glia(&["tests", "ingest", r.path(), "--junit", &junit, "--reset"]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(ingest_marker(&stderr).ends_with(" stored=1 runs=1 window=10 seq=0 surface=cli"), "{stderr}");
    assert_eq!(seqs(&r), [0]);
}

#[test]
fn no_report_flag_is_a_usage_error() {
    let r = Scratch::new("noflag");
    let out = r.glia(&["tests", "ingest", r.path()]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.starts_with("error: ") && stderr.contains("--junit"), "clap names the flags:\n{stderr}");
    assert!(
        stderr.contains("Usage: glia tests ingest <REPO> <--junit <PATH>...|--log <PATH>...|--lcov <PATH>...>"),
        "the usage puts the repo first, since each path flag takes every path after it:\n{stderr}"
    );
    assert!(!r.top.join(".glia").exists(), "a usage error writes nothing");

    // `--run` alone is not a report.
    let out = r.glia(&["tests", "ingest", r.path(), "--run", "ci-1"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));

    // The repo after a path flag is read as one more path: the repo is then
    // missing, a usage error rather than an ingest into the wrong place.
    let junit = r.report("pytest-junit.xml", PYTEST_JUNIT);
    let out = r.glia(&["tests", "ingest", "--junit", &junit, r.path()]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("<REPO>"), "{stderr}");
    assert!(!r.top.join(".glia").exists(), "a usage error writes nothing");
}

#[test]
fn a_malformed_report_warns_and_the_rest_is_ingested() {
    let r = Scratch::new("malformed");
    let bad = r.report("cut.xml", CUT_XML);
    let good = r.report("pytest-junit.xml", PYTEST_JUNIT);
    let out = r.glia(&["tests", "ingest", r.path(), "--junit", &bad, "--junit", &good]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");

    let warned = warnings(&stderr);
    assert_eq!(warned.len(), 1, "one warning per unreadable report:\n{stderr}");
    assert!(
        warned[0].starts_with(&format!("warning: skipped {bad}: ")) && warned[0].contains("still open"),
        "{}",
        warned[0]
    );
    assert!(stderr.contains(&format!("[tests] skip report={bad} ")), "{stderr}");
    assert!(stdout.lines().any(|l| l == format!("read {good}")), "{stdout}");
    assert!(!stdout.contains(&bad), "a skipped report is not listed as read:\n{stdout}");
    let marker = ingest_marker(&stderr);
    assert!(marker.contains(" junit_files=1 ") && marker.contains(" stored=1 "), "{marker}");
    let meta: Value = serde_json::from_str(&r.snapshot_file(META_FILE)).expect("meta.json parses");
    assert_eq!(meta["reports"], serde_json::json!([good]), "{meta}");
    assert_eq!(meta["run"], Value::Null, "no --run, no label: {meta}");
}

#[test]
fn every_report_failing_exits_1_and_writes_nothing() {
    let r = Scratch::new("allbad");
    let bad = r.report("cut.xml", CUT_XML);
    let missing = r.root.join("reports/no-such.lcov");
    let missing = missing.to_str().expect("utf-8");
    let out = r.glia(&["tests", "ingest", r.path(), "--junit", &bad, "--lcov", missing]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(1), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    let errors: Vec<&str> = stderr.lines().filter(|l| l.starts_with("error: ")).collect();
    assert_eq!(errors.len(), 1, "{stderr}");
    assert!(
        errors[0].contains("no report could be read") && errors[0].contains(&bad) && errors[0].contains(missing),
        "the error names every report and why: {}",
        errors[0]
    );
    assert!(!stderr.contains("[tests] ingest "), "no marker on failure: {stderr}");
    assert!(!r.top.join(".glia").exists(), "a failed ingest writes nothing");

    // A repo that is not a directory.
    let good = r.report("pytest-junit.xml", PYTEST_JUNIT);
    let gone = r.top.join("no-such-dir");
    let out = r.glia(&["tests", "ingest", gone.to_str().expect("utf-8"), "--junit", &good]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.starts_with("error: not a directory: "), "{stderr}");
}

/// Each path flag repeats, and takes several values after one flag, so a
/// shell glob (`--junit reports/*.xml`) passes every match.
#[test]
fn path_flags_repeat_and_take_a_glob_expansion() {
    let r = Scratch::new("globs");
    let a = r.report("a-junit.xml", PYTEST_JUNIT);
    let b = r.report("b-junit.xml", GO_JUNIT);
    let log = r.report("ci.log", CI_LOG);
    let lcov = r.report("coverage.lcov", LCOV);

    let out = r.glia(&["tests", "ingest", r.path(), "--junit", &a, &b, "--log", &log]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
    let marker = ingest_marker(&stderr);
    assert!(
        marker.ends_with(
            " junit_files=2 log_files=1 lcov_files=0 cases=6 failed=2 errors=1 skipped=1 passed=2 stored=3 \
             runs=1 window=10 seq=0 surface=cli"
        ),
        "{marker}"
    );
    assert!(stdout.contains("ingested 3 failing case(s) and 0 lcov file(s) -> "), "{stdout}");

    // The same reports as repeated flags, in another order, plus lcov: a
    // re-ingest is the next run of the window (CC.9b), so both runs' failing
    // cases are kept.
    let out = r.glia(&["tests", "ingest", r.path(), "--lcov", &lcov, "--junit", &b, "--junit", &a]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let marker = ingest_marker(&stderr);
    assert!(marker.contains(" junit_files=2 log_files=0 lcov_files=1 ") && marker.contains(" stored=2 "), "{marker}");
    assert!(marker.ends_with(" runs=2 window=10 seq=1 surface=cli"), "{marker}");
    assert!(stdout.contains("ingested 2 failing case(s) and 1 lcov file(s) -> "), "{stdout}");
    assert_eq!(r.snapshot_file(TESTS_CASES_FILE).lines().count(), 5);

    // `--window 1` replaces the snapshot, as 0.5.0 did.
    let out = r.glia(&["tests", "ingest", r.path(), "--junit", &a, &b, "--window", "1"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(ingest_marker(&text(&out.stderr)).ends_with(" runs=1 window=1 seq=2 surface=cli"));
    assert_eq!(r.snapshot_file(TESTS_CASES_FILE).lines().count(), 2);
}

/// Ingest is a separate step: the build afterwards still succeeds (its FAIL /
/// COVERAGE cells are LF.6b / LF.6c's engine tests), and `analyze` never
/// ingests on its own.
#[test]
fn the_build_still_succeeds_after_an_ingest() {
    let r = Scratch::new("build");
    let out = r.glia(&["analyze", r.path(), "--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(!tests_dir(&r.top).exists(), "analyze never ingests");

    let junit = r.report("pytest-junit.xml", PYTEST_JUNIT);
    let lcov = r.report("coverage.lcov", LCOV);
    let out = r.glia(&["tests", "ingest", r.path(), "--junit", &junit, "--lcov", &lcov]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));

    let out = r.glia(&["analyze", r.path(), "--format", "json"]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("[tests] snapshot incomplete"), "the snapshot reads back whole:\n{stderr}");
    let graph: Value = serde_json::from_str(stdout.trim()).expect("analyze json parses");
    let qnames: Vec<&str> = graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| n["qname"].as_str())
        .collect();
    for want in ["api::app::handler", "tests::test_app::test_boom"] {
        assert!(qnames.contains(&want), "{want} missing from {qnames:?}");
    }
}
