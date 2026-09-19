//! **effects** (LE.4d): what the named nodes do to the outside world — the
//! effect sinks downstream of them (DB read / write, queue produce, outbound
//! HTTP / RPC / WS / GraphQL call, event emit) with located witness paths —
//! the engine's `Effects` as a native `dict`.

use std::collections::BTreeMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_engine::effects::{Effects, EffectsArgs, effects};
use repo_graph_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The whole body of [`PyGraph::effects`], minus pyo3 — kept pyo3-free so
/// `cargo test -p repo-graph-py` covers it (see the crate doc). An absence
/// counts the build's unparsed files.
fn effects_answer(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    qnames: &[String],
    args: &EffectsArgs,
    unparsed_files: usize,
) -> Result<Effects, String> {
    let names: Vec<&str> = qnames.iter().map(String::as_str).collect();
    let mut answer = effects(merged, repo_labels, &names, args)?;
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    Ok(answer)
}

#[pymethods]
impl PyGraph {
    /// **effects** (LE.4d): the effect sinks downstream of `qnames` (each a
    /// qname, a dotted path or an exact simple name; at most 64; a
    /// `config:env:NAME` key seeds from the functions that read it).
    ///
    /// Returns a dict `{seeds, effects, counts, writes, unresolved,
    /// absence}`. `effects` is one row per sink `{class, qname, name, kind,
    /// file, line, mode, depth, seed, via_config, services_crossed,
    /// downstream, path, tier}`: `class` from the domain's sink table (`db`,
    /// `email`, `queue_produce`, `http_call`, `event_emit`, `rpc_call`,
    /// `ws_send`, `graphql_op`), `mode` the folded SQL verb (`read` / `write`
    /// / `read_write`) or None, `path` the witness hops `{from_qname,
    /// to_qname, category, site_file, site_line}` from `seed`, `downstream`
    /// the receivers one flow hop past the sink. Lines are 1-based. `depth`
    /// bounds the walk; `classes` keeps those classes; `writes_only` drops db
    /// reads; `cross_service` walks on past each send into the receiving
    /// handler (and counts `services_crossed`); `scope` keeps sinks under a
    /// path or project label. `absence` is the FACT-tier dict, with
    /// `unparsed_files` set, exactly when `effects` is empty. More than 64
    /// names, no name, or an unknown class raises ValueError.
    #[pyo3(signature = (qnames, depth=8, classes=None, writes_only=false, cross_service=false, scope=None))]
    #[allow(clippy::too_many_arguments)]
    fn effects(
        &self,
        py: Python<'_>,
        qnames: Vec<String>,
        depth: usize,
        classes: Option<Vec<String>>,
        writes_only: bool,
        cross_service: bool,
        scope: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let mut args = EffectsArgs::default();
        args.max_depth = depth;
        args.classes = classes;
        args.writes_only = writes_only;
        args.cross_service = cross_service;
        args.scope = scope;
        let answer = effects_answer(
            &self.merged,
            &self.repo_labels,
            &qnames,
            &args,
            self.parse_errors.len(),
        )
        .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LE.4d: pyo3 `effects` is the engine's answer with the parse-error
    /// count on an absence and engine errors passed through. Rows, modes and
    /// the cross-service walk are covered by `engine/tests/effects.rs`.
    #[test]
    fn effects_is_the_engine_answer() {
        let root = std::env::temp_dir().join(format!("glia-le4d-effects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("svc")).expect("temp dir");
        std::fs::write(
            root.join("svc").join("orders.ts"),
            "import { Pool } from 'pg';\nconst pool = new Pool();\n\n\
             export async function saveOrder(o) {\n  await pool.query('INSERT INTO orders (id) VALUES ($1)', [o.id]);\n}\n\n\
             export async function placeOrder(o) {\n  await saveOrder(o);\n}\n\n\
             export function label(o) {\n  return 'order ' + o.id;\n}\n",
        )
        .expect("write fixture");
        let built = repo_graph_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let g = built.expect("build");
        let names = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        let defaults = EffectsArgs::default();
        let hit = effects_answer(
            &g.merged,
            &g.repo_labels,
            &names(&["svc::orders::placeOrder"]),
            &defaults,
            3,
        )
        .expect("answer");
        let json = serde_json::to_string(&hit).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["effects"][0]["class"], "db", "{json}");
        assert_eq!(v["effects"][0]["qname"], "data_entity:sql:orders", "{json}");
        assert_eq!(v["effects"][0]["mode"], "write", "{json}");
        assert_eq!(v["effects"][0]["depth"], 2, "{json}");
        assert_eq!(v["effects"][0]["path"][0]["site_line"], 9, "{json}");
        assert_eq!(v["counts"]["db"], 1, "{json}");
        assert!(v["absence"].is_null(), "{json}");

        let miss = effects_answer(
            &g.merged,
            &g.repo_labels,
            &names(&["svc::orders::label"]),
            &defaults,
            3,
        )
        .expect("answer");
        let absence = miss.absence.expect("no effect");
        assert_eq!((absence.reason, absence.unparsed_files), ("no_edges", 3));

        let mut disk = EffectsArgs::default();
        disk.classes = Some(names(&["disk"]));
        let err = effects_answer(
            &g.merged,
            &g.repo_labels,
            &names(&["svc::orders::placeOrder"]),
            &disk,
            0,
        )
        .expect_err("unknown class");
        assert!(err.starts_with("unknown effect class `disk`"), "{err}");
    }
}
