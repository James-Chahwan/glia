//! Repo walk + gating: which directories collapse to a single region anchor,
//! which files are queued for parsing, and the one-node-per-region graph.
//!
//! DC.1 splits the walk in two: [`plan_walk`] is the sequential descent
//! (gating, regions, project roots, every entered directory with its mtime,
//! every `.gitignore`), [`read_walk`] the pooled reads with an injectable
//! per-file reuse and optional stat stamps. [`walk_source_files`] is their
//! composition, so a clean build prints and returns what it always did.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use glia_code_domain::glia_config::{self, ProjectDecl, Spanned};
use glia_code_domain::project_roots::{self, ProjectRoot};
use glia_code_domain::walk_gating::{self, Collapse, Gate, GateCounts, IgnoreStack};
use glia_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use glia_code_extractors::config::is_dotnet_settings_path;
use glia_code_extractors::contracts::sniff_json_contract;
use glia_code_extractors::schemas::sniff_json_schema;
use glia_core::{Confidence, Node, NodeId, RepoId};

use crate::extract::detect_language;

/// A build-output / vendored / gitignored directory collapsed to a single
/// anchor node instead of being parsed file-by-file. Preserves the repo's
/// spatial map without the per-file flood. (glia-v2 G1/G2/G10)
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RegionAnchor {
    /// Repo-relative path of the collapsed directory (`www`, `packages/x/dist`).
    rel_path: String,
    /// Why it collapsed — rendered into the anchor's ORIGIN cell `provenance`.
    provenance: Collapse,
    /// The directory's own name (`www`, `node_modules`).
    region: String,
}

/// Walk the repo, classifying each directory as source-to-parse or a collapsed
/// region. Returns `(files_to_parse, region_anchors, markdown, project_roots)`.
pub(crate) type WalkResult = (
    Vec<(String, String)>, // source files to parse
    Vec<RegionAnchor>,     // collapsed build/vendor regions
    Vec<(String, String)>, // markdown docs (rel_path, text) — G18
    Vec<ProjectRoot>,      // manifest-rooted and declared sub-projects, sorted by rel_path — A8.4, LF.3a
);

/// The engine's own output directories: the layout `<root>/.glia/graph`
/// (`glia_store::DEFAULT_GMAP_SUBDIR`, where the store writes the gmap and
/// `cache.rs` keeps `parse_cache.bin`) and the 0.4.x layout `<root>/.ai/repo-graph`
/// (`LEGACY_GMAP_SUBDIR`, which a 0.4.x wrapper may still write). Skipped
/// outright, like `store::scan_for_newer` skips them by prefix: otherwise a repo
/// that gitignores one grows a `region:.glia/graph` only after its FIRST
/// persisted build, so the graph depends on build history. (A8.2 → A8.4
/// hand-off, LC.9) Only the root's copies are ours; `.ai` (authored docs) is
/// still walked, and a nested `pkg/.ai/repo-graph` stays under the usual
/// gates. Since LF.1d `walk_gating::is_hard_skip` skips every `.glia`
/// (`CONTROL_DIR`) before this is asked, so its `.glia/graph` arm is a
/// backstop, never the deciding rule.
fn is_self_output(root: &Path, parent: &Path, name: &str) -> bool {
    let at_root =
        |dir: &str| parent.file_name().is_some_and(|n| n == dir) && parent.parent() == Some(root);
    (name == "graph" && at_root(".glia")) || (name == "repo-graph" && at_root(".ai"))
}

/// Largest `.json` the walk reads to sniff for an API contract (A10.8). A
/// generated spec above it (Kubernetes' swagger.json is ~4 MiB) is skipped and
/// counted in the `[contract] json over_cap=` line. The same cap is declared
/// in the coverage caveats, so the skip is visible rather than silent.
const JSON_CONTRACT_CAP: u64 = 512_000;

/// Largest migration `.sql` the walk reads (A13.9). A schema dump
/// (`db/structure.sql`) is well under it; a multi-megabyte data load under
/// `migrations/` is skipped with a `[migrations] skipped` line rather than
/// held in memory for a table scan.
const MIGRATION_SQL_CAP: u64 = 4 * 1024 * 1024;

/// Largest markdown doc the walk reads (G18). A bigger `.md`, or one that is
/// not UTF-8, is not a doc: it falls through to the source rule like any
/// other file ([`PendingClass::Markdown`]).
const MARKDOWN_CAP: u64 = 500_000;

/// A10.8 `[contract] json` marker counters. `sniffed` is every non-manifest
/// `.json` the walk reached, `admitted` is the ones queued as contracts,
/// (LA.16) as JSON Schemas or (CL.7b) as .NET `appsettings*.json` settings
/// files, and `over_cap` is the ones never read because they exceed
/// [`JSON_CONTRACT_CAP`].
#[derive(Default)]
struct JsonAdmission {
    sniffed: usize,
    admitted: usize,
    over_cap: usize,
}

/// A file the walk queued for a read (LG.1b). The descent only gates and
/// classifies; every read runs afterwards on the engine pool
/// ([`crate::parallel::par_map_ordered`], in [`read_walk`]) and one sequential
/// fold puts the texts into `files` / `md` in walk order, so both lists are
/// the ones the inline reads built.
#[derive(Clone, Debug)]
pub(crate) struct Pending {
    /// Repo-relative path, as `files` / `md` key it.
    pub(crate) rel: String,
    pub(crate) abs: PathBuf,
    pub(crate) class: PendingClass,
}

/// Which read rule a [`Pending`] file takes, decided during the descent. A
/// re-gated path can change class (a `.md` whose path starts passing the
/// source rule), so a reuse of a prior [`Read`] must match the class too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingClass {
    /// A `.md` doc (G18), read when it is at most [`MARKDOWN_CAP`] bytes.
    /// `source_fallback`: the path also passes the source rule (a
    /// `Dockerfile.md`, a `.env.md`), so a doc that is over the cap or not
    /// UTF-8 is read as source instead, as the inline walk did: its markdown
    /// branch only stopped the file on a successful read. A `.md` never
    /// reaches the `.json` sniff or the migration cap (neither suffix
    /// matches), so the source rule is the whole fall-through.
    Markdown { source_fallback: bool },
    /// A non-manifest `.json`, read only to be sniffed (A10.8, LA.16), or
    /// admitted by its path as a .NET settings file (CL.7b).
    JsonSniff,
    /// A language file or a bypass path (manifest, yaml, Dockerfile, ...).
    Source,
}

/// The outcome of one [`Pending`] read. `Clone` and `PartialEq` so a watch
/// session can keep it in its file table, hand it back to [`read_walk`] as a
/// reuse, and tell an unchanged re-read from a changed one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Read {
    Md(String),
    /// A sniffed `.json`: `over_cap` when it was never read, `admitted` its
    /// text when it is an API contract, a JSON Schema or a .NET settings file.
    Json { over_cap: bool, admitted: Option<String> },
    Src(String),
    /// Nothing to keep: a read failed, or a doc fell through to no rule.
    Unreadable,
}

/// Read one queued file by its class's rule. Runs on a pool worker: touches
/// nothing but the file. A watch session re-reads a named file through it.
pub(crate) fn read_one(p: &Pending) -> Read {
    let file_len = || std::fs::metadata(&p.abs).map(|m| m.len());
    match p.class {
        PendingClass::Markdown { source_fallback } => {
            if file_len().is_ok_and(|n| n <= MARKDOWN_CAP)
                && let Ok(text) = std::fs::read_to_string(&p.abs)
            {
                return Read::Md(text);
            }
            if source_fallback && let Ok(text) = std::fs::read_to_string(&p.abs) {
                return Read::Src(text);
            }
            Read::Unreadable
        }
        PendingClass::JsonSniff => match file_len() {
            Ok(n) if n > JSON_CONTRACT_CAP => Read::Json {
                over_cap: true,
                admitted: None,
            },
            // CL.7b: an `appsettings*.json` is admitted by its path; its
            // contents are never a contract (route.rs routes it first).
            Ok(_) => Read::Json {
                over_cap: false,
                admitted: std::fs::read_to_string(&p.abs).ok().filter(|t| {
                    is_dotnet_settings_path(&p.rel)
                        || sniff_json_contract(t).is_some()
                        || sniff_json_schema(t)
                }),
            },
            Err(_) => Read::Json {
                over_cap: false,
                admitted: None,
            },
        },
        PendingClass::Source => match std::fs::read_to_string(&p.abs) {
            Ok(text) => Read::Src(text),
            Err(_) => Read::Unreadable,
        },
    }
}

/// `[parallel] walk` marker counts: files queued per [`PendingClass`].
#[derive(Default)]
struct ReadCounts {
    md: usize,
    json: usize,
    source: usize,
}

/// A directory the descent recorded (DC.1), with its mtime at plan time.
/// `rel` is repo-relative, `""` for the root. A watcher stats it instead of
/// re-listing it: a moved mtime means an entry was added, removed or renamed
/// directly under it (Q11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WalkedDir {
    pub(crate) rel: String,
    pub(crate) mtime: Option<SystemTime>,
}

/// One file's stat (DC.1): what a watcher compares to see a file change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    pub(crate) len: u64,
    pub(crate) mtime: Option<SystemTime>,
}

impl FileStamp {
    /// The stamp of `path` now, `None` when it cannot be stat'ed (gone, or no
    /// permission). Follows symlinks, like the read.
    pub(crate) fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        Some(Self { len: m.len(), mtime: m.modified().ok() })
    }
}

/// The modification time of `path`, `None` when the stat or the platform
/// cannot give one.
fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// One queued file's read as [`read_walk`] took it (DC.1): its class, the
/// [`Read`] (fresh or reused), and the stamp taken before it when stamps were
/// asked for. A watch session's file table holds these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReadRecord {
    pub(crate) rel: String,
    pub(crate) class: PendingClass,
    pub(crate) read: Read,
    pub(crate) stamp: Option<FileStamp>,
}

