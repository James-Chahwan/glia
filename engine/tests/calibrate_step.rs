//! EC.5: the calibration step. `calibrate::calibrate` reads the history
//! snapshot, replays it over the layout's graph, writes
//! `.glia/calibration-snapshot/` and returns the online selection against the
//! fallback with a commit-cluster bootstrap CI.
//!
//! EC.12: review's set is SELt2 (SELt's ten fusions plus four id-routed ones);
//! the replay routes each test commit's subject at its PARENT revision
//! (`fusions::idr_at_parent`), so an id the commit itself defines routes
//! nowhere, and a tree with no git (the synthetic fixtures) routes nothing:
//! each LX[IDR, X] then scores exactly as X.
//!
//! EC.13: the default groups gain `find` (set SELf2t, 11 candidates, on the
//! find shape: each test commit's subject seeds the text's top files); the
//! record sorts it first. The replay's routes are quiet: one `[idr] replay
//! surface=calibrate ..` line per calibration replaces the per-commit
//! `[ids]` / `[id-defs]` / `[id-ns]` / `[id-paths]` / `[idr]` lines.

mod git_fixture;

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
    FIND_CANDIDATES, FIND_FALLBACK, REVIEW_CANDIDATES, REVIEW_FALLBACK, STRUCTURE_CANDIDATES,
    STRUCTURE_FALLBACK, SetSummary, Shape, bootstrap_ci, calibrate, calibrate_snapshot, select,
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
    // EC.11 / EC.13: the default groups are `structure`, `review`, `find`;
    // the record sorts its rows by set name (SELf2t first, its one shape).
    let mut want: Vec<(String, String)> = vec![("SELf2t".to_string(), "find".to_string())];
    want.extend(
        ["SELh0", "SELs0", "SELt2"]
            .iter()
            .flat_map(|set| ["k1", "k2", "k3", "half"].map(|sh| (set.to_string(), sh.to_string()))),
    );
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
    assert_eq!(s.sets.len(), 13);
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

