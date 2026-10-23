//! EC.5: the calibration step. `calibrate::calibrate` reads the history
//! snapshot, replays it over the layout's graph, writes
//! `.glia/calibration-snapshot/` and returns the online selection against the
//! fallback with a commit-cluster bootstrap CI.

use std::path::{Path, PathBuf};

use glia_code_domain::snapshots::{
    HistoryCommit, HistoryFile, HistoryMessage, HistoryMeta, read_history, read_history_messages,
    write_history, write_history_with_messages,
};
use glia_engine::calibrate::record::{
    CALIBRATION_FILE, CalibrationRead, calibration_dir, read_calibration,
};
use glia_engine::calibrate::{
    COCHANGE_CANDIDATES, COCHANGE_FALLBACK, CalibrateOptions, CalibrationSummary,
    REVIEW_CANDIDATES, REVIEW_FALLBACK, STRUCTURE_CANDIDATES, STRUCTURE_FALLBACK, SetSummary,
    Shape, bootstrap_ci, calibrate, calibrate_snapshot, select,
};

/// Files per cluster.
const N: usize = 12;
/// Commits in the synthetic history.
const COMMITS: usize = 60;
/// Hub functions that call `h`.
const FILLERS: usize = 1000;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(p, body).expect("write");
}

fn cluster_path(c: &str, i: usize) -> String {
    format!("{c}/{c}{i:02}.py")
}

/// Two clusters of `N` Python modules (`alpha/`, `zeta/`) around one hub
/// module every module imports. Each zeta module's function calls the hub's
/// `h` and its own relay `g<i>`; every relay calls `r2`, which calls each alpha
/// function. `h` is also called by `FILLERS` hub functions, so a walk through
/// it is diluted: a zeta module is two carry hops from another (B0 ranks the
/// cluster first), while PPR reaches the alpha functions (three hops through
/// the undiluted relays) with more mass than the zeta ones.
fn repo(root: &Path) {
    let mut hub = String::from("def h():\n    return 0\n\n");
    for i in 0..N {
        hub.push_str(&format!("from alpha.alpha{i:02} import alpha{i:02}_f\n"));
    }
    hub.push_str("\ndef r2():\n");
    for i in 0..N {
        hub.push_str(&format!("    alpha{i:02}_f()\n"));
    }
    hub.push_str("    return 0\n\n");
    for i in 0..N {
        hub.push_str(&format!("def g{i:02}():\n    return r2()\n\n"));
    }
    for i in 0..FILLERS {
        hub.push_str(&format!("def p{i:04}():\n    return h()\n\n"));
    }
    write(root, "hub.py", &hub);
    for c in ["alpha", "zeta"] {
        write(root, &format!("{c}/__init__.py"), "");
    }
    for i in 0..N {
        write(
            root,
            &cluster_path("alpha", i),
            &format!("import hub\n\ndef alpha{i:02}_f():\n    return {i}\n"),
        );
        write(
            root,
            &cluster_path("zeta", i),
            &format!(
                "from hub import h, g{i:02}\n\ndef zeta{i:02}_f():\n    h()\n    return g{i:02}()\n"
            ),
        );
    }
}

/// The files of commit `i` (oldest first): 2..=6 neighbouring zeta modules.
fn commit_files(i: usize) -> Vec<String> {
    let size = 2 + i % 5;
    let start = (i * 5) % N;
    (0..size)
        .map(|j| cluster_path("zeta", (start + j) % N))
        .collect()
}

/// The synthetic history: `COMMITS` commits, no messages, written newest first.
fn history(root: &Path) {
    let mut commits: Vec<HistoryCommit> = (0..COMMITS)
        .map(|i| HistoryCommit {
            c: format!("{:040x}", i + 1),
            t: 1_700_000_000 + i as i64 * 3600,
            files: commit_files(i)
                .into_iter()
                .map(|p| HistoryFile {
                    p,
                    a: Some(1),
                    d: Some(0),
                    from: None,
                })
                .collect(),
        })
        .collect();
    commits.reverse();
    let meta = HistoryMeta::new(format!("{:040x}", COMMITS), COMMITS, None, String::new());
    write_history(root, meta, &commits, &[]).expect("write history");
}

