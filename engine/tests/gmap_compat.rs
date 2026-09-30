//! LG.6c: the upgrade a real 0.4.18 user hits, end to end, over LG.6b's pinned
//! capture (`tests/fixtures/gmap_pre_leap/`). The repo still holds BOTH
//! pre-leap directories, the pyo3/MCP sharded layout at `.ai/repo-graph/` and
//! the flat `glia build` shards at `.glia/*.gmap`, and a 0.5.0 consumer loads
//! the new default layout through LC.8's `persist::load_or_rebuild`.
//!
//! What LC.1 / LC.8 / LC.9's own suites do not pin, and this one does:
//! - the load of the NEW default dir (`.glia/graph`, absent in a pre-leap repo)
//!   rebuilds as `no layout`, leaves the legacy layout byte-unchanged, sweeps
//!   the flat shards, and fires LC.8 / LC.9's markers;
//! - the walk never enters the legacy layout (its own
//!   `[walk] skipped engine output .ai/repo-graph` line; `.glia` is a silent
//!   hard skip) and the rebuilt shards are byte-identical to a clean build of
//!   the same sources, so no pre-leap file reaches the graph;
//! - a consumer still pointing at `.ai/repo-graph` gets the exact old-format
//!   reason and the same bytes;
//! - `ParseCache::load` on the captured 0.4.18 `parse_cache.bin`, at the
//!   sidecar path LC.9 reads, is an empty cache, and an incremental build over
//!   it reuses nothing, is byte-identical to a clean one and rewrites the
//!   sidecar under this build's stamp: a `ParseCache` layout change (LA.12)
//!   can never mis-decode old bytes into reused parses. Since the CD.7a
//!   frame, the 0.4.18 file (raw bincode, no `GLIAPCZ1` magic) is never
//!   decoded as a `ParseCache` at all: `ParseCache::load` reads only its
//!   leading bincode string, the stamp, and announces the discard as
//!   `[incremental] cache stamp mismatch (disk=0.4.18+p3d23e8828e7ba01a ...)`.
//!   The rewritten sidecar is a frame whose header carries this build's
//!   stamp. The child still prints whether the old bytes would decode
//!   (`[lg6c] pre-leap sidecar decodes as this build's ParseCache: <bool>`).
//!
//! Stderr markers are only visible outside libtest's capture, so every
//! scenario runs in a child re-run of this binary (`child_scenario`, a no-op
//! unless the parent set [`SCENARIO_ENV`]); the child asserts the in-process
//! results, the parent asserts the markers and the files. The fixture is read
//! only; everything is materialised into a tempdir (canonical, so the paths the
//! markers print compare equal), and `GLIA_NO_PERSIST` is removed from the
//! child's environment rather than mutated in this process.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_engine::persist::{LoadOutcome, default_layout_dir, layout_meta, load_or_rebuild};
use glia_engine::{BUILD_STAMP, ParseCache, generate_one, generate_one_incremental};
use glia_store::{
    MANIFEST_VERSION, read_manifest_lenient, write_merged_sharded, write_merged_sharded_meta,
};

/// Build identity of the code that wrote the capture (the README's pin).
const PRE_LEAP_STAMP: &str = "0.4.18+p3d23e8828e7ba01a";
const SCENARIO_ENV: &str = "GLIA_LG6C_SCENARIO";
const REPO_ENV: &str = "GLIA_LG6C_REPO";
const OUT_ENV: &str = "GLIA_LG6C_OUT";
const CHILD_DONE: &str = "[lg6c] child scenario done:";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/gmap_pre_leap")
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

/// A canonical tempdir holding the pre-leap checkout at `repo/` (the README's
/// materialisation: `layout-ai-repo-graph/*` -> `repo/.ai/repo-graph/`,
/// `layout-glia-build/*` -> `repo/.glia/`) and a pristine copy of the sources
/// at `clean/repo/`. Both are named `repo`: the RepoId is keyed on the
/// directory name outside git (LB.1), so the two builds share it.
struct Materialized {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    clean: PathBuf,
}

