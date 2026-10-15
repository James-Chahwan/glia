//! Build orchestration: the `generate_*` entry points and the per-repo loop
//! that feeds them. The per-file routing lives in [`crate::route`]; the rest
//! of a build is split by seam:
//!
//! - [`assemble`] — `build_graphs_for_repo`: parse, const table, post-cache
//!   grafts ([`assemble::parse_and_graft`], under one panic-hook guard), the
//!   per-group build, per-repo markers ([`assemble::repo_markers`]).
//! - [`grafts`] — the post-cache graft sequence (endpoint fold, HTTP owner
//!   segment, queue const fold, RPC needles and their markers, anchor census,
//!   IMPORTS-cell filter).
//! - [`rpc_needles`] — the build-wide proto service set and the gRPC
//!   client / server needle passes.
//! - [`lang_build`] — the per-group build as three stages (DC.2):
//!   [`lang_build::plan_groups`] (the deterministic group order),
//!   [`lang_build::build_groups`] (the `build_*` dispatch on the pool, with a
//!   per-group reuse slot) and [`lang_build::fold_groups`].
//! - [`timing`] — the per-phase timers' `[timing]` stderr lines (CA.9): one
//!   per built repo, one per build, one per layout write. Stderr only.
//!
//! After a repo's language graphs come its region, project and docs graphs
//! ([`aux_graphs`]). The build tail ([`finish_build`]: cross-graph resolvers,
//! the external edges, post-passes, evidence fill, the determinism sort) is
//! the code domain's pass registry, [`crate::profile::CODE_PASSES`], run by
//! one [`crate::profile::run_code_passes_with`] call (LD.13) given the build's
//! [`CodeBuildCtx`]: each repo's `.glia` inputs, loaded once per repo right
//! after its walk, and the [`BuildOptions`]. After it, the external node-cell
//! stage ([`crate::external::apply_external_cells`], LF.1a) applies the same
//! inputs.
//!
//! DC.2: those stages are crate-visible so a caller that holds its own file
//! set and per-group builds (`crate::watch`, DC.7a) can run exactly the calls
//! `generate_one_inner` makes. `assemble`, `c_includes`, `lang_build` and
//! `rpc_needles` are `pub(crate)` modules: their `pub(crate)` items are
//! reached by module path (`crate::build::lang_build::plan_groups`); their
//! `pub(super)` items stay inside `build`.
//!
//! [`BuildOptions`] is how a caller switches a build (LF.2b): `overlay`,
//! whether the `.glia/overlay.toml` overlay sections apply, and (CE.3b)
//! `overlay_text`, the primary repo's overlay given as text in place of its
//! file. It is a build option, not an env var, because pyo3 `generate()` may
//! run on several Python threads at once.

pub(crate) mod assemble;
pub(crate) mod c_includes;
mod grafts;
pub(crate) mod lang_build;
pub(crate) mod rpc_needles;
pub(crate) mod timing;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glia_code_domain::project_roots::ProjectRoot;
use glia_code_domain::walk_gating::{RepoIdentity, repo_identity};
use glia_core::RepoId;
use glia_graph::{MergedGraph, RepoGraph};

use crate::cache::ParseCache;
use crate::docs::{DocSource, FileDocSource, SnapshotDocSource, build_docs_graph};
use crate::extract::GoModules;
use crate::external::{RepoInputs, apply_external_cells, repo_inputs};
use crate::profile::{CodeBuildCtx, run_code_passes_with};
use crate::walk::{
    RegionAnchor, WalkResult, build_project_graph, build_region_graph, walk_source_files,
};

use assemble::{RepoBuildCtx, build_graphs_for_repo};
use c_includes::IncludeRoots;
use lang_build::TsAliasSet;
use rpc_needles::RpcContext;
use timing::BuildTimes;

/// One build's output. Outside this crate it comes from [`generate_one`] /
/// [`generate_many`] and their variants, never from a struct literal, so a new
/// field is not a break (LD.9, `engine/tests/api_stability.rs`):
///
/// ```compile_fail
/// let _ = glia_engine::GenerateResult {
///     merged: Default::default(),
///     total_nodes: 0,
///     total_edges: 0,
///     parse_errors: vec![],
///     repo_labels: Default::default(),
///     repo_roots: Default::default(),
/// };
/// ```
#[non_exhaustive]
#[derive(Clone)]
pub struct GenerateResult {
    pub merged: MergedGraph,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub parse_errors: Vec<String>,
    /// `RepoId.0` → human repo label (A9.2). The RepoId is an xxhash of the
    /// repo identity key (git remote / git dir / dir name, LB.1), so the human
    /// label — derived from the path the caller gave — cannot be recovered
    /// from the graph: it is captured where the path and the id still coexist,
    /// and deliberately NOT on `MergedGraph`, which would change the shard
    /// bytes. It is persisted in the layout's `manifest.json` instead (LC.7),
    /// so [`crate::persist::load_layout`] returns the same map a fresh build
    /// did; a layout written without metadata loads with none.
    pub repo_labels: std::collections::BTreeMap<u64, String>,
    /// `RepoId.0` → the repo root, as the caller gave it on a fresh build
    /// (LC.7). [`crate::persist::layout_meta`] records it relative to the
    /// layout dir; [`crate::persist::load_layout`] returns it resolved against
    /// the dir it loaded from, canonicalised when the path still exists.
    pub repo_roots: std::collections::BTreeMap<u64, String>,
}

