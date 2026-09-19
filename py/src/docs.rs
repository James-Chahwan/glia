//! **governing_docs** (tier-4 P3): the doc sections that document a symbol.

use pyo3::prelude::*;

use crate::convert::to_py;
use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    /// **governing_docs** (tier-4 P3 payoff): the doc sections that DOCUMENTS
    /// `qname` — "what are the rules for X?" — located, in one call. Returns a
    /// dict `{results, absence}` (LD.8a): `results` is the records
    /// `{id, qname, name, kind, score, live, file, line}` (`live`: an
    /// entrypoint reaches the section; `line` is 1-based);
    /// `absence` is `None` when there are results, else the FACT-tier reason
    /// — `unknown_symbol` (with `suggestions`), `no_edges` (with the DOCUMENTS
    /// coverage `caveats`) or `no_match` (scope removed every section) — and
    /// `unparsed_files` is `len(parse_errors)`. An unknown qname is an
    /// absence, not a ValueError.
    /// `scope` (optional) keeps only the sections whose own file lives under
    /// that repo-relative path or project label (see `project_roots`);
    /// sections with no file are KEPT.
    #[pyo3(signature = (qname, scope=None))]
    fn governing_docs(
        &self,
        py: Python<'_>,
        qname: &str,
        scope: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        let mut docs =
            repo_graph_engine::governing_docs_with_live(&self.merged, self.live(), qname, scope);
        if let Some(a) = docs.absence.as_mut() {
            a.unparsed_files = self.parse_errors.len();
        }
        to_py(py, serde_json::to_string(&docs))
    }
}
