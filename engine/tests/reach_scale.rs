//! Scale gate for the liveness walk (LD.15b).
//!
//! `entrypoint_reachable` runs on every blast-radius query (its `live` flag).
//! It used to scan the whole edge list for every node it dequeued -
//! O(live x E), 1.6e9 edge checks on a 40,000-node live chain (6.7 s in a
//! debug build). It now walks `repo_graph_activation::algo::reach` over a CSR
//! index built once per call, so the same chain is linear. The budget sits far
//! above what the index needs in a debug build and far below what the scan
//! took, so the test fails on the quadratic walk without being timing-flaky on
//! the linear one. Public API only.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, Node, NodeId, RepoId};
use repo_graph_engine::entrypoint_reachable;
use repo_graph_graph::{MergedGraph, RepoGraph, SymbolTable};

const N: usize = 40_000;
const BUDGET: Duration = Duration::from_secs(1);

/// FUNCTION `head` -> n1 -> ... -> n{n-1} over CALLS. With `head = "main"`
/// the head is the only entrypoint (the code profile's named entry), so the
/// whole chain is live through the carry walk alone.
fn chain(n: usize, head: &str) -> (MergedGraph, Vec<NodeId>) {
    let r = RepoId::from_canonical("test://reach-scale");
    let mut nav = CodeNav::default();
    let ids: Vec<NodeId> = (0..n)
        .map(|i| {
            let name = if i == 0 { head.to_string() } else { format!("n{i}") };
            let qname = format!("m::{name}");
            let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, &qname);
            nav.record(id, &name, &qname, node_kind::FUNCTION, None);
            id
        })
        .collect();
    let nodes = ids
        .iter()
        .map(|&id| Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] })
        .collect();
    let edges = ids
        .windows(2)
        .map(|w| Edge {
            from: w[0],
            to: w[1],
            category: edge_category::CALLS,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        })
        .collect();
    let g = RepoGraph {
        repo: r,
        nodes,
        edges,
        nav,
        symbols: SymbolTable::default(),
        unresolved_calls: vec![],
        unresolved_refs: vec![],
        properties: HashSet::new(),
    };
    (MergedGraph::new(vec![g]), ids)
}

#[test]
fn entrypoint_reachable_40k_chain_under_budget() {
    let (m, ids) = chain(N, "main");
    let t = Instant::now();
    let live = entrypoint_reachable(&m);
    let took = t.elapsed();
    assert_eq!(live.len(), N);
    assert!(live.contains(&ids[0]) && live.contains(&ids[N - 1]));
    assert!(took < BUDGET, "entrypoint_reachable over a {N}-node live chain took {took:?} (budget {BUDGET:?})");
}

#[test]
fn a_chain_with_no_entrypoint_is_dead() {
    // The same chain headed by `n0`: no `main`, no route, nothing live.
    let (m, _) = chain(3, "n0");
    assert!(entrypoint_reachable(&m).is_empty());
}
