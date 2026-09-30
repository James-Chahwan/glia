//! LF.6a: the test-report snapshot writer — JUnit XML, CI-log failure lines
//! and lcov, normalized into `<repo>/.glia/test-snapshot/`.
//!
//! The sample reports under `tests/data/` are real runner output (pytest 9
//! `-o junit_family=xunit1`, `go test`, `cargo test`) or follow the producer's
//! documented shape (Surefire 3, jest-junit, go-junit-report v2, jest). None
//! of them holds a secret: the leak test assembles its tokens at run time.

use std::path::{Path, PathBuf};

use glia_snapshots::{TestsIngestOptions, parse_ci_log, parse_junit, parse_lcov, tests_ingest};
use glia_code_domain::snapshots::{
    MESSAGE_CAP, META_FILE, SECRET_NEEDLES, STATUS_ERROR, STATUS_FAILED, TESTS_CASES_FILE, TESTS_LCOV_FILE, TRACE_CAP,
    TestCaseRecord, read_tests, tests_dir,
};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

fn read_data(name: &str) -> Vec<u8> {
    std::fs::read(data(name)).expect("sample report")
}

fn repo() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// The snapshot's three files, in write order.
fn snapshot_bytes(root: &Path) -> [Vec<u8>; 3] {
    [TESTS_CASES_FILE, TESTS_LCOV_FILE, META_FILE].map(|f| std::fs::read(tests_dir(root).join(f)).expect("snapshot file"))
}

#[test]
fn pytest_junit() {
    let (rows, counts) = parse_junit(&read_data("pytest-junit.xml"), "pytest-junit.xml").unwrap();
    assert_eq!((counts.cases, counts.failed, counts.errors, counts.skipped, counts.passed), (3, 1, 0, 0, 2));
    assert_eq!(rows.len(), 1);
    let boom = &rows[0];
    assert_eq!(boom.source, "junit");
    assert_eq!(boom.report, "pytest-junit.xml");
    assert_eq!(boom.suite.as_deref(), Some("pytest"));
    assert_eq!(boom.classname.as_deref(), Some("tests.test_app"));
    assert_eq!(boom.name, "test_boom");
    assert_eq!(boom.file.as_deref(), Some("tests/test_app.py"));
    // pytest wrote line="3" (0-based): `def test_boom():` is line 4.
    assert_eq!(boom.line, Some(4));
    assert_eq!(boom.status, STATUS_FAILED);
    assert_eq!(boom.message.as_deref(), Some("ValueError: boom"));
    let trace = boom.trace.as_deref().unwrap();
    assert!(trace.starts_with("def test_boom():\n>       raise ValueError(\"boom\")"), "entities unescaped: {trace}");
    assert!(trace.ends_with("tests/test_app.py:5: ValueError"), "{trace}");
    assert!(!boom.redacted);
}

#[test]
fn surefire() {
    let (rows, counts) = parse_junit(&read_data("surefire.xml"), "target/surefire-reports/TEST-x.xml").unwrap();
    assert_eq!((counts.cases, counts.failed, counts.errors, counts.skipped, counts.passed), (3, 0, 1, 1, 1));
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.classname.as_deref(), Some("com.example.UserServiceTest"));
    assert_eq!(r.name, "testGetUser");
    assert_eq!(r.suite.as_deref(), Some("com.example.UserServiceTest"));
    assert_eq!(r.status, STATUS_ERROR);
    assert_eq!(r.message.as_deref(), Some("Connection refused (user-db:5432)"));
    assert_eq!((r.file.as_deref(), r.line), (None, None));
    let trace = r.trace.as_deref().unwrap();
    assert!(trace.contains("\tat com.example.UserService.getUser(UserService.java:42)"), "CDATA kept: {trace}");
    // <system-out> is never read.
    assert!(!format!("{rows:?}").contains("pooled session"));
}

