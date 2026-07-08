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

use repo_graph_code_domain::{
    CodeNav, DocProvenance, DocRecord, FileParse, GRAPH_TYPE, edge_category, node_kind,
};
use repo_graph_core::{CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use repo_graph_graph::{
    CliInvocationResolver, ConfigResolver, CronResolver, DbResolver, EventBusResolver,
    GraphQLStackResolver, GrpcStackResolver, HttpStackResolver, IacResolver, MergedGraph,
    PackageResolver, QueueStackResolver, Reach, SharedSchemaResolver, WebSocketStackResolver,
};

pub use repo_graph_graph::MergedGraph as ReExportedMergedGraph;

pub mod cache;
pub use cache::{CacheStats, ParseCache};

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
    generate_one_inner(repo_path, None)
}

/// Incremental build using an in-memory [`ParseCache`] (WP-D): unchanged files
/// skip tree-sitter. Hold one `cache` across edits (e.g. neuropil's hot-reload).
/// The result is byte-identical to [`generate_one`] — only the parse step is
/// elided; the graph is rebuilt and resolvers re-run in full.
pub fn generate_one_with_cache(
    repo_path: &str,
    cache: &mut ParseCache,
) -> Result<GenerateResult, String> {
    generate_one_inner(repo_path, Some(cache))
}

/// Disk-backed incremental build: load the parse cache from
/// `<repo>/.ai/repo-graph/parse_cache.bin`, build, then persist it. Cache save
/// failures are logged, not fatal. Backs pyo3 `generate(incremental=True)`.
pub fn generate_one_incremental(repo_path: &str) -> Result<GenerateResult, String> {
    let mut cache = ParseCache::load(repo_path);
    let result = generate_one_inner(repo_path, Some(&mut cache))?;
    if let Err(e) = cache.save(repo_path) {
        eprintln!("[incremental] warning: failed to save parse cache: {e}");
    }
    Ok(result)
}

