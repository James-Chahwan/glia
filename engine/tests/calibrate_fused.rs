//! FB.7d: calibration measures the blend (ruling FB-7-FUSED, FB.6b's PASS).
//! With `CalibrateOptions::semantic` (the repo's t2 vectors from `embed_repo`
//! and the deterministic `test-hash` embedder) the replay embeds every find
//! query once and SELf2t is calibrated over `FIND_CANDIDATES_SEM` (the 11
//! lexical find fusions, then `FUSE[T1s,SEMp:t2]` and
//! `LX[IDR,FUSE[T1s,SEMp:t2]]`).
//!
//! The fixture: `M` Python modules, each defining `FNS` functions named
//! `w<m>x<j>`; commit `i` touches modules `i % M` and `(i + 7) % M` and its
//! subject names `w<a>x<i> w<b>x<i>`: tokens no older message holds (so the
//! BM25 over past messages scores nothing) but the touched modules' node
//! texts do (so the query's vector finds them).
//!
//! What is pinned:
//! (1) with vectors SELf2t's row lists the 13 candidates, its digest is the
//!     13-list's, and its pick is a fused name;
//! (2) without vectors the row is the 0.5.3 one: 11 candidates, their
//!     digest, no fused pick, and a source with the `find` group left out
//!     changes nothing;
//! (3) a record written with vectors read without them (and the reverse)
//!     falls back with reason `candidate_set`, through `select` and through
//!     `find_files`; read with them, `find_files` follows the calibrated pick;
//! (4) two calibrations with vectors write byte-identical records.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glia_code_domain::snapshots::{
    HistoryCommit, HistoryFile, HistoryMessage, HistoryMeta, read_history, read_history_messages,
    write_history_with_messages,
};
use glia_core::RepoId;
use glia_embed::{HashEmbedder, MODEL_TEST_HASH};
use glia_engine::GenerateResult;
use glia_engine::calibrate::fusions::FIND_SEM_NAMES;
use glia_engine::calibrate::record::{
    CALIBRATION_FILE, CalibrationRead, calibration_dir, read_calibration, set_digest,
};
use glia_engine::calibrate::sets::FIND_CANDIDATES_SEM;
use glia_engine::calibrate::{
    CalibrateOptions, FIND_CANDIDATES, FIND_FALLBACK, Shape, calibrate, calibrate_snapshot, select,
};
use glia_engine::find::{FIND_FILES_FUSED, FIND_FILES_FUSION, FindFilesOptions, find_files};
use glia_engine::semantic::{EmbedOptions, SemanticSource, embed_repo};

/// Modules.
const M: usize = 40;
/// Functions per module (one per commit index, so every subject token is new).
const FNS: usize = 60;
/// Commits in the synthetic history.
const COMMITS: usize = 60;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(p, body).expect("write");
}

fn module(m: usize) -> String {
    format!("pkg/m{m:02}.py")
}

fn touched(i: usize) -> [usize; 2] {
    [i % M, (i + 7) % M]
}

fn repo(root: &Path) {
    write(root, "pkg/__init__.py", "");
    for m in 0..M {
        let mut body = String::new();
        for j in 0..FNS {
            body.push_str(&format!("def w{m}x{j}():\n    return {j}\n\n"));
        }
        write(root, &module(m), &body);
    }
}

/// `COMMITS` two-module commits with messages, written newest first.
fn history(root: &Path) {
    let mut commits = Vec::new();
    let mut messages = Vec::new();
    for i in 0..COMMITS {
        let c = format!("{:040x}", i + 1);
        let t = 1_700_000_000 + i as i64 * 3600;
        let [a, b] = touched(i);
        commits.push(HistoryCommit {
            c: c.clone(),
            t,
            files: [a, b]
                .iter()
                .map(|&m| HistoryFile {
                    p: module(m),
                    a: Some(1),
                    d: Some(0),
                    from: None,
                })
                .collect(),
        });
        messages.push(HistoryMessage {
            c,
            at: t,
            s: format!("w{a}x{i} w{b}x{i}"),
            b: String::new(),
            br: None,
        });
    }
    commits.reverse();
    messages.reverse();
    let meta = HistoryMeta::new(commits[0].c.clone(), COMMITS, None, String::new());
    write_history_with_messages(root, meta, &commits, &[], Some(&messages)).expect("history");
}

fn build(root: &Path) -> GenerateResult {
    glia_engine::generate_one(root.to_str().expect("utf-8")).expect("build")
}

/// The tree, its history, its build and its t2 vectors.
fn fixture() -> (tempfile::TempDir, PathBuf, GenerateResult) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("words");
    repo(&root);
    history(&root);
    let b = build(&root);
    let id = *b.repo_roots.keys().next().expect("one repo");
    let rep = embed_repo(
        &root,
        &b.merged,
        RepoId(id),
        &HashEmbedder,
        &EmbedOptions::default(),
    )
    .expect("embed");
    assert!(rep.rows_written > 0, "{rep:?}");
    (tmp, root, b)
}

fn source(b: &GenerateResult) -> Arc<SemanticSource> {
    let roots: BTreeMap<u64, PathBuf> = b
        .repo_roots
        .iter()
        .map(|(k, v)| (*k, PathBuf::from(v)))
        .collect();
    Arc::new(
        SemanticSource::load(&b.merged, &roots, MODEL_TEST_HASH, Arc::new(HashEmbedder))
            .expect("rows"),
    )
}

