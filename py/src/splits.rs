//! **splits** (CD.2d): pyo3 surface for `glia_engine::splits` (CD.2b /
//! CD.2c) — `PyGraph.splits`, where a scope splits into services at the least
//! coupling: the module (or community) quotient cut into `parts` parts above a
//! balance floor, or, with `source` and `sink`, the minimum cut separating
//! the two; with the cut edges, the blockers (shared writes, cycles between
//! parts) and each part against `service_map`, as a native dict (LD.2). The
//! body is the pyo3-free [`split_args`] and [`splits_json`], so
//! `cargo test -p glia-py` covers it; the engine prints its
//! `[splits] ... surface=py` fired_on line.
//!
//! The argument errors `glia splits` exits 2 on raise ValueError here, before
//! the engine runs: `parts` outside 2..=8, `source` without `sink` (or the
//! reverse), `min_share` outside [0, 0.5]. An unknown `quotient` is the
//! engine's `no_match` absence naming it, not an error, as an unknown name is
//! in `communities` / `hubs`.

use std::collections::BTreeMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::splits::{MAX_PARTS, SplitArgs, splits};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// [`SplitArgs::surface`] for the marker.
const SURFACE: &str = "py";
/// `min_share` at most: the smaller side of a bisection holds at most half.
const MAX_MIN_SHARE: f64 = 0.5;

// `splits`' `parts=2, min_share=0.1, seed=42` below are literals so
// `__text_signature__` shows them; these keep them the engine's defaults
// (`quotient="module"` is pinned by `splits_json_matches_engine`).
const _: () = assert!(glia_engine::splits::DEFAULT_PARTS == 2);
const _: () = assert!(glia_engine::splits::DEFAULT_MIN_SHARE == 0.1);
const _: () = assert!(glia_engine::splits::DEFAULT_SEED == 42);

/// The engine arguments of a [`PyGraph::splits`] call, marked `surface=py`.
fn split_args(
    scope: Option<String>,
    parts: usize,
    quotient: String,
    source: Option<String>,
    sink: Option<String>,
    min_share: f64,
    seed: u64,
) -> SplitArgs {
    let mut args = SplitArgs::default();
    args.scope = scope;
    args.parts = parts;
    args.quotient = quotient;
    args.source = source;
    args.sink = sink;
    args.min_share = min_share;
    args.seed = seed;
    args.surface = SURFACE;
    args
}

/// Why `args` are refused before the engine runs (the arguments `glia
/// splits` exits 2 on), or `None`.
fn args_error(args: &SplitArgs) -> Option<String> {
    if !(2..=MAX_PARTS).contains(&args.parts) {
        return Some(format!(
            "parts must be in 2..={MAX_PARTS}, got {}",
            args.parts
        ));
    }
    match (&args.source, &args.sink) {
        (Some(_), None) => return Some("source needs sink: the sink is not set".to_string()),
        (None, Some(_)) => return Some("sink needs source: the source is not set".to_string()),
        _ => {}
    }
    let s = args.min_share;
    if !(0.0..=MAX_MIN_SHARE).contains(&s) {
        return Some(format!(
            "min_share must be a number in [0, {MAX_MIN_SHARE}], got {s}"
        ));
    }
    None
}

/// The whole body of [`PyGraph::splits`] after [`split_args`], minus pyo3:
/// the engine answer as JSON text, in the struct's field order (`to_py`
/// decodes it, so the dict keeps that order), or the argument error. An
/// absence counts the build's unparsed files.
fn splits_json(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    args: &SplitArgs,
    unparsed_files: usize,
) -> Result<String, String> {
    if let Some(e) = args_error(args) {
        return Err(e);
    }
    let mut answer = splits(merged, repo_labels, args);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    serde_json::to_string(&answer).map_err(|e| e.to_string())
}

