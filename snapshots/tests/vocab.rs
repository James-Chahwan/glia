//! FB.13a: `vocab_sync` against real temporary git repos.
//!
//! Commits use a fixed identity and fixed dates under an empty global git
//! config. These tests need a `git` binary: without one they FAIL saying so.

use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::snapshots::{
    META_FILE, VOCAB_FILES_FILE, VOCAB_TOKENS_FILE, read_vocab, vocab_dir,
};
use glia_snapshots::{VocabOptions, tokens_of, vocab_sync};

/// 2026-01-01T00:00:00Z.
const T0: i64 = 1_767_225_600;

struct Repo {
    tmp: tempfile::TempDir,
    top: PathBuf,
    gitconfig: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let top = tmp.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let gitconfig = tmp.path().join("gitconfig");
        std::fs::write(&gitconfig, "").unwrap();
        let repo = Repo {
            tmp,
            top,
            gitconfig,
        };
        repo.git(&repo.top, &["init", "-q", "-b", "main"]);
        repo
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let date = format!("@{T0} +0000");
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Fixture Author")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture Author")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap_or_else(|e| {
                panic!("FB.13a vocab tests need a `git` binary on PATH; running it failed: {e}")
            });
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(&self, rel: &str, content: &[u8]) {
        let path = self.top.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn commit(&self, msg: &str) {
        self.git(&self.top, &["add", "-A"]);
        self.git(&self.top, &["commit", "-q", "-m", msg]);
    }
}

fn snapshot_bytes(root: &Path) -> [Vec<u8>; 3] {
    [VOCAB_FILES_FILE, VOCAB_TOKENS_FILE, META_FILE]
        .map(|f| std::fs::read(vocab_dir(root).join(f)).unwrap())
}

/// The probe: 17 files share `sharedtok`, 16 share `sixteentok`; one binary,
/// one large blob.
fn probe() -> Repo {
    let r = Repo::new();
    r.write("src/main.rs", b"fn main() { let fooBar = x_y; }\n");
    r.write("docs/readme.md", b"Use fooBar here, caf\xc3\xa9s ok.\n");
    r.write("assets/logo.bin", b"PNGhdr\0\0binarytoken");
    let mut large = vec![b'a'; (4 << 20) + 1];
    large.extend_from_slice(b" largetoken");
    r.write("data/large.txt", &large);
    for i in 0..17 {
        let mut text = format!("sharedtok {i}\n");
        if i < 16 {
            text.push_str("sixteentok\n");
        }
        r.write(&format!("many/f{i:02}.txt"), text.as_bytes());
    }
    r.commit("init");
    r
}

#[test]
fn tokens_of_reads_identifiers_of_three_or_more_bytes() {
    let got = tokens_of(b"fooBar x_y abc 9ab");
    let want: Vec<(String, u32)> = vec![("abc".into(), 1), ("fooBar".into(), 1), ("x_y".into(), 1)];
    assert_eq!(got.into_iter().collect::<Vec<_>>(), want);
}

#[test]
fn sync_reads_the_commit_tree_only() {
    let r = probe();
    // Untracked, and staged but not committed: neither is in HEAD's tree.
    r.write("untracked.rs", b"untrackedtok\n");
    r.write("staged.rs", b"stagedtok\n");
    r.git(&r.top, &["add", "staged.rs"]);

    let summary = vocab_sync(&r.top, &VocabOptions::default()).unwrap();
    let head = r.git(&r.top, &["rev-parse", "HEAD"]);
    assert_eq!(summary.rev, head);
    assert_eq!(
        (summary.files, summary.skipped_binary, summary.skipped_large),
        (19, 1, 1)
    );

    let snap = read_vocab(&r.top).expect("a complete snapshot");
    assert_eq!(snap.meta.rev, head);
    assert_eq!(snap.meta.generator, "glia vocab sync");
    assert_eq!(snap.meta.files, 19);
    assert_eq!(snap.meta.tokens, summary.tokens);
    assert_eq!(snap.meta.common, summary.common);
    assert_eq!(snap.files[0], "docs/readme.md");
    assert!(snap.files.windows(2).all(|w| w[0] < w[1]));

    for absent in [
        "untrackedtok",
        "stagedtok",
        "binarytoken",
        "largetoken",
        "PNGhdr",
    ] {
        assert!(snap.files_of(absent).is_none(), "{absent} must be absent");
    }
    let foo = snap.files_of("fooBar").unwrap();
    assert_eq!(foo.files, ["docs/readme.md", "src/main.rs"]);
    assert_eq!((foo.n, foo.common), (2, false));
    // A non-ASCII byte ends a token.
    assert!(snap.files_of("caf").is_some());
    assert!(snap.files_of("x_y").is_some());

    let shared = snap.files_of("sharedtok").unwrap();
    assert!(shared.common);
    assert!(shared.files.is_empty());
    assert_eq!(shared.n, 17);
    let sixteen = snap.files_of("sixteentok").unwrap();
    assert!(!sixteen.common);
    assert_eq!(sixteen.files.len(), 16);

    // The raw rows: a common token carries no list; keys sorted, compact.
    let rows = String::from_utf8(snapshot_bytes(&r.top)[1].clone()).unwrap();
    assert!(rows.contains("{\"common\":true,\"n\":17,\"t\":\"sharedtok\"}\n"));
    assert!(rows.contains("{\"files\":[0,18],\"n\":2,\"t\":\"fooBar\"}\n"));
    let files = String::from_utf8(snapshot_bytes(&r.top)[0].clone()).unwrap();
    assert!(files.starts_with("{\"f\":0,\"path\":\"docs/readme.md\"}\n"));

    let gitignore = std::fs::read_to_string(r.top.join(".glia/.gitignore")).unwrap();
    assert!(gitignore.lines().any(|l| l == "vocab-snapshot/"));
}