/// The walk's descent (DC.1, [`plan_walk`]): every queued read, the collapsed
/// regions, the detected project roots, the tallies [`read_walk`] prints, and
/// what a watcher stats to see the tree change without re-walking it.
pub(crate) struct WalkPlan {
    /// The files to read, in walk (name) order.
    pub(crate) pending: Vec<Pending>,
    regions: Vec<RegionAnchor>,
    /// Manifest-detected roots in discovery order, before the `[[project]]`
    /// merge and the sort, which [`read_walk`] does.
    roots: Vec<ProjectRoot>,
    declared: DeclaredDirs,
    /// The user config's `[[project]]` stanzas (LF.3a).
    projects: Vec<Spanned<ProjectDecl>>,
    counts: GateCounts,
    /// The `.gitignore` and `[walk] skip` tallies, rendered once the descent
    /// is done and printed last by [`read_walk`], as the one-function walk did.
    ignore_lines: Vec<String>,
    /// Every directory the descent entered (listed), the root first, in walk
    /// order. A region or a hard-skipped directory is not entered, so not
    /// listed.
    pub(crate) dirs: Vec<WalkedDir>,
    /// Every collapsed region root, in walk order. `walk_gating::gate_dir`
    /// decides a region from that directory's own entries (a nested `.git`, a
    /// bundle's chunk names), so its mtime is watched although the walk never
    /// enters it.
    pub(crate) region_dirs: Vec<WalkedDir>,
    /// The repo-relative path of every `.gitignore` in an entered directory,
    /// whether or not it pushed a layer (a comments-only file pushes none).
    pub(crate) gitignores: Vec<String>,
}

/// [`read_walk`]'s sequential fold: the texts into `files` / `md` and the
/// `[contract] json` / `[parallel] walk` counters, in walk order (LG.1b).
#[derive(Default)]
struct ReadFold {
    files: Vec<(String, String)>,
    md: Vec<(String, String)>,
    json: JsonAdmission,
    counts: ReadCounts,
}

impl ReadFold {
    fn add(&mut self, rel: &str, class: PendingClass, read: Read) {
        match class {
            PendingClass::Markdown { .. } => self.counts.md += 1,
            PendingClass::JsonSniff => self.counts.json += 1,
            PendingClass::Source => self.counts.source += 1,
        }
        match read {
            Read::Md(text) => self.md.push((rel.to_string(), text)),
            Read::Json { over_cap, admitted } => {
                self.json.sniffed += 1;
                self.json.over_cap += usize::from(over_cap);
                if let Some(text) = admitted {
                    self.json.admitted += 1;
                    self.files.push((rel.to_string(), text));
                }
            }
            Read::Src(text) => self.files.push((rel.to_string(), text)),
            Read::Unreadable => {}
        }
    }
}

/// Walk the repo: [`plan_walk`], then [`read_walk`] with no reuse and no
/// stamps (DC.1). A clean build takes no extra stat per file and prints
/// exactly the lines the one-function walk printed, in its order.
pub(crate) fn walk_source_files(root: &Path) -> WalkResult {
    read_walk(root, &plan_walk(root), None, false).0
}

/// The walk's sequential descent (DC.1): config load, the `.gitignore`
/// stack, gating, region collapse, project-root detection and the queue of
/// reads. It prints only what the descent prints inline (`[walk] skipped
/// engine output`, `[migrations] skipped`, a `.gitignore` warning); every
/// tally line is [`read_walk`]'s.
pub(crate) fn plan_walk(root: &Path) -> WalkPlan {
    // LF.3a: the user config's `[walk]` and `[[project]]` sections. Loaded once
    // and silently: the build's external-input stage reports loader errors, and
    // the store loads the same file through `IgnoreStack::for_repo`, so a user
    // skip gates the freshness scan exactly as it gates this walk. User config
    // is config, not inference: `--no-overlay` never switches it off.
    let config = glia_config::load(root).map(|l| l.config).unwrap_or_default();
    // Per-directory `.gitignore` layers: pushed on the way down, popped on the
    // way back up, so each verdict sees exactly the files git would. (A8.2)
    let mut ignores = IgnoreStack::with_config(root, &config.walk);
    let mut plan = WalkPlan {
        pending: Vec::new(),
        regions: Vec::new(),
        roots: Vec::new(),
        declared: DeclaredDirs::new(&config.project),
        projects: config.project,
        counts: GateCounts::default(),
        ignore_lines: Vec::new(),
        dirs: Vec::new(),
        region_dirs: Vec::new(),
        gitignores: Vec::new(),
    };
    let pushed = ignores.push_dir(root);
    walk_dir(root, root, &mut ignores, &mut plan);
    if pushed {
        ignores.pop();
    }
    if ignores.files > 0 {
        plan.ignore_lines.push(ignores.marker());
    }
    // LF.3a fired_on: `grep '^\[walk\] config skip'`, attributes file skips
    // (directory skips are the `config=` count of the gate tally).
    if ignores.config_patterns > 0 {
        plan.ignore_lines.push(ignores.config_marker());
    }
    plan
}

/// [`read_walk`]'s per-file reuse: a queued file's prior [`Read`], or `None`
/// to read it. Called on pool workers, so `Sync`; `'a` lets it borrow the
/// caller's file table.
pub(crate) type Reuse<'a> = dyn Fn(&Pending) -> Option<Read> + Sync + 'a;

/// The walk's reads (DC.1, LG.1b): every file `plan` queued, on the engine
/// pool, then one sequential fold into `files` / `md` and the `[contract]
/// json` counters in walk order, the `[[project]]` merge, the explicit roots
/// sort and every tally line. The descent stays sequential ([`plan_walk`]:
/// gitignore layers are a stack); only the reads are pooled.
///
/// `reuse` answers a queued file's prior [`Read`] in place of a read. The
/// caller answers `Some` only from a prior [`ReadRecord`] with the same `rel`
/// AND the same [`PendingClass`] (a re-gated path can change class), and only
/// for a file it knows unchanged: a reused file is stamped fresh, so reusing a
/// file whose recorded stamp no longer matches would hide its change. Re-running
/// the fold over reused reads is exact by construction: a content edit can move
/// a file between `files` and `md` or in and out of `files`, and the fold is
/// what counts the `.json` admissions.
///
/// `stamps` takes each file's [`FileStamp`] BEFORE its read, so a write
/// racing the read leaves a stamp older than the content and the next stat
/// sees the change. The [`ReadRecord`]s, one per queued file in plan order,
/// come back only when `stamps` or `reuse` is asked for (a watch session's
/// file table); a clean read returns none, since keeping them copies every
/// text once.
///
/// fired_on markers, once per call, in this order:
///   `[parallel] walk <root>: read <n> files on <t> threads (md <m>, json <j>, source <s>)`
/// (`n` = `m + j + s`, the queued files by class; a `.md` that falls through to
/// the source rule counts as md), then with `reuse` only
///   `[walk] reuse root=<root> reused=<r> read=<k> of <n>` (`r + k = n`),
/// then the `[roots] declared` lines, `[contract] json sniffed=`, `[contract]
/// json over_cap=`, `[roots] <n> project roots`, the `[walk]` gate tally and
/// the `[walk] gitignore` / `[walk] config skip` lines, each gated on non-zero
/// as before.
pub(crate) fn read_walk(
    root: &Path,
    plan: &WalkPlan,
    reuse: Option<&Reuse<'_>>,
    stamps: bool,
) -> (WalkResult, Vec<ReadRecord>) {
    let (taken, threads) = crate::parallel::par_map_ordered(&plan.pending, |p| {
        let stamp = if stamps { FileStamp::of(&p.abs) } else { None };
        match reuse.and_then(|f| f(p)) {
            Some(read) => (read, stamp, true),
            None => (read_one(p), stamp, false),
        }
    });
    let reused = taken.iter().filter(|(_, _, hit)| *hit).count();
    let mut fold = ReadFold::default();
    let records = if stamps || reuse.is_some() {
        let records: Vec<ReadRecord> = plan
            .pending
            .iter()
            .zip(taken)
            .map(|(p, (read, stamp, _))| ReadRecord {
                rel: p.rel.clone(),
                class: p.class,
                read,
                stamp,
            })
            .collect();
        for r in &records {
            fold.add(&r.rel, r.class, r.read.clone());
        }
        records
    } else {
        for (p, (read, ..)) in plan.pending.iter().zip(taken) {
            fold.add(&p.rel, p.class, read);
        }
        Vec::new()
    };
    let ReadFold { files, md, json, counts } = fold;
    eprintln!(
        "[parallel] walk {}: read {} files on {threads} threads (md {}, json {}, source {})",
        root.display(),
        counts.md + counts.json + counts.source,
        counts.md,
        counts.json,
        counts.source
    );
    // DC.1 fired_on: `grep '^\[walk\] reuse root='`.
    if reuse.is_some() {
        eprintln!(
            "[walk] reuse root={} reused={reused} read={} of {}",
            root.display(),
            plan.pending.len() - reused,
            plan.pending.len()
        );
    }
    let mut roots = plan.roots.clone();
    merge_declared_roots(&plan.projects, &plan.declared, &plan.regions, &mut roots);
    // A10.8 fired_on marker: `... 2>&1 | grep '^\[contract\] json sniffed='`.
    // Gated on non-zero like the other walk lines.
    if json.sniffed > 0 {
        eprintln!("[contract] json sniffed={} admitted={}", json.sniffed, json.admitted);
    }
    if json.over_cap > 0 {
        eprintln!("[contract] json over_cap={} cap_bytes={JSON_CONTRACT_CAP}", json.over_cap);
    }
    // Explicit, although the name-sorted walk already discovers roots in a
    // stable order: A8.5's node order (and so shard bytes) keys on this Vec,
    // and implicit order always leaks (audit 2026-06-10).
    roots.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    if !roots.is_empty() {
        eprintln!("[roots] {}", project_roots::marker(&roots));
    }
    // Attributable collapse tally. Gated on non-zero so a region-free repo stays
    // quiet on the hot path, matching the `[incremental]` / `[gmap]` precedent.
    if plan.counts.total() > 0 {
        eprintln!("[walk] {}", plan.counts.marker());
    }
    for line in &plan.ignore_lines {
        eprintln!("[walk] {line}");
    }
    ((files, plan.regions.clone(), md, roots), records)
}

