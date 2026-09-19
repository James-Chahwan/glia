//! Scale gate for the `RepoGraph` traversal primitives (LD.15a).
//!
//! `bfs` and `predecessors` used to scan the whole edge list for every node
//! they dequeued — O(visited x E), 1.6e9 edge checks on a 40,000-node chain.
//! They now walk a CSR index (`glia_activation::algo::Adjacency`), so a
//! walk over a 40k chain is linear. The budget below sits well above what the
//! index needs in a debug build and well below what the scan loop took, so the
//! test fails on the quadratic walk without being timing-flaky on the linear
//! one. It uses only the public `RepoGraph` API.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use glia_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use glia_core::{Confidence, Edge, Node, NodeId, RepoId};
use glia_graph::{RepoGraph, SymbolTable};

const N: usize = 40_000;
const BUDGET: Duration = Duration::from_secs(1);

/// `n0 -> n1 -> ... -> n{N-1}` over CALLS; returns the graph and its ids in
/// chain order.
fn chain(n: usize) -> (RepoGraph, Vec<NodeId>) {
    let r = RepoId::from_canonical("test://traversal-scale");
    let mut nav = CodeNav::default();
    let ids: Vec<NodeId> = (0..n)
        .map(|i| {
            let qname = format!("m::n{i}");
            let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, &qname);
            nav.record(id, &format!("n{i}"), &qname, node_kind::FUNCTION, None);
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
    (g, ids)
}

#[test]
fn predecessors_and_bfs_on_40k_chain_are_linear() {
    let (g, ids) = chain(N);
    let first = ids[0];
    let last = ids[N - 1];

    let t = Instant::now();
    let preds = g.predecessors(last, &[edge_category::CALLS], usize::MAX);
    let took = t.elapsed();
    assert_eq!(preds.len(), N - 1);
    // Backward BFS discovers the chain from the sink outwards.
    assert_eq!(preds[0], ids[N - 2]);
    assert_eq!(preds[N - 2], first);
    assert!(took < BUDGET, "predecessors over a {N}-node chain took {took:?} (budget {BUDGET:?})");

    let t = Instant::now();
    let reached = g.bfs(first, &[edge_category::CALLS], usize::MAX);
    let took = t.elapsed();
    assert_eq!(reached.len(), N - 1);
    assert_eq!(reached, ids[1..].to_vec());
    assert!(took < BUDGET, "bfs over a {N}-node chain took {took:?} (budget {BUDGET:?})");

    let t = Instant::now();
    let hit = g.reachable_by(last, &[ids[N / 2], first], &[edge_category::CALLS], usize::MAX);
    let took = t.elapsed();
    assert_eq!(hit, vec![ids[N / 2], first]);
    assert!(took < BUDGET, "reachable_by over a {N}-node chain took {took:?} (budget {BUDGET:?})");
}
