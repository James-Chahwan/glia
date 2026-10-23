//! The PPR index (EA.1): the incidence lists, out-weights and dangling list
//! `crate::ppr_vector` used to rebuild per call, built once per (graph,
//! config) and run per seed set.
//!
//! [`PprIndex::vector`] is the pre-index `ppr_vector` arithmetic verbatim, in
//! the same operation order: each target's incidences in edge input order,
//! the dangling mass summed in index order, the L1 diff in index order. Do
//! not "simplify" any of it: `plan`'s oracle test and this module's
//! `ppr_vector_head` oracle compare f64 bits.

use std::collections::HashMap;

use glia_core::{Edge, EdgeCategoryId, NodeId};

use crate::{ActivationConfig, Direction};

/// One graph's PPR incidences under one config's direction, weights,
/// damping, iteration cap and epsilon. `top_k` and `node_specificity` act
/// after PPR and are no part of it.
#[derive(Debug, Clone)]
pub struct PprIndex {
    node_ids: Vec<NodeId>,
    /// Lookup only, never iterated: a repeated id maps to its last index.
    id_to_idx: HashMap<NodeId, usize>,
    /// CSR by target: `inc[start[i]..start[i + 1]]` sends to node i.
    start: Vec<usize>,
    /// `(source index, weight)`.
    inc: Vec<(usize, f64)>,
    out_w: Vec<f64>,
    /// Nodes with no outgoing weight, in index order.
    dangling: Vec<usize>,
    damping: f64,
    direction: Direction,
    /// The config's edge weights as `(category, f64 bits)`, sorted by
    /// category: the fingerprint [`Self::matches`] compares.
    weights: Vec<(EdgeCategoryId, u64)>,
    max_iterations: usize,
    epsilon: f64,
}

/// The sorted `(category, bits)` fingerprint of a config's edge weights.
fn weight_fingerprint(config: &ActivationConfig) -> Vec<(EdgeCategoryId, u64)> {
    let mut w: Vec<(EdgeCategoryId, u64)> = config.edge_weights.iter().map(|(&c, &w)| (c, w.to_bits())).collect();
    w.sort_unstable_by_key(|&(c, _)| c.0);
    w
}