/// The directories named by `[[project]]` stanzas and whether the walk
/// descended into each (LF.3a). A declared root must be a directory the walk
/// reads: one under a collapsed region or a skipped tree never gets a PROJECT.
struct DeclaredDirs {
    wanted: BTreeSet<String>,
    visited: BTreeSet<String>,
}

impl DeclaredDirs {
    fn new(decls: &[Spanned<ProjectDecl>]) -> Self {
        Self {
            wanted: decls.iter().map(|d| d.get_ref().rel_path().to_string()).collect(),
            visited: BTreeSet::new(),
        }
    }

    /// Record that the walk entered `rel` (the directory's path under the root).
    fn enter(&mut self, rel: &str) {
        if self.wanted.contains(rel) {
            self.visited.insert(rel.to_string());
        }
    }
}

/// The `[roots] declared=` marker's tally.
#[derive(Debug, Default, PartialEq, Eq)]
struct DeclaredCounts {
    added: usize,
    shadowed: usize,
    missing: usize,
}

/// Merge the `[[project]]` roots into the manifest-detected ones (LF.3a). Config
/// extends, never replaces: a detected root at the same path wins (`shadowed`),
/// and a declared path the walk never entered is reported and dropped
/// (`missing`). Called before the explicit sort, which fixes node order.
fn merge_declared_roots(
    decls: &[Spanned<ProjectDecl>],
    declared: &DeclaredDirs,
    regions: &[RegionAnchor],
    roots: &mut Vec<ProjectRoot>,
) -> DeclaredCounts {
    let mut n = DeclaredCounts::default();
    if decls.is_empty() {
        return n;
    }
    for decl in decls {
        let decl = decl.get_ref();
        let rel = decl.rel_path();
        if !declared.visited.contains(rel) {
            eprintln!("[roots] declared root {rel} skipped: {}", missing_reason(rel, regions));
            n.missing += 1;
            continue;
        }
        if let Some(found) = roots.iter().find(|r| r.rel_path == rel) {
            eprintln!(
                "[roots] declared root {rel} shadowed by {} manifest {}",
                found.ecosystem, found.manifest
            );
            n.shadowed += 1;
            continue;
        }
        let root = ProjectRoot::declared(rel.to_string(), decl.label.as_deref());
        if let Some(label) = decl.label.as_deref()
            && label.trim() != root.label
        {
            eprintln!(
                "[roots] declared root {rel} label {label:?} rejected (a label holds no `::`, `${{` or control chars, at most 200 chars): using {:?}",
                root.label
            );
        }
        roots.push(root);
        n.added += 1;
    }
    // LF.3a fired_on: `grep '^\[roots\] declared='`.
    eprintln!("[roots] declared={} shadowed={} missing={}", n.added, n.shadowed, n.missing);
    n
}

/// Why the walk never entered a declared directory: it sits at or under a
/// collapsed region, it is not a directory, or an ancestor was skipped outright
/// (VCS / editor metadata, glia's control dir, the engine's own output).
fn missing_reason(rel: &str, regions: &[RegionAnchor]) -> String {
    let covering = regions.iter().find(|r| {
        rel == r.rel_path || rel.strip_prefix(r.rel_path.as_str()).is_some_and(|t| t.starts_with('/'))
    });
    match covering {
        Some(r) => format!("inside region {} ({})", r.rel_path, r.provenance.provenance()),
        None => "not a directory the walk reads (absent, a file, or under a skipped directory)".to_string(),
    }
}

/// Descend `dir`: gate each directory, detect project roots, collapse
/// regions, record every entered directory, region root and `.gitignore`
/// (DC.1), and queue every file a rule reads into `plan.pending` (LG.1b: the
/// reads themselves run afterwards, in [`read_walk`]).
fn walk_dir(root: &Path, dir: &Path, ignores: &mut IgnoreStack, plan: &mut WalkPlan) {
    // Stat before the listing, as a file's stamp precedes its read: an entry
    // added after the stat moves the mtime past the recorded one, whether or
    // not the listing caught it.
    let mtime = mtime_of(dir);
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let dir_rel = dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().to_string();
    plan.declared.enter(&dir_rel);
    // Sort by name: read_dir yields filesystem/inode order, which leaked into
    // node/edge Vec order (and so shard bytes) — stable-ish on one machine,
    // not reproducible across machines or after file churn (audit 2026-06-10).
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_unstable_by_key(|e| e.file_name());
    // Project-root detection rides the walk (A8.4): it scans the entry names
    // already in hand, and a collapsed region (`node_modules/*/package.json`)
    // is never visited, so it can never become a root.
    let names: Vec<String> = entries
        .iter()
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    if names.iter().any(|n| n == ".gitignore") {
        plan.gitignores.push(match dir_rel.as_str() {
            "" => ".gitignore".to_string(),
            rel => format!("{rel}/.gitignore"),
        });
    }
    if let Some((ecosystem, manifest, label)) = project_roots::detect_root(&names, |base| {
        std::fs::read_to_string(dir.join(base)).unwrap_or_default()
    }) {
        plan.roots.push(ProjectRoot::new(dir_rel.clone(), ecosystem, &manifest, label));
    }
    plan.dirs.push(WalkedDir { rel: dir_rel, mtime });
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // Also guards FILES: a `.git`/`.hg` file must never be read as source.
        if walk_gating::is_hard_skip(&name) {
            continue;
        }
        if path.is_dir() {
            if is_self_output(root, dir, &name) {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                eprintln!("[walk] skipped engine output {}", rel.display());
                continue;
            }
            // Collapse vendored / build / gitignored / other-repo directories to
            // one anchor and do NOT descend — categorise the region instead of
            // dropping it or emitting a node per file inside. The rules live in
            // `code_domain::walk_gating` so `store::is_gmap_stale` scans exactly
            // this tree. (glia-v2 G1/G2/G10, A8.1)
            let gate = walk_gating::gate_dir(
                &path,
                &name,
                ignores.is_ignored(&path, true),
                ignores.is_excluded(&path, true),
            );
            plan.counts.record(gate);
            if let Some(provenance) = gate.collapse() {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().to_string();
                plan.region_dirs.push(WalkedDir {
                    rel: rel.clone(),
                    mtime: mtime_of(&path),
                });
                plan.regions.push(RegionAnchor {
                    rel_path: rel,
                    provenance,
                    region: name,
                });
                continue;
            }
            if gate == Gate::HardSkip {
                continue;
            }
            let pushed = ignores.push_dir(&path);
            walk_dir(root, &path, ignores, plan);
            if pushed {
                ignores.pop();
            }
        } else if path.is_file() {
            // File-level gitignore: committed-but-ignored output (`*.min.js`,
            // `*_pb2.py`) beside authored source never reaches a parser. Before
            // the markdown branch, so an ignored doc is not ingested either.
            // A user `[walk] skip` file (LF.3a) is dropped the same way, and
            // first, like its directory verdict in `gate_dir`.
            if ignores.exclude_file(&path) || ignores.skip_file(&path) {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().to_string();
            let lower = rel_str.to_ascii_lowercase();
            let source_rule = || detect_language(&rel_str).is_some() || is_bypass_path(&rel_str);
            // Markdown docs (G18) — collected separately; the include/skip rules
            // are applied in `build_docs_graph`. An oversize or non-UTF-8 doc
            // falls through to the source rule (`PendingClass::Markdown`).
            if lower.ends_with(".md") {
                let class = PendingClass::Markdown {
                    source_fallback: source_rule(),
                };
                plan.pending.push(Pending { rel: rel_str, abs: path, class });
                continue;
            }
            // A10.8: a `.json` is read only to be sniffed, and queued only when
            // it is an API contract (OpenAPI/Swagger, AsyncAPI, Pact), since
            // LA.16 (A10.12) a JSON Schema, or since CL.7b a .NET
            // `appsettings*.json` (by path). Lock files, tsconfig and test data
            // are read once, dropped by `read_walk`'s fold, and never kept.
            // `package.json` / `composer.json` are manifests (`is_bypass_path`)
            // and keep their own route below. Anything under a collapsed region
            // (node_modules, dist, ...) is never reached.
            if lower.ends_with(".json") && !is_bypass_path(&rel_str) {
                plan.pending.push(Pending {
                    rel: rel_str,
                    abs: path,
                    class: PendingClass::JsonSniff,
                });
                continue;
            }
            // A13.9: `is_bypass_path` admits a migration `.sql`; one over the
            // cap is skipped, and says so.
            if glia_code_extractors::migrations::is_migration_path(&rel_str)
                && std::fs::metadata(&path).is_ok_and(|m| m.len() > MIGRATION_SQL_CAP)
            {
                eprintln!("[migrations] skipped file={rel_str} over_cap={MIGRATION_SQL_CAP}");
                continue;
            }
            if source_rule() {
                plan.pending.push(Pending {
                    rel: rel_str,
                    abs: path,
                    class: PendingClass::Source,
                });
            }
        }
    }
}

