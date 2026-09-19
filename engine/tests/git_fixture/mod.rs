//! Two-commit git fixture harness (LE.1b), shared by the integration tests
//! that build a graph against a git rev (LE.1b graph delta, LE.2 diff impact,
//! LE.3b tests-for, LE.7a patterns) through `mod git_fixture;`.
//!
//! Every git call runs hermetically: a fixed identity, no signing, `main` as
//! the initial branch, no system or global config and `HOME` pointed into the
//! fixture's own temp dir, so a developer's git config cannot change a result.
//! A missing git binary fails the test; it is never a silent skip.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A throwaway git work tree under a temp dir, removed on drop.
pub struct GitRepo {
    dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl GitRepo {
    /// A fresh `git init` (branch `main`) with no commits.
    pub fn init() -> Self {
        let dir = tempfile::tempdir().expect("temp dir for the git fixture");
        let root = dir.path().join("repo");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&root).expect("fixture repo dir");
        std::fs::create_dir_all(&home).expect("fixture HOME dir");
        let repo = GitRepo { dir, root, home };
        repo.git(&["init", "-q"]);
        repo
    }

    /// The work tree's path.
    pub fn path(&self) -> &str {
        self.root.to_str().expect("utf-8 temp path")
    }

    /// The work tree's path as a `Path`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Write `text` to the repo-relative `rel`, creating parent dirs. Not staged.
    pub fn write(&self, rel: &str, text: &str) {
        let p = self.root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent dir");
        }
        std::fs::write(&p, text).expect("fixture write");
    }

    /// Delete the repo-relative file `rel` from the work tree. Not staged.
    pub fn remove(&self, rel: &str) {
        std::fs::remove_file(self.root.join(rel)).expect("fixture remove");
    }

    /// `git mv from to`: the rename is staged, the tree left uncommitted.
    pub fn git_mv(&self, from: &str, to: &str) {
        self.git(&["mv", from, to]);
    }

    /// Stage everything (`git add -A`) and commit; returns the new commit sha.
    pub fn commit(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", msg]);
        let out = self.git(&["rev-parse", "HEAD"]);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Run one hermetic git command in the work tree; panics on failure.
    pub fn git(&self, args: &[&str]) -> Output {
        let out = Command::new("git")
            .args([
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
            .arg(&self.root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("HOME", &self.home)
            .output()
            .unwrap_or_else(|e| panic!("git binary required for the graph-delta harness: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            self.dir.path().display(),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
}
