//! Pure-Rust orchestration for glia.
//!
//! Walk a repo → run language parsers + cross-cutting extractors → build per-
//! lang graphs → merge → run cross-graph resolvers. Returns a `MergedGraph`.
//!
//! Two entry points:
//!   - [`generate_one`] for a single repo path (each path's own RepoId).
//!   - [`generate_many`] for N repo paths under a single MergedGraph so
//!     cross-graph resolvers (HTTP, gRPC, DbResolver, etc.) fire across the
//!     boundary. The pyo3 wrapper and the `glia` CLI both call into here.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use repo_graph_code_domain::{CodeNav, FileParse, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, Node, NodeId, RepoId};
use repo_graph_graph::{
    CliInvocationResolver, ConfigResolver, CronResolver, DbResolver, EventBusResolver,
    GraphQLStackResolver, GrpcStackResolver, HttpStackResolver, IacResolver, MergedGraph,
    PackageResolver, QueueStackResolver, SharedSchemaResolver, WebSocketStackResolver,
};

pub use repo_graph_graph::MergedGraph as ReExportedMergedGraph;

pub struct GenerateResult {
    pub merged: MergedGraph,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub parse_errors: Vec<String>,
}

/// Generate a `MergedGraph` from a single repo path. The repo gets one RepoId
/// derived from `file://<path>`; cross-graph resolvers run but only emit
/// edges within this single repo (rare in practice).
pub fn generate_one(repo_path: &str) -> Result<GenerateResult, String> {
    let root = PathBuf::from(repo_path);
    if !root.is_dir() {
        return Err(format!("not a directory: {repo_path}"));
    }
    let repo = RepoId::from_canonical(&format!("file://{repo_path}"));
    let (files, regions, md) = walk_source_files(&root);
    let (mut graphs, mut parse_errors) = build_graphs_for_repo(&files, repo);
    if !regions.is_empty() {
        graphs.push(build_region_graph(&regions, repo));
    }
    if let Some(docs) = build_docs_graph(&md, repo) {
        graphs.push(docs);
    }
    let mut merged = MergedGraph::new(graphs);
    run_all_resolvers(&mut merged);
    post_passes(&mut merged);
    let total_nodes: usize = merged.graphs.iter().map(|g| g.nodes.len()).sum();
    let total_edges: usize = merged.graphs.iter().map(|g| g.edges.len()).sum::<usize>()
        + merged.cross_edges.len();
    parse_errors.shrink_to_fit();
    Ok(GenerateResult {
        merged,
        total_nodes,
        total_edges,
        parse_errors,
    })
}

/// Generate a `MergedGraph` from N repo paths. Each path becomes its own
/// RepoId so cross-graph resolvers fire across boundaries (the canonical
/// substrate-eval entry).
pub fn generate_many(repo_paths: &[String]) -> Result<GenerateResult, String> {
    let mut all_graphs = Vec::new();
    let mut all_errors = Vec::new();
    for path in repo_paths {
        let root = PathBuf::from(path);
        if !root.is_dir() {
            all_errors.push(format!("not a directory: {path}"));
            continue;
        }
        let repo = RepoId::from_canonical(&format!("file://{path}"));
        let (files, regions, md) = walk_source_files(&root);
        let (graphs, parse_errors) = build_graphs_for_repo(&files, repo);
        all_graphs.extend(graphs);
        if !regions.is_empty() {
            all_graphs.push(build_region_graph(&regions, repo));
        }
        if let Some(docs) = build_docs_graph(&md, repo) {
            all_graphs.push(docs);
        }
        all_errors.extend(parse_errors);
    }
    if all_graphs.is_empty() {
        return Err(format!(
            "no graphs produced from {} paths; first error: {}",
            repo_paths.len(),
            all_errors.first().cloned().unwrap_or_default(),
        ));
    }
    let mut merged = MergedGraph::new(all_graphs);
    run_all_resolvers(&mut merged);
    post_passes(&mut merged);
    let total_nodes: usize = merged.graphs.iter().map(|g| g.nodes.len()).sum();
    let total_edges: usize = merged.graphs.iter().map(|g| g.edges.len()).sum::<usize>()
        + merged.cross_edges.len();
    Ok(GenerateResult {
        merged,
        total_nodes,
        total_edges,
        parse_errors: all_errors,
    })
}

// ----------------------------------------------------------------------------
// Per-repo graph building
// ----------------------------------------------------------------------------