/// Build a one-node-per-region graph from the collapsed [`RegionAnchor`]s. Each
/// node carries an `ORIGIN` cell `{provenance, region}` so consumers filter by
/// coordinate instead of string-matching keys. (glia-v2 G1/G10)
pub(crate) fn build_region_graph(regions: &[RegionAnchor], repo: RepoId) -> glia_graph::RepoGraph {
    use glia_code_domain::cell_type;
    use glia_core::{Cell, CellPayload};

    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    for r in regions {
        let qname = format!("region:{}", r.rel_path);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::REGION, &qname);
        let origin = format!(
            r#"{{"provenance":"{}","region":"{}"}}"#,
            r.provenance.provenance(),
            r.region
        );
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(origin),
            }],
        });
        nav.record(id, &r.region, &qname, node_kind::REGION, None);
    }
    glia_graph::RepoGraph {
        repo,
        nodes,
        edges: Vec::new(),
        nav,
        symbols: Default::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    }
}

/// The qname of a detected project root: `project:<rel_path>`, with `.` for the
/// repo root. An empty tail would be ambiguous. A walked relative path never
/// contains `::`, so the locked separator never appears inside the qname. (A8.5)
pub(crate) fn project_qname(rel_path: &str) -> String {
    format!("project:{}", if rel_path.is_empty() { "." } else { rel_path })
}

/// Build one edge-less `PROJECT` node per detected [`ProjectRoot`] (A8.5). It
/// follows the same pattern as [`build_region_graph`]. The nav NAME is the
/// manifest label (`@shop/web`), which is what `--scope <label>` and service
/// labelling look up. The qname is derived from the path, so it stays stable
/// when a label changes. The single `ORIGIN` cell is
/// `{provenance:"project_root", ecosystem, manifest, label, path}`. Because the
/// nodes have ZERO edges, blast radius, trace and liveness cannot fan out
/// through them.
pub(crate) fn build_project_graph(roots: &[ProjectRoot], repo: RepoId) -> glia_graph::RepoGraph {
    use glia_code_domain::cell_type;
    use glia_core::{Cell, CellPayload};

    let mut nodes = Vec::with_capacity(roots.len());
    let mut nav = CodeNav::default();
    for r in roots {
        let qname = project_qname(&r.rel_path);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PROJECT, &qname);
        // `json!` rather than a format string: a label is manifest text, and a
        // quote in it must not produce malformed JSON.
        let origin = serde_json::json!({
            "provenance": "project_root",
            "ecosystem": r.ecosystem,
            "manifest": r.manifest,
            "label": r.label,
            "path": r.rel_path,
        })
        .to_string();
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(origin),
            }],
        });
        nav.record(id, &r.label, &qname, node_kind::PROJECT, None);
    }
    eprintln!("[roots] emitted {} PROJECT nodes", nodes.len());
    glia_graph::RepoGraph {
        repo,
        nodes,
        edges: Vec::new(),
        nav,
        symbols: Default::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    }
}

fn is_bypass_path(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if matches!(ext, "yml" | "yaml") {
        return true;
    }
    is_dockerfile_path(path)
        || is_dotenv_path(path)
        || is_angular_template_path(path)
        || glia_code_extractors::packages::is_manifest_path(path)
        // A13.9: a migration `.sql` (Flyway name, `.up.sql`, `db/migrate/`,
        // `migrations/`). Any other `.sql` stays unread.
        || glia_code_extractors::migrations::is_migration_path(path)
        // A13.16: a Prisma schema (`schema.prisma`, or any `.prisma` of a
        // prismaSchemaFolder), the file that declares the stack's data model.
        || glia_code_extractors::prisma::is_prisma_schema(path)
}

/// LA.6c: an Angular CLI component template (`home.component.html`). Read only
/// for its navigation links (`route.rs`); a plain `.html` page stays out.
pub(crate) fn is_angular_template_path(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.len() > ".component.html".len() && base.ends_with(".component.html")
}

pub(crate) fn is_dockerfile_path(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    base == "dockerfile"
        || base.ends_with(".dockerfile")
        || base.starts_with("dockerfile.")
}

pub(crate) fn is_dotenv_path(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    base == ".env" || base.starts_with(".env.")
}

#[cfg(test)]
mod walk_tests {
    use super::*;

    #[test]
    fn hashed_chunk_detection() {
        assert!(walk_gating::is_hashed_chunk("main.e188fddd19255ba1.js"));
        assert!(walk_gating::is_hashed_chunk("1624.4e9cc6119b4878fe.js"));
        assert!(walk_gating::is_hashed_chunk("styles.0a1b2c3d4e5f6a7b.css"));
        // Authored source is not a hashed chunk.
        assert!(!walk_gating::is_hashed_chunk("app.component.ts"));
        assert!(!walk_gating::is_hashed_chunk("index.js"));
        assert!(!walk_gating::is_hashed_chunk("user_service.py"));
        // 8-hex-ish word but wrong extension.
        assert!(!walk_gating::is_hashed_chunk("deadbeef.txt"));
    }