fn opts(semantic: Option<Arc<SemanticSource>>, groups: &[&'static str]) -> CalibrateOptions {
    let mut o = CalibrateOptions::default();
    o.groups = groups.to_vec();
    o.semantic = semantic;
    o
}

fn record(root: &Path) -> Vec<u8> {
    std::fs::read(calibration_dir(root).join(CALIBRATION_FILE)).expect("record")
}

fn loaded(root: &Path) -> CalibrationRead {
    let r = read_calibration(root);
    assert!(matches!(r, CalibrationRead::Loaded(_)), "not loaded");
    r
}

/// The SELf2t row: (candidates, digest, pick).
fn find_row(read: &CalibrationRead) -> (Vec<String>, String, String) {
    let CalibrationRead::Loaded(cal) = read else {
        panic!("not loaded");
    };
    let r = cal
        .sets
        .iter()
        .find(|r| r.set == "SELf2t" && r.shape == "find")
        .expect("SELf2t row");
    (r.candidates.clone(), r.digest.clone(), r.pick.clone())
}

fn find_opts(semantic: Option<Arc<SemanticSource>>) -> FindFilesOptions {
    let mut o = FindFilesOptions::default();
    o.semantic = semantic;
    o
}

fn sel_of(
    b: &GenerateResult,
    o: &FindFilesOptions,
) -> (String, &'static str, Option<&'static str>) {
    let a = find_files(&b.merged, &b.repo_roots, "w3x59 billing", o).expect("answer");
    let s = a.selection.expect("selection");
    (s.pick, s.source, s.reason)
}

#[test]
fn with_vectors_the_blend_is_calibrated_and_picked() {
    let (_tmp, root, b) = fixture();
    let src = source(&b);
    let s = calibrate(&root, &b.merged, &opts(Some(src.clone()), &["find"])).expect("calibrate");
    let row = s
        .sets
        .iter()
        .find(|r| r.set == "SELf2t")
        .expect("summary row");
    assert_eq!(row.queries as usize, COMMITS, "{row:?}");
    assert!(
        row.selection_recall10 > row.fallback_recall10,
        "the blend beats LX[IDR,T1s]: {row:?}"
    );

    let read = loaded(&root);
    let (cands, digest, pick) = find_row(&read);
    assert_eq!(cands, FIND_CANDIDATES_SEM);
    assert_eq!(digest, set_digest(&FIND_CANDIDATES_SEM));
    assert!(FIND_SEM_NAMES.contains(&pick.as_str()), "pick {pick}");

    // Read with vectors: calibrated, through select and find_files.
    let sel = select(
        &read,
        "SELf2t",
        Shape::Find,
        &FIND_CANDIDATES_SEM,
        FIND_FALLBACK,
    );
    assert_eq!(
        (sel.pick.as_str(), sel.source),
        (pick.as_str(), "calibrated")
    );
    assert_eq!(
        sel_of(&b, &find_opts(Some(src))),
        (pick.clone(), "calibrated", None)
    );

    // Read without vectors: the digest refuses it.
    let sel = select(
        &read,
        "SELf2t",
        Shape::Find,
        &FIND_CANDIDATES,
        FIND_FALLBACK,
    );
    assert_eq!(
        (sel.pick.as_str(), sel.source, sel.reason),
        (FIND_FALLBACK, "fallback", Some("candidate_set"))
    );
    assert_eq!(
        sel_of(&b, &find_opts(None)),
        (
            FIND_FILES_FUSION.to_string(),
            "fallback",
            Some("candidate_set")
        )
    );
}

#[test]
fn without_vectors_the_record_is_the_lexical_one() {
    let (_tmp, root, b) = fixture();
    calibrate(&root, &b.merged, &opts(None, &["find"])).expect("calibrate");
    let plain = record(&root);
    let read = loaded(&root);
    let (cands, digest, pick) = find_row(&read);
    assert_eq!(cands, FIND_CANDIDATES);
    assert_eq!(digest, set_digest(&FIND_CANDIDATES));
    assert_eq!(digest, "e682add57de8c8cd");
    assert!(!FIND_SEM_NAMES.contains(&pick.as_str()), "pick {pick}");

    // The reverse: a record without vectors read with them falls back, its
    // pick the fused order (FB.7a's fallback with vectors).
    let sel = select(
        &read,
        "SELf2t",
        Shape::Find,
        &FIND_CANDIDATES_SEM,
        FIND_FALLBACK,
    );
    assert_eq!(sel.reason, Some("candidate_set"));
    assert_eq!(
        sel_of(&b, &find_opts(Some(source(&b)))),
        (
            FIND_FILES_FUSED.to_string(),
            "fallback",
            Some("candidate_set")
        )
    );

    // A source the run never reads (no `find` group) changes nothing.
    calibrate(&root, &b.merged, &opts(None, &["structure"])).expect("structure");
    let without = record(&root);
    calibrate(&root, &b.merged, &opts(Some(source(&b)), &["structure"])).expect("structure");
    assert_eq!(record(&root), without);

    // In memory, no source: the same record the file step wrote.
    let snap = read_history(&root).expect("snapshot");
    let msgs = read_history_messages(&root, &snap);
    let (mem, _) = calibrate_snapshot(
        &b.merged,
        &root,
        &snap,
        msgs.as_deref(),
        &opts(None, &["find"]),
    )
    .expect("in memory");
    let mut bytes = serde_json::to_vec_pretty(&mem).expect("serialise");
    bytes.push(b'\n');
    assert_eq!(bytes, plain);
}

#[test]
fn calibrating_with_vectors_is_deterministic() {
    let (_tmp, root, b) = fixture();
    let o = opts(Some(source(&b)), &["find"]);
    calibrate(&root, &b.merged, &o).expect("first");
    let first = record(&root);
    calibrate(&root, &b.merged, &opts(Some(source(&b)), &["find"])).expect("second");
    assert_eq!(record(&root), first);
}
