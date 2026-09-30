//! Link prediction (CD.3a): undirected neighbourhoods read from an
//! [`Adjacency`], and common-neighbours, Adamic-Adar and resource-allocation
//! scores for given node pairs, as integer scores. Domain-free: the caller
//! chooses the edge categories when it builds the index (a
//! [`CategorySet`](crate::algo::CategorySet), or a domain's carry edges
//! through [`Adjacency::carry`]), and nothing here names a node kind.
//!
//! [`Neighbourhoods`] folds the index's directed incidence lists into one
//! undirected neighbour list per node: both directions merged, parallel edges
//! counted once, the node itself left out, sorted by dense index.
//! [`score_pairs`] scores only the pairs it is handed; choosing them is the
//! caller's job (the engine's suspected-edge report, CD.3b, draws candidates
//! from its own channel-token index), so no all-pairs or two-hop sweep lives
//! here.
//!
//! For a pair `(x, y)` with common neighbours `Z = N(x) ∩ N(y)`:
//! - `common` = `|Z|`
//! - `adamic_adar_milli` = sum over `z` in `Z` of `round(1000 / ln deg z)`
//! - `resource_allocation_micro` = sum over `z` in `Z` of `floor(1_000_000 / deg z)`
//!
//! A common neighbour neighbours both `x` and `y` (`x != y`), so `deg z >= 2`
//! and `ln deg z > 0`. Each Adamic-Adar term is rounded to an integer before
//! the sum, taken in ascending dense-index order; resource allocation is exact
//! integer arithmetic. The f64 `ln` may differ in its last ulp across
//! platforms, which moves a rounded term by at most 1; on one platform every
//! score is deterministic. Building costs O(V + E log E), a pair
//! O(deg x + deg y), and no map is ever iterated.

use std::cmp::Ordering;
use std::collections::HashMap;

use glia_core::NodeId;

use super::Adjacency;

/// Every indexed node's undirected neighbours, as one CSR list over the
/// [`Adjacency`]'s dense index (nodes, then dangling edge endpoints).
#[derive(Clone, Debug, Default)]
pub struct Neighbourhoods {
    /// The index's own id -> dense-index map, copied so the neighbourhoods
    /// outlive the index. Only looked up, never iterated.
    index: HashMap<NodeId, u32>,
    /// `nbr[start[i]..start[i + 1]]` are dense index `i`'s neighbours.
    start: Vec<usize>,
    /// Deduplicated, sorted by dense index, never the row's own index.
    nbr: Vec<u32>,
}

impl Neighbourhoods {
    /// Merge each node's kept outgoing and incoming edges into one undirected
    /// neighbour list.
    pub fn from_adjacency(adj: &Adjacency) -> Self {
        let n = adj.len();
        let mut start = Vec::with_capacity(n + 1);
        start.push(0);
        let mut nbr: Vec<u32> = Vec::with_capacity(2 * adj.kept_edges());
        let mut row: Vec<u32> = Vec::new();
        // The index holds at most u32::MAX ids, so every dense index fits.
        for ix in 0..n as u32 {
            row.clear();
            row.extend(
                adj.outgoing(ix).iter().chain(adj.incoming(ix)).map(|inc| inc.other).filter(|&other| other != ix),
            );
            row.sort_unstable();
            row.dedup();
            nbr.extend_from_slice(&row);
            start.push(nbr.len());
        }
        Self { index: adj.index.clone(), start, nbr }
    }

    /// Distinct neighbours of `id`, either direction, itself excluded; 0 for
    /// an id the index does not hold.
    pub fn degree(&self, id: NodeId) -> usize {
        self.index_of(id).map_or(0, |ix| self.neighbours(ix).len())
    }

    fn index_of(&self, id: NodeId) -> Option<u32> {
        self.index.get(&id).copied()
    }

    fn neighbours(&self, ix: u32) -> &[u32] {
        let i = ix as usize;
        &self.nbr[self.start[i]..self.start[i + 1]]
    }

    /// One pair's scores: a sorted-list merge of the two neighbour lists.
    fn score(&self, x: NodeId, y: NodeId) -> PairScore {
        let mut s = PairScore::default();
        let (Some(a), Some(b)) = (self.index_of(x), self.index_of(y)) else {
            return s;
        };
        if a == b {
            return s;
        }
        let (na, nb) = (self.neighbours(a), self.neighbours(b));
        let (mut i, mut j) = (0, 0);
        while i < na.len() && j < nb.len() {
            match na[i].cmp(&nb[j]) {
                Ordering::Less => i += 1,
                Ordering::Greater => j += 1,
                Ordering::Equal => {
                    // z neighbours both a and b, so its degree is at least 2.
                    let deg = self.neighbours(na[i]).len() as u64;
                    s.common += 1;
                    s.adamic_adar_milli += adamic_adar_term(deg);
                    s.resource_allocation_micro += 1_000_000 / deg;
                    i += 1;
                    j += 1;
                }
            }
        }
        s
    }
}