#[test]
fn jest_junit() {
    let (rows, counts) = parse_junit(&read_data("jest-junit.xml"), "reports/junit.xml").unwrap();
    assert_eq!((counts.cases, counts.failed, counts.passed), (2, 1, 1));
    let r = &rows[0];
    assert_eq!(r.file, None, "jest-junit writes no file attribute");
    assert_eq!(r.suite.as_deref(), Some("UserService"));
    assert_eq!(r.name, "UserService getUser returns the user");
    // No message attribute: the body's first line stands in.
    assert_eq!(r.message.as_deref(), Some("Error: expect(received).toBe(expected) // Object.is equality"));
    assert!(r.trace.as_deref().unwrap().contains("at Object.<anonymous> (src/user.test.ts:10:5)"), "{r:?}");
}

#[test]
fn go_junit() {
    let (rows, counts) = parse_junit(&read_data("go-junit.xml"), "go-junit.xml").unwrap();
    assert_eq!((counts.cases, counts.failed, counts.passed), (4, 3, 1));
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["TestGetUser", "TestList", "TestList/empty"]);
    for r in &rows {
        assert_eq!(r.classname.as_deref(), Some("github.com/acme/api/users"), "{r:?}");
    }
    assert_eq!(rows[0].message.as_deref(), Some("Failed"));
    assert_eq!(rows[0].trace.as_deref(), Some("users_test.go:6: expected 200, got 500\n    users_test.go:7: see logs"));
    assert_eq!(rows[1].trace, None, "an empty failure body stores no trace");
}

#[test]
fn passed_and_skipped_only_counted() {
    let xml = br#"<testsuite name="s"><testcase classname="c" name="a"/><testcase classname="c" name="b"></testcase>
        <testcase name="x"><skipped/></testcase><testcase name="y"><skipped message="later">why</skipped></testcase>
        <testcase name="z"><skipped type="pytest.xfail" message="xfail"/></testcase></testsuite>"#;
    let (rows, counts) = parse_junit(xml, "r.xml").unwrap();
    assert!(rows.is_empty());
    assert_eq!((counts.cases, counts.failed, counts.errors, counts.skipped, counts.passed), (5, 0, 0, 3, 2));

    let root = repo();
    std::fs::write(root.path().join("r.xml"), xml).unwrap();
    let opts = TestsIngestOptions { junit: vec![root.path().join("r.xml")], ..Default::default() };
    let summary = tests_ingest(root.path(), &opts).unwrap();
    assert_eq!((summary.cases, summary.failed, summary.skipped, summary.passed, summary.stored), (5, 0, 3, 2, 0));
    let snap = read_tests(root.path()).expect("snapshot");
    assert!(snap.cases.is_empty());
    assert_eq!((snap.meta.cases_total, snap.meta.skipped, snap.meta.passed, snap.meta.failed), (5, 3, 2, 0));
    assert_eq!(snap.meta.reports, ["r.xml"], "a report under the repo is stored repo-relative");
}

