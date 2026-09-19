//! **cross_stack_trace** (P3, LD.4a): the ranked cross-service paths of a
//! feature, or between two nodes; **entry_flows** (LD.4b): every entry
//! point's forward flow.

use pyo3::prelude::*;

use repo_graph_engine::trace::{TraceOptions, cross_stack_trace_with_live, entry_flows_with_live};

use crate::convert::to_py;
use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    /// **cross_stack_trace** (P3, LD.4a): follow `feature` forward across
    /// service boundaries and answer with the ranked DISTINCT paths it takes.
    /// Returns a dict `{seed, target, resolved_by, hops, paths, truncated,
    /// absence}`:
    ///
    /// - `paths`: ranked by (cross-service hops desc, distinct mechanisms desc,
    ///   length desc, qname sequence asc); each `{rank, hops, cross_service_hops,
    ///   mechanisms, length, directed}`. At most `max_paths` (0 = all).
    /// - `hops`: the seed's forward BFS tree (the pre-0.5.0 list), each hop
    ///   `{depth, mechanism, cross_service, cross_repo, from_qname, to_qname,
    ///   to_kind, to_live, to_file, to_line}`; `to_line` is 1-based.
    ///   `cross_service` also counts a manifest-project boundary inside one
    ///   repo (the service `glia arch` draws); `cross_repo` is the repo one.
    /// - `to`: two-node mode — the directed paths from `feature` to `to`, or
    ///   the shortest path over any edge walked either way (`directed` False).
    /// - `resolved_by`: `qname`, `name`, `find`, `entry_flow` or `none`.
    ///   `entry_flow` (LD.4b): the feature named a dead end (no carry edge
    ///   leaves it) or nothing, and the entry flow whose key matches the word
    ///   (see `entry_flows`) seeds the trace instead.
    /// - `truncated`: the path search hit its step budget.
    /// - `absence`: set exactly when `paths` is empty (an unknown feature or
    ///   target, a dead end, an unreachable target). Never raises for those.
    #[pyo3(signature = (feature, depth=6, to=None, max_paths=10))]
    fn cross_stack_trace(
        &self,
        py: Python<'_>,
        feature: &str,
        depth: usize,
        to: Option<&str>,
        max_paths: usize,
    ) -> PyResult<Py<PyAny>> {
        let mut opts = TraceOptions::default();
        opts.depth = depth;
        opts.to = to.map(str::to_string);
        opts.max_paths = max_paths;
        let answer = cross_stack_trace_with_live(&self.merged, self.live(), feature, &opts);
        to_py(py, serde_json::to_string(&answer))
    }

    /// **entry_flows** (LD.4b): every entry point's forward flow over the
    /// carry edges (calls, HTTP, queues, ...; never DEFINES / CONTAINS) within
    /// `depth` hops. Returns a list of dicts `{key, entry, reach,
    /// cross_service, mechanisms, services, hops}`, sorted by (key, entry
    /// qname, entry id):
    ///
    /// - `key`: the entry's name lower-cased, spaces and hyphens as `_`
    ///   (`post_/orders`), the feature word `cross_stack_trace` resolves to
    ///   this entry when the word names a dead end. Not unique: every row is
    ///   kept.
    /// - `entry`: the located entry `{id, name, qname, kind, file, line}`.
    /// - `reach`: nodes reached (`len(hops)`, at least 1).
    /// - `services`: the repos (or, in a manifest-rooted monorepo, the `glia
    ///   arch` services) the flow touches, the entry's first;
    ///   `cross_service` is `len(services) > 1`.
    /// - `mechanisms`: distinct hop edge categories, in first-seen order.
    /// - `hops`: the forward BFS tree, the hop dicts `cross_stack_trace`
    ///   returns.
    #[pyo3(signature = (depth=6))]
    fn entry_flows(&self, py: Python<'_>, depth: usize) -> PyResult<Py<PyAny>> {
        let flows = entry_flows_with_live(&self.merged, self.live(), &self.repo_labels, depth);
        to_py(py, serde_json::to_string(&flows))
    }
}
