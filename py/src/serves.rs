//! **serves** (LD.8b): who serves a channel — an HTTP `METHOD /path` through
//! the HTTP resolver's route matcher, or a queue topic — as the LD.8a
//! envelope `{results, absence}` in a native `dict`.

use std::collections::HashSet;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_core::NodeId;
use glia_engine::absence::Answer;
use glia_engine::serves::{Server, serves_with_live};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The whole body of [`PyGraph::serves`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` covers it (see the crate doc). `live` is the
/// graph's cached `entrypoint_reachable` set (`PyGraph::live`).
fn serves_answer(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    channel: &str,
    mechanism: &str,
    unparsed_files: usize,
) -> Result<Answer<Server>, String> {
    let mut answer = serves_with_live(merged, live, channel, mechanism)?;
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    Ok(answer)
}

#[pymethods]
impl PyGraph {
    /// **serves** (LD.8b): who serves `channel`, in one call. `channel` is
    /// `"METHOD /path"` (`"POST /orders"`), a bare `"/path"` (looked up under
    /// GET, POST, PUT, PATCH and DELETE) or a queue topic (`"orders.created"`).
    /// `mechanism` is `"auto"` (a leading HTTP verb or `/` means HTTP,
    /// anything else a topic), `"http"` or `"queue"`; any other value raises
    /// ValueError.
    ///
    /// Returns a dict `{results, absence}`: `results` is the servers
    /// `{id, qname, kind, file, line, live, match, confidence, handlers}` —
    /// `match` is the HTTP matcher tier that reached the route (`exact`,
    /// `endpoint_prefix`, `any` = only the method-agnostic fallback,
    /// `route_prefix`; `exact` for a queue consumer), `handlers` the
    /// HANDLED_BY targets `{id, name, qname, kind, file, line}` in edge order,
    /// lines 1-based. `absence` is `None` when something serves the channel,
    /// else the FACT-tier `unserved_channel` dict (a refused framework tag is
    /// `no_match`) with the mechanism's caveat rows, up to five near misses
    /// in `suggestions` (the route under another verb or the parent path's;
    /// the topic's producers or a look-alike topic) and `unparsed_files` set
    /// to `len(parse_errors)`.
    #[pyo3(signature = (channel, mechanism="auto"))]
    fn serves(&self, py: Python<'_>, channel: &str, mechanism: &str) -> PyResult<Py<PyAny>> {
        let answer = serves_answer(
            &self.merged,
            self.live(),
            channel,
            mechanism,
            self.parse_errors.len(),
        )
        .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LD.8b: pyo3 `serves` is the engine's `serves` over the cached live set,
    /// with the parse-error count on an absence and an unknown mechanism an
    /// error. The matching itself is covered by `engine/tests/serves.rs`.
    #[test]
    fn serves_is_the_engine_serves() {
        let root = std::env::temp_dir().join(format!("glia-ld8b-serves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        std::fs::write(
            root.join("app.py"),
            "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
             @app.route('/orders', methods=['POST'])\ndef create_order():\n    return {}\n",
        )
        .expect("write fixture");
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let merged = built.expect("build").merged;
        let live = glia_engine::entrypoint_reachable(&merged);

        let hit = serves_answer(&merged, &live, "POST /orders", "auto", 3).expect("answer");
        assert!(hit.absence.is_none());
        let json = serde_json::to_string(&hit).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let row = &v["results"][0];
        assert_eq!(row["match"], "exact", "{json}");
        assert_eq!(row["handlers"][0]["qname"], "app::create_order", "{json}");
        assert_eq!(row["live"], true, "{json}");

        let none = serves_answer(&merged, &live, "DELETE /orders", "http", 3).expect("answer");
        let absence = none.absence.expect("nothing takes DELETE");
        assert_eq!(
            (absence.reason, absence.unparsed_files),
            ("unserved_channel", 3)
        );

        let err =
            serves_answer(&merged, &live, "orders", "smtp", 0).expect_err("unknown mechanism");
        assert!(err.contains("smtp"), "{err}");
    }
}