/// How one build runs (LF.2b). `#[non_exhaustive]`: outside this crate it is
/// made by [`BuildOptions::default`] and its `with_*` setters, so a new option
/// is not a break.
///
/// - `overlay` (default `true`): apply the overlay (inference) sections of each
///   repo's `.glia/overlay.toml` — today its `[[edge]]` stanzas. `false` is the
///   extraction-only build (`glia --no-overlay`, pyo3 `overlay=False`): the
///   only consistent "without the overlay" view, because overlay sections
///   that re-key nodes cannot be undone by a query-time filter. User-config
///   (`[walk]`, `[[project]]`, `[entrypoints]`) and declared-knowledge
///   sections are not affected. A graph built without the overlay must never
///   be written to a repo's default layout dir, which holds the
///   overlay-applied graph: the CLI and pyo3 refuse it.
/// - `overlay_text` (default `None`, CE.3b): the PRIMARY repo's overlay as
///   text, parsed in place of its `.glia/overlay.toml` (the file is not read
///   for it; a missing file is no matter). The primary repo is
///   [`generate_one`]'s repo, or [`generate_many`]'s first path: a candidate
///   targets one repo's file, and every other repo reads its own. Every build
///   reader of the overlay (constant pins, wrappers, edges, route prefixes,
///   declared knowledge, entrypoints) sees the text; only the walk still reads
///   `[walk]` / `[[project]]` from the file on disk, sections an overlay-loop
///   candidate never carries. Parse errors print the same `[overlay] error:`
///   lines as the file's. `overlay` still switches the overlay sections of the
///   text. Only [`crate::overlay_loop`] sets it (no CLI flag, no pyo3
///   parameter), to build the tree with the overlay that WOULD be written. A
///   graph built with `overlay_text` must never be written to a repo's default
///   layout dir, which holds the graph of the file as it is (the rule
///   `--no-overlay` builds follow): the overlay loop never persists one.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildOptions {
    pub overlay: bool,
    pub overlay_text: Option<String>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self { overlay: true, overlay_text: None }
    }
}

impl BuildOptions {
    /// `self` with the overlay switched on or off.
    pub fn with_overlay(mut self, on: bool) -> Self {
        self.overlay = on;
        self
    }

    /// `self` building the primary repo with `text` as its overlay, in place
    /// of its `.glia/overlay.toml` (see [`BuildOptions`]).
    pub fn with_overlay_text(mut self, text: String) -> Self {
        self.overlay_text = Some(text);
        self
    }
}

/// Generate a `MergedGraph` from a single repo path. The repo gets one RepoId,
/// an xxhash of its path-independent identity key ([`repo_identity`]):
/// `git:<normalised origin url>[/<path within the checkout>]` for a git
/// checkout with a remote, `gitdir:<main checkout dir name>[/<rel>]` for one
/// without, `dir:<basename>` outside git. So every NodeId survives a re-spelled
/// path, a second clone, a linked worktree and a moved checkout. Cross-graph
/// resolvers run but only emit edges within this single repo (rare in practice).
pub fn generate_one(repo_path: &str) -> Result<GenerateResult, String> {
    generate_one_inner(repo_path, repo_path, None, &BuildOptions::default())
}

/// [`generate_one`] (`incremental = false`) or [`generate_one_incremental`]
/// (`true`), built with `opts`.
pub fn generate_one_opts(
    repo_path: &str,
    incremental: bool,
    opts: &BuildOptions,
) -> Result<GenerateResult, String> {
    if !incremental {
        return generate_one_inner(repo_path, repo_path, None, opts);
    }
    let mut cache = ParseCache::load(repo_path);
    let result = generate_one_inner(repo_path, repo_path, Some(&mut cache), opts)?;
    if let Err(e) = cache.save(repo_path) {
        eprintln!("[incremental] {repo_path}: warning: failed to save parse cache: {e}");
    }
    Ok(result)
}

/// Incremental build using an in-memory [`ParseCache`] (WP-D): unchanged files
/// skip tree-sitter. Hold one `cache` across edits (e.g. neuropil's hot-reload).
/// The result is byte-identical to [`generate_one`] — only the parse step is
/// elided; the graph is rebuilt and resolvers re-run in full.
pub fn generate_one_with_cache(
    repo_path: &str,
    cache: &mut ParseCache,
) -> Result<GenerateResult, String> {
    generate_one_inner(repo_path, repo_path, Some(cache), &BuildOptions::default())
}

/// [`generate_one_with_cache`] built with `opts` (CE.3c): the overlay loop's
/// trial threads one in-memory `cache` through every variant it builds (base,
/// base + candidate, each leave-one-out), so a parse is paid at most once. It
/// neither loads nor saves the sidecar; the caller does, once.
pub(crate) fn generate_one_with_cache_opts(
    repo_path: &str,
    cache: &mut ParseCache,
    opts: &BuildOptions,
) -> Result<GenerateResult, String> {
    generate_one_inner(repo_path, repo_path, Some(cache), opts)
}

/// Disk-backed incremental build: load the parse cache from
/// `<repo>/.glia/graph/parse_cache.bin` (beside the layout), build, then
/// persist it. Cache save failures are logged, not fatal. Backs pyo3
/// `generate(incremental=True)` and `glia build`.
pub fn generate_one_incremental(repo_path: &str) -> Result<GenerateResult, String> {
    generate_one_opts(repo_path, true, &BuildOptions::default())
}

/// A single-repo build of the tree at `root` under the identity of
/// `identity_root` (LE.1b): the files are walked, parsed and read (the
/// `.glia` inputs included) under `root`, while the RepoId, the repo label and
/// root, and the parse cache's context check come from `identity_root`. So a
/// materialised git rev (a temp dir with no `.git`, which would otherwise get
/// a `dir:` key) builds with the working tree's NodeIds and reuses its cached
/// parses. `root == identity_root` is exactly [`generate_one_with_cache`] /
/// [`generate_one`].
pub(crate) fn generate_one_as(
    root: &str,
    identity_root: &str,
    cache: Option<&mut ParseCache>,
) -> Result<GenerateResult, String> {
    generate_one_inner(root, identity_root, cache, &BuildOptions::default())
}

