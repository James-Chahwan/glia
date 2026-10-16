//! **serves** (LD.8b): who serves a channel — an HTTP `METHOD /path` through
//! the HTTP resolver's route matcher, or a queue topic — as the LD.8a
//! envelope `{results, absence}` in a native `dict`. DD.13: `collapse=True`
//! folds identity classes among servers and their handlers.

use std::collections::HashSet;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_core::NodeId;
use glia_engine::absence::Answer;
use glia_engine::identity::Collapse;
use glia_engine::serves::{Server, serves_with};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The whole body of [`PyGraph::serves`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` covers it (see the crate doc). `live` is the
/// graph's cached `entrypoint_reachable` set (`PyGraph::live`); `collapse`
/// is `Collapse::with_labels(&PyGraph::repo_labels)` or `Collapse::off()`
/// (DD.13).
fn serves_answer(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    channel: &str,
    mechanism: &str,
    collapse: &Collapse,
    unparsed_files: usize,
) -> Result<Answer<Server>, String> {
    let mut answer = serves_with(merged, live, channel, mechanism, collapse)?;
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
    /// `{id, qname, kind, file, line, live, match, confidence, handlers,
    /// external_hosts}` — `match` is the HTTP matcher tier that reached the
    /// route (`exact`, `endpoint_prefix`, `any` = only the method-agnostic
    /// fallback, `route_prefix`; `exact` for a queue consumer), or `external`
    /// for a client ENDPOINT (kind `ENDPOINT`, after the routes) the build
    /// stamped ORIGIN external: its server is outside the build, so it has no
    /// handlers and `external_hosts` lists the sorted hosts its call sites
    /// name (a full-URL channel keeps the rows naming its host; `[]` on every
    /// in-repo server). `handlers` is the HANDLED_BY targets `{id, name,
    /// qname, kind, file, line}` in edge order, lines 1-based. `absence` is
    /// `None` when something serves the channel, inside the build or out,
    /// else the FACT-tier `unserved_channel` dict (a refused framework tag is
    /// `no_match`) with the mechanism's caveat rows, up to five near misses
    /// in `suggestions` (the route under another verb or the parent path's;
    /// the topic's producers or a look-alike topic) and `unparsed_files` set
    /// to `len(parse_errors)`.
    ///
    /// `collapse` (DD.13, default `True`): identity classes fold. A server
    /// row in a FACT / DERIVED SAME_AS class with an earlier row folds into
    /// it and gains an `identity` dict `{tier, rules, diverged, also_in}`
    /// (no shipped rule pairs routes, so in practice none does); within a
    /// server, handlers of one class keep the first, and every kept handler
    /// standing for a class gets an entry `{qname, identity}` in the row's
    /// `handler_identity` list (two repos running one SCIP-identical
    /// handler: each names the other repo's copy in `also_in`, `repo` a
    /// build label). Rows without them have no `identity` /
    /// `handler_identity` key. `collapse=False` is the raw rows.
    #[pyo3(signature = (channel, mechanism="auto", collapse=true))]
    fn serves(&self, py: Python<'_>, channel: &str, mechanism: &str, collapse: bool) -> PyResult<Py<PyAny>> {
        let collapse = if collapse {
            Collapse::with_labels(&self.repo_labels)
        } else {
            Collapse::off()
        };
        let answer = serves_answer(
            &self.merged,
            self.live(),
            channel,
            mechanism,
            &collapse,
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

        let hit = serves_answer(&merged, &live, "POST /orders", "auto", &Collapse::default(), 3).expect("answer");
        assert!(hit.absence.is_none());
        let json = serde_json::to_string(&hit).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let row = &v["results"][0];
        assert_eq!(row["match"], "exact", "{json}");
        assert_eq!(row["handlers"][0]["qname"], "app::create_order", "{json}");
        assert_eq!(row["live"], true, "{json}");

        let none = serves_answer(&merged, &live, "DELETE /orders", "http", &Collapse::default(), 3).expect("answer");
        let absence = none.absence.expect("nothing takes DELETE");
        assert_eq!(
            (absence.reason, absence.unparsed_files),
            ("unserved_channel", 3)
        );

        let err =
            serves_answer(&merged, &live, "orders", "smtp", &Collapse::default(), 0).expect_err("unknown mechanism");
        assert!(err.contains("smtp"), "{err}");
    }

    /// DD.13: `collapse=True` (the build's labels) tags each of two repos'
    /// SCIP-identical handlers with the other repo's copy; `collapse=False`
    /// is the raw rows, with no `handler_identity` key.
    #[test]
    fn collapse_tags_a_shared_handler() {
        use glia_code_domain::snapshots::{
            ScipDefRow, ScipDocumentRecord, ScipMeta, ScipSymbolRecord, source_hash, write_scip,
        };
        const APP: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
@app.route(\"/orders\", methods=[\"POST\"])\ndef create_order():\n    return {\"ok\": True}\n";
        let root = std::env::temp_dir().join(format!("glia-dd13-serves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for repo in ["billing", "shop"] {
            let dir = root.join(repo);
            std::fs::create_dir_all(&dir).expect("temp dir");
            std::fs::write(dir.join("app.py"), APP).expect("write fixture");
            let documents = vec![ScipDocumentRecord {
                path: "app.py".to_string(),
                language: "python".to_string(),
                source_hash: source_hash(APP.as_bytes()),
                defs: vec![ScipDefRow { s: 0, line: 6, name: "create_order".to_string() }],
                refs: Vec::new(),
            }];
            let symbols = vec![ScipSymbolRecord {
                id: 0,
                symbol: "scip-python python shop-svc 1.0 `app`/create_order().".to_string(),
                implements: Vec::new(),
            }];
            let meta = ScipMeta::new("scip-python".to_string(), "0.6.0".to_string(), String::new(), 0);
            write_scip(&dir, meta, &documents, &symbols).expect("write snapshot");
        }
        let paths = [root.join("billing"), root.join("shop")].map(|p| p.to_string_lossy().into_owned());
        let built = glia_engine::generate_many(&paths);
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");
        let live = glia_engine::entrypoint_reachable(&built.merged);

        let on = Collapse::with_labels(&built.repo_labels);
        let a = serves_answer(&built.merged, &live, "POST /orders", "auto", &on, 0).expect("answer");
        assert_eq!(a.results.len(), 2, "{:?}", a.results);
        let mut also: Vec<&str> = Vec::new();
        for s in &a.results {
            assert!(s.identity.is_none(), "{s:?}");
            assert_eq!(s.handler_identity.len(), 1, "{s:?}");
            let hi = &s.handler_identity[0];
            assert_eq!((hi.qname.as_str(), hi.identity.tier), ("app::create_order", "fact"));
            assert_eq!(hi.identity.also_in.len(), 1, "{:?}", hi.identity.also_in);
            also.push(hi.identity.also_in[0].repo.as_str());
        }
        also.sort_unstable();
        assert_eq!(also, ["billing", "shop"]);
        let text = serde_json::to_string(&a).expect("json");
        assert!(text.contains("\"handler_identity\":[{\"qname\":\"app::create_order\""), "{text}");

        let raw = serves_answer(&built.merged, &live, "POST /orders", "auto", &Collapse::off(), 0).expect("answer");
        assert_eq!(raw.results.len(), 2);
        assert!(!serde_json::to_string(&raw).expect("json").contains("identity"));
    }
}
