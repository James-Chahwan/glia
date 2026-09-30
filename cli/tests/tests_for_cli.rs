//! LE.3b — `glia tests-for`, the CLI surface of the engine's `tests_for`,
//! driving the real binary over the `py-test-cells` tree (`price <- place <-
//! audited_place <- test_audited_place`, `test_price -> price`, module TESTS
//! pairings) in a scratch dir.
//!
//! The engine's `[tests-for] seeds=.. tests=..` stderr line is the fired_on
//! marker; asserting it here makes it a tested contract. Grep it from a run:
//! `glia tests-for <repo> <qname> 2>&1 >/dev/null | grep '^\[tests-for\] seeds='`.
//! CC.9a's `[tests-for] signals ..` line follows it unless `--no-signals`.
//!
//! The `--base` test needs a `git` binary: without one it FAILS with a
//! message saying so, never skips. Its git calls run hermetically (fixed
//! identity, no signing, no system or global config, `HOME` in the scratch
//! dir), as `engine/tests/git_fixture` does.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const FILES: [(&str, &str); 4] = [
    (
        "shop/orders/service.py",
        "def price(order):\n    return sum(i[\"p\"] for i in order[\"items\"])\n\n\n\
def place(order):\n    total = price(order)\n    return {\"total\": total}\n",
    ),
    (
        "shop/orders/audit.py",
        "from shop.orders.service import place\n\n\ndef audited_place(order):\n    return place(order)\n",
    ),
    (
        "shop/tests/test_service.py",
        "from shop.orders.service import price\n\n\n\
def test_price():\n    assert price({\"items\": [{\"p\": 2}]}) == 2\n",
    ),
    (
        "shop/tests/test_audit.py",
        "from shop.orders.audit import audited_place\n\n\n\
def test_audited_place():\n    assert audited_place({\"items\": []})[\"total\"] == 0\n",
    ),
];

const PRICE: &str = "shop::orders::service::price";
const MARKER: &str = "[tests-for] seeds=1 tests=3 fact=1 derived=1 heuristic=1 untested=0 files=2";
/// CC.9a: the signal pass ran and found nothing to rank by.
const NO_SIGNALS: &str =
    "[tests-for] signals failed_last_run=0 on_failing_trace=0 cochange=0 cochange_only=0 omitted=0";
/// A JUnit report where test_audited_place (shop/tests/test_audit.py:4) failed.
const AUDIT_FAILED_JUNIT: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<testsuites><testsuite name=\"pytest\" tests=\"2\" failures=\"1\">\n\
<testcase classname=\"shop.tests.test_audit\" name=\"test_audited_place\" file=\"shop/tests/test_audit.py\" line=\"4\">\
<failure message=\"KeyError: total\">shop/tests/test_audit.py:5: KeyError</failure></testcase>\n\
<testcase classname=\"shop.tests.test_service\" name=\"test_price\" file=\"shop/tests/test_service.py\" line=\"4\"/>\n\
</testsuite></testsuites>\n";
const TWO_FILES: &str = "shop/tests/test_audit.py\nshop/tests/test_service.py\n";
/// A unified diff editing `price`'s body.
const PRICE_DIFF: &str = "--- a/shop/orders/service.py\n+++ b/shop/orders/service.py\n@@ -1,2 +1,2 @@\n \
def price(order):\n-    return sum(i[\"p\"] for i in order[\"items\"])\n\
+    return sum(i[\"p\"] * 1 for i in order[\"items\"])\n";

