//! Build orchestration: the `generate_*` entry points, per-repo graph
//! assembly (per-language `build_*` dispatch), and cross-graph resolver
//! registration. The per-file routing that feeds it lives in [`crate::route`].

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use repo_graph_code_domain::{
    FileParse, GRAPH_TYPE, attach_imports_cell, cell_type, di_stats, edge_category, node_kind,
};
use repo_graph_code_extractors::anchor;
use repo_graph_code_extractors::constants::ConstTable;
use repo_graph_code_extractors::grpc::{self, ProtoServiceRef};
use repo_graph_core::{NodeId, RepoId};
use repo_graph_graph::{
    CliInvocationResolver, ConfigResolver, CronResolver, DbResolver, EventBusResolver,
    GraphQLStackResolver, GrpcStackResolver, HttpStackResolver, IacResolver, MergedGraph,
    PackageResolver, QueueStackResolver, SharedSchemaResolver, WebSocketStackResolver,
};

use crate::cache::ParseCache;
use crate::docs::{DocSource, FileDocSource, SnapshotDocSource, build_docs_graph};
use crate::endpoint_fold;
use crate::extract::{detect_language, merge_nav, path_to_qname};
use crate::passes::post_passes;
use crate::route::parse_repo_files;
use crate::walk::{WalkResult, build_project_graph, build_region_graph, walk_source_files};

pub struct GenerateResult {
    pub merged: MergedGraph,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub parse_errors: Vec<String>,
    /// `RepoId.0` → human repo label (A9.2). `RepoId::from_canonical` xxhashes
    /// the path away, so this is the ONLY place the human name survives — it is
    /// captured here, where the path and the id still coexist, and deliberately
    /// NOT on `MergedGraph`, which would change the `.gmap` bytes. Present only
    /// on a freshly generated result; a `.gmap` load has none.
    pub repo_labels: std::collections::BTreeMap<u64, String>,
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
        eprintln!("[incremental] {repo_path}: warning: failed to save parse cache: {e}");
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
    let repo_labels = crate::arch::repo_label_map(&[(repo.0, repo_path.to_string())]);
    // Project roots (A8.4) become PROJECT nodes below (A8.5); per-root go.mod
    // prefixes are A8.7.
    let (files, regions, md, roots) = walk_source_files(&root);
    let go_prefix = read_go_module_prefix(&root);
    // Cached parses are only valid under the exact repo identity + go.mod
    // module they were built with — neither is visible to per-file hashes.
    if let Some(c) = cache.as_deref_mut() {
        c.validate_context(&canonical, &go_prefix);
    }
    let mut rpc = RpcContext::default();
    rpc.add_files(&files);
    let (mut graphs, mut parse_errors) =
        build_graphs_for_repo(&files, repo, &go_prefix, cache, repo_path, &rpc);
    // Slot order is regions, then projects, then docs. It fixes the shard index,
    // so generate_many_inner must use the same order.
    if !regions.is_empty() {
        graphs.push(build_region_graph(&regions, repo));
    }
    if !roots.is_empty() {
        graphs.push(build_project_graph(&roots, repo));
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
        repo_labels,
    })
}

/// Generate a `MergedGraph` from N repo paths. Each path becomes its own
/// RepoId so cross-graph resolvers fire across boundaries (the canonical
/// substrate-eval entry). Always a cold build that writes nothing into the
/// repos: `bench/substrate-gap` grades every multi-dir fixture through here.
pub fn generate_many(repo_paths: &[String]) -> Result<GenerateResult, String> {
    generate_many_inner(repo_paths, false)
}

/// Disk-backed incremental multi-repo build: each path gets its OWN
/// `<repo>/.ai/repo-graph/parse_cache.bin`, loaded before and saved after that
/// repo's parse (audit 2026-06-10 #14). Byte-identical to [`generate_many`].
/// Opt-in, never the default: the substrate-gap eval grades through
/// `generate_many` and must stay hermetic. Cache save failures are logged, not
/// fatal. Backs pyo3 `generate_many(incremental=True)` and
/// `glia merge --incremental`.
pub fn generate_many_incremental(repo_paths: &[String]) -> Result<GenerateResult, String> {
    generate_many_inner(repo_paths, true)
}

