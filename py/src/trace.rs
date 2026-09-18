//! **cross_stack_trace** (P3): the ordered cross-service path of a feature.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    /// **cross_stack_trace** (P3, handoff v6): follow `feature` forward across
    /// service boundaries and return the ORDERED path — each hop labeled with its
    /// `mechanism` (http/queue/grpc/call/…) and `cross_service` — in one call.
    /// Where `blast_radius` gives a ranked set, this gives the sequence: how a
    /// request flows end to end. Each hop `{depth, mechanism, cross_service,
    /// from_qname, to_qname, to_kind, to_file, to_line}`. Returns a JSON array.
    #[pyo3(signature = (feature, depth=6))]
    fn cross_stack_trace(&self, feature: &str, depth: usize) -> PyResult<String> {
        let answer = repo_graph_engine::cross_stack_trace(&self.merged, feature, depth)
            .map_err(PyValueError::new_err)?;
        serde_json::to_string(&answer).map_err(|e| PyValueError::new_err(e.to_string()))
    }
}
