//! Per-repo graph assembly: `build_graphs_for_repo` and the facts and markers
//! it owns (the A11.1 const table and its LF.2d overlay pins, the A12.1
//! `[msgtype]` census, the quiet-panic scope that covers parse and grafts).
//! DC.2 split it into callable stages: [`parse_and_graft`] (parse, const
//! table, Cargo and Dart packages, post-cache grafts), the per-group build of
//! [`super::lang_build`] (plan, build, fold) and [`repo_markers`].

use std::collections::{BTreeMap, HashMap};

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::glia_config::LoadedConfig;
use glia_code_domain::hosts::{Refusal, StartTarget, TargetForm, normalise_argv};
use glia_code_domain::project_roots::ProjectRoot;
use glia_code_domain::walk_gating::IgnoreStack;
use glia_code_domain::{FileParse, cell_type, di_stats, node_kind};
use glia_code_extractors::constants::ConstTable;
use glia_core::RepoId;
use glia_graph::RepoGraph;
use glia_graph::rust_paths::RustCrate;

use std::time::Instant;

use super::BuildOptions;
use super::c_includes::IncludeRoots;
use super::grafts;
use super::lang_build::{
    self, DartPackages, GroupCtx, GroupPlan, LangBuild, LanguageGraphs, TsAliasSet,
};
use super::memo::{BuildMemo, ConstKey, ConstMemo, MemoStats};
use super::rpc_needles::RpcContext;
use super::timing::PhaseTimes;
use crate::cache::{ParseCache, content_hash};
use crate::external::RepoInputs;
use crate::extract::{GoModules, detect_language};
use crate::passes::GroupFill;
use crate::route::parse_repo_files_hashed;

/// The repo-scope name -> literal table (A11.1), built from every walked file
/// with a source language. Like the go.mod set (`go_modules_for`) it is a
/// cross-file fact the engine gathers, but unlike the go.mod set it is NOT
/// handed to the per-file extractors: their output is cached by the file's own
/// content hash, and a table lookup depends on other files. A consumer runs
/// after the cache, as `apply_rpc_needles` does, so incremental == clean.
///
/// `files` is name-sorted by the walk and the table is first-wins, so the
/// result does not depend on the process. `.env` / yaml / Dockerfile have no
/// source language and never reach the scan (env values are A13.7's ENV cell).
///
/// LG.1b: each file is scanned on the engine pool
/// ([`crate::parallel::par_map_ordered`]), then merged into the table and its
/// panics reported in file order, so the first-wins bindings, the conflict
/// count and the `parse_errors` order are the sequential scan's. Returns the
/// table, the per-file scans ([`FileConstTables`], DC.3), the files scanned
/// and the pool's thread count, for the `[parallel]` line.
///
/// DC.4: the build runs [`build_const_table_with`] (the memo-aware scan);
/// this memo-free form is the stage tests' entry (grafts.rs).
#[cfg(test)]
pub(super) fn build_const_table(
    files: &[(String, String)],
    parse_errors: &mut Vec<String>,
) -> (ConstTable, FileConstTables, usize, usize) {
    build_const_table_with(files, None, parse_errors)
}

