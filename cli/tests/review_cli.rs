//! CC.6b — `glia review`, the PR-report gate over the engine's review
//! (`glia_engine::review::{review_vs_rev, render_markdown}`, CC.6a / CC.6b),
//! driving the real binary over temporary git repos. These tests need a `git`
//! binary: without one they FAIL with a message saying so, never skip.
//!
//! Every git call — the fixture's and the binary's — runs hermetically: a
//! fixed identity, no signing, `main` as the initial branch, no system or
//! global config and `HOME` inside the scratch dir (the isolation of
//! `delta_cli.rs`; the cli crate has no dev-dependencies, so no `tempfile`).
//!
//! The tree is CC.6a's: two manifest projects (web/, services/api/),
//! `services/api/internal.py` `charge`, `web/app.py` `pay`,
//! `tests/test_pay.py` calling `pay`, and a `.glia/overlay.toml` forbidding
//! web -> services/api over IMPORTS + CALLS.
//!
//! The engine's `[review] base=<rev> ... blocking=<bool>` stderr line is the
//! fired_on marker, then this surface's `[review] surface=cli format=..
//! exit=..`; asserting both here makes them a tested contract. Grep them from
//! a run of the binary:
//! `glia review <repo> 2>&1 >/dev/null | grep '^\[review\] '`.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

const MANIFESTS: [(&str, &str); 2] = [
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
];
const INTERNAL: &str = "def charge(o):\n    return o\n";
/// `INTERNAL` with `charge`'s body edited: unrelated to the rule.
const INTERNAL_EDITED: &str = "def charge(o):\n    return o or 0\n";
const WEB_APP_CLEAN: &str = "def pay(o):\n    return o\n";
/// `pay` imports and calls the api's internals: IMPORTS on line 1, CALLS on
/// line 5.
const WEB_APP: &str =
    "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n";
const TEST_PAY: &str = "from web.app import pay\n\n\ndef test_pay():\n    assert pay(1) == 1\n";
/// The forbid_edge rule; `[[constraint]]` is on line 3.
const RULES: &str = "version = 1\n\n[[constraint]]\nid = \"web-no-api-internals\"\nkind = \"forbid_edge\"\nfrom = \"web\"\nto = \"services/api\"\ncategories = [\"IMPORTS\", \"CALLS\"]\n";

/// The Review's JSON keys, in the engine's field order.
const KEYS: [&str; 10] = [
    "base",
    "counts",
    "changed",
    "impact",
    "tests",
    "edges",
    "new_violations",
    "resolved_violations",
    "check_errors",
    "blocking",
];

/// A scratch dir holding a git work tree (`repo/`), an empty global git
/// config and a `HOME`; removed on drop.
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
    /// A fresh dir with no git work tree in it (`git_repo` inits one).
    fn plain(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-cc6b-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&top).expect("scratch repo dir");
        std::fs::create_dir_all(&home).expect("scratch HOME dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        Scratch {
            root,
            top,
            home,
            gitconfig,
        }
    }

    /// CC.6a's tree with `web_app` as `web/app.py`, committed on `main`.
    fn committed(name: &str, web_app: &str) -> Self {
        let s = Scratch::plain(name);
        s.git(&["init", "-q"]);
        for (rel, src) in MANIFESTS {
            s.write(rel, src);
        }
        s.write("services/api/internal.py", INTERNAL);
        s.write("web/app.py", web_app);
        s.write("tests/test_pay.py", TEST_PAY);
        s.write(".glia/overlay.toml", RULES);
        s.git(&["add", "-A"]);
        s.git(&["commit", "-q", "-m", "clean"]);
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
            panic!("CC.6b review tests need a `git` binary on PATH; running it failed: {e}")
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

    /// `glia review <repo> <extra>`.
    fn review(&self, extra: &[&str]) -> Output {
        let mut args = vec!["review", self.path()];
        args.extend_from_slice(extra);
        self.glia(&args)
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn show(o: &Output) -> String {
    format!(
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        stdout(o),
        stderr(o)
    )
}

/// The `[review] ` lines on stderr: the engine's, then the surface's.
fn markers(o: &Output) -> Vec<String> {
    stderr(o)
        .lines()
        .filter(|l| l.starts_with("[review] "))
        .map(str::to_string)
        .collect()
}

fn json(o: &Output) -> Value {
    serde_json::from_str(&stdout(o))
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{}", show(o)))
}

/// The keys of the JSON object `text`, in the order printed (`serde_json`'s
/// `Value` sorts them).
fn top_level_keys(text: &str) -> Vec<String> {
    let (mut keys, mut depth, mut in_str, mut escaped) = (Vec::new(), 0usize, false, false);
    let mut current = String::new();
    let mut prev_structural = ' ';
    for c in text.chars() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
                if depth == 1 && matches!(prev_structural, '{' | ',') {
                    keys.push(std::mem::take(&mut current));
                }
                current.clear();
                continue;
            }
            current.push(c);
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => {
                depth += 1;
                prev_structural = c;
            }
            '}' | ']' => {
                depth -= 1;
                prev_structural = c;
            }
            ',' | ':' => prev_structural = c,
            _ => {}
        }
    }
    keys
}

