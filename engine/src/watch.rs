//! `WatchSession`: an incremental build held in memory, updated from changed
//! paths or a stat poll, persisted on demand. Public module slot, reached by
//! module path (`glia_engine::watch::WatchSession`). Filled by DC.7a (+DC.7b,
//! DC.7c).
//!
//! A session is one repo's build kept open (DC.7a): the walk plan and its file
//! table (every queued file's class, `Read` and stat stamp), the repo-level
//! inputs the build reads besides the walked files, one [`ParseCache`] with its
//! DC.4 build memos, and the last [`GenerateResult`]. [`WatchSession::apply`]
//! takes the paths the caller saw change and rebuilds through the same DC.2
//! stage calls `generate_one_inner` makes (parse and graft, plan / build / fold
//! the groups, the per-repo markers, the region / project / docs graphs, the
//! pass tail), so every update's graph is a clean build of the tree as the
//! session read it, by construction. DC.7c adds the stat poll.
//!
//! Group early cutoff (DC.7b): a language group's pre-pass graph is a pure
//! function of its ordered post-graft parses, the repo id and the repo-level
//! inputs its builder reads (`hidden_fp`). The session keeps every group's
//! last build (`CachedGroup`) under a fingerprint of exactly those inputs
//! (each parse's canonical bincode through `cache::parse_fingerprint`, taken
//! after the grafts, on the pool), plus the proto service set (so its change
//! rebuilds every group, as its `full_rebuild` reason says). GA.17: the
//! overlay is no part of it: every overlay section reaches a group only
//! through the walk or the post-cache grafts, so through the post-graft
//! parses, or not at all (a pass-time or query-time section), and an overlay
//! edit (`full_rebuild=overlay`, still the full delta) rebuilds only the
//! groups whose parses it moved. An update rebuilds the groups whose
//! fingerprint moved and reuses a clone of the others' cached builds. Every
//! group costs one deep clone per update: the passes mutate the merged graph (node confidence, cells, intra
//! edge evidence), so a cached pre-pass graph is never handed over by
//! reference; a reused build is cloned out of the cache, a rebuilt one is
//! cloned in before the fold moves its graph into the merged graph. A Low
//! update with no `full_rebuild` reason and no SCIP snapshot (which
//! reconfidences intra edges of any graph) then takes the located delta over
//! the rebuilt groups' graphs and the region / project / docs graphs only
//! (`delta::located_delta_scoped`); every other update takes the full one.
//!
//! Body-only fast path (GA.4, R2-watch section 5 A2), the in-group cutoff
//! below the group one. A group whose language has a fast path
//! (`lang_build::fast_path_for`: python, go and the TypeScript family, through
//! `glia_graph::rebuild_python_bodies` (GA.3), `rebuild_go_bodies` (GA.7) and
//! `rebuild_ts_bodies` (GA.6); Rust has none, FA.1's verdict) also keeps each
//! parse's file, parse fingerprint and declaration-surface fingerprint
//! (`cache::surface_fingerprint`,
//! GA.1; only a changed file's surface is fingerprinted again). When such a
//! group's fingerprint moved but it reads the same files in the same order,
//! its fingerprint's other inputs (the overlay, the proto service set, the
//! builder's `hidden_fp`) are unchanged and every file whose parse moved kept
//! its surface, the update clones the cached build and re-stitches only the
//! edited files against it (`lang_build::rebody_group`: the graph crate
//! re-checks the surface and refuses, untouched, when it moved; then the
//! evidence stamp, the canonical order, the tallies and the evidence fill a
//! build makes). A refusal builds the group as before. Either way the group
//! is in `groups_rebuilt` (its graph moved); `body_only` lists the ones the
//! fast path made. The graph is a clean build's either way (the parity
//! gates, engine/tests/watch_parity.rs). `GLIA_WATCH_FAST_PATH=0`, read at
//! [`WatchSession::open`], turns it off.
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
//! Stat poll (DC.7c, no notify crate): [`WatchSession::changed_paths`] stats
//! (a) every table entry (`len`, mtime), (b) re-reads under the racy rule
//! (`Window`) each entry whose recorded mtime could hide a same-length
//! rewrite inside one timestamp tick, (c) stats every walked directory and
//! region root (an entry added, removed or renamed moves its directory's
//! mtime, and a region root's entries are what `gate_dir` reads) and (d) the
//! inputs a build reads besides the table (`Input`: the `.gitignore`s, the
//! manifests root detection and a re-plan read that are no table entry, the
//! `CMakeLists.txt` / `compile_commands.json` the include roots read, every
//! file under `.glia/` but the layout, the git identity files). A directory
//! or watched input inside the racy window is checked against a fingerprint
//! of its listing or bytes taken right before the update that last read it
//! (`RepoWatch::capture`); one with none is reported, and the update it
//! triggers takes one. [`WatchSession::poll`] is the scan, then
//! [`WatchSession::apply`] of what it found. The watermark the racy window
//! hangs from moves only when a scan finds nothing, so a recorded value is
//! never trusted past the last time all of them were checked.
//!
//! Several repos (GA.9). [`WatchSession::open_many`] holds one joint build
//! of several roots, composed exactly as `generate_many` composes it: each
//! root walked, the identities disambiguated against each other, one proto
//! service set over every repo's files, each repo's DC.2 stages appended in
//! argument order, one pass tail. [`WatchSession::apply`] routes each
//! changed path to the one repo whose root holds it (roots never nest:
//! `open_many` refuses nested or repeated roots); each repo with a named
//! path re-reads or re-plans as above (durability is per repo), every other
//! repo re-reads nothing and its stages run on its table's reads (its parse
//! cache entries, memos and cached groups make it a cache hit throughout), so
//! every update is a clean `generate_many` of the trees as the session read
//! them. A moved joint service set is the `proto_services` full rebuild of
//! every repo's groups; a re-read identity is disambiguated against every
//! root again. In a multi-repo session `groups_rebuilt` entries read
//! `<label>:<key>` and `WatchFiles` paths `<label>:<rel>`. Every repo's walk
//! table, cache and group cache is held, and the memory budget covers the
//! whole session.
//!
//! fired_on markers, on stderr:
//! - `[watch] open repo=<label> files=<n> groups=<g> nodes=<N> edges=<E> took=<ms>`
//!   once per [`WatchSession::open`]; `files` counts the walked sources and docs
//!   the build read, `groups` the language groups. A multi-repo
//!   [`WatchSession::open_many`] prints
//!   `[watch] open repos=<n> repo=<l1,l2,..> files=<n> groups=<g> nodes=<N> edges=<E> took=<ms>`
//!   (GA.9; `files` and `groups` summed over the repos).
//! - `[watch] update seq=<s> files +<a> ~<m> -<r> groups_rebuilt=<k>/<g> body_only=<k1,k2|-> full_rebuild=<reason|none> nodes +<a> -<d> ~<m> ><v> edges +<a> -<r> ~<c> restamped=<t> took=<ms> repo=<label>`
//!   once per update that rebuilt (`k` the language groups rebuilt of the
//!   plan's `g`, `body_only` those of them the fast path made, GA.4, `>`
//!   counts moved nodes, `~` on edges the reconfidenced ones, `restamped`
//!   the edges whose EVIDENCE emitter or rule changed at the same
//!   confidence, GA.18: rows with change `restamped`);
//!   GA.9: `repo=` lists the repos with named paths, comma-joined.
//! - `[watch] fast_path key=<k> files=<n> ms=<t> repo=<label>` per group
//!   that took the body-only fast path (GA.4: `n` files re-stitched in `t`
//!   ms), and `[watch] fast_path refused key=<k> reason=<r> repo=<label>`
//!   per group whose re-stitch was refused and that was built instead.
//! - `[watch] noop repo=<label> paths=<n>` for an update that changed nothing
//!   (`n` = the paths it was given; `repo=` the repos with named paths, or
//!   every root when none had one), and `[watch] outside repo=<label> paths=<n>`
//!   when some given paths lie under no root (`repo=` every root).
//! - `[watch] poll files=<n> dirs=<d> watched=<w> changed=<c> racy=<k> took=<ms> repo=<label>`
//!   once per repo whose [`WatchSession::changed_paths`] scan found something: `n`
//!   table entries, `d` directories and region roots and `w` watched inputs
//!   checked, `c` paths reported, `k` checks the racy rule made (an entry
//!   re-read, a listing or input fingerprinted, or one reported for want of
//!   a fingerprint).
//!
//! - `[watch] rss seq=<s> rss_mb=<m> budget_mb=<b|none> cached_groups=<k>/<g> evicted=<e> memo_cleared=<0|1> repo=<label>`
//!   (GA.12) once at open, once per update that rebuilt and once per
//!   [`WatchSession::set_memory_budget`], after the memory budget was
//!   enforced (module doc, "Memory budget"): the process's resident set then
//!   (`?` when unreadable), the budget, the cached language groups left of
//!   the plan's, the groups evicted and whether the build memos were cleared
//!   by this enforcement.
//!
//! Memory budget (GA.12). What a session holds beyond its graph is about
//! three graphs' worth (R2-watch section 13): every group's cached pre-pass
//! build, the parse cache's entries and DC.4 build memos, and the result.
//! The cached groups and the memos are pure caches (a hit replays exactly
//! what a miss computes), so dropping one changes an update's time, never its
//! bytes. With a [`MemoryBudget`] (`GLIA_MEMORY_BUDGET_MB`, read at
//! [`WatchSession::open`], or [`WatchSession::set_memory_budget`]) the
//! session reads the process's RSS after each update's trim (and at open):
//! while it is over the budget it evicts (a) one cached group at a time, the
//! least recently built or reused first (`CachedGroup::last_used`, the `seq`
//! of that update; ties by repo, then plan order), then (b) every repo's
//! build memos at once, trimming (`release_freed_memory("watch_budget")`)
//! and re-reading RSS after each step, until it is within the budget or
//! nothing evictable is left. The parse cache's entries are never evicted:
//! they are what [`WatchSession::save_cache`] writes and a re-parse is the
//! dearest miss. Neither is the ranking cache (EA.7): it is replaced after
//! every update that changed the graph and holds at most 8 PPR indexes. An
//! evicted group has no reuse slot at the next update and rebuilds
//! (`[groups] built=` and `groups_rebuilt` show it). The budget is a process
//! target: RSS counts whatever else the process holds (memory.rs's module
//! doc), so a budget below the floor (one graph, the parse cache, the host)
//! evicts everything at every update. Without an RSS reading nothing is
//! evicted. [`WatchSession::memory`] reports what is held.
//!
//! An update also prints the stage markers of what it ran (`[timing] repo=`,
//! `[timing] build`, `[graft-memo]`, the walk's lines on a re-plan, ...): never
//! gate on marker equality with a clean build (R2-watch section 8).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::hash::Hasher as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use glia_code_domain::external_inputs::VECTORS_FILE;
use glia_code_domain::glia_config::{LoadedConfig, OVERLAY_FILE};
use glia_code_domain::project_roots::{self, ProjectRoot};
use glia_code_domain::snapshots::scip_dir;
use glia_code_domain::walk_gating::{self, CONTROL_DIR, RepoIdentity, repo_identity};
use glia_code_domain::{FileParse, evidence, node_kind};
use glia_code_extractors::{grpc, migrations, packages};
use glia_core::RepoId;
use glia_graph::RepoGraph;
use serde::Serialize;
use twox_hash::XxHash64;

use crate::build::assemble::{RepoBuildCtx, parse_and_graft, repo_markers};
use crate::build::c_includes::IncludeRoots;
use crate::build::lang_build::{
    BUNDLER_CONFIGS, GroupCtx, GroupInput, GroupPlan, LangBuild, LanguageGraphs, Made, Seed,
    TsAliasSet, build_groups_seeded, fast_path_for, fold_groups, is_bundler_config, plan_groups,
};
use crate::build::rpc_needles::RpcContext;
use crate::build::timing::{self, BuildTimes, PhaseTimes};
use crate::build::{
    BuildOptions, Finished, GenerateResult, aux_graphs, canonical_display, disambiguate,
    finish_build, go_module_dirs, go_modules_for,
};
use crate::cache::{ParseCache, parse_fingerprint, surface_fingerprint};
use crate::calibrate::record::CALIBRATION_DIR;
use crate::delta::{
    DeltaCounts, DeltaEdge, DeltaNode, DeltaScope, located_delta_counted, located_delta_scoped,
};
use crate::external::{RepoInputs, repo_inputs};
use crate::extract::{GoModules, TS_FAMILY, detect_language};
use crate::memory::{self, MemoryBudget};
use crate::passes::GroupFill;
use crate::rank::{RankCache, RankCacheStats};
use crate::walk::{
    FileStamp, Pending, Read, ReadRecord, RegionAnchor, Reuse, WalkPlan, WalkedDir, plan_walk,
    read_one, read_walk,
};

/// DC.7c: the racy rule's window ([`Window`]): git's racy-index rule,
/// widened from one timestamp tick to 2 s so a filesystem whose file times
/// tick in whole seconds (or two) is covered too.
const RACY_WINDOW: Duration = Duration::from_secs(2);

/// DC.7c: the names besides [`project_roots::MANIFESTS`] a re-plan reads at
/// a project root (`TsAliasSet::read`).
const TSCONFIGS: [&str; 2] = ["tsconfig.json", "tsconfig.base.json"];

/// DC.7c: the C/C++ build files `IncludeRoots::read` reads off disk.
const CMAKE_LISTS: &str = "CMakeLists.txt";
const COMPILE_COMMANDS: &str = "compile_commands.json";

/// DC.7c: the identity files `repo_identity` (and the recorded rev) reads at
/// the root. `.git` is watched only while it is a file (a worktree or
/// submodule pointer): a `.git` directory's own mtime moves with every git
/// command, and `.git/HEAD` appearing or going tells a checkout's.
const IDENTITY_FILES: [&str; 3] = [".git", ".git/HEAD", ".git/config"];

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
/// - `glia_inputs`: `.glia` and everything under it but `.glia/graph/**`,
///   `.glia/calibration-snapshot/**` and the query-time inputs
///   `.glia/vectors.jsonl` / `.glia/vocab-snapshot/**` (FB.3), which no build
///   reads (the overlay, the
///   `[walk]` / `[[project]]` config, the snapshots passes read);
/// - `gitignore`: any `.gitignore`;
/// - `go_mod`: any `go.mod` (the module set, so the parse cache's context);
/// - `tsconfig`: `tsconfig.json` / `tsconfig.base.json` (the TS path aliases);
/// - `bundler`: a webpack / vite / vitest config (`webpack.config.{js,cjs,mjs,ts}`,
///   `vite.config.{js,ts,mjs}`, `vitest.config.{js,ts,mjs,cjs,mts}`: the
///   bundler aliases and module roots, FH.16, FH.16b);
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
        return if rest == "graph"
            || rest.starts_with("graph/")
            || rest == CALIBRATION_DIR
            || rest
                .strip_prefix(CALIBRATION_DIR)
                .is_some_and(|r| r.starts_with('/'))
            || is_query_time_rel(rest)
        {
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
        _ if is_bundler_config(base) => Some("bundler"),
        "CMakeLists.txt" | "compile_commands.json" => Some("include_roots"),
        _ if is_project_manifest(base) || packages::is_manifest_path(rel) => Some("manifest"),
        _ if migrations::is_migration_path(rel) => Some("migration"),
        _ => None,
    };
    reason.map_or(Durability::Low, Durability::High)
}

/// `.glia`-relative `rest` is a query-time input no build reads (FB.3): the
/// vectors sidecar (the semantic tier reads it, FB.4) or the identifier
/// vocabulary snapshot (FB.13a), so a write to it is no re-plan and no input
/// change (the layout fingerprint skips the same paths, `glia_store`'s
/// `is_query_time_input`). The sidecar writer's `vectors.jsonl.tmp` (written,
/// then renamed over the file) counts too, so a poll that catches it mid-write
/// re-plans nothing (the fingerprint skips every `*.tmp` already).
fn is_query_time_rel(rest: &str) -> bool {
    let vectors = VECTORS_FILE
        .strip_prefix(CONTROL_DIR)
        .map(|r| r.trim_start_matches('/'));
    vectors.is_some_and(|v| rest == v || rest.strip_prefix(v) == Some(".tmp"))
        || rest == VOCAB_SNAPSHOT_DIR
        || rest
            .strip_prefix(VOCAB_SNAPSHOT_DIR)
            .is_some_and(|r| r.starts_with('/'))
}

/// The identifier vocabulary snapshot's directory under `.glia` (FB.13a).
const VOCAB_SNAPSHOT_DIR: &str = "vocab-snapshot";

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
/// whole [`WatchSession::apply`] call. DC.7b: `fingerprint` is the group
/// fingerprints (every post-graft parse's, on the pool, and the group keys)
/// and `clone` the group cache's deep clones (the reused builds cloned out,
/// the rebuilt ones cloned in); `language_build` (plan, build and fold, also
/// the `[timing] repo=` line's) leaves both out, so the phases add up.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, Copy, Default, PartialEq)]
pub struct WatchPhases {
    pub read: f64,
    pub walk: f64,
    pub parse: f64,
    pub const_scan: f64,
    pub grafts: f64,
    pub language_build: f64,
    pub fingerprint: f64,
    pub clone: f64,
    pub resolve: f64,
    pub post: f64,
    pub finalize: f64,
    pub external_cells: f64,
    pub delta: f64,
    pub total: f64,
}

/// One [`WatchSession::apply`]'s answer: the session's `seq` after it (left
/// unchanged by an update that changed nothing), the files that moved, the
/// language groups rebuilt (plan keys in plan order: the groups whose
/// fingerprint moved, DC.7b; the others reuse their cached build) and those
/// of them the body-only fast path re-stitched (GA.4), the reason
/// the update counts as a full rebuild (`identity`, `go_modules`, `overlay`,
/// `proto_services`; `None` otherwise; every group rebuilds under all but
/// `go_modules`, which re-parses every file and still reuses a group whose
/// parses come back equal), the new totals, and the located delta against
/// the previous graph (`delta::located_delta_counted`, or
/// `delta::located_delta_scoped` on a Low update: the same rows, sorted as
/// `glia delta` sorts them, with their counts).
#[non_exhaustive]
#[derive(Serialize, Debug, Clone)]
pub struct WatchUpdate {
    pub seq: u64,
    pub files: WatchFiles,
    pub groups_rebuilt: Vec<String>,
    /// GA.4: the groups of `groups_rebuilt` that took the body-only fast
    /// path (their edited files re-stitched against the cached build), in
    /// plan order, tagged like `groups_rebuilt`.
    pub body_only: Vec<String>,
    pub full_rebuild: Option<String>,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub counts: DeltaCounts,
    pub nodes: Vec<DeltaNode>,
    pub edges: Vec<DeltaEdge>,
    pub phase_ms: WatchPhases,
}

