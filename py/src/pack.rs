//! **pack** (CC.4c): pyo3 surface for `glia_engine::pack` (CC.4b) —
//! `PyGraph.pack` (seeds from a query, as `find` matches it) and
//! `PyGraph.pack_ids` (seeds by node id: a diff-impact's, a trace's), the
//! context packed to a token budget, returned as the whole `Pack` — its text
//! and its manifest — in a native dict (LD.2). The bodies are the pyo3-free
//! [`pack_args`], [`pack_query`] and [`pack_seeds`], so `cargo test -p
//! glia-py` covers them; the engine prints its `[pack] query=...` fired_on
//! line once per call.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_core::NodeId;
use glia_engine::pack::{Pack, PackArgs, pack, pack_ids};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

// The `budget=8000, bytes_per_token=3.7, seeds=5, candidates=200` below are
// literals so `__text_signature__` shows them; these keep them the engine's
// defaults.
const _: () = assert!(glia_engine::pack::DEFAULT_BUDGET_TOKENS == 8000);
const _: () = assert!(glia_engine::pack::DEFAULT_BYTES_PER_TOKEN_X10 == 37);
const _: () = assert!(glia_engine::pack::DEFAULT_SEEDS == 5);
const _: () = assert!(glia_engine::pack::DEFAULT_CANDIDATES == 200);

/// The bytes-per-token range a caller may ask for, as `glia pack` takes it.
const MIN_BPT: f64 = 1.0;
const MAX_BPT: f64 = 20.0;

/// The engine arguments of a [`PyGraph::pack`] / [`PyGraph::pack_ids`] call.
/// `bytes_per_token` outside 1.0..=20.0 (or not finite) is an error (a
/// `ValueError`); inside, it is rounded to tenths, the engine's unit.
fn pack_args(
    budget: usize,
    bytes_per_token: f64,
    seeds: usize,
    candidates: usize,
    preset: Option<String>,
    scope: Option<String>,
) -> Result<PackArgs, String> {
    if !(bytes_per_token.is_finite() && (MIN_BPT..=MAX_BPT).contains(&bytes_per_token)) {
        return Err(format!(
            "bytes_per_token must be between {MIN_BPT:.1} and {MAX_BPT:.1}, got {bytes_per_token}"
        ));
    }
    let mut args = PackArgs::default();
    args.budget_tokens = budget;
    // In range, so the rounded tenths lie in 10..=200: the cast is exact.
    args.bytes_per_token_x10 = (bytes_per_token * 10.0).round() as u32;
    args.seeds = seeds;
    args.candidates = candidates;
    args.preset = preset;
    args.scope = scope;
    Ok(args)
}

/// The whole body of [`PyGraph::pack`] after [`pack_args`], minus pyo3: an
/// absence counts the build's unparsed files. Returns the engine answer
/// itself, not a `serde_json::Value`: `to_py` decodes its JSON text so the
/// dict keeps the struct's field order (a `Value` map would sort it;
/// `convert.rs`).
fn pack_query(merged: &MergedGraph, query: &str, args: &PackArgs, unparsed_files: usize) -> Pack {
    with_unparsed(pack(merged, query, args), unparsed_files)
}

/// [`pack_query`] for [`PyGraph::pack_ids`]: the ids are the seeds, in order.
fn pack_seeds(merged: &MergedGraph, ids: &[u64], args: &PackArgs, unparsed_files: usize) -> Pack {
    let seeds: Vec<NodeId> = ids.iter().copied().map(NodeId).collect();
    with_unparsed(pack_ids(merged, &seeds, args), unparsed_files)
}

fn with_unparsed(mut p: Pack, unparsed_files: usize) -> Pack {
    if let Some(a) = p.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    p
}

#[pymethods]
impl PyGraph {
    /// **pack** (CC.4b / CC.4c): the context for `query` packed to `budget`
    /// tokens, ready to hand to a model. The seeds are `find`'s first `seeds`
    /// rows (exact matches crowd out fuzzy ones); the rest of the up to
    /// `candidates` nodes are their callers and callees by personalised
    /// PageRank (`preset` a code-domain activation preset such as `"repair"`,
    /// `"review"` or `"onboard"`; `None` or an unknown name is the base
    /// weights). Each packed node gets the most detail the budget buys — full
    /// source, a preview, an outline line or a bare qname — and no neighbour
    /// more than a seed. `scope` (a path or project label) keeps nodes under
    /// it. Tokens are estimated at `bytes_per_token` bytes each (1.0 to 20.0,
    /// rounded to tenths; `ValueError` outside).
    ///
    /// Returns a dict `{query, text, budget_tokens, used_tokens, bytes,
    /// bytes_per_token, candidates, nodes, dropped, rerenders, absence}`:
    /// `text` the pack (`""` when empty), `bytes_per_token` the rate as text
    /// (`"3.7"`), `nodes` the packed nodes in rank order, each `{id, qname,
    /// kind, file, line, fidelity, tokens, rank, tier, reason, matched}` with
    /// `line` 1-based and `fidelity` `full` / `preview` / `outline` /
    /// `qname`. `absence` is `None` when a node is packed, else a dict whose
    /// `reason` is `no_match` or `budget_too_small`.
    #[pyo3(signature = (query, budget=8000, bytes_per_token=3.7, seeds=5, candidates=200, preset=None, scope=None))]
    #[allow(clippy::too_many_arguments)]
    fn pack(
        &self,
        py: Python<'_>,
        query: &str,
        budget: usize,
        bytes_per_token: f64,
        seeds: usize,
        candidates: usize,
        preset: Option<String>,
        scope: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let args = pack_args(budget, bytes_per_token, seeds, candidates, preset, scope)
            .map_err(PyValueError::new_err)?;
        let p = pack_query(&self.merged, query, &args, self.parse_errors.len());
        to_py(py, serde_json::to_string(&p))
    }

