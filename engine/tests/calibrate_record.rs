//! EC.1: the persisted calibration record (`.glia/calibration-snapshot/`) and
//! the one selection reader every ranked answer calls.

use std::collections::BTreeMap;
use std::path::Path;

use glia_engine::calibrate::record::{
    CALIBRATION_DIR, CALIBRATION_FILE, CALIBRATION_VERSION, CalibratedSet, Calibration,
    CalibrationHandle, CalibrationMeta, CalibrationRead, CommitScore, META_FILE, OnlineReplay,
    RANKER_REVISION, ReplayParamsRecord, Shape, calibration_dir, explicit, load_for_roots,
    log_selection, read_calibration, select, set_digest, write_calibration,
};

const CANDS: [&str; 3] = ["ppr", "cochange", "lex2"];

fn score(sha: &str, pick: &str, q: u32) -> CommitScore {
    let mut c = CommitScore::default();
    c.sha = sha.to_string();
    c.pick = pick.to_string();
    c.queries = q;
    c.sel_r10 = 0.5;
    c.fb_r10 = 0.25;
    c.sel_mrr = 0.75;
    c.fb_mrr = 0.125;
    c
}

fn row(set: &str, shape: Shape, queries: u32, pick: &str) -> CalibratedSet {
    let mut r = CalibratedSet::default();
    r.set = set.to_string();
    r.shape = shape.as_str().to_string();
    r.candidates = CANDS.iter().map(|s| s.to_string()).collect();
    r.digest = set_digest(&CANDS);
    r.fallback = "ppr".to_string();
    r.test_commits = if queries == 0 { 0 } else { 2 };
    r.queries = queries;
    r.recall10 = vec![1.5, 2.0, 0.5];
    r.mrr = vec![0.75, 1.0, 0.25];
    r.fallback_recall10 = 1.5;
    r.fallback_mrr = 0.75;
    r.pick = pick.to_string();
    r.last_online_pick = "ppr".to_string();
    let mut online = OnlineReplay::default();
    online.selection_recall10 = 0.5;
    online.fallback_recall10 = 0.25;
    online.selection_mrr = 0.75;
    online.fallback_mrr = 0.125;
    online.picks = BTreeMap::from([("ppr".to_string(), 1), ("cochange".to_string(), 1)]);
    online.per_commit = vec![score("a1", "ppr", 1), score("b2", "cochange", 2)];
    r.online = online;
    r
}

fn sample() -> Calibration {
    let mut replay = ReplayParamsRecord::default();
    replay.max_commits = 400;
    replay.max_seeds = 3;
    replay.max_files = 40;
    replay.commits = 120;
    replay.train_commits = 60;
    replay.test_commits = 60;
    let mut cal = Calibration::default();
    cal.replay = replay;
    // Written out of order: write_calibration sorts by (set, shape order).
    cal.sets = vec![
        row("impact", Shape::Half, 3, "cochange"),
        row("impact", Shape::K1, 3, "cochange"),
        row("empty", Shape::K2, 0, "ppr"),
        row("impact", Shape::Find, 3, "lex2"),
    ];
    cal
}

fn meta() -> CalibrationMeta {
    CalibrationMeta::new(
        "0123456789abcdef0123456789abcdef01234567".to_string(),
        "feedface".to_string(),
        None,
    )
}

fn written(root: &Path) -> Calibration {
    write_calibration(root, meta(), &sample()).expect("write");
    match read_calibration(root) {
        CalibrationRead::Loaded(c) => *c,
        other => panic!("expected Loaded, got {other:?}"),
    }
}

fn rewrite_meta(root: &Path, f: impl FnOnce(&mut serde_json::Value)) {
    let p = calibration_dir(root).join(META_FILE);
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    f(&mut v);
    std::fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
}

