//! Hubs (CD.4a): in / out degree per edge category over an [`Adjacency`],
//! and fixed-iteration HITS hub and authority scores in a deterministic
//! order.
//!
//! Domain-free like the rest of [`crate::algo`]: the categories that count
//! arrive as a [`CategorySet`], never a named kind, and every result is
//! index-aligned with the [`Adjacency`]'s dense ids (its nodes, then the
//! dangling edge endpoints it indexed; [`Adjacency::id`] maps an index back).
//!
//! [`degree_table`] reads each node's outgoing and incoming incidence lists
//! once, in index order: O(V + E log C) for C distinct counted categories (a
//! domain's registry size), with dense per-category scratch counters instead
//! of a map. [`hits`] runs Kleinberg's hub / authority iteration a fixed
//! number of rounds with no convergence test, visiting nodes in dense order
//! and each node's incidences in edge order, so every float sum is taken in
//! one fixed order and two runs over one index are bit-identical.
//!
//! `GLIA_ALGO_DEBUG=1` prints one `[algo] hubs degree ...` / `[algo] hubs
//! hits ...` line per call, beside the `[algo] adjacency` line.

use glia_core::EdgeCategoryId;

use super::{Adjacency, CategorySet};

/// In / out degree of every indexed id, over the counted categories.
///
/// A self-loop is one out and one in at its node; parallel edges each count.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DegreeTable {
    /// Counted edges leaving each dense index.
    pub out: Vec<u32>,
    /// Counted edges entering each dense index.
    pub inn: Vec<u32>,
    /// `(ix, category, out, in)`, sorted by `(ix, category id)`: one row per
    /// category with at least one counted incidence at `ix`.
    by_cat: Vec<(u32, EdgeCategoryId, u32, u32)>,
}

impl DegreeTable {
    /// `(category, out, in)` at dense index `ix`, sorted by category id. An
    /// index with no counted edge (or past the table) yields nothing.
    pub fn of(&self, ix: u32) -> impl Iterator<Item = (EdgeCategoryId, u32, u32)> + '_ {
        let lo = self.by_cat.partition_point(|r| r.0 < ix);
        let hi = lo + self.by_cat[lo..].partition_point(|r| r.0 == ix);
        self.by_cat[lo..hi].iter().map(|&(_, c, o, i)| (c, o, i))
    }

    /// The in-degree at percentile `p_milli` (per-mille: 990 is p99) of every
    /// indexed id, zero-degree ids included, by nearest rank. 0 is the
    /// minimum, 1000 (or more) the maximum; an empty table gives 0.
    pub fn percentile_in(&self, p_milli: u32) -> u32 {
        nearest_rank(&self.inn, p_milli)
    }

    /// [`Self::percentile_in`] over the out-degrees.
    pub fn percentile_out(&self, p_milli: u32) -> u32 {
        nearest_rank(&self.out, p_milli)
    }
}

/// Nearest-rank percentile: the value at rank `ceil(p / 1000 * n)` (at least
/// 1) of a sorted copy.
fn nearest_rank(xs: &[u32], p_milli: u32) -> u32 {
    if xs.is_empty() {
        return 0;
    }
    let mut sorted = xs.to_vec();
    sorted.sort_unstable();
    let n = sorted.len() as u64;
    let rank = (u64::from(p_milli.min(1000)) * n).div_ceil(1000).max(1);
    sorted[(rank - 1) as usize]
}