/// EC.11 / EC.12: with commit messages, the review group's SELt2 rows (14
/// candidates) are written for every seeded shape; the text corpus makes a
/// text fusion the k1 pick, at least as good as the fallback LX[IDR,P1]. The
/// fixture is no git tree, so no parent is readable (`[calibrate] idr
/// unavailable commits=<every test commit>`): IDR is all zero and each
/// id-routed fusion's recall sums equal its base's.
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
        [("SELt2", "k1"), ("SELt2", "k2"), ("SELt2", "k3"), ("SELt2", "half")]
    );
    let k1 = row(&s, "SELt2", "k1");
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
    for r in &cal.sets {
        assert_eq!(r.candidates, REVIEW_CANDIDATES, "{} {}", r.set, r.shape);
        assert_eq!(r.recall10.len(), 14);
        let sum = |n: &str| {
            let i = r.candidates.iter().position(|c| c == n).unwrap_or_else(|| panic!("{n}"));
            r.recall10[i].to_bits()
        };
        for (a, b) in [
            ("LX[IDR,H18s[PT*]]", "H18s[PT*]"),
            ("LX[IDR,H9s25[PT*]]", "H9s25[PT*]"),
            ("H18si[PT*]", "H18s[PT*]"),
            ("LX[IDR,C0,PT*]", "H10[PT*]"),
        ] {
            assert_eq!(sum(a), sum(b), "{} {}: {a} vs {b}", r.set, r.shape);
        }
    }
    let sel = select(
        &CalibrationRead::Loaded(cal.clone()),
        "SELt2",
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

/// EC.12: the replay's IDR is read at each test commit's PARENT revision. A
/// git tree whose spec-kit task list defines T012 (pending, naming
/// `pkg/b.py`) before the commit that does T012 (marking it done and adding
/// T020): routed at that commit's parent, T012 leads with `pkg/b.py` under
/// LX[IDR,H18s[PT*]]; T020, which only the commit itself defines, routes
/// nowhere (no leak); the root commit has no parent. The working tree has
/// since deleted the task list, which changes nothing. The calibration over
/// the same tree writes 14-candidate SELt2 rows.
#[test]
fn idr_routes_each_test_commit_at_its_parent() {
    use glia_engine::calibrate::fusions::{Signals, idr_at_parent};
    use glia_engine::calibrate::{seeded_keys, seeded_order};
    use glia_engine::intent::text::Candidates;
    use glia_engine::rank::FileIndex;

    let g = git_fixture::GitRepo::init();
    g.write("pkg/a.py", "from pkg.b import bill\nfrom pkg.c import other\n\ndef run():\n    other()\n    return bill()\n");
    g.write("pkg/b.py", "def bill():\n    return 1\n");
    g.write("pkg/c.py", "def other():\n    return 2\n");
    g.write("pkg/d.py", "from pkg.c import other\n\ndef misc():\n    return other()\n");
    g.write("specs/001-billing/tasks.md", "# Tasks\n\n- [ ] T012 billing export in pkg/b.py\n");
    let first = g.commit("initial skeleton");
    g.write("pkg/b.py", "def bill():\n    return 3\n");
    g.write("pkg/a.py", "from pkg.b import bill\nfrom pkg.c import other\n\ndef run():\n    other()\n    return bill() + 1\n");
    g.write(
        "specs/001-billing/tasks.md",
        "# Tasks\n\n- [x] T012 billing export in pkg/b.py\n- [ ] T020 new work in pkg/d.py\n",
    );
    let second = g.commit("T012 billing export");
    // The working tree moves on: the task list is gone and a module renamed.
    // The router reads the parent's tree, never the working tree's.
    g.remove("specs/001-billing/tasks.md");
    g.git_mv("pkg/d.py", "pkg/e.py");

    let built = glia_engine::generate_one(g.path()).expect("build");
    let repo = built.merged.graphs[0].repo.0;
    let labels = std::collections::BTreeMap::from([(repo, "repo".to_string())]);
    let cands = Candidates::of(&FileIndex::of(&built.merged, &labels), repo);
    let pos = |p: &str| cands.position(p).unwrap_or_else(|| panic!("{p} in {:?}", cands.paths));
    let (a, b) = (pos("pkg/a.py"), pos("pkg/b.py"));

    let idr = idr_at_parent(g.root(), &built.merged, &cands, &second, "T012 billing export", &[])
        .expect("parent");
    assert_eq!(idr.len(), cands.len());
    assert_eq!(idr[b], 1.0, "pending at the parent: {idr:?}");
    assert_eq!(idr.iter().filter(|&&x| x > 0.0).count(), 1, "{idr:?}");

    // LX[IDR,H18s[PT*]] from the changed pkg/a.py: the routed file leads.
    let n = cands.len();
    let mut excluded = vec![false; n];
    excluded[a] = true;
    let mut s = Signals::zeros(n);
    s.pt = (0..n).map(|i| if i == b { 0.1 } else { 0.5 }).collect();
    s.idr = Some(idr);
    let tie: Vec<u32> = (0..n as u32).collect();
    let keys = seeded_keys("LX[IDR,H18s[PT*]]", &s, &excluded).expect("keys");
    assert_eq!(seeded_order(&keys, &tie, &excluded)[0], b);
    let base = seeded_keys("H18s[PT*]", &s, &excluded).expect("keys");
    assert_ne!(seeded_order(&base, &tie, &excluded)[0], b, "without IDR another file leads");

    // T020 exists only in the commit itself: nothing at its parent.
    let leak = idr_at_parent(g.root(), &built.merged, &cands, &second, "T020 new work", &[])
        .expect("parent");
    assert!(leak.iter().all(|&x| x == 0.0), "{leak:?}");
    // no id, no scope: nothing
    let none = idr_at_parent(g.root(), &built.merged, &cands, &second, "billing export", &[])
        .expect("parent");
    assert!(none.iter().all(|&x| x == 0.0));
    // the root commit has no parent
    assert!(idr_at_parent(g.root(), &built.merged, &cands, &first, "T012 billing export", &[]).is_err());

    // The calibration over this tree's real history.
    let files = |ps: &[&str]| -> Vec<HistoryFile> {
        ps.iter()
            .map(|p| HistoryFile { p: p.to_string(), a: Some(1), d: Some(0), from: None })
            .collect()
    };
    let commits = vec![
        HistoryCommit {
            c: second.clone(),
            t: 1_700_000_100,
            files: files(&["pkg/a.py", "pkg/b.py", "specs/001-billing/tasks.md"]),
        },
        HistoryCommit {
            c: first.clone(),
            t: 1_700_000_000,
            files: files(&["pkg/a.py", "pkg/b.py", "pkg/c.py", "pkg/d.py", "specs/001-billing/tasks.md"]),
        },
    ];
    let msg = |c: &str, at: i64, s: &str| HistoryMessage { c: c.to_string(), at, s: s.to_string(), b: String::new(), br: None };
    let messages = vec![
        msg(&second, 1_700_000_100, "T012 billing export"),
        msg(&first, 1_700_000_000, "initial skeleton"),
    ];
    let meta = HistoryMeta::new(second.clone(), 2, None, String::new());
    write_history_with_messages(g.root(), meta, &commits, &[], Some(&messages)).expect("history");
    let mut opts = CalibrateOptions::default();
    opts.groups = vec!["review"];
    let summary = calibrate(g.root(), &built.merged, &opts).expect("calibrate");
    assert!(summary.sets.iter().all(|r| r.set == "SELt2"), "{summary:?}");
    let CalibrationRead::Loaded(cal) = read_calibration(g.root()) else {
        panic!("record not loaded");
    };
    assert!(!cal.sets.is_empty());
    assert!(cal.sets.iter().all(|r| r.candidates == REVIEW_CANDIDATES));
}


/// Two clusters of `N` stand-alone modules (no import, no call): a find
/// query's PPR stays on its seed files, so only the text and the co-change
/// of its top files rank.
fn isolated(root: &Path) {
    for c in ["alpha", "zeta"] {
        write(root, &format!("{c}/__init__.py"), "");
        for i in 0..N {
            write(root, &cluster_path(c, i), &format!("def {c}{i:02}_f():\n    return {i}\n"));
        }
    }
}

/// EC.13: with commit messages, the `find` group writes one SELf2t row on the
/// find shape (each test commit's subject is the query; its text's top files
/// seed PX1 / PXT / C0x), over the 11 candidates in rank_eval's order. The
/// subjects name only their cluster ([`text_history`]), so the text alone
/// ranks a cluster's busiest files; the co-change of its top files (H10x)
/// finds the window and is the calibrated pick, above the fallback
/// LX[IDR,T1s] (scored only). The pick is what `select` reads for
/// find_files.
#[test]
fn find_selection_calibrates_on_messages() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("modules");
    isolated(&root);
    text_history(&root);
    let r = build(&root);
    let mut opts = CalibrateOptions::default();
    opts.groups = vec!["find"];
    let s = calibrate(&root, &r.merged, &opts).expect("calibrate");
    let rows: Vec<(&str, &str)> = s.sets.iter().map(|x| (x.set.as_str(), x.shape.as_str())).collect();
    assert_eq!(rows, [("SELf2t", "find")]);
    let f = row(&s, "SELf2t", "find");
    assert_eq!(f.fallback, FIND_FALLBACK);
    assert_eq!(f.test_commits, COMMITS);
    assert_eq!(f.queries as usize, COMMITS, "one find query per test commit");
    assert!(FIND_CANDIDATES.contains(&f.pick.as_str()), "{f:?}");
    assert_eq!(f.pick, "H10x", "co-change from the text's top files: {f:?}");
    assert!(f.selection_recall10 >= f.fallback_recall10, "{f:?}");
    assert!(f.selection_recall10 > 0.0, "{f:?}");
    let CalibrationRead::Loaded(cal) = read_calibration(&root) else {
        panic!("record not loaded");
    };
    assert_eq!(cal.sets.len(), 1);
    let rec = &cal.sets[0];
    assert_eq!(rec.candidates, FIND_CANDIDATES);
    assert_eq!(rec.recall10.len(), 11);
    // no git: IDR routes nothing, so each LX[IDR, X] scores exactly as X
    let sum = |n: &str| {
        let i = rec.candidates.iter().position(|c| c == n).unwrap_or_else(|| panic!("{n}"));
        rec.recall10[i].to_bits()
    };
    assert_eq!(sum("LX[IDR,H18x]"), sum("H18x"));
    assert_eq!(sum("LX[IDR,H9x25T]"), sum("H9x25T"));
    assert_eq!(sum("LX[IDR,C0x,PXT]"), sum("H10x"));
    let sel = select(
        &CalibrationRead::Loaded(cal.clone()),
        "SELf2t",
        Shape::Find,
        &FIND_CANDIDATES,
        FIND_FALLBACK,
    );
    assert_eq!((sel.pick.as_str(), sel.source), (f.pick.as_str(), "calibrated"));

    // Without the messages there is no text, hence no seed: every candidate
    // ranks by degree and the rows are still written.
    let snap = read_history(&root).expect("snapshot");
    let (_, bare) = calibrate_snapshot(&r.merged, &root, &snap, None, &opts).expect("bare");
    assert_eq!(bare.sets.len(), 1);
    assert!(f.selection_recall10 > bare.sets[0].selection_recall10, "{f:?} vs {:?}", bare.sets[0]);
}

