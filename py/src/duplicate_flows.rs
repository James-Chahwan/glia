//! **duplicate_flows** (CD.4f): pyo3 surface for
//! `glia_engine::duplicate_flows` (CD.4e) — `PyGraph.duplicate_flows`, entry
//! flows whose reached sets are identical (exact, tier derived) or overlap at
//! a Jaccard threshold (near, tier heuristic), grouped and located, as a
//! native dict (LD.2). The body is the pyo3-free [`dup_flow_args`] and
//! [`duplicate_flows_json`], so `cargo test -p glia-py` covers it; the engine
//! prints its `[dupflows] ... surface=py` fired_on line.
//!
//! A `threshold` outside (0, 1] (NaN included) raises ValueError before the
//! engine runs, the argument `glia duplicate-flows` exits 2 on: the engine
//! itself takes any value (above 1 finds no near group, 0 pairs every
//! candidate).

use std::collections::BTreeMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::duplicate_flows::{DupFlowArgs, duplicate_flows};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// [`DupFlowArgs::surface`] for the marker.
const SURFACE: &str = "py";

// `duplicate_flows`' `depth=6, threshold=0.8, min_size=3` below are literals
// so `__text_signature__` shows them; these keep them the engine's defaults.
const _: () = assert!(glia_engine::duplicate_flows::DEFAULT_DEPTH == 6);
const _: () = assert!(glia_engine::duplicate_flows::DEFAULT_THRESHOLD == 0.8);
const _: () = assert!(glia_engine::duplicate_flows::DEFAULT_MIN_SIZE == 3);

/// The engine arguments of a [`PyGraph::duplicate_flows`] call, marked
/// `surface=py`; the MinHash seed stays the engine's.
fn dup_flow_args(
    scope: Option<String>,
    depth: usize,
    threshold: f64,
    min_size: usize,
    include_tests: bool,
    keep_hubs: bool,
) -> DupFlowArgs {
    let mut args = DupFlowArgs::default();
    args.scope = scope;
    args.depth = depth;
    args.threshold = threshold;
    args.min_size = min_size;
    args.include_tests = include_tests;
    args.keep_hubs = keep_hubs;
    args.surface = SURFACE;
    args
}

/// Why `threshold` is refused before the engine runs, or `None` for a number
/// in (0, 1].
fn threshold_error(threshold: f64) -> Option<String> {
    if threshold > 0.0 && threshold <= 1.0 {
        None
    } else {
        Some(format!("threshold must be in (0, 1], got {threshold}"))
    }
}

/// The whole body of [`PyGraph::duplicate_flows`] after [`dup_flow_args`],
/// minus pyo3: the engine answer as JSON text, in the struct's field order
/// (`to_py` decodes it, so the dict keeps that order), or the argument
/// error. An absence counts the build's unparsed files.
fn duplicate_flows_json(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    args: &DupFlowArgs,
    unparsed_files: usize,
) -> Result<String, String> {
    if let Some(e) = threshold_error(args.threshold) {
        return Err(e);
    }
    let mut answer = duplicate_flows(merged, repo_labels, args);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    serde_json::to_string(&answer).map_err(|e| e.to_string())
}