fn generate_one_inner(
    repo_path: &str,
    mut cache: Option<&mut ParseCache>,
) -> Result<GenerateResult, String> {
    let root = PathBuf::from(repo_path);
    if !root.is_dir() {
        return Err(format!("not a directory: {repo_path}"));
    }
    let canonical = format!("file://{repo_path}");
    let repo = RepoId::from_canonical(&canonical);
    let (files, regions, md) = walk_source_files(&root);
    let go_prefix = read_go_module_prefix(&root);
    // Cached parses are only valid under the exact repo identity + go.mod
    // module they were built with — neither is visible to per-file hashes.
    if let Some(c) = cache.as_deref_mut() {
        c.validate_context(&canonical, &go_prefix);
    }
    let (mut graphs, mut parse_errors) = build_graphs_for_repo(&files, repo, &go_prefix, cache);
    if !regions.is_empty() {
        graphs.push(build_region_graph(&regions, repo));
    }
    let mut doc_records = FileDocSource(md).collect();
    doc_records.extend(SnapshotDocSource::for_repo(&root).collect());
    if let Some(docs) = build_docs_graph(&doc_records, repo) {
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
        let go_prefix = read_go_module_prefix(&root);
        let (graphs, parse_errors) = build_graphs_for_repo(&files, repo, &go_prefix, None);
        all_graphs.extend(graphs);
        if !regions.is_empty() {
            all_graphs.push(build_region_graph(&regions, repo));
        }
        let mut doc_records = FileDocSource(md).collect();
        doc_records.extend(SnapshotDocSource::for_repo(&root).collect());
        if let Some(docs) = build_docs_graph(&doc_records, repo) {
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

/// Read the `module` path from a repo's `go.mod` (e.g. `github.com/foo/bar`),
/// or `""` if there's no go.mod. The Go parser uses it to tell internal package
/// imports from external libraries (WP-G / #6).
fn read_go_module_prefix(root: &Path) -> String {
    std::fs::read_to_string(root.join("go.mod"))
        .ok()
        .and_then(|s| {
            s.lines()
                .map(str::trim)
                .find_map(|l| l.strip_prefix("module ").map(|m| m.trim().to_string()))
        })
        .unwrap_or_default()
}

fn build_graphs_for_repo(
    files: &[(String, String)],
    repo: RepoId,
    go_module_prefix: &str,
    mut cache: Option<&mut ParseCache>,
) -> (Vec<repo_graph_graph::RepoGraph>, Vec<String>) {
    let mut parses_by_lang: HashMap<&str, Vec<FileParse>> = HashMap::new();
    let mut proto_parses = Vec::new();
    let mut parse_errors = Vec::new();
    // WP-D incremental: track which main-parser files we saw so deleted files
    // get evicted; count reuse vs reparse for the marker.
    let mut live_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut reused = 0usize;
    let mut reparsed = 0usize;

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
        eprintln!(
            "[incremental] reused {reused}, reparsed {reparsed}, evicted {} (parse cache)",
            c.stats.evicted
        );
    }

    let mut graphs = Vec::new();
    // Deterministic per-language build order: HashMap iteration is seeded per
    // process, and the resulting `graphs` order decides shard indices in
    // `write_sharded` (repo-<hash>-NN.gmap). Random order made every shard's
    // content hash flap across processes, so the write-side skip-unchanged-
    // shards optimization never fired (audit 2026-06-10 #5).
    let mut parses_by_lang: Vec<(&str, Vec<FileParse>)> = parses_by_lang.into_iter().collect();
    parses_by_lang.sort_unstable_by_key(|(lang, _)| *lang);
    // TS-family lang tags (typescript/angular/react/vue) share ONE module + symbol
    // space in a repo: an Angular component (`.component.ts` → "angular") injects a
    // service (`.service.ts` → "typescript"), and imports cross those tags. Build
    // them as a single graph so intra-repo ref/import resolution works across the
    // tag boundary (Pattern E DI, Pattern B imports). Other `_`-arm langs
    // (dart/swift/c_cpp/solidity/terraform) keep separate graphs — distinct symbol
    // spaces that must not cross-resolve. ts_family accumulates in the sorted lang
    // order and is built last, so graph/shard order stays deterministic.
    const TS_FAMILY: &[&str] = &["angular", "react", "typescript", "vue"];
    let mut ts_family: Vec<FileParse> = Vec::new();
    for (lang, parses) in parses_by_lang {
        if TS_FAMILY.contains(&lang) {
            ts_family.extend(parses);
            continue;
        }
        let graph = match lang {
            "python" => repo_graph_graph::build_python(repo, parses),
            "go" => repo_graph_graph::build_go(repo, parses),
            "java" | "csharp" | "php" | "rust" | "scala" | "clojure" | "elixir" => {
                repo_graph_graph::build_dotted(repo, parses)
            }
            "ruby" => repo_graph_graph::build_ruby(repo, parses),
            _ => repo_graph_graph::build_typescript(repo, parses, resolve_relative_source),
        };
        match graph {
            Ok(g) => graphs.push(g),
            Err(e) => parse_errors.push(format!("{lang} graph: {e}")),
        }
    }
    if !ts_family.is_empty() {
        match repo_graph_graph::build_typescript(repo, ts_family, resolve_ts_source) {
            Ok(g) => graphs.push(g),
            Err(e) => parse_errors.push(format!("typescript graph: {e}")),
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
    link_doc_sections(merged);
    tag_synthetic_provenance(merged);
    // Deterministic cross-edge order: several resolvers emit pairs by
    // iterating HashMap indexes (per-process seed), so the edge SET was stable
    // but its Vec order — and therefore cross_stack.gmap's bytes — flapped
    // across processes and even clean-vs-incremental in one process (audit
    // 2026-06-10 #6). One sort here covers all resolvers and post-passes.
    merged
        .cross_edges
        .sort_unstable_by_key(|e| (e.from.0, e.to.0, e.category.0, e.confidence as u8));
}

/// WP-H / #7: link `.md` DOC_SECTION nodes to the code symbols they document so
/// doc nodes aren't islands. High-precision signal — backtick-quoted
/// identifiers in the markdown (`` `MyClass` ``, `` `parse_file()` ``) — matched
/// against code-symbol names. Emits DOCUMENTS cross-edges (doc → symbol), capped
/// per doc node to bound noise.
fn link_doc_sections(merged: &mut MergedGraph) {
    use repo_graph_code_domain::cell_type;
    use repo_graph_core::CellPayload;

    // Index code symbols by simple name; lowest NodeId wins (deterministic).
    let mut symbol_by_name: HashMap<String, NodeId> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let Some(kind) = g.nav.kind_by_id.get(&n.id).copied() else {
                continue;
            };
            if !is_doc_linkable_symbol(kind) {
                continue;
            }
            let Some(name) = g.nav.name_by_id.get(&n.id) else {
                continue;
            };
            symbol_by_name
                .entry(name.clone())
                .and_modify(|cur| {
                    if n.id.0 < cur.0 {
                        *cur = n.id;
                    }
                })
                .or_insert(n.id);
        }
    }
    if symbol_by_name.is_empty() {
        return;
    }

    const MAX_LINKS_PER_DOC: usize = 25;
    let mut new_edges: Vec<Edge> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id).copied() != Some(node_kind::DOC_SECTION) {
                continue;
            }
            let Some(text) = n.cells.iter().find_map(|c| match &c.payload {
                CellPayload::Text(s) if c.kind == cell_type::CODE => Some(s.as_str()),
                _ => None,
            }) else {
                continue;
            };
            let mut seen: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
            for ident in backtick_identifiers(text) {
                if let Some(&sym) = symbol_by_name.get(&ident) {
                    if sym != n.id && seen.insert(sym) {
                        new_edges.push(Edge {
                            from: n.id,
                            to: sym,
                            category: edge_category::DOCUMENTS,
                            confidence: Confidence::Medium,
                        });
                        if seen.len() >= MAX_LINKS_PER_DOC {
                            break;
                        }
                    }
                }
            }
        }
    }
    merged.cross_edges.extend(new_edges);
}

/// Node kinds a doc section can meaningfully document.
fn is_doc_linkable_symbol(kind: repo_graph_core::NodeKindId) -> bool {
    use repo_graph_code_domain::node_kind as nk;
    kind == nk::FUNCTION
        || kind == nk::METHOD
        || kind == nk::CLASS
        || kind == nk::STRUCT
        || kind == nk::INTERFACE
        || kind == nk::ENUM
        || kind == nk::COMPONENT
        || kind == nk::SERVICE
        || kind == nk::STATE_VAR
        || kind == nk::DATA_ENTITY
}

/// Identifiers inside single-backtick inline-code spans in markdown. Triple-
/// backtick fenced blocks fall on even split segments and are skipped.
fn backtick_identifiers(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, seg) in text.split('`').enumerate() {
        if i % 2 == 1 {
            if let Some(id) = identifier_from_span(seg) {
                out.push(id);
            }
        }
    }
    out
}

