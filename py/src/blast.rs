//! **blast_radius** (P3, LD.5): the ranked, located closure around one or
//! more nodes, as one walk and one ranking.

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use glia_engine::BlastOptions;
use glia_engine::identity::Collapse;
use glia_engine::rank::Ranking;
use glia_graph::Reach;

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
    ///
    /// `collapse` (DD.10, default `True`): identity classes. A walk class (one
    /// message, contract op or SCIP symbol, or a package and its project, in
    /// several places) is ONE node in the walk and the ranking, so a change
    /// reaching one copy reaches the other copies' neighbours at zero hops; a
    /// FACT / DERIVED class is ONE row, folded before `scope` and `top_k`,
    /// carrying an `identity` dict `{tier, rules, diverged, also_in}`
    /// (`also_in`: `{repo, qname, file, line}` of every other member, `repo`
    /// a build label). A seed in a class carries its `identity` too. Rows and
    /// seeds outside a class have no `identity` key. `collapse=False` is the
    /// per-member walk and rows.
    ///
    /// `ranking` (0.5.3, EA.5 / EA.7; `ppr`, `ppr-wide` or `bfs`) is the
    /// structural file ranking that orders the rows by their files, and `cochange` (default `True`) puts co-change from the history
    /// snapshot (`glia history sync`, `.glia/history-snapshot/` under a repo
    /// root) first, the structural ranking breaking its ties; with no
    /// snapshot the order is the structural one. `files` lists the top
    /// `top_files` ranked files. The rows' PPR `score`s never change with the
    /// ranking. Any other `ranking` raises ValueError (``unknown ranking `x`:
    /// expected one of ppr, ppr-wide, bfs``) before any work. The indexes
    /// behind the ranking are held by this PyGraph and reused by later calls
    /// (`[rank-cache] holder=pygraph .. index=hit`).
    ///
    /// `ranking=None` (the default, EC.8) applies the repo's calibration
    /// (`history_calibrate`, `.glia/calibration-snapshot/` under the repo
    /// root): the record's pick for the seeds' shape from SELh0 (H10 /
    /// H10[PT*] / H10[B0]), or SELs0 (P1 / PT* / B0) with `cochange=False`;
    /// with no applicable record it is `ppr`. A given `ranking` is explicit and
    /// never overridden. The dict gains `selection` `{set, shape, pick,
    /// source, reason, test_commits}` (`source` `calibrated`, `fallback` or
    /// `explicit`) whenever a seed resolved.
    #[pyo3(signature = (qnames, direction="both", depth=4, top_k=None, live_only=false, scope=None, collapse=true, ranking=None, cochange=true, top_files=10))]
    #[allow(clippy::too_many_arguments)]
    fn blast_radius(
        &self,
        py: Python<'_>,
        qnames: &Bound<'_, PyAny>,
        direction: &str,
        depth: usize,
        top_k: Option<usize>,
        live_only: bool,
        scope: Option<&str>,
        collapse: bool,
        ranking: Option<&str>,
        cochange: bool,
        top_files: usize,
    ) -> PyResult<Py<PyAny>> {
        let (ranking, select) = ranking_selected(ranking, cochange).map_err(PyValueError::new_err)?;
        let queries = seed_queries(qnames)?;
        let mut opts = BlastOptions::default();
        opts.ranking = ranking;
        opts.select = select;
        opts.top_files = top_files;
        opts.rank_cache = Some(self.rank.clone());
        opts.direction = reach_named(direction).map_err(PyValueError::new_err)?;
        opts.depth = depth;
        opts.top_k = top_k;
        opts.live_only = live_only;
        opts.scope = scope.map(str::to_string);
        opts.collapse = if collapse {
            Collapse::with_labels(&self.repo_labels)
        } else {
            Collapse::off()
        };
        let queries: Vec<&str> = queries.iter().map(String::as_str).collect();
        let answer = glia_engine::blast_radius(&self.merged, &queries, &opts);
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

/// The `ranking` / `cochange` keyword arguments of every ranked answer
/// (`blast_radius`, `diff_impact`, `diff_impact_vs_rev`, `review_vs_rev`,
/// `pack`, `pack_ids`): `ranking` a structural base name, `cochange` whether
/// co-change ranks first. `Err` is the engine's parse message (``unknown
/// ranking `x`: expected one of ppr, ppr-wide, bfs``).
pub(crate) fn ranking_named(ranking: &str, cochange: bool) -> Result<Ranking, String> {
    Ranking::parse(ranking, cochange)
}

/// EC.8: the optional `ranking` keyword of the calibrated answers
/// (`blast_radius`, `diff_impact`, `diff_impact_vs_rev`, `review_vs_rev`):
/// `None` is `ppr` with the calibration applied (`BlastOptions.select`), a
/// name is [`ranking_named`]'s ranking, explicit. Returns (ranking, select).
pub(crate) fn ranking_selected(ranking: Option<&str>, cochange: bool) -> Result<(Ranking, bool), String> {
    match ranking {
        None => Ok((ranking_named("ppr", cochange)?, true)),
        Some(name) => Ok((ranking_named(name, cochange)?, false)),
    }
}

/// The `direction` argument: `forward` | `backward` | `both`. Shared with
/// `diff_impact`, which takes the same keyword.
pub(crate) fn reach_named(direction: &str) -> Result<Reach, String> {
    match direction {
        "forward" => Ok(Reach::Forward),
        "backward" => Ok(Reach::Backward),
        "both" => Ok(Reach::Both),
        o => Err(format!("direction must be forward|backward|both, got `{o}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_names() {
        assert_eq!(ranking_named("ppr", true).expect("ppr"), Ranking::default());
        assert_eq!(ranking_named("ppr", false).expect("ppr"), Ranking::head());
        assert_eq!(ranking_named("ppr-wide", true).expect("wide").harness_name(), "H10[PT*]");
        assert_eq!(ranking_named("bfs", false).expect("bfs").harness_name(), "B0");
        assert_eq!(
            ranking_named("x", true).expect_err("x is no ranking"),
            "unknown ranking `x`: expected one of ppr, ppr-wide, bfs"
        );
    }

    #[test]
    fn an_absent_ranking_selects() {
        assert_eq!(ranking_selected(None, true), Ok((Ranking::default(), true)));
        assert_eq!(ranking_selected(None, false), Ok((Ranking::head(), true)));
        let (r, select) = ranking_selected(Some("bfs"), true).expect("bfs");
        assert_eq!((r.harness_name(), select), ("H10[B0]", false));
        assert_eq!(
            ranking_selected(Some("x"), true),
            Err("unknown ranking `x`: expected one of ppr, ppr-wide, bfs".to_string())
        );
    }
}
