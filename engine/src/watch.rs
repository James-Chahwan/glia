//! `WatchSession`: an incremental build held in memory, updated from changed
//! paths or a stat poll, persisted on demand. Public module slot, reached by
//! module path (`glia_engine::watch::WatchSession`). Filled by DC.7a (+DC.7b,
//! DC.7c).
//!
//! A session is one repo's build kept open (DC.7a): the walk plan and its file
//! table (every queued file's class, [`Read`] and stat stamp), the repo-level
//! inputs the build reads besides the walked files, one [`ParseCache`] with its
//! DC.4 build memos, and the last [`GenerateResult`]. [`WatchSession::apply`]
//! takes the paths the caller saw change and rebuilds through the same DC.2
//! stage calls `generate_one_inner` makes (parse and graft, plan / build / fold
//! the groups, the per-repo markers, the region / project / docs graphs, the
//! pass tail), so every update's graph is a clean build of the tree as the
//! session read it, by construction. This packet rebuilds every language group
//! from the warm cache; DC.7b adds the group early cutoff, DC.7c the stat poll.
//!
//! Durability (Salsa's): a path is [`Durability::Low`] (a source file: only its
//! read moves) or [`Durability::High`] (a repo-level input: `.glia/`, a
//! `.gitignore`, a `go.mod`, a tsconfig, the C/C++ include sources, a project
//! manifest, a migration, the git identity files), see [`durability`]. A
//! Low-only update re-reads exactly the named table entries and keeps the walk
//! plan and the repo-level inputs; a High path, or a change of the tree's shape
//! (a file added or removed, a directory, a region root's direct entry),
//! re-plans the walk, reusing the table's read of every file it was not told
//! about, and recomputes the repo-level inputs.
//!
//! fired_on markers, on stderr:
//! - `[watch] open repo=<label> files=<n> groups=<g> nodes=<N> edges=<E> took=<ms>`
//!   once per [`WatchSession::open`]; `files` counts the walked sources and docs
//!   the build read, `groups` the language groups.
//! - `[watch] update seq=<s> files +<a> ~<m> -<r> groups_rebuilt=<k>/<g> full_rebuild=<reason|none> nodes +<a> -<d> ~<m> ><v> edges +<a> -<r> ~<c> took=<ms> repo=<label>`
//!   once per update that rebuilt (`>` counts moved nodes, `~` on edges the
//!   reconfidenced ones).
//! - `[watch] noop repo=<label> paths=<n>` for an update that changed nothing
//!   (`n` = the paths it was given), and `[watch] outside repo=<label> paths=<n>`
//!   when some given paths do not lie under the root.
//!
//! An update also prints the stage markers of what it ran (`[timing] repo=`,
//! `[timing] build`, `[graft-memo]`, the walk's lines on a re-plan, ...): never
//! gate on marker equality with a clean build (R2-watch section 8).

use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hasher as _;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use glia_code_domain::glia_config::{LoadedConfig, OVERLAY_FILE};
use glia_code_domain::project_roots::{self, ProjectRoot};
use glia_code_domain::walk_gating::{self, CONTROL_DIR, RepoIdentity, repo_identity};
use glia_code_extractors::{grpc, migrations, packages};
use glia_core::RepoId;
use serde::Serialize;
use twox_hash::XxHash64;

use crate::build::assemble::{RepoBuildCtx, parse_and_graft, repo_markers};
use crate::build::c_includes::IncludeRoots;
use crate::build::lang_build::{
    GroupCtx, GroupPlan, LanguageGraphs, TsAliasSet, build_groups, fold_groups, plan_groups,
};
use crate::build::rpc_needles::RpcContext;
use crate::build::timing::{self, BuildTimes, PhaseTimes};
use crate::build::{
    BuildOptions, Finished, GenerateResult, aux_graphs, finish_build, go_modules_for,
};
use crate::cache::ParseCache;
use crate::delta::{DeltaCounts, DeltaEdge, DeltaNode, located_delta_counted};
use crate::external::{RepoInputs, repo_inputs};
use crate::extract::{GoModules, detect_language};
use crate::passes::GroupFill;
use crate::walk::{
    FileStamp, Pending, Read, ReadRecord, RegionAnchor, Reuse, WalkPlan, plan_walk, read_one,
    read_walk,
};

/// How far a changed path reaches ([`durability`]). `High` carries the input
/// class it belongs to (`glia_inputs`, `gitignore`, `go_mod`, `tsconfig`,
/// `include_roots`, `manifest`, `migration`, `identity`).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    /// A walked file's content: only its own read moves.
    Low,
    /// A repo-level input: the walk is re-planned and the inputs recomputed.
    High(&'static str),
}

/// The durability of the repo-relative path `rel` (`/`-separated):
/// - `glia_inputs`: `.glia` and everything under it but `.glia/graph/**` (the
///   overlay, the `[walk]` / `[[project]]` config, the snapshots passes read);
/// - `gitignore`: any `.gitignore`;
/// - `go_mod`: any `go.mod` (the module set, so the parse cache's context);
/// - `tsconfig`: `tsconfig.json` / `tsconfig.base.json` (the TS path aliases);
/// - `include_roots`: `CMakeLists.txt` / `compile_commands.json`;
/// - `manifest`: a project-root manifest (a [`project_roots::MANIFESTS`] name,
///   `build.sbt` and `requirements.txt` included, or a
///   [`project_roots::MANIFEST_EXTS`] extension, the .NET `*.csproj` / `*.sln`
///   family) or a package manifest (`packages::is_manifest_path`): root
///   detection reads manifest content, and a .NET project gates its bin/obj;
/// - `migration`: a migration `.sql` (the descent's size cap queues it or not);
/// - `identity`: `.git` itself, `.git/HEAD`, `.git/config` (the repo identity
///   and the recorded rev).
///
/// Everything else is `Low`.
pub fn durability(rel: &str) -> Durability {
    let rel = rel.trim_start_matches("./");
    if matches!(rel, ".git" | ".git/HEAD" | ".git/config") {
        return Durability::High("identity");
    }
    if let Some(rest) = rel.strip_prefix(CONTROL_DIR)
        && (rest.is_empty() || rest.starts_with('/'))
    {
        let rest = rest.trim_start_matches('/');
        return if rest == "graph" || rest.starts_with("graph/") {
            Durability::Low
        } else {
            Durability::High("glia_inputs")
        };
    }
    let base = rel.rsplit('/').next().unwrap_or(rel);
    let reason = match base {
        ".gitignore" => Some("gitignore"),
        "go.mod" => Some("go_mod"),
        "tsconfig.json" | "tsconfig.base.json" => Some("tsconfig"),
        "CMakeLists.txt" | "compile_commands.json" => Some("include_roots"),
        _ if is_project_manifest(base) || packages::is_manifest_path(rel) => Some("manifest"),
        _ if migrations::is_migration_path(rel) => Some("migration"),
        _ => None,
    };
    reason.map_or(Durability::Low, Durability::High)
}