/// Reduce an inline-code span to a bare identifier: drop trailing `()`, take the
/// last `.`/`::` segment, require an identifier ≥3 chars. `None` if not one.
fn identifier_from_span(span: &str) -> Option<String> {
    let s = span.trim().trim_end_matches("()");
    let s = s.rsplit(|c| c == '.' || c == ':').next().unwrap_or(s);
    if s.len() < 3 {
        return None;
    }
    let mut chars = s.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(s.to_string())
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
    // Sort by name: read_dir yields filesystem/inode order, which leaked into
    // node/edge Vec order (and so shard bytes) — stable-ish on one machine,
    // not reproducible across machines or after file churn (audit 2026-06-10).
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_unstable_by_key(|e| e.file_name());
    for entry in entries {
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
            // An emoji/punctuation-only heading slugs to "" — fall through to the
            // ordinal fallback (overview/section-N) so sections don't collide.
            let s = heading_slug(t.trim_start_matches('#').trim());
            cur_slug = if s.is_empty() { None } else { Some(s) };
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

// ============================================================================
// Doc ingestion seam (Tier-4)
// ============================================================================
//
// `build_docs_graph` consumes `DocRecord`s from any `DocSource` instead of the
// raw file-walk pairs, so external adapters (Confluence/Notion/wiki, in the
// future `doc-sources` crate) feed the SAME DOC_SECTION builder + downstream
// `link_doc_sections`. `FileDocSource` reproduces the repo `.md` walk exactly.

/// Produces the documents to ingest. The repo file walk is `FileDocSource`;
/// external adapters implement this against a fetched snapshot.
trait DocSource {
    fn collect(self) -> Vec<DocRecord>;
}

/// Today's source: the `(rel_path, text)` markdown pairs the file walk collected.
/// Byte-identical to the pre-seam path — provenance is `File` and no new cell is
/// emitted.
struct FileDocSource(Vec<(String, String)>);

impl DocSource for FileDocSource {
    fn collect(self) -> Vec<DocRecord> {
        self.0
            .into_iter()
            .map(|(rel_path, text)| DocRecord {
                rel_path,
                text,
                provenance: DocProvenance::file(),
            })
            .collect()
    }
}

/// External docs previously fetched by `glia docs sync` into the repo's
/// gitignored `.glia/docs-snapshot/manifest.jsonl` (one `DocRecord` JSON per
/// line). Reading a LOCAL snapshot is deterministic — the non-deterministic
/// network fetch that produced it is a separate step — so the byte-identical
/// build gate stays valid. No snapshot → no external docs (repos without one
/// are byte-identical to before).
struct SnapshotDocSource {
    manifest: PathBuf,
}

impl SnapshotDocSource {
    fn for_repo(root: &Path) -> Self {
        Self {
            manifest: root
                .join(".glia")
                .join("docs-snapshot")
                .join("manifest.jsonl"),
        }
    }
}

impl DocSource for SnapshotDocSource {
    fn collect(self) -> Vec<DocRecord> {
        let Ok(content) = std::fs::read_to_string(&self.manifest) else {
            return Vec::new();
        };
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<DocRecord>(l).ok())
            .collect()
    }
}

/// Build a graph of `DOC_SECTION` nodes from the repo's markdown docs. Each node
/// carries the prose in a CODE cell, a POSITION cell (md path + line range), and
/// an ORIGIN cell `provenance=documentation`. The exporter maps the kind to
/// `Content::Proposition`. (glia-v5 G18)
fn build_docs_graph(records: &[DocRecord], repo: RepoId) -> Option<repo_graph_graph::RepoGraph> {
    use repo_graph_code_domain::{DocSourceKind, cell_type};
    use repo_graph_core::{Cell, CellPayload};

    fn esc(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }
    fn source_tag(k: DocSourceKind) -> &'static str {
        match k {
            DocSourceKind::File => "file",
            DocSourceKind::Confluence => "confluence",
            DocSourceKind::Notion => "notion",
            DocSourceKind::Wiki => "wiki",
        }
    }

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut nav = CodeNav::default();
    // Dedup DOC_SPACE nodes by qname (a space maps to many pages/records).
    let mut spaces: HashMap<String, NodeId> = HashMap::new();

    for rec in records {
        let (path, text) = (&rec.rel_path, &rec.text);
        let is_file = rec.provenance.kind == DocSourceKind::File;
        // `include_doc` gates repo files; external docs are pre-curated by sync.
        if is_file && !include_doc(path) {
            continue;
        }

        // A DOC_SPACE for an external container (Confluence space / Notion db /
        // wiki), emitted once. File docs have no container → stay flat, so their
        // output is byte-identical to before the seam.
        let space_id: Option<NodeId> = rec.provenance.container.as_deref().map(|container| {
            let tag = source_tag(rec.provenance.kind);
            let sqname = format!("docspace::{tag}::{container}");
            if let Some(&sid) = spaces.get(&sqname) {
                return sid;
            }
            let sid = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DOC_SPACE, &sqname);
            let origin = format!(
                r#"{{"provenance":"documentation","source":"{tag}","container":"{}"}}"#,
                esc(container)
            );
            nodes.push(Node {
                id: sid,
                repo,
                confidence: Confidence::Strong,
                cells: vec![Cell {
                    kind: cell_type::ORIGIN,
                    payload: CellPayload::Json(origin),
                }],
            });
            nav.record(sid, container, &sqname, node_kind::DOC_SPACE, None);
            spaces.insert(sqname, sid);
            sid
        });

        let stem = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("doc")
            .to_string();

        for chunk in chunk_markdown(text) {
            // File qname unchanged; external qnames are namespaced by container.
            let qname = match rec.provenance.container.as_deref() {
                Some(container) => format!("docs::{container}::{stem}::{}", chunk.slug),
                None => format!("docs::{stem}::{}", chunk.slug),
            };
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DOC_SECTION, &qname);
            let pos = format!(
                r#"{{"file":"{}","start_line":{},"end_line":{}}}"#,
                esc(path),
                chunk.start_line,
                chunk.end_line
            );
            // File docs keep today's exact ORIGIN cell (byte-identical); external
            // docs carry source + call-back url so a route traces to its page.
            let origin = if is_file {
                r#"{"provenance":"documentation"}"#.to_string()
            } else {
                let tag = source_tag(rec.provenance.kind);
                format!(
                    r#"{{"provenance":"documentation","source":"{tag}","url":"{}"}}"#,
                    esc(rec.provenance.url.as_deref().unwrap_or(""))
                )
            };
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![
                    Cell { kind: cell_type::CODE, payload: CellPayload::Text(chunk.text) },
                    Cell { kind: cell_type::POSITION, payload: CellPayload::Json(pos) },
                    Cell { kind: cell_type::ORIGIN, payload: CellPayload::Json(origin) },
                ],
            });
            nav.record(id, &chunk.slug, &qname, node_kind::DOC_SECTION, space_id);
            if let Some(sid) = space_id {
                edges.push(Edge {
                    from: sid,
                    to: id,
                    category: edge_category::CONTAINS,
                    confidence: Confidence::Strong,
                });
            }
        }
    }
    if nodes.is_empty() {
        return None;
    }
    Some(repo_graph_graph::RepoGraph {
        repo,
        nodes,
        edges,
        nav,
        symbols: Default::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    })
}