fn materialize() -> Materialized {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = root.join("repo");
    copy_tree(&fixture().join("repo"), &repo);
    copy_tree(&fixture().join("layout-ai-repo-graph"), &repo.join(".ai/repo-graph"));
    copy_tree(&fixture().join("layout-glia-build"), &repo.join(".glia"));
    let clean = root.join("clean").join("repo");
    copy_tree(&fixture().join("repo"), &clean);
    Materialized { _tmp: tmp, root, repo, clean }
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

/// Just the `.gmap` files of [`files_in`].
fn gmaps_in(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    files_in(dir).into_iter().filter(|(n, _)| n.ends_with(".gmap")).collect()
}

/// The shards and cross file of a clean, cache-free build of `clean`, written
/// by the store's sharded writer (the `byte_identical.rs` comparison).
fn clean_shards(clean: &Path, out: &Path) -> BTreeMap<String, Vec<u8>> {
    let r = generate_one(clean.to_str().expect("utf-8 path")).expect("clean build");
    write_merged_sharded(&r.merged, out).expect("write clean layout");
    gmaps_in(out)
}

/// [`clean_shards`] written as the one writer (`persist_result`) writes a
/// build: with its repo root recorded, so its CODE cells are stored as spans
/// into the sources (CD.7c) like a rebuilt layout's. A span names the file
/// relative to its root, so the clean copy's spans are the same bytes.
fn rooted_clean_shards(clean: &Path, out: &Path) -> BTreeMap<String, Vec<u8>> {
    let r = generate_one(clean.to_str().expect("utf-8 path")).expect("clean build");
    let meta = layout_meta(&r.repo_labels, &r.repo_roots, &r.parse_errors, out);
    write_merged_sharded_meta(&r.merged, &meta, out).expect("write clean layout");
    gmaps_in(out)
}

fn assert_same_shards(got: &BTreeMap<String, Vec<u8>>, want: &BTreeMap<String, Vec<u8>>, what: &str) {
    assert!(!want.is_empty(), "{what}: the clean build wrote no shards");
    assert_eq!(
        got.keys().collect::<Vec<_>>(),
        want.keys().collect::<Vec<_>>(),
        "{what}: shard file names differ from a clean build (RepoId or shard set moved)"
    );
    for (name, bytes) in want {
        assert!(got[name] == *bytes, "{what}: {name} differs from a clean build of the same sources");
    }
}

/// The child half of every scenario: a no-op unless the parent set
/// [`SCENARIO_ENV`], in which case it runs that scenario so its stderr (with
/// `--nocapture`) carries the markers, and asserts the in-process results.
#[test]
fn child_scenario() {
    let Ok(scenario) = std::env::var(SCENARIO_ENV) else {
        return;
    };
    let repo = PathBuf::from(std::env::var(REPO_ENV).expect("repo for the child"));
    match scenario.as_str() {
        "upgrade-default-dir" => upgrade_default_dir(&repo),
        "legacy-dir" => legacy_dir(&repo),
        "parse-cache" => {
            let out = PathBuf::from(std::env::var(OUT_ENV).expect("out dir for the child"));
            parse_cache(&repo, &out);
        }
        other => panic!("unknown scenario {other}"),
    }
    eprintln!("{CHILD_DONE} {scenario}");
}

/// Run `scenario` over `repo` in a child re-run of this binary; its stderr.
fn run_child(scenario: &str, repo: &Path, out: Option<&Path>) -> String {
    let mut cmd = Command::new(std::env::current_exe().expect("test binary path"));
    cmd.args(["--exact", "child_scenario", "--nocapture", "--test-threads=1"])
        .env(SCENARIO_ENV, scenario)
        .env(REPO_ENV, repo)
        .env_remove("GLIA_NO_PERSIST");
    if let Some(out) = out {
        cmd.env(OUT_ENV, out);
    }
    let run = cmd.output().expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(run.status.success(), "child {scenario} failed:\n{stderr}");
    assert!(
        stderr.contains(&format!("{CHILD_DONE} {scenario}")),
        "child {scenario} did not run:\n{stderr}"
    );
    stderr
}

fn lines_with<'a>(stderr: &'a str, prefix: &str) -> Vec<&'a str> {
    stderr.lines().filter(|l| l.starts_with(prefix)).collect()
}

fn rebuilt_reason(outcome: LoadOutcome) -> String {
    match outcome {
        LoadOutcome::Rebuilt { reason } => reason,
        other => panic!("expected Rebuilt, got {other:?}"),
    }
}

fn assert_fresh(outcome: &LoadOutcome, what: &str) {
    assert!(matches!(outcome, LoadOutcome::Fresh), "{what}: expected Fresh, got {outcome:?}");
}