/// `base` names a project-root manifest the walk's root detection reads: a
/// [`project_roots::MANIFESTS`] name, or a non-empty stem with a
/// [`project_roots::MANIFEST_EXTS`] extension (any case, as detection matches).
fn is_project_manifest(base: &str) -> bool {
    project_roots::MANIFESTS
        .iter()
        .any(|(name, _)| *name == base)
        || base.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && project_roots::MANIFEST_EXTS
                    .iter()
                    .any(|(e, _)| e.eq_ignore_ascii_case(ext))
        })
}

/// The walked-file side of one update: table entries by repo-relative path,
/// each list sorted, plus the parse cache's tally of the build.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, Default)]
pub struct WatchFiles {
    /// In the new table only.
    pub added: Vec<String>,
    /// In both tables with another class or read.
    pub modified: Vec<String>,
    /// In the old table only.
    pub removed: Vec<String>,
    /// Files the build parsed (`CacheStats::reparsed`).
    pub reparsed: usize,
    /// Files served from the parse cache (`CacheStats::reused`).
    pub reused: usize,
}

/// What one update's phases took, in milliseconds truncated to one decimal
/// (`timing::ms`). `read` is the file reads (the named entries, or the
/// re-planned walk's reads), `walk` the re-plan (0 on a Low update); `parse`,
/// `const_scan`, `grafts` and `language_build` are CA.9's repo phases,
/// `resolve`, `post`, `finalize` and `external_cells` its build tail;
/// `delta` is the located delta against the previous graph and `total` the
/// whole [`WatchSession::apply`] call.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, Copy, Default, PartialEq)]
pub struct WatchPhases {
    pub read: f64,
    pub walk: f64,
    pub parse: f64,
    pub const_scan: f64,
    pub grafts: f64,
    pub language_build: f64,
    pub resolve: f64,
    pub post: f64,
    pub finalize: f64,
    pub external_cells: f64,
    pub delta: f64,
    pub total: f64,
}

/// One [`WatchSession::apply`]'s answer: the session's `seq` after it (left
/// unchanged by an update that changed nothing), the files that moved, the
/// language groups rebuilt (every group of the plan in DC.7a), the reason the
/// update counts as a full rebuild (`identity`, `go_modules`, `overlay`,
/// `proto_services`; `None` otherwise), the new totals, and the located delta
/// against the previous graph (`delta::located_delta_counted`: rows sorted as
/// `glia delta` sorts them, with their counts).
#[non_exhaustive]
#[derive(Serialize, Debug, Clone)]
pub struct WatchUpdate {
    pub seq: u64,
    pub files: WatchFiles,
    pub groups_rebuilt: Vec<String>,
    pub full_rebuild: Option<String>,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub counts: DeltaCounts,
    pub nodes: Vec<DeltaNode>,
    pub edges: Vec<DeltaEdge>,
    pub phase_ms: WatchPhases,
}

/// One repo's build held open: see the module doc. Made by
/// [`WatchSession::open`]; every field is private, so a new one is not a
/// break.
///
/// The session holds no lock on the repo. [`WatchSession::persist`] writes
/// against the files on disk: CD.7c stores a CODE cell as a span only when its
/// text still matches the file, so persist right after an update.
pub struct WatchSession {
    /// The root as the caller gave it.
    root: PathBuf,
    /// The root's absolute and canonical spellings, which an absolute changed
    /// path is made relative against.
    abs_roots: Vec<PathBuf>,
    /// The root as given, as a string: every marker's `repo=`, the result's
    /// label and root, the parse cache's location.
    label: String,
    opts: BuildOptions,
    plan: WalkPlan,
    /// One record per `plan.pending`, index for index (walk order).
    records: Vec<ReadRecord>,
    /// `records` by repo-relative path.
    index: BTreeMap<String, usize>,
    regions: Vec<RegionAnchor>,
    roots: Vec<ProjectRoot>,
    setup: Setup,
    /// The build's proto service set ([`proto_services_fp`]).
    services_fp: u64,
    cache: ParseCache,
    result: GenerateResult,
    seq: u64,
}

/// The repo-level inputs a build reads besides the walked files, and the
/// fingerprints an update compares.
#[derive(Clone)]
struct Setup {
    ident: RepoIdentity,
    repo: RepoId,
    go: GoModules,
    ts_aliases: TsAliasSet,
    c_includes: IncludeRoots,
    /// The walked file list `c_includes` was read for ([`paths_fp`]).
    files_fp: u64,
    /// `.glia/overlay.toml` (or the given overlay text), loaded.
    config: Option<LoadedConfig>,
    /// [`overlay_fp`].
    overlay_fp: u64,
    /// [`glia_inputs_fp`].
    glia_fp: u64,
}

/// The session's root, label and options, as the stages read them.
struct At<'a> {
    root: &'a Path,
    label: &'a str,
    opts: &'a BuildOptions,
}

/// The file table an update would commit, and its walk's output.
struct Tree {
    /// `Some` after a re-plan; `None` keeps the session's plan.
    plan: Option<WalkPlan>,
    records: Vec<ReadRecord>,
    files: Vec<(String, String)>,
    md: Vec<(String, String)>,
    regions: Vec<RegionAnchor>,
    roots: Vec<ProjectRoot>,
    changes: WatchFiles,
    walk: Duration,
    read: Duration,
}

/// One build through the DC.2 stages.
struct Built {
    result: GenerateResult,
    /// The plan's group keys, in plan order.
    groups: Vec<&'static str>,
    phases: PhaseTimes,
    times: BuildTimes,
}

impl Setup {
    /// Every input, read as `generate_one_inner` reads them.
    fn read(
        at: &At<'_>,
        ident: RepoIdentity,
        files: &[(String, String)],
        roots: &[ProjectRoot],
    ) -> Self {
        let repo = RepoId::from_canonical(&ident.key);
        let config = load_config(at, repo);
        let go = go_modules_for(at.root, roots, at.label);
        let ts_aliases = TsAliasSet::read(at.root, roots, at.label);
        let c_includes = IncludeRoots::read(at.root, files, roots, at.label);
        Self {
            ident,
            repo,
            go,
            ts_aliases,
            c_includes,
            files_fp: paths_fp(files),
            config,
            overlay_fp: overlay_fp(at.root, at.opts),
            glia_fp: glia_inputs_fp(at.root),
        }
    }

