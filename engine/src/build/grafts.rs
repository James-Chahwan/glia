//! The post-cache graft sequence of `build_graphs_for_repo`. Every pass here
//! reads something that is not a function of one file's content (the repo's
//! const table, the build's proto services, the whole repo's declarations), so
//! it runs on the router's output — cached parses included — and never inside
//! the per-file extractors. That keeps incremental == clean.

use std::collections::HashMap;

use repo_graph_code_domain::{
    FileParse, LocalModuleIndex, attach_imports_cell_filtered, cell_type, node_kind,
};
use repo_graph_code_extractors::anchor;
use repo_graph_code_extractors::constants::ConstTable;
use repo_graph_core::RepoId;

use super::rpc_needles::{RpcContext, apply_rpc_needles};
use crate::endpoint_fold;

/// Run every post-cache graft over one repo's parses, in order: the A11.2
/// endpoint fold, the A5.2 / A5.3 RPC needles with their `[grpc-client]` /
/// `[grpc-server-impl]` markers, the A5.8 `[marker-anchor]` census, then the
/// A16.4 IMPORTS-cell filter. `const_table` is the repo's A11.1 table.
pub(super) fn apply_post_cache(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    repo: RepoId,
    rpc: &RpcContext,
    const_table: &ConstTable,
    parse_errors: &mut Vec<String>,
    repo_label: &str,
) {
    // A11.2: re-key client ENDPOINTs whose base the table resolves and record
    // their authority. Post-cache, so cached parses are folded too and the
    // cache keeps the pre-fold parse.
    endpoint_fold::fold_repo(parses_by_lang.values_mut().flatten(), const_table, repo)
        .report(repo_label);

    let rpc_added = apply_rpc_needles(parses_by_lang, files, repo, rpc, parse_errors);
    // A5.2 fired_on marker, once per repo. Printed whenever the build knows a
    // proto service or this repo holds a client stub.
    let grpc_clients = parses_by_lang
        .values()
        .flatten()
        .flat_map(|fp| fp.nav.kind_by_id.values())
        .filter(|k| **k == node_kind::GRPC_CLIENT)
        .count();
    if grpc_clients > 0 || !rpc.services.is_empty() {
        eprintln!(
            "[grpc-client] {grpc_clients} stubs from {} known services (proto-needle +{}) repo={repo_label}",
            rpc.services.len(),
            rpc_added.clients
        );
    }
    // A5.3 per-repo extraction marker; the resolver's `[grpc-server]` line
    // reports the pairing.
    if rpc_added.servers > 0 {
        eprintln!(
            "[grpc-server-impl] {} server markers from {} known services repo={repo_label}",
            rpc_added.servers,
            rpc.services.len()
        );
    }
    // A5.8 fired_on marker, once per repo that holds an RPC-family marker node:
    //   `[marker-anchor] {a} anchored to methods, {m} to module, {u} unanchored repo=<label>`
    // Counted off the finished parses, so cache-served files count too.
    let mut anchored = anchor::AnchorStats::default();
    for fp in parses_by_lang.values().flatten() {
        anchored.add(anchor::census(fp));
    }
    anchor::report(anchored, repo_label);

    // A16.4: drop intra-repo names from every IMPORTS cell. It needs the whole
    // repo's declarations, so like `apply_rpc_needles` it runs after the parse
    // cache (cached parses are filtered too) and after the RPC grafts (whose
    // markers carry the same cell).
    filter_imports_cells(parses_by_lang, repo_label);
}

/// A16.4 (audit 2026-06-10 #12): rewrite each file's IMPORTS cell without the
/// names that resolve inside this repo — a sibling module, the repo's own
/// package or namespace — which are not dependencies.
///
/// The router attaches the raw cell to every node of a language-parser parse,
/// and never to a synthetic one (yaml / proto / graphql / avro / …), so
/// "carries an IMPORTS cell" selects exactly the parses that take part. That
/// keeps synthetic parses out on both sides: their file-derived modules
/// (`config::logging`) must not shadow a real dependency, and their nodes must
/// not gain a cell they never had. Rewriting in place keeps the cell's slot, so
/// every node's cell order is unchanged.
///
/// fired_on marker, once per repo that holds a language-parser parse:
///   `[imports] local-filter: kept {k}, dropped {d} intra-repo name(s) across {n} language group(s) repo=<label>`
/// `kept` / `dropped` sum the per-file library names; `n` counts lang tags.
fn filter_imports_cells(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    repo_label: &str,
) {
    let carries_imports = |fp: &FileParse| {
        fp.nodes
            .iter()
            .any(|n| n.cells.iter().any(|c| c.kind == cell_type::IMPORTS))
    };
    let mut local = LocalModuleIndex::default();
    for fp in parses_by_lang.values().flatten().filter(|fp| carries_imports(fp)) {
        local.add_parse(fp);
    }
    let (mut kept, mut dropped, mut groups) = (0usize, 0usize, 0usize);
    for (lang, parses) in parses_by_lang.iter_mut() {
        let mut filtered_any = false;
        for fp in parses.iter_mut().filter(|fp| carries_imports(fp)) {
            let (k, d) = attach_imports_cell_filtered(fp, lang, &local);
            kept += k;
            dropped += d;
            filtered_any = true;
        }
        groups += usize::from(filtered_any);
    }
    if groups > 0 {
        eprintln!(
            "[imports] local-filter: kept {kept}, dropped {dropped} intra-repo name(s) across {groups} language group(s) repo={repo_label}"
        );
    }
}
