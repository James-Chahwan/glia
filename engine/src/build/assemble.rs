//! Per-repo graph assembly: `build_graphs_for_repo` and the facts and markers
//! it owns (the A11.1 const table and its LF.2d overlay pins, the A12.1
//! `[msgtype]` census, the panic-hook guard that covers the whole assembly).

use std::panic::{AssertUnwindSafe, catch_unwind};

use repo_graph_code_domain::endpoint::split_owner;
use repo_graph_code_domain::glia_config::LoadedConfig;
use repo_graph_code_domain::project_roots::ProjectRoot;
use repo_graph_code_domain::{cell_type, di_stats, node_kind};
use repo_graph_code_extractors::constants::ConstTable;
use repo_graph_core::RepoId;

use super::BuildOptions;
use super::grafts;
use super::lang_build;
use super::rpc_needles::RpcContext;
use crate::cache::ParseCache;
use crate::extract::detect_language;
use crate::route::parse_repo_files;

/// The repo-scope name -> literal table (A11.1), built from every walked file
/// with a source language. Like `read_go_module_prefix` it is a cross-file
/// fact the engine gathers, but unlike the go.mod prefix it is NOT
/// handed to the per-file extractors: their output is cached by the file's own
/// content hash, and a table lookup depends on other files. A consumer runs
/// after the cache, as `apply_rpc_needles` does, so incremental == clean.
///
/// `files` is name-sorted by the walk and the table is first-wins, so the
/// result does not depend on the process. `.env` / yaml / Dockerfile have no
/// source language and never reach the scan (env values are A13.7's ENV cell).
fn build_const_table(files: &[(String, String)], parse_errors: &mut Vec<String>) -> ConstTable {
    let mut table = ConstTable::default();
    for (path, source) in files {
        let Some(lang) = detect_language(path) else { continue };
        match catch_unwind(AssertUnwindSafe(|| ConstTable::scan_file(source, lang))) {
            Ok(file_table) => table.merge_from(&file_table),
            Err(_) => parse_errors.push(format!("{path}: PANIC (const table scan)")),
        }
    }
    table
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

/// `repo_label` is the repo path as the caller was given it. It only prefixes
/// the `[incremental]` marker, so a multi-repo build prints one attributable
/// line per repo; it never reaches the graph. `rpc` is the build-wide proto
/// service set (A5.2). `roots` are the walk's project roots (A8.4), the owner
/// vocabulary of the LB.4a HTTP owner segment. `config` is the repo's loaded
/// `.glia/overlay.toml` (`RepoInputs::config`) and `opts` the build's options:
/// together they decide the overlay stages that run before the graph is built
/// (LF.2d's constant pins; LF.2e's `[[wrapper]]` call sites, minted inside
/// `grafts::apply_post_cache`).
#[allow(clippy::too_many_arguments)]
pub(super) fn build_graphs_for_repo(
    files: &[(String, String)],
    repo: RepoId,
    go_module_prefix: &str,
    cache: Option<&mut ParseCache>,
    repo_label: &str,
    rpc: &RpcContext,
    roots: &[ProjectRoot],
    config: Option<&LoadedConfig>,
    opts: &BuildOptions,
) -> (Vec<repo_graph_graph::RepoGraph>, Vec<String>) {
    // Suppress the default panic-print-to-stderr while we run per-file parsers
    // — we catch panics below and report them as parse_errors. The default
    // hook would otherwise spam stderr (with a backtrace) for every bad file
    // even though we recover. Restored on scope exit via Drop guard so a
    // panic in non-loop code still gets the user-visible report.
    let _hook_guard = SuppressPanicHook::install();
    // A7.0: shape counters describe only the detectors that run in THIS build.
    di_stats::reset();

    let (mut parses_by_lang, mut parse_errors) =
        parse_repo_files(files, repo, go_module_prefix, cache, repo_label);

    // A11.1 fired_on marker, once per repo. Post-cache passes that read the
    // table (A11.2 endpoint fold, queue-topic const fold) take `&const_table`
    // and live in `grafts::apply_post_cache`, beside `apply_rpc_needles`.
    let mut const_table = build_const_table(files, &mut parse_errors);
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
        config.filter(|_| opts.overlay),
        &mut parse_errors,
        repo_label,
    );

    let (graphs, di_refs) = lang_build::build_language_graphs(
        parses_by_lang,
        repo,
        repo_label,
        &rust_crates,
        &mut parse_errors,
    );

    // A7.0 fired_on marker, once per repo: `[di] injects refs: … repo=<label>`.
    di_stats::flush_marker(&di_refs, repo_label);
    msgtype_marker(&graphs, repo_label);

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

/// RAII guard: replaces the global panic hook with a no-op for the lifetime
/// of the guard, then restores. Used by `build_graphs_for_repo` so caught
/// per-file panics don't flood stderr with backtraces. Process-global state,
/// so this assumes single-threaded parsing (true today). If parsing ever
/// goes parallel, switch to `panic::update_hook` filtering by thread.
struct SuppressPanicHook {
    prev: Option<Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>>,
}

impl SuppressPanicHook {
    fn install() -> Self {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        Self { prev: Some(prev) }
    }
}

impl Drop for SuppressPanicHook {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            std::panic::set_hook(prev);
        }
    }
}