/// In / out degree per category of every id `adj` indexes, counting only the
/// edges whose category `count` contains. `adj` may keep more categories than
/// `count`: those edges stay indexed and are skipped here.
pub fn degree_table(adj: &Adjacency, count: &CategorySet) -> DegreeTable {
    let n = adj.len();
    // The counted categories present, sorted by id; a category's position is
    // its scratch slot. Every kept edge sits on exactly one outgoing list.
    let mut cats: Vec<EdgeCategoryId> = Vec::new();
    for ix in 0..n as u32 {
        for inc in adj.outgoing(ix) {
            if count.contains(inc.category)
                && let Err(at) = cats.binary_search_by_key(&inc.category.0, |c| c.0)
            {
                cats.insert(at, inc.category);
            }
        }
    }
    // Uncounted categories are absent from `cats`, so a miss means "skip".
    let slot = |c: EdgeCategoryId| cats.binary_search_by_key(&c.0, |x| x.0).ok();

    let mut out = vec![0u32; n];
    let mut inn = vec![0u32; n];
    let mut slot_out = vec![0u32; cats.len()];
    let mut slot_in = vec![0u32; cats.len()];
    let mut touched: Vec<usize> = Vec::new();
    let mut by_cat: Vec<(u32, EdgeCategoryId, u32, u32)> = Vec::new();
    for ix in 0..n as u32 {
        let i = ix as usize;
        for inc in adj.outgoing(ix) {
            let Some(s) = slot(inc.category) else { continue };
            if slot_out[s] == 0 && slot_in[s] == 0 {
                touched.push(s);
            }
            slot_out[s] += 1;
            out[i] += 1;
        }
        for inc in adj.incoming(ix) {
            let Some(s) = slot(inc.category) else { continue };
            if slot_out[s] == 0 && slot_in[s] == 0 {
                touched.push(s);
            }
            slot_in[s] += 1;
            inn[i] += 1;
        }
        // Slots follow category id order, so sorting them sorts the rows.
        touched.sort_unstable();
        for &s in &touched {
            by_cat.push((ix, cats[s], slot_out[s], slot_in[s]));
            slot_out[s] = 0;
            slot_in[s] = 0;
        }
        touched.clear();
    }

    if super::algo_debug() {
        let counted: u64 = out.iter().map(|&d| u64::from(d)).sum();
        eprintln!(
            "[algo] hubs degree nodes={n} counted={counted} of {} kept edges, categories={} rows={}",
            adj.kept_edges(),
            cats.len(),
            by_cat.len()
        );
    }
    DegreeTable { out, inn, by_cat }
}

/// HITS scores, index-aligned with the [`Adjacency`]'s dense ids. Each vector
/// has unit L2 norm, or is all zeros when no id has that role.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Hits {
    /// How strongly each id points at good authorities.
    pub hub: Vec<f64>,
    /// How strongly each id is pointed at by good hubs.
    pub authority: Vec<f64>,
    /// Rounds run: always the requested count.
    pub iterations: u32,
}

/// Kleinberg's HITS over every kept edge of `adj` (a parallel edge counts
/// once per edge), `iterations` rounds exactly.
///
/// Both vectors start at 1. Each round sets `authority[v]` to the sum of
/// `hub[u]` over the edges `u -> v`, L2-normalises it, then sets `hub[u]` to
/// the sum of the new `authority[v]` over `u -> v` and L2-normalises that. A
/// vector whose sum of squares is 0 stays all zeros, so an index with no kept
/// edge scores 0 everywhere. Zero rounds return the start vectors,
/// normalised (every id `1 / sqrt(n)`), unless there is no kept edge.
pub fn hits(adj: &Adjacency, iterations: u32) -> Hits {
    let n = adj.len();
    let mut hub = vec![0.0f64; n];
    let mut authority = vec![0.0f64; n];
    if adj.kept_edges() > 0 {
        hub.fill(1.0);
        authority.fill(1.0);
        normalise(&mut hub);
        normalise(&mut authority);
        for _ in 0..iterations {
            for v in 0..n as u32 {
                authority[v as usize] = adj.incoming(v).iter().fold(0.0, |s, inc| s + hub[inc.other as usize]);
            }
            normalise(&mut authority);
            for u in 0..n as u32 {
                hub[u as usize] = adj.outgoing(u).iter().fold(0.0, |s, inc| s + authority[inc.other as usize]);
            }
            normalise(&mut hub);
        }
    }

    if super::algo_debug() {
        eprintln!("[algo] hubs hits nodes={n} edges={} rounds={iterations}", adj.kept_edges());
    }
    Hits { hub, authority, iterations }
}