// ============================================================================
// P2 coverage / blind-spot signaling (handoff v6) — turn silent blind spots
// into declared ones so the graph+grep fallback is deliberate, not lucky.
// ============================================================================

/// A known extraction limitation for a language/edge-category. Advisory: an
/// agent that sees this should verify that dimension with grep rather than trust
/// a silent absence. `language == "*"` applies to every repo.
#[derive(serde::Serialize, Clone, Copy)]
pub struct CoverageCaveat {
    /// Language the caveat applies to (`"*"` = universal), matched to files present.
    pub language: &'static str,
    /// Edge category (or `"*"`) whose extraction is partial.
    pub edge_category: &'static str,
    /// What glia does NOT catch here.
    pub note: &'static str,
    /// The grep-shaped fallback to confirm completeness.
    pub verify: &'static str,
}

/// Declared coverage caveats — derived from the `bench/substrate-gap` eval +
/// documented residuals (`BLINDSPOTS.md`). Update when the matrix changes. The
/// universal (`"*"`) entries are the load-bearing honesty: static analysis
/// cannot see dynamic dispatch / string-built targets, so completeness there is
/// never guaranteed.
static COVERAGE_CAVEATS: &[CoverageCaveat] = &[
    CoverageCaveat {
        language: "*",
        edge_category: "CALLS",
        note: "calls through reflection, dynamic dispatch, or higher-order indirection are not resolved",
        verify: "grep the callee name",
    },
    CoverageCaveat {
        language: "*",
        edge_category: "HTTP_CALLS",
        note: "URLs built dynamically (string concat / variables / base-url config) may not pair to a route",
        verify: "grep the path literal or base URL",
    },
    CoverageCaveat {
        language: "python",
        edge_category: "HTTP_CALLS",
        note: "requests / httpx / aiohttp clients are extracted; urllib / http.client are not",
        verify: "grep urllib / http.client",
    },
    CoverageCaveat {
        language: "python",
        edge_category: "HANDLED_BY",
        note: "Flask typed route converters (e.g. <int:id>) may not normalize to the client's path param",
        verify: "check the @app.route decorator vs the caller path",
    },
    CoverageCaveat {
        language: "typescript",
        edge_category: "HTTP_CALLS",
        note: "fetch/axios at component or hook scope are extracted; calls hidden behind custom wrappers may be missed",
        verify: "grep fetch / axios / the wrapper name",
    },
    CoverageCaveat {
        language: "dart",
        edge_category: "HTTP_CALLS",
        note: "dio / http client verbs are extracted; other HTTP libraries are not",
        verify: "grep the HTTP client class name",
    },
];

/// One coverage note surfaced for a repo: a caveat that applies because the repo
/// contains that language.
#[derive(serde::Serialize)]
pub struct CoverageNote {
    pub language: &'static str,
    pub edge_category: &'static str,
    pub note: &'static str,
    pub verify: &'static str,
    /// How many edges of this category the graph actually holds (0 = extra
    /// reason to grep: either none exist or extraction missed them).
    pub edges_found: usize,
}

/// **coverage** (P2): for the languages actually present in the repo, the known
/// extraction caveats + how many edges of each flagged category were found —
/// so an agent falls back to grep deliberately where glia is known-partial
/// instead of trusting a silent blind spot. One call.
pub fn coverage_report(merged: &MergedGraph) -> Vec<CoverageNote> {
    let langs = languages_present(merged);
    let counts = edge_category_counts(merged);
    COVERAGE_CAVEATS
        .iter()
        .filter(|c| c.language == "*" || langs.contains(c.language))
        .map(|c| CoverageNote {
            language: c.language,
            edge_category: c.edge_category,
            note: c.note,
            verify: c.verify,
            edges_found: *counts.get(c.edge_category).unwrap_or(&0),
        })
        .collect()
}

