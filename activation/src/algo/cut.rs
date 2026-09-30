//! Minimum cuts (CD.2a): the Stoer-Wagner global minimum cut, with every
//! phase cut it computes on the way, and the Dinic s-t maximum flow / minimum
//! cut, both with `u64` capacities over the weighted undirected view
//! [`WeightedGraph`] that [`community`](crate::algo::community) builds.
//! Domain-free: a capacity is the view's merged pair weight, which the
//! caller's category weights made, never a named kind. Self weights never
//! cross a cut and are ignored.
//!
//! [`stoer_wagner`] runs the `n - 1` maximum-adjacency phases. Each phase
//! grows a set from the lowest-id super-vertex by repeatedly adding the
//! super-vertex most tightly connected to it (a binary heap keyed
//! connectivity descending, smallest member id ascending, with lazy
//! deletion; a super-vertex no pair joins to the set waits outside the heap
//! and is taken in id order once the heap runs dry), records the cut
//! between the last super-vertex added and the rest, and merges the last
//! two. Merged vertices keep one member list and one sorted neighbour row
//! each, so a phase costs O(V' + E' log V') on the current V' super-vertices
//! and E' super-pairs: O(V E log V) at worst over the run (release build,
//! sparse random views of average degree 6: 1,000 nodes 0.1 s, 3,000 nodes
//! 1-2 s, 10,000 nodes about 20 s).
//!
//! Raw global minimum cuts are unbalanced on code graphs (the cheapest cut
//! peels one leaf), so every phase cut is returned for a caller to choose a
//! balanced one from. Their sides together hold O(V^2) ids at worst (on a
//! path, phase `k`'s side holds `k` nodes: 20,000 nodes hold 2 * 10^8 ids,
//! 800 MB).
//!
//! [`min_st_cut`] turns each pair into two arcs of capacity `w`, each the
//! other's reverse, joins the sources to a super source and the sinks to a
//! super sink with capacity `u64::MAX`, and runs Dinic: a BFS level graph,
//! then a blocking flow found by an explicit DFS stack with per-node
//! current-arc pointers, never recursion, so a 100,000-node path runs in a
//! debug build. Every add saturates. The cut is the residual graph's
//! source-reachable set.
//!
//! Determinism: both walk [`WeightedGraph::neighbours`]' sorted rows and
//! break every tie by dense id, and every sum is a saturating sum of
//! non-negative `u64`s, which no visiting order changes, so two runs over one
//! view are identical.
//!
//! `GLIA_ALGO_DEBUG=1` prints one `[algo] cut stoer_wagner ...` /
//! `[algo] cut dinic ...` line per call.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use super::community::WeightedGraph;

/// One Stoer-Wagner phase's cut: the phase's last super-vertex against the
/// rest of the view.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PhaseCut {
    /// The weight of every pair with exactly one end in `side`.
    pub weight: u64,
    /// The dense indices merged into the phase's last super-vertex, sorted.
    pub side: Vec<u32>,
}

/// A global minimum cut of a [`WeightedGraph`], with the phase cuts it was
/// chosen from.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MinCut {
    /// The least phase weight: the global minimum cut's weight.
    pub weight: u64,
    /// Per dense index: `true` for the members of the winning phase's last
    /// super-vertex. The winning phase is the earliest of least weight.
    pub side: Vec<bool>,
    /// Every phase's cut, in phase order: one fewer than the view's nodes.
    pub phases: Vec<PhaseCut>,
}

/// A minimum s-t cut, with the maximum flow that proves it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StCut {
    /// The maximum flow from the sources to the sinks: the cut's weight.
    pub flow: u64,
    /// Per dense index: `true` when the index is reachable from a source in
    /// the residual graph. Every source is on this side and no sink is.
    pub source_side: Vec<bool>,
    /// The pairs `(a, b)`, `a < b`, with exactly one end on the source side,
    /// sorted. Their weights sum to `flow`.
    pub cut_pairs: Vec<(u32, u32)>,
}

