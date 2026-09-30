//! LG.6c: `glia build` over a checkout as 0.4.18 left it, LG.6b's pinned
//! capture (`tests/fixtures/gmap_pre_leap/`) materialised the README's way:
//! `repo/*` -> `<scratch>/repo/`, `layout-ai-repo-graph/*` ->
//! `<scratch>/repo/.ai/repo-graph/`, `layout-glia-build/*` ->
//! `<scratch>/repo/.glia/`. No CLI query command reads a stored graph (every
//! one builds), so `glia build` is the CLI surface that meets pre-leap files.
//!
//! Beyond LC.9's `build_layout.rs::pre_leap_checkout_upgrades_in_place` (the
//! notices, the sweep, the untouched legacy dir), this pins what the upgrade
//! PRODUCES: a layout whose manifest the store reads back as this build's
//! schema and stamp, shards byte-identical to `glia build` of the same sources
//! with no pre-leap leftovers, and a parse cache the next build reuses whole.
//! `[gmap] ` and `[incremental] ` stderr lines are relayed:
//! `cargo test -p glia-cli --test gmap_compat_cli -- --nocapture 2>&1 | grep -E '^\[(gmap|incremental)\] '`
//!
//! CD.7b: `glia inspect` over a 0.5.0 layout (`tests/fixtures/gmap_v050`,
//! format 2) exits 1 and says rebuild, for one shard and for the directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_engine::BUILD_STAMP;
use glia_store::{
    MANIFEST_VERSION, default_gmap_dir, is_gmap_stale, read_manifest_lenient, read_merged_sharded,
};

/// A scratch dir, created fresh and removed on drop (`cli` has no
/// dev-dependencies, so no `tempfile`). Canonical, so paths compare equal to
/// what the binary prints.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glia-lg6c-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir.canonicalize().expect("canonical scratch dir"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli/ has a parent")
        .join("tests/fixtures/gmap_pre_leap")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let dest = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).expect("copy fixture file");
        }
    }
}

/// The pre-leap checkout at `<scratch>/repo` and a pristine copy of its
/// sources at `<scratch>/clean/repo`: both named `repo`, so both builds get
/// the same RepoId (keyed on the directory name outside git, LB.1).
fn materialize(s: &Scratch) -> (PathBuf, PathBuf) {
    let repo = s.0.join("repo");
    copy_tree(&fixture().join("repo"), &repo);
    copy_tree(&fixture().join("layout-ai-repo-graph"), &repo.join(".ai/repo-graph"));
    copy_tree(&fixture().join("layout-glia-build"), &repo.join(".glia"));
    let clean = s.0.join("clean").join("repo");
    copy_tree(&fixture().join("repo"), &clean);
    (repo, clean)
}

/// `glia build <repo>`, asserted successful (exit 0); its stderr, with the
/// `[gmap] ` / `[incremental] ` lines relayed.
fn build(repo: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["build", repo.to_str().expect("scratch path is UTF-8")])
        .env_remove("GLIA_NO_PERSIST")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "glia build exited {:?}\nstderr:\n{stderr}", out.status);
    for line in stderr.lines() {
        if line.starts_with("[gmap] ") || line.starts_with("[incremental] ") {
            eprintln!("{line}");
        }
    }
    stderr
}

/// Every file directly in `dir` with its bytes (none when `dir` is absent).
fn files_in(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| {
            let bytes = std::fs::read(e.path()).expect("read file");
            (e.file_name().to_string_lossy().into_owned(), bytes)
        })
        .collect()
}

fn gmaps_in(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    files_in(dir).into_iter().filter(|(n, _)| n.ends_with(".gmap")).collect()
}

fn lines_with<'a>(stderr: &'a str, prefix: &str) -> Vec<&'a str> {
    stderr.lines().filter(|l| l.starts_with(prefix)).collect()
}

fn assert_same_gmaps(got: &BTreeMap<String, Vec<u8>>, want: &BTreeMap<String, Vec<u8>>, what: &str) {
    assert!(!want.is_empty(), "{what}: the reference layout has no .gmap files");
    assert_eq!(
        got.keys().collect::<Vec<_>>(),
        want.keys().collect::<Vec<_>>(),
        "{what}: .gmap file names differ"
    );
    for (name, bytes) in want {
        assert!(got[name] == *bytes, "{what}: {name} bytes differ");
    }
}

