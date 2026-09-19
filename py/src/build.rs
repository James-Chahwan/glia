//! Module functions that BUILD a graph from source: `generate`,
//! `generate_many`, the single-file `parse_file_to_json`, and
//! `purge_parse_cache`.
//!
//! **One build contract (LD.2).** `generate` and `generate_many` share
//! [`build_graph`]: the same default (`incremental=False`), the same meaning
//! of it, the same error and the same markers. A build only builds: neither
//! function writes the `.gmap` layout — `PyGraph.save_to` /
//! `save_to_default` are its only writers from Python (LC.9's single
//! writer) — and `incremental=False` reads, writes and purges nothing, so a
//! default build leaves the repo exactly as it found it.
//!
//! **`overlay` (LF.2b).** Both take `overlay=True`: apply the overlay sections
//! of each repo's `.glia/overlay.toml` (its `[[edge]]` stanzas).
//! `overlay=False` is the extraction-only build, passed to the engine as a
//! build option (never process-global state: builds may run on several
//! Python threads). Such a graph is marked, and `PyGraph.save_to_default`
//! refuses it: the default layout dir holds the overlay-applied graph, which
//! the MCP warm path loads without rebuilding. `save_to(dir)` is allowed.

use pyo3::exceptions::{PyOSError, PyValueError};
use pyo3::prelude::*;

use glia_code_domain::node_kind;
use glia_core::{Confidence, RepoId};
use glia_engine::{BuildOptions, GenerateResult, ParseCache, parse_one};

use crate::convert::escape_json;
use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// What a build reads: one repo (`generate`, the engine's `generate_one*`) or
/// N repos under one graph (`generate_many`, the engine's `generate_many*`).
/// The two engine entries stay distinct so each Python entry point keeps the
/// graph it built before LD.2.
enum Inputs<'a> {
    One(&'a str),
    Many(&'a [String]),
}

impl Inputs<'_> {
    fn repos(&self) -> usize {
        match self {
            Inputs::One(_) => 1,
            Inputs::Many(paths) => paths.len(),
        }
    }
}

/// The whole build behind `generate` / `generate_many`, minus pyo3 — kept
/// pyo3-free so `cargo test -p glia-py` covers it (see the crate doc).
///
/// `incremental=false` is PURE: no parse-cache read, write or purge, and no
/// layout write. `incremental=true` loads and saves each repo's own sidecar
/// `<repo>/.glia/graph/parse_cache.bin` (engine `*_incremental`); the graph is
/// identical either way. Err when the build fails, or when it produced no node
/// at all while files failed to parse (the parse errors say why). Prints
/// `[build] surface=pyo3 repos=<n> incremental=<bool>` (the fired_on marker)
/// and, when any file failed, `[parse] <n> file(s) failed to parse`. Built
/// with `opts`; without the overlay it also prints
/// `[overlay] disabled (overlay=False): not persisting to the default gmap dir`.
fn build_result(
    inputs: Inputs<'_>,
    incremental: bool,
    opts: &BuildOptions,
) -> Result<GenerateResult, String> {
    eprintln!("[build] surface=pyo3 repos={} incremental={incremental}", inputs.repos());
    if !opts.overlay {
        eprintln!("[overlay] disabled (overlay=False): not persisting to the default gmap dir");
    }
    let result = match inputs {
        Inputs::One(path) => glia_engine::generate_one_opts(path, incremental, opts),
        Inputs::Many(paths) => glia_engine::generate_many_opts(paths, incremental, opts),
    }?;
    checked(result)
}

/// The check both entry points share: Err when no node was produced while
/// files failed to parse; otherwise the failures are counted on stderr and the
/// result passes through. Before LD.2 only `generate` ran it.
fn checked(result: GenerateResult) -> Result<GenerateResult, String> {
    if !result.parse_errors.is_empty() {
        if result.merged.graphs.iter().all(|g| g.nodes.is_empty()) {
            return Err(format!(
                "no nodes produced; {} parse errors: {}",
                result.parse_errors.len(),
                result.parse_errors.first().map(String::as_str).unwrap_or_default()
            ));
        }
        eprintln!(
            "[parse] {} file(s) failed to parse (see PyGraph.parse_errors)",
            result.parse_errors.len()
        );
    }
    Ok(result)
}

