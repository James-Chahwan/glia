//! LC.8: loading a layout self-heals. `persist::load_or_rebuild` serves a
//! fresh layout as it is and rebuilds a stale, old-format, corrupt or missing
//! one from its repo root (given, or recorded in the manifest by LC.7),
//! rewriting it in place unless `GLIA_NO_PERSIST=1`.
//!
//! The measured failure before LC.8: pyo3 `load_from_gmap(dir)` took only the
//! layout dir, so every one of these cases was a `ValueError` and only the
//! repo-graph wrapper, which pre-checks `is_stale` and swallows any exception
//! into a regenerate, coped.
//!
//! The 0.4.x layout is LG.6b's committed `tests/fixtures/gmap_pre_leap`,
//! materialised into a tempdir the way its README says (never used in place):
//! `repo/*` -> `D/`, `layout-ai-repo-graph/*` -> `D/.ai/repo-graph/`,
//! `layout-glia-build/*` -> `D/.glia/`. Shard names are read from the files,
//! never hard-coded, and staleness is keyed on the manifest, not on mtimes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use glia_engine::persist::{
    LoadOutcome, default_layout_dir, load_or_rebuild, persist_result,
};
use glia_engine::{BUILD_STAMP, GenerateResult, generate_many, generate_one};
use glia_store::{MANIFEST_VERSION, read_manifest_lenient};

/// `GLIA_NO_PERSIST` is process-global and the rebuild reads it, so every test
/// in this binary holds this lock for its whole run.
static ENV: Mutex<()> = Mutex::new(());

fn env_guard(no_persist: bool) -> MutexGuard<'static, ()> {
    let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: every test in this binary holds ENV while it runs, so no other
    // test thread touches the environment concurrently.
    unsafe {
        if no_persist {
            std::env::set_var("GLIA_NO_PERSIST", "1");
        } else {
            std::env::remove_var("GLIA_NO_PERSIST");
        }
    }
    guard
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/gmap_pre_leap")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The pre-leap fixture at `<tmp>/repo`, laid out as a 0.4.18 user's disk.
fn materialise(tmp: &Path) -> PathBuf {
    let d = tmp.join("repo");
    copy_dir(&fixture().join("repo"), &d);
    copy_dir(&fixture().join("layout-ai-repo-graph"), &d.join(".ai/repo-graph"));
    copy_dir(&fixture().join("layout-glia-build"), &d.join(".glia"));
    d
}

/// Just the fixture's sources at `<tmp>/repo`, no layout.
fn sources(tmp: &Path) -> PathBuf {
    let d = tmp.join("repo");
    copy_dir(&fixture().join("repo"), &d);
    d
}

/// A fresh 0.5.0 layout at the repo's default dir, written by the one writer.
fn persist_default(repo: &Path) -> (PathBuf, GenerateResult) {
    let r = generate_one(repo.to_str().unwrap()).unwrap();
    let dir = default_layout_dir(repo);
    persist_result(&r, &dir, "test").unwrap();
    (dir, r)
}

/// Every file directly in `dir`: bytes and mtime.
fn snapshot(dir: &Path) -> BTreeMap<String, (Vec<u8>, SystemTime)> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_type().unwrap().is_file())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let bytes = std::fs::read(e.path()).unwrap();
            (name, (bytes, e.metadata().unwrap().modified().unwrap()))
        })
        .collect()
}

fn gmap_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".gmap"))
        .collect();
    names.sort();
    names
}

/// The `.gmap` files `dir`'s manifest names, shards and cross.
fn manifest_gmaps(dir: &Path) -> Vec<String> {
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let mut names: Vec<String> = v["shards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_string())
        .chain(v["cross"]["path"].as_str().map(str::to_string))
        .collect();
    names.sort();
    names
}

/// A source edit the graph sees: one more Python function.
fn edit_source(repo: &Path) {
    std::thread::sleep(Duration::from_millis(20));
    let worker = repo.join("api/worker.py");
    let mut src = std::fs::read_to_string(&worker).unwrap();
    src.push_str("\n\ndef purge_users():\n    return None\n");
    std::fs::write(&worker, src).unwrap();
}

/// Move a source's mtime to now without changing a byte.
fn touch(path: &Path) {
    std::thread::sleep(Duration::from_millis(20));
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now()).unwrap();
}