    /// **pack_ids** (CC.4b / CC.4c): `pack` seeded by node
    /// ids instead of a query — each id a seed of tier `fact`, in the order
    /// given, a repeated id counted once, one no graph holds skipped. Explicit
    /// seeds are kept whatever `scope` says; their neighbours are scoped. The
    /// dict's `query` names the first three seeds' qnames (`" +N more"`
    /// after). No seed left is an absence `no_match`. Same dict as `pack`.
    #[pyo3(signature = (node_ids, budget=8000, bytes_per_token=3.7, candidates=200, preset=None, scope=None))]
    #[allow(clippy::too_many_arguments)]
    fn pack_ids(
        &self,
        py: Python<'_>,
        node_ids: Vec<u64>,
        budget: usize,
        bytes_per_token: f64,
        candidates: usize,
        preset: Option<String>,
        scope: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let seeds = glia_engine::pack::DEFAULT_SEEDS;
        let args = pack_args(budget, bytes_per_token, seeds, candidates, preset, scope)
            .map_err(PyValueError::new_err)?;
        let p = pack_seeds(&self.merged, &node_ids, &args, self.parse_errors.len());
        to_py(py, serde_json::to_string(&p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CC.4c: pyo3 `pack` / `pack_ids` are the engine's `pack` / `pack_ids`
    /// under the same arguments, byte for byte; `bytes_per_token` is rounded
    /// to tenths and range-checked; an absence counts the unparsed files. The
    /// packing itself is covered by `engine/tests/pack.rs`.
    #[test]
    fn pack_json_matches_engine() {
        let root = std::env::temp_dir().join(format!("glia-cc4c-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, src) in [
            (
                "shop/a.py",
                "def price(o):\n    total = o.qty * o.unit\n    return total\n\n\ndef place(o):\n    return price(o)\n",
            ),
            (
                "shop/b.py",
                "from shop.a import place\n\n\ndef checkout(o):\n    return place(o)\n",
            ),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("temp dir");
            std::fs::write(path, src).expect("write fixture");
        }
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");
        let m = &built.merged;

        let args = pack_args(100_000, 3.7, 5, 200, None, None).expect("in range");
        assert_eq!(args.bytes_per_token_x10, 37);
        let ours = serde_json::to_string(&pack_query(m, "price", &args, 0)).expect("json");
        let engine = serde_json::to_string(&pack(m, "price", &args)).expect("json");
        assert_eq!(ours, engine);
        assert!(
            ours.starts_with("{\"query\":\"price\",\"text\":\"# context for price\\n"),
            "{ours}"
        );
        assert!(ours.contains("\"bytes_per_token\":\"3.7\""), "{ours}");
        assert!(
            ours.contains("\"qname\":\"shop::a::price\",\"kind\":\"FUNCTION\",\"file\":\"shop/a.py\",\"line\":1,\"fidelity\":\"full\""),
            "{ours}"
        );

        let price = m.node_id_by_qname("shop::a::price").expect("price");
        let by_id = pack_seeds(m, &[price.0, price.0], &args, 0);
        assert_eq!(by_id.query, "shop::a::price");
        assert_eq!(
            (
                by_id.nodes[0].id,
                by_id.nodes[0].reason,
                by_id.nodes[0].tier
            ),
            (price.0, "seed", "fact")
        );
        assert_eq!(
            serde_json::to_string(&by_id).expect("json"),
            serde_json::to_string(&pack_ids(m, &[price], &args)).expect("json")
        );

        let mut custom = pack_args(
            50,
            4.04,
            2,
            10,
            Some("repair".to_string()),
            Some("shop".to_string()),
        )
        .expect("in range");
        assert_eq!(
            (
                custom.budget_tokens,
                custom.bytes_per_token_x10,
                custom.seeds,
                custom.candidates
            ),
            (50, 40, 2, 10)
        );
        assert_eq!(
            (custom.preset.as_deref(), custom.scope.as_deref()),
            (Some("repair"), Some("shop"))
        );
        custom.budget_tokens = 100_000;
        assert_eq!(
            serde_json::to_string(&pack_query(m, "checkout", &custom, 0)).expect("json"),
            serde_json::to_string(&pack(m, "checkout", &custom)).expect("json")
        );
        assert_eq!(
            pack_args(1, 1.0, 1, 1, None, None)
                .expect("low end")
                .bytes_per_token_x10,
            10
        );
        assert_eq!(
            pack_args(1, 20.0, 1, 1, None, None)
                .expect("high end")
                .bytes_per_token_x10,
            200
        );
        for bad in [0.5, 0.99, 20.01, f64::NAN, f64::INFINITY, -3.7] {
            let err = pack_args(1, bad, 1, 1, None, None).expect_err("out of range");
            assert!(err.contains("between 1.0 and 20.0"), "{err}");
        }

        let none = pack_query(m, "zzz_nothing", &args, 3);
        let why = none.absence.expect("no match is an absence");
        assert_eq!((why.reason, why.unparsed_files), ("no_match", 3));
        assert!(none.nodes.is_empty() && none.text.is_empty());
        let missing = pack_seeds(m, &[1, 2], &args, 0);
        assert_eq!(missing.absence.map(|a| a.reason), Some("no_match"));
    }
}
