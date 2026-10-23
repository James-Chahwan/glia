//! DE.2: every clone of one commit exports the same bytes. glia's NodeId
//! hashes the RepoId (LB.1: the origin URL, `gitdir:<name>` without one,
//! `dir:<name>` outside git), so the same tree under two identities has two id
//! sets; the export is keyed by qname and must never order or tie-break by
//! NodeId. Before DE.2 the edges followed glia's NodeId-sorted cross edges and
//! the identity-hint ordinals of same-kind nodes on one row (`var lo, hi`)
//! followed NodeId, so two clones wrote two gmap digests and Engram, whose
//! diff apply checks `base_digest`, could not take one clone's diff onto a
//! store seeded from the other.
//!
//! The T12 hints side file joins the compared artefacts: an `Identity` holds
//! no RepoId, so the three copies write one hints file too.
//!
//! Three copies of one fixture: `fx-alpha` and `fx-beta` outside git (keys
//! `dir:fx-alpha` / `dir:fx-beta`) and `fx-gamma` holding only a
//! `.git/config` whose origin is the path remote `/elsewhere/fx-alpha.git`
//! (Engram's clone-of-a-clone case; LB.1 reads the config, no git binary).
//! The scratch parent sits in `std::env::temp_dir()`, never inside the glia
//! checkout, where every copy would key `git:<glia remote>/<rel>`.

use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::walk_gating::repo_identity;
use glia_engram_export::diff::diff_path;
use glia_engram_export::{hints_side_path, meta_path, sidecar_path};

/// The copies, by directory name: each is its own RepoId.
const COPIES: [&str; 3] = ["fx-alpha", "fx-beta", "fx-gamma"];

/// DE.1's case: one bare `refresh` (two candidates) and one qualified
/// `Feed.refresh`, plus mentions of the Go symbols, so the section holds
/// several DOCUMENTS cross edges whose NodeId order differs per RepoId.
const README_MD: &str = "# Notes

## Caching

Call `refresh` after a write; `Feed.refresh` reloads the feed and `Wallet` keeps the balance.

## Ranges

`Span` measures from `lo` to `hi` through `helper`.
";

const WALLET_PY: &str = "class Wallet:
    def refresh(self):
        return 1
";

const FEED_PY: &str = "class Feed:
    def refresh(self):
        return 2
";

const GO_MOD: &str = "module example.com/fx

go 1.21
";

/// The row tie: two package vars of one kind on one row.
const VARS_GO: &str = "package fx

var lo, hi = 1, 2
";

/// A cross-file call: `Span` (calc.go) calls `helper` (util.go).
const CALC_GO: &str = "package fx

func Span() int {
	return helper(hi) - lo
}
";

const UTIL_GO: &str = "package fx

func helper(n int) int {
	return n * 2
}
";

/// `fx-gamma`'s only git state: a path remote naming another clone.
const GIT_CONFIG: &str = "[core]
	repositoryformatversion = 0