/// One repo's build, or several repos' joint build (GA.9), held open: see
/// the module doc. Made by [`WatchSession::open`] or
/// [`WatchSession::open_many`]; every field is private, so a new one is not
/// a break.
///
/// The session holds no lock on the repo. [`WatchSession::persist`] writes
/// against the files on disk: CD.7c stores a CODE cell as a span only when its
/// text still matches the file, so persist right after an update.
///
/// The per-repo state (walk table, repo-level inputs, parse cache, group
/// cache, watched inputs) lives in one `RepoWatch` per repo; the session
/// holds the joint build, its `seq`, the racy watermark, the build's proto
/// service set and the ranking cache. [`WatchSession::open`] makes exactly
/// one repo, [`WatchSession::open_many`] one per root, and nothing removes
/// one.
pub struct WatchSession {
    /// The repos watched, in build order (the roots' argument order): one
    /// from [`WatchSession::open`], one per root from
    /// [`WatchSession::open_many`].
    repos: Vec<RepoWatch>,
    opts: BuildOptions,
    result: GenerateResult,
    seq: u64,
    /// The build's proto service set ([`proto_services_fp`]): the union over
    /// every repo's walked files, as `generate_many` folds its `RpcContext`.
    services_fp: u64,
    /// DC.7c: the racy rule's watermark: every record, directory listing and
    /// watched input of every repo was read, or stat'ed and found unchanged,
    /// at or after it. Set at open (before any read) and moved only by a scan
    /// that finds nothing, never by an update: an update re-reads only what
    /// it was named (a re-plan reuses every other entry's read under its
    /// recorded stamp), so moving it there would age an entry out of the
    /// window unchecked. Behind a lock because
    /// [`WatchSession::changed_paths`] takes `&self`.
    scanned_at: Mutex<SystemTime>,
    /// 0.5.3 (EA.7): the ranking indexes of `result`'s graph (holder
    /// `watch`), shared with every snapshot of it ([`WatchSession::rank_cache`]).
    /// Built empty at open and replaced by a fresh one right after an update
    /// swaps `result`, so it never serves an index of an older graph; a noop
    /// update keeps it, since the graph did not change.
    rank: Arc<RankCache>,
    /// GA.12: the memory budget the caches are evicted against (module
    /// doc, "Memory budget"): [`MemoryBudget::from_env`] at open.
    budget: MemoryBudget,
    /// GA.12: cached groups evicted for the budget since open.
    evictions: u64,
    /// GA.12: times the build memos were cleared for the budget since open.
    memo_clears: u64,
    /// GA.4: whether a rebuilt group may take the body-only fast path
    /// (module doc, "Body-only fast path"): [`fast_path_from_env`] at open.
    fast_path: bool,
}

/// GA.12: what a [`WatchSession`] holds, from [`WatchSession::memory`]:
/// the process's resident set (`None` where `/proc/self/status` does not
/// give it), the budget, the cached language group builds of the plan's
/// groups, the parse cache's entries and the paths its DC.4 build memos
/// hold (summed over the session's repos), the evictions the budget made
/// since open, and the ranking cache's counters (EA.7; never evicted).
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchMemory {
    pub rss_kb: Option<u64>,
    pub budget_mb: Option<u64>,
    pub cached_groups: usize,
    pub groups: usize,
    pub cache_entries: usize,
    pub memo_paths: usize,
    pub evictions: u64,
    pub memo_clears: u64,
    pub rank: RankCacheStats,
}

/// One repo of a [`WatchSession`]: its walk plan and file table, the
/// repo-level inputs, the parse cache, the language group cache and the
/// watched inputs with their racy baselines.
struct RepoWatch {
    /// The root as the caller gave it.
    root: PathBuf,
    /// The root's absolute and canonical spellings, which an absolute changed
    /// path is made relative against.
    abs_roots: Vec<PathBuf>,
    /// The root as given, as a string: every marker's `repo=`, the result's
    /// label and root, the parse cache's location.
    label: String,
    plan: WalkPlan,
    /// One record per `plan.pending`, index for index (walk order).
    records: Vec<ReadRecord>,
    /// `records` by repo-relative path.
    index: BTreeMap<String, usize>,
    regions: Vec<RegionAnchor>,
    roots: Vec<ProjectRoot>,
    setup: Setup,
    /// GA.9: the root's own identity, before [`disambiguate`] suffixed a key
    /// another root of the session shares (`setup.ident` is the
    /// disambiguated one the build keys on); an update that re-reads no
    /// identity path re-disambiguates from it.
    ident_raw: RepoIdentity,
    cache: ParseCache,
    /// DC.7b: every language group's last pre-pass build whose fingerprint
    /// could be taken, in plan order; GA.12 evicts from it past the budget.
    groups: Vec<CachedGroup>,
    /// GA.12: the language groups of the last build's plan (the `<g>` of
    /// `[watch] rss .. cached_groups=<k>/<g>`).
    group_count: usize,
    /// The plan key of each language graph of the session's `result.merged`,
    /// index for index (`LanguageGraphs::keys`); the region / project / docs
    /// graphs follow them.
    graph_keys: Vec<&'static str>,
    /// GA.9: how many graphs of `result.merged` this repo holds (language,
    /// then region / project / docs); a repo's first graph index is the sum
    /// of the counts of the repos before it.
    graph_count: usize,
    /// DC.7c: every input a build reads besides the table ([`Input`]), by
    /// repo-relative path: its class and its stat as of the update that last
    /// read it (`None`: absent, or not a file). A class an update did not
    /// re-read keeps its entries, so a change nobody named stays visible.
    watched: BTreeMap<String, Watched>,
    /// DC.7c: the fingerprints [`RepoWatch::capture`] took right before
    /// the update that last read each class: of the walked directories
    /// ([`dir_fp`], class [`Input::Walk`]) and watched inputs ([`bytes_fp`])
    /// that were inside the racy window then.
    baselines: BTreeMap<String, (Input, u64)>,
}

/// DC.7c: which update re-reads a watched input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    /// The walk's descent, re-read by every re-plan: a `.gitignore`
    /// (`IgnoreStack::push_dir`), a [`project_roots::MANIFESTS`] file (root
    /// detection reads its text) or a [`TSCONFIGS`] / `BUNDLER_CONFIGS`
    /// (FH.16) file in an entered
    /// directory that is no table entry (a walk-skipped name, or a
    /// gitignored file: root detection lists ignored files too), a region
    /// root's `.git` file (`gate_dir` reads it: submodule or worktree). Also
    /// the class of the directory baselines.
    Walk,
    /// `IncludeRoots::read`, re-read on a re-plan or a changed file list:
    /// the `CMakeLists.txt` of every directory above a walked C/C++ file and
    /// every `compile_commands.json` it looks for (c_includes'
    /// `HeldDirs` and `compile_commands_dirs`, mirrored).
    Includes,
    /// Every file under `.glia/` but `.glia/graph/` (the overlay, the
    /// snapshots), re-read when the update holds a `.glia` path.
    Glia,
    /// [`IDENTITY_FILES`], re-read when the update holds one.
    Identity,
}

/// DC.7c: one watched input ([`RepoWatch::watched`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Watched {
    class: Input,
    stamp: Option<FileStamp>,
}

/// DC.7c: the [`Input`] classes one update re-read: a re-plan re-reads the
/// walk's and the include roots', a changed file list the include roots', a
/// High `glia_inputs` / `identity` path its own (`Setup::refresh`).
#[derive(Clone, Copy)]
struct Consumed {
    walk: bool,
    includes: bool,
    glia: bool,
    identity: bool,
}

impl Consumed {
    const ALL: Self = Self {
        walk: true,
        includes: true,
        glia: true,
        identity: true,
    };

    fn has(self, class: Input) -> bool {
        match class {
            Input::Walk => self.walk,
            Input::Includes => self.includes,
            Input::Glia => self.glia,
            Input::Identity => self.identity,
        }
    }
}

/// DC.7c: the racy rule. A stat that matches its record proves the content
/// unchanged only when the recorded mtime is older than every check since
/// by more than a timestamp tick: a rewrite inside the tick the record was
/// taken in keeps (`len`, mtime). So an mtime from `scanned_at` minus
/// [`RACY_WINDOW`] on is racy, up to [`RACY_WINDOW`] past the scan's own
/// clock (a write before the scan is stamped no later than the scan, so a
/// far-future mtime cannot hide one yet). An unknown mtime is racy.
#[derive(Clone, Copy)]
struct Window {
    from: SystemTime,
    to: SystemTime,
}

impl Window {
    fn at(scanned_at: SystemTime, now: SystemTime) -> Self {
        Self {
            from: scanned_at
                .checked_sub(RACY_WINDOW)
                .unwrap_or(SystemTime::UNIX_EPOCH),
            to: now.checked_add(RACY_WINDOW).unwrap_or(now),
        }
    }

    fn racy(self, mtime: Option<SystemTime>) -> bool {
        mtime.is_none_or(|m| m >= self.from && m <= self.to)
    }
}

/// DC.7c: one stat scan ([`RepoWatch::scan`]): the repo-relative paths it
/// found changed (sorted, deduplicated), what it checked, the racy checks it
/// made, when it started and what it took.
struct Scan {
    found: BTreeSet<String>,
    files: usize,
    dirs: usize,
    watched: usize,
    racy: usize,
    started: SystemTime,
    took: Duration,
}

/// One language group's pre-pass build (DC.7b), kept for the next update
/// under the fingerprint of the inputs it was built from
/// ([`group_fingerprints`]).
struct CachedGroup {
    key: &'static str,
    fingerprint: u64,
    /// GA.4: the fingerprint's inputs besides the parses ([`GroupPrint`]'s
    /// `context`): a fast path needs them unchanged.
    context: u64,
    /// GA.4: each parse's path and fingerprints, in plan order, for a group
    /// with a fast path ([`GroupPrint`]'s `files`); `None` otherwise.
    files: Option<Vec<FileFp>>,
    build: LangBuild,
    /// GA.12: the session `seq` of the update (0: open) that last built or
    /// reused this group: the memory budget evicts the lowest first.
    last_used: u64,
}

/// Where the refreshed group cache takes a group's entry from
/// ([`refresh_groups`]): the previous cache's entry `i`, reused as it is, or
/// a clone of this update's build.
enum Slot {
    Kept(usize),
    Fresh(Box<CachedGroup>),
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

/// One repo's root, label and options, as the stages read them, and the
/// overlay text it parses in place of `.glia/overlay.toml`: `opts.overlay_text`
/// for the session's first repo only, as `generate_many` hands it to its
/// first path alone (GA.9).
struct At<'a> {
    root: &'a Path,
    label: &'a str,
    opts: &'a BuildOptions,
    overlay_text: Option<&'a str>,
}

impl<'a> At<'a> {
    /// The stages' view of `repo`, the session's `slot`-th repo.
    fn of(repo: &'a RepoWatch, slot: usize, opts: &'a BuildOptions) -> Self {
        Self {
            root: &repo.root,
            label: &repo.label,
            opts,
            overlay_text: overlay_text_for(slot, opts),
        }
    }
}

