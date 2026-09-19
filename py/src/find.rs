//! Text → nodes: `find` (LD.3b's ranked fuzzy find, the one name / qname
//! lookup) and `resolve` (P3: a failure / change signal → located records).
//! Both return the LD.8a envelope `{results, absence}` as a native `dict`.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use std::collections::HashSet;

use glia_code_domain::node_kind;
use glia_core::{NodeId, NodeKindId};
use glia_engine::absence::Answer;
use glia_engine::find::{FindOptions, FoundNode, find_nodes_with_live};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

// `find`'s `top_k=20` below is a literal so `__text_signature__` shows it;
// this keeps it the engine's default.
const _: () = assert!(glia_engine::find::DEFAULT_TOP_K == 20);

/// Node-kind names → ids, case-insensitively, as `glia find --kind` reads
/// them. An unknown name is an error naming every valid one.
fn kinds_by_name(names: &[String]) -> Result<Vec<NodeKindId>, String> {
    names
        .iter()
        .map(|n| {
            node_kind::ALL
                .iter()
                .find(|(_, name)| name.eq_ignore_ascii_case(n))
                .map(|(id, _)| *id)
                .ok_or_else(|| {
                    let valid: Vec<&str> = node_kind::ALL.iter().map(|(_, name)| *name).collect();
                    format!("unknown kind '{n}'; valid kinds: {}", valid.join(", "))
                })
        })
        .collect()
}

/// The whole body of [`PyGraph::find`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` covers it (see the crate doc). An empty
/// `kinds` list filters nothing, as `glia find` with no `--kind`. `live` is
/// the graph's cached `entrypoint_reachable` set (`PyGraph::live`).
fn find_answer(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    query: &str,
    top_k: usize,
    kinds: &[String],
    scope: Option<String>,
    unparsed_files: usize,
) -> Result<Answer<FoundNode>, String> {
    let kinds = kinds_by_name(kinds)?;
    let mut opts = FindOptions::default();
    opts.top_k = top_k;
    opts.kinds = (!kinds.is_empty()).then_some(kinds);
    opts.scope = scope;
    let mut answer = find_nodes_with_live(merged, live, query, &opts);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    Ok(answer)
}

#[pymethods]
impl PyGraph {
    /// **find** (LD.3b): the ranked, located nodes whose name or qname
    /// `query` names, in one call — the one lookup (it replaced `find_node`
    /// and `find_nodes_by_qname`). Returns a dict `{results, absence}`:
    /// `results` is the records `{id, qname, name, kind, live, file, line,
    /// match}` (`live`: an entrypoint reaches the node, `false` = likely dead;
    /// `line` is 1-based; `match` names the tier that matched: `exact_qname`,
    /// `exact_name`, `exact_ci`, `qname_suffix`, `name_prefix`, `name_word`,
    /// `name_substring`, `qname_substring`, `subsequence`); `absence` is
    /// `None` when there are results, else a `no_match` dict with
    /// `unparsed_files` set to `len(parse_errors)`.
    ///
    /// Rows rank by tier, then — inside `exact_qname` / `exact_name` — by the
    /// same key the seeded answers resolve a name with (a declaration before
    /// a container, then degree): when `query` is a node's exact qname or
    /// name, `find(query, top_k=1)['results'][0]` is the node
    /// `blast_radius(query)` starts from.
    ///
    /// `top_k` keeps the first rows (0 keeps every match). `kinds` keeps only
    /// nodes of those kind names (`["FUNCTION", "CLASS"]`, any case; `None` or
    /// `[]` keeps every kind); an unknown name raises ValueError listing the
    /// valid ones. `scope` (a repo-relative path or a project label, see
    /// `project_roots`) is a FILTER applied before `top_k`: a node whose file
    /// is outside it is dropped, one with no file is kept.
    #[pyo3(signature = (query, top_k=20, kinds=None, scope=None))]
    fn find(
        &self,
        py: Python<'_>,
        query: &str,
        top_k: usize,
        kinds: Option<Vec<String>>,
        scope: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let answer = find_answer(
            &self.merged,
            self.live(),
            query,
            top_k,
            kinds.as_deref().unwrap_or_default(),
            scope,
            self.parse_errors.len(),
        )
        .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }

