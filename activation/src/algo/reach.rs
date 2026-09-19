//! Reachability over an [`Adjacency`]: breadth-first walks from seeds, the
//! closure they reach, and which candidate sources reach a sink.
//!
//! Every walk is O(V + E) over the index, with a `Vec<bool>` visited set over
//! the dense index. Results are exactly what the edge-scan loops they replaced
//! returned (the tests hold those loops as the oracle): the same nodes, in the
//! same discovery order, each with the same depth, first-reach category and
//! parent.

use std::collections::{HashSet, VecDeque};

use repo_graph_core::{EdgeCategoryId, NodeId};

use super::{Adjacency, Inc, Walk};

/// A node a walk reached: its depth in hops from the nearest seed, the
/// category of the edge that first reached it, and the node it was reached
/// from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Reached {
    pub id: NodeId,
    pub depth: usize,
    pub via: EdgeCategoryId,
    pub parent: NodeId,
}

/// A breadth-first walk's result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Bfs {
    /// Every reached node in discovery order; the seeds are not in it.
    pub reached: Vec<Reached>,
    /// Incidences examined: at most two per kept edge, whatever the walk.
    pub scanned: usize,
}

/// Breadth-first walk from `seeds` along `walk`. The seeds start visited at
/// depth 0; a node at depth `>= max_depth` is reached but not expanded, so
/// `max_depth = 0` reaches nothing. A seed the index does not hold is visited
/// but has no edges.
pub fn bfs(adj: &Adjacency, seeds: &[NodeId], walk: Walk, max_depth: usize) -> Bfs {
    let mut visited = vec![false; adj.len()];
    let mut queue: VecDeque<(u32, usize)> = VecDeque::new();
    for &s in seeds {
        if let Some(ix) = adj.index_of(s)
            && !visited[ix as usize]
        {
            visited[ix as usize] = true;
            queue.push_back((ix, 0));
        }
    }
    let mut out = Bfs::default();
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let mut step = |inc: &Inc, out: &mut Bfs| {
            out.scanned += 1;
            let next = inc.other as usize;
            if !visited[next] {
                visited[next] = true;
                out.reached.push(Reached {
                    id: adj.id(inc.other),
                    depth: depth + 1,
                    via: inc.category,
                    parent: adj.id(node),
                });
                queue.push_back((inc.other, depth + 1));
            }
        };
        match walk {
            Walk::Forward => adj.outgoing(node).iter().for_each(|inc| step(inc, &mut out)),
            Walk::Backward => adj.incoming(node).iter().for_each(|inc| step(inc, &mut out)),
            Walk::Both => {
                // One pass in edge order: the two lists merged by edge
                // position; a self-loop sits on both and is taken once, out.
                let (outs, ins) = (adj.outgoing(node), adj.incoming(node));
                let (mut i, mut j) = (0, 0);
                while i < outs.len() || j < ins.len() {
                    let take_out = j == ins.len() || (i < outs.len() && outs[i].edge <= ins[j].edge);
                    if take_out {
                        if j < ins.len() && outs[i].edge == ins[j].edge {
                            j += 1;
                        }
                        step(&outs[i], &mut out);
                        i += 1;
                    } else {
                        step(&ins[j], &mut out);
                        j += 1;
                    }
                }
            }
        }
    }
    out
}

/// The seeds plus everything `walk` reaches from them, at any depth.
pub fn reachable(adj: &Adjacency, seeds: &[NodeId], walk: Walk) -> HashSet<NodeId> {
    let mut set: HashSet<NodeId> = seeds.iter().copied().collect();
    set.extend(bfs(adj, seeds, walk, usize::MAX).reached.iter().map(|r| r.id));
    set
}

