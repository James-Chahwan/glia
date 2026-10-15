//! Stratification of the predicate dependency graph over
//! `activation::algo::cycles` (DA.5a). Analysis (DA.5b) hands [`stratify`]
//! one [`DepEdge`] per body literal that reads a rule-defined predicate:
//! `body` is the predicate read, `head` the predicate the rule defines, and
//! [`Dep`] says how it is read (positively, under negation, or by an
//! aggregate).
//!
//! The predicates are a tiny [`GraphSource`] (predicate `p` is `NodeId(p)`,
//! every edge `body -> head`), so the recursive components are
//! [`strongly_connected`]'s non-trivial components: two or more predicates
//! that read each other, or one that reads itself. Every other predicate is
//! a component of its own. glia is stricter than CodeQL's even-negation rule:
//! a negated or aggregated read never stays inside a component, so a `Neg` /
//! `Agg` edge with both ends in one component (a self edge included) is a
//! [`BadCycle`], and analysis words it.
//!
//! A stratifiable program gets one stratum per component, in a topological
//! order of the condensation (Kahn's algorithm, the ready component with the
//! smallest member taken first), so every edge, and every `Neg` / `Agg` edge
//! in particular, runs from a lower stratum to a higher one. Predicate indexes
//! are first-definition order and nothing here reads an input's order except
//! a [`BadCycle`]'s first offending edge, so a predicate's stratum is a
//! function of the program text. No hash map is iterated and nothing recurses:
//! Tarjan in activation is iterative and Kahn here loops over a min-heap.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use glia_activation::algo::cycles::strongly_connected;
use glia_activation::algo::{Adjacency, CategorySet, GraphSource};
use glia_core::{Confidence, Edge, EdgeCategoryId, NodeId};

/// How a rule's body reads a predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dep {
    /// A positive atom: may sit inside recursion.
    Pos,
    /// A negated atom: the read predicate must be complete first.
    Neg,
    /// An atom of an aggregate rule's body: complete first, like negation.
    Agg,
}

impl Dep {
    /// The edge category this read is indexed under. The index keeps every
    /// category, so the value only tells the three reads apart.
    fn category(self) -> EdgeCategoryId {
        EdgeCategoryId(match self {
            Dep::Pos => 0,
            Dep::Neg => 1,
            Dep::Agg => 2,
        })
    }
}

/// One dependency: the rule defining `head` reads `body` as `dep`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DepEdge {
    /// The predicate read.
    pub(crate) body: u32,
    /// The predicate defined.
    pub(crate) head: u32,
    pub(crate) dep: Dep,
    /// The caller's reference to the literal, echoed in a [`BadCycle`].
    pub(crate) site: usize,
}

/// The strata of a stratifiable program. Every per-predicate vector has one
/// entry per predicate of the range (see [`stratify`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Strata {
    /// Per predicate: its stratum, an index into `order`.
    pub(crate) stratum_of: Vec<usize>,
    /// Per stratum, lowest first: its predicates, ascending.
    pub(crate) order: Vec<Vec<u32>>,
    /// Per predicate: in a component of two or more, or reading itself, so
    /// its rules run as a fixpoint.
    pub(crate) recursive: Vec<bool>,
    /// Per predicate: its component's smallest member (itself when alone).
    pub(crate) component: Vec<u32>,
}

/// A component a negated or aggregated read stays inside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BadCycle {
    /// The component's predicates, ascending.
    pub(crate) preds: Vec<u32>,
    /// `Neg` or `Agg`: the first offending edge's read.
    pub(crate) dep: Dep,
    /// The first offending edge's site, in input order.
    pub(crate) site: usize,
}

/// The predicate dependency graph as a [`GraphSource`]: predicate `p` is
/// `NodeId(p)`, listed in index order, and each [`DepEdge`] is one edge in
/// input order, so the index's dense position of a predicate is the predicate.
struct PredGraph {
    nodes: Vec<NodeId>,
    edges: Vec<Edge>,
}

