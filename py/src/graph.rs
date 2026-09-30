//! The `PyGraph` class. Declared once, here; its methods are spread over one
//! `#[pymethods] impl PyGraph` block per primitive module (pyo3
//! `multiple-pymethods`). This block holds the graph's own accessors: parse
//! state, counts, raw cells, and persistence. Every `PyGraph` is made by
//! [`PyGraph::from_result`] and saved over `glia_engine::persist` (LC.7):
//! `save_to` through `persist_layout`, `save_to_default` through the single
//! layout writer `persist_graph` (LC.9), so a saved-then-loaded graph carries
//! the same labels, roots, parse errors and properties as the fresh one. These
//! two methods are the only way Python writes a layout: `generate` /
//! `generate_many` only build (LD.2). A graph built with `overlay=False`
//! (LF.2b) is extraction-only: `save_to_default` refuses it, so the repo's
//! default layout dir only ever holds the overlay-applied graph.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_core::{CellPayload, NodeId};
use glia_engine::GenerateResult;
use glia_engine::persist::{default_layout_dir, layout_meta, persist_graph, persist_layout};
use glia_graph::MergedGraph;
use glia_store::write_merged_sharded_for_repo;

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
    /// Whether the `.glia/overlay.toml` overlay sections applied (LF.2b):
    /// false only for a `generate(overlay=False)` build. A loaded layout is
    /// taken as overlay-applied: the writers keep an extraction-only graph out
    /// of the default dir.
    pub(crate) overlay_applied: bool,
    /// The entrypoint-reachable node set (LD.6), walked once on first use by
    /// [`PyGraph::live`] — `nodes_json`, `find`, `resolve`, `governing_docs`
    /// and `cross_stack_trace` all read it, so an MCP session pays the walk
    /// once. Valid while nothing liveness reads changes: node kinds and
    /// names, ROLE cells and edges. `set_cell` / `remove_cell` write only the
    /// externally-writable cells (`external_inputs::WRITABLE`: CONSTRAINT,
    /// DECISION, CONV, VECTOR), none of which it reads; a method that changes
    /// what it does read must reset this cell.
    live: OnceLock<HashSet<NodeId>>,
}

/// Why `save_to_default` refuses a graph, or `None` when it may write it: an
/// extraction-only graph (`overlay_applied == false`) never goes to the
/// default layout dir, whose graph the MCP warm path serves without
/// rebuilding. pyo3-free so a unit test covers it.
pub(crate) fn default_dir_refusal(overlay_applied: bool) -> Option<&'static str> {
    (!overlay_applied).then_some(
        "this graph was built with overlay=False; the default gmap dir holds the \
         overlay-applied graph (use save_to(dir) for an extraction-only graph)",
    )
}

impl PyGraph {
    /// The one constructor: every field from an engine result, fresh or
    /// loaded (`glia_engine::persist::load_layout`), overlay applied.
    pub(crate) fn from_result(r: GenerateResult) -> Self {
        Self {
            merged: r.merged,
            parse_errors: r.parse_errors,
            repo_labels: r.repo_labels,
            repo_roots: r.repo_roots,
            overlay_applied: true,
            live: OnceLock::new(),
        }
    }

    /// The live set of this graph: `entrypoint_reachable`, computed on the
    /// first call and cached (see the `live` field).
    pub(crate) fn live(&self) -> &HashSet<NodeId> {
        self.live.get_or_init(|| glia_engine::entrypoint_reachable(&self.merged))
    }

    /// `self` marked with whether its build applied the overlay.
    pub(crate) fn with_overlay_applied(mut self, applied: bool) -> Self {
        self.overlay_applied = applied;
        self
    }

    /// Persist this graph with its labels, roots and parse errors to an
    /// explicit `dir` through the engine's layout write (`save_to`); the
    /// repo's own layout dir goes through `persist_graph` instead.
    fn persist_to(&self, dir: &Path) -> Result<(), String> {
        let meta = layout_meta(&self.repo_labels, &self.repo_roots, &self.parse_errors, dir);
        persist_layout(&self.merged, &meta, dir, "py")
    }