/// One file's A11.1 scan as the pool returns it (DC.4). The inner `Option`
/// is the scan's table, `None` when it panicked.
enum ConstScan {
    /// No source language: never scanned.
    NoLang,
    /// Replayed from the const memo.
    Hit(Option<ConstTable>),
    /// Scanned now, as `lang`.
    Fresh(&'static str, Option<ConstTable>),
}

/// [`build_const_table`], and (DC.4) with `memo` = the walked files' content
/// hashes (index-aligned with `files`) and the held cache's memos: a file
/// whose scan the const memo holds under the same `(content hash, language)`
/// is replayed from it instead of rescanned, a panicking scan as the same
/// `PANIC (const table scan)` line and a `None` per-file entry; each fresh
/// scan is recorded after the pool, in the sequential fold. Hits and fresh
/// scans fold identically, so the table, its first-wins bindings, the
/// conflict count, [`FileConstTables`] and the `parse_errors` order are the
/// clean build's. Sets the memo's `const_hit` / `const_miss` (files with a
/// source language). A hash list of another length disables the memo.
fn build_const_table_with(
    files: &[(String, String)],
    memo: Option<(&[u64], &mut BuildMemo)>,
    parse_errors: &mut Vec<String>,
) -> (ConstTable, FileConstTables, usize, usize) {
    let mut memo = memo.filter(|(hashes, _)| hashes.len() == files.len());
    let lookup: Option<(&[u64], &ConstMemo)> = memo.as_ref().map(|(h, m)| (*h, &m.consts));
    let slots: Vec<usize> = (0..files.len()).collect();
    let (scans, threads) = crate::parallel::par_map_ordered(&slots, |&i| {
        let (path, source) = &files[i];
        let Some(lang) = detect_language(path) else {
            return ConstScan::NoLang;
        };
        if let Some((hashes, consts)) = lookup
            && let Some(scan) = consts.get(
                path,
                &ConstKey {
                    content_hash: hashes[i],
                    lang,
                },
            )
        {
            return ConstScan::Hit(scan.clone());
        }
        ConstScan::Fresh(
            lang,
            crate::parallel::quiet(|| ConstTable::scan_file(source, lang)).ok(),
        )
    });
    let mut table = ConstTable::default();
    let mut per_file = Vec::with_capacity(files.len());
    let mut scanned = 0usize;
    let (mut hit, mut miss) = (0usize, 0usize);
    for (i, ((path, _), scan)) in files.iter().zip(scans).enumerate() {
        let scan = match scan {
            ConstScan::NoLang => {
                per_file.push(None);
                continue;
            }
            ConstScan::Hit(scan) => {
                hit += 1;
                scan
            }
            ConstScan::Fresh(lang, scan) => {
                miss += 1;
                if let Some((hashes, m)) = memo.as_mut() {
                    let key = ConstKey {
                        content_hash: hashes[i],
                        lang,
                    };
                    m.consts.insert(path.clone(), key, scan.clone());
                }
                scan
            }
        };
        scanned += 1;
        match scan {
            Some(file_table) => {
                table.merge_from(&file_table);
                per_file.push(Some(file_table));
            }
            None => {
                parse_errors.push(format!("{path}: PANIC (const table scan)"));
                per_file.push(None);
            }
        }
    }
    if let Some((_, m)) = memo {
        m.stats.const_hit = hit;
        m.stats.const_miss = miss;
    }
    (table, FileConstTables(per_file), scanned, threads)
}

/// DC.3: the per-file A11.1 scans [`build_const_table`] ran, index-aligned
/// with the walked `files`: `Some` is the file's own table
/// (`ConstTable::scan_file`, before any merge or LF.2d pin), `None` a file
/// with no source language or whose scan panicked. The LA.4 queue-topic and
/// CB.3b event-name const folds (`grafts::apply_post_cache`) resolve a holder
/// file's SAME-FILE bindings here instead of re-scanning it; a fold target
/// whose entry is `None` had a panicking scan, and the fold reports its own
/// `PANIC (.. const fold)` line, as its re-scan used to.
pub(super) struct FileConstTables(pub(super) Vec<Option<ConstTable>>);

impl FileConstTables {
    /// The scan of `files[file]`; `None` when it has no language, its scan
    /// panicked, or `file` is out of range.
    pub(super) fn get(&self, file: usize) -> Option<&ConstTable> {
        self.0.get(file).and_then(Option::as_ref)
    }
}

/// LF.2d: pin the repo's `.glia/overlay.toml` `[constants]` into `table`,
/// after every source merge (`ConstTable::pin` replaces a first-wins binding
/// and clears its alternatives). Only when the build applies the overlay: an
/// extraction-only build (`--no-overlay`) folds against the source table, and
/// `external::apply_external_edges` prints its `[overlay] disabled` line.
///
/// fired_on marker, once per repo that declares `[constants]`:
///   `[overlay] constants repo=<label> pinned=<n> overrode=<o> rejected=<r>`
/// `pinned` counts bound pins, `overrode` the pinned keys that already had a
/// differing (or ambiguous) source binding, `rejected` the pins the scan's
/// secret-name / value gates refused (never bound: an overlay constant never
/// bypasses redaction).
fn pin_overlay_constants(
    table: &mut ConstTable,
    config: Option<&LoadedConfig>,
    opts: &BuildOptions,
    repo_label: &str,
) {
    let Some(cfg) = config.filter(|_| opts.overlay) else {
        return;
    };
    let constants = &cfg.config.constants;
    if constants.is_empty() {
        return;
    }
    let (mut pinned, mut overrode, mut rejected) = (0usize, 0usize, 0usize);
    for (key, value) in constants {
        let prior: Vec<String> = match table.candidates(key) {
            c if !c.is_empty() => c.into_iter().map(String::from).collect(),
            _ => table.get(key).map(String::from).into_iter().collect(),
        };
        if !table.pin(key, value.get_ref()) {
            rejected += 1;
            continue;
        }
        pinned += 1;
        let now = table.get(key);
        overrode += usize::from(prior.iter().any(|p| Some(p.as_str()) != now));
    }
    eprintln!(
        "[overlay] constants repo={repo_label} pinned={pinned} overrode={overrode} rejected={rejected}"
    );
}

/// Everything `build_graphs_for_repo` knows about one repo besides its files
/// and its parse cache (A6.8): the facts its caller gathered off the walk and
/// the repo dir before the parse. A packet that adds a per-repo input extends
/// this struct, never the parameter list.
#[derive(Clone, Copy)]
pub(crate) struct RepoBuildCtx<'a> {
    pub(crate) repo: RepoId,
    /// The repo path as the caller was given it. It only labels markers
    /// (`[incremental]`, `[const]`, ...), so a multi-repo build prints one
    /// attributable line per repo; it never reaches the graph.
    pub(crate) repo_label: &'a str,
    /// The repo's go.mod set (LA.13), handed to every Go parse.
    pub(crate) go: &'a GoModules,
    /// The repo's tsconfig `paths` aliases per project dir (A6.8): the TS
    /// family's import resolver and the IMPORTS-cell filter read them.
    pub(crate) ts_aliases: &'a TsAliasSet,
    /// The repo's C/C++ include search roots (CB.22): `build_c_cpp`'s include
    /// resolver reads them after the parse cache, so the cache needs no key.
    pub(crate) c_includes: &'a IncludeRoots,
    /// The build-wide proto service set (A5.2).
    pub(crate) rpc: &'a RpcContext,
    /// The walk's project roots (A8.4), the owner vocabulary of the LB.4a HTTP
    /// owner segment.
    pub(crate) roots: &'a [ProjectRoot],
    /// The repo's loaded `.glia/overlay.toml` (`RepoInputs::config`). With
    /// `opts` it decides the overlay stages that run before the graph is built
    /// (LF.2d's constant pins; LF.2e's `[[wrapper]]` call sites, minted inside
    /// `grafts::apply_post_cache`).
    pub(crate) config: Option<&'a LoadedConfig>,
    pub(crate) opts: &'a BuildOptions,
}

