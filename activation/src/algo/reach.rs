//! Reachability over an [`Adjacency`]: breadth-first walks from seeds, the
//! closure they reach, and which candidate sources reach a sink.
//!
//! Every walk is O(V + E) over the index, with a `Vec<bool>` visited set over
//! the dense index. Results are exactly what the edge-scan loops they replaced
//! returned (the tests hold those loops as the oracle): the same nodes, in the
//! same discovery order, each with the same depth, first-reach category and
//! parent.
//!
//! [`bfs_levels`] is the levelled walk (DA.1): the caller gives every kept edge
//! and every seed a small level, and pass `L` walks only the edges and starts
//! only the seeds of level `<= L`, so each reached node reports the first level
//! whose pass reached it, in O(levels x (V + E)) over one index instead of one
//! index per level. A membership mask keeps a walk inside a node set, a rank
//! orders each depth layer so a node's parent is the lowest-ranked node one
//! hop up rather than the first discovered, and `plus` reports a seed that a
//! walk of one or more hops reaches (the transitive closure `+`). Its first
//! consumer is glia-rules' closure builtins (`reaches` / `scc`, DA.11), where a
//! level is an evidence tier.

use std::collections::{HashSet, VecDeque};

use glia_core::{EdgeCategoryId, NodeId};

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
        each_incidence(adj, node, walk, |inc| {
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
        });
    }
    out
}

/// Hand `step` every incidence of `node` that `walk` follows, in edge order:
/// the out-list, the in-list, or ([`Walk::Both`]) the two merged by edge
/// position with a self-loop (on both lists) taken once, as forward. The one
/// step order [`bfs`] and [`bfs_levels`] share.
fn each_incidence(adj: &Adjacency, node: u32, walk: Walk, mut step: impl FnMut(&Inc)) {
    match walk {
        Walk::Forward => adj.outgoing(node).iter().for_each(step),
        Walk::Backward => adj.incoming(node).iter().for_each(step),
        Walk::Both => {
            let (outs, ins) = (adj.outgoing(node), adj.incoming(node));
            let (mut i, mut j) = (0, 0);
            while i < outs.len() || j < ins.len() {
                let take_out = j == ins.len() || (i < outs.len() && outs[i].edge <= ins[j].edge);
                if take_out {
                    if j < ins.len() && outs[i].edge == ins[j].edge {
                        j += 1;
                    }
                    step(&outs[i]);
                    i += 1;
                } else {
                    step(&ins[j]);
                    j += 1;
                }
            }
        }
    }
}

/// One node a levelled walk reached, at the first level whose pass reached it,
/// with that pass's depth, reaching category, parent and edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LevelReached {
    pub id: NodeId,
    /// The first pass that reached the node.
    pub level: u8,
    /// Hops from the nearest started seed, in that pass.
    pub depth: usize,
    pub via: EdgeCategoryId,
    pub parent: NodeId,
    /// Position of the reaching edge among the index's kept edges (the global
    /// edge order filtered by the kept categories): the index `edge_level` is
    /// read at.
    pub kept_edge: u32,
}

/// A levelled walk's result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LevelBfs {
    /// Every reported node once, at its smallest level: level ascending, in
    /// discovery order within a level.
    pub reached: Vec<LevelReached>,
    /// The seeds the walk started from (indexed, inside the mask, at a level
    /// below `levels`), each once at its lowest given level, in the order of
    /// its first occurrence in the input.
    pub seeds: Vec<(NodeId, u8)>,
    /// Incidences examined over every pass: at most four per kept edge per
    /// level (two, unless `plus` expands a seed a second time).
    pub scanned: usize,
}

/// How [`bfs_levels`] walks. Build one with [`LevelWalk::new`] and set the
/// optional fields on it.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct LevelWalk<'a> {
    pub walk: Walk,
    /// Passes `0..levels`.
    pub levels: u8,
    /// As in [`bfs`]: a node at depth `>= max_depth` is reached but not
    /// expanded.
    pub max_depth: usize,
    /// Dense-index mask (`len == adj.len()`): a node outside is never entered
    /// or reported, and a seed outside never starts.
    pub within: Option<&'a [bool]>,
    /// Dense-index rank (`len == adj.len()`): each depth layer is expanded in
    /// ascending (rank, dense index) order, so a node's parent is the
    /// lowest-ranked node one layer up with a walkable edge to it.
    pub parent_rank: Option<&'a [u32]>,
    /// The transitive-closure `+`: seeds start the walk unvisited, so a seed
    /// is reported when one or more hops reach it (itself, on a cycle).
    pub plus: bool,
}