/// The `sources` that reach `sink` within `max_depth` hops, in `sources`
/// order: a backward walk from `sink` that stops once every distinct source
/// is hit. The sink itself is never a hit, and a source the index does not
/// hold is never hit (so it also keeps the walk from stopping early).
pub fn reachable_by(adj: &Adjacency, sink: NodeId, sources: &[NodeId], max_depth: usize) -> Vec<NodeId> {
    if sources.is_empty() {
        return Vec::new();
    }
    let wanted = sources.iter().collect::<HashSet<_>>().len();
    let mut target = vec![false; adj.len()];
    for s in sources {
        if let Some(ix) = adj.index_of(*s) {
            target[ix as usize] = true;
        }
    }
    let mut visited = vec![false; adj.len()];
    let mut hit = vec![false; adj.len()];
    let mut hits = 0usize;
    let mut queue: VecDeque<(u32, usize)> = VecDeque::new();
    if let Some(ix) = adj.index_of(sink) {
        visited[ix as usize] = true;
        queue.push_back((ix, 0));
    }
    while let Some((node, depth)) = queue.pop_front() {
        if hits == wanted {
            break;
        }
        if depth >= max_depth {
            continue;
        }
        for inc in adj.incoming(node) {
            let next = inc.other as usize;
            if !visited[next] {
                visited[next] = true;
                if target[next] {
                    hit[next] = true;
                    hits += 1;
                }
                queue.push_back((inc.other, depth + 1));
            }
        }
    }
    sources
        .iter()
        .copied()
        .filter(|s| adj.index_of(*s).is_some_and(|ix| hit[ix as usize]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algo::{CategorySet, ToyGraph, toy_edge};
    use repo_graph_core::Edge;

    /// The walks this module replaced, verbatim at 3193854, over an edge list.
    mod oracle {
        use std::collections::{HashSet, VecDeque};

        use repo_graph_core::{Edge, EdgeCategoryId, NodeId};

        /// graph/src/blast.rs `MergedGraph::blast_radius`'s BFS, generalised
        /// from one seed to a seed list, recording discovery order (its
        /// nested `if let` / `if` written as one let-chain).
        pub fn blast(
            edges: &[Edge],
            allow: &HashSet<EdgeCategoryId>,
            seeds: &[NodeId],
            fwd: bool,
            bwd: bool,
            max_depth: usize,
        ) -> Vec<(NodeId, usize, EdgeCategoryId, NodeId)> {
            let edges: Vec<&Edge> = edges.iter().collect();
            let mut order = Vec::new();
            let mut visited: HashSet<NodeId> = seeds.iter().copied().collect();
            let mut queue: VecDeque<(NodeId, usize)> = seeds.iter().map(|&s| (s, 0)).collect();
            while let Some((node, depth)) = queue.pop_front() {
                if depth >= max_depth {
                    continue;
                }
                for e in &edges {
                    if !allow.contains(&e.category) {
                        continue;
                    }
                    let next = if fwd && e.from == node {
                        Some(e.to)
                    } else if bwd && e.to == node {
                        Some(e.from)
                    } else {
                        None
                    };
                    if let Some(next) = next
                        && visited.insert(next)
                    {
                        order.push((next, depth + 1, e.category, node));
                        queue.push_back((next, depth + 1));
                    }
                }
            }
            order
        }

        /// graph/src/traversal.rs `RepoGraph::bfs`.
        pub fn bfs(edges: &[Edge], start: NodeId, follow: &[EdgeCategoryId], max_depth: usize) -> Vec<NodeId> {
            let allow: HashSet<EdgeCategoryId> = follow.iter().copied().collect();
            let mut visited: HashSet<NodeId> = HashSet::from([start]);
            let mut out = Vec::new();
            let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(start, 0)]);
            while let Some((node, depth)) = queue.pop_front() {
                if depth >= max_depth {
                    continue;
                }
                for e in edges.iter().filter(|e| e.from == node) {
                    if !allow.contains(&e.category) {
                        continue;
                    }
                    if visited.insert(e.to) {
                        out.push(e.to);
                        queue.push_back((e.to, depth + 1));
                    }
                }
            }
            out
        }

        /// graph/src/traversal.rs `RepoGraph::predecessors`.
        pub fn predecessors(edges: &[Edge], sink: NodeId, follow: &[EdgeCategoryId], max_depth: usize) -> Vec<NodeId> {
            let allow: HashSet<EdgeCategoryId> = follow.iter().copied().collect();
            let mut visited: HashSet<NodeId> = HashSet::from([sink]);
            let mut out = Vec::new();
            let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(sink, 0)]);
            while let Some((node, depth)) = queue.pop_front() {
                if depth >= max_depth {
                    continue;
                }
                for e in edges.iter().filter(|e| e.to == node) {
                    if !allow.contains(&e.category) {
                        continue;
                    }
                    if visited.insert(e.from) {
                        out.push(e.from);
                        queue.push_back((e.from, depth + 1));
                    }
                }
            }
            out
        }

        /// graph/src/traversal.rs `RepoGraph::reachable_by`.
        pub fn reachable_by(
            edges: &[Edge],
            sink: NodeId,
            sources: &[NodeId],
            follow: &[EdgeCategoryId],
            max_depth: usize,
        ) -> Vec<NodeId> {
            if sources.is_empty() {
                return Vec::new();
            }
            let allow: HashSet<EdgeCategoryId> = follow.iter().copied().collect();
            let target_set: HashSet<NodeId> = sources.iter().copied().collect();
            let mut visited: HashSet<NodeId> = HashSet::from([sink]);
            let mut hit: HashSet<NodeId> = HashSet::new();
            let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(sink, 0)]);
            while let Some((node, depth)) = queue.pop_front() {
                if depth >= max_depth || hit.len() == target_set.len() {
                    if hit.len() == target_set.len() {
                        break;
                    }
                    continue;
                }
                for e in edges.iter().filter(|e| e.to == node) {
                    if !allow.contains(&e.category) {
                        continue;
                    }
                    if visited.insert(e.from) {
                        if target_set.contains(&e.from) {
                            hit.insert(e.from);
                        }
                        queue.push_back((e.from, depth + 1));
                    }
                }
            }
            sources.iter().copied().filter(|s| hit.contains(s)).collect()
        }
    }

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

    const CATEGORIES: u32 = 5;
    const DEPTHS: [usize; 5] = [0, 1, 2, 5, usize::MAX];

    /// One random graph: 1..=40 nodes (ids scattered, so id order is not index
    /// order; sometimes one repeated), 0..=120 edges over 5 categories with
    /// ~10% dangling endpoints, self-loops and parallel edges.
    struct Case {
        graph: ToyGraph,
        dangling: Vec<NodeId>,
        absent: NodeId,
    }

    fn scatter(salt: u64, i: u64) -> NodeId {
        NodeId(i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt)
    }

    fn case(rng: &mut Lcg) -> Case {
        let salt = rng.next();
        let n = 1 + rng.below(40);
        let mut nodes: Vec<NodeId> = (0..n).map(|i| scatter(salt, i)).collect();
        if rng.chance(20) {
            let again = nodes[rng.below(n) as usize];
            nodes.push(again);
        }
        let dangling: Vec<NodeId> = (0..3).map(|k| scatter(salt, 1_000 + k)).collect();
        let pick = |rng: &mut Lcg| {
            if rng.chance(10) {
                dangling[rng.below(dangling.len() as u64) as usize]
            } else {
                scatter(salt, rng.below(n))
            }
        };
        let m = rng.below(121);
        let mut edges: Vec<Edge> = Vec::new();
        for _ in 0..m {
            let category = 1 + rng.below(CATEGORIES as u64) as u32;
            let mut e = toy_edge(0, 0, category);
            if !edges.is_empty() && rng.chance(8) {
                // A parallel edge, same or another category.
                let prior = &edges[rng.below(edges.len() as u64) as usize];
                (e.from, e.to) = (prior.from, prior.to);
            } else {
                e.from = pick(rng);
                e.to = if rng.chance(5) { e.from } else { pick(rng) };
            }
            edges.push(e);
        }
        Case { graph: ToyGraph { nodes, edges }, dangling, absent: scatter(salt, 5_000) }
    }

    /// 1..=3 seeds: nodes, sometimes a dangling id, sometimes an id absent
    /// from the graph, sometimes a repeat.
    fn seeds(rng: &mut Lcg, c: &Case) -> Vec<NodeId> {
        let k = 1 + rng.below(3);
        (0..k)
            .map(|_| match rng.below(10) {
                0 => c.absent,
                1 => c.dangling[rng.below(c.dangling.len() as u64) as usize],
                _ => c.graph.nodes[rng.below(c.graph.nodes.len() as u64) as usize],
            })
            .collect()
    }

    /// Every category, then a random subset (possibly empty).
    fn category_sets(rng: &mut Lcg) -> Vec<Vec<EdgeCategoryId>> {
        let every: Vec<EdgeCategoryId> = (1..=CATEGORIES).map(EdgeCategoryId).collect();
        let some: Vec<EdgeCategoryId> = every.iter().copied().filter(|_| rng.chance(50)).collect();
        vec![every, some]
    }

    fn as_rows(b: &Bfs) -> Vec<(NodeId, usize, EdgeCategoryId, NodeId)> {
        b.reached.iter().map(|r| (r.id, r.depth, r.via, r.parent)).collect()
    }

    #[test]
    fn oracle_parity_bfs() {
        let mut rng = Lcg(0x1d_15a);
        let mut compared = 0usize;
        for _ in 0..300 {
            let c = case(&mut rng);
            for cats in category_sets(&mut rng) {
                let allow: HashSet<EdgeCategoryId> = cats.iter().copied().collect();
                let keep = if cats.len() == CATEGORIES as usize { CategorySet::all() } else { CategorySet::of(&cats) };
                let adj = Adjacency::build(&c.graph, &keep);
                for _ in 0..2 {
                    let s = seeds(&mut rng, &c);
                    for &max_depth in &DEPTHS {
                        for (walk, fwd, bwd) in
                            [(Walk::Forward, true, false), (Walk::Backward, false, true), (Walk::Both, true, true)]
                        {
                            let got = bfs(&adj, &s, walk, max_depth);
                            let want = oracle::blast(&c.graph.edges, &allow, &s, fwd, bwd, max_depth);
                            assert_eq!(as_rows(&got), want, "walk {walk:?} depth {max_depth} seeds {s:?}");
                            assert!(got.scanned <= 2 * adj.kept_edges());
                            compared += 1;
                        }
                        // RepoGraph::bfs / predecessors are single-seed walks.
                        let one = s[0];
                        let ids = |b: Bfs| b.reached.into_iter().map(|r| r.id).collect::<Vec<_>>();
                        assert_eq!(
                            ids(bfs(&adj, &[one], Walk::Forward, max_depth)),
                            oracle::bfs(&c.graph.edges, one, &cats, max_depth)
                        );
                        assert_eq!(
                            ids(bfs(&adj, &[one], Walk::Backward, max_depth)),
                            oracle::predecessors(&c.graph.edges, one, &cats, max_depth)
                        );
                    }
                }
            }
        }
        assert_eq!(compared, 300 * 2 * 2 * DEPTHS.len() * 3);
    }

    #[test]
    fn oracle_parity_reachable_by() {
        let mut rng = Lcg(0x5eed_ba5e);
        let mut nonempty = 0usize;
        for _ in 0..300 {
            let c = case(&mut rng);
            for cats in category_sets(&mut rng) {
                let adj = Adjacency::build(&c.graph, &CategorySet::of(&cats));
                for _ in 0..3 {
                    let sink = seeds(&mut rng, &c)[0];
                    // 0..=5 sources: nodes, dangling or absent ids, repeats,
                    // sometimes the sink itself.
                    let mut sources: Vec<NodeId> = Vec::new();
                    for _ in 0..rng.below(6) {
                        let s = if rng.chance(10) { sink } else { seeds(&mut rng, &c)[0] };
                        sources.push(s);
                    }
                    for &max_depth in &DEPTHS {
                        let got = reachable_by(&adj, sink, &sources, max_depth);
                        let want = oracle::reachable_by(&c.graph.edges, sink, &sources, &cats, max_depth);
                        assert_eq!(got, want, "sink {sink:?} sources {sources:?} depth {max_depth}");
                        nonempty += usize::from(!got.is_empty());
                    }
                }
            }
        }
        assert!(nonempty > 100, "the corpus must exercise hits, got {nonempty}");
    }

    #[test]
    fn reachable_is_seeds_plus_the_unbounded_closure() {
        let g = ToyGraph {
            nodes: vec![NodeId(1), NodeId(2), NodeId(3), NodeId(4)],
            edges: vec![toy_edge(1, 2, 1), toy_edge(2, 3, 1), toy_edge(4, 3, 1), toy_edge(3, 9, 1)],
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let fwd = reachable(&adj, &[NodeId(1), NodeId(77)], Walk::Forward);
        assert_eq!(fwd, HashSet::from([NodeId(1), NodeId(77), NodeId(2), NodeId(3), NodeId(9)]));
        let back = reachable(&adj, &[NodeId(3)], Walk::Backward);
        assert_eq!(back, HashSet::from([NodeId(3), NodeId(2), NodeId(1), NodeId(4)]));
    }

    #[test]
    fn scanned_is_linear() {
        const N: u64 = 40_000;
        let g = ToyGraph {
            nodes: (0..N).map(NodeId).collect(),
            edges: (1..N).map(|i| toy_edge(i - 1, i, 1)).collect(),
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        assert_eq!(adj.kept_edges(), (N - 1) as usize);
        for (seed, walk) in [(0, Walk::Forward), (N - 1, Walk::Backward), (N / 2, Walk::Both)] {
            let b = bfs(&adj, &[NodeId(seed)], walk, usize::MAX);
            assert_eq!(b.reached.len(), (N - 1) as usize, "{walk:?}");
            assert!(b.scanned <= 2 * adj.kept_edges(), "{walk:?} scanned {}", b.scanned);
        }
    }
}
