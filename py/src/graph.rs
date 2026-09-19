//! The `PyGraph` class. Declared once, here; its methods are spread over one
//! `#[pymethods] impl PyGraph` block per primitive module (pyo3
//! `multiple-pymethods`). This block holds the graph's own accessors: parse
//! state, counts, raw cells, and persistence. Every `PyGraph` is made by
//! [`PyGraph::from_result`] and saved over `repo_graph_engine::persist` (LC.7):
//! `save_to` through `persist_layout`, `save_to_default` through the single
//! layout writer `persist_graph` (LC.9), so a saved-then-loaded graph carries
//! the same labels, roots, parse errors and properties as the fresh one. These
//! two methods are the only way Python writes a layout: `generate` /
//! `generate_many` only build (LD.2).

use std::collections::BTreeMap;
use std::path::Path;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_core::{CellPayload, NodeId};
use repo_graph_engine::GenerateResult;
use repo_graph_engine::persist::{default_layout_dir, layout_meta, persist_graph, persist_layout};
use repo_graph_graph::MergedGraph;

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
    /// layout dir by `save_to` / `save_to_default`.
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

    /// Persist this graph with its labels, roots and parse errors to an
    /// explicit `dir` through the engine's layout write (`save_to`); the
    /// repo's own layout dir goes through `persist_graph` instead.
    fn persist_to(&self, dir: &Path) -> Result<(), String> {
        let meta = layout_meta(&self.repo_labels, &self.repo_roots, &self.parse_errors, dir);
        persist_layout(&self.merged, &meta, dir, "py")
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

    /// Save to the repo's layout dir `<repo>/.glia/graph/` (see
    /// `default_gmap_dir`) with the same single writer as `glia build` and the
    /// hooks: the dir's self-ignoring `.gitignore`, orphan-shard cleanup and
    /// the legacy `.ai/repo-graph` notice included. `load_from_gmap` finds it
    /// there. `generate` writes no layout (LD.2), so a caller that wants the
    /// next session to load instead of rebuild calls this after it.
    fn save_to_default(&self, repo_path: &str) -> PyResult<()> {
        let dir = default_layout_dir(Path::new(repo_path));
        persist_graph(
            &self.merged,
            &self.repo_labels,
            &self.repo_roots,
            &self.parse_errors,
            &dir,
            "py",
        )
        .map_err(|e| PyValueError::new_err(format!("save_to_default({}): {e}", dir.display())))
    }
}
