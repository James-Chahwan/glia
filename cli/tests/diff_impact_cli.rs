//! LE.2 — `glia diff-impact`, the CLI surface of the engine's `diff_impact`,
//! driving the real binary over a two-file shop (`checkout` calls `place`
//! calls `price`) in a scratch dir.
//!
//! The engine's `[diff-impact] mode=.. base=.. changed=..` stderr line is the
//! fired_on marker; asserting it here makes it a tested contract. Grep it
//! from a run: `glia diff-impact <repo> 2>&1 >/dev/null | grep '^\[diff-impact\] mode='`.
//!
//! The rev-mode test needs a `git` binary: without one it FAILS with a
//! message saying so, never skips. Its git calls run hermetically (fixed
//! identity, no signing, no system or global config, `HOME` in the scratch
//! dir), as `engine/tests/git_fixture` does.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const A_PY: &str = "def price(o):\n    return o\n\n\ndef place(o):\n    return price(o)\n";
const B_PY: &str = "from shop.a import place\n\n\ndef checkout(o):\n    return place(o)\n";
/// `A_PY` with `price`'s body edited.
const A_PY_EDITED: &str = "def price(o):\n    return o or 0\n\n\ndef place(o):\n    return price(o)\n";
/// The unified diff of `A_PY` -> `A_PY_EDITED`.
const PRICE_DIFF: &str = "--- a/shop/a.py\n+++ b/shop/a.py\n@@ -1,2 +1,2 @@\n def price(o):\n-    return o\n+    return o or 0\n";