/// [`build_result`] as the `PyGraph` both entry points return, marked with
/// whether the overlay applied; its error is a `ValueError`.
fn build_graph(inputs: Inputs<'_>, incremental: bool, overlay: bool) -> PyResult<PyGraph> {
    let opts = BuildOptions::default().with_overlay(overlay);
    build_result(inputs, incremental, &opts)
        .map(|r| PyGraph::from_result(r).with_overlay_applied(overlay))
        .map_err(PyValueError::new_err)
}

/// Build the graph of one repo. The repo gets one RepoId, from its
/// path-independent identity (git remote / git dir / dir name).
///
/// `incremental=False` (the default) is a pure build: it reads, writes and
/// deletes nothing under the repo. `incremental=True` reuses and refreshes the
/// per-file parse cache `<repo>/.glia/graph/parse_cache.bin`, so unchanged
/// files skip tree-sitter; the graph is identical to a clean build.
/// `purge_parse_cache` deletes that cache.
///
/// Nothing here writes the `.gmap` layout: call `save_to_default(repo_path)`
/// (or `save_to(dir)`) to persist it for `load_from_gmap`.
///
/// `overlay=True` (the default) applies the overlay sections of the repo's
/// `.glia/overlay.toml` (its `[[edge]]` stanzas); `overlay=False` builds the
/// extraction-only graph, which `save_to_default` refuses (use `save_to`).
///
/// Raises ValueError when the path is not a directory, or when no node was
/// produced and files failed to parse. Otherwise parse failures are listed in
/// `PyGraph.parse_errors` and counted on stderr.
#[pyfunction]
#[pyo3(signature = (repo_path, incremental=false, overlay=true))]
fn generate(repo_path: &str, incremental: bool, overlay: bool) -> PyResult<PyGraph> {
    build_graph(Inputs::One(repo_path), incremental, overlay)
}

/// Build ONE graph from several repos. Each path becomes its own RepoId, so
/// the cross-graph resolvers (HTTP, gRPC, queues, DB, ...) pair across the
/// boundary.
///
/// Same contract as `generate`: `incremental=False` (the default) reads,
/// writes and deletes nothing; `incremental=True` gives each path its own
/// parse cache `<repo>/.glia/graph/parse_cache.bin`; nothing writes the
/// `.gmap` layout; `overlay=False` skips every repo's overlay sections;
/// ValueError when no node was produced and files failed to parse. The
/// substrate-gap eval grades every multi-dir fixture through the default,
/// which keeps it hermetic.
#[pyfunction]
#[pyo3(signature = (repo_paths, incremental=false, overlay=true))]
fn generate_many(repo_paths: Vec<String>, incremental: bool, overlay: bool) -> PyResult<PyGraph> {
    build_graph(Inputs::Many(&repo_paths), incremental, overlay)
}

/// Delete the repo's parse cache `<repo>/.glia/graph/parse_cache.bin`, so the
/// next `incremental=True` build starts cold. The escape hatch
/// `generate(incremental=False)` was before LD.2 — that build no longer
/// touches the cache. A missing cache is not an error; any other I/O failure
/// raises OSError.
#[pyfunction]
fn purge_parse_cache(repo_path: &str) -> PyResult<()> {
    ParseCache::purge(repo_path)
        .map_err(|e| PyOSError::new_err(format!("purge_parse_cache({repo_path}): {e}")))
}

