//! EC.8: calibrated selection in the text-free blast-radius family. With a
//! RankCache over the repo's root and `BlastOptions.select` (the default),
//! `radius` reads `.glia/calibration-snapshot/` once per cache and picks the
//! ranking of SELh0 (co-change first) or SELs0 (`cochange_first` off) for the
//! shape of the seeds' distinct files; an explicit ranking (`select = false`)
//! is never overridden, and with no record the rows are EA's.
//!
//! EC.12: review's intent selection reads set SELt2 (SELt's ten fusions plus
//! the four id-routed ones); a record written before EC.12 holds SELt rows
//! only, so review answers its fallback with reason `no_set` until the repo
//! is recalibrated.

mod git_fixture;

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

// EC.9: pack / pack_ids read the same record. The shape is the seeds'
// distinct located files; SELh0 / SELs0 by `PackArgs.ranking.cochange_first`;
// the pick orders the neighbours through EA.6's file lift (a B0 pick groups
// them by carry-BFS depth from the seed files, then file degree).

fn pack_args(c: &Arc<RankCache>, ranking: Ranking, select: bool) -> glia_engine::pack::PackArgs {
    let mut a = glia_engine::pack::PackArgs::default();
    a.budget_tokens = 1_000_000;
    a.ranking = ranking;
    a.select = select;
    a.rank_cache = Some(Arc::clone(c));
    a
}

fn packed(r: &GenerateResult, c: &Arc<RankCache>, seed: &str, ranking: Ranking, select: bool) -> glia_engine::pack::Pack {
    let id = r.merged.node_id_by_qname(seed).expect(seed);
    glia_engine::pack::pack_ids(&r.merged, &[id], &pack_args(c, ranking, select))
}

fn pack_summary(p: &glia_engine::pack::Pack) -> (String, String, String, &'static str, Option<&'static str>) {
    let s = p.selection.as_ref().expect("a pack selection");
    (s.set.clone(), s.shape.clone(), s.pick.clone(), s.source, s.reason)
}

/// The packed nodes in rank order, as (qname, rank): what the pick moves.
fn pack_rows(p: &glia_engine::pack::Pack) -> Vec<(String, usize)> {
    p.nodes.iter().map(|n| (n.qname.clone(), n.rank)).collect()
}

const ZETA00: &str = "zeta::zeta00::zeta00_f";

#[test]
fn pack_without_a_record_is_eas_pack() {
    let (_tmp, _root, r) = fixture();
    let c = cache(&r);
    let p = packed(&r, &c, ZETA00, Ranking::default(), true);
    assert_eq!(pack_summary(&p), ("SELh0".into(), "k1".into(), "H10".into(), "fallback", Some("absent")));
    let ea = packed(&r, &cache(&r), ZETA00, h("H10"), false);
    assert!(p.nodes.len() > 1);
    assert_eq!((pack_rows(&p), &p.text), (pack_rows(&ea), &ea.text), "no record: EA.6's pack");
    assert_eq!(pack_summary(&ea), ("SELh0".into(), "k1".into(), "H10".into(), "explicit", None));
    // No cache: explicit, as EA left it.
    let mut bare = glia_engine::pack::PackArgs::default();
    bare.budget_tokens = 1_000_000;
    let id = r.merged.node_id_by_qname(ZETA00).expect("zeta00");
    let b = glia_engine::pack::pack_ids(&r.merged, &[id], &bare);
    assert_eq!(pack_summary(&b).3, "explicit");
    // An empty pack has no selection, and the key is skipped.
    let none = glia_engine::pack::pack(&r.merged, "no_such_symbol_anywhere", &pack_args(&c, Ranking::default(), true));
    assert!(none.absence.is_some() && none.selection.is_none());
    let v = serde_json::to_value(&none).expect("json");
    assert!(v.get("selection").is_none(), "skipped when None");
}

