//! Pins the pre-leap on-disk artefacts committed under
//! `tests/fixtures/gmap_pre_leap/` (LG.6b): the pyo3/MCP sharded layout
//! (stored as `layout-ai-repo-graph/` because `.gitignore` swallows any
//! `.ai/repo-graph/`) and the flat `glia build` layout (`layout-glia-build/`).
//!
//! `pre_leap_fixture_is_intact` pins the FIXTURE, not the current format: it
//! deliberately uses no `repo_graph_store` constant, so it keeps passing after
//! LC.1 bumps `FORMAT_VERSION` / `MANIFEST_VERSION`. LC.1 / LC.8 / LC.9 read
//! these bytes to prove old directories are reported stale and rebuilt; they
//! never rewrite them.
//!
//! `every_pre_leap_file_reports_rebuild` (LG.6c) is the other half: every
//! store entry point that takes a path, handed every pre-leap file or
//! directory, reports "rebuild" and writes nothing, and
//! `loaders_print_the_needs_rebuild_marker` pins LC.1's `[gmap] needs rebuild:`
//! line for both pre-leap directories. Both read a materialised copy in a
//! tempdir, never the fixture in place.

use std::collections::BTreeMap;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::process::Command;

use repo_graph_code_domain::cell_type;
use repo_graph_core::{CellPayload, NodeId};
use repo_graph_store::{
    MANIFEST_VERSION, MmapContainer, ShardedMmap, StoreError, is_gmap_stale,
    read_manifest_lenient, read_merged_sharded, read_merged_sharded_meta, read_to_owned,
    remove_cell, upsert_cell, upsert_cell_sharded,
};
use twox_hash::XxHash64;

/// Build identity of the pre-leap code (0.4.18 at 7a4883b / fe23c8d).
const PRE_LEAP_STAMP: &str = "0.4.18+p3d23e8828e7ba01a";
/// Archived rkyv `Header` prefix: magic `GMAP` then `version: u32` = 1 (LE).
/// rkyv lays the root out near the END of the buffer, so it is contained in,
/// not a prefix of, every `.gmap`.
const HEADER_V1: &[u8] = b"GMAP\x01\x00\x00\x00";
const LAYOUT_DIRS: [&str; 2] = ["layout-ai-repo-graph", "layout-glia-build"];

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/gmap_pre_leap")
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
fn pre_leap_fixture_is_intact() {
    let root = fixture_root();
    let table = readme_table(&root);
    assert!(!table.is_empty(), "README.md hash table has no rows");

    let mut on_disk = Vec::new();
    for dir in LAYOUT_DIRS {
        let entries = std::fs::read_dir(root.join(dir)).unwrap_or_else(|e| panic!("{dir}: {e}"));
        for entry in entries {
            let name = entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned();
            on_disk.push(format!("{dir}/{name}"));
        }
    }
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
                bytes.windows(HEADER_V1.len()).any(|w| w == HEADER_V1),
                "{rel}: no archived GMAP v1 header"
            );
        }
    }

    // Shard names embed the capture path's RepoId: discover them from the manifest.
    let dir = root.join("layout-ai-repo-graph");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).expect("manifest.json"))
            .expect("manifest json");
    assert_eq!(manifest["schema_version"], 1, "manifest schema_version");
    assert_eq!(
        manifest["build_stamp"], PRE_LEAP_STAMP,
        "manifest build_stamp is not the pre-leap stamp"
    );
    let shards = manifest["shards"].as_array().expect("manifest shards");
    assert!(!shards.is_empty(), "manifest lists no shards");
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
        "cross_stack shard missing: fixture repo is cross-service on purpose"
    );
}

// ---------------------------------------------------------------------------
// LG.6c: the store surfaces over the pre-leap bytes
// ---------------------------------------------------------------------------

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

