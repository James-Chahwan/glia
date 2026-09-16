//! Per-file routing: which extractor or language parser sees which file.
//! Holds the non-source branches (yaml / Dockerfile / package manifest /
//! dotenv / `.proto`), the WP-D incremental parse-cache lookup, and the
//! per-file panic isolation. Split out of `build_graphs_for_repo`.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use repo_graph_code_domain::{CodeNav, FileParse, GRAPH_TYPE, node_kind};
use repo_graph_core::{Cell, Confidence, Edge, Node, NodeId, RepoId};

use crate::cache::{self, ParseCache};
use crate::extract::{
    apply_cross_cutting_extractors, detect_language, merge_nav, parse_one_with, path_to_qname,
};
use crate::walk::{is_dockerfile_path, is_dotenv_path};

/// The per-file half of `build_graphs_for_repo`: route every walked file to
/// its parser or synthetic extractor and return the parses grouped by
/// language tag and the per-file errors. `.proto` files are no longer a
/// separate bucket — they stash under the `"proto"` lang tag like any other
/// synthetic parse (A5.1), so they get a MODULE node and a file position.
pub(crate) fn parse_repo_files(
    files: &[(String, String)],
    repo: RepoId,
    go_module_prefix: &str,
    mut cache: Option<&mut ParseCache>,
) -> (HashMap<&'static str, Vec<FileParse>>, Vec<String>) {
    let mut parses_by_lang: HashMap<&str, Vec<FileParse>> = HashMap::new();
    let mut parse_errors = Vec::new();
    // A5.1 `[proto]` marker counters.
    let mut proto_files = 0usize;
    let mut proto_services = 0usize;
    let mut proto_rpcs = 0usize;
    let mut proto_packages = 0usize;
    // WP-D incremental: track which main-parser files we saw so deleted files
    // get evicted; count reuse vs reparse for the marker.
    let mut live_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut reused = 0usize;
    let mut reparsed = 0usize;

    for (path, source) in files {
        let yaml_ext = matches!(
            std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str()),
            Some("yml" | "yaml")
        );
        if yaml_ext {
            let module_id = NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::MODULE,
                &path_to_qname(path),
            );
            let cron_out = repo_graph_code_extractors::cron::extract_cron_nodes(
                source, path, module_id, repo,
            );
            let cfg_out = repo_graph_code_extractors::config::extract_yaml_env_defs(
                source, module_id, repo,
            );
            let iac_out =
                repo_graph_code_extractors::iac::extract_yaml(source, module_id, repo);
            if !cron_out.nodes.is_empty()
                || !cfg_out.nodes.is_empty()
                || !iac_out.nodes.is_empty()
            {
                stash_synthetic_parse(
                    "yaml",
                    path,
                    module_id,
                    repo,
                    vec![cron_out.nodes, cfg_out.nodes, iac_out.nodes],
                    vec![cron_out.edges, cfg_out.edges, iac_out.edges],
                    vec![cron_out.nav, cfg_out.nav, iac_out.nav],
                    vec![],
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        if is_dockerfile_path(path) {
            let module_id = NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::MODULE,
                &path_to_qname(path),
            );
            let cfg_out = repo_graph_code_extractors::config::extract_dockerfile_defs(
                source, module_id, repo,
            );
            let iac_out = repo_graph_code_extractors::iac::extract_dockerfile(
                source, path, module_id, repo,
            );
            if !cfg_out.nodes.is_empty() || !iac_out.nodes.is_empty() {
                stash_synthetic_parse(
                    "dockerfile",
                    path,
                    module_id,
                    repo,
                    vec![cfg_out.nodes, iac_out.nodes],
                    vec![cfg_out.edges, iac_out.edges],
                    vec![cfg_out.nav, iac_out.nav],
                    vec![],
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        if repo_graph_code_extractors::packages::is_manifest_path(path) {
            let module_id = NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::MODULE,
                &path_to_qname(path),
            );
            let pkg_out = repo_graph_code_extractors::packages::extract_for_path(
                source, path, module_id, repo,
            );
            if !pkg_out.nodes.is_empty() {
                stash_synthetic_parse(
                    "manifest",
                    path,
                    module_id,
                    repo,
                    vec![pkg_out.nodes],
                    vec![pkg_out.edges],
                    vec![pkg_out.nav],
                    vec![],
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        if is_dotenv_path(path) {
            let module_id = NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::MODULE,
                &path_to_qname(path),
            );
            let cfg_out = repo_graph_code_extractors::config::extract_dotenv_defs(
                source, module_id, repo,
            );
            if !cfg_out.nodes.is_empty() {
                stash_synthetic_parse(
                    "dotenv",
                    path,
                    module_id,
                    repo,
                    vec![cfg_out.nodes],
                    vec![cfg_out.edges],
                    vec![cfg_out.nav],
                    vec![],
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        let Some(lang) = detect_language(path) else { continue };

        if lang == "proto" {
            let module_id = NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::MODULE,
                &path_to_qname(path),
            );
            let out = repo_graph_code_extractors::grpc::extract_grpc_service_nodes(
                source, path, module_id, repo,
            );
            proto_files += 1;
            proto_services += out.service_count;
            proto_rpcs += out.rpc_count;
            if out.package.is_some() {
                proto_packages += 1;
            }
            if !out.nodes.is_empty() {
                // Same synthetic path as yaml / Dockerfile / manifest / dotenv:
                // the file itself becomes a MODULE (with a POSITION cell) that
                // parents the GRPC_SERVICE, so `locate_node` / `docs-for` can
                // answer "where is this service declared".
                stash_synthetic_parse(
                    "proto",
                    path,
                    module_id,
                    repo,
                    vec![out.nodes],
                    vec![out.edges],
                    vec![out.nav],
                    out.module_cells,
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        // WP-D incremental: reuse the cached parse if the source is unchanged;
        // only changed / new files pay tree-sitter.
        let hash = cache.is_some().then(|| cache::content_hash(source));
        let cached_fp = match hash {
            Some(h) => cache.as_deref().and_then(|c| c.get(path, h, lang)),
            None => None,
        };
        if let Some(fp) = cached_fp {
            reused += 1;
            live_paths.insert(path.clone());
            parses_by_lang.entry(lang).or_default().push(fp);
            continue;
        }

        // Per-file panic isolation. Parsers occasionally hit slice/regex bugs
        // on adversarial inputs (e.g. parsers/code/rust/src/lib.rs:511 slice
        // OOB on glia's own source as of 2026-05-09). One bad file shouldn't
        // kill an N-file repo build — log it, skip it, keep going.
        let parse_result = catch_unwind(AssertUnwindSafe(|| {
            let mut fp = parse_one_with(source, path, lang, repo, go_module_prefix)?;
            let module_id = NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::MODULE,
                &path_to_qname(path),
            );
            apply_cross_cutting_extractors(&mut fp, source, path, lang, module_id, repo);
            // G15: denormalize the file's external library names onto every node
            // as an IMPORTS cell (one place, all languages).
            repo_graph_code_domain::attach_imports_cell(&mut fp, lang);
            Ok::<_, String>(fp)
        }));
        match parse_result {
            Ok(Ok(fp)) => {
                reparsed += 1;
                if let (Some(c), Some(h)) = (cache.as_deref_mut(), hash) {
                    c.put(path.clone(), h, lang, fp.clone());
                }
                live_paths.insert(path.clone());
                parses_by_lang.entry(lang).or_default().push(fp);
            }
            Ok(Err(e)) => {
                parse_errors.push(format!("{path}: {e}"));
            }
            Err(payload) => {
                parse_errors.push(format!(
                    "{path}: PANIC ({lang} parser/extractors): {}",
                    panic_payload_str(&payload)
                ));
            }
        }
    }

    // WP-D: evict cached parses for files gone this build, and emit the
    // greppable marker so a cycle can confirm the cache engaged.
    if let Some(c) = cache.as_deref_mut() {
        c.retain_paths(&live_paths);
        c.stats.reused = reused;
        c.stats.reparsed = reparsed;
        // `stamp=` makes the always-on line grep-proof of WHICH code produced
        // these parses — a rebuilt wheel that quietly kept the old .so shows the
        // old stamp here (dev-notes memory: feedback_maturin_stale_wheel).
        eprintln!(
            "[incremental] reused {reused}, reparsed {reparsed}, evicted {} (parse cache, stamp={})",
            c.stats.evicted,
            cache::CACHE_VERSION
        );
    }

    // A5.1 fired_on marker: a `.proto` is now a first-class parsed file, not a
    // floating service node. Only printed when a build actually saw one.
    if proto_files > 0 {
        eprintln!(
            "[proto] {proto_files} files -> {proto_services} services, {proto_rpcs} rpcs, {proto_packages} packages"
        );
    }

    (parses_by_lang, parse_errors)
}

/// Best-effort string extraction from a panic payload returned by
/// `catch_unwind`. The payload is `Box<dyn Any + Send>` and the message is
/// commonly a `&'static str` (from `panic!("literal")`) or `String` (from
/// `panic!("{}", x)` / `unwrap`).
fn panic_payload_str(payload: &Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic>".to_string()
    }
}

fn stash_synthetic_parse(
    lang_key: &'static str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    node_groups: Vec<Vec<Node>>,
    edge_groups: Vec<Vec<Edge>>,
    nav_groups: Vec<CodeNav>,
    // Cells for the synthetic MODULE node itself (e.g. a `.proto`'s whole-file
    // POSITION). Empty for the extractors that have nothing to say about the
    // file as a whole.
    module_cells: Vec<Cell>,
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
) {
    let mut nodes = vec![Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: module_cells,
    }];
    let mut edges = Vec::new();
    let mut merged_nav = CodeNav::default();
    merged_nav.record(
        module_id,
        path.rsplit('/').next().unwrap_or(path),
        &path_to_qname(path),
        node_kind::MODULE,
        None,
    );
    for group in node_groups {
        nodes.extend(group);
    }
    for group in edge_groups {
        edges.extend(group);
    }
    for nav in nav_groups {
        merge_nav(&mut merged_nav, nav);
    }
    let fp = FileParse {
        nodes,
        edges,
        nav: merged_nav,
        ..Default::default()
    };
    parses_by_lang.entry(lang_key).or_default().push(fp);
}