    /// **resolve** (P3, handoff v6): a failure/change signal (stacktrace, diff,
    /// test id, or `auto`-sniffed) → the ranked, LOCATED nodes it points at, in
    /// one call. Resolution order preserved. Returns a dict `{results,
    /// absence}` (LD.8a): `results` is the records
    /// `{id, qname, name, kind, score, live, file, line}` (`live`: an
    /// entrypoint reaches the node, `false` = likely dead; `line` is 1-based);
    /// `absence` is `None` when there are results, else the FACT-tier reason —
    /// `no_signal_match` (the signal resolved to no node; the note counts what
    /// it held) or `no_match` (`scope` or `top_k=0` removed every node) — with
    /// `unparsed_files` set to `len(parse_errors)`.
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
        py: Python<'_>,
        text: &str,
        kind: &str,
        top_k: Option<usize>,
        scope: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        let mut answer = glia_engine::resolve_signal_located_with_live(
            &self.merged,
            self.live(),
            text,
            kind,
            top_k,
            scope,
        );
        if let Some(a) = answer.absence.as_mut() {
            a.unparsed_files = self.parse_errors.len();
        }
        to_py(py, serde_json::to_string(&answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_engine::find::find_nodes;

    fn built(tag: &str) -> MergedGraph {
        let root = std::env::temp_dir().join(format!("glia-ld2-find-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("svc")).expect("temp dir");
        std::fs::write(
            root.join("app.py"),
            "class Helper:\n    pass\n\n\ndef helper(x):\n    return x + 1\n\n\ndef main():\n    return helper(2)\n",
        )
        .expect("write fixture");
        std::fs::write(root.join("svc/other.py"), "def helper_two():\n    return 2\n")
            .expect("write fixture");
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        built.expect("build").merged
    }

    /// LD.2: pyo3 `find` is the engine's `find_nodes` — the same rows, in the
    /// same order, as `glia find` — with kind NAMES validated (any case), the
    /// scope a filter, and the parse-error count on an empty answer.
    #[test]
    fn find_is_the_engine_find_with_kind_names() {
        let merged = built("kinds");
        let live = glia_engine::entrypoint_reachable(&merged);
        let all = find_answer(&merged, &live, "helper", 20, &[], None, 0).expect("answer");
        let engine = find_nodes(&merged, "helper", &FindOptions::default());
        assert_eq!(all.results, engine.results);
        assert_eq!(all.results.first().map(|r| r.qname.as_str()), Some("app::helper"));
        // LD.6: `main` calls `helper`, so both are live; the rows carry it.
        assert!(all.results.first().is_some_and(|r| r.live), "{:?}", all.results);

        let funcs = find_answer(&merged, &live, "helper", 0, &["function".into()], None, 0).expect("answer");
        assert!(!funcs.results.is_empty());
        assert!(funcs.results.iter().all(|r| r.kind == "FUNCTION"), "{:?}", funcs.results);

        let bad = find_answer(&merged, &live, "helper", 20, &["FUNCTOIN".into()], None, 0);
        let err = bad.expect_err("an unknown kind name is an error");
        assert!(err.contains("unknown kind 'FUNCTOIN'") && err.contains("FUNCTION"), "{err}");

        let scoped = find_answer(&merged, &live, "helper", 0, &[], Some("svc".into()), 0).expect("answer");
        assert!(!scoped.results.is_empty());
        assert!(
            scoped.results.iter().all(|r| r.file.as_deref().is_none_or(|f| f.starts_with("svc/"))),
            "scope is a filter: {:?}",
            scoped.results
        );

        let none = find_answer(&merged, &live, "zzqqxx", 20, &[], None, 7).expect("answer");
        assert!(none.results.is_empty());
        let absence = none.absence.expect("an empty answer carries its absence");
        assert_eq!((absence.reason, absence.unparsed_files), ("no_match", 7));
    }
}