/// Languages present in the repo, inferred from POSITION-cell file extensions.
fn languages_present(merged: &MergedGraph) -> std::collections::HashSet<&'static str> {
    let mut langs = std::collections::HashSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            for c in &n.cells {
                if c.kind != repo_graph_code_domain::cell_type::POSITION {
                    continue;
                }
                if let CellPayload::Json(s) | CellPayload::Text(s) = &c.payload {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                        if let Some(file) = v.get("file").and_then(|f| f.as_str()) {
                            if let Some(lang) = ext_to_language(file) {
                                langs.insert(lang);
                            }
                        }
                    }
                }
            }
        }
    }
    langs
}

/// Map a file path's extension to the analyzer language name used in caveats.
fn ext_to_language(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    Some(match ext {
        "py" => "python",
        "go" => "go",
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => "typescript",
        "dart" => "dart",
        "rs" => "rust",
        "java" => "java",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "scala" | "sc" => "scala",
        "sol" => "solidity",
        _ => return None,
    })
}

/// Count edges per category name across the merged graph (intra + cross).
fn edge_category_counts(merged: &MergedGraph) -> std::collections::HashMap<&'static str, usize> {
    let mut counts = std::collections::HashMap::new();
    for e in merged.all_edges() {
        *counts.entry(edge_category::name(e.category)).or_insert(0) += 1;
    }
    counts
}

// ============================================================================
// P3 answer-shaped primitives (handoff v6) — rank + locate + why, in the ENGINE
// so the CLI, pyo3/MCP, and future TUI/3d-viewer all share one implementation.
// ============================================================================

/// One node in a blast-radius answer: identity + kind + why-it's-here (`reason`)
/// + PPR `score` + `file`:`line` + `live`. Serialized straight to pyo3/CLI.
#[derive(serde::Serialize)]
pub struct BlastAnswer {
    pub id: u64,
    pub qname: String,
    pub name: String,
    pub kind: &'static str,
    /// Edge category that first put this node in scope ("why it's in the radius").
    pub reason: &'static str,
    pub depth: usize,
    pub score: f64,
    /// Reachable from an entrypoint (route/handler/main/test/component) — `false`
    /// = likely dead. Best-effort; annotated, not filtered, unless `live_only`.
    pub live: bool,
    pub file: Option<String>,
    pub line: Option<i64>,
}

/// Is this node an entrypoint — an externally-triggered root from which live
/// code is reachable? Routes, gRPC/WS/event handlers, CLI commands, framework
/// components, and `main`/`test*` functions.
fn is_entrypoint(kind: Option<repo_graph_core::NodeKindId>, name: &str) -> bool {
    match kind {
        Some(k)
            if k == node_kind::ROUTE
                || k == node_kind::GRPC_SERVICE
                || k == node_kind::WS_HANDLER
                || k == node_kind::EVENT_HANDLER
                || k == node_kind::CLI_COMMAND
                || k == node_kind::COMPONENT =>
        {
            true
        }
        Some(k) if k == node_kind::FUNCTION || k == node_kind::METHOD => {
            name == "main" || name.starts_with("test") || name.starts_with("Test")
        }
        _ => false,
    }
}

/// The entrypoint-reachable ("live") node set: every entrypoint plus everything
/// forward-reachable from one along semantic carry edges. A node absent from
/// this set is likely dead code. Conservative (generous entrypoint set) to avoid
/// false-dead flags — the failure mode the handoff warns about.
pub fn entrypoint_reachable(merged: &MergedGraph) -> std::collections::HashSet<NodeId> {
    use std::collections::{HashSet, VecDeque};
    let carry: HashSet<repo_graph_core::EdgeCategoryId> =
        repo_graph_graph::blast_carry_edges().into_iter().collect();
    let edges: Vec<&Edge> = merged.all_edges().collect();

    let mut live: HashSet<NodeId> = HashSet::new();
    let mut queue: VecDeque<NodeId> = VecDeque::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            let name = g.nav.name_by_id.get(&n.id).map(String::as_str).unwrap_or("");
            if is_entrypoint(kind, name) && live.insert(n.id) {
                queue.push_back(n.id);
            }
        }
    }
    while let Some(node) = queue.pop_front() {
        for e in &edges {
            if e.from == node && carry.contains(&e.category) && live.insert(e.to) {
                queue.push_back(e.to);
            }
        }
    }
    live
}

/// `blast_radius`, resolved from a qname/name and fully located — the P3 answer
/// that `find`→`impact`→`activate`→`read×N` collapses to. `direction` ∈
/// {`forward`, `backward`, `both`}. Err on an unknown qname or bad direction.
pub fn blast_radius_by_qname(
    merged: &MergedGraph,
    qname: &str,
    direction: &str,
    max_depth: usize,
    top_k: Option<usize>,
    live_only: bool,
) -> Result<Vec<BlastAnswer>, String> {
    let seed = merged
        .node_id_by_qname(qname)
        .or_else(|| merged.resolve_name(qname))
        .ok_or_else(|| format!("no node with qname/name `{qname}`"))?;
    let reach = match direction {
        "forward" => Reach::Forward,
        "backward" => Reach::Backward,
        "both" => Reach::Both,
        o => return Err(format!("direction must be forward|backward|both, got `{o}`")),
    };
    let live = entrypoint_reachable(merged);
    let hits = merged.blast_radius(seed, reach, max_depth, None);
    let mut out: Vec<BlastAnswer> = hits
        .iter()
        .filter(|h| !live_only || live.contains(&h.id))
        .map(|h| {
            let (name, qname, kind, file, line) = locate_node(merged, h.id);
            BlastAnswer {
                id: h.id.0,
                qname,
                name,
                kind,
                reason: edge_category::name(h.reason),
                depth: h.depth,
                score: h.score,
                live: live.contains(&h.id),
                file,
                line,
            }
        })
        .collect();
    if let Some(k) = top_k {
        out.truncate(k);
    }
    Ok(out)
}

