//! FB.13d: `glia-export-engram --vocab` writes `<out>.vocab.jsonl` - a header
//! binding it to the gmap by `gmap_digest`, path rows for the tracked files
//! outside the export's file table (ids after the table's largest), then
//! FB.13a's token rows with their file ids in that one id space.
//!
//! Each test commits a scratch repo (outside any other work tree, git's
//! global config and hooks off), exports it with `--no-persist` to an `--out`
//! outside the repo and reads the side file back.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const CHAIN_PY: &str = "def alpha():
    return beta()


def beta():
    return 1


class Ledger:
    def balance(self, entries):
        return sum(entry.amount for entry in entries)
";

const CART_PY: &str = "class Cart:
    def total(self):
        return sum(item.price for item in self.items)
";

/// A file no parser reads and no doc rule admits: outside the file table.
const NOTES_TXT: &str = "zebraNotesOnly appears here and in no source file.\n";

const MARKER: &str = "[engram-export] vocab side: ";

/// Env vars that would point git at another repository.
const GIT_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

fn git(dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=fb13d",
            "-c",
            "user.email=fb13d@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z");
    for var in GIT_ENV {
        cmd.env_remove(var);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Scratch(PathBuf);

impl Scratch {
    /// `<tmp>/repo`, committed, and an empty `<tmp>/out`.
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-fb13d-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        for (rel, body) in [
            ("svc/chain.py", CHAIN_PY),
            ("svc/cart.py", CART_PY),
            ("NOTES.txt", NOTES_TXT),
        ] {
            let path = repo.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        std::fs::create_dir_all(root.join("out")).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        Scratch(root)
    }

    fn repo(&self) -> PathBuf {
        self.0.join("repo")
    }

    fn out(&self, name: &str) -> PathBuf {
        self.0.join("out").join(format!("{name}.engram-gmap"))
    }

    fn vocab(&self, name: &str) -> PathBuf {
        glia_engram_export::vocab_side_path(&self.out(name))
    }

    fn run_at(&self, repo: &Path, name: &str, extra: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia-export-engram"));
        cmd.arg(repo)
            .arg("--out")
            .arg(self.out(name))
            .arg("--no-persist")
            .args(extra)
            .env_remove("GLIA_NO_PERSIST");
        for var in GIT_ENV {
            cmd.env_remove(var);
        }
        cmd.output().unwrap()
    }

    /// Export `repo` to `out/<name>`, asserting exit 0; returns stderr.
    fn export_at(&self, repo: &Path, name: &str, extra: &[&str]) -> String {
        let out = self.run_at(repo, name, extra);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(
            out.status.code(),
            Some(0),
            "export {name} failed:\n{stderr}"
        );
        stderr
    }

    fn export(&self, name: &str, extra: &[&str]) -> String {
        self.export_at(&self.repo(), name, extra)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn lines(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.ends_with('\n'), "every line is newline-terminated");
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The file table the export wrote (`files.json`), as `path -> id`.
fn table(out: &Path) -> BTreeMap<String, u32> {
    let raw: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(glia_engram_export::sidecar_path(out)).unwrap())
            .unwrap();
    raw.into_iter()
        .map(|(id, path)| (path, id.parse().unwrap()))
        .collect()
}

fn marker(stderr: &str) -> Option<&str> {
    stderr.lines().find(|l| l.starts_with(MARKER))
}

#[test]
fn header_path_rows_and_tokens_share_the_file_table() {
    let s = Scratch::new("shape");
    let stderr = s.export("a", &["--vocab"]);
    let line = marker(&stderr).expect("the vocab marker");
    assert!(line.contains(" dirty=0 -> "), "{line}");
    assert!(line.contains(" 1 extra files, "), "{line}");

    let rows = lines(&s.vocab("a"));
    let header = &rows[0];
    let meta: Value =
        serde_json::from_slice(&std::fs::read(glia_engram_export::meta_path(&s.out("a"))).unwrap())
            .unwrap();
    assert_eq!(header["gmap_digest"], meta["gmap_digest"]);
    assert_eq!(header["vocab_side"], 1);
    let keys: Vec<&String> = header.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["files", "gmap_digest", "tokens", "vocab_side"]);

    let table = table(&s.out("a"));
    assert!(table.contains_key("svc/chain.py"), "{table:?}");
    assert!(!table.contains_key("NOTES.txt"), "{table:?}");
    let max = *table.values().max().unwrap();

    let path_rows: Vec<&Value> = rows[1..].iter().filter(|r| r.get("f").is_some()).collect();
    let token_rows: Vec<&Value> = rows[1..].iter().filter(|r| r.get("t").is_some()).collect();
    assert_eq!(1 + path_rows.len() + token_rows.len(), rows.len());
    // Path rows come before every token row.
    assert!(
        rows[1..=path_rows.len()]
            .iter()
            .all(|r| r.get("f").is_some())
    );
    assert_eq!(path_rows.len(), 1);
    assert_eq!(path_rows[0]["path"], "NOTES.txt");
    let notes_id = path_rows[0]["f"].as_u64().unwrap() as u32;
    assert_eq!(notes_id, max + 1, "extra ids follow the table's largest");
    assert_eq!(header["files"], table.len() + 1);
    assert_eq!(header["tokens"], token_rows.len());

    let tokens: Vec<&str> = token_rows
        .iter()
        .map(|r| r["t"].as_str().unwrap())
        .collect();
    let mut sorted = tokens.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(tokens, sorted, "token rows strictly sorted by token");

    let by_token: BTreeMap<&str, &Value> = token_rows
        .iter()
        .map(|r| (r["t"].as_str().unwrap(), *r))
        .collect();
    let ids = |t: &str| -> Vec<u32> {
        by_token[t]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect()
    };
    assert_eq!(ids("zebraNotesOnly"), [notes_id]);
    assert_eq!(ids("Ledger"), [table["svc/chain.py"]]);
    let mut both = vec![table["svc/chain.py"], table["svc/cart.py"]];
    both.sort_unstable();
    assert_eq!(ids("sum"), both);
    assert_eq!(by_token["sum"]["n"], 2);
    // Every id is in the one space.
    let space: BTreeSet<u32> = table.values().copied().chain([notes_id]).collect();
    for row in &token_rows {
        for id in row["files"].as_array().unwrap() {
            assert!(space.contains(&(id.as_u64().unwrap() as u32)), "{row}");
        }
    }
}

#[test]
fn two_clones_write_identical_bytes() {
    let s = Scratch::new("clones");
    let origin = s.repo();
    let mut bytes = Vec::new();
    for (i, dir) in ["one", "nested/deeper/two"].iter().enumerate() {
        let clone = s.0.join(dir);
        std::fs::create_dir_all(clone.parent().unwrap()).unwrap();
        git(
            &s.0,
            &[
                "clone",
                "-q",
                "--no-hardlinks",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        let name = format!("c{i}");
        s.export_at(&clone, &name, &["--vocab"]);
        bytes.push((
            std::fs::read(s.vocab(&name)).unwrap(),
            std::fs::read(s.out(&name)).unwrap(),
        ));
    }
    assert_eq!(bytes[0].1, bytes[1].1, "the gmaps match (DE.2)");
    assert_eq!(bytes[0].0, bytes[1].0, "the vocab side files match");
}

#[test]
fn without_the_flag_none_is_written_and_a_stale_one_goes() {
    let s = Scratch::new("stale");
    s.export("a", &["--vocab"]);
    assert!(s.vocab("a").exists());
    let stderr = s.export("a", &[]);
    assert!(marker(&stderr).is_none(), "{stderr}");
    assert!(!s.vocab("a").exists(), "the stale side file is removed");
    s.export("b", &[]);
    assert!(!s.vocab("b").exists());
}

#[test]
fn a_dirty_tree_exports_heads_vocabulary_and_counts_the_edits() {
    let s = Scratch::new("dirty");
    let cart = s.repo().join("svc/cart.py");
    std::fs::write(&cart, format!("{CART_PY}\nuncommittedName = 1\n")).unwrap();
    let stderr = s.export("a", &["--vocab"]);
    let line = marker(&stderr).expect("the vocab marker");
    assert!(line.contains(" dirty=1 -> "), "{line}");
    let text = std::fs::read_to_string(s.vocab("a")).unwrap();
    assert!(
        !text.contains("uncommittedName"),
        "the vocabulary is HEAD's"
    );
    assert!(text.contains("\"t\":\"Cart\""));
}

#[test]
fn outside_git_vocab_exits_10_and_writes_nothing() {
    let s = Scratch::new("nogit");
    let plain = s.0.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("m.py"), CART_PY).unwrap();
    let out = s.run_at(&plain, "p", &["--vocab"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(10), "{stderr}");
    assert!(
        stderr.contains("error: --vocab needs a git work tree"),
        "{stderr}"
    );
    assert!(!s.out("p").exists(), "no gmap is written");
    assert!(!s.vocab("p").exists());
}