#[test]
fn pack_applies_the_calibrated_pick() {
    use glia_activation::algo::{Walk, reach};
    let (_tmp, root, r) = fixture();
    calibrate(&root, &r.merged, &CalibrateOptions::default()).expect("calibrate");
    let CalibrationRead::Loaded(cal) = read_calibration(&root) else {
        panic!("record not loaded");
    };
    let pick_of = |set: &str, shape: &str| -> String {
        cal.sets.iter().find(|s| s.set == set && s.shape == shape).map(|s| s.pick.clone()).expect("row")
    };
    let c = cache(&r);

    // Co-change off: SELs0 at k1 picks B0 here.
    let s = packed(&r, &c, ZETA00, Ranking::head(), true);
    assert_eq!(pack_summary(&s), ("SELs0".into(), "k1".into(), "B0".into(), "calibrated", None));
    assert_eq!(s.selection.as_ref().map(|x| x.test_commits), Some(COMMITS));
    let b0 = packed(&r, &cache(&r), ZETA00, h("B0"), false);
    let p1 = packed(&r, &cache(&r), ZETA00, h("P1"), false);
    assert_eq!((pack_rows(&s), &s.text), (pack_rows(&b0), &b0.text), "neighbours in B0's order");
    assert_ne!(pack_rows(&s), pack_rows(&p1), "the pick moved the order");
    // The seed first, then the neighbours' candidate files by carry-BFS depth
    // from the seed file's nodes (non-decreasing down the pack).
    assert_eq!(s.nodes[0].qname, ZETA00);
    let files = c.files(&r.merged);
    let seed_file = files.file_of_node(r.merged.node_id_by_qname(ZETA00).expect("zeta00")).expect("seed file");
    let adj = c.carry(&r.merged);
    let walk = reach::bfs(&adj, files.nodes(seed_file), Walk::Both, usize::MAX);
    let mut depth: BTreeMap<u32, usize> = BTreeMap::new();
    for hit in &walk.reached {
        if let Some(f) = files.file_of_node(hit.id) {
            let d = depth.entry(f).or_insert(usize::MAX);
            *d = (*d).min(hit.depth);
        }
    }
    let seen: Vec<usize> = s
        .nodes
        .iter()
        .skip(1)
        .filter_map(|n| files.file_of_node(glia_core::NodeId(n.id)))
        .filter(|&f| f != seed_file)
        .map(|f| depth.get(&f).copied().unwrap_or(usize::MAX))
        .collect();
    assert!(seen.len() > 1, "{seen:?}");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "carry-depth order: {seen:?}");

    // Co-change on: SELh0.
    let a = packed(&r, &c, ZETA00, Ranking::default(), true);
    let want = pick_of("SELh0", "k1");
    assert_eq!(pack_summary(&a), ("SELh0".into(), "k1".into(), want.clone(), "calibrated", None));
    let explicit = packed(&r, &cache(&r), ZETA00, h(&want), false);
    assert_eq!((pack_rows(&a), &a.text), (pack_rows(&explicit), &explicit.text));

    // An explicit ranking is never overridden.
    let e = packed(&r, &c, ZETA00, h("PT*"), false);
    assert_eq!(pack_summary(&e), ("SELs0".into(), "k1".into(), "PT*".into(), "explicit", None));

    // Seeds in two files are shape k2.
    let args = pack_args(&c, Ranking::default(), true);
    let ids = [
        r.merged.node_id_by_qname(ZETA00).expect("zeta00"),
        r.merged.node_id_by_qname("zeta::zeta01::zeta01_f").expect("zeta01"),
    ];
    let k2 = glia_engine::pack::pack_ids(&r.merged, &ids, &args);
    assert_eq!(pack_summary(&k2).1, "k2");
    assert_eq!(pack_summary(&k2).2, pick_of("SELh0", "k2"));
}