impl<'a> LevelWalk<'a> {
    /// `levels` passes along `walk`: unbounded depth, no mask, no rank, seeds
    /// start visited.
    pub fn new(walk: Walk, levels: u8) -> Self {
        Self { walk, levels, max_depth: usize::MAX, within: None, parent_rank: None, plus: false }
    }
}

/// Levelled multi-source breadth-first walk. Pass `L` (for `L` in
/// `0..how.levels`) is a plain BFS that starts every seed of level `<= L` at
/// depth 0 and walks the kept edge at position `k` only when
/// `edge_level[k] <= L` (`u8::MAX` therefore means never). A node first
/// reached in pass `L` is reported at level `L` with that pass's depth, via,
/// parent and kept edge; a node reported in an earlier pass is walked again
/// but not reported again, so `reached` is level-ascending.
///
/// With `plus = false` a seed starts visited and is reported only when an
/// earlier pass reached it first. With `plus = true` it is reported (once)
/// when a walk of one or more hops reaches it, and a seed reached that way is
/// expanded a second time, which finds nothing new. Without `parent_rank`,
/// `levels = 1` with every edge at level 0 is exactly [`bfs`]'s walk. A node's
/// parent, depth and kept edge are those of its FIRST pass, so following
/// reported parents can mix passes: a shortest path at one level `L` is a
/// one-level walk with `edge_level' = 0` for every level `<= L` and
/// `u8::MAX` otherwise.
///
/// An `edge_level`, mask or rank of the wrong length returns an empty result
/// and prints `[algo] bfs_levels: <what> has <n> entries for <m>`.
pub fn bfs_levels(adj: &Adjacency, seeds: &[(NodeId, u8)], edge_level: &[u8], how: &LevelWalk<'_>) -> LevelBfs {
    let n = adj.len();
    if edge_level.len() != adj.kept_edges() {
        return length_guard("edge_level", edge_level.len(), adj.kept_edges(), "kept edges");
    }
    if let Some(mask) = how.within
        && mask.len() != n
    {
        return length_guard("within", mask.len(), n, "indexed nodes");
    }
    if let Some(rank) = how.parent_rank
        && rank.len() != n
    {
        return length_guard("parent_rank", rank.len(), n, "indexed nodes");
    }
    let inside = |ix: u32| how.within.is_none_or(|mask| mask[ix as usize]);

    // Each started seed once, at its lowest level, in first-occurrence order.
    let mut slot = vec![usize::MAX; n];
    let mut start: Vec<(u32, u8)> = Vec::new();
    for &(id, level) in seeds {
        let Some(ix) = adj.index_of(id) else { continue };
        if !inside(ix) {
            continue;
        }
        match slot[ix as usize] {
            usize::MAX => {
                slot[ix as usize] = start.len();
                start.push((ix, level));
            }
            at => start[at].1 = start[at].1.min(level),
        }
    }
    start.retain(|&(_, level)| level < how.levels);

    let mut out = LevelBfs { seeds: start.iter().map(|&(ix, level)| (adj.id(ix), level)).collect(), ..LevelBfs::default() };
    let mut reported = vec![false; n];
    let mut visited = vec![false; n];
    let mut frontier: Vec<u32> = Vec::new();
    let mut next: Vec<u32> = Vec::new();
    for level in 0..how.levels {
        visited.fill(false);
        frontier.clear();
        for &(ix, seed_level) in &start {
            if seed_level <= level {
                visited[ix as usize] = !how.plus;
                frontier.push(ix);
            }
        }
        // Layer by layer: without a rank, the same order a FIFO queue gives.
        let mut depth = 0usize;
        while !frontier.is_empty() && depth < how.max_depth {
            if let Some(rank) = how.parent_rank {
                frontier.sort_unstable_by_key(|&ix| (rank[ix as usize], ix));
            }
            next.clear();
            for &node in &frontier {
                each_incidence(adj, node, how.walk, |inc| {
                    out.scanned += 1;
                    let to = inc.other as usize;
                    if edge_level[inc.edge as usize] > level || visited[to] || !inside(inc.other) {
                        return;
                    }
                    visited[to] = true;
                    if !reported[to] {
                        reported[to] = true;
                        out.reached.push(LevelReached {
                            id: adj.id(inc.other),
                            level,
                            depth: depth + 1,
                            via: inc.category,
                            parent: adj.id(node),
                            kept_edge: inc.edge,
                        });
                    }
                    next.push(inc.other);
                });
            }
            std::mem::swap(&mut frontier, &mut next);
            depth += 1;
        }
    }
    out
}

