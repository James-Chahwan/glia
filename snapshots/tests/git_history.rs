//! LF.5a: `history_sync` against real temporary git repos.
//!
//! Every commit is made with a fixed identity and fixed author / committer
//! dates, under an empty global git config, so SHAs are reproducible. These
//! tests need a `git` binary: without one they FAIL with a message saying so.

use std::path::{Path, PathBuf};
use std::process::Command;

use glia_snapshots::{HistoryOptions, history_sync};
use glia_code_domain::snapshots::{
    HISTORY_BLAME_FILE, HISTORY_COMMITS_FILE, HISTORY_MESSAGES_FILE, HistoryFile, META_FILE, data_hash,
    history_dir, read_history, read_history_messages,
};

const AUTHOR_NAME: &str = "Quillon Identitymarker";
const AUTHOR_EMAIL: &str = "quillon.author@identity.invalid";
const COMMITTER_NAME: &str = "Pemberly Committertoken";
const COMMITTER_EMAIL: &str = "pemberly.committer@identity.invalid";
/// 2026-01-01T00:00:00Z.
const T0: i64 = 1_767_225_600;
const DAY: i64 = 86_400;

struct Repo {
    _tmp: tempfile::TempDir,
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
        let repo = Repo { _tmp: tmp, top, gitconfig };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    /// Run git in the repo with the fixed identity; its trimmed stdout.
    fn git(&self, args: &[&str]) -> String {
        self.git_at(&self.top, args, T0)
    }

