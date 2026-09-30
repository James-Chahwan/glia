//! Communities (CD.1a, extended by CD.1b): a weighted undirected view of an
//! [`Adjacency`] with integer weights, seeded label
//! propagation and modularity (CD.1a), then seeded Leiden with its local
//! moving, refinement and aggregation, and the weighted quotient graph
//! (CD.1b). Domain-free: the categories and their weights come from the
//! caller or a domain's [`DomainTables`](crate::profile::DomainTables), never
//! a named code kind. Filled by CD.1a (+CD.1b).
//!
//! Determinism is the rule every item here keeps:
//! - Weights are integers: a category weighs a `u32`, a pair or a strength
//!   sums to a `u64`, so every comparison an algorithm decides on is exact.
//!   `f64` appears only in the reported [`Partition::modularity`].
//! - The view is a pure function of the edge SET. Its dense order is the ids
//!   sorted by value, never source order, and parallel edges merge by a sort,
//!   never by iterating a hash map, so permuting a source's nodes or edges
//!   cannot change a [`WeightedGraph`] or anything computed from it.
//! - The only randomness is one SplitMix64 stream from the caller's
//!   [`CommunityOptions::seed`].
//!
//! Conventions, as networkx's `modularity`: a self-loop of weight `w` is the
//! diagonal entry `A_ii = 2w`, which [`WeightedGraph::self_weight`] returns
//! and [`WeightedGraph::strength`] counts; `total_weight` is `2m`.
//!
//! Label propagation breaks a tie between equally heavy neighbour labels by
//! a draw from the seeded stream (Raghavan et al.'s uniform tie-break), not
//! by the smallest label: on Zachary's karate club the smallest-label rule
//! floods the whole graph into one community for 72 of seeds 0..200 (mean
//! modularity 0.204), the seeded draw for none (mean 0.359).
//!
//! Leiden, not Louvain: Louvain can return a community whose only bridge
//! node moved out, a disconnected set no one can read as a unit. Every
//! [`leiden`] community is connected: refinement merges only along edges,
//! and a final pass splits any community that is not. Two departures from
//! the paper, both for one answer per seed:
//! - Every gain and well-connectedness test is an integer comparison of
//!   `den * 2m * a - num * b * c`, held exactly in 256 bits, never an `f64`.
//! - Refinement is greedy: a node joins the well-connected sub-community
//!   with the best non-negative gain (ties to the smaller id), where the
//!   paper draws one at random with probability `exp(gain / theta)`. It
//!   keeps the paper's guarantees that matter here (sub-communities are
//!   connected and well connected); it gives up the randomised search's
//!   chance of escaping a local optimum, which a caller buys back by
//!   trying another seed.
//!
//! [`communities`] picks: Leiden up to
//! [`CommunityOptions::leiden_edge_cap`] pairs, label propagation above.

use std::cmp::Ordering;
use std::collections::VecDeque;

use glia_core::{EdgeCategoryId, NodeId};

use super::{Adjacency, CategorySet, GraphSource};

/// The dense-index limit (LD.15): indices and neighbour offsets are `u32`.
const CAP: usize = u32::MAX as usize;

/// A weighted undirected view of a graph: each unordered pair of distinct
/// nodes joined by any weighted edge holds the sum of those edges' weights,
/// each node its self-loop weight and its strength (weighted degree).
///
/// Dense index `i` is the `i`-th id in ascending value; each neighbour list
/// is sorted by neighbour index and holds no zero weight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeightedGraph {
    /// Sorted by value.
    ids: Vec<NodeId>,
    /// Node `i`'s neighbours are `nbr[start[i]..start[i + 1]]`.
    start: Vec<u32>,
    /// `(neighbour, merged weight)`, sorted by neighbour.
    nbr: Vec<(u32, u64)>,
    /// The diagonal `A_ii`: twice the summed self-loop weight.
    self_w: Vec<u64>,
    /// `self_w[i]` plus node `i`'s neighbour weights.
    strength: Vec<u64>,
    /// `2m`: every strength summed.
    total: u64,
}

impl WeightedGraph {
    /// The view of `g`'s edges whose category `weights` gives a positive
    /// weight. The first entry listed for a category wins; a category that is
    /// absent or weighs 0 adds nothing. Parallel and opposite edges sum into
    /// one pair. The nodes are `g`'s node ids plus every endpoint of a
    /// weighted edge that is no node; one with no weighted edge is present
    /// with strength 0.
    pub fn from_source<G: GraphSource + ?Sized>(g: &G, weights: &[(EdgeCategoryId, u32)]) -> Self {
        let table = weight_table(weights);
        let cats: Vec<EdgeCategoryId> = table.iter().map(|&(c, _)| EdgeCategoryId(c)).collect();
        let adj = Adjacency::build(g, &CategorySet::of(&cats));
        let n = adj.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        order.sort_unstable_by_key(|&ix| adj.id(ix).0);
        let mut rank = vec![0u32; n];
        for (r, &ix) in order.iter().enumerate() {
            rank[ix as usize] = r as u32;
        }
        let ids = order.iter().map(|&ix| adj.id(ix)).collect();
        let mut pairs = Vec::with_capacity(adj.kept_edges());
        for ix in 0..n as u32 {
            for inc in adj.outgoing(ix) {
                let w = table.binary_search_by_key(&inc.category.0, |&(c, _)| c).map_or(0, |i| table[i].1);
                if w == 0 {
                    continue;
                }
                let (a, b) = (rank[ix as usize], rank[inc.other as usize]);
                pairs.push((a.min(b), a.max(b), u64::from(w)));
            }
        }
        Self::assemble(ids, pairs, CAP)
    }

    /// The view of `n` nodes `NodeId(0)..NodeId(n)` joined by `pairs`
    /// `(a, b, weight)` (quotient graphs, tests). A pair `(a, a, w)` is a
    /// self-loop of weight `w`; a pair with an endpoint `>= n` or weight 0 is
    /// skipped.
    pub fn from_pairs(n: usize, pairs: &[(u32, u32, u64)]) -> Self {
        if n > CAP {
            return Self::over_limit("nodes", CAP);
        }
        let kept = pairs
            .iter()
            .filter(|&&(a, b, w)| w > 0 && (a as usize) < n && (b as usize) < n)
            .map(|&(a, b, w)| (a.min(b), a.max(b), w))
            .collect();
        Self::assemble((0..n as u64).map(NodeId).collect(), kept, CAP)
    }

    /// Merge `pairs` (`a <= b`, dense indices into `ids`) by sort and lay
    /// out the neighbour lists. Over `cap` nodes or neighbour entries the
    /// view is empty and a warning is printed, never a panic.
    fn assemble(ids: Vec<NodeId>, pairs: Vec<(u32, u32, u64)>, cap: usize) -> Self {
        let n = ids.len();
        if n > cap {
            return Self::over_limit("nodes", cap);
        }
        Self::assemble_with(ids, pairs, vec![0; n], cap)
    }