/// One repo's parses after every post-cache graft ([`parse_and_graft`]): the
/// input of its per-group build, and what [`repo_markers`] reports.
pub(crate) struct Grafted {
    /// The finished parses per parse-language tag; [`lang_build::plan_groups`]
    /// orders them.
    pub(crate) parses_by_lang: HashMap<&'static str, Vec<FileParse>>,
    /// The parse, const-table-scan and graft error lines, in that order.
    pub(crate) parse_errors: Vec<String>,
    /// `parse`, `const_scan` and `grafts` (CA.9); `walk` and
    /// `language_build` are zero, for the caller that ran them to fill.
    pub(crate) times: PhaseTimes,
    /// The Cargo packages ([`lang_build::rust_crates`]), read by the A16.4
    /// IMPORTS filter inside the grafts and by `build_rust`
    /// ([`GroupCtx::rust_crates`]).
    pub(crate) rust_crates: Vec<RustCrate>,
    /// The repo's own Dart packages ([`DartPackages::from_roots`]), read by
    /// the Dart import resolver ([`GroupCtx::dart_packages`], DF.4a).
    pub(crate) dart_packages: DartPackages,
    /// Files the A11.1 const-table scan read, for the `[parallel]` line.
    pub(crate) const_files: usize,
    /// The engine pool's thread count, for the `[parallel]` line.
    pub(crate) threads: usize,
    /// Files the RPC needle pass ran on, for the `[parallel]` line.
    pub(crate) rpc_files: usize,
}

/// Stage 1 of a repo's assembly: parse `files` (reusing `cache` when given),
/// build the const table and its LF.2d pins, read the Cargo and Dart packages
/// (DF.4a) and run the post-cache grafts, under the per-repo context `ctx`
/// ([`RepoBuildCtx`]). `files` is the whole walked file set of the repo: a
/// caller that holds its own file table (a watch session) injects it here.
///
/// Its times (CA.9): `parse`, `const_scan` (the table, its `[const]` line and
/// the LF.2d pins) and `grafts` (the Cargo- and Dart-package reads and
/// `grafts::apply_post_cache`).
///
/// DC.4: with a cache, the walked files' content hashes are computed once
/// here, on the pool (inside `parse`), and handed to the router, the const
/// scan and the RPC needles, which each replay an unchanged file's output
/// from the cache's build memos (`build::memo`) instead of recomputing it.
/// The memo's stats are reset first, and its fired_on line follows the
/// grafts, once per repo build that holds a cache:
///   `[graft-memo] const hit=<h>/<n> needles hit=<h>/<n> noncode hit=<h>/<n> imports hit=<h>/<n> index=<hit|built|none> wrappers hit=<h>/<n> scan hit=<h>/<n> repo=<label>`
pub(crate) fn parse_and_graft(
    files: &[(String, String)],
    mut cache: Option<&mut ParseCache>,
    ctx: &RepoBuildCtx<'_>,
) -> Grafted {
    let RepoBuildCtx {
        repo,
        repo_label,
        go,
        ts_aliases,
        c_includes: _,
        rpc,
        roots,
        config,
        opts,
    } = *ctx;
    // Keep caught per-file panics off stderr: the default hook would print
    // (with a backtrace) for every bad file even though it becomes a
    // parse_errors line. LG.1a: the flag is per thread (`parallel`), so this
    // scope covers every `catch_unwind` the assembly runs on THIS thread
    // (grafts, the overlay wrapper stage, the needle passes), the pool
    // workers go through `parallel::quiet`, and a panic on any other thread
    // of the process still reaches the original hook. The group builds run
    // inside `parallel::quiet` on whichever thread maps them.
    let _quiet = crate::parallel::quiet_scope();
    // A7.0: shape counters describe only the detectors that run in THIS build.
    di_stats::reset();

    let mut times = PhaseTimes::default();
    let started = Instant::now();
    // DC.4: one content hash per walked file, only when a cache is held.
    let hashes: Option<Vec<u64>> = cache.as_deref_mut().map(|c| {
        c.memo_mut().stats = MemoStats::default();
        crate::parallel::par_map_ordered(files, |(_, source)| content_hash(source)).0
    });
    let (mut parses_by_lang, mut parse_errors) = parse_repo_files_hashed(
        files,
        hashes.as_deref(),
        repo,
        go,
        cache.as_deref_mut(),
        repo_label,
    );
    times.parse = started.elapsed();

    // A11.1 fired_on marker, once per repo. Post-cache passes that read the
    // table (A11.2 endpoint fold, queue-topic const fold) take `&const_table`
    // and live in `grafts::apply_post_cache`, beside `apply_rpc_needles`; the
    // queue / event const folds also read `file_tables`, the per-file scans
    // the table was merged from (DC.3).
    let started = Instant::now();
    let memo = hashes
        .as_deref()
        .zip(cache.as_deref_mut().map(ParseCache::memo_mut));
    let (mut const_table, file_tables, const_files, threads) =
        build_const_table_with(files, memo, &mut parse_errors);
    if !const_table.is_empty() {
        eprintln!(
            "[const] repo table: {} bindings from {} files ({} conflicts) repo={repo_label}",
            const_table.len(),
            const_table.files(),
            const_table.conflicts()
        );
    }
    // LF.2d: overlay constants, after every source binding and after the
    // `[const]` line (which keeps describing the source alone).
    pin_overlay_constants(&mut const_table, config, opts, repo_label);
    times.const_scan = started.elapsed();
    // LA.1a / LA.1b: the Cargo packages, read by the A16.4 IMPORTS filter
    // (a sibling crate is not a dependency) and by `build_rust`.
    let started = Instant::now();
    let rust_crates = lang_build::rust_crates(files, roots);
    // DF.4a: the pubspec packages, which own-package `package:` URIs name.
    let dart_packages = DartPackages::from_roots(roots);
    // DC.4: the needle memo travels to `apply_rpc_needles` inside a per-repo
    // copy of the build's proto service set (`RpcContext::with_memo`), and
    // comes back with this build's fresh outputs below. Not when the build
    // knows no service: the needles do not run.
    let mut repo_rpc = match (cache.as_deref_mut(), hashes) {
        (Some(c), Some(hashes)) if !rpc.services.is_empty() => {
            Some(rpc.with_memo(std::mem::take(&mut c.memo_mut().needles), hashes))
        }
        _ => None,
    };
    // LF.2e (http), A11.2, LA.6d, LA.4, LF.2e (queue), A5.2 / A5.3, A5.8,
    // LB.4a / LB.8, A16.4: the post-cache grafts, in that order. The overlay
    // `[[wrapper]]` stage runs only when the build applies the overlay.
    let mut rpc_added = grafts::apply_post_cache(
        &mut parses_by_lang,
        files,
        repo,
        repo_rpc.as_ref().unwrap_or(rpc),
        &const_table,
        &file_tables,
        roots,
        &rust_crates,
        ts_aliases,
        config.filter(|_| opts.overlay),
        cache.as_deref_mut().map(ParseCache::memo_mut),
        &mut parse_errors,
        repo_label,
    );
    times.grafts = started.elapsed();
    if let Some(c) = cache {
        let memo = c.memo_mut();
        if let Some(needles) = repo_rpc.as_mut().and_then(RpcContext::take_memo) {
            memo.needles = needles;
        }
        rpc_added.record_memo(memo);
        eprintln!("{}", memo.marker(repo_label));
    }

    Grafted {
        parses_by_lang,
        parse_errors,
        times,
        rust_crates,
        dart_packages,
        const_files,
        threads,
        rpc_files: rpc_added.files,
    }
}