    /// Git's `.gitignore` semantics end to end through the builder walk (A8.2).
    /// Replaces `gitignore_dir_parsing`, which pinned the old final-component
    /// match that collapsed every `generated` directory anywhere in the tree.
    #[test]
    fn gitignore_matcher_semantics() {
        let root = std::env::temp_dir().join(format!("glia_gi_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["gen", "src/gen", "dist-x", "dist-keep", "src/generated", "notes/generated"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join(".gitignore"), "/gen\ndist-*\n!dist-keep\n*.min.js\n").unwrap();
        std::fs::write(root.join("src/.gitignore"), "generated/\n").unwrap();
        for f in [
            "app.js",
            "vendor.min.js",
            "gen/g.py",
            "src/gen/builder.py",
            "dist-x/d.py",
            "dist-keep/k.py",
            "src/generated/out.py",
            "notes/generated/n.py",
        ] {
            std::fs::write(root.join(f), "x = 1\n").unwrap();
        }

        let (files, regions, _md, _roots) = walk_source_files(&root);
        let parsed: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        let region_paths: Vec<&str> = regions.iter().map(|r| r.rel_path.as_str()).collect();

        // Anchoring: `/gen` collapses the root `gen` only.
        assert!(region_paths.contains(&"gen"), "{region_paths:?}");
        assert!(parsed.contains(&"src/gen/builder.py"), "{parsed:?}");
        // Globs: `dist-*` collapses `dist-x`.
        assert!(region_paths.contains(&"dist-x"), "{region_paths:?}");
        assert!(!parsed.contains(&"dist-x/d.py"), "{parsed:?}");
        // Negation: `!dist-keep` stays source.
        assert!(parsed.contains(&"dist-keep/k.py"), "{parsed:?}");
        // Nested file: `src/.gitignore` collapses `src/generated`...
        assert!(region_paths.contains(&"src/generated"), "{region_paths:?}");
        // ...and does not leak upward to a sibling tree.
        assert!(parsed.contains(&"notes/generated/n.py"), "{parsed:?}");
        assert!(!region_paths.contains(&"notes/generated"), "{region_paths:?}");
        // File-level: `*.min.js` drops the bundle beside the authored file.
        assert!(parsed.contains(&"app.js"), "{parsed:?}");
        assert!(!parsed.contains(&"vendor.min.js"), "{parsed:?}");
        // Exactly the four sources above, and gitignore-attributed regions only.
        assert_eq!(parsed.len(), 4, "{parsed:?}");
        assert_eq!(regions.len(), 3, "{region_paths:?}");
        assert!(regions.iter().all(|r| r.provenance == Collapse::BuildOutput));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn walk_collapses_build_and_vendor_regions() {
        let root = std::env::temp_dir().join(format!("glia_walk_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/left-pad")).unwrap();
        std::fs::create_dir_all(root.join("www")).unwrap();
        std::fs::create_dir_all(root.join("android/app/src/main/assets/public")).unwrap();
        std::fs::write(root.join(".gitignore"), "/www\n").unwrap();
        // authored source
        std::fs::write(root.join("src/app/a.ts"), "export class A {}").unwrap();
        // vendored dep (must NOT be parsed)
        std::fs::write(root.join("node_modules/left-pad/index.js"), "module.exports=1").unwrap();
        // gitignored build mirror
        std::fs::write(root.join("www/main.abc12345def0.js"), "var a=1").unwrap();
        // capacitor bundle mirror NOT in .gitignore — caught by hash-chunk probe
        let pub_dir = root.join("android/app/src/main/assets/public");
        for h in ["1624.4e9cc6119b4878fe", "1102.7837812dd7ed4d51", "2075.f756d5b13b56050a"] {
            std::fs::write(pub_dir.join(format!("{h}.js")), "var b=2").unwrap();
        }

        let (files, regions, _md, _roots) = walk_source_files(&root);

        // Only the authored source file is queued for parsing.
        assert_eq!(files.len(), 1, "files: {files:?}");
        assert!(files[0].0.ends_with("a.ts"));

        let region_paths: Vec<&str> = regions.iter().map(|r| r.rel_path.as_str()).collect();
        assert!(region_paths.contains(&"node_modules"), "{region_paths:?}");
        assert!(region_paths.contains(&"www"), "{region_paths:?}");
        assert!(
            region_paths.contains(&"android/app/src/main/assets/public"),
            "hash-chunk bundle mirror should collapse: {region_paths:?}"
        );
        // node_modules is vendored; the bundle mirrors are build_output.
        let nm = regions.iter().find(|r| r.rel_path == "node_modules").unwrap();
        assert_eq!(nm.provenance, Collapse::Vendored);
        let pub_region = regions
            .iter()
            .find(|r| r.rel_path.ends_with("assets/public"))
            .unwrap();
        assert_eq!(pub_region.provenance, Collapse::BuildOutput);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Fresh temp root per test name; `walk_collapses_*` already owns the
    /// pid-suffixed `glia_walk_` prefix, so these take their own.
    fn walk_tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("glia_walk_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn dotnet_bin_obj_collapse_next_to_a_project() {
        let root = walk_tmp("dotnet");
        std::fs::create_dir_all(root.join("obj/Debug/net8.0")).unwrap();
        std::fs::create_dir_all(root.join("bin/Debug")).unwrap();
        std::fs::write(root.join("Api.csproj"), "<Project/>").unwrap();
        std::fs::write(root.join("Program.cs"), "public class Startup {}").unwrap();
        std::fs::write(root.join("obj/project.assets.json"), "{}").unwrap();
        std::fs::write(
            root.join("obj/Debug/net8.0/Api.AssemblyInfo.cs"),
            "internal sealed class PhantomAssemblyInfo {}",
        )
        .unwrap();
        std::fs::write(root.join("bin/Debug/Api.g.cs"), "internal sealed class PhantomBinClass {}")
            .unwrap();

        let (files, regions, _md, _roots) = walk_source_files(&root);

        // PRECISION: the generated C# never reaches a parser. grade.py has no
        // expect_absent_nodes, so this half of the fix is provable only here.
        assert!(
            files.iter().all(|(p, _)| !p.starts_with("obj/") && !p.starts_with("bin/")),
            "generated .NET output must not be parsed: {files:?}"
        );
        assert!(files.iter().any(|(p, _)| p == "Program.cs"), "{files:?}");
        for dir in ["obj", "bin"] {
            let r = regions
                .iter()
                .find(|r| r.rel_path == dir)
                .unwrap_or_else(|| panic!("no region for {dir}: {:?}", regions.len()));
            assert_eq!(r.provenance, Collapse::BuildOutput);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dotnet_gate_requires_a_project_sibling() {
        let root = walk_tmp("nodotnet");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/console_entry.py"), "x = 1\n").unwrap();

        let (files, regions, _md, _roots) = walk_source_files(&root);

        // A Python repo's `bin/` of console scripts is authored source.
        assert!(files.iter().any(|(p, _)| p == "bin/console_entry.py"), "{files:?}");
        assert!(regions.is_empty(), "no project file => no collapse: {}", regions.len());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nested_git_becomes_a_region() {
        let root = walk_tmp("nested");
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::create_dir_all(root.join("libs/sdk")).unwrap();
        std::fs::write(root.join("app/main.py"), "def run():\n    pass\n").unwrap();
        std::fs::write(root.join("libs/sdk/.git"), "gitdir: ../../.git/modules/sdk\n").unwrap();
        std::fs::write(root.join("libs/sdk/vendor_client.py"), "class VendorClient:\n    pass\n")
            .unwrap();

        let (files, regions, _md, _roots) = walk_source_files(&root);

        // The submodule's symbols must not be minted into the PARENT RepoId —
        // that is the duplicate-name pair `resolve_name` had to pick between.
        assert!(
            files.iter().all(|(p, _)| !p.starts_with("libs/sdk/")),
            "submodule source must not be parsed into the parent: {files:?}"
        );
        assert!(files.iter().any(|(p, _)| p == "app/main.py"), "{files:?}");
        let r = regions.iter().find(|r| r.rel_path == "libs/sdk").unwrap();
        assert_eq!(r.provenance, Collapse::Submodule);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A13.9: a migration `.sql` is read and a query fixture `.sql` is not;
    /// the admitted file's DDL becomes a DATA_ENTITY under its own MODULE, and
    /// one over the cap is never read.
    #[test]
    fn walk_admits_migration_sql_only() {
        let root = walk_tmp("migrations");
        for d in ["db/migrations", "db/migrate", "tests/data", "big/migrations"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let files_in = [
            ("db/migrations/V1__create_users.sql", "CREATE TABLE users (id INT);\n"),
            ("db/migrate/002_orders.sql", "ALTER TABLE orders ADD COLUMN note TEXT;\n"),
            ("tests/data/q.sql", "SELECT * FROM fixtures_only;\n"),
            ("report.sql", "CREATE TABLE not_a_migration (id INT);\n"),
        ];
        for (rel, body) in files_in {
            std::fs::write(root.join(rel), body).unwrap();
        }
        let pad = "-- pad\n".repeat(MIGRATION_SQL_CAP as usize / 7 + 1);
        std::fs::write(
            root.join("big/migrations/001_load.sql"),
            format!("CREATE TABLE huge (id INT);\n{pad}"),
        )
        .unwrap();

        let (files, ..) = walk_source_files(&root);
        let mut sql: Vec<&str> = files
            .iter()
            .map(|(p, _)| p.as_str())
            .filter(|p| p.ends_with(".sql"))
            .collect();
        sql.sort_unstable();
        assert_eq!(
            sql,
            ["db/migrate/002_orders.sql", "db/migrations/V1__create_users.sql"],
            "{files:?}"
        );

        let r = crate::build::generate_one(root.to_str().unwrap()).unwrap();
        let mut accessed: Vec<(String, String)> = Vec::new();
        for g in &r.merged.graphs {
            for e in &g.edges {
                if e.category != glia_code_domain::edge_category::ACCESSES_DATA {
                    continue;
                }
                let from = g.nav.qname_by_id.get(&e.from).cloned().unwrap_or_default();
                let to = g.nav.qname_by_id.get(&e.to).cloned().unwrap_or_default();
                accessed.push((from, to));
            }
        }
        accessed.sort();
        assert_eq!(
            accessed,
            [
                ("db::migrate::002_orders.sql".to_string(), "data_entity:sql:orders".to_string()),
                (
                    "db::migrations::V1__create_users.sql".to_string(),
                    "data_entity:sql:users".to_string()
                ),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A13.16: a `.prisma` schema is read (it matches no language) and a
    /// backup beside it is not.
    #[test]
    fn walk_admits_prisma_schema() {
        let root = walk_tmp("prisma");
        std::fs::create_dir_all(root.join("prisma")).unwrap();
        for rel in ["prisma/schema.prisma", "prisma/schema.prisma.bak"] {
            std::fs::write(root.join(rel), "model User {\n  id Int @id\n}\n").unwrap();
        }
        let (files, ..) = walk_source_files(&root);
        let read: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(read, ["prisma/schema.prisma"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A10.8 precision: `.json` is read and sniffed, and only an API contract
    /// is queued. The lock file, tsconfig and a substrate-gap key.json are
    /// dropped. The manifest keeps its own route, and a contract inside a
    /// collapsed region is never reached. grade.py has no expect_absent, so
    /// this test is the gate.
    #[test]
    fn walk_admits_only_sniffed_contract_json() {
        let root = walk_tmp("json");
        std::fs::create_dir_all(root.join("node_modules/swagger-ui")).unwrap();
        std::fs::create_dir_all(root.join("big")).unwrap();
        let files_in = [
            ("openapi.json", r#"{"openapi":"3.0.3","paths":{"/users":{"get":{}}}}"#),
            (
                "package-lock.json",
                r#"{"name":"shop","lockfileVersion":3,"packages":{"":{"dependencies":{"swagger-ui":"5"}}}}"#,
            ),
            ("tsconfig.json", r#"{"compilerOptions":{"paths":{"@app/*":["src/app/*"]}}}"#),
            (
                "key.json",
                r#"{"framework":"contract-pact","language":"json+python","dirs":["."],"expect_nodes":[{"kind":"DOC_SECTION","name":"GET /users","note":"openapi"}]}"#,
            ),
            ("package.json", r#"{"name":"shop","dependencies":{"flask-openapi":"1"}}"#),
            ("node_modules/swagger-ui/swagger.json", r#"{"swagger":"2.0","paths":{}}"#),
        ];
        for (rel, body) in files_in {
            std::fs::write(root.join(rel), body).unwrap();
        }

        let (files, regions, _md, _roots) = walk_source_files(&root);
        let json: Vec<&str> = files
            .iter()
            .map(|(p, _)| p.as_str())
            .filter(|p| p.ends_with(".json") && *p != "package.json")
            .collect();
        assert_eq!(json, ["openapi.json"], "exactly one of four candidate .json admitted: {files:?}");
        assert!(files.iter().any(|(p, _)| p == "package.json"), "the manifest keeps its own route");
        assert!(regions.iter().any(|r| r.rel_path == "node_modules"));

        // Over the cap: a real contract, but never read.
        let pad = "x".repeat(JSON_CONTRACT_CAP as usize);
        std::fs::write(
            root.join("big/swagger.json"),
            format!(r#"{{"swagger":"2.0","paths":{{}},"x-pad":"{pad}"}}"#),
        )
        .unwrap();
        let (files, ..) = walk_source_files(&root);
        assert!(files.iter().all(|(p, _)| p != "big/swagger.json"), "over-cap json is skipped");

        // End to end: the admitted contract becomes a DOC_SECTION under its
        // own MODULE, and nothing else in the tree does.
        let r = crate::build::generate_one(root.to_str().unwrap()).unwrap();
        let docs: Vec<String> = r
            .merged
            .graphs
            .iter()
            .flat_map(|g| {
                g.nodes
                    .iter()
                    .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::DOC_SECTION))
                    .map(move |n| g.nav.qname_by_id[&n.id].clone())
            })
            .collect();
        assert_eq!(docs, ["contract::openapi::GET:/users"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// CL.7b: an `appsettings*.json` is admitted by its path (it never sniffs
    /// as a contract or schema); its look-alikes are still sniffed and dropped.
    #[test]
    fn walk_admits_dotnet_settings_json() {
        let root = walk_tmp("appsettings");
        std::fs::create_dir_all(root.join("src/Api/Properties")).unwrap();
        let files_in = [
            ("src/Api/appsettings.json", r#"{"Stripe":{"SecretKey":"sk_test_x"}}"#),
            ("src/Api/appsettings.Development.json", r#"{"Logging":{"LogLevel":{"Default":"Debug"}}}"#),
            ("src/Api/Properties/launchSettings.json", r#"{"profiles":{"Api":{"commandName":"Project"}}}"#),
            ("src/Api/mysettings.json", r#"{"Stripe":{"SecretKey":"sk_test_y"}}"#),
        ];
        for (rel, body) in files_in {
            std::fs::write(root.join(rel), body).unwrap();
        }
        let (files, ..) = walk_source_files(&root);
        let json: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(json, ["src/Api/appsettings.Development.json", "src/Api/appsettings.json"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// LA.16 (A10.12): a JSON Schema is admitted beside the contracts. The
    /// sniff is only a gate, so a data file whose NESTED object looks like a
    /// schema is admitted too, and the build then mints nothing from it; data
    /// that does not even look schema-shaped is dropped at the walk.
    #[test]
    fn walk_admits_json_schema_and_rejects_lookalike_data() {
        let root = walk_tmp("jsonschema");
        for d in ["schemas", "testdata", "config", "data"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let files_in = [
            (
                "schemas/user.schema.json",
                r##"{"$schema":"http://json-schema.org/draft-07/schema#","title":"User","type":"object",
 "properties":{"address":{"$ref":"#/definitions/Address"}},
 "definitions":{"Address":{"type":"object","properties":{"street":{"type":"string"}}}}}"##,
            ),
            ("schemas/refund.json", r#"{"type":"object","properties":{"refundId":{"type":"string"}}}"#),
            (
                "testdata/settings.json",
                r#"{"name":"billing","config":{"type":"object","properties":{"retries":3}}}"#,
            ),
            ("config/app.json", r#"{"$schema":"https://json.schemastore.org/app","type":"module"}"#),
            ("data/event.json", r#"{"type":"object","id":7}"#),
        ];
        for (rel, body) in files_in {
            std::fs::write(root.join(rel), body).unwrap();
        }

        let (files, ..) = walk_source_files(&root);
        let json: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            json,
            ["schemas/refund.json", "schemas/user.schema.json", "testdata/settings.json"],
            "the two schemas and the nested look-alike pass the gate; the rest never do"
        );

        let r = crate::build::generate_one(root.to_str().unwrap()).unwrap();
        let mut types: Vec<String> = Vec::new();
        let mut settings_nodes = 0usize;
        for g in &r.merged.graphs {
            for n in &g.nodes {
                let q = g.nav.qname_by_id.get(&n.id).map(String::as_str).unwrap_or("");
                if g.nav.kind_by_id.get(&n.id) == Some(&node_kind::MESSAGE_TYPE) {
                    types.push(q.to_string());
                }
                if q.contains("settings") {
                    settings_nodes += 1;
                }
            }
        }
        types.sort_unstable();
        assert_eq!(
            types,
            ["message:jsonschema:User", "message:jsonschema:User.Address", "message:jsonschema:refund"]
        );
        assert_eq!(settings_nodes, 0, "the look-alike mints no MESSAGE_TYPE and no MODULE");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// LA.6c: an Angular `*.component.html` template is walked (for its
    /// navigation links); a plain `.html` page is not.
    #[test]
    fn walk_admits_component_templates_only() {
        let root = walk_tmp("templates");
        std::fs::create_dir_all(root.join("src/app/home")).unwrap();
        for (rel, body) in [
            ("src/index.html", "<base href=\"/\">"),
            (
                "src/app/home/home.component.html",
                "<a routerLink=\"/home\">h</a>",
            ),
            (
                "src/app/home/home.component.ts",
                "export class HomeComponent {}",
            ),
            ("src/app/.component.html", "<a routerLink=\"/x\">x</a>"),
        ] {
            std::fs::write(root.join(rel), body).unwrap();
        }
        let (files, ..) = walk_source_files(&root);
        let html: Vec<&str> = files
            .iter()
            .map(|(p, _)| p.as_str())
            .filter(|p| p.ends_with(".html"))
            .collect();
        assert_eq!(html, ["src/app/home/home.component.html"]);
        assert!(is_angular_template_path("a/b/user-list.component.html"));
        assert!(!is_angular_template_path("a/b/index.html"));
        assert!(!is_angular_template_path("a/b/component.html"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A8.4 acceptance: detection rides the walk, so the vendored manifest under
    /// the collapsed `node_modules` is never a root, and roots come back sorted.
    #[test]
    fn project_roots_ride_the_walk() {
        let root = walk_tmp("roots");
        for d in ["apps/web", "services/api", "node_modules/left-pad", "libs/plain"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("package.json"), r#"{"name":"@shop/monorepo"}"#).unwrap();
        std::fs::write(root.join("apps/web/package.json"), r#"{"private":true}"#).unwrap();
        std::fs::write(root.join("apps/web/index.ts"), "export const x = 1;\n").unwrap();
        std::fs::write(root.join("services/api/go.mod"), "module github.com/shop/api\n\ngo 1.22\n").unwrap();
        std::fs::write(root.join("services/api/main.go"), "package main\n").unwrap();
        std::fs::write(root.join("node_modules/left-pad/package.json"), r#"{"name":"left-pad"}"#).unwrap();
        std::fs::write(root.join("libs/plain/util.py"), "x = 1\n").unwrap();

        let (_files, regions, _md, roots) = walk_source_files(&root);

        let rels: Vec<&str> = roots.iter().map(|r| r.rel_path.as_str()).collect();
        assert_eq!(rels, ["", "apps/web", "services/api"]);
        let got: Vec<(&str, &str, &str)> =
            roots.iter().map(|r| (r.ecosystem, r.label.as_str(), r.manifest.as_str())).collect();
        assert_eq!(
            got,
            [
                ("npm", "@shop/monorepo", "package.json"),
                ("npm", "web", "apps/web/package.json"),
                ("go", "github.com/shop/api", "services/api/go.mod"),
            ]
        );
        assert!(regions.iter().any(|r| r.rel_path == "node_modules"));
        assert_eq!(project_roots::marker(&roots), "3 project roots (go=1 npm=2)");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A8.5: each detected root is ONE PROJECT node, labelled by its manifest
    /// and keyed by its path. No edge touches it, so blast radius never
    /// returns it, and a function that shares its label still wins name
    /// resolution. `generate_one` and `generate_many` put the project graph at
    /// the same shard index.
    #[test]
    fn project_roots_become_edgeless_project_nodes() {
        use std::collections::HashSet;

        use crate::answers::{BlastOptions, blast_radius};
        use crate::build::{generate_many, generate_one};

        let root = walk_tmp("projects");
        for d in ["apps/web", "services/api", "libs/core/src", "node_modules/left-pad"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let files = [
            ("package.json", r#"{"name":"shop-monorepo","private":true}"#),
            ("README.md", "# Shop\n\nThe `shop-core` crate and the `apisvc` module.\n"),
            ("apps/web/package.json", r#"{"name":"@shop/web"}"#),
            ("apps/web/index.ts", "export function webEntry(): number { return 1; }\n"),
            // The Go module label `apisvc` is also the name of a function.
            ("services/api/go.mod", "module apisvc\n\ngo 1.22\n"),
            ("services/api/main.go", "package main\n\nfunc main() { apisvc() }\n\nfunc apisvc() {}\n"),
            ("libs/core/Cargo.toml", "[package]\nname = \"shop-core\"\n"),
            ("libs/core/src/lib.rs", "pub fn core_fn() {}\n"),
            ("node_modules/left-pad/package.json", r#"{"name":"left-pad"}"#),
        ];
        for (rel, body) in files {
            std::fs::write(root.join(rel), body).unwrap();
        }
        let path = root.to_str().unwrap().to_string();
        let r = generate_one(&path).unwrap();
        let m = &r.merged;

        let mut projects: Vec<(String, String, NodeId, String)> = m
            .graphs
            .iter()
            .flat_map(|g| {
                g.nodes
                    .iter()
                    .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::PROJECT))
                    .map(move |n| {
                        let origin = match &n.cells[..] {
                            [c] if c.kind == glia_code_domain::cell_type::ORIGIN => {
                                match &c.payload {
                                    glia_core::CellPayload::Json(j) => j.clone(),
                                    other => panic!("ORIGIN must be JSON, got {other:?}"),
                                }
                            }
                            other => panic!("exactly one ORIGIN cell, got {other:?}"),
                        };
                        (g.nav.qname_by_id[&n.id].clone(), g.nav.name_by_id[&n.id].clone(), n.id, origin)
                    })
            })
            .collect();
        projects.sort_by(|a, b| a.0.cmp(&b.0));
        let got: Vec<(&str, &str)> =
            projects.iter().map(|(q, n, ..)| (q.as_str(), n.as_str())).collect();
        assert_eq!(
            got,
            [
                ("project:.", "shop-monorepo"),
                ("project:apps/web", "@shop/web"),
                ("project:libs/core", "shop-core"),
                ("project:services/api", "apisvc"),
            ],
            "one PROJECT per root; the vendored node_modules manifest is not one"
        );
        assert!(projects.iter().all(|(q, ..)| !q.contains("::")), "{got:?}");
        assert_eq!(project_qname(""), "project:.");

        let origin: serde_json::Value = serde_json::from_str(&projects[0].3).unwrap();
        assert_eq!(
            origin,
            serde_json::json!({
                "provenance": "project_root",
                "ecosystem": "npm",
                "manifest": "package.json",
                "label": "shop-monorepo",
                "path": "",
            })
        );

        // The literal substrings that walk-project-roots/key.json's
        // expect_cells grade.
        let origin_of = |q: &str| projects.iter().find(|p| p.0 == q).map(|p| p.3.as_str()).unwrap();
        assert!(origin_of("project:.").contains(r#""manifest":"package.json""#));
        assert!(origin_of("project:libs/core").contains(r#""provenance":"project_root""#));
        assert!(origin_of("project:services/api").contains(r#""ecosystem":"go""#));

        // No edge touches a PROJECT node, including the doc-linker's DOCUMENTS
        // edges from a README that names two of the labels.
        let ids: HashSet<NodeId> = projects.iter().map(|p| p.2).collect();
        let touching: Vec<_> = m
            .all_edges()
            .filter(|e| ids.contains(&e.from) || ids.contains(&e.to))
            .collect();
        assert!(touching.is_empty(), "{touching:?}");

        // Blast radius from a nearby function never includes a PROJECT node.
        // The function `apisvc` shares a label with a project but has a CALLS
        // edge, and PROJECT has degree 0, so name resolution picks the
        // function.
        let func = m.resolve_name("apisvc").unwrap();
        assert!(!ids.contains(&func), "the degree-0 PROJECT must lose pick_primary");
        for seed in ["apisvc", "main", "core_fn", "webEntry"] {
            let hits = blast_radius(m, &[seed], &BlastOptions::default()).results;
            assert!(hits.iter().all(|h| h.kind != "PROJECT"), "{seed}: {:?}",
                hits.iter().map(|h| &h.qname).collect::<Vec<_>>());
        }
        let apisvc = blast_radius(m, &["apisvc"], &BlastOptions::default()).results;
        assert!(!apisvc.is_empty(), "the control seed has a real neighbour (main)");

        // Same shard slot from both entry points (regions, projects, docs).
        let slot = |m: &glia_graph::MergedGraph| {
            m.graphs.iter().position(|g| {
                g.nodes.first().is_some_and(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::PROJECT))
            })
        };
        let many = generate_many(std::slice::from_ref(&path)).unwrap();
        assert_eq!(slot(m), slot(&many.merged));
        assert!(slot(m).is_some());
        assert_eq!(m.graphs.len(), many.merged.graphs.len());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The A8.2 hand-off, LC.9: the engine's own `<root>/.glia/graph` and the
    /// legacy `<root>/.ai/repo-graph` are neither walked nor a region,
    /// gitignored or not, while the rest of `.ai` is. LF.1d: `.glia` is
    /// glia's control dir, never walked at any depth.
    #[test]
    fn engine_output_dir_is_skipped() {
        let root = walk_tmp("selfout");
        for d in [".glia/graph", ".ai/repo-graph", "pkg/.glia/graph", "pkg/.ai/repo-graph"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join(".gitignore"), "**/.glia/graph/\n**/.ai/repo-graph/\n").unwrap();
        std::fs::write(root.join(".glia/graph/leak.py"), "x = 1\n").unwrap();
        std::fs::write(root.join(".ai/repo-graph/leak.py"), "x = 1\n").unwrap();
        std::fs::write(root.join(".ai/notes.md"), "# Notes\n").unwrap();
        std::fs::write(root.join(".glia/notes.md"), "# Glia notes\n").unwrap();
        std::fs::write(root.join("pkg/.glia/graph/other.py"), "y = 2\n").unwrap();
        std::fs::write(root.join("pkg/.ai/repo-graph/other.py"), "y = 2\n").unwrap();

        let (files, regions, md, _roots) = walk_source_files(&root);

        let region_paths: Vec<&str> = regions.iter().map(|r| r.rel_path.as_str()).collect();
        assert!(!region_paths.contains(&".glia/graph"), "{region_paths:?}");
        assert!(!region_paths.contains(&".ai/repo-graph"), "{region_paths:?}");
        let own = |p: &str| p.starts_with(".glia/graph/") || p.starts_with(".ai/repo-graph/");
        assert!(files.iter().all(|(p, _)| !own(p)), "{files:?}");
        assert!(md.iter().any(|(p, _)| p == ".ai/notes.md"), "authored .ai is still walked");
        assert!(md.iter().all(|(p, _)| !p.contains(".glia")), "LF.1d: .glia is never walked: {md:?}");
        // Only the ROOT legacy copy is ours; a nested one stays under the
        // usual gates. A nested `.glia` is the control dir too: no region.
        assert_eq!(region_paths, ["pkg/.ai/repo-graph"]);

        // Without a gitignore they are still never parsed.
        std::fs::remove_file(root.join(".gitignore")).unwrap();
        let (files, regions, _md, _roots) = walk_source_files(&root);
        assert!(files.iter().all(|(p, _)| !own(p)), "{files:?}");
        assert!(files.iter().any(|(p, _)| p == "pkg/.ai/repo-graph/other.py"), "{files:?}");
        assert!(files.iter().all(|(p, _)| !p.contains(".glia")), "{files:?}");
        assert!(regions.is_empty(), "{}", regions.len());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// LF.3a: `[[project]]` roots merge into the detected ones. A detected
    /// manifest at the same path wins, and a path the walk never entered
    /// (under a collapsed region, or absent) never becomes a root.
    #[test]
    fn declared_roots_extend_and_never_enter_a_region() {
        let root = walk_tmp("declared");
        for d in ["tools/migrator", "svc/api", "node_modules/pkg", "legacy/tool", ".glia"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("tools/migrator/run.py"), "def migrate():\n    return 2\n").unwrap();
        std::fs::write(root.join("svc/api/pyproject.toml"), "[project]\nname = \"api-svc\"\n").unwrap();
        std::fs::write(
            root.join(".glia/overlay.toml"),
            concat!(
                "version = 1\n[walk]\nskip = [\"legacy\"]\n",
                "[[project]]\npath = \"./tools/migrator/\"\nlabel = \"migrator\"\n",
                "[[project]]\npath = \"svc/api\"\nlabel = \"declared-api\"\n",
                "[[project]]\npath = \"node_modules/pkg\"\n",
                "[[project]]\npath = \"legacy/tool\"\n",
                "[[project]]\npath = \"nowhere\"\n",
            ),
        )
        .unwrap();

        let (files, regions, _md, roots) = walk_source_files(&root);
        let got: Vec<(&str, &str, &str, &str)> = roots
            .iter()
            .map(|r| (r.rel_path.as_str(), r.label.as_str(), r.ecosystem, r.manifest.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("svc/api", "api-svc", "python", "svc/api/pyproject.toml"),
                ("tools/migrator", "migrator", "declared", ".glia/overlay.toml"),
            ],
            "sorted by path; the manifest shadows the declaration"
        );
        assert!(files.iter().any(|(p, _)| p == "tools/migrator/run.py"), "{files:?}");
        let legacy = regions.iter().find(|r| r.rel_path == "legacy").expect("legacy region");
        assert_eq!(legacy.provenance, Collapse::Excluded);
        assert_eq!(
            missing_reason("legacy/tool", &regions),
            "inside region legacy (excluded)"
        );
        assert_eq!(
            missing_reason("node_modules/pkg", &regions),
            "inside region node_modules (vendored)"
        );
        assert!(missing_reason("nowhere", &regions).starts_with("not a directory"));

        // The tally behind `[roots] declared=`, recomputed over the same walk.
        let cfg = glia_config::load(&root).expect("overlay").config;
        let mut declared = DeclaredDirs::new(&cfg.project);
        for rel in ["", "svc", "svc/api", "tools", "tools/migrator"] {
            declared.enter(rel);
        }
        let mut detected = vec![ProjectRoot::new("svc/api".into(), "python", "pyproject.toml", None)];
        let n = merge_declared_roots(&cfg.project, &declared, &regions, &mut detected);
        assert_eq!(n, DeclaredCounts { added: 1, shadowed: 1, missing: 3 });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn worktree_and_nested_clone_get_their_own_provenance() {
        let root = walk_tmp("worktree");
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::create_dir_all(root.join("clone/.git")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: /main/.git/worktrees/wt\n").unwrap();
        std::fs::write(root.join("wt/a.py"), "x = 1\n").unwrap();
        std::fs::write(root.join("clone/b.py"), "y = 2\n").unwrap();

        let (files, regions, _md, _roots) = walk_source_files(&root);

        assert!(files.is_empty(), "no source outside the two other repos: {files:?}");
        let wt = regions.iter().find(|r| r.rel_path == "wt").unwrap();
        assert_eq!(wt.provenance, Collapse::Worktree);
        assert_eq!(wt.provenance.provenance(), "worktree");
        let clone = regions.iter().find(|r| r.rel_path == "clone").unwrap();
        assert_eq!(clone.provenance, Collapse::NestedRepo);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// LG.1b: the reads moved off the descent onto the pool, and a doc's
    /// fall-through moved with them. The inline walk only stopped a `.md` on a
    /// SUCCESSFUL doc read, so an oversize or non-UTF-8 doc went on to the
    /// source rule: read as source when its path passes it (a
    /// `Dockerfile.md`), dropped otherwise. Same rules on 1 and 8 threads,
    /// and `files` / `md` keep walk (name) order.
    #[test]
    fn oversize_and_unreadable_docs_fall_through_to_the_source_rule() {
        let root = walk_tmp("mdfall");
        for d in ["ops/big", "ops/small", "notes", "api"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let over = "x".repeat(MARKDOWN_CAP as usize + 100_000);
        let big_dockerfile = format!("FROM python:3.12\n# {over}\n");
        let big_json = format!(r#"{{"openapi":"3.0.0","paths":{{}},"x-pad":"{over}"}}"#);
        let not_utf8: &[u8] = b"# Notes\n\xff\xfe broken\n";
        let files_in: [(&str, &[u8]); 10] = [
            ("README.md", b"# Fixture\n"),
            ("BIG.md", over.as_bytes()),
            ("notes/broken.md", not_utf8),
            ("ops/big/Dockerfile.md", big_dockerfile.as_bytes()),
            ("ops/small/Dockerfile.md", b"# the small one is a doc\n"),
            ("api/openapi.json", big_json.as_bytes()),
            ("api/small.json", br#"{"openapi":"3.0.0","paths":{}}"#),
            ("app.py", b"x = 1\n"),
            ("bad.py", not_utf8),
            ("zz.go", b"package zz\n"),
        ];
        for (rel, body) in files_in {
            std::fs::write(root.join(rel), body).unwrap();
        }

        let expect_files = ["api/small.json", "app.py", "ops/big/Dockerfile.md", "zz.go"];
        let expect_md = ["README.md", "ops/small/Dockerfile.md"];
        let keys = |v: &[(String, String)]| v.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>();
        let pool = |n| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap();
        let seq = pool(1).install(|| walk_source_files(&root));
        let par = pool(8).install(|| walk_source_files(&root));
        for (label, (files, _, md, _)) in [("1 thread", &seq), ("8 threads", &par)] {
            assert_eq!(keys(files), expect_files, "{label}: files");
            assert_eq!(keys(md), expect_md, "{label}: md");
        }
        assert_eq!(seq.0, par.0, "same texts on 1 and 8 threads");
        assert_eq!(seq.2, par.2);
        let big = seq.0.iter().find(|(p, _)| p == "ops/big/Dockerfile.md").unwrap();
        assert_eq!(big.1, big_dockerfile, "the oversize doc is read whole, as source");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The DC.1 fixture tree: one file per read rule, a gitignored file and
    /// directory, a vendored region, a `[walk] skip` and a manifest root.
    fn dc1_tree(tag: &str) -> std::path::PathBuf {
        let root = walk_tmp(tag);
        for d in ["docs", "api", "svc", "node_modules/left-pad", ".glia", "legacy", "build"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for (rel, body) in [
            (".gitignore", "/build\n*.gen.py\n"),
            ("README.md", "# Fixture\n"),
            ("docs/guide.md", "# Guide\n\nRun `svc/app.py`.\n"),
            ("api/openapi.json", r#"{"openapi":"3.0.3","paths":{"/users":{"get":{}}}}"#),
            ("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#),
            ("svc/app.py", "def run():\n    return 1\n"),
            ("svc/out.gen.py", "generated = 1\n"),
            ("svc/main.go", "package main\n\nfunc main() {}\n"),
            ("node_modules/left-pad/index.js", "module.exports = 1;\n"),
            ("package.json", r#"{"name":"@shop/fixture"}"#),
            (".glia/overlay.toml", "version = 1\n[walk]\nskip = [\"legacy\"]\n"),
            ("legacy/old.py", "old = 2\n"),
            ("build/b.py", "built = 3\n"),
        ] {
            std::fs::write(root.join(rel), body).unwrap();
        }
        root
    }

    /// DC.1: `walk_source_files` is `read_walk(plan_walk)`, and stamping the
    /// reads changes nothing but the records it hands back.
    #[test]
    fn plan_then_read_is_walk_source_files() {
        let root = dc1_tree("planread");
        let walked = walk_source_files(&root);
        let plan = plan_walk(&root);
        let (read, records) = read_walk(&root, &plan, None, false);
        assert_eq!(read, walked, "files, regions, md and roots match the composed walk");
        assert!(records.is_empty(), "a clean read keeps no file table");

        // The fixture exercises every rule it names.
        let keys = |v: &[(String, String)]| v.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&walked.0), ["api/openapi.json", "package.json", "svc/app.py", "svc/main.go"]);
        assert_eq!(keys(&walked.2), ["README.md", "docs/guide.md"]);
        let regions: Vec<(&str, Collapse)> =
            walked.1.iter().map(|r| (r.rel_path.as_str(), r.provenance)).collect();
        assert_eq!(
            regions,
            [
                ("build", Collapse::BuildOutput),
                ("legacy", Collapse::Excluded),
                ("node_modules", Collapse::Vendored),
            ]
        );
        let roots: Vec<&str> = walked.3.iter().map(|r| r.rel_path.as_str()).collect();
        assert_eq!(roots, [""]);

        // With stamps: the same result, one record per queued file in plan order.
        let (stamped, records) = read_walk(&root, &plan, None, true);
        assert_eq!(stamped, walked);
        let pending: Vec<&str> = plan.pending.iter().map(|p| p.rel.as_str()).collect();
        let recorded: Vec<&str> = records.iter().map(|r| r.rel.as_str()).collect();
        assert_eq!(recorded, pending);
        assert!(pending.contains(&"tsconfig.json"), "sniffed, not admitted: {pending:?}");
        assert!(records.iter().all(|r| r.stamp.is_some()), "every queued file is stamped");
        let ts = records.iter().find(|r| r.rel == "tsconfig.json").unwrap();
        assert_eq!(ts.class, PendingClass::JsonSniff);
        assert_eq!(ts.read, Read::Json { over_cap: false, admitted: None });
        let len = |rel: &str| std::fs::metadata(root.join(rel)).unwrap().len();
        assert!(records.iter().all(|r| r.stamp.unwrap().len == len(&r.rel)));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// DC.1: a re-read with a reuse closure reads only what the closure does
    /// not answer, and its result is a fresh walk's.
    #[test]
    fn reuse_rereads_only_changed() {
        use std::collections::BTreeMap;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let root = dc1_tree("reuse");
        let plan = plan_walk(&root);
        let (first, before) = read_walk(&root, &plan, None, true);
        assert_eq!(first, walk_source_files(&root));
        assert_eq!(before.len(), plan.pending.len());

        // A different length, so the stamp moves even inside one mtime tick.
        let edited = "svc/app.py";
        std::fs::write(root.join(edited), "def run():\n    return 2 + 40\n").unwrap();

        let prior: BTreeMap<&str, &ReadRecord> = before.iter().map(|r| (r.rel.as_str(), r)).collect();
        let hits = AtomicUsize::new(0);
        let reuse = |p: &Pending| -> Option<Read> {
            if p.rel == edited {
                return None;
            }
            let r = prior.get(p.rel.as_str()).filter(|r| r.class == p.class)?;
            hits.fetch_add(1, Ordering::Relaxed);
            Some(r.read.clone())
        };
        let plan = plan_walk(&root);
        let (second, after) = read_walk(&root, &plan, Some(&reuse), true);

        assert_eq!(hits.load(Ordering::Relaxed), plan.pending.len() - 1, "every other file reused");
        assert_eq!(second, walk_source_files(&root), "the reused read is a fresh walk");
        let app = second.0.iter().find(|(p, _)| p == edited).unwrap();
        assert!(app.1.contains("2 + 40"), "the edited file was re-read: {:?}", app.1);
        let rels = |v: &[ReadRecord]| v.iter().map(|r| r.rel.clone()).collect::<Vec<_>>();
        assert_eq!(rels(&before), rels(&after));
        let moved: Vec<&str> = before
            .iter()
            .zip(&after)
            .filter(|(b, a)| b.stamp != a.stamp)
            .map(|(b, _)| b.rel.as_str())
            .collect();
        assert_eq!(moved, [edited], "only the edited file's stamp differs");
        let changed: Vec<&str> = before
            .iter()
            .zip(&after)
            .filter(|(b, a)| b.read != a.read)
            .map(|(b, _)| b.rel.as_str())
            .collect();
        assert_eq!(changed, [edited]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// DC.1: the plan lists every directory the descent entered, every region
    /// root it collapsed and every `.gitignore` it saw, each directory with its
    /// mtime, which moves when an entry is added.
    #[test]
    fn plan_records_dirs_and_gitignores() {
        use std::time::{Duration, SystemTime};

        let root = walk_tmp("plandirs");
        for d in ["sub/deep", "other", "node_modules/left-pad", "gen"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for (rel, body) in [
            (".gitignore", "/gen\n"),
            ("sub/.gitignore", "*.tmp\n"),
            ("sub/deep/a.py", "a = 1\n"),
            ("other/b.go", "package other\n"),
            ("node_modules/left-pad/.gitignore", "*.log\n"),
            ("node_modules/left-pad/index.js", "module.exports = 1;\n"),
            ("gen/g.py", "g = 1\n"),
        ] {
            std::fs::write(root.join(rel), body).unwrap();
        }

        let plan = plan_walk(&root);
        let dirs: Vec<&str> = plan.dirs.iter().map(|d| d.rel.as_str()).collect();
        assert_eq!(dirs, ["", "other", "sub", "sub/deep"], "entered dirs, no region");
        let region_dirs: Vec<&str> = plan.region_dirs.iter().map(|d| d.rel.as_str()).collect();
        assert_eq!(region_dirs, ["gen", "node_modules"]);
        assert_eq!(plan.gitignores, [".gitignore", "sub/.gitignore"], "none inside a region");
        assert!(plan.dirs.iter().chain(&plan.region_dirs).all(|d| d.mtime.is_some()));

        // An mtime pinned in the past, so the move below never hides in one tick.
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        for d in ["other", "node_modules"] {
            std::fs::File::open(root.join(d)).unwrap().set_modified(old).unwrap();
        }
        let mtime = |plan: &WalkPlan, rel: &str| {
            plan.dirs.iter().chain(&plan.region_dirs).find(|d| d.rel == rel).unwrap().mtime
        };
        let plan = plan_walk(&root);
        assert_eq!(mtime(&plan, "other"), Some(old));
        assert_eq!(mtime(&plan, "node_modules"), Some(old));

        std::fs::write(root.join("other/c.go"), "package other\n\nfunc C() {}\n").unwrap();
        std::fs::create_dir_all(root.join("node_modules/right-pad")).unwrap();
        let plan = plan_walk(&root);
        assert_ne!(mtime(&plan, "other"), Some(old), "a new file moves its directory's mtime");
        assert_ne!(mtime(&plan, "node_modules"), Some(old), "a region root's own entries move it");
        assert!(plan.pending.iter().any(|p| p.rel == "other/c.go"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