    /// The inputs for `tree`: the identity re-read when an identity path is
    /// in `high`, the go modules and tsconfig aliases on a re-plan (they read
    /// the project roots), the include roots on a re-plan or a changed file
    /// list (they read the dirs that hold a C/C++ file), the overlay and the
    /// `.glia` fingerprint when a `.glia` path is in `high`. The rest is kept.
    fn refresh(&self, at: &At<'_>, tree: &Tree, high: &BTreeSet<&str>, replan: bool) -> Self {
        let ident = if high.contains("identity") {
            let ident = repo_identity(at.root);
            repo_id_marker(&ident, at.label);
            ident
        } else {
            self.ident.clone()
        };
        let repo = RepoId::from_canonical(&ident.key);
        let (go, ts_aliases) = if replan {
            (
                go_modules_for(at.root, &tree.roots, at.label),
                TsAliasSet::read(at.root, &tree.roots, at.label),
            )
        } else {
            (self.go.clone(), self.ts_aliases.clone())
        };
        let files_fp = paths_fp(&tree.files);
        let c_includes = if replan || files_fp != self.files_fp {
            IncludeRoots::read(at.root, &tree.files, &tree.roots, at.label)
        } else {
            self.c_includes.clone()
        };
        let (config, overlay, glia) = if high.contains("glia_inputs") {
            (
                load_config(at, repo),
                overlay_fp(at.root, at.opts),
                glia_inputs_fp(at.root),
            )
        } else {
            (self.config.clone(), self.overlay_fp, self.glia_fp)
        };
        Self {
            ident,
            repo,
            go,
            ts_aliases,
            c_includes,
            files_fp,
            config,
            overlay_fp: overlay,
            glia_fp: glia,
        }
    }

    /// Whether every input a build reads is the same as `other`'s.
    /// `TsAliasSet` holds only strings and vecs, so its `Debug` is a pure
    /// function of its content.
    fn same_inputs(&self, other: &Self) -> bool {
        self.ident.key == other.ident.key
            && self.go == other.go
            && format!("{:?}", self.ts_aliases) == format!("{:?}", other.ts_aliases)
            && self.c_includes == other.c_includes
            && self.overlay_fp == other.overlay_fp
            && self.glia_fp == other.glia_fp
    }
}

impl WatchSession {
    /// Open a session on the repo at `root`: load its parse cache sidecar
    /// once, plan and read the walk (with stat stamps), read the repo-level
    /// inputs and run one build. Errs when `root` is not a directory.
    pub fn open(root: &str, opts: &BuildOptions) -> Result<Self, String> {
        let started = Instant::now();
        let root_path = PathBuf::from(root);
        if !root_path.is_dir() {
            return Err(format!("not a directory: {root}"));
        }
        let mut cache = ParseCache::load(root);
        let ident = repo_identity(&root_path);
        repo_id_marker(&ident, root);
        let walk_started = Instant::now();
        let plan = plan_walk(&root_path);
        let walk = walk_started.elapsed();
        let read_started = Instant::now();
        let ((files, regions, md, roots), records) = read_walk(&root_path, &plan, None, true);
        let read = read_started.elapsed();
        let at = At {
            root: &root_path,
            label: root,
            opts,
        };
        let setup = Setup::read(&at, ident, &files, &roots);
        let services_fp = proto_services_fp(&files);
        let walked = files.len() + md.len();
        let mut tree = Tree {
            plan: None,
            records: Vec::new(),
            files,
            md,
            regions,
            roots,
            changes: WatchFiles::default(),
            walk,
            read,
        };
        let built = build(&at, &setup, &mut tree, &mut cache, started);
        eprintln!(
            "[watch] open repo={root} files={walked} groups={} nodes={} edges={} took={}",
            built.groups.len(),
            built.result.total_nodes,
            built.result.total_edges,
            timing::ms(started.elapsed())
        );
        Ok(Self {
            abs_roots: abs_roots(&root_path),
            index: index_of(&records),
            root: root_path,
            label: root.to_string(),
            opts: opts.clone(),
            plan,
            records,
            regions: tree.regions,
            roots: tree.roots,
            setup,
            services_fp,
            cache,
            result: built.result,
            seq: 0,
        })
    }

    /// Bring the session up to date with `changed`, the paths the caller saw
    /// change (absolute under the root, or repo-relative). A path the caller
    /// does not name is not re-read. Paths under `.glia/graph/`, under a
    /// hard-skipped directory (`.git/`, but `.git/HEAD` and `.git/config`,
    /// `.idea/`, a nested `.glia/`, ...) and two or more levels below a
    /// collapsed region root are dropped: their content never reaches the
    /// graph. A direct entry of a region root re-plans (`gate_dir` decides
    /// the region from exactly those entries).
    ///
    /// Re-plans when a path is High ([`durability`]), a known file is gone or
    /// no longer a file, an unknown path exists, or a gone path is a directory
    /// the walk knew (or held table entries); otherwise re-reads only the named
    /// entries. When the table, the regions, the project roots and every
    /// repo-level input are unchanged the update is a noop: no rows, `seq`
    /// unchanged. Otherwise the tree is rebuilt, the located delta taken
    /// against the previous graph, `seq` advanced and the freed heap trimmed
    /// (`memory::release_freed_memory`). Errs when the root is no longer a
    /// directory; state is committed only once an update succeeded, so an
    /// error (or a panic an embedding app catches) leaves the previous state.
    pub fn apply(&mut self, changed: &[PathBuf]) -> Result<WatchUpdate, String> {
        let started = Instant::now();
        if !self.root.is_dir() {
            return Err(format!("not a directory: {}", self.label));
        }
        let mut outside = 0usize;
        let mut named = BTreeSet::new();
        for p in changed {
            match self.rel_of(p) {
                Some(rel) if !self.dropped(&rel) => {
                    named.insert(rel);
                }
                Some(_) => {}
                None => outside += 1,
            }
        }
        if outside > 0 {
            eprintln!("[watch] outside repo={} paths={outside}", self.label);
        }
        let high: BTreeSet<&'static str> = named
            .iter()
            .filter_map(|rel| match durability(rel) {
                Durability::High(why) => Some(why),
                Durability::Low => None,
            })
            .collect();
        let replan = !high.is_empty() || named.iter().any(|rel| self.reshapes(rel));
        let tree = if named.is_empty() {
            None
        } else if replan {
            Some(self.replanned(&named))
        } else {
            self.reread(&named)
        };
        let Some(mut tree) = tree else {
            return Ok(self.noop(changed.len(), started, Duration::ZERO, Duration::ZERO));
        };
        let at = At {
            root: &self.root,
            label: &self.label,
            opts: &self.opts,
        };
        let setup = self.setup.refresh(&at, &tree, &high, replan);
        let unchanged = tree.changes.added.is_empty()
            && tree.changes.modified.is_empty()
            && tree.changes.removed.is_empty()
            && tree.regions == self.regions
            && tree.roots == self.roots
            && setup.same_inputs(&self.setup);
        if unchanged {
            let (walk, read) = (tree.walk, tree.read);
            self.commit_tree(tree, setup);
            return Ok(self.noop(changed.len(), started, walk, read));
        }
        let services_fp = proto_services_fp(&tree.files);
        let full_rebuild = full_rebuild_reason(&self.setup, &setup, self.services_fp, services_fp);
        let built = build(&at, &setup, &mut tree, &mut self.cache, started);
        let delta_started = Instant::now();
        let rows = located_delta_counted(&self.result.merged, &built.result.merged, &[]);
        let delta = delta_started.elapsed();
        let Built {
            result,
            groups,
            phases,
            times,
        } = built;
        let (walk, read) = (tree.walk, tree.read);
        let changes = std::mem::take(&mut tree.changes);
        self.commit_tree(tree, setup);
        self.services_fp = services_fp;
        self.seq += 1;
        drop(std::mem::replace(&mut self.result, result));
        crate::memory::release_freed_memory("watch_update");
        let stats = self.cache.stats;
        let update = WatchUpdate {
            seq: self.seq,
            files: WatchFiles {
                reparsed: stats.reparsed,
                reused: stats.reused,
                ..changes
            },
            groups_rebuilt: groups.iter().map(|g| (*g).to_string()).collect(),
            full_rebuild: full_rebuild.map(str::to_string),
            total_nodes: self.result.total_nodes,
            total_edges: self.result.total_edges,
            counts: rows.counts,
            nodes: rows.nodes,
            edges: rows.edges,
            phase_ms: WatchPhases {
                read: ms(read),
                walk: ms(walk),
                parse: ms(phases.parse),
                const_scan: ms(phases.const_scan),
                grafts: ms(phases.grafts),
                language_build: ms(phases.language_build),
                resolve: ms(times.resolve),
                post: ms(times.post),
                finalize: ms(times.finalize),
                external_cells: ms(times.external_cells.unwrap_or_default()),
                delta: ms(delta),
                total: ms(started.elapsed()),
            },
        };
        eprintln!(
            "{}",
            update_marker(&update, groups.len(), started.elapsed(), &self.label)
        );
        Ok(update)
    }

