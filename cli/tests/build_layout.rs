//! LC.9 — one on-disk layout, one writer. `glia build <repo>` writes the
//! sharded layout (manifest.json + shards + cross_stack.gmap + parse cache) to
//! `<repo>/.glia/graph/`, the directory pyo3's `default_gmap_dir`,
//! `load_from_gmap` and the MCP read, instead of the flat per-graph `.gmap`
//! files it used to drop in `<repo>/.glia/` (no manifest, cross edges lost).
//!
//! Every test copies `tests/fixtures/http_stack_smoke` (a Go backend and an
//! Angular frontend joined by HTTP_CALLS cross edges) into a scratch dir and
//! drives the real binary. `[gmap] ` stderr lines are relayed, so the fired_on
//! marker is grep-able:
//! `cargo test -p glia-cli --test build_layout -- --nocapture 2>&1 | grep -o '\[gmap\] wrote .*'`

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use glia_store::{MANIFEST_VERSION, default_gmap_dir, is_gmap_stale, read_merged_sharded};

/// The content every persisted layout dir carries as its own `.gitignore`.
const SELF_IGNORE: &str = "# written by glia - this directory is regenerated\n*\n";

/// A scratch dir, created fresh and removed on drop (`cli` has no
/// dev-dependencies, so no `tempfile`). Canonical, so paths compare equal to
/// what the binary prints.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glia-lc9-{}-{name}", std::process::id()));
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

/// A fresh copy of the HTTP fixture at `<scratch>/repo`.
fn fixture_repo(s: &Scratch) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli/ has a parent")
        .join("tests/fixtures/http_stack_smoke");
    let repo = s.0.join("repo");
    copy_tree(&fixture, &repo);
    repo
}

/// `glia <args>`, asserted successful, `[gmap] ` lines relayed.
fn glia(args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env_remove("GLIA_NO_PERSIST")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "glia {args:?} exited {:?}\nstderr:\n{stderr}", out.status);
    for line in stderr.lines() {
        if line.starts_with("[gmap] ") {
            eprintln!("{line}");
        }
    }
    out
}

fn path_str(p: &Path) -> &str {
    p.to_str().expect("scratch path is UTF-8")
}

fn gmaps_directly_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "gmap"))
        .collect();
    v.sort();
    v
}

fn cross_triples(m: &glia_graph::MergedGraph) -> Vec<(u64, u64, u32)> {
    let mut v: Vec<(u64, u64, u32)> =
        m.cross_edges.iter().map(|e| (e.from.0, e.to.0, e.category.0)).collect();
    v.sort_unstable();
    v
}

#[test]
fn build_writes_the_layout_the_mcp_reads() {
    let s = Scratch::new("layout");
    let repo = fixture_repo(&s);
    let out = glia(&["build", path_str(&repo)]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    let layout = default_gmap_dir(&repo);
    assert_eq!(layout, repo.join(".glia").join("graph"));
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(layout.join("manifest.json")).expect("manifest.json written"),
    )
    .expect("manifest is JSON");
    assert_eq!(manifest["schema_version"], MANIFEST_VERSION);
    assert!(layout.join("cross_stack.gmap").is_file(), "cross_stack.gmap missing");
    assert!(layout.join("parse_cache.bin").is_file(), "parse cache not beside the layout");
    assert_eq!(gmaps_directly_in(&repo.join(".glia")), Vec::<PathBuf>::new(), "flat shards");
    assert!(!repo.join(".ai").exists(), "nothing is written under .ai any more");
    assert_eq!(
        std::fs::read_to_string(layout.join(".gitignore")).expect("self-ignoring .gitignore"),
        SELF_IGNORE
    );

    // What the MCP loads is what a fresh build computes, cross edges included.
    let loaded = read_merged_sharded(&layout).expect("layout loads");
    let fresh = glia_engine::generate_one(path_str(&repo)).expect("fresh build");
    let loaded_nodes: usize = loaded.graphs.iter().map(|g| g.nodes.len()).sum();
    assert_eq!(loaded_nodes, fresh.total_nodes);
    assert_eq!(cross_triples(&loaded), cross_triples(&fresh.merged));
    assert_eq!(loaded.cross_edges.len(), 3, "http_stack_smoke's HTTP_CALLS cross edges");
    assert!(!is_gmap_stale(&layout, &repo), "a fresh build must not read as stale");

    let marker = format!("[gmap] wrote {} writer=cli shards=", layout.display());
    assert!(stderr.contains(&marker), "fired_on marker missing:\n{stderr}");
    assert!(stderr.contains(" cross=3 bytes="), "marker counts:\n{stderr}");
}

