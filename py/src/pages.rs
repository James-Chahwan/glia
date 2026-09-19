//! **pages** (LA.6e): the `page_flow` answer — client-router pages, the links
//! between them, dead deep links and unlinked pages.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_graph::MergedGraph;

use crate::graph::PyGraph;

/// The whole body of [`PyGraph::page_flow`], minus pyo3 — kept pyo3-free so
/// `cargo test -p repo-graph-py` can cover the binding (see the crate doc).
fn page_flow_json(merged: &MergedGraph) -> Result<String, serde_json::Error> {
    // `graphs` is one entry per (repo, language), so count distinct repos.
    let repos: std::collections::BTreeSet<u64> = merged.graphs.iter().map(|g| g.repo.0).collect();
    let flow = repo_graph_engine::pages::page_flow(merged);
    eprintln!("{}", flow.marker("pyo3", repos.len()));
    serde_json::to_string(&flow)
}

#[pymethods]
impl PyGraph {
    /// **page_flow** (LA.6e): the frontend's page flow as one JSON object
    /// `{pages, links, dead, unlinked}`. `pages`: `{path, route_qname,
    /// handler, handler_file, handler_line, redirect_to, catchall,
    /// inbound_links}` per client-router route. `links`: `{from_qname,
    /// from_file, from_line, to_path, confidence}` per resolved navigation.
    /// `dead`: `{from_qname, from_file, from_line, link, absorbed_by}` per
    /// router link no route serves (`absorbed_by` names the catch-all and its
    /// redirect, e.g. `"/** -> /login"`). `unlinked`: paths no in-repo link or
    /// redirect reaches (a fact, not a dead page). Lines are 1-based.
    /// Report-only: nothing is added to the graph.
    fn page_flow(&self) -> PyResult<String> {
        page_flow_json(&self.merged).map_err(|e| PyValueError::new_err(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LA.6e: `page_flow()` is transport only — pin the wiring (the documented
    /// object shape, and a real build's dead link reaching Python). The page
    /// logic is covered by the engine's `pages` tests.
    #[test]
    fn page_flow_returns_the_documented_json() {
        let empty = page_flow_json(&MergedGraph::new(Vec::new())).expect("serialises");
        assert_eq!(empty, r#"{"pages":[],"links":[],"dead":[],"unlinked":[]}"#);

        let root = std::env::temp_dir().join(format!("glia-la6e-{}", std::process::id()));
        let app = root.join("src/app");
        std::fs::create_dir_all(&app).expect("temp dir");
        std::fs::write(
            app.join("app.routes.ts"),
            "import { Routes } from '@angular/router';\n\
             import { HomeComponent } from './home.component';\n\n\
             export const routes: Routes = [\n  { path: 'home', component: HomeComponent },\n  \
             { path: '**', redirectTo: '/home' },\n];\n",
        )
        .expect("write routes");
        std::fs::write(
            app.join("home.component.ts"),
            "import { Component } from '@angular/core';\nimport { Router } from '@angular/router';\n\n\
             @Component({ selector: 'app-home', template: '<p>home</p>' })\n\
             export class HomeComponent {\n  constructor(private router: Router) {}\n  \
             go() {\n    this.router.navigateByUrl('/gone');\n  }\n}\n",
        )
        .expect("write component");
        let built = repo_graph_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let merged = built.expect("build").merged;

        let json = page_flow_json(&merged).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let obj = v.as_object().expect("a JSON object, not an array");
        for key in ["pages", "links", "dead", "unlinked"] {
            assert!(obj[key].is_array(), "`{key}` is an array: {json}");
        }
        assert_eq!(obj.len(), 4, "{json}");
        let dead = obj["dead"].as_array().expect("array");
        assert_eq!(dead.len(), 1, "{json}");
        assert_eq!(dead[0]["link"], "/gone", "{json}");
        assert_eq!(dead[0]["absorbed_by"], "/** -> /home", "{json}");
        assert!(
            obj["pages"]
                .as_array()
                .expect("array")
                .iter()
                .any(|p| p["path"] == "/home" && p["handler_file"].is_string()),
            "{json}"
        );
    }
}