    /// [`Self::assemble`] from a starting diagonal `self_w` (one entry per
    /// id): a quotient's communities carry their internal weight there.
    fn assemble_with(ids: Vec<NodeId>, mut pairs: Vec<(u32, u32, u64)>, mut self_w: Vec<u64>, cap: usize) -> Self {
        let n = ids.len();
        if n > cap {
            return Self::over_limit("nodes", cap);
        }
        self_w.resize(n, 0);
        pairs.sort_unstable_by_key(|&(a, b, _)| (a, b));
        let mut merged: Vec<(u32, u32, u64)> = Vec::with_capacity(pairs.len());
        for (a, b, w) in pairs {
            match merged.last_mut() {
                Some(last) if last.0 == a && last.1 == b => last.2 = last.2.saturating_add(w),
                _ => merged.push((a, b, w)),
            }
        }
        let mut start = vec![0u32; n + 1];
        let mut links = 0usize;
        for &(a, b, w) in &merged {
            if a == b {
                self_w[a as usize] = self_w[a as usize].saturating_add(w.saturating_mul(2));
            } else {
                links += 2;
                if links > cap {
                    return Self::over_limit("neighbour entries", cap);
                }
                start[a as usize + 1] += 1;
                start[b as usize + 1] += 1;
            }
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        // Filled in (a, b) order, each list comes out sorted: node x meets
        // its lower neighbours (pairs (a, x)) first, ascending, then its
        // higher ones (pairs (x, b)), ascending.
        let mut cursor = start.clone();
        let mut nbr = vec![(0u32, 0u64); links];
        for &(a, b, w) in &merged {
            if a == b {
                continue;
            }
            for (at, other) in [(a, b), (b, a)] {
                let slot = &mut cursor[at as usize];
                nbr[*slot as usize] = (other, w);
                *slot += 1;
            }
        }
        let strength: Vec<u64> = (0..n)
            .map(|i| {
                let row = &nbr[start[i] as usize..start[i + 1] as usize];
                row.iter().fold(self_w[i], |s, &(_, w)| s.saturating_add(w))
            })
            .collect();
        let total = strength.iter().fold(0u64, |s, &k| s.saturating_add(k));
        Self { ids, start, nbr, self_w, strength, total }
    }

    fn over_limit(what: &str, cap: usize) -> Self {
        eprintln!("[algo] community view: more than {cap} {what} - u32 index limit, returning an empty view");
        Self { ids: Vec::new(), start: vec![0], nbr: Vec::new(), self_w: Vec::new(), strength: Vec::new(), total: 0 }
    }

    /// Nodes in the view, weighted or not.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Unordered pairs of distinct nodes with a positive merged weight.
    pub fn pair_count(&self) -> usize {
        self.nbr.len() / 2
    }

    /// The id at dense index `ix` (`ix < len()`).
    pub fn id(&self, ix: u32) -> NodeId {
        self.ids[ix as usize]
    }

    /// The dense index of `id`, when the view holds it.
    pub fn index_of(&self, id: NodeId) -> Option<u32> {
        self.ids.binary_search_by_key(&id.0, |x| x.0).ok().map(|i| i as u32)
    }

    /// `(neighbour, merged weight)` of `ix` (`ix < len()`), sorted by
    /// neighbour; a self-loop is not listed.
    pub fn neighbours(&self, ix: u32) -> &[(u32, u64)] {
        let i = ix as usize;
        &self.nbr[self.start[i] as usize..self.start[i + 1] as usize]
    }

    /// The diagonal entry `A_ii`: twice the summed weight of `ix`'s
    /// self-loops.
    pub fn self_weight(&self, ix: u32) -> u64 {
        self.self_w[ix as usize]
    }

    /// Weighted degree: [`Self::self_weight`] plus every neighbour weight.
    pub fn strength(&self, ix: u32) -> u64 {
        self.strength[ix as usize]
    }

    /// `2m`: the sum of every strength.
    pub fn total_weight(&self) -> u64 {
        self.total
    }

    /// The quotient of this view by `membership` (a community in `0..k` per
    /// dense index): node `c` of the result, id `NodeId(c)`, is community
    /// `c`. The pair between two communities sums every pair between their
    /// members, and community `c`'s self weight is `in_c`, the sum of `A_ij`
    /// over the ordered pairs inside it: twice its internal pair weight plus
    /// its members' self weights. So every strength is its members' strengths
    /// summed and [`Self::total_weight`] is unchanged. A node `membership`
    /// does not reach, or labels `k` or more, is left out with its pairs; a
    /// community with no member is a node of strength 0.
    pub fn quotient(&self, membership: &[u32], k: usize) -> WeightedGraph {
        if k > CAP {
            return Self::over_limit("nodes", CAP);
        }
        let label = |v: usize| membership.get(v).copied().filter(|&c| (c as usize) < k);
        let mut self_w = vec![0u64; k];
        let mut pairs = Vec::new();
        for v in 0..self.len() {
            let Some(c) = label(v) else { continue };
            let ci = c as usize;
            self_w[ci] = self_w[ci].saturating_add(self.self_w[v]);
            for &(u, w) in self.neighbours(v as u32) {
                // Each unordered pair once, from its lower end.
                if (u as usize) < v {
                    continue;
                }
                match label(u as usize) {
                    Some(d) if d == c => self_w[ci] = self_w[ci].saturating_add(w.saturating_mul(2)),
                    Some(d) => pairs.push((c.min(d), c.max(d), w)),
                    None => {}
                }
            }
        }
        Self::assemble_with((0..k as u64).map(NodeId).collect(), pairs, self_w, CAP)
    }
}

/// The positive category weights, sorted by category id. The sort is stable,
/// so the first entry listed for a category is the one kept; a kept 0 then
/// drops the category.
fn weight_table(weights: &[(EdgeCategoryId, u32)]) -> Vec<(u32, u32)> {
    let mut t: Vec<(u32, u32)> = weights.iter().map(|&(c, w)| (c.0, w)).collect();
    t.sort_by_key(|&(c, _)| c);
    t.dedup_by_key(|e| e.0);
    t.retain(|&(_, w)| w > 0);
    t
}

/// Modularity's resolution `num / den` (gamma): above 1 favours more,
/// smaller communities. A `den` of 0 is read as 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolution {
    pub num: u32,
    pub den: u32,
}

impl Resolution {
    /// Gamma 1, the textbook modularity.
    pub const ONE: Self = Self { num: 1, den: 1 };

    /// `(num, den)` with a zero denominator read as 1.
    fn ratio(self) -> (u32, u32) {
        (self.num, self.den.max(1))
    }
}

impl Default for Resolution {
    fn default() -> Self {
        Self::ONE
    }
}

/// The algorithm that produced a [`Partition`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Method {
    Leiden,
    LabelPropagation,
}

impl Method {
    /// `leiden` / `label_propagation`, the name a marker or answer prints.
    pub fn name(self) -> &'static str {
        match self {
            Method::Leiden => "leiden",
            Method::LabelPropagation => "label_propagation",
        }
    }
}

/// What a community search is seeded and bounded by. Outside this crate,
/// build it as `CommunityOptions::default()` plus field assignment.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommunityOptions {
    /// Seeds the one SplitMix64 stream every random choice draws from.
    pub seed: u64,
    pub resolution: Resolution,
    /// Leiden: aggregation levels at most.
    pub max_levels: u32,
    /// Label propagation: rounds at most.
    pub max_rounds: u32,
    /// Leiden runs up to this many merged pairs; a larger view falls back to
    /// label propagation.
    pub leiden_edge_cap: usize,
}

impl Default for CommunityOptions {
    fn default() -> Self {
        Self { seed: 0, resolution: Resolution::ONE, max_levels: 10, max_rounds: 20, leiden_edge_cap: 20_000_000 }
    }
}

/// A partition of a [`WeightedGraph`]'s nodes.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Partition {
    /// Community of each dense index. Ids are canonical: numbered by size
    /// descending, then by smallest member (the smallest id value), from 0.
    pub membership: Vec<u32>,
    /// Communities, isolated nodes' singletons included.
    pub communities: usize,
    /// At the resolution the search used; for display, never a decision.
    pub modularity: f64,
    /// Leiden: aggregation levels run. Label propagation: rounds run, the
    /// last one the round that changed nothing unless `max_rounds` stopped
    /// it.
    pub levels: u32,
    pub method: Method,
}

/// Newman-Girvan modularity of `membership` (a community id per dense
/// index, any `u32` values) at resolution `res`:
/// `Q = sum_c [ in_c / 2m - gamma * (K_c / 2m)^2 ]`, where `in_c` sums
/// `A_ij` over ordered pairs inside `c` (self-loops at `A_ii`) and `K_c` is
/// `c`'s summed strength. Accumulated in integers, converted to `f64` at
/// the end. 0 when the view has no weight; NaN when `membership` is not one
/// id per node.
pub fn modularity(g: &WeightedGraph, membership: &[u32], res: Resolution) -> f64 {
    let n = g.len();
    if membership.len() != n {
        return f64::NAN;
    }
    if g.total == 0 {
        return 0.0;
    }
    let mut inside: u128 = 0;
    for v in 0..n {
        inside += u128::from(g.self_w[v]);
        for &(u, w) in g.neighbours(v as u32) {
            if membership[u as usize] == membership[v] {
                inside += u128::from(w);
            }
        }
    }
    let mut ks: Vec<(u32, u64)> = membership.iter().copied().zip(g.strength.iter().copied()).collect();
    ks.sort_unstable_by_key(|&(c, _)| c);
    let mut squares: u128 = 0;
    for group in ks.chunk_by(|a, b| a.0 == b.0) {
        let k: u128 = group.iter().map(|&(_, k)| u128::from(k)).sum();
        squares = squares.saturating_add(k.saturating_mul(k));
    }
    let (num, den) = res.ratio();
    let t = g.total as f64;
    inside as f64 / t - (f64::from(num) / f64::from(den)) * (squares as f64 / (t * t))
}