#[test]
fn ci_log_shapes() {
    let text = String::from_utf8(read_data("ci.log")).unwrap();
    let rows = parse_ci_log(&text, "ci.log");
    type Row<'a> = (Option<&'a str>, Option<&'a str>, &'a str, Option<&'a str>);
    let got: Vec<Row> = rows
        .iter()
        .map(|r| (r.file.as_deref(), r.classname.as_deref().or(r.suite.as_deref()), r.name.as_str(), r.message.as_deref()))
        .collect();
    assert_eq!(
        got,
        [
            (Some("tests/test_app.py"), Some("tests.test_app"), "test_boom", Some("ValueError: boom")),
            (
                Some("tests/test_app.py"),
                Some("tests.test_app.TestUser"),
                "test_name",
                Some("AssertionError: assert 'a' ==...")
            ),
            (None, Some("github.com/acme/api/orders"), "TestCreate", Some("orders_test.go:6: bad total")),
            (None, Some("github.com/acme/api/users"), "TestGetUser", Some("users_test.go:6: expected 200, got 500")),
            (None, Some("github.com/acme/api/users"), "TestList", None),
            (None, Some("github.com/acme/api/users"), "TestList/empty", Some("users_test.go:12: list was nil")),
            (None, Some("tests::math"), "adds", Some("assertion `left == right` failed: sum was wrong")),
            (Some("src/lib.rs"), None, "add", Some("Test executable failed (exit status: 101).")),
            (
                Some("src/user.test.ts"),
                Some("UserService › getUser"),
                "returns the user",
                Some("expect(received).toBe(expected) // Object.is equality")
            ),
        ]
    );
    assert!(rows.iter().all(|r| r.source == "log" && r.report == "ci.log" && r.status == STATUS_FAILED));
    // go: the indented lines under `--- FAIL:` are the trace.
    assert_eq!(rows[3].trace.as_deref(), Some("users_test.go:6: expected 200, got 500\nusers_test.go:7: see logs"));
    assert_eq!(rows[4].trace, None, "TestList's only indented line is its subtest's header");
    // cargo: the `---- name stdout ----` block, and a doctest's line.
    assert!(rows[6].trace.as_deref().unwrap().contains("panicked at src/lib.rs:10:21:"), "{:?}", rows[6]);
    assert_eq!(rows[7].line, Some(1));
    // jest: the block under `●`, once although the summary repeats it.
    let jest = rows[8].trace.as_deref().unwrap();
    assert!(jest.contains("> 10 |     expect(r.status).toBe(200);"), "{jest}");
    assert!(jest.ends_with("at Object.<anonymous> (src/user.test.ts:10:5)"), "{jest}");
}

#[test]
fn malformed_xml_is_per_file() {
    let root = repo();
    let r = root.path();
    std::fs::write(r.join("cut.xml"), "<testsuite name=\"s\"><testcase name=\"x\">").unwrap();
    std::fs::write(r.join("mismatch.xml"), "<testsuite><testcase name=\"x\"></testsuit>").unwrap();
    std::fs::write(r.join("good.xml"), read_data("pytest-junit.xml")).unwrap();
    let opts = TestsIngestOptions {
        junit: vec![r.join("cut.xml"), r.join("good.xml"), r.join("mismatch.xml"), r.join("absent.xml")],
        ..Default::default()
    };
    let summary = tests_ingest(r, &opts).unwrap();
    assert_eq!(summary.junit_files, 1);
    assert_eq!(summary.reports, ["good.xml"]);
    let failed: Vec<&str> = summary.report_errors.iter().map(|e| e.report.as_str()).collect();
    assert_eq!(failed.len(), 3, "{:?}", summary.report_errors);
    assert!(failed.contains(&"cut.xml") && failed.contains(&"mismatch.xml"), "{failed:?}");
    assert_eq!((summary.stored, summary.passed), (1, 2), "the good report is still ingested");

    // Nothing readable: an error, and the earlier snapshot is left alone.
    let before = snapshot_bytes(r);
    let bad = TestsIngestOptions { junit: vec![r.join("cut.xml")], ..Default::default() };
    let err = tests_ingest(r, &bad).unwrap_err();
    assert!(err.contains("cut.xml"), "{err}");
    assert_eq!(before, snapshot_bytes(r));
    assert!(tests_ingest(r, &TestsIngestOptions::default()).is_err(), "no reports given");
}

#[test]
fn lcov_parses_da_and_strips_root() {
    let text = String::from_utf8(read_data("coverage.lcov")).unwrap();
    let rows = parse_lcov(&text, Path::new("/ci/work/repo"));
    type Row<'a> = (&'a str, Option<&'a str>, &'a [[u32; 2]]);
    let got: Vec<Row> =
        rows.iter().map(|r| (r.sf.as_str(), r.rel.as_deref(), r.lines.as_slice())).collect();
    assert_eq!(
        got,
        [
            ("/ci/work/repo/api/app.py", Some("api/app.py"), &[[1, 1], [2, 1], [4, 0]][..]),
            ("/usr/lib/python3.12/json/__init__.py", None, &[[1, 1]][..]),
            ("tests/test_app.py", Some("tests/test_app.py"), &[[1, 1], [4, 1], [5, 1]][..]),
        ]
    );
}