    /// The session's current build.
    pub fn result(&self) -> &GenerateResult {
        &self.result
    }

    /// Write the current build as the layout at `dir` (`persist::persist_result`,
    /// writer `watch`). The caller enforces the default-dir rule for an
    /// overlay-free session, as `glia build` and `save_to_default` do.
    pub fn persist(&self, dir: &Path) -> Result<(), String> {
        crate::persist::persist_result(&self.result, dir, "watch")
    }

    /// Save the held parse cache beside the repo's layout
    /// (`ParseCache::save`: written only when it changed, LC.11).
    pub fn save_cache(&self) -> std::io::Result<()> {
        self.cache.save(&self.label)
    }

    /// Updates that rebuilt since [`WatchSession::open`] (0 right after it).
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// `p` as a repo-relative `/` path: an absolute `p` must lie under the
    /// root (as given made absolute, or canonical), a relative one is taken as
    /// repo-relative; `None` for a path outside the root or one that climbs
    /// (`..`). The root itself is `""`.
    fn rel_of(&self, p: &Path) -> Option<String> {
        let rel = if p.is_absolute() {
            self.abs_roots.iter().find_map(|r| p.strip_prefix(r).ok())?
        } else {
            p
        };
        let mut parts = Vec::new();
        for c in rel.components() {
            match c {
                Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        Some(parts.join("/"))
    }

    /// Whether a changed `rel` cannot reach the graph: the layout dir, a path
    /// under a hard-skipped directory (the root's `.git/HEAD` / `.git/config`
    /// and `.glia/` inputs aside), the 0.4.x layout `.ai/repo-graph`, or a path
    /// two or more levels below a region root.
    fn dropped(&self, rel: &str) -> bool {
        if matches!(rel, ".git/HEAD" | ".git/config") {
            return false;
        }
        let parts: Vec<&str> = rel.split('/').collect();
        if parts.first() == Some(&CONTROL_DIR) && parts.len() > 1 {
            return parts.get(1) == Some(&"graph");
        }
        if rel == ".ai/repo-graph" || rel.starts_with(".ai/repo-graph/") {
            return true;
        }
        if let Some((_, dirs)) = parts.split_last()
            && dirs.iter().any(|d| walk_gating::is_hard_skip(d))
        {
            return true;
        }
        self.plan
            .region_dirs
            .iter()
            .any(|d| below(rel, &d.rel).is_some_and(|rest| rest.contains('/')))
    }

    /// Whether a changed `rel` can change the walk's shape: the root, a direct
    /// entry of a region root, a known file now gone or not a file, an unknown
    /// path that exists, or a gone path the walk knew as a directory (an
    /// entered or region dir, or the parent of table entries).
    fn reshapes(&self, rel: &str) -> bool {
        if rel.is_empty() {
            return true;
        }
        if self
            .plan
            .region_dirs
            .iter()
            .any(|d| below(rel, &d.rel).is_some_and(|rest| !rest.contains('/')))
        {
            return true;
        }
        let abs = self.root.join(rel);
        if self.index.contains_key(rel) {
            return !abs.is_file();
        }
        if std::fs::symlink_metadata(&abs).is_ok() {
            return true;
        }
        let prefix = format!("{rel}/");
        self.index
            .range(prefix.clone()..)
            .next()
            .is_some_and(|(k, _)| k.starts_with(&prefix))
            || self
                .plan
                .dirs
                .iter()
                .chain(&self.plan.region_dirs)
                .any(|d| d.rel == rel || d.rel.starts_with(&prefix))
    }

    /// A Low update: re-read each named table entry (stamp first, then the
    /// read rule of its class); `None` when every read is unchanged (their
    /// stamps are refreshed). Otherwise the walk's fold over the session's
    /// plan with every other entry's read reused, each keeping its stamp.
    fn reread(&mut self, named: &BTreeSet<String>) -> Option<Tree> {
        let started = Instant::now();
        let mut fresh: BTreeMap<usize, ReadRecord> = BTreeMap::new();
        for rel in named {
            let Some(&i) = self.index.get(rel) else {
                continue;
            };
            let Some(p) = self.plan.pending.get(i) else {
                continue;
            };
            let stamp = FileStamp::of(&p.abs);
            let read = read_one(p);
            let Some(old) = self.records.get_mut(i) else {
                continue;
            };
            if old.read == read {
                old.stamp = stamp;
            } else {
                fresh.insert(
                    i,
                    ReadRecord {
                        rel: p.rel.clone(),
                        class: p.class,
                        read,
                        stamp,
                    },
                );
            }
        }
        if fresh.is_empty() {
            return None;
        }
        let (records, index) = (&self.records, &self.index);
        let reuse = |p: &Pending| -> Option<Read> {
            let &i = index.get(&p.rel)?;
            fresh
                .get(&i)
                .or_else(|| records.get(i))
                .map(|r| r.read.clone())
        };
        let reuse: &Reuse<'_> = &reuse;
        let ((files, regions, md, roots), mut next) =
            read_walk(&self.root, &self.plan, Some(reuse), false);
        for (i, r) in next.iter_mut().enumerate() {
            r.stamp = fresh
                .get(&i)
                .or_else(|| records.get(i))
                .and_then(|old| old.stamp);
        }
        let changes = WatchFiles {
            modified: fresh.into_values().map(|r| r.rel).collect(),
            ..WatchFiles::default()
        };
        Some(Tree {
            plan: None,
            records: next,
            files,
            md,
            regions,
            roots,
            changes,
            walk: Duration::ZERO,
            read: started.elapsed(),
        })
    }

    /// A re-plan: `plan_walk`, then `read_walk` with stamps, reusing the
    /// table's read of every queued file that is not named and keeps its
    /// class. A reused entry keeps its recorded stamp (DC.1's handoff): the
    /// fresh stamp would hide a change nobody named from the next stat poll.
    fn replanned(&self, named: &BTreeSet<String>) -> Tree {
        let started = Instant::now();
        let plan = plan_walk(&self.root);
        let walk = started.elapsed();
        let read_started = Instant::now();
        let (records, index) = (&self.records, &self.index);
        let kept = |p: &Pending| -> Option<&ReadRecord> {
            if named.contains(&p.rel) {
                return None;
            }
            let &i = index.get(&p.rel)?;
            records.get(i).filter(|r| r.class == p.class)
        };
        let reuse = |p: &Pending| kept(p).map(|r| r.read.clone());
        let reuse: &Reuse<'_> = &reuse;
        let ((files, regions, md, roots), mut next) =
            read_walk(&self.root, &plan, Some(reuse), true);
        for (p, r) in plan.pending.iter().zip(next.iter_mut()) {
            if let Some(old) = kept(p) {
                r.stamp = old.stamp;
            }
        }
        let changes = file_changes(records, &next);
        Tree {
            plan: Some(plan),
            records: next,
            files,
            md,
            regions,
            roots,
            changes,
            walk,
            read: read_started.elapsed(),
        }
    }

    /// Commit `tree`'s file table (and its plan after a re-plan) and `setup`.
    fn commit_tree(&mut self, tree: Tree, setup: Setup) {
        let Tree {
            plan,
            records,
            regions,
            roots,
            ..
        } = tree;
        if let Some(plan) = plan {
            self.plan = plan;
            self.index = index_of(&records);
        }
        self.records = records;
        self.regions = regions;
        self.roots = roots;
        self.setup = setup;
    }

    /// The answer of an update that changed nothing, with its marker.
    fn noop(&self, paths: usize, started: Instant, walk: Duration, read: Duration) -> WatchUpdate {
        eprintln!("[watch] noop repo={} paths={paths}", self.label);
        WatchUpdate {
            seq: self.seq,
            files: WatchFiles::default(),
            groups_rebuilt: Vec::new(),
            full_rebuild: None,
            total_nodes: self.result.total_nodes,
            total_edges: self.result.total_edges,
            counts: DeltaCounts::default(),
            nodes: Vec::new(),
            edges: Vec::new(),
            phase_ms: WatchPhases {
                read: ms(read),
                walk: ms(walk),
                total: ms(started.elapsed()),
                ..WatchPhases::default()
            },
        }
    }
}

/// One build of `tree` through the DC.2 stages, composed exactly as
/// `generate_one_inner` composes them (`build_graphs_for_repo` with no group
/// reuse, then the region / project / docs graphs and `finish_build`, whose
/// pass context is `CodeBuildCtx::new(inputs, opts)`), with `cache` held.
/// Prints the `[timing] repo=` and `[timing] build` lines. Takes `tree.md`.
fn build(
    at: &At<'_>,
    setup: &Setup,
    tree: &mut Tree,
    cache: &mut ParseCache,
    started: Instant,
) -> Built {
    let (root, label, opts) = (at.root, at.label, at.opts);
    cache.validate_context(&setup.ident.key, &setup.go.context_key());
    let mut rpc = RpcContext::default();
    rpc.add_files(&tree.files);
    let inputs = vec![RepoInputs {
        repo: setup.repo,
        root: root.to_path_buf(),
        label: label.to_string(),
        config: setup.config.clone(),
    }];
    let ctx = RepoBuildCtx {
        repo: setup.repo,
        repo_label: label,
        go: &setup.go,
        ts_aliases: &setup.ts_aliases,
        c_includes: &setup.c_includes,
        rpc: &rpc,
        roots: &tree.roots,
        config: inputs.first().and_then(|i| i.config.as_ref()),
        opts,
    };
    let mut g = parse_and_graft(&tree.files, Some(cache), &ctx);
    let language_started = Instant::now();
    let GroupPlan {
        groups,
        di_refs,
        recv_fields,
    } = plan_groups(std::mem::take(&mut g.parses_by_lang), label);
    let keys: Vec<&'static str> = groups.iter().map(|group| group.key).collect();
    let group_ctx = GroupCtx {
        repo: setup.repo,
        repo_label: label,
        rust_crates: &g.rust_crates,
        ts_aliases: &setup.ts_aliases,
        c_includes: &setup.c_includes,
        dart_packages: &g.dart_packages,
    };
    let builds = build_groups(groups, Vec::new(), &group_ctx);
    if let Some(line) = g.dart_packages.marker(label) {
        eprintln!("{line}");
    }
    let LanguageGraphs {
        mut graphs,
        fills,
        pooled,
        ..
    } = fold_groups(builds, &recv_fields, &mut g.parse_errors, label);
    g.times.language_build = language_started.elapsed();
    g.times.walk = tree.walk + tree.read;
    repo_markers(&graphs, &di_refs, &g, pooled, label);
    let mut fills: Vec<Option<GroupFill>> = fills.into_iter().map(Some).collect();
    graphs.extend(aux_graphs(
        root,
        setup.repo,
        &tree.regions,
        &tree.roots,
        std::mem::take(&mut tree.md),
    ));
    fills.resize_with(graphs.len(), || None);
    eprintln!("{}", g.times.repo_marker(label));
    let Finished {
        merged,
        total_nodes,
        total_edges,
        times,
    } = finish_build(graphs, fills, inputs, opts, 1, started);
    eprintln!("{}", times.marker());
    let mut parse_errors = g.parse_errors;
    parse_errors.shrink_to_fit();
    Built {
        result: GenerateResult {
            merged,
            total_nodes,
            total_edges,
            parse_errors,
            repo_labels: crate::arch::repo_label_map(&[(setup.repo.0, label.to_string())]),
            repo_roots: BTreeMap::from([(setup.repo.0, label.to_string())]),
        },
        groups: keys,
        phases: g.times,
        times,
    }
}

/// `repo_inputs`' overlay config (it prints the `[overlay] loaded` line).
fn load_config(at: &At<'_>, repo: RepoId) -> Option<LoadedConfig> {
    repo_inputs(
        repo,
        at.root.to_path_buf(),
        at.label.to_string(),
        at.opts.overlay_text.as_deref(),
    )
    .config
}

/// The reason an update counts as a full rebuild: the identity key moved
/// (every NodeId re-keys), the go module context moved (the parse cache
/// discards every parse), the overlay moved, or the proto service set moved
/// (every gRPC needle re-runs).
fn full_rebuild_reason(
    before: &Setup,
    after: &Setup,
    services_before: u64,
    services_after: u64,
) -> Option<&'static str> {
    if before.ident.key != after.ident.key {
        Some("identity")
    } else if before.go.context_key() != after.go.context_key() {
        Some("go_modules")
    } else if before.overlay_fp != after.overlay_fp {
        Some("overlay")
    } else if services_before != services_after {
        Some("proto_services")
    } else {
        None
    }
}

/// The table entries added, modified (class or read) and removed between
/// `old` and `new`, each sorted.
fn file_changes(old: &[ReadRecord], new: &[ReadRecord]) -> WatchFiles {
    let before: BTreeMap<&str, &ReadRecord> = old.iter().map(|r| (r.rel.as_str(), r)).collect();
    let after: BTreeMap<&str, &ReadRecord> = new.iter().map(|r| (r.rel.as_str(), r)).collect();
    let mut changes = WatchFiles::default();
    for (rel, r) in &after {
        match before.get(rel) {
            None => changes.added.push((*rel).to_string()),
            Some(o) if o.class != r.class || o.read != r.read => {
                changes.modified.push((*rel).to_string())
            }
            Some(_) => {}
        }
    }
    changes.removed = before
        .keys()
        .filter(|rel| !after.contains_key(*rel))
        .map(|rel| (*rel).to_string())
        .collect();
    changes
}

/// The rest of `rel` below the directory `dir`, `None` when `rel` is not
/// under it.
fn below<'a>(rel: &'a str, dir: &str) -> Option<&'a str> {
    rel.strip_prefix(dir)?.strip_prefix('/')
}