fn rebuilt_reason(outcome: LoadOutcome) -> String {
    match outcome {
        LoadOutcome::Rebuilt { reason } => reason,
        other => panic!("expected Rebuilt, got {other:?}"),
    }
}

fn assert_fresh(outcome: &LoadOutcome) {
    assert!(matches!(outcome, LoadOutcome::Fresh), "expected Fresh, got {outcome:?}");
}

/// The graph a load served equals a cold build of the same sources.
fn assert_matches_cold_build(r: &GenerateResult, repo: &Path) {
    let cold = generate_one(repo.to_str().unwrap()).unwrap();
    assert_eq!((r.total_nodes, r.total_edges), (cold.total_nodes, cold.total_edges));
    assert!(r.total_nodes > 0);
}

#[test]
fn old_format_with_repo_rebuilds() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let d = materialise(tmp.path());
    let dir = d.join(".ai/repo-graph");
    let old = read_manifest_lenient(&dir).unwrap();
    assert_eq!(old.schema_version, 1);
    assert!(old.repos.is_empty(), "a 0.4.x manifest records no roots");

    let (r, outcome) = load_or_rebuild(&dir, Some(&d), true).unwrap();
    let reason = rebuilt_reason(outcome);
    assert!(reason.contains("old format"), "{reason}");
    assert!(reason.contains("manifest schema 1"), "{reason}");
    assert_matches_cold_build(&r, &d);

    let new = read_manifest_lenient(&dir).unwrap();
    assert_eq!(new.schema_version, MANIFEST_VERSION);
    assert_eq!(new.build_stamp, BUILD_STAMP);
    assert_eq!(new.repos.len(), 1);
    assert!(new.repos[0].root.is_some(), "the rebuild records its root");
    // Nothing of the 0.4.x layout is left beside the new one.
    assert_eq!(gmap_names(&dir), manifest_gmaps(&dir));

    let (again, outcome) = load_or_rebuild(&dir, Some(&d), true).unwrap();
    assert_fresh(&outcome);
    assert_eq!(again.total_nodes, r.total_nodes);
    // The root is now recorded, so no repo_path is needed either.
    let (_, outcome) = load_or_rebuild(&dir, None, true).unwrap();
    assert_fresh(&outcome);
}

/// The pre-leap layout AND its parse cache at the 0.5.0 location: the cache's
/// foreign stamp and repo identity are discarded, the flat 0.4.x `glia build`
/// shards under `.glia/` are swept by the writer, and the result is a clean
/// build.
#[test]
fn old_format_at_the_default_dir_discards_its_parse_cache() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let d = materialise(tmp.path());
    let dir = default_layout_dir(&d);
    copy_dir(&fixture().join("layout-ai-repo-graph"), &dir);
    let old_cache = std::fs::read(dir.join("parse_cache.bin")).unwrap();
    assert!(!gmap_names(&d.join(".glia")).is_empty(), "flat 0.4.x shards present");

    let (r, outcome) = load_or_rebuild(&dir, Some(&d), true).unwrap();
    assert!(rebuilt_reason(outcome).contains("old format"));
    assert_matches_cold_build(&r, &d);
    let cache = std::fs::read(dir.join("parse_cache.bin")).unwrap();
    assert_ne!(cache, old_cache, "the foreign parse cache was rewritten");
    assert!(
        cache.windows(BUILD_STAMP.len()).any(|w| w == BUILD_STAMP.as_bytes()),
        "the parse cache now carries this build's stamp"
    );
    assert!(gmap_names(&d.join(".glia")).is_empty(), "flat 0.4.x shards swept");
    assert_eq!(gmap_names(&dir), manifest_gmaps(&dir));

    let (_, outcome) = load_or_rebuild(&dir, None, true).unwrap();
    assert_fresh(&outcome);
}