#[pymethods]
impl PyGraph {
    /// **duplicate_flows** (CD.4e / CD.4f): entry points whose forward flows
    /// (within `depth` hops over the carry edges, flows of fewer than
    /// `min_size` nodes skipped) reach the same nodes. An `exact` group
    /// (tier `derived`) is two or more entries with one reached set, e.g. an
    /// aliased route; a `near` group (tier `heuristic`) joins sets whose
    /// verified Jaccard is at least `threshold` (in (0, 1]; outside it,
    /// ValueError). Utility hubs (fan-in hubs such as a logger) are left out
    /// of every flow unless `keep_hubs`, test entries unless
    /// `include_tests`. `scope` (a path or project label) picks the entries.
    ///
    /// Returns a dict `{entries, flows, hubs_ignored, candidates,
    /// oversized_buckets, groups, absence}`. Each group is `{kind, tier,
    /// entries, jaccard, shared, union, differing, services}`: `jaccard` 1.0
    /// for an exact group and a near group's weakest kept pair, `shared` /
    /// `union` node counts, `differing` the first ten nodes some but not
    /// every set holds, `services` the `service_map` services of its
    /// entries. An entry or differing row is `{id, name, qname, kind, file,
    /// line}`, `line` 1-based. `absence` is `None` when there is a group,
    /// else a dict saying why.
    #[pyo3(signature = (scope=None, depth=6, threshold=0.8, min_size=3, include_tests=false, keep_hubs=false))]
    // Python keywords, one per engine option: the signature is the surface.
    #[allow(clippy::too_many_arguments)]
    fn duplicate_flows(
        &self,
        py: Python<'_>,
        scope: Option<String>,
        depth: usize,
        threshold: f64,
        min_size: usize,
        include_tests: bool,
        keep_hubs: bool,
    ) -> PyResult<Py<PyAny>> {
        let args = dup_flow_args(scope, depth, threshold, min_size, include_tests, keep_hubs);
        let text = duplicate_flows_json(
            &self.merged,
            &self.repo_labels,
            &args,
            self.parse_errors.len(),
        )
        .map_err(PyValueError::new_err)?;
        to_py(py, Ok(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CD.4e's fixture (engine/tests/duplicate_flows.rs, no log hub): GET
    /// /orders and GET /v2/orders stacked on one handler, POST /orders and
    /// PUT /orders/<id> sharing 14 of 18 nodes, GET /health alone, and a
    /// test entry calling the list handler.
    fn tree() -> Vec<(&'static str, String)> {
        const HELPERS: [&str; 14] = [
            "validate",
            "price",
            "tax",
            "discount",
            "stock",
            "reserve",
            "ship_date",
            "currency",
            "rounding",
            "fraud_check",
            "ledger_line",
            "audit_row",
            "receipt",
            "totals",
        ];
        let mut views = String::from("from flask import Flask\n\nfrom app import repo\n");
        views.push_str("\napp = Flask(__name__)\n\n\n");
        views.push_str("@app.route('/orders')\n@app.route('/v2/orders')\ndef list_orders():\n");
        views
            .push_str("    rows = repo.find()\n    return {'rows': rows, 'n': repo.count()}\n\n\n");
        for h in HELPERS.iter().chain(&["audit_create", "audit_update"]) {
            views.push_str(&format!("def {h}(x):\n    return x\n\n\n"));
        }
        for (route, method, name, audit) in [
            ("/orders", "POST", "create_order", "audit_create"),
            ("/orders/<id>", "PUT", "update_order", "audit_update"),
        ] {
            views.push_str(&format!(
                "@app.route('{route}', methods=['{method}'])\ndef {name}():\n    x = {{}}\n"
            ));
            for h in HELPERS {
                views.push_str(&format!("    x = {h}(x)\n"));
            }
            views.push_str(&format!("    return {audit}(x)\n\n\n"));
        }
        views.push_str("@app.route('/health')\ndef health():\n    return 'ok'\n");
        vec![
            ("app/__init__.py", String::new()),
            ("app/views.py", views),
            (
                "app/repo.py",
                "def find():\n    return []\n\n\ndef count():\n    return 0\n".to_string(),
            ),
            (
                "tests/test_orders.py",
                "from app.views import list_orders\n\n\ndef test_list():\n    list_orders()\n"
                    .to_string(),
            ),
        ]
    }

    /// CD.4f: pyo3 `duplicate_flows` is the engine's `duplicate_flows` under
    /// the same arguments, byte for byte; a threshold outside (0, 1] is
    /// refused before the engine runs; an absence counts the unparsed files.
    /// The grouping itself is covered by `engine/tests/duplicate_flows.rs`.
    #[test]
    fn duplicate_flows_json_matches_engine() {
        let root = std::env::temp_dir().join(format!("glia-cd4f-dupflows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, src) in tree() {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("temp dir");
            std::fs::write(path, src).expect("write fixture");
        }
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");

        let engine = |f: &dyn Fn(&mut DupFlowArgs)| {
            let mut args = DupFlowArgs::default();
            f(&mut args);
            serde_json::to_string(&duplicate_flows(&built.merged, &built.repo_labels, &args))
                .expect("json")
        };
        let ours = |args: &DupFlowArgs| {
            duplicate_flows_json(&built.merged, &built.repo_labels, args, 0).expect("accepted")
        };

        let args = dup_flow_args(None, 6, 0.8, 3, false, false);
        assert_eq!(args.surface, "py");
        let json = ours(&args);
        assert_eq!(json, engine(&|_| {}));
        assert!(
            json.starts_with("{\"entries\":5,\"flows\":4,\"hubs_ignored\":0,\"candidates\":"),
            "{json}"
        );
        assert!(
            json.contains(
                "\"groups\":[{\"kind\":\"exact\",\"tier\":\"derived\",\"entries\":[{\"id\":"
            ) && json.contains("\"qname\":\"GET /v2/orders\"")
                && json.ends_with(",\"absence\":null}"),
            "{json}"
        );
        assert!(!json.contains("\"kind\":\"near\""), "{json}");

        let near = ours(&dup_flow_args(Some("app".into()), 6, 0.7, 3, true, true));
        assert_eq!(
            near,
            engine(&|a| {
                a.scope = Some("app".into());
                a.threshold = 0.7;
                a.include_tests = true;
                a.keep_hubs = true;
            })
        );
        assert!(
            near.contains("\"kind\":\"near\",\"tier\":\"heuristic\""),
            "{near}"
        );
        let deep = ours(&dup_flow_args(None, 1, 1.0, 1, false, false));
        assert_eq!(
            deep,
            engine(&|a| {
                a.depth = 1;
                a.threshold = 1.0;
                a.min_size = 1;
            })
        );

        for bad in [0.0, -1.0, 1.5, f64::NAN] {
            let args = dup_flow_args(None, 6, bad, 3, false, false);
            let e = duplicate_flows_json(&built.merged, &built.repo_labels, &args, 0)
                .expect_err("refused");
            assert!(e.starts_with("threshold must be in (0, 1], got "), "{e}");
        }

        let args = dup_flow_args(Some("tests".into()), 6, 0.8, 3, false, false);
        let none =
            duplicate_flows_json(&built.merged, &built.repo_labels, &args, 2).expect("accepted");
        let v: serde_json::Value = serde_json::from_str(&none).expect("json");
        assert_eq!(v["groups"], serde_json::json!([]));
        assert_eq!(v["absence"]["reason"], "no_match", "{none}");
        assert_eq!(v["absence"]["unparsed_files"], 2, "{none}");
    }
}