/// A repo holding the pytest report and a tracefile whose absolute `SF:`
/// paths point into it.
fn repo_with_reports() -> (tempfile::TempDir, TestsIngestOptions) {
    let root = repo();
    let r = root.path();
    std::fs::create_dir_all(r.join("reports")).unwrap();
    std::fs::write(r.join("reports/junit.xml"), read_data("pytest-junit.xml")).unwrap();
    let lcov = String::from_utf8(read_data("coverage.lcov")).unwrap();
    let canonical = std::fs::canonicalize(r).unwrap();
    std::fs::write(r.join("reports/coverage.lcov"), lcov.replace("/ci/work/repo", &canonical.to_string_lossy())).unwrap();
    let opts = TestsIngestOptions {
        junit: vec![r.join("reports/junit.xml")],
        lcov: vec![r.join("reports/coverage.lcov")],
        run: Some("ci-1234".into()),
        ..Default::default()
    };
    (root, opts)
}

#[test]
fn ingest_writes_the_snapshot_and_gitignore() {
    let (root, opts) = repo_with_reports();
    let r = root.path();
    let summary = tests_ingest(r, &opts).unwrap();
    // The fired_on marker's numbers: junit_files=1 log_files=0 lcov_files=1
    // cases=3 failed=1 errors=0 skipped=0 passed=2 stored=1.
    assert_eq!(
        (summary.junit_files, summary.log_files, summary.lcov_files),
        (1, 0, 1)
    );
    assert_eq!(
        (summary.cases, summary.failed, summary.errors, summary.skipped, summary.passed, summary.stored),
        (3, 1, 0, 0, 2, 1)
    );
    assert_eq!(summary.reports, ["reports/coverage.lcov", "reports/junit.xml"]);
    let snap = read_tests(r).expect("complete snapshot");
    assert_eq!(snap.meta.run.as_deref(), Some("ci-1234"));
    assert_eq!(snap.cases[0].report, "reports/junit.xml");
    assert_eq!(snap.cases[0].line, Some(4));
    // Sorted by rel, else sf: the out-of-repo `/usr/...` file sorts first.
    let rels: Vec<Option<&str>> = snap.lcov.iter().map(|l| l.rel.as_deref()).collect();
    assert_eq!(rels, [None, Some("api/app.py"), Some("tests/test_app.py")]);
    assert_eq!(snap.lcov[1].lines, vec![[1, 1], [2, 1], [4, 0]]);
    let gitignore = std::fs::read_to_string(r.join(".glia/.gitignore")).unwrap();
    assert!(gitignore.lines().any(|l| l == "test-snapshot/"), "{gitignore}");
}

#[test]
fn ingest_is_byte_identical_for_same_inputs() {
    let (root, mut opts) = repo_with_reports();
    let r = root.path();
    std::fs::write(r.join("reports/ci.log"), read_data("ci.log")).unwrap();
    std::fs::write(r.join("reports/go.xml"), read_data("go-junit.xml")).unwrap();
    opts.logs = vec![r.join("reports/ci.log")];
    opts.junit.push(r.join("reports/go.xml"));
    tests_ingest(r, &opts).unwrap();
    let first = snapshot_bytes(r);
    // Same reports, given in another order, re-ingested from scratch.
    opts.junit.reverse();
    opts.reset = true;
    tests_ingest(r, &opts).unwrap();
    assert_eq!(first, snapshot_bytes(r));
    let first_run = read_tests(r).unwrap().cases.len();
    // CC.9b: a re-ingest is the next run of the window; the top level and
    // the coverage are the newest run's.
    let only_go = TestsIngestOptions { junit: vec![r.join("reports/go.xml")], ..Default::default() };
    let summary = tests_ingest(r, &only_go).unwrap();
    assert_eq!((summary.runs, summary.seq, summary.stored), (2, 1, 3));
    let snap = read_tests(r).unwrap();
    assert_eq!(snap.meta.reports, ["reports/go.xml"]);
    assert_eq!(snap.meta.runs.iter().map(|r| r.seq).collect::<Vec<_>>(), [0, 1]);
    assert_eq!((snap.cases.len(), snap.lcov.len()), (first_run + 3, 0));
    // A window of 1 replaces the snapshot, as 0.5.0 did; the seq keeps counting.
    let replace = TestsIngestOptions { window: 1, ..only_go.clone() };
    let summary = tests_ingest(r, &replace).unwrap();
    assert_eq!((summary.runs, summary.seq), (1, 2));
    let snap = read_tests(r).unwrap();
    assert_eq!((snap.cases.len(), snap.lcov.len()), (3, 0));
    // A window of 0 is an error, and the snapshot is left alone.
    let before = snapshot_bytes(r);
    let err = tests_ingest(r, &TestsIngestOptions { window: 0, ..only_go }).unwrap_err();
    assert!(err.contains("at least 1 run"), "{err}");
    assert_eq!(before, snapshot_bytes(r));
}

