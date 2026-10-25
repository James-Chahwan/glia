//! **health** (FH.2b): pyo3 surface for `glia_engine::health` (FH.2a) —
//! `PyGraph.health()`, the `glia health` report as a native dict (LD.2): per
//! boundary mechanism how much of the client and server side the graph
//! pairs, with a verdict, then parse coverage, unresolved workspace imports,
//! suspicious endpoint / route qnames and the dead-symbol share. A
//! `#[pymethods]` block only, no `#[pyfunction]`, so no inventory
//! registration.
//!
//! The body is the pyo3-free [`health_report`], so `cargo test -p glia-py`
//! covers it; the engine prints its `[health] repo=<label> .. surface=py`
//! fired_on line once per call.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pyo3::prelude::*;

use glia_engine::health::{Health, HealthOptions, health_with};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The whole body of [`PyGraph::health`], minus pyo3: the engine answer over
/// the graph's repo roots (walked for the files no parser takes) and labels,
/// marker surface `py`. Returns the engine struct itself, so `to_py` keeps
/// its field order (`convert.rs`).
fn health_report(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    repo_labels: &BTreeMap<u64, String>,
) -> Health {
    let roots: Vec<(u64, PathBuf)> = repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect();
    let mut opts = HealthOptions::default();
    opts.labels = repo_labels.clone();
    opts.surface = "py";
    health_with(merged, &roots, &opts)
}

#[pymethods]
impl PyGraph {
    /// **health** (FH.2a / FH.2b): whether answers about this graph can be
    /// trusted, before asking one. Reads the built graph plus one gated walk
    /// of each repo root (the files no parser takes); writes nothing.
    ///
    /// Returns a dict `{repos, role, role_basis, verdict, reason,
    /// mechanisms, parse, workspace_unresolved, suspicious, dead}`: `role`
    /// `"app"` or `"library"` (a library is never judged on reach),
    /// `verdict` the worst row's (`"OK"`, `"DEGRADED"` or `"BLIND"`) and
    /// `reason` `<row>: <reason> (<measure>)`. Each of `mechanisms` (http,
    /// grpc, rpc, graphql, queue, ws, in that order) is `{mechanism,
    /// producers, external_producers, consumers, pairs, paired_producers,
    /// reached_consumers, client_reach, server_reach, verdict, reason, reach,
    /// measure}`, a reach `None` when its side is empty. `parse` is
    /// `{parsed_files, unparsed, docs_unparsed, unparsed_files,
    /// docs_unparsed_files, verdict, reason, measure}` (the two maps by
    /// extension; prose is never a flag); each `workspace_unresolved` row
    /// `{package, ecosystem, path, importers}`; `suspicious` `{count,
    /// examples}`; `dead` `{dead_symbols, symbols, share}`.
    fn health(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let h = health_report(&self.merged, &self.repo_roots, &self.repo_labels);
        to_py(py, serde_json::to_string(&h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use glia_engine::generate_one;
    use glia_engine::health::{Role, Verdict};

    /// pyo3 `health` is the engine answer over the build's roots and labels,
    /// field for field: a TypeScript library with one unparsed `.marko`.
    #[test]
    fn report_matches_engine() {
        let dir = std::env::temp_dir().join(format!("glia-py-health-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("mkdir");
        std::fs::create_dir_all(dir.join("ui")).expect("mkdir");
        std::fs::write(
            dir.join("src/index.ts"),
            "export function pad(s: string): string {\n  return ' ' + s;\n}\n",
        )
        .expect("write");
        std::fs::write(dir.join("ui/App.marko"), "<p>x</p>\n").expect("write");
        let r = generate_one(dir.to_str().expect("utf-8 path")).expect("builds");
        let got = health_report(&r.merged, &r.repo_roots, &r.repo_labels);
        let roots: Vec<(u64, PathBuf)> = r
            .repo_roots
            .iter()
            .map(|(id, p)| (*id, PathBuf::from(p)))
            .collect();
        let mut opts = HealthOptions::default();
        opts.labels = r.repo_labels.clone();
        let want = health_with(&r.merged, &roots, &opts);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, want);
        assert_eq!((got.role, got.verdict), (Role::Library, Verdict::Ok));
        assert_eq!(got.repos.len(), 1, "{:?}", got.repos);
        assert_eq!(got.parse.unparsed.get("marko"), Some(&1), "{:?}", got.parse);
        assert_eq!(got.mechanisms.len(), 6);
    }
}
