//! LF.1b: the cell write API. `glia_store::write_cell` /
//! `remove_cell_entry` upsert `.glia/cells.jsonl` (`.glia/vectors.jsonl` for a
//! VECTOR) under `.glia/cells.lock`, and write through into a persisted gmap
//! only when that gmap is fresh right now, so a write never makes a stale
//! layout look fresh and a warm load serves what a rebuild would build.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use glia_code_domain::cell_type;
use glia_code_domain::external_inputs::{
    CELLS_FILE, CellRow, CellWrite, VECTORS_FILE, VectorRow, read_rows,
};
use glia_code_domain::snapshots::REDACTED;
use glia_core::{Cell, CellPayload, CellTypeId, RepoId};
use glia_engine::persist::layout_meta;
use glia_engine::{generate_many, generate_one};
use glia_graph::MergedGraph;
use glia_graph::cells::{CellTarget, QnameIndex, apply_cell_write};
use glia_store::{
    CELLS_LOCK, CellRemoval, MANIFEST_NAME, WriteThrough, default_gmap_dir, is_gmap_stale,
    read_merged_sharded, remove_cell_entry, write_cell, write_merged_sharded,
    write_merged_sharded_meta,
};
use serde_json::json;

const SOURCE: &str = "def charge(order_id):\n    return order_id\n\n\ndef refund(order_id):\n    return charge(order_id)\n";
const VECTOR: [u8; 8] = [0, 0, 128, 63, 0, 0, 0, 64];

/// `<tmp>/<name>` holding `a.py`.
fn repo(tmp: &Path, name: &str) -> PathBuf {
    let dir = tmp.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.py"), SOURCE).unwrap();
    dir
}

fn build(root: &Path) -> MergedGraph {
    generate_one(root.to_str().unwrap()).unwrap().merged
}

/// Build `root` and persist it to its default layout dir, as `glia build` does.
fn persist(root: &Path) -> PathBuf {
    let dir = default_gmap_dir(root);
    write_merged_sharded(&build(root), &dir).unwrap();
    dir
}

/// The cells of the one node named `qname`.
fn cells_of(m: &MergedGraph, qname: &str) -> Vec<Cell> {
    let hits: Vec<&Vec<Cell>> = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter(|n| g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname)))
        .map(|n| &n.cells)
        .collect();
    assert_eq!(hits.len(), 1, "exactly one node is {qname}");
    hits[0].clone()
}

fn payload(m: &MergedGraph, qname: &str, t: CellTypeId) -> Option<CellPayload> {
    cells_of(m, qname).into_iter().find(|c| c.kind == t).map(|c| c.payload)
}

fn conv(text: &str) -> CellWrite {
    CellWrite::entry("a::charge", cell_type::CONV, json!({ "text": text }))
}

fn cell_rows(root: &Path) -> Vec<CellRow> {
    let (rows, errors) = read_rows::<CellRow>(&root.join(CELLS_FILE));
    assert!(errors.is_empty(), "{errors:?}");
    rows
}

/// Every `.gmap` file of a layout dir, by name.
fn shards(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".gmap"))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), fs::read(e.path()).unwrap()))
        .collect();
    out.sort();
    out
}

