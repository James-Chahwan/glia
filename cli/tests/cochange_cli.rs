//! CC.11c — `glia cochange`, the CLI surface of the engine's
//! `cochange::{cochange_multi, cochange_vs_rev}` (CC.11a / CC.11b), driving the
//! real binary over the engine tests' two acceptance trees in a scratch dir,
//! each with a synthetic history snapshot (`write_history`, no git log), so
//! every count is exact:
//!
//! - pairwise: svc/admin.py has 45 commits — 3 with svc/report.py (report's
//!   only 3), 20 with web/page.ts (page's only 20), 22 alone. admin imports
//!   report; nothing links page to admin.
//! - multi (`--base`): a.py + a_test.py + db/migrate.sql together 4 times,
//!   a.py + b.py 6 times, a_test.py alone twice. a.py imports b.py; the
//!   migration is no MODULE and nothing links it.
//!
//! The engine's `[cochange-suggest] ...` stderr line is the fired_on marker;
//! asserting it here makes it a tested contract. Grep it from a run:
//! `glia cochange <repo> <file> 2>&1 >/dev/null | grep '^\[cochange-suggest\] '`.
//!
//! The `--base` test needs a `git` binary: without one it FAILS with a message
//! saying so, never skips. Its git calls run hermetically (fixed identity, no
//! signing, no system or global config, `HOME` in the scratch dir).

use std::path::PathBuf;
use std::process::{Command, Output};

use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};
use serde_json::Value;

const ADMIN: &str = "svc/admin.py";
const REPORT: &str = "svc/report.py";
const PAGE: &str = "web/page.ts";

const PAIRWISE_FILES: [(&str, &str); 4] = [
    (
        ADMIN,
        "from svc.report import format_report\n\n\ndef admin_summary(rows):\n    return format_report(rows)\n",
    ),
    (
        REPORT,
        "def format_report(rows):\n    return \", \".join(rows)\n",
    ),
    (
        PAGE,
        "export function renderPage(title: string): string {\n  return `<h1>${title}</h1>`;\n}\n",
    ),
    ("README.md", "# demo\n"),
];

const MA: &str = "a.py";
const MA_TEST: &str = "a_test.py";
const MB: &str = "b.py";
const MIGRATE: &str = "db/migrate.sql";
const MA_PY: &str = "from b import helper\n\n\ndef run():\n    return helper()\n";
const MA_TEST_PY: &str = "from a import run\n\n\ndef test_run():\n    assert run() == 1\n";

const MULTI_FILES: [(&str, &str); 4] = [
    (MA, MA_PY),
    (MA_TEST, MA_TEST_PY),
    (MB, "def helper():\n    return 1\n"),
    (MIGRATE, "ALTER TABLE t ADD COLUMN c INT;\n"),
];

/// The acceptance row for `svc/report.py`: admin changed in all 3 of report's
/// commits, and admin imports report.
const REPORT_ROW: &str = "| svc/admin.py | 1.00 (3/3) | 3 | svc/report.py | direct | pairwise |";
/// The marker of that query: one file, the one CO_CHANGES edge touching it a
/// candidate, one row, no multi-file antecedent over the 45 commits.
const REPORT_MARKER: &str = "[cochange-suggest] query_files=1 unmapped=0 candidates=1 rows=1 unlinked=0 source=multi antecedents=0 commits_scanned=45";

const T0: i64 = 1_767_225_600;