/// EC.12: `review --title` selects from SELt2: with no record its fallback
/// (`absent`); calibrated, SELt2's pick (or its fallback when the replay
/// refused the set); a record whose review rows are pre-EC.12 SELt answers
/// the fallback with `no_set`.
#[test]
fn review_selects_from_selt2() {
    use glia_code_domain::snapshots::read_history;
    use glia_engine::calibrate::{CalibrationMeta, set_digest, write_calibration};
    use glia_engine::review::{ReviewArgs, review_vs_rev};

    let g = git_fixture::GitRepo::init();
    repo(g.root());
    g.commit("clean");
    history(g.root());
    let built = glia_engine::generate_one(g.path()).expect("build");
    // The PR: an edit, a rename and a deletion in the zeta cluster.
    g.write("zeta/zeta03.py", "from hub import h, g03\n\ndef zeta03_f():\n    h()\n    return 33\n");
    g.git_mv("zeta/zeta04.py", "zeta/zeta24.py");
    g.remove("zeta/zeta05.py");
    let mut args = ReviewArgs::default();
    args.title = Some("zeta cluster work".into());
    let sel = |r: &glia_engine::review::Review| {
        let s = r.selection_intent.as_ref().expect("selection_intent");
        (s.set.clone(), s.pick.clone(), s.source, s.reason)
    };

    let u = review_vs_rev(g.path(), "HEAD", &args).expect("review");
    assert_eq!(sel(&u), ("SELt2".into(), "LX[IDR,P1]".into(), "fallback", Some("absent")));
    let shape = u.selection_intent.as_ref().map(|s| s.shape.clone()).expect("shape");

    let mut opts = CalibrateOptions::default();
    opts.groups = vec!["review"];
    calibrate(g.root(), &built.merged, &opts).expect("calibrate");
    let CalibrationRead::Loaded(cal) = read_calibration(g.root()) else {
        panic!("record not loaded");
    };
    let k1 = cal.sets.iter().find(|r| r.set == "SELt2" && r.shape == shape).expect("SELt2 row");
    let c = review_vs_rev(g.path(), "HEAD", &args).expect("review");
    let (set, pick, source, reason) = sel(&c);
    assert_eq!((set.as_str(), pick.as_str()), ("SELt2", k1.pick.as_str()));
    if k1.candidates.contains(&k1.pick) {
        assert_eq!((source, reason), ("calibrated", None));
    } else {
        assert_eq!((pick.as_str(), source, reason), ("LX[IDR,P1]", "fallback", Some("refused")));
    }

    // A pre-EC.12 record: its review rows are set SELt over SELt's ten.
    let mut old_cal = cal.clone();
    for r in &mut old_cal.sets {
        r.set = "SELt".into();
        r.candidates.truncate(10);
        r.recall10.truncate(10);
        r.mrr.truncate(10);
        let names: Vec<&str> = r.candidates.iter().map(String::as_str).collect();
        r.digest = set_digest(&names);
    }
    let snap = read_history(g.root()).expect("snapshot");
    let meta = CalibrationMeta::new(
        snap.meta.head.clone(),
        snap.meta.data_hash.clone(),
        snap.meta.messages_hash.clone(),
    );
    write_calibration(g.root(), meta, &old_cal).expect("write");
    let CalibrationRead::Loaded(reread) = read_calibration(g.root()) else {
        panic!("old record not loaded");
    };
    assert!(reread.sets.iter().all(|r| r.set == "SELt" && r.candidates.len() == 10));
    let old = review_vs_rev(g.path(), "HEAD", &args).expect("review");
    assert_eq!(sel(&old), ("SELt2".into(), "LX[IDR,P1]".into(), "fallback", Some("no_set")));
}

/// Two clusters of `N` stand-alone modules (EC.13; calibrate_step's
/// `isolated`): a find query's PPR stays on its seed files.
fn isolated(root: &Path) {
    for c in ["alpha", "zeta"] {
        write(root, &format!("{c}/__init__.py"), "");
        for i in 0..N {
            write(root, &cluster_path(c, i), &format!("def {c}{i:02}_f():\n    return {i}\n"));
        }
    }
}

/// calibrate_step's `text_history`: commits alternating between the
/// clusters, 2..=6 neighbouring modules each, every subject naming only its
/// cluster.
fn cluster_history(root: &Path) {
    use glia_code_domain::snapshots::{HistoryMessage, write_history_with_messages};
    let mut commits = Vec::new();
    let mut messages = Vec::new();
    for i in 0..COMMITS {
        let c = if i.is_multiple_of(2) { "alpha" } else { "zeta" };
        let size = 2 + (i / 2) % 5;
        let start = (i * 5) % N;
        let sha = format!("{:040x}", i + 1);
        let t = 1_700_000_000 + i as i64 * 3600;
        commits.push(HistoryCommit {
            c: sha.clone(),
            t,
            files: (0..size)
                .map(|j| HistoryFile { p: cluster_path(c, (start + j) % N), a: Some(1), d: Some(0), from: None })
                .collect(),
        });
        messages.push(HistoryMessage { c: sha, at: t, s: format!("{c} cluster work {i}"), b: String::new(), br: None });
    }
    commits.reverse();
    messages.reverse();
    let meta = HistoryMeta::new(format!("{:040x}", COMMITS), COMMITS, None, String::new());
    write_history_with_messages(root, meta, &commits, &[], Some(&messages)).expect("write history");
}

