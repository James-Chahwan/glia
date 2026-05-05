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

use std::collections::HashMap;
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
    let files = walk_source_files(&root);
    let (graphs, mut parse_errors) = build_graphs_for_repo(&files, repo);
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
        let files = walk_source_files(&root);
        let (graphs, parse_errors) = build_graphs_for_repo(&files, repo);
        all_graphs.extend(graphs);
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

        match parse_one(source, path, lang, repo) {
            Ok(mut fp) => {
                let module_id = NodeId::from_parts(
                    GRAPH_TYPE,
                    repo,
                    node_kind::MODULE,
                    &path_to_qname(path),
                );
                apply_cross_cutting_extractors(&mut fp, source, path, lang, module_id, repo);
                parses_by_lang.entry(lang).or_default().push(fp);
            }
            Err(e) => {
                parse_errors.push(format!("{path}: {e}"));
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
        react, ts_routes, vue, websocket,
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
}

// ----------------------------------------------------------------------------
// Helpers — file walk, path mapping, nav merge
// ----------------------------------------------------------------------------

fn walk_source_files(root: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    walk_dir(root, root, &mut files);
    files
}

fn walk_dir(root: &Path, dir: &Path, files: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(
            name.as_str(),
            ".git" | ".hg" | ".svn" | ".cache" | ".venv" | ".idea" | ".vscode"
                | "node_modules" | "vendor" | "__pycache__" | "target" | "dist" | "build"
        ) {
            continue;
        }
        if path.is_dir() {
            walk_dir(root, &path, files);
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().to_string();
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