/// Commits, newest first: `n` commits per group touching every file of it,
/// each with its own sha and time.
fn commits(groups: &[(&[&str], usize)]) -> Vec<HistoryCommit> {
    let mut out = Vec::new();
    let mut i = 0usize;
    for (files, n) in groups {
        for _ in 0..*n {
            i += 1;
            out.push(HistoryCommit {
                c: format!("{i:03}{}", "0".repeat(37)),
                t: T0 + i64::try_from(i).expect("small") * 3_600,
                files: files
                    .iter()
                    .map(|p| HistoryFile {
                        p: (*p).to_string(),
                        a: Some(1),
                        d: Some(0),
                        from: None,
                    })
                    .collect(),
            });
        }
    }
    out.reverse();
    out
}

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
    /// `files` under a fresh scratch repo dir, with `history` as its history
    /// snapshot when it is `Some`.
    fn new(name: &str, files: &[(&str, &str)], history: Option<&[HistoryCommit]>) -> Self {
        let root = std::env::temp_dir().join(format!("glia-cc11c-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("scratch HOME dir");
        std::fs::create_dir_all(&top).expect("scratch repo dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        let s = Scratch {
            root,
            top,
            home,
            gitconfig,
        };
        for (rel, text) in files {
            s.write(rel, text);
        }
        if let Some(h) = history {
            s.snapshot(h);
        }
        s
    }

    /// The pairwise acceptance tree and history.
    fn pairwise(name: &str) -> Self {
        let h = commits(&[(&[ADMIN, REPORT], 3), (&[ADMIN, PAGE], 20), (&[ADMIN], 22)]);
        Self::new(name, &PAIRWISE_FILES, Some(&h))
    }

    fn path(&self) -> &str {
        self.top.to_str().expect("utf-8 scratch path")
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.top.join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("fixture dir");
        std::fs::write(&p, text).expect("fixture write");
    }

    fn snapshot(&self, history: &[HistoryCommit]) {
        let head = history
            .first()
            .map_or_else(|| "0".repeat(40), |c| c.c.clone());
        write_history(
            &self.top,
            HistoryMeta::new(head, 2000, None, String::new()),
            history,
            &[],
        )
        .expect("write snapshot");
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
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .arg("-C")
        .arg(&self.top)
        .args(args);
        let out = self.hermetic(cmd).output().unwrap_or_else(|e| {
            panic!("CC.11c cochange --base needs a `git` binary on PATH; running it failed: {e}")
        });
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `glia cochange <repo> <args>`, relaying the marker lines so
    /// `-- --nocapture | grep '^\[cochange-suggest\] '` sees them.
    fn cochange(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
        cmd.arg("cochange").arg(self.path()).args(args);
        let out = self.hermetic(cmd).output().expect("run glia");
        for line in stderr(&out).lines() {
            if line.starts_with("[cochange-suggest] ") {
                eprintln!("{line}");
            }
        }
        out
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn json(o: &Output) -> Value {
    serde_json::from_str(&stdout(o)).expect("stdout is JSON")
}

/// The files of a JSON answer's rows, in order.
fn row_files(v: &Value) -> Vec<String> {
    v["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["file"].as_str().expect("file").to_string())
        .collect()
}

/// `svc/report.py` changed: admin changed with it every time (3/3), and
/// admin imports it. Exit 0, the row, the header and the marker.
#[test]
fn report_predicts_admin() {
    let s = Scratch::pairwise("report");
    let out = s.cochange(&[REPORT]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("# glia cochange"), "{text}");
    assert!(
        text.contains("| file | confidence | support | because | static link | source |"),
        "{text}"
    );
    assert!(text.lines().any(|l| l == REPORT_ROW), "{text}");
    assert!(text.contains("- unmapped: none"), "{text}");
    assert!(
        stderr(&out).lines().any(|l| l == REPORT_MARKER),
        "{}",
        stderr(&out)
    );
}

/// admin -> page (20/45) is the one unlinked row: `--unlinked-only` lists
/// page only, at any floor; the linked converse admin -> report (3/45) shows
/// under `--min-confidence 0` without it.
#[test]
fn unlinked_only_lists_page() {
    let s = Scratch::pairwise("unlinked");
    for floor in ["0.3", "0"] {
        let out = s.cochange(&[
            ADMIN,
            "--unlinked-only",
            "--min-confidence",
            floor,
            "--json",
        ]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert_eq!(row_files(&json(&out)), [PAGE], "floor {floor}");
    }
    let out = s.cochange(&[ADMIN, "--unlinked-only"]);
    let text = stdout(&out);
    assert!(
        text.lines()
            .any(|l| l == "| web/page.ts | 0.44 (20/45) | 20 | svc/admin.py | none | pairwise |"),
        "{text}"
    );
    assert!(
        text.contains("no static link joins them (a blind spot, or coupling outside code)"),
        "{text}"
    );

    let out = s.cochange(&[ADMIN, "--min-confidence", "0", "--json"]);
    assert_eq!(row_files(&json(&out)), [PAGE, REPORT]);
    let out = s.cochange(&[ADMIN, "--min-confidence", "0", "--top", "1", "--json"]);
    assert_eq!(row_files(&json(&out)), [PAGE]);
}

/// `--json` is the engine's answer: {query_files, unmapped, rows, absence}.
#[test]
fn json_keys() {
    let s = Scratch::pairwise("json");
    let out = s.cochange(&[REPORT, "README.md", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = json(&out);
    let mut keys: Vec<&str> = v
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["absence", "query_files", "rows", "unmapped"]);
    // serde_json's map sorts; the printed text keeps the engine's field order.
    let text = stdout(&out);
    let at: Vec<usize> = [
        "\"query_files\":",
        "\"unmapped\":",
        "\"rows\":",
        "\"absence\":",
    ]
    .iter()
    .map(|k| text.find(k).unwrap_or(usize::MAX))
    .collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "field order: {text}");
    assert_eq!(v["query_files"], serde_json::json!(["README.md", REPORT]));
    assert_eq!(v["unmapped"], serde_json::json!(["README.md"]));
    assert!(v["absence"].is_null());
    let row = &v["rows"][0];
    let mut row_keys: Vec<&str> = row
        .as_object()
        .expect("a row object")
        .keys()
        .map(String::as_str)
        .collect();
    row_keys.sort_unstable();
    assert_eq!(
        row_keys,
        [
            "antecedent",
            "antecedent_commits",
            "confidence_permille",
            "file",
            "link",
            "module_qname",
            "note",
            "source",
            "support",
            "tier"
        ]
    );
    assert_eq!(
        (
            row["file"].as_str(),
            row["confidence_permille"].as_u64(),
            row["tier"].as_str()
        ),
        (Some(ADMIN), Some(1000), Some("heuristic"))
    );

    // The table lists the unmapped query file after the rows.
    let text = stdout(&s.cochange(&[REPORT, "README.md"]));
    assert!(text.contains("- unmapped: `README.md`"), "{text}");
}

/// Query paths are repo-relative: `./` and an absolute path under the repo
/// give the same row; a path outside the repo is a usage error, never
/// silently unmapped.
#[test]
fn paths_are_repo_relative() {
    let s = Scratch::pairwise("paths");
    let absolute = s.top.join(REPORT);
    for given in [
        "./svc/report.py".to_string(),
        absolute.to_str().expect("utf-8").to_string(),
    ] {
        let out = s.cochange(&[&given]);
        assert_eq!(out.status.code(), Some(0), "{given}: {}", stderr(&out));
        assert!(stdout(&out).lines().any(|l| l == REPORT_ROW), "{given}");
    }
    let outside = s.root.join("elsewhere.py");
    for given in [outside.to_str().expect("utf-8"), "../elsewhere.py"] {
        let out = s.cochange(&[given]);
        assert_eq!(out.status.code(), Some(2), "{given}: {}", stdout(&out));
        assert!(
            stderr(&out).contains("is no file under the repo"),
            "{}",
            stderr(&out)
        );
        assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    }
}

/// Usage errors exit 2: files AND `--base`, neither, `--base` with `--with`,
/// `--min-confidence` outside [0, 1].
#[test]
fn usage_errors_exit_2() {
    let s = Scratch::pairwise("usage");
    let other = s.root.join("home");
    let other = other.to_str().expect("utf-8");
    for args in [
        vec![REPORT, "--base", "HEAD"],
        vec![],
        vec!["--base", "HEAD", "--with", other],
        vec![REPORT, "--min-confidence", "1.5"],
        vec![REPORT, "--min-confidence", "-0.1"],
        vec![REPORT, "--min-confidence", "high"],
    ] {
        let out = s.cochange(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stdout(&out));
        assert!(stdout(&out).is_empty(), "{args:?}: {}", stdout(&out));
    }
    let out = s.cochange(&[REPORT, "--base", "HEAD"]);
    assert!(stderr(&out).contains("exactly one of"), "{}", stderr(&out));
}

/// No snapshot, so no co-change history: exit 1, the absence naming
/// `glia history sync`.
#[test]
fn no_snapshot_exits_1_with_the_sync_hint() {
    let s = Scratch::new("nohistory", &PAIRWISE_FILES, None);
    let out = s.cochange(&[ADMIN]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("glia history sync"), "{text}");
    assert!(text.contains("_(no co-change suggestions)_"), "{text}");
    let v = json(&s.cochange(&[ADMIN, "--json"]));
    assert_eq!(v["absence"]["reason"], "no_history");
    assert_eq!(v["rows"], serde_json::json!([]));

    // History, but no rule over the floors: no_match, naming them.
    let s = Scratch::pairwise("nomatch");
    let out = s.cochange(&[REPORT, "--min-support", "4"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stdout(&out).contains("min_support=4"), "{}", stdout(&out));
}

/// `--base`: the working tree's change against a rev is the query. Editing
/// a.py and a_test.py predicts the migration (a multi rule, 4/4, unlinked)
/// over the pairwise a.py -> b.py (6/10). A clean tree exits 1; an unknown
/// rev exits 2.
#[test]
fn base_uses_the_working_tree_change() {
    let s = Scratch::new("base", &MULTI_FILES, None);
    s.git(&["init", "-q"]);
    s.git(&["add", "-A"]);
    s.git(&["commit", "-q", "-m", "init"]);
    s.snapshot(&commits(&[
        (&[MA, MA_TEST, MIGRATE], 4),
        (&[MA, MB], 6),
        (&[MA_TEST], 2),
    ]));

    let clean = s.cochange(&["--base", "HEAD"]);
    assert_eq!(clean.status.code(), Some(1), "{}", stderr(&clean));
    assert!(
        stdout(&clean).contains("the working tree has no change against HEAD"),
        "{}",
        stdout(&clean)
    );

    s.write(MA, &format!("{MA_PY}# edited\n"));
    s.write(MA_TEST, &format!("{MA_TEST_PY}# edited\n"));
    let out = s.cochange(&["--base", "HEAD"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("the working tree's change against `HEAD`"),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|l| l == "| db/migrate.sql | 1.00 (4/4) | 4 | a.py, a_test.py | none | multi |"),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|l| l == "| b.py | 0.60 (6/10) | 6 | a.py | direct | pairwise |"),
        "{text}"
    );
    assert!(
        stderr(&out)
            .lines()
            .any(|l| l.starts_with("[cochange-suggest] query_files=2 ")
                && l.ends_with(" rows=2 unlinked=1 source=multi antecedents=1 commits_scanned=12")),
        "{}",
        stderr(&out)
    );

    let v = json(&s.cochange(&["--base", "HEAD", "--json"]));
    assert_eq!(v["query_files"], serde_json::json!([MA, MA_TEST]));
    assert_eq!(row_files(&v), [MIGRATE, MB]);
    assert_eq!(v["rows"][0]["module_qname"], "");

    let bad = s.cochange(&["--base", "no-such-rev"]);
    assert_eq!(bad.status.code(), Some(2), "{}", stdout(&bad));
    assert!(stderr(&bad).contains("no-such-rev"), "{}", stderr(&bad));
}
