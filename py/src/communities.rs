//! **communities** (CD.1e): pyo3 surface for `glia_engine::communities`
//! (CD.1d) — `PyGraph.communities`, the seeded-Leiden communities and their
//! structured summaries as a native dict (LD.2). The body is the pyo3-free
//! [`community_args`] and [`communities_json`], so `cargo test -p glia-py`
//! covers it; the engine prints its `[communities] ... surface=py` fired_on
//! line.

use std::collections::BTreeMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::communities::{CommunityArgs, communities};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// [`CommunityArgs::surface`] for the marker.
const SURFACE: &str = "py";

// `communities`' `seed=42, resolution=1.0, top=30, members=10` below are
// literals so `__text_signature__` shows them; these keep them the engine's
// defaults.
const _: () = assert!(glia_engine::communities::DEFAULT_SEED == 42);
const _: () = assert!(glia_engine::communities::DEFAULT_RESOLUTION == 1.0);
const _: () = assert!(glia_engine::communities::DEFAULT_TOP == 30);
const _: () = assert!(glia_engine::communities::DEFAULT_MEMBERS == 10);

/// The engine arguments of a [`PyGraph::communities`] call, marked
/// `surface=py`.
fn community_args(
    scope: Option<String>,
    seed: u64,
    resolution: f64,
    top: usize,
    members: usize,
    method: Option<String>,
) -> CommunityArgs {
    let mut args = CommunityArgs::default();
    args.scope = scope;
    args.seed = seed;
    args.resolution = resolution;
    args.top = top;
    args.members = members;
    args.method = method;
    args.surface = SURFACE;
    args
}

/// The whole body of [`PyGraph::communities`] after [`community_args`],
/// minus pyo3: the engine answer as JSON text, in the struct's field order
/// (`to_py` decodes it, so the dict keeps that order). A resolution that is
/// not a finite number above 0 is an error, as `glia communities` refuses it
/// (the engine would run it at 1.0); an unknown method is the engine's
/// `no_match` absence naming it, not an error. An absence counts the build's
/// unparsed files.
fn communities_json(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    args: &CommunityArgs,
    unparsed_files: usize,
) -> Result<String, String> {
    let r = args.resolution;
    if !(r.is_finite() && r > 0.0) {
        return Err(format!("resolution must be a finite number > 0, got {r}"));
    }
    let mut answer = communities(merged, repo_labels, args);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    serde_json::to_string(&answer).map_err(|e| e.to_string())
}

