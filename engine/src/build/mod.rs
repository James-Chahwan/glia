//! Build orchestration: the `generate_*` entry points and the per-repo loop
//! that feeds them. The per-file routing lives in [`crate::route`]; the rest
//! of a build is split by seam:
//!
//! - [`assemble`] — `build_graphs_for_repo`: parse, const table, post-cache
//!   grafts, per-language build, per-repo markers, under one panic-hook guard.
//! - [`grafts`] — the post-cache graft sequence (endpoint fold, HTTP owner
//!   segment, queue const fold, RPC needles and their markers, anchor census,
//!   IMPORTS-cell filter).
//! - [`rpc_needles`] — the build-wide proto service set and the gRPC
//!   client / server needle passes.
//! - [`lang_build`] — the deterministic per-language `build_*` dispatch.
//!
//! The build tail (cross-graph resolvers, post-passes, evidence fill, the
//! determinism sort) is the code domain's pass registry,
//! [`crate::profile::CODE_PASSES`], run by one
//! [`crate::profile::run_code_passes`] call (LD.13). After it, the external
//! node-cell stage ([`crate::external::apply_external_cells`], LF.1a) applies
//! each repo's `.glia` inputs, loaded once per repo right after its walk.

mod assemble;
mod grafts;
mod lang_build;
mod rpc_needles;

use std::path::{Path, PathBuf};

use repo_graph_code_domain::walk_gating::{RepoIdentity, repo_identity};
use repo_graph_core::RepoId;
use repo_graph_graph::MergedGraph;

use crate::cache::ParseCache;
use crate::docs::{DocSource, FileDocSource, SnapshotDocSource, build_docs_graph};
use crate::external::{RepoInputs, apply_external_cells, repo_inputs};
use crate::profile::run_code_passes;
use crate::walk::{WalkResult, build_project_graph, build_region_graph, walk_source_files};

use assemble::build_graphs_for_repo;
use rpc_needles::RpcContext;

/// One build's output. Outside this crate it comes from [`generate_one`] /
/// [`generate_many`] and their variants, never from a struct literal, so a new
/// field is not a break (LD.9, `engine/tests/api_stability.rs`):
///
/// ```compile_fail
/// let _ = repo_graph_engine::GenerateResult {
///     merged: Default::default(),
///     total_nodes: 0,
///     total_edges: 0,
///     parse_errors: vec![],
///     repo_labels: Default::default(),
///     repo_roots: Default::default(),
/// };
/// ```
#[non_exhaustive]
pub struct GenerateResult {
    pub merged: MergedGraph,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub parse_errors: Vec<String>,
    /// `RepoId.0` → human repo label (A9.2). The RepoId is an xxhash of the
    /// repo identity key (git remote / git dir / dir name, LB.1), so the human
    /// label — derived from the path the caller gave — cannot be recovered
    /// from the graph: it is captured where the path and the id still coexist,
    /// and deliberately NOT on `MergedGraph`, which would change the shard
    /// bytes. It is persisted in the layout's `manifest.json` instead (LC.7),
    /// so [`crate::persist::load_layout`] returns the same map a fresh build
    /// did; a layout written without metadata loads with none.
    pub repo_labels: std::collections::BTreeMap<u64, String>,
    /// `RepoId.0` → the repo root, as the caller gave it on a fresh build
    /// (LC.7). [`crate::persist::layout_meta`] records it relative to the
    /// layout dir; [`crate::persist::load_layout`] returns it resolved against
    /// the dir it loaded from, canonicalised when the path still exists.
    pub repo_roots: std::collections::BTreeMap<u64, String>,
}

