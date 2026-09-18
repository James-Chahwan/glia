//! Text → nodes: `resolve` (P3, located records), the id-level
//! `resolve_signal`, and the name / qname lookups.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    /// **resolve** (P3, handoff v6): a failure/change signal (stacktrace, diff,
    /// test id, or `auto`-sniffed) → the ranked, LOCATED nodes it points at, in
    /// one call — the answer that `resolve_signal`→`activate`→`read×N` collapses
    /// to. Resolution order preserved. Returns a JSON object `{results,
    /// absence}` (LD.8a): `results` is the records
    /// `{id, qname, name, kind, score, file, line}`; `absence` is `null` when
    /// there are results, else the FACT-tier reason — `no_signal_match` (the
    /// signal resolved to no node; the note counts what it held) or `no_match`
    /// (`scope` or `top_k=0` removed every node) — with `unparsed_files` set to
    /// `len(parse_errors)`.
    ///
    /// `scope` (optional, default `None` = no-op; a path or a project label —
    /// see `project_roots`) filters the SEEDS before the
    /// PPR run, not the rendered result: frames resolve by file BASENAME, so an
    /// unscoped `utils.py` frame seeds every `utils.py` in the monorepo and
    /// those bogus seeds shape the scores of the real one. Scoped scores
    /// therefore legitimately differ from unscoped ones.
    #[pyo3(signature = (text, kind="auto", top_k=None, scope=None))]
    fn resolve(
        &self,
        text: &str,
        kind: &str,
        top_k: Option<usize>,
        scope: Option<&str>,
    ) -> PyResult<String> {
        let mut answer =
            repo_graph_engine::resolve_signal_located(&self.merged, text, kind, top_k, scope);
        if let Some(a) = answer.absence.as_mut() {
            a.unparsed_files = self.parse_errors.len();
        }
        serde_json::to_string(&answer).map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// Resolve a simple name to a node id. Deterministic across processes: when
    /// several nodes share the name (e.g. a class and a same-named module, or
    /// one symbol in two repos of a merge), a declaration beats a
    /// container, then the highest-degree node wins, rather than whichever the
    /// per-process `HashMap` seed happened to order first — the root cause of
    /// `impact`/`trace` intermittently returning empty. A framework role
    /// (component, service, ...) is no longer a same-name twin: LB.3a folds it
    /// into its declaration as a ROLE cell, listed in `nodes_json`'s `roles`.
    fn find_node(&self, name: &str) -> Option<u64> {
        self.merged.resolve_name(name).map(|id| id.0)
    }

    /// Substring search over qnames, returned sorted by node id so repeated
    /// calls (and any caller that takes `[0]`) are reproducible across processes.
    /// Resolve a failure/change signal to seed node ids (WP-B / GR-2 `locate`).
    /// `kind` ∈ {"stacktrace", "test", "diff", "auto"}; "auto" sniffs the shape.
    /// Frame/symbol/path → node-id resolution (and the sniffer) run in Rust;
    /// unresolvable tokens are simply absent. Feed the result to `activate`.
    #[pyo3(signature = (text, kind="auto"))]
    fn resolve_signal(&self, text: &str, kind: &str) -> Vec<u64> {
        self.merged.resolve_signal(text, kind).into_iter().map(|id| id.0).collect()
    }

    /// `scope` (optional) narrows the hits to one part of a monorepo using the
    /// same `/`-boundary rule as `blast_radius`/`resolve`/`governing_docs`, so
    /// a consumer never has to re-derive a path guess in Python. Nodes with no
    /// locatable file are KEPT. A project label (see `project_roots`) works
    /// here too — it is resolved once, before the per-node filter.
    #[pyo3(signature = (pattern, scope=None))]
    fn find_nodes_by_qname(&self, pattern: &str, scope: Option<&str>) -> Vec<u64> {
        let scope = scope.map(|s| repo_graph_engine::resolve_scope(&self.merged, s));
        self.merged
            .qnames_containing(pattern)
            .into_iter()
            .filter(|id| repo_graph_engine::node_in_scope(&self.merged, *id, scope.as_deref()))
            .map(|id| id.0)
            .collect()
    }
}