/// The Stoer-Wagner global minimum cut of `g`, with every phase cut. `None`
/// when `g` has fewer than two nodes.
///
/// A disconnected view yields weight 0: the first phase whose last
/// super-vertex has no pair to the rest wins, and its side is one connected
/// component (the maximum-adjacency order exhausts a component before it
/// starts the next, so the last super-vertex of that phase is the whole last
/// component).
pub fn stoer_wagner(g: &WeightedGraph) -> Option<MinCut> {
    let n = g.len();
    if n < 2 {
        return None;
    }
    // A super-vertex is labelled by its smallest member, so the label is
    // also the heap's tie-break and `alive` stays in ascending order.
    let mut adj: Vec<Vec<(u32, u64)>> = (0..n as u32).map(|v| g.neighbours(v).to_vec()).collect();
    let mut members: Vec<Vec<u32>> = (0..n as u32).map(|v| vec![v]).collect();
    let mut alive: Vec<u32> = (0..n as u32).collect();
    let mut key = vec![0u64; n];
    let mut added = vec![false; n];
    let mut heap: BinaryHeap<(u64, Reverse<u32>)> = BinaryHeap::new();
    let mut phases: Vec<PhaseCut> = Vec::with_capacity(n - 1);
    let mut best = 0usize;

    while alive.len() > 1 {
        for &v in &alive {
            key[v as usize] = 0;
            added[v as usize] = false;
        }
        heap.clear();
        let (mut prev, mut last, mut cut) = (0u32, 0u32, 0u64);
        let (mut count, mut scan) = (0usize, 0usize);
        while count < alive.len() {
            let v = match heap.pop() {
                // Keys only grow within a phase: a smaller one is stale.
                Some((k, Reverse(v))) if added[v as usize] || k != key[v as usize] => continue,
                Some((_, Reverse(v))) => v,
                // Nothing left touches the set: every unadded super-vertex
                // has connectivity 0, and the smallest label goes next (the
                // phase's start, or a new component's first vertex).
                None => {
                    while scan < alive.len() && added[alive[scan] as usize] {
                        scan += 1;
                    }
                    let Some(&v) = alive.get(scan) else { break };
                    v
                }
            };
            let vi = v as usize;
            added[vi] = true;
            (prev, last, cut) = (last, v, key[vi]);
            count += 1;
            if count == alive.len() {
                break;
            }
            for &(u, w) in &adj[vi] {
                let ui = u as usize;
                if !added[ui] {
                    key[ui] = key[ui].saturating_add(w);
                    heap.push((key[ui], Reverse(u)));
                }
            }
        }
        let mut side = members[last as usize].clone();
        side.sort_unstable();
        if phases.is_empty() || cut < phases[best].weight {
            best = phases.len();
        }
        phases.push(PhaseCut { weight: cut, side });
        let gone = merge(&mut adj, &mut members, prev, last);
        alive.retain(|&v| v != gone);
    }

    let weight = phases[best].weight;
    let mut side = vec![false; n];
    for &v in &phases[best].side {
        side[v as usize] = true;
    }
    if super::algo_debug() {
        eprintln!(
            "[algo] cut stoer_wagner nodes={n} pairs={} phases={} min={weight} phase={best} side={}",
            g.pair_count(),
            phases.len(),
            phases[best].side.len()
        );
    }
    Some(MinCut { weight, side, phases })
}

/// Merge super-vertices `a` and `b` into one and return the label that is
/// gone: the larger, so the merged vertex keeps its smallest member as its
/// label. Every row stays sorted and names live labels only.
fn merge(adj: &mut [Vec<(u32, u64)>], members: &mut [Vec<u32>], a: u32, b: u32) -> u32 {
    let (keep, gone) = (a.min(b), a.max(b));
    let (ki, gi) = (keep as usize, gone as usize);
    let moved = std::mem::take(&mut adj[gi]);
    for &(x, w) in &moved {
        if x == keep {
            continue;
        }
        let row = &mut adj[x as usize];
        if let Ok(i) = row.binary_search_by_key(&gone, |e| e.0) {
            row.remove(i);
        }
        match row.binary_search_by_key(&keep, |e| e.0) {
            Ok(i) => row[i].1 = row[i].1.saturating_add(w),
            Err(i) => row.insert(i, (keep, w)),
        }
    }
    let kept = std::mem::take(&mut adj[ki]);
    adj[ki] = merge_rows(&kept, &moved, keep, gone);

    // Append the shorter member list to the longer.
    let mut from = std::mem::take(&mut members[gi]);
    if from.len() > members[ki].len() {
        std::mem::swap(&mut from, &mut members[ki]);
    }
    members[ki].extend_from_slice(&from);
    gone
}