fn set_mtime(path: &Path, t: SystemTime) {
    fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

/// The sidecar is the durable home: a write with no gmap lands on the node of
/// every later build.
#[test]
fn write_survives_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(tmp.path(), "repo");
    let out = write_cell(&root, None, None, &conv("retries are safe")).unwrap();
    assert_eq!(out.write_through, WriteThrough::NoGmap);
    assert_eq!(out.entry_id.as_deref(), Some("000001"));
    assert_eq!((out.rows, out.target.as_str()), (1, "pending"));
    let want = CellPayload::Json(r#"[{"id":"000001","source":"api","text":"retries are safe"}]"#.into());
    for _ in 0..2 {
        assert_eq!(payload(&build(&root), "a::charge", cell_type::CONV), Some(want.clone()));
    }
}

/// A write into a fresh layout rewrites it so it holds exactly what a rebuild
/// builds (shard bytes equal), records the new sidecar in the manifest, and
/// stays fresh. The row carries the bound node's move-stable hint.
#[test]
fn write_through_equals_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(tmp.path(), "repo");
    let dir = persist(&root);
    let w = CellWrite::entry(
        "a::charge",
        cell_type::DECISION,
        json!({"id": "d1", "title": "Charges are idempotent", "status": "Accepted"}),
    );
    let out = write_cell(&root, Some(&dir), None, &w).unwrap();
    assert_eq!(out.write_through, WriteThrough::Applied);
    assert_eq!(out.target, "bound");
    assert!(out.node.is_some());
    assert!(!is_gmap_stale(&dir, &root), "the write-through keeps the layout fresh");

    let warm = read_merged_sharded(&dir).unwrap();
    let fresh = build(&root);
    let decision = payload(&warm, "a::charge", cell_type::DECISION);
    assert_eq!(
        decision,
        Some(CellPayload::Json(
            r#"[{"id":"d1","source":"api","status":"accepted","title":"Charges are idempotent"}]"#.into()
        ))
    );
    assert_eq!(decision, payload(&fresh, "a::charge", cell_type::DECISION));
    let rebuilt = tmp.path().join("rebuilt");
    write_merged_sharded(&fresh, &rebuilt).unwrap();
    assert_eq!(shards(&dir), shards(&rebuilt), "the write-through gmap is the rebuild's, byte for byte");

    let rows = cell_rows(&root);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].hint.as_deref().is_some_and(|h| h.starts_with("v1|")), "{:?}", rows[0].hint);
    assert_eq!(out.hint, rows[0].hint);
    assert!(!root.join(CELLS_LOCK).exists(), "the lock is released");
}

/// The regression the fresh-only rule prevents: a source edited after the
/// persist would predate a rewritten manifest and a stale gmap would be served
/// as fresh.
#[test]
fn stale_source_is_never_masked() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(tmp.path(), "repo");
    let dir = persist(&root);
    let manifest = fs::read(dir.join(MANIFEST_NAME)).unwrap();
    fs::write(root.join("a.py"), format!("{SOURCE}\n\ndef void(order_id):\n    return None\n")).unwrap();
    set_mtime(&root.join("a.py"), SystemTime::now() + Duration::from_secs(5));
    assert!(is_gmap_stale(&dir, &root));

    let out = write_cell(&root, Some(&dir), None, &conv("n")).unwrap();
    assert_eq!(out.write_through, WriteThrough::GmapStale);
    assert_eq!(out.target, "pending");
    assert!(is_gmap_stale(&dir, &root), "a stale layout stays stale");
    assert_eq!(fs::read(dir.join(MANIFEST_NAME)).unwrap(), manifest, "the manifest is untouched");
    assert_eq!(cell_rows(&root).len(), 1, "the sidecar still takes the write");
}

/// A sidecar edited by hand after the persist makes the layout stale, so the
/// write only lands in the sidecar.
#[test]
fn hand_edited_sidecar_skips_write_through() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(tmp.path(), "repo");
    let dir = persist(&root);
    fs::create_dir_all(root.join(".glia")).unwrap();
    fs::write(
        root.join(CELLS_FILE),
        r#"{"qname":"a::refund","cell":"CONV","entry":{"source":"api","id":"000001","text":"by hand"}}"#,
    )
    .unwrap();
    let out = write_cell(&root, Some(&dir), None, &conv("api")).unwrap();
    assert_eq!(out.write_through, WriteThrough::GmapStale);
    let rows = cell_rows(&root);
    let names: Vec<&str> = rows.iter().map(|r| r.qname.as_str()).collect();
    assert_eq!(names, ["a::charge", "a::refund"], "the hand row is kept, rows sorted");
    assert!(is_gmap_stale(&dir, &root));
}