#[test]
fn legacy_layout_is_reported_not_read() {
    let s = Scratch::new("legacy");
    let repo = fixture_repo(&s);
    let legacy = repo.join(".ai").join("repo-graph");
    std::fs::create_dir_all(&legacy).expect("mkdir legacy");
    let legacy_manifest = br#"{"schema_version":1,"shards":[{"name":"repo-7","path":"repo-7.gmap","content_hash":"0"}]}"#;
    std::fs::write(legacy.join("manifest.json"), legacy_manifest).expect("legacy manifest");
    std::fs::write(legacy.join("config.yaml"), "skip: []\n").expect("wrapper config");

    let out = glia(&["build", path_str(&repo)]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let notice = format!("[gmap] legacy layout ignored: {}", legacy.display());
    assert!(stderr.contains(&notice), "legacy notice missing:\n{stderr}");
    assert_eq!(stderr.matches("[gmap] legacy layout ignored:").count(), 1, "{stderr}");
    assert_eq!(std::fs::read(legacy.join("manifest.json")).expect("still there"), legacy_manifest);
    assert_eq!(std::fs::read_to_string(legacy.join("config.yaml")).expect("still there"), "skip: []\n");
    assert!(!legacy.join("parse_cache.bin").exists(), "the cache moved to the new layout");
    let loaded = read_merged_sharded(&default_gmap_dir(&repo)).expect("new layout loads");
    assert_eq!(loaded.cross_edges.len(), 3);
}

/// LB.1 hand-off: shard files no manifest names (an old RepoId's shards, a
/// graph-count change, the 0.4.x flat `glia build` output) are removed by the
/// writer; everything else in those directories is left alone.
#[test]
fn orphan_shards_are_cleaned() {
    let s = Scratch::new("orphans");
    let repo = fixture_repo(&s);
    let glia_dir = repo.join(".glia");
    let layout = default_gmap_dir(&repo);
    let legacy = repo.join(".ai").join("repo-graph");
    for d in [&layout, &legacy] {
        std::fs::create_dir_all(d).expect("mkdir");
    }
    std::fs::write(glia_dir.join("repo-42-00.gmap"), b"old flat").expect("flat shard");
    std::fs::write(glia_dir.join("keep.gmap"), b"not a shard name").expect("user file");
    std::fs::write(layout.join("repo-42-07.gmap"), b"orphan").expect("layout orphan");
    std::fs::write(layout.join("cross_stack.gmap"), b"replaced").expect("prior cross");
    std::fs::write(
        legacy.join("manifest.json"),
        br#"{"schema_version":1,"shards":[{"name":"repo-7","path":"repo-7.gmap","content_hash":"0"}]}"#,
    )
    .expect("legacy manifest");
    std::fs::write(legacy.join("repo-7.gmap"), b"live legacy").expect("legacy shard");
    std::fs::write(legacy.join("repo-42.gmap"), b"legacy orphan").expect("legacy orphan");

    let out = glia(&["build", path_str(&repo)]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!glia_dir.join("repo-42-00.gmap").exists(), "0.4.x flat shard kept");
    assert!(glia_dir.join("keep.gmap").exists(), "a non-shard file was removed");
    assert!(!layout.join("repo-42-07.gmap").exists(), "layout orphan kept");
    assert!(legacy.join("repo-7.gmap").exists(), "a shard the legacy manifest names was removed");
    assert!(!legacy.join("repo-42.gmap").exists(), "legacy orphan kept");
    assert!(stderr.contains("[gmap] removed 1 orphan shard(s) from "), "{stderr}");
    // Every shard left in the layout is one its manifest names.
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(layout.join("manifest.json")).expect("manifest"))
            .expect("JSON");
    let mut named: Vec<String> = manifest["shards"]
        .as_array()
        .expect("shards")
        .iter()
        .filter_map(|e| e["path"].as_str().map(str::to_string))
        .collect();
    named.push("cross_stack.gmap".to_string());
    named.sort();
    let on_disk: Vec<String> = gmaps_directly_in(&layout)
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect();
    assert_eq!(on_disk, named);
    assert!(read_merged_sharded(&layout).is_ok());
}

