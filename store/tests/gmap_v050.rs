//! Pins the 0.5.0 on-disk layout committed under `tests/fixtures/gmap_v050/`
//! (CD.7b) and proves a format-3 reader reports every file in it as an old
//! format to rebuild.
//!
//! `v050_fixture_is_intact` pins the FIXTURE, not the current format: it uses
//! no `glia_store` constant, so it keeps passing after any later bump. The
//! capture came from the `v0.5.0` tag (`2170ff8`), `BUILD_STAMP`
//! `0.5.0+p5fa8bd06e59848d5`; its README says how, and why it is never
//! regenerated.
//!
//! `v050_files_report_old_format` reads a materialised copy in a tempdir (the
//! README's recipe: `../gmap_pre_leap/repo/*` -> `D/`, `layout/*` ->
//! `D/.glia/graph/`), never the fixture in place, and asserts that no store
//! call writes a byte.

use std::collections::BTreeMap;
use std::hash::Hasher;
use std::path::{Path, PathBuf};

use glia_code_domain::cell_type;
use glia_core::{CellPayload, NodeId};
use glia_store::{
    FORMAT_VERSION, MmapContainer, ShardedMmap, StoreError, inspect_path, is_gmap_stale,
    read_merged_sharded, read_merged_sharded_meta, read_to_owned, upsert_cell, upsert_cell_sharded,
};
use twox_hash::XxHash64;

/// Build identity of the v0.5.0 tag.
const V050_STAMP: &str = "0.5.0+p5fa8bd06e59848d5";
/// Archived rkyv `Header` prefix: magic `GMAP` then `version: u32` = 2 (LE).
/// rkyv lays the root near the END of the core, so it is contained in, not a
/// prefix of, every `.gmap`.
const HEADER_V2: &[u8] = b"GMAP\x02\x00\x00\x00";
/// The LC.1 preamble of a format-2 file: `GLIAGMAP` then `2u32` LE.
const PREAMBLE_V2: &[u8] = b"GLIAGMAP\x02\x00\x00\x00";

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/gmap_v050")
}

fn xxh64(bytes: &[u8]) -> String {
    let mut h = XxHash64::with_seed(0);
    h.write(bytes);
    format!("{:016x}", h.finish())
}

/// Rows of the README table headed `| file | bytes | xxhash64 | sha256 |`.
fn readme_table(root: &Path) -> BTreeMap<String, (u64, String)> {
    let readme = std::fs::read_to_string(root.join("README.md")).expect("README.md missing");
    let mut rows = BTreeMap::new();
    let mut in_table = false;
    for line in readme.lines() {
        let cols: Vec<&str> = line
            .split('|')
            .map(|c| c.trim().trim_matches('`'))
            .collect();
        if cols.get(1..5) == Some(&["file", "bytes", "xxhash64", "sha256"][..]) {
            in_table = true;
            continue;
        }
        if !in_table || cols.len() < 5 || cols[1].starts_with("---") {
            in_table &= line.starts_with('|');
            continue;
        }
        let len = cols[2]
            .parse()
            .unwrap_or_else(|_| panic!("bad byte count in README row: {line}"));
        assert!(
            rows.insert(cols[1].to_string(), (len, cols[3].to_string()))
                .is_none(),
            "duplicate README row {}",
            cols[1]
        );
    }
    rows
}