fn generate_one_inner(
    repo_path: &str,
    identity_root: &str,
    mut cache: Option<&mut ParseCache>,
    opts: &BuildOptions,
) -> Result<GenerateResult, String> {
    let started = Instant::now();
    let root = PathBuf::from(repo_path);
    if !root.is_dir() {
        return Err(format!("not a directory: {repo_path}"));
    }
    // Every public entry passes `repo_path` twice; only `generate_one_as`
    // (the LE.1b delta's rev side) walks one tree under another's identity.
    let ident = repo_identity(Path::new(identity_root));
    let repo = RepoId::from_canonical(&ident.key);
    repo_id_marker(&ident, identity_root);
    let repo_labels = crate::arch::repo_label_map(&[(repo.0, identity_root.to_string())]);
    let repo_roots = std::collections::BTreeMap::from([(repo.0, identity_root.to_string())]);
    // Project roots (A8.4) become PROJECT nodes below (A8.5); each root's
    // go.mod joins the repo's Go module map (A8.7 / LA.13).
    let walk_started = Instant::now();
    let (files, regions, md, roots) = walk_source_files(&root);
    let walk = walk_started.elapsed();
    // External inputs (LF.1a): `.glia/overlay.toml` (or the given overlay
    // text, CE.3b) loaded once, before any graph is built.
    let overlay_text = opts.overlay_text.as_deref();
    let inputs = vec![repo_inputs(repo, root.clone(), repo_path.to_string(), overlay_text)];
    let go = go_modules_for(&root, &roots, repo_path);
    // Cached parses are only valid under the exact repo identity + go.mod
    // set they were built with — neither is visible to per-file hashes.
    if let Some(c) = cache.as_deref_mut() {
        c.validate_context(&ident.key, &go.context_key());
    }
    // A6.8: tsconfig `paths`, read per project dir. Consumed after the parse
    // cache (graph build, IMPORTS-cell filter), so the cache needs no key.
    let ts_aliases = TsAliasSet::read(&root, &roots, repo_path);
    // CB.22: the C/C++ include search roots, read the same way (off the walk
    // and the disk) and consumed by the C/C++ graph build.
    let c_includes = IncludeRoots::read(&root, &files, &roots, repo_path);
    let mut rpc = RpcContext::default();
    rpc.add_files(&files);
    let ctx = RepoBuildCtx {
        repo,
        repo_label: repo_path,
        go: &go,
        ts_aliases: &ts_aliases,
        c_includes: &c_includes,
        rpc: &rpc,
        roots: &roots,
        config: inputs.first().and_then(|i| i.config.as_ref()),
        opts,
    };
    let (mut graphs, mut parse_errors, mut times) = build_graphs_for_repo(&files, cache, &ctx);
    times.walk = walk;
    graphs.extend(aux_graphs(&root, repo, &regions, &roots, md));
    eprintln!("{}", times.repo_marker(repo_path));
    let Finished { merged, total_nodes, total_edges, times } =
        finish_build(graphs, inputs, opts, 1, started);
    parse_errors.shrink_to_fit();
    eprintln!("{}", times.marker());
    Ok(GenerateResult {
        merged,
        total_nodes,
        total_edges,
        parse_errors,
        repo_labels,
        repo_roots,
    })
}

/// Generate a `MergedGraph` from N repo paths. Each path becomes its own
/// RepoId so cross-graph resolvers fire across boundaries (the canonical
/// substrate-eval entry). Always a cold build that writes nothing into the
/// repos: `bench/substrate-gap` grades every multi-dir fixture through here.
pub fn generate_many(repo_paths: &[String]) -> Result<GenerateResult, String> {
    generate_many_inner(&self_pairs(repo_paths), false, &BuildOptions::default())
}

/// [`generate_many`] (`incremental = false`) or [`generate_many_incremental`]
/// (`true`), built with `opts`.
pub fn generate_many_opts(
    repo_paths: &[String],
    incremental: bool,
    opts: &BuildOptions,
) -> Result<GenerateResult, String> {
    generate_many_inner(&self_pairs(repo_paths), incremental, opts)
}

/// Disk-backed incremental multi-repo build: each path gets its OWN
/// `<repo>/.glia/graph/parse_cache.bin`, loaded before and saved after that
/// repo's parse (audit 2026-06-10 #14). Byte-identical to [`generate_many`].
/// Opt-in, never the default: the substrate-gap eval grades through
/// `generate_many` and must stay hermetic. Cache save failures are logged, not
/// fatal. Backs pyo3 `generate_many(incremental=True)` and
/// `glia merge --incremental`.
pub fn generate_many_incremental(repo_paths: &[String]) -> Result<GenerateResult, String> {
    generate_many_inner(&self_pairs(repo_paths), true, &BuildOptions::default())
}

/// A multi-repo build where each input's tree at `pair.0` is built under the
/// identity of `pair.1` (CC.8c): the multi-repo twin of [`generate_one_as`].
/// The walk, the file reads and the `.glia` inputs come from `pair.0`; the
/// RepoId, the repo label and root, and the parse cache (loaded from, and
/// context-checked against, `pair.1`) from `pair.1`. So a materialised git
/// rev (a temp dir with no `.git`) builds beside other repos with the working
/// tree's NodeIds. Incremental: every input loads `pair.1`'s parse-cache
/// sidecar, and only an input with `pair.0 == pair.1` saves it back, so the
/// sidecar keeps the working-tree state (LE.1b's rule). All pairs `(p, p)`
/// is exactly [`generate_many_opts`]`(paths, true, opts)`.
pub(crate) fn generate_many_as(
    pairs: &[(String, String)],
    opts: &BuildOptions,
) -> Result<GenerateResult, String> {
    generate_many_inner(pairs, true, opts)
}

/// Every path built under its own identity: `(p, p)`, in argument order.
fn self_pairs(repo_paths: &[String]) -> Vec<(String, String)> {
    repo_paths.iter().map(|p| (p.clone(), p.clone())).collect()
}

/// One walked input of a multi-repo build: the `(root, identity root)` pair
/// as given, its root, its walk, its identity (the identity root's,
/// disambiguated before phase 2 mints any RepoId) and what the walk took
/// (CA.9's `walk=`).
type Walked<'a> = (&'a (String, String), PathBuf, WalkResult, RepoIdentity, Duration);

fn generate_many_inner(
    pairs: &[(String, String)],
    incremental: bool,
    opts: &BuildOptions,
) -> Result<GenerateResult, String> {
    let started = Instant::now();
    let Assembled { merged, parse_errors, label_inputs, repo_roots, inputs } =
        assemble_many_with(pairs, incremental, opts)?;
    // `merged` is the assembly's fresh merge (no cross edge, no pass undo
    // record), so the tail re-merges its graphs unchanged.
    let Finished { merged, total_nodes, total_edges, times } =
        finish_build(merged.graphs, inputs, opts, label_inputs.len(), started);
    eprintln!("{}", times.marker());
    Ok(GenerateResult {
        merged,
        total_nodes,
        total_edges,
        parse_errors,
        repo_labels: crate::arch::repo_label_map(&label_inputs),
        repo_roots,
    })
}

