//! The one persist / load module shared by py and cli (LC.7): write a build's
//! `MergedGraph` plus its layout metadata to a sharded `.gmap` directory, and
//! load it back as a [`GenerateResult`] that answers like the fresh one.
//! Extended by LC.8 (`load_or_rebuild`), LC.9 and LC.10a.
//!
//! LC.9 — one layout, one writer. [`default_layout_dir`] is `<repo>/.glia/graph`
//! (`repo_graph_store::DEFAULT_GMAP_SUBDIR`) and [`persist_result`] is the
//! writer `glia build` (so the git hooks), pyo3 `generate`'s auto-persist and
//! `save_to_default` all call. On top of [`persist_layout`] it:
//! - writes the dir's self-ignoring `.gitignore` (`*`), so the layout never
//!   shows in any repo's `git status` without glia touching the user's own
//!   `.gitignore`;
//! - removes orphan shards (the LB.1 hand-off): `repo-<u64>[-NN].gmap` files no
//!   manifest names, in the layout it writes and, when it writes a repo's
//!   default layout, in that repo's 0.4.x locations (the flat `glia build`
//!   output directly under `<repo>/.glia/` and shards the legacy
//!   `<repo>/.ai/repo-graph/manifest.json` no longer names). Nothing else is
//!   ever deleted: not a legacy manifest, not a wrapper's `config.yaml`, not a
//!   directory;
//! - reports a legacy `<repo>/.ai/repo-graph` layout once, never reading it.
//!
//! What survives the round trip, and where it lives:
//! - repo labels and roots, and the build's parse errors describe the LAYOUT
//!   (a multi-repo build has several repos and one error list), so they go in
//!   `manifest.json` ([`repo_graph_store::LayoutMeta`]);
//! - `RepoGraph.properties` is per-graph code state, so it goes in each
//!   shard's code section beside nav and symbols.
//!
//! A root is recorded RELATIVE to the layout dir (`../..` for the in-repo
//! `<repo>/.glia/graph`), so a committed layout carries no absolute path
//! (no username) and still resolves after a clone; [`load_layout`] resolves it
//! against the dir it loads from.
//!
//! Markers, one line per call:
//! `[gmap] meta: repos=<r> labeled=<l> rooted=<o> parse_errors=<p> properties=<q> writer=<w>`
//! from [`persist_layout`] (the store's own `[gmap] layout` line follows it),
//! `[gmap] wrote <dir> writer=<w> shards=<n> cross=<c> bytes=<b>` from
//! [`persist_result`] (`n` per-graph shards, `c` cross edges, `b` the bytes of
//! every `.gmap` the manifest names), with `[gmap] legacy layout ignored: <dir> ...`
//! and `[gmap] removed <k> orphan shard(s) from <dir>` when they apply, and
//! `[gmap] loaded <dir>: repos=<r> labeled=<l> parse_errors=<p> properties=<q>`
//! from [`load_layout`].
//!
//! Module slot declared by L0.2: reached as `repo_graph_engine::persist::<item>`,
//! never flattened into the crate root.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use repo_graph_graph::MergedGraph;
use repo_graph_store::{
    CROSS_STACK_NAME, LEGACY_GMAP_SUBDIR, LayoutMeta, MANIFEST_NAME, Manifest, RepoMeta,
    StoreError, default_gmap_dir, read_merged_sharded_meta, write_merged_sharded_meta,
};

use crate::GenerateResult;

/// Why [`load_layout`] could not serve a layout. `needs_rebuild` is the
/// store's classification (`StoreError::needs_rebuild`): true when the bytes on
/// disk are missing, old, foreign or damaged and the fix is to regenerate;
/// false for a failure a rebuild would not fix (permissions, a full disk).
#[derive(Debug)]
#[non_exhaustive]
pub struct LoadError {
    pub reason: String,
    pub needs_rebuild: bool,
}