/// One hop in a cross-stack trace: a typed edge from one entity to the next,
/// with the `mechanism` (edge category) and whether it crossed a service
/// boundary. The destination is located.
#[derive(serde::Serialize)]
pub struct TraceHop {
    pub depth: usize,
    /// Edge category name — the mechanism (`CALLS`, `HTTP_CALLS`, `QUEUE_FLOWS`…).
    pub mechanism: &'static str,
    /// True when `from` and `to` live in different repos/services.
    pub cross_service: bool,
    pub from_qname: String,
    pub to_qname: String,
    pub to_kind: &'static str,
    pub to_file: Option<String>,
    pub to_line: Option<i64>,
}

/// **cross_stack_trace** (P3): follow a feature forward across service
/// boundaries and return the ORDERED path — each hop labeled with its mechanism
/// (http/queue/grpc/call/…) and whether it crossed a service boundary — in one
/// call. Where `blast_radius` returns a ranked *set*, this returns the *sequence*
/// of typed edges, so an agent sees how a request flows end to end.
pub fn cross_stack_trace(
    merged: &MergedGraph,
    feature: &str,
    max_depth: usize,
) -> Result<Vec<TraceHop>, String> {
    use std::collections::{HashMap, HashSet, VecDeque};
    let seed = merged
        .node_id_by_qname(feature)
        .or_else(|| merged.resolve_name(feature))
        .ok_or_else(|| format!("no node with qname/name `{feature}`"))?;

    let mut repo_of: HashMap<NodeId, u64> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            repo_of.insert(n.id, g.repo.0);
        }
    }
    let carry: HashSet<repo_graph_core::EdgeCategoryId> =
        repo_graph_graph::blast_carry_edges().into_iter().collect();
    let edges: Vec<&Edge> = merged.all_edges().collect();

    let mut hops = Vec::new();
    let mut visited: HashSet<NodeId> = HashSet::from([seed]);
    let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(seed, 0)]);
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        for e in &edges {
            if e.from != node || !carry.contains(&e.category) {
                continue;
            }
            if visited.insert(e.to) {
                let (_, from_qname, _, _, _) = locate_node(merged, e.from);
                let (_, to_qname, to_kind, to_file, to_line) = locate_node(merged, e.to);
                let cross_service = repo_of.get(&e.from) != repo_of.get(&e.to);
                hops.push(TraceHop {
                    depth: depth + 1,
                    mechanism: edge_category::name(e.category),
                    cross_service,
                    from_qname,
                    to_qname,
                    to_kind,
                    to_file,
                    to_line,
                });
                queue.push_back((e.to, depth + 1));
            }
        }
    }
    Ok(hops)
}

/// One located node in a `resolve` answer: identity + kind + PPR relevance +
/// `file`:`line`. (No `reason`/`depth` — `resolve` locates seeds, it doesn't walk.)
#[derive(serde::Serialize)]
pub struct LocatedNode {
    pub id: u64,
    pub qname: String,
    pub name: String,
    pub kind: &'static str,
    pub score: f64,
    pub file: Option<String>,
    pub line: Option<i64>,
}

/// **resolve** (P3): a failure/change signal (stacktrace, diff, test id, or
/// `auto`) → the ranked, LOCATED nodes it points at, in one call. Resolution
/// order is preserved (stacktrace frames stay in order); `score` is PPR
/// relevance seeded by the whole resolved set, so shared-context nodes rank up.
/// `kind` ∈ {`stacktrace`, `test`, `diff`, `auto`}.
pub fn resolve_signal_located(
    merged: &MergedGraph,
    text: &str,
    kind: &str,
    top_k: Option<usize>,
) -> Vec<LocatedNode> {
    let seeds = merged.resolve_signal(text, kind);
    if seeds.is_empty() {
        return Vec::new();
    }
    let mut config = repo_graph_graph::code_activation_defaults();
    config.direction = repo_graph_activation::Direction::Undirected;
    config.top_k = usize::MAX;
    let scores: HashMap<NodeId, f64> =
        merged.activate(&seeds, &config).scores.into_iter().collect();
    let mut out: Vec<LocatedNode> = seeds
        .iter()
        .map(|id| {
            let (name, qname, kind, file, line) = locate_node(merged, *id);
            LocatedNode {
                id: id.0,
                qname,
                name,
                kind,
                score: scores.get(id).copied().unwrap_or(0.0),
                file,
                line,
            }
        })
        .collect();
    if let Some(k) = top_k {
        out.truncate(k);
    }
    out
}

