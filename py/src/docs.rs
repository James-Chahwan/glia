//! **governing_docs** (tier-4 P3): the doc sections that document a symbol.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    /// **governing_docs** (tier-4 P3 payoff): the doc sections that DOCUMENTS
    /// `qname` — "what are the rules for X?" — located, in one call. Each record
    /// `{id, qname, name, kind, score, file, line}`. Returns a JSON array.
    /// `scope` (optional) keeps only the sections whose own file lives under
    /// that repo-relative path or project label (see `project_roots`);
    /// sections with no file are KEPT.
    #[pyo3(signature = (qname, scope=None))]
    fn governing_docs(&self, qname: &str, scope: Option<&str>) -> PyResult<String> {
        let docs = repo_graph_engine::governing_docs(&self.merged, qname, scope)
            .map_err(PyValueError::new_err)?;
        serde_json::to_string(&docs).map_err(|e| PyValueError::new_err(e.to_string()))
    }
}