#[test]
fn build_over_a_pre_leap_repo() {
    let s = Scratch::new("build");
    let (repo, clean) = materialize(&s);
    let legacy = repo.join(".ai").join("repo-graph");
    let glia_dir = repo.join(".glia");
    let legacy_before = files_in(&legacy);
    let flat_before = gmaps_in(&glia_dir);
    assert!(legacy_before.contains_key("parse_cache.bin"), "pre-leap sidecar not materialised");
    assert!(!flat_before.is_empty(), "flat glia build shards not materialised");

    let stderr = build(&repo);
    let layout = default_gmap_dir(&repo);

    // The new layout, read back through the store: this build's schema and
    // stamp, loadable, fresh against its sources.
    let manifest = read_manifest_lenient(&layout).expect("glia build wrote no manifest");
    assert_eq!(manifest.schema_version, MANIFEST_VERSION);
    assert_eq!(manifest.build_stamp, BUILD_STAMP);
    let loaded = read_merged_sharded(&layout).expect("the new layout loads");
    assert!(loaded.graphs.iter().map(|g| g.nodes.len()).sum::<usize>() > 0);
    assert_eq!(loaded.cross_edges.len(), 1, "the fixture's one TS -> Go HTTP pairing");
    assert!(!is_gmap_stale(&layout, &repo), "a fresh upgrade reads stale");

    // LC.9's markers: the legacy layout is named once and left byte-for-byte;
    // the flat 0.4.x shards, which no manifest names, are swept and said so.
    assert_eq!(
        lines_with(&stderr, "[gmap] legacy layout ignored: "),
        [format!(
            "[gmap] legacy layout ignored: {} (0.5.0 reads and writes .glia/graph; safe to delete)",
            legacy.display()
        )],
        "{stderr}"
    );
    let swept = format!(
        "[gmap] removed {} orphan shard(s) from {}",
        flat_before.len(),
        glia_dir.display()
    );
    assert!(stderr.lines().any(|l| l == swept), "flat sweep marker missing:\n{stderr}");
    assert!(files_in(&legacy) == legacy_before, "the legacy .ai/repo-graph layout was modified");
    assert!(gmaps_in(&glia_dir).is_empty(), "flat 0.4.x shards survived the upgrade");
    // Neither pre-leap sidecar is read: nothing reports a foreign cache.
    assert!(lines_with(&stderr, "[incremental] cache stamp mismatch").is_empty(), "{stderr}");

    // No leftover reaches the graph: the same bytes as a build of the sources alone.
    build(&clean);
    let upgraded = gmaps_in(&layout);
    assert_same_gmaps(&upgraded, &gmaps_in(&default_gmap_dir(&clean)), "pre-leap repo vs clean sources");

    // The sidecar the upgrade wrote is this build's: the next build reuses
    // every parse and rewrites nothing.
    let again = build(&repo);
    let reuse: Vec<&str> = lines_with(&again, "[incremental] ")
        .into_iter()
        .filter(|l| l.contains(": reused "))
        .collect();
    assert_eq!(reuse.len(), 1, "{again}");
    assert!(reuse[0].contains(", reparsed 0, "), "the second build reparsed: {}", reuse[0]);
    assert!(!reuse[0].contains(": reused 0,"), "the second build reused nothing: {}", reuse[0]);
    assert!(lines_with(&again, "[incremental] cache stamp mismatch").is_empty(), "{again}");
    assert_same_gmaps(&gmaps_in(&layout), &upgraded, "second build over the upgraded repo");
    assert!(files_in(&legacy) == legacy_before, "the second build modified the legacy layout");
}

fn v050_layout() -> PathBuf {
    fixture().parent().expect("fixtures dir").join("gmap_v050/layout")
}

/// `glia inspect <path>`: exit code and stderr.
fn inspect(path: &Path) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["inspect", path.to_str().expect("scratch path is UTF-8")])
        .output()
        .expect("glia runs");
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn inspect_a_v050_layout_says_rebuild() {
    let s = Scratch::new("inspect-v050");
    let layout = s.0.join("repo").join(".glia").join("graph");
    copy_tree(&v050_layout(), &layout);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(layout.join("manifest.json")).expect("manifest"))
            .expect("manifest json");
    let shard = layout.join(manifest["shards"][0]["path"].as_str().expect("a shard path"));
    let cross = layout.join("cross_stack.gmap");
    for path in [&shard, &cross, &layout] {
        let (code, stderr) = inspect(path);
        assert_eq!(code, Some(1), "glia inspect {}: {stderr}", path.display());
        let want = format!("error: {}: old format v2 (this build reads v3) - rebuild the graph", path.display());
        assert!(stderr.lines().any(|l| l == want), "glia inspect {}:\n{stderr}", path.display());
        assert!(!stderr.contains("[inspect] "), "a 0.5.0 file was inspected:\n{stderr}");
    }
}