/// EC.13: `find_files` selects from SELf2t on the find shape. With no record
/// it answers its fallback LX[IDR,T1s] (EB.8's order, `absent`), and
/// `select = false` is the same order as an explicit choice; calibrated, the
/// record's pick (H10x on this tree: the co-change of the text's top files),
/// its rows exactly `find_keys(pick)` over the head signals
/// (`find_signals_at_head`, the replay's arithmetic). A query matching
/// nothing is still EB.8's absence, carrying the selection.
#[test]
fn find_files_selects_from_self2t() {
    use std::collections::BTreeSet;

    use glia_code_domain::snapshots::{read_history, read_history_messages};
    use glia_engine::calibrate::{FindHead, find_keys, find_order, find_signals_at_head};
    use glia_engine::find::{FindFilesOptions, FoundFile, find_files};
    use glia_engine::intent::text::{Candidates, TextIndex, subject};

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("modules");
    isolated(&root);
    cluster_history(&root);
    let r = glia_engine::generate_one(root.to_str().expect("utf-8")).expect("build");
    let q = "alpha cluster work";
    let ask = |q: &str, select: bool| {
        let mut o = FindFilesOptions::default();
        o.select = select;
        o.rank_cache = Some(cache(&r));
        find_files(&r.merged, &r.repo_roots, q, &o).expect("answer")
    };
    let sel = |a: &glia_engine::absence::Answer<FoundFile>| {
        let s = a.selection.as_ref().expect("a selection");
        (s.set.clone(), s.shape.clone(), s.pick.clone(), s.source, s.reason, s.test_commits)
    };
    let files = |a: &glia_engine::absence::Answer<FoundFile>| -> Vec<String> {
        a.results.iter().map(|x| x.file.clone()).collect()
    };

    // uncalibrated: EB.8's LX[IDR,T1s] (no id here: text desc, degree desc, path)
    let u = ask(q, true);
    assert_eq!(sel(&u), ("SELf2t".into(), "find".into(), "LX[IDR,T1s]".into(), "fallback", Some("absent"), 0));
    assert!(!u.results.is_empty());
    let mut eb8 = u.results.clone();
    eb8.sort_by(|x, y| {
        y.idr
            .total_cmp(&x.idr)
            .then(y.text.total_cmp(&x.text))
            .then(y.degree.total_cmp(&x.degree))
            .then(x.file.cmp(&y.file))
    });
    assert_eq!(files(&u), eb8.iter().map(|x| x.file.clone()).collect::<Vec<_>>());
    assert!(u.results.iter().all(|x| x.text > 0.0));
    let e = ask(q, false);
    assert_eq!(sel(&e), ("SELf2t".into(), "find".into(), "LX[IDR,T1s]".into(), "explicit", None, 0));
    assert_eq!(serde_json::to_value(&e.results).ok(), serde_json::to_value(&u.results).ok());

    // calibrated
    let mut opts = CalibrateOptions::default();
    opts.groups = vec!["find"];
    calibrate(&root, &r.merged, &opts).expect("calibrate");
    let c = ask(q, true);
    assert_eq!(sel(&c), ("SELf2t".into(), "find".into(), "H10x".into(), "calibrated", None, COMMITS));
    let rc = cache(&r);
    let fi = rc.files(&r.merged);
    let repo = *r.repo_roots.keys().next().expect("repo");
    let cands = Candidates::of(&fi, repo);
    let t1s = TextIndex::load(&root, &cands, &BTreeSet::new()).expect("text").scores(subject(q));
    let snap = read_history(&root).expect("snapshot");
    let msgs = read_history_messages(&root, &snap).expect("messages");
    let head = FindHead::new(&r.merged, &rc, &cands, Some((&snap, &msgs)));
    let keys = find_keys("H10x", &find_signals_at_head(&head, "H10x", t1s, vec![0.0; cands.len()])).expect("keys");
    let want: Vec<String> = find_order(&keys).into_iter().take(20).map(|i| cands.paths[i].clone()).collect();
    assert_eq!(files(&c), want);
    assert_ne!(files(&c), files(&u), "the calibrated pick moves the order");
    assert!(c.results.iter().enumerate().all(|(i, x)| x.rank == i + 1));
    // select = false keeps EB.8's order on a calibrated tree
    assert_eq!(files(&ask(q, false)), files(&u));

    // nothing matches: no seed, every key ties to degree, no row is kept
    let none = ask("quagga zebra", true);
    assert!(none.results.is_empty());
    assert_eq!(none.absence.as_ref().map(|a| a.reason), Some("no_match"));
    assert_eq!(sel(&none).2, "H10x");
    let v = serde_json::to_value(&none).expect("json");
    assert_eq!(v["selection"]["set"], "SELf2t");
    assert_eq!(v["selection"]["source"], "calibrated");
}