#[test]
fn rev_reads_an_older_tree() {
    let r = probe();
    let first = r.git(&r.top, &["rev-parse", "HEAD"]);
    r.write("src/later.rs", b"latertoken\n");
    r.commit("later");
    vocab_sync(&r.top, &VocabOptions::at(first.clone())).unwrap();
    let snap = read_vocab(&r.top).unwrap();
    assert_eq!(snap.meta.rev, first);
    assert!(snap.files_of("latertoken").is_none());
    vocab_sync(&r.top, &VocabOptions::default()).unwrap();
    assert!(read_vocab(&r.top).unwrap().files_of("latertoken").is_some());

    let err = vocab_sync(&r.top, &VocabOptions::at("no-such-rev")).unwrap_err();
    assert!(err.contains("does not name a commit"), "{err}");
    let err = vocab_sync(&r.top, &VocabOptions::at("--all")).unwrap_err();
    assert!(err.contains("is not a revision"), "{err}");
}

#[test]
fn a_subdirectory_root_reads_its_own_files() {
    let r = probe();
    let sub = r.top.join("many");
    let summary = vocab_sync(&sub, &VocabOptions::default()).unwrap();
    assert_eq!(summary.files, 17);
    let snap = read_vocab(&sub).unwrap();
    assert_eq!(snap.files[0], "f00.txt");
    assert!(snap.files_of("fooBar").is_none());
}

#[test]
fn two_clones_of_one_commit_write_identical_bytes() {
    let r = probe();
    vocab_sync(&r.top, &VocabOptions::default()).unwrap();
    let other = r.tmp.path().join("elsewhere").join("clone-two");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    r.git(
        r.tmp.path(),
        &[
            "clone",
            "-q",
            r.top.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    vocab_sync(&other, &VocabOptions::default()).unwrap();
    assert_eq!(snapshot_bytes(&r.top), snapshot_bytes(&other));
    // And a re-sync of the same commit rewrites the same bytes.
    let before = snapshot_bytes(&r.top);
    vocab_sync(&r.top, &VocabOptions::default()).unwrap();
    assert_eq!(snapshot_bytes(&r.top), before);
}

#[test]
fn a_truncated_or_meta_less_snapshot_reads_as_none() {
    let r = probe();
    assert!(read_vocab(&r.top).is_none(), "never synced");
    vocab_sync(&r.top, &VocabOptions::default()).unwrap();
    let path = vocab_dir(&r.top).join(VOCAB_TOKENS_FILE);
    let full = std::fs::read(&path).unwrap();
    std::fs::write(&path, &full[..full.len() / 2]).unwrap();
    assert!(read_vocab(&r.top).is_none(), "data_hash mismatch");

    vocab_sync(&r.top, &VocabOptions::default()).unwrap();
    assert!(read_vocab(&r.top).is_some());
    std::fs::remove_file(vocab_dir(&r.top).join(META_FILE)).unwrap();
    assert!(read_vocab(&r.top).is_none(), "no meta is incomplete");

    vocab_sync(&r.top, &VocabOptions::default()).unwrap();
    let meta_path = vocab_dir(&r.top).join(META_FILE);
    let meta = std::fs::read_to_string(&meta_path).unwrap();
    std::fs::write(
        &meta_path,
        meta.replace("\"version\": 1", "\"version\": 99"),
    )
    .unwrap();
    assert!(read_vocab(&r.top).is_none(), "unknown version");
}

#[test]
fn errors_come_back_before_anything_is_written() {
    let tmp = tempfile::tempdir().unwrap();
    let err = vocab_sync(tmp.path(), &VocabOptions::default()).unwrap_err();
    assert!(err.contains("not inside a git work tree"), "{err}");
    assert!(!tmp.path().join(".glia").exists());
}