fn generate_many_inner(repo_paths: &[String], incremental: bool) -> Result<GenerateResult, String> {
    let mut all_graphs = Vec::new();
    let mut all_errors = Vec::new();
    let mut label_inputs: Vec<(u64, String)> = Vec::new();

    // Phase 1 — walk every repo before building any (A5.2), so the proto
    // service set is the UNION across the build: in a client/server split the
    // client repo ships no `.proto` of its own. The cost is holding every
    // repo's sources at once, which the 2-5 repo `--with` merges absorb.
    // A missing path keeps its slot so errors stay in argument order.
    let mut rpc = RpcContext::default();
    let mut walked: Vec<Result<(&String, PathBuf, WalkResult), String>> =
        Vec::with_capacity(repo_paths.len());
    for path in repo_paths {
        let root = PathBuf::from(path);
        if !root.is_dir() {
            walked.push(Err(format!("not a directory: {path}")));
            continue;
        }
        let walk = walk_source_files(&root);
        rpc.add_files(&walk.0);
        walked.push(Ok((path, root, walk)));
    }

    // Phase 2 — build each repo against the union.
    for entry in walked {
        let (path, root, (files, regions, md, roots)) = match entry {
            Ok(w) => w,
            Err(e) => {
                all_errors.push(e);
                continue;
            }
        };
        // One string feeds both the RepoId and the cache's context check: every
        // cached FileParse has this RepoId baked into its NodeIds, so a sidecar
        // written under another spelling of the path must be discarded (#2).
        let canonical = format!("file://{path}");
        let repo = RepoId::from_canonical(&canonical);
        label_inputs.push((repo.0, path.clone()));
        let go_prefix = read_go_module_prefix(&root);
        let mut cache = incremental.then(|| ParseCache::load(path));
        if let Some(c) = cache.as_mut() {
            c.validate_context(&canonical, &go_prefix);
        }
        let (graphs, parse_errors) =
            build_graphs_for_repo(&files, repo, &go_prefix, cache.as_mut(), path, &rpc);
        if let Some(c) = cache.as_ref()
            && let Err(e) = c.save(path)
        {
            eprintln!("[incremental] {path}: warning: failed to save parse cache: {e}");
        }
        all_graphs.extend(graphs);
        // Same slot order as generate_one_inner: regions, projects, docs.
        if !regions.is_empty() {
            all_graphs.push(build_region_graph(&regions, repo));
        }
        if !roots.is_empty() {
            all_graphs.push(build_project_graph(&roots, repo));
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
        repo_labels: crate::arch::repo_label_map(&label_inputs),
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

/// Every gRPC service a `.proto` declares anywhere in this build (A5.2). The
/// client-needle pass keys on it, so a client repo with no `.proto` of its own
/// still recognises stubs for the server repo's services.
#[derive(Default)]
struct RpcContext {
    /// Sorted and deduplicated, so needle order (and therefore GRPC_CLIENT
    /// emission order) does not depend on walk or repo order.
    services: Vec<ProtoServiceRef>,
}

impl RpcContext {
    /// Fold in every service the `.proto` files among `files` declare.
    fn add_files(&mut self, files: &[(String, String)]) {
        for (path, source) in files {
            if detect_language(path) == Some("proto") {
                self.services.extend(grpc::proto_service_refs(source));
            }
        }
        self.services.sort_unstable();
        self.services.dedup();
    }
}

/// The data-driven gRPC client pass (A5.2). It runs here, on the router's
/// output, not inside the per-file cross-cutting extractors: its input (the
/// build's proto service set) is not a function of the file's own content, so a
/// cached `FileParse` (WP-D) would otherwise replay clients minted against a
/// stale service set. Running after the cache keeps incremental == clean.
///
/// Every code parser emits the file's MODULE node first, and that is how a
/// parse is paired back to its source. Returns the GRPC_CLIENT nodes it added.
///
/// A5.8: the added clients are anchored here too (POSITION + the owning
/// method's USES edge), because the per-file anchor pass in
/// `apply_cross_cutting_extractors` ran before they existed.
fn apply_rpc_client_needles(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    repo: RepoId,
    rpc: &RpcContext,
    parse_errors: &mut Vec<String>,
) -> usize {
    if rpc.services.is_empty() {
        return 0;
    }
    let mut added = 0;
    for (path, source) in files {
        if !grpc::file_has_grpc_context(source) {
            continue;
        }
        let Some(lang) = detect_language(path) else { continue };
        if lang == "proto" {
            continue;
        }
        let Some(parses) = parses_by_lang.get_mut(lang) else { continue };
        let module_id =
            NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, &path_to_qname(path));
        // No match = the file failed to parse; there is no module to hang a client on.
        let Some(fp) = parses
            .iter_mut()
            .find(|fp| fp.nodes.first().is_some_and(|n| n.id == module_id))
        else {
            continue;
        };
        let out = match catch_unwind(AssertUnwindSafe(|| {
            grpc::extract_known_grpc_client_nodes(source, module_id, repo, &rpc.services)
        })) {
            Ok(out) => out,
            Err(_) => {
                parse_errors.push(format!("{path}: PANIC (grpc client needles)"));
                continue;
            }
        };
        if out.nodes.is_empty() {
            continue;
        }
        let grpc::GrpcNodes {
            nodes,
            nav,
            mut anchors,
        } = out;
        // Anchor first, then add the IMPORTS cell, so a data-driven client's
        // cells come in the same order as a fallback client's (whose POSITION
        // lands in the extractor pass, before the router's IMPORTS cell).
        let first_new = fp.nodes.len();
        fp.nodes.extend(nodes);
        merge_nav(&mut fp.nav, nav);
        anchor::attach(fp, path, module_id, &mut anchors);
        // The same G15 IMPORTS cell the router gave every other node in the file.
        let mut extra = FileParse {
            nodes: fp.nodes.split_off(first_new),
            imports: fp.imports.clone(),
            ..Default::default()
        };
        attach_imports_cell(&mut extra, lang);
        added += extra.nodes.len();
        fp.nodes.extend(extra.nodes);
    }
    added
}

/// The repo-scope name -> literal table (A11.1), built from every walked file
/// with a source language. It sits beside `read_go_module_prefix` as a
/// cross-file fact the engine gathers, but unlike the go.mod prefix it is NOT
/// handed to the per-file extractors: their output is cached by the file's own
/// content hash, and a table lookup depends on other files. A consumer runs
/// after the cache, as `apply_rpc_client_needles` does, so incremental == clean.
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
/// service set (A5.2).
fn build_graphs_for_repo(
    files: &[(String, String)],
    repo: RepoId,
    go_module_prefix: &str,
    cache: Option<&mut ParseCache>,
    repo_label: &str,
    rpc: &RpcContext,
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
    // and sit beside `apply_rpc_client_needles` below.
    let const_table = build_const_table(files, &mut parse_errors);
    if !const_table.is_empty() {
        eprintln!(
            "[const] repo table: {} bindings from {} files ({} conflicts) repo={repo_label}",
            const_table.len(),
            const_table.files(),
            const_table.conflicts()
        );
    }
    // A11.2: re-key client ENDPOINTs whose base the table resolves and record
    // their authority. Post-cache, so cached parses are folded too and the
    // cache keeps the pre-fold parse.
    endpoint_fold::fold_repo(parses_by_lang.values_mut().flatten(), &const_table, repo)
        .report(repo_label);

    let rpc_added =
        apply_rpc_client_needles(&mut parses_by_lang, files, repo, rpc, &mut parse_errors);
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
            "[grpc-client] {grpc_clients} stubs from {} known services (proto-needle +{rpc_added}) repo={repo_label}",
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
    // A7.0 `[di]` marker input: INJECTS refs per language, counted off the
    // parses themselves so cache-served files count too. The TS-family tags
    // report as `typescript`, their matrix row.
    let di_refs: Vec<(&str, usize)> = parses_by_lang
        .iter()
        .map(|(lang, parses)| {
            let row = if TS_FAMILY.contains(lang) {
                "typescript"
            } else {
                *lang
            };
            let n = parses
                .iter()
                .flat_map(|fp| &fp.refs)
                .filter(|r| r.category == edge_category::INJECTS)
                .count();
            (row, n)
        })
        .collect();
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

#[cfg(test)]
mod cache_tests {
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
}

#[cfg(test)]
mod rpc_needle_tests {
    use super::*;
    use repo_graph_code_domain::{cell_type, edge_category};
    use repo_graph_core::{Cell, EdgeCategoryId as CategoryId};

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// `(qname, node id)` of every GRPC_CLIENT in `repo`, sorted by qname.
    fn clients(m: &MergedGraph, repo: RepoId) -> Vec<(String, NodeId)> {
        let mut out: Vec<(String, NodeId)> = m
            .graphs
            .iter()
            .filter(|g| g.repo == repo)
            .flat_map(|g| {
                g.nodes
                    .iter()
                    .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::GRPC_CLIENT))
                    .map(move |n| (g.nav.qname_by_id[&n.id].clone(), n.id))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn node_id_by_qname(m: &MergedGraph, qname: &str) -> NodeId {
        m.graphs
            .iter()
            .find_map(|g| {
                g.nav
                    .qname_by_id
                    .iter()
                    .find_map(|(id, q)| (q == qname).then_some(*id))
            })
            .unwrap_or_else(|| panic!("no node {qname}"))
    }

    fn cells_of(m: &MergedGraph, id: NodeId) -> Vec<Cell> {
        m.graphs
            .iter()
            .flat_map(|g| g.nodes.iter())
            .find(|n| n.id == id)
            .map(|n| n.cells.clone())
            .unwrap_or_default()
    }

    fn incoming(m: &MergedGraph, id: NodeId) -> Vec<CategoryId> {
        let mut cats: Vec<CategoryId> =
            m.all_edges().filter(|e| e.to == id).map(|e| e.category).collect();
        cats.sort_by_key(|c| c.0);
        cats
    }

    fn has_edge(m: &MergedGraph, from: NodeId, to: NodeId, cat: CategoryId) -> bool {
        m.all_edges()
            .any(|e| e.from == from && e.to == to && e.category == cat)
    }

    const PROTO: &str = "syntax = \"proto3\";\npackage shop;\noption go_package = \"example.com/shop/pb\";\n\nservice Greeter {\n  rpc SayHello (HelloRequest) returns (HelloReply);\n}\n\nservice OrderService {\n  rpc Place (PlaceRequest) returns (PlaceReply);\n}\n";

    const GO_CLIENT: &str = "package main\n\nimport (\n\t\"google.golang.org/grpc\"\n\tpb \"example.com/shop/pb\"\n)\n\nfunc main() {\n\tconn, _ := grpc.Dial(\"server:50051\")\n\tgreeter := pb.NewGreeterClient(conn)\n\torders := pb.NewOrderServiceClient(conn)\n\t_, _ = greeter, orders\n}\n";

    #[test]
    fn generate_many_mints_clients_from_the_union_of_proto_services() {
        let tmp = tempfile::tempdir().unwrap();
        let server = tmp.path().join("server");
        let client = tmp.path().join("client");
        write(&server, "api.proto", PROTO);
        write(&client, "main.go", GO_CLIENT);
        let (server_s, client_s) = (
            server.to_str().unwrap().to_string(),
            client.to_str().unwrap().to_string(),
        );
        let client_repo = RepoId::from_canonical(&format!("file://{client_s}"));

        // Alone, the client repo knows no proto: only the suffix fallback fires.
        let alone = generate_one(&client_s).unwrap();
        let names: Vec<String> = clients(&alone.merged, client_repo)
            .into_iter()
            .map(|(q, _)| q)
            .collect();
        assert_eq!(names, vec!["grpc_client:OrderService".to_string()]);

        // Merged, the server's .proto names `Greeter` for the client repo too.
        let merged = generate_many(&[server_s.clone(), client_s.clone()]).unwrap().merged;
        let found = clients(&merged, client_repo);
        let names: Vec<&str> = found.iter().map(|(q, _)| q.as_str()).collect();
        assert_eq!(names, vec!["grpc_client:Greeter", "grpc_client:OrderService"]);
        let (greeter, orders) = (found[0].1, found[1].1);
        assert!(has_edge(
            &merged,
            greeter,
            node_id_by_qname(&merged, "grpc:shop.Greeter"),
            edge_category::GRPC_CALLS
        ));
        assert!(has_edge(
            &merged,
            orders,
            node_id_by_qname(&merged, "grpc:shop.OrderService"),
            edge_category::GRPC_CALLS
        ));

        // A data-driven client is shaped exactly like a fallback one: same
        // cells in the same order (the file's IMPORTS), same incoming
        // structural edges. POSITION is the one per-client cell (A5.8): each
        // stub is located at its own construction line.
        let greeter_cells = cells_of(&merged, greeter);
        let orders_cells = cells_of(&merged, orders);
        assert!(greeter_cells.iter().any(|c| c.kind == cell_type::IMPORTS));
        let kinds = |cells: &[Cell]| cells.iter().map(|c| c.kind).collect::<Vec<_>>();
        assert_eq!(kinds(&greeter_cells), kinds(&orders_cells));
        let without_position = |cells: &[Cell]| {
            cells
                .iter()
                .filter(|c| c.kind != cell_type::POSITION)
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(without_position(&greeter_cells), without_position(&orders_cells));
        let position = |cells: &[Cell]| {
            cells
                .iter()
                .find(|c| c.kind == cell_type::POSITION)
                .map(|c| c.payload.clone())
        };
        assert_eq!(
            position(&greeter_cells),
            Some(repo_graph_core::CellPayload::Json(
                r#"{"file":"main.go","start_line":9,"end_line":9}"#.to_string()
            ))
        );
        assert_eq!(
            position(&orders_cells),
            Some(repo_graph_core::CellPayload::Json(
                r#"{"file":"main.go","start_line":10,"end_line":10}"#.to_string()
            ))
        );
        let mut in_greeter = incoming(&merged, greeter);
        let mut in_orders = incoming(&merged, orders);
        in_greeter.retain(|c| *c != edge_category::GRPC_CALLS);
        in_orders.retain(|c| *c != edge_category::GRPC_CALLS);
        assert_eq!(in_greeter, in_orders);

        // Repo order does not change what the client repo gets.
        let reversed = generate_many(&[client_s, server_s]).unwrap().merged;
        let rev_names: Vec<String> = clients(&reversed, client_repo)
            .into_iter()
            .map(|(q, _)| q)
            .collect();
        assert_eq!(rev_names, names);
    }

    fn write_store(m: &MergedGraph, dir: &Path) -> Vec<(String, Vec<u8>)> {
        repo_graph_store::write_merged_sharded(m, dir).unwrap();
        let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| {
                (
                    e.file_name().to_string_lossy().to_string(),
                    std::fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        out.sort();
        out
    }

    const GREETER_PROTO: &str = "package hello;\nservice Greeter {\n  rpc Hi (A) returns (B);\n}\n";
    const FAREWELL_PROTO: &str = "package hello;\nservice Farewell {\n  rpc Bye (A) returns (B);\n}\n";

    #[test]
    fn rpc_needles_follow_the_proto_under_a_warm_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        write(
            &repo,
            "client.py",
            "import grpc\nimport hello_pb2_grpc\n\n\ndef call(channel):\n    return hello_pb2_grpc.GreeterStub(channel)\n",
        );
        write(&repo, "api.proto", GREETER_PROTO);
        let repo_s = repo.to_str().unwrap();
        let rid = RepoId::from_canonical(&format!("file://{repo_s}"));
        let names = |m: &MergedGraph| -> Vec<String> {
            clients(m, rid).into_iter().map(|(q, _)| q).collect()
        };

        let mut cache = ParseCache::new();
        let cold = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(names(&cold.merged), vec!["grpc_client:Greeter".to_string()]);

        // Only the .proto changes: client.py is replayed from the cache, and
        // must still lose the client its needle no longer names.
        write(&repo, "api.proto", FAREWELL_PROTO);
        let warm = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 1, "client.py must come from the cache");
        assert!(names(&warm.merged).is_empty(), "stale client replayed from cache");
        let clean = generate_one(repo_s).unwrap();
        assert_eq!(
            write_store(&warm.merged, &tmp.path().join("warm")),
            write_store(&clean.merged, &tmp.path().join("clean")),
            "incremental vs clean after a proto-only edit"
        );

        // And back: the cached parse picks the client up again.
        write(&repo, "api.proto", GREETER_PROTO);
        let warm2 = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 1);
        assert_eq!(names(&warm2.merged), vec!["grpc_client:Greeter".to_string()]);
        let clean2 = generate_one(repo_s).unwrap();
        assert_eq!(
            write_store(&warm2.merged, &tmp.path().join("warm2")),
            write_store(&clean2.merged, &tmp.path().join("clean2")),
            "incremental vs clean with a data-driven client present"
        );
    }
}