/// A CONV entry without an id appends: the next zero-padded id among the
/// qname's rows of its source, in write order, no clock.
#[test]
fn conv_appends_get_sequential_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(tmp.path(), "repo");
    let a = write_cell(&root, None, None, &conv("first")).unwrap();
    let b = write_cell(&root, None, None, &conv("second")).unwrap();
    assert_eq!((a.entry_id.as_deref(), b.entry_id.as_deref()), (Some("000001"), Some("000002")));
    assert_eq!(b.rows, 2);
    let ids: Vec<String> = cell_rows(&root).iter().map(|r| r.entry["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, ["000001", "000002"]);
    let other = write_cell(&root, None, None, &CellWrite::entry("a::refund", cell_type::CONV, json!({"text": "x"})))
        .unwrap();
    assert_eq!(other.entry_id.as_deref(), Some("000001"), "ids count per qname");
}

/// A VECTOR goes to vectors.jsonl as base64, writes through as bytes, and a
/// rebuild decodes the same bytes.
#[test]
fn vector_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(tmp.path(), "repo");
    let dir = persist(&root);
    let w = CellWrite::vector("a::charge", VECTOR.to_vec(), Some("m1".into()), Some(2));
    let out = write_cell(&root, Some(&dir), None, &w).unwrap();
    assert_eq!((out.write_through, out.entry_id.as_deref(), out.cell.as_str()), (WriteThrough::Applied, None, "VECTOR"));
    let (rows, errors) = read_rows::<VectorRow>(&root.join(VECTORS_FILE));
    assert!(errors.is_empty());
    assert_eq!((rows.len(), rows[0].b64.as_str(), rows[0].dims), (1, "AACAPwAAAEA=", Some(2)));
    assert!(!root.join(CELLS_FILE).exists(), "a vector never touches cells.jsonl");
    let bytes = Some(CellPayload::Bytes(VECTOR.to_vec()));
    assert_eq!(payload(&read_merged_sharded(&dir).unwrap(), "a::charge", cell_type::VECTOR), bytes);
    assert_eq!(payload(&build(&root), "a::charge", cell_type::VECTOR), bytes);
    // A second vector for the qname replaces the row.
    write_cell(&root, Some(&dir), None, &CellWrite::vector("a::charge", vec![1; 4], None, Some(1))).unwrap();
    let (rows, _) = read_rows::<VectorRow>(&root.join(VECTORS_FILE));
    assert_eq!(rows.len(), 1);
    assert!(!is_gmap_stale(&dir, &root));
}

/// Only CONSTRAINT / DECISION / CONV / VECTOR are writable, each with its own
/// payload shape; a refused write touches nothing.
#[test]
fn code_cell_is_not_writable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(tmp.path(), "repo");
    let code = CellWrite::entry("a::charge", cell_type::CODE, json!({"text": "x"}));
    let err = write_cell(&root, None, None, &code).unwrap_err().to_string();
    assert!(err.contains("WRITABLE"), "{err}");
    let entry_on_vector = CellWrite::entry("a::charge", cell_type::VECTOR, json!({"text": "x"}));
    assert!(write_cell(&root, None, None, &entry_on_vector).is_err());
    let bad_kind = conv("x").with_kind(Some("NOPE".into()));
    assert!(write_cell(&root, None, None, &bad_kind).unwrap_err().to_string().contains("NOPE"));
    let no_id = CellWrite::entry("a::charge", cell_type::DECISION, json!({"title": "t"}));
    assert!(write_cell(&root, None, None, &no_id).unwrap_err().to_string().contains("`id`"));
    assert!(!root.join(".glia").join("cells.jsonl").exists());
}

/// Removing an entry drops it from the sidecar, the fresh layout and every
/// later build; removing it again finds nothing.
#[test]
fn remove_entry_drops_the_cell() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(tmp.path(), "repo");
    let dir = persist(&root);
    write_cell(&root, Some(&dir), None, &conv("gone soon")).unwrap();
    assert!(payload(&read_merged_sharded(&dir).unwrap(), "a::charge", cell_type::CONV).is_some());

    let rm = CellRemoval::entry("a::charge", cell_type::CONV, "api", "000001");
    let out = remove_cell_entry(&root, Some(&dir), None, &rm).unwrap().expect("the row existed");
    assert_eq!((out.write_through, out.rows, out.target.as_str()), (WriteThrough::Applied, 0, "bound"));
    assert!(cell_rows(&root).is_empty());
    assert_eq!(payload(&read_merged_sharded(&dir).unwrap(), "a::charge", cell_type::CONV), None);
    assert_eq!(payload(&build(&root), "a::charge", cell_type::CONV), None);
    assert!(!is_gmap_stale(&dir, &root));
    assert_eq!(remove_cell_entry(&root, Some(&dir), None, &rm).unwrap(), None);
}

