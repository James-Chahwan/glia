//! **cycles** (LE.6b): cross-service event loops, service-level possible
//! loops and module import cycles, as a native list of dicts.

use std::collections::BTreeMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::cycles::{CycleArgs, CycleRow, cycles, kinds_for};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The whole body of [`PyGraph::cycles`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` covers it (see the crate doc). An unknown
/// `kind` is an error naming the three kinds.
fn cycle_rows(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    kind: &str,
    scope: Option<&str>,
) -> Result<Vec<CycleRow>, String> {
    let mut args = CycleArgs::default();
    args.kinds = kinds_for(kind)?;
    args.scope = scope.map(str::to_string);
    Ok(cycles(merged, repo_labels, &args))
}

#[pymethods]
impl PyGraph {
    /// **cycles** (LE.6b): the loops in this graph. `kind` is `"event"`
    /// (cross-service loops), `"import"` (module import cycles) or `"all"`;
    /// any other value raises ValueError. `scope` (a path or project label)
    /// keeps only cycles whose members all sit under it.
    ///
    /// Returns a list of dicts `{kind, tier, services, mechanisms, channels,
    /// size, members, witness, note}`, ordered `event_loop` (a node-level
    /// loop across two or more services through a queue / event hop),
    /// `call_loop` (the same with no queue / event hop), `possible_loop`
    /// (services that publish to each other with no handler-to-producer path
    /// in the graph: tier `heuristic`, members are service ids), then
    /// `import_cycle`; within a kind by size descending. `witness` is a
    /// shortest cycle as hops `{from_qname, to_qname, category, channel,
    /// file, line}`, each located at its edge's evidence site (lines
    /// 1-based). `members` lists at most 50; `size` is the full count.
    #[pyo3(signature = (kind="all", scope=None))]
    fn cycles(&self, py: Python<'_>, kind: &str, scope: Option<&str>) -> PyResult<Py<PyAny>> {
        let rows = cycle_rows(&self.merged, &self.repo_labels, kind, scope)
            .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LE.6b: pyo3 `cycles` is the engine's `cycles` under the parsed kind,
    /// with an unknown kind an error. The cycle finding itself is covered by
    /// `engine/tests/cycles.rs`.
    #[test]
    fn cycles_is_the_engine_cycles() {
        let root = std::env::temp_dir().join(format!("glia-le6b-cycles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("pkg")).expect("temp dir");
        for (rel, src) in [
            ("pkg/__init__.py", ""),
            (
                "pkg/a.py",
                "from pkg.b import f\n\n\ndef g():\n    return 1\n",
            ),
            (
                "pkg/b.py",
                "from pkg.a import g\n\n\ndef f():\n    return 2\n",
            ),
        ] {
            std::fs::write(root.join(rel), src).expect("write fixture");
        }
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");

        let rows = cycle_rows(&built.merged, &built.repo_labels, "all", None).expect("rows");
        let json = serde_json::to_string(&rows).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v[0]["kind"], "import_cycle", "{json}");
        assert_eq!(
            v[0]["members"],
            serde_json::json!(["pkg::a", "pkg::b"]),
            "{json}"
        );
        assert_eq!(v[0]["witness"].as_array().map(Vec::len), Some(2), "{json}");

        let event = cycle_rows(&built.merged, &built.repo_labels, "event", None).expect("rows");
        assert!(event.is_empty(), "no cross-service loop in one package");
        let scoped = cycle_rows(
            &built.merged,
            &built.repo_labels,
            "import",
            Some("pkg/a.py"),
        )
        .expect("rows");
        assert!(scoped.is_empty(), "pkg/b.py is outside the scope");

        let err =
            cycle_rows(&built.merged, &built.repo_labels, "loops", None).expect_err("unknown kind");
        assert!(err.contains("loops"), "{err}");
    }
}