const CHILD_ENV: &str = "GLIA_TEST_CALIBRATE_IDR_CHILD";

/// The git tree of `idr_routes_each_test_commit_at_its_parent` with its
/// snapshot: two commits, the second's subject naming T012.
fn idr_tree() -> (git_fixture::GitRepo, String, String) {
    let g = git_fixture::GitRepo::init();
    g.write("pkg/a.py", "from pkg.b import bill\n\ndef run():\n    return bill()\n");
    g.write("pkg/b.py", "def bill():\n    return 1\n");
    g.write("pkg/c.py", "def other():\n    return 2\n");
    g.write("specs/001-billing/tasks.md", "# Tasks\n\n- [ ] T012 billing export in pkg/b.py\n");
    let first = g.commit("initial skeleton");
    g.write("pkg/b.py", "def bill():\n    return 3\n");
    g.write("pkg/a.py", "from pkg.b import bill\n\ndef run():\n    return bill() + 1\n");
    let second = g.commit("T012 billing export");
    (g, first, second)
}

/// Child half of `replay_routes_print_one_aggregate_idr_line`: calibrates
/// [`CHILD_ENV`]'s tree over `review` and `find`; a no-op in a normal run.
#[test]
fn child_calibrate_idr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        let root = PathBuf::from(dir);
        let built = glia_engine::generate_one(root.to_str().expect("utf-8")).expect("build");
        let mut opts = CalibrateOptions::default();
        opts.groups = vec!["review", "find"];
        calibrate(&root, &built.merged, &opts).expect("calibrate");
    }
}