/// Seeded asynchronous label propagation: every node starts in its own
/// community; each round visits the nodes in a fresh SplitMix64
/// Fisher-Yates order of `0..n`, and a node takes the label with the
/// largest summed edge weight among its neighbours. Staying put wins a tie
/// with the current label; a tie between other labels is a seeded draw
/// among them in ascending label order. Stops after a round that changes
/// nothing, or after `max_rounds`. Self-loops never vote; a node with no
/// neighbour stays a singleton.
pub fn label_propagation(g: &WeightedGraph, opts: &CommunityOptions) -> Partition {
    let n = g.len();
    let mut label: Vec<u32> = (0..n as u32).collect();
    let mut rng = SplitMix64::new(opts.seed);
    let mut order: Vec<u32> = vec![0; n];
    let mut score = vec![0u64; n];
    let mut touched: Vec<u32> = Vec::new();
    let mut ties: Vec<u32> = Vec::new();
    let mut rounds = 0u32;
    while rounds < opts.max_rounds {
        rounds += 1;
        for (i, o) in order.iter_mut().enumerate() {
            *o = i as u32;
        }
        rng.shuffle(&mut order);
        let mut changed = false;
        for &v in &order {
            // Every weight is positive, so a label's score is 0 until touched.
            for &(u, w) in g.neighbours(v) {
                let l = label[u as usize] as usize;
                if score[l] == 0 {
                    touched.push(l as u32);
                }
                score[l] = score[l].saturating_add(w);
            }
            let best = touched.iter().map(|&l| score[l as usize]).max().unwrap_or(0);
            let current = label[v as usize];
            if best > 0 && score[current as usize] != best {
                ties.clear();
                ties.extend(touched.iter().copied().filter(|&l| score[l as usize] == best));
                ties.sort_unstable();
                let pick = if ties.len() == 1 { 0 } else { rng.below(ties.len() as u64) as usize };
                label[v as usize] = ties[pick];
                changed = true;
            }
            for &l in &touched {
                score[l as usize] = 0;
            }
            touched.clear();
        }
        if !changed {
            break;
        }
    }
    let (membership, communities) = canonical(&label);
    let modularity = modularity(g, &membership, opts.resolution);
    Partition { membership, communities, modularity, levels: rounds, method: Method::LabelPropagation }
}

/// Leiden, or [`label_propagation`] when the view has more than
/// `opts.leiden_edge_cap` pairs; [`Partition::method`] says which ran.
pub fn communities(g: &WeightedGraph, opts: &CommunityOptions) -> Partition {
    if g.pair_count() > opts.leiden_edge_cap { label_propagation(g, opts) } else { leiden(g, opts) }
}

/// Seeded Leiden (Traag, Waltman and van Eck, 2019) maximising modularity
/// at `opts.resolution`. Each level runs fast local moving from the level's
/// partition, refines each of its communities into well-connected
/// sub-communities, and aggregates the refined ones into the next level's
/// graph ([`WeightedGraph::quotient`]), whose starting partition is the
/// moved one (the Leiden trick). It stops at a level where local moving
/// leaves every node alone, where refinement merges nothing, or after
/// `opts.max_levels` levels ([`Partition::levels`] counts them).
///
/// Every decision is an exact integer comparison: moving a node of strength
/// `k_v` into a community of strength `K_C` it joins with weight `k_vC`
/// scores `den * 2m * k_vC - num * k_v * K_C` (the modularity gain times
/// `den * (2m)^2 / 2`), held in 256 bits, and ties go to the smaller community
/// id, so a seed gives one answer on every platform.
///
/// Refinement is the greedy, deterministic form of the paper's: a node
/// still alone and well connected to its community `S` joins the
/// well-connected sub-community it has an edge into with the best
/// non-negative gain, where the paper draws one at random weighted by
/// `exp(gain / theta)`. Every merge follows an edge, so every sub-community
/// is connected. After the last level a pass splits any community whose
/// members are not connected inside it into its components; splitting never
/// lowers modularity, so every returned community is connected even when
/// `max_levels` stops the search early. Ids are canonical, as
/// [`label_propagation`]'s.
pub fn leiden(g: &WeightedGraph, opts: &CommunityOptions) -> Partition {
    let n = g.len();
    let scale = Scale::new(opts.resolution, g.total);
    let mut rng = SplitMix64::new(opts.seed);
    let mut scratch = Scratch::new(n);
    // The current level's node holding each original node.
    let mut to_level: Vec<u32> = (0..n as u32).collect();
    // The current level's partition of its nodes, ids in `0..level.len()`.
    let mut comm: Vec<u32> = (0..n as u32).collect();
    let mut aggregate: Option<WeightedGraph> = None;
    let mut levels = 0u32;
    while levels < opts.max_levels {
        let level = aggregate.as_ref().unwrap_or(g);
        levels += 1;
        let live = move_nodes(level, &mut comm, &scale, &mut rng, &mut scratch);
        if live == level.len() || levels == opts.max_levels {
            break;
        }
        let refined = refine(level, &comm, &scale, &mut rng, &mut scratch);
        let (sub, r) = first_seen(&refined);
        if r == level.len() {
            break;
        }
        // Refinement stays inside a community, so every member of a refined
        // sub-community carries the same moved community.
        let mut lifted = vec![0u32; r];
        for (v, &t) in sub.iter().enumerate() {
            lifted[t as usize] = comm[v];
        }
        let next = level.quotient(&sub, r);
        comm = first_seen(&lifted).0;
        for x in &mut to_level {
            *x = sub[*x as usize];
        }
        aggregate = Some(next);
    }
    let flat: Vec<u32> = to_level.iter().map(|&x| comm[x as usize]).collect();
    let (membership, communities) = canonical(&split_disconnected(g, &flat));
    let modularity = modularity(g, &membership, opts.resolution);
    Partition { membership, communities, modularity, levels, method: Method::Leiden }
}