/// Two sorted rows as one, summing a neighbour both hold and leaving out the
/// two labels being merged.
fn merge_rows(a: &[(u32, u64)], b: &[(u32, u64)], x: u32, y: u32) -> Vec<(u32, u64)> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    loop {
        let next = match (a.get(i), b.get(j)) {
            (Some(&p), Some(&q)) if p.0 == q.0 => {
                i += 1;
                j += 1;
                (p.0, p.1.saturating_add(q.1))
            }
            (Some(&p), Some(&q)) if p.0 < q.0 => {
                i += 1;
                p
            }
            (_, Some(&q)) => {
                j += 1;
                q
            }
            (Some(&p), None) => {
                i += 1;
                p
            }
            (None, None) => break,
        };
        if next.0 != x && next.0 != y {
            out.push(next);
        }
    }
    out
}

/// A minimum cut separating every `sources` index from every `sinks` index
/// in `g`, by Dinic's maximum flow. `None` when either set is empty, the
/// sets share an index, or an index is not below `g.len()`; a repeated index
/// counts once.
pub fn min_st_cut(g: &WeightedGraph, sources: &[u32], sinks: &[u32]) -> Option<StCut> {
    let n = g.len();
    if sources.is_empty() || sinks.is_empty() {
        return None;
    }
    let mut role = vec![Role::Inner; n];
    for &s in sources {
        *role.get_mut(s as usize)? = Role::Source;
    }
    for &t in sinks {
        let r = role.get_mut(t as usize)?;
        if *r == Role::Source {
            return None;
        }
        *r = Role::Sink;
    }
    let mut net = Network::build(g, &role);
    let (flow, rounds) = net.max_flow();
    let source_side: Vec<bool> = (0..n).map(|v| net.level[v] != UNSEEN).collect();
    let mut cut_pairs = Vec::new();
    for a in 0..n as u32 {
        for &(b, _) in g.neighbours(a) {
            if b > a && source_side[a as usize] != source_side[b as usize] {
                cut_pairs.push((a, b));
            }
        }
    }
    if super::algo_debug() {
        let count = |r: Role| role.iter().filter(|&&x| x == r).count();
        eprintln!(
            "[algo] cut dinic nodes={n} pairs={} sources={} sinks={} flow={flow} rounds={rounds} cut_pairs={} source_side={}",
            g.pair_count(),
            count(Role::Source),
            count(Role::Sink),
            cut_pairs.len(),
            source_side.iter().filter(|&&s| s).count()
        );
    }
    Some(StCut { flow, source_side, cut_pairs })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Inner,
    Source,
    Sink,
}

/// A BFS level no walk reached.
const UNSEEN: usize = usize::MAX;

/// A flow network in CSR form: node `v`'s arcs are `start[v]..start[v + 1]`,
/// arc `a` runs to `to[a]` with residual capacity `cap[a]` and reverse arc
/// `rev[a]`. Nodes `0..n` are the view's, `n` the super source, `n + 1` the
/// super sink.
struct Network {
    n: usize,
    start: Vec<usize>,
    to: Vec<usize>,
    cap: Vec<u64>,
    rev: Vec<usize>,
    level: Vec<usize>,
    /// Current-arc pointers of the blocking-flow search.
    next: Vec<usize>,
    queue: VecDeque<usize>,
}

