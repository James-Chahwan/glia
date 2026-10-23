//! EC.8: calibrated selection in the text-free blast-radius family. With a
//! RankCache over the repo's root and `BlastOptions.select` (the default),
//! `radius` reads `.glia/calibration-snapshot/` once per cache and picks the
//! ranking of SELh0 (co-change first) or SELs0 (`cochange_first` off) for the
//! shape of the seeds' distinct files; an explicit ranking (`select = false`)
//! is never overridden, and with no record the rows are EA's.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};
use glia_engine::calibrate::record::{META_FILE, calibration_dir};
use glia_engine::calibrate::{CalibrateOptions, CalibrationRead, read_calibration, calibrate};
use glia_engine::diff_impact::diff_impact_from_diff;
use glia_engine::rank::{RankCache, Ranking};
use glia_engine::{BlastOptions, BlastRadius, GenerateResult};

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

/// EC.5's fixture (engine/tests/calibrate_step.rs): two clusters of `N`
/// modules around a diluted hub, so SELs0 calibrates to B0 and SELh0 to an
/// H10 candidate.
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
        write(root, &cluster_path("alpha", i), &format!("import hub\n\ndef alpha{i:02}_f():\n    return {i}\n"));
        write(
            root,
            &cluster_path("zeta", i),
            &format!("from hub import h, g{i:02}\n\ndef zeta{i:02}_f():\n    h()\n    return g{i:02}()\n"),
        );
    }
}

fn commit_files(i: usize) -> Vec<String> {
    let size = 2 + i % 5;
    let start = (i * 5) % N;
    (0..size).map(|j| cluster_path("zeta", (start + j) % N)).collect()
}

fn history(root: &Path) {
    let mut commits: Vec<HistoryCommit> = (0..COMMITS)
        .map(|i| HistoryCommit {
            c: format!("{:040x}", i + 1),
            t: 1_700_000_000 + i as i64 * 3600,
            files: commit_files(i)
                .into_iter()
                .map(|p| HistoryFile { p, a: Some(1), d: Some(0), from: None })
                .collect(),
        })
        .collect();
    commits.reverse();
    let meta = HistoryMeta::new(format!("{:040x}", COMMITS), COMMITS, None, String::new());
    write_history(root, meta, &commits, &[]).expect("write history");
}

fn fixture() -> (tempfile::TempDir, PathBuf, GenerateResult) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("clusters");
    repo(&root);
    history(&root);
    let r = glia_engine::generate_one(root.to_str().expect("utf-8")).expect("build");
    (tmp, root, r)
}

fn cache(r: &GenerateResult) -> Arc<RankCache> {
    Arc::new(RankCache::new("answer", &r.repo_roots, &r.repo_labels))
}

fn answer(r: &GenerateResult, cache: &Arc<RankCache>, seeds: &[&str], ranking: Ranking, select: bool) -> BlastRadius {
    let mut opts = BlastOptions::default();
    opts.ranking = ranking;
    opts.select = select;
    opts.rank_cache = Some(Arc::clone(cache));
    glia_engine::blast_radius(&r.merged, seeds, &opts)
}

fn rows(a: &BlastRadius) -> String {
    serde_json::to_string(&(&a.results, &a.files)).expect("serialise")
}

fn summary(a: &BlastRadius) -> (String, String, String, &'static str, Option<&'static str>) {
    let s = a.selection.as_ref().expect("a selection");
    (s.set.clone(), s.shape.clone(), s.pick.clone(), s.source, s.reason)
}

fn h(name: &str) -> Ranking {
    Ranking::from_harness_name(name).expect(name)
}

#[test]
fn no_record_falls_back_to_the_default_ranking() {
    let (_tmp, _root, r) = fixture();
    let c = cache(&r);
    let a = answer(&r, &c, &["zeta00_f"], Ranking::default(), true);
    assert_eq!(
        summary(&a),
        ("SELh0".into(), "k1".into(), "H10".into(), "fallback", Some("absent"))
    );
    assert_eq!(a.selection.as_ref().map(|s| s.test_commits), Some(0));
    let ea = answer(&r, &cache(&r), &["zeta00_f"], h("H10"), false);
    assert!(!a.results.is_empty());
    assert_eq!(rows(&a), rows(&ea), "no record: EA's answer");
    // A library caller with no cache answers as EA left it: explicit.
    let mut opts = BlastOptions::default();
    opts.rank_cache = None;
    let bare = glia_engine::blast_radius(&r.merged, &["zeta00_f"], &opts);
    assert_eq!(summary(&bare), ("SELh0".into(), "k1".into(), "H10".into(), "explicit", None));
}