/// A 0.5.0 consumer loads the repo's default layout, which a pre-leap repo
/// does not have.
fn upgrade_default_dir(repo: &Path) {
    let dir = default_layout_dir(repo);
    assert!(!dir.exists(), "{} exists before the upgrade", dir.display());

    let (r, outcome) = load_or_rebuild(&dir, Some(repo), true).expect("upgrade load");
    assert_eq!(rebuilt_reason(outcome), "no layout");
    assert!(r.total_nodes > 0, "the rebuild served an empty graph");

    let m = read_manifest_lenient(&dir).expect("the rebuild wrote a manifest");
    assert_eq!(m.schema_version, MANIFEST_VERSION);
    assert_eq!(m.build_stamp, BUILD_STAMP);
    assert_eq!(m.repos.len(), 1, "one repo recorded");
    assert!(m.repos[0].root.is_some(), "the rebuild records its root");

    let (again, outcome) = load_or_rebuild(&dir, Some(repo), true).expect("second load");
    assert_fresh(&outcome, "second load");
    assert_eq!((again.total_nodes, again.total_edges), (r.total_nodes, r.total_edges));
    let (_, outcome) = load_or_rebuild(&dir, None, true).expect("load by recorded root");
    assert_fresh(&outcome, "load by the recorded root");
}

/// A consumer still pointing at the 0.4.x MCP directory.
fn legacy_dir(repo: &Path) {
    let dir = repo.join(".ai/repo-graph");
    let (_, outcome) = load_or_rebuild(&dir, Some(repo), true).expect("legacy-dir load");
    assert_eq!(
        rebuilt_reason(outcome),
        format!("old format (manifest schema 1, this build reads {MANIFEST_VERSION})")
    );
    let m = read_manifest_lenient(&dir).expect("rewritten manifest");
    assert_eq!((m.schema_version, m.build_stamp.as_str()), (MANIFEST_VERSION, BUILD_STAMP));
    let (_, outcome) = load_or_rebuild(&dir, None, true).expect("second load");
    assert_fresh(&outcome, "second load of the rewritten legacy dir");
}

/// The leading bincode string of a pre-CD.7a (unframed) parse-cache sidecar:
/// its build stamp (`ParseCache`'s first serialized field; bincode 1 allows
/// trailing bytes).
fn sidecar_stamp(bytes: &[u8]) -> String {
    bincode::deserialize::<String>(bytes).expect("a parse-cache sidecar opens with its stamp")
}

/// The build stamp in a CD.7a parse-cache frame's header: after the
/// `GLIAPCZ1` magic, in the same `u64`-length-prefixed encoding.
fn framed_sidecar_stamp(bytes: &[u8]) -> String {
    let rest = bytes.strip_prefix(b"GLIAPCZ1").expect("the sidecar opens with the GLIAPCZ1 frame magic");
    sidecar_stamp(rest)
}

/// The captured 0.4.18 sidecar sits where LC.9 reads it.
fn parse_cache(repo: &Path, out: &Path) {
    let sidecar = default_layout_dir(repo).join("parse_cache.bin");
    let old = std::fs::read(&sidecar).expect("planted sidecar");
    assert_eq!(sidecar_stamp(&old), PRE_LEAP_STAMP, "the planted sidecar is not the capture's");
    assert_ne!(
        BUILD_STAMP, PRE_LEAP_STAMP,
        "this build still carries the capture's stamp: the discard cannot be observed"
    );
    eprintln!(
        "[lg6c] pre-leap sidecar decodes as this build's ParseCache: {}",
        bincode::deserialize::<ParseCache>(&old).is_ok()
    );
    let cache = ParseCache::load(repo.to_str().expect("utf-8 path"));
    assert!(cache.is_empty(), "the pre-leap parse cache was accepted");
    assert_eq!(cache.len(), 0);
    let r = generate_one_incremental(repo.to_str().expect("utf-8 path")).expect("incremental build");
    write_merged_sharded(&r.merged, out).expect("write incremental layout");
}

#[test]
fn upgrade_rebuilds_into_the_new_layout() {
    let m = materialize();
    let legacy = m.repo.join(".ai/repo-graph");
    let glia_dir = m.repo.join(".glia");
    let legacy_before = files_in(&legacy);
    let flat_before = gmaps_in(&glia_dir);
    assert!(legacy_before.contains_key("manifest.json"), "legacy layout not materialised");
    assert!(!flat_before.is_empty(), "flat glia build shards not materialised");

    let stderr = run_child("upgrade-default-dir", &m.repo, None);
    let dir = default_layout_dir(&m.repo);

    // LC.8's marker: exactly one rebuild, of the new dir, for want of a layout.
    assert_eq!(
        lines_with(&stderr, "[gmap] rebuilt "),
        [format!("[gmap] rebuilt {} (no layout)", dir.display())],
        "{stderr}"
    );
    // LC.9's markers: the legacy layout is named, never read; the flat 0.4.x
    // shards (no manifest names them) are the documented sweep.
    let legacy_lines = lines_with(&stderr, "[gmap] legacy layout ignored: ");
    assert!(!legacy_lines.is_empty(), "no legacy notice:\n{stderr}");
    for line in &legacy_lines {
        assert!(line.contains(&*legacy.display().to_string()), "{line}");
    }
    let swept = format!(
        "[gmap] removed {} orphan shard(s) from {}",
        flat_before.len(),
        glia_dir.display()
    );
    assert_eq!(stderr.matches(&swept).count(), 1, "flat sweep marker:\n{stderr}");
    assert!(lines_with(&stderr, "[gmap] needs rebuild: ").is_empty(), "{stderr}");

    assert!(files_in(&legacy) == legacy_before, "the legacy .ai/repo-graph layout was modified");
    assert!(gmaps_in(&glia_dir).is_empty(), "flat 0.4.x shards survived the upgrade");
    let m2 = read_manifest_lenient(&dir).expect("new layout manifest");
    assert_eq!(m2.schema_version, MANIFEST_VERSION);
    assert!(dir.join("parse_cache.bin").is_file(), "no parse cache beside the new layout");
}