/// Fast local moving over `g` from the partition `comm` (ids in
/// `0..g.len()`): a queue of every node in a seeded order; a popped node
/// leaves its community and joins the neighbouring community with the best
/// gain when that beats staying (ties: staying, then the smaller id), or an
/// empty community when being alone beats both. After a move, the node's
/// neighbours outside its new community are queued unless already queued.
/// Every move strictly raises modularity, so the queue drains. Returns the
/// number of non-empty communities.
fn move_nodes(g: &WeightedGraph, comm: &mut [u32], scale: &Scale, rng: &mut SplitMix64, s: &mut Scratch) -> usize {
    let n = g.len();
    let mut tot = vec![0u64; n];
    let mut size = vec![0u32; n];
    for (&c, &k) in comm.iter().zip(&g.strength) {
        tot[c as usize] = tot[c as usize].saturating_add(k);
        size[c as usize] += 1;
    }
    // The empty ids, largest first, so a pop hands out the smallest.
    let mut empty: Vec<u32> = (0..n as u32).rev().filter(|&c| size[c as usize] == 0).collect();
    s.order.clear();
    s.order.extend(0..n as u32);
    rng.shuffle(&mut s.order);
    s.queue.clear();
    s.queue.extend(s.order.iter().copied());
    s.queued[..n].fill(true);
    while let Some(v) = s.queue.pop_front() {
        let vi = v as usize;
        s.queued[vi] = false;
        let (old, k_v) = (comm[vi], g.strength[vi]);
        // Every weight is positive, so a community's link is 0 until touched.
        for &(u, w) in g.neighbours(v) {
            let c = comm[u as usize] as usize;
            if s.link[c] == 0 {
                s.touched.push(c as u32);
            }
            s.link[c] = s.link[c].saturating_add(w);
        }
        let oi = old as usize;
        tot[oi] = tot[oi].saturating_sub(k_v);
        size[oi] -= 1;
        let mut best = old;
        let mut best_gain = scale.gain(s.link[oi], k_v, tot[oi]);
        for &c in &s.touched {
            if c == old {
                continue;
            }
            let gain = scale.gain(s.link[c as usize], k_v, tot[c as usize]);
            let better = match gain.compare(&best_gain) {
                Ordering::Greater => true,
                Ordering::Equal => best != old && c < best,
                Ordering::Less => false,
            };
            if better {
                best = c;
                best_gain = gain;
            }
        }
        // Alone scores 0. A negative best means `old` still holds another
        // node (alone in it, staying scores 0), so an empty id exists.
        if !best_gain.non_negative()
            && let Some(e) = empty.pop()
        {
            best = e;
        }
        let bi = best as usize;
        comm[vi] = best;
        tot[bi] = tot[bi].saturating_add(k_v);
        size[bi] += 1;
        if best != old {
            if size[oi] == 0 {
                empty.push(old);
            }
            for &(u, _) in g.neighbours(v) {
                let ui = u as usize;
                if comm[ui] != best && !s.queued[ui] {
                    s.queued[ui] = true;
                    s.queue.push_back(u);
                }
            }
        }
        for &c in &s.touched {
            s.link[c as usize] = 0;
        }
        s.touched.clear();
    }
    size.iter().filter(|&&z| z > 0).count()
}

/// Leiden's refinement of the moved partition `comm`: inside each community
/// `S`, every node starts alone; in a seeded order, a node still alone that
/// is well connected to `S` (`den * 2m * k_{v,S-v} >= num * k_v * (K_S -
/// k_v)`) joins the sub-community `T` of `S` it has an edge into that is
/// itself well connected (`den * 2m * w(T, S-T) >= num * K_T * (K_S -
/// K_T)`) with the best non-negative gain, ties to the smaller id. Returns
/// each node's sub-community, ids in `0..g.len()`.
fn refine(g: &WeightedGraph, comm: &[u32], scale: &Scale, rng: &mut SplitMix64, s: &mut Scratch) -> Vec<u32> {
    let n = g.len();
    let mut tot_s = vec![0u64; n];
    for (&c, &k) in comm.iter().zip(&g.strength) {
        tot_s[c as usize] = tot_s[c as usize].saturating_add(k);
    }
    let mut refined: Vec<u32> = (0..n as u32).collect();
    let mut rtot: Vec<u64> = g.strength.clone();
    let mut rsize = vec![1u32; n];
    // w(T, S - T) per sub-community; a lone node's is its weight into S.
    let mut ext: Vec<u64> = (0..n)
        .map(|v| {
            g.neighbours(v as u32)
                .iter()
                .filter(|&&(u, _)| comm[u as usize] == comm[v])
                .fold(0u64, |acc, &(_, w)| acc.saturating_add(w))
        })
        .collect();
    s.order.clear();
    s.order.extend(0..n as u32);
    rng.shuffle(&mut s.order);
    for &v in &s.order {
        let vi = v as usize;
        if refined[vi] != v || rsize[vi] != 1 {
            continue;
        }
        let home = comm[vi];
        let (k_v, k_s) = (g.strength[vi], tot_s[home as usize]);
        if !scale.gain(ext[vi], k_v, k_s.saturating_sub(k_v)).non_negative() {
            continue;
        }
        for &(u, w) in g.neighbours(v) {
            if comm[u as usize] != home {
                continue;
            }
            let t = refined[u as usize] as usize;
            if s.link[t] == 0 {
                s.touched.push(t as u32);
            }
            s.link[t] = s.link[t].saturating_add(w);
        }
        let mut best: Option<(u32, Gain)> = None;
        for &t in &s.touched {
            let ti = t as usize;
            if !scale.gain(ext[ti], rtot[ti], k_s.saturating_sub(rtot[ti])).non_negative() {
                continue;
            }
            let gain = scale.gain(s.link[ti], k_v, rtot[ti]);
            if !gain.non_negative() {
                continue;
            }
            let better = match &best {
                None => true,
                Some((b, bg)) => match gain.compare(bg) {
                    Ordering::Greater => true,
                    Ordering::Equal => t < *b,
                    Ordering::Less => false,
                },
            };
            if better {
                best = Some((t, gain));
            }
        }
        if let Some((t, _)) = best {
            let ti = t as usize;
            refined[vi] = t;
            rsize[ti] += 1;
            rsize[vi] = 0;
            rtot[ti] = rtot[ti].saturating_add(k_v);
            rtot[vi] = 0;
            // The v-T edges turn internal: out of both boundaries.
            ext[ti] = ext[ti].saturating_add(ext[vi]).saturating_sub(s.link[ti].saturating_mul(2));
            ext[vi] = 0;
        }
        for &t in &s.touched {
            s.link[t as usize] = 0;
        }
        s.touched.clear();
    }
    refined
}

/// Split every community of `labels` into its connected components: one
/// BFS over the pairs inside a community per component, each component its
/// own label, numbered by its first node.
fn split_disconnected(g: &WeightedGraph, labels: &[u32]) -> Vec<u32> {
    let n = g.len().min(labels.len());
    let mut out = vec![u32::MAX; n];
    let mut queue: VecDeque<u32> = VecDeque::new();
    let mut next = 0u32;
    for start in 0..n {
        if out[start] != u32::MAX {
            continue;
        }
        out[start] = next;
        queue.push_back(start as u32);
        while let Some(v) = queue.pop_front() {
            let home = labels[v as usize];
            for &(u, _) in g.neighbours(v) {
                let ui = u as usize;
                if ui < n && out[ui] == u32::MAX && labels[ui] == home {
                    out[ui] = next;
                    queue.push_back(u);
                }
            }
        }
        next += 1;
    }
    out
}

/// Renumber `labels` by first appearance: index 0's label becomes 0, the
/// next unseen label 1, and so on. Returns the labels and their count.
fn first_seen(labels: &[u32]) -> (Vec<u32>, usize) {
    let top = labels.iter().max().map_or(0, |&m| m as usize + 1);
    let mut map = vec![u32::MAX; top];
    let mut next = 0u32;
    let out = labels
        .iter()
        .map(|&l| {
            let slot = &mut map[l as usize];
            if *slot == u32::MAX {
                *slot = next;
                next += 1;
            }
            *slot
        })
        .collect();
    (out, next as usize)
}

/// Reusable per-run buffers, sized for the first (largest) level: the
/// per-community link weights with the list of those touched, a node order,
/// and the local-moving queue with its in-queue bitmap.
struct Scratch {
    link: Vec<u64>,
    touched: Vec<u32>,
    order: Vec<u32>,
    queue: VecDeque<u32>,
    queued: Vec<bool>,
}

impl Scratch {
    fn new(n: usize) -> Self {
        Self {
            link: vec![0; n],
            touched: Vec::new(),
            order: Vec::with_capacity(n),
            queue: VecDeque::with_capacity(n),
            queued: vec![false; n],
        }
    }
}

/// The integer scale Leiden decides at: resolution `num / den` and `2m`.
#[derive(Clone, Copy, Debug)]
struct Scale {
    num: u64,
    den_total: u128,
}

impl Scale {
    fn new(res: Resolution, total: u64) -> Self {
        let (num, den) = res.ratio();
        Self { num: u64::from(num), den_total: u128::from(den) * u128::from(total) }
    }

    /// `den * 2m * link - num * k * tot`. A move's gain with `link = k_vC`,
    /// `k = k_v`, `tot = K_C`; a well-connectedness margin with the set's
    /// boundary weight, its strength and the rest of its community's.
    fn gain(&self, link: u64, k: u64, tot: u64) -> Gain {
        Gain { pos: U256::mul(self.den_total, link), neg: U256::mul(u128::from(self.num) * u128::from(k), tot) }
    }
}