/// Every file under `dir` (recursive) with its bytes, keyed by relative path.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read dir").flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).expect("under dir").to_path_buf();
                out.push((rel, std::fs::read(&p).expect("read file")));
            }
        }
    }
    out.sort();
    out
}

/// LG.6b hand-off: a checkout as 0.4.18 left it (`tests/fixtures/gmap_pre_leap`
/// materialised: the sharded `.ai/repo-graph/` layout and the flat
/// `glia build` shards in `.glia/`). The first 0.5.0 build writes
/// `.glia/graph`, removes the flat shards (no manifest names them) and leaves
/// every byte of the legacy layout alone (its manifest names all its shards).
#[test]
fn pre_leap_checkout_upgrades_in_place() {
    let s = Scratch::new("preleap");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli/ has a parent")
        .join("tests/fixtures/gmap_pre_leap");
    let d = s.0.join("repo");
    copy_tree(&fixture.join("repo"), &d);
    copy_tree(&fixture.join("layout-ai-repo-graph"), &d.join(".ai/repo-graph"));
    copy_tree(&fixture.join("layout-glia-build"), &d.join(".glia"));
    let flat = gmaps_directly_in(&d.join(".glia"));
    assert!(!flat.is_empty(), "the fixture carries the flat glia build shards");
    let legacy = d.join(".ai").join("repo-graph");
    let legacy_before = snapshot(&legacy);

    let out = glia(&["build", path_str(&d)]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(stderr.contains(&format!("[gmap] legacy layout ignored: {}", legacy.display())), "{stderr}");
    let swept = format!("[gmap] removed {} orphan shard(s) from {}", flat.len(), d.join(".glia").display());
    assert!(stderr.contains(&swept), "{stderr}");
    assert_eq!(gmaps_directly_in(&d.join(".glia")), Vec::<PathBuf>::new());
    assert_eq!(snapshot(&legacy), legacy_before, "the legacy layout was modified");
    let layout = default_gmap_dir(&d);
    let loaded = read_merged_sharded(&layout).expect("the 0.5.0 layout loads");
    assert!(loaded.graphs.iter().map(|g| g.nodes.len()).sum::<usize>() > 0);
    assert!(layout.join("parse_cache.bin").is_file());
    assert!(!is_gmap_stale(&layout, &d));
}

#[test]
fn build_out_names_a_layout_dir() {
    let s = Scratch::new("out");
    let repo = fixture_repo(&s);
    let out_dir = s.0.join("elsewhere").join("layout");
    glia(&["build", path_str(&repo), "--out", path_str(&out_dir)]);
    assert!(out_dir.join("manifest.json").is_file());
    assert_eq!(read_merged_sharded(&out_dir).expect("loads").cross_edges.len(), 3);
    assert!(!default_gmap_dir(&repo).join("manifest.json").exists());
}

/// James, 2026-09-19: the layout never shows in any repo's `git status`, and
/// glia never edits the user's root `.gitignore` to get there.
#[test]
fn layout_never_shows_in_git_status() {
    let s = Scratch::new("gitstatus");
    let repo = fixture_repo(&s);
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", &s.0)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
    };
    let git_ok = git(&["init", "-q"]).is_ok_and(|o| o.status.success());
    glia(&["build", path_str(&repo)]);
    let layout = default_gmap_dir(&repo);
    assert_eq!(std::fs::read_to_string(layout.join(".gitignore")).expect("written"), SELF_IGNORE);
    assert!(!repo.join(".gitignore").exists(), "the root .gitignore is the user's");
    if !git_ok {
        eprintln!("[lc9] git unavailable - git status assertion skipped");
        return;
    }
    let status = git(&["status", "--porcelain", "--untracked-files=all"]).expect("git status");
    let listed = String::from_utf8_lossy(&status.stdout);
    assert!(listed.contains("backend/"), "the fixture itself is untracked: {listed}");
    assert!(!listed.contains(".glia"), "the layout shows in git status:\n{listed}");
}
