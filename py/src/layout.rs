//! Module functions for the on-disk `.gmap` layout: loading a cached graph,
//! its conventional directory, and the staleness check.

use std::path::Path;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_store::{
    default_gmap_dir as store_default_gmap_dir, is_gmap_stale, read_merged_sharded,
};

use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// Load a previously-generated graph from a sharded `.gmap` directory.
/// `dir` must contain `manifest.json` + the per-shard `.gmap` files written by
/// `PyGraph.save_to` / `save_to_default`. Returns a `PyGraph` whose downstream
/// methods (node_count, dense_text, activate, …) behave identically to a fresh
/// `generate()` result, except `RepoGraph.properties` is empty (parse-time-only
/// state, not persisted at FORMAT_VERSION=1).
#[pyfunction]
fn load_from_gmap(dir: &str) -> PyResult<PyGraph> {
    let merged = read_merged_sharded(Path::new(dir))
        .map_err(|e| PyValueError::new_err(format!("load_from_gmap({dir}): {e}")))?;
    Ok(PyGraph {
        merged,
        parse_errors: Vec::new(),
        // Not persisted at FORMAT_VERSION=1: `service_map` degrades to
        // `repo<id>` ids for this graph. Documented on the method.
        repo_labels: std::collections::BTreeMap::new(),
    })
}

/// Conventional gmap directory path for a repo: `<repo>/.ai/repo-graph`.
/// The Python wrapper uses this to know where to look for a cached graph.
#[pyfunction]
fn default_gmap_dir(repo_path: &str) -> String {
    store_default_gmap_dir(Path::new(repo_path))
        .to_string_lossy()
        .into_owned()
}

/// Is the cached gmap at `gmap_dir` older than anything under `repo_path` that
/// the builder would read? Used by the wrapper to decide between load and
/// regenerate. Returns `true` if the gmap is missing entirely, or if it was
/// written by a different engine build.
///
/// Directory gating is shared with the builder's walk, so the scan skips
/// exactly what the parse skips: VCS/editor metadata (`.git`, `.hg`, `.svn`,
/// `.idea`, `.vscode`), the gmap dir (`.ai/`), dependency and build-output
/// trees (`node_modules`, `vendor`, `bower_components`, `.venv`,
/// `site-packages`, `target`, `dist`, `build`, `out`, `__pycache__`, `.cache`,
/// `.next`, `.nuxt`, `.angular`, `coverage`), plain directory entries in the
/// repo's top-level `.gitignore`, and copied web bundles. Churn confined to
/// one of those no longer forces a regenerate. A collapsed directory is still
/// one REGION node, so a gated directory whose OWN mtime is newer than the
/// manifest (an entry created or removed directly inside it) does mark the
/// gmap stale.
#[pyfunction]
fn is_stale(gmap_dir: &str, repo_path: &str) -> bool {
    is_gmap_stale(Path::new(gmap_dir), Path::new(repo_path))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(load_from_gmap, m)?)?;
    m.add_function(wrap_pyfunction!(default_gmap_dir, m)?)?;
    m.add_function(wrap_pyfunction!(is_stale, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "layout", add: register } }
