//! The one persist / load module shared by py and cli (LC.7): write a build's
//! `MergedGraph` plus its layout metadata to a sharded `.gmap` directory, and
//! load it back as a [`GenerateResult`] that answers like the fresh one.
//! Extended by LC.8 ([`load_or_rebuild`]), LC.9 and LC.10a.
//!
//! LC.8 — a load self-heals. [`load_or_rebuild`] serves a layout that is
//! current and rebuilds one that is not (an older format, another build's
//! stamp, sources changed since the write, a damaged or missing shard, no
//! layout at all) from its repo root: the one the caller gives, else the roots
//! LC.7 recorded in the manifest. The rebuilt graph is written back through
//! [`persist_result`] unless `GLIA_NO_PERSIST=1`. Without a root, or with
//! rebuilding turned off, the answer is a [`LoadError`] with `needs_rebuild`
//! and the reason, never an archive-library message.
//!
//! LC.9 — one layout, one writer. [`default_layout_dir`] is `<repo>/.glia/graph`
//! (`glia_store::DEFAULT_GMAP_SUBDIR`) and [`persist_result`] is the
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
//!   `manifest.json` ([`glia_store::LayoutMeta`]);
//! - `RepoGraph.properties` is per-graph code state, so it goes in each
//!   shard's code section beside nav and symbols;
//! - `MergedGraph::pass_undo` (LC.10a), the confidences a set-dependent
//!   post-pass overwrote, rides the manifest's `pass_undo` and comes back on
//!   the loaded graph, so a merge of loaded layouts can undo the demotions
//!   (`undo_pass_mutations`) before it re-runs the passes over the union.
//!
//! A root is recorded RELATIVE to the layout dir (`../..` for the in-repo
//! `<repo>/.glia/graph`), so a committed layout carries no absolute path
//! (no username) and still resolves after a clone; [`load_layout`] resolves it
//! against the dir it loads from.
//!
//! Markers, one line per call:
//! `[gmap] meta: repos=<r> labeled=<l> rooted=<o> parse_errors=<p> properties=<q> pass_undo=<u> writer=<w>`
//! from [`persist_layout`] (the store's own `[gmap] layout` line follows it),
//! `[gmap] wrote <dir> writer=<w> shards=<n> cross=<c> bytes=<b>` from
//! [`persist_result`] (`n` per-graph shards, `c` cross edges, `b` the bytes of
//! every `.gmap` the manifest names), with `[gmap] legacy layout ignored: <dir> ...`
//! and `[gmap] removed <k> orphan shard(s) from <dir>` when they apply, and
//! `[gmap] loaded <dir>: repos=<r> labeled=<l> parse_errors=<p> properties=<q> pass_undo=<u>`
//! from [`load_layout`], and `[gmap] rebuilt <dir> (<reason>)` from
//! [`load_or_rebuild`] each time it rebuilds (written or not).
//!
//! Module slot declared by L0.2: reached as `glia_engine::persist::<item>`,
//! never flattened into the crate root.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use glia_graph::MergedGraph;
use glia_store::{
    CROSS_STACK_NAME, LEGACY_GMAP_SUBDIR, LayoutMeta, LenientManifest, MANIFEST_NAME,
    MANIFEST_VERSION, Manifest, RepoMeta, StoreError, default_gmap_dir, is_gmap_stale,
    read_manifest_lenient, read_merged_sharded_meta, write_merged_sharded_meta,
};

use crate::{BUILD_STAMP, GenerateResult, generate_many, generate_one, generate_one_incremental};

/// Why [`load_layout`] or [`load_or_rebuild`] could not serve a layout.
/// `needs_rebuild` is the store's classification (`StoreError::needs_rebuild`):
/// true when the bytes on disk are missing, old, foreign or damaged and the
/// fix is to regenerate (for [`load_or_rebuild`]: a rebuild it could not or
/// was told not to do); false for a failure a rebuild would not fix
/// (permissions, a full disk).
#[derive(Debug)]
#[non_exhaustive]
pub struct LoadError {
    pub reason: String,
    pub needs_rebuild: bool,
    /// The fix, when `reason` does not already name it; `Display` appends it
    /// as ` - <advice>`. Empty for [`load_or_rebuild`]'s own errors, whose
    /// reason says what to pass.
    advice: &'static str,
}

impl LoadError {
    fn from_store(dir: &Path, e: &StoreError) -> Self {
        let reason = e.rebuild_reason().unwrap_or_else(|| e.to_string());
        let needs_rebuild = e.needs_rebuild();
        Self {
            reason: format!("{}: {reason}", dir.display()),
            needs_rebuild,
            advice: if needs_rebuild { "rebuild the graph" } else { "" },
        }
    }

