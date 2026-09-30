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
    /// Returns a dict `{rules, checked, unchecked, errors, violations,
    /// reflexion}`:
    /// `rules` read, `checked` evaluated (forbid_edge + no_cycle, plus the
    /// allows of an evaluated model), `unchecked` the ids of the rules no
    /// query evaluates (invariant), `errors` `[rule_id, message]` pairs for
    /// a rule that could not be evaluated (an unknown edge category, a scope
    /// no node sits in, an allow naming an undeclared component).
    /// `violations`, sorted by rule id: `{rule_id, rule_kind, decl, severity,
    /// tier, count, evidence}` — a forbid_edge rule's direct edges from scope
    /// `from` into scope `to` (`count` every edge, evidence at most 100,
    /// strongest tier first), or one no_cycle cycle (`count` the nodes in its
    /// component, evidence a shortest cycle). Evidence rows are
    /// `{from_qname, to_qname, category, file, line, emitter, tier, note}`
    /// with 1-based lines; `tier` is the one `why()` gives that edge (`fact`
    /// read at a site, `derived` paired by a resolver or pass or inferred
    /// below strong confidence, `heuristic` declared, co-changed or guessed
    /// by name) and `note` says why when it is not the stage's plain tier.
    /// A forbid_edge violation's `tier` is its strongest row's; a no_cycle
    /// violation is `derived` unless a hop is `heuristic`. A node is in a
    /// scope only when its file sits under that path (or it is a PROJECT
    /// there): a node with no file never matches.
    ///
    /// `reflexion` (CC.5b) is `None` unless the overlay declares a reflexion
    /// model (`[[component]]`, `[[layer]]`, `kind = "allow"`); components and
    /// layers are declarations, not rules, and an allow is a checked rule.
    /// Then it is `{closed, components, matrix, absences, unmapped,
    /// convergences, divergences}`: `components` `{name, paths, layer, nodes,
    /// decl}` by name (a file belongs to the component with the longest path
    /// above it); `matrix` one `{from, to, edges, status, tier, allowed_by}`
    /// per dependent component pair, `status` `convergence` / `divergence`,
    /// or `observed` in an open model (components only), `allowed_by` an
    /// allow id or `layer:<upper>><lower>`; `absences` `{from, to, rule_id,
    /// decl, tier, caveats}` for an allow the code never realises (`tier`
    /// `fact`, `caveats` the coverage notes of the checked categories: a
    /// blind extraction looks like an absence); `unmapped` `{files, nodes,
    /// edges_to_mapped, sample}` for the code no component owns. Each
    /// divergence is also a violation, `rule_id` `reflexion:<from>-><to>`,
    /// `rule_kind` `divergence`, `decl` the `from` component's stanza,
    /// `count` and evidence as forbid_edge's. `glia check` renders the model
    /// as its `## reflexion model` section (CC.5c).
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
            r#"{"rules":0,"checked":0,"unchecked":[],"errors":[],"violations":[],"reflexion":null}"#
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
        // CC.5b: no component declared, so no reflexion model.
        assert!(v["reflexion"].is_null(), "{json}");
        assert_eq!(row["evidence"][0]["line"], 1, "{json}");
        // CC.3: every row carries why's tier and note; two observed imports
        // are facts, and the computed cycle is derived.
        assert_eq!(row["tier"], "derived", "{json}");
        for hop in row["evidence"].as_array().into_iter().flatten() {
            assert_eq!(hop["tier"], "fact", "{json}");
            assert!(hop["note"].is_null(), "{json}");
        }
    }
}