/// The overlay text the `slot`-th repo of a build parses (`generate_many`'s
/// rule): `opts.overlay_text` for slot 0, none for the rest.
fn overlay_text_for(slot: usize, opts: &BuildOptions) -> Option<&str> {
    if slot == 0 {
        opts.overlay_text.as_deref()
    } else {
        None
    }
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

/// One repo's input to a joint [`build`]: its tree and, when this update
/// moved them, its repo-level inputs (`None`: the repo's committed
/// [`Setup`]). `updated`: the tree came from this update's reads (a re-read
/// or a re-plan), so it is committed after the build; otherwise it is the
/// table as it stands ([`RepoWatch::current_tree`]) and nothing of it is.
struct Work {
    tree: Tree,
    setup: Option<Setup>,
    updated: bool,
}

/// One joint build through the DC.2 stages ([`build`]).
struct Built {
    result: GenerateResult,
    /// Per repo, in build order.
    repos: Vec<RepoBuilt>,
    times: BuildTimes,
}

/// One repo's part of a [`Built`].
struct RepoBuilt {
    /// The plan's group keys, in plan order.
    groups: Vec<&'static str>,
    /// The keys of the groups built (not reused), in plan order.
    rebuilt: Vec<&'static str>,
    /// GA.4: the keys of `rebuilt` that took the body-only fast path.
    body_only: Vec<&'static str>,
    /// The group cache this build leaves ([`refresh_groups`]), in plan order.
    slots: Vec<Slot>,
    /// `LanguageGraphs::keys` of the build.
    graph_keys: Vec<&'static str>,
    /// The graphs this repo added to the joint result (language, then
    /// region / project / docs).
    graphs: usize,
    phases: PhaseTimes,
    fingerprint: Duration,
    clone: Duration,
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
            overlay_fp: overlay_fp(at),
            glia_fp: glia_inputs_fp(at.root),
        }
    }

    /// The inputs for `tree` under `ident` (the identity the session settled
    /// for this repo: re-read when an identity path is in `high`, then
    /// disambiguated against the session's other roots, GA.9): the go modules
    /// and tsconfig aliases on a re-plan (they read the project roots), the
    /// include roots on a re-plan or a changed file list (they read the dirs
    /// that hold a C/C++ file), the overlay and the `.glia` fingerprint when a
    /// `.glia` path is in `high`. The rest is kept.
    fn refresh(
        &self,
        at: &At<'_>,
        tree: &Tree,
        high: &BTreeSet<&str>,
        replan: bool,
        ident: RepoIdentity,
    ) -> Self {
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
                overlay_fp(at),
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

    /// GA.9: these inputs under another identity `ident` (an untouched repo
    /// whose disambiguated key moved with another root's identity). Nothing
    /// else a build reads depends on the identity: the overlay config is the
    /// root's file, the rest its tree.
    fn reidentified(&self, ident: RepoIdentity) -> Self {
        Self {
            repo: RepoId::from_canonical(&ident.key),
            ident,
            ..self.clone()
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
    /// Open a session on the repo at `root`: [`WatchSession::open_many`] of
    /// that one root (GA.9): load its parse cache sidecar once, plan and read
    /// the walk (with stat stamps), read the repo-level inputs and run one
    /// build. Errs when `root` is not a directory.
    pub fn open(root: &str, opts: &BuildOptions) -> Result<Self, String> {
        Self::open_many(&[root.to_string()], opts)
    }

    /// GA.9: open one session over the repos at `roots`, built exactly as
    /// `generate_many` builds them. Per root, in argument order: its parse
    /// cache sidecar loaded once, its walk planned and read (with stat
    /// stamps) and its identity read; then the identities disambiguated
    /// against each other (`build::disambiguate`, by each root's canonical
    /// path), each root's repo-level inputs read (the first root parses
    /// `opts.overlay_text` when it is set, every other root its own
    /// `.glia/overlay.toml`, as `generate_many` hands the text to its first
    /// path alone), and one build: each repo's DC.2 stages against the proto
    /// service set of every repo's files, appended in argument order, then
    /// one pass tail over all of them. The result's labels and roots are the
    /// roots as given.
    ///
    /// A multi-repo session ([`WatchSession::roots`] holds more than one)
    /// names each `WatchUpdate::groups_rebuilt` entry `<label>:<key>` and
    /// each `WatchFiles` path `<label>:<rel>` (`label` the root as given);
    /// a one-root session keeps the bare key and path.
    ///
    /// Errs when `roots` is empty, when a root is not a directory (unlike
    /// `generate_many`, which builds the rest: a watch needs every repo), when
    /// two roots are one directory, or when one root lies inside another after
    /// canonicalisation (`nested watch roots: <inner> is inside <outer>`): a
    /// clean `generate_many` reads the inner repo's files in both walks, which
    /// routing each changed path to one repo cannot reproduce.
    pub fn open_many(roots: &[String], opts: &BuildOptions) -> Result<Self, String> {
        let started = Instant::now();
        // DC.7c: before any read, so every record is newer than it.
        let scanned_at = SystemTime::now();
        if roots.is_empty() {
            return Err("no watch roots".to_string());
        }
        if let Some(root) = roots.iter().find(|r| !Path::new(r).is_dir()) {
            return Err(format!("not a directory: {root}"));
        }
        check_roots(roots)?;
        // Phase 1 (generate_many's): every root's cache, identity and walk.
        let mut walked = Vec::with_capacity(roots.len());
        for root in roots {
            let root_path = PathBuf::from(root);
            let cache = ParseCache::load(root);
            let ident = repo_identity(&root_path);
            let walk_started = Instant::now();
            let plan = plan_walk(&root_path);
            let walk = walk_started.elapsed();
            let read_started = Instant::now();
            let ((files, regions, md, project_roots), records) =
                read_walk(&root_path, &plan, None, true);
            let tree = Tree {
                plan: None,
                records: Vec::new(),
                files,
                md,
                regions,
                roots: project_roots,
                changes: WatchFiles::default(),
                walk,
                read: read_started.elapsed(),
            };
            walked.push((root_path, cache, ident, plan, records, tree));
        }
        let raws: Vec<RepoIdentity> = walked.iter().map(|w| w.2.clone()).collect();
        let paths: Vec<&Path> = walked.iter().map(|w| w.0.as_path()).collect();
        let idents = settle_identities(&paths, raws);
        let files: usize = walked.iter().map(|w| w.5.files.len() + w.5.md.len()).sum();
        let mut repos = Vec::with_capacity(walked.len());
        let mut works = Vec::with_capacity(walked.len());
        for (slot, ((root_path, cache, ident_raw, plan, records, tree), ident)) in
            walked.into_iter().zip(idents).enumerate()
        {
            let label = roots.get(slot).cloned().unwrap_or_default();
            repo_id_marker(&ident, &label);
            let at = At {
                root: &root_path,
                label: &label,
                opts,
                overlay_text: overlay_text_for(slot, opts),
            };
            let setup = Setup::read(&at, ident, &tree.files, &tree.roots);
            repos.push(RepoWatch {
                abs_roots: abs_roots(&root_path),
                index: index_of(&records),
                root: root_path,
                label,
                plan,
                records,
                regions: tree.regions.clone(),
                roots: tree.roots.clone(),
                setup,
                ident_raw,
                cache,
                groups: Vec::new(),
                group_count: 0,
                graph_keys: Vec::new(),
                graph_count: 0,
                watched: BTreeMap::new(),
                baselines: BTreeMap::new(),
            });
            works.push(Work {
                tree,
                setup: None,
                updated: true,
            });
        }
        let services_fp = proto_services_fp(works.iter().map(|w| w.tree.files.as_slice()));
        let fast_path = fast_path_from_env();
        let built = build(
            &mut repos,
            &mut works,
            opts,
            services_fp,
            fast_path,
            started,
        );
        for (repo, part) in repos.iter_mut().zip(built.repos) {
            repo.groups = refresh_groups(Vec::new(), part.slots, 0);
            repo.group_count = part.groups.len();
            repo.graph_keys = part.graph_keys;
            repo.graph_count = part.graphs;
            repo.refresh_watch(Consumed::ALL, BTreeMap::new());
        }
        let groups: usize = repos.iter().map(|r| r.group_count).sum();
        let labels: Vec<&str> = repos.iter().map(|r| r.label.as_str()).collect();
        let shape = if labels.len() > 1 {
            format!("repos={} repo={}", labels.len(), labels.join(","))
        } else {
            format!("repo={}", labels.join(","))
        };
        eprintln!(
            "[watch] open {shape} files={files} groups={groups} nodes={} edges={} took={}",
            built.result.total_nodes,
            built.result.total_edges,
            timing::ms(started.elapsed())
        );
        let rank = Arc::new(RankCache::new(
            "watch",
            &built.result.repo_roots,
            &built.result.repo_labels,
        ));
        let mut session = Self {
            repos,
            opts: opts.clone(),
            result: built.result,
            seq: 0,
            services_fp,
            scanned_at: Mutex::new(scanned_at),
            rank,
            budget: MemoryBudget::from_env(),
            evictions: 0,
            memo_clears: 0,
            fast_path,
        };
        session.enforce_budget();
        Ok(session)
    }

    /// GA.9: the session's roots as given, in build order.
    pub fn roots(&self) -> Vec<&str> {
        self.repos.iter().map(|r| r.label.as_str()).collect()
    }

    /// GA.12: bound the session's caches by `budget` from now on (module
    /// doc, "Memory budget"), replacing the one [`WatchSession::open`] read
    /// from `GLIA_MEMORY_BUDGET_MB`; [`MemoryBudget::none`] lifts it. The
    /// budget is enforced right away (one `[watch] rss` line) and after
    /// every update that rebuilds.
    pub fn set_memory_budget(&mut self, budget: MemoryBudget) {
        self.budget = budget;
        self.enforce_budget();
    }

    /// GA.12: the budget the session evicts against.
    pub fn memory_budget(&self) -> MemoryBudget {
        self.budget
    }

    /// GA.12: what the session holds now ([`WatchMemory`]); reads the
    /// process's RSS, evicts nothing.
    pub fn memory(&self) -> WatchMemory {
        WatchMemory {
            rss_kb: memory::rss_kb(),
            budget_mb: self.budget.budget_mb(),
            cached_groups: self.repos.iter().map(|r| r.groups.len()).sum(),
            groups: self.repos.iter().map(|r| r.group_count).sum(),
            cache_entries: self.repos.iter().map(|r| r.cache.len()).sum(),
            memo_paths: self.repos.iter().map(|r| r.cache.memo().len()).sum(),
            evictions: self.evictions,
            memo_clears: self.memo_clears,
            rank: self.rank.stats(),
        }
    }

    /// GA.12: evict cached groups, then the build memos, while the process's
    /// RSS is over the budget (module doc, "Memory budget"), trimming and
    /// re-reading RSS after each step; then print each repo's `[watch] rss`
    /// line. Nothing is evicted without a reading or a budget.
    fn enforce_budget(&mut self) {
        let mut evicted = vec![0usize; self.repos.len()];
        let mut cleared = vec![false; self.repos.len()];
        let mut rss = memory::rss_kb();
        while rss.is_some_and(|kb| self.budget.exceeded(kb)) {
            if let Some((r, i)) = self.oldest_group() {
                if let Some(repo) = self.repos.get_mut(r) {
                    drop(repo.groups.remove(i));
                }
                if let Some(n) = evicted.get_mut(r) {
                    *n += 1;
                }
                self.evictions += 1;
            } else {
                let mut any = false;
                for (repo, c) in self.repos.iter_mut().zip(cleared.iter_mut()) {
                    if repo.cache.clear_memo() {
                        *c = true;
                        any = true;
                    }
                }
                if !any {
                    break;
                }
                self.memo_clears += 1;
            }
            memory::release_freed_memory("watch_budget");
            rss = memory::rss_kb();
        }
        for ((repo, evicted), cleared) in self.repos.iter().zip(evicted).zip(cleared) {
            eprintln!(
                "{}",
                rss_marker(self.seq, rss, self.budget, repo, evicted, cleared)
            );
        }
    }

    /// GA.12: the cached group the budget evicts next: the lowest
    /// `last_used`, ties by repo order, then plan order (each repo's cache is
    /// in plan order). `(repo index, group index)`.
    fn oldest_group(&self) -> Option<(usize, usize)> {
        self.repos
            .iter()
            .enumerate()
            .flat_map(|(r, repo)| {
                repo.groups
                    .iter()
                    .enumerate()
                    .map(move |(i, g)| (g.last_used, r, i))
            })
            .min()
            .map(|(_, r, i)| (r, i))
    }

    /// Bring the session up to date with `changed`, the paths the caller saw
    /// change (absolute under a root, or repo-relative). A path the caller
    /// does not name is not re-read. GA.9: each absolute path goes to the one
    /// repo whose root holds it (roots never nest, [`WatchSession::open_many`]);
    /// a relative path is taken under the first root, as a one-root session
    /// takes it. Paths under `.glia/graph/` or
    /// `.glia/calibration-snapshot/`, under a
    /// hard-skipped directory (`.git/`, but `.git/HEAD` and `.git/config`,
    /// `.idea/`, a nested `.glia/`, ...) and two or more levels below a
    /// collapsed region root are dropped: their content never reaches the
    /// graph. A direct entry of a region root re-plans (`gate_dir` decides
    /// the region from exactly those entries).
    ///
    /// Per repo: re-plans when a path is High ([`durability`]), a known file
    /// is gone or no longer a file, an unknown path exists, or a gone path is
    /// a directory the walk knew (or held table entries); otherwise re-reads
    /// only the named entries. When every repo's table, regions, project
    /// roots and repo-level inputs are unchanged the update is a noop: no
    /// rows, `seq` unchanged. Otherwise the joint tree is rebuilt (a repo with
    /// no named path re-reads nothing: its stages run on its table's reads, a
    /// cache hit throughout), the located delta taken against the previous
    /// graph, `seq` advanced and the freed heap trimmed
    /// (`memory::release_freed_memory`). A re-read identity is disambiguated
    /// against every root again; any key that moved is the `identity` full
    /// rebuild. Errs when a root is no longer a directory; state is committed
    /// only once an update succeeded, so an error (or a panic an embedding
    /// app catches) leaves the previous state.
    pub fn apply(&mut self, changed: &[PathBuf]) -> Result<WatchUpdate, String> {
        let started = Instant::now();
        let scanned = watermark(&self.scanned_at);
        if self.repos.is_empty() {
            return Err("watch session holds no repo".to_string());
        }
        if let Some(repo) = self.repos.iter().find(|r| !r.root.is_dir()) {
            return Err(format!("not a directory: {}", repo.label));
        }
        let (named, outside) = self.route(changed);
        if outside > 0 {
            eprintln!("[watch] outside repo={} paths={outside}", self.labels(None));
        }
        let label = self.labels(Some(&named));
        let highs: Vec<BTreeSet<&'static str>> = named
            .iter()
            .map(|named| {
                named
                    .iter()
                    .filter_map(|rel| match durability(rel) {
                        Durability::High(why) => Some(why),
                        Durability::Low => None,
                    })
                    .collect()
            })
            .collect();
        let mut replans = Vec::with_capacity(self.repos.len());
        let mut captured = Vec::with_capacity(self.repos.len());
        let mut trees: Vec<Option<Tree>> = Vec::with_capacity(self.repos.len());
        for ((repo, named), high) in self.repos.iter_mut().zip(&named).zip(&highs) {
            let replan = !high.is_empty() || named.iter().any(|rel| repo.reshapes(rel));
            // DC.7c: the baselines, before this update reads anything.
            captured.push(if named.is_empty() {
                BTreeMap::new()
            } else {
                repo.capture(replan, scanned)
            });
            trees.push(if named.is_empty() {
                None
            } else if replan {
                Some(repo.replanned(named))
            } else {
                repo.reread(named)
            });
            replans.push(replan);
        }
        if trees.iter().all(Option::is_none) {
            return Ok(noop_update(
                &label,
                self.seq,
                &self.result,
                changed.len(),
                started,
                Duration::ZERO,
                Duration::ZERO,
            ));
        }
        // GA.9: the identities, re-read where an identity path changed and
        // disambiguated against every root again.
        let reread: Vec<bool> = trees
            .iter()
            .zip(&highs)
            .map(|(tree, high)| tree.is_some() && high.contains("identity"))
            .collect();
        let raws: Vec<RepoIdentity> = self
            .repos
            .iter()
            .zip(&reread)
            .map(|(repo, &again)| {
                if again {
                    repo_identity(&repo.root)
                } else {
                    repo.ident_raw.clone()
                }
            })
            .collect();
        let idents: Vec<RepoIdentity> = if reread.contains(&true) {
            let paths: Vec<&Path> = self.repos.iter().map(|r| r.root.as_path()).collect();
            let idents = settle_identities(&paths, raws.clone());
            for ((repo, ident), &again) in self.repos.iter().zip(&idents).zip(&reread) {
                if again {
                    repo_id_marker(ident, &repo.label);
                }
            }
            idents
        } else {
            self.repos.iter().map(|r| r.setup.ident.clone()).collect()
        };
        let mut setups: Vec<Option<Setup>> = Vec::with_capacity(self.repos.len());
        let mut changed_repo = false;
        for (slot, ((repo, tree), ident)) in self.repos.iter().zip(&trees).zip(idents).enumerate() {
            let at = At::of(repo, slot, &self.opts);
            let setup = match tree {
                Some(tree) => {
                    let setup = repo
                        .setup
                        .refresh(&at, tree, &highs[slot], replans[slot], ident);
                    changed_repo |= !(tree.changes.added.is_empty()
                        && tree.changes.modified.is_empty()
                        && tree.changes.removed.is_empty()
                        && tree.regions == repo.regions
                        && tree.roots == repo.roots
                        && setup.same_inputs(&repo.setup));
                    Some(setup)
                }
                None if ident.key != repo.setup.ident.key => {
                    changed_repo = true;
                    Some(repo.setup.reidentified(ident))
                }
                None => None,
            };
            setups.push(setup);
        }
        let consumed: Vec<Consumed> = self
            .repos
            .iter()
            .zip(&setups)
            .enumerate()
            .map(|(slot, (repo, setup))| Consumed {
                walk: replans[slot],
                includes: replans[slot]
                    || setup
                        .as_ref()
                        .is_some_and(|s| s.files_fp != repo.setup.files_fp),
                glia: highs[slot].contains("glia_inputs"),
                identity: highs[slot].contains("identity"),
            })
            .collect();
        if !changed_repo {
            let (mut walk, mut read) = (Duration::ZERO, Duration::ZERO);
            for (slot, (tree, setup)) in trees.into_iter().zip(setups).enumerate() {
                let (Some(tree), Some(setup)) = (tree, setup) else {
                    continue;
                };
                walk += tree.walk;
                read += tree.read;
                let Some(repo) = self.repos.get_mut(slot) else {
                    continue;
                };
                repo.commit_tree(tree, setup);
                repo.refresh_watch(consumed[slot], std::mem::take(&mut captured[slot]));
            }
            return Ok(noop_update(
                &label,
                self.seq,
                &self.result,
                changed.len(),
                started,
                walk,
                read,
            ));
        }
        let mut works: Vec<Work> = self
            .repos
            .iter()
            .zip(trees)
            .zip(setups)
            .map(|((repo, tree), setup)| {
                let updated = tree.is_some();
                Work {
                    tree: tree.unwrap_or_else(|| repo.current_tree()),
                    setup,
                    updated,
                }
            })
            .collect();
        let services_fp = proto_services_fp(works.iter().map(|w| w.tree.files.as_slice()));
        // The first reason by kind (identity, go modules, overlay) over the
        // repos, then the joint service set.
        let reasons: Vec<&'static str> = self
            .repos
            .iter()
            .zip(&works)
            .filter_map(|(repo, w)| {
                w.setup
                    .as_ref()
                    .and_then(|s| full_rebuild_reason(&repo.setup, s))
            })
            .collect();
        let full_rebuild = REBUILD_REASONS
            .into_iter()
            .find(|r| reasons.contains(r))
            .or_else(|| (services_fp != self.services_fp).then_some("proto_services"));
        let multi = self.repos.len() > 1;
        let built = build(
            &mut self.repos,
            &mut works,
            &self.opts,
            services_fp,
            self.fast_path,
            started,
        );
        let delta_started = Instant::now();
        // DC.6's exactness condition: no graph outside the scope changed. A
        // reused group's pre-pass graph is a clone of its last build, which
        // holds on a Low update; a High input (the overlay, `.glia`, a
        // manifest, ...) or a SCIP snapshot can move a clean graph after the
        // passes, so those take the full delta.
        let scoped = highs.iter().all(BTreeSet::is_empty)
            && full_rebuild.is_none()
            && !self.repos.iter().any(|r| scip_dir(&r.root).exists());
        let rows = if scoped {
            let mut scope = DeltaScope {
                before: BTreeSet::new(),
                after: BTreeSet::new(),
                moves: works.iter().any(|w| {
                    !w.tree.changes.added.is_empty() || !w.tree.changes.removed.is_empty()
                }),
            };
            let (mut before_at, mut after_at) = (0usize, 0usize);
            for (repo, part) in self.repos.iter().zip(&built.repos) {
                let before = scope_side(
                    &repo.graph_keys,
                    repo.graph_count,
                    &part.rebuilt,
                    &part.graph_keys,
                );
                let after = scope_side(
                    &part.graph_keys,
                    part.graphs,
                    &part.rebuilt,
                    &repo.graph_keys,
                );
                scope
                    .before
                    .extend(before.into_iter().map(|i| i + before_at));
                scope.after.extend(after.into_iter().map(|i| i + after_at));
                before_at += repo.graph_count;
                after_at += part.graphs;
            }
            located_delta_scoped(&self.result.merged, &built.result.merged, &scope)
        } else {
            located_delta_counted(&self.result.merged, &built.result.merged, &[])
        };
        let delta = delta_started.elapsed();
        let Built {
            result,
            repos: parts,
            times,
        } = built;
        let mut phases = PhaseTimes::default();
        let (mut walk, mut read, mut fingerprint, mut clone) = (
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
        );
        let mut files = WatchFiles::default();
        let mut groups_rebuilt = Vec::new();
        let mut body_only = Vec::new();
        let mut groups = 0usize;
        for (slot, ((repo, work), part)) in self.repos.iter_mut().zip(works).zip(parts).enumerate()
        {
            let Work {
                mut tree,
                setup,
                updated,
            } = work;
            let tag = |s: &str| {
                if multi {
                    format!("{}:{s}", repo.label)
                } else {
                    s.to_string()
                }
            };
            let changes = std::mem::take(&mut tree.changes);
            files.added.extend(changes.added.iter().map(|s| tag(s)));
            files
                .modified
                .extend(changes.modified.iter().map(|s| tag(s)));
            files.removed.extend(changes.removed.iter().map(|s| tag(s)));
            groups_rebuilt.extend(part.rebuilt.iter().map(|g| tag(g)));
            body_only.extend(part.body_only.iter().map(|g| tag(g)));
            walk += tree.walk;
            read += tree.read;
            phases.parse += part.phases.parse;
            phases.const_scan += part.phases.const_scan;
            phases.grafts += part.phases.grafts;
            phases.language_build += part.phases.language_build;
            fingerprint += part.fingerprint;
            clone += part.clone;
            groups += part.groups.len();
            let stats = repo.cache.stats;
            files.reparsed += stats.reparsed;
            files.reused += stats.reused;
            if updated {
                if let Some(setup) = setup {
                    repo.commit_tree(tree, setup);
                }
                repo.refresh_watch(consumed[slot], std::mem::take(&mut captured[slot]));
            } else if let Some(setup) = setup {
                repo.setup = setup;
            }
            if let Some(raw) = raws.get(slot) {
                repo.ident_raw = raw.clone();
            }
            repo.groups =
                refresh_groups(std::mem::take(&mut repo.groups), part.slots, self.seq + 1);
            repo.group_count = part.groups.len();
            repo.graph_keys = part.graph_keys;
            repo.graph_count = part.graphs;
        }
        self.services_fp = services_fp;
        self.seq += 1;
        drop(std::mem::replace(&mut self.result, result));
        self.rank = Arc::new(RankCache::new(
            "watch",
            &self.result.repo_roots,
            &self.result.repo_labels,
        ));
        crate::memory::release_freed_memory("watch_update");
        let update = WatchUpdate {
            seq: self.seq,
            files,
            groups_rebuilt,
            body_only,
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
                fingerprint: ms(fingerprint),
                clone: ms(clone),
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
            update_marker(&update, groups, started.elapsed(), &label)
        );
        self.enforce_budget();
        Ok(update)
    }

    /// GA.9: `changed` routed to the repos ([`WatchSession::apply`]): each
    /// repo's named repo-relative paths (dropped ones left out), and the
    /// count of paths under no root.
    fn route(&self, changed: &[PathBuf]) -> (Vec<BTreeSet<String>>, usize) {
        let mut named = vec![BTreeSet::new(); self.repos.len()];
        let mut outside = 0usize;
        for p in changed {
            let hit = if p.is_absolute() {
                self.repos
                    .iter()
                    .enumerate()
                    .find_map(|(i, r)| r.rel_of(p).map(|rel| (i, rel)))
            } else {
                self.repos
                    .first()
                    .and_then(|r| r.rel_of(p))
                    .map(|rel| (0, rel))
            };
            match hit {
                Some((i, rel)) => {
                    if let (Some(repo), Some(set)) = (self.repos.get(i), named.get_mut(i))
                        && !repo.dropped(&rel)
                    {
                        set.insert(rel);
                    }
                }
                None => outside += 1,
            }
        }
        (named, outside)
    }

    /// GA.9: the `repo=` of a marker: the labels of the repos `named` holds
    /// a path for, or every label when it holds none (or is `None`), in
    /// build order, comma-joined. A one-root session's label either way.
    fn labels(&self, named: Option<&[BTreeSet<String>]>) -> String {
        let hit: Vec<&str> = match named {
            Some(named) => self
                .repos
                .iter()
                .zip(named)
                .filter(|(_, n)| !n.is_empty())
                .map(|(r, _)| r.label.as_str())
                .collect(),
            None => Vec::new(),
        };
        if hit.is_empty() {
            self.roots().join(",")
        } else {
            hit.join(",")
        }
    }

    /// The session's current build.
    pub fn result(&self) -> &GenerateResult {
        &self.result
    }

    /// 0.5.3 (EA.7): the ranking cache of the current build (holder `watch`):
    /// a ranked answer over [`Self::result`]'s graph, or over a clone of it
    /// (pyo3's `snapshot()`), reuses its PPR, file and co-change indexes. A
    /// new `Arc` after every update that changed the graph, the same one
    /// after a noop.
    pub fn rank_cache(&self) -> Arc<RankCache> {
        Arc::clone(&self.rank)
    }

    /// Write the current build as the layout at `dir` (`persist::persist_result`,
    /// writer `watch`). The caller enforces the default-dir rule for an
    /// overlay-free session, as `glia build` and `save_to_default` do.
    pub fn persist(&self, dir: &Path) -> Result<(), String> {
        crate::persist::persist_result(&self.result, dir, "watch")
    }

    /// Save each repo's held parse cache beside that repo's layout
    /// (`ParseCache::save`: written only when it changed, LC.11), in build
    /// order. GA.9: every repo's is attempted; the first error is returned.
    pub fn save_cache(&self) -> std::io::Result<()> {
        let mut first = Ok(());
        for repo in &self.repos {
            if let Err(e) = repo.cache.save(&repo.label)
                && first.is_ok()
            {
                first = Err(e);
            }
        }
        first
    }

    /// Updates that rebuilt since [`WatchSession::open`] (0 right after it).
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// DC.7c: the stat scan, no rebuild, for a caller that debounces and
    /// then hands the result to [`WatchSession::apply`]: the paths that
    /// changed since the update that last read them (module doc, "Stat
    /// poll"), absolute (the root made absolute, joined with each
    /// repo-relative path; the root itself for its own directory), sorted
    /// and deduplicated. A path keeps being reported until an update reads
    /// it, so a quiet scan is an empty one. GA.9: every repo is scanned, in
    /// build order. A scan that finds nothing in any repo moves the racy
    /// watermark to the first repo scan's start; each repo whose scan finds
    /// something prints its `[watch] poll` line.
    pub fn changed_paths(&self) -> Vec<PathBuf> {
        let scanned = self.scanned();
        let mut out = Vec::new();
        let mut first_start: Option<SystemTime> = None;
        for repo in &self.repos {
            let scan = repo.scan(scanned);
            first_start.get_or_insert(scan.started);
            if scan.found.is_empty() {
                continue;
            }
            eprintln!("{}", poll_marker(&scan, &repo.label));
            let base = repo.abs_roots.first().unwrap_or(&repo.root);
            out.extend(scan.found.iter().map(|rel| {
                if rel.is_empty() {
                    base.clone()
                } else {
                    base.join(rel)
                }
            }));
        }
        if out.is_empty() {
            if let Some(started) = first_start {
                let mut at = self
                    .scanned_at
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                *at = (*at).max(started);
            }
            return out;
        }
        out.sort();
        out.dedup();
        out
    }

    /// DC.7c: [`WatchSession::changed_paths`], then [`WatchSession::apply`]
    /// of what it found. `None` when nothing changed: a quiet scan, or an
    /// update that found every reported path's content unchanged (a touch)
    /// and left `seq` as it was. Errs as `apply` does.
    pub fn poll(&mut self) -> Result<Option<WatchUpdate>, String> {
        let paths = self.changed_paths();
        if paths.is_empty() {
            return Ok(None);
        }
        let seq = self.seq;
        let update = self.apply(&paths)?;
        Ok((update.seq != seq).then_some(update))
    }

    /// DC.7c: the racy watermark ([`WatchSession::scanned_at`]).
    fn scanned(&self) -> SystemTime {
        watermark(&self.scanned_at)
    }
}

impl RepoWatch {
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

    /// Whether a changed `rel` cannot reach the graph: the layout dir, the
    /// ranking calibration (`.glia/calibration-snapshot/`), the query-time
    /// inputs (`.glia/vectors.jsonl`, `.glia/vocab-snapshot/`, FB.3), a path
    /// under a hard-skipped directory (the root's `.git/HEAD` / `.git/config`
    /// and `.glia/` inputs aside), the 0.4.x layout `.ai/repo-graph`, or a path
    /// two or more levels below a region root.
    fn dropped(&self, rel: &str) -> bool {
        if matches!(rel, ".git/HEAD" | ".git/config") {
            return false;
        }
        let parts: Vec<&str> = rel.split('/').collect();
        if parts.first() == Some(&CONTROL_DIR) && parts.len() > 1 {
            return matches!(parts.get(1), Some(&"graph") | Some(&CALIBRATION_DIR))
                || rel
                    .strip_prefix(CONTROL_DIR)
                    .is_some_and(|r| is_query_time_rel(r.trim_start_matches('/')));
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

    /// GA.9: the tree as the table holds it, for a repo with no named path
    /// in an update that rebuilds: the walk's fold over the plan with every
    /// entry's recorded read reused (no file is read; the fold's lines
    /// print). Never committed, so every record keeps its stamp.
    fn current_tree(&self) -> Tree {
        let started = Instant::now();
        let (records, index) = (&self.records, &self.index);
        let reuse = |p: &Pending| -> Option<Read> {
            index
                .get(&p.rel)
                .and_then(|&i| records.get(i))
                .map(|r| r.read.clone())
        };
        let reuse: &Reuse<'_> = &reuse;
        let ((files, regions, md, roots), _) =
            read_walk(&self.root, &self.plan, Some(reuse), false);
        Tree {
            plan: None,
            records: Vec::new(),
            files,
            md,
            regions,
            roots,
            changes: WatchFiles::default(),
            walk: Duration::ZERO,
            read: started.elapsed(),
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

    /// DC.7c: one stat scan (module doc, "Stat poll"). A table entry is
    /// found when its stat differs from its record or, inside the racy
    /// window, when a fresh read (by its class's rule) differs from the
    /// recorded one; a directory when it is gone, its mtime moved or, inside
    /// the window, its listing differs from its baseline (or it has none); a
    /// watched input when it appeared, vanished or its stat moved or, inside
    /// the window, its bytes differ from its baseline (or it has none). The
    /// table and the directories are stat'ed on the pool.
    fn scan(&self, scanned: SystemTime) -> Scan {
        let started = SystemTime::now();
        let timer = Instant::now();
        let window = Window::at(scanned, started);
        let mut found = BTreeSet::new();
        let mut racy = 0usize;
        let table: Vec<(&Pending, &ReadRecord)> =
            self.plan.pending.iter().zip(&self.records).collect();
        let (hits, _) = crate::parallel::par_map_ordered(&table, |(p, r)| {
            if FileStamp::of(&p.abs) != r.stamp {
                return (true, false);
            }
            if !r.stamp.is_some_and(|s| window.racy(s.mtime)) {
                return (false, false);
            }
            (read_one(p) != r.read, true)
        });
        for ((p, _), (hit, checked)) in table.iter().zip(hits) {
            racy += usize::from(checked);
            if hit {
                found.insert(p.rel.clone());
            }
        }
        let dirs = self.walked_dirs();
        let (root, baselines) = (&self.root, &self.baselines);
        let (hits, _) = crate::parallel::par_map_ordered(&dirs, |d| {
            let abs = root.join(&d.rel);
            if dir_mtime(&abs) != Some(d.mtime) {
                return (true, false);
            }
            if !window.racy(d.mtime) {
                return (false, false);
            }
            (
                baselines.get(&d.rel).map(|(_, fp)| *fp) != Some(dir_fp(&abs)),
                true,
            )
        });
        for (d, (hit, checked)) in dirs.iter().zip(hits) {
            racy += usize::from(checked);
            if hit {
                found.insert(d.rel.clone());
            }
        }
        let inputs = self.watched_now();
        for (rel, _) in &inputs {
            let abs = self.root.join(rel);
            let was = self.watched.get(rel).and_then(|w| w.stamp);
            if file_stamp(&abs) != was {
                found.insert(rel.clone());
            } else if was.is_some_and(|s| window.racy(s.mtime)) {
                racy += 1;
                if self.baselines.get(rel).map(|(_, fp)| *fp) != Some(bytes_fp(&abs)) {
                    found.insert(rel.clone());
                }
            }
        }
        Scan {
            found,
            files: table.len(),
            dirs: dirs.len(),
            watched: inputs.len(),
            racy,
            started,
            took: timer.elapsed(),
        }
    }

    /// DC.7c: every walked directory and region root of the plan.
    fn walked_dirs(&self) -> Vec<&WalkedDir> {
        self.plan
            .dirs
            .iter()
            .chain(&self.plan.region_dirs)
            .collect()
    }

    /// DC.7c: the watched inputs to check now: the recorded ones, plus every
    /// file under `.glia/` (but the layout) not recorded, listed afresh: a
    /// new one moves no directory the walk entered.
    fn watched_now(&self) -> Vec<(String, Input)> {
        let mut inputs: BTreeMap<String, Input> = self
            .watched
            .iter()
            .map(|(rel, w)| (rel.clone(), w.class))
            .collect();
        for (rel, _) in glia_files(&self.root) {
            inputs
                .entry(format!("{CONTROL_DIR}/{rel}"))
                .or_insert(Input::Glia);
        }
        inputs.into_iter().collect()
    }

    /// DC.7c: the baselines an update takes BEFORE it reads anything: the
    /// listing fingerprint of every walked directory inside the racy window
    /// (on a re-plan, the only update that lists them again) and the content
    /// fingerprint of every watched input inside it. Taken first, a baseline
    /// never holds a change its update did not read: a change after it is a
    /// mismatch on a later scan, one between it and the read a spurious one
    /// that the next update settles. A directory or input new to the update
    /// has none until the update after.
    fn capture(&self, replan: bool, scanned: SystemTime) -> BTreeMap<String, (Input, u64)> {
        let window = Window::at(scanned, SystemTime::now());
        let mut out = BTreeMap::new();
        if replan {
            let dirs = self.walked_dirs();
            let root = &self.root;
            let (fps, _) = crate::parallel::par_map_ordered(&dirs, |d| {
                let abs = root.join(&d.rel);
                dir_mtime(&abs)
                    .filter(|m| window.racy(*m))
                    .map(|_| dir_fp(&abs))
            });
            for (d, fp) in dirs.iter().zip(fps) {
                if let Some(fp) = fp {
                    out.insert(d.rel.clone(), (Input::Walk, fp));
                }
            }
        }
        for (rel, class) in self.watched_now() {
            let abs = self.root.join(&rel);
            if file_stamp(&abs).is_some_and(|s| window.racy(s.mtime)) {
                let fp = bytes_fp(&abs);
                out.insert(rel, (class, fp));
            }
        }
        out
    }

    /// DC.7c: after an update (or the open) that read the classes in
    /// `consumed`: those classes' watched inputs recomputed from the
    /// committed table and plan with a stat now, their baselines replaced by
    /// `captured`'s. Every other class keeps its entries and baselines, and
    /// a kept entry wins over a recomputed one at the same path (a
    /// `CMakeLists.txt` the walk reads stays [`Input::Walk`] when only the
    /// include roots were re-read). A stat taken after the read is safe:
    /// a write between them is newer than the watermark, so it sits in the
    /// racy window and meets the baseline taken before the read.
    fn refresh_watch(&mut self, consumed: Consumed, captured: BTreeMap<String, (Input, u64)>) {
        self.watched.retain(|_, w| !consumed.has(w.class));
        self.baselines.retain(|_, (class, _)| !consumed.has(*class));
        for (rel, class) in self.input_rels(consumed) {
            let stamp = file_stamp(&self.root.join(&rel));
            self.watched.entry(rel).or_insert(Watched { class, stamp });
        }
        self.baselines.extend(
            captured
                .into_iter()
                .filter(|(_, (class, _))| consumed.has(*class)),
        );
    }

    /// DC.7c: the repo-relative path of every watched input of a class in
    /// `classes`, as the table, the plan, the project roots and `.glia/`
    /// stand ([`Input`]). Where the walk and the include roots read one path
    /// it is [`Input::Walk`]'s (a re-plan re-reads both).
    fn input_rels(&self, classes: Consumed) -> BTreeMap<String, Input> {
        let mut out = BTreeMap::new();
        if classes.includes {
            let c_files: Vec<&str> = self
                .records
                .iter()
                .filter(|r| matches!(r.read, Read::Src(_)))
                .filter(|r| detect_language(&r.rel) == Some("c_cpp"))
                .map(|r| r.rel.as_str())
                .collect();
            if !c_files.is_empty() {
                for dir in held_dirs(&c_files) {
                    out.insert(join_rel(&dir, CMAKE_LISTS), Input::Includes);
                }
                for rel in compile_dbs(&self.root, &self.roots) {
                    out.insert(rel, Input::Includes);
                }
            }
        }
        if classes.walk {
            for rel in &self.plan.gitignores {
                out.insert(rel.clone(), Input::Walk);
            }
            for d in &self.plan.region_dirs {
                let pointer = join_rel(&d.rel, ".git");
                if self.root.join(&pointer).is_file() {
                    out.insert(pointer, Input::Walk);
                }
            }
            let probes: Vec<String> = self
                .plan
                .dirs
                .iter()
                .flat_map(|d| {
                    project_roots::MANIFESTS
                        .iter()
                        .map(|(name, _)| *name)
                        .chain(TSCONFIGS)
                        .chain(BUNDLER_CONFIGS)
                        .map(|name| join_rel(&d.rel, name))
                })
                .filter(|rel| !self.index.contains_key(rel))
                .collect();
            let root = &self.root;
            let (present, _) =
                crate::parallel::par_map_ordered(&probes, |rel| root.join(rel).is_file());
            for (rel, present) in probes.into_iter().zip(present) {
                if present {
                    out.insert(rel, Input::Walk);
                }
            }
        }
        if classes.glia {
            for (rel, _) in glia_files(&self.root) {
                out.insert(format!("{CONTROL_DIR}/{rel}"), Input::Glia);
            }
        }
        if classes.identity {
            for rel in IDENTITY_FILES {
                out.insert(rel.to_string(), Input::Identity);
            }
        }
        out
    }
}

/// DC.7c: the racy watermark held in `at` ([`WatchSession::scanned_at`]).
fn watermark(at: &Mutex<SystemTime>) -> SystemTime {
    *at.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The answer of an update that changed nothing (the session's `seq` and
/// `result` totals as they stand), with its marker for the repo `label`.
fn noop_update(
    label: &str,
    seq: u64,
    result: &GenerateResult,
    paths: usize,
    started: Instant,
    walk: Duration,
    read: Duration,
) -> WatchUpdate {
    eprintln!("[watch] noop repo={label} paths={paths}");
    WatchUpdate {
        seq,
        files: WatchFiles::default(),
        groups_rebuilt: Vec::new(),
        body_only: Vec::new(),
        full_rebuild: None,
        total_nodes: result.total_nodes,
        total_edges: result.total_edges,
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

/// One build of every repo's `Work` through the DC.2 stages, composed exactly
/// as `generate_many` composes them (`assemble_many_with`'s phase 2, then
/// `finish_build`): one proto service set from every repo's files, in build
/// order; per repo ([`build_repo`]) its stages with its cache held, its
/// language graphs, then its region / project / docs graphs, appended in
/// build order with their fills aligned; then one pass tail over all of
/// them, whose inputs are one `RepoInputs` per repo. `services_fp` is that
/// service set's fingerprint ([`proto_services_fp`]). The result's labels
/// are `arch::repo_label_map` over each repo's (RepoId, label), its roots
/// first-wins, its parse errors concatenated in build order. A single repo
/// is exactly `generate_one_inner`'s composition. Prints each repo's
/// `[timing] repo=` line and the `[timing] build` line. Takes every
/// `tree.md`.
fn build(
    repos: &mut [RepoWatch],
    works: &mut [Work],
    opts: &BuildOptions,
    services_fp: u64,
    fast_path: bool,
    started: Instant,
) -> Built {
    let mut rpc = RpcContext::default();
    for w in works.iter() {
        rpc.add_files(&w.tree.files);
    }
    let count = repos.len();
    let mut graphs = Vec::new();
    let mut fills: Vec<Option<GroupFill>> = Vec::new();
    let mut parse_errors = Vec::new();
    let mut inputs = Vec::with_capacity(count);
    let mut label_inputs: Vec<(u64, String)> = Vec::with_capacity(count);
    let mut repo_roots: BTreeMap<u64, String> = BTreeMap::new();
    let mut parts = Vec::with_capacity(count);
    for (slot, (repo, work)) in repos.iter_mut().zip(works.iter_mut()).enumerate() {
        let RepoWatch {
            root,
            label,
            setup,
            cache,
            groups,
            ..
        } = repo;
        let at = At {
            root: root.as_path(),
            label: label.as_str(),
            opts,
            overlay_text: overlay_text_for(slot, opts),
        };
        let setup = work.setup.as_ref().unwrap_or(setup);
        let keyed = Keyed {
            services_fp,
            fast_path,
        };
        let stage = build_repo(&at, setup, &mut work.tree, cache, groups, keyed, &rpc);
        label_inputs.push((setup.repo.0, at.label.to_string()));
        // First root wins, like `repo_label_map` (disambiguated keys never
        // repeat; a repeat is one repo given twice, which open refuses).
        repo_roots
            .entry(setup.repo.0)
            .or_insert_with(|| at.label.to_string());
        parts.push(stage.part);
        graphs.extend(stage.graphs);
        fills.extend(stage.fills);
        parse_errors.extend(stage.parse_errors);
        inputs.push(stage.input);
    }
    let Finished {
        merged,
        total_nodes,
        total_edges,
        times,
    } = finish_build(graphs, fills, inputs, opts, count, started);
    eprintln!("{}", times.marker());
    parse_errors.shrink_to_fit();
    Built {
        result: GenerateResult {
            merged,
            total_nodes,
            total_edges,
            parse_errors,
            repo_labels: crate::arch::repo_label_map(&label_inputs),
            repo_roots,
        },
        repos: parts,
        times,
    }
}

/// One repo's stages ([`build`]): what it adds to the joint build, in order.
struct RepoStage {
    part: RepoBuilt,
    /// Its language graphs, then its region / project / docs graphs.
    graphs: Vec<RepoGraph>,
    /// Aligned with `graphs`: `Some` for a language graph, `None` after.
    fills: Vec<Option<GroupFill>>,
    parse_errors: Vec<String>,
    input: RepoInputs,
}

/// One repo's DC.2 stages for [`build`]: `parse_and_graft` against the
/// build's `rpc` with `cache` held, then plan / fingerprint / reuse / build /
/// fold the groups (DC.7b: each language group whose fingerprint matches its
/// entry of `cached` taken from a clone of that entry's build instead of
/// built), the per-repo markers and the region / project / docs graphs.
/// Prints its `[timing] repo=` line. Takes `tree.md`.
fn build_repo(
    at: &At<'_>,
    setup: &Setup,
    tree: &mut Tree,
    cache: &mut ParseCache,
    cached: &[CachedGroup],
    keyed: Keyed,
    rpc: &RpcContext,
) -> RepoStage {
    let Keyed {
        services_fp,
        fast_path,
    } = keyed;
    let (root, label, opts) = (at.root, at.label, at.opts);
    cache.validate_context(&setup.ident.key, &setup.go.context_key());
    let input = RepoInputs {
        repo: setup.repo,
        root: root.to_path_buf(),
        label: label.to_string(),
        config: setup.config.clone(),
    };
    let ctx = RepoBuildCtx {
        repo: setup.repo,
        repo_label: label,
        go: &setup.go,
        ts_aliases: &setup.ts_aliases,
        c_includes: &setup.c_includes,
        rpc,
        roots: &tree.roots,
        config: input.config.as_ref(),
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
    let go_module_dirs = go_module_dirs(&setup.go);
    let group_ctx = GroupCtx {
        repo: setup.repo,
        repo_label: label,
        rust_crates: &g.rust_crates,
        ts_aliases: &setup.ts_aliases,
        c_includes: &setup.c_includes,
        dart_packages: &g.dart_packages,
        go_module_dirs: &go_module_dirs,
    };
    // DC.7b: the group fingerprints, then a clone of each matching cached
    // build OUT into its reuse slot; GA.4: a clone of the cached build of
    // each other group whose edits are body-only into a re-stitch seed.
    let fingerprint_started = Instant::now();
    let prints = group_fingerprints(
        &groups,
        &group_ctx,
        GroupKey {
            overlay_fp: setup.overlay_fp,
            services_fp,
        },
        cached,
        fast_path,
    );
    let fingerprint = fingerprint_started.elapsed();
    let clone_started = Instant::now();
    let (reuse, kept) = reuse_slots(&keys, &prints, cached);
    let seeds: Vec<Option<Seed>> = reuse
        .into_iter()
        .zip(&keys)
        .zip(&prints)
        .map(|((reuse, key), print)| match reuse {
            Some(build) => Some(Seed::Reuse(build)),
            None if fast_path => rebody_seed(key, print, cached),
            None => None,
        })
        .collect();
    let mut clone = clone_started.elapsed();
    let rebuilt: Vec<&'static str> = keys
        .iter()
        .zip(&kept)
        .filter(|(_, kept)| kept.is_none())
        .map(|(key, _)| *key)
        .collect();
    let made = build_groups_seeded(groups, seeds, &group_ctx);
    let mut body_only = Vec::new();
    let mut builds = Vec::with_capacity(made.len());
    for (build, made) in made {
        if let Some(line) = fast_path_marker(build.key(), &made, label) {
            eprintln!("{line}");
        }
        if matches!(made, Made::Rebodied { .. }) {
            body_only.push(build.key());
        }
        builds.push(build);
    }
    if let Some(line) = g.dart_packages.marker(label) {
        eprintln!("{line}");
    }
    // Each rebuilt group's build cloned IN before the fold moves its graph.
    let clone_started = Instant::now();
    let slots = cache_slots(&builds, prints, &kept);
    clone += clone_started.elapsed();
    let LanguageGraphs {
        mut graphs,
        keys: graph_keys,
        fills,
        pooled,
    } = fold_groups(builds, &recv_fields, &mut g.parse_errors, label);
    g.times.language_build = language_started
        .elapsed()
        .saturating_sub(fingerprint + clone);
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
    RepoStage {
        part: RepoBuilt {
            groups: keys,
            rebuilt,
            body_only,
            slots,
            graph_keys,
            graphs: graphs.len(),
            phases: g.times,
            fingerprint,
            clone,
        },
        graphs,
        fills,
        parse_errors: g.parse_errors,
        input,
    }
}

/// GA.4: whether the body-only fast path is on: `GLIA_WATCH_FAST_PATH=0`
/// turns it off, any other value (or none) leaves it on. Off, every group
/// whose fingerprint moved rebuilds, as before GA.4; the graph is the same
/// either way (the parity gates).
fn fast_path_from_env() -> bool {
    fast_path_from_value(std::env::var("GLIA_WATCH_FAST_PATH").ok().as_deref())
}

/// [`fast_path_from_env`]'s reading of the variable's value.
fn fast_path_from_value(value: Option<&str>) -> bool {
    value.map(str::trim) != Some("0")
}

/// One build's inputs to every group's fingerprints besides the group's own:
/// [`overlay_fp`] (GA.17: the fast path's `context` only) and
/// [`proto_services_fp`] (the whole key and the context).
#[derive(Clone, Copy)]
struct GroupKey {
    overlay_fp: u64,
    services_fp: u64,
}

/// What [`build`] hands each repo's [`build_repo`] besides its own state:
/// the build's proto service set and whether the fast path is on.
#[derive(Clone, Copy)]
struct Keyed {
    services_fp: u64,
    fast_path: bool,
}

/// GA.4: one parse of a fast-path group, as [`group_fingerprints`] keys it:
/// its file ([`parse_path`]), its `cache::parse_fingerprint` and its
/// `cache::surface_fingerprint` (the declaration surface, GA.1).
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileFp {
    path: String,
    parse: u64,
    surface: u64,
}

/// One group's fingerprints ([`group_fingerprints`]).
struct GroupPrint {
    /// DC.7b: the whole key; `None` when a parse could not be fingerprinted
    /// (the group is always rebuilt and never cached).
    fingerprint: Option<u64>,
    /// GA.4: xxh64 of the key's inputs besides the parses (the group key,
    /// the repo id, the proto services and [`hidden_fp`]) and of the overlay
    /// ([`overlay_fp`], GA.17: in this compare, not in the whole key).
    context: u64,
    /// GA.4: every parse's [`FileFp`], in plan order, for a group with a
    /// fast path ([`fast_path_for`], with the session's fast path on);
    /// `None` for any other group, or when a parse has no file, a file is
    /// named twice or a fingerprint failed: such a group never takes it.
    files: Option<Vec<FileFp>>,
}

/// GA.4: the file a parse was read from: its MODULE's POSITION file (every
/// code parser emits the file's MODULE first, `rpc_needles::parse_index`).
/// `None` when its first node is no MODULE or has no file.
fn parse_path(fp: &FileParse) -> Option<String> {
    let first = fp.nodes.first()?;
    if fp.nav.kind_by_id.get(&first.id) != Some(&node_kind::MODULE) {
        return None;
    }
    evidence::locate(&first.cells).map(|(file, _)| file)
}

/// DC.7b: each group's fingerprints ([`GroupPrint`]), in plan order. The
/// whole key is xxh64 of its key, the repo id, `key.services_fp`
/// ([`proto_services_fp`]), its [`hidden_fp`] and its parse count and
/// parses' fingerprints in plan order (`cache::parse_fingerprint`, every
/// parse of the plan mapped on the pool at once).
///
/// GA.17: `key.overlay_fp` ([`overlay_fp`]) is no part of the whole key.
/// Every overlay section reaches a language group only through the walk or
/// the post-cache grafts, so through the post-graft parses the key already
/// holds, or not at all (it is read by the pass tail or at query time):
/// [`OVERLAY_REACH`], whose test pins every section. An overlay edit then
/// rebuilds only the groups whose parses it moved (`[groups] ..
/// reused=<r>` counts the rest). The overlay stays in the fast path's
/// `context`, so a group is never re-stitched against a build made under
/// another overlay.
///
/// GA.4: with `fast_path` on, a group whose key has a fast path also gets
/// its [`FileFp`]s. A parse whose path and parse fingerprint equal its slot
/// in the group's `cached` entry takes that entry's surface fingerprint (an
/// equal parse has an equal surface); every other parse's surface is
/// fingerprinted on the pool, so an update fingerprints only its changed
/// files' surfaces.
fn group_fingerprints(
    groups: &[GroupInput],
    ctx: &GroupCtx<'_>,
    key: GroupKey,
    cached: &[CachedGroup],
    fast_path: bool,
) -> Vec<GroupPrint> {
    let parses: Vec<&FileParse> = groups.iter().flat_map(|g| &g.parses).collect();
    let (parse_fps, _threads) = crate::parallel::par_map_ordered(&parses, |p| parse_fingerprint(p));
    let mut parse_fps = parse_fps.into_iter();
    // The parses a fast-path group still needs a surface fingerprint for,
    // as (group, slot): computed below in one pool pass.
    let mut prints: Vec<GroupPrint> = Vec::with_capacity(groups.len());
    let mut wanted: Vec<(usize, usize)> = Vec::new();
    for (at, g) in groups.iter().enumerate() {
        let hidden = hidden_fp(g.key, ctx);
        // GA.17: the overlay reaches a group only through its post-graft
        // parses (`OVERLAY_REACH`), so it is in the fast path's context and
        // not in the whole key.
        let mut c = XxHash64::with_seed(0);
        c.write(g.key.as_bytes());
        c.write_u8(0);
        c.write_u64(ctx.repo.0);
        c.write_u64(key.overlay_fp);
        c.write_u64(key.services_fp);
        c.write_u64(hidden);
        let context = c.finish();
        let mut h = XxHash64::with_seed(0);
        h.write(g.key.as_bytes());
        h.write_u8(0);
        h.write_u64(ctx.repo.0);
        h.write_u64(key.services_fp);
        h.write_u64(hidden);
        h.write_u64(g.parses.len() as u64);
        let fps: Vec<Option<u64>> = parse_fps.by_ref().take(g.parses.len()).collect();
        let mut whole = true;
        for fp in &fps {
            match fp {
                Some(fp) => h.write_u64(*fp),
                None => whole = false,
            }
        }
        let fingerprint = whole.then(|| h.finish());
        let files = (fast_path && whole && fast_path_for(g.key))
            .then(|| {
                let old = cached
                    .iter()
                    .find(|c| c.key == g.key)
                    .and_then(|c| c.files.as_deref());
                let mut seen: BTreeSet<String> = BTreeSet::new();
                let mut files = Vec::with_capacity(g.parses.len());
                for (slot, (p, fp)) in g.parses.iter().zip(&fps).enumerate() {
                    let (path, parse) = (parse_path(p)?, (*fp)?);
                    if !seen.insert(path.clone()) {
                        return None;
                    }
                    let surface = match old.and_then(|o| o.get(slot)) {
                        Some(o) if o.path == path && o.parse == parse => o.surface,
                        _ => {
                            wanted.push((at, slot));
                            0
                        }
                    };
                    files.push(FileFp {
                        path,
                        parse,
                        surface,
                    });
                }
                Some(files)
            })
            .flatten();
        prints.push(GroupPrint {
            fingerprint,
            context,
            files,
        });
    }
    let fresh: Vec<&FileParse> = wanted
        .iter()
        .filter_map(|&(at, slot)| groups.get(at).and_then(|g| g.parses.get(slot)))
        .collect();
    let (surfaces, _threads) = crate::parallel::par_map_ordered(&fresh, |p| surface_fingerprint(p));
    for ((at, slot), surface) in wanted.into_iter().zip(surfaces) {
        let Some(print) = prints.get_mut(at) else {
            continue;
        };
        match surface {
            Some(fp) => {
                if let Some(f) = print.files.as_mut().and_then(|fs| fs.get_mut(slot)) {
                    f.surface = fp;
                }
            }
            None => print.files = None,
        }
    }
    prints
}

/// GA.4: the re-stitch seed of group `key`, rebuilt this update, when its
/// edits are body-only: its `cached` entry holds [`FileFp`]s and the same
/// `context` (no overlay, proto service or builder input moved), the group
/// reads the same files in the same order (none added or removed), and
/// every file whose parse moved kept its surface fingerprint. The seed is a
/// clone of the entry's build and the moved parses' slots. `None` sends the
/// group to an ordinary build.
fn rebody_seed(key: &'static str, print: &GroupPrint, cached: &[CachedGroup]) -> Option<Seed> {
    if !fast_path_for(key) {
        return None;
    }
    let entry = cached.iter().find(|c| c.key == key)?;
    let (old, new) = (entry.files.as_deref()?, print.files.as_deref()?);
    if entry.context != print.context || old.len() != new.len() {
        return None;
    }
    let mut edited = Vec::new();
    for (slot, (o, n)) in old.iter().zip(new).enumerate() {
        if o.path != n.path || o.surface != n.surface {
            return None;
        }
        if o.parse != n.parse {
            edited.push(slot);
        }
    }
    if edited.is_empty() {
        return None;
    }
    Some(Seed::Rebody {
        cached: entry.build.clone(),
        edited,
    })
}

/// GA.4's fired_on markers for one group of `label`'s build:
/// `[watch] fast_path key=<k> files=<n> ms=<t> repo=<label>` for a group
/// that took the body-only fast path, `[watch] fast_path refused key=<k>
/// reason=<r> repo=<label>` for one whose re-stitch was refused (it was
/// built instead); `None` for every other group.
fn fast_path_marker(key: &str, made: &Made, label: &str) -> Option<String> {
    match made {
        Made::Rebodied { files, ms } => Some(format!(
            "[watch] fast_path key={key} files={files} ms={ms:.1} repo={label}"
        )),
        Made::Refused { reason } => Some(format!(
            "[watch] fast_path refused key={key} reason={reason} repo={label}"
        )),
        Made::Built | Made::Reused => None,
    }
}

/// DC.7b: the repo-level inputs group `key`'s builder reads besides its
/// parses and the repo id (lang_build's `build_one_group` / `build_solo`):
/// the Cargo packages for `rust` (`build_rust`), the include roots for
/// `c_cpp` (`build_c_cpp`'s resolver), the own Dart packages for `dart`
/// (`resolve_dart_source`), the tsconfig aliases for the TS family's group
/// (`build_typescript_family`), the go.mod root dirs for `go`
/// (`build_go_with`'s cross-module gate, FD.81); 0 for every other group. Each is hashed
/// through its `Debug`, which holds only strings, vecs, options, ordered maps
/// and counts, so it is a pure function of the content (`DartPackages`' own
/// resolved-URI counter is in it too: a count left over could only cost a
/// rebuild, never hide a change).
///
/// A later builder that reads a new repo-level input adds it to `GroupCtx`,
/// and the exhaustive destructuring below stops compiling until it joins a
/// row here: a builder input left out of the key is a stale group.
fn hidden_fp(key: &str, ctx: &GroupCtx<'_>) -> u64 {
    // `repo` is in every key already; `repo_label` only labels markers.
    let GroupCtx {
        repo: _,
        repo_label: _,
        rust_crates,
        ts_aliases,
        c_includes,
        dart_packages,
        go_module_dirs,
    } = *ctx;
    let debug = match key {
        "rust" => format!("{rust_crates:?}"),
        "c_cpp" => format!("{c_includes:?}"),
        "dart" => format!("{dart_packages:?}"),
        "go" => format!("{go_module_dirs:?}"),
        _ if TS_FAMILY.contains(&key) => format!("{ts_aliases:?}"),
        _ => return 0,
    };
    let mut h = XxHash64::with_seed(0);
    h.write(debug.as_bytes());
    h.finish()
}

/// DC.7b: the reuse slots `build_groups` takes, aligned with the plan `keys`:
/// a clone of the cached build of a group whose key and fingerprint match an
/// entry of `cached`, else `None`; and that entry's index in `cached`.
fn reuse_slots(
    keys: &[&'static str],
    prints: &[GroupPrint],
    cached: &[CachedGroup],
) -> (Vec<Option<LangBuild>>, Vec<Option<usize>>) {
    let by_key: BTreeMap<&str, usize> =
        cached.iter().enumerate().map(|(i, c)| (c.key, i)).collect();
    keys.iter()
        .zip(prints)
        .map(|(key, print)| {
            let hit = print.fingerprint.and_then(|fp| {
                by_key.get(key).and_then(|&i| {
                    cached
                        .get(i)
                        .filter(|c| c.fingerprint == fp)
                        .map(|c| (i, c))
                })
            });
            (hit.map(|(_, c)| c.build.clone()), hit.map(|(i, _)| i))
        })
        .unzip()
}

/// DC.7b: the group cache an update leaves, aligned with the plan: a reused
/// group keeps its previous entry (`kept`), a rebuilt one with a fingerprint
/// (built, or re-stitched by the fast path) takes a clone of its build and
/// its prints.
fn cache_slots(builds: &[LangBuild], prints: Vec<GroupPrint>, kept: &[Option<usize>]) -> Vec<Slot> {
    builds
        .iter()
        .zip(prints)
        .zip(kept)
        .filter_map(|((build, print), kept)| match kept {
            Some(i) => Some(Slot::Kept(*i)),
            None => print.fingerprint.map(|fingerprint| {
                Slot::Fresh(Box::new(CachedGroup {
                    key: build.key(),
                    fingerprint,
                    context: print.context,
                    files: print.files,
                    build: build.clone(),
                    last_used: 0,
                }))
            }),
        })
        .collect()
}

/// DC.7b: the refreshed group cache: `slots` in plan order, a kept slot
/// moving its entry out of `old` (the previous cache); a group that vanished
/// from the plan is dropped with `old`. Every entry it keeps was built or
/// reused by the update `seq` (GA.12's `last_used`).
fn refresh_groups(old: Vec<CachedGroup>, slots: Vec<Slot>, seq: u64) -> Vec<CachedGroup> {
    let mut old: Vec<Option<CachedGroup>> = old.into_iter().map(Some).collect();
    slots
        .into_iter()
        .filter_map(|slot| match slot {
            Slot::Kept(i) => old.get_mut(i).and_then(Option::take),
            Slot::Fresh(group) => Some(*group),
        })
        .map(|mut group| {
            group.last_used = seq;
            group
        })
        .collect()
}

/// DC.7b: one side of a Low update's [`DeltaScope`]: the indices of that
/// side's language graphs (`keys`, index for index) whose group was rebuilt
/// or is missing from the other side's graphs (`other`: a new or vanished
/// group, or one whose reused build failed), plus every graph after them
/// (the region / project / docs graphs, rebuilt every time) up to `graphs`.
fn scope_side(
    keys: &[&'static str],
    graphs: usize,
    rebuilt: &[&'static str],
    other: &[&'static str],
) -> BTreeSet<usize> {
    let mut scope: BTreeSet<usize> = keys
        .iter()
        .enumerate()
        .filter(|(_, key)| rebuilt.contains(key) || !other.contains(key))
        .map(|(i, _)| i)
        .collect();
    scope.extend(keys.len()..graphs);
    scope
}

/// `repo_inputs`' overlay config (it prints the `[overlay] loaded` line).
fn load_config(at: &At<'_>, repo: RepoId) -> Option<LoadedConfig> {
    repo_inputs(
        repo,
        at.root.to_path_buf(),
        at.label.to_string(),
        at.overlay_text,
    )
    .config
}

/// The per-repo full-rebuild reasons, in the order an update reports the
/// first one found over its repos ([`full_rebuild_reason`]); a moved joint
/// proto service set (`proto_services`) comes after all of them.
const REBUILD_REASONS: [&str; 3] = ["identity", "go_modules", "overlay"];

/// The reason a repo's update counts as a full rebuild: the identity key
/// moved (every NodeId re-keys), the go module context moved (the parse
/// cache discards every parse) or the overlay moved. The build-wide reason,
/// the proto service set moving (every gRPC needle re-runs), is the
/// caller's ([`WatchSession::apply`]).
fn full_rebuild_reason(before: &Setup, after: &Setup) -> Option<&'static str> {
    if before.ident.key != after.ident.key {
        Some("identity")
    } else if before.go.context_key() != after.go.context_key() {
        Some("go_modules")
    } else if before.overlay_fp != after.overlay_fp {
        Some("overlay")
    } else {
        None
    }
}

/// GA.9: refuse roots one session cannot route between: two that are one
/// directory, or one inside another, compared by canonical path (a root
/// that cannot be canonicalised is compared as given).
fn check_roots(roots: &[String]) -> Result<(), String> {
    let canon: Vec<PathBuf> = roots
        .iter()
        .map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| PathBuf::from(r)))
        .collect();
    for (i, (outer, a)) in roots.iter().zip(&canon).enumerate() {
        for (inner, b) in roots.iter().zip(&canon).skip(i + 1) {
            if a == b {
                return Err(format!(
                    "duplicate watch roots: {outer} and {inner} are one directory"
                ));
            }
            if b.starts_with(a) {
                return Err(format!("nested watch roots: {inner} is inside {outer}"));
            }
            if a.starts_with(b) {
                return Err(format!("nested watch roots: {outer} is inside {inner}"));
            }
        }
    }
    Ok(())
}

/// GA.9: the build identities of `raws` (each root's own, in build order),
/// disambiguated as `generate_many` disambiguates them (`build::disambiguate`
/// over each root's canonical path); prints its collision lines.
fn settle_identities(roots: &[&Path], mut raws: Vec<RepoIdentity>) -> Vec<RepoIdentity> {
    let abs: Vec<String> = roots.iter().map(|r| canonical_display(r)).collect();
    for line in disambiguate(&mut raws, &abs) {
        eprintln!("{line}");
    }
    raws
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

/// The build's proto service set over every repo's walked files, as
/// `RpcContext::add_files` folds it (sorted, deduplicated), fingerprinted as
/// the DC.4 needle memo keys it.
fn proto_services_fp<'a>(repos: impl IntoIterator<Item = &'a [(String, String)]>) -> u64 {
    let mut services: Vec<grpc::ProtoServiceRef> = repos
        .into_iter()
        .flatten()
        .filter(|(path, _)| detect_language(path) == Some("proto"))
        .flat_map(|(_, source)| grpc::proto_service_refs(source))
        .collect();
    services.sort_unstable();
    services.dedup();
    crate::build::memo::services_fp(&services)
}

/// xxh64 of `opts.overlay` and the overlay the repo's build reads: its given
/// text ([`At::overlay_text`]), or `.glia/overlay.toml`'s bytes (a missing
/// file apart from an empty one).
fn overlay_fp(at: &At<'_>) -> u64 {
    let root = at.root;
    let mut h = XxHash64::with_seed(0);
    h.write_u8(u8::from(at.opts.overlay));
    match at.overlay_text {
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
    let mut h = XxHash64::with_seed(0);
    for (rel, path) in glia_files(root) {
        h.write(rel.as_bytes());
        h.write_u8(0);
        h.write(&std::fs::read(&path).unwrap_or_default());
        h.write_u8(0);
    }
    h.finish()
}

/// Every file under `<root>/.glia` but the layout dir `.glia/graph`, the
/// ranking calibration `.glia/calibration-snapshot` and the query-time inputs
/// (`vectors.jsonl`, `vocab-snapshot`; no build reads any of them), by
/// sorted path relative to `.glia`: what [`glia_inputs_fp`] hashes and the
/// stat poll watches ([`Input::Glia`]).
fn glia_files(root: &Path) -> Vec<(String, PathBuf)> {
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
            if rel == "graph" || rel == CALIBRATION_DIR || is_query_time_rel(&rel) {
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
    files
}

/// DC.7c: a watched input's stat: `None` when it is absent or not a file.
fn file_stamp(path: &Path) -> Option<FileStamp> {
    let m = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    Some(FileStamp {
        len: m.len(),
        mtime: m.modified().ok(),
    })
}

/// DC.7c: a walked directory's mtime now, as the plan took it (following
/// symlinks); `None` when it is gone or no longer a directory.
fn dir_mtime(path: &Path) -> Option<Option<SystemTime>> {
    let m = std::fs::metadata(path).ok().filter(|m| m.is_dir())?;
    Some(m.modified().ok())
}

/// DC.7c: a directory's listing fingerprint: xxh64 of its entries by sorted
/// name, each with the kinds the walk and `gate_dir` tell apart (a symlink,
/// a directory, a file, following symlinks for the last two). An unlistable
/// directory has its own value.
fn dir_fp(dir: &Path) -> u64 {
    let mut h = XxHash64::with_seed(0);
    let Ok(entries) = std::fs::read_dir(dir) else {
        h.write_u8(0);
        return h.finish();
    };
    let mut names: Vec<(OsString, u8)> = entries
        .flatten()
        .map(|e| {
            let path = e.path();
            let kind = u8::from(e.file_type().is_ok_and(|t| t.is_symlink())) << 2
                | u8::from(path.is_dir()) << 1
                | u8::from(path.is_file());
            (e.file_name(), kind)
        })
        .collect();
    names.sort();
    h.write_u8(1);
    for (name, kind) in names {
        h.write(name.as_encoded_bytes());
        h.write_u8(0);
        h.write_u8(kind);
    }
    h.finish()
}

/// DC.7c: a watched input's content fingerprint: xxh64 of its bytes, an
/// unreadable one its own value.
fn bytes_fp(path: &Path) -> u64 {
    let mut h = XxHash64::with_seed(0);
    match std::fs::read(path) {
        Ok(bytes) => {
            h.write_u8(1);
            h.write(&bytes);
        }
        Err(_) => h.write_u8(0),
    }
    h.finish()
}

/// DC.7c: every directory `IncludeRoots::read` reads a `CMakeLists.txt` in
/// (c_includes' `HeldDirs`, mirrored): the root and each directory above a
/// walked C/C++ file, repo-relative.
fn held_dirs(c_files: &[&str]) -> BTreeSet<String> {
    let mut out = BTreeSet::from([String::new()]);
    for file in c_files {
        let parts: Vec<&str> = file.split('/').filter(|s| !s.is_empty()).collect();
        let mut dir = String::new();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            if !dir.is_empty() {
                dir.push('/');
            }
            dir.push_str(part);
            out.insert(dir.clone());
        }
    }
    out
}

/// DC.7c: every `compile_commands.json` `IncludeRoots::read` looks for
/// (c_includes' `compile_commands_dirs`, mirrored), repo-relative: at the
/// root and at each CMake project root, and in each of their depth-1
/// `build*`, `out` and `cmake-build-*` directories (listed now; a new one
/// moves its parent's mtime, which re-plans and lists them again).
fn compile_dbs(root: &Path, roots: &[ProjectRoot]) -> Vec<String> {
    let dirs: BTreeSet<&str> = std::iter::once("")
        .chain(
            roots
                .iter()
                .filter(|r| r.ecosystem == "cmake")
                .map(|r| r.rel_path.as_str()),
        )
        .collect();
    let mut out = Vec::new();
    for dir in dirs {
        out.push(join_rel(dir, COMPILE_COMMANDS));
        let mut subs: Vec<String> = std::fs::read_dir(root.join(dir))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.starts_with("build") || n == "out" || n.starts_with("cmake-build-"))
            .collect();
        subs.sort();
        out.extend(
            subs.iter()
                .map(|sub| join_rel(&join_rel(dir, sub), COMPILE_COMMANDS)),
        );
    }
    out
}

/// `name` under the repo-relative directory `dir` (`""` the root).
fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
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

/// The `[watch] poll` line (module doc).
fn poll_marker(scan: &Scan, label: &str) -> String {
    format!(
        "[watch] poll files={} dirs={} watched={} changed={} racy={} took={} repo={label}",
        scan.files,
        scan.dirs,
        scan.watched,
        scan.found.len(),
        scan.racy,
        timing::ms(scan.took),
    )
}

/// GA.12: `repo`'s `[watch] rss` line (module doc): `evicted` groups and
/// `memo_cleared` by the enforcement that just ran.
fn rss_marker(
    seq: u64,
    rss_kb: Option<u64>,
    budget: MemoryBudget,
    repo: &RepoWatch,
    evicted: usize,
    memo_cleared: bool,
) -> String {
    let (cached, groups, label) = (repo.groups.len(), repo.group_count, &repo.label);
    let rss = rss_kb.map_or_else(|| "?".to_string(), |kb| (kb / 1024).to_string());
    let budget = budget
        .budget_mb()
        .map_or_else(|| "none".to_string(), |mb| mb.to_string());
    format!(
        "[watch] rss seq={seq} rss_mb={rss} budget_mb={budget} cached_groups={cached}/{groups} \
         evicted={evicted} memo_cleared={} repo={label}",
        u8::from(memo_cleared)
    )
}

/// The `[watch] update` line (module doc): `groups` the plans' language
/// groups summed over the repos, `label` the repos with named paths
/// (GA.9, comma-joined).
fn update_marker(u: &WatchUpdate, groups: usize, took: Duration, label: &str) -> String {
    let c = &u.counts;
    format!(
        "[watch] update seq={} files +{} ~{} -{} groups_rebuilt={}/{groups} body_only={} \
         full_rebuild={} nodes +{} -{} ~{} >{} edges +{} -{} ~{} restamped={} took={} repo={label}",
        u.seq,
        u.files.added.len(),
        u.files.modified.len(),
        u.files.removed.len(),
        u.groups_rebuilt.len(),
        if u.body_only.is_empty() {
            "-".to_string()
        } else {
            u.body_only.join(",")
        },
        u.full_rebuild.as_deref().unwrap_or("none"),
        c.nodes_added,
        c.nodes_removed,
        c.nodes_modified,
        c.nodes_moved,
        c.edges_added,
        c.edges_removed,
        c.edges_reconfidenced,
        c.edges_restamped,
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
        // DC.7b: the go group's parses are unchanged, so its build is reused.
        assert_eq!(u.groups_rebuilt, ["python"]);
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

    /// GA.4: four python files, each importing the one before.
    fn py_chain(tmp: &Path) -> PathBuf {
        let dir = tmp.join("chain");
        write(
            &dir,
            "pkg/models.py",
            "class User:\n    def __init__(self, name):\n        self.name = name\n\n    def greet(self):\n        return \"hi \" + self.name\n",
        );
        write(
            &dir,
            "pkg/store.py",
            "from pkg.models import User\n\n\ndef load(name):\n    return User(name)\n",
        );
        write(
            &dir,
            "pkg/service.py",
            "from pkg.store import load\n\n\ndef greet_all(names):\n    return [load(n).greet() for n in names]\n",
        );
        write(
            &dir,
            "app.py",
            "from pkg.service import greet_all\n\n\ndef main():\n    return greet_all([\"a\"])\n",
        );
        dir
    }

    #[test]
    fn fast_path_values() {
        assert!(fast_path_from_value(None));
        assert!(fast_path_from_value(Some("1")));
        assert!(fast_path_from_value(Some("")));
        assert!(!fast_path_from_value(Some("0")));
        assert!(!fast_path_from_value(Some(" 0 ")));
    }

    /// GA.4: a body edit takes the fast path (and a clean build's bytes); a
    /// declaration edit does not; with the fast path off the same body edit
    /// rebuilds the group to the same bytes.
    #[test]
    fn body_only_fast_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = py_chain(tmp.path());
        let root = dir.to_str().unwrap();
        let mut s = WatchSession::open(root, &BuildOptions::default()).unwrap();
        s.fast_path = true;
        write(
            &dir,
            "pkg/store.py",
            "from pkg.models import User\n\n\ndef load(name):\n    user = User(name)\n    user.greet()\n    return user\n",
        );
        let u = s.apply(&[PathBuf::from("pkg/store.py")]).unwrap();
        assert_eq!(u.groups_rebuilt, ["python"]);
        assert_eq!(u.body_only, ["python"]);
        assert!(update_marker(&u, 1, Duration::ZERO, "chain").contains(" body_only=python "));
        assert_clean(&s, &dir, tmp.path(), "fast_body");

        // A new function: the surface moved, the group rebuilds.
        let store = std::fs::read_to_string(dir.join("pkg/store.py")).unwrap();
        write(
            &dir,
            "pkg/store.py",
            &format!("{store}\n\ndef drop(name):\n    return name\n"),
        );
        let u = s.apply(&[PathBuf::from("pkg/store.py")]).unwrap();
        assert_eq!(u.groups_rebuilt, ["python"]);
        assert!(u.body_only.is_empty(), "{:?}", u.body_only);
        assert!(update_marker(&u, 1, Duration::ZERO, "chain").contains(" body_only=- "));
        assert_clean(&s, &dir, tmp.path(), "fast_surface");

        // Off: the same kind of body edit rebuilds the group.
        s.fast_path = false;
        write(
            &dir,
            "app.py",
            "from pkg.service import greet_all\n\n\ndef main():\n    names = [\"a\", \"b\"]\n    return greet_all(names)\n",
        );
        let u = s.apply(&[PathBuf::from("app.py")]).unwrap();
        assert_eq!(u.groups_rebuilt, ["python"]);
        assert!(u.body_only.is_empty(), "{:?}", u.body_only);
        assert_clean(&s, &dir, tmp.path(), "fast_off");
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
        // EC.1: the ranking calibration is no build input: the stat poll
        // never lists it and naming it is a noop.
        write(&dir, ".glia/calibration-snapshot/calibration.json", "{}\n");
        // (Creating `.glia/` moves the root's mtime, and the racy-mtime rule
        // may list fresh files: only the calibration path is pinned.)
        let listed = s.changed_paths();
        assert!(
            !listed
                .iter()
                .any(|p| p.to_string_lossy().contains("calibration-snapshot")),
            "{listed:?}"
        );
        write(&dir, ".idea/workspace.xml", "<x/>");
        for paths in [
            vec![PathBuf::from("a.py")],
            vec![PathBuf::from("debug.log")],
            vec![PathBuf::from(".glia/graph/manifest.json")],
            vec![
                PathBuf::from(".glia/calibration-snapshot/calibration.json"),
                PathBuf::from(".glia/calibration-snapshot"),
            ],
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

    /// FB.3: the vectors sidecar and the vocab snapshot are query-time: the
    /// stat poll never lists them, and naming them (or the sidecar writer's
    /// `.tmp`) is a noop, no re-plan.
    #[test]
    fn query_time_inputs_are_noops() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        let mut s = WatchSession::open(dir.to_str().unwrap(), &BuildOptions::default()).unwrap();
        let totals = (s.result().total_nodes, s.result().total_edges);
        write(&dir, ".glia/vectors.jsonl", "{\"qname\":\"a::foo\"}\n");
        write(&dir, ".glia/vocab-snapshot/terms.jsonl", "{}\n");
        let listed = s.changed_paths();
        assert!(
            !listed.iter().any(|p| {
                let p = p.to_string_lossy();
                p.contains("vectors.jsonl") || p.contains("vocab-snapshot")
            }),
            "{listed:?}"
        );
        for paths in [
            vec![PathBuf::from(".glia/vectors.jsonl")],
            vec![PathBuf::from(".glia/vectors.jsonl.tmp")],
            vec![
                PathBuf::from(".glia/vocab-snapshot/terms.jsonl"),
                PathBuf::from(".glia/vocab-snapshot"),
            ],
        ] {
            let u = s.apply(&paths).unwrap();
            assert_eq!(u.seq, 0, "{paths:?}: {u:?}");
            assert!(u.full_rebuild.is_none(), "{paths:?}: {u:?}");
            assert!(u.groups_rebuilt.is_empty(), "{paths:?}: {u:?}");
            assert_eq!((u.total_nodes, u.total_edges), totals, "{paths:?}");
        }
        assert_clean(&s, &dir, tmp.path(), "query_time_inputs");
    }

    #[test]
    fn durability_table() {
        let high = [
            (".glia/overlay.toml", "glia_inputs"),
            (".glia/scip-snapshot/index.json", "glia_inputs"),
            (".glia/vectors.jsonl.bak", "glia_inputs"),
            (".glia/vocab-snapshotx/a", "glia_inputs"),
            (".glia", "glia_inputs"),
            (".gitignore", "gitignore"),
            ("web/.gitignore", "gitignore"),
            ("svc/go.mod", "go_mod"),
            ("tsconfig.json", "tsconfig"),
            ("app/tsconfig.base.json", "tsconfig"),
            ("webpack.config.js", "bundler"),
            ("frontend/webpack.config.ts", "bundler"),
            ("web/vite.config.mjs", "bundler"),
            ("vitest.config.js", "bundler"),
            ("web/vitest.config.mts", "bundler"),
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
            ".glia/calibration-snapshot/calibration.json",
            ".glia/vectors.jsonl",
            ".glia/vectors.jsonl.tmp",
            ".glia/vocab-snapshot",
            ".glia/vocab-snapshot/terms.jsonl",
            "report.sql",
            "src/webpack.config.helper.js",
            "vitest.workspace.ts",
            "web/vitest.setup.ts",
        ] {
            assert_eq!(durability(rel), Durability::Low, "{rel}");
        }
    }

    /// [`repo`] plus TypeScript behind a root tsconfig `paths` alias that maps
    /// nowhere yet, a Cargo workspace of one member crate `a` calling into a
    /// crate dir `b` with no manifest yet, and a proto service.
    fn mixed_repo(tmp: &Path) -> PathBuf {
        let dir = repo(tmp);
        write(&dir, "tsconfig.json", &tsconfig("web/none/*"));
        write(
            &dir,
            "web/app.ts",
            "import { util } from \"@lib/util\";\n\nexport function main(): number {\n  return util();\n}\n",
        );
        write(
            &dir,
            "web/lib/util.ts",
            "export function util(): number {\n  return 1;\n}\n",
        );
        write(
            &dir,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/a\"]\n",
        );
        write(
            &dir,
            "crates/a/Cargo.toml",
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\n",
        );
        write(
            &dir,
            "crates/a/src/lib.rs",
            "pub fn run() -> i32 {\n    b::helper()\n}\n",
        );
        write(
            &dir,
            "crates/b/src/lib.rs",
            "pub fn helper() -> i32 {\n    1\n}\n",
        );
        write(&dir, "api/greet.proto", &proto(""));
        dir
    }

    /// A root tsconfig whose one `paths` alias `@lib/*` maps to `target`.
    fn tsconfig(target: &str) -> String {
        format!(
            "{{\n  \"compilerOptions\": {{\n    \"baseUrl\": \".\",\n    \"paths\": {{ \"@lib/*\": [\"{target}\"] }}\n  }}\n}}\n"
        )
    }

    /// One proto service, with `extra` appended inside it.
    fn proto(extra: &str) -> String {
        format!(
            "syntax = \"proto3\";\npackage greet;\n\nservice Greeter {{\n  rpc Hello (Req) returns (Rep);\n{extra}}}\n\nmessage Req {{}}\nmessage Rep {{}}\n"
        )
    }

    fn open(dir: &Path) -> WatchSession {
        WatchSession::open(dir.to_str().unwrap(), &BuildOptions::default()).unwrap()
    }

    /// The edits the cutoff tests make, each as (tag, the files it writes,
    /// the paths it names), applied in order on one [`mixed_repo`] session.
    type Edit = (&'static str, Vec<(&'static str, String)>, Vec<&'static str>);

    fn edits() -> Vec<Edit> {
        vec![
            (
                "python_body",
                vec![(
                    "a.py",
                    "def foo():\n    return bar() + 1\n\ndef bar():\n    return 1\n".into(),
                )],
                vec!["a.py"],
            ),
            (
                "tsconfig_paths",
                vec![("tsconfig.json", tsconfig("web/lib/*"))],
                vec!["tsconfig.json"],
            ),
            (
                "cargo_member_add",
                vec![
                    (
                        "Cargo.toml",
                        "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n".into(),
                    ),
                    (
                        "crates/b/Cargo.toml",
                        "[package]\nname = \"b\"\nversion = \"0.1.0\"\n".into(),
                    ),
                ],
                vec!["Cargo.toml", "crates/b/Cargo.toml"],
            ),
            (
                "proto_service",
                vec![("api/greet.proto", proto("  rpc Bye (Req) returns (Rep);\n"))],
                vec!["api/greet.proto"],
            ),
            (
                "go_body",
                vec![(
                    "m.go",
                    "package m\n\nfunc F() int { return G() + 1 }\n\nfunc G() int { return 1 }\n"
                        .into(),
                )],
                vec!["m.go"],
            ),
            (
                "python_add",
                vec![("pkg/c.py", "def c():\n    return foo()\n".into())],
                vec!["pkg/c.py"],
            ),
            ("python_remove", Vec::new(), vec!["pkg/c.py"]),
        ]
    }

    /// Write `edit`'s files (an empty list removes its one named path) and
    /// apply its paths.
    fn run_edit(s: &mut WatchSession, dir: &Path, edit: &Edit) -> WatchUpdate {
        let (_, files, paths) = edit;
        if files.is_empty() {
            for p in paths {
                std::fs::remove_file(dir.join(p)).unwrap();
            }
        }
        for (rel, body) in files {
            write(dir, rel, body);
        }
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        s.apply(&paths).unwrap()
    }

    #[test]
    fn python_body_edit_rebuilds_only_python() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mixed_repo(tmp.path());
        let mut s = open(&dir);
        let keys = cached_keys(&s);
        let u = run_edit(&mut s, &dir, &edits()[0]);
        assert_eq!(u.groups_rebuilt, ["python"]);
        assert_eq!(u.full_rebuild, None);
        assert_eq!(cached_keys(&s), keys, "every group stays cached");
        assert_clean(&s, &dir, tmp.path(), "python_body");
    }

    #[test]
    fn tsconfig_paths_edit_rebuilds_typescript() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mixed_repo(tmp.path());
        let mut s = open(&dir);
        let u = run_edit(&mut s, &dir, &edits()[1]);
        assert!(
            u.groups_rebuilt.iter().any(|k| k == "typescript"),
            "{:?}",
            u.groups_rebuilt
        );
        assert!(
            !u.groups_rebuilt.iter().any(|k| k == "rust"),
            "{:?}",
            u.groups_rebuilt
        );
        assert_clean(&s, &dir, tmp.path(), "tsconfig_paths");
    }

    /// FH.16: a webpack config is a `bundler` input. Adding a module root
    /// re-plans and rebinds `common/api` (no source file changed), and the
    /// update equals a clean build.
    #[test]
    fn bundler_config_edit_rebinds_typescript() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        write(&dir, "package.json", "{\"name\":\"fx\"}\n");
        write(
            &dir,
            "webpack.config.js",
            "module.exports = { entry: './src/page.js' };\n",
        );
        write(
            &dir,
            "src/common/api.js",
            "export function getUser(id) {\n  return id;\n}\n",
        );
        write(
            &dir,
            "src/page.js",
            "import { getUser } from 'common/api';\n\nexport function show() {\n  return getUser(1);\n}\n",
        );
        let mut s = open(&dir);
        write(
            &dir,
            "webpack.config.js",
            "const path = require('path');\nmodule.exports = { resolve: { modules: [path.resolve(__dirname, 'src'), 'node_modules'] } };\n",
        );
        let u = s.apply(&[PathBuf::from("webpack.config.js")]).unwrap();
        assert!(
            u.groups_rebuilt.iter().any(|k| k == "typescript"),
            "{:?}",
            u.groups_rebuilt
        );
        assert_clean(&s, &dir, tmp.path(), "bundler_config");
    }

    /// The new crate `b` holds no new source file: only the Cargo packages
    /// `build_rust` resolves against move, so the rust group rebuilds through
    /// its hidden input alone (a reused rust graph would leave `b::helper()`
    /// unbound and differ from the clean build).
    #[test]
    fn cargo_member_add_rebuilds_rust() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mixed_repo(tmp.path());
        let mut s = open(&dir);
        let u = run_edit(&mut s, &dir, &edits()[2]);
        assert!(
            u.groups_rebuilt.iter().any(|k| k == "rust"),
            "{:?}",
            u.groups_rebuilt
        );
        assert!(
            !u.groups_rebuilt.iter().any(|k| k == "python" || k == "go"),
            "{:?}",
            u.groups_rebuilt
        );
        assert_clean(&s, &dir, tmp.path(), "cargo_member_add");
    }

    /// The plan keys the session caches a build for, in plan order.
    fn cached_keys(s: &WatchSession) -> Vec<String> {
        s.repos[0]
            .groups
            .iter()
            .map(|c| c.key.to_string())
            .collect()
    }

    /// A proto service change is a full rebuild: the service set is in
    /// every group's key, so every group rebuilds.
    #[test]
    fn proto_service_change_rebuilds_all() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mixed_repo(tmp.path());
        let mut s = open(&dir);
        let keys = cached_keys(&s);
        for key in ["go", "python", "rust", "typescript"] {
            assert!(keys.iter().any(|k| k == key), "{key}: {keys:?}");
        }
        let u = run_edit(&mut s, &dir, &edits()[3]);
        assert_eq!(u.full_rebuild.as_deref(), Some("proto_services"));
        assert_eq!(u.groups_rebuilt, keys);
        assert_eq!(cached_keys(&s), keys);
        assert_clean(&s, &dir, tmp.path(), "proto_service");
    }

    /// A reused build that failed replays its `parse_errors` line at its fold
    /// position: after the parse / graft lines, the groups' lines in plan
    /// order, as a clean build whose go and TS builds failed would hold them;
    /// and the scoped delta still equals the full one with those graphs gone.
    #[test]
    fn cached_error_keeps_clean_order() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mixed_repo(tmp.path());
        let mut s = open(&dir);
        for c in &mut s.repos[0].groups {
            if c.key == "go" || c.key == "typescript" {
                c.build = LangBuild::failed(c.key, &format!("{} graph: e", c.key));
            }
        }
        let prev = s.result().merged.clone();
        let u = run_edit(&mut s, &dir, &edits()[0]);
        assert_eq!(u.groups_rebuilt, ["python"]);
        let clean = generate_one(dir.to_str().unwrap()).unwrap();
        let mut want = clean.parse_errors.clone();
        want.extend(["go graph: e".to_string(), "typescript graph: e".to_string()]);
        assert_eq!(s.result().parse_errors, want);
        assert!(
            !s.repos[0]
                .graph_keys
                .iter()
                .any(|k| *k == "go" || *k == "typescript"),
            "{:?}",
            s.repos[0].graph_keys
        );
        assert_eq!(
            s.result().merged.graphs.len() + 2,
            clean.merged.graphs.len(),
            "two language graphs fewer"
        );
        let full = located_delta_counted(&prev, &s.result().merged, &[]);
        assert!(
            u.nodes
                .iter()
                .any(|n| n.change == "removed" && n.qname.ends_with("F")),
            "the go graph's nodes are gone: {:?}",
            u.nodes
        );
        assert_eq!(json(&u.nodes), json(&full.nodes));
        assert_eq!(json(&u.edges), json(&full.edges));
        assert_eq!(json(&u.counts), json(&full.counts));
        // The failed builds stay cached: a later update replays them again.
        write(&dir, "a.py", "def foo():\n    return 3\n");
        let u = s.apply(&[PathBuf::from("a.py")]).unwrap();
        assert_eq!(u.groups_rebuilt, ["python"]);
        assert_eq!(s.result().parse_errors, want);
    }

    /// Every edit's rows equal the located delta of the clean builds before
    /// and after it (a scoped delta on a Low update, the full one otherwise).
    #[test]
    fn scoped_delta_equals_full() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mixed_repo(tmp.path());
        let mut s = open(&dir);
        for edit in edits() {
            let tag = edit.0;
            let prev = generate_one(dir.to_str().unwrap()).unwrap();
            let u = run_edit(&mut s, &dir, &edit);
            let next = generate_one(dir.to_str().unwrap()).unwrap();
            let (_, nodes, edges) = crate::delta::located_delta(&prev.merged, &next.merged);
            assert!(
                !nodes.is_empty() || !edges.is_empty(),
                "{tag}: the edit moves the graph"
            );
            assert_eq!(json(&u.nodes), json(&nodes), "{tag}: node rows");
            assert_eq!(json(&u.edges), json(&edges), "{tag}: edge rows");
            assert_eq!(
                u.counts.nodes_added
                    + u.counts.nodes_removed
                    + u.counts.nodes_modified
                    + u.counts.nodes_moved,
                nodes.len(),
                "{tag}"
            );
            assert_clean(&s, &dir, tmp.path(), tag);
        }
    }

    /// `v` as a `serde_json::Value`, for row comparisons.
    fn json<T: serde::Serialize>(v: &T) -> serde_json::Value {
        serde_json::to_value(v).expect("serialise")
    }

    /// DC.7c: an hour ago, the mtime [`settle`] gives a fixture.
    fn old() -> SystemTime {
        SystemTime::now() - Duration::from_secs(3600)
    }

    /// Set the mtime of the file or directory at `path` (no clock ticks
    /// slept on).
    fn set_mtime(path: &Path, t: SystemTime) {
        std::fs::File::open(path).unwrap().set_modified(t).unwrap();
    }

    /// Every file and directory under `dir`, `dir` included, set to [`old`]:
    /// nothing in the fixture sits in the racy window when a session opens.
    fn settle(dir: &Path) {
        let t = old();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    set_mtime(&p, t);
                }
            }
            set_mtime(&d, t);
        }
    }

    /// `paths` (from `changed_paths`) as repo-relative paths, `""` the root.
    fn rels(s: &WatchSession, paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| s.repos[0].rel_of(p).unwrap())
            .collect()
    }

    #[test]
    fn poll_sees_edit_add_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        settle(&dir);
        let mut s = open(&dir);
        assert!(s.changed_paths().is_empty(), "a settled tree is quiet");
        assert!(s.poll().unwrap().is_none());

        write(
            &dir,
            "a.py",
            "def foo():\n    return bar() + 1\n\ndef bar():\n    return 1\n",
        );
        let found = s.changed_paths();
        assert!(found.iter().all(|p| p.is_absolute()), "{found:?}");
        assert_eq!(rels(&s, &found), ["a.py"]);
        let scan = s.repos[0].scan(s.scanned());
        let line = poll_marker(&scan, "r");
        assert!(
            line.starts_with(&format!(
                "[watch] poll files={} dirs={} watched={} changed=1 racy=",
                s.repos[0].records.len(),
                s.repos[0].plan.dirs.len() + s.repos[0].plan.region_dirs.len(),
                scan.watched
            )) && line.ends_with(" repo=r"),
            "{line}"
        );
        let u = s.poll().unwrap().expect("an edit rebuilds");
        assert_eq!(
            (u.seq, u.files.modified.clone()),
            (1, vec!["a.py".to_string()])
        );
        assert!(u.files.added.is_empty() && u.files.removed.is_empty());
        assert_clean(&s, &dir, tmp.path(), "poll_edit");
        assert!(s.changed_paths().is_empty(), "quiet after the update");
        assert!(s.poll().unwrap().is_none());

        write(&dir, "b.py", "def b():\n    return foo()\n");
        assert_eq!(rels(&s, &s.changed_paths()), [""], "the root's mtime moved");
        let u = s.poll().unwrap().expect("an add rebuilds");
        assert_eq!(
            (u.seq, u.files.added.clone()),
            (2, vec!["b.py".to_string()])
        );
        assert_clean(&s, &dir, tmp.path(), "poll_add");
        assert!(s.changed_paths().is_empty(), "quiet after the re-plan");

        std::fs::remove_file(dir.join("b.py")).unwrap();
        assert_eq!(rels(&s, &s.changed_paths()), ["", "b.py"]);
        let u = s.poll().unwrap().expect("a delete rebuilds");
        assert_eq!(
            (u.seq, u.files.removed.clone()),
            (3, vec!["b.py".to_string()])
        );
        assert_clean(&s, &dir, tmp.path(), "poll_delete");
        assert!(s.changed_paths().is_empty());
        assert!(s.poll().unwrap().is_none());
        assert_eq!(s.seq(), 3);
    }

    /// A new file in a subdirectory moves only that directory's mtime; a
    /// nested clone's `.git` removed moves only its region root's.
    #[test]
    fn poll_relists_a_changed_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        write(&dir, "pkg/c.py", "def c():\n    return 3\n");
        write(&dir, "third/lib/x.py", "def x():\n    return 4\n");
        write(&dir, "third/lib/.git/HEAD", "ref: refs/heads/main\n");
        settle(&dir);
        let mut s = open(&dir);
        let region = |s: &WatchSession| {
            s.repos[0]
                .plan
                .region_dirs
                .iter()
                .any(|d| d.rel == "third/lib")
        };
        assert!(region(&s), "{:?}", s.repos[0].plan.region_dirs);
        assert!(s.changed_paths().is_empty());

        write(&dir, "pkg/d.py", "def d():\n    return c()\n");
        assert_eq!(rels(&s, &s.changed_paths()), ["pkg"]);
        let u = s.poll().unwrap().expect("a new file");
        assert_eq!(u.files.added, ["pkg/d.py"]);
        assert_clean(&s, &dir, tmp.path(), "subdir_add");
        assert!(s.poll().unwrap().is_none());

        std::fs::remove_dir_all(dir.join("third/lib/.git")).unwrap();
        assert_eq!(rels(&s, &s.changed_paths()), ["third/lib"]);
        let u = s.poll().unwrap().expect("the region un-collapses");
        assert_eq!(u.files.added, ["third/lib/x.py"]);
        assert!(!region(&s), "{:?}", s.repos[0].plan.region_dirs);
        assert_clean(&s, &dir, tmp.path(), "unregioned");
        assert!(s.changed_paths().is_empty());
    }

    #[test]
    fn racy_same_length_rewrite_is_seen() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        settle(&dir);
        let path = dir.join("a.py");
        // Written "just now": inside the racy window when the session opens.
        set_mtime(&path, SystemTime::now());
        let mut s = open(&dir);
        let recorded = s.repos[0].records[s.repos[0].index["a.py"]].stamp.unwrap();
        let scan = s.repos[0].scan(s.scanned());
        assert!(scan.found.is_empty(), "{:?}", scan.found);
        assert_eq!(scan.racy, 1, "a.py alone sits in the window");
        assert!(s.changed_paths().is_empty());

        // Same length, other bytes, the recorded mtime put back: the stat
        // alone cannot tell.
        let text = std::fs::read_to_string(&path).unwrap();
        let rewrite = text.replace("return 1", "return 7");
        assert_eq!((rewrite.len(), rewrite != text), (text.len(), true));
        std::fs::write(&path, &rewrite).unwrap();
        set_mtime(&path, recorded.mtime.unwrap());
        assert_eq!(FileStamp::of(&path), Some(recorded));
        assert_eq!(rels(&s, &s.changed_paths()), ["a.py"]);
        let u = s.poll().unwrap().expect("the racy rule re-reads it");
        assert_eq!(u.files.modified, ["a.py"]);
        assert_clean(&s, &dir, tmp.path(), "racy");
        assert!(s.changed_paths().is_empty());
    }

    /// The inputs a build reads besides the table: the overlay (`.glia/`
    /// is a hard skip of the walk), a `.gitignore`, a `CMakeLists.txt`
    /// beside a C++ file (the walk admits none) and the git identity files,
    /// each edited in place so no directory mtime moves (`.glia/` exists at
    /// open, so the overlay's creation moves only `.glia`'s own).
    #[test]
    fn poll_watches_inputs_outside_the_walk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        write(&dir, "pkg/c.py", "def c():\n    return 3\n");
        write(
            &dir,
            "src/main.cpp",
            "#include \"util.hpp\"\n\nint main() { return util(); }\n",
        );
        write(&dir, "lib/inc/util.hpp", "int util();\n");
        write(&dir, "src/CMakeLists.txt", "add_executable(app main.cpp)\n");
        std::fs::create_dir_all(dir.join(".glia")).unwrap();
        settle(&dir);
        let mut s = open(&dir);
        for rel in [
            ".gitignore",
            "src/CMakeLists.txt",
            ".git/HEAD",
            ".git/config",
        ] {
            assert!(
                s.repos[0].watched.contains_key(rel),
                "{rel}: {:?}",
                s.repos[0].watched.keys()
            );
        }
        assert!(s.changed_paths().is_empty());

        write(&dir, ".glia/overlay.toml", "version = 1\n");
        assert_eq!(rels(&s, &s.changed_paths()), [".glia/overlay.toml"]);
        let u = s.poll().unwrap().expect("the overlay");
        assert_eq!(u.full_rebuild.as_deref(), Some("overlay"));
        assert_clean(&s, &dir, tmp.path(), "overlay");
        assert!(s.changed_paths().is_empty());

        write(&dir, ".gitignore", "*.log\npkg/\n");
        assert_eq!(rels(&s, &s.changed_paths()), [".gitignore"]);
        let u = s.poll().unwrap().expect("the gitignore");
        assert_eq!(u.files.removed, ["pkg/c.py"]);
        assert_clean(&s, &dir, tmp.path(), "gitignore");
        assert!(s.changed_paths().is_empty());

        write(
            &dir,
            "src/CMakeLists.txt",
            "add_executable(app main.cpp)\ninclude_directories(../lib/inc)\n",
        );
        assert_eq!(rels(&s, &s.changed_paths()), ["src/CMakeLists.txt"]);
        let u = s.poll().unwrap().expect("the include roots");
        assert!(
            u.groups_rebuilt.iter().any(|k| k == "c_cpp"),
            "{:?}",
            u.groups_rebuilt
        );
        assert_eq!(u.full_rebuild, None);
        assert_clean(&s, &dir, tmp.path(), "cmake");
        assert!(s.changed_paths().is_empty());

        // `git init`: the root gains `.git` (its mtime moves) and `.git/HEAD`.
        write(&dir, ".git/HEAD", "ref: refs/heads/main\n");
        assert_eq!(rels(&s, &s.changed_paths()), ["", ".git/HEAD"]);
        let u = s.poll().unwrap().expect("the identity");
        assert_eq!(u.full_rebuild.as_deref(), Some("identity"));
        assert_clean(&s, &dir, tmp.path(), "identity");
        assert!(s.changed_paths().is_empty());
    }

    /// A touch (a new mtime, the same bytes) of a file, a directory or a
    /// watched input reaches `apply`, which finds nothing changed.
    #[test]
    fn touch_without_change_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        settle(&dir);
        let mut s = open(&dir);
        let t = old() + Duration::from_secs(60);
        set_mtime(&dir.join("a.py"), t);
        assert_eq!(rels(&s, &s.changed_paths()), ["a.py"]);
        assert!(s.poll().unwrap().is_none());
        assert_eq!(s.seq(), 0);
        assert!(s.changed_paths().is_empty(), "the noop took the new stamp");

        set_mtime(&dir, t);
        set_mtime(&dir.join(".gitignore"), t);
        assert_eq!(rels(&s, &s.changed_paths()), ["", ".gitignore"]);
        assert!(s.poll().unwrap().is_none());
        assert_eq!(s.seq(), 0);
        assert!(s.changed_paths().is_empty());
    }

    /// A session moves between threads (a pyo3 class, a watcher thread).
    /// GA.12: a 1 MB budget (below any process) evicts every cached group
    /// and then the memos, at once and after every update; each update
    /// rebuilds every plan group and still equals a clean build byte for
    /// byte. Lifting the budget caches again, and the next edit reuses.
    #[test]
    fn budget_evicts_and_stays_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mixed_repo(tmp.path());
        let mut s = open(&dir);
        let m = s.memory();
        assert_eq!(m.budget_mb, None);
        assert!(m.groups > 1, "{m:?}");
        assert_eq!(m.cached_groups, m.groups, "no budget keeps every group");
        assert!(m.memo_paths > 0, "the build memoised something: {m:?}");
        assert!(m.cache_entries > 0, "{m:?}");
        assert_eq!((m.evictions, m.memo_clears), (0, 0));
        s.set_memory_budget(MemoryBudget::mb(1));
        assert_eq!(s.memory_budget(), MemoryBudget::mb(1));
        let readable = memory::rss_kb().is_some();
        let m = s.memory();
        assert_eq!(m.budget_mb, Some(1));
        if readable {
            assert_eq!((m.cached_groups, m.memo_paths), (0, 0), "{m:?}");
            assert_eq!(m.evictions, m.groups as u64, "{m:?}");
            assert_eq!(m.memo_clears, 1, "{m:?}");
        }
        let entries = m.cache_entries;
        assert!(entries > 0, "the parse cache entries are never evicted");
        for edit in edits() {
            let u = run_edit(&mut s, &dir, &edit);
            let m = s.memory();
            if readable {
                assert_eq!(
                    u.groups_rebuilt.len(),
                    m.groups,
                    "{}: nothing cached, every group rebuilds",
                    edit.0
                );
                assert_eq!((m.cached_groups, m.memo_paths), (0, 0), "{}: {m:?}", edit.0);
            }
            assert!(m.cache_entries > 0, "{}", edit.0);
            assert_clean(&s, &dir, tmp.path(), &format!("budget_{}", edit.0));
        }
        s.set_memory_budget(MemoryBudget::none());
        let python = edits().remove(0);
        let mut again = python.clone();
        again.1 = vec![(
            "a.py",
            "def foo():\n    return bar() + 2\n\ndef bar():\n    return 1\n".into(),
        )];
        run_edit(&mut s, &dir, &again);
        let m = s.memory();
        assert_eq!(m.cached_groups, m.groups, "no budget caches again: {m:?}");
        let u = run_edit(&mut s, &dir, &python);
        assert_eq!(u.groups_rebuilt, ["python"], "the cache serves again");
        assert_clean(&s, &dir, tmp.path(), "budget_lifted");
    }

    /// GA.12: the eviction order: the lowest `last_used` first, ties by plan
    /// order; every group an update built or reused carries its `seq`.
    #[test]
    fn budget_evicts_least_recently_used_first() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mixed_repo(tmp.path());
        let mut s = open(&dir);
        assert!(s.repos[0].groups.iter().all(|g| g.last_used == 0));
        assert_eq!(s.oldest_group(), Some((0, 0)), "ties go by plan order");
        run_edit(&mut s, &dir, &edits()[0]);
        assert!(
            s.repos[0].groups.iter().all(|g| g.last_used == 1),
            "built or reused at seq 1"
        );
        let last = s.repos[0].groups.len() - 1;
        if let Some(g) = s.repos[0].groups.get_mut(last) {
            g.last_used = 0;
        }
        assert_eq!(s.oldest_group(), Some((0, last)));
        s.repos[0].groups.clear();
        assert_eq!(s.oldest_group(), None);
    }

    #[test]
    fn rss_marker_text() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mixed_repo(tmp.path());
        let s = open(&dir);
        let repo = &s.repos[0];
        let (k, g) = (repo.groups.len(), repo.group_count);
        assert_eq!(
            rss_marker(
                3,
                Some(700 * 1024 + 5),
                MemoryBudget::mb(512),
                repo,
                2,
                true
            ),
            format!(
                "[watch] rss seq=3 rss_mb=700 budget_mb=512 cached_groups={k}/{g} evicted=2 \
                 memo_cleared=1 repo={}",
                repo.label
            )
        );
        assert_eq!(
            rss_marker(0, None, MemoryBudget::none(), repo, 0, false),
            format!(
                "[watch] rss seq=0 rss_mb=? budget_mb=none cached_groups={k}/{g} evicted=0 \
                 memo_cleared=0 repo={}",
                repo.label
            )
        );
    }

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

    /// GA.9: two repos under one directory name (`one/app`, `two/app`), so
    /// their dir identities share a key and are disambiguated by path: a
    /// proto-owning server (python, go, `api/greet.proto`) and a Go gRPC
    /// client of its service with no proto of its own, plus python.
    fn two_repos(tmp: &Path) -> (PathBuf, PathBuf) {
        let server = tmp.join("one").join("app");
        write(
            &server,
            "a.py",
            "def foo():\n    return bar()\n\ndef bar():\n    return 1\n",
        );
        write(&server, "go.mod", "module example.com/server\n");
        write(
            &server,
            "m.go",
            "package m\n\nfunc F() int { return G() }\n\nfunc G() int { return 1 }\n",
        );
        write(&server, "api/greet.proto", &proto(""));
        let client = tmp.join("two").join("app");
        write(&client, "go.mod", "module example.com/client\n");
        write(
            &client,
            "client.go",
            "package main\n\nimport (\n\t\"context\"\n\n\tpb \"example.com/gen/greet\"\n\t\"google.golang.org/grpc\"\n)\n\nfunc Call(conn *grpc.ClientConn) {\n\tc := pb.NewGreeterClient(conn)\n\tc.Hello(context.Background(), &pb.Req{})\n}\n",
        );
        write(&client, "b.py", "def baz():\n    return 2\n");
        (server, client)
    }

    fn roots_of(dirs: &[&Path]) -> Vec<String> {
        dirs.iter()
            .map(|d| d.to_str().unwrap().to_string())
            .collect()
    }

    /// GA.9: the session's layout equals a clean `generate_many` of the
    /// trees as they are (persisted at the same depth, as [`assert_clean`]),
    /// and so do its totals, parse errors, labels and roots.
    fn assert_clean_many(s: &WatchSession, roots: &[String], tmp: &Path, tag: &str) {
        let out = tmp.join("out").join(tag);
        s.persist(&out.join("watch")).unwrap();
        let clean =
            crate::build::generate_many_opts(roots, false, &BuildOptions::default()).unwrap();
        crate::persist::persist_result(&clean, &out.join("clean"), "test").unwrap();
        let (w, c) = (
            layout_bytes(&out.join("watch")),
            layout_bytes(&out.join("clean")),
        );
        assert!(!w.is_empty());
        assert_eq!(w.len(), c.len(), "{tag}: same files");
        for ((wn, wb), (cn, cb)) in w.iter().zip(&c) {
            assert_eq!(wn, cn, "{tag}");
            assert!(wb == cb, "{tag}: {wn} differs from a clean generate_many");
        }
        let r = s.result();
        assert_eq!(
            (r.total_nodes, r.total_edges, &r.parse_errors),
            (clean.total_nodes, clean.total_edges, &clean.parse_errors),
            "{tag}"
        );
        assert_eq!(r.repo_labels, clean.repo_labels, "{tag}");
        assert_eq!(r.repo_roots, clean.repo_roots, "{tag}");
    }

    #[test]
    fn open_many_equals_generate_many() {
        let tmp = tempfile::tempdir().unwrap();
        let (server, client) = two_repos(tmp.path());
        let roots = roots_of(&[&server, &client]);
        let s = WatchSession::open_many(&roots, &BuildOptions::default()).unwrap();
        assert_eq!(
            s.roots(),
            roots.iter().map(String::as_str).collect::<Vec<_>>()
        );
        assert_eq!(s.repos.len(), 2);
        assert_ne!(
            s.repos[0].setup.repo, s.repos[1].setup.repo,
            "a shared dir key is disambiguated"
        );
        assert_eq!(s.repos[0].ident_raw.key, s.repos[1].ident_raw.key);
        assert_eq!(s.result().repo_labels.len(), 2);
        assert_eq!(
            s.repos.iter().map(|r| r.graph_count).sum::<usize>(),
            s.result().merged.graphs.len()
        );
        assert_clean_many(&s, &roots, tmp.path(), "open_many");
        // One root is `open`: the same build as a one-root session.
        let one = WatchSession::open_many(&roots[..1], &BuildOptions::default()).unwrap();
        assert_clean(&one, &server, tmp.path(), "open_many_one");
        // A root that is not a directory refuses the whole session.
        let missing = tmp.path().join("nope").to_str().unwrap().to_string();
        let err = WatchSession::open_many(
            &[roots[0].clone(), missing.clone()],
            &BuildOptions::default(),
        )
        .err()
        .unwrap();
        assert_eq!(err, format!("not a directory: {missing}"));
        assert!(WatchSession::open_many(&[], &BuildOptions::default()).is_err());
    }

    /// A path in the second repo rebuilds only that repo's group, named
    /// `<label>:<key>`; the scoped delta (offset into the joint graph list)
    /// equals the full one; a relative path goes to the first root, a path
    /// under no root is outside; the stat poll sees an edit in either repo;
    /// a proto service change in the first repo rebuilds every group of
    /// both. Every update equals a clean `generate_many`.
    #[test]
    fn multi_repo_edit_rebuilds_only_its_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let (server, client) = two_repos(tmp.path());
        settle(&server);
        settle(&client);
        let roots = roots_of(&[&server, &client]);
        let mut s = WatchSession::open_many(&roots, &BuildOptions::default()).unwrap();
        assert!(s.changed_paths().is_empty(), "settled trees are quiet");
        let server_keys: Vec<String> = s.repos[0]
            .groups
            .iter()
            .map(|c| format!("{}:{}", roots[0], c.key))
            .collect();
        let client_keys: Vec<String> = s.repos[1]
            .groups
            .iter()
            .map(|c| format!("{}:{}", roots[1], c.key))
            .collect();
        assert!(
            client_keys.contains(&format!("{}:python", roots[1])),
            "{client_keys:?}"
        );

        let prev = s.result().merged.clone();
        write(
            &client,
            "b.py",
            "def baz():\n    return qux()\n\ndef qux():\n    return 3\n",
        );
        let u = s.apply(&[client.join("b.py")]).unwrap();
        assert_eq!(u.seq, 1);
        assert_eq!(u.groups_rebuilt, [format!("{}:python", roots[1])]);
        assert_eq!(u.files.modified, [format!("{}:b.py", roots[1])]);
        assert_eq!(u.full_rebuild, None);
        let full = located_delta_counted(&prev, &s.result().merged, &[]);
        assert!(
            u.nodes.iter().any(|n| n.qname.ends_with("qux")),
            "{:?}",
            u.nodes
        );
        assert_eq!(json(&u.nodes), json(&full.nodes));
        assert_eq!(json(&u.edges), json(&full.edges));
        assert_eq!(json(&u.counts), json(&full.counts));
        assert_clean_many(&s, &roots, tmp.path(), "client_body");

        // A relative path is the first root's; one under no root is outside.
        write(
            &server,
            "a.py",
            "def foo():\n    return bar() + 1\n\ndef bar():\n    return 1\n",
        );
        let elsewhere = tmp.path().join("elsewhere.py");
        let u = s
            .apply(&[PathBuf::from("a.py"), elsewhere.clone()])
            .unwrap();
        assert_eq!(u.groups_rebuilt, [format!("{}:python", roots[0])]);
        assert_eq!(u.files.modified, [format!("{}:a.py", roots[0])]);
        assert_clean_many(&s, &roots, tmp.path(), "server_body");
        let u = s.apply(&[elsewhere]).unwrap();
        assert_eq!(u.seq, 2, "a path under no root is a noop");

        // The stat poll scans both repos.
        assert!(s.poll().unwrap().is_none(), "quiet after the updates");
        write(&client, "b.py", "def baz():\n    return 22\n");
        let u = s.poll().unwrap().expect("the client edit is seen");
        assert_eq!(u.groups_rebuilt, [format!("{}:python", roots[1])]);
        assert_clean_many(&s, &roots, tmp.path(), "client_poll");

        // The proto service set is the union: a service change in the
        // server rebuilds the client's groups too.
        write(
            &server,
            "api/greet.proto",
            &proto("  rpc Bye (Req) returns (Rep);\n"),
        );
        let u = s.apply(&[server.join("api/greet.proto")]).unwrap();
        assert_eq!(u.full_rebuild.as_deref(), Some("proto_services"));
        let mut all = server_keys.clone();
        all.extend(client_keys);
        assert_eq!(u.groups_rebuilt, all);
        assert_clean_many(&s, &roots, tmp.path(), "proto_service");
    }

    /// Nested roots (either order) and one directory given twice are
    /// refused, naming both roots.
    #[test]
    fn open_many_refuses_nested_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let outer = repo(tmp.path());
        let inner = outer.join("sub");
        write(&inner, "c.py", "def c():\n    return 1\n");
        let roots = roots_of(&[&outer, &inner]);
        let opts = BuildOptions::default();
        let want = format!("nested watch roots: {} is inside {}", roots[1], roots[0]);
        assert_eq!(WatchSession::open_many(&roots, &opts).err().unwrap(), want);
        let reversed = [roots[1].clone(), roots[0].clone()];
        assert_eq!(
            WatchSession::open_many(&reversed, &opts).err().unwrap(),
            want
        );
        let twice = [roots[0].clone(), format!("{}/.", roots[0])];
        let err = WatchSession::open_many(&twice, &opts).err().unwrap();
        assert!(err.starts_with("duplicate watch roots: "), "{err}");
    }

    /// `save_cache` writes every repo's parse cache to its own root.
    #[test]
    fn save_cache_writes_every_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let (server, client) = two_repos(tmp.path());
        let roots = roots_of(&[&server, &client]);
        let s = WatchSession::open_many(&roots, &BuildOptions::default()).unwrap();
        s.save_cache().unwrap();
        for root in [&server, &client] {
            assert!(
                root.join(".glia/graph/parse_cache.bin").is_file(),
                "{}",
                root.display()
            );
        }
        // A reopened session reads them back: every parse is a cache hit.
        let mut s = WatchSession::open_many(&roots, &BuildOptions::default()).unwrap();
        assert!(s.repos.iter().all(|r| r.cache.stats.reparsed == 0));
        write(&server, "a.py", "def foo():\n    return 7\n");
        s.apply(&[server.join("a.py")]).unwrap();
        assert_clean_many(&s, &roots, tmp.path(), "reopened");
    }

    /// Release timing harness for DC.7c's acceptance (a quiet poll of the
    /// glia copy), run by hand on a `git archive` copy:
    /// `GLIA_WATCH_POLL_BENCH=<repo> cargo test --release -p glia-engine --lib watch::tests::bench_quiet_poll -- --ignored --nocapture`.
    /// Opens a session, then times ten scans of the untouched tree, printing
    /// what each checked; asserts each was quiet and the poll after them too.
    /// Then touches the root directory (its mtime moved back one second,
    /// out of the racy window) and times the poll, a re-plan that finds
    /// nothing changed: what a directory touch costs.
    #[test]
    #[ignore = "release timing harness: set GLIA_WATCH_POLL_BENCH=<repo>; touches its root"]
    fn bench_quiet_poll() {
        let Ok(repo) = std::env::var("GLIA_WATCH_POLL_BENCH") else {
            return;
        };
        let mut s = WatchSession::open(&repo, &BuildOptions::default()).unwrap();
        for i in 1..=10 {
            let started = Instant::now();
            let found = s.changed_paths();
            let took = started.elapsed();
            let scan = s.repos[0].scan(s.scanned());
            eprintln!(
                "[watch-bench] quiet poll {i} files={} dirs={} watched={} racy={} changed={} took={}",
                scan.files,
                scan.dirs,
                scan.watched,
                scan.racy,
                found.len(),
                timing::ms(took)
            );
            assert!(found.is_empty(), "{found:?}");
        }
        assert!(s.poll().unwrap().is_none());
        let root = Path::new(&repo);
        let mtime = std::fs::metadata(root).unwrap().modified().unwrap();
        set_mtime(root, mtime - Duration::from_secs(1));
        let started = Instant::now();
        assert!(s.poll().unwrap().is_none());
        eprintln!(
            "[watch-bench] dir touch poll (noop re-plan) took={}",
            timing::ms(started.elapsed())
        );
        assert!(s.changed_paths().is_empty());
    }

    /// GA.17: where an overlay section reaches a build. The whole group key
    /// leaves the overlay out ([`group_fingerprints`]), which holds only while
    /// no section is read by a language group's builder: a section reaches a
    /// group only through the walk (the walked files and project roots) or
    /// the post-cache grafts (`parse_and_graft`), both of which the post-graft
    /// parses in the key already hold, or not at all (the pass tail, or query
    /// time).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Reach {
        /// `walk::plan_walk`: which files are walked, the declared project
        /// roots; so the parses.
        Walk,
        /// The post-cache grafts (`parse_and_graft`: the constant table, the
        /// `[[wrapper]]` scan): the post-graft parses.
        Graft,
        /// After every language group is built: the resolvers, the external
        /// edge and cell stages, the post-passes.
        Pass,
        /// Read only by an answer; no build reads it.
        Query,
        /// Read by a language group's builder (`GroupCtx`, `hidden_fp`):
        /// THE RULE: such a section must put `overlay_fp` back into the whole
        /// key in [`group_fingerprints`], or a reused group goes stale.
        Builder,
    }

    /// GA.17: every overlay section (`glia_config::SECTIONS` order) and
    /// the stages that read it (`engine/src/` readers, checked by grep when
    /// this landed): `[walk] skip` the walk, `[walk] tests` the wrapper scan
    /// and the TESTS / provenance passes; `[[project]]` the walk; the
    /// entrypoints and the declared stanzas the external cell stage (and
    /// gaps); `[constants]` the constant table the grafts fold through;
    /// `[[route_prefix]]` the http pass's mounts; `[[wrapper]]` the graft
    /// scan (and gaps); `[[edge]]` the external edge stage; `[ids]`
    /// `intent::ids` only.
    const OVERLAY_REACH: [(&str, &[Reach]); 14] = [
        ("walk", &[Reach::Walk, Reach::Graft, Reach::Pass]),
        ("project", &[Reach::Walk]),
        ("entrypoints", &[Reach::Pass, Reach::Query]),
        ("constants", &[Reach::Graft]),
        ("route_prefix", &[Reach::Pass]),
        ("wrapper", &[Reach::Graft, Reach::Query]),
        ("edge", &[Reach::Pass, Reach::Query]),
        ("constraint", &[Reach::Pass, Reach::Query]),
        ("decision", &[Reach::Pass, Reach::Query]),
        ("note", &[Reach::Pass, Reach::Query]),
        ("component", &[Reach::Pass, Reach::Query]),
        ("layer", &[Reach::Pass, Reach::Query]),
        ("ids", &[Reach::Query]),
        ("convention", &[Reach::Pass, Reach::Query]),
    ];

    /// GA.17's guard: every overlay section is classified, and none reaches
    /// a language group's builder. A new `GliaConfig` field fails to compile
    /// here (the exhaustive destructuring) until it joins [`OVERLAY_REACH`];
    /// a section classified `Builder` fails the assertion, naming the rule.
    #[test]
    fn every_overlay_section_reaches_groups_through_parses() {
        use glia_code_domain::glia_config::{self, GliaConfig};
        let GliaConfig {
            // The version picks which sections load: it reaches a group only
            // as they do.
            version: _,
            walk: _,
            project: _,
            entrypoints: _,
            constants: _,
            route_prefix: _,
            wrapper: _,
            edge: _,
            constraint: _,
            decision: _,
            note: _,
            component: _,
            layer: _,
            ids: _,
            convention: _,
        } = GliaConfig::default();
        let names: Vec<&str> = OVERLAY_REACH.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            glia_config::SECTIONS,
            "every overlay section is classified, in SECTIONS order"
        );
        for (section, reach) in OVERLAY_REACH {
            assert!(!reach.is_empty(), "[{section}] reaches nothing?");
            assert!(
                !reach.contains(&Reach::Builder),
                "[{section}] is read by a language group's builder: the whole \
                 group key must hold overlay_fp again (group_fingerprints, GA.17)"
            );
        }
    }
}