    /// A layout that needs a rebuild [`load_or_rebuild`] did not do: `fix`
    /// says why not, or what the caller can pass.
    fn not_rebuilt(dir: &Path, reason: &str, fix: &str) -> Self {
        Self {
            reason: format!("{}: needs rebuild ({reason}); {fix}", dir.display()),
            needs_rebuild: true,
            advice: "",
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.advice.is_empty() {
            f.write_str(&self.reason)
        } else {
            write!(f, "{} - {}", self.reason, self.advice)
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
        "[gmap] meta: repos={} labeled={labeled} rooted={rooted} parse_errors={} properties={} pass_undo={} writer={writer}",
        meta.repos.len(),
        meta.parse_errors.len(),
        property_count(merged),
        merged.pass_undo.len(),
    );
    write_merged_sharded_meta(merged, meta, dir)
        .map_err(|e| format!("{writer}: persist to {}: {e}", dir.display()))
}

/// The layout directory for a repo: `<repo>/.glia/graph`
/// (`glia_store::default_gmap_dir`). The one place cli and py ask where
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
        // Not when the caller is writing the legacy dir itself (a
        // `load_or_rebuild` of a 0.4.x layout rewrites it in place).
        let legacy_abs = lenient_absolute(&legacy);
        if legacy.join(MANIFEST_NAME).is_file()
            && legacy_abs != dir_abs
            && legacy_seen.insert(legacy_abs)
        {
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
    read_layout(dir).map_err(|e| LoadError::from_store(dir, &e))
}

/// [`load_layout`] keeping the store's error, so [`load_or_rebuild`] can take
/// its bare `rebuild_reason` (no directory prefix, no advice).
fn read_layout(dir: &Path) -> Result<GenerateResult, StoreError> {
    let (merged, meta) = read_merged_sharded_meta(dir)?;
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
        "[gmap] loaded {}: repos={} labeled={} parse_errors={} properties={} pass_undo={}",
        dir.display(),
        meta.repos.len(),
        repo_labels.len(),
        meta.parse_errors.len(),
        property_count(&merged),
        merged.pass_undo.len(),
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

/// How [`load_or_rebuild`] served a layout.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoadOutcome {
    /// Loaded as it was on disk; nothing was written.
    Fresh,
    /// Rebuilt from its repo root(s). `reason` is the first thing found wrong,
    /// most specific first: `old format (manifest schema <v>, this build reads
    /// <MANIFEST_VERSION>)`, `written by another glia build (<stamp>)`,
    /// `stale: sources changed since the layout was written`, `no layout`, or
    /// the store's reason for a layout it could not read (`old format (no
    /// preamble, ...)`, `shard <name> does not match its manifest hash`, ...).
    Rebuilt { reason: String },
}

/// Load the layout at `dir`, rebuilding it first when it cannot be served as
/// it is (LC.8). The steps:
///
/// 1. Roots: `repo` when given (a multi-repo layout loaded with one `repo` is
///    rebuilt as that one repo), else every `repos[].root` the manifest
///    records, resolved against `dir`. A manifest that records none (0.4.x),
///    or leaves any repo without one, gives no roots.
/// 2. Reason, read from the manifest whatever its schema
///    ([`read_manifest_lenient`], so a layout whose shards no longer open
///    still says why): another schema, then another build stamp, then (roots
///    known) [`is_gmap_stale`] for any root; no manifest at all is `no layout`.
/// 3. No reason: [`load_layout`]. A failure the store classifies as
///    `needs_rebuild` (a damaged, missing or old-format shard) becomes the
///    reason; any other failure (permissions, I/O) is returned as is.
/// 4. With a reason: `rebuild` false, no roots, or a root that is not a
///    directory is a [`LoadError`] with `needs_rebuild` naming the reason and
///    the fix, and nothing is written (never a partial layout). Otherwise one
///    root is rebuilt with [`generate_one_incremental`] (the repo's parse
///    cache is read and saved) and several with [`generate_many`] in manifest
///    order; the result is written to `dir` with [`persist_result`]
///    (`writer=rebuild`), which sweeps the shards the old manifest named, and
///    `[gmap] rebuilt <dir> (<reason>)` is printed.
///
/// `GLIA_NO_PERSIST=1` rebuilds in memory and writes nothing at all: not the
/// layout, and not the parse cache either (a single root uses
/// [`generate_one`]). A write that fails is a warning: the rebuilt graph is
/// still returned.
///
/// A rebuild whose output is byte-identical to what is on disk (a source
/// touched but not changed) leaves the manifest unwritten, so its mtime would
/// still predate the touched file and every later load would rebuild again.
/// Its mtime is set to the moment the rebuild started instead: the layout
/// then reads as checked against the sources as of that moment, and a file
/// edited after the walk began stays newer than it.
pub fn load_or_rebuild(
    dir: &Path,
    repo: Option<&Path>,
    rebuild: bool,
) -> Result<(GenerateResult, LoadOutcome), LoadError> {
    let lenient = read_manifest_lenient(dir);
    let roots: Option<Vec<PathBuf>> = match repo {
        Some(r) => Some(vec![r.to_path_buf()]),
        None => lenient.as_ref().and_then(|m| manifest_roots(dir, m)),
    };
    let reason = match &lenient {
        Some(m) if m.schema_version != MANIFEST_VERSION => Some(format!(
            "old format (manifest schema {}, this build reads {MANIFEST_VERSION})",
            m.schema_version
        )),
        Some(m) if m.build_stamp != BUILD_STAMP => Some(format!(
            "written by another glia build ({})",
            if m.build_stamp.is_empty() { "no build stamp" } else { m.build_stamp.as_str() }
        )),
        Some(_) if roots.iter().flatten().any(|root| is_gmap_stale(dir, root)) => {
            Some("stale: sources changed since the layout was written".to_string())
        }
        None if matches!(dir.join(MANIFEST_NAME).try_exists(), Ok(false)) => {
            Some("no layout".to_string())
        }
        _ => None,
    };
    let reason = match reason {
        Some(reason) => reason,
        None => match read_layout(dir) {
            Ok(r) => return Ok((r, LoadOutcome::Fresh)),
            Err(e) => match e.rebuild_reason() {
                Some(reason) => reason,
                None => return Err(LoadError::from_store(dir, &e)),
            },
        },
    };
    let r = rebuild_layout(dir, roots, rebuild, &reason)?;
    Ok((r, LoadOutcome::Rebuilt { reason }))
}

/// Every repo root `m` records, resolved against `dir`; `None` when it records
/// no repos or any repo without a root (a rebuild from some of them would
/// write a partial layout).
fn manifest_roots(dir: &Path, m: &LenientManifest) -> Option<Vec<PathBuf>> {
    if m.repos.is_empty() {
        return None;
    }
    m.repos
        .iter()
        .map(|r| r.root.as_deref().map(|root| lenient_absolute(&dir.join(root))))
        .collect()
}

/// Step 4 of [`load_or_rebuild`].
fn rebuild_layout(
    dir: &Path,
    roots: Option<Vec<PathBuf>>,
    rebuild: bool,
    reason: &str,
) -> Result<GenerateResult, LoadError> {
    let refuse = |fix: &str| LoadError::not_rebuilt(dir, reason, fix);
    if !rebuild {
        return Err(refuse("rebuild disabled by the caller"));
    }
    let Some(roots) = roots else {
        return Err(refuse("pass repo_path to rebuild it"));
    };
    let mut paths: Vec<String> = Vec::with_capacity(roots.len());
    for root in &roots {
        if !root.is_dir() {
            return Err(refuse(&format!(
                "cannot rebuild: repo root {} does not exist",
                root.display()
            )));
        }
        let Some(path) = root.to_str() else {
            return Err(refuse(&format!(
                "cannot rebuild: repo root {} is not UTF-8",
                root.display()
            )));
        };
        paths.push(path.to_string());
    }
    let persist = std::env::var("GLIA_NO_PERSIST").as_deref() != Ok("1");
    let started = SystemTime::now();
    let built = match (paths.as_slice(), persist) {
        ([one], true) => generate_one_incremental(one),
        ([one], false) => generate_one(one),
        (many, _) => generate_many(many),
    }
    .map_err(|e| refuse(&format!("rebuild failed: {e}")))?;
    if persist {
        match persist_result(&built, dir, "rebuild") {
            Ok(()) => settle_manifest_mtime(dir, started),
            Err(e) => {
                eprintln!("[gmap] warning: rebuilt {} but could not write it: {e}", dir.display());
            }
        }
    }
    eprintln!("[gmap] rebuilt {} ({reason})", dir.display());
    Ok(built)
}

/// Move `<dir>/manifest.json`'s mtime up to `started` when the write left it
/// older (see [`load_or_rebuild`]). A failure is a warning: the next load
/// rebuilds once more, nothing worse.
fn settle_manifest_mtime(dir: &Path, started: SystemTime) {
    let path = dir.join(MANIFEST_NAME);
    let older = std::fs::metadata(&path).and_then(|m| m.modified()).is_ok_and(|t| t < started);
    if !older {
        return;
    }
    let set =
        std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(started));
    if let Err(e) = set {
        eprintln!("[gmap] warning: could not refresh the mtime of {}: {e}", path.display());
    }
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
