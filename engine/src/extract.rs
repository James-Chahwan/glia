//! Language detection, per-file parser dispatch, and the cross-cutting
//! extractor dispatch every parsed file runs through.

use std::path::Path;

use repo_graph_code_domain::{CodeNav, FileParse, evidence};
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
        "java" => Some("java"),
        // A14.2: Kotlin has its own parser. It shares ONE graph with Java
        // (the JVM family, `build::lang_build`), so the tag is only routing.
        "kt" => Some("kotlin"),
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

/// TS-family lang tags (typescript / angular / react / vue) share ONE module
/// and symbol space in a repo, so they build as one graph
/// (`build::lang_build`) and form one [`build_group`].
pub(crate) const TS_FAMILY: &[&str] = &["angular", "react", "typescript", "vue"];

/// The build group of a language-parser tag: the tags whose parses resolve
/// inside ONE per-repo `RepoGraph`. The TS family is `typescript`, Kotlin joins
/// Java's JVM graph (A14.2), every other tag is its own group. The graph
/// grouping (`build::lang_build`) and the LB.9b module plan
/// (`route::ModuleQnames`) both read it, so they cannot drift apart.
pub(crate) fn build_group(lang: &'static str) -> &'static str {
    if TS_FAMILY.contains(&lang) {
        "typescript"
    } else if lang == "kotlin" {
        "java"
    } else {
        lang
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
    // LB.10a: every C/C++ MODULE is named by its full file name, so a
    // single-file parse names it exactly as the repo build does.
    let module_qname = if lang == "c_cpp" {
        synthetic_module_qname(path)
    } else {
        path_to_qname(path)
    };
    parse_one_as(source, path, lang, repo, go_module_prefix, &module_qname)
}

/// [`parse_one_with`] under an explicit MODULE qname. The router passes the
/// LB.9b plan's qname (`route::ModuleQnames::module_qname`): the file-name
/// form (`api::user.py`) when a file of another build group shares the
/// stem, and for every C/C++ file (LB.10a), [`path_to_qname`] otherwise.
/// Every symbol qname follows it.
pub(crate) fn parse_one_as(
    source: &str,
    path: &str,
    lang: &str,
    repo: RepoId,
    go_module_prefix: &str,
    module_qname: &str,
) -> Result<FileParse, String> {
    let module_qname = module_qname.to_string();
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
        "kotlin" => repo_graph_parser_kotlin::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "csharp" => repo_graph_parser_csharp::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "ruby" => repo_graph_parser_ruby::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "php" => repo_graph_parser_php::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "swift" => repo_graph_parser_swift::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "c_cpp" => repo_graph_parser_c_cpp::parse_file(
            source,
            path,
            &module_qname,
            repo_graph_parser_c_cpp::Dialect::from_path(path),
            repo,
        )
        .map_err(|e| e.to_string()),
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
    /// LA.6b: minted once per file by the shared route-table walker.
    pub nav_routes: usize,
    /// LA.6b: route -> component `HANDLED_BY` refs the route tables emitted.
    pub nav_bound: usize,
    /// LA.6b: route -> route `NAVIGATES_TO` refs (redirects) emitted.
    pub nav_redirects: usize,
    /// LA.6b: records whose path was composed onto a parent record's.
    pub nav_children: usize,
    /// LA.6b: `path` objects refused as route records (no router key, or not
    /// a URL path).
    pub nav_rejected: usize,
    /// LA.6b: nav ROUTEs whose path ends in a wildcard (`**`, `*`, `:x*`).
    pub nav_catchalls: usize,
    /// A3.5: ts_routes Express-scan matches rejected because the receiver is
    /// an HTTP client (`this.http.get('/users')`) — phantom server ROUTEs no
    /// longer minted.
    pub ts_client_calls_skipped: usize,
    /// LA.6c: router-tier `NAVIGATES_TO` link refs (`/path`) emitted.
    pub nav_links_router: usize,
    /// LA.6c: plain-anchor link refs (`href:/path`) emitted.
    pub nav_links_href: usize,
    /// LA.6c: the router-tier links built from `location.origin`.
    pub nav_links_origin: usize,
    /// LA.6c: link sites skipped because the target is not a literal path.
    pub nav_links_dynamic: usize,
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
        nav_links, nav_routes, openapi_annot, queues, react, secrets_flags, services, trpc,
        ts_routes, vue, websocket,
    };

    macro_rules! run {
        ($call:expr) => {{
            let out = $call;
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

    // LC.3a: `$name` is the extractor's evidence name; its edges are stamped
    // `extractor:<name>` before they join the file's.
    macro_rules! run_with_edges {
        ($name:literal, $call:expr) => {{
            let mut out = $call;
            evidence::stamp_missing(&mut out.edges, concat!("extractor:", $name));
            fp.nodes.extend(out.nodes);
            fp.edges.extend(out.edges);
            merge_nav(&mut fp.nav, out.nav);
        }};
    }

    // LE.4c: `run_with_edges!` plus the extractor's marker anchors — the
    // queue extractor's nodes carry both its own edges and anchors.
    macro_rules! run_marked_with_edges {
        ($name:literal, $call:expr) => {{
            let mut out = $call;
            evidence::stamp_missing(&mut out.edges, concat!("extractor:", $name));
            anchors.extend(out.anchors);
            fp.nodes.extend(out.nodes);
            fp.edges.extend(out.edges);
            merge_nav(&mut fp.nav, out.nav);
        }};
    }

    // A2.8: not `run!` — queue nodes carry a CONTAINS edge from the module
    // that publishes/consumes the topic. Dropping `out.edges` here would
    // silently discard them. LE.4c: and one anchor per call site, so the
    // A5.8 pass below adds `function -USES-> queue_producer:<t>` and
    // `queue_consumer:<t> -HANDLED_BY-> function` on top of that CONTAINS.
    // LA.33: the consumers' callbacks are kept apart (the macro moves the
    // nodes) and bound after the anchor pass below, once every span is in.
    let mut consumers = queues::extract_queue_consumer_nodes(source, path, module_id, repo);
    let callbacks = std::mem::take(&mut consumers.callbacks);
    run_marked_with_edges!("queues", consumers);
    run_marked_with_edges!(
        "queues",
        queues::extract_queue_producer_nodes(source, path, module_id, repo)
    );
    // LA.20a: not `run!` — declarations are dispatched by language and carry
    // `cli:<name> --HANDLED_BY--> <implementation>` refs (bound by the graph
    // builder's `resolve_refs`). Per-file marker, printed only when the file
    // declared a command.
    {
        // A14.2 stopgap: `cli`'s picocli arm admits only the `java` tag, and
        // Kotlin picocli (`@Command(name = ..) class X : Runnable`) rode it
        // while `.kt` parsed as Java. Removal: once that arm reads
        // `"java" | "kotlin"`, pass `lang` straight through again.
        let cli_lang = if lang == "kotlin" { "java" } else { lang };
        let decl = cli::extract_cli_command_nodes(source, cli_lang, module_id, repo);
        if let Some(marker) = cli::decl_marker(cli_lang, &decl, path) {
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
    run_marked!(graphql::extract_graphql_resolver_nodes(source, lang, module_id, repo));
    run_marked!(grpc::extract_grpc_client_nodes(source, module_id, repo));
    run_with_edges!(
        "data_sources",
        data_sources::extract_data_source_nodes(source, module_id, repo)
    );
    run_with_edges!(
        "data_entities",
        data_entities::extract_data_entity_nodes(source, module_id, repo)
    );
    // LA.19a: not `run_with_edges!` — code-sourced jobs (Quartz, Hangfire,
    // robfig / gocron, APScheduler, Spring `@Scheduled`) also carry
    // `CRON_JOB --HANDLED_BY--> handler` refs, bound by the graph builder's
    // `resolve_refs`. The extractor prints its own `[cron] code` marker.
    {
        let mut out = cron::extract_cron_nodes(source, path, module_id, repo);
        evidence::stamp_missing(&mut out.edges, "extractor:cron");
        fp.nodes.extend(out.nodes);
        fp.edges.extend(out.edges);
        fp.refs.extend(out.refs);
        merge_nav(&mut fp.nav, out.nav);
    }
    run_with_edges!(
        "config",
        config::extract_config_reads(source, module_id, repo)
    );
    // A13.8: secrets-manager refs (`config:secret:<provider>/<ref>`) and
    // feature-flag checks (`config:flag:<key>`), language-blind, so every
    // code file of every language is scanned. Per-file marker, printed only
    // when the file captured one.
    {
        let secrets = secrets_flags::extract_secret_refs(source, module_id, repo);
        let flags = secrets_flags::extract_feature_flags(source, module_id, repo);
        if let Some(marker) = secrets_flags::marker(&[&secrets, &flags], lang) {
            eprintln!("{marker} path={path}");
        }
        run_with_edges!("secrets_flags", secrets);
        run_with_edges!("secrets_flags", flags);
    }

    if matches!(lang, "typescript" | "react" | "angular" | "vue") {
        // A3.5: not `run!` — the routes also carry HANDLED_BY refs for named
        // Express handlers (bound by the graph builder's `resolve_refs`) and
        // a count of client calls the scan declined to mint as routes.
        let routes = ts_routes::extract_ts_backend_routes(source, path, module_id, repo);
        stats.ts_client_calls_skipped += routes.skipped_client_calls;
        // LB.11b fired_on marker: one ROUTE per (method, path), each located
        // by a POSITION per registration; `any=` counts method-agnostic ones.
        if !routes.nodes.is_empty() {
            eprintln!(
                "[ts-routes] routes={} positioned={} any={} path={path}",
                routes.nodes.len(),
                routes.positioned,
                routes.any
            );
        }
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
        // LA.6b: Angular Router / React Router / vue-router tables, ONE walk
        // per file (the three framework extractors below used to scan `path:`
        // each, minting one literal up to three times on a plain `.ts`). Not
        // `run!`: the routes carry HANDLED_BY (route -> component) and
        // NAVIGATES_TO (redirect) refs, bound by the graph builder's
        // `resolve_refs`.
        let tables = nav_routes::extract_route_tables(source, module_id, repo);
        stats.nav_routes += tables.nav_routes;
        stats.nav_bound += tables.bound;
        stats.nav_redirects += tables.redirects;
        stats.nav_children += tables.children;
        stats.nav_rejected += tables.rejected;
        stats.nav_catchalls += tables.catchalls;
        fp.refs.extend(tables.refs);
        fp.nodes.extend(tables.nodes);
        merge_nav(&mut fp.nav, tables.nav);
        // LA.6c: the file's navigation link sites (router APIs, link
        // components, anchors, origin share links) as NAVIGATES_TO refs from
        // its MODULE, LA.6a's link contract. A `.component.html` template's
        // links take the template branch in route.rs instead.
        let links = nav_links::extract_nav_links(source, lang, module_id);
        stats.nav_links_router += links.router;
        stats.nav_links_href += links.href;
        stats.nav_links_origin += links.origin;
        stats.nav_links_dynamic += links.dynamic_skipped;
        fp.refs.extend(links.refs);
    }
    if matches!(lang, "react" | "typescript") {
        let module_qname = fp
            .nav
            .qname_by_id
            .get(&module_id)
            .cloned()
            .unwrap_or_default();
        run!(react::extract_react_nodes(
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
        run!(angular::extract_angular_nodes(
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
        run!(vue::extract_vue_nodes(
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

    // A5.8: locate every RPC-family marker (LE.4c: and every queue marker)
    // and tie it to the METHOD/FUNCTION whose span holds its needle (module
    // CONTAINS when none does; a queue node already has one). Runs after
    // every extractor above so the owner index sees all of the file's spans.
    // The result is a function of this file alone, so it is cached with the
    // parse; the build-level `[marker-anchor]` marker counts it post-cache.
    let before = fp.edges.len();
    anchor::attach(fp, path, module_id, &mut anchors);
    evidence::stamp_missing(&mut fp.edges[before..], "extractor:anchor");

    // LA.33: a consumer is HANDLED_BY the callback it passes, on top of the
    // subscribing function above: `this.x` / a method value bound in-file
    // (stamped `extractor:queue_callbacks`), a name or member as a HANDLED_BY
    // ref the graph builder's `resolve_refs` binds. Per-file fired_on marker,
    // printed only when the file had a callback.
    let cb = queues::bind_consumer_callbacks(fp, module_id, &callbacks);
    if let Some(marker) = cb.marker(path) {
        eprintln!("{marker}");
    }

    // G14: cross-language SERVICE classification. Runs LAST so the per-file
    // nav already has every CLASS / STRUCT and its METHOD children populated
    // by the language parser + framework extractors above. Emits SERVICE
    // nodes + CONTAINS edges to owned methods.
    {
        let mut svc = services::extract_service_nodes(source, lang, &fp.nav, module_id, repo);
        evidence::stamp_missing(&mut svc.edges, "extractor:services");
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

/// A non-code file's MODULE qname (LB.9a): the directories `::`-joined plus
/// the FULL file name (`svc/Dockerfile.prod` -> `svc::Dockerfile.prod`,
/// `.env.local` -> `.env.local`, root `openapi.json` -> `openapi.json`).
/// [`path_to_qname`] drops the last extension, which is right for code
/// (imports name the stem) but lets `api/user.proto` share `api::user` with
/// `api/user.go`, and folds `Dockerfile.prod` into `Dockerfile`. Nothing
/// imports a non-code module by qname and its children are never
/// path-qualified, so the full name costs no code qname. Code files take the
/// same form when LB.9b qualifies a cross-group stem, and every C/C++ file
/// always does (LB.10a: an `#include` names the file with its extension).
pub(crate) fn synthetic_module_qname(path: &str) -> String {
    let p = path.replace('\\', "/");
    match p.rsplit_once('/') {
        Some((dir, file)) => format!("{}::{file}", dir.replace('/', "::")),
        None => p,
    }
}
