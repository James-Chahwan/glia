//! Language detection, per-file parser dispatch, and the cross-cutting
//! extractor dispatch every parsed file runs through.

use std::path::Path;

use repo_graph_code_domain::{CodeNav, FileParse};
use repo_graph_core::{NodeId, RepoId};

// ----------------------------------------------------------------------------
// Language detection + per-language parser dispatch
// ----------------------------------------------------------------------------

pub(crate) fn detect_language(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_str()?;
    match ext {
        "py" => Some("python"),
        "go" => Some("go"),
        "ts" | "tsx" => {
            if path.contains(".component.ts") {
                Some("angular")
            } else {
                Some("typescript")
            }
        }
        "js" | "jsx" => Some("typescript"),
        "vue" => Some("vue"),
        "rs" => Some("rust"),
        "java" | "kt" => Some("java"),
        "cs" => Some("csharp"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "swift" => Some("swift"),
        "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" => Some("c_cpp"),
        "scala" => Some("scala"),
        "clj" | "cljs" | "cljc" => Some("clojure"),
        "dart" => Some("dart"),
        "ex" | "exs" => Some("elixir"),
        "sol" => Some("solidity"),
        "tf" | "hcl" => Some("terraform"),
        "proto" => Some("proto"),
        // A10.4: a standalone GraphQL schema. Routed to the SDL resolver scan
        // in `route.rs`, never to a language parser.
        "graphql" | "gql" => Some("graphql"),
        // A10.6: an Avro schema. Routed to the MESSAGE_TYPE scan in
        // `route.rs` by extension, never through A10.8's `.json` sniff.
        "avsc" => Some("avro"),
        _ => None,
    }
}

/// Parse a single file with the appropriate language parser. Public so the
/// pyo3 wrapper's `parse_file_to_json` can call directly without going
/// through the full repo-walk pipeline.
pub fn parse_one(
    source: &str,
    path: &str,
    lang: &str,
    repo: RepoId,
) -> Result<FileParse, String> {
    parse_one_with(source, path, lang, repo, "")
}

/// Like [`parse_one`] but with the Go `module` prefix (from `go.mod`) so the Go
/// parser recognises internal package imports as internal instead of leaking
/// their names into `Symbol.imports` (WP-G / #6). Non-Go languages ignore it.
pub fn parse_one_with(
    source: &str,
    path: &str,
    lang: &str,
    repo: RepoId,
    go_module_prefix: &str,
) -> Result<FileParse, String> {
    let module_qname = path_to_qname(path);
    match lang {
        "python" => repo_graph_parser_python::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "go" => repo_graph_parser_go::parse_file(source, path, &module_qname, go_module_prefix, repo)
            .map_err(|e| e.to_string()),
        "typescript" | "js" => repo_graph_parser_typescript::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "rust" => repo_graph_parser_rust::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "java" => repo_graph_parser_java::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "csharp" => repo_graph_parser_csharp::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "ruby" => repo_graph_parser_ruby::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "php" => repo_graph_parser_php::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "swift" => repo_graph_parser_swift::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "c_cpp" => {
            let is_cpp = matches!(
                Path::new(path).extension().and_then(|e| e.to_str()),
                Some("cpp" | "cc" | "cxx" | "hpp")
            );
            repo_graph_parser_c_cpp::parse_file(source, path, &module_qname, is_cpp, repo)
                .map_err(|e| e.to_string())
        }
        "scala" => repo_graph_parser_scala::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "clojure" => repo_graph_parser_clojure::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "dart" => repo_graph_parser_dart::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "elixir" => repo_graph_parser_elixir::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "solidity" => repo_graph_parser_solidity::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "terraform" => repo_graph_parser_terraform::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "react" => repo_graph_parser_react::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "angular" => repo_graph_parser_angular::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "vue" => repo_graph_parser_vue::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        _ => Err(format!("unsupported language: {lang}")),
    }
}

/// Per-file counters the cross-cutting extractors accumulate for the build's
/// greppable stderr markers. An out-param rather than a return value so a new
/// counter is one field, not another signature churn at the single call site.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ExtractStats {
    /// A3.4: ROUTE nodes marked `provenance: nav_route` (client-router browser
    /// navigation targets, excluded from the HTTP pairing index downstream).
    pub nav_routes: usize,
    /// A3.5: ts_routes Express-scan matches rejected because the receiver is
    /// an HTTP client (`this.http.get('/users')`) — phantom server ROUTEs no
    /// longer minted.
    pub ts_client_calls_skipped: usize,
}

