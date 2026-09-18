//! Pins the pre-leap on-disk artefacts committed under
//! `tests/fixtures/gmap_pre_leap/` (LG.6b): the pyo3/MCP sharded layout
//! (stored as `layout-ai-repo-graph/` because `.gitignore` swallows any
//! `.ai/repo-graph/`) and the flat `glia build` layout (`layout-glia-build/`).
//!
//! This test pins the FIXTURE, not the current format: it deliberately uses no
//! `repo_graph_store` constant, so it keeps passing after LC.1 bumps
//! `FORMAT_VERSION` / `MANIFEST_VERSION`. LC.1 / LC.8 / LC.9 read these bytes to
//! prove old directories are reported stale and rebuilt; they never rewrite them.

use std::collections::BTreeMap;
use std::hash::Hasher;
use std::path::{Path, PathBuf};

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
