//! LF.5d — `glia history sync`, the CLI surface of the history snapshot step
//! (`glia_snapshots::history_sync`), driving the real binary over temporary git
//! repos. Every commit is made with LF.5a's fixed identity and fixed author /
//! committer dates under an empty global git config, so the SHAs are
//! reproducible. These tests need a `git` binary: without one they FAIL with a
//! message saying so.
//!
//! The `[history] sync ... surface=cli` stderr line is the fired_on marker;
//! asserting it here makes it a tested contract. Grep it with
//! `cargo test -p glia-cli --test history_cli -- --nocapture 2>&1 | grep -o '\[history\] sync .*surface=cli'`.

use std::path::PathBuf;
use std::process::{Command, Output};

use glia_code_domain::snapshots::{
    HISTORY_BLAME_FILE, HISTORY_COMMITS_FILE, HISTORY_GENERATOR, HISTORY_MESSAGES_FILE, META_FILE, history_dir,
};
use serde_json::Value;

const AUTHOR_NAME: &str = "Quillon Identitymarker";
const AUTHOR_EMAIL: &str = "quillon.author@identity.invalid";
const COMMITTER_NAME: &str = "Pemberly Committertoken";
const COMMITTER_EMAIL: &str = "pemberly.committer@identity.invalid";
/// 2026-01-01T00:00:00Z.
const T0: i64 = 1_767_225_600;
const DAY: i64 = 86_400;