/// Parse, graft and build one repo's graphs from its walked `files`, reusing
/// `cache` when given, under the per-repo context `ctx` ([`RepoBuildCtx`]):
/// [`parse_and_graft`], then every group of [`lang_build::plan_groups`] built
/// ([`lang_build::build_groups`], no reuse) and folded
/// ([`lang_build::fold_groups`]), then [`repo_markers`].
///
/// Also returns each graph's evidence fill, index for index with the graphs
/// (DC.5, [`LanguageGraphs::fills`]), and what its phases took (CA.9):
/// `parse`, `const_scan` and `grafts` ([`parse_and_graft`]) and
/// `language_build` (plan, build and fold). `walk` is left zero for the
/// caller, which ran the walk, to fill.
pub(super) fn build_graphs_for_repo(
    files: &[(String, String)],
    cache: Option<&mut ParseCache>,
    ctx: &RepoBuildCtx<'_>,
) -> (Vec<RepoGraph>, Vec<GroupFill>, Vec<String>, PhaseTimes) {
    let mut grafted = parse_and_graft(files, cache, ctx);

    let started = Instant::now();
    let GroupPlan {
        groups,
        di_refs,
        recv_fields,
    } = lang_build::plan_groups(std::mem::take(&mut grafted.parses_by_lang), ctx.repo_label);
    let go_module_dirs = super::go_module_dirs(ctx.go);
    let group_ctx = GroupCtx {
        repo: ctx.repo,
        repo_label: ctx.repo_label,
        rust_crates: &grafted.rust_crates,
        ts_aliases: ctx.ts_aliases,
        c_includes: ctx.c_includes,
        dart_packages: &grafted.dart_packages,
        go_module_dirs: &go_module_dirs,
    };
    // A clean build reuses nothing: every group is built.
    let reuse: Vec<Option<LangBuild>> = Vec::new();
    let builds = lang_build::build_groups(groups, reuse, &group_ctx);
    // DF.4a fired_on marker, once per repo whose Dart build resolved an
    // own-package URI ([`DartPackages::marker`]):
    //   `[dart-imports] own-package uris resolved=<n> packages=<k> repo=<label>`
    if let Some(line) = grafted.dart_packages.marker(ctx.repo_label) {
        eprintln!("{line}");
    }
    let LanguageGraphs {
        graphs,
        keys,
        fills,
        pooled,
    } = lang_build::fold_groups(
        builds,
        &recv_fields,
        &mut grafted.parse_errors,
        ctx.repo_label,
    );
    grafted.times.language_build = started.elapsed();
    // `keys` is index-aligned with `graphs` (a failed group adds neither): a
    // caller that keeps a per-graph side table beside them (a watch session's
    // group of each graph) maps a graph to its group through it.
    debug_assert_eq!(keys.len(), graphs.len(), "one plan key per language graph");
    debug_assert_eq!(
        fills.len(),
        graphs.len(),
        "one evidence fill per language graph"
    );

    repo_markers(&graphs, &di_refs, &grafted, pooled, ctx.repo_label);
    (graphs, fills, grafted.parse_errors, grafted.times)
}