#[test]
fn a_calibrated_record_picks_the_ranking() {
    let (_tmp, root, r) = fixture();
    calibrate(&root, &r.merged, &CalibrateOptions::default()).expect("calibrate");
    let CalibrationRead::Loaded(cal) = read_calibration(&root) else {
        panic!("record not loaded");
    };
    let pick_of = |set: &str, shape: &str| -> String {
        cal.sets.iter().find(|s| s.set == set && s.shape == shape).map(|s| s.pick.clone()).expect("row")
    };
    let c = cache(&r);

    // (b) co-change first: SELh0 at k1.
    let a = answer(&r, &c, &["zeta00_f"], Ranking::default(), true);
    let want = pick_of("SELh0", "k1");
    assert!(want.starts_with("H10"), "{want}");
    assert_eq!(summary(&a), ("SELh0".into(), "k1".into(), want.clone(), "calibrated", None));
    assert_eq!(a.selection.as_ref().map(|s| s.test_commits), Some(COMMITS));
    let explicit = answer(&r, &cache(&r), &["zeta00_f"], h(&want), false);
    assert_eq!(rows(&a), rows(&explicit), "rows ordered by the picked ranking");

    // (c) co-change off: SELs0, B0 on this fixture.
    let s = answer(&r, &c, &["zeta00_f"], Ranking::head(), true);
    assert_eq!(pick_of("SELs0", "k1"), "B0");
    assert_eq!(summary(&s), ("SELs0".into(), "k1".into(), "B0".into(), "calibrated", None));
    let b0 = answer(&r, &cache(&r), &["zeta00_f"], h("B0"), false);
    let p1 = answer(&r, &cache(&r), &["zeta00_f"], h("P1"), false);
    assert_eq!(rows(&s), rows(&b0), "rows ordered by carry depth then file degree");
    assert_ne!(rows(&s), rows(&p1), "the pick moved the order");

    // (d) seeds in two files: shape k2.
    let two = answer(&r, &c, &["zeta00_f", "zeta01_f"], Ranking::default(), true);
    assert_eq!(summary(&two).1, "k2");
    assert_eq!(summary(&two).2, pick_of("SELh0", "k2"));
    // Two seeds in ONE file are one seed file: k1.
    let one = answer(&r, &c, &["h", "r2"], Ranking::default(), true);
    assert_eq!(summary(&one).1, "k1", "both in hub.py");

    // (e) an explicit ranking is never overridden.
    let e = answer(&r, &c, &["zeta00_f"], h("H10[PT*]"), false);
    assert_eq!(summary(&e), ("SELh0".into(), "k1".into(), "H10[PT*]".into(), "explicit", None));
    let e = answer(&r, &c, &["zeta00_f"], h("PT*"), false);
    assert_eq!(summary(&e), ("SELs0".into(), "k1".into(), "PT*".into(), "explicit", None));

    // diff_impact reads the same record through the same radius.
    let diff = "--- a/zeta/zeta00.py\n+++ b/zeta/zeta00.py\n@@ -4,1 +4,1 @@\n-    h()\n+    h()\n";
    let mut opts = BlastOptions::default();
    opts.rank_cache = Some(Arc::clone(&c));
    let d = diff_impact_from_diff(&r.merged, diff, &opts);
    let sel = d.impact.selection.as_ref().expect("diff_impact selection");
    assert_eq!((sel.set.as_str(), sel.source), ("SELh0", "calibrated"));
}

#[test]
fn a_rewritten_build_stamp_falls_back() {
    let (_tmp, root, r) = fixture();
    calibrate(&root, &r.merged, &CalibrateOptions::default()).expect("calibrate");
    let meta = calibration_dir(&root).join(META_FILE);
    let text = std::fs::read_to_string(&meta).expect("meta");
    let mut v: serde_json::Value = serde_json::from_str(&text).expect("json");
    v["build_stamp"] = serde_json::Value::String("0000-not-this-glia".into());
    std::fs::write(&meta, serde_json::to_vec_pretty(&v).expect("json")).expect("write meta");
    let a = answer(&r, &cache(&r), &["zeta00_f"], Ranking::default(), true);
    assert_eq!(summary(&a), ("SELh0".into(), "k1".into(), "H10".into(), "fallback", Some("build_stamp")));
    let ea = answer(&r, &cache(&r), &["zeta00_f"], h("H10"), false);
    assert_eq!(rows(&a), rows(&ea));
}

#[test]
fn two_roots_are_multi_repo() {
    let (_tmp, root, r) = fixture();
    calibrate(&root, &r.merged, &CalibrateOptions::default()).expect("calibrate");
    let mut roots: BTreeMap<u64, String> = r.repo_roots.clone();
    roots.insert(u64::MAX, root.join("elsewhere").display().to_string());
    let c = Arc::new(RankCache::new("answer", &roots, &r.repo_labels));
    let a = answer(&r, &c, &["zeta00_f"], Ranking::head(), true);
    assert_eq!(summary(&a), ("SELs0".into(), "k1".into(), "P1".into(), "fallback", Some("multi_repo")));
}

#[test]
fn no_seed_has_no_selection() {
    let (_tmp, _root, r) = fixture();
    let a = answer(&r, &cache(&r), &["no_such_symbol_anywhere"], Ranking::default(), true);
    assert!(a.selection.is_none());
    let v = serde_json::to_value(&a).expect("json");
    assert!(v.get("selection").is_none(), "skipped when None");
}