/// A multi-repo build before its passes run: every repo's graphs merged in
/// slot order, no cross edge yet. [`generate_many_inner`] runs
/// [`crate::profile::CODE_PASSES`] over `merged`; a test can run them one at
/// a time instead.
pub(crate) struct Assembled {
    pub(crate) merged: MergedGraph,
    pub(crate) parse_errors: Vec<String>,
    /// `(RepoId.0, identity root as given)` per built repo, in argument
    /// order: the input of [`crate::arch::repo_label_map`].
    pub(crate) label_inputs: Vec<(u64, String)>,
    pub(crate) repo_roots: std::collections::BTreeMap<u64, String>,
    /// Each built repo's external inputs (LF.1a), in argument order.
    pub(crate) inputs: Vec<RepoInputs>,
}

/// Walk, parse and build every repo of a multi-repo build and merge the
/// graphs (phases 1 and 2 of `generate_many`), without running a pass, with
/// the default [`BuildOptions`]: the test entry for running the passes one at
/// a time. Errs when no path produced a graph.
#[cfg(test)]
pub(crate) fn assemble_many(repo_paths: &[String], incremental: bool) -> Result<Assembled, String> {
    assemble_many_with(&self_pairs(repo_paths), incremental, &BuildOptions::default())
}

/// `assemble_many` over `(root, identity root)` pairs, built with `opts`: the
/// overlay stages that run before a graph is built (LF.2d's constant pins)
/// read `opts.overlay`, and the first pair's inputs parse `opts.overlay_text`
/// when it is set (CE.3b). Each pair's walk, `.glia` inputs, file reads and
/// markers use its root; its identity (and so its RepoId and the path
/// disambiguation of a shared key), label, repo root and parse cache use its
/// identity root. With `incremental`, the cache is loaded from the identity
/// root and saved back only when the two are the same path (CC.8c,
/// [`generate_many_as`]).
pub(crate) fn assemble_many_with(
    pairs: &[(String, String)],
    incremental: bool,
    opts: &BuildOptions,
) -> Result<Assembled, String> {
    let mut all_graphs = Vec::new();
    let mut all_errors = Vec::new();
    let mut label_inputs: Vec<(u64, String)> = Vec::new();
    let mut repo_roots: std::collections::BTreeMap<u64, String> = std::collections::BTreeMap::new();
    let mut inputs: Vec<RepoInputs> = Vec::new();

    // Phase 1 — walk every repo before building any (A5.2), so the proto
    // service set is the UNION across the build: in a client/server split the
    // client repo ships no `.proto` of its own. The cost is holding every
    // repo's sources at once, which the 2-5 repo `--with` merges absorb.
    // A missing path keeps its slot so errors stay in argument order.
    // Each input's identity (LB.1) is computed here too, so inputs that share
    // a key are disambiguated against each other BEFORE any RepoId is minted.
    // LG.1b: the repos are walked concurrently on the engine pool (each walk
    // reads its files on the same pool); the results come back in argument
    // order and the proto service set is folded from them in that order. Walk
    // markers of different repos may interleave; each repo's lines keep their
    // order. CA.9: each walk is timed inside its own closure, so a repo's
    // `walk=` is its walk alone even while the walks overlap.
    let (walks, _threads) = crate::parallel::par_map_ordered(pairs, |(path, identity_root)| {
        let root = PathBuf::from(path);
        if !root.is_dir() {
            return Err(format!("not a directory: {path}"));
        }
        let started = Instant::now();
        let walk = walk_source_files(&root);
        let took = started.elapsed();
        let ident = repo_identity(Path::new(identity_root));
        Ok((root, walk, ident, took))
    });
    let mut walked: Vec<Result<Walked<'_>, String>> = pairs
        .iter()
        .zip(walks)
        .map(|(pair, w)| w.map(|(root, walk, ident, took)| (pair, root, walk, ident, took)))
        .collect();
    let mut rpc = RpcContext::default();
    for w in walked.iter().flatten() {
        rpc.add_files(&w.2.0);
    }
    let mut idents: Vec<RepoIdentity> = walked.iter().flatten().map(|w| w.3.clone()).collect();
    // A shared key is disambiguated by the IDENTITY root's path, so a rev
    // built in a temp dir keeps its working tree's disambiguated key.
    let abs_paths: Vec<String> =
        walked.iter().flatten().map(|w| canonical_display(Path::new(&w.0.1))).collect();
    for line in disambiguate(&mut idents, &abs_paths) {
        eprintln!("{line}");
    }
    for (w, ident) in walked.iter_mut().flatten().zip(idents) {
        w.3 = ident;
    }

    // Phase 2 — build each repo against the union. `walked` keeps a slot per
    // pair, so index 0 is the first path: the only repo `overlay_text` is for.
    for (slot, entry) in walked.into_iter().enumerate() {
        let ((path, identity_root), root, (files, regions, md, roots), ident, walk) = match entry {
            Ok(w) => w,
            Err(e) => {
                all_errors.push(e);
                continue;
            }
        };
        // One key feeds both the RepoId and the cache's context check: every
        // cached FileParse has this RepoId baked into its NodeIds, so a sidecar
        // written under another identity must be discarded (#2). A re-spelled
        // or moved path keeps the key, so its sidecar is reused.
        let repo = RepoId::from_canonical(&ident.key);
        repo_id_marker(&ident, identity_root);
        let overlay_text = if slot == 0 { opts.overlay_text.as_deref() } else { None };
        let input = repo_inputs(repo, root.clone(), path.clone(), overlay_text);
        label_inputs.push((repo.0, identity_root.clone()));
        // First path wins, like `repo_label_map` (inputs sharing a key are
        // disambiguated above, so a repeat is the same repo given twice).
        repo_roots.entry(repo.0).or_insert_with(|| identity_root.clone());
        let go = go_modules_for(&root, &roots, path);
        let mut cache = incremental.then(|| ParseCache::load(identity_root));
        if let Some(c) = cache.as_mut() {
            c.validate_context(&ident.key, &go.context_key());
        }
        let ts_aliases = TsAliasSet::read(&root, &roots, path);
        let c_includes = IncludeRoots::read(&root, &files, &roots, path);
        let ctx = RepoBuildCtx {
            repo,
            repo_label: path,
            go: &go,
            ts_aliases: &ts_aliases,
            c_includes: &c_includes,
            rpc: &rpc,
            roots: &roots,
            config: input.config.as_ref(),
            opts,
        };
        let (graphs, parse_errors, mut times) = build_graphs_for_repo(&files, cache.as_mut(), &ctx);
        times.walk = walk;
        inputs.push(input);
        // A tree built under another's identity (a materialised rev) never
        // overwrites that identity's sidecar: it keeps the working tree's state.
        if path == identity_root
            && let Some(c) = cache.as_ref()
            && let Err(e) = c.save(identity_root)
        {
            eprintln!("[incremental] {identity_root}: warning: failed to save parse cache: {e}");
        }
        all_graphs.extend(graphs);
        all_graphs.extend(aux_graphs(&root, repo, &regions, &roots, md));
        all_errors.extend(parse_errors);
        eprintln!("{}", times.repo_marker(path));
    }
    if all_graphs.is_empty() {
        return Err(format!(
            "no graphs produced from {} paths; first error: {}",
            pairs.len(),
            all_errors.first().cloned().unwrap_or_default(),
        ));
    }
    Ok(Assembled {
        merged: MergedGraph::new(all_graphs),
        parse_errors: all_errors,
        label_inputs,
        repo_roots,
        inputs,
    })
}