/// The per-repo lines printed once its language graphs are built:
/// - A7.0 `[di] injects refs: … repo=<label>` (`di_refs`: the plan's);
/// - A12.1 `[msgtype]` (counted off `graphs`, see [`msgtype_marker`]);
/// - LG.1b / LG.1c `[parallel] <repo>: const-scan <c> files, rpc-needles <r> files, <g> language graphs on <t> threads`:
///   `c` = files the A11.1 const-table scan read (every file with a source
///   language), `r` = files the RPC needle pass ran on (text-gated, with a
///   parse; 0 when the build knows no proto service), `g` = `pooled`, the
///   group builds mapped on the pool (every build group, the TS family's one
///   graph the last of them, CA.7), `t` = the pool that ran all three.
pub(crate) fn repo_markers(
    graphs: &[RepoGraph],
    di_refs: &[(&'static str, usize)],
    grafted: &Grafted,
    pooled: usize,
    repo_label: &str,
) {
    di_stats::flush_marker(di_refs, repo_label);
    msgtype_marker(graphs, repo_label);
    eprintln!(
        "[parallel] {repo_label}: const-scan {} files, rpc-needles {} files, {pooled} language graphs on {} threads",
        grafted.const_files, grafted.rpc_files, grafted.threads
    );
}

/// A12.1 fired_on marker, once per repo that holds a queue node:
///   `[msgtype] queue_nodes=N typed=T tag_topics=K repo=<label>`
/// `typed` counts nodes carrying a `cell_type::MESSAGE_TYPE` cell, `tag_topics`
/// the identity-free framework tags that can never carry a contract. Counted
/// off the built graphs, so cache-served files count too; silent otherwise.
fn msgtype_marker(graphs: &[RepoGraph], repo_label: &str) {
    let (mut queue_nodes, mut typed, mut tags) = (0usize, 0usize, 0usize);
    for g in graphs {
        for n in &g.nodes {
            let is_queue = g.nav.kind_by_id.get(&n.id).is_some_and(|k| {
                *k == node_kind::QUEUE_PRODUCER || *k == node_kind::QUEUE_CONSUMER
            });
            if !is_queue {
                continue;
            }
            queue_nodes += 1;
            if n.cells.iter().any(|c| c.kind == cell_type::MESSAGE_TYPE) {
                typed += 1;
            }
            // The LB.8 owner segment is not part of the topic.
            if let Some(q) = g.nav.qname_by_id.get(&n.id)
                && let Some((_, topic)) = split_owner(q).0.split_once(':')
                && glia_code_extractors::queues::is_framework_tag(topic)
            {
                tags += 1;
            }
        }
    }
    if queue_nodes > 0 {
        eprintln!(
            "[msgtype] queue_nodes={queue_nodes} typed={typed} tag_topics={tags} repo={repo_label}"
        );
    }
}

// ---------------------------------------------------------------------------
// GH.19b: the entry shell scripts a host start names
// ---------------------------------------------------------------------------

/// GH.19b: the most entry scripts one repo's build reads.
const START_SCRIPT_BUDGET: usize = 64;

/// GH.19b: the most hops a start is followed through (package-manager
/// scripts and entry scripts together), as the host-start rules count them.
const START_SCRIPT_HOPS: usize = 3;

/// The shells a `sh <script>` start runs its script with.
const START_SHELLS: [&str; 6] = ["sh", "bash", "dash", "ash", "zsh", "ksh"];

/// GH.19b: read the entry shell scripts the host starts of each repo name,
/// and put each script's kept line(s) as an `iac::entry_script_cell`
/// (`{"script","exec","row"}`) on the host resource whose start names it,
/// for the host-start rules (`resolver:host`) to follow.
///
/// Only a script a start names is read, never a file by its extension: a
/// final Dockerfile stage's ENTRYPOINT / CMD, a compose service's or k8s
/// container's override (with the image start it completes), a process
/// manager's command, followed through package-manager scripts (`yarn x`,
/// from the package.json cells), the kept lines of a script read before, and
/// an `exec "$@"` passthrough to the start's own arguments, at most
/// [`START_SCRIPT_HOPS`] hops. A script target is a path the start runs
/// (`./bin/run.sh`, `/app/entrypoint.sh`) or a shell's script argument (`sh
/// start.sh`); a container path maps through the image's COPY map to the
/// build context, the contexts a compose service or build cell declares for
/// the Dockerfile, and the repo root; a process manager's relative path to its
/// file's directory (and working directory). The file must be one the walk
/// admits ([`crate::walk::admits_start_script`]), a shell script
/// ([`glia_code_extractors::iac::is_entry_script`]) and at most
/// `iac::ENTRY_SCRIPT_CAP` bytes; at most [`START_SCRIPT_BUDGET`] per repo.
/// Of its text only the `exec` lines, else the last simple command, are kept
/// (`iac::entry_script_lines`): the shell is read, never evaluated. Script
/// cells a graph already carries (a watch re-finishing a reused graph) are
/// replaced, so the result is the clean build's.
pub(crate) fn attach_start_scripts(graphs: &mut [RepoGraph], inputs: &[RepoInputs]) {
    for input in inputs {
        let members: Vec<usize> = (0..graphs.len())
            .filter(|&i| graphs[i].repo == input.repo)
            .collect();
        for &i in &members {
            for n in &mut graphs[i].nodes {
                n.cells
                    .retain(|c| glia_code_extractors::iac::parse_entry_script_cell(c).is_none());
            }
        }
        let starts = StartSeeds::collect(graphs, &members);
        if starts.seeds.is_empty() {
            continue;
        }
        let mut reader = ScriptReader {
            root: &input.root,
            ignores: IgnoreStack::for_repo(&input.root),
            read: BTreeMap::new(),
            pkg_scripts: &starts.pkg_scripts,
        };
        let mut adds: Vec<((usize, usize), Vec<ReadLine>)> = Vec::new();
        for seed in &starts.seeds {
            let mut found = Vec::new();
            reader.follow(&seed.argv, &seed.env, 0, &mut found);
            if !found.is_empty() {
                adds.push((seed.node, found));
            }
        }
        for ((g, n), found) in adds {
            let Some(node) = graphs.get_mut(g).and_then(|g| g.nodes.get_mut(n)) else {
                continue;
            };
            for (path, line, row) in found {
                let cell = glia_code_extractors::iac::entry_script_cell(&path, &line, row);
                if !node.cells.contains(&cell) {
                    node.cells.push(cell);
                }
            }
        }
    }
}

/// One kept script line the reader found: `(repo path, line, 1-based row)`.
type ReadLine = (String, String, u32);

/// Where a start's script paths are looked up ([`attach_start_scripts`]).
#[derive(Clone)]
enum ScriptEnv {
    /// In an image: container paths through its COPY map, joined to each base
    /// (the build context first, the repo root last).
    Container {
        copies: Vec<(String, String)>,
        workdir: Option<String>,
        bases: Vec<String>,
    },
    /// On a machine: relative paths joined to the declaring file's directory
    /// (and a relative working directory).
    Local { base: String },
}

impl ScriptEnv {
    /// The repo paths `target` may name, in lookup order.
    fn paths(&self, target: &str) -> Vec<String> {
        match self {
            ScriptEnv::Container {
                copies,
                workdir,
                bases,
            } => {
                let Some(path) = container_path(target, workdir.as_deref()) else {
                    return Vec::new();
                };
                let Some(rel) = copied_from(copies, &path) else {
                    return Vec::new();
                };
                bases.iter().filter_map(|b| join_repo(b, &rel)).collect()
            }
            ScriptEnv::Local { base } => {
                if target.starts_with('/') {
                    return Vec::new();
                }
                join_repo(base, target).into_iter().collect()
            }
        }
    }

    /// The package.json files a package-manager script is looked up in.
    fn manifests(&self) -> Vec<String> {
        let mut out = self.paths("package.json");
        match self {
            ScriptEnv::Container { bases, .. } => {
                out.extend(bases.first().and_then(|b| join_repo(b, "package.json")));
            }
            ScriptEnv::Local { base } => out.extend(join_repo(base, "package.json")),
        }
        out
    }
}

/// One start to follow: the host resource it is written on, its argv and
/// where it runs.
struct Seed {
    node: (usize, usize),
    argv: Vec<String>,
    env: ScriptEnv,
}

/// A final-stage docker start cell, as the seeds read it.
struct DockerCell {
    image: String,
    stage: String,
    is_final: bool,
    entrypoint: Vec<String>,
    cmd: Vec<String>,
    argv: Vec<String>,
    workdir: Option<String>,
    context: String,
    copies: Vec<(String, String)>,
    file: String,
    node: (usize, usize),
}

/// One repo's start seeds and package scripts.
#[derive(Default)]
struct StartSeeds {
    seeds: Vec<Seed>,
    /// package.json path -> its `(name, cmd)` scripts.
    pkg_scripts: BTreeMap<String, Vec<(String, String)>>,
}

impl StartSeeds {
    /// Read the start, build and package-script cells of `members` (graph
    /// then node then cell order) into seeds, as the host-start rules'
    /// `launches` compute them.
    fn collect(graphs: &[RepoGraph], members: &[usize]) -> Self {
        let mut out = StartSeeds::default();
        let mut docker: Vec<DockerCell> = Vec::new();
        let mut others: Vec<((usize, usize), serde_json::Value)> = Vec::new();
        let mut contexts: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut add_context = |file: &str, ctx: &str| {
            let list = contexts.entry(file.to_string()).or_default();
            let ctx = join_repo("", ctx).unwrap_or_default();
            if !list.contains(&ctx) {
                list.push(ctx);
            }
        };
        for &gi in members {
            let g = &graphs[gi];
            for (ni, n) in g.nodes.iter().enumerate() {
                let kind = g.nav.kind_by_id.get(&n.id).copied();
                for c in &n.cells {
                    if c.kind != cell_type::CODE {
                        continue;
                    }
                    let glia_core::CellPayload::Json(text) = &c.payload else {
                        continue;
                    };
                    if kind == Some(node_kind::MODULE) && text.starts_with(r#"{"pkg_scripts":"#) {
                        if let Some((file, scripts)) = read_pkg_scripts(text) {
                            out.pkg_scripts.entry(file).or_insert(scripts);
                        }
                        continue;
                    }
                    if kind != Some(node_kind::INFRA_RESOURCE)
                        || !(text.starts_with(r#"{"start":"#) || text.starts_with(r#"{"build":"#))
                    {
                        continue;
                    }
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
                        continue;
                    };
                    if let Some(b) = v.get("build") {
                        if let (Some(f), Some(ctx)) =
                            (str_at(b, "dockerfile"), str_at(b, "context"))
                        {
                            add_context(f, ctx);
                        }
                        continue;
                    }
                    let Some(st) = v.get("start") else {
                        continue;
                    };
                    let file = str_at(st, "decl")
                        .and_then(|d| d.rsplit_once(':'))
                        .map(|(f, _)| f.to_string())
                        .unwrap_or_default();
                    if str_at(st, "host") == Some("docker") {
                        if str_at(st, "trigger") != Some("container_start") {
                            continue;
                        }
                        let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
                        docker.push(DockerCell {
                            image: qname
                                .strip_prefix("infra:image:")
                                .unwrap_or(&qname)
                                .to_string(),
                            stage: str_at(st, "stage").unwrap_or_default().to_string(),
                            is_final: st.get("final").and_then(serde_json::Value::as_bool)
                                == Some(true),
                            entrypoint: strs_at(st, "entrypoint").unwrap_or_default(),
                            cmd: strs_at(st, "cmd").unwrap_or_default(),
                            argv: strs_at(st, "argv").unwrap_or_default(),
                            workdir: str_at(st, "workdir").map(str::to_string),
                            context: str_at(st, "context").unwrap_or(".").to_string(),
                            copies: st
                                .get("copies")
                                .and_then(serde_json::Value::as_array)
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|p| {
                                            Some((
                                                p.get(0)?.as_str()?.to_string(),
                                                p.get(1)?.as_str()?.to_string(),
                                            ))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                            file,
                            node: (gi, ni),
                        });
                        continue;
                    }
                    if str_at(st, "host") == Some("compose")
                        && let (Some(f), Some(ctx)) =
                            (str_at(st, "dockerfile"), str_at(st, "context"))
                    {
                        add_context(f, ctx);
                    }
                    others.push(((gi, ni), st.clone()));
                }
            }
        }
        let container = |d: &DockerCell, context: Option<&str>, workdir: Option<String>| {
            let mut bases: Vec<String> = Vec::new();
            let mut push = |b: &str| {
                let b = join_repo("", b).unwrap_or_default();
                if !bases.contains(&b) {
                    bases.push(b);
                }
            };
            push(context.unwrap_or(&d.context));
            for c in contexts.get(&d.file).into_iter().flatten() {
                push(c);
            }
            push("");
            ScriptEnv::Container {
                copies: d.copies.clone(),
                workdir: workdir.or_else(|| d.workdir.clone()),
                bases,
            }
        };
        for d in docker.iter().filter(|d| d.is_final) {
            out.seeds.push(Seed {
                node: d.node,
                argv: d.argv.clone(),
                env: container(d, None, None),
            });
        }
        for (node, st) in &others {
            let host = str_at(st, "host").unwrap_or_default();
            match host {
                "compose" | "k8s" => {
                    let (over_ep, over_cmd) = if host == "compose" {
                        (strs_at(st, "entrypoint"), strs_at(st, "command"))
                    } else {
                        (strs_at(st, "command"), strs_at(st, "args"))
                    };
                    if over_ep.is_none() && over_cmd.is_none() {
                        continue;
                    }
                    let name = match host {
                        "compose" => str_at(st, "built_image")
                            .map(str::to_string)
                            .or_else(|| str_at(st, "image").map(image_basename)),
                        _ => str_at(st, "image").map(image_basename),
                    };
                    let Some(name) = name else {
                        continue;
                    };
                    let dockerfile = str_at(st, "dockerfile");
                    let stage = str_at(st, "target_stage");
                    for d in docker.iter().filter(|d| {
                        d.image == name
                            && dockerfile.is_none_or(|f| f == d.file)
                            && stage.map_or(d.is_final, |s| s == d.stage)
                    }) {
                        let mut argv = over_ep.clone().unwrap_or_else(|| d.entrypoint.clone());
                        argv.extend(over_cmd.clone().unwrap_or_else(|| {
                            if over_ep.is_some() {
                                Vec::new()
                            } else {
                                d.cmd.clone()
                            }
                        }));
                        let workdir = str_at(st, "workdir").map(str::to_string);
                        out.seeds.push(Seed {
                            node: *node,
                            argv,
                            env: container(d, str_at(st, "context"), workdir),
                        });
                    }
                }
                "procfile" | "systemd" | "supervisor" => {
                    let dir = str_at(st, "dir").unwrap_or(".");
                    let base = match str_at(st, "workdir") {
                        Some(w) if w.starts_with('/') => continue,
                        Some(w) => join_repo(dir, w),
                        None => join_repo(dir, "."),
                    };
                    let Some(base) = base else {
                        continue;
                    };
                    out.seeds.push(Seed {
                        node: *node,
                        argv: strs_at(st, "argv").unwrap_or_default(),
                        env: ScriptEnv::Local { base },
                    });
                }
                _ => {}
            }
        }
        out
    }
}

/// Reads the entry scripts of one repo ([`attach_start_scripts`]).
struct ScriptReader<'a> {
    root: &'a std::path::Path,
    ignores: IgnoreStack,
    /// repo path -> its kept lines, `None` when not read (missing, refused,
    /// not a shell script, over the cap or past the budget). Looked up only.
    read: BTreeMap<String, Option<Vec<(String, u32)>>>,
    pkg_scripts: &'a BTreeMap<String, Vec<(String, String)>>,
}

impl ScriptReader<'_> {
    /// Follow `argv` from `env`, `depth` hops in, collecting each script
    /// line read as `(path, line, row)`.
    fn follow(&mut self, argv: &[String], env: &ScriptEnv, depth: usize, out: &mut Vec<ReadLine>) {
        let t = normalise_argv(argv);
        self.follow_target(&t, env, depth, out);
    }

    fn follow_target(
        &mut self,
        t: &StartTarget,
        env: &ScriptEnv,
        depth: usize,
        out: &mut Vec<ReadLine>,
    ) {
        if t.refused.is_some() || depth >= START_SCRIPT_HOPS {
            return;
        }
        if t.form == TargetForm::PkgScript {
            let mut cmds: Vec<String> = Vec::new();
            for m in env.manifests() {
                for (name, cmd) in self.pkg_scripts.get(&m).into_iter().flatten() {
                    if *name == t.target && !cmds.contains(cmd) {
                        cmds.push(cmd.clone());
                    }
                }
            }
            for cmd in cmds {
                let argv = ["sh".to_string(), "-c".to_string(), cmd];
                self.follow(&argv, env, depth + 1, out);
            }
            return;
        }
        let shell_launched =
            t.form == TargetForm::Script && t.launcher.is_some_and(|n| START_SHELLS.contains(&n));
        if !(shell_launched || (t.form == TargetForm::Binary && t.target.contains('/'))) {
            return;
        }
        for path in env.paths(&t.target) {
            let Some(lines) = self.read(&path) else {
                continue;
            };
            for (line, row) in lines {
                if !out
                    .iter()
                    .any(|(p, l, r)| *p == path && *l == line && *r == row)
                {
                    out.push((path.clone(), line.clone(), row));
                }
                let argv = ["sh".to_string(), "-c".to_string(), line];
                let next = normalise_argv(&argv);
                let passthrough = next.refused == Some(Refusal::Dynamic)
                    && matches!(next.target.as_str(), "$@" | "$*" | "${@}" | "${*}");
                let next = if passthrough {
                    normalise_argv(&t.args)
                } else {
                    next
                };
                self.follow_target(&next, env, depth + 1, out);
            }
        }
    }

    /// The kept lines of the script at repo path `path`, read once.
    fn read(&mut self, path: &str) -> Option<Vec<(String, u32)>> {
        if let Some(done) = self.read.get(path) {
            return done.clone();
        }
        let opened = self.read.values().filter(|v| v.is_some()).count();
        let lines = (opened < START_SCRIPT_BUDGET)
            .then(|| self.read_file(path))
            .flatten();
        self.read.insert(path.to_string(), lines.clone());
        lines
    }

    fn read_file(&self, path: &str) -> Option<Vec<(String, u32)>> {
        use glia_code_extractors::iac::{ENTRY_SCRIPT_CAP, entry_script_lines, is_entry_script};
        if !crate::walk::admits_start_script(self.root, &self.ignores, path) {
            return None;
        }
        let abs = self.root.join(path);
        let len = std::fs::metadata(&abs).ok()?.len();
        if usize::try_from(len).map_or(true, |l| l > ENTRY_SCRIPT_CAP) {
            return None;
        }
        let text = String::from_utf8(std::fs::read(&abs).ok()?).ok()?;
        is_entry_script(path, &text).then(|| entry_script_lines(&text))
    }
}

/// A package.json MODULE's `{"pkg_scripts":{"file","scripts":[{name,cmd}]}}`.
fn read_pkg_scripts(text: &str) -> Option<(String, Vec<(String, String)>)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let body = v.get("pkg_scripts")?;
    let file = str_at(body, "file")?.to_string();
    let scripts = body
        .get("scripts")?
        .as_array()?
        .iter()
        .filter_map(|s| {
            Some((
                str_at(s, "name")?.to_string(),
                str_at(s, "cmd")?.to_string(),
            ))
        })
        .collect();
    Some((file, scripts))
}

fn str_at<'v>(v: &'v serde_json::Value, key: &str) -> Option<&'v str> {
    v.get(key).and_then(serde_json::Value::as_str)
}

