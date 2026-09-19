//! Scale gate for `MergedGraph::blast_radius` (LD.15b).
//!
//! The blast walk used to scan the whole edge list for every node it
//! dequeued - O(reached x E), 1.6e9 edge checks on a 40,000-node chain. It now
//! walks `repo_graph_activation::algo::reach::bfs` over a CSR index built once
//! per call, so the walk is linear and the PPR ranking (O(iterations x E))
//! dominates. The budget covers both and sits far below what the scan loop
//! took in a debug build, so the test fails on the quadratic walk without
//! being timing-flaky on the linear one. Public API only.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use repo_graph_code_domain::profile::CODE_TABLES;
use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, Node, NodeId, RepoId};
use repo_graph_graph::{MergedGraph, Reach, RepoGraph, SymbolTable};

const N: usize = 40_000;
const BUDGET: Duration = Duration::from_millis(1_500);

/// `n0 -> n1 -> ... -> n{n-1}` over CALLS; returns the merged graph and its
/// ids in chain order.
fn chain(n: usize) -> (MergedGraph, Vec<NodeId>) {
    let r = RepoId::from_canonical("test://blast-scale");
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
    (MergedGraph::new(vec![g]), ids)
}

#[test]
fn blast_radius_forward_40k_chain() {
    let (m, ids) = chain(N);
    let t = Instant::now();
    let hits = m.blast_radius(ids[0], Reach::Forward, usize::MAX, &CODE_TABLES);
    let took = t.elapsed();
    assert_eq!(hits.len(), N - 1);
    let last = hits.iter().find(|h| h.id == ids[N - 1]).expect("the chain's tail is in the radius");
    assert_eq!(last.depth, N - 1, "depth is the hop count along the chain");
    assert_eq!(last.reason, edge_category::CALLS);
    assert!(took < BUDGET, "blast_radius over a {N}-node chain took {took:?} (budget {BUDGET:?})");
}

#[test]
fn blast_radius_depth_and_direction_on_a_short_chain() {
    // n0 -> n1 -> n2 -> n3 -> n4: from n2, depth 1 reaches one hop each way.
    let (m, ids) = chain(5);
    let depth_of = |reach, depth| {
        let mut got: Vec<(NodeId, usize)> =
            m.blast_radius(ids[2], reach, depth, &CODE_TABLES).iter().map(|h| (h.id, h.depth)).collect();
        got.sort_by_key(|(id, _)| id.0);
        got
    };
    let sorted = |mut v: Vec<(NodeId, usize)>| {
        v.sort_by_key(|(id, _)| id.0);
        v
    };
    assert_eq!(depth_of(Reach::Forward, 1), vec![(ids[3], 1)]);
    assert_eq!(depth_of(Reach::Backward, 1), vec![(ids[1], 1)]);
    assert_eq!(depth_of(Reach::Both, 1), sorted(vec![(ids[1], 1), (ids[3], 1)]));
    assert_eq!(
        depth_of(Reach::Both, usize::MAX),
        sorted(vec![(ids[0], 2), (ids[1], 1), (ids[3], 1), (ids[4], 2)])
    );
    assert!(depth_of(Reach::Forward, 0).is_empty(), "depth 0 reaches nothing");
}