#[test]
fn message_truncation_is_char_boundary_safe() {
    let message = format!("{}🦀", "é".repeat(MESSAGE_CAP - 1));
    let body = "漢".repeat(TRACE_CAP + 10);
    let xml = format!(
        "<testsuite name=\"s\"><testcase name=\"t\"><failure message=\"{message}tail\">{body}</failure></testcase></testsuite>"
    );
    let (rows, _) = parse_junit(xml.as_bytes(), "r.xml").unwrap();
    let kept = rows[0].message.as_deref().unwrap();
    assert_eq!(kept.chars().count(), MESSAGE_CAP);
    assert!(kept.ends_with('🦀') && message.starts_with(kept), "cut after the 4-byte char, not inside it");
    assert_eq!(rows[0].trace.as_deref().unwrap().chars().count(), TRACE_CAP);

    let log = format!("FAILED tests/t.py::test_x - {}", "ü".repeat(MESSAGE_CAP * 2));
    let rows = parse_ci_log(&log, "ci.log");
    assert_eq!(rows[0].message.as_deref().unwrap().chars().count(), MESSAGE_CAP);
}

/// Every file under `dir`, recursively.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() { stack.push(path) } else { out.push(path) }
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// The LF.6a security correction: a token a failing test prints never reaches
/// the snapshot, the graph, the `.gmap` layout or the dense text. The Engram
/// export (`engram-export/`, outside the workspace) is a function of the same
/// `MergedGraph` this test scans in full (every node, edge and cell via its
/// `Debug` form), so a token absent here cannot reach it.
#[test]
fn secrets_never_reach_the_snapshot_graph_or_dense_text() {
    // Assembled at run time so no scanner mistakes this source for a leak.
    let stripe = format!("sk_{}_{}", "live", "4eC39HqLyjWDarjtT1zdp7dc");
    let jwt = ["eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9", "eyJzdWIiOiIxMjM0NTY3ODkwIn0", "dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"]
        .join(".");
    let aws = format!("AKIA{}", "IOSFODNN7EXAMPLE");
    let bearer = "9f8e7d6c5b4a3210ffee";
    let named = "hunter2value";
    let url_password = "s3cr3tpw";
    let secrets = [stripe.as_str(), jwt.as_str(), aws.as_str(), bearer, named, url_password];

    let root = repo();
    let r = root.path();
    std::fs::create_dir_all(r.join("api")).unwrap();
    std::fs::create_dir_all(r.join("tests")).unwrap();
    std::fs::write(r.join("api/app.py"), "def charge(amount):\n    return amount\n").unwrap();
    std::fs::write(
        r.join("tests/test_app.py"),
        "from api.app import charge\n\n\ndef test_charge():\n    assert charge(1) == 2\n\n\ndef test_refund():\n    assert False\n",
    )
    .unwrap();
    let junit = format!(
        "<testsuites name=\"pytest tests\"><testsuite name=\"pytest\"><testcase classname=\"tests.test_app\" \
         name=\"test_charge\" file=\"tests/test_app.py\" line=\"3\"><failure message=\"charge failed for {stripe}\">\
         Authorization: Bearer {bearer}\nsession {jwt}\nSTRIPE_SECRET_KEY={named}\n\
         postgres://app:{url_password}@db:5432/x\nkey {aws}\ntests/test_app.py:5: AssertionError</failure>\
         </testcase></testsuite></testsuites>"
    );
    // The raw reports live outside the repo (CI's artefact dir): only the
    // snapshot can carry their text into the repo.
    let ci = tempfile::tempdir().unwrap();
    let (junit_path, log_path) = (ci.path().join("junit.xml"), ci.path().join("ci.log"));
    std::fs::write(&junit_path, junit).unwrap();
    let log = format!(
        "FAILED tests/test_app.py::test_refund - AssertionError: token {jwt} rejected\n\
         --- FAIL: TestPay (0.00s)\n    pay_test.go:9: key={stripe}\nFAIL\tgithub.com/acme/pay\t0.01s\n"
    );
    std::fs::write(&log_path, log).unwrap();
    let inputs = [std::fs::read(&junit_path).unwrap(), std::fs::read(&log_path).unwrap()].concat();
    for secret in secrets {
        assert!(contains(&inputs, secret), "control: the reports carry {secret}");
    }

    let opts = TestsIngestOptions { junit: vec![junit_path], logs: vec![log_path], ..Default::default() };
    let summary = tests_ingest(r, &opts).unwrap();
    assert_eq!((summary.stored, summary.redacted), (3, 3));

    // 1. The snapshot: redacted, but the failure itself is kept.
    let snapshot: Vec<u8> = snapshot_bytes(r).concat();
    for secret in secrets {
        assert!(!contains(&snapshot, secret), "{secret} reached the test snapshot");
    }
    let cases = String::from_utf8(snapshot).unwrap();
    for kept in ["charge failed for ***", "Authorization: Bearer ***", "STRIPE_SECRET_KEY=***", "postgres://***@db:5432/x", "tests/test_app.py:5: AssertionError"] {
        assert!(cases.contains(kept), "control: {kept:?} kept in {cases}");
    }
    let snap = read_tests(r).unwrap();
    assert!(snap.cases.iter().all(|c: &TestCaseRecord| c.redacted), "{:?}", snap.cases);

    // 2. The graph a build of this repo produces, the layout it writes and the dense text.
    let built = glia_engine::generate_one(r.to_str().unwrap()).unwrap();
    let graph_dump = format!("{:?}", built.merged);
    assert!(graph_dump.contains("charge"), "control: the build saw the repo");
    let layout = r.join(".glia/graph");
    glia_store::write_merged_sharded(&built.merged, &layout).unwrap();
    let gmap: Vec<u8> = files_under(&layout).iter().flat_map(|p| std::fs::read(p).unwrap()).collect();
    assert!(!gmap.is_empty(), "control: a layout was written");
    let dense = format!(
        "{}\n{}",
        glia_projection_text::render_merged(&built.merged),
        glia_projection_text::render_merged_full(&built.merged)
    );
    assert!(dense.contains("charge"), "control: the dense text renders the repo");
    for secret in secrets {
        assert!(!graph_dump.contains(secret), "{secret} reached the graph");
        assert!(!contains(&gmap, secret), "{secret} reached the .gmap layout");
        assert!(!dense.contains(secret), "{secret} reached the dense text");
    }
}

/// The redaction denylist is A13.7's, not a second one: `code-domain`'s copy
/// must equal the lists in `parsers/code/extractors/src/{config,constants}.rs`
/// (see `SECRET_NEEDLES`' removal note).
#[test]
fn secret_denylist_is_a13_7s() {
    let extractors = Path::new(env!("CARGO_MANIFEST_DIR")).join("../parsers/code/extractors/src");
    for file in ["config.rs", "constants.rs"] {
        let source = std::fs::read_to_string(extractors.join(file)).unwrap();
        let start = source.find("const SECRET_NEEDLES").unwrap_or_else(|| panic!("{file}: no SECRET_NEEDLES"));
        let list = &source[start..];
        let list = &list[list.find('=').unwrap()..list.find("];").unwrap()];
        let needles: Vec<&str> = list.split('"').skip(1).step_by(2).collect();
        assert_eq!(needles, SECRET_NEEDLES, "{file}");
    }
}
