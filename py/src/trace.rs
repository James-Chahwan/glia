//! **cross_stack_trace** (P3, LD.4a): the ranked cross-service paths of a
//! feature, or between two nodes.

use pyo3::prelude::*;

use repo_graph_engine::trace::{TraceOptions, cross_stack_trace_with_live};

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
    /// - `resolved_by`: `qname`, `name`, `find` or `none`.
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
}