/// **governing_docs** (P3 payoff, tier-4): the doc sections that DOCUMENTS a
/// symbol — "what are the rules for X?" — located, in one call. Direct
/// `doc --DOCUMENTS--> symbol` predecessors (the conservative, precise linker
/// signal). Reuses `LocatedNode` (score = 0; docs aren't PPR-ranked here).
pub fn governing_docs(merged: &MergedGraph, qname: &str) -> Result<Vec<LocatedNode>, String> {
    let target = merged
        .node_id_by_qname(qname)
        .or_else(|| merged.resolve_name(qname))
        .ok_or_else(|| format!("no node with qname/name `{qname}`"))?;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for e in merged.all_edges() {
        if e.to == target
            && e.category == edge_category::DOCUMENTS
            && seen.insert(e.from)
        {
            let (name, qn, kind, file, line) = locate_node(merged, e.from);
            out.push(LocatedNode {
                id: e.from.0,
                qname: qn,
                name,
                kind,
                score: 0.0,
                file,
                line,
            });
        }
    }
    Ok(out)
}

/// `(name, qname, kind_name, file, line)` for a node across the merged graphs.
/// `file`/`line` come from the POSITION cell; `None` for synthetic nodes
/// (ENDPOINT/DOC_SPACE) that carry no span. Shared "locate" for the primitives.
pub fn locate_node(
    merged: &MergedGraph,
    id: NodeId,
) -> (String, String, &'static str, Option<String>, Option<i64>) {
    for g in &merged.graphs {
        if !g.nav.qname_by_id.contains_key(&id) {
            continue;
        }
        let name = g.nav.name_by_id.get(&id).cloned().unwrap_or_default();
        let qname = g.nav.qname_by_id.get(&id).cloned().unwrap_or_default();
        let kind = g
            .nav
            .kind_by_id
            .get(&id)
            .map(|k| node_kind::name(*k))
            .unwrap_or("UNKNOWN");
        let (mut file, mut line) = (None, None);
        if let Some(n) = g.nodes.iter().find(|n| n.id == id) {
            for c in &n.cells {
                if c.kind != repo_graph_code_domain::cell_type::POSITION {
                    continue;
                }
                if let CellPayload::Json(s) | CellPayload::Text(s) = &c.payload {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                        file = v.get("file").and_then(|f| f.as_str()).map(String::from);
                        line = v.get("start_line").and_then(serde_json::Value::as_i64);
                    }
                }
            }
        }
        return (name, qname, kind, file, line);
    }
    (String::new(), format!("(unknown:{})", id.0), "UNKNOWN", None, None)
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

/// Resolve a TS/JS relative import specifier to the in-repo module qname it
/// targets — the inverse of `path_to_qname` (drop extension, `/`→`::`). Bare or
/// scoped specifiers (`@angular/core`, `lodash`) are external → None (no edge).
/// This is the resolver the engine previously stubbed with `|_,_| None`, which
/// is why NO TS/JS/Angular/React/Vue import ever became a category-3 IMPORTS
/// edge — imports lived only as `Symbol.imports` cells (handoff Pattern B).
fn resolve_ts_source(from_module: &str, specifier: &str) -> Option<String> {
    let spec = specifier.trim().trim_matches(|c| c == '"' || c == '\'');
    if !spec.starts_with('.') {
        return None; // external package — no intra-repo edge
    }
    // Directory of the importing module = its qname minus the final (file) segment.
    let mut segs: Vec<String> = from_module.split("::").map(String::from).collect();
    segs.pop();
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            p => {
                let p = p
                    .strip_suffix(".ts")
                    .or_else(|| p.strip_suffix(".tsx"))
                    .or_else(|| p.strip_suffix(".js"))
                    .or_else(|| p.strip_suffix(".jsx"))
                    .unwrap_or(p);
                segs.push(p.to_string());
            }
        }
    }
    if segs.is_empty() {
        return None;
    }
    Some(segs.join("::"))
}