/// The wrong-length guard: report and return an empty walk.
fn length_guard(what: &str, got: usize, want: usize, unit: &str) -> LevelBfs {
    eprintln!("[algo] bfs_levels: {what} has {got} entries for {want} {unit}, returning an empty walk");
    LevelBfs::default()
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
    use glia_core::Edge;

    /// The walks this module replaced, verbatim at 3193854, over an edge list.
    mod oracle {
        use std::collections::{HashSet, VecDeque};

        use glia_core::{Edge, EdgeCategoryId, NodeId};

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

    // ---- bfs_levels (DA.1) ----

    const WALKS: [Walk; 3] = [Walk::Forward, Walk::Backward, Walk::Both];

    /// `n` nodes `0..n` and `m` edges with LCG endpoints over categories
    /// `1..=cats` (self-loops and parallel edges included).
    fn lcg_graph(rng: &mut Lcg, n: u64, m: usize, cats: u32) -> ToyGraph {
        ToyGraph {
            nodes: (0..n).map(NodeId).collect(),
            edges: (0..m)
                .map(|_| toy_edge(rng.below(n), rng.below(n), 1 + rng.below(cats as u64) as u32))
                .collect(),
        }
    }

    fn level_rows(b: &LevelBfs) -> Vec<(NodeId, usize, EdgeCategoryId, NodeId)> {
        b.reached.iter().map(|r| (r.id, r.depth, r.via, r.parent)).collect()
    }

    fn at(level: u8, ids: &[NodeId]) -> Vec<(NodeId, u8)> {
        ids.iter().map(|&s| (s, level)).collect()
    }

    fn ids_of(b: &LevelBfs) -> Vec<u64> {
        b.reached.iter().map(|r| r.id.0).collect()
    }

    #[test]
    fn levels_one_pass_equals_bfs() {
        let mut rng = Lcg(0xda_0001);
        let g = lcg_graph(&mut rng, 300, 900, 3);
        let adj = Adjacency::build(&g, &CategorySet::all());
        let zero = vec![0u8; adj.kept_edges()];
        let s = [NodeId(0), NodeId(7)];
        for walk in WALKS {
            let want = bfs(&adj, &s, walk, usize::MAX);
            let got = bfs_levels(&adj, &at(0, &s), &zero, &LevelWalk::new(walk, 1));
            assert!(want.reached.len() > 100, "{walk:?}: the graph must reach widely");
            assert_eq!(level_rows(&got), as_rows(&want), "{walk:?}");
            assert!(got.reached.iter().all(|r| r.level == 0));
            assert_eq!(got.seeds, at(0, &s));
            assert_eq!(got.scanned, want.scanned, "{walk:?}");
        }

        // The random corpus: dangling endpoints, absent and repeated seeds,
        // self-loops, parallel edges, category subsets, every depth.
        let mut rng = Lcg(0xda_0002);
        let mut compared = 0usize;
        for _ in 0..300 {
            let c = case(&mut rng);
            for cats in category_sets(&mut rng) {
                let adj = Adjacency::build(&c.graph, &CategorySet::of(&cats));
                let zero = vec![0u8; adj.kept_edges()];
                let s = seeds(&mut rng, &c);
                for &max_depth in &DEPTHS {
                    for walk in WALKS {
                        let want = bfs(&adj, &s, walk, max_depth);
                        let mut how = LevelWalk::new(walk, 1);
                        how.max_depth = max_depth;
                        let got = bfs_levels(&adj, &at(0, &s), &zero, &how);
                        assert_eq!(level_rows(&got), as_rows(&want), "{walk:?} depth {max_depth} seeds {s:?}");
                        assert_eq!(got.scanned, want.scanned);
                        compared += 1;
                    }
                }
            }
        }
        assert_eq!(compared, 300 * 2 * DEPTHS.len() * 3);
    }

    #[test]
    fn levels_match_filtered_bfs_oracle() {
        let mut rng = Lcg(0xda_0003);
        let g = lcg_graph(&mut rng, 300, 900, 3);
        // Index categories 1 and 2 only, so a kept position is not a graph
        // edge position.
        let keep = [EdgeCategoryId(1), EdgeCategoryId(2)];
        let adj = Adjacency::build(&g, &CategorySet::of(&keep));
        let kept: Vec<&Edge> = g.edges.iter().filter(|e| keep.contains(&e.category)).collect();
        assert_eq!(kept.len(), adj.kept_edges());
        assert!(kept.len() < g.edges.len());
        let edge_level: Vec<u8> = kept.iter().map(|_| rng.below(3) as u8).collect();
        // The level-0 seed has a level-0 edge each way, so pass 0 reaches
        // something whichever way it walks.
        let lvl0: Vec<&Edge> =
            kept.iter().zip(&edge_level).filter(|&(e, &l)| l == 0 && e.from != e.to).map(|(e, _)| *e).collect();
        let x = lvl0
            .iter()
            .map(|e| e.from)
            .find(|&f| ![7, 42, 150].contains(&f.0) && lvl0.iter().any(|e| e.to == f))
            .expect("a node with a level-0 edge each way");
        // 7 is given twice; its lowest level (1) counts.
        let seeds = [(x, 0), (NodeId(7), 2), (NodeId(42), 1), (NodeId(7), 1), (NodeId(150), 2)];
        for walk in WALKS {
            let got = bfs_levels(&adj, &seeds, &edge_level, &LevelWalk::new(walk, 3));
            assert_eq!(got.seeds, vec![(x, 0), (NodeId(7), 1), (NodeId(42), 1), (NodeId(150), 2)]);
            assert!(got.reached.windows(2).all(|w| w[0].level <= w[1].level), "{walk:?}: level ascending");
            let once: HashSet<NodeId> = got.reached.iter().map(|r| r.id).collect();
            assert_eq!(once.len(), got.reached.len(), "{walk:?}: each node once");
            for r in &got.reached {
                // The kept edge joins parent and node with `via`, walkable at the node's level.
                let e = kept[r.kept_edge as usize];
                let (tail, head) = (e.from, e.to);
                let joined = match walk {
                    Walk::Forward => (tail, head) == (r.parent, r.id),
                    Walk::Backward => (head, tail) == (r.parent, r.id),
                    Walk::Both => (tail, head) == (r.parent, r.id) || (head, tail) == (r.parent, r.id),
                };
                assert!(joined && e.category == r.via, "{walk:?}: {r:?} via {e:?}");
                assert!(edge_level[r.kept_edge as usize] <= r.level);
            }
            for l in 0..3u8 {
                let sub = ToyGraph {
                    nodes: g.nodes.clone(),
                    edges: kept.iter().zip(&edge_level).filter(|&(_, &x)| x <= l).map(|(e, _)| (*e).clone()).collect(),
                };
                let sub_adj = Adjacency::build(&sub, &CategorySet::all());
                let s_l: Vec<NodeId> = seeds.iter().filter(|&&(_, x)| x <= l).map(|&(s, _)| s).collect();
                // The closure at level l: seeds of level <= l plus what edges of level <= l reach.
                let got_set: HashSet<NodeId> =
                    got.reached.iter().filter(|r| r.level <= l).map(|r| r.id).chain(s_l.iter().copied()).collect();
                assert_eq!(got_set, reachable(&sub_adj, &s_l, walk), "{walk:?} level {l}");
                // The rows new at l are the filtered graph's BFS minus what an
                // earlier level reported: same order, depth, via and parent.
                let earlier: HashSet<NodeId> = got.reached.iter().filter(|r| r.level < l).map(|r| r.id).collect();
                let want: Vec<_> = as_rows(&bfs(&sub_adj, &s_l, walk, usize::MAX))
                    .into_iter()
                    .filter(|row| !earlier.contains(&row.0))
                    .collect();
                let new_at_l: Vec<_> = got
                    .reached
                    .iter()
                    .filter(|r| r.level == l)
                    .map(|r| (r.id, r.depth, r.via, r.parent))
                    .collect();
                assert!(!new_at_l.is_empty(), "{walk:?}: level {l} must add nodes");
                assert_eq!(new_at_l, want, "{walk:?} level {l}");
            }
        }
    }

    #[test]
    fn seed_levels_gate_their_pass() {
        // 1 -> 2, 3 -> 4, 5 -> 6, 2 -> 8; every edge at level 0.
        let g = ToyGraph {
            nodes: (1..=8).map(NodeId).collect(),
            edges: vec![toy_edge(1, 2, 1), toy_edge(3, 4, 1), toy_edge(5, 6, 1), toy_edge(2, 8, 1)],
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let zero = vec![0u8; adj.kept_edges()];
        // 3 is given at 2 (and its repeat at 2 changes nothing), 5 at 3 (never
        // starts in 3 passes), 99 is absent, 2 is a seed at 2 that pass 0 reaches.
        let seeds = [(NodeId(1), 0), (NodeId(3), 2), (NodeId(5), 3), (NodeId(99), 0), (NodeId(2), 2), (NodeId(3), 2)];
        let got = bfs_levels(&adj, &seeds, &zero, &LevelWalk::new(Walk::Forward, 3));
        assert_eq!(got.seeds, vec![(NodeId(1), 0), (NodeId(3), 2), (NodeId(2), 2)]);
        let rows: Vec<(u64, u8, usize, u64)> = got.reached.iter().map(|r| (r.id.0, r.level, r.depth, r.parent.0)).collect();
        assert_eq!(rows, vec![(2, 0, 1, 1), (8, 0, 2, 2), (4, 2, 1, 3)]);
        // Two passes never start 3: 4 is never reached.
        let two = bfs_levels(&adj, &seeds, &zero, &LevelWalk::new(Walk::Forward, 2));
        assert_eq!(ids_of(&two), vec![2, 8]);
        assert_eq!(two.seeds, vec![(NodeId(1), 0)]);
    }

    #[test]
    fn within_mask_confines_walk() {
        let g = ToyGraph {
            nodes: (0..4).map(NodeId).collect(),
            edges: vec![toy_edge(0, 1, 1), toy_edge(1, 2, 1), toy_edge(2, 3, 1)],
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let zero = vec![0u8; adj.kept_edges()];
        let mask: Vec<bool> = (0..adj.len() as u32).map(|ix| adj.id(ix) != NodeId(2)).collect();
        let mut how = LevelWalk::new(Walk::Forward, 1);
        how.within = Some(&mask);
        assert_eq!(ids_of(&bfs_levels(&adj, &at(0, &[NodeId(0)]), &zero, &how)), vec![1]);
        // From 3 both ways the walk cannot step onto 2.
        how.walk = Walk::Both;
        assert!(bfs_levels(&adj, &at(0, &[NodeId(3)]), &zero, &how).reached.is_empty());
        // A seed outside the mask never starts.
        let outside = bfs_levels(&adj, &at(0, &[NodeId(2)]), &zero, &how);
        assert_eq!(outside, LevelBfs::default());
        // Without the mask the chain is walked whole.
        how.within = None;
        how.walk = Walk::Forward;
        assert_eq!(ids_of(&bfs_levels(&adj, &at(0, &[NodeId(0)]), &zero, &how)), vec![1, 2, 3]);
    }

    #[test]
    fn parent_rank_picks_lowest_ranked_parent() {
        // s=1 -> a=2, s -> b=3, a -> t=4, b -> t; a's edge to t first.
        let g = ToyGraph {
            nodes: (1..=4).map(NodeId).collect(),
            edges: vec![toy_edge(1, 2, 1), toy_edge(1, 3, 2), toy_edge(2, 4, 3), toy_edge(3, 4, 4)],
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let zero = vec![0u8; adj.kept_edges()];
        let ix = |id: u64| adj.index_of(NodeId(id)).unwrap() as usize;
        let ranked = |a: u32, b: u32| {
            let mut rank = vec![0u32; adj.len()];
            (rank[ix(2)], rank[ix(3)]) = (a, b);
            rank
        };
        let t_of = |rank: Option<&[u32]>| {
            let mut how = LevelWalk::new(Walk::Forward, 1);
            how.parent_rank = rank;
            let got = bfs_levels(&adj, &at(0, &[NodeId(1)]), &zero, &how);
            assert_eq!(ids_of(&got), vec![2, 3, 4], "depth-1 discovery stays in s's edge order");
            let t = got.reached[2];
            (t.parent.0, t.via.0, t.kept_edge)
        };
        assert_eq!(t_of(None), (2, 3, 2), "unranked: the first discovered parent");
        assert_eq!(t_of(Some(&ranked(5, 1))), (3, 4, 3), "rank(b) < rank(a)");
        assert_eq!(t_of(Some(&ranked(1, 5))), (2, 3, 2), "rank(a) < rank(b)");
        assert_eq!(t_of(Some(&ranked(7, 7))), (2, 3, 2), "a rank tie falls to the dense index");
    }

    #[test]
    fn unwalkable_level() {
        let g = ToyGraph {
            nodes: (1..=3).map(NodeId).collect(),
            edges: vec![toy_edge(1, 2, 1), toy_edge(2, 3, 1)],
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let levels = [0, u8::MAX];
        for passes in [1, 3, u8::MAX] {
            for walk in WALKS {
                let got = bfs_levels(&adj, &at(0, &[NodeId(1)]), &levels, &LevelWalk::new(walk, passes));
                let want: Vec<u64> = if walk == Walk::Backward { vec![] } else { vec![2] };
                assert_eq!(ids_of(&got), want, "{walk:?} over {passes} passes");
            }
        }
        // u8::MAX passes run passes 0..=254; the last still skips a u8::MAX edge.
        let got = bfs_levels(&adj, &at(0, &[NodeId(2)]), &levels, &LevelWalk::new(Walk::Forward, u8::MAX));
        assert!(got.reached.is_empty());
        assert_eq!(got.scanned, u8::MAX as usize, "2's one out-edge examined once per pass");
    }

    #[test]
    fn length_mismatch_is_empty_not_a_panic() {
        let g = ToyGraph {
            nodes: (1..=3).map(NodeId).collect(),
            edges: vec![toy_edge(1, 2, 1), toy_edge(2, 3, 1)],
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let seeds = at(0, &[NodeId(1)]);
        let short = vec![0u8; adj.kept_edges() - 1];
        assert_eq!(bfs_levels(&adj, &seeds, &short, &LevelWalk::new(Walk::Forward, 3)), LevelBfs::default());
        let zero = vec![0u8; adj.kept_edges()];
        let long = vec![0u8; adj.kept_edges() + 1];
        assert_eq!(bfs_levels(&adj, &seeds, &long, &LevelWalk::new(Walk::Forward, 3)), LevelBfs::default());
        let mask = vec![true; adj.len() - 1];
        let mut how = LevelWalk::new(Walk::Forward, 3);
        how.within = Some(&mask);
        assert_eq!(bfs_levels(&adj, &seeds, &zero, &how), LevelBfs::default());
        let rank = vec![0u32; adj.len() + 1];
        let mut how = LevelWalk::new(Walk::Forward, 3);
        how.parent_rank = Some(&rank);
        assert_eq!(bfs_levels(&adj, &seeds, &zero, &how), LevelBfs::default());
        // Zero passes start nothing.
        assert_eq!(bfs_levels(&adj, &seeds, &zero, &LevelWalk::new(Walk::Forward, 0)), LevelBfs::default());
        // The right lengths walk.
        assert_eq!(ids_of(&bfs_levels(&adj, &seeds, &zero, &LevelWalk::new(Walk::Forward, 3))), vec![2, 3]);
    }

    #[test]
    fn levelled_scanned_is_linear() {
        let mut rng = Lcg(0xda_0008);
        let g = lcg_graph(&mut rng, 300, 900, 3);
        let adj = Adjacency::build(&g, &CategorySet::all());
        let edge_level: Vec<u8> = (0..adj.kept_edges()).map(|_| rng.below(3) as u8).collect();
        let seeds: Vec<(NodeId, u8)> = (0..12).map(|i| (NodeId(i * 23), (i % 3) as u8)).collect();
        let rank: Vec<u32> = (0..adj.len()).map(|_| rng.below(50) as u32).collect();
        for walk in WALKS {
            for plus in [false, true] {
                for levels in [1u8, 3] {
                    for ranked in [false, true] {
                        let mut how = LevelWalk::new(walk, levels);
                        how.plus = plus;
                        how.parent_rank = ranked.then_some(rank.as_slice());
                        let got = bfs_levels(&adj, &seeds, &edge_level, &how);
                        let bound = 4 * adj.kept_edges() * levels as usize;
                        assert!(got.scanned <= bound, "{walk:?} plus {plus} levels {levels}: {} > {bound}", got.scanned);
                        assert!(!got.reached.is_empty());
                    }
                }
            }
        }
    }

    #[test]
    fn plus_mode_reports_seeds_on_cycles() {
        // 0 -> 1 -> 2 -> 0, 2 -> 3.
        let g = ToyGraph {
            nodes: (0..4).map(NodeId).collect(),
            edges: vec![toy_edge(0, 1, 1), toy_edge(1, 2, 1), toy_edge(2, 0, 1), toy_edge(2, 3, 1)],
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let zero = vec![0u8; adj.kept_edges()];
        let mut how = LevelWalk::new(Walk::Forward, 1);
        how.plus = true;
        let got = bfs_levels(&adj, &at(0, &[NodeId(0)]), &zero, &how);
        let rows: Vec<(u64, usize, u64)> = got.reached.iter().map(|r| (r.id.0, r.depth, r.parent.0)).collect();
        assert_eq!(rows, vec![(1, 1, 0), (2, 2, 1), (0, 3, 2), (3, 3, 2)]);
        // Without plus the seed is never reported.
        assert_eq!(ids_of(&bfs_levels(&adj, &at(0, &[NodeId(0)]), &zero, &LevelWalk::new(Walk::Forward, 1))), vec![1, 2, 3]);

        // Chain 0 -> 1, seeds [0, 1]: plus reports 1 (reached from 0), never 0.
        let chain = ToyGraph { nodes: (0..2).map(NodeId).collect(), edges: vec![toy_edge(0, 1, 1)] };
        let cadj = Adjacency::build(&chain, &CategorySet::all());
        let both = at(0, &[NodeId(0), NodeId(1)]);
        let plus = bfs_levels(&cadj, &both, &[0], &how);
        assert_eq!(plus.reached.iter().map(|r| (r.id.0, r.depth, r.parent.0)).collect::<Vec<_>>(), vec![(1, 1, 0)]);
        assert!(bfs_levels(&cadj, &both, &[0], &LevelWalk::new(Walk::Forward, 1)).reached.is_empty());
        // A self-loop puts its node on a cycle of one hop.
        let self_loop = ToyGraph { nodes: vec![NodeId(5)], edges: vec![toy_edge(5, 5, 1)] };
        let sadj = Adjacency::build(&self_loop, &CategorySet::all());
        assert_eq!(ids_of(&bfs_levels(&sadj, &at(0, &[NodeId(5)]), &[0], &how)), vec![5]);
    }

    /// Plus mode is the one-or-more-hop closure (the closure of the seeds'
    /// one-hop neighbours), and its non-seed rows are the plain walk's.
    #[test]
    fn plus_mode_matches_one_or_more_hop_oracle() {
        let mut rng = Lcg(0xda_0009);
        let mut seeds_reported = 0usize;
        for _ in 0..300 {
            let c = case(&mut rng);
            for cats in category_sets(&mut rng) {
                let allow: HashSet<EdgeCategoryId> = cats.iter().copied().collect();
                let adj = Adjacency::build(&c.graph, &CategorySet::of(&cats));
                let zero = vec![0u8; adj.kept_edges()];
                let s = seeds(&mut rng, &c);
                let indexed: Vec<NodeId> = s.iter().copied().filter(|&x| adj.index_of(x).is_some()).collect();
                for walk in WALKS {
                    let mut how = LevelWalk::new(walk, 1);
                    how.plus = true;
                    let got = bfs_levels(&adj, &at(0, &s), &zero, &how);
                    let one_hop: Vec<NodeId> = c
                        .graph
                        .edges
                        .iter()
                        .filter(|e| allow.contains(&e.category))
                        .flat_map(|e| {
                            let fwd = walk != Walk::Backward && indexed.contains(&e.from);
                            let bwd = walk != Walk::Forward && indexed.contains(&e.to);
                            [fwd.then_some(e.to), bwd.then_some(e.from)]
                        })
                        .flatten()
                        .collect();
                    let got_set: HashSet<NodeId> = got.reached.iter().map(|r| r.id).collect();
                    assert_eq!(got_set.len(), got.reached.len());
                    assert_eq!(got_set, reachable(&adj, &one_hop, walk), "{walk:?} seeds {s:?}");
                    let plain = bfs_levels(&adj, &at(0, &s), &zero, &LevelWalk::new(walk, 1));
                    let non_seed: Vec<_> = level_rows(&got).into_iter().filter(|row| !s.contains(&row.0)).collect();
                    assert_eq!(non_seed, level_rows(&plain), "{walk:?} seeds {s:?}");
                    seeds_reported += got.reached.iter().filter(|r| s.contains(&r.id)).count();
                }
            }
        }
        assert!(seeds_reported > 100, "the corpus must reach seeds, got {seeds_reported}");
    }
}
