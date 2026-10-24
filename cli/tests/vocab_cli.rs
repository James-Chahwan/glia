//! FB.13b — `glia vocab sync`, the CLI surface of the identifier vocabulary
//! snapshot step (`glia_snapshots::vocab_sync`, FB.13a), driving the real
//! binary over temporary git repos made under an empty global git config.
//! These tests need a `git` binary: without one they FAIL saying so.
//!
//! fired_on, on stderr: the library's `[vocab] sync rev=<12> ...` line, then
//! the surface's `[vocab] surface=cli rev=<12> ...`. Grep them with
//! `cargo test -p glia-cli --test vocab_cli -- --nocapture 2>&1 | grep '^\[vocab\] '`.

use std::path::PathBuf;
use std::process::{Command, Output};

use glia_code_domain::snapshots::{
    META_FILE, VOCAB_FILES_FILE, VOCAB_GENERATOR, VOCAB_TOKENS_FILE, vocab_dir,
};
use serde_json::Value;

/// 2026-01-01T00:00:00Z.
const T0: i64 = 1_767_225_600;

/// A scratch dir holding `repo/` and the empty global git config every git
/// call runs under; removed on drop (`cli` has no `tempfile`).
struct Scratch {
    root: PathBuf,
    top: PathBuf,
    gitconfig: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Scratch {
    /// A fresh scratch dir; `repo/` exists but is not a git repo.
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-fb13b-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        std::fs::create_dir_all(&top).expect("scratch dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        Scratch {
            root,
            top,
            gitconfig,
        }
    }

    fn path(&self) -> &str {
        self.top.to_str().expect("utf-8 scratch path")
    }

    /// Run git in the repo at date `t`; its trimmed stdout.
    fn git(&self, args: &[&str], t: i64) -> String {
        let date = format!("@{t} +0000");
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.top)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Fixture Author")
            .env("GIT_AUTHOR_EMAIL", "author@fixture.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture Committer")
            .env("GIT_COMMITTER_EMAIL", "committer@fixture.invalid")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap_or_else(|e| panic!("FB.13b vocab tests need a `git` binary on PATH: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            text(&out.stderr)
        );
        text(&out.stdout).trim().to_string()
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.top.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, content).expect("write");
    }

    fn commit(&self, msg: &str, t: i64) {
        self.git(&["add", "-A"], t);
        self.git(&["commit", "-q", "-m", msg], t);
    }

    /// Run the real binary; relays the `[vocab] ` markers for `--nocapture`.
    fn glia(&self, args: &[&str]) -> Output {
        let out = Command::new(env!("CARGO_BIN_EXE_glia"))
            .args(args)
            .env("GLIA_NO_PERSIST", "1")
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("glia runs");
        for line in text(&out.stderr).lines() {
            if line.starts_with("[vocab] ") {
                eprintln!("{line}");
            }
        }
        out
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Two commits: the first holds one file, the second adds another.
fn two_commit_repo(name: &str) -> Scratch {
    let s = Scratch::new(name);
    s.git(&["init", "-q", "-b", "main"], T0);
    s.write("app.py", "def checkout_total(cart):\n    return cart\n");
    s.commit("init", T0);
    s.write(
        "svc/billing.py",
        "def invoice_line(order):\n    return order\n",
    );
    s.commit("billing", T0 + 86_400);
    s
}

fn line_with<'a>(stderr: &'a str, prefix: &str) -> &'a str {
    stderr
        .lines()
        .find(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no `{prefix}` line in stderr:\n{stderr}"))
}

#[test]
fn sync_writes_the_snapshot_and_names_the_cli_surface() {
    let r = two_commit_repo("sync");
    let head = r.git(&["rev-parse", "HEAD"], T0);
    let head12: String = head.chars().take(12).collect();
    let out = r.glia(&["vocab", "sync", r.path()]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let dir = vocab_dir(&r.top);
    for f in [META_FILE, VOCAB_FILES_FILE, VOCAB_TOKENS_FILE] {
        assert!(dir.join(f).is_file(), "{f} written under {}", dir.display());
    }
    let meta: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(META_FILE)).expect("meta"))
            .expect("meta json");
    assert_eq!(meta["rev"], head.as_str());
    assert_eq!(meta["files"], 2);
    assert_eq!(meta["generator"], VOCAB_GENERATOR);
    let tokens = meta["tokens"].as_u64().expect("tokens");
    assert!(tokens > 0, "{meta}");

    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [format!(
            "synced {tokens} tokens from 2 files at {head12} -> {}/",
            dir.display()
        )],
    );
    let lib = line_with(&stderr, "[vocab] sync ");
    assert!(
        lib.starts_with(&format!(
            "[vocab] sync rev={head12} files=2 tokens={tokens} "
        )),
        "{lib}"
    );
    assert_eq!(
        line_with(&stderr, "[vocab] surface="),
        format!("[vocab] surface=cli rev={head12} files=2 tokens={tokens}")
    );
}

#[test]
fn rev_reads_an_older_commit_and_json_prints_the_summary() {
    let r = two_commit_repo("rev");
    let first = r.git(&["rev-parse", "HEAD~1"], T0);
    let out = r.glia(&["vocab", "sync", r.path(), "--rev", "HEAD~1", "--json"]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let summary: Value = serde_json::from_str(&stdout).expect("--json prints one JSON object");
    let mut keys: Vec<&str> = summary
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "common",
            "files",
            "ms",
            "rev",
            "skipped_binary",
            "skipped_large",
            "tokens"
        ]
    );
    assert!(
        stdout.starts_with("{\n  \"rev\": "),
        "field order: {stdout}"
    );
    assert_eq!(summary["rev"], first.as_str());
    assert_eq!(summary["files"], 1);
    assert!(line_with(&stderr, "[vocab] surface=cli ").contains(" files=1 "));
}

#[test]
fn a_non_git_dir_exits_1_and_writes_nothing() {
    let r = Scratch::new("plain");
    r.write("app.py", "def f():\n    return 1\n");
    let out = r.glia(&["vocab", "sync", r.path()]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr:\n{stderr}");
    assert!(
        stderr.contains("error: ") && stderr.contains("is not inside a git work tree"),
        "{stderr}"
    );
    assert!(
        !r.top.join(".glia").exists(),
        "a failed sync writes nothing"
    );
    assert!(!stderr.contains("[vocab] surface="), "{stderr}");
}

#[test]
fn a_rev_that_names_no_commit_exits_1() {
    let r = two_commit_repo("badrev");
    let out = r.glia(&["vocab", "sync", r.path(), "--rev", "no-such-branch"]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr:\n{stderr}");
    assert!(stderr.contains("does not name a commit"), "{stderr}");
    assert!(!vocab_dir(&r.top).exists(), "a failed sync writes nothing");
}
