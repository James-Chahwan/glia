//! **check** (LE.8): the declared `[[constraint]]` rules evaluated to
//! VIOLATION with located evidence, as one native dict.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pyo3::prelude::*;

use glia_engine::check::{CheckOptions, check_with_roots};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The whole body of [`PyGraph::check`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` covers it (see the crate doc). `repo_roots` is the
/// graph's (DT.7b: each root's `.glia/overlay.toml` is bound again, as
/// `gaps` reads it).
fn check_json(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    strict: bool,
) -> Result<String, serde_json::Error> {
    let roots: Vec<(u64, PathBuf)> = repo_roots
        .iter()
        .map(|(r, p)| (*r, PathBuf::from(p)))
        .collect();
    let mut opts = CheckOptions::default();
    opts.strict = strict;
    serde_json::to_string(&check_with_roots(merged, &roots, &opts))
}

#[pymethods]
impl PyGraph {
    /// **check** (LE.8): every declared rule (`.glia/overlay.toml`
    /// `[[constraint]]` stanzas and cell-API rules, stored as CONSTRAINT
    /// cells) evaluated against this graph.
    ///
    /// Returns a dict `{rules, checked, unchecked, errors, unanchored,
    /// violations, reflexion}` (`unanchored` only when it holds a row):
    /// `rules` read, `checked` evaluated (forbid_edge + no_cycle + invariants
    /// with a `rule`, plus the allows of an evaluated model), `unchecked` the
    /// ids of the rules no query evaluates (an invariant without `rule`),
    /// `errors` `[rule_id, message]` pairs for a rule that could not be
    /// evaluated (an unknown edge category, an allow naming an undeclared
    /// component, an invariant `rule` that does not parse or analyse: `<id>
    /// (<decl>):<line>:<col>: error: ...`, line and column 1-based in the
    /// rule text).
    ///
    /// `unanchored` (DT.7b) lists the declared rules that check nothing:
    /// `{rule_id, section, decl, repo, status, reason, fix, blocking}`, first
    /// the rules whose scope no located node sits in (`status`
    /// `empty_scope`, `reason` `<role> scope `<s>` matches no located node
    /// (..)`, `repo` None), then each repo root's `.glia/overlay.toml`
    /// `[[constraint]]` / `[[component]]` / `[[layer]]` stanza that reached
    /// no CONSTRAINT cell, bound again through the build's own binder:
    /// `orphaned` (`orphaned: anchor=<qname> binds no node`, `orphaned:
    /// scope=<raw> -> <path> (no PROJECT at or above it, no MODULE under
    /// it)`, `orphaned: component=<name> (..)`, ..), `rejected` (`rejected:
    /// <why>`) or `absent` (it binds, but this graph holds no rule for it: a
    /// layout older than the overlay). A stanza the overlay loader itself
    /// dropped (a version-2 key in a version-1 file, an exact path two
    /// components claim, a layer or allow naming a dropped component, a bad
    /// kind; DT.7c) is a `rejected` row at its line, `reason` `rejected:
    /// <the loader's message> (stanza dropped)`, listed before the binder's
    /// rows of its repo; a whole file the loader ignored (a TOML or schema
    /// error, an unsupported or missing `version`) while it holds a rule
    /// stanza is one row, `rule_id` `.glia/overlay.toml`, `section`
    /// `overlay`, `reason` `rejected: <message> (rule stanzas not read:
    /// constraint=<c> component=<m> layer=<l>)` (stderr: `[check] overlay
    /// rejected root=<r> whole_file=<b> ..`). `fix` suggests what would
    /// anchor it, or what to change so the loader keeps it.
    /// `blocking` is `strict`: by default the rows never fail CI; with
    /// `strict=True` each is also an `errors` row (an empty scope's message,
    /// or `<section> <decl>: <reason> - not evaluated`, then a `fix: ..`
    /// line), as `glia check --strict` exits 2 on it.
    /// `violations`, sorted by rule id: `{rule_id, rule_kind, decl, severity,
    /// tier, blocking, count, evidence}`, plus `subject`, `file`, `line` and
    /// `caveats` when set. `rule_kind` is `forbid_edge` (a rule's direct
    /// edges from scope `from` into scope `to`: `count` every edge, evidence
    /// at most 100, strongest tier first), `no_cycle` (one cycle: `count` the
    /// nodes in its component, evidence a shortest cycle), `rule` (one
    /// `violation` row of an invariant's Datalog `rule`: `subject` the row's
    /// values rendered, a node as its qname; `file` / `line` its first
    /// node's location; `evidence` the row's witness edges, `count` all of
    /// them; `caveats` the coverage notes of the edge categories its
    /// negations read, since a blind extraction looks like a missing edge),
    /// `forbid_path` (a transitive forbid_edge hit) or `divergence` (below).
    /// `blocking` is whether the violation fails CI: its tier is at least as
    /// strong as its rule's `min_tier` (`fact` | `derived`, default
    /// `derived`) and it is not `heuristic`; a heuristic violation is
    /// reported and never blocking. Evidence rows are
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
    /// `count` and evidence as forbid_edge's, `blocking` under the default
    /// `min_tier` (the model has no per-rule gate). `glia check` renders the
    /// model as its `## reflexion model` section (CC.5c).
    #[pyo3(signature = (strict=false))]
    fn check(&self, py: Python<'_>, strict: bool) -> PyResult<Py<PyAny>> {
        to_py(py, check_json(&self.merged, &self.repo_roots, strict))
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
        let empty =
            check_json(&MergedGraph::new(Vec::new()), &BTreeMap::new(), false).expect("serialises");
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

        let json = check_json(&built.merged, &built.repo_roots, false).expect("serialises");
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
        // DB.3a: a derived violation blocks under the default min_tier; a
        // no_cycle violation has no subject, so none is serialised.
        assert_eq!(row["blocking"], true, "{json}");
        assert!(row.get("subject").is_none(), "{json}");
        for hop in row["evidence"].as_array().into_iter().flatten() {
            assert_eq!(hop["tier"], "fact", "{json}");
            assert!(hop["note"].is_null(), "{json}");
        }
        // DT.7b: every rule anchored, so no `unanchored` key.
        assert!(v.get("unanchored").is_none(), "{json}");
    }

    /// `from` copied into `to`, recursively.
    fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).expect("mkdir");
        for entry in std::fs::read_dir(from).expect("read fixture dir") {
            let entry = entry.expect("dir entry");
            let target = to.join(entry.file_name());
            if entry.file_type().expect("file type").is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).expect("copy fixture file");
            }
        }
    }

    /// DT.7b: `check` reads the graph's repo roots, so DT.7a's
    /// overlay-subdir-constraint fixture's stale-anchor stanza, which no
    /// CONSTRAINT cell holds, is an `unanchored` row that does not block;
    /// with `strict` it is an `errors` row too.
    #[test]
    fn check_lists_the_rules_that_check_nothing() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../bench/substrate-gap/fixtures/overlay-subdir-constraint");
        let root = std::env::temp_dir().join(format!("glia-dt7b-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        copy_tree(&src, &root);
        let built =
            glia_engine::generate_one(root.to_str().expect("utf-8 temp path")).expect("build");
        let lax = check_json(&built.merged, &built.repo_roots, false).expect("serialises");
        let strict = check_json(&built.merged, &built.repo_roots, true).expect("serialises");
        let _ = std::fs::remove_dir_all(&root);

        let v: serde_json::Value = serde_json::from_str(&lax).expect("valid JSON");
        assert_eq!(v["errors"], serde_json::json!([]), "{lax}");
        let rows = v["unanchored"].as_array().expect("unanchored rows");
        assert_eq!(rows.len(), 1, "{lax}");
        assert_eq!(rows[0]["rule_id"], "stale-anchor", "{lax}");
        assert_eq!(rows[0]["section"], "constraint", "{lax}");
        assert_eq!(rows[0]["decl"], ".glia/overlay.toml:20", "{lax}");
        assert_eq!(rows[0]["status"], "orphaned", "{lax}");
        assert_eq!(
            rows[0]["reason"], "orphaned: anchor=backend::internal::gone::Thing binds no node",
            "{lax}"
        );
        assert_eq!(rows[0]["blocking"], false, "{lax}");

        let v: serde_json::Value = serde_json::from_str(&strict).expect("valid JSON");
        assert_eq!(v["unanchored"][0]["blocking"], true, "{strict}");
        let errors = v["errors"].as_array().expect("errors rows");
        assert_eq!(errors.len(), 1, "{strict}");
        assert_eq!(errors[0][0], "stale-anchor", "{strict}");
        let msg = errors[0][1].as_str().unwrap_or_default();
        assert!(
            msg.starts_with(
                "constraint .glia/overlay.toml:20: orphaned: anchor=backend::internal::gone::Thing binds no node - not evaluated\nfix: "
            ),
            "{msg}"
        );
    }
}
