//! **gaps** (LF.2c): `PyGraph.gaps`, the ranked blind-spot report an overlay
//! agent works from, and the module function `overlay_delta`, the measurement
//! that decides whether an overlay edit is kept. Both are transport: the
//! categories, ranking and markers live in `glia_engine::gaps`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::gaps::{GapsOptions, GapsReport, OverlayDelta, gaps_report, overlay_delta};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// The report behind [`PyGraph::gaps`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` can cover the binding (see the crate doc).
/// `Err` names an unknown category.
fn gaps_of(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    top_k: Option<usize>,
    category: Option<&str>,
) -> Result<GapsReport, String> {
    let roots: Vec<(u64, PathBuf)> = repo_roots
        .iter()
        .map(|(r, p)| (*r, PathBuf::from(p)))
        .collect();
    let mut opts = GapsOptions::default();
    opts.top_k_per_category = top_k;
    opts.category = category.map(str::to_string);
    opts.surface = "py";
    gaps_report(merged, &roots, &opts)
}

#[pymethods]
impl PyGraph {
    /// **gaps** (LF.2c): the ranked blind-spot report, as one dict
    /// `{counts, skipped, rows}` — the prompt material of an overlay agent.
    ///
    /// `rows`: `{id, category, qname, kind, file, line, detail, suggest,
    /// tier}`, `line` 1-based, sorted by (category, file, line, qname). `id`
    /// (CE.3a) is `gap:<16 hex>`, unique in the report and stable across
    /// rebuilds: keyed by the node / stanza / sidecar row, never by a line or
    /// an ordinal, so a stanza can name the gap it targets. Categories, in
    /// order: `unpaired_endpoint`, `ambiguous_endpoint`,
    /// `unresolved_endpoint`, `wrapped_sink` (an `<unresolved>` sink owned by
    /// a declared `[[wrapper]]`: informational), `unpaired_route`,
    /// `tag_only_queue`, `dead_symbol`, `cochange_no_edge` (LF.5c: a file
    /// pair git history says changes together with no static link; heuristic),
    /// then the overlay's own rot —
    /// `orphaned_rule`, `redundant_rule`, `orphaned_cell`. `wrapped_sink` and
    /// the last three read each repo's files through `repo_roots` (listed in
    /// `skipped` for a graph that has none).
    /// `suggest` names the overlay section that could repair the row;
    /// `tier` is `fact` or `heuristic`. `counts`: the total per computed
    /// category, before `top_k`.
    ///
    /// `top_k` keeps at most that many rows per category; `category` keeps
    /// one category (an unknown one raises `ValueError`). Read-only.
    #[pyo3(signature = (top_k=None, category=None))]
    fn gaps(
        &self,
        py: Python<'_>,
        top_k: Option<usize>,
        category: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        let report = gaps_of(&self.merged, &self.repo_roots, top_k, category)
            .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&report))
    }
}

/// The measurement behind [`overlay_delta_py`], minus pyo3.
fn delta_of(repo_paths: &[String], incremental: bool) -> Result<OverlayDelta, String> {
    overlay_delta(repo_paths, incremental)
}

/// **overlay_delta** (LF.2c, CE.3a): build `repo_paths` without and then
/// with their `.glia/overlay.toml` overlay sections and return what the
/// overlay changed, as one dict `{rules, edges_without, edges_with,
/// added_by_category, orphans_without, orphans_with, without, with,
/// nodes_added_by_kind, verdict}`. Orphans are unpaired + unresolved
/// endpoints + tag-only queue nodes. `without` / `with` are each build's
/// counts `{nodes_by_kind, edges_by_category, gaps_by_category}` (the gaps
/// of the categories that need no repo root); `nodes_added_by_kind` and
/// `added_by_category` are the non-zero per-kind / per-category deltas.
/// `verdict` is `keep` (some gap category fell or some node kind or edge
/// category other than DEFINES / CONTAINS rose, and no gap category rose),
/// `review` (improved, but a gap category rose too) or `drop` (not
/// improved). Two builds, by design; neither is persisted. One path builds
/// like `generate`, several like `generate_many`. Prints
/// `[overlay] N rules, +M edges, orphans K→J, gaps G0→G1, verdict=<v>`.
#[pyfunction]
#[pyo3(name = "overlay_delta", signature = (repo_paths, incremental=false))]
fn overlay_delta_py(
    py: Python<'_>,
    repo_paths: Vec<String>,
    incremental: bool,
) -> PyResult<Py<PyAny>> {
    let delta = delta_of(&repo_paths, incremental).map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&delta))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(overlay_delta_py, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "gaps", add: register } }

#[cfg(test)]
mod tests {
    use super::*;