/// An exact signed difference `pos - neg` of two products of three `u64`s.
#[derive(Clone, Copy, Debug)]
struct Gain {
    pos: U256,
    neg: U256,
}

impl Gain {
    /// Numeric order: `a - b` against `c - d` is `a + d` against `c + b`.
    fn compare(&self, other: &Gain) -> Ordering {
        self.pos.add(other.neg).cmp(&other.pos.add(self.neg))
    }

    fn non_negative(&self) -> bool {
        self.pos >= self.neg
    }
}

/// An unsigned `hi * 2^128 + lo`. A gain's terms reach 160 bits (`u32 *
/// u64 * u64`), past `i128`; the sums [`Gain::compare`] forms reach 161.
/// Field order makes the derived order numeric.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct U256 {
    hi: u128,
    lo: u128,
}

impl U256 {
    /// `a * b`, exact: `(a_hi * 2^64 + a_lo) * b`, each partial product
    /// under `2^128`.
    fn mul(a: u128, b: u64) -> Self {
        let b = u128::from(b);
        let low = (a & u128::from(u64::MAX)) * b;
        let high = (a >> 64) * b;
        let (lo, carry) = (high << 64).overflowing_add(low);
        Self { hi: (high >> 64) + u128::from(carry), lo }
    }

    /// `self + o`. Every operand here is under `2^161`, so `hi` never wraps.
    fn add(self, o: Self) -> Self {
        let (lo, carry) = self.lo.overflowing_add(o.lo);
        Self { hi: self.hi.wrapping_add(o.hi).wrapping_add(u128::from(carry)), lo }
    }
}

/// Renumber `labels` canonically: by community size descending, then by
/// smallest member index (in a view, the smallest id value). Returns the
/// membership and the community count.
fn canonical(labels: &[u32]) -> (Vec<u32>, usize) {
    let mut by: Vec<(u32, u32)> = labels.iter().enumerate().map(|(i, &l)| (l, i as u32)).collect();
    by.sort_unstable();
    // (size, smallest member, offset of the group in `by`)
    let mut groups: Vec<(usize, u32, usize)> = Vec::new();
    let mut at = 0usize;
    for group in by.chunk_by(|a, b| a.0 == b.0) {
        groups.push((group.len(), group[0].1, at));
        at += group.len();
    }
    groups.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out = vec![0u32; labels.len()];
    for (id, &(size, _, from)) in groups.iter().enumerate() {
        for &(_, ix) in &by[from..from + size] {
            out[ix as usize] = id as u32;
        }
    }
    (out, groups.len())
}

/// SplitMix64's increment, the 64-bit golden ratio.
const GOLDEN: u64 = 0x9e37_79b9_7f4a_7c15;