/// `records` by repo-relative path.
fn index_of(records: &[ReadRecord]) -> BTreeMap<String, usize> {
    records
        .iter()
        .enumerate()
        .map(|(i, r)| (r.rel.clone(), i))
        .collect()
}

/// `root` made absolute and canonicalised, the spellings an absolute changed
/// path is matched against (distinct, in that order).
fn abs_roots(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in [
        std::path::absolute(root).ok(),
        std::fs::canonicalize(root).ok(),
    ]
    .into_iter()
    .flatten()
    {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// xxh64 of the walked file list's paths, in order.
fn paths_fp(files: &[(String, String)]) -> u64 {
    let mut h = XxHash64::with_seed(0);
    for (path, _) in files {
        h.write(path.as_bytes());
        h.write_u8(0);
    }
    h.finish()
}

/// The build's proto service set, as `RpcContext::add_files` folds it
/// (sorted, deduplicated), fingerprinted as the DC.4 needle memo keys it.
fn proto_services_fp(files: &[(String, String)]) -> u64 {
    let mut services: Vec<grpc::ProtoServiceRef> = files
        .iter()
        .filter(|(path, _)| detect_language(path) == Some("proto"))
        .flat_map(|(_, source)| grpc::proto_service_refs(source))
        .collect();
    services.sort_unstable();
    services.dedup();
    crate::build::memo::services_fp(&services)
}

/// xxh64 of `opts.overlay` and the overlay the build reads: the given text,
/// or `.glia/overlay.toml`'s bytes (a missing file apart from an empty one).
fn overlay_fp(root: &Path, opts: &BuildOptions) -> u64 {
    let mut h = XxHash64::with_seed(0);
    h.write_u8(u8::from(opts.overlay));
    match &opts.overlay_text {
        Some(text) => {
            h.write_u8(2);
            h.write(text.as_bytes());
        }
        None => match std::fs::read(root.join(OVERLAY_FILE)) {
            Ok(bytes) => {
                h.write_u8(1);
                h.write(&bytes);
            }
            Err(_) => h.write_u8(0),
        },
    }
    h.finish()
}

/// xxh64 over every file under `<root>/.glia` but the layout dir
/// `.glia/graph`, by sorted relative path and content: the overlay, the
/// config and the snapshots (`glia history sync`, `tests ingest`, `scip
/// import`, `docs sync`) that a build reads at pass time.
fn glia_inputs_fp(root: &Path) -> u64 {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut stack = vec![(root.join(CONTROL_DIR), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if rel == "graph" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, rel));
            } else {
                files.push((rel, path));
            }
        }
    }
    files.sort();
    let mut h = XxHash64::with_seed(0);
    for (rel, path) in files {
        h.write(rel.as_bytes());
        h.write_u8(0);
        h.write(&std::fs::read(&path).unwrap_or_default());
        h.write_u8(0);
    }
    h.finish()
}