#[test]
fn v050_fixture_is_intact() {
    let root = fixture_root();
    let table = readme_table(&root);
    assert!(!table.is_empty(), "README.md hash table has no rows");

    let mut on_disk: Vec<String> = std::fs::read_dir(root.join("layout"))
        .expect("layout/ missing")
        .map(|e| {
            format!(
                "layout/{}",
                e.expect("dir entry").file_name().to_string_lossy()
            )
        })
        .collect();
    on_disk.sort();
    assert_eq!(
        on_disk,
        table.keys().cloned().collect::<Vec<_>>(),
        "files on disk != README table"
    );

    for (rel, (len, hash)) in &table {
        let bytes = std::fs::read(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!(
            bytes.len() as u64,
            *len,
            "{rel}: byte count differs from README"
        );
        assert_eq!(
            &xxh64(&bytes),
            hash,
            "{rel}: xxhash64 differs from README (fixture bytes changed)"
        );
        if rel.ends_with(".gmap") {
            assert!(
                bytes.starts_with(PREAMBLE_V2),
                "{rel}: no GLIAGMAP format-2 preamble"
            );
            assert!(
                bytes.windows(HEADER_V2.len()).any(|w| w == HEADER_V2),
                "{rel}: no archived GMAP v2 header"
            );
        }
    }

    let dir = root.join("layout");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).expect("manifest.json"))
            .expect("manifest json");
    assert_eq!(manifest["schema_version"], 2, "manifest schema_version");
    assert_eq!(
        manifest["build_stamp"], V050_STAMP,
        "manifest build_stamp is not v0.5.0's"
    );
    assert_eq!(manifest["engine_version"], "0.5.0");
    assert_eq!(
        manifest["repos"][0]["root"], "../..",
        "the root is recorded relative to the layout"
    );
    let shards = manifest["shards"].as_array().expect("manifest shards");
    assert_eq!(shards.len(), 3, "Go, Python and TypeScript shards");
    for entry in shards.iter().chain(manifest.get("cross")) {
        let path = entry["path"].as_str().expect("shard path");
        let bytes =
            std::fs::read(dir.join(path)).unwrap_or_else(|e| panic!("manifest shard {path}: {e}"));
        assert_eq!(
            entry["content_hash"].as_str(),
            Some(xxh64(&bytes).as_str()),
            "{path}: manifest content_hash"
        );
    }
    assert!(
        manifest.get("cross").is_some(),
        "cross_stack shard missing: the repo is cross-service on purpose"
    );
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap_or_else(|e| panic!("mkdir {}: {e}", to.display()));
    for entry in std::fs::read_dir(from).unwrap_or_else(|e| panic!("{}: {e}", from.display())) {
        let entry = entry.expect("dir entry");
        let dst = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &dst);
        } else {
            std::fs::copy(entry.path(), &dst).expect("copy fixture file");
        }
    }
}

/// The README's materialisation at `<tmp>/repo`: the shared sources plus the
/// 0.5.0 layout at `repo/.glia/graph/`, where 0.5.0 wrote it.
fn materialize() -> (tempfile::TempDir, PathBuf) {
    let root = fixture_root();
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    copy_tree(&root.join("../gmap_pre_leap/repo"), &repo);
    copy_tree(&root.join("layout"), &repo.join(".glia/graph"));
    (tmp, repo)
}

/// Every file under `dir`, recursively, keyed by its path relative to `dir`.
fn tree_bytes(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read dir").flatten() {
            let p = entry.path();
            if entry.file_type().expect("file type").is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).expect("under dir").to_path_buf();
                out.insert(rel, std::fs::read(&p).expect("read file"));
            }
        }
    }
    out
}

/// A 0.5.0 file's failure: `OldFormat { found: Some(2) }`, a rebuild, the
/// format-3 reason, and no archive internals in its Display.
fn assert_old_format_v2(what: &str, e: &StoreError) {
    assert!(
        matches!(e, StoreError::OldFormat { found: Some(2) }),
        "{what}: expected OldFormat {{ found: Some(2) }}, got {e:?}"
    );
    assert!(
        e.needs_rebuild(),
        "{what}: {e:?} does not ask for a rebuild"
    );
    assert_eq!(
        e.rebuild_reason().as_deref(),
        Some("old format v2 (this build reads v3)"),
        "{what}"
    );
    let shown = e.to_string();
    assert!(shown.ends_with("- rebuild the graph"), "{what}: {shown}");
    assert!(
        !shown.contains("rkyv") && !shown.contains("rancor"),
        "{what}: {shown}"
    );
}