/// SplitMix64's output finaliser: an integer-only 64-bit mix (xor-shift,
/// multiply).
pub(crate) fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Steele, Lea and Flood's SplitMix64: the one pseudo-random stream the
/// community algorithms (and MinHash, CD.4d) draw from, integer-only and
/// identical on every platform.
#[derive(Clone, Debug)]
pub(crate) struct SplitMix64(u64);

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(GOLDEN);
        mix64(self.0)
    }

    /// Uniform in `0..n` (Lemire's multiply-shift with rejection, no modulo
    /// bias); 0 when `n` is 0.
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let mut m = u128::from(self.next_u64()) * u128::from(n);
        if (m as u64) < n {
            let floor = n.wrapping_neg() % n;
            while (m as u64) < floor {
                m = u128::from(self.next_u64()) * u128::from(n);
            }
        }
        (m >> 64) as u64
    }

    /// Fisher-Yates, from the last position down.
    pub(crate) fn shuffle(&mut self, xs: &mut [u32]) {
        for i in (1..xs.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            xs.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algo::{ToyGraph, toy_edge};

    const CALLS: u32 = 1;
    const USES: u32 = 2;
    const DROPPED: u32 = 3;
    const UNLISTED: u32 = 4;
    const WEIGHTS: &[(EdgeCategoryId, u32)] =
        &[(EdgeCategoryId(CALLS), 4), (EdgeCategoryId(USES), 2), (EdgeCategoryId(DROPPED), 0)];

    /// A 60-node graph from a 64-bit LCG: scattered id values, 240 edges over
    /// four categories, self-loops, repeats and a few dangling endpoints.
    fn lcg_graph(seed: u64) -> ToyGraph {
        let mut x = seed;
        let mut step = || {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            x >> 33
        };
        let nodes: Vec<NodeId> = (0..60).map(|i| NodeId(mix64(i) % 1_000_000)).collect();
        let mut edges = Vec::new();
        for _ in 0..240 {
            let from = nodes[(step() % 60) as usize].0;
            let to = if step() % 25 == 0 { 2_000_000 + step() % 4 } else { nodes[(step() % 60) as usize].0 };
            edges.push(toy_edge(from, to, 1 + (step() % 4) as u32));
        }
        ToyGraph { nodes, edges }
    }

    fn cliques(k: u32, size: u32) -> WeightedGraph {
        let mut pairs = Vec::new();
        for c in 0..k {
            for i in 0..size {
                for j in i + 1..size {
                    pairs.push((c * size + i, c * size + j, 1));
                }
            }
            if c + 1 < k {
                pairs.push((c * size + size - 1, (c + 1) * size, 1));
            }
        }
        WeightedGraph::from_pairs((k * size) as usize, &pairs)
    }

    fn options(seed: u64) -> CommunityOptions {
        CommunityOptions { seed, ..CommunityOptions::default() }
    }

    #[test]
    fn view_is_order_invariant() {
        let g = lcg_graph(11);
        let view = WeightedGraph::from_source(&g, WEIGHTS);
        let reversed = ToyGraph {
            nodes: g.nodes.iter().rev().copied().collect(),
            edges: g.edges.iter().rev().cloned().collect(),
        };
        assert_eq!(WeightedGraph::from_source(&reversed, WEIGHTS), view);
        let mut perm: Vec<u32> = (0..g.edges.len() as u32).collect();
        SplitMix64::new(3).shuffle(&mut perm);
        let shuffled = ToyGraph { nodes: g.nodes.clone(), edges: perm.iter().map(|&i| g.edges[i as usize].clone()).collect() };
        assert_eq!(WeightedGraph::from_source(&shuffled, WEIGHTS), view);
        // Not vacuous: dangling endpoints joined, pairs merged, ids sorted.
        assert!(view.len() > g.nodes.len() && view.pair_count() > 50);
        assert!(view.ids.windows(2).all(|w| w[0].0 < w[1].0));
        let strengths: u64 = (0..view.len() as u32).map(|i| view.strength(i)).sum();
        assert_eq!(strengths, view.total_weight());
        for i in 0..view.len() as u32 {
            assert_eq!(view.index_of(view.id(i)), Some(i));
            assert!(view.neighbours(i).windows(2).all(|w| w[0].0 < w[1].0));
        }
    }

    #[test]
    fn parallel_and_opposite_edges_merge() {
        let g = ToyGraph {
            nodes: vec![NodeId(20), NodeId(10)],
            edges: vec![toy_edge(10, 20, CALLS), toy_edge(20, 10, CALLS), toy_edge(10, 20, USES)],
        };
        let view = WeightedGraph::from_source(&g, WEIGHTS);
        let (a, b) = (view.index_of(NodeId(10)).unwrap(), view.index_of(NodeId(20)).unwrap());
        assert_eq!((a, b), (0, 1));
        assert_eq!(view.neighbours(a), &[(b, 10)]);
        assert_eq!(view.neighbours(b), &[(a, 10)]);
        assert_eq!((view.strength(a), view.pair_count(), view.total_weight()), (10, 1, 20));
    }

    #[test]
    fn zero_weight_categories_dropped() {
        let g = ToyGraph {
            nodes: vec![NodeId(1), NodeId(2), NodeId(3)],
            edges: vec![toy_edge(1, 2, UNLISTED), toy_edge(2, 3, DROPPED), toy_edge(3, 9, UNLISTED)],
        };
        let view = WeightedGraph::from_source(&g, WEIGHTS);
        assert_eq!((view.len(), view.pair_count(), view.total_weight()), (3, 0, 0));
        assert_eq!(view.index_of(NodeId(9)), None, "an unweighted edge's dangling endpoint is not indexed");
        // The first entry listed for a category wins, a 0 included.
        let first_wins =
            [(EdgeCategoryId(CALLS), 3), (EdgeCategoryId(CALLS), 5), (EdgeCategoryId(USES), 0), (EdgeCategoryId(USES), 7)];
        let g = ToyGraph { nodes: vec![NodeId(1), NodeId(2)], edges: vec![toy_edge(1, 2, CALLS), toy_edge(2, 1, USES)] };
        let view = WeightedGraph::from_source(&g, &first_wins);
        assert_eq!(view.neighbours(0), &[(1, 3)]);
    }

    #[test]
    fn self_loops_follow_the_networkx_convention() {
        // networkx 3.6.1: modularity(G, [{0, 1}, {2}], weight='weight') = -0.125
        // and the singletons -0.045, for loop 0-0 (3), 0-1 (2), 1-2 (5).
        let g = WeightedGraph::from_pairs(3, &[(0, 0, 3), (0, 1, 2), (1, 2, 5), (7, 1, 4), (1, 2, 0)]);
        assert_eq!((g.self_weight(0), g.strength(0), g.strength(1), g.total_weight()), (6, 8, 7, 20));
        assert!((modularity(&g, &[0, 0, 1], Resolution::ONE) + 0.125).abs() < 1e-12);
        assert!((modularity(&g, &[0, 1, 2], Resolution::ONE) + 0.045).abs() < 1e-12);
        let src = ToyGraph { nodes: vec![NodeId(5)], edges: vec![toy_edge(5, 5, CALLS)] };
        let view = WeightedGraph::from_source(&src, WEIGHTS);
        assert_eq!((view.self_weight(0), view.strength(0), view.pair_count()), (8, 8, 0));
    }

    #[test]
    fn modularity_matches_textbook() {
        // Two 4-cliques joined by one edge: m = 13, each side K = 13.
        let g = cliques(2, 4);
        assert_eq!(g.total_weight(), 26);
        let split = [0, 0, 0, 0, 1, 1, 1, 1];
        let q = modularity(&g, &split, Resolution::ONE);
        assert!((q - 2.0 * (6.0 / 13.0 - 0.25)).abs() < 1e-12 && (q - 0.4231).abs() < 5e-5, "{q}");
        assert_eq!(modularity(&g, &[7; 8], Resolution::ONE), 0.0);
        let half = modularity(&g, &split, Resolution { num: 1, den: 2 });
        assert!((half - 2.0 * (6.0 / 13.0 - 0.125)).abs() < 1e-12);
        assert_eq!(modularity(&g, &split, Resolution { num: 1, den: 0 }), q, "den 0 reads as 1");
        assert!(modularity(&g, &[0; 7], Resolution::ONE).is_nan());
        assert_eq!(modularity(&WeightedGraph::from_pairs(3, &[]), &[0, 1, 2], Resolution::ONE), 0.0);
    }

    /// Zachary's karate club, networkx 3.6.1 `karate_club_graph()`, 0-based.
    const KARATE: [(u32, u32); 78] = [
        (0, 1), (0, 2), (0, 3), (0, 4), (0, 5), (0, 6), (0, 7), (0, 8), (0, 10), (0, 11), (0, 12), (0, 13), (0, 17),
        (0, 19), (0, 21), (0, 31), (1, 2), (1, 3), (1, 7), (1, 13), (1, 17), (1, 19), (1, 21), (1, 30), (2, 3),
        (2, 7), (2, 8), (2, 9), (2, 13), (2, 27), (2, 28), (2, 32), (3, 7), (3, 12), (3, 13), (4, 6), (4, 10),
        (5, 6), (5, 10), (5, 16), (6, 16), (8, 30), (8, 32), (8, 33), (9, 33), (13, 33), (14, 32), (14, 33),
        (15, 32), (15, 33), (18, 32), (18, 33), (19, 33), (20, 32), (20, 33), (22, 32), (22, 33), (23, 25),
        (23, 27), (23, 29), (23, 32), (23, 33), (24, 25), (24, 27), (24, 31), (25, 31), (26, 29), (26, 33),
        (27, 33), (28, 31), (28, 33), (29, 32), (29, 33), (30, 32), (30, 33), (31, 32), (31, 33), (32, 33),
    ];
    /// Its `club` attribute: the members who followed the Officer.
    const OFFICER: [u32; 17] = [9, 14, 15, 18, 20, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33];

    fn karate() -> WeightedGraph {
        let pairs: Vec<(u32, u32, u64)> = KARATE.iter().map(|&(a, b)| (a, b, 1)).collect();
        WeightedGraph::from_pairs(34, &pairs)
    }

    #[test]
    fn karate_club_modularity() {
        let g = karate();
        assert_eq!((g.pair_count(), g.total_weight()), (78, 156));
        let club: Vec<u32> = (0..34).map(|i| u32::from(OFFICER.contains(&i))).collect();
        let q = modularity(&g, &club, Resolution::ONE);
        // networkx 3.6.1: 0.3582347140039448.
        assert!((q - 0.3582).abs() <= 0.0005 && (q - 0.358_234_714_003_944_8).abs() < 1e-12, "{q}");
    }

    #[test]
    fn lpa_two_cliques() {
        let g = cliques(2, 6);
        let two = [0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1];
        let p = label_propagation(&g, &options(0));
        assert_eq!((p.communities, p.membership.as_slice(), p.method), (2, &two[..], Method::LabelPropagation));
        // A reference implementation of the same rules in Python recovers the
        // two cliques for 995 of seeds 0..1000 and merges them for 5.
        let (mut recovered, mut merged) = (0, 0);
        for seed in 0..1000 {
            let p = label_propagation(&g, &options(seed));
            recovered += usize::from(p.membership == two);
            merged += usize::from(p.communities == 1);
        }
        assert_eq!((recovered, merged), (995, 5));
    }

    #[test]
    fn lpa_karate_matches_reference() {
        // The Python reference over seeds 0..200: no seed floods the graph,
        // mean networkx modularity 0.359111604207758; seed 0 runs 5 rounds
        // into 3 communities (Q 0.34393491124260345).
        let g = karate();
        let runs: Vec<Partition> = (0..200).map(|seed| label_propagation(&g, &options(seed))).collect();
        assert!(runs.iter().all(|p| p.communities > 1));
        let mean = runs.iter().map(|p| p.modularity).sum::<f64>() / 200.0;
        assert!((mean - 0.359_111_604_207_758).abs() < 1e-9, "{mean}");
        assert_eq!((runs[0].levels, runs[0].communities), (5, 3));
        assert!((runs[0].modularity - 0.343_934_911_242_603_45).abs() < 1e-12);
    }

    #[test]
    fn lpa_deterministic() {
        let g = lcg_graph(5);
        let view = WeightedGraph::from_source(&g, WEIGHTS);
        let p = label_propagation(&view, &options(9));
        assert_eq!(label_propagation(&view, &options(9)), p);
        let reversed = ToyGraph {
            nodes: g.nodes.iter().rev().copied().collect(),
            edges: g.edges.iter().rev().cloned().collect(),
        };
        assert_eq!(label_propagation(&WeightedGraph::from_source(&reversed, WEIGHTS), &options(9)), p);
        assert_eq!(p.membership.len(), view.len());
        assert_eq!(p.modularity, modularity(&view, &p.membership, Resolution::ONE));
    }

    #[test]
    fn lpa_ids_are_canonical_and_isolated_nodes_stay_singletons() {
        // Nodes 5 and 6 are isolated; 0-1 a pair; 2-3-4 a triangle.
        let g = WeightedGraph::from_pairs(7, &[(0, 1, 5), (2, 3, 1), (3, 4, 1), (2, 4, 1)]);
        let p = label_propagation(&g, &options(1));
        assert_eq!((p.membership, p.communities), (vec![1, 1, 0, 0, 0, 2, 3], 4));
        let none = label_propagation(&g, &CommunityOptions { max_rounds: 0, ..options(1) });
        assert_eq!((none.membership, none.communities, none.levels), ((0..7).collect::<Vec<u32>>(), 7, 0));
        let empty = label_propagation(&WeightedGraph::from_pairs(0, &[]), &options(1));
        assert_eq!((empty.communities, empty.modularity, empty.levels), (0, 0.0, 1));
    }

    #[test]
    fn over_the_index_limit_is_empty_not_a_panic() {
        let ids: Vec<NodeId> = (0..3).map(NodeId).collect();
        let over_nodes = WeightedGraph::assemble(ids.clone(), vec![(0, 1, 1)], 2);
        assert!(over_nodes.is_empty() && over_nodes.pair_count() == 0);
        // Two pairs are four neighbour entries: over a cap of 3.
        let over_links = WeightedGraph::assemble(ids.clone(), vec![(0, 1, 1), (1, 2, 1)], 3);
        assert!(over_links.is_empty() && over_links.total_weight() == 0);
        assert_eq!(label_propagation(&over_links, &options(0)).communities, 0);
        assert_eq!(WeightedGraph::assemble(ids, vec![(0, 1, 1), (0, 1, 2)], 3).neighbours(0), &[(1, 3)]);
    }

    #[test]
    fn splitmix_known_vector() {
        let mut r = SplitMix64::new(0);
        assert_eq!(
            [r.next_u64(), r.next_u64(), r.next_u64()],
            [0xe220_a839_7b1d_cdaf, 0x6e78_9e6a_a1b9_65f4, 0x06c4_5d18_8009_454f]
        );
        let mut r = SplitMix64::new(42);
        assert!((0..1000).all(|_| r.below(7) < 7));
        assert_eq!(r.below(0), 0);
        let mut xs: Vec<u32> = (0..50).collect();
        r.shuffle(&mut xs);
        assert_ne!(xs, (0..50).collect::<Vec<u32>>());
        xs.sort_unstable();
        assert_eq!(xs, (0..50).collect::<Vec<u32>>());
    }

    /// 4 planted groups of 25 (group `i / 25`): a pair inside a group is an
    /// edge with probability 0.30, across groups 0.01, drawn from
    /// SplitMix64(seed) over the pairs in (i, j) order.
    fn planted(seed: u64) -> WeightedGraph {
        let mut rng = SplitMix64::new(seed);
        let mut pairs = Vec::new();
        for i in 0..100u32 {
            for j in i + 1..100 {
                let percent = if i / 25 == j / 25 { 30 } else { 1 };
                if rng.below(100) < percent {
                    pairs.push((i, j, 1));
                }
            }
        }
        WeightedGraph::from_pairs(100, &pairs)
    }

    /// A 64-bit LCG view: 5..=80 nodes, 0..=300 weighted pairs (weights
    /// 1..=4, repeats and self-loops included).
    fn random_view(seed: u64) -> WeightedGraph {
        let mut x = mix64(seed);
        let mut step = move || {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            x >> 33
        };
        let n = 5 + step() % 76;
        let m = step() % 301;
        let pairs: Vec<(u32, u32, u64)> =
            (0..m).map(|_| ((step() % n) as u32, (step() % n) as u32, 1 + step() % 4)).collect();
        WeightedGraph::from_pairs(n as usize, &pairs)
    }

    /// Every community of `p` is non-empty and induces a connected subgraph.
    fn assert_connected(g: &WeightedGraph, p: &Partition) {
        assert_eq!(p.membership.len(), g.len());
        for c in 0..p.communities as u32 {
            let members: Vec<u32> = (0..g.len() as u32).filter(|&v| p.membership[v as usize] == c).collect();
            assert!(!members.is_empty(), "community {c} is empty");
            let mut seen = vec![false; g.len()];
            let mut stack = vec![members[0]];
            seen[members[0] as usize] = true;
            let mut reached = 1;
            while let Some(v) = stack.pop() {
                for &(u, _) in g.neighbours(v) {
                    if p.membership[u as usize] == c && !seen[u as usize] {
                        seen[u as usize] = true;
                        reached += 1;
                        stack.push(u);
                    }
                }
            }
            assert_eq!(reached, members.len(), "community {c} is not connected: {members:?}");
        }
    }

    #[test]
    fn planted_partition_recovered() {
        let g = planted(7);
        let p = leiden(&g, &options(0));
        let groups: Vec<u32> = (0..100).map(|i| i / 25).collect();
        assert_eq!((p.membership.as_slice(), p.communities, p.method), (groups.as_slice(), 4, Method::Leiden));
        assert_eq!(p.modularity, modularity(&g, &groups, Resolution::ONE));
    }

    #[test]
    fn karate_club_quality() {
        // networkx 3.6.1 Louvain over 200 seeds: 0.4198 x42, 0.4188 x62,
        // 0.4156 x49, 0.4151 x29, minimum 0.3854.
        let g = karate();
        let qs: Vec<f64> = (0..=19).map(|seed| leiden(&g, &options(seed)).modularity).collect();
        let best = qs.iter().copied().fold(f64::MIN, f64::max);
        assert!(best >= 0.4180, "best {best}: {qs:?}");
        assert!(qs.iter().all(|&q| q >= 0.3900), "{qs:?}");
        for seed in 0..=19 {
            assert_connected(&g, &leiden(&g, &options(seed)));
        }
    }

    #[test]
    fn communities_are_connected() {
        let mut split = 0;
        for seed in 0..200 {
            let g = random_view(seed);
            let p = leiden(&g, &options(seed));
            assert_connected(&g, &p);
            assert!(p.levels >= 1 && p.levels <= CommunityOptions::default().max_levels);
            // One level stops before refinement: connected by the final pass.
            let early = leiden(&g, &CommunityOptions { max_levels: 1, ..options(seed) });
            assert_connected(&g, &early);
            split += usize::from(early.communities > p.communities);
        }
        // Not vacuous: over these graphs one level leaves communities that
        // later levels merge.
        assert!(split > 0);
    }

    #[test]
    fn leiden_beats_lpa_on_average() {
        let (mut leiden_sum, mut lpa_sum) = (0.0, 0.0);
        for seed in 0..200 {
            let g = random_view(seed);
            leiden_sum += leiden(&g, &options(seed)).modularity;
            lpa_sum += label_propagation(&g, &options(seed)).modularity;
        }
        assert!(leiden_sum / 200.0 >= lpa_sum / 200.0, "leiden {} lpa {}", leiden_sum / 200.0, lpa_sum / 200.0);
        let g = planted(7);
        assert!(leiden(&g, &options(0)).modularity >= label_propagation(&g, &options(0)).modularity);
    }

    #[test]
    fn deterministic_and_order_invariant() {
        let g = lcg_graph(5);
        let view = WeightedGraph::from_source(&g, WEIGHTS);
        let p = leiden(&view, &options(9));
        assert_eq!(leiden(&view, &options(9)), p);
        let reversed = ToyGraph {
            nodes: g.nodes.iter().rev().copied().collect(),
            edges: g.edges.iter().rev().cloned().collect(),
        };
        assert_eq!(leiden(&WeightedGraph::from_source(&reversed, WEIGHTS), &options(9)), p);
        // Not vacuous: a real partition, canonical ids.
        assert!(p.communities > 1 && p.communities < view.len());
        assert_eq!(canonical(&p.membership), (p.membership.clone(), p.communities));
        assert_eq!(p.modularity, modularity(&view, &p.membership, Resolution::ONE));
    }

    #[test]
    fn quotient_preserves_weight() {
        // With self-loops: in_c = 2 x the internal pair weight + members' self weights.
        let view = WeightedGraph::from_source(&lcg_graph(5), WEIGHTS);
        let p = leiden(&view, &options(9));
        let q = view.quotient(&p.membership, p.communities);
        assert_eq!((q.len(), q.total_weight()), (p.communities, view.total_weight()));
        for c in 0..p.communities as u32 {
            let members: Vec<u32> = (0..view.len() as u32).filter(|&v| p.membership[v as usize] == c).collect();
            let loops: u64 = members.iter().map(|&v| view.self_weight(v)).sum();
            let mut internal = 0u64;
            for &v in &members {
                for &(u, w) in view.neighbours(v) {
                    if u > v && p.membership[u as usize] == c {
                        internal += w;
                    }
                }
            }
            assert_eq!(q.self_weight(c), 2 * internal + loops);
            assert_eq!(q.strength(c), members.iter().map(|&v| view.strength(v)).sum::<u64>());
            assert_eq!(q.id(c), NodeId(u64::from(c)));
        }
        let singletons: Vec<u32> = (0..q.len() as u32).collect();
        assert_eq!(modularity(&q, &singletons, Resolution::ONE), p.modularity);
        // Without self-loops: exactly twice the internal weight.
        let g = cliques(2, 4);
        let q = g.quotient(&[0, 0, 0, 0, 1, 1, 1, 1], 2);
        assert_eq!((q.self_weight(0), q.self_weight(1), q.total_weight()), (12, 12, 26));
        assert_eq!(q.neighbours(0), &[(1, 1)]);
        // A node without a label in range is left out; an unused id is a node of strength 0.
        let q = g.quotient(&[0, 0, 0, 0, 9, 9, 9], 3);
        assert_eq!((q.len(), q.self_weight(0), q.pair_count(), q.strength(2)), (3, 12, 0, 0));
    }

    #[test]
    fn cap_falls_back_to_lpa() {
        let g = karate();
        let capped = communities(&g, &CommunityOptions { leiden_edge_cap: 1, ..options(3) });
        assert_eq!(capped.method, Method::LabelPropagation);
        assert_eq!(capped, label_propagation(&g, &options(3)));
        let at_cap = communities(&g, &CommunityOptions { leiden_edge_cap: g.pair_count(), ..options(3) });
        assert_eq!(at_cap, leiden(&g, &options(3)));
        assert_eq!((communities(&g, &options(3)).method, Method::Leiden.name()), (Method::Leiden, "leiden"));
    }

    #[test]
    fn no_overflow_on_heavy_weights() {
        let mut pairs = Vec::new();
        for i in 0..10 {
            for j in i + 1..10 {
                pairs.push((i, j, u64::from(u32::MAX)));
            }
        }
        let clique = WeightedGraph::from_pairs(10, &pairs);
        let p = leiden(&clique, &options(0));
        assert_eq!((p.communities, p.modularity), (1, 0.0));
        let fine = Resolution { num: u32::MAX, den: 1 };
        assert_eq!(leiden(&clique, &CommunityOptions { resolution: fine, ..options(0) }).communities, 10);
        // Weights that saturate the u64 sums still finish.
        let saturated: Vec<(u32, u32, u64)> = pairs.iter().map(|&(a, b, _)| (a, b, u64::MAX)).collect();
        let g = WeightedGraph::from_pairs(10, &saturated);
        assert_eq!(g.total_weight(), u64::MAX);
        assert_connected(&g, &leiden(&g, &options(0)));
    }

    #[test]
    fn exact_gains_are_scale_invariant() {
        // Scaling every weight by 2^44 and gamma's terms by u32::MAX scales
        // each side of every decision alike, and pushes `den * 2m * k_v`
        // past i128: exact arithmetic gives the same partition.
        let huge = Resolution { num: u32::MAX, den: u32::MAX };
        let mut past_i128 = 0;
        for seed in 0..40 {
            let g = random_view(seed);
            let scaled_pairs: Vec<(u32, u32, u64)> = (0..g.len() as u32)
                .flat_map(|v| g.neighbours(v).iter().filter(move |&&(u, _)| u > v).map(move |&(u, w)| (v, u, w << 44)))
                .chain((0..g.len() as u32).map(|v| (v, v, (g.self_weight(v) / 2) << 44)))
                .collect();
            let scaled = WeightedGraph::from_pairs(g.len(), &scaled_pairs);
            assert_eq!(scaled.total_weight(), g.total_weight() << 44);
            let base = leiden(&g, &options(seed));
            let big = leiden(&scaled, &CommunityOptions { resolution: huge, ..options(seed) });
            assert_eq!((big.membership, big.levels), (base.membership, base.levels), "seed {seed}");
            let k_max = (0..scaled.len() as u32).map(|v| scaled.strength(v)).max().unwrap_or(0);
            let term = U256::mul(u128::from(u32::MAX) * u128::from(scaled.total_weight()), k_max);
            past_i128 += usize::from(term > U256 { hi: 0, lo: i128::MAX as u128 });
        }
        // Not vacuous: most of these views need more than i128 holds.
        assert!(past_i128 >= 20, "{past_i128}");
    }

    #[test]
    fn wide_arithmetic() {
        let max = U256::mul(u128::MAX, u64::MAX);
        // (2^128 - 1)(2^64 - 1) = (2^64 - 2) * 2^128 + (2^128 - 2^64 + 1).
        assert_eq!(max, U256 { hi: (1u128 << 64) - 2, lo: u128::MAX - (1u128 << 64) + 2 });
        assert_eq!(U256::mul(12_345, 678), U256 { hi: 0, lo: 12_345 * 678 });
        assert_eq!(U256::mul(1u128 << 100, 1 << 40), U256 { hi: 1 << 12, lo: 0 });
        let carry = U256 { hi: 0, lo: u128::MAX }.add(U256 { hi: 0, lo: 1 });
        assert_eq!(carry, U256 { hi: 1, lo: 0 });
        let g = |pos: u128, neg: u128| Gain { pos: U256::mul(pos, 1), neg: U256::mul(neg, 1) };
        assert_eq!(g(5, 3).compare(&g(10, 8)), Ordering::Equal);
        assert_eq!(g(5, 3).compare(&g(1, 0)), Ordering::Greater);
        assert_eq!(g(0, 7).compare(&g(0, 6)), Ordering::Less);
        assert!(g(4, 4).non_negative() && !g(3, 4).non_negative());
    }

    #[test]
    fn connectivity_pass_splits_components() {
        // Nodes 0-1 and 2-3 are two pairs, node 4 alone: one label for all
        // five becomes three connected pieces.
        let g = WeightedGraph::from_pairs(5, &[(0, 1, 1), (2, 3, 1)]);
        assert_eq!(split_disconnected(&g, &[0; 5]), vec![0, 0, 1, 1, 2]);
        assert_eq!(split_disconnected(&g, &[0, 1, 1, 1, 1]), vec![0, 1, 2, 2, 3]);
        assert_eq!(first_seen(&[7, 3, 7, 0]), (vec![0, 1, 0, 2], 3));
    }

    #[test]
    fn resolution_sets_granularity() {
        let g = cliques(4, 6);
        let p = leiden(&g, &options(1));
        let four: Vec<u32> = (0..24).map(|i| i / 6).collect();
        assert_eq!((p.membership.as_slice(), p.communities), (four.as_slice(), 4));
        // Gamma 1/100 merges the chain of cliques; that takes an aggregate level.
        let coarse = leiden(&g, &CommunityOptions { resolution: Resolution { num: 1, den: 100 }, ..options(1) });
        assert_eq!(coarse.communities, 1);
        assert!(coarse.levels >= 2, "{}", coarse.levels);
        let den0 = leiden(&g, &CommunityOptions { resolution: Resolution { num: 1, den: 0 }, ..options(1) });
        assert_eq!(den0, p, "den 0 reads as 1");
    }

    #[test]
    fn level_bounds_and_degenerate_views() {
        let g = karate();
        let none = leiden(&g, &CommunityOptions { max_levels: 0, ..options(0) });
        assert_eq!((none.membership, none.communities, none.levels), ((0..34).collect::<Vec<u32>>(), 34, 0));
        let empty = leiden(&WeightedGraph::from_pairs(0, &[]), &options(0));
        assert_eq!((empty.communities, empty.modularity, empty.levels), (0, 0.0, 1));
        // Isolated nodes (a self-loop is no edge to anyone) stay singletons.
        let g = WeightedGraph::from_pairs(6, &[(0, 1, 5), (2, 3, 1), (3, 4, 1), (2, 4, 1), (5, 5, 3)]);
        let p = leiden(&g, &options(1));
        assert_eq!((p.membership, p.communities), (vec![1, 1, 0, 0, 0, 2], 3));
    }
}
