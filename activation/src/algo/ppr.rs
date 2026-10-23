//! Approximate personalised PageRank by push (Andersen, Chung and Lang,
//! "Local graph partitioning using PageRank vectors", FOCS 2006) over an
//! [`Adjacency`] (DE.5): one seed's PPR vector computed locally, with work
//! bounded by the push tolerance instead of the graph's size. Its caller is
//! the ranking harness (engine/examples/rank_eval.rs, the P3 ranker); 0.5.2's
//! Engram side file that used it was removed in 0.5.3. No glia read path
//! calls it, and the whole-graph power iteration behind
//! [`activate`](crate::activate) is untouched.
//!
//! The walk is the **undirected** view of the index: a node's kept outgoing
//! and incoming edges, merged by edge position, a self-loop (on both lists)
//! taken once. Each incidence carries `weight(category)`; a node's weighted
//! degree `d(u)` is the sum over its incidences. From `u` the walk stops with
//! probability `alpha` (the restart probability: glia's damping 0.5 is
//! `alpha` 0.5) and otherwise steps along an incidence with probability
//! `weight / d(u)`. A self-loop contributes its weight to `d(u)` once and
//! steps back to `u`.
//!
//! The push keeps a settled vector `p` and a residual `r`, starting at
//! `r = seed`. Pushing `u` moves `alpha * r[u]` into `p[u]` and spreads
//! `(1 - alpha) * r[u] * weight / d(u)` to each incidence's other end, then
//! `r[u]` is 0 (before the spread, so a self-loop's share stays). A node is
//! pushed while `r[u] >= epsilon * d(u)`; the seed's first push is
//! unconditional, so a hub seed (`d(seed) > 1 / epsilon`) still reaches its
//! neighbours. Every push keeps `p + ppr(r) = ppr(seed)` and conserves mass
//! (`sum p + sum r = 1`, [`PushStats::residual`] is `sum r`).
//!
//! Guarantees, for weights that are symmetric by construction (each edge
//! weighs the same at both ends):
//! - **Error.** At termination every `r[v] < epsilon * d(v)`, so
//!   `0 <= ppr(v) - p(v) <= epsilon * d(v)` for every node `v`.
//! - **Work.** Each push past the seed's first settles at least
//!   `alpha * epsilon * d(u)`, and `sum p <= 1`, so the weighted degrees of
//!   the pushed nodes sum to at most `1 / (epsilon * alpha)`: at most that
//!   many pushes divided by the least positive degree, and as many
//!   incidences divided by the least positive weight. Neither depends on the
//!   graph's size.
//!
//! A node of degree 0 (an isolated seed, or one whose every incidence weighs
//! 0) cannot spread: it keeps its residual as score. A weight that is not a
//! finite positive number counts as 0 (the incidence is not walked).
//!
//! Determinism: the queue is FIFO, incidences are visited in edge order
//! ([`Adjacency`] parity rule 2), degrees are summed in that order and the
//! result is sorted by score descending then dense index (`f64::total_cmp`),
//! so the output is a pure function of the index, the weights and the two
//! parameters; no map is iterated. [`PushPpr`] reuses its scratch across
//! seeds and resets only what a run touched, so one seed costs its pushes,
//! not `O(V)`.

use std::collections::VecDeque;

use glia_core::{EdgeCategoryId, NodeId};

use super::{Adjacency, Inc};

/// What one [`PushPpr::run`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct PushStats {
    /// Nodes pushed (a node pushed twice counts twice), the seed's first push
    /// included; 0 when the seed is not indexed or a parameter is invalid.
    pub pushes: usize,
    /// The residual mass left unsettled, `1 - sum p`: each node's share is
    /// below `epsilon * d(v)`.
    pub residual: f64,
}

/// One seed's approximate PPR over the undirected, weighted view of `adj`:
/// every `(id, score)` with score > 0, by score descending then dense index.
/// Empty when `seed` is not indexed, `alpha` is outside `(0, 1]` or
/// `epsilon` is not a finite positive number.
///
/// Builds the degree table (`O(V + E)`) per call; a caller with many seeds
/// over one index builds a [`PushPpr`] once and calls [`PushPpr::run`].
pub fn push_ppr(
    adj: &Adjacency,
    seed: NodeId,
    weight: &dyn Fn(EdgeCategoryId) -> f64,
    alpha: f64,
    epsilon: f64,
) -> Vec<(NodeId, f64)> {
    PushPpr::new(adj, weight).run(seed, alpha, epsilon).0
}