/// A new violation in the working tree fails the gate: exit 1, the markdown
/// report on stdout, the engine's marker saying `blocking=true`.
#[test]
fn a_new_violation_exits_1_with_the_markdown_report() {
    let s = Scratch::committed("new", WEB_APP_CLEAN);
    s.write("web/app.py", WEB_APP);
    let o = s.review(&[]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let out = stdout(&o);
    assert!(
        out.starts_with("## glia review vs `HEAD`\n**1 new violation(s)**"),
        "{}",
        show(&o)
    );
    for want in [
        "### New violations (blocking)",
        "#### web-no-api-internals (forbid_edge, .glia/overlay.toml:3)",
        "| IMPORTS | `web::app` | `services::api::internal` | web/app.py:1 | fact |",
        "### Tests to run",
        "| `tests::test_pay::test_pay` |",
    ] {
        assert!(out.contains(want), "missing {want:?}\n{}", show(&o));
    }
    let m = markers(&o);
    assert_eq!(m.len(), 2, "{}", show(&o));
    assert!(
        m[0].starts_with("[review] base=HEAD changed=")
            && m[0].contains(" edges +2 -0 (fact=2 derived=0 heuristic=0) violations new=1 resolved=0 blocking=true"),
        "{}",
        show(&o)
    );
    assert_eq!(m[1], "[review] surface=cli format=markdown exit=1");
}

/// A violation the base already had does not fail the gate (`glia check`
/// would): exit 0, no blocking section.
#[test]
fn a_pre_existing_violation_exits_0() {
    let s = Scratch::committed("pre-existing", WEB_APP);
    s.write("services/api/internal.py", INTERNAL_EDITED);
    let o = s.review(&[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let out = stdout(&o);
    assert!(
        out.starts_with("## glia review vs `HEAD`\n**0 new violation(s)** | 0 resolved |"),
        "{}",
        show(&o)
    );
    assert!(!out.contains("### New violations"), "{}", show(&o));
    assert!(
        markers(&o)[0].ends_with("violations new=0 resolved=0 blocking=false"),
        "{}",
        show(&o)
    );
    let check = s.glia(&["check", s.path()]);
    assert_eq!(
        check.status.code(),
        Some(1),
        "check fails on any violation\n{}",
        show(&check)
    );
}

/// `--json` is the engine's Review with its keys in field order, and keeps
/// the exit code.
#[test]
fn json_is_the_review_and_keeps_the_exit_code() {
    let s = Scratch::committed("json", WEB_APP_CLEAN);
    s.write("web/app.py", WEB_APP);
    let o = s.review(&["--json", "--base", "HEAD"]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let v = json(&o);
    assert_eq!(
        top_level_keys(&stdout(&o)),
        KEYS,
        "exactly the Review's keys, in field order"
    );
    assert_eq!(
        (v["base"].as_str(), v["blocking"].as_bool()),
        (Some("HEAD"), Some(true))
    );
    assert_eq!(v["new_violations"][0]["rule_id"], "web-no-api-internals");
    assert_eq!(markers(&o)[1], "[review] surface=cli format=json exit=1");

    let capped = s.review(&[
        "--json",
        "--max-tests",
        "0",
        "--max-impact",
        "0",
        "--depth",
        "1",
    ]);
    let c = json(&capped);
    assert_eq!(
        c["tests"]["tests"].as_array().map(Vec::len),
        Some(0),
        "{}",
        show(&capped)
    );
    assert_eq!(
        c["impact"]["results"].as_array().map(Vec::len),
        Some(0),
        "{}",
        show(&capped)
    );
    assert_eq!(c["counts"], v["counts"], "the counts stay uncut");
}

/// `--markdown-rows` cuts each table and says so.
#[test]
fn markdown_rows_cut_the_tables() {
    let s = Scratch::committed("rows", WEB_APP_CLEAN);
    s.write("web/app.py", WEB_APP);
    let o = s.review(&["--markdown-rows", "1"]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let footers: Vec<String> = stdout(&o)
        .lines()
        .filter(|l| l.starts_with("_("))
        .map(str::to_string)
        .collect();
    assert_eq!(
        footers,
        ["_(1 of 2)_", "_(1 of 2)_", "_(1 of 3)_"],
        "{}",
        show(&o)
    );
}

/// A clean tree: exit 0 and the no-change sentence.
#[test]
fn no_change_exits_0() {
    let s = Scratch::committed("clean", WEB_APP_CLEAN);
    let o = s.review(&[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(
        stdout(&o).ends_with("\n\nNo graph change vs HEAD.\n"),
        "{}",
        show(&o)
    );
}

/// The rev pair is one repo: `--with` is refused, and nothing is built.
#[test]
fn with_is_refused() {
    let s = Scratch::committed("with", WEB_APP_CLEAN);
    let o = s.review(&["--with", "x"]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    assert!(stderr(&o).contains("--with"), "{}", show(&o));
    assert!(markers(&o).is_empty(), "nothing is built\n{}", show(&o));
    assert!(stdout(&o).is_empty(), "{}", show(&o));
}

#[test]
fn not_a_git_work_tree_exits_2() {
    let s = Scratch::plain("not-git");
    s.write("web/app.py", WEB_APP);
    let o = s.review(&[]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    assert!(
        stderr(&o).contains("error: not a git work tree"),
        "the engine's message, not a clap usage error\n{}",
        show(&o)
    );
    assert!(stdout(&o).is_empty(), "{}", show(&o));
}

#[test]
fn unknown_rev_and_no_overlay_exit_2() {
    let s = Scratch::committed("usage", WEB_APP_CLEAN);
    let o = s.review(&["--base", "no-such-rev"]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    assert!(stderr(&o).contains("no-such-rev"), "{}", show(&o));
    let o = s.glia(&["--no-overlay", "review", s.path()]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    assert!(stderr(&o).contains("--no-overlay"), "{}", show(&o));
    assert!(markers(&o).is_empty(), "nothing is built\n{}", show(&o));
}