const PRICE: &str = "shop::a::price";
const REV_MARKER: &str = "[diff-impact] mode=rev base=HEAD changed=2 seeds=1 impact=2 edges +0 -0 unresolved_files=0";
const DIFF_MARKER: &str = "[diff-impact] mode=diff base=- changed=1 seeds=1 impact=2 edges +0 -0 unresolved_files=0";

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
        let root = std::env::temp_dir().join(format!("glia-le2-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("scratch HOME dir");
        let gitconfig = root.join("gitconfig");
        let s = Scratch { root, top, home, gitconfig };
        std::fs::create_dir_all(&s.top).expect("scratch repo dir");
        std::fs::write(&s.gitconfig, "").expect("empty gitconfig");
        s.write("shop/a.py", A_PY);
        s.write("shop/b.py", B_PY);
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

    /// A file beside the repo (outside the tree the build walks).
    fn outside(&self, name: &str, text: &str) -> String {
        let p = self.root.join(name);
        std::fs::write(&p, text).expect("write outside the repo");
        p.to_str().expect("utf-8 path").to_string()
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
        cmd.args(["-c", "user.name=glia", "-c", "user.email=glia@example.invalid"])
            .args(["-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .arg("-C")
            .arg(&self.top)
            .args(args);
        let out = self.hermetic(cmd).output().unwrap_or_else(|e| {
            panic!("LE.2 diff-impact rev mode needs a `git` binary on PATH; running it failed: {e}")
        });
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// `glia diff-impact <repo> <args>`.
    fn diff_impact(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
        cmd.arg("diff-impact").arg(self.path()).args(args);
        self.hermetic(cmd).output().expect("run glia")
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// `(qname, depth, seed)` per impact row of a JSON answer, sorted.
fn rows(v: &Value) -> Vec<(String, u64, String)> {
    let mut out: Vec<(String, u64, String)> = v["impact"]["results"]
        .as_array()
        .expect("impact.results")
        .iter()
        .map(|r| {
            (
                r["qname"].as_str().unwrap_or("").to_string(),
                r["depth"].as_u64().unwrap_or(99),
                r["seed"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

fn price_rows() -> Vec<(String, u64, String)> {
    vec![
        ("shop::a::place".to_string(), 1, PRICE.to_string()),
        ("shop::b::checkout".to_string(), 2, PRICE.to_string()),
    ]
}

#[test]
fn diff_file_json() {
    let s = Scratch::shop("json");
    s.write("shop/a.py", A_PY_EDITED);
    let diff = s.outside("price.diff", PRICE_DIFF);
    let out = s.diff_impact(&["--diff", &diff, "--direction", "backward", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let mut keys: Vec<&str> = v.as_object().expect("an object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["base", "changed", "edges_added", "edges_removed", "impact", "unresolved_diff_files"]
    );
    assert!(v["base"].is_null());
    assert_eq!(v["changed"][0]["qname"], PRICE);
    assert_eq!(v["changed"][0]["change"], "diff_hit");
    assert_eq!(rows(&v), price_rows());
    assert_eq!(v["impact"]["results"][0]["line"].as_i64().map(|l| l >= 1), Some(true));
    assert!(v["impact"]["absence"].is_null());
    assert!(stderr(&out).lines().any(|l| l == DIFF_MARKER), "{}", stderr(&out));
}

#[test]
fn base_and_diff_together_exit_2() {
    let s = Scratch::shop("both");
    let diff = s.outside("price.diff", PRICE_DIFF);
    let out = s.diff_impact(&["--base", "HEAD", "--diff", &diff]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("not both"), "{}", stderr(&out));
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}

#[test]
fn with_needs_diff_exit_2() {
    let s = Scratch::shop("with");
    let out = s.diff_impact(&["--with", s.path()]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--with needs --diff"), "{}", stderr(&out));
}

/// The default change source is the working tree against `HEAD`; the table
/// lists the changed nodes and the radius with its `seed` column.
#[test]
fn rev_mode_table_defaults_to_head() {
    let s = Scratch::shop("rev");
    s.git(&["init", "-q"]);
    s.git(&["add", "-A"]);
    s.git(&["commit", "-q", "-m", "shop"]);
    s.write("shop/a.py", A_PY_EDITED);
    let out = s.diff_impact(&["--direction", "backward"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("(vs `HEAD`, backward, depth ≤ 4)"), "{text}");
    assert!(text.contains("- changed: 2 nodes, 1 seeds; edges +0 -0; in radius: 2"), "{text}");
    assert!(text.contains("| modified | ● | FUNCTION | `shop::a::price` | shop/a.py:1 |"), "{text}");
    assert!(text.contains("| score | depth | live | via | kind | qname | seed | location |"), "{text}");
    assert!(text.contains("| `shop::b::checkout` | `shop::a::price` | shop/b.py:4 |"), "{text}");
    assert!(stderr(&out).lines().any(|l| l == REV_MARKER), "{}", stderr(&out));

    let json = s.diff_impact(&["--base", "HEAD", "--direction", "backward", "--json"]);
    assert_eq!(json.status.code(), Some(0), "{}", stderr(&json));
    let v: Value = serde_json::from_str(&stdout(&json)).expect("valid JSON");
    assert_eq!(v["base"], "HEAD");
    assert_eq!(rows(&v), price_rows());

    let bad = s.diff_impact(&["--base", "no-such-rev"]);
    assert_eq!(bad.status.code(), Some(2), "{}", stderr(&bad));
    assert!(stderr(&bad).contains("no-such-rev"), "{}", stderr(&bad));
}

/// `--diff -` reads stdin; a hunk that only deletes lines places no node and
/// the answer says which file, with its absence.
#[test]
fn stdin_deletion_only_diff_names_the_file() {
    let s = Scratch::shop("stdin");
    let deletion = "--- a/shop/a.py\n+++ b/shop/a.py\n@@ -1,3 +1,2 @@\n def price(o):\n-    o = o\n     return o\n";
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
    cmd.arg("diff-impact").arg(s.path()).args(["--diff", "-"]);
    let mut child = s
        .hermetic(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn glia");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(deletion.as_bytes())
        .expect("write the diff");
    let out = child.wait_with_output().expect("glia output");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("- diff files no node placed: shop/a.py"), "{text}");
    assert!(text.contains("_(nothing in radius)_"), "{text}");
    assert!(text.contains("> FACT: the diff resolved to no node"), "{text}");
    assert!(
        stderr(&out).lines().any(|l| l == "[diff-impact] mode=diff base=- changed=0 seeds=0 impact=0 edges +0 -0 unresolved_files=1"),
        "{}",
        stderr(&out)
    );
}