/// A scratch dir holding the tree (`repo/`), an empty global git config and a
/// `HOME`; removed on drop (`cli` has no dev-dependencies, so no `tempfile`).
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
    fn shop(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-le3b-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("scratch HOME dir");
        let gitconfig = root.join("gitconfig");
        let s = Scratch {
            root,
            top,
            home,
            gitconfig,
        };
        std::fs::create_dir_all(&s.top).expect("scratch repo dir");
        std::fs::write(&s.gitconfig, "").expect("empty gitconfig");
        for (rel, text) in FILES {
            s.write(rel, text);
        }
        s
    }

    fn path(&self) -> &str {
        self.top.to_str().expect("utf-8 scratch path")
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.top.join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("fixture dir");
        std::fs::write(&p, text).expect("fixture write");
    }

    /// `cmd` with this scratch dir's hermetic git environment and no layout
    /// persisted.
    fn hermetic(&self, mut cmd: Command) -> Command {
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("HOME", &self.home)
            .env("GLIA_NO_PERSIST", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        cmd
    }

    fn git(&self, args: &[&str]) {
        let mut cmd = Command::new("git");
        cmd.args([
            "-c",
            "user.name=glia",
            "-c",
            "user.email=glia@example.invalid",
        ])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .arg("-C")
        .arg(&self.top)
        .args(args);
        let out = self.hermetic(cmd).output().unwrap_or_else(|e| {
            panic!("LE.3b tests-for --base needs a `git` binary on PATH; running it failed: {e}")
        });
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `glia tests-for <repo> <args>`.
    fn tests_for(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
        cmd.arg("tests-for").arg(self.path()).args(args);
        self.hermetic(cmd).output().expect("run glia")
    }

    /// `glia tests ingest <repo> --junit <report>`, the report written
    /// beside the repo from `xml`.
    fn ingest_junit(&self, xml: &str) {
        let report = self.root.join("junit.xml");
        std::fs::write(&report, xml).expect("write the report");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
        cmd.args(["tests", "ingest", self.path(), "--junit"])
            .arg(&report);
        let out = self.hermetic(cmd).output().expect("run glia tests ingest");
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn files_only_prints_exactly_the_two_files() {
    let s = Scratch::shop("files");
    let out = s.tests_for(&[PRICE, "--files-only"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), TWO_FILES);
    assert!(
        stderr(&out).lines().any(|l| l == MARKER),
        "{}",
        stderr(&out)
    );
}

#[test]
fn json_parses_into_the_answer() {
    let s = Scratch::shop("json");
    let out = s.tests_for(&[PRICE, "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let keys: Vec<&str> = v
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    let mut want = vec![
        "seeds",
        "tests",
        "omitted",
        "test_files",
        "untested",
        "unresolved",
        "absence",
    ];
    want.sort_unstable();
    assert_eq!(keys, want);
    assert_eq!(v["seeds"], serde_json::json!([PRICE]));
    let rows: Vec<(&str, &str, u64)> = v["tests"]
        .as_array()
        .expect("tests list")
        .iter()
        .map(|t| {
            (
                t["qname"].as_str().unwrap_or(""),
                t["tier"].as_str().unwrap_or(""),
                t["depth"].as_u64().unwrap_or(99),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("shop::tests::test_service::test_price", "fact", 1),
            ("shop::tests::test_audit::test_audited_place", "derived", 3),
            ("shop::tests::test_service", "heuristic", 2),
        ]
    );
    // A witness hop is `[qname, category]`; the first row's line is 1-based.
    assert_eq!(v["tests"][0]["path"], serde_json::json!([[PRICE, "TESTS"]]));
    assert_eq!(v["tests"][0]["line"], 4);
    assert!(v["absence"].is_null());
    // CC.9a: no failure or history ingested, so no row carries a signal.
    assert_eq!(v["omitted"], 0);
    assert_eq!(v["tests"][0]["signals"], serde_json::json!([]));
    assert!(v["tests"][0]["cochange_permille"].is_null());
    assert!(
        stderr(&out).lines().any(|l| l == NO_SIGNALS),
        "{}",
        stderr(&out)
    );
}

/// CC.9a: a failure ingested with `glia tests ingest` ranks its test first,
/// and `--limit 1` keeps exactly that row: one table row, its file alone
/// under `--files-only`, the cut counted in the signals line.
#[test]
fn limit_keeps_the_failing_test() {
    let s = Scratch::shop("limit");
    s.ingest_junit(AUDIT_FAILED_JUNIT);

    let out = s.tests_for(&[PRICE, "--limit", "1"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    let rows: Vec<&str> = text.lines().filter(|l| l.starts_with("| 1 ") || l.starts_with("| 2 ")).collect();
    assert_eq!(rows.len(), 1, "{text}");
    assert!(
        rows[0].starts_with("| 1 | derived | `shop::tests::test_audit::test_audited_place` | FUNCTION | shop/tests/test_audit.py:4 | 3 | failed_last_run |"),
        "{text}"
    );
    assert!(text.contains("- omitted: 2 (past --limit)"), "{text}");
    let err = stderr(&out);
    assert!(
        err.lines().any(|l| l == "[tests-for] seeds=1 tests=1 fact=0 derived=1 heuristic=0 untested=0 files=1"),
        "{err}"
    );
    assert!(
        err.lines().any(|l| l == "[tests-for] signals failed_last_run=1 on_failing_trace=0 cochange=0 cochange_only=0 omitted=2"),
        "{err}"
    );

    let out = s.tests_for(&[PRICE, "--limit", "1", "--files-only"]);
    assert_eq!(stdout(&out), "shop/tests/test_audit.py\n", "{}", stderr(&out));

    // --no-signals: the structural order, no signals line; the pinned
    // marker still reads the same.
    let out = s.tests_for(&[PRICE, "--no-signals", "--json"]);
    let v: Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(
        v["tests"][0]["qname"],
        "shop::tests::test_service::test_price"
    );
    let err = stderr(&out);
    assert!(err.lines().any(|l| l == MARKER), "{err}");
    assert!(!err.contains("[tests-for] signals"), "{err}");

    let out = s.tests_for(&[PRICE, "--limit", "0"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("limit of 0"), "{}", stderr(&out));
}

#[test]
fn table_mode_lists_tiers_and_files() {
    let s = Scratch::shop("table");
    let out = s.tests_for(&["price", "--no-module-level"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("- tests: 2 (fact 1, derived 1, heuristic 0) in 2 files"),
        "{text}"
    );
    assert!(
        text.contains("| 1 | fact | `shop::tests::test_service::test_price` | FUNCTION | shop/tests/test_service.py:4 | 1 |"),
        "{text}"
    );
    assert!(
        text.contains("`test_audited_place` -[") && text.contains("]-> `place` -[CALLS]-> `price`"),
        "{text}"
    );
    assert!(
        text.contains("- shop/tests/test_audit.py\n- shop/tests/test_service.py"),
        "{text}"
    );
}

#[test]
fn untested_and_unknown_seeds_answer_with_their_absence() {
    let s = Scratch::shop("absent");
    s.write(
        "shop/orders/refund.py",
        "def refund(order):\n    return 0\n",
    );
    let out = s.tests_for(&["shop::orders::refund::refund"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("- untested: `shop::orders::refund::refund`"),
        "{text}"
    );
    assert!(
        text.contains("_(no tests: no test case reaches `shop::orders::refund::refund`"),
        "{text}"
    );
    assert!(
        stderr(&out).contains("[absence] primitive=tests_for reason=no_edges"),
        "{}",
        stderr(&out)
    );

    let out = s.tests_for(&["no_such_symbol", "--files-only"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(
        stderr(&out).contains("[absence] primitive=tests_for reason=unknown_symbol"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn diff_from_stdin_equals_the_qname_answer() {
    let s = Scratch::shop("diff");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
    cmd.args(["tests-for", s.path(), "--diff", "-", "--files-only"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = s.hermetic(cmd).spawn().expect("spawn glia");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(PRICE_DIFF.as_bytes())
        .expect("write the diff");
    let out = child.wait_with_output().expect("glia exits");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), TWO_FILES);
    assert!(
        stderr(&out).lines().any(|l| l == MARKER),
        "{}",
        stderr(&out)
    );

    let file = s.root.join("price.diff");
    std::fs::write(&file, PRICE_DIFF).expect("write the diff file");
    let out = s.tests_for(&["--diff", file.to_str().expect("utf-8"), "--files-only"]);
    assert_eq!(stdout(&out), TWO_FILES, "{}", stderr(&out));
}

#[test]
fn base_mode_seeds_from_the_working_tree_change() {
    let s = Scratch::shop("base");
    s.git(&["init", "-q"]);
    s.git(&["add", "-A"]);
    s.git(&["commit", "-q", "-m", "shop"]);
    s.write(
        "shop/orders/service.py",
        &FILES[0].1.replace("i[\"p\"] for", "i[\"p\"] * 1 for"),
    );
    let out = s.tests_for(&["--base", "HEAD", "--files-only"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), TWO_FILES);
    assert!(
        stderr(&out).lines().any(|l| l == MARKER),
        "{}",
        stderr(&out)
    );

    let out = s.tests_for(&["--base", "HEAD", "--with", s.path()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("does not take --with"),
        "{}",
        stderr(&out)
    );
    let out = s.tests_for(&["--base", "no-such-rev"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("no-such-rev"), "{}", stderr(&out));
}

#[test]
fn exactly_one_seed_source() {
    let s = Scratch::shop("usage");
    for args in [
        &[][..],
        &[PRICE, "--diff", "-"][..],
        &["--diff", "-", "--base", "HEAD"][..],
    ] {
        let out = s.tests_for(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            stderr(&out).contains("give exactly one seed source"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
    let out = s.tests_for(&[PRICE, "--json", "--files-only"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "clap refuses --json with --files-only"
    );
}
