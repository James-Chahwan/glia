//! **cross_stack_trace** (P3, LD.4a): the ranked cross-service paths of a
//! feature, or between two nodes; **entry_flows** (LD.4b): every entry
//! point's forward flow; **concept_flows** (GB.1): the flows that touch a
//! word.

use pyo3::prelude::*;

use glia_engine::identity::Collapse;
use glia_engine::trace::{
    ConceptOptions, TraceOptions, concept_flows_with_live, cross_stack_trace_with_live,
    entry_flows_with_live,
};

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
    /// - `resolved_by`: `qname`, `name`, `find`, `entry_flow`, `concept` or
    ///   `none`. `entry_flow` (LD.4b): the feature named a dead end (no carry
    ///   edge leaves it) or nothing, and the entry flow whose key matches the
    ///   word (see `entry_flows`) seeds the trace instead. `concept` (GB.1):
    ///   no key matched and the feature was no exact qname, so the entry of
    ///   the top `concept_flows` hit (the flow that touches the word) seeds
    ///   it.
    /// - `truncated`: the path search hit its step budget.
    /// - `absence`: set exactly when `paths` is empty (an unknown feature or
    ///   target, a dead end, an unreachable target). Never raises for those.
    /// - `collapse` (DD.11, default `True`): identity classes. A walk class
    ///   (one message, contract op or SCIP symbol, or a package and its
    ///   project, in several places) is ONE node of the walk, so a path
    ///   reaching one copy goes on through every copy's successors and two
    ///   copies' identical paths are one path; its hops show the class's
    ///   display member. A hop into a FACT / DERIVED class carries a
    ///   `to_identity` dict `{tier, rules, diverged, also_in}` (`also_in`:
    ///   `{repo, qname, file, line}` of every other member); other hops have
    ///   no `to_identity` key. `collapse=False` is the per-member walk.
    #[pyo3(signature = (feature, depth=6, to=None, max_paths=10, collapse=true))]
    fn cross_stack_trace(
        &self,
        py: Python<'_>,
        feature: &str,
        depth: usize,
        to: Option<&str>,
        max_paths: usize,
        collapse: bool,
    ) -> PyResult<Py<PyAny>> {
        let mut opts = TraceOptions::default();
        opts.depth = depth;
        opts.to = to.map(str::to_string);
        opts.max_paths = max_paths;
        opts.collapse = collapse_of(self, collapse);
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
    /// - `collapse` (DD.11, default `True`): two entries in one walk class
    ///   are one flow (its `entry` the class's display member when that is
    ///   one of them), and a walk class is one node of every flow, as in
    ///   `cross_stack_trace`. `collapse=False` is one flow per entry.
    #[pyo3(signature = (depth=6, collapse=true))]
    fn entry_flows(&self, py: Python<'_>, depth: usize, collapse: bool) -> PyResult<Py<PyAny>> {
        let flows = entry_flows_with_live(
            &self.merged,
            self.live(),
            &self.repo_labels,
            depth,
            &collapse_of(self, collapse),
        );
        to_py(py, serde_json::to_string(&flows))
    }

    /// **concept_flows** (GB.1): the entry flows that TOUCH `word`: by the
    /// entry's name, a hop's name or qname, a node kind (`cache` finds the
    /// flows that reach a CACHE), a hop's mechanism, a data node a hop reads,
    /// an external host, or the heading of a doc section documenting a hop.
    /// Every word of a multi-word query must match. Returns a dict
    /// `{results, absence}`:
    ///
    /// - `results`: at most `limit` rows (0 = all), ranked by (tier, depth,
    ///   reach desc, key, entry qname), each `{rank, key, entry, tier,
    ///   matched, hop, depth, reach, mechanisms}` (+ `blind`, the flow's
    ///   BLIND / DEGRADED caveats, when it has any). `tier` is one of
    ///   `entry`, `hop_name`, `hop_qname`, `kind`, `mechanism`,
    ///   `data_entity`, `external_host`, `doc` (best first): the weakest of
    ///   the words' best matches. `matched` lists per word what it matched
    ///   (`cache -> kind CACHE`); `hop` is the deepest match's hop dict (the
    ///   `cross_stack_trace` hop shape), `None` when it was the entry, and
    ///   `depth` its depth (0 for the entry).
    /// - `absence`: set exactly when `results` is empty: a fact-tier
    ///   `no_match`. Never raises for it.
    /// - `depth` and `collapse` as in `entry_flows`.
    #[pyo3(signature = (word, depth=6, limit=20, collapse=true))]
    fn concept_flows(
        &self,
        py: Python<'_>,
        word: &str,
        depth: usize,
        limit: usize,
        collapse: bool,
    ) -> PyResult<Py<PyAny>> {
        let mut opts = ConceptOptions::default();
        opts.depth = depth;
        opts.limit = limit;
        opts.collapse = collapse_of(self, collapse);
        opts.surface = "py";
        let answer =
            concept_flows_with_live(&self.merged, self.live(), &self.repo_labels, word, &opts);
        to_py(py, serde_json::to_string(&answer))
    }
}

/// The identity option of a `collapse=` keyword: on with the build's repo
/// labels, or off.
fn collapse_of(graph: &PyGraph, on: bool) -> Collapse {
    if on {
        Collapse::with_labels(&graph.repo_labels)
    } else {
        Collapse::off()
    }
}