pub(crate) fn apply_cross_cutting_extractors(
    fp: &mut FileParse,
    source: &str,
    path: &str,
    lang: &str,
    module_id: NodeId,
    repo: RepoId,
    stats: &mut ExtractStats,
) {
    use repo_graph_code_extractors::{
        anchor, angular, cli, config, cron, data_entities, data_sources, eventbus, graphql, grpc,
        openapi_annot, queues, react, services, trpc, ts_routes, vue, websocket,
    };

    macro_rules! run {
        ($call:expr) => {{
            let out = $call;
            fp.nodes.extend(out.nodes);
            merge_nav(&mut fp.nav, out.nav);
        }};
    }

    /// Like `run!` but also accumulates the extractor's own counters into
    /// `stats` for the build markers (A3.4 `nav_routes`).
    macro_rules! run_counted {
        ($call:expr) => {{
            let out = $call;
            stats.nav_routes += out.nav_routes;
            fp.nodes.extend(out.nodes);
            merge_nav(&mut fp.nav, out.nav);
        }};
    }

    // A5.8: `run_marked!` is `run!` plus the extractor's marker anchors, which
    // `anchor::attach` turns into POSITION cells and owner edges once every
    // node of the file is in.
    let mut anchors: Vec<anchor::Anchor> = Vec::new();
    macro_rules! run_marked {
        ($call:expr) => {{
            let out = $call;
            anchors.extend(out.anchors);
            fp.nodes.extend(out.nodes);
            merge_nav(&mut fp.nav, out.nav);
        }};
    }

    macro_rules! run_with_edges {
        ($call:expr) => {{
            let out = $call;
            fp.nodes.extend(out.nodes);
            fp.edges.extend(out.edges);
            merge_nav(&mut fp.nav, out.nav);
        }};
    }

    // A2.8: `run_with_edges!`, not `run!` — queue nodes now carry a CONTAINS
    // edge from the module that publishes/consumes the topic. Dropping
    // `out.edges` here would silently discard them.
    run_with_edges!(queues::extract_queue_consumer_nodes(
        source, path, module_id, repo
    ));
    run_with_edges!(queues::extract_queue_producer_nodes(
        source, path, module_id, repo
    ));
    // LA.20a: not `run!` — declarations are dispatched by language and carry
    // `cli:<name> --HANDLED_BY--> <implementation>` refs (bound by the graph
    // builder's `resolve_refs`). Per-file marker, printed only when the file
    // declared a command.
    {
        let decl = cli::extract_cli_command_nodes(source, lang, module_id, repo);
        if let Some(marker) = cli::decl_marker(lang, &decl, path) {
            eprintln!("{marker}");
        }
        fp.refs.extend(decl.refs);
        fp.nodes.extend(decl.nodes);
        merge_nav(&mut fp.nav, decl.nav);
    }
    run!(cli::extract_cli_invocation_nodes(source, module_id, repo));
    run_marked!(websocket::extract_ws_handler_nodes(
        source, path, module_id, repo
    ));
    run_marked!(websocket::extract_ws_client_nodes(
        source, path, module_id, repo
    ));
    run_marked!(eventbus::extract_event_emitter_nodes(source, module_id, repo));
    run_marked!(eventbus::extract_event_handler_nodes(source, module_id, repo));
    run_marked!(graphql::extract_graphql_operation_nodes(source, module_id, repo));
    run_marked!(graphql::extract_graphql_resolver_nodes(source, module_id, repo));
    run_marked!(grpc::extract_grpc_client_nodes(source, module_id, repo));
    run_with_edges!(data_sources::extract_data_source_nodes(
        source, module_id, repo
    ));
    run_with_edges!(data_entities::extract_data_entity_nodes(
        source, module_id, repo
    ));
    run_with_edges!(cron::extract_cron_nodes(source, path, module_id, repo));
    run_with_edges!(config::extract_config_reads(source, module_id, repo));

    if matches!(lang, "typescript" | "react" | "angular" | "vue") {
        // A3.5: not `run!` — the routes also carry HANDLED_BY refs for named
        // Express handlers (bound by the graph builder's `resolve_refs`) and
        // a count of client calls the scan declined to mint as routes.
        let routes = ts_routes::extract_ts_backend_routes(source, path, module_id, repo);
        stats.ts_client_calls_skipped += routes.skipped_client_calls;
        fp.refs.extend(routes.refs);
        fp.nodes.extend(routes.nodes);
        merge_nav(&mut fp.nav, routes.nav);
        // A10.9: tRPC routers/procedures (server) and hook/vanilla calls
        // (client). Per-file marker, printed only when the file yielded one —
        // the aggregation point (route.rs) is a sibling packet's file. LA.31:
        // both are marker kinds now, so their anchors (one per procedure key /
        // call site) join the A5.8 pass below; `anchors=` is read before
        // `run_marked!` moves them.
        let procs = trpc::extract_trpc_procedure_nodes(source, module_id, repo);
        let calls = trpc::extract_trpc_call_nodes(source, module_id, repo);
        if !procs.nodes.is_empty() || !calls.nodes.is_empty() {
            eprintln!(
                "[trpc] routers={} procedures={} calls={} anchors={} path={path}",
                procs.routers,
                procs.nodes.len(),
                calls.nodes.len(),
                procs.anchors.len() + calls.anchors.len()
            );
        }
        run_marked!(procs);
        run_marked!(calls);
    }
    if matches!(lang, "react" | "typescript") {
        let module_qname = fp
            .nav
            .qname_by_id
            .get(&module_id)
            .cloned()
            .unwrap_or_default();
        run_counted!(react::extract_react_nodes(
            source, &module_qname, module_id, repo
        ));
    }
    if matches!(lang, "angular" | "typescript") {
        let module_qname = fp
            .nav
            .qname_by_id
            .get(&module_id)
            .cloned()
            .unwrap_or_default();
        run_counted!(angular::extract_angular_nodes(
            source, &module_qname, module_id, repo
        ));
    }
    if matches!(lang, "vue" | "typescript") {
        let module_qname = fp
            .nav
            .qname_by_id
            .get(&module_id)
            .cloned()
            .unwrap_or_default();
        run_counted!(vue::extract_vue_nodes(
            source, path, &module_qname, module_id, repo
        ));
    }

    // LA.15a: OpenAPI annotations on a handler (springdoc / springfox,
    // Swashbuckle / [ProducesResponseType], @nestjs/swagger) become contract
    // ops, keyed on the ROUTE the language parser above already emitted for
    // that handler. DOC_SECTION is not a marker kind, so `anchor::attach`
    // below ignores them; the result is a function of this file alone, so it
    // is cached with the parse.
    run!(openapi_annot::extract_annotated_ops(
        source, path, lang, fp, module_id, repo
    ));

    // A5.8: locate every RPC-family marker and tie it to the METHOD/FUNCTION
    // whose span holds its needle (module CONTAINS when none does). Runs after
    // every extractor above so the owner index sees all of the file's spans.
    // The result is a function of this file alone, so it is cached with the
    // parse; the build-level `[marker-anchor]` marker counts it post-cache.
    anchor::attach(fp, path, module_id, &mut anchors);

    // G14: cross-language SERVICE classification. Runs LAST so the per-file
    // nav already has every CLASS / STRUCT and its METHOD children populated
    // by the language parser + framework extractors above. Emits SERVICE
    // nodes + CONTAINS edges to owned methods.
    {
        let svc = services::extract_service_nodes(source, lang, &fp.nav, module_id, repo);
        fp.nodes.extend(svc.nodes);
        fp.edges.extend(svc.edges);
        merge_nav(&mut fp.nav, svc.nav);
    }
}

pub(crate) fn merge_nav(dst: &mut CodeNav, src: CodeNav) {
    dst.name_by_id.extend(src.name_by_id);
    dst.qname_by_id.extend(src.qname_by_id);
    dst.kind_by_id.extend(src.kind_by_id);
    dst.parent_of.extend(src.parent_of);
    for (k, v) in src.children_of {
        dst.children_of.entry(k).or_default().extend(v);
    }
}

pub(crate) fn path_to_qname(path: &str) -> String {
    Path::new(path)
        .with_extension("")
        .to_string_lossy()
        .replace(['/', '\\'], "::")
}