/// `generate_one` persists nothing (the layout write is `persist`'s); the
/// tree is a tempdir either way.
fn build(root: &Path) -> glia_engine::GenerateResult {
    glia_engine::generate_one(root.to_str().expect("utf-8")).expect("build")
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("clusters");
    repo(&root);
    (tmp, root)
}

fn row<'a>(s: &'a CalibrationSummary, set: &str, shape: &str) -> &'a SetSummary {
    s.sets
        .iter()
        .find(|r| r.set == set && r.shape == shape)
        .unwrap_or_else(|| panic!("no row {set} {shape}"))
}

#[test]
fn calibrate_writes_a_loaded_record_and_measures_the_selection() {
    let (_tmp, root) = fixture();
    history(&root);
    let r = build(&root);
    let opts = CalibrateOptions::default();
    let s = calibrate(&root, &r.merged, &opts).expect("calibrate");

    let CalibrationRead::Loaded(cal) = read_calibration(&root) else {
        panic!("record not loaded");
    };
    let rows: Vec<(String, String)> = cal
        .sets
        .iter()
        .map(|r| (r.set.clone(), r.shape.clone()))
        .collect();
    // EC.11: the default groups are `structure` then `review`; the record
    // sorts its rows by set name.
    let want: Vec<(String, String)> = ["SELh0", "SELs0", "SELt"]
        .iter()
        .flat_map(|set| ["k1", "k2", "k3", "half"].map(|sh| (set.to_string(), sh.to_string())))
        .collect();
    assert_eq!(rows, want);
    assert_eq!(
        s.sets
            .iter()
            .map(|r| (r.set.clone(), r.shape.clone()))
            .collect::<Vec<_>>(),
        want
    );

    // Every commit maps 2..=6 files: all are test commits, each with
    // min(5, files) one-seed queries.
    assert_eq!(s.commits, COMMITS);
    assert_eq!(s.test_commits, COMMITS);
    assert!(!s.messages);
    let k1: u32 = (0..COMMITS)
        .map(|i| commit_files(i).len().min(5) as u32)
        .sum();
    for set in ["SELh0", "SELs0"] {
        assert_eq!(row(&s, set, "k1").queries, k1, "{set}");
        assert_eq!(row(&s, set, "k1").test_commits, COMMITS, "{set}");
    }

    // Co-change on the co-changed files: an H10 candidate is SELh0's pick;
    // the hub draws PPR away from the cluster, so SELs0 picks B0.
    let h = row(&s, "SELh0", "k1");
    assert!(h.pick.starts_with("H10"), "{}", h.pick);
    assert_eq!(row(&s, "SELs0", "k1").pick, "B0");
    assert_eq!(row(&s, "SELs0", "k1").last_online_pick, "B0");
    assert!(
        row(&s, "SELs0", "k1").ci_low > 0.0,
        "{:?}",
        row(&s, "SELs0", "k1")
    );
    for set in ["SELh0", "SELs0"] {
        let r = row(&s, set, "k1");
        assert!(r.selection_recall10 >= r.fallback_recall10, "{set}: {r:?}");
        assert_eq!(r.delta_recall10, r.selection_recall10 - r.fallback_recall10);
        assert!(r.ci_low <= r.ci_high);
    }

    // The CI is the bootstrap of the record's per-commit scores, identical
    // across runs.
    for (sum, rec) in s.sets.iter().zip(&cal.sets) {
        assert_eq!(
            (sum.ci_low, sum.ci_high),
            bootstrap_ci(&rec.online.per_commit)
        );
    }
    let first = std::fs::read(calibration_dir(&root).join(CALIBRATION_FILE)).expect("record");
    let again = calibrate(&root, &r.merged, &opts).expect("calibrate again");
    let second = std::fs::read(calibration_dir(&root).join(CALIBRATION_FILE)).expect("record");
    assert_eq!(
        first, second,
        "calibration.json is byte-identical across runs"
    );
    let ci = |x: &CalibrationSummary| {
        x.sets
            .iter()
            .map(|r| (r.ci_low, r.ci_high))
            .collect::<Vec<_>>()
    };
    assert_eq!(ci(&s), ci(&again));

    // The in-memory step returns the record the file step wrote (compared as
    // the written bytes: a JSON read-back may move an f64's last bit).
    let snap = read_history(&root).expect("snapshot");
    let (mem, msum) = calibrate_snapshot(&r.merged, &root, &snap, None, &opts).expect("in memory");
    let mut mem_bytes = serde_json::to_vec_pretty(&mem).expect("serialise");
    mem_bytes.push(b'\n');
    assert_eq!(mem_bytes, second);
    assert_eq!(ci(&msum), ci(&s));

    // A row the selection reads is calibrated.
    let sel = select(
        &CalibrationRead::Loaded(cal.clone()),
        "SELs0",
        Shape::K1,
        &STRUCTURE_CANDIDATES,
        STRUCTURE_FALLBACK,
    );
    assert_eq!((sel.pick.as_str(), sel.source), ("B0", "calibrated"));
}

