//! EB.3: `glia_engine::intent::text`, BM25 over the history snapshot's past
//! commit messages scoring the files those commits touched.
//!
//! A temp tree of 32 Python files is built with `generate_one`, and a history
//! snapshot with messages is written through code-domain's
//! `write_history_with_messages` (no git needed), as history_ingest.rs does.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use glia_code_domain::snapshots::{
    HistoryCommit, HistoryFile, HistoryMessage, HistoryMeta, write_history, write_history_with_messages,
};
use glia_engine::generate_one;
use glia_engine::intent::text::{Candidates, TextAbsent, TextIndex};
use glia_engine::rank::FileIndex;

const T0: i64 = 1_767_225_600;

/// The six named sources plus `svc/m00.py` .. `svc/m25.py`.
fn sources() -> Vec<String> {
    let mut v: Vec<String> = ["a", "b", "c", "d", "e", "f"].iter().map(|n| format!("svc/{n}.py")).collect();
    v.extend((0..26).map(|i| format!("svc/m{i:02}.py")));
    v
}

fn sha(n: usize) -> String {
    format!("{n:02}{}", "0".repeat(38))
}

fn file(p: &str) -> HistoryFile {
    HistoryFile { p: p.to_string(), a: Some(1), d: Some(0), from: None }
}

/// One (commit, message) pair; `files` are paths.
fn commit(n: usize, files: &[&str], subject: &str, body: &str) -> (HistoryCommit, HistoryMessage) {
    let c = HistoryCommit { c: sha(n), t: T0 + n as i64 * 3600, files: files.iter().map(|p| file(p)).collect() };
    let m = HistoryMessage { c: sha(n), at: c.t, s: subject.to_string(), b: body.to_string(), br: None };
    (c, m)
}

/// The history, oldest first here; written newest first.
fn history() -> Vec<(HistoryCommit, HistoryMessage)> {
    let all = sources();
    let wide: Vec<&str> = all.iter().take(31).map(String::as_str).collect();
    let mut v = vec![
        commit(1, &["svc/a.py", "svc/b.py"], "Teach the tokenizer about unicode", "Splits words properly."),
        commit(2, &["svc/c.py"], "codec: handle CODEC-1234 padding", ""),
        commit(3, &["svc/d.py", "README.md"], "codec: handle CODEC-4242 framing", "Framing bytes."),
        commit(4, &["svc/old_e.py", "svc/f.py"], "Zebra striping in the renderer", ""),
        commit(5, &wide, "Sweeping walrus rename everywhere", ""),
        commit(6, &["svc/c.py"], "Excluded pelican change", ""),
        commit(7, &["docs/x.md"], "pelican docs only", ""),
    ];
    let (mut rename, m) = commit(8, &["svc/e.py"], "Move the renderer module", "");
    rename.files[0].from = Some("svc/old_e.py".to_string());
    v.push((rename, m));
    v
}

fn tree() -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(d.path().join("svc")).expect("mkdir");
    for (i, p) in sources().iter().enumerate() {
        std::fs::write(d.path().join(p), format!("def f{i}():\n    return {i}\n")).expect("write");
    }
    std::fs::write(d.path().join("README.md"), "# t\n").expect("write");
    d
}

fn write_snapshot(root: &Path, with_messages: bool) {
    let mut h = history();
    h.reverse();
    let (commits, messages): (Vec<HistoryCommit>, Vec<HistoryMessage>) = h.into_iter().unzip();
    let meta = HistoryMeta::new(commits[0].c.clone(), 2000, None, String::new());
    if with_messages {
        write_history_with_messages(root, meta, &commits, &[], Some(&messages)).expect("write snapshot");
    } else {
        write_history(root, meta, &commits, &[]).expect("write snapshot");
    }
}

/// The repo's candidates and the label it prints by.
fn candidates(root: &Path) -> (Candidates, String) {
    let built = generate_one(&root.to_string_lossy()).expect("build");
    let files = FileIndex::of(&built.merged, &built.repo_labels);
    let (&repo, label) = built.repo_labels.iter().next().expect("one repo");
    (Candidates::of(&files, repo), label.clone())
}

fn index(root: &Path, exclude: &[usize]) -> (TextIndex, Candidates, String) {
    let (c, label) = candidates(root);
    let ex: BTreeSet<String> = exclude.iter().map(|&n| sha(n)).collect();
    let idx = TextIndex::load(root, &c, &ex).expect("index");
    (idx, c, label)
}

/// Candidate paths with a score above 0, best first, ties by path.
fn ranked(idx: &TextIndex, c: &Candidates, q: &str) -> Vec<(String, f64)> {
    let s = idx.scores(q);
    let mut v: Vec<(usize, f64)> = s.iter().copied().enumerate().filter(|x| x.1 > 0.0).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().map(|(i, x)| (c.paths[i].clone(), x)).collect()
}

fn names(r: &[(String, f64)]) -> Vec<&str> {
    r.iter().map(|x| x.0.as_str()).collect()
}

#[test]
fn candidates_are_the_code_modules() {
    let d = tree();
    write_snapshot(d.path(), true);
    let (c, _) = candidates(d.path());
    assert_eq!(c.paths, sources().into_iter().collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>());
    assert_eq!(c.position("svc/c.py"), Some(2));
    assert_eq!(c.position("README.md"), None);
    assert_eq!(c.deg.len(), c.len());
}

