//! The `PyGraph` class. Declared once, here; its methods are spread over one
//! `#[pymethods] impl PyGraph` block per primitive module (pyo3
//! `multiple-pymethods`). This block holds the graph's own accessors: parse
//! state, counts, raw cells, and persistence. Every `PyGraph` is made by
//! [`PyGraph::from_result`] and saved by [`PyGraph::persist`], both over
//! `repo_graph_engine::persist` (LC.7), so a saved-then-loaded graph carries
//! the same labels, roots, parse errors and properties as the fresh one.

use std::collections::BTreeMap;
use std::path::Path;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_core::{CellPayload, NodeId};
use repo_graph_engine::GenerateResult;
use repo_graph_engine::persist::{layout_meta, persist_layout};
use repo_graph_graph::MergedGraph;
use repo_graph_store::default_gmap_dir as store_default_gmap_dir;

#[pyclass]
pub(crate) struct PyGraph {
    pub(crate) merged: MergedGraph,
    /// Files this build could not parse; a `.gmap`-loaded graph reports the
    /// list of the build that wrote the layout (LC.7).
    pub(crate) parse_errors: Vec<String>,
    /// `RepoId.0` → human repo label, carried over from `GenerateResult`.
    /// A `RepoId` is an xxhash of the repo identity key (git remote / git dir /
    /// dir name, LB.1), so the label cannot be recovered from the graph itself;
    /// it is persisted in the layout's `manifest.json` (LC.7) and restored by
    /// `load_from_gmap`. Only `service_map` reads it.
    pub(crate) repo_labels: BTreeMap<u64, String>,
    /// `RepoId.0` → repo root: the path as given on a fresh build, resolved
    /// against the layout dir on a load (LC.7). Recorded relative to the
    /// layout dir by `save_to` / `save_to_default` / `generate`'s auto-persist.
    pub(crate) repo_roots: BTreeMap<u64, String>,
}

impl PyGraph {
    /// The one constructor: every field from an engine result, fresh or
    /// loaded (`repo_graph_engine::persist::load_layout`).
    pub(crate) fn from_result(r: GenerateResult) -> Self {
        Self {
            merged: r.merged,
            parse_errors: r.parse_errors,
            repo_labels: r.repo_labels,
            repo_roots: r.repo_roots,
        }
    }

    /// Persist `merged` with its labels, roots and parse errors to `dir`
    /// through the engine's one persist path. `writer` names the caller in the
    /// `[gmap] meta` marker.
    pub(crate) fn persist(
        merged: &MergedGraph,
        labels: &BTreeMap<u64, String>,
        roots: &BTreeMap<u64, String>,
        parse_errors: &[String],
        dir: &Path,
        writer: &str,
    ) -> Result<(), String> {
        let meta = layout_meta(labels, roots, parse_errors, dir);
        persist_layout(merged, &meta, dir, writer)
    }

    fn persist_to(&self, dir: &Path) -> Result<(), String> {
        Self::persist(&self.merged, &self.repo_labels, &self.repo_roots, &self.parse_errors, dir, "py")
    }
}

#[pymethods]
impl PyGraph {
    /// Files this build could not parse, as `"<path>: <reason>"`, in walk
    /// order. A graph loaded from a `.gmap` reports the errors of the build
    /// that wrote it (persisted in `manifest.json` since LC.7). Lets a caller
    /// tell "this repo has no gRPC" apart from "the 40 files that would have
    /// shown gRPC all failed to parse".
    #[getter]
    fn parse_errors(&self) -> Vec<String> {
        self.parse_errors.clone()
    }

    fn node_count(&self) -> usize {
        self.merged.graphs.iter().map(|g| g.nodes.len()).sum()
    }

    fn edge_count(&self) -> usize {
        self.merged.graphs.iter().map(|g| g.edges.len()).sum::<usize>()
            + self.merged.cross_edges.len()
    }

    fn cross_edge_count(&self) -> usize {
        self.merged.cross_edges.len()
    }

    /// All cells on a node as `(cell_type_id, payload)` pairs (WP-J / #8).
    /// Pair the id with `cell_type_names()` to label. Text/Json payloads return
    /// their string (imports/state-var/doc cells included); Bytes payloads
    /// (cached embeddings) return "". Structured access instead of scraping
    /// `dense_text`. Empty if the node id is unknown.
    fn node_cells(&self, node_id: u64) -> Vec<(u32, String)> {
        let id = NodeId(node_id);
        for g in &self.merged.graphs {
            for n in &g.nodes {
                if n.id == id {
                    return n
                        .cells
                        .iter()
                        .map(|c| {
                            let payload = match &c.payload {
                                CellPayload::Text(s) | CellPayload::Json(s) => s.clone(),
                                CellPayload::Bytes(_) => String::new(),
                            };
                            (c.kind.0, payload)
                        })
                        .collect();
                }
            }
        }
        Vec::new()
    }

    /// Persist this graph to a sharded `.gmap` layout at `dir`, with its repo
    /// labels, repo roots (relative to `dir`) and parse errors in
    /// `manifest.json`. Creates `dir` if missing. Idempotent: re-writing the
    /// same graph is content-hash skipped (see `write_sharded`'s
    /// skip-when-unchanged logic).
    fn save_to(&self, dir: &str) -> PyResult<()> {
        self.persist_to(Path::new(dir))
            .map_err(|e| PyValueError::new_err(format!("save_to({dir}): {e}")))
    }

    /// Convenience: save under the conventional `<repo>/.ai/repo-graph/`. The
    /// wrapper's cache-load path will find it there.
    fn save_to_default(&self, repo_path: &str) -> PyResult<()> {
        let dir = store_default_gmap_dir(Path::new(repo_path));
        self.persist_to(&dir)
            .map_err(|e| PyValueError::new_err(format!("save_to_default({}): {e}", dir.display())))
    }
}