/// A string array field; `None` when absent or `null`.
fn strs_at(v: &serde_json::Value, key: &str) -> Option<Vec<String>> {
    let a = v.get(key)?.as_array()?;
    Some(
        a.iter()
            .filter_map(|s| s.as_str().map(str::to_string))
            .collect(),
    )
}

/// An image reference's repository basename (`ghcr.io/o/api:1.2` -> `api`).
fn image_basename(image: &str) -> String {
    let last = image.rsplit('/').next().unwrap_or(image);
    let last = last.split('@').next().unwrap_or(last);
    last.split(':').next().unwrap_or(last).to_string()
}

/// A container path, absolute and cleaned: `cand` when absolute, else joined
/// to the working directory (`None` when that is unknown).
fn container_path(cand: &str, workdir: Option<&str>) -> Option<String> {
    let joined = if cand.starts_with('/') {
        cand.to_string()
    } else {
        format!("{}/{cand}", workdir?)
    };
    Some(format!("/{}", join_repo("", &joined).unwrap_or_default()))
}

/// The build-context path a container path was copied from: the last COPY
/// whose destination holds it.
fn copied_from(copies: &[(String, String)], path: &str) -> Option<String> {
    copies.iter().rev().find_map(|(dst, src)| {
        let dst = format!("/{}", join_repo("", dst).unwrap_or_default());
        let rest = if path == dst {
            ""
        } else if dst == "/" {
            path.strip_prefix('/')?
        } else {
            path.strip_prefix(&dst)?.strip_prefix('/')?
        };
        Some(join_repo(src, rest).unwrap_or_default())
    })
}

