//! Build orchestration: the `generate_*` entry points, per-repo graph
//! assembly (per-language `build_*` dispatch), and cross-graph resolver
//! registration. The per-file routing that feeds it lives in [`crate::route`].

use std::path::{Path, PathBuf};

use repo_graph_code_domain::FileParse;
use repo_graph_core::RepoId;
use repo_graph_graph::{
    CliInvocationResolver, ConfigResolver, CronResolver, DbResolver, EventBusResolver,
    GraphQLStackResolver, GrpcStackResolver, HttpStackResolver, IacResolver, MergedGraph,
    PackageResolver, QueueStackResolver, SharedSchemaResolver, WebSocketStackResolver,
};

use crate::cache::ParseCache;
use crate::docs::{DocSource, FileDocSource, SnapshotDocSource, build_docs_graph};
use crate::passes::post_passes;
use crate::route::parse_repo_files;
use crate::walk::{build_region_graph, walk_source_files};

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
    cache: Option<&mut ParseCache>,
) -> (Vec<repo_graph_graph::RepoGraph>, Vec<String>) {
    // Suppress the default panic-print-to-stderr while we run per-file parsers
    // — we catch panics below and report them as parse_errors. The default
    // hook would otherwise spam stderr (with a backtrace) for every bad file
    // even though we recover. Restored on scope exit via Drop guard so a
    // panic in non-loop code still gets the user-visible report.
    let _hook_guard = SuppressPanicHook::install();

    let (parses_by_lang, proto_parses, mut parse_errors) =
        parse_repo_files(files, repo, go_module_prefix, cache);

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