/// A reusable push over one [`Adjacency`] and one weighting: the weighted
/// degrees, computed once, and dense scratch vectors that each run resets
/// only where it wrote.
pub struct PushPpr<'a> {
    adj: &'a Adjacency,
    weight: &'a dyn Fn(EdgeCategoryId) -> f64,
    /// Weighted undirected degree per dense index.
    degree: Vec<f64>,
    p: Vec<f64>,
    r: Vec<f64>,
    queued: Vec<bool>,
    /// Dense indices a run wrote `p` or `r` at, in first-touch order.
    touched: Vec<u32>,
    seen: Vec<bool>,
    queue: VecDeque<u32>,
}

impl<'a> PushPpr<'a> {
    /// Index `adj`'s weighted degrees under `weight`.
    pub fn new(adj: &'a Adjacency, weight: &'a dyn Fn(EdgeCategoryId) -> f64) -> Self {
        let n = adj.len();
        let mut degree = vec![0.0; n];
        for (ix, d) in degree.iter_mut().enumerate() {
            undirected(adj, ix as u32, |inc| *d += sane(weight(inc.category)));
        }
        Self {
            adj,
            weight,
            degree,
            p: vec![0.0; n],
            r: vec![0.0; n],
            queued: vec![false; n],
            touched: Vec::new(),
            seen: vec![false; n],
            queue: VecDeque::new(),
        }
    }

    /// The weighted degree of `id` (0 when it is not indexed).
    pub fn degree(&self, id: NodeId) -> f64 {
        self.adj.index_of(id).map_or(0.0, |ix| self.degree[ix as usize])
    }

    /// [`push_ppr`] from `seed`, with what the run did.
    pub fn run(&mut self, seed: NodeId, alpha: f64, epsilon: f64) -> (Vec<(NodeId, f64)>, PushStats) {
        let mut stats = PushStats::default();
        let valid = alpha > 0.0 && alpha <= 1.0 && epsilon > 0.0 && epsilon.is_finite();
        let Some(s) = self.adj.index_of(seed).filter(|_| valid) else {
            return (Vec::new(), stats);
        };
        self.touch(s);
        self.r[s as usize] = 1.0;
        self.queued[s as usize] = true;
        self.queue.push_back(s);
        let mut first = true;
        while let Some(u) = self.queue.pop_front() {
            let ui = u as usize;
            self.queued[ui] = false;
            let (ru, du) = (self.r[ui], self.degree[ui]);
            if !std::mem::take(&mut first) && ru < epsilon * du {
                continue;
            }
            if ru <= 0.0 {
                continue;
            }
            stats.pushes += 1;
            self.r[ui] = 0.0;
            if du <= 0.0 {
                self.p[ui] += ru;
                continue;
            }
            self.p[ui] += alpha * ru;
            let spread = (1.0 - alpha) * ru / du;
            if spread <= 0.0 {
                continue;
            }
            let (adj, weight) = (self.adj, self.weight);
            undirected(adj, u, |inc| {
                let w = sane(weight(inc.category));
                if w <= 0.0 {
                    return;
                }
                let v = inc.other;
                let vi = v as usize;
                self.touch(v);
                self.r[vi] += spread * w;
                if !self.queued[vi] && self.r[vi] >= epsilon * self.degree[vi] {
                    self.queued[vi] = true;
                    self.queue.push_back(v);
                }
            });
        }

        let mut out: Vec<(u32, f64)> = Vec::new();
        let mut residual = 0.0;
        for &ix in &self.touched {
            let i = ix as usize;
            residual += self.r[i];
            if self.p[i] > 0.0 {
                out.push((ix, self.p[i]));
            }
            self.p[i] = 0.0;
            self.r[i] = 0.0;
            self.seen[i] = false;
        }
        self.touched.clear();
        stats.residual = residual;
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let ranked = out.into_iter().map(|(ix, p)| (self.adj.id(ix), p)).collect();
        (ranked, stats)
    }