fn build_graphs_for_repo(
    files: &[(String, String)],
    repo: RepoId,
) -> (Vec<repo_graph_graph::RepoGraph>, Vec<String>) {
    let mut parses_by_lang: HashMap<&str, Vec<FileParse>> = HashMap::new();
    let mut proto_parses = Vec::new();
    let mut parse_errors = Vec::new();

    // Suppress the default panic-print-to-stderr while we run per-file parsers
    // — we catch panics below and report them as parse_errors. The default
    // hook would otherwise spam stderr (with a backtrace) for every bad file
    // even though we recover. Restored on scope exit via Drop guard so a
    // panic in non-loop code still gets the user-visible report.
    let _hook_guard = SuppressPanicHook::install();

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
            let svc_nodes = repo_graph_code_extractors::grpc::extract_grpc_service_nodes(
                source, module_id, repo,
            );
            let fp = FileParse {
                nodes: svc_nodes.nodes,
                nav: svc_nodes.nav,
                ..Default::default()
            };
            proto_parses.push(fp);
            continue;
        }

        // Per-file panic isolation. Parsers occasionally hit slice/regex bugs
        // on adversarial inputs (e.g. parsers/code/rust/src/lib.rs:511 slice
        // OOB on glia's own source as of 2026-05-09). One bad file shouldn't
        // kill an N-file repo build — log it, skip it, keep going.
        let parse_result = catch_unwind(AssertUnwindSafe(|| {
            let mut fp = parse_one(source, path, lang, repo)?;
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

    let mut graphs = Vec::new();
    for (lang, parses) in parses_by_lang {
        let graph = match lang {
            "python" => repo_graph_graph::build_python(repo, parses),
            "go" => repo_graph_graph::build_go(repo, parses),
            "java" | "csharp" | "php" | "rust" | "scala" | "clojure" | "elixir" => {
                repo_graph_graph::build_dotted(repo, parses)
            }
            "ruby" => repo_graph_graph::build_ruby(repo, parses),
            _ => repo_graph_graph::build_typescript(repo, parses, |_, _| None),
        };
        match graph {
            Ok(g) => graphs.push(g),
            Err(e) => parse_errors.push(format!("{lang} graph: {e}")),
        }
    }

    if !proto_parses.is_empty()
        && let Ok(g) = repo_graph_graph::build_python(repo, proto_parses)
    {
        graphs.push(g);
    }

    (graphs, parse_errors)
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

fn run_all_resolvers(merged: &mut MergedGraph) {
    merged.run(&HttpStackResolver);
    merged.run(&GrpcStackResolver);
    merged.run(&QueueStackResolver);
    merged.run(&GraphQLStackResolver);
    merged.run(&WebSocketStackResolver);
    merged.run(&EventBusResolver);
    merged.run(&SharedSchemaResolver);
    merged.run(&CliInvocationResolver);
    merged.run(&DbResolver);
    merged.run(&CronResolver);
    merged.run(&ConfigResolver);
    merged.run(&IacResolver);
    merged.run(&PackageResolver);
}

fn post_passes(merged: &mut MergedGraph) {
    downgrade_test_paths(merged);
    demote_unmatched_http_nodes(merged);
    emit_tests_edges(merged);
    tag_synthetic_provenance(merged);
}

/// Tag the substrate-only synthetic node kinds with an `ORIGIN` cell so
/// consumers (the engram exporter, neuropil) can filter them by coordinate
/// rather than string-matching keys. These nodes are real and load-bearing for
/// the cross-repo resolvers (`PackageResolver` pairs `package:npm:*`,
/// `EventBusResolver` pairs `event_*`), so they are NOT dropped here — only
/// categorised. (glia-v2 G6/G9/G11)
fn tag_synthetic_provenance(merged: &mut MergedGraph) {
    use repo_graph_code_domain::{cell_type, node_kind};
    use repo_graph_core::{Cell, CellPayload};

    for g in &mut merged.graphs {
        let nodes = &mut g.nodes;
        let nav = &g.nav;
        for n in nodes.iter_mut() {
            // Don't double-tag (region anchors are tagged at creation).
            if n.cells.iter().any(|c| c.kind == cell_type::ORIGIN) {
                continue;
            }
            let kind = nav.kind_by_id.get(&n.id).copied();
            let qname = nav.qname_by_id.get(&n.id).map(String::as_str).unwrap_or("");
            let file = position_file(&n.cells).unwrap_or_default();
            let provenance = if matches!(kind, Some(node_kind::PACKAGE_DEP)) {
                // npm/cargo/etc. dependency pseudo-nodes (G9 hub).
                "dependency"
            } else if matches!(
                kind,
                Some(node_kind::EVENT_EMITTER) | Some(node_kind::EVENT_HANDLER)
            ) {
                // event emitter/handler name pseudo-nodes (G6).
                "synthetic"
            } else if qname.contains("ExampleInstrumentedTest")
                || qname.starts_with("androidTest::")
            {
                // Framework-generated test stubs — Capacitor's
                // ExampleInstrumentedTest, anything under androidTest (G11).
                "generated"
            } else if is_generated_proto(&file) {
                // protobuf-generated reflection code — `chatpb::*::Reset` etc.
                // swamp recall on matching trigrams. Distinct from `generated`
                // so engram can opt it back in independently. (glia-v3 #5)
                "generated_proto"
            } else if is_test_fixture(&file, qname) {
                // test files + seeders/load-testers — droppable by default at
                // recall, opt-in via engram's --include-tests. (glia-v3 #6)
                "test_fixture"
            } else {
                continue;
            };
            n.cells.push(Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(format!(r#"{{"provenance":"{provenance}"}}"#)),
            });
        }
    }
}

/// Pull the `file` path out of a node's POSITION cell. Lightweight string
/// scan of the `{"file":"...","start_line":..}` payload — avoids a serde_json
/// dependency in the engine crate.
fn position_file(cells: &[repo_graph_core::Cell]) -> Option<String> {
    use repo_graph_code_domain::cell_type;
    use repo_graph_core::CellPayload;
    for c in cells {
        if c.kind != cell_type::POSITION {
            continue;
        }
        if let CellPayload::Json(j) = &c.payload
            && let Some(rest) = j.split("\"file\":\"").nth(1)
            && let Some(end) = rest.find('"')
        {
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// Generated-protobuf source: codegen file extensions across the languages
/// quokka-stack mixes (Go / Dart / TS / Python). The reflection-method noise
/// (`Reset`/`String`/`ProtoReflect`/`Marshal`…) lives in these files. (glia-v3 #5)
fn is_generated_proto(file: &str) -> bool {
    file.ends_with(".pb.go")
        || file.ends_with(".pb-grpc.go")
        || file.ends_with(".pb.dart")
        || file.ends_with(".pbjson.dart")
        || file.ends_with(".pbenum.dart")
        || file.ends_with(".pbserver.dart")
        || file.ends_with(".pb.ts")
        || file.ends_with("_pb2.py")
        || file.ends_with("_pb2_grpc.py")
        || file.contains(".pb.")
}

/// Test / fixture / seeder code, by file path or qname shape. (glia-v3 #6)
fn is_test_fixture(file: &str, qname: &str) -> bool {
    file.ends_with("_test.go")
        || file.ends_with("_test.dart")
        || file.ends_with(".spec.ts")
        || file.ends_with(".test.ts")
        || file.ends_with(".spec.js")
        || file.ends_with(".test.js")
        || file.ends_with("_test.py")
        || file.ends_with("_spec.rb")
        || file.contains("/tests/")
        || file.contains("/__tests__/")
        || file.contains("/test/")
        || is_test_qname(qname)
}

// ----------------------------------------------------------------------------
// Language detection + per-language parser dispatch
// ----------------------------------------------------------------------------

fn detect_language(path: &str) -> Option<&'static str> {
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
    let module_qname = path_to_qname(path);
    match lang {
        "python" => repo_graph_parser_python::parse_file(source, path, &module_qname, repo)
            .map_err(|e| e.to_string()),
        "go" => repo_graph_parser_go::parse_file(source, path, &module_qname, "", repo)
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

fn apply_cross_cutting_extractors(
    fp: &mut FileParse,
    source: &str,
    path: &str,
    lang: &str,
    module_id: NodeId,
    repo: RepoId,
) {
    use repo_graph_code_extractors::{
        angular, cli, config, cron, data_entities, data_sources, eventbus, graphql, grpc, queues,
        react, services, ts_routes, vue, websocket,
    };

    macro_rules! run {
        ($call:expr) => {{
            let out = $call;
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

    run!(queues::extract_queue_consumer_nodes(source, module_id, repo));
    run!(queues::extract_queue_producer_nodes(source, module_id, repo));
    run!(cli::extract_cli_command_nodes(source, module_id, repo));
    run!(cli::extract_cli_invocation_nodes(source, module_id, repo));
    run!(websocket::extract_ws_handler_nodes(source, module_id, repo));
    run!(websocket::extract_ws_client_nodes(source, module_id, repo));
    run!(eventbus::extract_event_emitter_nodes(source, module_id, repo));
    run!(eventbus::extract_event_handler_nodes(source, module_id, repo));
    run!(graphql::extract_graphql_operation_nodes(source, module_id, repo));
    run!(graphql::extract_graphql_resolver_nodes(source, module_id, repo));
    run!(grpc::extract_grpc_client_nodes(source, module_id, repo));
    run_with_edges!(data_sources::extract_data_source_nodes(
        source, module_id, repo
    ));
    run_with_edges!(data_entities::extract_data_entity_nodes(
        source, module_id, repo
    ));
    run_with_edges!(cron::extract_cron_nodes(source, path, module_id, repo));
    run_with_edges!(config::extract_config_reads(source, module_id, repo));

    if matches!(lang, "typescript" | "react" | "angular" | "vue") {
        run!(ts_routes::extract_ts_backend_routes(
            source, path, module_id, repo
        ));
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

// ----------------------------------------------------------------------------
// Helpers — file walk, path mapping, nav merge
// ----------------------------------------------------------------------------

/// A build-output / vendored / gitignored directory collapsed to a single
/// anchor node instead of being parsed file-by-file. Preserves the repo's
/// spatial map without the per-file flood. (glia-v2 G1/G2/G10)
struct RegionAnchor {
    /// Repo-relative path of the collapsed directory (`www`, `packages/x/dist`).
    rel_path: String,
    /// `vendored` | `build_output` — recorded in the anchor's ORIGIN cell.
    provenance: &'static str,
    /// The directory's own name (`www`, `node_modules`).
    region: String,
}

/// Walk the repo, classifying each directory as source-to-parse or a collapsed
/// region. Returns `(files_to_parse, region_anchors)`.
type WalkResult = (
    Vec<(String, String)>, // source files to parse
    Vec<RegionAnchor>,     // collapsed build/vendor regions
    Vec<(String, String)>, // markdown docs (rel_path, text) — G18
);

fn walk_source_files(root: &Path) -> WalkResult {
    let mut files = Vec::new();
    let mut regions = Vec::new();
    let mut md = Vec::new();
    let gitignore_dirs = load_gitignore_dirs(root);
    walk_dir(root, root, &gitignore_dirs, &mut files, &mut regions, &mut md);
    (files, regions, md)
}

/// VCS internals and editor metadata: no graph-meaningful content, skipped
/// outright (not even recorded as a region).
fn is_hard_skip(name: &str) -> bool {
    matches!(name, ".git" | ".hg" | ".svn" | ".idea" | ".vscode")
}

/// Provenance for directories always collapsed to a region anchor regardless of
/// `.gitignore` — dependency trees and conventional build output. `None` for an
/// ordinary source directory.
fn always_region(name: &str) -> Option<&'static str> {
    match name {
        "node_modules" | "vendor" | "bower_components" | ".venv" | "site-packages" => {
            Some("vendored")
        }
        "target" | "dist" | "build" | "out" | "__pycache__" | ".cache" | ".next" | ".nuxt"
        | ".angular" | "coverage" => Some("build_output"),
        _ => None,
    }
}

/// Directory names the repo's top-level `.gitignore` marks ignored. Build
/// mirrors a project gitignores (Capacitor's `www/`, `android/`) collapse to a
/// region anchor rather than being parsed as source. Only plain, non-glob,
/// non-negated entries are honored, matched by final path component against
/// directory names during the walk. (glia-v2 G10)
fn load_gitignore_dirs(root: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Ok(text) = std::fs::read_to_string(root.join(".gitignore")) else {
        return out;
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with('!')
            || line.contains('*')
            || line.contains('?')
            || line.contains('[')
        {
            continue;
        }
        let trimmed = line.trim_matches('/');
        let comp = trimmed.rsplit('/').next().unwrap_or(trimmed);
        if !comp.is_empty() {
            out.insert(comp.to_string());
        }
    }
    out
}

/// True for a filename that looks like a bundler-emitted, content-hashed chunk
/// (`main.e188fddd19255ba1.js`, `1624.4e9cc6119b4878fe.js`, `styles.<hash>.css`)
/// — i.e. build output, not authored source. The signal is a dot-delimited
/// segment of ≥8 hex digits before a JS/CSS extension.
fn is_hashed_chunk(name: &str) -> bool {
    let ext_ok = name.ends_with(".js")
        || name.ends_with(".mjs")
        || name.ends_with(".css")
        || name.ends_with(".map");
    if !ext_ok {
        return false;
    }
    name.split('.').any(|seg| {
        seg.len() >= 8 && seg.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// True when a directory is a built web-bundle mirror — it directly contains
/// several content-hashed chunk files. Catches Capacitor's copied bundle
/// (`android/app/src/main/assets/public`, `ios/App/App/public`) and any other
/// build mirror that `.gitignore` doesn't flag, regardless of path. (glia-v2 G10)
fn dir_is_build_bundle(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut hashed = 0usize;
    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            && is_hashed_chunk(&entry.file_name().to_string_lossy())
        {
            hashed += 1;
            if hashed >= 3 {
                return true;
            }
        }
    }
    false
}

fn walk_dir(
    root: &Path,
    dir: &Path,
    gitignore_dirs: &std::collections::HashSet<String>,
    files: &mut Vec<(String, String)>,
    regions: &mut Vec<RegionAnchor>,
    md: &mut Vec<(String, String)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if is_hard_skip(&name) {
            continue;
        }
        if path.is_dir() {
            // Collapse vendored/build/gitignored directories to one anchor and
            // do NOT descend — categorise the region instead of dropping it or
            // emitting a node per file inside. (glia-v2 G1/G2/G10)
            let provenance = always_region(&name)
                .or_else(|| gitignore_dirs.contains(&name).then_some("build_output"))
                .or_else(|| dir_is_build_bundle(&path).then_some("build_output"));
            if let Some(provenance) = provenance {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                regions.push(RegionAnchor {
                    rel_path: rel.to_string_lossy().to_string(),
                    provenance,
                    region: name,
                });
                continue;
            }
            walk_dir(root, &path, gitignore_dirs, files, regions, md);
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().to_string();
            // Markdown docs (G18) — collected separately; the include/skip rules
            // are applied in `build_docs_graph`.
            if rel_str.to_ascii_lowercase().ends_with(".md")
                && std::fs::metadata(&path).map(|m| m.len() <= 500_000).unwrap_or(false)
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                md.push((rel_str.clone(), text));
                continue;
            }
            let matches_lang = detect_language(&rel_str).is_some();
            let matches_bypass = is_bypass_path(&rel_str);
            if (matches_lang || matches_bypass)
                && let Ok(source) = std::fs::read_to_string(&path)
            {
                files.push((rel_str, source));
            }
        }
    }
}

/// Build a one-node-per-region graph from the collapsed [`RegionAnchor`]s. Each
/// node carries an `ORIGIN` cell `{provenance, region}` so consumers filter by
/// coordinate instead of string-matching keys. (glia-v2 G1/G10)
fn build_region_graph(regions: &[RegionAnchor], repo: RepoId) -> repo_graph_graph::RepoGraph {
    use repo_graph_code_domain::cell_type;
    use repo_graph_core::{Cell, CellPayload};

    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    for r in regions {
        let qname = format!("region:{}", r.rel_path);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::REGION, &qname);
        let origin = format!(
            r#"{{"provenance":"{}","region":"{}"}}"#,
            r.provenance, r.region
        );
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(origin),
            }],
        });
        nav.record(id, &r.region, &qname, node_kind::REGION, None);
    }
    repo_graph_graph::RepoGraph {
        repo,
        nodes,
        edges: Vec::new(),
        nav,
        symbols: Default::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    }
}

fn is_bypass_path(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if matches!(ext, "yml" | "yaml") {
        return true;
    }
    is_dockerfile_path(path)
        || is_dotenv_path(path)
        || repo_graph_code_extractors::packages::is_manifest_path(path)
}

// ----------------------------------------------------------------------------
// G18 — external `.md` doc ingest (README/ARCHITECTURE/docs/) → prose nodes
// the exporter maps to `Content::Proposition` with provenance `documentation`.
// ----------------------------------------------------------------------------

/// Repo-root markdown files worth ingesting as documentation.
fn is_wellknown_doc(rel: &str) -> bool {
    matches!(
        rel,
        "README.md"
            | "ARCHITECTURE.md"
            | "CHANGELOG.md"
            | "CONTRIBUTING.md"
            | "CODE_OF_CONDUCT.md"
            | "CLAUDE.md"
            | "AGENTS.md"
            | "CODE_RULES.md"
    )
}

/// Should this markdown path be ingested? Root well-known files, anything under
/// a top-level `docs/`, or under `.ai/` (≤2 levels). License boilerplate skipped.
fn include_doc(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    if lower.ends_with("license.md") || lower.contains("license") {
        return false;
    }
    if is_wellknown_doc(rel) {
        return true;
    }
    rel.starts_with("docs/") || rel.starts_with(".ai/")
}

/// A slug for a heading line: lowercased, alnum runs joined by `-` (GitHub anchor).
fn heading_slug(heading: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in heading.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Cap to 500 chars, truncating at a sentence boundary when possible.
fn cap_prose(s: &str) -> String {
    const MAX: usize = 500;
    let s = s.trim();
    if s.len() <= MAX {
        return s.to_string();
    }
    let mut end = MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let slice = &s[..end];
    // prefer the last sentence end within the cap
    if let Some(dot) = slice.rfind(". ") {
        return slice[..=dot].trim().to_string();
    }
    slice.trim_end().to_string()
}

struct DocChunk {
    slug: String,
    text: String,
    start_line: u32,
    end_line: u32,
}

/// Split markdown at `#`/`##` headings into chunks. Falls back to one chunk
/// (first 500 chars) when the file has no headings.
fn chunk_markdown(text: &str) -> Vec<DocChunk> {
    let lines: Vec<&str> = text.lines().collect();
    let mut chunks: Vec<DocChunk> = Vec::new();
    let mut cur_slug: Option<String> = None;
    let mut cur_start = 0u32;
    let mut buf: Vec<&str> = Vec::new();
    let mut seq = 0u32;

    let flush = |chunks: &mut Vec<DocChunk>,
                 slug: &Option<String>,
                 buf: &[&str],
                 start: u32,
                 end: u32,
                 seq: &mut u32| {
        let body = cap_prose(&buf.join("\n"));
        if body.is_empty() {
            return;
        }
        let slug = slug.clone().unwrap_or_else(|| {
            let s = if *seq == 0 {
                "overview".to_string()
            } else {
                format!("section-{seq}")
            };
            *seq += 1;
            s
        });
        chunks.push(DocChunk { slug, text: body, start_line: start, end_line: end });
    };

    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("# ") || t.starts_with("## ") {
            flush(&mut chunks, &cur_slug, &buf, cur_start, i as u32, &mut seq);
            buf.clear();
            cur_slug = Some(heading_slug(t.trim_start_matches('#').trim()));
            cur_start = i as u32;
            buf.push(line);
        } else {
            buf.push(line);
        }
    }
    flush(
        &mut chunks,
        &cur_slug,
        &buf,
        cur_start,
        lines.len() as u32,
        &mut seq,
    );

    // Fallback: no headings → one chunk of the whole file.
    if chunks.is_empty() {
        let body = cap_prose(text);
        if !body.is_empty() {
            chunks.push(DocChunk { slug: "overview".into(), text: body, start_line: 0, end_line: lines.len() as u32 });
        }
    }
    chunks
}

/// Build a graph of `DOC_SECTION` nodes from the repo's markdown docs. Each node
/// carries the prose in a CODE cell, a POSITION cell (md path + line range), and
/// an ORIGIN cell `provenance=documentation`. The exporter maps the kind to
/// `Content::Proposition`. (glia-v5 G18)
fn build_docs_graph(md: &[(String, String)], repo: RepoId) -> Option<repo_graph_graph::RepoGraph> {
    use repo_graph_code_domain::cell_type;
    use repo_graph_core::{Cell, CellPayload};

    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    for (path, text) in md {
        if !include_doc(path) {
            continue;
        }
        let stem = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("doc")
            .to_string();
        for chunk in chunk_markdown(text) {
            let qname = format!("docs::{stem}::{}", chunk.slug);
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DOC_SECTION, &qname);
            let pos = format!(
                r#"{{"file":"{}","start_line":{},"end_line":{}}}"#,
                path.replace('\\', "\\\\").replace('"', "\\\""),
                chunk.start_line,
                chunk.end_line
            );
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![
                    Cell { kind: cell_type::CODE, payload: CellPayload::Text(chunk.text) },
                    Cell { kind: cell_type::POSITION, payload: CellPayload::Json(pos) },
                    Cell {
                        kind: cell_type::ORIGIN,
                        payload: CellPayload::Json(r#"{"provenance":"documentation"}"#.into()),
                    },
                ],
            });
            nav.record(id, &chunk.slug, &qname, node_kind::DOC_SECTION, None);
        }
    }
    if nodes.is_empty() {
        return None;
    }
    Some(repo_graph_graph::RepoGraph {
        repo,
        nodes,
        edges: Vec::new(),
        nav,
        symbols: Default::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    })
}

fn is_dockerfile_path(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    base == "dockerfile"
        || base.ends_with(".dockerfile")
        || base.starts_with("dockerfile.")
}

fn is_dotenv_path(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    base == ".env" || base.starts_with(".env.")
}

fn stash_synthetic_parse(
    lang_key: &'static str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    node_groups: Vec<Vec<Node>>,
    edge_groups: Vec<Vec<Edge>>,
    nav_groups: Vec<CodeNav>,
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
) {
    let mut nodes = vec![Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: vec![],
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

fn merge_nav(dst: &mut CodeNav, src: CodeNav) {
    dst.name_by_id.extend(src.name_by_id);
    dst.qname_by_id.extend(src.qname_by_id);
    dst.kind_by_id.extend(src.kind_by_id);
    dst.parent_of.extend(src.parent_of);
    for (k, v) in src.children_of {
        dst.children_of.entry(k).or_default().extend(v);
    }
}

fn path_to_qname(path: &str) -> String {
    Path::new(path)
        .with_extension("")
        .to_string_lossy()
        .replace(['/', '\\'], "::")
}

// ----------------------------------------------------------------------------
// Post passes
// ----------------------------------------------------------------------------

fn emit_tests_edges(merged: &mut MergedGraph) {
    let mut modules_by_tail: HashMap<String, Vec<(NodeId, String)>> = HashMap::new();
    let mut module_info: Vec<(NodeId, String)> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id).copied() != Some(node_kind::MODULE) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            module_info.push((n.id, qname.clone()));
            if let Some(tail) = qname.rsplit("::").next() {
                modules_by_tail
                    .entry(tail.to_string())
                    .or_default()
                    .push((n.id, qname.clone()));
            }
        }
    }
    for (from_id, qname) in &module_info {
        if !is_test_qname(qname) {
            continue;
        }
        let Some(tail) = qname.rsplit("::").next() else { continue };
        let stripped = strip_test_affixes(tail);
        if stripped.is_empty() || stripped == tail {
            continue;
        }
        let Some(candidates) = modules_by_tail.get(stripped) else { continue };
        for to_id in select_test_targets(*from_id, qname, candidates) {
            merged.cross_edges.push(Edge {
                from: *from_id,
                to: to_id,
                category: edge_category::TESTS,
                confidence: Confidence::Strong,
            });
        }
    }
}

fn select_test_targets(
    from_id: NodeId,
    test_qname: &str,
    candidates: &[(NodeId, String)],
) -> Vec<NodeId> {
    const MAX_TEST_TARGETS: usize = 3;
    let test_parent: Vec<&str> = qname_parent_segments(test_qname);
    let mut scored: Vec<(usize, NodeId)> = candidates
        .iter()
        .filter(|(id, _)| *id != from_id)
        .map(|(id, qn)| {
            let cand_parent = qname_parent_segments(qn);
            (common_prefix_len(&test_parent, &cand_parent), *id)
        })
        .collect();
    if scored.is_empty() {
        return Vec::new();
    }
    let max_score = scored.iter().map(|(s, _)| *s).max().unwrap_or(0);
    scored.retain(|(s, _)| *s == max_score);
    scored.truncate(MAX_TEST_TARGETS);
    scored.into_iter().map(|(_, id)| id).collect()
}

fn qname_parent_segments(qname: &str) -> Vec<&str> {
    let mut segs: Vec<&str> = qname.split("::").collect();
    segs.pop();
    segs
}

fn common_prefix_len(a: &[&str], b: &[&str]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

fn downgrade_test_paths(merged: &mut MergedGraph) {
    for g in &mut merged.graphs {
        for n in &mut g.nodes {
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            if qname.starts_with("route:") {
                continue;
            }
            if qname_is_noncritical_path(qname) {
                n.confidence = Confidence::Weak;
            }
        }
    }
}

fn demote_unmatched_http_nodes(merged: &mut MergedGraph) {
    use std::collections::HashSet;
    let mut matched: HashSet<NodeId> = HashSet::new();
    for e in &merged.cross_edges {
        if e.category == edge_category::HTTP_CALLS {
            matched.insert(e.from);
            matched.insert(e.to);
        }
    }
    for g in &mut merged.graphs {
        for n in &mut g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            let is_http_node = matches!(kind, Some(k) if k == node_kind::ROUTE || k == node_kind::ENDPOINT);
            if !is_http_node {
                continue;
            }
            if matches!(n.confidence, Confidence::Weak) {
                continue;
            }
            if !matched.contains(&n.id) {
                n.confidence = Confidence::Medium;
            }
        }
    }
}

fn qname_is_noncritical_path(qname: &str) -> bool {
    const NONCRITICAL: &[&str] = &[
        "tests", "test", "__tests__", "spec", "specs",
        "fixtures", "fixture", "examples", "example",
        "e2e", "__mocks__", "mocks", "testdata",
    ];
    qname.split("::").any(|seg| {
        let lowered = seg.to_ascii_lowercase();
        NONCRITICAL.contains(&lowered.as_str())
    })
}

fn is_test_qname(qname: &str) -> bool {
    let lowered = qname.to_ascii_lowercase();
    if lowered.contains("::tests::")
        || lowered.contains("::test::")
        || lowered.contains("::__tests__::")
        || lowered.contains("::spec::")
        || lowered.starts_with("tests::")
        || lowered.starts_with("test::")
        || lowered.starts_with("spec::")
    {
        return true;
    }
    let Some(tail) = qname.rsplit("::").next() else {
        return false;
    };
    let t = tail.to_ascii_lowercase();
    t.starts_with("test_")
        || t.ends_with("_test")
        || t.ends_with("_spec")
        || t.ends_with(".test")
        || t.ends_with(".spec")
}

fn strip_test_affixes(name: &str) -> &str {
    let lowered = name.to_ascii_lowercase();
    if let Some(rest) = lowered.strip_prefix("test_") {
        return &name[name.len() - rest.len()..];
    }
    for suffix in ["_test", "_spec", ".test", ".spec"] {
        if lowered.ends_with(suffix) {
            return &name[..name.len() - suffix.len()];
        }
    }
    name
}

#[cfg(test)]
mod walk_tests {
    use super::*;

    #[test]
    fn hashed_chunk_detection() {
        assert!(is_hashed_chunk("main.e188fddd19255ba1.js"));
        assert!(is_hashed_chunk("1624.4e9cc6119b4878fe.js"));
        assert!(is_hashed_chunk("styles.0a1b2c3d4e5f6a7b.css"));
        // Authored source is not a hashed chunk.
        assert!(!is_hashed_chunk("app.component.ts"));
        assert!(!is_hashed_chunk("index.js"));
        assert!(!is_hashed_chunk("user_service.py"));
        // 8-hex-ish word but wrong extension.
        assert!(!is_hashed_chunk("deadbeef.txt"));
    }

    #[test]
    fn markdown_chunking_and_include_rules() {
        // Heading-split chunking (G18).
        let md = "# Overview\nIntro text.\n## Setup\nRun the thing.\n";
        let chunks = chunk_markdown(md);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].slug, "overview");
        assert_eq!(chunks[1].slug, "setup");
        // Fallback: no headings → one "overview" chunk.
        let plain = chunk_markdown("just a paragraph with no heading at all.");
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].slug, "overview");
        // Include rules.
        assert!(include_doc("README.md"));
        assert!(include_doc("docs/architecture.md"));
        assert!(!include_doc("LICENSE.md"));
        assert!(!include_doc("src/notes.md")); // not root-wellknown / docs/ / .ai/
    }

    #[test]
    fn proto_and_test_fixture_detection() {
        // generated_proto: codegen extensions across languages (glia-v3 #5).
        assert!(is_generated_proto("chatpb/chat.pb.go"));
        assert!(is_generated_proto("gen/chat.pb-grpc.go"));
        assert!(is_generated_proto("lib/proto/chat.pbjson.dart"));
        assert!(is_generated_proto("proto/chat_pb2.py"));
        assert!(!is_generated_proto("src/chat.go"));
        assert!(!is_generated_proto("src/app/chat.component.ts"));
        // test_fixture: paths + qname shapes (glia-v3 #6).
        assert!(is_test_fixture("services/auth_test.go", "turps::auth"));
        assert!(is_test_fixture("app/login.spec.ts", "quokka_web::login"));
        assert!(is_test_fixture("pkg/foo.go", "pkg::tests::seed_users"));
        assert!(!is_test_fixture("services/auth.go", "turps::auth::HashPassword"));
    }

    #[test]
    fn position_file_extraction() {
        use repo_graph_code_domain::cell_type;
        use repo_graph_core::{Cell, CellPayload};
        let cells = vec![Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(
                r#"{"file":"src/app/a.ts","start_line":3,"end_line":9}"#.into(),
            ),
        }];
        assert_eq!(position_file(&cells).as_deref(), Some("src/app/a.ts"));
        assert_eq!(position_file(&[]), None);
    }

    #[test]
    fn gitignore_dir_parsing() {
        let root = std::env::temp_dir().join(format!("glia_gi_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(".gitignore"),
            "# comment\n/www\nandroid/\n*.log\n!keep\n/dist\nsrc/generated\n",
        )
        .unwrap();
        let dirs = load_gitignore_dirs(&root);
        assert!(dirs.contains("www"));
        assert!(dirs.contains("android"));
        assert!(dirs.contains("dist"));
        assert!(dirs.contains("generated")); // final component of src/generated
        assert!(!dirs.contains("keep")); // negation skipped
        assert!(dirs.iter().all(|d| !d.contains('*'))); // globs skipped
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn walk_collapses_build_and_vendor_regions() {
        let root = std::env::temp_dir().join(format!("glia_walk_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/left-pad")).unwrap();
        std::fs::create_dir_all(root.join("www")).unwrap();
        std::fs::create_dir_all(root.join("android/app/src/main/assets/public")).unwrap();
        std::fs::write(root.join(".gitignore"), "/www\n").unwrap();
        // authored source
        std::fs::write(root.join("src/app/a.ts"), "export class A {}").unwrap();
        // vendored dep (must NOT be parsed)
        std::fs::write(root.join("node_modules/left-pad/index.js"), "module.exports=1").unwrap();
        // gitignored build mirror
        std::fs::write(root.join("www/main.abc12345def0.js"), "var a=1").unwrap();
        // capacitor bundle mirror NOT in .gitignore — caught by hash-chunk probe
        let pub_dir = root.join("android/app/src/main/assets/public");
        for h in ["1624.4e9cc6119b4878fe", "1102.7837812dd7ed4d51", "2075.f756d5b13b56050a"] {
            std::fs::write(pub_dir.join(format!("{h}.js")), "var b=2").unwrap();
        }

        let (files, regions, _md) = walk_source_files(&root);

        // Only the authored source file is queued for parsing.
        assert_eq!(files.len(), 1, "files: {files:?}");
        assert!(files[0].0.ends_with("a.ts"));

        let region_paths: Vec<&str> = regions.iter().map(|r| r.rel_path.as_str()).collect();
        assert!(region_paths.contains(&"node_modules"), "{region_paths:?}");
        assert!(region_paths.contains(&"www"), "{region_paths:?}");
        assert!(
            region_paths.contains(&"android/app/src/main/assets/public"),
            "hash-chunk bundle mirror should collapse: {region_paths:?}"
        );
        // node_modules is vendored; the bundle mirrors are build_output.
        let nm = regions.iter().find(|r| r.rel_path == "node_modules").unwrap();
        assert_eq!(nm.provenance, "vendored");
        let pub_region = regions
            .iter()
            .find(|r| r.rel_path.ends_with("assets/public"))
            .unwrap();
        assert_eq!(pub_region.provenance, "build_output");

        let _ = std::fs::remove_dir_all(&root);
    }
}