[remote \"origin\"]
	url = /elsewhere/fx-alpha.git
	fetch = +refs/heads/*:refs/remotes/origin/*
";

/// A per-test parent holding the three copies and `out/`, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-de2-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("out")).unwrap();
        let s = Scratch(root);
        for copy in COPIES {
            s.write(copy, "README.md", README_MD);
            s.write(copy, "billing/wallet.py", WALLET_PY);
            s.write(copy, "web/feed.py", FEED_PY);
            s.write(copy, "go.mod", GO_MOD);
            s.write(copy, "vars.go", VARS_GO);
            s.write(copy, "calc.go", CALC_GO);
            s.write(copy, "util.go", UTIL_GO);
        }
        s.write("fx-gamma", ".git/config", GIT_CONFIG);
        let keys: Vec<String> = COPIES.iter().map(|c| repo_identity(&s.repo(c)).key).collect();
        assert_eq!(
            keys,
            ["dir:fx-alpha", "dir:fx-beta", "git:elsewhere/fx-alpha"],
            "each copy must be its own RepoId"
        );
        s
    }

    fn repo(&self, copy: &str) -> PathBuf {
        self.0.join(copy)
    }

    fn out(&self, copy: &str, name: &str) -> PathBuf {
        self.0.join("out").join(format!("{copy}-{name}.engram-gmap"))
    }

    fn write(&self, copy: &str, rel: &str, body: &str) {
        let path = self.repo(copy).join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn edit(&self, copy: &str, rel: &str, from: &str, to: &str) {
        let path = self.repo(copy).join(rel);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "{copy}/{rel}: no {from:?}");
        std::fs::write(path, text.replace(from, to)).unwrap();
    }

    /// The same edit in every copy: a method appended to `Wallet`, and
    /// `helper` renamed to `double` with its caller.
    fn edit_all(&self) {
        for copy in COPIES {
            self.edit(copy, "billing/wallet.py", "        return 1\n", "        return 1\n\n    def settle(self):\n        return 0\n");
            self.edit(copy, "util.go", "func helper(", "func double(");
            self.edit(copy, "calc.go", "helper(hi)", "double(hi)");
        }
    }

    /// Export `copy` to `out/<copy>-<name>`, `--since out/<copy>-<since>`
    /// when given, with `--no-persist` (nothing is written into the copy).
    /// Asserts exit 0 and returns stderr.
    fn export(&self, copy: &str, name: &str, since: Option<&str>) -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia-export-engram"));
        cmd.arg(self.repo(copy)).arg("--out").arg(self.out(copy, name)).arg("--no-persist");
        if let Some(prior) = since {
            cmd.arg("--since").arg(self.out(copy, prior));
        }
        let out = cmd.env_remove("GLIA_NO_PERSIST").output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(0), "export {copy}-{name} failed:\n{stderr}");
        assert!(!self.repo(copy).join(".glia").exists(), "--no-persist wrote into {copy}");
        stderr
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The one `[engram-export] v6 order:` line.
fn order_line(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| l.starts_with("[engram-export] v6 order:")).collect();
    assert_eq!(lines.len(), 1, "want one v6 order line in:\n{stderr}");
    lines[0].to_string()
}

/// `(what, bytes)` per copy: assert the copies agree.
fn assert_all_equal(what: &str, per_copy: Vec<(&str, Vec<u8>)>) {
    let (first, want) = &per_copy[0];
    for (copy, got) in &per_copy[1..] {
        assert!(got == want, "{what}: {copy} differs from {first} ({} vs {} bytes)", got.len(), want.len());
    }
}

#[test]
fn two_clones_export_identical_bytes() {
    let s = Scratch::new("bytes");
    let errs: Vec<(&str, String)> = COPIES.iter().map(|c| (*c, s.export(c, "a", None))).collect();
    // (label, the artefact's path for an export at a path).
    type Artefact = (&'static str, fn(&Path) -> PathBuf);
    let artefacts: [Artefact; 4] = [
        ("gmap", |p: &Path| p.to_path_buf()),
        ("files.json", sidecar_path),
        ("meta.json", meta_path),
        ("hints.jsonl", hints_side_path),
    ];
    for (label, path_of) in artefacts {
        assert_all_equal(label, COPIES.iter().map(|c| (*c, read(&path_of(&s.out(c, "a"))))).collect());
    }
    let lines: Vec<(&str, Vec<u8>)> = errs.iter().map(|(c, e)| (*c, order_line(e).into_bytes())).collect();
    assert_all_equal("v6 order line", lines);
    // The fixture exercises the tie it guards: `lo` and `hi` share a row.
    let line = order_line(&errs[0].1);
    assert!(line.contains("hint row ties by qname=2;"), "{line}");
    assert!(line.ends_with("key ties by kind=0"), "{line}");
}

#[test]
fn two_clones_write_identical_diffs() {
    let s = Scratch::new("diffs");
    for copy in COPIES {
        s.export(copy, "a", None);
    }
    s.edit_all();
    let mut since = Vec::new();
    for copy in COPIES {
        let err = s.export(copy, "b", Some("a"));
        let line = err
            .lines()
            .find(|l| l.starts_with("[engram-export] v6 since: base="))
            .unwrap_or_else(|| panic!("{copy}: no since marker in:\n{err}"))
            .to_string();
        since.push((copy, line));
    }
    // The edit is a real change: one method added, one function renamed.
    assert!(!since[0].1.contains(" unchanged "), "{}", since[0].1);
    assert_all_equal("v6 since line", since.iter().map(|(c, l)| (*c, l.clone().into_bytes())).collect());
    assert_all_equal("gmap after the edit", COPIES.iter().map(|c| (*c, read(&s.out(c, "b")))).collect());
    assert_all_equal("diff", COPIES.iter().map(|c| (*c, read(&diff_path(&s.out(c, "b"))))).collect());
    assert_all_equal(
        "hints after the edit",
        COPIES.iter().map(|c| (*c, read(&hints_side_path(&s.out(c, "b"))))).collect(),
    );
}