#[test]
fn fresh_layout_is_not_rewritten() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let d = sources(tmp.path());
    let (dir, built) = persist_default(&d);
    let before = snapshot(&dir);

    let (r, outcome) = load_or_rebuild(&dir, Some(&d), true).unwrap();
    assert_fresh(&outcome);
    assert_eq!((r.total_nodes, r.total_edges), (built.total_nodes, built.total_edges));
    assert_eq!(snapshot(&dir), before, "a fresh layout is served, not rewritten");
}

#[test]
fn stale_sources_rebuild() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let d = sources(tmp.path());
    let (dir, built) = persist_default(&d);
    edit_source(&d);

    let (r, outcome) = load_or_rebuild(&dir, Some(&d), true).unwrap();
    let reason = rebuilt_reason(outcome);
    assert!(reason.starts_with("stale"), "{reason}");
    assert!(r.total_nodes > built.total_nodes, "the new function is in the graph");
    assert_matches_cold_build(&r, &d);
    let (_, outcome) = load_or_rebuild(&dir, Some(&d), true).unwrap();
    assert_fresh(&outcome);
}

/// A source touched but not changed rebuilds to byte-identical output, which
/// the store does not rewrite; the next load must still be fresh, not another
/// rebuild (the manifest's mtime is what `is_gmap_stale` compares).
#[test]
fn touched_source_rebuilds_once() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let d = sources(tmp.path());
    let (dir, _) = persist_default(&d);
    let manifest = std::fs::read(dir.join("manifest.json")).unwrap();
    touch(&d.join("api/main.go"));

    let (_, outcome) = load_or_rebuild(&dir, Some(&d), true).unwrap();
    assert!(rebuilt_reason(outcome).starts_with("stale"));
    assert_eq!(std::fs::read(dir.join("manifest.json")).unwrap(), manifest);
    let (_, outcome) = load_or_rebuild(&dir, Some(&d), true).unwrap();
    assert_fresh(&outcome);
}

#[test]
fn roots_come_from_the_manifest() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let d = sources(tmp.path());
    let (dir, _) = persist_default(&d);
    edit_source(&d);

    let (r, outcome) = load_or_rebuild(&dir, None, true).unwrap();
    assert!(rebuilt_reason(outcome).starts_with("stale"));
    let canon = std::fs::canonicalize(&d).unwrap();
    let roots: Vec<PathBuf> = r.repo_roots.values().map(PathBuf::from).collect();
    assert_eq!(roots, [canon], "the root was read from repos[].root");
    assert_matches_cold_build(&r, &d);
}

#[test]
fn no_root_no_rebuild_is_a_clear_error() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let d = materialise(tmp.path());
    let dir = d.join(".ai/repo-graph");
    let before = snapshot(&dir);

    let err = load_or_rebuild(&dir, None, true).err().unwrap();
    assert!(err.needs_rebuild, "{err}");
    let text = err.to_string();
    assert!(text.contains("needs rebuild"), "{text}");
    assert!(text.contains("pass repo_path"), "{text}");
    assert!(text.contains("manifest schema 1"), "{text}");
    assert!(!text.contains("rkyv"), "{text}");
    assert_eq!(snapshot(&dir), before, "nothing written");
}

#[test]
fn rebuild_false_reports() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let d = sources(tmp.path());
    let (dir, _) = persist_default(&d);
    edit_source(&d);
    let before = snapshot(&dir);

    let err = load_or_rebuild(&dir, Some(&d), false).err().unwrap();
    assert!(err.needs_rebuild, "{err}");
    assert!(err.to_string().contains("stale"), "{err}");
    assert_eq!(snapshot(&dir), before, "nothing written");
}

#[test]
fn no_persist_rebuild_writes_nothing() {
    let _env = env_guard(true);
    let tmp = tempfile::tempdir().unwrap();
    let d = sources(tmp.path());
    // GLIA_NO_PERSIST gates load_or_rebuild's write, not this explicit one.
    let (dir, built) = persist_default(&d);
    edit_source(&d);
    let before = snapshot(&dir);

    let (r, outcome) = load_or_rebuild(&dir, Some(&d), true).unwrap();
    assert!(rebuilt_reason(outcome).starts_with("stale"));
    assert!(r.total_nodes > built.total_nodes);
    assert_eq!(snapshot(&dir), before, "manifest, shards and parse cache untouched");
}

