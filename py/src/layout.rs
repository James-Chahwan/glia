//! Module functions for the on-disk `.gmap` layout: loading a cached graph,
//! its conventional directory, and the staleness check.

use std::path::Path;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_engine::persist::{default_layout_dir, load_layout};
use repo_graph_store::is_gmap_stale;

use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// Load a previously-generated graph from a sharded `.gmap` directory.
/// `dir` must contain `manifest.json` + the per-shard `.gmap` files written by
/// `PyGraph.save_to` / `save_to_default` / `generate`'s auto-persist. Returns a
/// `PyGraph` whose downstream methods (node_count, dense_text, activate,
/// service_map, …) behave like the fresh `generate()` result that was saved:
/// repo labels, repo roots, parse errors and `RepoGraph.properties` are all
/// persisted (LC.7). A layout written without that metadata loads with none,
/// so `service_map` names its repos `repo<id>`. The error text is the engine's
/// `LoadError`, ending "rebuild the graph" when regenerating is the fix.
#[pyfunction]
fn load_from_gmap(dir: &str) -> PyResult<PyGraph> {
    load_layout(Path::new(dir))
        .map(PyGraph::from_result)
        .map_err(|e| PyValueError::new_err(format!("load_from_gmap({dir}): {e}")))
}

/// Conventional gmap directory path for a repo: `<repo>/.glia/graph` (0.4.x:
/// `<repo>/.ai/repo-graph`, no longer read or written). The one layout
/// `generate`'s auto-persist, `save_to_default`, `glia build` and the
/// `glia install-hooks` hooks all write, and the directory holding only
/// engine output (a watcher should skip exactly this prefix, not all of
/// `.glia/`, whose `overlay.toml` is an input). The Python wrapper uses this
/// to know where to look for a cached graph.
#[pyfunction]
fn default_gmap_dir(repo_path: &str) -> String {
    default_layout_dir(Path::new(repo_path))
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
/// `.idea`, `.vscode`), engine output (the gmap dir, `<repo>/.glia/graph` and
/// the legacy `<repo>/.ai/repo-graph`, by prefix), dependency and build-output
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