impl PprIndex {
    /// Index `node_ids` and `edges` (walked in iterator order) under
    /// `config`. An edge whose category weighs 0 or less, or with an endpoint
    /// that is no node, is skipped.
    pub fn build<'e>(node_ids: &[NodeId], edges: impl Iterator<Item = &'e Edge>, config: &ActivationConfig) -> Self {
        let n = node_ids.len();
        let id_to_idx: HashMap<NodeId, usize> = node_ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();

        // (target, source, weight) in ppr_vector's push order.
        let mut events: Vec<(usize, usize, f64)> = Vec::new();
        let mut out_w: Vec<f64> = vec![0.0; n];
        for edge in edges {
            let w = config.edge_weights.get(&edge.category).copied().unwrap_or(1.0);
            if w <= 0.0 {
                continue;
            }
            let (Some(&fi), Some(&ti)) = (id_to_idx.get(&edge.from), id_to_idx.get(&edge.to)) else {
                continue;
            };
            match config.direction {
                Direction::Forward => {
                    events.push((ti, fi, w));
                    out_w[fi] += w;
                }
                Direction::Backward => {
                    events.push((fi, ti, w));
                    out_w[ti] += w;
                }
                Direction::Undirected => {
                    events.push((ti, fi, w));
                    events.push((fi, ti, w));
                    out_w[fi] += w;
                    out_w[ti] += w;
                }
            }
        }

        let mut start = vec![0usize; n + 1];
        for e in &events {
            start[e.0 + 1] += 1;
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        let mut cursor = start.clone();
        let mut inc = vec![(0usize, 0.0f64); events.len()];
        for &(t, s, w) in &events {
            inc[cursor[t]] = (s, w);
            cursor[t] += 1;
        }
        let dangling = (0..n).filter(|&j| out_w[j] == 0.0).collect();

        Self {
            node_ids: node_ids.to_vec(),
            id_to_idx,
            start,
            inc,
            out_w,
            dangling,
            damping: config.damping,
            direction: config.direction,
            weights: weight_fingerprint(config),
            max_iterations: config.max_iterations,
            epsilon: config.epsilon,
        }
    }

    /// The PPR score vector for `seeds`, index-aligned with
    /// [`Self::node_ids`], and the rounds that produced it: `ppr_vector`'s
    /// answer. Empty, after 0 rounds, when there are no nodes, no seeds, or
    /// no seed is a node. Each seed that is a node weighs `1 / count`, a seed
    /// listed twice counted twice.
    pub fn vector(&self, seeds: &[NodeId]) -> (Vec<f64>, usize) {
        let n = self.node_ids.len();
        if n == 0 || seeds.is_empty() {
            return (Vec::new(), 0);
        }
        let mut personalization = vec![0.0; n];
        let seed_count = seeds.iter().filter(|s| self.id_to_idx.contains_key(s)).count();
        if seed_count == 0 {
            return (Vec::new(), 0);
        }
        let seed_weight = 1.0 / seed_count as f64;
        for seed in seeds {
            if let Some(&idx) = self.id_to_idx.get(seed) {
                personalization[idx] = seed_weight;
            }
        }
        self.iterate(personalization)
    }

    /// The PPR score vector for a weighted personalisation, as rank_eval's
    /// `Ppr::power` fills it: `personalization[node] = w` in the given order
    /// (a repeated node keeps its last weight), no normalisation, a node
    /// outside the index skipped. All zeros after 0 rounds when nothing is
    /// left after the skip (not the empty vector [`Self::vector`] returns:
    /// callers pool by index).
    pub fn vector_weighted(&self, pers: &[(NodeId, f64)]) -> (Vec<f64>, usize) {
        let n = self.node_ids.len();
        let mut personalization = vec![0.0; n];
        let mut any = false;
        for (id, w) in pers {
            if let Some(&idx) = self.id_to_idx.get(id) {
                personalization[idx] = *w;
                any = true;
            }
        }
        if n == 0 || !any {
            return (personalization, 0);
        }
        self.iterate(personalization)
    }

    /// Power iteration `v = (1-d) * p + d * M * v` from `personalization`.
    fn iterate(&self, personalization: Vec<f64>) -> (Vec<f64>, usize) {
        let n = self.node_ids.len();
        let d = self.damping.clamp(0.01, 0.99);
        let mut scores = personalization.clone();
        let mut new_scores = vec![0.0; n];
        let mut iterations = 0;

        for _ in 0..self.max_iterations {
            // Dangling nodes (no outgoing edges) give their mass back to the
            // personalization vector, keeping scores summing to 1.0.
            let dangling_sum: f64 = self.dangling.iter().map(|&j| scores[j]).sum();

            for i in 0..n {
                let propagated: f64 = self.inc[self.start[i]..self.start[i + 1]]
                    .iter()
                    .map(|&(j, w)| if self.out_w[j] > 0.0 { scores[j] * w / self.out_w[j] } else { 0.0 })
                    .sum();
                new_scores[i] =
                    (1.0 - d) * personalization[i] + d * (propagated + dangling_sum * personalization[i]);
            }

            let diff: f64 = scores.iter().zip(new_scores.iter()).map(|(a, b)| (a - b).abs()).sum();
            std::mem::swap(&mut scores, &mut new_scores);
            iterations += 1;

            if diff < self.epsilon {
                break;
            }
        }

        (scores, iterations)
    }

    /// The indexed nodes, in the order the vectors are aligned with.
    pub fn node_ids(&self) -> &[NodeId] {
        &self.node_ids
    }

    /// The slot `id` scores in; a repeated id answers its last index, the
    /// one PPR scores (an earlier occurrence scores 0).
    pub fn index_of(&self, id: NodeId) -> Option<usize> {
        self.id_to_idx.get(&id).copied()
    }

    /// The number of indexed node slots (repeats included).
    pub fn len(&self) -> usize {
        self.node_ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.node_ids.is_empty()
    }

    /// The incidences the index holds (an undirected edge counts twice).
    pub fn incidences(&self) -> usize {
        self.inc.len()
    }

    /// Whether this index computes `config`'s PPR: damping, direction,
    /// iteration cap, epsilon and every edge weight equal, bit for bit.
    pub fn matches(&self, config: &ActivationConfig) -> bool {
        self.damping.to_bits() == config.damping.to_bits()
            && self.direction == config.direction
            && self.max_iterations == config.max_iterations
            && self.epsilon.to_bits() == config.epsilon.to_bits()
            && self.weights == weight_fingerprint(config)
    }
}

