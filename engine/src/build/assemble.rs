//! Per-repo graph assembly: `build_graphs_for_repo` and the facts and markers
//! it owns (the A11.1 const table, the A12.1 `[msgtype]` census, the panic-hook
//! guard that covers the whole assembly).

use std::panic::{AssertUnwindSafe, catch_unwind};

use repo_graph_code_domain::project_roots::ProjectRoot;
use repo_graph_code_domain::{cell_type, di_stats, node_kind};
use repo_graph_code_extractors::constants::ConstTable;
use repo_graph_core::RepoId;

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

/// `repo_label` is the repo path as the caller was given it. It only prefixes
/// the `[incremental]` marker, so a multi-repo build prints one attributable
/// line per repo; it never reaches the graph. `rpc` is the build-wide proto
/// service set (A5.2). `roots` are the walk's project roots (A8.4), the owner
/// vocabulary of the LB.4a HTTP owner segment.
pub(super) fn build_graphs_for_repo(
    files: &[(String, String)],
    repo: RepoId,
    go_module_prefix: &str,
    cache: Option<&mut ParseCache>,
    repo_label: &str,
    rpc: &RpcContext,
    roots: &[ProjectRoot],
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
    let const_table = build_const_table(files, &mut parse_errors);
    if !const_table.is_empty() {
        eprintln!(
            "[const] repo table: {} bindings from {} files ({} conflicts) repo={repo_label}",
            const_table.len(),
            const_table.files(),
            const_table.conflicts()
        );
    }
    // A11.2, LB.4a, LA.4, A5.2 / A5.3, A5.8, A16.4: the post-cache grafts,
    // in that order.
    grafts::apply_post_cache(
        &mut parses_by_lang,
        files,
        repo,
        rpc,
        &const_table,
        roots,
        &mut parse_errors,
        repo_label,
    );

    let (graphs, di_refs) =
        lang_build::build_language_graphs(parses_by_lang, repo, &mut parse_errors);

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
            if let Some(q) = g.nav.qname_by_id.get(&n.id)
                && let Some((_, topic)) = q.split_once(':')
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