    /// [`Self::persist_to`] recording the `.glia` inputs fingerprint of
    /// `repo_root` (LF.1d, `write_merged_sharded_for_repo`), for `save_to(dir,
    /// repo_path=...)`: the manifest of a custom dir then covers that repo's
    /// sidecars, so `is_stale(dir, repo_path)` sees a sidecar edit.
    fn persist_to_for_repo(&self, dir: &Path, repo_root: &Path) -> Result<(), String> {
        let meta = layout_meta(&self.repo_labels, &self.repo_roots, &self.parse_errors, dir);
        write_merged_sharded_for_repo(&self.merged, &meta, dir, repo_root)
            .map(|_| ())
            .map_err(|e| format!("py: persist to {}: {e}", dir.display()))
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

    /// False for a graph built with `generate(..., overlay=False)` /
    /// `generate_many(..., overlay=False)`: the extraction-only graph, which
    /// `save_to_default` refuses. True otherwise, a loaded layout included.
    #[getter]
    fn overlay_applied(&self) -> bool {
        self.overlay_applied
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
    ///
    /// A CODE payload of a graph loaded from a layout (CD.7c) is the source
    /// text while the repo's files are unchanged; when a file moved, changed
    /// or is gone it is instead the span to fetch yourself,
    /// `{"code_span":{"file":"src/a.rs","start":120,"end":480,"xxh64":"<16 hex>"}}`:
    /// bytes `[start, end)` of `file` under the repo root, xxhash64 seed 0.
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
    /// skip-when-unchanged logic). With `repo_path` (LF.1b), the manifest also
    /// records that repo's `.glia` inputs (cell sidecars, overlay, snapshots),
    /// so `is_stale(dir, repo_path)` sees them change; without it, a custom
    /// `dir` records them only for a graph of one repo root.
    #[pyo3(signature = (dir, repo_path=None))]
    fn save_to(&self, dir: &str, repo_path: Option<&str>) -> PyResult<()> {
        let d = Path::new(dir);
        match repo_path {
            None => self.persist_to(d),
            Some(root) => self.persist_to_for_repo(d, Path::new(root)),
        }
        .map_err(|e| PyValueError::new_err(format!("save_to({dir}): {e}")))
    }

    /// Save to the repo's layout dir `<repo>/.glia/graph/` (see
    /// `default_gmap_dir`) with the same single writer as `glia build` and the
    /// hooks: the dir's self-ignoring `.gitignore`, orphan-shard cleanup and
    /// the legacy `.ai/repo-graph` notice included. `load_from_gmap` finds it
    /// there. `generate` writes no layout (LD.2), so a caller that wants the
    /// next session to load instead of rebuild calls this after it. Raises
    /// ValueError on a graph built with `overlay=False` (LF.2b).
    fn save_to_default(&self, repo_path: &str) -> PyResult<()> {
        let dir = default_layout_dir(Path::new(repo_path));
        if let Some(why) = default_dir_refusal(self.overlay_applied) {
            return Err(PyValueError::new_err(format!("save_to_default({}): {why}", dir.display())));
        }
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

#[cfg(test)]
mod tests {
    use super::default_dir_refusal;
    use glia_code_domain::cell_type;
    use glia_code_domain::external_inputs::WRITABLE;

    /// LD.6 guard: `PyGraph.live` is cached for the graph's lifetime, and the
    /// only mutating methods (`set_cell` / `remove_cell`) write WRITABLE cells
    /// only. Liveness reads ROLE cells (the COMPONENT role seeds it), so a
    /// ROLE in WRITABLE would make the cache stale after a write: reset the
    /// cell in those methods before allowing it.
    #[test]
    fn writable_cells_leave_the_live_cache_valid() {
        assert!(!WRITABLE.contains(&cell_type::ROLE), "a ROLE write would change liveness");
    }

    /// LF.2b persist guard: only an overlay-applied graph may go to the
    /// default layout dir.
    #[test]
    fn default_dir_takes_only_overlay_applied_graphs() {
        assert_eq!(default_dir_refusal(true), None);
        let why = default_dir_refusal(false).expect("an overlay=False graph is refused");
        assert!(why.contains("overlay=False") && why.contains("save_to(dir)"), "{why}");
    }
}