impl LoadError {
    fn from_store(dir: &Path, e: &StoreError) -> Self {
        let reason = e.rebuild_reason().unwrap_or_else(|| e.to_string());
        Self { reason: format!("{}: {reason}", dir.display()), needs_rebuild: e.needs_rebuild() }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.needs_rebuild {
            write!(f, "{} - rebuild the graph", self.reason)
        } else {
            f.write_str(&self.reason)
        }
    }
}

impl std::error::Error for LoadError {}

/// The layout metadata of one build, for a layout written to `dir`: one
/// [`RepoMeta`] per repo id in `labels` or `roots` (sorted by id), each root
/// made relative to `dir`, and `parse_errors` in build order. `dir` need not
/// exist yet.
pub fn layout_meta(
    labels: &BTreeMap<u64, String>,
    roots: &BTreeMap<u64, String>,
    parse_errors: &[String],
    dir: &Path,
) -> LayoutMeta {
    let mut ids: Vec<u64> = labels.keys().chain(roots.keys()).copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let repos = ids
        .into_iter()
        .map(|id| RepoMeta {
            id,
            label: labels.get(&id).cloned().unwrap_or_default(),
            root: roots.get(&id).map(|root| {
                let root = Path::new(root);
                relative_root(dir, root)
                    .unwrap_or_else(|| lenient_absolute(root).to_string_lossy().into_owned())
            }),
        })
        .collect();
    LayoutMeta { repos, parse_errors: parse_errors.to_vec() }
}

/// Write `merged` and `meta` as a sharded layout at `dir` (created if missing;
/// unchanged shards and an unchanged manifest are not rewritten). `writer`
/// names the caller (`py`, `cli`, ...) in the marker and in the error text.
pub fn persist_layout(
    merged: &MergedGraph,
    meta: &LayoutMeta,
    dir: &Path,
    writer: &str,
) -> Result<(), String> {
    write_layout(merged, meta, dir, writer).map(|_| ())
}

/// [`persist_layout`], handing back the manifest the store wrote.
fn write_layout(
    merged: &MergedGraph,
    meta: &LayoutMeta,
    dir: &Path,
    writer: &str,
) -> Result<Manifest, String> {
    let labeled = meta.repos.iter().filter(|r| !r.label.is_empty()).count();
    let rooted = meta.repos.iter().filter(|r| r.root.is_some()).count();
    eprintln!(
        "[gmap] meta: repos={} labeled={labeled} rooted={rooted} parse_errors={} properties={} writer={writer}",
        meta.repos.len(),
        meta.parse_errors.len(),
        property_count(merged),
    );
    write_merged_sharded_meta(merged, meta, dir)
        .map_err(|e| format!("{writer}: persist to {}: {e}", dir.display()))
}

/// The layout directory for a repo: `<repo>/.glia/graph`
/// (`repo_graph_store::default_gmap_dir`). The one place cli and py ask where
/// a repo's graph lives; nothing is created.
pub fn default_layout_dir(repo: &Path) -> PathBuf {
    default_gmap_dir(repo)
}

/// The single writer (LC.9): persist a build to the layout at `dir` — the
/// self-ignoring `.gitignore`, the shards, `cross_stack.gmap` and the manifest
/// with the build's labels, roots and parse errors — then clean orphan shards
/// and report a legacy layout (see the module doc). `writer` names the caller
/// (`cli`, `py`) in the markers and the error text. Errors only when the
/// layout itself cannot be written; a failed orphan removal is a warning.
pub fn persist_result(r: &GenerateResult, dir: &Path, writer: &str) -> Result<(), String> {
    persist_graph(&r.merged, &r.repo_labels, &r.repo_roots, &r.parse_errors, dir, writer)
}