/// One repo's non-language graphs, in slot order: its region graph (when the
/// walk collapsed a build / vendor region), its project graph (when it found
/// a project root, A8.5), then its docs graph (the walked markdown `md` and
/// the `glia docs sync` snapshot under `root`, when either holds a doc). They
/// follow the repo's language graphs, and the slot order fixes the shard
/// index, so every build appends them through here (`generate_one_inner`,
/// `assemble_many_with`, and the watch session of DC.7a).
pub(crate) fn aux_graphs(
    root: &Path,
    repo: RepoId,
    regions: &[RegionAnchor],
    roots: &[ProjectRoot],
    md: Vec<(String, String)>,
) -> Vec<RepoGraph> {
    let mut graphs = Vec::new();
    if !regions.is_empty() {
        graphs.push(build_region_graph(regions, repo));
    }
    if !roots.is_empty() {
        graphs.push(build_project_graph(roots, repo));
    }
    let mut doc_records = FileDocSource(md).collect();
    doc_records.extend(SnapshotDocSource::for_repo(root).collect());
    if let Some(docs) = build_docs_graph(&doc_records, repo, roots) {
        graphs.push(docs);
    }
    graphs
}

/// What [`finish_build`] hands back: the finished graph, its totals and the
/// build's tail and total times.
pub(crate) struct Finished {
    pub(crate) merged: MergedGraph,
    pub(crate) total_nodes: usize,
    /// Every repo graph's edges plus the cross edges.
    pub(crate) total_edges: usize,
    /// The `[timing] build` line's times (CA.9): `resolve` / `post` /
    /// `finalize` from the pass report, `external_cells`, `total` since the
    /// build's start. The caller prints [`BuildTimes::marker`].
    pub(crate) times: BuildTimes,
}

/// The build tail over every built repo's graphs, in slot order: merge them,
/// run [`crate::profile::CODE_PASSES`] ([`run_code_passes_with`], LD.13) and
/// then the external node-cell stage ([`apply_external_cells`], LF.1a) over
/// `inputs` (each built repo's `.glia` inputs, in argument order), and total
/// the nodes and edges. `repos` (the repos built) and `started` (the build's
/// start) are the returned [`BuildTimes`]' `repos` and `total`.
pub(crate) fn finish_build(
    graphs: Vec<RepoGraph>,
    inputs: Vec<RepoInputs>,
    opts: &BuildOptions,
    repos: usize,
    started: Instant,
) -> Finished {
    let mut merged = MergedGraph::new(graphs);
    let ctx = CodeBuildCtx::new(inputs, opts);
    let report = run_code_passes_with(&mut merged, &ctx);
    let external = timed(|| apply_external_cells(&mut merged, &ctx.inputs));
    let total_nodes: usize = merged.graphs.iter().map(|g| g.nodes.len()).sum();
    let total_edges: usize = merged.graphs.iter().map(|g| g.edges.len()).sum::<usize>()
        + merged.cross_edges.len();
    let times = BuildTimes::new(repos, &report, Some(external), started.elapsed());
    Finished { merged, total_nodes, total_edges, times }
}

/// Run `f` and return what it took (CA.9).
fn timed(f: impl FnOnce()) -> Duration {
    let started = Instant::now();
    f();
    started.elapsed()
}

/// LB.1 fired_on marker, one line per repo per build:
///   `[repo-id] source=<git-remote|git-local|dir> key=<identity key> repo=<path as given>`
/// The key never carries remote-URL userinfo (`normalise_remote_url` drops it).
fn repo_id_marker(ident: &RepoIdentity, repo_path: &str) {
    eprintln!("[repo-id] source={} key={} repo={repo_path}", ident.source.as_str(), ident.key);
}