impl Network {
    /// Each view pair becomes two arcs of capacity `w`, each the other's
    /// reverse; each source gets an arc from the super source and each sink
    /// one to the super sink, of capacity `u64::MAX` with a reverse of 0. A
    /// row lists its terminal arc first, then its neighbours ascending.
    fn build(g: &WeightedGraph, role: &[Role]) -> Self {
        let n = g.len();
        let (src, snk) = (n, n + 1);
        let total = n + 2;
        let mut start = vec![0usize; total + 1];
        for v in 0..n {
            start[v + 1] = g.neighbours(v as u32).len() + usize::from(role[v] != Role::Inner);
            match role[v] {
                Role::Source => start[src + 1] += 1,
                Role::Sink => start[snk + 1] += 1,
                Role::Inner => {}
            }
        }
        for i in 0..total {
            start[i + 1] += start[i];
        }
        let arcs = start[total];
        let mut net = Network {
            n,
            start,
            to: vec![0; arcs],
            cap: vec![0; arcs],
            rev: vec![0; arcs],
            level: vec![UNSEEN; total],
            next: vec![0; total],
            queue: VecDeque::new(),
        };
        let mut cursor = net.start.clone();
        for (v, r) in role.iter().enumerate() {
            match r {
                Role::Source => net.link(&mut cursor, src, v, u64::MAX, 0),
                Role::Sink => net.link(&mut cursor, v, snk, u64::MAX, 0),
                Role::Inner => {}
            }
        }
        for a in 0..n {
            for &(b, w) in g.neighbours(a as u32) {
                if b as usize > a {
                    net.link(&mut cursor, a, b as usize, w, w);
                }
            }
        }
        net
    }

    /// An arc `a -> b` of capacity `ab` and its reverse of capacity `ba`.
    fn link(&mut self, cursor: &mut [usize], a: usize, b: usize, ab: u64, ba: u64) {
        let (i, j) = (cursor[a], cursor[b]);
        cursor[a] += 1;
        cursor[b] += 1;
        (self.to[i], self.cap[i], self.rev[i]) = (b, ab, j);
        (self.to[j], self.cap[j], self.rev[j]) = (a, ba, i);
    }

    /// Dinic's maximum flow from the super source to the super sink, and the
    /// level graphs it built. On return `level` holds the last BFS, the one
    /// that did not reach the sink: the residual source-reachable set.
    fn max_flow(&mut self) -> (u64, u32) {
        let (src, snk) = (self.n, self.n + 1);
        let mut flow = 0u64;
        let mut rounds = 0u32;
        let mut path: Vec<usize> = Vec::new();
        while self.bfs(src, snk) {
            rounds += 1;
            self.next.copy_from_slice(&self.start[..self.n + 2]);
            path.clear();
            let mut v = src;
            loop {
                if v == snk {
                    // Every arc on the path has capacity left, so f > 0 and
                    // at least one arc saturates.
                    let f = path.iter().map(|&a| self.cap[a]).min().unwrap_or(0);
                    for &a in &path {
                        self.cap[a] -= f;
                        let r = self.rev[a];
                        self.cap[r] = self.cap[r].saturating_add(f);
                    }
                    flow = flow.saturating_add(f);
                    // Resume from the tail of the first saturated arc.
                    let keep = path.iter().position(|&a| self.cap[a] == 0).unwrap_or(0);
                    path.truncate(keep);
                    v = path.last().map_or(src, |&a| self.to[a]);
                    continue;
                }
                match self.advance(v) {
                    Some(a) => {
                        path.push(a);
                        v = self.to[a];
                    }
                    None => {
                        // A dead end: step back and skip the arc that led here.
                        let Some(a) = path.pop() else { break };
                        v = self.to[self.rev[a]];
                        self.next[v] += 1;
                    }
                }
            }
        }
        (flow, rounds)
    }

    /// The first admissible arc out of `v` from its current-arc pointer on:
    /// residual capacity left, one level deeper. Arcs passed over are never
    /// admissible again in this round.
    fn advance(&mut self, v: usize) -> Option<usize> {
        let end = self.start[v + 1];
        while self.next[v] < end {
            let a = self.next[v];
            if self.cap[a] > 0 && self.level[self.to[a]] == self.level[v] + 1 {
                return Some(a);
            }
            self.next[v] += 1;
        }
        None
    }

    /// BFS levels over arcs with capacity left; `true` when `snk` is reached.
    fn bfs(&mut self, src: usize, snk: usize) -> bool {
        self.level.fill(UNSEEN);
        self.level[src] = 0;
        self.queue.clear();
        self.queue.push_back(src);
        while let Some(v) = self.queue.pop_front() {
            for a in self.start[v]..self.start[v + 1] {
                let u = self.to[a];
                if self.cap[a] > 0 && self.level[u] == UNSEEN {
                    self.level[u] = self.level[v] + 1;
                    self.queue.push_back(u);
                }
            }
        }
        self.level[snk] != UNSEEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
    }