    fn git_at(&self, dir: &Path, args: &[&str], t: i64) -> String {
        let date = format!("@{t} +0000");
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", AUTHOR_NAME)
            .env("GIT_AUTHOR_EMAIL", AUTHOR_EMAIL)
            .env("GIT_COMMITTER_NAME", COMMITTER_NAME)
            .env("GIT_COMMITTER_EMAIL", COMMITTER_EMAIL)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap_or_else(|e| {
                panic!("LF.5a history tests need a `git` binary on PATH; running it failed: {e}")
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

    fn append(&self, rel: &str, line: &str) {
        let path = self.top.join(rel);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(line);
        std::fs::write(path, text).unwrap();
    }

    /// Stage everything and commit at unix time `t`.
    fn commit(&self, msg: &str, t: i64) {
        self.git_at(&self.top, &["add", "-A"], t);
        self.git_at(&self.top, &["commit", "-q", "-m", msg], t);
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn snapshot_bytes(&self, root: &Path) -> [Vec<u8>; 3] {
        [HISTORY_COMMITS_FILE, HISTORY_BLAME_FILE, META_FILE]
            .map(|f| std::fs::read(history_dir(root).join(f)).unwrap())
    }
}

/// Probe g1: svc/a.py and svc/b.py change together in 4 commits (the initial
/// one and 3 edits), then svc/c.py is renamed to svc/c2.py.
fn probe_g1() -> Repo {
    let r = Repo::new();
    r.write("svc/a.py", b"def a():\n    return 1\n");
    r.write("svc/b.py", b"def b():\n    return 2\n");
    r.write("svc/c.py", b"def c():\n    return 3\n");
    r.commit("init", T0);
    for i in 1..=3 {
        r.append("svc/a.py", &format!("# edit {i}\n"));
        r.append("svc/b.py", &format!("# edit {i}\n"));
        r.commit(&format!("co-change {i}"), T0 + i * DAY);
    }
    r.git(&["mv", "svc/c.py", "svc/c2.py"]);
    r.commit("rename c", T0 + 4 * DAY);
    r
}

fn file(p: &str, a: u32, d: u32) -> HistoryFile {
    HistoryFile { p: p.into(), a: Some(a), d: Some(d), from: None }
}

fn with_blame() -> HistoryOptions {
    HistoryOptions { blame: true, ..HistoryOptions::default() }
}

#[test]
fn probe_g1_shape() {
    let r = probe_g1();
    let s = history_sync(&r.top, &HistoryOptions::default()).expect("sync");
    assert_eq!(
        (s.commits, s.files, s.renames, s.binary, s.blame_files, s.runs),
        (5, 3, 1, 0, 0, 0),
        "{s:?}"
    );
    assert_eq!(s.head, r.head());

    let snap = read_history(&r.top).expect("complete snapshot");
    assert_eq!(snap.meta.head, r.head());
    assert_eq!(snap.meta.commits, 5);
    assert_eq!(snap.meta.max_commits, 2000);
    assert_eq!(snap.meta.since, None);
    assert_eq!(snap.meta.relative_to, "");
    assert_eq!(snap.meta.blame_files, 0);
    assert_eq!(snap.meta.generator, "glia history sync");

    // Newest first, as git gives, with committer times.
    let times: Vec<i64> = snap.commits.iter().map(|c| c.t).collect();
    assert_eq!(times, [T0 + 4 * DAY, T0 + 3 * DAY, T0 + 2 * DAY, T0 + DAY, T0]);
    assert!(snap.commits.iter().all(|c| c.c.len() == 40));
    assert_eq!(snap.commits[0].c, r.head());

    assert_eq!(
        snap.commits[0].files,
        [HistoryFile {
            p: "svc/c2.py".into(),
            a: Some(0),
            d: Some(0),
            from: Some("svc/c.py".into()),
        }]
    );
    for commit in &snap.commits[1..4] {
        assert_eq!(commit.files, [file("svc/a.py", 1, 0), file("svc/b.py", 1, 0)]);
    }
    assert_eq!(
        snap.commits[4].files,
        [file("svc/a.py", 2, 0), file("svc/b.py", 2, 0), file("svc/c.py", 2, 0)]
    );

    // Blame off: blame.jsonl is written, empty.
    assert!(snap.blame.is_empty());
    assert_eq!(std::fs::read(history_dir(&r.top).join(HISTORY_BLAME_FILE)).unwrap(), b"");

    // .glia/.gitignore lists the regenerable inputs, never the checked-in ones.
    let gitignore = std::fs::read_to_string(r.top.join(".glia/.gitignore")).unwrap();
    for line in ["vectors.jsonl", "docs-snapshot/", "history-snapshot/", "test-snapshot/"] {
        assert!(gitignore.lines().any(|l| l == line), "{line} missing from {gitignore}");
    }
    assert!(!gitignore.contains("overlay.toml") && !gitignore.contains("cells.jsonl"));
    // The sync commits nothing and leaves the history untouched.
    assert_eq!(r.git(&["rev-list", "--count", "HEAD"]), "5");
}

#[test]
fn identity_never_captured() {
    let r = probe_g1();
    r.append("svc/a.py", "# message sentinel edit\n");
    r.commit("MessageSentinel subject line", T0 + 5 * DAY);
    let s = history_sync(&r.top, &with_blame()).expect("sync");
    assert!(s.blame_files > 0 && s.runs > 0, "blame must actually run: {s:?}");

    let [commits, blame, meta] = r.snapshot_bytes(&r.top);
    assert!(!blame.is_empty());
    for (name, bytes) in [("commits.jsonl", &commits), ("blame.jsonl", &blame), ("meta.json", &meta)] {
        let text = String::from_utf8_lossy(bytes);
        for needle in [
            AUTHOR_NAME,
            AUTHOR_EMAIL,
            COMMITTER_NAME,
            COMMITTER_EMAIL,
            "Identitymarker",
            "Committertoken",
            "identity.invalid",
            "MessageSentinel",
        ] {
            assert!(!text.contains(needle), "{name} captured {needle:?}:\n{text}");
        }
    }
    // EB.1: the message sidecar holds the subject, never an identity.
    let messages = std::fs::read_to_string(history_dir(&r.top).join(HISTORY_MESSAGES_FILE)).unwrap();
    assert!(messages.contains("MessageSentinel subject line"), "{messages}");
    for needle in [AUTHOR_NAME, AUTHOR_EMAIL, COMMITTER_NAME, COMMITTER_EMAIL, "identity.invalid"] {
        assert!(!messages.contains(needle), "messages.jsonl captured {needle:?}:\n{messages}");
    }
}

#[test]
fn binary_file_counts_are_none() {
    let r = Repo::new();
    r.write("app.py", b"print('hi')\n");
    r.write("logo.png", b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x01");
    r.commit("add", T0);
    r.write("logo.png", b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x02\x00");
    r.commit("new logo", T0 + DAY);

    let s = history_sync(&r.top, &with_blame()).expect("sync");
    assert_eq!(s.binary, 2, "{s:?}");
    let snap = read_history(&r.top).expect("snapshot");
    assert_eq!(
        snap.commits[0].files,
        [HistoryFile { p: "logo.png".into(), a: None, d: None, from: None }]
    );
    assert_eq!(snap.commits[1].files[0], file("app.py", 1, 0));
    assert_eq!(snap.commits[1].files[1], HistoryFile { p: "logo.png".into(), a: None, d: None, from: None });
    // Binary files are not blamed.
    let blamed: Vec<&str> = snap.blame.iter().map(|b| b.p.as_str()).collect();
    assert_eq!(blamed, ["app.py"]);
    let text = std::fs::read_to_string(history_dir(&r.top).join(HISTORY_COMMITS_FILE)).unwrap();
    assert!(text.contains(r#"{"p":"logo.png","a":null,"d":null}"#), "{text}");
}

#[test]
fn subdir_repo_uses_relative() {
    let r = Repo::new();
    r.write("sub/x.py", b"def x():\n    return 1\n");
    r.write("out/y.py", b"def y():\n    return 2\n");
    r.commit("both", T0);
    r.append("out/y.py", "# outside only\n");
    r.commit("outside only", T0 + DAY);
    r.append("sub/x.py", "# inside\n");
    r.commit("inside", T0 + 2 * DAY);

    let root = r.top.join("sub");
    let s = history_sync(&root, &with_blame()).expect("sync");
    assert_eq!((s.commits, s.files), (2, 1), "{s:?}");
    let snap = read_history(&root).expect("snapshot under the sub root");
    assert_eq!(snap.meta.relative_to, "sub/");
    let times: Vec<i64> = snap.commits.iter().map(|c| c.t).collect();
    assert_eq!(times, [T0 + 2 * DAY, T0], "a commit touching only out/ is outside the window");
    assert_eq!(snap.commits[0].files, [file("x.py", 1, 0)]);
    assert_eq!(snap.commits[1].files, [file("x.py", 2, 0)]);
    let blamed: Vec<&str> = snap.blame.iter().map(|b| b.p.as_str()).collect();
    assert_eq!(blamed, ["x.py"]);
    assert!(!r.top.join(".glia").exists(), "the snapshot belongs to the sub root");
}

#[test]
fn not_a_git_repo_is_a_clean_error() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("a.py"), "x = 1\n").unwrap();
    let err = history_sync(&plain, &HistoryOptions::default()).unwrap_err();
    assert!(err.contains("not inside a git work tree"), "{err}");
    assert!(!plain.join(".glia").exists(), "nothing is written on error");

    let missing = tmp.path().join("missing");
    let err = history_sync(&missing, &HistoryOptions::default()).unwrap_err();
    assert!(err.contains("not a directory"), "{err}");

    let r = Repo::new();
    let err = history_sync(&r.top, &HistoryOptions::default()).unwrap_err();
    assert!(err.contains("has no commits"), "{err}");
    assert!(!r.top.join(".glia").exists());

    r.write("a.py", b"x = 1\n");
    r.commit("one", T0);
    let zero = HistoryOptions { max_commits: 0, ..HistoryOptions::default() };
    let err = history_sync(&r.top, &zero).unwrap_err();
    assert!(err.contains("max_commits"), "{err}");
    assert!(!r.top.join(".glia").exists());
}

#[test]
fn resync_is_byte_identical() {
    let r = probe_g1();
    // A hand-written .glia/.gitignore is never overwritten.
    r.write(".glia/.gitignore", b"keep-me\n");
    let first_summary = history_sync(&r.top, &with_blame()).expect("first sync");
    let first = r.snapshot_bytes(&r.top);
    assert!(!first[1].is_empty(), "blame on writes rows");
    let second_summary = history_sync(&r.top, &with_blame()).expect("second sync");
    assert_eq!(first_summary, second_summary);
    assert_eq!(first, r.snapshot_bytes(&r.top));
    assert_eq!(std::fs::read_to_string(r.top.join(".glia/.gitignore")).unwrap(), "keep-me\n");
}

#[test]
fn meta_written_last_guards_partial() {
    let r = probe_g1();
    history_sync(&r.top, &HistoryOptions::default()).expect("sync");
    assert!(read_history(&r.top).is_some());

    let commits = history_dir(&r.top).join(HISTORY_COMMITS_FILE);
    let bytes = std::fs::read(&commits).unwrap();
    std::fs::write(&commits, &bytes[..bytes.len() / 2]).unwrap();
    assert_eq!(read_history(&r.top), None, "truncated commits.jsonl");

    history_sync(&r.top, &HistoryOptions::default()).expect("resync");
    assert!(read_history(&r.top).is_some(), "a resync repairs it");
    std::fs::remove_file(history_dir(&r.top).join(META_FILE)).unwrap();
    assert_eq!(read_history(&r.top), None, "no meta.json");
}

#[test]
fn blame_runs_follow_commit_times() {
    let r = Repo::new();
    let t1 = T0 + DAY;
    let t2 = T0 + 9 * DAY;
    r.write("m.py", b"def f():\n    return 1\ndef g():\n    return 2\n");
    r.commit("f and g", t1);
    r.write("m.py", b"def f():\n    return 1\ndef g(x):\n    return x\n");
    r.commit("g changes", t2);

    let s = history_sync(&r.top, &with_blame()).expect("sync");
    assert_eq!((s.blame_files, s.runs), (1, 2), "{s:?}");
    let snap = read_history(&r.top).expect("snapshot");
    assert_eq!(snap.blame.len(), 1);
    assert_eq!(snap.blame[0].p, "m.py");
    // f() (lines 1-2) last changed at t1, g() (lines 3-4) at t2.
    assert_eq!(snap.blame[0].runs, [[1, 2, t1], [3, 4, t2]]);
}

#[test]
fn blame_cap_respected() {
    let r = Repo::new();
    // Commit counts: a=3, b=3, c=2, d=1, e=1; gone.py=4 but deleted at HEAD.
    let rounds: [&[&str]; 4] = [
        &["a.py", "b.py", "c.py", "d.py", "e.py", "gone.py"],
        &["a.py", "b.py", "c.py", "gone.py"],
        &["a.py", "b.py", "gone.py"],
        &[],
    ];
    for (i, files) in rounds.iter().enumerate() {
        for f in *files {
            if i == 0 {
                r.write(f, format!("# {f}\n").as_bytes());
            } else {
                r.append(f, &format!("# round {i}\n"));
            }
        }
        if i == 3 {
            r.git(&["rm", "-q", "gone.py"]);
        }
        r.commit(&format!("round {i}"), T0 + i as i64 * DAY);
    }

    let blamed = |cap: usize| -> Vec<String> {
        let opts = HistoryOptions { blame: true, blame_max_files: cap, ..HistoryOptions::default() };
        let s = history_sync(&r.top, &opts).expect("sync");
        let snap = read_history(&r.top).expect("snapshot");
        assert_eq!(s.blame_files, snap.blame.len());
        assert_eq!(snap.meta.blame_files, snap.blame.len());
        snap.blame.iter().map(|b| b.p.clone()).collect()
    };
    assert_eq!(blamed(2), ["a.py", "b.py"]);
    assert_eq!(blamed(3), ["a.py", "b.py", "c.py"]);
    // Ties on commit count break by path: d before e.
    assert_eq!(blamed(4), ["a.py", "b.py", "c.py", "d.py"]);
    assert_eq!(blamed(0), Vec::<String>::new());
    assert_eq!(blamed(300), ["a.py", "b.py", "c.py", "d.py", "e.py"]);
}

#[test]
fn since_and_max_commits_bound_the_window() {
    let r = probe_g1();
    let opts = HistoryOptions { max_commits: 2, ..HistoryOptions::default() };
    let s = history_sync(&r.top, &opts).expect("sync");
    assert_eq!(s.commits, 2);
    let snap = read_history(&r.top).expect("snapshot");
    assert_eq!(snap.meta.max_commits, 2);
    assert_eq!(snap.commits.iter().map(|c| c.t).collect::<Vec<_>>(), [T0 + 4 * DAY, T0 + 3 * DAY]);

    let since = "2026-01-02T12:00:00Z".to_string();
    let opts = HistoryOptions { since: Some(since.clone()), ..HistoryOptions::default() };
    let s = history_sync(&r.top, &opts).expect("sync");
    assert_eq!(s.commits, 3, "{s:?}");
    let snap = read_history(&r.top).expect("snapshot");
    assert_eq!(snap.meta.since, Some(since));
}

// --- EB.1: the commit-message sidecar ---

/// Set in a re-run of this test binary: the child half of a test whose
/// stderr the parent reads.
const CHILD_ENV: &str = "GLIA_EB1_STDERR_CHILD";

/// Re-run test `name` of this binary in a child process with [`CHILD_ENV`]
/// set; its stderr. The library's markers go straight to stderr there.
fn child_stderr(name: &str) -> String {
    let out = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, "1")
        .output()
        .expect("re-run the test binary");
    assert!(out.status.success(), "child {name} failed:\n{}", String::from_utf8_lossy(&out.stdout));
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn messages_off() -> HistoryOptions {
    HistoryOptions { messages: false, ..HistoryOptions::default() }
}

/// Three commits whose messages carry trailers, an identity line and a secret.
fn probe_messages() -> Repo {
    let r = Repo::new();
    r.write("a.py", b"x = 1\n");
    r.commit("init the app\n\nFirst body line.\n\nCo-Authored-By: Helper Bot <bot@identity.invalid>\n", T0);
    r.append("a.py", "x = 2\n");
    r.commit("second edit\n\nDeploy with token=sk-abcdefghijklmnopqrstuv\nSigned-off-by: Someone <s@identity.invalid>\n", T0 + DAY);
    r.append("a.py", "x = 3\n");
    r.commit("third edit", T0 + 2 * DAY);
    r
}

#[test]
fn messages_sidecar_written() {
    let r = probe_messages();
    let s = history_sync(&r.top, &HistoryOptions::default()).expect("sync");
    assert_eq!((s.commits, s.messages), (3, 3), "{s:?}");
    assert!(s.trailers_dropped >= 2 && s.redacted >= 1, "{s:?}");
    let snap = read_history(&r.top).expect("snapshot");
    assert_eq!(snap.meta.messages, Some(3));
    let msgs = read_history_messages(&r.top, &snap).expect("aligned messages");
    let shas: Vec<&str> = msgs.iter().map(|m| m.c.as_str()).collect();
    let commit_shas: Vec<&str> = snap.commits.iter().map(|c| c.c.as_str()).collect();
    assert_eq!(shas, commit_shas, "messages.jsonl is in commits.jsonl order");
    let subjects: Vec<&str> = msgs.iter().map(|m| m.s.as_str()).collect();
    assert_eq!(subjects, ["third edit", "second edit", "init the app"]);
    assert_eq!(msgs[2].b, "First body line.");
    assert!(msgs[1].b.starts_with("Deploy with token="), "{:?}", msgs[1].b);
    let text = std::fs::read_to_string(history_dir(&r.top).join(HISTORY_MESSAGES_FILE)).unwrap();
    assert_eq!(text.lines().count(), 3);
    for needle in ["Co-Authored-By", "Signed-off-by", "sk-abcdefghijklmnopqrstuv", "identity.invalid", AUTHOR_NAME] {
        assert!(!text.contains(needle), "messages.jsonl kept {needle:?}:\n{text}");
    }
    let at: i64 = r.git(&["log", "-1", "--format=%at", &msgs[0].c]).parse().unwrap();
    assert_eq!(msgs[0].at, at);
    assert!(msgs.iter().all(|m| m.br.is_none()), "no merge, no label");
}

#[test]
fn inputs_unchanged() {
    let r = probe_messages();
    let off = history_sync(&r.top, &messages_off()).expect("sync without messages");
    assert_eq!((off.messages, off.redacted, off.trailers_dropped, off.branch_labels), (0, 0, 0, 0));
    let [commits_off, blame_off, _] = r.snapshot_bytes(&r.top);
    let meta_off = read_history(&r.top).expect("snapshot").meta;
    history_sync(&r.top, &HistoryOptions::default()).expect("sync with messages");
    let [commits_on, blame_on, _] = r.snapshot_bytes(&r.top);
    let meta_on = read_history(&r.top).expect("snapshot").meta;
    assert_eq!(commits_off, commits_on);
    assert_eq!(blame_off, blame_on);
    assert_eq!(meta_off.data_hash, meta_on.data_hash);
    assert_eq!((meta_off.messages, meta_on.messages), (None, Some(3)));
}

#[test]
fn v1_snapshot_reads() {
    if std::env::var_os(CHILD_ENV).is_some() {
        let r = probe_messages();
        history_sync(&r.top, &messages_off()).expect("sync");
        let meta = read_history(&r.top).expect("snapshot").meta;
        // A hand-written 0.5.2 meta: version 1, no messages keys.
        let v1 = format!(
            "{{\"version\":1,\"generator\":\"glia history sync\",\"head\":\"{}\",\"commits\":3,\"max_commits\":2000,\"since\":null,\"relative_to\":\"\",\"blame_files\":0,\"data_hash\":\"{}\"}}",
            meta.head, meta.data_hash
        );
        std::fs::write(history_dir(&r.top).join(META_FILE), v1).unwrap();
        let snap = read_history(&r.top).expect("a version 1 snapshot reads");
        assert_eq!(snap.meta.version, 1);
        assert_eq!(read_history_messages(&r.top, &snap), None);
        return;
    }
    let err = child_stderr("v1_snapshot_reads");
    assert!(!err.contains("[history] messages incomplete") && !err.contains("snapshot incomplete"), "{err}");
}

#[test]
fn misaligned_messages_rejected() {
    if std::env::var_os(CHILD_ENV).is_some() {
        let r = probe_messages();
        history_sync(&r.top, &HistoryOptions::default()).expect("sync");
        let dir = history_dir(&r.top);
        let text = std::fs::read_to_string(dir.join(HISTORY_MESSAGES_FILE)).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        lines.swap(0, 1);
        let swapped = format!("{}\n", lines.join("\n"));
        std::fs::write(dir.join(HISTORY_MESSAGES_FILE), &swapped).unwrap();
        // Keep the hash in step, so only the sha alignment can catch it.
        let snap = read_history(&r.top).expect("snapshot");
        let old_hash = snap.meta.messages_hash.clone().expect("messages hash");
        let new_hash = data_hash(&[swapped.as_bytes()]);
        let meta = std::fs::read_to_string(dir.join(META_FILE)).unwrap();
        assert!(meta.contains(&old_hash));
        std::fs::write(dir.join(META_FILE), meta.replace(&old_hash, &new_hash)).unwrap();
        let snap = read_history(&r.top).expect("the build's snapshot is untouched");
        assert_eq!(read_history_messages(&r.top, &snap), None);
        return;
    }
    let err = child_stderr("misaligned_messages_rejected");
    let lines: Vec<&str> = err.lines().filter(|l| l.starts_with("[history] messages incomplete dir=")).collect();
    assert_eq!(lines.len(), 1, "{err}");
    assert!(lines[0].contains(" reason=row 1 names commit "), "{}", lines[0]);
}

#[test]
fn no_messages_removes_sidecar() {
    let r = probe_messages();
    history_sync(&r.top, &HistoryOptions::default()).expect("sync");
    assert!(history_dir(&r.top).join(HISTORY_MESSAGES_FILE).exists());
    history_sync(&r.top, &messages_off()).expect("sync without messages");
    assert!(!history_dir(&r.top).join(HISTORY_MESSAGES_FILE).exists());
    let snap = read_history(&r.top).expect("snapshot");
    assert_eq!(snap.meta.messages, None);
    assert_eq!(read_history_messages(&r.top, &snap), None);
}

#[test]
fn branch_labels_and_author_time() {
    let r = Repo::new();
    r.write("a.py", b"x = 1\n");
    r.commit("main init", T0);
    r.git(&["checkout", "-q", "-b", "feat-x"]);
    r.write("f.py", b"y = 1\n");
    r.commit("feat one", T0 + DAY);
    r.append("f.py", "y = 2\n");
    r.commit("feat two", T0 + 2 * DAY);
    r.git(&["checkout", "-q", "main"]);
    r.append("a.py", "x = 2\n");
    r.commit("main edit", T0 + 3 * DAY);
    r.git_at(&r.top, &["merge", "-q", "--no-ff", "-m", "Merge branch 'feat-x'", "feat-x"], T0 + 4 * DAY);

    let s = history_sync(&r.top, &HistoryOptions::default()).expect("sync");
    assert_eq!((s.commits, s.messages, s.branch_labels), (4, 4, 2), "{s:?}");
    let snap = read_history(&r.top).expect("snapshot");
    let msgs = read_history_messages(&r.top, &snap).expect("messages");
    let labels: Vec<(&str, Option<&str>)> = msgs.iter().map(|m| (m.s.as_str(), m.br.as_deref())).collect();
    assert_eq!(
        labels,
        [("main edit", None), ("feat two", Some("feat-x")), ("feat one", Some("feat-x")), ("main init", None)]
    );
    for m in &msgs {
        let at: i64 = r.git(&["log", "-1", "--format=%at", &m.c]).parse().unwrap();
        assert_eq!(m.at, at, "{}", m.s);
    }
    // --no-messages turns the labels off with the rest.
    let off = history_sync(&r.top, &messages_off()).expect("sync");
    assert_eq!(off.branch_labels, 0);
}

#[test]
fn branch_labels_take_the_smallest_side() {
    // A lane merged into a wave branch, the wave merged into main: the lane's
    // commits keep the lane (the smaller side), the wave's own the wave.
    let r = Repo::new();
    r.write("a.py", b"x = 1\n");
    r.commit("main init", T0);
    r.git(&["checkout", "-q", "-b", "wave"]);
    r.write("w.py", b"w = 1\n");
    r.commit("wave own", T0 + DAY);
    r.git(&["checkout", "-q", "-b", "lane"]);
    r.write("l.py", b"l = 1\n");
    r.commit("lane own", T0 + 2 * DAY);
    r.git(&["checkout", "-q", "wave"]);
    r.git_at(&r.top, &["merge", "-q", "--no-ff", "-m", "Merge branch 'lane' into wave", "lane"], T0 + 3 * DAY);
    r.git(&["checkout", "-q", "main"]);
    r.git_at(&r.top, &["merge", "-q", "--no-ff", "-m", "Merge branch 'wave'", "wave"], T0 + 4 * DAY);
    history_sync(&r.top, &HistoryOptions::default()).expect("sync");
    let snap = read_history(&r.top).expect("snapshot");
    let msgs = read_history_messages(&r.top, &snap).expect("messages");
    let mut labels: Vec<(&str, Option<&str>)> = msgs.iter().map(|m| (m.s.as_str(), m.br.as_deref())).collect();
    labels.sort();
    assert_eq!(labels, [("lane own", Some("lane")), ("main init", None), ("wave own", Some("wave"))]);
}