#[test]
fn no_test_commit_writes_zero_query_rows() {
    let (_tmp, root) = fixture();
    // One-file commits train but never test.
    let commits: Vec<HistoryCommit> = (0..5)
        .map(|i| HistoryCommit {
            c: format!("{:040x}", 100 + i),
            t: 1_700_000_000 + i as i64,
            files: vec![HistoryFile {
                p: cluster_path("zeta", i),
                a: Some(1),
                d: Some(0),
                from: None,
            }],
        })
        .collect();
    let meta = HistoryMeta::new(format!("{:040x}", 104), 5, None, String::new());
    write_history(&root, meta, &commits, &[]).expect("write history");
    let r = build(&root);
    let s = calibrate(&root, &r.merged, &CalibrateOptions::default()).expect("calibrate");
    assert_eq!(s.test_commits, 0);
    assert_eq!(s.sets.len(), 12);
    assert!(
        s.sets
            .iter()
            .all(|x| x.queries == 0 && (x.ci_low, x.ci_high) == (0.0, 0.0))
    );
    let read = read_calibration(&root);
    assert!(matches!(read, CalibrationRead::Loaded(_)));
    let sel = select(
        &read,
        "SELh0",
        Shape::K1,
        &COCHANGE_CANDIDATES,
        COCHANGE_FALLBACK,
    );
    assert_eq!((sel.source, sel.reason), ("fallback", Some("no_queries")));
}

#[test]
fn no_snapshot_names_history_sync() {
    let (_tmp, root) = fixture();
    let r = build(&root);
    let err = calibrate(&root, &r.merged, &CalibrateOptions::default()).expect_err("no snapshot");
    assert!(err.contains("glia history sync"), "{err}");
    assert!(!calibration_dir(&root).exists());
}

#[test]
fn a_merge_of_two_repos_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    write(&a, "x.py", "def x():\n    return 1\n");
    write(&b, "y.py", "def y():\n    return 2\n");
    history(&a);
    let roots = [a.display().to_string(), b.display().to_string()];
    let r = glia_engine::generate_many(&roots).expect("build");
    let err = calibrate(&a, &r.merged, &CalibrateOptions::default()).expect_err("multi repo");
    assert_eq!(err, "calibration is per repo: got 2 repos");
    assert!(!calibration_dir(&a).exists());
}

/// The files of commit `i` of the message history: even commits touch the
/// alpha cluster, odd ones the zeta cluster, 2..=6 neighbouring modules.
fn text_commit_files(i: usize) -> (&'static str, Vec<String>) {
    let c = if i.is_multiple_of(2) { "alpha" } else { "zeta" };
    let size = 2 + (i / 2) % 5;
    let start = (i * 5) % N;
    (c, (0..size).map(|j| cluster_path(c, (start + j) % N)).collect())
}