/// Relative-import resolver for the non-TS `_`-arm languages (dart / c_cpp /
/// solidity). Handles dotted specifiers (`./x`, `../a/b`) AND bare filenames
/// that carry a source extension (`import 'models.dart'`, `#include
/// "mathutil.h"`) — both resolve against the importing file's directory to the
/// `path_to_qname` form. A bare specifier with no source extension (a package /
/// system import like `package:collection`, `import Foundation`, `<stdio.h>`)
/// is external → None. Superset of `resolve_ts_source`; kept separate so the
/// verified TS-family path is untouched.
fn resolve_relative_source(from_module: &str, specifier: &str) -> Option<String> {
    const SRC_EXT: &[&str] = &[
        ".dart", ".h", ".hpp", ".hh", ".hxx", ".sol", ".swift", ".ts", ".tsx", ".js", ".jsx",
    ];
    let spec = specifier
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>');
    let has_src_ext = SRC_EXT.iter().any(|e| spec.ends_with(e));
    if !spec.starts_with('.') && !has_src_ext {
        return None; // external package / system header
    }
    let mut segs: Vec<String> = from_module.split("::").map(String::from).collect();
    segs.pop(); // directory of the importing file
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            p => {
                let stem = SRC_EXT
                    .iter()
                    .find_map(|e| p.strip_suffix(e))
                    .unwrap_or(p);
                segs.push(stem.to_string());
            }
        }
    }
    if segs.is_empty() {
        return None;
    }
    Some(segs.join("::"))
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

    fn unique_tmp(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("glia_wpd_{}_{}_{}", std::process::id(), tag, n))
    }

    /// Sorted (node-ids, edges) — a deterministic fingerprint of a graph's
    /// content, independent of build order.
    fn fingerprint(g: &MergedGraph) -> (Vec<u64>, Vec<(u64, u64, u32)>) {
        let mut nodes: Vec<u64> =
            g.graphs.iter().flat_map(|r| r.nodes.iter().map(|n| n.id.0)).collect();
        nodes.sort_unstable();
        let mut edges: Vec<(u64, u64, u32)> =
            g.all_edges().map(|e| (e.from.0, e.to.0, e.category.0)).collect();
        edges.sort_unstable();
        (nodes, edges)
    }

    #[test]
    fn incremental_reuses_unchanged_and_matches_clean_build() {
        let dir = unique_tmp("incr");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.py");
        std::fs::write(&a, "def foo():\n    return 1\n").unwrap();
        std::fs::write(dir.join("b.py"), "def bar():\n    return 2\n").unwrap();
        let repo = dir.to_str().unwrap();

        // Cold cache: both files parsed; graph identical to a clean build.
        let clean = generate_one(repo).unwrap();
        let mut cache = ParseCache::new();
        let cold = generate_one_with_cache(repo, &mut cache).unwrap();
        assert_eq!(cache.stats.reparsed, 2);
        assert_eq!(cache.stats.reused, 0);
        assert_eq!(fingerprint(&clean.merged), fingerprint(&cold.merged));

        // Edit one file → only that file reparses, the other is reused…
        std::fs::write(&a, "def foo():\n    return 1 + 1\n").unwrap();
        let warm = generate_one_with_cache(repo, &mut cache).unwrap();
        assert_eq!(cache.stats.reparsed, 1, "only the edited file reparsed");
        assert_eq!(cache.stats.reused, 1, "the unchanged file reused");

        // …and the incremental result equals a fresh clean build as a SET
        // (sorted fingerprint — fast unit check). The real WP-D acceptance
        // gate is tests/byte_identical.rs, which compares the actual bytes
        // the store writes and fails on any ordering nondeterminism.
        let clean2 = generate_one(repo).unwrap();
        assert_eq!(fingerprint(&warm.merged), fingerprint(&clean2.merged));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn incremental_evicts_deleted_files() {
        let dir = unique_tmp("evict");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def foo():\n    return 1\n").unwrap();
        let b = dir.join("b.py");
        std::fs::write(&b, "def bar():\n    return 2\n").unwrap();
        let repo = dir.to_str().unwrap();

        let mut cache = ParseCache::new();
        generate_one_with_cache(repo, &mut cache).unwrap();
        assert_eq!(cache.len(), 2);

        std::fs::remove_file(&b).unwrap();
        generate_one_with_cache(repo, &mut cache).unwrap();
        assert_eq!(cache.stats.evicted, 1, "deleted file evicted from cache");
        assert_eq!(cache.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cache_discarded_when_build_context_changes() {
        let dir = unique_tmp("ctx");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def foo():\n    return 1\n").unwrap();
        let repo = dir.to_str().unwrap();

        // Same dir, different path spelling → different RepoId baked into
        // cached NodeIds → every entry must be discarded, not reused.
        let mut cache = ParseCache::new();
        generate_one_with_cache(repo, &mut cache).unwrap();
        assert_eq!(cache.stats.reparsed, 1);
        let alt = format!("{repo}/.");
        generate_one_with_cache(&alt, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 0, "path-spelling change must not reuse");
        assert_eq!(cache.stats.reparsed, 1);

        // Same spelling again → reuse works.
        generate_one_with_cache(&alt, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 1);

        // go.mod module change → .go parses are context-dependent → discard.
        std::fs::write(dir.join("m.go"), "package m\nfunc F() {}\n").unwrap();
        std::fs::write(dir.join("go.mod"), "module example.com/one\n").unwrap();
        generate_one_with_cache(&alt, &mut cache).unwrap();
        std::fs::write(dir.join("go.mod"), "module example.com/two\n").unwrap();
        generate_one_with_cache(&alt, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 0, "go.mod module change must not reuse");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_cache_disk_roundtrip() {
        let dir = unique_tmp("disk");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "def foo():\n    return 1\n").unwrap();
        let repo = dir.to_str().unwrap();

        // Missing cache loads empty.
        assert!(ParseCache::load(repo).is_empty());
        // Incremental build persists it; a fresh load sees the entry.
        generate_one_incremental(repo).unwrap();
        assert_eq!(ParseCache::load(repo).len(), 1);

        // Explicit purge (the --no-incremental escape hatch) removes the
        // sidecar; the next load starts cold. Purging a missing file is Ok.
        ParseCache::purge(repo).unwrap();
        assert!(ParseCache::load(repo).is_empty());
        ParseCache::purge(repo).unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn backtick_identifiers_extract_inline_code(){
        let md = "Use `parse_config` and `WidgetFactory.build()`.\n\
                  Run `npm install` (ignored). `x` too short.\n\
                  ```\nfenced `not_this`\n```";
        let ids = backtick_identifiers(md);
        assert!(ids.contains(&"parse_config".to_string()));
        // method span reduces to the trailing identifier.
        assert!(ids.contains(&"build".to_string()));
        // "npm install" has a space → not an identifier; "x" too short.
        assert!(!ids.iter().any(|s| s.contains(' ')));
        assert!(!ids.contains(&"x".to_string()));
    }

    #[test]
    fn identifier_from_span_normalises() {
        assert_eq!(identifier_from_span("parse_config()"), Some("parse_config".into()));
        assert_eq!(identifier_from_span("mod::Thing"), Some("Thing".into()));
        assert_eq!(identifier_from_span("a.b.method"), Some("method".into()));
        assert_eq!(identifier_from_span("--flag"), None);
        assert_eq!(identifier_from_span("ab"), None); // too short
    }

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