/// A pair's link-prediction scores. All zero for a self pair, a pair with an
/// id the index does not hold, or a pair with no common neighbour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct PairScore {
    /// Common neighbours.
    pub common: u32,
    /// Adamic-Adar in thousandths: the sum of `round(1000 / ln deg z)`.
    pub adamic_adar_milli: u64,
    /// Resource allocation in millionths: the sum of `floor(1_000_000 / deg z)`.
    pub resource_allocation_micro: u64,
}

/// Score each pair, in order: `result[i]` is `pairs[i]`'s score. Adjacency
/// between the two ids does not matter, and neither counts as its own
/// common neighbour.
pub fn score_pairs(nb: &Neighbourhoods, pairs: &[(NodeId, NodeId)]) -> Vec<PairScore> {
    pairs.iter().map(|&(x, y)| nb.score(x, y)).collect()
}

/// One common neighbour's Adamic-Adar term in thousandths, rounded. `deg >= 2`
/// (a common neighbour's degree), so the term is at most 1443.
fn adamic_adar_term(deg: u64) -> u64 {
    (1000.0 / (deg as f64).ln()).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algo::{CategorySet, ToyGraph, toy_edge};
    use glia_core::EdgeCategoryId;

    fn ids(xs: &[u64]) -> Vec<NodeId> {
        xs.iter().map(|&x| NodeId(x)).collect()
    }

    fn nb_of(nodes: &[u64], edges: Vec<glia_core::Edge>) -> Neighbourhoods {
        let g = ToyGraph { nodes: ids(nodes), edges };
        Neighbourhoods::from_adjacency(&Adjacency::build(&g, &CategorySet::all()))
    }

    fn score(nb: &Neighbourhoods, x: u64, y: u64) -> PairScore {
        score_pairs(nb, &[(NodeId(x), NodeId(y))])[0]
    }

    fn ps(common: u32, aa: u64, ra: u64) -> PairScore {
        PairScore { common, adamic_adar_milli: aa, resource_allocation_micro: ra }
    }

    /// x = 1 and y = 2 share z = 3 (degree 2: x, y) and z = 4 (degree 3: x,
    /// y, 5); 6 hangs off x. Edge directions are mixed on purpose.
    fn textbook_edges() -> Vec<glia_core::Edge> {
        vec![
            toy_edge(1, 3, 1),
            toy_edge(3, 2, 1),
            toy_edge(4, 1, 1),
            toy_edge(2, 4, 1),
            toy_edge(4, 5, 1),
            toy_edge(1, 6, 1),
        ]
    }

    #[test]
    fn textbook_values() {
        assert_eq!(adamic_adar_term(2), 1443, "1000 / ln 2 = 1442.695");
        assert_eq!(adamic_adar_term(3), 910, "1000 / ln 3 = 910.239");
        let nb = nb_of(&[1, 2, 3, 4, 5, 6], textbook_edges());
        assert_eq!((nb.degree(NodeId(3)), nb.degree(NodeId(4)), nb.degree(NodeId(1))), (2, 3, 3));
        assert_eq!(score(&nb, 1, 2), ps(2, 1443 + 910, 500_000 + 333_333));
        assert_eq!(score(&nb, 1, 2), score(&nb, 2, 1), "symmetric");
        // 3 and 4 share x (degree 3: 3, 4, 6) and y (degree 2: 3, 4), summed
        // in ascending dense order.
        assert_eq!((nb.degree(NodeId(1)), nb.degree(NodeId(2))), (3, 2));
        assert_eq!(score(&nb, 3, 4), ps(2, 910 + 1443, 333_333 + 500_000));
        assert_eq!(score(&nb, 5, 6), PairScore::default(), "no common neighbour");
    }

    #[test]
    fn adjacent_pairs_still_score() {
        let mut edges = textbook_edges();
        edges.push(toy_edge(1, 2, 1));
        let nb = nb_of(&[1, 2, 3, 4, 5, 6], edges);
        // x and y now neighbour each other; neither is its own common neighbour,
        // and the degrees of 3 and 4 are unchanged.
        assert_eq!(score(&nb, 1, 2), ps(2, 2353, 833_333));
    }

    #[test]
    fn self_pair_scores_zero() {
        let nb = nb_of(&[1, 2, 3, 4, 5, 6], textbook_edges());
        assert_eq!(nb.degree(NodeId(1)), 3);
        assert_eq!(score(&nb, 1, 1), PairScore::default());
        assert_eq!(score(&nb, 4, 4), PairScore::default());
    }

    #[test]
    fn direction_ignored() {
        // a = 1, z = 2, b = 3: a -> z -> b, then a <- z <- b.
        let chain = nb_of(&[1, 2, 3], vec![toy_edge(1, 2, 1), toy_edge(2, 3, 1)]);
        let back = nb_of(&[1, 2, 3], vec![toy_edge(2, 1, 1), toy_edge(3, 2, 1)]);
        assert_eq!(score(&chain, 1, 3), ps(1, 1443, 500_000));
        assert_eq!(score(&chain, 1, 3), score(&back, 1, 3));
    }

    #[test]
    fn deterministic_under_edge_order() {
        // Parallel edges, a self-loop and dangling endpoints (7, 8) on top of
        // the textbook graph: reversing the list moves the dangling ids'
        // dense order, never a score or a degree.
        let mut edges = textbook_edges();
        edges.extend([toy_edge(1, 3, 1), toy_edge(3, 1, 1), toy_edge(3, 3, 1), toy_edge(4, 7, 1), toy_edge(8, 1, 1)]);
        let mut reversed = edges.clone();
        reversed.reverse();
        let nodes = [1, 2, 3, 4, 5, 6];
        let (fwd, rev) = (nb_of(&nodes, edges), nb_of(&nodes, reversed));
        let pairs: Vec<(NodeId, NodeId)> = [(1, 2), (2, 1), (3, 4), (7, 1), (8, 4), (5, 7), (6, 8), (1, 1)]
            .iter()
            .map(|&(x, y)| (NodeId(x), NodeId(y)))
            .collect();
        let scores = score_pairs(&fwd, &pairs);
        assert_eq!(scores, score_pairs(&rev, &pairs));
        // 3 stays degree 2 (the parallel edges and the self-loop add nothing);
        // 4 gains the dangling 7, so degree 4: 1000 / ln 4 = 721.35.
        assert_eq!(scores[0], ps(2, 1443 + 721, 500_000 + 250_000));
        assert_eq!(scores[3], ps(1, 721, 250_000), "7 and 1 share 4 (neighbours 1, 2, 5, 7)");
        for id in [1, 2, 3, 4, 7, 8] {
            assert_eq!(fwd.degree(NodeId(id)), rev.degree(NodeId(id)));
        }
        assert_eq!((fwd.degree(NodeId(3)), fwd.degree(NodeId(1))), (2, 4));
    }

    #[test]
    fn unknown_ids_score_zero() {
        let nb = nb_of(&[1, 2, 3, 4, 5, 6], textbook_edges());
        let pairs =
            [(NodeId(1), NodeId(99)), (NodeId(99), NodeId(2)), (NodeId(98), NodeId(99)), (NodeId(1), NodeId(2))];
        let scores = score_pairs(&nb, &pairs);
        assert_eq!(scores.len(), pairs.len(), "index-aligned with the pairs");
        assert_eq!(scores[..3], [PairScore::default(); 3]);
        assert_eq!(scores[3].common, 2);
        assert_eq!(nb.degree(NodeId(99)), 0);
        let empty = Neighbourhoods::from_adjacency(&Adjacency::default());
        assert_eq!(score_pairs(&empty, &pairs[..1]), vec![PairScore::default()]);
        assert_eq!(empty.degree(NodeId(1)), 0);
    }

    #[test]
    fn categories_come_from_the_index() {
        // 3 joins 1 and 2 only through category-2 edges; an index that keeps
        // category 1 alone sees no common neighbour.
        let g =
            ToyGraph { nodes: ids(&[1, 2, 3]), edges: vec![toy_edge(1, 3, 2), toy_edge(2, 3, 2), toy_edge(1, 2, 1)] };
        let only_1 = Neighbourhoods::from_adjacency(&Adjacency::build(&g, &CategorySet::of(&[EdgeCategoryId(1)])));
        assert_eq!(score(&only_1, 1, 2), PairScore::default());
        assert_eq!(only_1.degree(NodeId(3)), 0);
        let both = Neighbourhoods::from_adjacency(&Adjacency::build(&g, &CategorySet::all()));
        assert_eq!(score(&both, 1, 2), ps(1, 1443, 500_000));
    }
}