/// `ppr_vector` at 07a63dee (before EA.1), verbatim: the oracle the index is
/// compared to in f64 bits.
#[cfg(test)]
pub(crate) fn ppr_vector_head<'e>(
    node_ids: &[NodeId],
    edges: impl Iterator<Item = &'e Edge>,
    seeds: &[NodeId],
    config: &ActivationConfig,
) -> (Vec<f64>, usize) {
    let n = node_ids.len();
    if n == 0 || seeds.is_empty() {
        return (Vec::new(), 0);
    }

    let id_to_idx: HashMap<NodeId, usize> = node_ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();

    let mut incoming: Vec<Vec<(usize, f64)>> = vec![vec![]; n];
    let mut out_weight: Vec<f64> = vec![0.0; n];

    for edge in edges {
        let w = config.edge_weights.get(&edge.category).copied().unwrap_or(1.0);
        if w <= 0.0 {
            continue;
        }

        let from_idx = id_to_idx.get(&edge.from).copied();
        let to_idx = id_to_idx.get(&edge.to).copied();

        match (from_idx, to_idx) {
            (Some(fi), Some(ti)) => match config.direction {
                Direction::Forward => {
                    incoming[ti].push((fi, w));
                    out_weight[fi] += w;
                }
                Direction::Backward => {
                    incoming[fi].push((ti, w));
                    out_weight[ti] += w;
                }
                Direction::Undirected => {
                    incoming[ti].push((fi, w));
                    incoming[fi].push((ti, w));
                    out_weight[fi] += w;
                    out_weight[ti] += w;
                }
            },
            _ => continue,
        }
    }

    let mut personalization = vec![0.0; n];
    let seed_count = seeds.iter().filter(|s| id_to_idx.contains_key(s)).count();
    if seed_count == 0 {
        return (Vec::new(), 0);
    }
    let seed_weight = 1.0 / seed_count as f64;
    for seed in seeds {
        if let Some(&idx) = id_to_idx.get(seed) {
            personalization[idx] = seed_weight;
        }
    }

    let d = config.damping.clamp(0.01, 0.99);
    let mut scores = personalization.clone();
    let mut iterations = 0;

    for _ in 0..config.max_iterations {
        let mut new_scores = vec![0.0; n];

        let dangling_sum: f64 = (0..n).filter(|&j| out_weight[j] == 0.0).map(|j| scores[j]).sum();

        for i in 0..n {
            let propagated: f64 = incoming[i]
                .iter()
                .map(|&(j, w)| if out_weight[j] > 0.0 { scores[j] * w / out_weight[j] } else { 0.0 })
                .sum();

            new_scores[i] = (1.0 - d) * personalization[i] + d * (propagated + dangling_sum * personalization[i]);
        }

        let diff: f64 = scores.iter().zip(new_scores.iter()).map(|(a, b)| (a - b).abs()).sum();

        scores = new_scores;
        iterations += 1;

        if diff < config.epsilon {
            break;
        }
    }

    (scores, iterations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_core::Confidence;

    const CALLS: EdgeCategoryId = EdgeCategoryId(4);
    const CONTAINS: EdgeCategoryId = EdgeCategoryId(2);
    const ZERO: EdgeCategoryId = EdgeCategoryId(9);

    fn edge(from: u64, to: u64, category: EdgeCategoryId) -> Edge {
        Edge { from: NodeId(from), to: NodeId(to), category, confidence: Confidence::Strong, cells: Vec::new() }
    }

    fn ids(ns: &[u64]) -> Vec<NodeId> {
        ns.iter().map(|&n| NodeId(n)).collect()
    }

    fn bits(v: &[f64]) -> Vec<u64> {
        v.iter().map(|s| s.to_bits()).collect()
    }

    /// Node 6 is dangling; node 3 repeats (slots 2 and 6); category ZERO
    /// weighs 0; 99 is no node; 4 -> 4 is a self-loop.
    fn graph() -> (Vec<NodeId>, Vec<Edge>) {
        let nodes = ids(&[1, 2, 3, 4, 5, 6, 3, 7]);
        let edges = vec![
            edge(1, 2, CALLS),
            edge(1, 3, CONTAINS),
            edge(2, 3, CALLS),
            edge(3, 4, CALLS),
            edge(4, 4, CALLS),
            edge(4, 5, ZERO),
            edge(5, 6, CALLS),
            edge(2, 99, CALLS),
            edge(99, 7, CALLS),
            edge(7, 1, CONTAINS),
            edge(3, 1, CALLS),
            edge(2, 5, CONTAINS),
        ];
        (nodes, edges)
    }

    fn config(direction: Direction, damping: f64) -> ActivationConfig {
        let mut edge_weights = HashMap::new();
        edge_weights.insert(CALLS, 1.0);
        edge_weights.insert(CONTAINS, 0.3);
        edge_weights.insert(ZERO, 0.0);
        ActivationConfig { damping, direction, edge_weights, epsilon: 1e-12, ..Default::default() }
    }

    const DIRECTIONS: [Direction; 3] = [Direction::Forward, Direction::Backward, Direction::Undirected];

    /// (a) The index's vector equals HEAD's ppr_vector in bits and rounds.
    #[test]
    fn vector_equals_head_ppr_vector() {
        let (nodes, edges) = graph();
        let seed_sets = [ids(&[1]), ids(&[3]), ids(&[2, 2, 5]), ids(&[6, 99]), ids(&[99]), vec![]];
        let mut nonempty = 0usize;
        for direction in DIRECTIONS {
            for damping in [0.5, 0.85] {
                let c = config(direction, damping);
                let ix = PprIndex::build(&nodes, edges.iter(), &c);
                for seeds in &seed_sets {
                    let (old, oi) = ppr_vector_head(&nodes, edges.iter(), seeds, &c);
                    let (new, ni) = ix.vector(seeds);
                    assert_eq!((bits(&old), oi), (bits(&new), ni), "{direction:?} d={damping} seeds={seeds:?}");
                    nonempty += usize::from(!new.is_empty());
                }
            }
        }
        assert_eq!(nonempty, 3 * 2 * 4);
        // The repeated id's earlier slot scores 0, its last slot scores.
        let ix = PprIndex::build(&nodes, edges.iter(), &config(Direction::Forward, 0.5));
        let (v, _) = ix.vector(&ids(&[1]));
        assert_eq!(v[2], 0.0);
        assert!(v[6] > 0.0);
        assert_eq!(ix.index_of(NodeId(3)), Some(6));
        assert_eq!(ix.index_of(NodeId(99)), None);
        // 12 edges minus the zero-weight one and the two touching 99.
        assert_eq!(ix.incidences(), 9);
        assert_eq!(ix.len(), 8);
    }

    /// (b) One index, several seed sets in turn: no state leaks.
    #[test]
    fn reuse_leaks_no_state() {
        let (nodes, edges) = graph();
        for direction in DIRECTIONS {
            let c = config(direction, 0.85);
            let ix = PprIndex::build(&nodes, edges.iter(), &c);
            for seeds in [ids(&[1]), ids(&[5, 7]), ids(&[4])] {
                let fresh = PprIndex::build(&nodes, edges.iter(), &c).vector(&seeds);
                let reused = ix.vector(&seeds);
                assert_eq!((bits(&fresh.0), fresh.1), (bits(&reused.0), reused.1), "{direction:?} {seeds:?}");
            }
        }
    }

    /// (d) matches() reads damping, direction and every weight.
    #[test]
    fn matches_reads_the_config() {
        let (nodes, edges) = graph();
        let c = config(Direction::Forward, 0.5);
        let ix = PprIndex::build(&nodes, edges.iter(), &c);
        assert!(ix.matches(&c));
        assert!(ix.matches(&ActivationConfig { top_k: 3, node_specificity: crate::Specificity::Idf, ..c.clone() }));
        assert!(!ix.matches(&ActivationConfig { damping: 0.85, ..c.clone() }));
        assert!(!ix.matches(&ActivationConfig { direction: Direction::Backward, ..c.clone() }));
        assert!(!ix.matches(&ActivationConfig { max_iterations: 7, ..c.clone() }));
        assert!(!ix.matches(&ActivationConfig { epsilon: 1e-6, ..c.clone() }));
        let mut w = c.clone();
        w.edge_weights.insert(CONTAINS, 0.4);
        assert!(!ix.matches(&w));
        let mut extra = c.clone();
        extra.edge_weights.insert(EdgeCategoryId(11), 1.0);
        assert!(!ix.matches(&extra));
    }

    /// rank_eval's `Ppr::power` at 07a63dee over a prebuilt index, verbatim
    /// in its arithmetic (iteration count added).
    fn power_head(ix: &PprIndex, pers: &[(usize, f64)]) -> Vec<f64> {
        let n = ix.node_ids.len();
        if n == 0 || pers.is_empty() {
            return vec![0.0; n];
        }
        let mut personalization = vec![0.0; n];
        for &(i, w) in pers {
            personalization[i] = w;
        }
        let d = ix.damping.clamp(0.01, 0.99);
        let mut scores = personalization.clone();
        let mut new_scores = vec![0.0; n];
        for _ in 0..ix.max_iterations {
            let dangling_sum: f64 = ix.dangling.iter().map(|&j| scores[j]).sum();
            for i in 0..n {
                let propagated: f64 = ix.inc[ix.start[i]..ix.start[i + 1]]
                    .iter()
                    .map(|&(j, w)| if ix.out_w[j] > 0.0 { scores[j] * w / ix.out_w[j] } else { 0.0 })
                    .sum();
                new_scores[i] = (1.0 - d) * personalization[i] + d * (propagated + dangling_sum * personalization[i]);
            }
            let diff: f64 = scores.iter().zip(new_scores.iter()).map(|(a, b)| (a - b).abs()).sum();
            std::mem::swap(&mut scores, &mut new_scores);
            if diff < ix.epsilon {
                break;
            }
        }
        scores
    }

    /// (e) vector_weighted: uniform 1/k equals vector; a weighted case equals
    /// rank_eval's Ppr::power; nothing in the index gives zeros.
    #[test]
    fn vector_weighted_matches_vector_and_power() {
        let (nodes, edges) = graph();
        for direction in DIRECTIONS {
            for damping in [0.5, 0.85] {
                let c = config(direction, damping);
                let ix = PprIndex::build(&nodes, edges.iter(), &c);
                let seeds = ids(&[1, 4, 7]);
                let k = seeds.len() as f64;
                let uniform: Vec<(NodeId, f64)> = seeds.iter().map(|&s| (s, 1.0 / k)).collect();
                let (a, ai) = ix.vector(&seeds);
                let (b, bi) = ix.vector_weighted(&uniform);
                assert_eq!((bits(&a), ai), (bits(&b), bi), "{direction:?} d={damping}");

                // Weighted, a repeated node (last weight wins), a non-node.
                let pers = [(NodeId(1), 0.2), (NodeId(5), 0.5), (NodeId(1), 0.3), (NodeId(99), 9.0)];
                let (w, _) = ix.vector_weighted(&pers);
                let by_index = [(0usize, 0.2), (4, 0.5), (0, 0.3)];
                assert_eq!(bits(&w), bits(&power_head(&ix, &by_index)), "{direction:?} d={damping}");
            }
        }
        let ix = PprIndex::build(&nodes, edges.iter(), &config(Direction::Forward, 0.5));
        assert_eq!(ix.vector_weighted(&[(NodeId(99), 1.0)]), (vec![0.0; 8], 0));
        assert_eq!(ix.vector_weighted(&[]), (vec![0.0; 8], 0));
        let empty = PprIndex::build(&[], std::iter::empty(), &ActivationConfig::default());
        assert!(empty.is_empty());
        assert_eq!(empty.vector_weighted(&[(NodeId(1), 1.0)]), (Vec::new(), 0));
        assert_eq!(empty.vector(&ids(&[1])), (Vec::new(), 0));
    }
}