#[pyfunction]
fn parse_file_to_json(source: &str, path: &str, lang: &str) -> PyResult<String> {
    let repo = RepoId(1);
    let fp = parse_one(source, path, lang, repo).map_err(PyValueError::new_err)?;
    let _ = node_kind::MODULE; // ensure the import is preserved if module impl evolves

    let mut out = String::from("[");
    let mut first = true;
    for n in &fp.nodes {
        let kind = fp.nav.kind_by_id.get(&n.id).map(|k| k.0).unwrap_or(0);
        let name = fp.nav.name_by_id.get(&n.id).map(|s| s.as_str()).unwrap_or("");
        let qname = fp.nav.qname_by_id.get(&n.id).map(|s| s.as_str()).unwrap_or("");
        let conf = match n.confidence {
            Confidence::Strong => "strong",
            Confidence::Medium => "medium",
            Confidence::Weak => "weak",
        };
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!(
            r#"{{"id":{},"kind":{},"name":"{}","qname":"{}","confidence":"{}"}}"#,
            n.id.0,
            kind,
            escape_json(name),
            escape_json(qname),
            conf,
        ));
    }
    out.push(']');
    Ok(out)
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(generate, m)?)?;
    m.add_function(wrap_pyfunction!(generate_many, m)?)?;
    m.add_function(wrap_pyfunction!(purge_parse_cache, m)?)?;
    m.add_function(wrap_pyfunction!(parse_file_to_json, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "build", add: register } }

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// Every path under `root`, relative, directories included.
    fn tree(root: &Path) -> BTreeSet<PathBuf> {
        let mut out = BTreeSet::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("readable dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                out.insert(path.strip_prefix(root).expect("under root").to_path_buf());
            }
        }
        out
    }

    fn python_repo(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("glia-ld2-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        std::fs::write(
            root.join("app.py"),
            "def helper(x):\n    return x + 1\n\n\ndef main():\n    return helper(2)\n",
        )
        .expect("write fixture");
        root
    }

    /// LD.2: the default build is PURE. Before LD.2, `generate(path)` (default
    /// `incremental=True`) left `<repo>/.glia/graph/parse_cache.bin` and, unless
    /// `GLIA_NO_PERSIST=1`, the whole sharded layout with its `manifest.json`
    /// under the repo. Now two `incremental=false` builds — one repo and the
    /// many-repo entry — add nothing, while the incremental build (the control)
    /// writes exactly its sidecar and never a layout, and `purge` removes it.
    #[test]
    fn pure_build_writes_nothing_into_the_repo() {
        let root = python_repo("pure");
        let path = root.to_str().expect("utf-8 temp path").to_string();
        let before = tree(&root);

        let one = build_result(Inputs::One(&path), false, &BuildOptions::default()).expect("build");
        let again = build_result(Inputs::One(&path), false, &BuildOptions::default()).expect("build");
        let many = build_result(Inputs::Many(std::slice::from_ref(&path)), false, &BuildOptions::default()).expect("build");
        assert_eq!(tree(&root), before, "a pure build wrote into the repo");
        assert!(one.total_nodes > 0 && one.total_nodes == again.total_nodes);
        assert_eq!(one.total_nodes, many.total_nodes);

        // Control: the incremental build does write, so the check above can
        // see a write; it writes the parse cache only, never the layout.
        build_result(Inputs::One(&path), true, &BuildOptions::default()).expect("build");
        let added: Vec<PathBuf> = tree(&root).difference(&before).cloned().collect();
        let cache = Path::new(".glia").join("graph").join("parse_cache.bin");
        assert!(added.contains(&cache), "incremental build wrote {added:?}");
        assert!(
            !added.iter().any(|p| p.ends_with("manifest.json")),
            "a build must never write the layout: {added:?}"
        );

        ParseCache::purge(&path).expect("purge");
        assert!(!root.join(&cache).exists(), "purge left the cache");
        ParseCache::purge(&path).expect("purging a missing cache is not an error");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// LD.2: one error contract for both entries. `generate_many` now raises
    /// on "no node + parse errors" like `generate` always did (both run
    /// `checked`); a result with nodes passes whatever failed; a path that is
    /// not a directory is an error for both.
    #[test]
    fn both_entries_share_one_error_contract() {
        let empty = std::env::temp_dir().join(format!("glia-ld2-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).expect("temp dir");
        let path = empty.to_str().expect("utf-8 temp path").to_string();
        let mut nothing = glia_engine::generate_one(&path).expect("build");
        let _ = std::fs::remove_dir_all(&empty);
        assert!(nothing.merged.graphs.iter().all(|g| g.nodes.is_empty()));
        nothing.parse_errors.push("app.py: boom".into());
        match checked(nothing) {
            Err(e) => assert_eq!(e, "no nodes produced; 1 parse errors: app.py: boom"),
            Ok(_) => panic!("no node + parse errors must raise"),
        }

        let root = python_repo("errors");
        let path = root.to_str().expect("utf-8 temp path").to_string();
        let mut some = glia_engine::generate_one(&path).expect("build");
        let _ = std::fs::remove_dir_all(&root);
        some.parse_errors.push("other.py: boom".into());
        assert!(checked(some).is_ok(), "a graph with nodes passes its parse errors through");

        let missing = std::env::temp_dir().join(format!("glia-ld2-missing-{}", std::process::id()));
        let missing = missing.to_str().expect("utf-8 temp path").to_string();
        assert!(build_result(Inputs::One(&missing), false, &BuildOptions::default()).is_err());
        assert!(build_result(Inputs::Many(std::slice::from_ref(&missing)), false, &BuildOptions::default()).is_err());
    }
}