/// The canonical absolute spelling of `root`, or `root` as given when it
/// cannot be resolved. Only [`disambiguate`] suffixes it onto a key.
fn canonical_display(root: &Path) -> String {
    std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Inputs of ONE multi-repo build that share an identity key (two non-git
/// `app/` dirs, two worktrees of one clone) must still be distinct repos, or
/// every same-qname node collides. Every member of a colliding group becomes
/// `<key>@<canonical abs path>` — every member, never "first wins", so the
/// result does not depend on argument order. Groups are found in argument
/// order by linear scan (no HashMap iteration reaches the output). Returns one
/// `[repo-id]` collision line per group, for the caller to print.
fn disambiguate(idents: &mut [RepoIdentity], abs_paths: &[String]) -> Vec<String> {
    let original: Vec<String> = idents.iter().map(|i| i.key.clone()).collect();
    let mut grouped = vec![false; original.len()];
    let mut lines = Vec::new();
    for i in 0..original.len() {
        if grouped[i] {
            continue;
        }
        let members: Vec<usize> =
            (i..original.len()).filter(|&j| original[j] == original[i]).collect();
        for &j in &members {
            grouped[j] = true;
        }
        if members.len() < 2 {
            continue;
        }
        for &j in &members {
            if let (Some(id), Some(abs)) = (idents.get_mut(j), abs_paths.get(j)) {
                id.key = format!("{}@{abs}", original[j]);
            }
        }
        lines.push(format!(
            "[repo-id] {} inputs share key {}; disambiguated by path",
            members.len(),
            original[i]
        ));
    }
    lines
}

/// The repo's Go module map (LA.13): the `go.mod` of every project root the
/// walk found (A8.4), the repo root's included, each with its `module` path.
/// The Go parser maps an import under any of them onto that module's root dir
/// and treats the rest as libraries (WP-G / #6; before LA.13 only
/// `<root>/go.mod` was read, so a repo whose go.mods are all nested kept every
/// internal import raw).
///
/// Every root's dir is checked, not only the `go` ecosystem's: a dir holding
/// both a `package.json` and a `go.mod` is an `npm` root by manifest
/// precedence, and its Go module is no less real. A go.mod with no `module`
/// line is skipped.
///
/// fired_on marker, once per repo with at least one module (the first four,
/// then `+K more`; the repo root's dir prints as `.`):
///   `[go-modules] N module roots (svc=example.com/svc svc-b=example.com/svc-b) repo=<label>`
///
/// `pub(crate)` for the shared cache (CE.2a), whose keys hash this module set
/// for a Go file exactly as the build's parse cache checks it.
pub(crate) fn go_modules_for(
    root: &Path,
    roots: &[ProjectRoot],
    repo_label: &str,
) -> GoModules {
    let entries: Vec<(String, String)> = roots
        .iter()
        .filter_map(|r| {
            let module = read_go_module_path(&root.join(&r.rel_path))?;
            Some((r.rel_path.clone(), module))
        })
        .collect();
    let go = GoModules::from_entries(entries);
    if !go.is_empty() {
        let shown: Vec<String> = go
            .iter()
            .take(4)
            .map(|(dir, module)| {
                format!("{}={module}", if dir.is_empty() { "." } else { dir })
            })
            .collect();
        let more = go.len().saturating_sub(4);
        let more = if more > 0 { format!(" +{more} more") } else { String::new() };
        eprintln!(
            "[go-modules] {} module root{} ({}{more}) repo={repo_label}",
            go.len(),
            if go.len() == 1 { "" } else { "s" },
            shown.join(" ")
        );
    }
    go
}

/// The `module` path of `<dir>/go.mod` (`module example.com/svc`, also quoted
/// or with a trailing `//` comment), or `None` without a go.mod or a module
/// line.
fn read_go_module_path(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("go.mod")).ok()?;
    text.lines().map(str::trim).find_map(|l| {
        let rest = l.strip_prefix("module")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let module = rest.split("//").next().unwrap_or("").trim();
        let module = module.trim_matches(|c| c == '"' || c == '`');
        (!module.is_empty()).then(|| module.to_string())
    })
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn unique_tmp(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("glia_wpd_{}_{}_{}", std::process::id(), tag, n))
    }

    /// Sorted (node-ids, edges) — a deterministic fingerprint of a graph's
    /// content, independent of build order.
    fn fingerprint(g: &MergedGraph) -> (Vec<u64>, Vec<(u64, u64, u32)>) {
        let mut nodes: Vec<u64> =
            g.graphs.iter().flat_map(|r| r.nodes.iter().map(|n| n.id.0)).collect();
        nodes.sort_unstable();
        let mut edges: Vec<(u64, u64, u32)> =
            g.all_edges().map(|e| (e.from.0, e.to.0, e.category.0)).collect();
        edges.sort_unstable();
        (nodes, edges)
    }

    #[test]
    fn incremental_reuses_unchanged_and_matches_clean_build() {
        let dir = unique_tmp("incr");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.py");
        std::fs::write(&a, "def foo():\n    return 1\n").unwrap();
        std::fs::write(dir.join("b.py"), "def bar():\n    return 2\n").unwrap();
        let repo = dir.to_str().unwrap();

        // Cold cache: both files parsed; graph identical to a clean build.
        let clean = generate_one(repo).unwrap();
        let mut cache = ParseCache::new();
        let cold = generate_one_with_cache(repo, &mut cache).unwrap();
        assert_eq!(cache.stats.reparsed, 2);
        assert_eq!(cache.stats.reused, 0);
        assert_eq!(fingerprint(&clean.merged), fingerprint(&cold.merged));

        // Edit one file → only that file reparses, the other is reused…
        std::fs::write(&a, "def foo():\n    return 1 + 1\n").unwrap();
        let warm = generate_one_with_cache(repo, &mut cache).unwrap();
        assert_eq!(cache.stats.reparsed, 1, "only the edited file reparsed");
        assert_eq!(cache.stats.reused, 1, "the unchanged file reused");

        // …and the incremental result equals a fresh clean build as a SET
        // (sorted fingerprint — fast unit check). The real WP-D acceptance
        // gate is tests/byte_identical.rs, which compares the actual bytes
        // the store writes and fails on any ordering nondeterminism.
        let clean2 = generate_one(repo).unwrap();
        assert_eq!(fingerprint(&warm.merged), fingerprint(&clean2.merged));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn incremental_evicts_deleted_files() {
        let dir = unique_tmp("evict");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def foo():\n    return 1\n").unwrap();
        let b = dir.join("b.py");
        std::fs::write(&b, "def bar():\n    return 2\n").unwrap();
        let repo = dir.to_str().unwrap();

        let mut cache = ParseCache::new();
        generate_one_with_cache(repo, &mut cache).unwrap();
        assert_eq!(cache.len(), 2);

        std::fs::remove_file(&b).unwrap();
        generate_one_with_cache(repo, &mut cache).unwrap();
        assert_eq!(cache.stats.evicted, 1, "deleted file evicted from cache");
        assert_eq!(cache.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cache_discarded_when_build_context_changes() {
        let dir = unique_tmp("ctx");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def foo():\n    return 1\n").unwrap();
        let repo = dir.to_str().unwrap();

        // Same dir, different path spelling → same identity key (LB.1), so the
        // same RepoId is baked into cached NodeIds → reuse.
        let mut cache = ParseCache::new();
        generate_one_with_cache(repo, &mut cache).unwrap();
        assert_eq!(cache.stats.reparsed, 1);
        let alt = format!("{repo}/.");
        generate_one_with_cache(&alt, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 1, "a path-spelling change must reuse");
        assert_eq!(cache.stats.reparsed, 0);

        // A different identity (another basename, no git) → another RepoId →
        // every entry discarded, even though a.py's path and bytes match.
        let other = unique_tmp("ctx_other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("a.py"), "def foo():\n    return 1\n").unwrap();
        generate_one_with_cache(other.to_str().unwrap(), &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 0, "another repo identity must not reuse");
        assert_eq!(cache.stats.reparsed, 1);
        std::fs::remove_dir_all(&other).ok();

        // Back to the first repo: discarded again, then reuse works.
        generate_one_with_cache(&alt, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 0);
        generate_one_with_cache(&alt, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 1);

        // go.mod module change → .go parses are context-dependent → discard.
        std::fs::write(dir.join("m.go"), "package m\nfunc F() {}\n").unwrap();
        std::fs::write(dir.join("go.mod"), "module example.com/one\n").unwrap();
        generate_one_with_cache(&alt, &mut cache).unwrap();
        std::fs::write(dir.join("go.mod"), "module example.com/two\n").unwrap();
        generate_one_with_cache(&alt, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 0, "go.mod module change must not reuse");

        std::fs::remove_dir_all(&dir).ok();
    }

    fn ident(key: &str) -> RepoIdentity {
        RepoIdentity {
            key: key.to_string(),
            source: glia_code_domain::walk_gating::IdentitySource::Directory,
        }
    }

    /// Every member of a colliding group is suffixed (never "first wins"), a
    /// unique key is left alone, and one collision line is emitted per group.
    #[test]
    fn disambiguate_suffixes_every_member_of_a_shared_key() {
        let mut ids = vec![ident("dir:app"), ident("dir:lib"), ident("dir:app"), ident("dir:app")];
        let abs: Vec<String> = ["/t/a/app", "/t/lib", "/t/b/app", "/t/c/app"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let lines = disambiguate(&mut ids, &abs);
        let keys: Vec<&str> = ids.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["dir:app@/t/a/app", "dir:lib", "dir:app@/t/b/app", "dir:app@/t/c/app"]);
        assert_eq!(lines, ["[repo-id] 3 inputs share key dir:app; disambiguated by path"]);

        // Argument order changes the order of the inputs, never their keys.
        let mut rev = vec![ident("dir:app"), ident("dir:app")];
        let rev_abs = vec!["/t/b/app".to_string(), "/t/a/app".to_string()];
        disambiguate(&mut rev, &rev_abs);
        assert_eq!(rev[0].key, "dir:app@/t/b/app");
        assert_eq!(rev[1].key, "dir:app@/t/a/app");

        let mut solo = vec![ident("dir:app"), ident("git:github.com/x/y")];
        assert!(disambiguate(&mut solo, &abs[..2]).is_empty());
        assert_eq!(solo[0].key, "dir:app");
    }

    #[test]
    fn parse_cache_disk_roundtrip() {
        let dir = unique_tmp("disk");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def foo():\n    return 1\n").unwrap();
        let repo = dir.to_str().unwrap();

        // Missing cache loads empty.
        assert!(ParseCache::load(repo).is_empty());
        // Incremental build persists it; a fresh load sees the entry.
        generate_one_incremental(repo).unwrap();
        assert_eq!(ParseCache::load(repo).len(), 1);

        // Explicit purge (the --no-incremental escape hatch) removes the
        // sidecar; the next load starts cold. Purging a missing file is Ok.
        ParseCache::purge(repo).unwrap();
        assert!(ParseCache::load(repo).is_empty());
        ParseCache::purge(repo).unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod stage_tests {
    use std::cell::RefCell;

    use super::assemble::{parse_and_graft, repo_markers};
    use super::lang_build::{
        GroupCtx, LangBuild, LanguageGraphs, build_groups, fold_groups, plan_groups,
    };
    use super::*;

    /// One single-repo build through the DC.2 stages, composed as
    /// `generate_one_inner` composes them, with `reuse(key)` filling each
    /// group's reuse slot.
    struct Staged {
        result: GenerateResult,
        /// The group builds before the fold, cloned: what a later build of
        /// the same repo can reuse.
        builds: Vec<LangBuild>,
        /// The plan's keys, in plan order.
        plan_keys: Vec<&'static str>,
        /// `LanguageGraphs::keys`: the key of each language graph.
        graph_keys: Vec<&'static str>,
    }

    fn staged(repo_path: &str, reuse: impl Fn(&'static str) -> Option<LangBuild>) -> Staged {
        let started = Instant::now();
        let root = PathBuf::from(repo_path);
        let ident = repo_identity(&root);
        let repo = RepoId::from_canonical(&ident.key);
        let (files, regions, md, roots) = walk_source_files(&root);
        let opts = BuildOptions::default();
        let inputs = vec![repo_inputs(repo, root.clone(), repo_path.to_string(), None)];
        let go = go_modules_for(&root, &roots, repo_path);
        let ts_aliases = TsAliasSet::read(&root, &roots, repo_path);
        let c_includes = IncludeRoots::read(&root, &files, &roots, repo_path);
        let mut rpc = RpcContext::default();
        rpc.add_files(&files);
        let ctx = RepoBuildCtx {
            repo,
            repo_label: repo_path,
            go: &go,
            ts_aliases: &ts_aliases,
            c_includes: &c_includes,
            rpc: &rpc,
            roots: &roots,
            config: inputs.first().and_then(|i| i.config.as_ref()),
            opts: &opts,
        };
        let mut grafted = parse_and_graft(&files, None, &ctx);
        let plan = plan_groups(std::mem::take(&mut grafted.parses_by_lang), repo_path);
        let plan_keys: Vec<&'static str> = plan.groups.iter().map(|g| g.key).collect();
        let slots: Vec<Option<LangBuild>> = plan_keys.iter().map(|k| reuse(k)).collect();
        let group_ctx = GroupCtx {
            repo,
            repo_label: repo_path,
            rust_crates: &grafted.rust_crates,
            ts_aliases: &ts_aliases,
            c_includes: &c_includes,
        };
        let builds = build_groups(plan.groups, slots, &group_ctx);
        let kept = builds.clone();
        let LanguageGraphs { mut graphs, keys, pooled } =
            fold_groups(builds, &plan.recv_fields, &mut grafted.parse_errors, repo_path);
        repo_markers(&graphs, &plan.di_refs, &grafted, pooled, repo_path);
        graphs.extend(aux_graphs(&root, repo, &regions, &roots, md));
        let Finished { merged, total_nodes, total_edges, .. } =
            finish_build(graphs, inputs, &opts, 1, started);
        let result = GenerateResult {
            merged,
            total_nodes,
            total_edges,
            parse_errors: grafted.parse_errors,
            repo_labels: crate::arch::repo_label_map(&[(repo.0, repo_path.to_string())]),
            repo_roots: std::collections::BTreeMap::from([(repo.0, repo_path.to_string())]),
        };
        Staged { result, builds: kept, plan_keys, graph_keys: keys }
    }

    /// A repo of every group shape the plan orders: python, go (with a
    /// go.mod), rust (a Cargo package) and TS (the family, built last), plus
    /// a README for the docs graph and the project graph its manifests make.
    fn write_repo(dir: &Path) {
        let write = |rel: &str, body: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        write("a.py", "def foo():\n    return bar()\n\ndef bar():\n    return 1\n");
        write("go.mod", "module example.com/fx\n");
        write("m.go", "package m\n\nfunc F() int { return G() }\n\nfunc G() int { return 1 }\n");
        write("c.ts", "export function tsEntry(): number {\n  return helper();\n}\nfunction helper(): number { return 3; }\n");
        write("Cargo.toml", "[package]\nname = \"fx-crate\"\nversion = \"0.1.0\"\n");
        write("src/lib.rs", "pub fn run() -> u32 { step() }\nfn step() -> u32 { 1 }\n");
        write("README.md", "# Fixture\n\nCall `foo` and `run`.\n");
    }

    /// The bytes of every file `write_merged_sharded` writes for `merged`.
    fn shard_bytes(merged: &MergedGraph, dir: &Path) -> Vec<(String, Vec<u8>)> {
        glia_store::write_merged_sharded(merged, dir).unwrap();
        let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn stages_equal_generate_one() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        write_repo(&repo);
        let repo_s = repo.to_str().unwrap();

        let clean = generate_one(repo_s).unwrap();
        let stages = staged(repo_s, |_| None);
        assert_eq!(stages.plan_keys, ["go", "python", "rust", "typescript"], "TS family last");
        assert_eq!(stages.graph_keys, stages.plan_keys, "one graph per group, no failure");
        assert_eq!(stages.builds.iter().map(LangBuild::key).collect::<Vec<_>>(), stages.plan_keys);
        assert_eq!(stages.result.parse_errors, clean.parse_errors);
        assert_eq!(
            (stages.result.total_nodes, stages.result.total_edges),
            (clean.total_nodes, clean.total_edges)
        );
        assert_eq!(
            shard_bytes(&stages.result.merged, &tmp.path().join("stages")),
            shard_bytes(&clean.merged, &tmp.path().join("clean")),
            "the stages write generate_one's bytes"
        );
    }

    #[test]
    fn group_reuse_reassembles_in_clean_order() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        write_repo(&repo);
        let repo_s = repo.to_str().unwrap();

        let a = staged(repo_s, |_| None);
        let a_bytes = shard_bytes(&a.result.merged, &tmp.path().join("a"));
        // A body edit of the python file only: every other group's inputs
        // are unchanged, so its build from A is the build B would make.
        std::fs::write(repo.join("a.py"), "def foo():\n    return bar() + 1\n\ndef bar():\n    return 2\n")
            .unwrap();
        let offered: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());
        let b = staged(repo_s, |key| {
            let build = a.builds.iter().find(|b| b.key() == key && key != "python").cloned();
            if build.is_some() {
                offered.borrow_mut().push(key);
            }
            build
        });
        let clean = generate_one(repo_s).unwrap();

        let reused = offered.into_inner();
        let expected: Vec<&'static str> =
            b.plan_keys.iter().copied().filter(|k| *k != "python").collect();
        assert_eq!(reused, expected, "every group but python reused");
        assert_eq!(b.graph_keys, b.plan_keys);
        assert_eq!(b.result.parse_errors, clean.parse_errors);
        let b_bytes = shard_bytes(&b.result.merged, &tmp.path().join("b"));
        assert_ne!(b_bytes, a_bytes, "the python edit reached the graph");
        assert_eq!(
            b_bytes,
            shard_bytes(&clean.merged, &tmp.path().join("clean")),
            "reused groups reassemble into the clean build's bytes"
        );
    }

    /// A reuse slot holding another group's build is not taken: the group is
    /// built, so the result is still the clean build.
    #[test]
    fn a_reuse_of_another_key_is_built() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        write_repo(&repo);
        let repo_s = repo.to_str().unwrap();

        let a = staged(repo_s, |_| None);
        let go = a.builds.iter().find(|b| b.key() == "go").cloned().unwrap();
        let b = staged(repo_s, |key| (key == "python").then(|| go.clone()));
        assert_eq!(b.builds.iter().map(LangBuild::key).collect::<Vec<_>>(), b.plan_keys);
        assert_eq!(
            shard_bytes(&b.result.merged, &tmp.path().join("b")),
            shard_bytes(&generate_one(repo_s).unwrap().merged, &tmp.path().join("clean")),
        );
    }
}
