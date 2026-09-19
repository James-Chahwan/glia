//! **implementors** (LD.7c): who implements or extends a type (or overrides a
//! method), and its supertypes, tiered FACT / DERIVED / HEURISTIC — the LD.8a
//! envelope `{results, absence}` in a native `dict`.

use std::collections::HashSet;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_core::NodeId;
use glia_engine::absence::Answer;
use glia_engine::implementors::{HierarchyDirection, Implementor, implementors_with_live};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// `"down"` or `"up"`, any case; anything else is an error naming both.
fn direction_of(direction: &str) -> Result<HierarchyDirection, String> {
    match direction.trim().to_ascii_lowercase().as_str() {
        "down" => Ok(HierarchyDirection::Down),
        "up" => Ok(HierarchyDirection::Up),
        _ => Err(format!(
            "unknown direction '{}'; valid: down, up",
            direction.trim()
        )),
    }
}

/// The whole body of [`PyGraph::implementors`], minus pyo3 — kept pyo3-free
/// so `cargo test -p glia-py` covers it (see the crate doc). `live` is
/// the graph's cached `entrypoint_reachable` set (`PyGraph::live`).
fn implementors_answer(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    qname: &str,
    direction: &str,
    transitive: bool,
    unparsed_files: usize,
) -> Result<Answer<Implementor>, String> {
    let direction = direction_of(direction)?;
    let mut answer = implementors_with_live(merged, live, qname, direction, transitive);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    Ok(answer)
}

#[pymethods]
impl PyGraph {
    /// **implementors** (LD.7c): who implements or extends `qname`
    /// (`direction="down"`: implementors, subclasses, sub-interfaces; a method
    /// target lists the methods that implement or override it) or what it
    /// implements or extends (`direction="up"`), over IMPLEMENTS /
    /// INHERITS_FROM — every level when `transitive`, else the direct ones.
    /// Any other `direction` raises ValueError.
    ///
    /// Returns a dict `{results, absence}`: `results` is the rows
    /// `{id, qname, name, kind, file, line, live, relation, depth, via, tier}`
    /// in discovery order — `relation` the edge category that entered the
    /// row, `via` the node it was reached through when `depth > 1`, `tier`
    /// `FACT` (declared) / `DERIVED` (inferred by a rule, e.g. Go method
    /// sets) / `HEURISTIC`, the weakest edge on its path; lines 1-based.
    /// `absence` is `None` when a row was found, else the FACT-tier dict
    /// (`unknown_symbol` with suggestions, or `no_edges` with the heritage
    /// caveat rows of the target's language) with `unparsed_files` set to
    /// `len(parse_errors)`.
    #[pyo3(signature = (qname, direction="down", transitive=true))]
    fn implementors(
        &self,
        py: Python<'_>,
        qname: &str,
        direction: &str,
        transitive: bool,
    ) -> PyResult<Py<PyAny>> {
        let answer = implementors_answer(
            &self.merged,
            self.live(),
            qname,
            direction,
            transitive,
            self.parse_errors.len(),
        )
        .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LD.7c: pyo3 `implementors` is the engine's walk over the cached live
    /// set, with `direction` parsed, the parse-error count on an absence, and
    /// an unknown direction an error. The walk itself is covered by
    /// `engine/tests/implementors.rs`.
    #[test]
    fn implementors_is_the_engine_walk() {
        let root = std::env::temp_dir().join(format!("glia-ld7c-impl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        std::fs::write(
            root.join("Repos.cs"),
            "namespace Shop {\n    public interface IRepo { string Get(string id); }\n    \
             public interface IUserRepo : IRepo { string ByEmail(string email); }\n    \
             public class PgUserRepo : IUserRepo {\n        \
             public string Get(string id) { return id; }\n        \
             public string ByEmail(string email) { return email; }\n    }\n}\n",
        )
        .expect("write fixture");
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let merged = built.expect("build").merged;
        let live = glia_engine::entrypoint_reachable(&merged);

        let down =
            implementors_answer(&merged, &live, "Shop::IRepo", "down", true, 3).expect("answer");
        let json = serde_json::to_string(&down).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["results"][0]["qname"], "Shop::IUserRepo", "{json}");
        assert_eq!(v["results"][0]["tier"], "FACT", "{json}");
        assert_eq!(v["results"][1]["via"], "Shop::IUserRepo", "{json}");
        assert!(v["absence"].is_null(), "{json}");

        let up = implementors_answer(&merged, &live, "Shop::PgUserRepo", "UP", false, 0)
            .expect("answer");
        let qnames: Vec<&str> = up.results.iter().map(|r| r.qname.as_str()).collect();
        assert_eq!(qnames, ["Shop::IUserRepo"], "direct supertypes only");

        let none = implementors_answer(&merged, &live, "Shop::PgUserRepo", "down", true, 3)
            .expect("answer");
        let absence = none.absence.expect("nothing extends PgUserRepo");
        assert_eq!((absence.reason, absence.unparsed_files), ("no_edges", 3));

        let err = implementors_answer(&merged, &live, "Shop::IRepo", "sideways", true, 0)
            .expect_err("unknown direction");
        assert_eq!(err, "unknown direction 'sideways'; valid: down, up");
    }
}
