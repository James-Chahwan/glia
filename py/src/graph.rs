//! The `PyGraph` class. Declared once, here; its methods are spread over one
//! `#[pymethods] impl PyGraph` block per primitive module (pyo3
//! `multiple-pymethods`). This block holds the graph's own accessors: parse
//! state, counts, raw cells, and persistence.

use std::path::Path;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_core::{CellPayload, NodeId};
use repo_graph_graph::MergedGraph;
use repo_graph_store::{default_gmap_dir as store_default_gmap_dir, write_merged_sharded};

#[pyclass]
pub(crate) struct PyGraph {
    pub(crate) merged: MergedGraph,
    /// Files this build could not parse. Empty for a `.gmap`-loaded graph.
    pub(crate) parse_errors: Vec<String>,
    /// `RepoId.0` → human repo label, carried over from `GenerateResult`.
    /// A `RepoId` is an xxhash of the repo path, so the label cannot be
    /// recovered from the graph itself and does NOT live in the `.gmap`;
    /// empty for a `.gmap`-loaded graph. Only `service_map` reads it.
    pub(crate) repo_labels: std::collections::BTreeMap<u64, String>,
}

#[pymethods]
impl PyGraph {
    /// Files this build could not parse, as `"<path>: <reason>"`, in walk
    /// order. Empty for a graph loaded from a `.gmap` — parse state is not
    /// persisted. Lets a caller tell "this repo has no gRPC" apart from "the
    /// 40 files that would have shown gRPC all failed to parse".
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

    /// Persist this graph to a sharded `.gmap` layout at `dir`. Creates `dir`
    /// if missing. Idempotent: re-writing the same graph is content-hash
    /// skipped (see `write_sharded`'s skip-when-unchanged logic).
    fn save_to(&self, dir: &str) -> PyResult<()> {
        write_merged_sharded(&self.merged, Path::new(dir))
            .map(|_| ())
            .map_err(|e| PyValueError::new_err(format!("save_to({dir}): {e}")))
    }

    /// Convenience: save under the conventional `<repo>/.ai/repo-graph/`. The
    /// wrapper's cache-load path will find it there.
    fn save_to_default(&self, repo_path: &str) -> PyResult<()> {
        let dir = store_default_gmap_dir(Path::new(repo_path));
        write_merged_sharded(&self.merged, &dir)
            .map(|_| ())
            .map_err(|e| PyValueError::new_err(format!("save_to_default({}): {e}", dir.display())))
    }
}