#[pymethods]
impl PyGraph {
    /// **splits** (CD.2b / CD.2c / CD.2d): where the graph (or `scope`, a
    /// path or project label) splits into services at the least coupling.
    /// Without `source` / `sink` (the global mode) the `quotient` units
    /// (`"module"`: each node's enclosing module; `"community"`: its seeded
    /// Leiden community, `seed`) are cut by Stoer-Wagner into `parts` parts
    /// (2 to 8), each bisection's smaller side holding at least `min_share`
    /// (0 to 0.5) of the nodes when it can. With both `source` and `sink`
    /// (each a path, project label, or a node's qname or name) it is the
    /// `st` mode: the minimum cut separating the two, part 0 the source side.
    /// `parts` outside 2..=8, one of `source` / `sink` without the other, or
    /// `min_share` outside [0, 0.5] raise ValueError; an unknown `quotient`
    /// is an absence `no_match`.
    ///
    /// Returns a dict `{mode, quotient, units, parts, cut_weight,
    /// global_min_weight, balanced, cut_edges_total, cut_edges, arch,
    /// shared_writes, cycles, tier, absence}`. Each part is `{id, nodes,
    /// units, label, modules, services, entries, top_members}` (`services`
    /// `[name, count]` lists); a cut edge `{from_qname, to_qname, category,
    /// from_part, to_part, weight, file, line, basis}`; an arch row `{part,
    /// services, verdict}` (`aligned` / `splits_service` / `spans_services`
    /// / `unplaced`); a shared write `{entity, kind, parts, modes, writers,
    /// writers_total, tier}` (`modes` `[part, mode]` lists); a cycle `{parts,
    /// witness, tier}` (`witness` cut edges). Lines are 1-based; `tier` is
    /// `"heuristic"` for the cut. `absence` is `None` when there is a cut,
    /// else a dict saying why.
    #[pyo3(signature = (scope=None, parts=2, quotient="module", source=None, sink=None, min_share=0.1, seed=42))]
    // Python keywords, one per engine option: the signature is the surface.
    #[allow(clippy::too_many_arguments)]
    fn splits(
        &self,
        py: Python<'_>,
        scope: Option<String>,
        parts: usize,
        quotient: &str,
        source: Option<String>,
        sink: Option<String>,
        min_share: f64,
        seed: u64,
    ) -> PyResult<Py<PyAny>> {
        let args = split_args(
            scope,
            parts,
            quotient.to_string(),
            source,
            sink,
            min_share,
            seed,
        );
        let text = splits_json(
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

    /// `<pkg>/<m>.py` of a two-package tree: `orders` and `billing` hold two
    /// modules each, every function calling both functions of its package's
    /// other module; `orders.api.checkout` calls `billing.charge.charge`
    /// and `billing.charge.refund` calls `orders.repo.reopen` (the seam).
    fn tree() -> Vec<(&'static str, &'static str)> {
        vec![
            ("orders/__init__.py", ""),
            (
                "orders/api.py",
                "from orders.repo import save, reopen\nfrom billing.charge import charge\n\n\ndef create():\n    save()\n    reopen()\n\n\ndef checkout():\n    save()\n    reopen()\n    charge()\n",
            ),
            (
                "orders/repo.py",
                "from orders.api import create, checkout\n\n\ndef save():\n    create()\n    checkout()\n\n\ndef reopen():\n    create()\n    checkout()\n",
            ),
            ("billing/__init__.py", ""),
            (
                "billing/charge.py",
                "from billing.ledger import post, balance\nfrom orders.repo import reopen\n\n\ndef charge():\n    post()\n    balance()\n\n\ndef refund():\n    post()\n    balance()\n    reopen()\n",
            ),
            (
                "billing/ledger.py",
                "from billing.charge import charge, refund\n\n\ndef post():\n    charge()\n    refund()\n\n\ndef balance():\n    charge()\n    refund()\n",
            ),
        ]
    }

    /// CD.2d: pyo3 `splits` is the engine's `splits` under the same
    /// arguments, byte for byte; the argument errors `glia splits` exits 2
    /// on are refused before the engine runs; an absence counts the unparsed
    /// files. The cut itself is covered by `engine/tests/splits.rs`.
    #[test]
    fn splits_json_matches_engine() {
        let root = std::env::temp_dir().join(format!("glia-cd2d-splits-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, src) in tree() {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("temp dir");
            std::fs::write(path, src).expect("write fixture");
        }
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");

        let engine = |f: &dyn Fn(&mut SplitArgs)| {
            let mut args = SplitArgs::default();
            f(&mut args);
            serde_json::to_string(&splits(&built.merged, &built.repo_labels, &args)).expect("json")
        };
        let ours = |args: &SplitArgs| splits_json(&built.merged, &built.repo_labels, args, 0);

        // The Python defaults are the engine's.
        let defaults = split_args(None, 2, "module".to_string(), None, None, 0.1, 42);
        assert_eq!(defaults.surface, "py");
        let plain = SplitArgs::default();
        assert_eq!(
            (
                &defaults.quotient,
                defaults.parts,
                defaults.min_share,
                defaults.seed
            ),
            (&plain.quotient, plain.parts, plain.min_share, plain.seed),
            "the signature's literals are SplitArgs::default()"
        );

        let json = ours(&defaults).expect("a cut");
        assert_eq!(json, engine(&|_| {}));
        assert!(
            json.starts_with(
                "{\"mode\":\"global\",\"quotient\":\"module\",\"units\":4,\"parts\":["
            ),
            "{json}"
        );
        let v: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert!(v["absence"].is_null(), "{v}");
        let calls: Vec<(&str, &str)> = v["cut_edges"]
            .as_array()
            .expect("cut_edges")
            .iter()
            .filter(|c| c["category"] == "CALLS")
            .map(|c| {
                (
                    c["from_qname"].as_str().unwrap_or(""),
                    c["to_qname"].as_str().unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(
            calls,
            [
                ("billing::charge::refund", "orders::repo::reopen"),
                ("orders::api::checkout", "billing::charge::charge"),
            ],
            "the seam: {v}"
        );

        let st = split_args(
            None,
            2,
            "community".to_string(),
            Some("orders".to_string()),
            Some("billing".to_string()),
            0.2,
            7,
        );
        let st_json = ours(&st).expect("an st cut");
        assert_eq!(
            st_json,
            engine(&|a| {
                a.quotient = "community".to_string();
                a.source = Some("orders".to_string());
                a.sink = Some("billing".to_string());
                a.min_share = 0.2;
                a.seed = 7;
            })
        );
        assert!(st_json.starts_with("{\"mode\":\"st\","), "{st_json}");

        // Refused before the engine runs: no marker, no answer.
        let refused = |f: &dyn Fn(&mut SplitArgs)| {
            let mut args = split_args(None, 2, "module".to_string(), None, None, 0.1, 42);
            f(&mut args);
            ours(&args).expect_err("an argument error")
        };
        assert!(
            refused(&|a| a.source = Some("orders".to_string())).contains("the sink is not set")
        );
        assert!(
            refused(&|a| a.sink = Some("billing".to_string())).contains("the source is not set")
        );
        assert_eq!(refused(&|a| a.parts = 9), "parts must be in 2..=8, got 9");
        assert_eq!(refused(&|a| a.parts = 1), "parts must be in 2..=8, got 1");
        for bad in [-0.1, 0.6, f64::NAN] {
            assert!(
                refused(&|a| a.min_share = bad)
                    .starts_with("min_share must be a number in [0, 0.5]"),
                "{bad}"
            );
        }

        // An unknown quotient is the engine's absence, counting unparsed files.
        let files = split_args(None, 2, "files".to_string(), None, None, 0.1, 42);
        let unknown = splits_json(&built.merged, &built.repo_labels, &files, 2).expect("an answer");
        let v: serde_json::Value = serde_json::from_str(&unknown).expect("json");
        assert_eq!(v["quotient"], "none");
        assert_eq!(v["absence"]["reason"], "no_match");
        assert_eq!(v["absence"]["unparsed_files"], 2);
        assert!(
            v["absence"]["note"]
                .as_str()
                .is_some_and(|n| n.contains("files")),
            "{v}"
        );
    }
}
