//! **blast_radius** (P3): the ranked, located closure around one node.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::convert::to_py;
use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    /// **blast_radius** (P3, handoff v6): the complete, deduped,
    /// edge-category-aware, PPR-ranked, LOCATED closure around `qname` — the
    /// answer that `find`→`impact`→`activate`→`read×N` composed to, in ONE call.
    /// Structural `imports`/`contains` edges are excluded so the radius doesn't
    /// fan out through shared containers (handoff P1 bullet 4). Each record:
    /// `{id, qname, name, kind, reason, depth, score, live, file, line}` where
    /// `reason` is the edge category that first put the node in scope, `live`
    /// says whether an entry point reaches the node, and `line` is 1-based.
    /// `direction` ∈ {`forward` (what it affects), `backward` (what affects
    /// it), `both`}. Returns a list of dicts, ranked by PPR score (desc).
    /// Raises ValueError if `qname` resolves to no node.
    ///
    /// `scope` (optional, default `None` = no-op) restricts the answer to nodes
    /// whose file lives under that repo-relative path — or under the project
    /// with that label (see `project_roots`) — applied BEFORE the
    /// `top_k` cut so a scoped `top_k` spends its budget in scope. Nodes with no
    /// locatable file (ENDPOINT/ROUTE/doc spaces) are KEPT. `scope` narrows
    /// WITHIN a repo — under a multi-repo merge each repo's paths are relative
    /// to its OWN root, so a path passed as a separate repo will not match.
    #[pyo3(signature = (qname, direction="both", depth=4, top_k=None, live_only=false, scope=None))]
    fn blast_radius(
        &self,
        py: Python<'_>,
        qname: &str,
        direction: &str,
        depth: usize,
        top_k: Option<usize>,
        live_only: bool,
        scope: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        let answer = repo_graph_engine::blast_radius_by_qname(
            &self.merged,
            qname,
            direction,
            depth,
            top_k,
            live_only,
            scope,
        )
        .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }
}
