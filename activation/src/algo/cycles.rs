//! Cycles (LE.6a): the strongly-connected components of an [`Adjacency`] and
//! a shortest witness cycle through one of them.
//!
//! Category-agnostic like the rest of [`crate::algo`]: the caller picks the
//! categories that count when it builds the index ([`super::CategorySet`]),
//! which is how a consumer keeps import cycles and flow loops apart.
//!
//! [`strongly_connected`] is Tarjan's algorithm written iteratively (an
//! explicit frame stack, never recursion), so a 100,000-node call chain costs
//! heap, not thread stack. Both functions follow each node's kept outgoing
//! edges in edge order, the walk `reach` uses, and are O(V + E) time and O(V)
//! extra memory per call. Neither output depends on traversal order: ids are
//! sorted by value, and a witness is a shortest cycle whose hops are chosen by
//! edge order.

use std::collections::VecDeque;

use glia_core::{EdgeCategoryId, NodeId};

use super::Adjacency;

/// Dense-index marker for "not visited yet". The index never holds
/// `u32::MAX` ids, so no visit order reaches it.
const UNSEEN: u32 = u32::MAX;

/// Every non-trivial strongly-connected component of `adj`: two or more
/// nodes that each reach the others, or one node with an edge to itself.
///
/// Each component lists its ids sorted by value; the components are sorted by
/// their first id. A dangling edge endpoint the index holds is a member like
/// any node.
pub fn strongly_connected(adj: &Adjacency) -> Vec<Vec<NodeId>> {
    let n = adj.len();
    let mut order = vec![UNSEEN; n];
    let mut low = vec![0u32; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<u32> = Vec::new();
    // (vertex, position of its next outgoing edge to examine)
    let mut frames: Vec<(u32, u32)> = Vec::new();
    let mut next = 0u32;
    let mut found: Vec<Vec<NodeId>> = Vec::new();

    for root in 0..n as u32 {
        if order[root as usize] != UNSEEN {
            continue;
        }
        // The vertex to open a frame for: the root, then each tree edge's
        // target. One place numbers a vertex and pushes it.
        let mut enter = Some(root);
        loop {
            if let Some(v) = enter.take() {
                order[v as usize] = next;
                low[v as usize] = next;
                next += 1;
                stack.push(v);
                on_stack[v as usize] = true;
                frames.push((v, 0));
            }
            let Some(frame) = frames.last_mut() else {
                break;
            };
            let (v, pos) = *frame;
            let outs = adj.outgoing(v);
            if let Some(inc) = outs.get(pos as usize) {
                frame.1 += 1;
                let w = inc.other as usize;
                if order[w] == UNSEEN {
                    enter = Some(inc.other);
                } else if on_stack[w] {
                    low[v as usize] = low[v as usize].min(order[w]);
                }
                continue;
            }
            // Every edge of `v` examined: fold its lowlink into its parent,
            // then close its component if it is a root.
            frames.pop();
            if let Some(&(parent, _)) = frames.last() {
                low[parent as usize] = low[parent as usize].min(low[v as usize]);
            }
            if low[v as usize] != order[v as usize] {
                continue;
            }
            let mut members: Vec<NodeId> = Vec::new();
            while let Some(w) = stack.pop() {
                on_stack[w as usize] = false;
                members.push(adj.id(w));
                if w == v {
                    break;
                }
            }
            if members.len() >= 2 || outs.iter().any(|inc| inc.other == v) {
                members.sort_unstable_by_key(|id| id.0);
                found.push(members);
            }
        }
    }
    // Components are disjoint, so their first ids are distinct.
    found.sort_unstable_by_key(|c| c[0].0);
    found
}

/// A shortest cycle through `start` inside `component`, as hops
/// `(from, category, to)` in order: the first hop leaves `start`, the last
/// enters it, and each hop's `to` is the next hop's `from`.
///
/// A breadth-first walk from `start` along outgoing edges between members; the
/// first edge found back into `start` closes the cycle, so the cycle is a
/// shortest one through `start` and no longer than the component. Each hop
/// takes the category of the first edge (in edge order) that reached its
/// target. Empty when `start` is not a member, or when no cycle through it
/// stays inside `component` (a trivial component, or a set that is not
/// strongly connected).
pub fn witness_cycle(adj: &Adjacency, component: &[NodeId], start: NodeId) -> Vec<(NodeId, EdgeCategoryId, NodeId)> {
    let mut member = vec![false; adj.len()];
    for id in component {
        if let Some(ix) = adj.index_of(*id) {
            member[ix as usize] = true;
        }
    }
    let Some(s) = adj.index_of(start).filter(|&s| member[s as usize]) else {
        return Vec::new();
    };
    // parent[w] = (the node `w` was reached from, that edge's category)
    let mut parent: Vec<Option<(u32, EdgeCategoryId)>> = vec![None; adj.len()];
    let mut seen = vec![false; adj.len()];
    seen[s as usize] = true;
    let mut queue = VecDeque::from([s]);
    while let Some(v) = queue.pop_front() {
        for inc in adj.outgoing(v) {
            let w = inc.other;
            if !member[w as usize] {
                continue;
            }
            if w == s {
                let mut hops = vec![(adj.id(v), inc.category, start)];
                let mut at = v;
                while let Some((from, category)) = parent[at as usize] {
                    hops.push((adj.id(from), category, adj.id(at)));
                    at = from;
                }
                hops.reverse();
                return hops;
            }
            if !seen[w as usize] {
                seen[w as usize] = true;
                parent[w as usize] = Some((v, inc.category));
                queue.push_back(w);
            }
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::time::{Duration, Instant};

    use glia_core::Edge;

    use super::*;
    use crate::algo::reach::{bfs, reachable};
    use crate::algo::{CategorySet, ToyGraph, Walk, toy_edge};

    /// Deterministic PCG-style generator, so the corpus needs no dependency.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        fn below(&mut self, n: u64) -> u64 {
            if n == 0 { 0 } else { self.next() % n }
        }

        fn chance(&mut self, percent: u64) -> bool {
            self.below(100) < percent
        }
    }

    const CATEGORIES: u32 = 4;

    fn scatter(salt: u64, i: u64) -> NodeId {
        NodeId(i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt)
    }

    /// One random graph: 1..=40 nodes with scattered ids (id order is not
    /// index order), 0..=120 edges over 4 categories, with self-loops,
    /// parallel edges and ~5% dangling endpoints.
    fn random_graph(rng: &mut Lcg) -> ToyGraph {
        let salt = rng.next();
        let n = 1 + rng.below(40);
        let nodes: Vec<NodeId> = (0..n).map(|i| scatter(salt, i)).collect();
        let pick = |rng: &mut Lcg| {
            if rng.chance(5) { scatter(salt, 1_000 + rng.below(2)) } else { scatter(salt, rng.below(n)) }
        };
        let mut edges: Vec<Edge> = Vec::new();
        for _ in 0..rng.below(121) {
            let category = 1 + rng.below(CATEGORIES as u64) as u32;
            let mut e = toy_edge(0, 0, category);
            if !edges.is_empty() && rng.chance(8) {
                let prior = &edges[rng.below(edges.len() as u64) as usize];
                (e.from, e.to) = (prior.from, prior.to);
            } else {
                e.from = pick(rng);
                e.to = if rng.chance(6) { e.from } else { pick(rng) };
            }
            edges.push(e);
        }
        ToyGraph { nodes, edges }
    }

    /// Every category, or a random subset (possibly empty).
    fn random_keep(rng: &mut Lcg) -> CategorySet {
        if rng.chance(50) {
            return CategorySet::all();
        }
        let some: Vec<EdgeCategoryId> = (1..=CATEGORIES).map(EdgeCategoryId).filter(|_| rng.chance(60)).collect();
        CategorySet::of(&some)
    }

    /// Brute force: u ~ v iff each reaches the other; a singleton counts only
    /// with a kept self-loop in the edge list.
    fn oracle(g: &ToyGraph, keep: &CategorySet, adj: &Adjacency) -> BTreeSet<Vec<u64>> {
        let all: Vec<NodeId> = (0..adj.len() as u32).map(|i| adj.id(i)).collect();
        let reach: HashMap<NodeId, _> = all.iter().map(|&u| (u, reachable(adj, &[u], Walk::Forward))).collect();
        let mut out = BTreeSet::new();
        for &u in &all {
            let mut comp: Vec<u64> =
                all.iter().filter(|&&v| reach[&u].contains(&v) && reach[&v].contains(&u)).map(|v| v.0).collect();
            comp.sort_unstable();
            let self_loop = g.edges.iter().any(|e| e.from == u && e.to == u && keep.contains(e.category));
            if comp.len() >= 2 || self_loop {
                out.insert(comp);
            }
        }
        out
    }

    fn as_sets(comps: &[Vec<NodeId>]) -> BTreeSet<Vec<u64>> {
        comps.iter().map(|c| c.iter().map(|id| id.0).collect()).collect()
    }

    /// Asserts `hops` is a shortest cycle through `start` inside `comp`, each
    /// hop carrying the category of the first kept edge (in edge order)
    /// between its ends.
    fn assert_real_cycle(
        g: &ToyGraph,
        keep: &CategorySet,
        adj: &Adjacency,
        comp: &[NodeId],
        start: NodeId,
        hops: &[(NodeId, EdgeCategoryId, NodeId)],
    ) {
        assert!(!hops.is_empty(), "a non-trivial component has a witness from {start:?}");
        assert!(hops.len() <= comp.len(), "witness {hops:?} longer than component {comp:?}");
        assert_eq!(hops[0].0, start);
        assert_eq!(hops[hops.len() - 1].2, start);
        for pair in hops.windows(2) {
            assert_eq!(pair[0].2, pair[1].0, "hops chain: {hops:?}");
        }
        for &(from, category, to) in hops {
            assert!(comp.contains(&from) && comp.contains(&to), "hop {from:?}->{to:?} leaves the component");
            let first = g.edges.iter().find(|e| e.from == from && e.to == to && keep.contains(e.category));
            assert_eq!(first.map(|e| e.category), Some(category), "hop {from:?}->{to:?} is not the first edge");
        }
        // Shortest: 1 + the least forward depth of a node with an edge into
        // `start` (0 for `start` itself). Any cycle through `start` lies in
        // its component, so the unrestricted walk gives the same length.
        let depth: HashMap<NodeId, usize> = bfs(adj, &[start], Walk::Forward, usize::MAX)
            .reached
            .iter()
            .map(|r| (r.id, r.depth))
            .chain([(start, 0)])
            .collect();
        let shortest = g
            .edges
            .iter()
            .filter(|e| e.to == start && keep.contains(e.category))
            .filter_map(|e| depth.get(&e.from))
            .min()
            .map(|d| d + 1);
        assert_eq!(Some(hops.len()), shortest, "witness {hops:?} is not a shortest cycle");
    }

    #[test]
    fn oracle_parity() {
        let mut rng = Lcg(0x1e_6a);
        let (mut components, mut singletons, mut witnessed) = (0usize, 0usize, 0usize);
        for _ in 0..300 {
            let g = random_graph(&mut rng);
            let keep = random_keep(&mut rng);
            let adj = Adjacency::build(&g, &keep);
            let got = strongly_connected(&adj);
            assert_eq!(as_sets(&got), oracle(&g, &keep, &adj), "graph {:?}", g.edges);
            components += got.len();
            singletons += got.iter().filter(|c| c.len() == 1).count();
            for comp in &got {
                for &start in comp {
                    let hops = witness_cycle(&adj, comp, start);
                    assert_real_cycle(&g, &keep, &adj, comp, start, &hops);
                    witnessed += 1;
                }
            }
        }
        assert!(
            components > 100 && singletons > 20,
            "corpus too thin: {components} components, {singletons} self-loops"
        );
        assert!(witnessed > components);
    }

    #[test]
    fn chain_100k_with_back_edge_is_one_component_no_overflow() {
        const N: u64 = 100_000;
        let mut edges: Vec<Edge> = (1..N).map(|i| toy_edge(i - 1, i, 1)).collect();
        edges.push(toy_edge(N - 1, 0, 2));
        let g = ToyGraph { nodes: (0..N).map(NodeId).collect(), edges };
        let adj = Adjacency::build(&g, &CategorySet::all());

        let t = Instant::now();
        let comps = strongly_connected(&adj);
        let hops = witness_cycle(&adj, &comps[0], NodeId(0));
        let took = t.elapsed();

        assert_eq!(comps.len(), 1);
        assert_eq!(comps[0].len(), N as usize);
        assert!(comps[0].iter().zip(0..N).all(|(id, i)| id.0 == i));
        assert_eq!(hops.len(), N as usize);
        assert_eq!(hops[0], (NodeId(0), EdgeCategoryId(1), NodeId(1)));
        assert_eq!(hops[hops.len() - 1], (NodeId(N - 1), EdgeCategoryId(2), NodeId(0)));
        assert!(took < Duration::from_secs(1), "scc + witness over {N} nodes took {took:?}");
    }

    #[test]
    fn self_loop_is_a_component() {
        let g = ToyGraph {
            nodes: vec![NodeId(1), NodeId(2), NodeId(3)],
            edges: vec![toy_edge(1, 2, 1), toy_edge(2, 2, 3), toy_edge(2, 2, 1), toy_edge(2, 3, 1)],
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let comps = strongly_connected(&adj);
        assert_eq!(comps, vec![vec![NodeId(2)]]);
        assert_eq!(witness_cycle(&adj, &comps[0], NodeId(2)), vec![(NodeId(2), EdgeCategoryId(3), NodeId(2))]);
        // The self-loop's categories dropped: node 2 is trivial again.
        let adj = Adjacency::build(&g, &CategorySet::of(&[EdgeCategoryId(2)]));
        assert!(strongly_connected(&adj).is_empty());
    }

    #[test]
    fn dag_has_none() {
        // A diamond with a parallel edge and a dangling sink; no back edge.
        let g = ToyGraph {
            nodes: vec![NodeId(1), NodeId(2), NodeId(3), NodeId(4)],
            edges: vec![
                toy_edge(1, 2, 1),
                toy_edge(1, 3, 1),
                toy_edge(2, 4, 1),
                toy_edge(3, 4, 2),
                toy_edge(3, 4, 1),
                toy_edge(4, 9, 1),
            ],
        };
        assert!(strongly_connected(&Adjacency::build(&g, &CategorySet::all())).is_empty());
        assert!(strongly_connected(&Adjacency::default()).is_empty());
    }

    #[test]
    fn witness_is_a_real_cycle() {
        // 1 -> 2 -> 3 -> 1 and a shortcut 2 -> 1; a parallel 1 -> 2 of a
        // later category. From 1 the shortest cycle is 1 -> 2 -> 1.
        let g = ToyGraph {
            nodes: vec![NodeId(1), NodeId(2), NodeId(3)],
            edges: vec![toy_edge(1, 2, 4), toy_edge(2, 3, 1), toy_edge(3, 1, 2), toy_edge(1, 2, 5), toy_edge(2, 1, 3)],
        };
        let keep = CategorySet::all();
        let adj = Adjacency::build(&g, &keep);
        let comps = strongly_connected(&adj);
        assert_eq!(comps, vec![vec![NodeId(1), NodeId(2), NodeId(3)]]);
        let from_1 = witness_cycle(&adj, &comps[0], NodeId(1));
        assert_eq!(from_1, vec![(NodeId(1), EdgeCategoryId(4), NodeId(2)), (NodeId(2), EdgeCategoryId(3), NodeId(1))]);
        let from_3 = witness_cycle(&adj, &comps[0], NodeId(3));
        assert_eq!(
            from_3,
            vec![
                (NodeId(3), EdgeCategoryId(2), NodeId(1)),
                (NodeId(1), EdgeCategoryId(4), NodeId(2)),
                (NodeId(2), EdgeCategoryId(1), NodeId(3)),
            ]
        );
        for start in [NodeId(1), NodeId(2), NodeId(3)] {
            assert_real_cycle(&g, &keep, &adj, &comps[0], start, &witness_cycle(&adj, &comps[0], start));
        }
    }

    #[test]
    fn witness_outside_component_is_empty() {
        // Two components {1, 2} and {3, 4}, joined one way by 2 -> 3.
        let g = ToyGraph {
            nodes: vec![NodeId(1), NodeId(2), NodeId(3), NodeId(4), NodeId(5)],
            edges: vec![toy_edge(1, 2, 1), toy_edge(2, 1, 1), toy_edge(2, 3, 1), toy_edge(3, 4, 1), toy_edge(4, 3, 1)],
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let comps = strongly_connected(&adj);
        assert_eq!(comps, vec![vec![NodeId(1), NodeId(2)], vec![NodeId(3), NodeId(4)]]);
        assert!(witness_cycle(&adj, &comps[0], NodeId(3)).is_empty(), "a node of another component");
        assert!(witness_cycle(&adj, &comps[0], NodeId(5)).is_empty(), "a node in no component");
        assert!(witness_cycle(&adj, &comps[0], NodeId(77)).is_empty(), "an id the index does not hold");
        // A member set that is not strongly connected: 2 -> 3 never returns.
        assert!(witness_cycle(&adj, &[NodeId(2), NodeId(3)], NodeId(2)).is_empty());
        // The walk stays inside the given members: without 2, 1 has no cycle.
        assert!(witness_cycle(&adj, &[NodeId(1)], NodeId(1)).is_empty());
    }

    #[test]
    fn output_sorted() {
        // Ids chosen so dense-index order, edge order and id order all differ.
        let nodes = [50u64, 7, 31, 90, 12, 64, 3];
        let edges = vec![
            toy_edge(50, 7, 1),
            toy_edge(7, 50, 1),
            toy_edge(31, 90, 1),
            toy_edge(90, 12, 1),
            toy_edge(12, 31, 1),
            toy_edge(64, 64, 1),
            toy_edge(3, 50, 1),
            toy_edge(90, 7, 1),
        ];
        let want = vec![vec![NodeId(7), NodeId(50)], vec![NodeId(12), NodeId(31), NodeId(90)], vec![NodeId(64)]];
        let forward = ToyGraph { nodes: nodes.iter().copied().map(NodeId).collect(), edges: edges.clone() };
        let reversed = ToyGraph {
            nodes: nodes.iter().rev().copied().map(NodeId).collect(),
            edges: edges.into_iter().rev().collect(),
        };
        for g in [&forward, &reversed] {
            assert_eq!(strongly_connected(&Adjacency::build(g, &CategorySet::all())), want);
        }
        // The same holds across the random corpus: every component sorted,
        // components ordered by first id.
        let mut rng = Lcg(0x50_77ed);
        for _ in 0..100 {
            let g = random_graph(&mut rng);
            let comps = strongly_connected(&Adjacency::build(&g, &CategorySet::all()));
            assert!(comps.iter().all(|c| c.windows(2).all(|w| w[0].0 < w[1].0)));
            assert!(comps.windows(2).all(|w| w[0][0].0 < w[1][0].0));
        }
    }
}