/// A scratch dir holding a git work tree (`repo/`) and the empty global git
/// config every git call runs under; removed on drop (`cli` has no
/// dev-dependencies, so no `tempfile`).
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
    /// A fresh scratch dir; `repo/` exists but is not a git repo yet.
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-lf5d-{}-{name}", std::process::id()));
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

    /// A fresh scratch dir with `repo/` initialised as a git repo.
    fn git_repo(name: &str) -> Self {
        let s = Scratch::new(name);
        s.git(&["init", "-q", "-b", "main"], T0);
        s
    }

    fn path(&self) -> &str {
        self.top.to_str().expect("utf-8 scratch path")
    }

    /// Run git in the repo with the fixed identity and date `t`; its trimmed stdout.
    fn git(&self, args: &[&str], t: i64) -> String {
        let date = format!("@{t} +0000");
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.top)
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
                panic!("LF.5d history tests need a `git` binary on PATH; running it failed: {e}")
            });
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

    fn append(&self, rel: &str, line: &str) {
        let path = self.top.join(rel);
        let mut body = std::fs::read_to_string(&path).expect("read");
        body.push_str(line);
        std::fs::write(path, body).expect("append");
    }

    /// Stage everything and commit at unix time `t`.
    fn commit(&self, msg: &str, t: i64) {
        self.git(&["add", "-A"], t);
        self.git(&["commit", "-q", "-m", msg], t);
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"], T0)
    }

    /// `<repo>/.glia/history-snapshot/<file>`, or "" when absent.
    fn snapshot_file(&self, file: &str) -> String {
        std::fs::read_to_string(history_dir(&self.top).join(file)).unwrap_or_default()
    }

    /// Run the real binary, git config pinned to the empty file. Relays the
    /// markers so `-- --nocapture | grep '^\[history\] '` sees them.
    fn glia(&self, args: &[&str]) -> Output {
        let out = Command::new(env!("CARGO_BIN_EXE_glia"))
            .args(args)
            .env("GLIA_NO_PERSIST", "1")
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .output()
            .expect("glia runs");
        for line in text(&out.stderr).lines() {
            if line.starts_with("[history] ") {
                eprintln!("{line}");
            }
        }
        out
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The `[history] sync` line of `stderr`.
fn sync_marker(stderr: &str) -> &str {
    stderr
        .lines()
        .find(|l| l.starts_with("[history] sync "))
        .unwrap_or_else(|| panic!("no `[history] sync` marker in stderr:\n{stderr}"))
}

/// Probe g1 (LF.5a's): svc/a.py and svc/b.py change together in 4 commits
/// (the initial one and 3 edits), then svc/c.py is renamed to svc/c2.py.
/// 5 commits on 2026-01-01 .. 2026-01-05 (UTC midnight).
fn probe_g1(name: &str) -> Scratch {
    let r = Scratch::git_repo(name);
    r.write("svc/a.py", "def a():\n    return 1\n");
    r.write("svc/b.py", "def b():\n    return 2\n");
    r.write("svc/c.py", "def c():\n    return 3\n");
    r.commit("init", T0);
    for i in 1..=3 {
        r.append("svc/a.py", &format!("# edit {i}\n"));
        r.append("svc/b.py", &format!("# edit {i}\n"));
        r.commit(&format!("co-change {i}"), T0 + i * DAY);
    }
    r.git(&["mv", "svc/c.py", "svc/c2.py"], T0 + 4 * DAY);
    r.commit("rename c", T0 + 4 * DAY);
    r
}

#[test]
fn sync_writes_the_snapshot_and_names_the_cli_surface() {
    let r = probe_g1("sync");
    let out = r.glia(&["history", "sync", r.path()]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let head12: String = r.head().chars().take(12).collect();
    let snapshot = history_dir(&r.top);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            format!("synced 5 commits (head {head12}) -> {}", snapshot.display()),
            format!("run `glia build {}` to ingest.", r.path()),
        ],
    );
    let marker = sync_marker(&stderr);
    assert!(
        marker.ends_with(&format!(
            " head={head12} commits=5 files=3 renames=1 binary=0 blame_files=0 runs=0 window=max:2000 \
             messages=5 redacted=0 branch_labels=0 trailers_dropped=0 surface=cli"
        )),
        "{marker}"
    );

    for file in [HISTORY_COMMITS_FILE, HISTORY_BLAME_FILE, HISTORY_MESSAGES_FILE, META_FILE] {
        assert!(
            snapshot.join(file).is_file(),
            "{file} not written under {}",
            snapshot.display()
        );
    }
    assert_eq!(r.snapshot_file(HISTORY_COMMITS_FILE).lines().count(), 5);
    assert_eq!(
        r.snapshot_file(HISTORY_BLAME_FILE),
        "",
        "no --blame, no blame rows"
    );
    assert_eq!(r.snapshot_file(HISTORY_MESSAGES_FILE).lines().count(), 5);
    let meta: Value = serde_json::from_str(&r.snapshot_file(META_FILE)).expect("meta.json parses");
    assert_eq!(meta["generator"], HISTORY_GENERATOR, "{meta}");
    assert_eq!((meta["version"].as_u64(), meta["messages"].as_u64()), (Some(2), Some(5)), "{meta}");
}

