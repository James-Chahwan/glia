//! **hubs** (CD.4c): pyo3 surface for `glia_engine::hubs` (CD.4b) —
//! `PyGraph.hubs`, the fan-in, fan-out and cross-service hub rows, ranked and
//! located, as a native dict (LD.2). The body is the pyo3-free [`hub_args`]
//! and [`hubs_answer`], so `cargo test -p glia-py` covers it; the engine
//! prints its `[hubs] ... surface=py` fired_on line.

use std::collections::BTreeMap;

use pyo3::prelude::*;

use glia_engine::hubs::{HubArgs, HubsAnswer, hubs};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// [`HubArgs::surface`] for the marker.
const SURFACE: &str = "py";

// `hubs`' `top=20, min_degree=5` below are literals so `__text_signature__`
// shows them; these keep them the engine's defaults.
const _: () = assert!(glia_engine::hubs::DEFAULT_TOP == 20);
const _: () = assert!(glia_engine::hubs::DEFAULT_MIN_DEGREE == 5);

/// The engine arguments of a [`PyGraph::hubs`] call, marked `surface=py`.
fn hub_args(
    scope: Option<String>,
    top: usize,
    category: Option<String>,
    min_degree: u32,
    include_tests: bool,
) -> HubArgs {
    let mut args = HubArgs::default();
    args.scope = scope;
    args.top = top;
    args.category = category;
    args.min_degree = min_degree;
    args.include_tests = include_tests;
    args.surface = SURFACE;
    args
}

/// The whole body of [`PyGraph::hubs`] after [`hub_args`], minus pyo3. An
/// unknown category is the engine's `no_match` absence (naming it), not an
/// error; an absence counts the build's unparsed files. Returns the engine
/// answer itself, not a `serde_json::Value`: `to_py` decodes its JSON text so
/// the dict keeps the struct's field order (a `Value` map would sort it;
/// `convert.rs`).
fn hubs_answer(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    args: &HubArgs,
    unparsed_files: usize,
) -> HubsAnswer {
    let mut answer = hubs(merged, repo_labels, args);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    answer
}

#[pymethods]
impl PyGraph {
    /// **hubs** (CD.4b / CD.4c): the nodes that carry the most structural
    /// load, in three ranked lists. `fan_in` holds nodes with at least
    /// `max(min_degree, p99_in)` counted edges in (utilities), `fan_out` the
    /// same outward (orchestrators), `cross_service` nodes whose callers or
    /// callees sit in two or more `service_map` services. Counted edges are
    /// every carry edge but test coverage, docs and manifest dependencies,
    /// or exactly `category` (a registered edge-category name such as
    /// `"CALLS"`, ASCII case ignored; an unknown name is an absence
    /// `no_match`). Test nodes and their edges are left out unless
    /// `include_tests`. `scope` (a path or project label) picks the rows; an
    /// edge from outside it still counts. `top` rows per list (`0` = every
    /// qualifying row).
    ///
    /// Returns a dict `{fan_in, fan_out, cross_service, nodes, edges,
    /// p99_in, p99_out, absence}`. Each row is `{qname, kind, file, line,
    /// label, fan_in, fan_out, by_category, caller_services,
    /// callee_services, authority, hub, live, tier}`: `line` 1-based, `label`
    /// `utility` / `orchestrator` / `bottleneck` / `connector`,
    /// `by_category` a list of `[category, in, out]` (the five busiest),
    /// `authority` / `hub` the HITS scores, `tier` always `"derived"`.
    /// `absence` is `None` when any list has a row, else a dict saying why.
    #[pyo3(signature = (scope=None, top=20, category=None, min_degree=5, include_tests=false))]
    fn hubs(
        &self,
        py: Python<'_>,
        scope: Option<String>,
        top: usize,
        category: Option<String>,
        min_degree: u32,
        include_tests: bool,
    ) -> PyResult<Py<PyAny>> {
        let args = hub_args(scope, top, category, min_degree, include_tests);
        let answer = hubs_answer(
            &self.merged,
            &self.repo_labels,
            &args,
            self.parse_errors.len(),
        );
        to_py(py, serde_json::to_string(&answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CD.4c: pyo3 `hubs` is the engine's `hubs` under the same arguments,
    /// field for field; an absence counts the unparsed files. The ranking
    /// itself is covered by `engine/tests/hubs.rs`.
    #[test]
    fn hubs_json_matches_engine() {
        let root = std::env::temp_dir().join(format!("glia-cd4c-hubs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let callers = |svc: &str| {
            let mut s = String::from("from util.log import log\n\n");
            for i in 1..=4 {
                s.push_str(&format!("\ndef {svc}_{i}():\n    return log({i})\n\n"));
            }
            s
        };
        let mut tests = String::from("from util.log import log\n\n");
        for i in 1..=3 {
            tests.push_str(&format!(
                "\ndef test_log_{i}():\n    assert log({i}) == {i}\n\n"
            ));
        }
        for (rel, src) in [
            ("util/log.py", "def log(m):\n    return m\n".to_string()),
            ("svc_a/calls.py", callers("a")),
            ("svc_b/calls.py", callers("b")),
            ("tests/test_log.py", tests),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("temp dir");
            std::fs::write(path, src).expect("write fixture");
        }
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");

        let engine = |f: &dyn Fn(&mut HubArgs)| {
            let mut args = HubArgs::default();
            f(&mut args);
            serde_json::to_string(&hubs(&built.merged, &built.repo_labels, &args)).expect("json")
        };
        let ours = |scope: Option<&str>, top, category: Option<&str>, min_degree, include_tests| {
            let args = hub_args(
                scope.map(str::to_string),
                top,
                category.map(str::to_string),
                min_degree,
                include_tests,
            );
            assert_eq!(args.surface, "py");
            let answer = hubs_answer(&built.merged, &built.repo_labels, &args, 0);
            serde_json::to_string(&answer).expect("json")
        };

        let json = ours(None, 20, None, 5, false);
        assert_eq!(json, engine(&|_| {}));
        assert!(
            json.starts_with(
                "{\"fan_in\":[{\"qname\":\"util::log::log\",\"kind\":\"FUNCTION\",\"file\":\"util/log.py\",\"line\":1,\"label\":\"utility\",\"fan_in\":8,\"fan_out\":0,\"by_category\":[[\"CALLS\",8,0]],\"caller_services\":[\"svc_a\",\"svc_b\"]"
            ),
            "{json}"
        );
        assert_eq!(
            ours(Some("svc_a"), 0, Some("calls"), 1, true),
            engine(&|a| {
                a.scope = Some("svc_a".to_string());
                a.top = 0;
                a.category = Some("calls".to_string());
                a.min_degree = 1;
                a.include_tests = true;
            })
        );
        let with_tests = ours(None, 20, None, 5, true);
        assert!(with_tests.contains("\"fan_in\":11,"), "{with_tests}");

        let args = hub_args(None, 20, Some("NO_SUCH".to_string()), 5, false);
        let unknown = hubs_answer(&built.merged, &built.repo_labels, &args, 2);
        let why = unknown.absence.expect("an unknown category is an absence");
        assert_eq!((why.reason, why.unparsed_files), ("no_match", 2));
        assert!(why.note.contains("NO_SUCH"), "{}", why.note);
    }
}
