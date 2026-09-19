//! **check** (LE.8): the declared `[[constraint]]` rules evaluated to
//! VIOLATION with located evidence, as one native dict.

use pyo3::prelude::*;

use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The whole body of [`PyGraph::check`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` covers it (see the crate doc).
fn check_json(merged: &MergedGraph) -> Result<String, serde_json::Error> {
    serde_json::to_string(&glia_engine::check::check(merged))
}

#[pymethods]
impl PyGraph {
    /// **check** (LE.8): every declared rule (`.glia/overlay.toml`
    /// `[[constraint]]` stanzas and cell-API rules, stored as CONSTRAINT
    /// cells) evaluated against this graph.
    ///
    /// Returns a dict `{rules, checked, unchecked, errors, violations}`:
    /// `rules` read, `checked` evaluated (forbid_edge + no_cycle),
    /// `unchecked` the ids of the rules no query evaluates (invariant),
    /// `errors` `[rule_id, message]` pairs for a rule that could not be
    /// evaluated (an unknown edge category, a scope no node sits in).
    /// `violations`, sorted by rule id: `{rule_id, rule_kind, decl, severity,
    /// tier, count, evidence}` — a forbid_edge rule's direct edges from scope
    /// `from` into scope `to` (tier `fact`, `count` every edge, evidence at
    /// most 100), or one no_cycle cycle (tier `derived`, `count` the nodes in
    /// its component, evidence a shortest cycle). Evidence rows are
    /// `{from_qname, to_qname, category, file, line, emitter}` with 1-based
    /// lines. A node is in a scope only when its file sits under that path
    /// (or it is a PROJECT there): a node with no file never matches.
    fn check(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        to_py(py, check_json(&self.merged))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LE.8: `check()` is transport only — pin the documented object shape
    /// on an empty graph and a real build's violation reaching the JSON. The
    /// rule evaluation is covered by `engine/tests/check.rs`.
    #[test]
    fn check_returns_the_engine_report() {
        let empty = check_json(&MergedGraph::new(Vec::new())).expect("serialises");
        assert_eq!(
            empty,
            r#"{"rules":0,"checked":0,"unchecked":[],"errors":[],"violations":[]}"#
        );

        let root = std::env::temp_dir().join(format!("glia-le8-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, src) in [
            ("pkg/pyproject.toml", "[project]\nname = \"pkg\"\n"),
            (
                "pkg/a.py",
                "from pkg.b import f\n\n\ndef g():\n    return 1\n",
            ),
            (
                "pkg/b.py",
                "from pkg.a import g\n\n\ndef f():\n    return 2\n",
            ),
            (
                ".glia/overlay.toml",
                "version = 1\n\n[[constraint]]\nid = \"pkg-acyclic\"\nkind = \"no_cycle\"\nscope = \"pkg\"\n",
            ),
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
            std::fs::write(p, src).expect("write fixture");
        }
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");

        let json = check_json(&built.merged).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["rules"], 1, "{json}");
        assert_eq!(v["checked"], 1, "{json}");
        let row = &v["violations"][0];
        assert_eq!(row["rule_id"], "pkg-acyclic", "{json}");
        assert_eq!(row["rule_kind"], "no_cycle", "{json}");
        assert_eq!(row["severity"], "VIOLATION", "{json}");
        assert_eq!(row["decl"], ".glia/overlay.toml:3", "{json}");
        assert_eq!(row["evidence"].as_array().map(Vec::len), Some(2), "{json}");
        assert_eq!(row["evidence"][0]["line"], 1, "{json}");
    }
}