#[test]
fn leftovers_do_not_leak_into_the_graph() {
    let m = materialize();
    let stderr = run_child("upgrade-default-dir", &m.repo, None);
    // One rebuild, one walk, and it stepped over the 0.4.x layout. (The shard
    // comparison alone cannot see a walk INTO it: nothing a 0.4.x wrapper
    // leaves there is a parseable source.)
    assert_eq!(
        lines_with(&stderr, "[walk] skipped engine output "),
        ["[walk] skipped engine output .ai/repo-graph"],
        "{stderr}"
    );
    let want = rooted_clean_shards(&m.clean, &m.root.join("clean-layout"));
    assert_same_shards(&gmaps_in(&default_layout_dir(&m.repo)), &want, "upgrade of the default dir");
}

#[test]
fn legacy_dir_load_rebuilds_with_the_old_format_reason() {
    let m = materialize();
    let legacy = m.repo.join(".ai/repo-graph");
    let flat_before = gmaps_in(&m.repo.join(".glia"));

    let stderr = run_child("legacy-dir", &m.repo, None);
    assert_eq!(
        lines_with(&stderr, "[gmap] rebuilt "),
        [format!(
            "[gmap] rebuilt {} (old format (manifest schema 1, this build reads {MANIFEST_VERSION}))",
            legacy.display()
        )],
        "{stderr}"
    );
    // Rewriting the legacy dir in place is not a legacy layout beside the new
    // one, and the flat shards belong to the default layout's sweep only.
    assert!(lines_with(&stderr, "[gmap] legacy layout ignored: ").is_empty(), "{stderr}");
    assert!(gmaps_in(&m.repo.join(".glia")) == flat_before, "flat shards touched");

    let want = rooted_clean_shards(&m.clean, &m.root.join("clean-layout"));
    assert_same_shards(&gmaps_in(&legacy), &want, "rebuild of the legacy dir");
}

#[test]
fn pre_leap_parse_cache_is_discarded() {
    let m = materialize();
    let sidecar = default_layout_dir(&m.repo).join("parse_cache.bin");
    std::fs::create_dir_all(sidecar.parent().expect("layout dir")).expect("mkdir layout dir");
    std::fs::copy(fixture().join("layout-ai-repo-graph/parse_cache.bin"), &sidecar)
        .expect("plant the pre-leap parse cache");
    let out = m.root.join("incremental-layout");

    let stderr = run_child("parse-cache", &m.repo, Some(&out));
    // The build's own marker: every file parsed afresh, none reused.
    let reuse: Vec<&str> = lines_with(&stderr, "[incremental] ")
        .into_iter()
        .filter(|l| l.contains(": reused "))
        .collect();
    assert_eq!(reuse.len(), 1, "{stderr}");
    assert!(reuse[0].contains(": reused 0, reparsed "), "parses reused from the pre-leap cache: {}", reuse[0]);
    assert!(!reuse[0].contains(", reparsed 0,"), "nothing was parsed: {}", reuse[0]);
    // The discard is announced from the unframed file's leading stamp alone.
    let mismatch =
        format!("[incremental] cache stamp mismatch (disk={PRE_LEAP_STAMP} build={BUILD_STAMP}) — full reparse");
    assert!(stderr.lines().any(|l| l == mismatch), "no stamp-mismatch line for the pre-leap cache:\n{stderr}");
    assert_eq!(
        framed_sidecar_stamp(&std::fs::read(&sidecar).expect("sidecar")),
        BUILD_STAMP,
        "the sidecar was not rewritten as a frame under this build's stamp"
    );

    let want = clean_shards(&m.clean, &m.root.join("clean-layout"));
    assert_same_shards(&gmaps_in(&out), &want, "incremental build over the pre-leap cache");
}