/// The README's materialisation, at `<tmp>/repo`: `repo/*` -> `repo/`,
/// `layout-ai-repo-graph/*` -> `repo/.ai/repo-graph/` and `layout-glia-build/*`
/// -> `repo/.glia/`, the directories the pre-leap code wrote.
fn materialize() -> (tempfile::TempDir, PathBuf) {
    let root = fixture_root();
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    copy_tree(&root.join("repo"), &repo);
    copy_tree(&root.join("layout-ai-repo-graph"), &repo.join(".ai/repo-graph"));
    copy_tree(&root.join("layout-glia-build"), &repo.join(".glia"));
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

/// A `.gmap`-level failure on a pre-leap file: it asks for a rebuild, its
/// Display says so, and it carries no archive internals (the pre-LC.1 text was
/// `rkyv: failed without error information`).
fn assert_file_says_rebuild(what: &str, e: &StoreError) {
    assert!(e.needs_rebuild(), "{what}: {e:?} does not ask for a rebuild");
    assert!(e.rebuild_reason().is_some(), "{what}: {e:?} has no rebuild reason");
    let shown = e.to_string();
    assert!(shown.contains("rebuild"), "{what}: Display does not say rebuild: {shown}");
    assert!(!shown.contains("rkyv"), "{what}: Display leaks rkyv: {shown}");
    assert!(!shown.contains("failed without error information"), "{what}: {shown}");
}

/// A layout-level failure on the pre-leap sharded directory: the manifest
/// schema flavour, whatever the entry point.
fn assert_manifest_schema_1(what: &str, e: &StoreError) {
    assert!(
        matches!(e, StoreError::ManifestSchemaVersion { got: 1, supported } if *supported == MANIFEST_VERSION),
        "{what}: expected ManifestSchemaVersion {{ got: 1, supported: {MANIFEST_VERSION} }}, got {e:?}"
    );
    assert!(e.needs_rebuild(), "{what}: {e:?}");
    assert_eq!(
        e.rebuild_reason(),
        Some(format!("manifest schema 1, this build reads {MANIFEST_VERSION}")),
        "{what}"
    );
}

fn payload() -> CellPayload {
    CellPayload::Text("lg6c".to_string())
}

#[test]
fn every_pre_leap_file_reports_rebuild() {
    let (_tmp, repo) = materialize();
    let sharded = repo.join(".ai/repo-graph");
    let flat = repo.join(".glia");
    let before = tree_bytes(&repo);

    // Every .gmap of the capture: the shards and cross_stack the manifest names
    // (read as JSON, not through the store), then every flat `glia build` file.
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(sharded.join("manifest.json")).expect("manifest"))
            .expect("manifest json");
    let mut files: Vec<PathBuf> = manifest["shards"]
        .as_array()
        .expect("manifest shards")
        .iter()
        .chain(manifest.get("cross"))
        .map(|e| sharded.join(e["path"].as_str().expect("shard path")))
        .collect();
    assert!(
        files.iter().any(|p| p.ends_with("cross_stack.gmap")),
        "the manifest names no cross_stack.gmap: {files:?}"
    );
    let named = files.len();
    let mut flat_files: Vec<PathBuf> = std::fs::read_dir(&flat)
        .expect("flat dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "gmap"))
        .collect();
    flat_files.sort();
    assert!(!flat_files.is_empty(), "no flat glia build files materialised");
    files.extend(flat_files);
    assert!(named > 1, "the manifest names no shards");

    for p in &files {
        let rel = p.strip_prefix(&repo).expect("under repo").display().to_string();
        match MmapContainer::open(p) {
            Ok(_) => panic!("{rel}: a pre-leap file opened"),
            Err(e) => assert_file_says_rebuild(&format!("MmapContainer::open {rel}"), &e),
        }
        match read_to_owned(p) {
            Ok(_) => panic!("{rel}: a pre-leap file deserialised"),
            Err(e) => assert_file_says_rebuild(&format!("read_to_owned {rel}"), &e),
        }
        match upsert_cell(p, NodeId(1), cell_type::INTENT, payload()) {
            Ok(()) => panic!("{rel}: upsert_cell rewrote a pre-leap file"),
            Err(e) => assert_file_says_rebuild(&format!("upsert_cell {rel}"), &e),
        }
        match remove_cell(p, NodeId(1), cell_type::INTENT) {
            Ok(_) => panic!("{rel}: remove_cell read a pre-leap file"),
            Err(e) => assert_file_says_rebuild(&format!("remove_cell {rel}"), &e),
        }
    }

    // The sharded directory: every strict reader and the sharded writer report
    // the manifest schema, before any shard is opened.
    match ShardedMmap::open(&sharded) {
        Ok(_) => panic!("ShardedMmap::open served the pre-leap layout"),
        Err(e) => assert_manifest_schema_1("ShardedMmap::open", &e),
    }
    match read_merged_sharded(&sharded) {
        Ok(_) => panic!("read_merged_sharded served the pre-leap layout"),
        Err(e) => assert_manifest_schema_1("read_merged_sharded", &e),
    }
    match read_merged_sharded_meta(&sharded) {
        Ok(_) => panic!("read_merged_sharded_meta served the pre-leap layout"),
        Err(e) => assert_manifest_schema_1("read_merged_sharded_meta", &e),
    }
    match upsert_cell_sharded(&sharded, NodeId(1), cell_type::INTENT, payload()) {
        Ok(()) => panic!("upsert_cell_sharded wrote into the pre-leap layout"),
        Err(e) => assert_manifest_schema_1("upsert_cell_sharded", &e),
    }
    // The lenient reader still says what the directory is.
    let lenient = read_manifest_lenient(&sharded).expect("a schema-1 manifest reads leniently");
    assert_eq!(lenient.schema_version, 1);
    assert_eq!(lenient.build_stamp, PRE_LEAP_STAMP);
    assert!(lenient.repos.is_empty(), "a 0.4.x manifest records no repos");

    // The flat directory has no manifest: to a layout reader it is no layout,
    // which is a rebuild too, never an attempt to read the files in it.
    assert!(read_manifest_lenient(&flat).is_none());
    for (what, e) in [
        ("ShardedMmap::open", ShardedMmap::open(&flat).err()),
        ("read_merged_sharded", read_merged_sharded(&flat).err()),
    ] {
        let e = e.unwrap_or_else(|| panic!("{what}: the flat directory served as a layout"));
        assert!(
            matches!(&e, StoreError::Io(io) if io.kind() == std::io::ErrorKind::NotFound),
            "{what} on the flat dir: expected Io(NotFound), got {e:?}"
        );
        assert!(e.needs_rebuild(), "{what}: {e:?}");
    }

    assert!(is_gmap_stale(&sharded, &repo), "the pre-leap sharded layout reads fresh");
    assert!(is_gmap_stale(&flat, &repo), "the flat glia build dir reads fresh");

    let after = tree_bytes(&repo);
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>(),
        "a store call created or removed a file"
    );
    for (rel, bytes) in &before {
        assert!(after[rel] == *bytes, "{}: a store call changed its bytes", rel.display());
    }
}