#[pymethods]
impl PyGraph {
    /// **communities** (CD.1d / CD.1e): the graph partitioned into
    /// communities by seeded Leiden (label propagation above Leiden's pair
    /// cap), each summarised from observed edges. `scope` (a path or project
    /// label) partitions only the nodes under it. `seed` seeds every random
    /// choice; `resolution` is modularity's gamma (above 1 favours more,
    /// smaller communities; not a finite number above 0 raises ValueError).
    /// `top` communities are summarised, largest first, with `members` top
    /// members each (`0` keeps every one). `method` is `"leiden"`, `"lpa"` /
    /// `"label_propagation"` or `None` (the default choice); an unknown name
    /// is an absence `no_match`, not an error.
    ///
    /// Returns a dict `{method, seed, resolution, modularity, total, nodes,
    /// isolated, communities, absence}`: `total` counts every community
    /// found, `isolated` the nodes with no weighted edge (in none). Each
    /// community is `{id, size, label, tier, cohesion, files, kinds,
    /// top_members, entries, services, sinks, links}`: `kinds` / `services` /
    /// `sinks` are `[name, count]` lists, `top_members` `{qname, kind, file,
    /// line, weight}`, `entries` located nodes `{id, name, qname, kind, file,
    /// line}`, `links` `{to, weight, edges, categories}`; lines 1-based, `tier`
    /// always `"heuristic"`. `absence` is `None` when a community is listed,
    /// else a dict saying why.
    #[pyo3(signature = (scope=None, seed=42, resolution=1.0, top=30, members=10, method=None))]
    // Python keywords, one per engine option: the signature is the surface.
    #[allow(clippy::too_many_arguments)]
    fn communities(
        &self,
        py: Python<'_>,
        scope: Option<String>,
        seed: u64,
        resolution: f64,
        top: usize,
        members: usize,
        method: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let args = community_args(scope, seed, resolution, top, members, method);
        let text = communities_json(
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

    /// CD.1d's fixture `core.py`: `<p>0..<p>5` in a ring with chords, `f3`
    /// also calling the imported `g1`, and `main` in `pkg_a`.
    fn core_py(p: char, import: Option<&str>) -> String {
        let mut s = import
            .map(|m| format!("from {m}.core import g1\n\n"))
            .unwrap_or_default();
        for i in 0..6 {
            let extra = if p == 'f' && i == 3 { "\n    g1()" } else { "" };
            s.push_str(&format!(
                "\ndef {p}{i}():\n    {p}{}()\n    {p}{}(){extra}\n\n",
                (i + 1) % 6,
                (i + 2) % 6
            ));
        }
        if p == 'f' {
            s.push_str("\ndef main():\n    f0()\n");
        }
        s
    }

    /// CD.1e: pyo3 `communities` is the engine's `communities` under the same
    /// arguments, byte for byte; a bad resolution is an error, an unknown
    /// method an absence counting the unparsed files. The partition itself is
    /// covered by `engine/tests/communities.rs`.
    #[test]
    fn communities_json_is_the_engine_answer() {
        let root =
            std::env::temp_dir().join(format!("glia-cd1e-communities-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, src) in [
            ("pkg_a/__init__.py", String::new()),
            ("pkg_a/core.py", core_py('f', Some("pkg_b"))),
            ("pkg_b/__init__.py", String::new()),
            ("pkg_b/core.py", core_py('g', None)),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("temp dir");
            std::fs::write(path, src).expect("write fixture");
        }
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");

        let engine = |f: &dyn Fn(&mut CommunityArgs)| {
            let mut args = CommunityArgs::default();
            f(&mut args);
            serde_json::to_string(&communities(&built.merged, &built.repo_labels, &args))
                .expect("json")
        };
        let ours = |scope: Option<&str>, seed, resolution, top, members, method: Option<&str>| {
            let args = community_args(
                scope.map(str::to_string),
                seed,
                resolution,
                top,
                members,
                method.map(str::to_string),
            );
            assert_eq!(args.surface, "py");
            communities_json(&built.merged, &built.repo_labels, &args, 0)
        };

        let json = ours(None, 42, 1.0, 30, 10, None).expect("an answer");
        assert_eq!(json, engine(&|_| {}));
        assert!(
            json.starts_with(
                "{\"method\":\"leiden\",\"seed\":42,\"resolution\":1.0,\"modularity\":"
            ),
            "{json}"
        );
        let v: serde_json::Value = serde_json::from_str(&json).expect("parses");
        let labels: Vec<&str> = v["communities"]
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|c| c["label"].as_str())
            .collect();
        assert_eq!(labels, ["pkg_a::core", "pkg_b::core"], "{json}");
        assert_eq!(
            (
                v["total"].as_u64(),
                v["nodes"].as_u64(),
                v["isolated"].as_u64()
            ),
            (Some(2), Some(17), Some(2))
        );
        assert!(v["absence"].is_null());

        assert_eq!(
            ours(Some("pkg_b"), 7, 0.5, 1, 3, Some("lpa")).expect("an answer"),
            engine(&|a| {
                a.scope = Some("pkg_b".to_string());
                a.seed = 7;
                a.resolution = 0.5;
                a.top = 1;
                a.members = 3;
                a.method = Some("lpa".to_string());
            })
        );

        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let err = ours(None, 42, bad, 30, 10, None).expect_err("a bad resolution");
            assert!(
                err.starts_with("resolution must be a finite number > 0"),
                "{err}"
            );
        }

        let args = community_args(None, 42, 1.0, 30, 10, Some("louvain".to_string()));
        let unknown = communities_json(&built.merged, &built.repo_labels, &args, 2)
            .expect("an unknown method is an answer");
        let v: serde_json::Value = serde_json::from_str(&unknown).expect("parses");
        assert_eq!(v["method"], "none");
        assert_eq!(v["absence"]["reason"], "no_match");
        assert_eq!(v["absence"]["unparsed_files"], 2);
        assert!(
            v["absence"]["note"]
                .as_str()
                .is_some_and(|n| n.contains("louvain")),
            "{unknown}"
        );
    }
}
