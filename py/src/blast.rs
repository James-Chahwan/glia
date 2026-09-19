//! **blast_radius** (P3, LD.5): the ranked, located closure around one or
//! more nodes, as one walk and one ranking.

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use repo_graph_engine::BlastOptions;
use repo_graph_graph::Reach;

use crate::convert::to_py;
use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    /// **blast_radius** (P3, handoff v6; many seeds LD.5): the complete,
    /// deduped, edge-category-aware, PPR-ranked, LOCATED closure around every
    /// node `qnames` names — the answer that `find`→`impact`→`activate`→
    /// `read×N` composed to, in ONE call. `qnames` is one qname / name (a
    /// `str`) or a `list` of them: many seeds are ONE walk and ONE PPR, so
    /// every score comes from the same run, each node's `depth` counts from
    /// the nearest seed, and no seed is ever a row.
    ///
    /// Returns a dict `{seeds, unresolved, results, absence}`:
    /// - `seeds`: one per distinct node, `{query, id, qname, kind, file,
    ///   line, linked_seeds}`; `linked_seeds` names the other seeds one carry
    ///   edge away in `direction` (the fact a union of per-seed answers loses);
    /// - `unresolved`: the queries that name no node (never an error);
    /// - `results`: `{id, qname, name, kind, reason, depth, score, live, file,
    ///   line, seed}` ranked by PPR score (desc), where `reason` is the edge
    ///   category that first put the node in scope, `live` says whether an
    ///   entry point reaches it, `line` is 1-based and `seed` is the qname of
    ///   the seed whose wave reached it first;
    /// - `absence`: why `results` is empty (LD.8a), else `None`.
    ///
    /// Structural `imports`/`contains` edges are excluded so the radius doesn't
    /// fan out through shared containers (handoff P1 bullet 4). `direction` ∈
    /// {`forward` (what it affects), `backward` (what affects it), `both`};
    /// any other value raises ValueError, a `qnames` that is neither a `str`
    /// nor a list of `str` raises TypeError.
    ///
    /// `scope` (optional, default `None` = no-op) restricts the answer to nodes
    /// whose file lives under that repo-relative path — or under the project
    /// with that label (see `project_roots`) — applied BEFORE the
    /// `top_k` cut so a scoped `top_k` spends its budget in scope. Nodes with no
    /// locatable file (ENDPOINT/ROUTE/doc spaces) are KEPT. `scope` narrows
    /// WITHIN a repo — under a multi-repo merge each repo's paths are relative
    /// to its OWN root, so a path passed as a separate repo will not match.
    #[pyo3(signature = (qnames, direction="both", depth=4, top_k=None, live_only=false, scope=None))]
    fn blast_radius(
        &self,
        py: Python<'_>,
        qnames: &Bound<'_, PyAny>,
        direction: &str,
        depth: usize,
        top_k: Option<usize>,
        live_only: bool,
        scope: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        let queries = seed_queries(qnames)?;
        let mut opts = BlastOptions::default();
        opts.direction = reach_named(direction).map_err(PyValueError::new_err)?;
        opts.depth = depth;
        opts.top_k = top_k;
        opts.live_only = live_only;
        opts.scope = scope.map(str::to_string);
        let queries: Vec<&str> = queries.iter().map(String::as_str).collect();
        let answer = repo_graph_engine::blast_radius(&self.merged, &queries, &opts);
        to_py(py, serde_json::to_string(&answer))
    }
}

/// `qnames` as a query list: a `str` is one query, a sequence of `str` many.
/// A `str` is tried first — pyo3 refuses to read a `str` as a `Vec`.
fn seed_queries(qnames: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    if let Ok(one) = qnames.extract::<String>() {
        return Ok(vec![one]);
    }
    qnames.extract::<Vec<String>>().map_err(|_| {
        PyTypeError::new_err("qnames must be a str or a list of str")
    })
}

/// The `direction` argument: `forward` | `backward` | `both`.
fn reach_named(direction: &str) -> Result<Reach, String> {
    match direction {
        "forward" => Ok(Reach::Forward),
        "backward" => Ok(Reach::Backward),
        "both" => Ok(Reach::Both),
        o => Err(format!("direction must be forward|backward|both, got `{o}`")),
    }
}