    fn touch(&mut self, ix: u32) {
        let i = ix as usize;
        if !self.seen[i] {
            self.seen[i] = true;
            self.touched.push(ix);
        }
    }
}

/// A weight the walk can use: finite and positive, else 0.
fn sane(w: f64) -> f64 {
    if w.is_finite() && w > 0.0 { w } else { 0.0 }
}

/// Hand `step` every undirected incidence of `ix` in edge order: the out-list
/// and the in-list merged by edge position, a self-loop (on both) once.
fn undirected(adj: &Adjacency, ix: u32, mut step: impl FnMut(&Inc)) {
    let (outs, ins) = (adj.outgoing(ix), adj.incoming(ix));
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

#[cfg(test)]
mod tests {
    use super::super::{CategorySet, ToyGraph, toy_edge};
    use super::*;

    /// Category 1 weighs 1.0, 2 weighs 0.5, 3 weighs 0.25, anything else 0.
    fn weight(c: EdgeCategoryId) -> f64 {
        match c.0 {
            1 => 1.0,
            2 => 0.5,
            3 => 0.25,
            _ => 0.0,
        }
    }

    /// Six nodes: a triangle 1-2-3 with a parallel 1-2 edge and a self-loop
    /// at 2, a bridge 3-4, a triangle 4-5-6, and a light chord 5-1.
    fn toy() -> ToyGraph {
        ToyGraph {
            nodes: (1..=6).map(NodeId).collect(),
            edges: vec![
                toy_edge(1, 2, 1),
                toy_edge(2, 3, 1),
                toy_edge(1, 3, 2),
                toy_edge(3, 4, 3),
                toy_edge(4, 5, 1),
                toy_edge(5, 6, 2),
                toy_edge(6, 4, 1),
                toy_edge(2, 2, 2),
                toy_edge(1, 2, 3),
                toy_edge(5, 1, 3),
            ],
        }
    }

    /// Dense power iteration of the same walk: `x = alpha s + (1 - alpha) x W`
    /// with `W[u][v] = weight(u, v) / d(u)`, a self-loop once, a degree-0 node
    /// keeping its mass. Indexed like `g.nodes`.
    fn oracle(g: &ToyGraph, seed: NodeId, alpha: f64) -> Vec<f64> {
        let n = g.nodes.len();
        let at = |id: NodeId| g.nodes.iter().position(|&x| x == id).unwrap();
        let mut m = vec![vec![0.0; n]; n];
        for e in &g.edges {
            let (a, b, w) = (at(e.from), at(e.to), weight(e.category));
            m[a][b] += w;
            if a != b {
                m[b][a] += w;
            }
        }
        let d: Vec<f64> = m.iter().map(|row| row.iter().sum()).collect();
        let mut s = vec![0.0; n];
        s[at(seed)] = 1.0;
        let mut x = s.clone();
        for _ in 0..400 {
            let mut y: Vec<f64> = s.iter().map(|v| alpha * v).collect();
            for u in 0..n {
                if d[u] == 0.0 {
                    y[u] += (1.0 - alpha) * x[u];
                    continue;
                }
                for v in 0..n {
                    y[v] += (1.0 - alpha) * x[u] * m[u][v] / d[u];
                }
            }
            x = y;
        }
        x
    }

    fn scores(g: &ToyGraph, got: &[(NodeId, f64)]) -> Vec<f64> {
        let mut v = vec![0.0; g.nodes.len()];
        for &(id, p) in got {
            v[g.nodes.iter().position(|&x| x == id).unwrap()] = p;
        }
        v
    }

    fn top(g: &ToyGraph, s: &[f64], k: usize) -> Vec<NodeId> {
        let mut ix: Vec<usize> = (0..s.len()).collect();
        ix.sort_by(|&a, &b| s[b].total_cmp(&s[a]).then(a.cmp(&b)));
        ix.into_iter().take(k).map(|i| g.nodes[i]).collect()
    }

    #[test]
    fn tight_epsilon_matches_power_iteration() {
        let g = toy();
        let adj = Adjacency::build(&g, &CategorySet::all());
        for seed in [1, 4, 6] {
            let want = oracle(&g, NodeId(seed), 0.5);
            let got = scores(&g, &push_ppr(&adj, NodeId(seed), &weight, 0.5, 1e-9));
            for (i, (w, p)) in want.iter().zip(&got).enumerate() {
                assert!((w - p).abs() <= 1e-6, "seed {seed} node {}: push {p} vs power {w}", g.nodes[i].0);
            }
        }
    }

    #[test]
    fn loose_epsilon_keeps_the_top_three_and_the_error_bound() {
        let g = toy();
        let adj = Adjacency::build(&g, &CategorySet::all());
        let mut push = PushPpr::new(&adj, &weight);
        for seed in [1, 4, 6] {
            let want = oracle(&g, NodeId(seed), 0.5);
            let (ranked, stats) = push.run(NodeId(seed), 0.5, 1e-3);
            let got = scores(&g, &ranked);
            assert_eq!(top(&g, &got, 3), top(&g, &want, 3), "seed {seed}");
            for (i, (w, p)) in want.iter().zip(&got).enumerate() {
                let bound = 1e-3 * push.degree(g.nodes[i]);
                assert!(*w - p >= -1e-12 && w - p <= bound, "seed {seed} node {}: {p} vs {w}", g.nodes[i].0);
            }
            // At most 1 / (epsilon * alpha) weighted degree is pushed; the
            // least degree here is 1.5.
            assert!(stats.pushes as f64 <= 1.0 + 1.0 / (1e-3 * 0.5 * 1.5), "{stats:?}");
            let settled: f64 = ranked.iter().map(|r| r.1).sum();
            assert!((settled + stats.residual - 1.0).abs() < 1e-12, "mass: {settled} + {}", stats.residual);
        }
    }

    #[test]
    fn a_dangling_seed_keeps_its_mass() {
        // 7 is a node with no edge; 9 is reached only through a weight-0 edge.
        let mut g = toy();
        g.nodes.push(NodeId(7));
        g.nodes.push(NodeId(8));
        g.edges.push(toy_edge(8, 9, 99));
        let adj = Adjacency::build(&g, &CategorySet::all());
        let (got, stats) = PushPpr::new(&adj, &weight).run(NodeId(7), 0.5, 1e-3);
        assert_eq!(got, vec![(NodeId(7), 1.0)]);
        assert_eq!(stats.pushes, 1);
        assert_eq!(stats.residual, 0.0);
        assert_eq!(push_ppr(&adj, NodeId(8), &weight, 0.5, 1e-3), vec![(NodeId(8), 1.0)]);
        // A seed with edges settles 1 - residual, every score positive.
        let (got, stats) = PushPpr::new(&adj, &weight).run(NodeId(1), 0.5, 1e-3);
        let settled: f64 = got.iter().map(|r| r.1).sum();
        assert!((settled - (1.0 - stats.residual)).abs() < 1e-12);
        assert!(got.iter().all(|r| r.1 > 0.0) && got[0].0 == NodeId(1));
    }

    #[test]
    fn a_dangling_endpoint_is_walked() {
        // 9 is no node: the edge 2 -> 9 is still indexed (parity rule 1).
        let g = ToyGraph { nodes: vec![NodeId(1), NodeId(2)], edges: vec![toy_edge(1, 2, 1), toy_edge(2, 9, 1)] };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let got = push_ppr(&adj, NodeId(1), &weight, 0.5, 1e-9);
        let ids: Vec<u64> = got.iter().map(|r| r.0.0).collect();
        assert_eq!(ids, vec![1, 2, 9]);
        let want = oracle(&ToyGraph { nodes: vec![NodeId(1), NodeId(2), NodeId(9)], edges: g.edges.clone() }, NodeId(1), 0.5);
        for (r, w) in got.iter().zip(&want) {
            assert!((r.1 - w).abs() <= 1e-6, "{r:?} vs {w}");
        }
    }

    #[test]
    fn a_hub_seed_is_pushed_once_at_least() {
        // d(0) = 1500 > 1 / epsilon: below its threshold from the start, so
        // only the unconditional first push settles anything at the seed.
        let g = ToyGraph {
            nodes: (0..=1500).map(NodeId).collect(),
            edges: (1..=1500).map(|t| toy_edge(0, t, 1)).collect(),
        };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let mut push = PushPpr::new(&adj, &weight);
        let (got, stats) = push.run(NodeId(0), 0.5, 1e-3);
        // Each leaf holds 1 / 3000 < epsilon * d(leaf): residual, no score.
        assert_eq!(got, vec![(NodeId(0), 0.5)]);
        assert_eq!(stats.pushes, 1);
        assert!((stats.residual - 0.5).abs() < 1e-12, "{stats:?}");
        // A tighter epsilon pushes every leaf once; the leaves tie bit for
        // bit and the tie goes by dense index.
        let (got, _) = push.run(NodeId(0), 0.5, 1e-4);
        assert_eq!(got.len(), 1501);
        assert_eq!(got[0].0, NodeId(0));
        assert!(got[1..].iter().all(|r| r.1.to_bits() == got[1].1.to_bits()), "leaves tie");
        let leaves: Vec<u64> = got[1..].iter().map(|r| r.0.0).collect();
        assert_eq!(leaves, (1..=1500).collect::<Vec<u64>>());
    }

    #[test]
    fn invalid_input_is_empty_not_a_panic() {
        let g = toy();
        let adj = Adjacency::build(&g, &CategorySet::all());
        assert!(push_ppr(&adj, NodeId(42), &weight, 0.5, 1e-3).is_empty());
        for (alpha, eps) in [(0.0, 1e-3), (1.5, 1e-3), (f64::NAN, 1e-3), (0.5, 0.0), (0.5, f64::INFINITY), (0.5, -1.0)] {
            assert!(push_ppr(&adj, NodeId(1), &weight, alpha, eps).is_empty(), "{alpha} {eps}");
        }
        // alpha 1 settles everything at the seed.
        assert_eq!(push_ppr(&adj, NodeId(1), &weight, 1.0, 1e-3), vec![(NodeId(1), 1.0)]);
        // A non-finite or negative weight is not walked.
        let odd = |c: EdgeCategoryId| if c.0 == 1 { f64::NAN } else { -1.0 };
        assert_eq!(push_ppr(&adj, NodeId(1), &odd, 0.5, 1e-3), vec![(NodeId(1), 1.0)]);
        assert!(Adjacency::default().is_empty());
        assert!(push_ppr(&Adjacency::default(), NodeId(1), &weight, 0.5, 1e-3).is_empty());
    }

    #[test]
    fn runs_are_bit_identical_and_scratch_resets() {
        let g = toy();
        let adj = Adjacency::build(&g, &CategorySet::all());
        let bits = |v: &[(NodeId, f64)]| v.iter().map(|r| (r.0, r.1.to_bits())).collect::<Vec<_>>();
        let mut reused = PushPpr::new(&adj, &weight);
        for seed in [1, 4, 6, 1, 3] {
            let fresh = push_ppr(&adj, NodeId(seed), &weight, 0.5, 1e-3);
            let again = push_ppr(&adj, NodeId(seed), &weight, 0.5, 1e-3);
            let (from_reused, _) = reused.run(NodeId(seed), 0.5, 1e-3);
            assert_eq!(bits(&fresh), bits(&again), "seed {seed}");
            assert_eq!(bits(&fresh), bits(&from_reused), "seed {seed}: reused scratch");
        }
    }

    #[test]
    fn a_self_loop_counts_once() {
        // 1 - 2 plus a self-loop at 2: d(2) = 1 + 0.5, not 1 + 2 * 0.5.
        let g = ToyGraph { nodes: vec![NodeId(1), NodeId(2)], edges: vec![toy_edge(1, 2, 1), toy_edge(2, 2, 2)] };
        let adj = Adjacency::build(&g, &CategorySet::all());
        let push = PushPpr::new(&adj, &weight);
        assert_eq!(push.degree(NodeId(2)), 1.5);
        assert_eq!(push.degree(NodeId(1)), 1.0);
        assert_eq!(push.degree(NodeId(42)), 0.0);
        let want = oracle(&g, NodeId(1), 0.5);
        let got = scores(&g, &push_ppr(&adj, NodeId(1), &weight, 0.5, 1e-10));
        for (w, p) in want.iter().zip(&got) {
            assert!((w - p).abs() <= 1e-6, "{p} vs {w}");
        }
    }
}