/// Generate a `MergedGraph` from a single repo path. The repo gets one RepoId,
/// an xxhash of its path-independent identity key ([`repo_identity`]):
/// `git:<normalised origin url>[/<path within the checkout>]` for a git
/// checkout with a remote, `gitdir:<main checkout dir name>[/<rel>]` for one
/// without, `dir:<basename>` outside git. So every NodeId survives a re-spelled
/// path, a second clone, a linked worktree and a moved checkout. Cross-graph
/// resolvers run but only emit edges within this single repo (rare in practice).
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
/// `<repo>/.glia/graph/parse_cache.bin` (beside the layout), build, then
/// persist it. Cache save failures are logged, not fatal. Backs pyo3
/// `generate(incremental=True)` and `glia build`.
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
    let ident = repo_identity(&root);
    let repo = RepoId::from_canonical(&ident.key);
    repo_id_marker(&ident, repo_path);
    let repo_labels = crate::arch::repo_label_map(&[(repo.0, repo_path.to_string())]);
    let repo_roots = std::collections::BTreeMap::from([(repo.0, repo_path.to_string())]);
    // Project roots (A8.4) become PROJECT nodes below (A8.5); per-root go.mod
    // prefixes are A8.7.
    let (files, regions, md, roots) = walk_source_files(&root);
    // External inputs (LF.1a): `.glia/overlay.toml` loaded once, before any
    // graph is built.
    let inputs = [repo_inputs(repo, root.clone(), repo_path.to_string())];
    let go_prefix = read_go_module_prefix(&root);
    // Cached parses are only valid under the exact repo identity + go.mod
    // module they were built with — neither is visible to per-file hashes.
    if let Some(c) = cache.as_deref_mut() {
        c.validate_context(&ident.key, &go_prefix);
    }
    let mut rpc = RpcContext::default();
    rpc.add_files(&files);
    let (mut graphs, mut parse_errors) =
        build_graphs_for_repo(&files, repo, &go_prefix, cache, repo_path, &rpc, &roots);
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
    run_code_passes(&mut merged);
    apply_external_cells(&mut merged, &inputs);
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
        repo_roots,
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
/// `<repo>/.glia/graph/parse_cache.bin`, loaded before and saved after that
/// repo's parse (audit 2026-06-10 #14). Byte-identical to [`generate_many`].
/// Opt-in, never the default: the substrate-gap eval grades through
/// `generate_many` and must stay hermetic. Cache save failures are logged, not
/// fatal. Backs pyo3 `generate_many(incremental=True)` and
/// `glia merge --incremental`.
pub fn generate_many_incremental(repo_paths: &[String]) -> Result<GenerateResult, String> {
    generate_many_inner(repo_paths, true)
}

/// One walked input of a multi-repo build: the path as given, its root, its
/// walk, and its identity (disambiguated before phase 2 mints any RepoId).
type Walked<'a> = (&'a String, PathBuf, WalkResult, RepoIdentity);

fn generate_many_inner(repo_paths: &[String], incremental: bool) -> Result<GenerateResult, String> {
    let Assembled { mut merged, parse_errors, label_inputs, repo_roots, inputs } =
        assemble_many(repo_paths, incremental)?;
    run_code_passes(&mut merged);
    apply_external_cells(&mut merged, &inputs);
    let total_nodes: usize = merged.graphs.iter().map(|g| g.nodes.len()).sum();
    let total_edges: usize = merged.graphs.iter().map(|g| g.edges.len()).sum::<usize>()
        + merged.cross_edges.len();
    Ok(GenerateResult {
        merged,
        total_nodes,
        total_edges,
        parse_errors,
        repo_labels: crate::arch::repo_label_map(&label_inputs),
        repo_roots,
    })
}

/// A multi-repo build before its passes run: every repo's graphs merged in
/// slot order, no cross edge yet. [`generate_many_inner`] runs
/// [`crate::profile::CODE_PASSES`] over `merged`; a test can run them one at
/// a time instead.
pub(crate) struct Assembled {
    pub(crate) merged: MergedGraph,
    pub(crate) parse_errors: Vec<String>,
    /// `(RepoId.0, path as given)` per built repo, in argument order: the
    /// input of [`crate::arch::repo_label_map`].
    pub(crate) label_inputs: Vec<(u64, String)>,
    pub(crate) repo_roots: std::collections::BTreeMap<u64, String>,
    /// Each built repo's external inputs (LF.1a), in argument order.
    pub(crate) inputs: Vec<RepoInputs>,
}