/// EC.13 (routed from EC.12): the replay routes each test commit's subject
/// through a quiet side, so no per-commit `[idr] repo=..`, `[ids] vocab`,
/// `[id-defs]`, `[id-ns]` or `[id-paths]` line prints; one `[idr] replay
/// surface=calibrate ..` line does, after the replay. Here the newer commit's
/// T012 resolves at its parent and the root commit has no parent.
#[test]
fn replay_routes_print_one_aggregate_idr_line() {
    let (g, first, second) = idr_tree();
    let files = |ps: &[&str]| -> Vec<HistoryFile> {
        ps.iter()
            .map(|p| HistoryFile { p: p.to_string(), a: Some(1), d: Some(0), from: None })
            .collect()
    };
    let commits = vec![
        HistoryCommit { c: second.clone(), t: 1_700_000_100, files: files(&["pkg/a.py", "pkg/b.py"]) },
        HistoryCommit { c: first.clone(), t: 1_700_000_000, files: files(&["pkg/a.py", "pkg/b.py", "pkg/c.py"]) },
    ];
    let msg = |c: &str, at: i64, s: &str| HistoryMessage { c: c.to_string(), at, s: s.to_string(), b: String::new(), br: None };
    let messages = vec![
        msg(&second, 1_700_000_100, "T012 billing export"),
        msg(&first, 1_700_000_000, "initial skeleton"),
    ];
    let meta = HistoryMeta::new(second.clone(), 2, None, String::new());
    write_history_with_messages(g.root(), meta, &commits, &[], Some(&messages)).expect("history");

    let exe = std::env::current_exe().expect("test binary path");
    let out = std::process::Command::new(exe)
        .args(["--exact", "child_calibrate_idr", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, g.root())
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child failed: {stderr}");
    let lines = |p: &str| stderr.lines().filter(|l| l.starts_with(p)).collect::<Vec<_>>();
    let agg = lines("[idr] replay ");
    assert_eq!(agg.len(), 1, "one aggregate line: {stderr}");
    eprintln!("{}", agg[0]);
    assert!(
        agg[0].starts_with("[idr] replay surface=calibrate sides=1 routed=1 resolved=1 unavailable=1 ms="),
        "{}",
        agg[0]
    );
    for quiet in ["[idr] repo=", "[ids] vocab", "[id-defs]", "[id-ns]", "[id-paths]"] {
        assert!(lines(quiet).is_empty(), "{quiet} printed by the replay: {stderr}");
    }
    let find = lines("[calibrate] set=SELf2t shape=find candidates=11 ");
    assert_eq!(find.len(), 1, "{stderr}");
    eprintln!("{}", find[0]);
    assert_eq!(lines("[calibrate] idr unavailable commits=1").len(), 1, "{stderr}");
    // EG.7 (routed from EC.13): the calibration's rank cache prints no
    // per-lookup line, one summary after the replay.
    assert!(lines("[rank-cache] holder=calibrate source=").is_empty(), "per-lookup line: {stderr}");
    let summary = lines("[rank-cache] holder=calibrate summary lookups=");
    assert_eq!(summary.len(), 1, "one calibrate summary: {stderr}");
    eprintln!("{}", summary[0]);
    assert!(summary[0].contains(" built=") && summary[0].contains(" hits=") && summary[0].contains(" build_ms="), "{}", summary[0]);
}

const VOCAB_CHILD_ENV: &str = "GLIA_TEST_CALIBRATE_VOCAB_CHILD";

/// The tree of `an_override_vocabulary_routes_ids_the_root_does_not_declare`:
/// `idr_tree`'s, its task named by an id (`PX7`) no default `[ids]` shape
/// matches, and no `.glia/overlay.toml` at the root.
fn vocab_tree() -> (git_fixture::GitRepo, String, String) {
    let g = git_fixture::GitRepo::init();
    g.write("pkg/a.py", "from pkg.b import bill\n\ndef run():\n    return bill()\n");
    g.write("pkg/b.py", "def bill():\n    return 1\n");
    g.write("pkg/c.py", "def other():\n    return 2\n");
    g.write("specs/001-billing/tasks.md", "# Tasks\n\n- [ ] PX7 billing export in pkg/b.py\n");
    let first = g.commit("initial skeleton");
    g.write("pkg/b.py", "def bill():\n    return 3\n");
    g.write("pkg/a.py", "from pkg.b import bill\n\ndef run():\n    return bill() + 1\n");
    let second = g.commit("PX7 billing export");
    (g, first, second)
}

/// Child half of `an_override_vocabulary_routes_ids_the_root_does_not_declare`:
/// calibrates [`VOCAB_CHILD_ENV`]'s tree over `review`, with the override
/// vocabulary `PX{N}` when `<env>_OVERRIDE` is set; a no-op in a normal run.
#[test]
fn child_calibrate_vocab() {
    if let Ok(dir) = std::env::var(VOCAB_CHILD_ENV) {
        let root = PathBuf::from(dir);
        let built = glia_engine::generate_one(root.to_str().expect("utf-8")).expect("build");
        let mut opts = CalibrateOptions::default();
        opts.groups = vec!["review"];
        if std::env::var_os(format!("{VOCAB_CHILD_ENV}_OVERRIDE")).is_some() {
            opts.vocab = Some(
                glia_engine::intent::ids::IdVocab::from_patterns(&["PX{N}"]).expect("shape"),
            );
        }
        calibrate(&root, &built.merged, &opts).expect("calibrate");
    }
}

/// EC.14: `CalibrateOptions.vocab` is the replay's `[ids]` vocabulary. A root
/// with no overlay reads the defaults, which do not match `PX7`, so nothing
/// resolves; the same run handed a vocabulary naming the shape routes the
/// subject at its parent, and says so once: `[ids] vocab .. surface=calibrate
/// .. source=override`.
#[test]
fn an_override_vocabulary_routes_ids_the_root_does_not_declare() {
    let (g, first, second) = vocab_tree();
    assert!(!g.root().join(".glia/overlay.toml").exists());
    let files = |ps: &[&str]| -> Vec<HistoryFile> {
        ps.iter()
            .map(|p| HistoryFile { p: p.to_string(), a: Some(1), d: Some(0), from: None })
            .collect()
    };
    let commits = vec![
        HistoryCommit { c: second.clone(), t: 1_700_000_100, files: files(&["pkg/a.py", "pkg/b.py"]) },
        HistoryCommit { c: first.clone(), t: 1_700_000_000, files: files(&["pkg/a.py", "pkg/b.py", "pkg/c.py"]) },
    ];
    let msg = |c: &str, at: i64, s: &str| HistoryMessage { c: c.to_string(), at, s: s.to_string(), b: String::new(), br: None };
    let messages = vec![
        msg(&second, 1_700_000_100, "PX7 billing export"),
        msg(&first, 1_700_000_000, "initial skeleton"),
    ];
    let meta = HistoryMeta::new(second.clone(), 2, None, String::new());
    write_history_with_messages(g.root(), meta, &commits, &[], Some(&messages)).expect("history");

    let run = |overlay: bool| -> String {
        let exe = std::env::current_exe().expect("test binary path");
        let mut cmd = std::process::Command::new(exe);
        cmd.args(["--exact", "child_calibrate_vocab", "--nocapture", "--test-threads=1"])
            .env(VOCAB_CHILD_ENV, g.root());
        if overlay {
            cmd.env(format!("{VOCAB_CHILD_ENV}_OVERRIDE"), "1");
        }
        let out = cmd.output().expect("re-run the test binary");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "child failed: {stderr}");
        stderr
    };
    let lines = |s: &str, p: &str| s.lines().filter(|l| l.starts_with(p)).map(str::to_string).collect::<Vec<_>>();

    let plain = run(false);
    let agg = lines(&plain, "[idr] replay ");
    assert_eq!(agg.len(), 1, "{plain}");
    eprintln!("without: {}", agg[0]);
    assert!(agg[0].contains(" resolved=0 "), "{}", agg[0]);
    assert!(lines(&plain, "[ids] vocab").is_empty(), "no override, no vocab line: {plain}");

    let over = run(true);
    let agg = lines(&over, "[idr] replay ");
    assert_eq!(agg.len(), 1, "{over}");
    eprintln!("with: {}", agg[0]);
    assert!(
        agg[0].starts_with("[idr] replay surface=calibrate sides=1 routed=1 resolved=1 unavailable=1 ms="),
        "{}",
        agg[0]
    );
    let vocab = lines(&over, "[ids] vocab ");
    assert_eq!(vocab.len(), 1, "one override line: {over}");
    eprintln!("{}", vocab[0]);
    assert!(vocab[0].contains(" surface=calibrate patterns=1 source=override trackers=0 rejected=0 markdown=0"), "{}", vocab[0]);
}