/// Scale `xs` to unit L2 norm, summing squares in index order; all zeros stay
/// zeros.
fn normalise(xs: &mut [f64]) {
    let norm = xs.iter().fold(0.0, |s, &x| s + x * x).sqrt();
    if norm > 0.0 {
        for x in xs.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use glia_core::{Edge, NodeId};

    use super::super::{ToyGraph, toy_edge};
    use super::*;

    const CALLS: EdgeCategoryId = EdgeCategoryId(1);
    const USES: EdgeCategoryId = EdgeCategoryId(2);

    fn ix(adj: &Adjacency, id: u64) -> u32 {
        adj.index_of(NodeId(id)).expect("indexed")
    }

    fn rows(t: &DegreeTable, ix: u32) -> Vec<(u32, u32, u32)> {
        t.of(ix).map(|(c, o, i)| (c.0, o, i)).collect()
    }

    /// 0 -> 1..=9 (CALLS) plus 5 -> 0 (USES).
    fn star() -> ToyGraph {
        let mut edges: Vec<Edge> = (1..=9).map(|t| toy_edge(0, t, CALLS.0)).collect();
        edges.push(toy_edge(5, 0, USES.0));
        ToyGraph { nodes: (0..=9).map(NodeId).collect(), edges }
    }

    #[test]
    fn star_degrees() {
        let adj = Adjacency::build(&star(), &CategorySet::all());
        let t = degree_table(&adj, &CategorySet::all());
        let (zero, five) = (ix(&adj, 0), ix(&adj, 5));
        assert_eq!((t.out[zero as usize], t.inn[zero as usize]), (9, 1));
        assert_eq!(rows(&t, zero), vec![(CALLS.0, 9, 0), (USES.0, 0, 1)]);
        assert_eq!((t.out[five as usize], t.inn[five as usize]), (1, 1));
        assert_eq!(rows(&t, five), vec![(CALLS.0, 0, 1), (USES.0, 1, 0)]);
        let three = ix(&adj, 3);
        assert_eq!(rows(&t, three), vec![(CALLS.0, 0, 1)]);
        assert_eq!(t.out.iter().sum::<u32>(), 10);
        assert_eq!(t.inn.iter().sum::<u32>(), 10);
        assert_eq!(t.of(adj.len() as u32).count(), 0, "past the table");
    }

    #[test]
    fn uncounted_categories_are_skipped() {
        let adj = Adjacency::build(&star(), &CategorySet::all());
        assert_eq!(adj.kept_edges(), 10, "the USES edge stays indexed");
        let t = degree_table(&adj, &CategorySet::of(&[CALLS]));
        let (zero, five) = (ix(&adj, 0), ix(&adj, 5));
        assert_eq!((t.out[zero as usize], t.inn[zero as usize]), (9, 0));
        assert_eq!(rows(&t, zero), vec![(CALLS.0, 9, 0)]);
        assert_eq!((t.out[five as usize], t.inn[five as usize]), (0, 1));
        assert_eq!(rows(&t, five), vec![(CALLS.0, 0, 1)]);
        let none = degree_table(&adj, &CategorySet::of(&[]));
        assert!(none.out.iter().chain(&none.inn).all(|&d| d == 0));
        assert_eq!(none.of(zero).count(), 0);
    }

    #[test]
    fn self_loops_and_dangling_endpoints() {
        // 1 -> 1 twice (parallel self-loops); 1 -> 7, and 7 is no node.
        let g = ToyGraph {
            nodes: vec![NodeId(1)],
            edges: vec![toy_edge(1, 1, CALLS.0), toy_edge(1, 1, CALLS.0), toy_edge(1, 7, USES.0)],
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let t = degree_table(&adj, &CategorySet::all());
        let (one, seven) = (ix(&adj, 1), ix(&adj, 7));
        assert_eq!(t.out.len(), 2);
        assert_eq!(rows(&t, one), vec![(CALLS.0, 2, 2), (USES.0, 1, 0)]);
        assert_eq!(rows(&t, seven), vec![(USES.0, 0, 1)]);
        let h = hits(&adj, 20);
        assert!(h.authority[seven as usize] > 0.0, "a dangling endpoint is an authority like any node");
    }

    #[test]
    fn percentiles() {
        let t = DegreeTable { out: (1..=100).collect(), inn: (1..=100).rev().collect(), by_cat: Vec::new() };
        assert_eq!(t.percentile_out(990), 99);
        assert_eq!(t.percentile_in(990), 99);
        assert_eq!(t.percentile_out(500), 50);
        assert_eq!(t.percentile_out(0), 1);
        assert_eq!(t.percentile_out(1000), 100);
        assert_eq!(t.percentile_out(5000), 100, "clamped to the maximum");
        let small = DegreeTable { out: vec![3, 1, 2], inn: vec![0, 0, 7], by_cat: Vec::new() };
        assert_eq!(small.percentile_out(500), 2);
        assert_eq!(small.percentile_in(500), 0, "zero-degree ids count");
        assert_eq!(small.percentile_in(990), 7);
        assert_eq!(DegreeTable::default().percentile_in(990), 0);
    }

    /// Hubs a, b -> authorities x, y, z; c -> x. Ids: a=1 b=2 c=3 x=10 y=11 z=12.
    #[test]
    fn hits_bipartite() {
        let mut edges = Vec::new();
        for hub in [1, 2] {
            for auth in [10, 11, 12] {
                edges.push(toy_edge(hub, auth, CALLS.0));
            }
        }
        edges.push(toy_edge(3, 10, CALLS.0));
        let g = ToyGraph { nodes: [1, 2, 3, 10, 11, 12].map(NodeId).to_vec(), edges };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let h = hits(&adj, 20);
        assert_eq!(h.iterations, 20);
        let at = |id| ix(&adj, id) as usize;
        let (a, b, c, x, y, z) = (at(1), at(2), at(3), at(10), at(11), at(12));
        let max_auth = h.authority.iter().copied().fold(f64::MIN, f64::max);
        assert_eq!(h.authority[x], max_auth);
        assert!(h.authority[x] > h.authority[y]);
        assert_eq!(h.authority[y].to_bits(), h.authority[z].to_bits());
        assert_eq!(h.hub[a].to_bits(), h.hub[b].to_bits());
        assert!(h.hub[a] > h.hub[c] && h.hub[c] > 0.0);
        for pure_hub in [a, b, c] {
            assert_eq!(h.authority[pure_hub], 0.0);
        }
        for pure_auth in [x, y, z] {
            assert_eq!(h.hub[pure_auth], 0.0);
        }
        // Known answer: authority is the principal eigenvector of A^T A =
        // [[3,2,2],[2,2,2],[2,2,2]], which is (p, 1, 1) with 2p^2 + p - 4 = 0.
        let p = (33f64.sqrt() - 1.0) / 4.0;
        assert!((h.authority[x] / h.authority[y] - p).abs() < 1e-9);
        let unit = |v: &[f64]| (v.iter().map(|s| s * s).sum::<f64>() - 1.0).abs() < 1e-12;
        assert!(unit(&h.hub) && unit(&h.authority));
    }

    #[test]
    fn hits_zero_rounds_is_the_normalised_start() {
        let adj = Adjacency::build(&star(), &CategorySet::all());
        let h = hits(&adj, 0);
        let start = 1.0 / (adj.len() as f64).sqrt();
        assert!(h.hub.iter().chain(&h.authority).all(|&s| (s - start).abs() < 1e-15));
        assert_eq!(h.iterations, 0);
    }

    #[test]
    fn hits_deterministic() {
        let mut rng = Lcg(0x5EED);
        for _ in 0..50 {
            let g = random_graph(&mut rng);
            let first = hits(&Adjacency::build(&g, &CategorySet::all()), 20);
            let second = hits(&Adjacency::build(&g, &CategorySet::all()), 20);
            let bits = |h: &Hits| -> Vec<u64> { h.hub.iter().chain(&h.authority).map(|s| s.to_bits()).collect() };
            assert_eq!(bits(&first), bits(&second));
            assert!(first.hub.iter().chain(&first.authority).all(|s| s.is_finite() && *s >= 0.0));
        }
    }

    #[test]
    fn empty_graph_zeros() {
        let g = ToyGraph { nodes: vec![NodeId(1), NodeId(2), NodeId(3)], edges: Vec::new() };
        let adj = Adjacency::build(&g, &CategorySet::all());
        for rounds in [0, 1, 20] {
            let h = hits(&adj, rounds);
            assert_eq!(h.hub, vec![0.0; 3]);
            assert_eq!(h.authority, vec![0.0; 3]);
            assert_eq!(h.iterations, rounds);
        }
        let t = degree_table(&adj, &CategorySet::all());
        assert_eq!((t.out.clone(), t.inn.clone()), (vec![0; 3], vec![0; 3]));
        assert_eq!((t.percentile_in(990), t.percentile_out(990)), (0, 0));
        assert_eq!(t.of(0).count(), 0);

        let nothing = Adjacency::build(&ToyGraph { nodes: Vec::new(), edges: Vec::new() }, &CategorySet::all());
        assert_eq!(hits(&nothing, 20), Hits { hub: Vec::new(), authority: Vec::new(), iterations: 20 });
        assert_eq!(degree_table(&nothing, &CategorySet::all()), DegreeTable::default());
        // Edges the index dropped (category 2 not kept) score nothing either.
        let dropped = Adjacency::build(&star(), &CategorySet::of(&[EdgeCategoryId(3)]));
        assert!(hits(&dropped, 5).hub.iter().all(|&s| s == 0.0));
    }

    /// The table against a scan of the raw edge list: every kept edge whose
    /// category is also counted adds one out at `from` and one in at `to`.
    #[test]
    fn degree_matches_an_edge_scan() {
        let mut rng = Lcg(0xDE6);
        for _ in 0..200 {
            let g = random_graph(&mut rng);
            let keep = random_set(&mut rng);
            let count = random_set(&mut rng);
            let adj = Adjacency::build(&g, &keep);
            let t = degree_table(&adj, &count);
            let mut want: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
            let (mut out, mut inn) = (vec![0u32; adj.len()], vec![0u32; adj.len()]);
            for e in &g.edges {
                if !keep.contains(e.category) || !count.contains(e.category) {
                    continue;
                }
                let (f, to) = (ix(&adj, e.from.0), ix(&adj, e.to.0));
                want.entry((f, e.category.0)).or_default().0 += 1;
                want.entry((to, e.category.0)).or_default().1 += 1;
                out[f as usize] += 1;
                inn[to as usize] += 1;
            }
            assert_eq!((&t.out, &t.inn), (&out, &inn));
            let got: BTreeMap<(u32, u32), (u32, u32)> = (0..adj.len() as u32)
                .flat_map(|i| t.of(i).map(move |(c, o, n)| ((i, c.0), (o, n))))
                .collect();
            assert_eq!(got, want);
            assert_eq!(t.by_cat.len(), want.len(), "one row per (ix, category)");
        }
    }

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }

        fn chance(&mut self, percent: u64) -> bool {
            self.below(100) < percent
        }
    }

    const CATEGORIES: u64 = 4;

    /// 1..=30 nodes with scattered ids, 0..=90 edges over 4 categories, with
    /// self-loops, parallel edges and ~5% dangling endpoints.
    fn random_graph(rng: &mut Lcg) -> ToyGraph {
        let salt = rng.next();
        let scatter = |i: u64| NodeId(i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt);
        let n = 1 + rng.below(30);
        let nodes: Vec<NodeId> = (0..n).map(scatter).collect();
        let mut edges: Vec<Edge> = Vec::new();
        for _ in 0..rng.below(91) {
            let mut e = toy_edge(0, 0, 1 + rng.below(CATEGORIES) as u32);
            if !edges.is_empty() && rng.chance(8) {
                let prior = &edges[rng.below(edges.len() as u64) as usize];
                (e.from, e.to) = (prior.from, prior.to);
            } else {
                let mut pick = || if rng.chance(5) { scatter(1_000 + rng.below(2)) } else { scatter(rng.below(n)) };
                e.from = pick();
                e.to = pick();
                if rng.chance(6) {
                    e.to = e.from;
                }
            }
            edges.push(e);
        }
        ToyGraph { nodes, edges }
    }

    /// Every category, or a random subset (possibly empty).
    fn random_set(rng: &mut Lcg) -> CategorySet {
        if rng.chance(40) {
            return CategorySet::all();
        }
        let some: Vec<EdgeCategoryId> =
            (1..=CATEGORIES as u32).map(EdgeCategoryId).filter(|_| rng.chance(60)).collect();
        CategorySet::of(&some)
    }
}