#[test]
fn v050_files_report_old_format() {
    assert_eq!(
        FORMAT_VERSION, 3,
        "this test pins the 0.5.0 layout against a format-3 reader"
    );
    let (_tmp, repo) = materialize();
    let layout = repo.join(".glia/graph");
    let before = tree_bytes(&repo);

    // Every .gmap the manifest names (read as JSON, not through the store).
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(layout.join("manifest.json")).expect("manifest"))
            .expect("manifest json");
    let files: Vec<PathBuf> = manifest["shards"]
        .as_array()
        .expect("manifest shards")
        .iter()
        .chain(manifest.get("cross"))
        .map(|e| layout.join(e["path"].as_str().expect("shard path")))
        .collect();
    assert_eq!(
        files.len(),
        4,
        "three shards and cross_stack.gmap: {files:?}"
    );
    assert!(files.iter().any(|p| p.ends_with("cross_stack.gmap")));

    for p in &files {
        let rel = p
            .strip_prefix(&repo)
            .expect("under repo")
            .display()
            .to_string();
        match MmapContainer::open(p) {
            Ok(_) => panic!("{rel}: a 0.5.0 file opened as format {FORMAT_VERSION}"),
            Err(e) => assert_old_format_v2(&format!("MmapContainer::open {rel}"), &e),
        }
        match read_to_owned(p) {
            Ok(_) => panic!("{rel}: a 0.5.0 file deserialised"),
            Err(e) => assert_old_format_v2(&format!("read_to_owned {rel}"), &e),
        }
        match upsert_cell(
            p,
            NodeId(1),
            cell_type::INTENT,
            CellPayload::Text("cd7b".into()),
        ) {
            Ok(()) => panic!("{rel}: upsert_cell rewrote a 0.5.0 file"),
            Err(e) => assert_old_format_v2(&format!("upsert_cell {rel}"), &e),
        }
        match inspect_path(p) {
            Ok(_) => panic!("{rel}: inspect_path read a 0.5.0 file"),
            Err(e) => assert_old_format_v2(&format!("inspect_path {rel}"), &e),
        }
    }

    // The layout: the manifest is schema 2 (unchanged in 0.5.1) and its hashes
    // match, so every layout reader gets as far as the first shard and reports
    // the same old format.
    match ShardedMmap::open(&layout) {
        Ok(_) => panic!("ShardedMmap::open served the 0.5.0 layout"),
        Err(e) => assert_old_format_v2("ShardedMmap::open", &e),
    }
    match read_merged_sharded(&layout) {
        Ok(_) => panic!("read_merged_sharded served the 0.5.0 layout"),
        Err(e) => assert_old_format_v2("read_merged_sharded", &e),
    }
    match read_merged_sharded_meta(&layout) {
        Ok(_) => panic!("read_merged_sharded_meta served the 0.5.0 layout"),
        Err(e) => assert_old_format_v2("read_merged_sharded_meta", &e),
    }
    match upsert_cell_sharded(
        &layout,
        NodeId(1),
        cell_type::INTENT,
        CellPayload::Text("cd7b".into()),
    ) {
        Ok(()) => panic!("upsert_cell_sharded wrote into the 0.5.0 layout"),
        Err(e) => assert_old_format_v2("upsert_cell_sharded", &e),
    }
    match inspect_path(&layout) {
        Ok(_) => panic!("inspect_path read the 0.5.0 layout"),
        Err(e) => assert_old_format_v2("inspect_path <layout>", &e),
    }
    // Another build wrote it, so the freshness scan says stale too.
    assert!(
        is_gmap_stale(&layout, &repo),
        "the 0.5.0 layout reads fresh"
    );

    let after = tree_bytes(&repo);
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>(),
        "a store call created or removed a file"
    );
    for (rel, bytes) in &before {
        assert!(
            after[rel] == *bytes,
            "{}: a store call changed its bytes",
            rel.display()
        );
    }
}