/// LB.1's `[repo-id]` line, as a build prints it.
fn repo_id_marker(ident: &RepoIdentity, label: &str) {
    eprintln!(
        "[repo-id] source={} key={} repo={label}",
        ident.source.as_str(),
        ident.key
    );
}

/// `d` in milliseconds truncated to one decimal, as `timing::ms` prints it.
fn ms(d: Duration) -> f64 {
    let tenths = d.as_micros() / 100;
    (tenths as f64) / 10.0
}

/// The `[watch] update` line (module doc).
fn update_marker(u: &WatchUpdate, groups: usize, took: Duration, label: &str) -> String {
    let c = &u.counts;
    format!(
        "[watch] update seq={} files +{} ~{} -{} groups_rebuilt={}/{groups} full_rebuild={} \
         nodes +{} -{} ~{} >{} edges +{} -{} ~{} took={} repo={label}",
        u.seq,
        u.files.added.len(),
        u.files.modified.len(),
        u.files.removed.len(),
        u.groups_rebuilt.len(),
        u.full_rebuild.as_deref().unwrap_or("none"),
        c.nodes_added,
        c.nodes_removed,
        c.nodes_modified,
        c.nodes_moved,
        c.edges_added,
        c.edges_removed,
        c.edges_reconfidenced,
        timing::ms(took),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// A repo of python, go (with a go.mod) and a README.
    fn repo(tmp: &Path) -> PathBuf {
        let dir = tmp.join("repo");
        write(
            &dir,
            "a.py",
            "def foo():\n    return bar()\n\ndef bar():\n    return 1\n",
        );
        write(&dir, "go.mod", "module example.com/fx\n");
        write(
            &dir,
            "m.go",
            "package m\n\nfunc F() int { return G() }\n\nfunc G() int { return 1 }\n",
        );
        write(&dir, "README.md", "# Fixture\n\nCall `foo`.\n");
        write(&dir, ".gitignore", "*.log\n");
        dir
    }

    /// Every file under `dir`, recursively, by relative path.
    fn layout_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
                    out.push((rel, std::fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    /// The session's layout equals a clean `generate_one` of the tree as it
    /// is, both persisted at the same depth under `tmp/out/<tag>` so their
    /// relative roots are equal.
    fn assert_clean(s: &WatchSession, repo: &Path, tmp: &Path, tag: &str) {
        let out = tmp.join("out").join(tag);
        s.persist(&out.join("watch")).unwrap();
        let clean = generate_one(repo.to_str().unwrap()).unwrap();
        crate::persist::persist_result(&clean, &out.join("clean"), "test").unwrap();
        let (w, c) = (
            layout_bytes(&out.join("watch")),
            layout_bytes(&out.join("clean")),
        );
        assert!(!w.is_empty());
        assert_eq!(w.len(), c.len(), "{tag}: same files");
        for ((wn, wb), (cn, cb)) in w.iter().zip(&c) {
            assert_eq!(wn, cn, "{tag}");
            assert!(wb == cb, "{tag}: {wn} differs from a clean build");
        }
        assert_eq!(
            (
                s.result().total_nodes,
                s.result().total_edges,
                &s.result().parse_errors
            ),
            (clean.total_nodes, clean.total_edges, &clean.parse_errors),
            "{tag}"
        );
    }

    use crate::build::generate_one;

    #[test]
    fn open_persists_like_a_clean_build() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        let s = WatchSession::open(dir.to_str().unwrap(), &BuildOptions::default()).unwrap();
        assert_eq!(s.seq(), 0);
        assert!(s.result().total_nodes > 0);
        assert_clean(&s, &dir, tmp.path(), "open");
        assert!(
            WatchSession::open(
                tmp.path().join("nope").to_str().unwrap(),
                &BuildOptions::default()
            )
            .is_err()
        );
    }

    #[test]
    fn body_edit_matches_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        let mut s = WatchSession::open(dir.to_str().unwrap(), &BuildOptions::default()).unwrap();
        write(
            &dir,
            "a.py",
            "def foo():\n    return bar() + 1\n\ndef bar():\n    return 1\n",
        );
        // An absolute path under the root, spelled as the caller gave the root.
        let u = s.apply(&[dir.join("a.py")]).unwrap();
        assert_eq!(u.seq, 1);
        assert_eq!(u.files.modified, ["a.py"]);
        assert!(u.files.added.is_empty() && u.files.removed.is_empty());
        assert!(
            u.files.reparsed >= 1 && u.files.reused >= 1,
            "{:?}",
            u.files
        );
        assert_eq!(u.groups_rebuilt, ["go", "python"]);
        assert_eq!(u.full_rebuild, None);
        assert!(
            u.nodes
                .iter()
                .any(|n| n.change == "modified" && n.qname.ends_with("foo")),
            "a modified row for foo: {:?}",
            u.nodes
        );
        assert_eq!(
            u.counts.nodes_modified,
            u.nodes.iter().filter(|n| n.change == "modified").count()
        );
        assert!(u.phase_ms.total >= u.phase_ms.delta);
        assert_eq!(
            (u.total_nodes, u.total_edges),
            (s.result().total_nodes, s.result().total_edges)
        );
        assert_clean(&s, &dir, tmp.path(), "body_edit");
    }

    #[test]
    fn add_delete_replan() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        let mut s = WatchSession::open(dir.to_str().unwrap(), &BuildOptions::default()).unwrap();
        write(&dir, "b.go", "package m\n\nfunc B() int { return F() }\n");
        let added = s.apply(&[PathBuf::from("b.go")]).unwrap();
        assert_eq!(
            (added.seq, added.files.added.clone()),
            (1, vec!["b.go".to_string()])
        );
        assert!(added.files.modified.is_empty() && added.files.removed.is_empty());
        assert!(
            added
                .nodes
                .iter()
                .any(|n| n.change == "added" && n.qname.ends_with("B")),
            "{:?}",
            added.nodes
        );
        assert_clean(&s, &dir, tmp.path(), "added");

        std::fs::remove_file(dir.join("b.go")).unwrap();
        let removed = s.apply(&[PathBuf::from("./b.go")]).unwrap();
        assert_eq!(
            (removed.seq, removed.files.removed.clone()),
            (2, vec!["b.go".to_string()])
        );
        assert!(
            removed
                .nodes
                .iter()
                .any(|n| n.change == "removed" && n.qname.ends_with("B"))
        );
        assert_clean(&s, &dir, tmp.path(), "removed");

        // A new directory with a file in it, named by its directory only.
        write(&dir, "pkg/c.py", "def c():\n    return 3\n");
        let dir_added = s.apply(&[PathBuf::from("pkg")]).unwrap();
        assert_eq!(dir_added.files.added, ["pkg/c.py"]);
        assert_clean(&s, &dir, tmp.path(), "dir_added");
        // The whole directory deleted, named by its directory only.
        std::fs::remove_dir_all(dir.join("pkg")).unwrap();
        let dir_removed = s.apply(&[PathBuf::from("pkg")]).unwrap();
        assert_eq!(dir_removed.files.removed, ["pkg/c.py"]);
        assert_clean(&s, &dir, tmp.path(), "dir_removed");
    }

    #[test]
    fn go_mod_is_a_full_rebuild() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        let mut s = WatchSession::open(dir.to_str().unwrap(), &BuildOptions::default()).unwrap();
        write(&dir, "go.mod", "module example.com/other\n");
        let u = s.apply(&[PathBuf::from("go.mod")]).unwrap();
        assert_eq!(u.full_rebuild.as_deref(), Some("go_modules"));
        assert_eq!(u.seq, 1);
        assert_eq!(u.files.modified, ["go.mod"]);
        assert_clean(&s, &dir, tmp.path(), "go_mod");
    }

    /// High inputs outside the walked files: the overlay (a full rebuild) and
    /// a `.gitignore` that collapses a directory into a region and back.
    #[test]
    fn high_inputs_replan() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        write(&dir, "pkg/c.py", "def c():\n    return 3\n");
        let mut s = WatchSession::open(dir.to_str().unwrap(), &BuildOptions::default()).unwrap();

        write(&dir, ".glia/overlay.toml", "version = 1\n");
        let u = s.apply(&[PathBuf::from(".glia/overlay.toml")]).unwrap();
        assert_eq!((u.seq, u.full_rebuild.as_deref()), (1, Some("overlay")));
        assert!(
            u.files.added.is_empty(),
            "the walk never reads .glia: {:?}",
            u.files
        );
        assert_clean(&s, &dir, tmp.path(), "overlay");

        write(&dir, ".gitignore", "*.log\npkg/\n");
        let u = s.apply(&[PathBuf::from(".gitignore")]).unwrap();
        assert_eq!(u.files.removed, ["pkg/c.py"]);
        assert_eq!(u.full_rebuild, None);
        assert_clean(&s, &dir, tmp.path(), "ignored");
        // A file inside the collapsed region: dropped two levels down, a
        // re-plan (and a noop) as the region root's direct entry.
        write(&dir, "pkg/sub/d.py", "def d():\n    return 4\n");
        assert_eq!(s.apply(&[PathBuf::from("pkg/sub/d.py")]).unwrap().seq, 2);
        assert_eq!(s.apply(&[PathBuf::from("pkg/sub")]).unwrap().seq, 2);

        write(&dir, ".gitignore", "*.log\n");
        let u = s.apply(&[PathBuf::from(".gitignore")]).unwrap();
        assert_eq!(u.files.added, ["pkg/c.py", "pkg/sub/d.py"]);
        assert_clean(&s, &dir, tmp.path(), "unignored");
    }

    #[test]
    fn noop_keeps_seq() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        let mut s = WatchSession::open(dir.to_str().unwrap(), &BuildOptions::default()).unwrap();
        let totals = (s.result().total_nodes, s.result().total_edges);
        let noop = |u: &WatchUpdate| {
            u.seq == 0
                && u.files.added.is_empty()
                && u.files.modified.is_empty()
                && u.files.removed.is_empty()
                && u.groups_rebuilt.is_empty()
                && u.nodes.is_empty()
                && u.edges.is_empty()
                && (u.total_nodes, u.total_edges) == totals
        };
        // An untouched file, a gitignored new file (one re-plan), a path in
        // the layout dir, ones under hard-skipped dirs (`.idea/`, `.git/`: no
        // `.git` is written, which would move the clean build's identity), one
        // outside the root, nothing.
        write(&dir, "debug.log", "noise\n");
        write(&dir, ".idea/workspace.xml", "<x/>");
        for paths in [
            vec![PathBuf::from("a.py")],
            vec![PathBuf::from("debug.log")],
            vec![PathBuf::from(".glia/graph/manifest.json")],
            vec![
                PathBuf::from(".idea/workspace.xml"),
                PathBuf::from(".git/objects/ab"),
            ],
            vec![tmp.path().join("elsewhere.py")],
            vec![],
        ] {
            let u = s.apply(&paths).unwrap();
            assert!(noop(&u), "{paths:?}: {u:?}");
        }
        // A real edit still rebuilds after them.
        write(&dir, "a.py", "def foo():\n    return 2\n");
        assert_eq!(s.apply(&[PathBuf::from("a.py")]).unwrap().seq, 1);
        assert_clean(&s, &dir, tmp.path(), "after_noops");
    }

    #[test]
    fn durability_table() {
        let high = [
            (".glia/overlay.toml", "glia_inputs"),
            (".glia/scip-snapshot/index.json", "glia_inputs"),
            (".glia", "glia_inputs"),
            (".gitignore", "gitignore"),
            ("web/.gitignore", "gitignore"),
            ("svc/go.mod", "go_mod"),
            ("tsconfig.json", "tsconfig"),
            ("app/tsconfig.base.json", "tsconfig"),
            ("CMakeLists.txt", "include_roots"),
            ("build/compile_commands.json", "include_roots"),
            ("package.json", "manifest"),
            ("api/Api.csproj", "manifest"),
            ("App.sln", "manifest"),
            ("build.sbt", "manifest"),
            ("tools/requirements.txt", "manifest"),
            ("lib/x.gemspec", "manifest"),
            ("db/migrations/V1__init.sql", "migration"),
            (".git/HEAD", "identity"),
            (".git/config", "identity"),
            (".git", "identity"),
        ];
        for (rel, why) in high {
            assert_eq!(durability(rel), Durability::High(why), "{rel}");
        }
        for rel in [
            "src/main.rs",
            "a.py",
            "docs/guide.md",
            ".glia/graph/manifest.json",
            "report.sql",
        ] {
            assert_eq!(durability(rel), Durability::Low, "{rel}");
        }
    }

    /// A session moves between threads (a pyo3 class, a watcher thread).
    #[test]
    fn session_is_send() {
        fn send<T: Send>() {}
        send::<WatchSession>();
        send::<WatchUpdate>();
    }

    /// Release timing harness for DC.7a's acceptance (3), run by hand on a
    /// `git archive` copy (it writes into the repo it names, its parse cache
    /// sidecar included):
    /// `GLIA_WATCH_BENCH=<repo>::<repo-relative file> cargo test --release -p glia-engine --lib watch::tests::bench_one_line_edit -- --ignored --nocapture`.
    /// Opens a session, then three times appends one comment line to the file
    /// and applies it, printing each update's phases, each followed by the
    /// disk-incremental build of the same tree (`generate_one_incremental`:
    /// sidecar load, build, save; its `[timing] build` line is the S1 warm1
    /// column), so both are measured under the same machine load. Restores
    /// the file.
    #[test]
    #[ignore = "release timing harness: set GLIA_WATCH_BENCH=<repo>::<file>; writes into that repo"]
    fn bench_one_line_edit() {
        let Ok(spec) = std::env::var("GLIA_WATCH_BENCH") else {
            return;
        };
        let (repo, rel) = spec
            .split_once("::")
            .expect("GLIA_WATCH_BENCH=<repo>::<file>");
        let path = Path::new(repo).join(rel);
        let original = std::fs::read(&path).unwrap();
        let comment = if rel.ends_with(".py") { "#" } else { "//" };
        // The warm-up: writes the sidecar the incremental builds below load.
        crate::build::generate_one_incremental(repo).unwrap();
        let opened = Instant::now();
        let mut s = WatchSession::open(repo, &BuildOptions::default()).unwrap();
        eprintln!("[watch-bench] open took={}", timing::ms(opened.elapsed()));
        for i in 1..=3 {
            let mut text = original.clone();
            text.extend_from_slice(format!("\n{comment} watch bench edit {i}\n").as_bytes());
            std::fs::write(&path, &text).unwrap();
            let u = s.apply(&[PathBuf::from(rel)]).unwrap();
            eprintln!(
                "[watch-bench] update {i} files ~{} reparsed={} reused={} phase_ms={}",
                u.files.modified.len(),
                u.files.reparsed,
                u.files.reused,
                serde_json::to_string(&u.phase_ms).unwrap()
            );
            let incremental = Instant::now();
            crate::build::generate_one_incremental(repo).unwrap();
            eprintln!(
                "[watch-bench] incremental {i} wall={}",
                timing::ms(incremental.elapsed())
            );
        }
        std::fs::write(&path, &original).unwrap();
    }
}