/// [`persist_result`] over borrowed parts, for a caller that holds the graph
/// but no longer the [`GenerateResult`] it came from (pyo3's `PyGraph`, whose
/// `save_to_default` writes the same layout the same way).
pub fn persist_graph(
    merged: &MergedGraph,
    labels: &BTreeMap<u64, String>,
    roots: &BTreeMap<u64, String>,
    parse_errors: &[String],
    dir: &Path,
    writer: &str,
) -> Result<(), String> {
    let fail = |e: &dyn fmt::Display| format!("{writer}: persist to {}: {e}", dir.display());
    std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    // First, so a layout interrupted mid-write is already invisible to git.
    write_self_ignore(dir).map_err(|e| fail(&e))?;
    // The 0.4.x locations are swept BEFORE the manifest is written: a removal
    // moves its directory's mtime, and one that post-dates the manifest could
    // read as source churn to `is_gmap_stale` (for a repo that gitignores
    // `.glia/` whole, which turns `.glia` into a REGION). Only this repo's
    // own default layout owns them, so a `--out` elsewhere leaves them be.
    let dir_abs = lenient_absolute(dir);
    let mut legacy_seen: BTreeSet<PathBuf> = BTreeSet::new();
    for root in roots.values().map(Path::new) {
        let legacy = root.join(LEGACY_GMAP_SUBDIR);
        if legacy.join(MANIFEST_NAME).is_file() && legacy_seen.insert(lenient_absolute(&legacy)) {
            eprintln!(
                "[gmap] legacy layout ignored: {} (0.5.0 reads and writes .glia/graph; safe to delete)",
                legacy.display()
            );
        }
        if lenient_absolute(&default_layout_dir(root)) == dir_abs {
            if let Some(glia_dir) = dir.parent() {
                remove_orphan_shards(glia_dir, &BTreeSet::new());
            }
            if let Some(live) = manifest_paths(&legacy) {
                remove_orphan_shards(&legacy, &live);
            }
        }
    }
    let meta = layout_meta(labels, roots, parse_errors, dir);
    let manifest = write_layout(merged, &meta, dir, writer)?;
    let live: BTreeSet<String> =
        manifest.shards.iter().chain(manifest.cross.as_ref()).map(|e| e.path.clone()).collect();
    let bytes: u64 = live
        .iter()
        .filter_map(|p| std::fs::metadata(dir.join(p)).ok())
        .map(|m| m.len())
        .sum();
    eprintln!(
        "[gmap] wrote {} writer={writer} shards={} cross={} bytes={bytes}",
        dir.display(),
        manifest.shards.len(),
        merged.cross_edges.len(),
    );
    remove_orphan_shards(dir, &live);
    Ok(())
}

/// Name of the file that hides a layout directory from git.
const SELF_IGNORE_NAME: &str = ".gitignore";
/// Its content: a `*` in a directory's own `.gitignore` ignores every file in
/// it, itself included, so the whole directory drops out of `git status`
/// (untracked or not) while the user's root `.gitignore` stays untouched.
/// `git add -f` still works for anyone who wants to commit a layout.
const SELF_IGNORE: &str = "# written by glia - this directory is regenerated\n*\n";