#[test]
fn no_messages_flag_keeps_no_sidecar() {
    let r = probe_g1("nomessages");
    let out = r.glia(&["history", "sync", r.path()]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let commits = r.snapshot_file(HISTORY_COMMITS_FILE);
    let out = r.glia(&["history", "sync", r.path(), "--no-messages"]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let marker = sync_marker(&stderr);
    assert!(marker.ends_with(" window=max:2000 messages=off surface=cli"), "{marker}");
    assert!(!history_dir(&r.top).join(HISTORY_MESSAGES_FILE).exists(), "--no-messages removes the sidecar");
    assert_eq!(r.snapshot_file(HISTORY_COMMITS_FILE), commits, "the build's input is unchanged");
    let meta: Value = serde_json::from_str(&r.snapshot_file(META_FILE)).expect("meta.json parses");
    assert!(meta.get("messages").is_none() && meta.get("messages_hash").is_none(), "{meta}");
}

#[test]
fn blame_flag_writes_blame_rows() {
    let r = probe_g1("blame");
    let out = r.glia(&[
        "history",
        "sync",
        r.path(),
        "--blame",
        "--blame-max-files",
        "2",
    ]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");

    let blame = r.snapshot_file(HISTORY_BLAME_FILE);
    assert_eq!(
        blame.lines().count(),
        2,
        "--blame-max-files 2 blames the 2 most-changed files:\n{blame}"
    );
    let paths: Vec<String> = blame
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).expect("blame row parses")["p"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert_eq!(paths, ["svc/a.py", "svc/b.py"]);
    let marker = sync_marker(&stderr);
    assert!(
        marker.contains(" commits=5 ") && marker.contains(" blame_files=2 "),
        "{marker}"
    );
    assert!(
        !marker.contains(" runs=0 "),
        "blamed files have runs: {marker}"
    );
}

#[test]
fn window_flags_reach_the_capture() {
    let r = probe_g1("window");
    let out = r.glia(&["history", "sync", r.path(), "--max-commits", "2"]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stdout.starts_with("synced 2 commits (head "), "{stdout}");
    let marker = sync_marker(&stderr);
    assert!(
        marker.contains(" commits=2 ") && marker.contains(" window=max:2 messages=2 ") && marker.ends_with(" surface=cli"),
        "{marker}"
    );
    assert_eq!(r.snapshot_file(HISTORY_COMMITS_FILE).lines().count(), 2);

    // Commits dated 2026-01-03 .. 2026-01-05 are after the cut; a re-sync
    // replaces the earlier snapshot.
    let out = r.glia(&[
        "history",
        "sync",
        r.path(),
        "--since",
        "2026-01-02T12:00:00Z",
    ]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let marker = sync_marker(&stderr);
    assert!(
        marker.contains(" commits=3 ")
            && marker.contains(" window=max:2000,since:2026-01-02T12:00:00Z messages=3 ")
            && marker.ends_with(" surface=cli"),
        "{marker}"
    );
    assert_eq!(r.snapshot_file(HISTORY_COMMITS_FILE).lines().count(), 3);
}

#[test]
fn a_bad_repo_exits_1_and_writes_nothing() {
    // A directory outside any git work tree.
    let plain = Scratch::new("plain");
    plain.write("app.py", "def f():\n    return 1\n");
    let out = plain.glia(&["history", "sync", plain.path()]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.starts_with("error: ") && stderr.contains("is not inside a git work tree"),
        "{stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        !plain.top.join(".glia").exists(),
        "a failed sync writes nothing"
    );
    assert!(
        !stderr.contains("[history] sync "),
        "no marker on failure: {stderr}"
    );

    // A path that does not exist.
    let missing = plain.top.join("no-such-dir");
    let out = plain.glia(&["history", "sync", missing.to_str().expect("utf-8")]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.starts_with("error: ") && stderr.contains("is not a directory"),
        "{stderr}"
    );

    // A git repo with no commits.
    let empty = Scratch::git_repo("nocommits");
    let out = empty.glia(&["history", "sync", empty.path()]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.starts_with("error: ") && stderr.contains("has no commits"),
        "{stderr}"
    );

    // An empty window is refused by the library, not clamped.
    let r = probe_g1("zero");
    let out = r.glia(&["history", "sync", r.path(), "--max-commits", "0"]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.starts_with("error: ") && stderr.contains("max_commits must be at least 1"),
        "{stderr}"
    );
    assert!(
        !history_dir(&r.top).exists(),
        "a refused sync writes nothing"
    );
}

#[test]
fn a_synced_snapshot_becomes_a_co_changes_edge() {
    let r = probe_g1("cochange");
    let out = r.glia(&["history", "sync", r.path()]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));

    let out = r.glia(&["analyze", r.path(), "--format", "json"]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("[history] ingest repo="),
        "the build ingested the snapshot:\n{stderr}"
    );
    let graph: Value = serde_json::from_str(stdout.trim()).expect("analyze json parses");
    let qname_of = |id: &Value| -> String {
        graph["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|n| &n["id"] == id)
            .map(|n| {
                format!(
                    "{} {}",
                    n["kind_name"].as_str().unwrap_or(""),
                    n["qname"].as_str().unwrap_or("")
                )
            })
            .unwrap_or_default()
    };
    let pairs: Vec<(String, String)> = graph["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["category"] == "CO_CHANGES")
        .map(|e| (qname_of(&e["from"]), qname_of(&e["to"])))
        .collect();
    assert_eq!(
        pairs,
        [("MODULE svc::a".to_string(), "MODULE svc::b".to_string())]
    );
}

/// The build never runs git: without a sync, the same repo has no history
/// input and no CO_CHANGES edge.
#[test]
fn without_a_sync_the_build_has_no_history() {
    let r = probe_g1("nosync");
    let out = r.glia(&["analyze", r.path(), "--format", "json"]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("[history] "), "{stderr}");
    assert!(
        !stdout.contains("\"CO_CHANGES\""),
        "no snapshot, no CO_CHANGES edge"
    );
    assert!(!history_dir(&r.top).exists(), "analyze never syncs");
}

// ---------------------------------------------------------------------------
// EC.6 — `glia history calibrate`: the calibration step's CLI transport over
// `glia_engine::calibrate::calibrate`. The fixture is EC.5's (two clusters of
// Python modules around a hub; engine/tests/calibrate_step.rs), smaller hub,
// with a synthetic history snapshot written by `write_history`: no git.
// The fired_on marker is `[calibrate] done surface=cli sets=<n>
// test_commits=<t> ms=<ms>` (`cargo test -p glia-cli --test history_cli --
// --nocapture 2>&1 | grep '^\[calibrate\] done surface=cli'`).

/// Files per cluster.
const CAL_N: usize = 12;
/// Commits in the synthetic history.
const CAL_COMMITS: usize = 40;
/// Hub functions that call `h`.
const CAL_FILLERS: usize = 60;

fn cal_cluster(c: &str, i: usize) -> String {
    format!("{c}/{c}{i:02}.py")
}

/// EC.5's cluster repo under `<scratch>/repo`.
fn calibrate_repo(name: &str) -> Scratch {
    let s = Scratch::new(name);
    let mut hub = String::from("def h():\n    return 0\n\n");
    for i in 0..CAL_N {
        hub.push_str(&format!("from alpha.alpha{i:02} import alpha{i:02}_f\n"));
    }
    hub.push_str("\ndef r2():\n");
    for i in 0..CAL_N {
        hub.push_str(&format!("    alpha{i:02}_f()\n"));
    }
    hub.push_str("    return 0\n\n");
    for i in 0..CAL_N {
        hub.push_str(&format!("def g{i:02}():\n    return r2()\n\n"));
    }
    for i in 0..CAL_FILLERS {
        hub.push_str(&format!("def p{i:04}():\n    return h()\n\n"));
    }
    s.write("hub.py", &hub);
    for c in ["alpha", "zeta"] {
        s.write(&format!("{c}/__init__.py"), "");
    }
    for i in 0..CAL_N {
        s.write(
            &cal_cluster("alpha", i),
            &format!("import hub\n\ndef alpha{i:02}_f():\n    return {i}\n"),
        );
        s.write(
            &cal_cluster("zeta", i),
            &format!(
                "from hub import h, g{i:02}\n\ndef zeta{i:02}_f():\n    h()\n    return g{i:02}()\n"
            ),
        );
    }
    s
}

/// The synthetic history snapshot: commit i touches 2..=6 neighbouring zeta
/// modules; written newest first, no messages.
fn calibrate_history(s: &Scratch) {
    use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};
    let mut commits: Vec<HistoryCommit> = (0..CAL_COMMITS)
        .map(|i| HistoryCommit {
            c: format!("{:040x}", i + 1),
            t: 1_700_000_000 + i as i64 * 3600,
            files: (0..2 + i % 5)
                .map(|j| HistoryFile {
                    p: cal_cluster("zeta", ((i * 5) % CAL_N + j) % CAL_N),
                    a: Some(1),
                    d: Some(0),
                    from: None,
                })
                .collect(),
        })
        .collect();
    commits.reverse();
    let meta = HistoryMeta::new(format!("{:040x}", CAL_COMMITS), CAL_COMMITS, None, String::new());
    write_history(&s.top, meta, &commits, &[]).expect("write history");
}

fn calibration_record(s: &Scratch) -> PathBuf {
    s.top.join(".glia").join("calibration-snapshot")
}

#[test]
fn calibrate_writes_the_record_and_prints_each_pick() {
    let s = calibrate_repo("cal-ok");
    calibrate_history(&s);
    let out = s.glia(&["history", "calibrate", s.path()]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("SELh0 k1: pick "), "{stdout}");
    assert!(stdout.contains("SELs0 half: pick "), "{stdout}");
    // EC.11: the default groups include `review` (set SELt2, EC.12).
    assert!(stdout.contains("SELt2 k1: pick "), "{stdout}");
    // EC.13: and `find` (set SELf2t, the find shape).
    assert!(stdout.contains("SELf2t find: pick "), "{stdout}");
    assert!(stdout.contains(" queries (online recall@10 "), "{stdout}");
    assert!(
        stdout.lines().last().is_some_and(|l| l.starts_with("wrote ")
            && l.ends_with("calibration-snapshot")),
        "{stdout}"
    );
    let marker = stderr
        .lines()
        .find(|l| l.starts_with("[calibrate] done "))
        .unwrap_or_else(|| panic!("no `[calibrate] done` marker:\n{stderr}"));
    eprintln!("{marker}");
    assert!(
        marker.starts_with(&format!(
            "[calibrate] done surface=cli sets=13 test_commits={CAL_COMMITS} ms="
        )),
        "{marker}"
    );
    assert!(calibration_record(&s).is_dir(), "the record is written");
}

#[test]
fn calibrate_json_prints_the_summary() {
    let s = calibrate_repo("cal-json");
    calibrate_history(&s);
    let out = s.glia(&[
        "history",
        "calibrate",
        s.path(),
        "--json",
        "--max-commits",
        "20",
        "--max-seeds",
        "2",
        "--sets",
        "structure",
    ]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let v: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}"));
    let sets = v["sets"].as_array().expect("sets");
    assert_eq!(sets.len(), 8, "{v}");
    assert_eq!(sets[0]["set"], "SELh0");
    assert_eq!(sets[0]["shape"], "k1");
    assert_eq!(v["test_commits"], 20, "--max-commits reaches the replay");
    // --max-seeds 2: every commit maps at least 2 files, so 2 k1 queries each.
    assert_eq!(sets[0]["queries"], 40, "--max-seeds reaches the replay");
    assert_eq!(v["commits"], CAL_COMMITS);
}

#[test]
fn calibrate_without_a_snapshot_exits_1_naming_sync() {
    let s = calibrate_repo("cal-nosnap");
    let out = s.glia(&["history", "calibrate", s.path()]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("glia history sync"), "{stderr}");
    assert!(!calibration_record(&s).exists(), "nothing written");
}

#[test]
fn calibrate_an_unknown_set_exits_1_listing_the_known_ones() {
    let s = calibrate_repo("cal-nope");
    calibrate_history(&s);
    let out = s.glia(&["history", "calibrate", s.path(), "--sets", "nope"]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("nope") && stderr.contains("structure"), "{stderr}");
    assert!(!calibration_record(&s).exists(), "nothing written");
}

#[test]
fn calibrate_a_missing_repo_exits_2() {
    let s = Scratch::new("cal-missing");
    let gone = s.top.join("no-such-dir");
    let out = s.glia(&["history", "calibrate", gone.to_str().expect("utf-8")]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
}
