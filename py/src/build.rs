//! Module functions that BUILD a graph from source: `generate`,
//! `generate_many`, and the single-file `parse_file_to_json`.

use std::path::Path;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_code_domain::node_kind;
use repo_graph_core::{Confidence, RepoId};
use repo_graph_engine::{generate_many as engine_generate_many, generate_one, parse_one};
use repo_graph_store::default_gmap_dir as store_default_gmap_dir;

use crate::convert::escape_json;
use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// Build the graph for a repo. `incremental` (default True, WP-D) reuses a
/// per-file parse cache at `<repo>/.ai/repo-graph/parse_cache.bin` so unchanged
/// files skip tree-sitter; the result is identical to a clean build. Pass
/// `incremental=False` to force a full reparse (this also deletes the sidecar,
/// so the next incremental build starts cold).
#[pyfunction]
#[pyo3(signature = (repo_path, incremental=true))]
fn generate(repo_path: &str, incremental: bool) -> PyResult<PyGraph> {
    let result = if incremental {
        repo_graph_engine::generate_one_incremental(repo_path)
    } else {
        // An explicit clean build also discards the sidecar — otherwise the
        // next default-on build would reuse the cache the user was escaping.
        if let Err(e) = repo_graph_engine::ParseCache::purge(repo_path) {
            eprintln!("warning: could not remove parse cache: {e}");
        }
        generate_one(repo_path)
    }
    .map_err(PyValueError::new_err)?;
    if !result.parse_errors.is_empty()
        && result.merged.graphs.iter().all(|g| g.nodes.is_empty())
    {
        return Err(PyValueError::new_err(format!(
            "no nodes produced; {} parse errors: {}",
            result.parse_errors.len(),
            result.parse_errors.first().unwrap_or(&String::new())
        )));
    }
    // Auto-persist to the conventional gmap dir so the next session can
    // `load_from_gmap` instead of regenerating — labels, roots and parse
    // errors included (LC.7), so the loaded graph answers like this one.
    // Failure to write is logged but not fatal — a fresh in-memory graph is
    // still usable, the cache layer is an optimization. Opt out with
    // `GLIA_NO_PERSIST=1` (tests / experiments that don't want side effects on
    // the target repo).
    if std::env::var("GLIA_NO_PERSIST").as_deref() != Ok("1") {
        let dir = store_default_gmap_dir(Path::new(repo_path));
        if let Err(e) = PyGraph::persist(
            &result.merged,
            &result.repo_labels,
            &result.repo_roots,
            &result.parse_errors,
            &dir,
            "py",
        ) {
            eprintln!("[repo-graph-py] warning: failed to persist gmap: {e}");
        }
    }
    if !result.parse_errors.is_empty() {
        eprintln!(
            "[parse] {} file(s) failed to parse (see PyGraph.parse_errors)",
            result.parse_errors.len()
        );
    }
    Ok(PyGraph::from_result(result))
}

/// Generate a single MergedGraph from multiple repo paths. Each path becomes
/// its own RepoId, so cross-graph resolvers (HttpStack, DbResolver, etc.) fire
/// across the boundary. Used for substrate eval where one wants to validate
/// that two unrelated services pair correctly under the resolver layer.
///
/// `incremental=True` (WP-D, A1.4) gives each path its own per-file parse cache
/// at `<repo>/.ai/repo-graph/parse_cache.bin`, so unchanged files skip
/// tree-sitter; the result is identical to a clean build. The default is
/// False here — unlike `generate` — BECAUSE the substrate-gap eval grades
/// every multi-dir fixture through this entry point and must stay hermetic
/// (`GLIA_NO_PERSIST=1` does not gate the parse-cache sidecar). False never
/// touches an existing sidecar: this function has never written one by
/// default, so there is nothing to escape from.
#[pyfunction]
#[pyo3(signature = (repo_paths, incremental=false))]
fn generate_many(repo_paths: Vec<String>, incremental: bool) -> PyResult<PyGraph> {
    let result = if incremental {
        repo_graph_engine::generate_many_incremental(&repo_paths)
    } else {
        engine_generate_many(&repo_paths)
    }
    .map_err(PyValueError::new_err)?;
    if !result.parse_errors.is_empty() {
        eprintln!(
            "[parse] {} file(s) failed to parse (see PyGraph.parse_errors)",
            result.parse_errors.len()
        );
    }
    Ok(PyGraph::from_result(result))
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
    m.add_function(wrap_pyfunction!(parse_file_to_json, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "build", add: register } }