/// Write `<dir>/.gitignore` = [`SELF_IGNORE`] unless it already holds exactly
/// that (idempotent; byte-identical on every persist). Staged through a
/// per-process file and renamed, so two writers at once never leave a partial
/// file. Also called by `ParseCache::save`, the other writer into the dir.
pub(crate) fn write_self_ignore(dir: &Path) -> std::io::Result<()> {
    let path = dir.join(SELF_IGNORE_NAME);
    if std::fs::read(&path).is_ok_and(|b| b == SELF_IGNORE.as_bytes()) {
        return Ok(());
    }
    let tmp = dir.join(format!("{SELF_IGNORE_NAME}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, SELF_IGNORE)?;
    std::fs::rename(&tmp, &path)
}

/// Is `name` a shard file glia writes: `repo-<digits>.gmap`,
/// `repo-<digits>-<digits>.gmap` or `cross_stack.gmap`? Anything else in a
/// directory glia sweeps is not glia's to remove.
fn is_shard_name(name: &str) -> bool {
    if name == CROSS_STACK_NAME {
        return true;
    }
    let Some(stem) = name.strip_prefix("repo-").and_then(|n| n.strip_suffix(".gmap")) else {
        return false;
    };
    let mut parts = stem.splitn(2, '-');
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    parts.next().is_some_and(digits) && parts.next().is_none_or(digits)
}

/// The file names a layout's `manifest.json` names (shards and cross), read
/// schema-agnostically so a 0.4.x manifest counts. `None` when there is no
/// readable manifest: then nothing in the directory can be called an orphan.
fn manifest_paths(dir: &Path) -> Option<BTreeSet<String>> {
    let bytes = std::fs::read(dir.join(MANIFEST_NAME)).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let shards = v.get("shards")?.as_array()?;
    let mut live: BTreeSet<String> =
        shards.iter().filter_map(|e| e.get("path")?.as_str().map(str::to_string)).collect();
    if let Some(p) = v.get("cross").and_then(|c| c.get("path")).and_then(|p| p.as_str()) {
        live.insert(p.to_string());
    }
    Some(live)
}

/// Remove every shard-named file directly in `dir` that `live` does not name;
/// one `[gmap] removed <k> orphan shard(s) from <dir>` line when any went.
/// Never fails: a file that cannot be removed is a warning.
fn remove_orphan_shards(dir: &Path, live: &BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut orphans: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            (is_shard_name(&name) && !live.contains(&name)).then(|| e.path())
        })
        .collect();
    orphans.sort();
    let mut removed = 0usize;
    for p in &orphans {
        match std::fs::remove_file(p) {
            Ok(()) => removed += 1,
            Err(e) => {
                eprintln!("[gmap] warning: could not remove orphan shard {}: {e}", p.display());
            }
        }
    }
    if removed > 0 {
        eprintln!("[gmap] removed {removed} orphan shard(s) from {}", dir.display());
    }
}

/// Load the layout at `dir` as a [`GenerateResult`]: the merged graph (with
/// `properties`), `repo_labels` from the manifest's labels, `repo_roots` with
/// each relative root joined onto `dir` and canonicalised (as joined when the
/// path no longer exists), `parse_errors`, and the totals recomputed. A layout
/// written without metadata loads with empty labels, roots and errors.
pub fn load_layout(dir: &Path) -> Result<GenerateResult, LoadError> {
    let (merged, meta) =
        read_merged_sharded_meta(dir).map_err(|e| LoadError::from_store(dir, &e))?;
    let repo_labels: BTreeMap<u64, String> = meta
        .repos
        .iter()
        .filter(|r| !r.label.is_empty())
        .map(|r| (r.id, r.label.clone()))
        .collect();
    let repo_roots: BTreeMap<u64, String> = meta
        .repos
        .iter()
        .filter_map(|r| {
            let root = r.root.as_deref()?;
            let joined = dir.join(root);
            let resolved = std::fs::canonicalize(&joined).unwrap_or(joined);
            Some((r.id, resolved.to_string_lossy().into_owned()))
        })
        .collect();
    let total_nodes: usize = merged.graphs.iter().map(|g| g.nodes.len()).sum();
    let total_edges: usize = merged.graphs.iter().map(|g| g.edges.len()).sum::<usize>()
        + merged.cross_edges.len();
    eprintln!(
        "[gmap] loaded {}: repos={} labeled={} parse_errors={} properties={}",
        dir.display(),
        meta.repos.len(),
        repo_labels.len(),
        meta.parse_errors.len(),
        property_count(&merged),
    );
    Ok(GenerateResult {
        merged,
        total_nodes,
        total_edges,
        parse_errors: meta.parse_errors,
        repo_labels,
        repo_roots,
    })
}

fn property_count(merged: &MergedGraph) -> usize {
    merged.graphs.iter().map(|g| g.properties.len()).sum()
}

