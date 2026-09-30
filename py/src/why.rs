//! **why** (LE.5): every edge from one node to another with the extractor or
//! resolver that emitted it, its call site and confidence, tiered fact /
//! derived / heuristic; with no direct edge the witness path and the LD.8a
//! absence — the engine's `WhyAnswer` as a native `dict`.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::why::{WhyAnswer, why_edge};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The whole body of [`PyGraph::why`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` covers it (see the crate doc). An absence
/// counts the build's unparsed files.
fn why_answer(
    merged: &MergedGraph,
    from_qname: &str,
    to_qname: &str,
    category: Option<&str>,
    unparsed_files: usize,
) -> Result<WhyAnswer, String> {
    let mut answer = why_edge(merged, from_qname, to_qname, category)?;
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    Ok(answer)
}

#[pymethods]
impl PyGraph {
    /// **why** (LE.5): every edge from `from_qname` to `to_qname` (each a
    /// qname, a dotted path or an exact simple name — every node it names, up
    /// to 8), of `category` when given (an edge-category name, any case).
    ///
    /// Returns a dict `{found, edges, path, from_nodes, to_nodes, note,
    /// absence}`. `edges` is one row per edge `{from_id, from_qname, to_id,
    /// to_qname, category, confidence, tier, emitter, rule, basis, site,
    /// cross_repo, note}`: `emitter` / `rule` / `basis` from the edge's
    /// EVIDENCE cell, `site` `{file, line}` (1-based) where it was asserted,
    /// `tier` `fact` (read at a site, or bound or confirmed by a SCIP index)
    /// / `derived` (paired by a resolver or pass; also any edge without
    /// evidence, or a graph edge inferred below strong confidence, noted
    /// `inferred binding (..)`) / `heuristic` (a name-only guess, an overlay
    /// declaration, git co-change). When `found` is False,
    /// `path` is a shortest carry path explained hop by hop (empty when none
    /// is within 6 hops) and `absence` the FACT-tier `no_edges` dict with
    /// `unparsed_files` set. An unknown node or category raises ValueError.
    #[pyo3(signature = (from_qname, to_qname, category=None))]
    fn why(
        &self,
        py: Python<'_>,
        from_qname: &str,
        to_qname: &str,
        category: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        let answer = why_answer(
            &self.merged,
            from_qname,
            to_qname,
            category,
            self.parse_errors.len(),
        )
        .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LE.5: pyo3 `why` is the engine's answer with the parse-error count on
    /// an absence and engine errors passed through. The tiers and the path
    /// are covered by `engine/tests/why.rs`.
    #[test]
    fn why_is_the_engine_answer() {
        let root = std::env::temp_dir().join(format!("glia-le5-why-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("shop")).expect("temp dir");
        std::fs::write(
            root.join("shop").join("a.py"),
            "def price(o):\n    return o\n\n\ndef place(o):\n    return price(o)\n\n\n\
             def checkout(o):\n    return place(o)\n",
        )
        .expect("write fixture");
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let merged = built.expect("build").merged;

        let hit = why_answer(&merged, "shop::a::place", "shop::a::price", None, 3).expect("answer");
        let json = serde_json::to_string(&hit).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["found"], true, "{json}");
        assert_eq!(v["edges"][0]["category"], "CALLS", "{json}");
        assert_eq!(v["edges"][0]["tier"], "fact", "{json}");
        assert_eq!(v["edges"][0]["site"]["file"], "shop/a.py", "{json}");
        assert_eq!(v["edges"][0]["site"]["line"], 6, "{json}");
        assert!(v["absence"].is_null(), "{json}");

        let miss =
            why_answer(&merged, "shop::a::checkout", "shop::a::price", None, 3).expect("answer");
        assert!(!miss.found && miss.path.len() == 2, "{miss:?}");
        let absence = miss.absence.expect("no direct edge");
        assert_eq!((absence.reason, absence.unparsed_files), ("no_edges", 3));

        let err = why_answer(&merged, "shop::a::place", "shop::a::price", Some("CALS"), 0)
            .expect_err("unknown category");
        assert!(err.starts_with("unknown edge category `CALS`"), "{err}");
        let err =
            why_answer(&merged, "nowhere", "shop::a::price", None, 0).expect_err("unknown node");
        assert!(
            err.starts_with("no node with qname/name `nowhere`"),
            "{err}"
        );
    }
}
