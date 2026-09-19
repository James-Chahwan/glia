//! The whole-stack view: extraction `coverage` (P2), the manifest-rooted
//! `project_roots`, and the `service_map` behind `glia arch`.

use pyo3::prelude::*;

use repo_graph_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The whole body of [`PyGraph::service_map`], minus pyo3 — kept pyo3-free so
/// `cargo test -p repo-graph-py` can cover the binding (see the crate doc).
fn service_map_json(
    merged: &MergedGraph,
    repo_labels: &std::collections::BTreeMap<u64, String>,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&repo_graph_engine::service_map(merged, repo_labels))
}

#[pymethods]
impl PyGraph {
    /// **coverage** (P2, handoff v6): for the languages present in the repo, the
    /// known extraction caveats + how many edges of each flagged category exist
    /// — so a consumer falls back to grep DELIBERATELY where glia is
    /// known-partial (dynamic dispatch, string-built URLs, non-standard HTTP
    /// clients) instead of trusting a silent blind spot. Each note
    /// `{language, edge_category, note, verify, edges_found}`. Returns a list
    /// of dicts.
    fn coverage(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let report = repo_graph_engine::coverage_report(&self.merged);
        to_py(py, serde_json::to_string(&report))
    }

    /// **project_roots** (A8.6): the manifest-rooted sub-projects in this graph
    /// — the vocabulary for every `scope=` argument. Each record `{qname,
    /// label, ecosystem, manifest, path}`, sorted by `path` (`.` is the repo
    /// root). Pass a `label` (e.g. `@shop/web`) or a `path` as `scope`; both
    /// give the same answer. Read back out of the graph's PROJECT anchors, so
    /// a graph from `load_from_gmap` answers exactly like a fresh one.
    /// Returns a list of dicts.
    fn project_roots(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let roots = repo_graph_engine::project_roots(&self.merged);
        to_py(py, serde_json::to_string(&roots))
    }

    /// **service_map** (v6 follow-on): the architecture summary — one record
    /// per service (a manifest project root in a monorepo, with files under
    /// no root in one `(outside projects)` bucket; a top-level directory when
    /// the repo has no nested roots; one per repo when several were merged)
    /// plus the aggregated cross-service links, each
    /// labelled with its mechanism (HTTP_CALLS / QUEUE_FLOWS / GRPC_CALLS …)
    /// and the channel it travels over (route, topic, service name). Returns
    /// `{keying, services:[…], links:[…], self_links, unlocated_nodes}` as a
    /// dict; `keying` says which rule produced the service ids.
    ///
    /// Transport only — the keying and the link aggregation live in
    /// `repo_graph_engine::arch`, shared with `glia analyze`, so the CLI and
    /// MCP answers cannot drift apart.
    ///
    /// A graph from `load_from_gmap` carries the repo labels its layout's
    /// `manifest.json` recorded (LC.7), so it names services like the fresh
    /// build did; only a layout written without that metadata falls back to
    /// `repo<id>` ids.
    fn service_map(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        to_py(py, service_map_json(&self.merged, &self.repo_labels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A9.3: the binding is transport only, so the thing worth pinning is the
    /// wiring — the engine's `ServiceMap` serialises and the shape Python
    /// receives is the object the docstring promises. (Content correctness
    /// lives in the engine's own `arch_service_map` test; duplicating it here
    /// would only drift.)
    #[test]
    fn service_map_returns_the_documented_json_object() {
        let merged = MergedGraph::new(Vec::new());
        let json = service_map_json(&merged, &std::collections::BTreeMap::new())
            .expect("service_map serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let obj = v.as_object().expect("a JSON object, not an array");
        for key in [
            "keying",
            "services",
            "links",
            "self_links",
            "unlocated_nodes",
        ] {
            assert!(obj.contains_key(key), "missing `{key}` in {json}");
        }
        assert!(obj["services"].is_array());
        assert!(obj["links"].is_array());
    }
}