/// Walk, parse and build every repo of a multi-repo build and merge the
/// graphs (phases 1 and 2 of `generate_many`), without running a pass. Errs
/// when no path produced a graph.
pub(crate) fn assemble_many(repo_paths: &[String], incremental: bool) -> Result<Assembled, String> {
    let mut all_graphs = Vec::new();
    let mut all_errors = Vec::new();
    let mut label_inputs: Vec<(u64, String)> = Vec::new();
    let mut repo_roots: std::collections::BTreeMap<u64, String> = std::collections::BTreeMap::new();
    let mut inputs: Vec<RepoInputs> = Vec::new();

    // Phase 1 — walk every repo before building any (A5.2), so the proto
    // service set is the UNION across the build: in a client/server split the
    // client repo ships no `.proto` of its own. The cost is holding every
    // repo's sources at once, which the 2-5 repo `--with` merges absorb.
    // A missing path keeps its slot so errors stay in argument order.
    // Each input's identity (LB.1) is computed here too, so inputs that share
    // a key are disambiguated against each other BEFORE any RepoId is minted.
    let mut rpc = RpcContext::default();
    let mut walked: Vec<Result<Walked<'_>, String>> = Vec::with_capacity(repo_paths.len());
    for path in repo_paths {
        let root = PathBuf::from(path);
        if !root.is_dir() {
            walked.push(Err(format!("not a directory: {path}")));
            continue;
        }
        let walk = walk_source_files(&root);
        rpc.add_files(&walk.0);
        let ident = repo_identity(&root);
        walked.push(Ok((path, root, walk, ident)));
    }
    let mut idents: Vec<RepoIdentity> = walked.iter().flatten().map(|w| w.3.clone()).collect();
    let abs_paths: Vec<String> = walked.iter().flatten().map(|w| canonical_display(&w.1)).collect();
    for line in disambiguate(&mut idents, &abs_paths) {
        eprintln!("{line}");
    }
    for (w, ident) in walked.iter_mut().flatten().zip(idents) {
        w.3 = ident;
    }

    // Phase 2 — build each repo against the union.
    for entry in walked {
        let (path, root, (files, regions, md, roots), ident) = match entry {
            Ok(w) => w,
            Err(e) => {
                all_errors.push(e);
                continue;
            }
        };
        // One key feeds both the RepoId and the cache's context check: every
        // cached FileParse has this RepoId baked into its NodeIds, so a sidecar
        // written under another identity must be discarded (#2). A re-spelled
        // or moved path keeps the key, so its sidecar is reused.
        let repo = RepoId::from_canonical(&ident.key);
        repo_id_marker(&ident, path);
        inputs.push(repo_inputs(repo, root.clone(), path.clone()));
        label_inputs.push((repo.0, path.clone()));
        // First path wins, like `repo_label_map` (inputs sharing a key are
        // disambiguated above, so a repeat is the same repo given twice).
        repo_roots.entry(repo.0).or_insert_with(|| path.clone());
        let go_prefix = read_go_module_prefix(&root);
        let mut cache = incremental.then(|| ParseCache::load(path));
        if let Some(c) = cache.as_mut() {
            c.validate_context(&ident.key, &go_prefix);
        }
        let (graphs, parse_errors) =
            build_graphs_for_repo(&files, repo, &go_prefix, cache.as_mut(), path, &rpc, &roots);
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
    Ok(Assembled {
        merged: MergedGraph::new(all_graphs),
        parse_errors: all_errors,
        label_inputs,
        repo_roots,
        inputs,
    })
}

/// LB.1 fired_on marker, one line per repo per build:
///   `[repo-id] source=<git-remote|git-local|dir> key=<identity key> repo=<path as given>`
/// The key never carries remote-URL userinfo (`normalise_remote_url` drops it).
fn repo_id_marker(ident: &RepoIdentity, repo_path: &str) {
    eprintln!("[repo-id] source={} key={} repo={repo_path}", ident.source.as_str(), ident.key);
}

/// The canonical absolute spelling of `root`, or `root` as given when it
/// cannot be resolved. Only [`disambiguate`] suffixes it onto a key.
fn canonical_display(root: &Path) -> String {
    std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Inputs of ONE multi-repo build that share an identity key (two non-git
/// `app/` dirs, two worktrees of one clone) must still be distinct repos, or
/// every same-qname node collides. Every member of a colliding group becomes
/// `<key>@<canonical abs path>` — every member, never "first wins", so the
/// result does not depend on argument order. Groups are found in argument
/// order by linear scan (no HashMap iteration reaches the output). Returns one
/// `[repo-id]` collision line per group, for the caller to print.
fn disambiguate(idents: &mut [RepoIdentity], abs_paths: &[String]) -> Vec<String> {
    let original: Vec<String> = idents.iter().map(|i| i.key.clone()).collect();
    let mut grouped = vec![false; original.len()];
    let mut lines = Vec::new();
    for i in 0..original.len() {
        if grouped[i] {
            continue;
        }
        let members: Vec<usize> =
            (i..original.len()).filter(|&j| original[j] == original[i]).collect();
        for &j in &members {
            grouped[j] = true;
        }
        if members.len() < 2 {
            continue;
        }
        for &j in &members {
            if let (Some(id), Some(abs)) = (idents.get_mut(j), abs_paths.get(j)) {
                id.key = format!("{}@{abs}", original[j]);
            }
        }
        lines.push(format!(
            "[repo-id] {} inputs share key {}; disambiguated by path",
            members.len(),
            original[i]
        ));
    }
    lines
}

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

        // Same dir, different path spelling → same identity key (LB.1), so the
        // same RepoId is baked into cached NodeIds → reuse.
        let mut cache = ParseCache::new();
        generate_one_with_cache(repo, &mut cache).unwrap();
        assert_eq!(cache.stats.reparsed, 1);
        let alt = format!("{repo}/.");
        generate_one_with_cache(&alt, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 1, "a path-spelling change must reuse");
        assert_eq!(cache.stats.reparsed, 0);

        // A different identity (another basename, no git) → another RepoId →
        // every entry discarded, even though a.py's path and bytes match.
        let other = unique_tmp("ctx_other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("a.py"), "def foo():\n    return 1\n").unwrap();
        generate_one_with_cache(other.to_str().unwrap(), &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 0, "another repo identity must not reuse");
        assert_eq!(cache.stats.reparsed, 1);
        std::fs::remove_dir_all(&other).ok();

        // Back to the first repo: discarded again, then reuse works.
        generate_one_with_cache(&alt, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 0);
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

    fn ident(key: &str) -> RepoIdentity {
        RepoIdentity {
            key: key.to_string(),
            source: repo_graph_code_domain::walk_gating::IdentitySource::Directory,
        }
    }

    /// Every member of a colliding group is suffixed (never "first wins"), a
    /// unique key is left alone, and one collision line is emitted per group.
    #[test]
    fn disambiguate_suffixes_every_member_of_a_shared_key() {
        let mut ids = vec![ident("dir:app"), ident("dir:lib"), ident("dir:app"), ident("dir:app")];
        let abs: Vec<String> = ["/t/a/app", "/t/lib", "/t/b/app", "/t/c/app"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let lines = disambiguate(&mut ids, &abs);
        let keys: Vec<&str> = ids.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["dir:app@/t/a/app", "dir:lib", "dir:app@/t/b/app", "dir:app@/t/c/app"]);
        assert_eq!(lines, ["[repo-id] 3 inputs share key dir:app; disambiguated by path"]);

        // Argument order changes the order of the inputs, never their keys.
        let mut rev = vec![ident("dir:app"), ident("dir:app")];
        let rev_abs = vec!["/t/b/app".to_string(), "/t/a/app".to_string()];
        disambiguate(&mut rev, &rev_abs);
        assert_eq!(rev[0].key, "dir:app@/t/b/app");
        assert_eq!(rev[1].key, "dir:app@/t/a/app");

        let mut solo = vec![ident("dir:app"), ident("git:github.com/x/y")];
        assert!(disambiguate(&mut solo, &abs[..2]).is_empty());
        assert_eq!(solo[0].key, "dir:app");
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