/// `root` relative to `dir`, `/`-separated: both made absolute and
/// canonicalised (leniently, so a layout dir that does not exist yet still
/// works), the common prefix stripped, one `..` per remaining `dir` component.
/// `.` when they are the same directory. `None` when no relative path exists
/// (different Windows drives) or a component is not UTF-8.
fn relative_root(dir: &Path, root: &Path) -> Option<String> {
    let dir = lenient_absolute(dir);
    let root = lenient_absolute(root);
    let dir_parts: Vec<Component<'_>> = dir.components().collect();
    let root_parts: Vec<Component<'_>> = root.components().collect();
    // Absolute paths start with their prefix / root: a mismatch there is
    // another drive, which no relative path reaches.
    if dir_parts.first() != root_parts.first() {
        return None;
    }
    let common = dir_parts.iter().zip(&root_parts).take_while(|(a, b)| a == b).count();
    let mut out: Vec<&str> = Vec::new();
    for c in &dir_parts[common..] {
        match c {
            Component::Normal(_) => out.push(".."),
            _ => return None,
        }
    }
    for c in &root_parts[common..] {
        match c {
            Component::Normal(s) => out.push(s.to_str()?),
            _ => return None,
        }
    }
    Some(if out.is_empty() { ".".to_string() } else { out.join("/") })
}

/// `p` absolute and canonical as far as it exists: the deepest existing
/// ancestor is canonicalised and the missing tail re-appended (lexically, `.`
/// dropped and `..` popped). Never fails: with nothing canonicalisable it is
/// `p` made absolute against the working directory, or `p` itself.
fn lenient_absolute(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut base = abs.as_path();
    let mut tail: Vec<Component<'_>> = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(base) {
            let mut out = c;
            for comp in tail.iter().rev() {
                match comp {
                    Component::ParentDir => {
                        out.pop();
                    }
                    Component::CurDir => {}
                    other => out.push(other.as_os_str()),
                }
            }
            return out;
        }
        let Some(parent) = base.parent() else {
            return abs.clone();
        };
        if let Some(last) = base.components().next_back() {
            tail.push(last);
        }
        base = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_root_walks_up_then_down() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let dir = repo.join(".glia/graph");
        assert_eq!(relative_root(&dir, &repo).as_deref(), Some("../.."));
        assert_eq!(relative_root(&repo, &repo).as_deref(), Some("."));
        assert_eq!(relative_root(&repo, &repo.join("src")).as_deref(), Some("src"));
        let sibling = tmp.path().join("other");
        assert_eq!(relative_root(&dir, &sibling).as_deref(), Some("../../../other"));
    }

    #[test]
    fn shard_names_are_exactly_what_glia_writes() {
        for n in ["repo-1.gmap", "repo-12576015503297116104-02.gmap", "cross_stack.gmap"] {
            assert!(is_shard_name(n), "{n}");
        }
        let not_shards = [
            "keep.gmap",
            "repo-.gmap",
            "repo-1-.gmap",
            "repo-a-01.gmap",
            "repo-1-01-2.gmap",
            "repo-1.gmap.tmp",
            "manifest.json",
            "parse_cache.bin",
        ];
        for n in not_shards {
            assert!(!is_shard_name(n), "{n}");
        }
    }

    #[test]
    fn self_ignore_is_idempotent_and_exact() {
        let tmp = tempfile::tempdir().unwrap();
        write_self_ignore(tmp.path()).unwrap();
        let path = tmp.path().join(".gitignore");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SELF_IGNORE);
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_self_ignore(tmp.path()).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before, "rewritten");
        std::fs::write(&path, "edited\n").unwrap();
        write_self_ignore(tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SELF_IGNORE);
        let names: Vec<_> =
            std::fs::read_dir(tmp.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, [".gitignore"], "no staging file left behind");
    }

    #[test]
    fn manifest_paths_reads_any_schema() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(manifest_paths(tmp.path()), None);
        std::fs::write(
            tmp.path().join(MANIFEST_NAME),
            r#"{"schema_version":1,"shards":[{"path":"repo-7.gmap"}],"cross":{"path":"cross_stack.gmap"}}"#,
        )
        .unwrap();
        let live = manifest_paths(tmp.path()).unwrap();
        assert_eq!(live.into_iter().collect::<Vec<_>>(), ["cross_stack.gmap", "repo-7.gmap"]);
    }

    #[test]
    fn lenient_absolute_keeps_a_missing_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let p = tmp.path().join("a/./b/../c");
        assert_eq!(lenient_absolute(&p), base.join("a/c"));
    }
}