fn rejected(root: &Path) -> &'static str {
    match read_calibration(root) {
        CalibrationRead::Rejected(r) => r,
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[test]
fn layout_and_constants() {
    let root = Path::new("/r");
    assert_eq!(
        calibration_dir(root),
        root.join(".glia").join(CALIBRATION_DIR)
    );
    assert_eq!(CALIBRATION_DIR, "calibration-snapshot");
    assert_eq!(CALIBRATION_FILE, "calibration.json");
    assert_eq!(META_FILE, "meta.json");
    assert_eq!(CALIBRATION_VERSION, 1);
    assert_eq!(RANKER_REVISION, 1);
    let m = meta();
    assert_eq!(m.build_stamp, glia_stamp::BUILD_STAMP);
    assert_eq!(m.generator, "glia history calibrate");
}

#[test]
fn round_trip_is_loaded_equal_and_sorted() {
    let tmp = tempfile::tempdir().unwrap();
    let got = written(tmp.path());
    let mut want = sample();
    want.sets.sort_by(|a, b| {
        (a.set.as_str(), Shape::parse(&a.shape).map(Shape::order))
            .cmp(&(b.set.as_str(), Shape::parse(&b.shape).map(Shape::order)))
    });
    assert_eq!(got, want);
    let order: Vec<(&str, &str)> = got
        .sets
        .iter()
        .map(|s| (s.set.as_str(), s.shape.as_str()))
        .collect();
    assert_eq!(
        order,
        vec![
            ("empty", "k2"),
            ("impact", "k1"),
            ("impact", "half"),
            ("impact", "find")
        ]
    );
    let m: CalibrationMeta = serde_json::from_slice(
        &std::fs::read(calibration_dir(tmp.path()).join(META_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(m.sets, 4);
    assert_eq!(m.data_hash.len(), 16);
}

#[test]
fn two_writes_are_byte_identical() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    write_calibration(a.path(), meta(), &sample()).unwrap();
    write_calibration(b.path(), meta(), &sample()).unwrap();
    for f in [CALIBRATION_FILE, META_FILE] {
        let x = std::fs::read(calibration_dir(a.path()).join(f)).unwrap();
        let y = std::fs::read(calibration_dir(b.path()).join(f)).unwrap();
        assert_eq!(x, y, "{f}");
        assert_eq!(x.last(), Some(&b'\n'), "{f}");
    }
    // A rewrite over an existing record is the same bytes too.
    write_calibration(a.path(), meta(), &sample()).unwrap();
    assert_eq!(
        std::fs::read(calibration_dir(a.path()).join(CALIBRATION_FILE)).unwrap(),
        std::fs::read(calibration_dir(b.path()).join(CALIBRATION_FILE)).unwrap()
    );
}

#[test]
fn absent_and_rejections() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(read_calibration(tmp.path()), CalibrationRead::Absent);

    // meta removed
    written(tmp.path());
    std::fs::remove_file(calibration_dir(tmp.path()).join(META_FILE)).unwrap();
    assert_eq!(rejected(tmp.path()), "incomplete");

    // calibration.json edited by one byte
    written(tmp.path());
    let p = calibration_dir(tmp.path()).join(CALIBRATION_FILE);
    let mut bytes = std::fs::read(&p).unwrap();
    let i = bytes.iter().position(|&c| c == b'3').unwrap();
    bytes[i] = b'4';
    std::fs::write(&p, bytes).unwrap();
    assert_eq!(rejected(tmp.path()), "incomplete");

    // calibration.json missing
    written(tmp.path());
    std::fs::remove_file(calibration_dir(tmp.path()).join(CALIBRATION_FILE)).unwrap();
    assert_eq!(rejected(tmp.path()), "incomplete");

    // sets count disagrees
    written(tmp.path());
    rewrite_meta(tmp.path(), |v| v["sets"] = serde_json::json!(9));
    assert_eq!(rejected(tmp.path()), "incomplete");

    // unparsable meta
    written(tmp.path());
    std::fs::write(calibration_dir(tmp.path()).join(META_FILE), b"{not json").unwrap();
    assert_eq!(rejected(tmp.path()), "incomplete");

    written(tmp.path());
    rewrite_meta(tmp.path(), |v| {
        v["build_stamp"] = serde_json::json!("0.0.0+pdeadbeef")
    });
    assert_eq!(rejected(tmp.path()), "build_stamp");

    written(tmp.path());
    rewrite_meta(tmp.path(), |v| v["version"] = serde_json::json!(2));
    assert_eq!(rejected(tmp.path()), "version");

    written(tmp.path());
    rewrite_meta(tmp.path(), |v| v["ranker_revision"] = serde_json::json!(0));
    assert_eq!(rejected(tmp.path()), "ranker_revision");
}

#[test]
fn select_reasons() {
    let tmp = tempfile::tempdir().unwrap();
    let loaded = CalibrationRead::Loaded(Box::new(written(tmp.path())));
    let fb = |r: &CalibrationRead, set: &str, shape: Shape, cands: &[&str]| {
        select(r, set, shape, cands, "ppr")
    };

    let s = fb(&CalibrationRead::NotLoaded, "impact", Shape::K1, &CANDS);
    assert_eq!(
        (s.pick.as_str(), s.source, s.reason),
        ("ppr", "fallback", Some("not_loaded"))
    );
    assert_eq!(
        fb(&CalibrationRead::Absent, "impact", Shape::K1, &CANDS).reason,
        Some("absent")
    );
    assert_eq!(
        fb(
            &CalibrationRead::Rejected("build_stamp"),
            "impact",
            Shape::K1,
            &CANDS
        )
        .reason,
        Some("build_stamp")
    );
    assert_eq!(
        fb(&loaded, "impact", Shape::K3, &CANDS).reason,
        Some("no_set")
    );
    assert_eq!(
        fb(&loaded, "nope", Shape::K1, &CANDS).reason,
        Some("no_set")
    );
    // candidate set by digest: a reordered or renamed list
    let s = fb(&loaded, "impact", Shape::K1, &["cochange", "ppr", "lex2"]);
    assert_eq!(
        (s.pick.as_str(), s.source, s.reason),
        ("ppr", "fallback", Some("candidate_set"))
    );
    assert_eq!(
        fb(&loaded, "impact", Shape::K1, &["ppr", "cochange"]).reason,
        Some("candidate_set")
    );
    assert_eq!(
        fb(&loaded, "empty", Shape::K2, &CANDS).reason,
        Some("no_queries")
    );
    let s = fb(&loaded, "impact", Shape::Half, &CANDS);
    assert_eq!(s.set, "impact");
    assert_eq!(s.shape, "half");
    assert_eq!(
        (s.pick.as_str(), s.source, s.reason, s.test_commits),
        ("cochange", "calibrated", None, 2)
    );
    assert_eq!(fb(&loaded, "impact", Shape::Find, &CANDS).pick, "lex2");
    log_selection("test", &s);

    // candidate set by pick: digest matches but the pick names no candidate
    let mut cal = match &loaded {
        CalibrationRead::Loaded(c) => (**c).clone(),
        _ => unreachable!(),
    };
    for r in &mut cal.sets {
        r.pick = "gone".to_string();
    }
    let bad = CalibrationRead::Loaded(Box::new(cal));
    assert_eq!(
        fb(&bad, "impact", Shape::K1, &CANDS).reason,
        Some("candidate_set")
    );

    let e = explicit("impact", Shape::K2, "lex2");
    assert_eq!(
        (e.pick.as_str(), e.source, e.reason, e.shape.as_str()),
        ("lex2", "explicit", None, "k2")
    );
    log_selection("test", &e);

    let h = CalibrationHandle::new(loaded.clone());
    assert_eq!(h.read(), &loaded);
    assert_eq!(
        CalibrationHandle::default().read(),
        &CalibrationRead::NotLoaded
    );
}

#[test]
fn shapes() {
    let got: Vec<Shape> = (0..=6).map(Shape::from_seed_files).collect();
    assert_eq!(
        got,
        vec![
            Shape::K1,
            Shape::K1,
            Shape::K2,
            Shape::K3,
            Shape::Half,
            Shape::Half,
            Shape::Half
        ]
    );
    let all = [Shape::K1, Shape::K2, Shape::K3, Shape::Half, Shape::Find];
    let names: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    assert_eq!(names, vec!["k1", "k2", "k3", "half", "find"]);
    let orders: Vec<u8> = all.iter().map(|s| s.order()).collect();
    assert!(orders.windows(2).all(|w| w[0] < w[1]));
    for s in all {
        assert_eq!(Shape::parse(s.as_str()), Some(s));
    }
    assert_eq!(Shape::parse("k4"), None);
}

#[test]
fn load_for_roots_rules() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(load_for_roots(&BTreeMap::new()), CalibrationRead::NotLoaded);
    let mut roots = BTreeMap::new();
    roots.insert(1u64, tmp.path().display().to_string());
    assert_eq!(load_for_roots(&roots), CalibrationRead::Absent);
    written(tmp.path());
    assert!(matches!(load_for_roots(&roots), CalibrationRead::Loaded(_)));
    roots.insert(2u64, "/elsewhere".to_string());
    assert_eq!(
        load_for_roots(&roots),
        CalibrationRead::Rejected("multi_repo")
    );
}

#[test]
fn set_digest_is_revision_bound_and_order_sensitive() {
    let d = set_digest(&CANDS);
    assert_eq!(
        d,
        glia_code_domain::snapshots::data_hash(&[b"rev=1\n", b"ppr\ncochange\nlex2"])
    );
    assert_ne!(d, set_digest(&["cochange", "ppr", "lex2"]));
}