const CHILD_ENV: &str = "GLIA_LG6C_STORE_CHILD";
const CHILD_DONE: &str = "[lg6c] store child done";

/// The child half of [`loaders_print_the_needs_rebuild_marker`]: a no-op
/// unless the parent set [`CHILD_ENV`] to a materialised repo, in which case it
/// calls both layout loaders on both pre-leap directories so its stderr (with
/// `--nocapture`) carries their markers.
#[test]
fn child_load_pre_leap_layouts() {
    let Ok(repo) = std::env::var(CHILD_ENV) else {
        return;
    };
    let repo = PathBuf::from(repo);
    for dir in [repo.join(".ai/repo-graph"), repo.join(".glia")] {
        assert!(read_merged_sharded(&dir).is_err(), "{} loaded", dir.display());
        assert!(read_merged_sharded_meta(&dir).is_err(), "{} loaded", dir.display());
    }
    eprintln!("{CHILD_DONE}");
}

#[test]
fn loaders_print_the_needs_rebuild_marker() {
    let (_tmp, repo) = materialize();
    let repo = repo.canonicalize().expect("canonical repo");
    let run = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", "child_load_pre_leap_layouts", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, &repo)
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "child failed:\n{stderr}");
    assert!(stderr.contains(CHILD_DONE), "child did not run:\n{stderr}");
    let marks: Vec<&str> = stderr.lines().filter(|l| l.starts_with("[gmap] needs rebuild: ")).collect();

    let sharded = format!(
        "[gmap] needs rebuild: {}: manifest schema 1, this build reads {MANIFEST_VERSION}",
        repo.join(".ai/repo-graph").display()
    );
    let flat = format!("[gmap] needs rebuild: {}: not found: ", repo.join(".glia").display());
    assert_eq!(marks.len(), 4, "one marker per failed load:\n{stderr}");
    assert_eq!(marks.iter().filter(|l| **l == sharded).count(), 2, "{stderr}");
    assert_eq!(marks.iter().filter(|l| l.starts_with(&flat)).count(), 2, "{stderr}");
}