/// `COMMITS` commits alternating between the clusters, each subject naming
/// its cluster (EB.1's messages sidecar), written newest first.
fn text_history(root: &Path) {
    let mut commits = Vec::new();
    let mut messages = Vec::new();
    for i in 0..COMMITS {
        let (c, files) = text_commit_files(i);
        let sha = format!("{:040x}", i + 1);
        let t = 1_700_000_000 + i as i64 * 3600;
        commits.push(HistoryCommit {
            c: sha.clone(),
            t,
            files: files
                .into_iter()
                .map(|p| HistoryFile {
                    p,
                    a: Some(1),
                    d: Some(0),
                    from: None,
                })
                .collect(),
        });
        messages.push(HistoryMessage {
            c: sha,
            at: t,
            s: format!("{c} cluster work {i}"),
            b: String::new(),
            br: None,
        });
    }
    commits.reverse();
    messages.reverse();
    let meta = HistoryMeta::new(format!("{:040x}", COMMITS), COMMITS, None, String::new());
    write_history_with_messages(root, meta, &commits, &[], Some(&messages))
        .expect("write history");
}

/// EC.11: with commit messages, the review group's SELt rows are written
/// for every seeded shape; the text corpus makes a text fusion the k1 pick,
/// at least as good as the fallback LX[IDR,P1] (P1 in the replay: no router).
#[test]
fn review_text_selection_calibrates_on_messages() {
    let (_tmp, root) = fixture();
    text_history(&root);
    let r = build(&root);
    let mut opts = CalibrateOptions::default();
    opts.groups = vec!["review"];
    let s = calibrate(&root, &r.merged, &opts).expect("calibrate");
    assert!(s.messages);
    let rows: Vec<(&str, &str)> = s
        .sets
        .iter()
        .map(|x| (x.set.as_str(), x.shape.as_str()))
        .collect();
    assert_eq!(
        rows,
        [("SELt", "k1"), ("SELt", "k2"), ("SELt", "k3"), ("SELt", "half")]
    );
    let k1 = row(&s, "SELt", "k1");
    assert_eq!(k1.fallback, REVIEW_FALLBACK);
    assert!(k1.queries > 0);
    assert!(REVIEW_CANDIDATES.contains(&k1.pick.as_str()), "{k1:?}");
    assert_ne!(k1.pick, "H10[PT*]", "a text fusion is the k1 pick: {k1:?}");
    assert!(
        k1.selection_recall10 >= k1.fallback_recall10,
        "{k1:?}"
    );
    let CalibrationRead::Loaded(cal) = read_calibration(&root) else {
        panic!("record not loaded");
    };
    let sel = select(
        &CalibrationRead::Loaded(cal.clone()),
        "SELt",
        Shape::K1,
        &REVIEW_CANDIDATES,
        REVIEW_FALLBACK,
    );
    assert_eq!((sel.pick.as_str(), sel.source), (k1.pick.as_str(), "calibrated"));

    // Without the messages every T1s is 0: the replay still writes the rows.
    let snap = read_history(&root).expect("snapshot");
    let msgs = read_history_messages(&root, &snap).expect("messages");
    let (_, bare) = calibrate_snapshot(&r.merged, &root, &snap, None, &opts).expect("bare");
    assert!(!bare.messages);
    assert_eq!(bare.sets.len(), 4);
    let bare_k1 = bare.sets.iter().find(|x| x.shape == "k1").expect("bare k1");
    assert!(
        k1.selection_recall10 > bare_k1.selection_recall10,
        "the messages help: {k1:?} vs {bare_k1:?}"
    );
    // The in-memory step with the messages is the file step.
    let (_, again) =
        calibrate_snapshot(&r.merged, &root, &snap, Some(&msgs), &opts).expect("again");
    assert_eq!(
        again.sets.iter().map(|x| (&x.pick, x.selection_recall10)).collect::<Vec<_>>(),
        s.sets.iter().map(|x| (&x.pick, x.selection_recall10)).collect::<Vec<_>>()
    );
}