#[test]
fn no_layout_is_built() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let d = sources(tmp.path());
    let dir = default_layout_dir(&d);
    assert!(!dir.exists());

    let (r, outcome) = load_or_rebuild(&dir, Some(&d), true).unwrap();
    assert_eq!(rebuilt_reason(outcome), "no layout");
    assert_matches_cold_build(&r, &d);
    let (_, outcome) = load_or_rebuild(&dir, None, true).unwrap();
    assert_fresh(&outcome);
}

/// A shard damaged after the write (manifest schema and stamp still current):
/// the store's hash check classifies it, and the load rebuilds.
#[test]
fn corrupt_shard_rebuilds() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let d = sources(tmp.path());
    let (dir, _) = persist_default(&d);
    let shard = manifest_gmaps(&dir).into_iter().find(|n| n.starts_with("repo-")).unwrap();
    let path = dir.join(&shard);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&path, bytes).unwrap();

    let (r, outcome) = load_or_rebuild(&dir, Some(&d), true).unwrap();
    let reason = rebuilt_reason(outcome);
    let name = shard.trim_end_matches(".gmap");
    assert_eq!(reason, format!("shard {name} does not match its manifest hash"));
    assert_matches_cold_build(&r, &d);
    let (_, outcome) = load_or_rebuild(&dir, None, true).unwrap();
    assert_fresh(&outcome);
}

/// Two repos under one scratch dir, built and persisted together.
fn two_repo_layout(tmp: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let api = tmp.join("api");
    let web = tmp.join("web");
    copy_dir(&fixture().join("repo/api"), &api);
    copy_dir(&fixture().join("repo/web"), &web);
    let paths = [api.to_string_lossy().into_owned(), web.to_string_lossy().into_owned()];
    let r = generate_many(&paths).unwrap();
    let dir = tmp.join("layout");
    persist_result(&r, &dir, "test").unwrap();
    (dir, api, web)
}

#[test]
fn multi_repo_layout_rebuilds_from_every_root() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let (dir, api, web) = two_repo_layout(tmp.path());
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(api.join("extra.py"), "def audit_users():\n    return None\n").unwrap();

    let (r, outcome) = load_or_rebuild(&dir, None, true).unwrap();
    assert!(rebuilt_reason(outcome).starts_with("stale"));
    let mut labels: Vec<&str> = r.repo_labels.values().map(String::as_str).collect();
    labels.sort_unstable();
    assert_eq!(labels, ["api", "web"]);
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap().to_string_lossy().into_owned();
    let mut roots: Vec<String> = r.repo_roots.values().cloned().collect();
    roots.sort();
    assert_eq!(roots, [canon(&api), canon(&web)]);
    assert!(!r.merged.cross_edges.is_empty(), "the HTTP pairing survives the rebuild");
    let (_, outcome) = load_or_rebuild(&dir, None, true).unwrap();
    assert_fresh(&outcome);
}

#[test]
fn missing_root_is_named_and_nothing_is_written() {
    let _env = env_guard(false);
    let tmp = tempfile::tempdir().unwrap();
    let (dir, api, web) = two_repo_layout(tmp.path());
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(api.join("extra.py"), "def audit_users():\n    return None\n").unwrap();
    std::fs::remove_dir_all(&web).unwrap();
    let before = snapshot(&dir);

    let err = load_or_rebuild(&dir, None, true).err().unwrap();
    assert!(err.needs_rebuild, "{err}");
    let text = err.to_string();
    let missing = std::fs::canonicalize(tmp.path()).unwrap().join("web");
    assert!(text.contains(&*missing.to_string_lossy()), "{text}");
    assert!(text.contains("does not exist"), "{text}");
    assert_eq!(snapshot(&dir), before, "never a partial layout");
}