/// `base` joined with `path` as a repo-relative path (`.` dropped, `..`
/// popped, `""` the root); `None` when it climbs above the root.
fn join_repo(base: &str, path: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod start_script_tests {
    use glia_code_domain::{cell_type, node_kind};
    use glia_core::CellPayload;

    fn write(root: &std::path::Path, rel: &str, text: &str) {
        let path = root.join(rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    /// `(host qname, script cell text)` of every entry-script cell.
    fn script_cells(result: &crate::build::GenerateResult) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for g in &result.merged.graphs {
            for n in &g.nodes {
                for c in &n.cells {
                    let CellPayload::Json(t) = &c.payload else {
                        continue;
                    };
                    if c.kind == cell_type::CODE && t.starts_with(r#"{"script":"#) {
                        assert_eq!(
                            g.nav.kind_by_id.get(&n.id),
                            Some(&node_kind::INFRA_RESOURCE)
                        );
                        let q = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
                        out.push((q, t.clone()));
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// GH.19b: only the scripts a start names are read, and of them only the
    /// exec / last-command line; a script the walk would skip is never read.
    #[test]
    fn the_build_reads_only_start_named_entry_scripts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "api/Dockerfile",
            "FROM python:3.12\nWORKDIR /code\nCOPY . .\nCMD [\"./bin/start.sh\"]\n",
        );
        write(
            root,
            "api/bin/start.sh",
            "#!/bin/sh\nset -e\npython -m app.seed\nexec ./bin/next.sh\n",
        );
        write(
            root,
            "api/bin/next.sh",
            "#!/bin/sh\nexec python -m app.main\n",
        );
        write(
            root,
            "api/bin/unused.sh",
            "#!/bin/sh\nexec python -m app.unused\n",
        );
        write(root, "api/app/__init__.py", "");
        write(root, "api/app/main.py", "def main():\n    pass\n");
        write(
            root,
            "docker-compose.yml",
            "services:\n  hidden:\n    build: ./api\n    command: ./private/run.sh\n",
        );
        // A gitignored script a start names is refused, like the walk.
        write(root, "api/.gitignore", "private/\n");
        write(
            root,
            "api/private/run.sh",
            "#!/bin/sh\nexec python -m app.main\n",
        );
        let result = crate::build::generate_one(root.to_str().unwrap()).unwrap();
        assert_eq!(
            script_cells(&result),
            [
                (
                    "infra:image:api".to_string(),
                    r#"{"script":"api/bin/next.sh","exec":"exec python -m app.main","row":2}"#
                        .to_string()
                ),
                (
                    "infra:image:api".to_string(),
                    r#"{"script":"api/bin/start.sh","exec":"exec ./bin/next.sh","row":4}"#
                        .to_string()
                ),
            ]
        );
        // A second finish over the same graphs replaces, never duplicates.
        let mut graphs = result.merged.graphs.clone();
        let inputs = vec![crate::external::RepoInputs {
            repo: graphs[0].repo,
            root: root.to_path_buf(),
            label: root.to_string_lossy().into_owned(),
            config: None,
        }];
        super::attach_start_scripts(&mut graphs, &inputs);
        let again: usize = graphs
            .iter()
            .flat_map(|g| g.nodes.iter())
            .flat_map(|n| n.cells.iter())
            .filter(|c| glia_code_extractors::iac::parse_entry_script_cell(c).is_some())
            .count();
        assert_eq!(again, 2);
    }
}