    /// LF.2c: `gaps()` and `overlay_delta()` are transport only — pin the
    /// wiring (the documented object shape, a real build's rows reaching the
    /// JSON with the options applied, an unknown category refused). The
    /// categories are covered by engine/tests/gaps_report.rs.
    #[test]
    fn gaps_and_overlay_delta_return_the_documented_json() {
        let empty =
            gaps_of(&MergedGraph::new(Vec::new()), &BTreeMap::new(), None, None).expect("report");
        assert_eq!(
            serde_json::to_string(&empty).expect("json"),
            r#"{"counts":{"ambiguous_endpoint":0,"cochange_no_edge":0,"dead_symbol":0,"tag_only_queue":0,"unpaired_endpoint":0,"unpaired_route":0,"unresolved_endpoint":0},"skipped":["wrapped_sink","orphaned_rule","redundant_rule","orphaned_cell"],"rows":[]}"#
        );
        assert!(
            gaps_of(
                &MergedGraph::new(Vec::new()),
                &BTreeMap::new(),
                None,
                Some("nope")
            )
            .is_err()
        );

        let root = std::env::temp_dir().join(format!("glia-lf2c-py-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (web, api) = (root.join("web"), root.join("api"));
        std::fs::create_dir_all(web.join("src")).expect("mkdir");
        std::fs::create_dir_all(web.join(".glia")).expect("mkdir");
        std::fs::create_dir_all(&api).expect("mkdir");
        std::fs::write(
            web.join("src/client.ts"),
            "export function request(method: string, path: string) {\n  return fetch(path, { method });\n}\n\nexport async function loadUsers() {\n  return request('GET', '/users');\n}\n",
        )
        .expect("write");
        std::fs::write(
            api.join("app.py"),
            "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\", methods=[\"GET\"])\ndef list_users():\n    return []\n\n\n@app.route(\"/orders\", methods=[\"GET\"])\ndef list_orders():\n    return []\n",
        )
        .expect("write");
        std::fs::write(
            web.join(".glia/overlay.toml"),
            "version = 1\n\n[[edge]]\nfrom = \"endpoint:GET:<unresolved>\"\nto = \"GET /users\"\ncategory = \"HTTP_CALLS\"\n",
        )
        .expect("write");
        let paths = [web, api].map(|p| p.to_string_lossy().into_owned());
        let built = glia_engine::generate_many_opts(
            &paths,
            false,
            &glia_engine::BuildOptions::default().with_overlay(false),
        );
        let delta = delta_of(&paths, false);
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");

        let rep = gaps_of(
            &built.merged,
            &built.repo_roots,
            Some(1),
            Some("unpaired_route"),
        )
        .expect("report");
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&rep).expect("json")).expect("json");
        let rows = v["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 1, "{v}");
        assert!(
            rows[0]["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("gap:")),
            "{v}"
        );
        let json = serde_json::to_string(&rep).expect("json");
        assert!(
            json.contains("\"rows\":[{\"id\":\"gap:"),
            "id is a row's first key: {json}"
        );
        assert_eq!(rows[0]["qname"], "GET /users", "{v}");
        assert!(rows[0]["line"].is_i64(), "a 1-based int line: {v}");
        assert_eq!(v["counts"]["unpaired_route"], 2, "{v}");
        assert_eq!(v["counts"]["unresolved_endpoint"], 1, "{v}");
        assert!(
            v["skipped"].as_array().is_some_and(Vec::is_empty),
            "roots given: {v}"
        );

        let delta = serde_json::to_string(&delta.expect("both builds")).expect("json");
        let d: serde_json::Value = serde_json::from_str(&delta).expect("json");
        let keys: Vec<&str> = d
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        let order = [
            "{\"rules\":",
            ",\"edges_without\":",
            ",\"edges_with\":",
            ",\"added_by_category\":",
            ",\"orphans_without\":",
            ",\"orphans_with\":",
            ",\"without\":",
            ",\"with\":",
            ",\"nodes_added_by_kind\":",
            ",\"verdict\":",
        ];
        // The top-level keys in field order: each after the previous one.
        let mut at = 0;
        for k in order {
            let found = delta[at..].find(k).map(|i| at + i);
            assert!(found.is_some(), "{k} after byte {at}: {delta}");
            at = found.unwrap_or(at) + k.len();
        }
        assert_eq!(keys.len(), 10, "{delta}");
        assert_eq!(d["verdict"], "keep", "{delta}");
        assert_eq!(
            (d["orphans_without"].as_u64(), d["orphans_with"].as_u64()),
            (Some(1), Some(0)),
            "{delta}"
        );
        assert_eq!(d["added_by_category"]["HTTP_CALLS"], 1, "{delta}");
    }
}