impl GraphSource for PredGraph {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

/// Stratify predicates `0..npreds` under `edges`.
///
/// The predicate range is `0..npreds`, widened to cover any edge endpoint at
/// or past `npreds`, so no edge is dropped. `Err` lists every [`BadCycle`],
/// one per offending component, in component order (by smallest member),
/// each with the read and site of its first offending edge in input order.
pub(crate) fn stratify(npreds: usize, edges: &[DepEdge]) -> Result<Strata, Vec<BadCycle>> {
    let n = edges.iter().map(|e| e.body.max(e.head) as usize + 1).fold(npreds, usize::max);
    let graph = PredGraph {
        nodes: (0..n as u64).map(NodeId).collect(),
        edges: edges
            .iter()
            .map(|e| {
                Edge::new(NodeId(u64::from(e.body)), NodeId(u64::from(e.head)), e.dep.category(), Confidence::Strong)
            })
            .collect(),
    };
    let adj = Adjacency::build(&graph, &CategorySet::all());

    // Ids are predicates (each < n <= u32::MAX + 1), members sorted, the
    // components sorted by their first member.
    let recursive_components: Vec<Vec<u32>> = strongly_connected(&adj)
        .into_iter()
        .map(|members| members.into_iter().map(|id| id.0 as u32).collect())
        .collect();
    let mut component: Vec<u32> = (0..n as u32).collect();
    let mut recursive = vec![false; n];
    for members in &recursive_components {
        for &p in members {
            component[p as usize] = members[0];
            recursive[p as usize] = true;
        }
    }

    // Per component (by its smallest member): its first offending edge.
    let mut offence: Vec<Option<(Dep, usize)>> = vec![None; n];
    for e in edges {
        let rep = component[e.body as usize];
        if e.dep != Dep::Pos && rep == component[e.head as usize] && offence[rep as usize].is_none() {
            offence[rep as usize] = Some((e.dep, e.site));
        }
    }
    // A self edge makes its predicate a recursive component, so every
    // offending component is one strongly_connected listed.
    let bad: Vec<BadCycle> = recursive_components
        .iter()
        .filter_map(|members| {
            offence[members[0] as usize].map(|(dep, site)| BadCycle { preds: members.clone(), dep, site })
        })
        .collect();
    if !bad.is_empty() {
        return Err(bad);
    }

    // The condensation, indexed by smallest member: members ascending, and
    // one out-entry and one in-degree count per crossing edge.
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); n];
    for p in 0..n as u32 {
        members[component[p as usize] as usize].push(p);
    }
    let mut out: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut indegree = vec![0usize; n];
    for e in edges {
        let (from, to) = (component[e.body as usize], component[e.head as usize]);
        if from != to {
            out[from as usize].push(to);
            indegree[to as usize] += 1;
        }
    }

    // Kahn over components, the ready one with the smallest member first.
    let mut ready: BinaryHeap<Reverse<u32>> =
        (0..n as u32).filter(|&c| component[c as usize] == c && indegree[c as usize] == 0).map(Reverse).collect();
    let mut stratum_of_component = vec![0usize; n];
    let mut order: Vec<Vec<u32>> = Vec::new();
    while let Some(Reverse(c)) = ready.pop() {
        stratum_of_component[c as usize] = order.len();
        order.push(std::mem::take(&mut members[c as usize]));
        for &to in &out[c as usize] {
            indegree[to as usize] -= 1;
            if indegree[to as usize] == 0 {
                ready.push(Reverse(to));
            }
        }
    }
    // The condensation is acyclic, so every component was emitted.
    let stratum_of = component.iter().map(|&c| stratum_of_component[c as usize]).collect();

    Ok(Strata { stratum_of, order, recursive, component })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(body: u32, head: u32, dep: Dep, site: usize) -> DepEdge {
        DepEdge { body, head, dep, site }
    }

    fn pos(body: u32, head: u32) -> DepEdge {
        edge(body, head, Dep::Pos, 0)
    }

    fn ok(npreds: usize, edges: &[DepEdge]) -> Strata {
        match stratify(npreds, edges) {
            Ok(s) => s,
            Err(bad) => panic!("expected strata, got {bad:?}"),
        }
    }

    fn err(npreds: usize, edges: &[DepEdge]) -> Vec<BadCycle> {
        match stratify(npreds, edges) {
            Ok(s) => panic!("expected bad cycles, got {s:?}"),
            Err(bad) => bad,
        }
    }

    /// Every permutation of `items` (Heap's algorithm, iterative).
    fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
        let mut a = items.to_vec();
        let mut c = vec![0usize; a.len()];
        let mut all = vec![a.clone()];
        let mut i = 0;
        while i < a.len() {
            if c[i] < i {
                if i % 2 == 0 {
                    a.swap(0, i);
                } else {
                    a.swap(c[i], i);
                }
                all.push(a.clone());
                c[i] += 1;
                i = 0;
            } else {
                c[i] = 0;
                i += 1;
            }
        }
        all
    }

    #[test]
    fn independent_in_index_order() {
        let s = ok(2, &[]);
        assert_eq!(s.order, vec![vec![0], vec![1]]);
        assert_eq!(s.stratum_of, vec![0, 1]);
        assert_eq!(s.recursive, vec![false, false]);
        assert_eq!(s.component, vec![0, 1]);

        // 2 reads both: it comes after them, whichever it reads first.
        for edges in [[pos(0, 2), pos(1, 2)], [pos(1, 2), pos(0, 2)]] {
            let s = ok(3, &edges);
            assert_eq!(s.order, vec![vec![0], vec![1], vec![2]]);
            assert_eq!(s.stratum_of, vec![0, 1, 2]);
        }

        // An independent predicate with a larger index than a dependent one
        // still waits its turn: the smallest ready component goes first.
        let s = ok(3, &[pos(2, 0)]);
        assert_eq!(s.order, vec![vec![1], vec![2], vec![0]]);
        assert_eq!(s.stratum_of, vec![2, 0, 1]);

        assert_eq!(ok(0, &[]), Strata { stratum_of: vec![], order: vec![], recursive: vec![], component: vec![] });
    }

    #[test]
    fn recursion_is_one_stratum() {
        let s = ok(2, &[pos(0, 1), pos(1, 0)]);
        assert_eq!(s.order, vec![vec![0, 1]]);
        assert_eq!(s.stratum_of, vec![0, 0]);
        assert_eq!(s.recursive, vec![true, true]);
        assert_eq!(s.component, vec![0, 0]);

        // A self edge marks 2 recursive alone; 3, reading 2 with no self
        // edge, is not recursive.
        let s = ok(4, &[pos(0, 1), pos(1, 0), pos(2, 2), pos(2, 3)]);
        assert_eq!(s.recursive, vec![true, true, true, false]);
        assert_eq!(s.component, vec![0, 0, 2, 3]);
        assert_eq!(s.order, vec![vec![0, 1], vec![2], vec![3]]);

        // A component's members sit ascending in one stratum, named by the
        // smallest, however the cycle is written.
        let s = ok(4, &[pos(3, 1), pos(1, 2), pos(2, 3), pos(0, 3)]);
        assert_eq!(s.order, vec![vec![0], vec![1, 2, 3]]);
        assert_eq!(s.component, vec![0, 1, 1, 1]);
        assert_eq!(s.stratum_of, vec![0, 1, 1, 1]);
        assert_eq!(s.recursive, vec![false, true, true, true]);
    }

    #[test]
    fn negation_across_strata() {
        let s = ok(2, &[edge(0, 1, Dep::Neg, 4)]);
        assert!(s.stratum_of[0] < s.stratum_of[1]);

        // Read under negation from a higher index: still lower.
        let s = ok(2, &[edge(1, 0, Dep::Neg, 4)]);
        assert!(s.stratum_of[1] < s.stratum_of[0]);
        assert_eq!(s.order, vec![vec![1], vec![0]]);

        // Negation of a recursive component from outside it is fine, and an
        // aggregate over it likewise: every such edge runs upward.
        let edges = [pos(0, 1), pos(1, 0), edge(1, 2, Dep::Neg, 5), edge(0, 3, Dep::Agg, 6)];
        let s = ok(4, &edges);
        for e in &edges {
            if e.dep != Dep::Pos {
                assert!(s.stratum_of[e.body as usize] < s.stratum_of[e.head as usize], "{e:?}");
            }
        }
    }

    #[test]
    fn negation_inside_recursion() {
        assert_eq!(
            err(2, &[pos(0, 1), edge(1, 0, Dep::Neg, 7)]),
            vec![BadCycle { preds: vec![0, 1], dep: Dep::Neg, site: 7 }]
        );

        // Two bad components, the later one's edges first in the input:
        // listed in component order, each with its first offending edge in
        // input order (site 9 before site 3), not its smallest site.
        let bad = err(
            5,
            &[
                pos(3, 4),
                edge(4, 3, Dep::Agg, 11),
                edge(1, 0, Dep::Neg, 9),
                pos(0, 1),
                edge(0, 1, Dep::Neg, 3),
                pos(2, 2),
            ],
        );
        assert_eq!(
            bad,
            vec![
                BadCycle { preds: vec![0, 1], dep: Dep::Neg, site: 9 },
                BadCycle { preds: vec![3, 4], dep: Dep::Agg, site: 11 },
            ]
        );

        // A negated self read is a one-predicate bad cycle.
        assert_eq!(err(1, &[edge(0, 0, Dep::Neg, 2)]), vec![BadCycle { preds: vec![0], dep: Dep::Neg, site: 2 }]);
    }

    #[test]
    fn aggregate_inside_recursion() {
        assert_eq!(
            err(3, &[pos(0, 1), edge(1, 1, Dep::Agg, 12)]),
            vec![BadCycle { preds: vec![1], dep: Dep::Agg, site: 12 }]
        );
        // A positive self read is recursion, not an error.
        let s = ok(1, &[pos(0, 0)]);
        assert_eq!(s.recursive, vec![true]);
        assert_eq!(s.order, vec![vec![0]]);
    }

    #[test]
    fn order_is_text_determined() {
        let edges = [
            pos(1, 2),
            pos(2, 1),
            edge(0, 1, Dep::Neg, 1),
            edge(2, 4, Dep::Agg, 2),
            pos(5, 4),
            edge(3, 0, Dep::Neg, 3),
        ];
        // Ready at the start: 3 and 5; 3 first frees 0, 0 frees {1, 2}, which
        // with 5 frees 4.
        let want = Strata {
            stratum_of: vec![1, 2, 2, 0, 4, 3],
            order: vec![vec![3], vec![0], vec![1, 2], vec![5], vec![4]],
            recursive: vec![false, true, true, false, false, false],
            component: vec![0, 1, 1, 3, 4, 5],
        };
        assert_eq!(ok(6, &edges), want);
        let perms = permutations(&edges);
        assert_eq!(perms.len(), 720);
        for p in &perms {
            assert_eq!(ok(6, p), want, "{p:?}");
        }
    }

    #[test]
    fn long_chain() {
        let n = 10_000u32;
        let chain: Vec<DepEdge> = (0..n - 1).map(|p| pos(p, p + 1)).collect();
        let s = ok(n as usize, &chain);
        assert_eq!(s.order.len(), n as usize);
        assert!(s.stratum_of.iter().enumerate().all(|(p, &k)| k == p));
        assert!(s.recursive.iter().all(|&r| !r));

        // Closed into one cycle: one recursive stratum of every predicate.
        let mut cycle = chain;
        cycle.push(pos(n - 1, 0));
        let s = ok(n as usize, &cycle);
        assert_eq!(s.order, vec![(0..n).collect::<Vec<u32>>()]);
        assert!(s.recursive.iter().all(|&r| r));
        assert!(s.component.iter().all(|&c| c == 0));
    }

    #[test]
    fn endpoints_past_npreds_widen_the_range() {
        let s = ok(1, &[pos(0, 2)]);
        assert_eq!(s.stratum_of.len(), 3);
        assert_eq!(s.order, vec![vec![0], vec![1], vec![2]]);
        assert_eq!(err(0, &[edge(1, 1, Dep::Neg, 8)]), vec![BadCycle { preds: vec![1], dep: Dep::Neg, site: 8 }]);
    }
}
