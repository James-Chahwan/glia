//! **spec_status** (LE.9b): the SDD `spec_status` answer — per feature, which
//! declared API ops are implemented and which are missing, and which routes of
//! governed services nobody declared.

use std::collections::BTreeMap;

use pyo3::prelude::*;

use repo_graph_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The whole body of [`PyGraph::spec_status`], minus pyo3 — kept pyo3-free so
/// `cargo test -p repo-graph-py` can cover the binding (see the crate doc).
fn spec_status_json(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    feature: Option<&str>,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&repo_graph_engine::spec_status::spec_status(
        merged,
        repo_labels,
        feature,
    ))
}

#[pymethods]
impl PyGraph {
    /// **spec_status** (LE.9b): declared API ops against the routes that
    /// implement them, as one dict `{rows, by_feature, governed_services,
    /// ungoverned_routes}`.
    ///
    /// `rows`: `{feature, source, method, path, status, confidence, pairing,
    /// route, handler, decl}` — `status` is `implemented` (one row per
    /// (op, route)), `declared_missing` (an OpenAPI / quokka feature.yaml op
    /// no route implements) or `undeclared` (a server route of a governed
    /// service no op declares; `feature` is `None`). `route` / `handler` /
    /// `decl` are `{id, name, qname, kind, file, line}` with 1-based lines, or
    /// `None`. `by_feature`: `{feature: {declared, implemented,
    /// declared_missing}}`. `governed_services`: the services implementing at
    /// least one declared op — only their routes can be undeclared; every
    /// other server route is only counted in `ungoverned_routes`.
    ///
    /// `feature=` keeps that feature's rows and drops the undeclared ones.
    /// Pact interactions and handler-annotation ops are not declarations.
    /// Report-only: nothing is added to the graph.
    #[pyo3(signature = (feature=None))]
    fn spec_status(&self, py: Python<'_>, feature: Option<&str>) -> PyResult<Py<PyAny>> {
        to_py(
            py,
            spec_status_json(&self.merged, &self.repo_labels, feature),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LE.9b: `spec_status()` is transport only — pin the wiring (the
    /// documented object shape, and a real build's rows reaching Python with
    /// the feature filter applied). The status logic is covered by the
    /// engine's `spec_status` test.
    #[test]
    fn spec_status_returns_the_documented_json() {
        let empty = spec_status_json(&MergedGraph::new(Vec::new()), &BTreeMap::new(), None)
            .expect("serialises");
        assert_eq!(
            empty,
            r#"{"rows":[],"by_feature":{},"governed_services":[],"ungoverned_routes":0}"#
        );

        let root = std::env::temp_dir().join(format!("glia-le9b-{}", std::process::id()));
        let root = root.as_path();
        std::fs::create_dir_all(root.join("features/orders")).expect("mkdir");
        std::fs::create_dir_all(root.join("app")).expect("mkdir");
        std::fs::write(
            root.join("features/orders/feature.yaml"),
            "name: Orders\nbackend_routes:\n  - GET /orders\n  - POST /orders\n",
        )
        .expect("write feature");
        std::fs::write(
            root.join("app/main.py"),
            "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
             @app.get(\"/orders\")\ndef list_orders():\n    return []\n\n\n\
             @app.get(\"/health\")\ndef health():\n    return {}\n",
        )
        .expect("write app");
        let built = repo_graph_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(root);
        let built = built.expect("build");

        let json = spec_status_json(&built.merged, &built.repo_labels, None).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let rows = v["rows"].as_array().expect("rows is an array");
        let got: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| {
                (
                    r["status"].as_str().unwrap_or(""),
                    r["path"].as_str().unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("implemented", "/orders"),
                ("declared_missing", "/orders"),
                ("undeclared", "/health")
            ],
            "{json}"
        );
        assert_eq!(rows[0]["feature"], "orders", "{json}");
        assert!(
            rows[0]["route"]["line"].is_u64(),
            "a 1-based int line: {json}"
        );
        assert_eq!(v["by_feature"]["orders"]["declared"], 2, "{json}");

        let only =
            spec_status_json(&built.merged, &built.repo_labels, Some("nope")).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&only).expect("valid JSON");
        assert!(v["rows"].as_array().is_some_and(Vec::is_empty), "{only}");
    }
}
