//! LD.12c: blast-radius ranking, its `live_only` cut and resolve's scores run
//! through one `ActivationPlan` instead of `activate()` plus a lookup and a
//! post-sort filter.
//!
//! The pre-plan computation is kept here as the oracle: PPR through
//! `MergedGraph::activate` (`top_k = usize::MAX`), each closure node or seed
//! looked up at its score, 0.0 where PPR gave none, and (blast radius) sorted
//! by score desc, then node id asc. The plan must reproduce it bit for bit, so
//! every CLI / pyo3 answer stays byte-identical.
//!
//! The fixture is `bench/substrate-gap/fixtures/xstack-go-http`, built as the
//! CLI builds `client --with server`: the Go client's `GET /users` ENDPOINT
//! paired to the chi server's `GET /users` ROUTE.

use std::collections::HashMap;

use repo_graph_activation::Direction;
use repo_graph_engine::profile::CODE_PROFILE;
use repo_graph_engine::{
    BlastAnswer, BlastOptions, blast_radius, entrypoint_reachable, generate_many, resolve_signal_located,
};
use repo_graph_graph::{MergedGraph, Reach};
use repo_graph_core::NodeId;

fn fixture() -> MergedGraph {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../bench/substrate-gap/fixtures/xstack-go-http");
    let repos = vec![format!("{root}/client"), format!("{root}/server")];
    generate_many(&repos).expect("xstack-go-http builds").merged
}

/// Every field a caller sees, the score as bits.
type Row = (u64, String, &'static str, usize, u64, bool, Option<String>, Option<i64>);

fn rows(answer: &[BlastAnswer]) -> Vec<Row> {
    answer
        .iter()
        .map(|a| (a.id, a.qname.clone(), a.reason, a.depth, a.score.to_bits(), a.live, a.file.clone(), a.line))
        .collect()
}

const SEED: &str = "GET /users";

/// The rows of LD.5's `blast_radius` from [`SEED`] alone, which must resolve.
fn blast(m: &MergedGraph, direction: Reach, top_k: Option<usize>, live_only: bool) -> Vec<BlastAnswer> {
    let mut opts = BlastOptions::default();
    opts.direction = direction;
    opts.top_k = top_k;
    opts.live_only = live_only;
    let answer = blast_radius(m, &[SEED], &opts);
    assert!(answer.unresolved.is_empty(), "seed resolves");
    answer.results
}

#[test]
fn live_only_is_an_order_preserving_filter() {
    let m = fixture();
    for dir in [Reach::Forward, Reach::Backward, Reach::Both] {
        let all = blast(&m, dir, None, false);
        let live = blast(&m, dir, None, true);
        let expected: Vec<Row> = rows(&all).into_iter().filter(|r| r.5).collect();
        assert_eq!(rows(&live), expected, "{dir:?}: live_only == the full answer's live rows, same order");
        // `top_k` still cuts after the filter: the first live row, not the
        // first row filtered afterwards.
        let one = blast(&m, dir, Some(1), true);
        assert_eq!(rows(&one), expected.into_iter().take(1).collect::<Vec<_>>(), "{dir:?}: live + top_k 1");
    }
    // Both ways from the ROUTE: the client ENDPOINT, the chi handler and the
    // client function that calls the endpoint. Only the handler is reachable
    // from an entrypoint.
    let both = blast(&m, Reach::Both, None, false);
    let qnames: Vec<&str> = both.iter().map(|a| a.qname.as_str()).collect();
    assert_eq!(qnames, ["endpoint:GET:/users", "main::listUsers", "client::FetchUsers"]);
    let live = blast(&m, Reach::Both, None, true);
    let live_q: Vec<&str> = live.iter().map(|a| a.qname.as_str()).collect();
    assert_eq!(live_q, ["main::listUsers"], "--live-only drops the two dead rows");
}

/// The pre-LD.12c blast ranking: `activate()` over the whole graph, looked up
/// per closure node, sorted by (score desc, id asc).
fn legacy_blast_ranking(m: &MergedGraph, seed: NodeId, reach: Reach) -> Vec<(NodeId, u64)> {
    let closure: Vec<NodeId> =
        m.blast_radius(&[seed], reach, 4, &CODE_PROFILE.tables).iter().map(|h| h.id).collect();
    let mut config = CODE_PROFILE.tables.activation_config(None);
    config.direction = match reach {
        Reach::Forward => Direction::Forward,
        Reach::Backward => Direction::Backward,
        _ => Direction::Undirected,
    };
    config.top_k = usize::MAX;
    let scores: HashMap<NodeId, f64> = m.activate(&[seed], &config).scores.into_iter().collect();
    let mut ranked: Vec<(NodeId, f64)> =
        closure.into_iter().map(|id| (id, scores.get(&id).copied().unwrap_or(0.0))).collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.0.cmp(&b.0.0))
    });
    ranked.into_iter().map(|(id, s)| (id, s.to_bits())).collect()
}

#[test]
fn blast_ranking_is_the_legacy_activate_lookup_bit_for_bit() {
    let m = fixture();
    let seed = m.qnames_exact(SEED).first().copied().expect("the ROUTE's qname is `GET /users`");
    for reach in [Reach::Forward, Reach::Backward, Reach::Both] {
        let plan: Vec<(NodeId, u64)> = m
            .blast_radius(&[seed], reach, 4, &CODE_PROFILE.tables)
            .iter()
            .map(|h| (h.id, h.score.to_bits()))
            .collect();
        assert_eq!(plan, legacy_blast_ranking(&m, seed, reach), "{reach:?}");
    }
    let both = m.blast_radius(&[seed], Reach::Both, 4, &CODE_PROFILE.tables);
    assert_eq!(both.len(), 3, "the closure the parity case ranks");
}

#[test]
fn resolve_scores_are_the_legacy_activate_lookup_bit_for_bit() {
    let m = fixture();
    let live = entrypoint_reachable(&m);
    for (signal, kind) in [
        ("goroutine 1 [running]:\nmain.listUsers(...)\n\t/app/server/main.go:9 +0x1d\nclient.FetchUsers()\n\t/app/client/client.go:9 +0x2a", "stacktrace"),
        ("server/main.go\nclient/client.go", "diff"),
    ] {
        let seeds = m.resolve_signal(signal, kind);
        assert!(seeds.len() >= 2, "{kind}: the signal resolves both files");
        let mut config = CODE_PROFILE.tables.activation_config(None);
        config.direction = Direction::Undirected;
        config.top_k = usize::MAX;
        let scores: HashMap<NodeId, f64> = m.activate(&seeds, &config).scores.into_iter().collect();
        let legacy: Vec<(u64, u64, bool)> = seeds
            .iter()
            .map(|id| (id.0, scores.get(id).copied().unwrap_or(0.0).to_bits(), live.contains(id)))
            .collect();
        let got: Vec<(u64, u64, bool)> = resolve_signal_located(&m, signal, kind, None, None)
            .results
            .iter()
            .map(|r| (r.id, r.score.to_bits(), r.live))
            .collect();
        assert_eq!(got, legacy, "{kind}: resolution order and scores unchanged");
        // top_k keeps the resolution-order prefix.
        let one: Vec<u64> =
            resolve_signal_located(&m, signal, kind, Some(1), None).results.iter().map(|r| r.id).collect();
        assert_eq!(one, vec![seeds[0].0], "{kind}: top_k 1 is the first resolved seed");
    }
}
