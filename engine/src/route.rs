//! Per-file routing: which extractor or language parser sees which file.
//! Holds the non-source branches (yaml / Dockerfile / package manifest /
//! dotenv / contract JSON / `.proto` / `.graphql`), the WP-D incremental parse-cache
//! lookup, and the per-file panic isolation. Split out of
//! `build_graphs_for_repo`.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use repo_graph_code_domain::{CodeNav, FileParse, GRAPH_TYPE, node_kind};
use repo_graph_core::{Cell, Confidence, Edge, Node, NodeId, RepoId};

use crate::cache::{self, ParseCache};
use crate::extract::{
    ExtractStats, apply_cross_cutting_extractors, detect_language, merge_nav, parse_one_with,
    path_to_qname,
};
use crate::walk::{is_dockerfile_path, is_dotenv_path};

/// The per-file half of `build_graphs_for_repo`: route every walked file to
/// its parser or synthetic extractor and return the parses grouped by
/// language tag and the per-file errors. `.proto` files are no longer a
/// separate bucket — they stash under the `"proto"` lang tag like any other
/// synthetic parse (A5.1), so they get a MODULE node and a file position.
/// `repo_label` only prefixes the `[incremental]` marker (A1.4).
pub(crate) fn parse_repo_files(
    files: &[(String, String)],
    repo: RepoId,
    go_module_prefix: &str,
    mut cache: Option<&mut ParseCache>,
    repo_label: &str,
) -> (HashMap<&'static str, Vec<FileParse>>, Vec<String>) {
    let mut parses_by_lang: HashMap<&str, Vec<FileParse>> = HashMap::new();
    let mut parse_errors = Vec::new();
    // A5.1 `[proto]` marker counters.
    let mut proto_files = 0usize;
    let mut proto_services = 0usize;
    let mut proto_rpcs = 0usize;
    let mut proto_packages = 0usize;
    // A10.5 `[proto] files=` marker counters.
    let mut proto_messages = 0usize;
    let mut proto_enums = 0usize;
    // A10.4 `[graphql-sdl]` marker counters.
    let mut sdl_files = 0usize;
    let mut sdl_resolvers = 0usize;
    // A10.1 `[contract]` marker counters: yaml (A10.1 / A10.3) and sniffed
    // JSON (A10.8) contracts both fold in through `ContractCounts::record`.
    let mut contracts = repo_graph_code_extractors::contracts::ContractCounts::default();
    // WP-D incremental: track which main-parser files we saw so deleted files
    // get evicted; count reuse vs reparse for the marker.
    let mut live_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut reused = 0usize;
    let mut reparsed = 0usize;
    // A3.4 `[extract]` marker counter. Only counts files actually reparsed this
    // build — a cache hit replays a FileParse whose ROUTE nodes already carry
    // the mark, so the graph-side `[http]` marker is the complete figure.
    let mut nav_routes_marked = 0usize;

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
            // A10.1: an `openapi.yaml` / `swagger.yaml` declares the service's
            // API surface. Non-contract yaml takes a cheap sniff-miss here.
            let contract_out = repo_graph_code_extractors::contracts::extract_yaml_contracts(
                source, path, module_id, repo,
            );
            // A10.3: the same call also covers `asyncapi.yaml`; `record`
            // routes the count to the format the file sniffed as.
            contracts.record(&contract_out);
            if !cron_out.nodes.is_empty()
                || !cfg_out.nodes.is_empty()
                || !iac_out.nodes.is_empty()
                || !contract_out.nodes.is_empty()
            {
                stash_synthetic_parse(
                    "yaml",
                    path,
                    module_id,
                    repo,
                    vec![cron_out.nodes, cfg_out.nodes, iac_out.nodes, contract_out.nodes],
                    vec![cron_out.edges, cfg_out.edges, iac_out.edges, contract_out.edges],
                    vec![cron_out.nav, cfg_out.nav, iac_out.nav, contract_out.nav],
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

        // A10.8: the walker queues a `.json` only when it sniffed as an API
        // contract. After the manifest branch, so package.json / composer.json
        // never land here; before detect_language, which has no json arm.
        let json_ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if json_ext {
            let module_id = NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::MODULE,
                &path_to_qname(path),
            );
            let out = repo_graph_code_extractors::contracts::extract_json_contract(
                source, path, module_id, repo,
            );
            contracts.record(&out);
            if !out.nodes.is_empty() {
                stash_synthetic_parse(
                    "json",
                    path,
                    module_id,
                    repo,
                    vec![out.nodes],
                    vec![out.edges],
                    vec![out.nav],
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
            // A10.5: the file's `message` / `enum` declarations, as
            // MESSAGE_TYPE nodes under the same MODULE.
            let msgs = repo_graph_code_extractors::schemas::extract_proto_messages(
                source, path, module_id, repo,
            );
            proto_files += 1;
            proto_services += out.service_count;
            proto_rpcs += out.rpc_count;
            proto_messages += msgs.message_count;
            proto_enums += msgs.enum_count;
            if out.package.is_some() {
                proto_packages += 1;
            }
            // A messages-only `.proto` (a shared `common.proto`) is still a
            // parsed file, not a skipped one.
            if !out.nodes.is_empty() || !msgs.nodes.is_empty() {
                // Same synthetic path as yaml / Dockerfile / manifest / dotenv:
                // the file itself becomes a MODULE (with a POSITION cell) that
                // parents the GRPC_SERVICE, so `locate_node` / `docs-for` can
                // answer "where is this service declared".
                stash_synthetic_parse(
                    "proto",
                    path,
                    module_id,
                    repo,
                    vec![out.nodes, msgs.nodes],
                    vec![out.edges, msgs.edges],
                    vec![out.nav, msgs.nav],
                    out.module_cells,
                    &mut parses_by_lang,
                );
            }
            continue;
        }

        // A10.4: a `.graphql` / `.gql` schema reaches the SDL field scan that
        // embedded `type Query {` blocks already get, so a schema-first
        // service has resolvers for its clients' operations to pair with.
        // Resolver side only: a schema declares server fields, and the
        // operation needles would mint client ops from its keywords.
        if lang == "graphql" {
            let module_id = NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::MODULE,
                &path_to_qname(path),
            );
            let repo_graph_code_extractors::graphql::GraphqlNodes {
                nodes,
                nav,
                mut anchors,
            } = repo_graph_code_extractors::graphql::extract_graphql_resolver_nodes(
                source, module_id, repo,
            );
            sdl_files += 1;
            sdl_resolvers += nodes.len();
            if !nodes.is_empty() {
                stash_synthetic_parse(
                    "graphql",
                    path,
                    module_id,
                    repo,
                    vec![nodes],
                    vec![],
                    vec![nav],
                    vec![],
                    &mut parses_by_lang,
                );
                // A5.8: POSITION on each field, and the MODULE CONTAINS
                // fallback, since no function in a schema owns a field.
                if let Some(fp) = parses_by_lang.get_mut("graphql").and_then(|v| v.last_mut()) {
                    repo_graph_code_extractors::anchor::attach(fp, path, module_id, &mut anchors);
                }
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
            let mut stats = ExtractStats::default();
            apply_cross_cutting_extractors(
                &mut fp, source, path, lang, module_id, repo, &mut stats,
            );
            // G15: denormalize the file's external library names onto every node
            // as an IMPORTS cell (one place, all languages).
            repo_graph_code_domain::attach_imports_cell(&mut fp, lang);
            Ok::<_, String>((fp, stats))
        }));
        match parse_result {
            Ok(Ok((fp, stats))) => {
                reparsed += 1;
                nav_routes_marked += stats.nav_routes;
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
        // old stamp here (dev-notes memory: feedback_maturin_stale_wheel). The
        // repo prefix gives a multi-repo build one attributable line per repo.
        eprintln!(
            "[incremental] {repo_label}: reused {reused}, reparsed {reparsed}, evicted {} (parse cache, stamp={})",
            c.stats.evicted,
            cache::CACHE_VERSION
        );
    }

    // A3.4 fired_on marker: client-router ROUTE nodes were tagged
    // `provenance: nav_route` so the HTTP route index can skip them. Only
    // printed when a build actually marked one.
    if nav_routes_marked > 0 {
        eprintln!("[extract] nav-routes marked: {nav_routes_marked}");
    }

    // A5.1 fired_on marker: a `.proto` is now a first-class parsed file, not a
    // floating service node. Only printed when a build actually saw one.
    if proto_files > 0 {
        eprintln!(
            "[proto] {proto_files} files -> {proto_services} services, {proto_rpcs} rpcs, {proto_packages} packages"
        );
        // A10.5 fired_on marker: the declared message / enum types. Its own
        // line (`files=` shape) so A5.1's line above keeps its format.
        eprintln!(
            "[proto] files={proto_files} services={proto_services} messages={proto_messages} enums={proto_enums}"
        );
    }

    // A10.4 fired_on marker: `.graphql` / `.gql` files are walked and their
    // SDL fields are GRAPHQL_RESOLVER nodes. `files` counts every schema
    // file routed, so `resolvers=0` flags operation-only documents.
    if sdl_files > 0 {
        eprintln!("[graphql-sdl] files={sdl_files} resolvers={sdl_resolvers}");
    }

    // A10.1 fired_on marker: the repo's own API contract is now substrate.
    // Only printed when a build actually saw a spec file.
    let contract_ops = contracts.openapi + contracts.asyncapi + contracts.pact;
    if contract_ops > 0 {
        eprintln!(
            "[contract] files={} ops={contract_ops} (openapi={} asyncapi={} pact={})",
            contracts.files, contracts.openapi, contracts.asyncapi, contracts.pact
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

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{cell_type, edge_category};
    use repo_graph_core::CellPayload;

    fn route(path: &str, source: &str) -> HashMap<&'static str, Vec<FileParse>> {
        let files = vec![(path.to_string(), source.to_string())];
        let (parses, errors) = parse_repo_files(&files, RepoId(1), "", None, "test");
        assert!(errors.is_empty(), "{errors:?}");
        parses
    }

    #[test]
    fn graphql_schema_files_route_to_the_sdl_scan() {
        assert_eq!(detect_language("api/schema.graphql"), Some("graphql"));
        assert_eq!(detect_language("api/schema.gql"), Some("graphql"));

        let sdl = "type Query {\n  getUser(id: ID!): User\n}\n\ntype User {\n  id: ID!\n}\n";
        let parses = route("api/schema.graphql", sdl);
        let fps = &parses["graphql"];
        assert_eq!(fps.len(), 1);
        let fp = &fps[0];

        let module_id =
            NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::MODULE, "api::schema");
        assert_eq!(fp.nodes[0].id, module_id, "the schema file is a MODULE");

        let mut resolvers: Vec<&str> = fp
            .nodes
            .iter()
            .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::GRAPHQL_RESOLVER))
            .filter_map(|n| fp.nav.qname_by_id.get(&n.id).map(String::as_str))
            .collect();
        resolvers.sort_unstable();
        assert_eq!(resolvers, ["graphql_resolver:Query", "graphql_resolver:getUser"]);

        let get_user = NodeId::from_parts(
            GRAPH_TYPE,
            RepoId(1),
            node_kind::GRAPHQL_RESOLVER,
            "graphql_resolver:getUser",
        );
        let node = fp.nodes.iter().find(|n| n.id == get_user).expect("getUser node");
        let position = node
            .cells
            .iter()
            .find(|c| c.kind == cell_type::POSITION)
            .map(|c| match &c.payload {
                CellPayload::Json(s) | CellPayload::Text(s) => s.clone(),
                CellPayload::Bytes(_) => String::new(),
            });
        assert_eq!(
            position.as_deref(),
            Some(r#"{"file":"api/schema.graphql","start_line":1,"end_line":1}"#)
        );
        assert!(fp.edges.iter().any(|e| e.from == module_id
            && e.to == get_user
            && e.category == edge_category::CONTAINS));
        assert!(
            !fp.nav.kind_by_id.values().any(|k| *k == node_kind::GRAPHQL_OPERATION),
            "a schema declares no client operations"
        );
    }

    #[test]
    fn graphql_operation_documents_mint_nothing() {
        let doc = "query getUser($id: ID!) {\n  getUser(id: $id) { id }\n}\n";
        assert!(route("client/getUser.graphql", doc).is_empty());
    }
}