#[test]
fn a_subject_word_ranks_its_commit_files_first() {
    let d = tree();
    write_snapshot(d.path(), true);
    let (idx, c, _) = index(d.path(), &[6]);
    let r = ranked(&idx, &c, "tokenizer crash");
    assert_eq!(names(&r), ["svc/a.py", "svc/b.py"]);
    assert_eq!(r[0].1.to_bits(), r[1].1.to_bits());
    assert_eq!(idx.matched_docs("tokenizer crash"), 1);
    assert_eq!(idx.top_commits("tokenizer crash").iter().map(|x| x.0).collect::<Vec<_>>(), [sha(1)]);
}

#[test]
fn a_digit_token_matches_its_commit() {
    let d = tree();
    write_snapshot(d.path(), true);
    let (idx, c, _) = index(d.path(), &[]);
    // `codec` matches commits 2 and 3; `4242` only 3, which ranks first.
    let r = ranked(&idx, &c, "CODEC-4242 regression");
    assert_eq!(names(&r), ["svc/d.py", "svc/c.py"]);
    assert!(r[0].1 > r[1].1);
    // README.md is no candidate: commit 3 scores d.py alone.
    assert_eq!(idx.matched_docs("CODEC-4242"), 2);
}

#[test]
fn a_wide_commit_and_an_empty_one_are_skipped() {
    let d = tree();
    write_snapshot(d.path(), true);
    let (idx, c, _) = index(d.path(), &[]);
    let st = idx.stats();
    // 8 commits: 5 (31 files) wide, 7 (docs only) empty, 6 indexed.
    assert_eq!((st.docs, st.skipped_wide, st.skipped_empty, st.excluded), (6, 1, 1, 0));
    assert!(ranked(&idx, &c, "walrus").is_empty());
    assert_eq!(idx.matched_docs("walrus"), 0);
}

#[test]
fn an_excluded_sha_contributes_nothing() {
    let d = tree();
    write_snapshot(d.path(), true);
    let (all, c, _) = index(d.path(), &[]);
    assert_eq!(names(&ranked(&all, &c, "pelican")), ["svc/c.py"]);
    let (idx, c, _) = index(d.path(), &[6]);
    assert!(ranked(&idx, &c, "pelican").is_empty());
    assert_eq!(idx.stats().excluded, 1);
    assert_eq!(idx.stats().docs, 5);
}

#[test]
fn a_pre_rename_commit_old_path_drops() {
    let d = tree();
    write_snapshot(d.path(), true);
    let (idx, c, _) = index(d.path(), &[]);
    // Commit 4 touched svc/old_e.py (now svc/e.py) and svc/f.py: no rename
    // fold, so only f.py scores.
    assert_eq!(names(&ranked(&idx, &c, "zebra")), ["svc/f.py"]);
    // The rename commit itself names the current path.
    assert_eq!(names(&ranked(&idx, &c, "renderer")), ["svc/e.py", "svc/f.py"]);
}

#[test]
fn absent_history_and_absent_messages() {
    let d = tree();
    let (c, _) = candidates(d.path());
    assert_eq!(TextIndex::load(d.path(), &c, &BTreeSet::new()).err(), Some(TextAbsent::History));
    write_snapshot(d.path(), false);
    assert_eq!(TextIndex::load(d.path(), &c, &BTreeSet::new()).err(), Some(TextAbsent::Messages));
    assert_eq!(TextAbsent::Messages.as_str(), "messages=absent");
}

const QUERIES: [&str; 4] = ["tokenizer crash", "CODEC-4242 regression", "zebra renderer", "pelican"];

/// Every query's scores as bits, one line each.
fn score_lines(root: &Path) -> Vec<String> {
    let (idx, _, _) = index(root, &[]);
    QUERIES
        .iter()
        .map(|q| idx.scores(q).iter().map(|x| format!("{:016x}", x.to_bits())).collect::<Vec<_>>().join(","))
        .collect()
}

/// Set by [`scores_are_identical_across_builds_and_threads`] on its child run.
const CHILD_ENV: &str = "GLIA_EB3_CHILD_DIR";

/// A no-op unless run as the child of
/// [`scores_are_identical_across_builds_and_threads`]: prints its scores and
/// the fired_on markers on stderr.
#[test]
fn child_scores_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        let root = Path::new(&dir);
        for l in score_lines(root) {
            eprintln!("[eb3-scores] {l}");
        }
        let (idx, _, label) = index(root, &[6]);
        idx.report(&label, "test", "CODEC-4242 regression");
        TextAbsent::History.report(&label, "test");
    }
}

#[test]
fn scores_are_identical_across_builds_and_threads() {
    let d = tree();
    write_snapshot(d.path(), true);
    let first = score_lines(d.path());
    assert_eq!(first, score_lines(d.path()), "two builds");
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_scores_for_stderr", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, d.path())
        .env("GLIA_THREADS", "1")
        .output()
        .expect("re-run the test binary");
    assert!(out.status.success(), "child failed: {}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let child: Vec<String> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix("[eb3-scores] "))
        .map(String::from)
        .collect();
    assert_eq!(child, first, "GLIA_THREADS=1");
    let markers: Vec<&str> = stderr.lines().filter(|l| l.starts_with("[intent] text ")).collect();
    let label = candidates(d.path()).1;
    assert_eq!(
        markers,
        [
            format!(
                "[intent] text repo={label} surface=test docs=5 skipped_wide=1 excluded=1 tokens=3 matched_docs=2 files=2"
            ),
            format!("[intent] text repo={label} surface=test history=absent"),
        ]
    );
}
