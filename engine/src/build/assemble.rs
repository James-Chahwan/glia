//! Per-repo graph assembly: `build_graphs_for_repo` and the facts and markers
//! it owns (the A11.1 const table and its LF.2d overlay pins, the A12.1
//! `[msgtype]` census, the quiet-panic scope that covers the whole assembly).

use repo_graph_code_domain::endpoint::split_owner;
use repo_graph_code_domain::glia_config::LoadedConfig;
use repo_graph_code_domain::project_roots::ProjectRoot;
use repo_graph_code_domain::{cell_type, di_stats, node_kind};
use repo_graph_code_extractors::constants::ConstTable;
use repo_graph_core::RepoId;

use super::BuildOptions;
use super::grafts;
use super::lang_build::{self, TsAliasSet};
use super::rpc_needles::RpcContext;
use crate::cache::ParseCache;
use crate::extract::{GoModules, detect_language};
use crate::route::parse_repo_files;

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
/// table, the files scanned and the pool's thread count, for the
/// `[parallel]` line.
fn build_const_table(
    files: &[(String, String)],
    parse_errors: &mut Vec<String>,
) -> (ConstTable, usize, usize) {
    let (scans, threads) = crate::parallel::par_map_ordered(files, |(path, source)| {
        let lang = detect_language(path)?;
        Some(crate::parallel::quiet(|| {
            ConstTable::scan_file(source, lang)
        }))
    });
    let mut table = ConstTable::default();
    let mut scanned = 0usize;
    for ((path, _), scan) in files.iter().zip(scans) {
        let Some(scan) = scan else { continue };
        scanned += 1;
        match scan {
            Ok(file_table) => table.merge_from(&file_table),
            Err(_) => parse_errors.push(format!("{path}: PANIC (const table scan)")),
        }
    }
    (table, scanned, threads)
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
pub(super) struct RepoBuildCtx<'a> {
    pub(super) repo: RepoId,
    /// The repo path as the caller was given it. It only labels markers
    /// (`[incremental]`, `[const]`, ...), so a multi-repo build prints one
    /// attributable line per repo; it never reaches the graph.
    pub(super) repo_label: &'a str,
    /// The repo's go.mod set (LA.13), handed to every Go parse.
    pub(super) go: &'a GoModules,
    /// The repo's tsconfig `paths` aliases per project dir (A6.8): the TS
    /// family's import resolver and the IMPORTS-cell filter read them.
    pub(super) ts_aliases: &'a TsAliasSet,
    /// The build-wide proto service set (A5.2).
    pub(super) rpc: &'a RpcContext,
    /// The walk's project roots (A8.4), the owner vocabulary of the LB.4a HTTP
    /// owner segment.
    pub(super) roots: &'a [ProjectRoot],
    /// The repo's loaded `.glia/overlay.toml` (`RepoInputs::config`). With
    /// `opts` it decides the overlay stages that run before the graph is built
    /// (LF.2d's constant pins; LF.2e's `[[wrapper]]` call sites, minted inside
    /// `grafts::apply_post_cache`).
    pub(super) config: Option<&'a LoadedConfig>,
    pub(super) opts: &'a BuildOptions,
}

/// Parse, graft and build one repo's graphs from its walked `files`, reusing
/// `cache` when given, under the per-repo context `ctx` ([`RepoBuildCtx`]).
pub(super) fn build_graphs_for_repo(
    files: &[(String, String)],
    cache: Option<&mut ParseCache>,
    ctx: &RepoBuildCtx<'_>,
) -> (Vec<repo_graph_graph::RepoGraph>, Vec<String>) {
    let RepoBuildCtx { repo, repo_label, go, ts_aliases, rpc, roots, config, opts } = *ctx;
    // Keep caught per-file panics off stderr: the default hook would print
    // (with a backtrace) for every bad file even though it becomes a
    // parse_errors line. LG.1a: the flag is per thread (`parallel`), so this
    // scope covers every `catch_unwind` the assembly runs on THIS thread
    // (grafts, the overlay wrapper stage, the needle passes), the pool
    // workers go through `parallel::quiet`, and a panic on any other thread
    // of the process still reaches the original hook.
    let _quiet = crate::parallel::quiet_scope();
    // A7.0: shape counters describe only the detectors that run in THIS build.
    di_stats::reset();

    let (mut parses_by_lang, mut parse_errors) =
        parse_repo_files(files, repo, go, cache, repo_label);

    // A11.1 fired_on marker, once per repo. Post-cache passes that read the
    // table (A11.2 endpoint fold, queue-topic const fold) take `&const_table`
    // and live in `grafts::apply_post_cache`, beside `apply_rpc_needles`.
    let (mut const_table, const_files, threads) = build_const_table(files, &mut parse_errors);
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
    // LA.1a / LA.1b: the Cargo packages, read by the A16.4 IMPORTS filter
    // (a sibling crate is not a dependency) and by `build_rust`.
    let rust_crates = lang_build::rust_crates(files, roots);
    // LF.2e (http), A11.2, LA.6d, LA.4, LF.2e (queue), A5.2 / A5.3, A5.8,
    // LB.4a / LB.8, A16.4: the post-cache grafts, in that order. The overlay
    // `[[wrapper]]` stage runs only when the build applies the overlay.
    grafts::apply_post_cache(
        &mut parses_by_lang,
        files,
        repo,
        rpc,
        &const_table,
        roots,
        &rust_crates,
        ts_aliases,
        config.filter(|_| opts.overlay),
        &mut parse_errors,
        repo_label,
    );

    let (graphs, di_refs) = lang_build::build_language_graphs(
        parses_by_lang,
        repo,
        repo_label,
        &rust_crates,
        ts_aliases,
        &mut parse_errors,
    );

    // A7.0 fired_on marker, once per repo: `[di] injects refs: … repo=<label>`.
    di_stats::flush_marker(&di_refs, repo_label);
    msgtype_marker(&graphs, repo_label);
    // LG.1b fired_on marker, once per repo, after its graphs are built:
    //   `[parallel] <repo>: const-scan <c> files, rpc-needles <r> files on <t> threads`
    // `c` = files the A11.1 const-table scan read (every file with a source
    // language), `r` = files the RPC needle pass ran on (text-gated, with a
    // parse; 0 when the build knows no proto service), `t` = the pool that ran
    // both. The per-language graph builds stay sequential: `recv_stats` (the
    // A6.2a `[recv]` counter) is one process-global count taken after each
    // language's build, so concurrent builds would mix its per-language counts.
    eprintln!(
        "[parallel] {repo_label}: const-scan {const_files} files, rpc-needles {} files on {threads} threads",
        rpc.take_needle_files()
    );

    (graphs, parse_errors)
}

/// A12.1 fired_on marker, once per repo that holds a queue node:
///   `[msgtype] queue_nodes=N typed=T tag_topics=K repo=<label>`
/// `typed` counts nodes carrying a `cell_type::MESSAGE_TYPE` cell, `tag_topics`
/// the identity-free framework tags that can never carry a contract. Counted
/// off the built graphs, so cache-served files count too; silent otherwise.
fn msgtype_marker(graphs: &[repo_graph_graph::RepoGraph], repo_label: &str) {
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
                && repo_graph_code_extractors::queues::is_framework_tag(topic)
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