    /// Graph `seed` of the corpus: 2..=10 nodes, a density drawn per graph
    /// (so some come out sparse and disconnected), weights 1..=9, a few
    /// self-loops and repeated pairs.
    fn lcg_view(seed: u64) -> WeightedGraph {
        let mut r = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03);
        let n = 2 + (r.next() % 9) as u32;
        let density = 1 + r.next() % 10;
        let mut pairs = Vec::new();
        for a in 0..n {
            if r.next().is_multiple_of(5) {
                pairs.push((a, a, 1 + r.next() % 50));
            }
            for b in a + 1..n {
                if r.next() % 10 < density {
                    let (x, y) = if r.next().is_multiple_of(2) { (a, b) } else { (b, a) };
                    pairs.push((x, y, 1 + r.next() % 9));
                    if r.next().is_multiple_of(8) {
                        pairs.push((a, b, 1 + r.next() % 9));
                    }
                }
            }
        }
        WeightedGraph::from_pairs(n as usize, &pairs)
    }

    fn weight_of(g: &WeightedGraph, a: u32, b: u32) -> u64 {
        let row = g.neighbours(a);
        row.binary_search_by_key(&b, |e| e.0).map_or(0, |i| row[i].1)
    }

    /// The weight of every pair with exactly one end inside.
    fn crossing(g: &WeightedGraph, inside: impl Fn(u32) -> bool) -> u64 {
        let mut s = 0;
        for a in 0..g.len() as u32 {
            for &(b, w) in g.neighbours(a) {
                if b > a && inside(a) != inside(b) {
                    s += w;
                }
            }
        }
        s
    }

    /// `crossing` of every subset, indexed by its bitmask.
    fn crossings(g: &WeightedGraph) -> Vec<u64> {
        (0u32..1 << g.len()).map(|mask| crossing(g, |v| mask >> v & 1 == 1)).collect()
    }

    fn brute_global(g: &WeightedGraph) -> u64 {
        let n = g.len();
        let cross = crossings(g);
        // Node n - 1 stays outside: each of the 2^(n-1) - 1 bipartitions once.
        (1..1usize << (n - 1)).map(|mask| cross[mask]).min().unwrap_or(0)
    }

    fn brute_st(cross: &[u64], sources: &[u32], sinks: &[u32]) -> u64 {
        let must: usize = sources.iter().map(|&s| 1 << s).sum();
        let never: usize = sinks.iter().map(|&t| 1 << t).sum();
        (0..cross.len()).filter(|m| m & must == must && m & never == 0).map(|m| cross[m]).min().unwrap_or(0)
    }

    fn members(side: &[bool]) -> Vec<u32> {
        side.iter().enumerate().filter(|&(_, &s)| s).map(|(v, _)| v as u32).collect()
    }

    #[test]
    fn sw_matches_brute_force() {
        let (mut zero, mut positive) = (0, 0);
        for seed in 0..300 {
            let g = lcg_view(seed);
            let cut = stoer_wagner(&g).expect("two or more nodes");
            assert_eq!(cut.weight, brute_global(&g), "seed {seed}");
            let inside = members(&cut.side);
            assert!(!inside.is_empty() && inside.len() < g.len(), "seed {seed}: a proper side");
            assert_eq!(crossing(&g, |v| cut.side[v as usize]), cut.weight, "seed {seed}");
            if cut.weight == 0 {
                zero += 1;
            } else {
                positive += 1;
            }
        }
        // The corpus exercises both disconnected and connected views.
        assert!(zero >= 20 && positive >= 100, "zero={zero} positive={positive}");
    }

    #[test]
    fn sw_phases_cover_the_min() {
        for seed in 0..300 {
            let g = lcg_view(seed);
            let cut = stoer_wagner(&g).expect("two or more nodes");
            assert_eq!(cut.phases.len(), g.len() - 1, "seed {seed}");
            for (i, p) in cut.phases.iter().enumerate() {
                assert!(p.side.windows(2).all(|w| w[0] < w[1]), "seed {seed} phase {i}: sorted, distinct");
                assert!(!p.side.is_empty() && p.side.len() < g.len(), "seed {seed} phase {i}");
                assert_eq!(crossing(&g, |v| p.side.binary_search(&v).is_ok()), p.weight, "seed {seed} phase {i}");
            }
            let first_min = cut.phases.iter().position(|p| p.weight == cut.weight).expect("a phase has the min");
            assert!(cut.phases.iter().all(|p| p.weight >= cut.weight), "seed {seed}");
            assert_eq!(members(&cut.side), cut.phases[first_min].side, "seed {seed}: the earliest min phase wins");
        }
    }

    #[test]
    fn dinic_matches_brute_force() {
        let mut checked = 0;
        for seed in 0..300 {
            let g = lcg_view(seed);
            let cross = crossings(&g);
            let n = g.len() as u32;
            for s in 0..n {
                for t in 0..n {
                    if s == t {
                        continue;
                    }
                    let st = min_st_cut(&g, &[s], &[t]).expect("disjoint terminals");
                    assert_eq!(st.flow, brute_st(&cross, &[s], &[t]), "seed {seed} s={s} t={t}");
                    assert!(st.source_side[s as usize] && !st.source_side[t as usize], "seed {seed} s={s} t={t}");
                    assert_eq!(crossing(&g, |v| st.source_side[v as usize]), st.flow, "seed {seed} s={s} t={t}");
                    let expect: Vec<(u32, u32)> = (0..n)
                        .flat_map(|a| g.neighbours(a).iter().map(move |&(b, _)| (a, b)))
                        .filter(|&(a, b)| b > a && st.source_side[a as usize] != st.source_side[b as usize])
                        .collect();
                    assert_eq!(st.cut_pairs, expect, "seed {seed} s={s} t={t}");
                    let sum: u64 = st.cut_pairs.iter().map(|&(a, b)| weight_of(&g, a, b)).sum();
                    assert_eq!(sum, st.flow, "seed {seed} s={s} t={t}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 5_000, "checked {checked} pairs");
    }

    /// Edmonds-Karp over a dense capacity matrix: the independent oracle for
    /// views too large to enumerate. Sources and sinks join a super source
    /// and sink by capacities larger than every pair weight summed.
    fn edmonds_karp(g: &WeightedGraph, sources: &[u32], sinks: &[u32]) -> u64 {
        let n = g.len();
        let (src, snk) = (n, n + 1);
        let big = 1 + (0..n as u32).flat_map(|a| g.neighbours(a).iter().map(|&(_, w)| w)).sum::<u64>();
        let mut cap = vec![vec![0u64; n + 2]; n + 2];
        for a in 0..n as u32 {
            for &(b, w) in g.neighbours(a) {
                cap[a as usize][b as usize] = w;
            }
        }
        for &s in sources {
            cap[src][s as usize] = big;
        }
        for &t in sinks {
            cap[t as usize][snk] = big;
        }
        let mut flow = 0;
        loop {
            let mut parent = vec![usize::MAX; n + 2];
            parent[src] = src;
            let mut queue = VecDeque::from([src]);
            while let Some(v) = queue.pop_front() {
                for u in 0..n + 2 {
                    if parent[u] == usize::MAX && cap[v][u] > 0 {
                        parent[u] = v;
                        queue.push_back(u);
                    }
                }
            }
            if parent[snk] == usize::MAX {
                return flow;
            }
            let (mut f, mut v) = (u64::MAX, snk);
            while v != src {
                f = f.min(cap[parent[v]][v]);
                v = parent[v];
            }
            v = snk;
            while v != src {
                let p = parent[v];
                cap[p][v] -= f;
                cap[v][p] += f;
                v = p;
            }
            flow += f;
        }
    }

    #[test]
    fn dinic_matches_edmonds_karp() {
        for seed in 0..120u64 {
            let mut r = Lcg(seed ^ 0x5EED);
            let n = 20 + (r.next() % 60) as u32;
            let mut pairs = Vec::new();
            for _ in 0..n * (2 + (r.next() % 3) as u32) {
                pairs.push(((r.next() % n as u64) as u32, (r.next() % n as u64) as u32, 1 + r.next() % 12));
            }
            let g = WeightedGraph::from_pairs(n as usize, &pairs);
            let k = 1 + (r.next() % 3) as u32;
            let sources: Vec<u32> = (0..k).collect();
            let sinks: Vec<u32> = (n - k..n).collect();
            let st = min_st_cut(&g, &sources, &sinks).expect("disjoint terminals");
            assert_eq!(st.flow, edmonds_karp(&g, &sources, &sinks), "seed {seed}");
            assert_eq!(crossing(&g, |v| st.source_side[v as usize]), st.flow, "seed {seed}");
            assert!(sources.iter().all(|&s| st.source_side[s as usize]), "seed {seed}");
            assert!(sinks.iter().all(|&t| !st.source_side[t as usize]), "seed {seed}");
        }
    }

    #[test]
    fn reverse_flow_beyond_the_pair_weight() {
        // Round 1's shortest path 0-1-2-3 sends 1 over pair (1, 2) as 1 -> 2.
        // The maximum flow needs a net 1 over it the other way, so round 2
        // must push 2 along 2 -> 1: the residual reverse arc has to grow by
        // the flow it carries, not stay at the pair weight.
        let g = WeightedGraph::from_pairs(
            8,
            &[(0, 1, 1), (1, 2, 1), (2, 3, 1), (0, 4, 2), (4, 5, 2), (5, 2, 2), (1, 6, 2), (6, 7, 2), (7, 3, 2)],
        );
        let st = min_st_cut(&g, &[0], &[3]).expect("disjoint terminals");
        assert_eq!(st.flow, 3);
        assert_eq!(st.flow, brute_st(&crossings(&g), &[0], &[3]));
        assert_eq!(crossing(&g, |v| st.source_side[v as usize]), 3);
    }

    /// Top rail 0-1-2-3 and bottom rail 4-5-6-7 of weight 10, joined by four
    /// rungs `(i, i + 4)` of weight 1.
    fn ladder() -> WeightedGraph {
        let mut pairs = Vec::new();
        for i in 0..3 {
            pairs.push((i, i + 1, 10));
            pairs.push((i + 4, i + 5, 10));
        }
        for i in 0..4 {
            pairs.push((i, i + 4, 1));
        }
        WeightedGraph::from_pairs(8, &pairs)
    }

    #[test]
    fn multi_terminal() {
        let g = ladder();
        // Sources on the top rail, sinks on the bottom: every rung is cut.
        let st = min_st_cut(&g, &[0, 1], &[6, 7]).expect("disjoint terminals");
        assert_eq!(st.flow, 4, "the rung count");
        assert_eq!(members(&st.source_side), vec![0, 1, 2, 3]);
        assert_eq!(st.cut_pairs, vec![(0, 4), (1, 5), (2, 6), (3, 7)]);
        assert_eq!(st.flow, brute_st(&crossings(&g), &[0, 1], &[6, 7]));
        // Repeats and order in the terminal lists do not matter.
        assert_eq!(min_st_cut(&g, &[1, 0, 1], &[7, 6]), Some(st));
        // Sources at one end of both rails: the two rails are cut.
        let ends = min_st_cut(&g, &[0, 4], &[3, 7]).expect("disjoint terminals");
        assert_eq!(ends.flow, brute_st(&crossings(&g), &[0, 4], &[3, 7]));
        assert_eq!(ends.flow, 20);
    }

    #[test]
    fn bad_terminals_and_tiny_views_are_none() {
        let g = ladder();
        assert_eq!(min_st_cut(&g, &[], &[1]), None);
        assert_eq!(min_st_cut(&g, &[0], &[]), None);
        assert_eq!(min_st_cut(&g, &[0, 3], &[3]), None, "overlapping sets");
        assert_eq!(min_st_cut(&g, &[0], &[8]), None, "out of range");
        assert_eq!(min_st_cut(&g, &[9], &[1]), None, "out of range");
        assert_eq!(stoer_wagner(&WeightedGraph::from_pairs(0, &[])), None);
        assert_eq!(stoer_wagner(&WeightedGraph::from_pairs(1, &[(0, 0, 3)])), None);
        let two = stoer_wagner(&WeightedGraph::from_pairs(2, &[])).expect("two nodes");
        assert_eq!((two.weight, two.phases.len()), (0, 1));
    }

    #[test]
    fn two_cliques_split_at_the_bridge() {
        let mut pairs = Vec::new();
        for c in 0..2u32 {
            for i in 0..5 {
                for j in i + 1..5 {
                    pairs.push((c * 5 + i, c * 5 + j, 3));
                }
            }
        }
        pairs.push((4, 5, 2));
        let g = WeightedGraph::from_pairs(10, &pairs);
        let cut = stoer_wagner(&g).expect("ten nodes");
        assert_eq!(cut.weight, 2);
        let side = members(&cut.side);
        assert!(side == vec![0, 1, 2, 3, 4] || side == vec![5, 6, 7, 8, 9], "{side:?}");
        let st = min_st_cut(&g, &[0], &[9]).expect("disjoint terminals");
        assert_eq!((st.flow, st.cut_pairs.clone()), (2, vec![(4, 5)]));
        assert_eq!(members(&st.source_side), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn disconnected_is_zero() {
        // A triangle {0, 1, 2} and a path 3-4-5, a self-loop on 4.
        let g = WeightedGraph::from_pairs(6, &[(0, 1, 3), (1, 2, 3), (0, 2, 3), (3, 4, 2), (4, 5, 2), (4, 4, 9)]);
        let cut = stoer_wagner(&g).expect("six nodes");
        assert_eq!(cut.weight, 0);
        let side = members(&cut.side);
        assert!(side == vec![0, 1, 2] || side == vec![3, 4, 5], "one component: {side:?}");
        let first_zero = cut.phases.iter().position(|p| p.weight == 0).expect("a zero phase");
        assert_eq!(cut.phases[first_zero].side, side);
        assert!(cut.phases[..first_zero].iter().all(|p| p.weight > 0));

        let st = min_st_cut(&g, &[0], &[4]).expect("disjoint terminals");
        assert_eq!(st.flow, 0);
        assert!(st.cut_pairs.is_empty());
        assert_eq!(members(&st.source_side), vec![0, 1, 2]);
    }

    #[test]
    fn path_100k_no_stack_overflow() {
        let n = 100_000u32;
        let pairs: Vec<(u32, u32, u64)> = (0..n - 1).map(|i| (i, i + 1, 1)).collect();
        let g = WeightedGraph::from_pairs(n as usize, &pairs);
        let st = min_st_cut(&g, &[0], &[n - 1]).expect("disjoint terminals");
        assert_eq!(st.flow, 1);
        assert_eq!(st.cut_pairs, vec![(0, 1)]);
        assert_eq!(members(&st.source_side), vec![0]);
        let mid = min_st_cut(&g, &[n / 2], &[0, n - 1]).expect("disjoint terminals");
        assert_eq!(mid.flow, 2);
        assert_eq!(mid.cut_pairs, vec![(n / 2 - 1, n / 2), (n / 2, n / 2 + 1)]);
    }

    #[test]
    fn deterministic() {
        let mut pairs = Vec::new();
        let mut r = Lcg(7);
        for _ in 0..1_500 {
            let (a, b) = ((r.next() % 300) as u32, (r.next() % 300) as u32);
            pairs.push((a, b, 1 + r.next() % 20));
        }
        let big = WeightedGraph::from_pairs(300, &pairs);
        let mut views: Vec<WeightedGraph> = (0..40).map(lcg_view).collect();
        views.push(big);
        for g in &views {
            let a = (stoer_wagner(g), min_st_cut(g, &[0], &[1]));
            let b = (stoer_wagner(g), min_st_cut(g, &[0], &[1]));
            assert_eq!(a, b);
        }
        // Reversing the pair list builds the same view, so the same cuts.
        pairs.reverse();
        let back = WeightedGraph::from_pairs(300, &pairs);
        assert_eq!(stoer_wagner(&back), stoer_wagner(&views[40]));
        assert_eq!(min_st_cut(&back, &[5, 9], &[200]), min_st_cut(&views[40], &[5, 9], &[200]));
    }
}