/// A layout of several repos has no one repo to bind in unless the caller
/// names it; with the RepoId the write lands in that repo's graphs and both
/// repos stay fresh.
#[test]
fn multi_repo_gmap_without_repo_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (repo(tmp.path(), "alpha"), repo(tmp.path(), "beta"));
    let paths = [a.to_str().unwrap().to_string(), b.to_str().unwrap().to_string()];
    let persist_both = |dir: &Path| {
        let r = generate_many(&paths).unwrap();
        let meta = layout_meta(&r.repo_labels, &r.repo_roots, &r.parse_errors, dir);
        write_merged_sharded_meta(&r.merged, &meta, dir).unwrap();
        r
    };
    let dir = tmp.path().join("layout");
    let r = persist_both(&dir);
    let out = write_cell(&a, Some(&dir), None, &conv("which repo?")).unwrap();
    assert_eq!(out.write_through, WriteThrough::MultiRepo);
    assert!(is_gmap_stale(&dir, &a), "the unwritten layout no longer matches alpha's sidecar");

    let repo_a = r.repo_roots.iter().find(|(_, p)| Path::new(p) == a).map(|(id, _)| RepoId(*id)).unwrap();
    persist_both(&dir);
    assert!(!is_gmap_stale(&dir, &a));
    let out = write_cell(&a, Some(&dir), Some(repo_a), &conv("alpha only")).unwrap();
    assert_eq!((out.write_through, out.target.as_str()), (WriteThrough::Applied, "bound"));
    assert!(!is_gmap_stale(&dir, &a) && !is_gmap_stale(&dir, &b));
    let warm = read_merged_sharded(&dir).unwrap();
    let convs: Vec<(RepoId, CellPayload)> = warm
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(move |n| (g.repo, n)))
        .flat_map(|(r, n)| n.cells.iter().filter(|c| c.kind == cell_type::CONV).map(move |c| (r, c.payload.clone())))
        .collect();
    assert_eq!(convs.len(), 1, "{convs:?}");
    assert_eq!(convs[0].0, repo_a);
}

/// The live in-memory write binds and merges exactly as the next build does.
#[test]
fn in_memory_apply_matches_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(tmp.path(), "repo");
    let mut live = build(&root);
    let out = write_cell(&root, None, None, &conv("live")).unwrap();
    let stored = CellWrite::entry("a::charge", cell_type::CONV, out.entry.clone().unwrap());
    let idx = QnameIndex::build(&live, None);
    assert!(matches!(apply_cell_write(&mut live, &idx, &stored), Ok(CellTarget::Bound(_))));
    assert_eq!(payload(&live, "a::charge", cell_type::CONV), payload(&build(&root), "a::charge", cell_type::CONV));
    assert!(payload(&live, "a::charge", cell_type::CONV).is_some());
}

/// Free text is redacted before it reaches the checked-in sidecar (A13.7).
#[test]
fn free_text_is_redacted_before_it_is_stored() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(tmp.path(), "repo");
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let out = write_cell(&root, None, None, &conv(&format!("rotate {secret} soon"))).unwrap();
    assert_eq!(out.redacted, 1);
    let sidecar = fs::read_to_string(root.join(CELLS_FILE)).unwrap();
    assert!(!sidecar.contains(secret) && sidecar.contains(REDACTED), "{sidecar}");
}

/// A crashed writer's lock expires; a live one makes the next writer give up
/// without touching the sidecar or the other writer's lock.
#[test]
fn lock_expires_and_blocks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(tmp.path(), "repo");
    fs::create_dir_all(root.join(".glia")).unwrap();
    let lock = root.join(CELLS_LOCK);
    fs::write(&lock, "999999\n").unwrap();
    set_mtime(&lock, SystemTime::now() - Duration::from_secs(120));
    write_cell(&root, None, None, &conv("after a crash")).unwrap();
    assert!(!lock.exists());

    fs::write(&lock, "999999\n").unwrap();
    let err = write_cell(&root, None, None, &conv("blocked")).unwrap_err().to_string();
    assert!(err.contains("cells.lock"), "{err}");
    assert!(lock.exists(), "another writer's live lock is left alone");
    assert_eq!(cell_rows(&root).len(), 1);
}
