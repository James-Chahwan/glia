//! Identity classes: a union-find over NodeIds (min-id representative, path
//! compression) and a Quotient<G: GraphSource> adapter that maps every node to
//! its class representative, so a walk visits a class once. Filled by DD.1.
//!
//! A class is a set of node ids a domain says are one thing (the same package
//! in two repos, one message type copied into three). [`Classes`] holds them;
//! [`Quotient`] is a [`GraphSource`] view of a graph in which each class is
//! ONE node, so [`Adjacency::build`](crate::algo::Adjacency::build),
//! [`reach::bfs`](crate::algo::reach::bfs) and PPR cross a class at zero hops
//! instead of needing a carry edge between the copies. An edge between two
//! members of one class (the identity edge itself, or a carry edge the domain
//! also drew between the copies) is dropped and counted
//! ([`Quotient::collapsed_edges`]), so such a pair is walked once, not twice.
//!
//! Domain-free: the caller names the categories that join a class and an
//! edge filter ([`Classes::from_source`]), or hands over the pairs
//! ([`Classes::from_pairs`]); nothing here names a node kind or category.
//!
//! Determinism: no hash map anywhere. Members are kept in a sorted `Vec` and
//! looked up by binary search, and the union-find runs over dense indices
//! assigned in NodeId order, so a class, its member order and its
//! representative depend only on the SET of pairs, never on their order.
//!
//! The representative is the class's minimum NodeId. It is an internal
//! handle for walks, never an identity to show a reader: it moves when a
//! RepoId (and so a NodeId) changes. A domain that persists a class (a star
//! of identity edges) chooses its center by content instead, and an answer
//! maps a representative back to a member it displays.
//!
//! Hand-rolled rather than an e-graph library (egg / egglog): that would be a
//! new dependency, its e-class ids are not stable across runs, and there is
//! no rewriting to do here.

use glia_core::{Edge, NodeId};

use super::{CategorySet, GraphSource};

/// Disjoint sets over dense indices `0..n`. The root of a set is always its
/// SMALLEST index ([`Self::union`] links the larger root under the smaller,
/// and compression only ever points a node at its root), so with indices
/// assigned in NodeId order the root is the minimum NodeId.
///
/// Invariant: `parent[i] <= i` for every `i`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    /// `n` singleton sets. Over `u32::MAX` indices (the dense-index limit
    /// [`Adjacency`](crate::algo::Adjacency) shares) the set is empty, never a
    /// panic.
    pub fn new(n: usize) -> Self {
        if n > u32::MAX as usize {
            eprintln!("[algo] union-find: more than {} indices - u32 index limit, returning an empty set", u32::MAX);
            return Self::default();
        }
        Self { parent: (0..n as u32).collect() }
    }

    /// The root (smallest index) of `i`'s set, with full path compression:
    /// every index on the path then points straight at the root. Iterative
    /// (one pass up, one pass rewriting), so a long chain cannot overflow a
    /// stack. An index at or past [`Self::len`] is outside the set and is its
    /// own root.
    pub fn find(&mut self, i: u32) -> u32 {
        if i as usize >= self.parent.len() {
            return i;
        }
        let mut root = i;
        while self.parent[root as usize] != root {
            root = self.parent[root as usize];
        }
        let mut cur = i;
        while cur != root {
            let next = self.parent[cur as usize];
            self.parent[cur as usize] = root;
            cur = next;
        }
        root
    }

    /// Join `a`'s and `b`'s sets and return the root `a` belongs to after:
    /// `min(find(a), find(b))`. An index outside the set links nothing, so
    /// the result is then `find(a)`.
    pub fn union(&mut self, a: u32, b: u32) -> u32 {
        let (ra, rb) = (self.find(a), self.find(b));
        let n = self.parent.len();
        if ra as usize >= n || rb as usize >= n {
            return ra;
        }
        let (lo, hi) = if ra <= rb { (ra, rb) } else { (rb, ra) };
        if lo != hi {
            self.parent[hi as usize] = lo;
        }
        lo
    }

    /// Indices in the set.
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }
}

/// Equivalence classes of NodeIds. Only classes of two or more members are
/// kept: an id in no pair (or only in pairs with itself) is its own
/// representative and in no class.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Classes {
    /// Every member of every class, sorted by `.0`, each once.
    ids: Vec<NodeId>,
    /// `class[i]` is the position in `classes` of `ids[i]`'s class.
    class: Vec<u32>,
    /// `(representative, members ascending)`, sorted by representative; the
    /// representative is the first (smallest) member.
    classes: Vec<(NodeId, Vec<NodeId>)>,
}

impl Classes {
    /// The classes the `pairs` join, transitively. A pair `(a, a)` and a
    /// repeated pair change nothing, and neither the pair order nor the order
    /// within a pair changes the result.
    pub fn from_pairs<I: IntoIterator<Item = (NodeId, NodeId)>>(pairs: I) -> Self {
        Self::from_pairs_capped(pairs, u32::MAX as usize)
    }

    /// The classes `g`'s edges join: every edge whose category `keep`
    /// contains and that `filter` accepts is one pair, in one pass over
    /// `g.edges()`. Its direction does not matter.
    pub fn from_source<G: GraphSource + ?Sized>(g: &G, keep: &CategorySet, filter: impl Fn(&Edge) -> bool) -> Self {
        Self::from_pairs(g.edges().filter(|e| keep.contains(e.category) && filter(e)).map(|e| (e.from, e.to)))
    }

    /// [`Self::from_pairs`] with the dense-index limit as a parameter, so the
    /// over-limit path is testable. Over `cap` distinct ids the result is
    /// empty and a warning is printed.
    fn from_pairs_capped<I: IntoIterator<Item = (NodeId, NodeId)>>(pairs: I, cap: usize) -> Self {
        let pairs: Vec<(NodeId, NodeId)> = pairs.into_iter().filter(|(a, b)| a != b).collect();
        if pairs.is_empty() {
            return Self::default();
        }
        let mut ids: Vec<NodeId> = pairs.iter().flat_map(|&(a, b)| [a, b]).collect();
        ids.sort_unstable_by_key(|id| id.0);
        ids.dedup();
        if ids.len() > cap {
            eprintln!("[algo] classes: more than {cap} ids - u32 index limit, returning no classes");
            return Self::default();
        }
        let n = ids.len();
        let mut uf = UnionFind::new(n);
        for (a, b) in &pairs {
            if let (Ok(i), Ok(j)) = (dense(&ids, *a), dense(&ids, *b)) {
                uf.union(i as u32, j as u32);
            }
        }
        // Every id is an endpoint of a pair with a different partner, so no
        // set is a singleton. A root is its set's smallest index, so walking
        // the indices upwards meets each root before the rest of its set and
        // pushes members in ascending order.
        let mut class_of_root = vec![0u32; n];
        let mut class = Vec::with_capacity(n);
        let mut classes: Vec<(NodeId, Vec<NodeId>)> = Vec::new();
        for (i, &id) in ids.iter().enumerate() {
            let root = uf.find(i as u32) as usize;
            if root == i {
                class_of_root[i] = classes.len() as u32;
                classes.push((id, Vec::new()));
            }
            let c = class_of_root[root];
            classes[c as usize].1.push(id);
            class.push(c);
        }
        Self { ids, class, classes }
    }

    /// The representative of `id`'s class: the class's minimum id, or `id`
    /// itself when it is in no class.
    pub fn rep(&self, id: NodeId) -> NodeId {
        match dense(&self.ids, id) {
            Ok(i) => self.classes[self.class[i] as usize].0,
            Err(_) => id,
        }
    }

    /// The members of `id`'s class, ascending, when it is in one.
    pub fn class_of(&self, id: NodeId) -> Option<&[NodeId]> {
        dense(&self.ids, id).ok().map(|i| self.classes[self.class[i] as usize].1.as_slice())
    }

    /// Whether `a` and `b` are one node: equal, or members of one class.
    pub fn same(&self, a: NodeId, b: NodeId) -> bool {
        self.rep(a) == self.rep(b)
    }

    /// Every class as `(representative, members ascending)`, by
    /// representative ascending.
    pub fn classes(&self) -> impl Iterator<Item = (NodeId, &[NodeId])> + '_ {
        self.classes.iter().map(|(rep, members)| (*rep, members.as_slice()))
    }

    /// Classes (each of two or more members).
    pub fn len(&self) -> usize {
        self.classes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    /// Ids in any class.
    pub fn members(&self) -> usize {
        self.ids.len()
    }
}

/// `id`'s position in the sorted `ids`.
fn dense(ids: &[NodeId], id: NodeId) -> Result<usize, usize> {
    ids.binary_search_by_key(&id.0, |x| x.0)
}

/// `inner` with every class contracted to its representative: a
/// [`GraphSource`], so the existing [`Adjacency`](crate::algo::Adjacency)
/// indexes it unchanged.
///
/// - Nodes: `inner`'s node ids mapped through [`Classes::rep`], each kept at
///   its first occurrence. A class member that is no node of `inner` can
///   still be a representative, so a walk may reach an id `inner` does not
///   list; the caller maps it back through its own display rule.
/// - Edges, in `inner`'s order (so Adjacency's parity rule 2 holds over the
///   quotient), each endpoint mapped through [`Classes::rep`]: an edge whose
///   original endpoints differ but map to one representative is intra-class
///   and dropped, counted by [`Self::collapsed_edges`]; a genuine self-loop
///   (`from == to` before mapping) is kept. Kept edges carry no cells: walks
///   and PPR read endpoints, category and confidence only, and cloning every
///   evidence cell per query would cost an allocation per edge.
///
/// With empty classes the node ids and the `(from, to, category,
/// confidence)` of every edge equal `inner`'s.
pub struct Quotient<'g, G: GraphSource + ?Sized> {
    inner: &'g G,
    classes: &'g Classes,
    nodes: Vec<NodeId>,
    edges: Vec<Edge>,
    collapsed: usize,
}

impl<'g, G: GraphSource + ?Sized> Quotient<'g, G> {
    /// The quotient of `inner` by `classes`, in one pass over its nodes and
    /// one over its edges.
    pub fn new(inner: &'g G, classes: &'g Classes) -> Self {
        let nodes = first_occurrences(inner.node_ids().into_iter().map(|id| classes.rep(id)).collect());
        let mut edges = Vec::new();
        let mut collapsed = 0usize;
        for e in inner.edges() {
            let (from, to) = (classes.rep(e.from), classes.rep(e.to));
            if from == to && e.from != e.to {
                collapsed += 1;
                continue;
            }
            edges.push(Edge::new(from, to, e.category, e.confidence));
        }
        Self { inner, classes, nodes, edges, collapsed }
    }

    /// The node `id` is in the quotient: its class representative, or `id`.
    pub fn rep(&self, id: NodeId) -> NodeId {
        self.classes.rep(id)
    }

    /// Edges of `inner` dropped because both ends fell in one class.
    pub fn collapsed_edges(&self) -> usize {
        self.collapsed
    }

    /// The graph this is the quotient of.
    pub fn inner(&self) -> &'g G {
        self.inner
    }
}

impl<G: GraphSource + ?Sized> GraphSource for Quotient<'_, G> {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

/// `ids` with each repeated id kept only at its first occurrence, in order.
/// Hash-free: a stable sort by id puts each id's first position first.
fn first_occurrences(ids: Vec<NodeId>) -> Vec<NodeId> {
    let mut order: Vec<usize> = (0..ids.len()).collect();
    order.sort_by_key(|&i| ids[i].0);
    let mut keep = vec![false; ids.len()];
    let mut prev: Option<u64> = None;
    for &i in &order {
        if prev != Some(ids[i].0) {
            keep[i] = true;
            prev = Some(ids[i].0);
        }
    }
    ids.into_iter().zip(keep).filter_map(|(id, k)| k.then_some(id)).collect()
}

#[cfg(test)]
mod tests {
    use glia_core::{Confidence, EdgeCategoryId};

    use super::*;
    use crate::algo::reach::bfs;
    use crate::algo::{Adjacency, ToyGraph, Walk, toy_edge};

    const X: EdgeCategoryId = EdgeCategoryId(1);
    const Y: EdgeCategoryId = EdgeCategoryId(2);
    const CALLS: EdgeCategoryId = EdgeCategoryId(3);
    const SAME: EdgeCategoryId = EdgeCategoryId(4);

    fn n(x: u64) -> NodeId {
        NodeId(x)
    }

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
    }

    fn signature(g: &dyn GraphSource) -> Vec<(u64, u64, u32, Confidence)> {
        g.edges().map(|e| (e.from.0, e.to.0, e.category.0, e.confidence)).collect()
    }

    #[test]
    fn union_find_root_is_min() {
        let mut uf = UnionFind::new(10);
        assert_eq!(uf.union(5, 2), 2);
        assert_eq!(uf.union(2, 9), 2);
        assert_eq!(uf.union(7, 9), 2);
        assert_eq!((uf.find(9), uf.find(7), uf.find(5), uf.find(2)), (2, 2, 2, 2));
        // Untouched indices stay singletons; a repeated union changes nothing.
        assert_eq!((uf.find(0), uf.find(8)), (0, 8));
        assert_eq!(uf.union(9, 5), 2);
        assert_eq!(uf.len(), 10);
        assert!(uf.parent.iter().enumerate().all(|(i, &p)| p as usize <= i));
    }

    #[test]
    fn path_compression() {
        // Descending unions build a true chain 9_999 -> 9_998 -> .. -> 0: each
        // union links a fresh root under the index just below it.
        let len = 10_000u32;
        let mut uf = UnionFind::new(len as usize);
        for i in (0..len - 1).rev() {
            uf.union(i, i + 1);
        }
        assert_eq!(uf.parent[9_999], 9_998, "the walk under test is a chain");
        assert_eq!(uf.find(9_999), 0);
        assert_eq!(uf.parent[9_999], 0, "one find points the far end at the root");
        assert!(uf.parent.iter().all(|&p| p == 0), "and every index on the path");
        // Ascending unions (the other order) give the same root everywhere.
        let mut up = UnionFind::new(len as usize);
        for i in 0..len - 1 {
            up.union(i, i + 1);
        }
        assert_eq!(up.find(9_999), 0);
        assert_eq!(up.parent[9_999], 0);
        // A million-long chain: iterative, so no stack to overflow.
        let mut big = UnionFind::new(1_000_000);
        for i in (0..999_999u32).rev() {
            big.union(i, i + 1);
        }
        assert_eq!(big.find(999_999), 0);
    }

    #[test]
    fn union_find_outside_the_set() {
        let mut uf = UnionFind::new(3);
        assert_eq!(uf.find(7), 7);
        assert_eq!(uf.union(1, 7), 1, "links nothing, returns find(a)");
        assert_eq!(uf.union(7, 1), 7);
        assert_eq!(uf.find(1), 1);
        // Over the u32 index limit: empty, without allocating.
        let over = UnionFind::new(u32::MAX as usize + 1);
        assert!(over.is_empty());
        assert_eq!(over.len(), 0);
    }

    /// Components of `pairs` by repeated label relaxation: the oracle.
    fn oracle(pairs: &[(NodeId, NodeId)]) -> Vec<Vec<u64>> {
        let mut label: std::collections::BTreeMap<u64, u64> = std::collections::BTreeMap::new();
        for &(a, b) in pairs {
            if a != b {
                label.insert(a.0, a.0);
                label.insert(b.0, b.0);
            }
        }
        loop {
            let mut changed = false;
            for &(a, b) in pairs {
                if a == b {
                    continue;
                }
                let m = label[&a.0].min(label[&b.0]);
                for x in [a.0, b.0] {
                    if label[&x] != m {
                        label.insert(x, m);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        let mut groups: std::collections::BTreeMap<u64, Vec<u64>> = std::collections::BTreeMap::new();
        for (id, l) in label {
            groups.entry(l).or_default().push(id);
        }
        groups.into_values().collect()
    }

    #[test]
    fn classes_order_invariant() {
        for seed in 0..20u64 {
            let mut r = Lcg(seed ^ 0xC1A55);
            let pairs: Vec<(NodeId, NodeId)> = (0..200).map(|_| (n(r.next() % 60), n(r.next() % 60))).collect();
            let forward = Classes::from_pairs(pairs.iter().copied());
            let reversed = Classes::from_pairs(pairs.iter().rev().copied());
            let swapped = Classes::from_pairs(pairs.iter().map(|&(a, b)| (b, a)));
            assert_eq!(forward, reversed, "seed {seed}");
            assert_eq!(forward, swapped, "seed {seed}");

            let got: Vec<Vec<u64>> = forward.classes().map(|(_, m)| m.iter().map(|x| x.0).collect()).collect();
            assert_eq!(got, oracle(&pairs), "seed {seed}");
            let reps: Vec<u64> = forward.classes().map(|(rep, _)| rep.0).collect();
            assert!(reps.windows(2).all(|w| w[0] < w[1]), "classes ascending by rep");
            let mut total = 0;
            for (rep, members) in forward.classes() {
                assert!(members.len() >= 2);
                assert!(members.windows(2).all(|w| w[0].0 < w[1].0), "members ascending");
                assert_eq!(rep, members[0], "rep is the class minimum");
                for &m in members {
                    assert_eq!(forward.rep(m), rep);
                    assert_eq!(forward.class_of(m), Some(members));
                    assert!(forward.same(m, rep));
                }
                total += members.len();
            }
            assert_eq!(forward.members(), total);
            for x in 0..60 {
                if forward.class_of(n(x)).is_none() {
                    assert_eq!(forward.rep(n(x)), n(x));
                }
            }
        }
    }

    #[test]
    fn self_and_repeated_pairs() {
        let (a, b, c) = (n(40), n(11), n(3));
        let classes = Classes::from_pairs([(a, a), (a, b), (b, a), (a, b)]);
        assert_eq!(classes.len(), 1);
        assert_eq!(classes.members(), 2);
        assert!(!classes.is_empty());
        assert_eq!(classes.classes().collect::<Vec<_>>(), vec![(b, &[b, a][..])]);
        assert_eq!(classes.rep(a), b);
        assert_eq!(classes.rep(c), c);
        assert_eq!(classes.class_of(c), None);
        assert!(classes.same(c, c) && !classes.same(a, c));
        // Only self-pairs: no class at all.
        let none = Classes::from_pairs([(a, a), (c, c)]);
        assert!(none.is_empty());
        assert_eq!(none, Classes::default());
    }

    #[test]
    fn over_the_index_limit_is_no_classes() {
        let pairs = [(n(1), n(2)), (n(2), n(3))];
        assert!(Classes::from_pairs_capped(pairs, 2).is_empty());
        assert_eq!(Classes::from_pairs_capped(pairs, 3).members(), 3);
    }

    #[test]
    fn from_source_filters() {
        let g = ToyGraph {
            nodes: (1..=6).map(n).collect(),
            edges: vec![toy_edge(1, 2, X.0), toy_edge(3, 4, Y.0), toy_edge(5, 6, X.0)],
        };
        let classes = Classes::from_source(&g, &CategorySet::of(&[X]), |e| e.from != n(5));
        assert_eq!(classes.classes().collect::<Vec<_>>(), vec![(n(1), &[n(1), n(2)][..])]);
        let both = Classes::from_source(&g, &CategorySet::all(), |_| true);
        assert_eq!(both.len(), 3);
    }

    #[test]
    fn quotient_contracts_and_counts() {
        let (a, b, c, d, e) = (1, 7, 4, 9, 5);
        let g = ToyGraph {
            nodes: [a, b, c, d, e].into_iter().map(n).collect(),
            edges: vec![
                toy_edge(a, b, CALLS.0),
                toy_edge(b, c, CALLS.0),
                toy_edge(c, d, CALLS.0),
                toy_edge(e, e, CALLS.0),
                toy_edge(b, c, SAME.0),
            ],
        };
        let classes = Classes::from_source(&g, &CategorySet::of(&[SAME]), |_| true);
        let q = Quotient::new(&g, &classes);
        let m = b.min(c);
        let strong = Confidence::Strong;
        assert_eq!(
            signature(&q),
            vec![(a, m, CALLS.0, strong), (m, d, CALLS.0, strong), (e, e, CALLS.0, strong)]
        );
        assert_eq!(q.collapsed_edges(), 2, "b->c CALLS and b->c SAME");
        assert_eq!(q.node_ids(), vec![n(a), n(m), n(d), n(e)]);
        assert!(!q.node_ids().contains(&n(b.max(c))));
        assert_eq!(q.rep(n(b)), n(m));
        assert_eq!(q.rep(n(a)), n(a));
        assert_eq!(q.inner().edges.len(), 5);
        assert!(q.edges().all(|e| e.cells.is_empty()));
    }

    #[test]
    fn quotient_walk_parity() {
        let (a, b, c, d) = (1, 7, 4, 9);
        let g = ToyGraph {
            nodes: [a, b, c, d].into_iter().map(n).collect(),
            edges: vec![toy_edge(a, b, CALLS.0), toy_edge(b, c, CALLS.0), toy_edge(c, d, CALLS.0), toy_edge(b, c, SAME.0)],
        };
        let depths = |src: &dyn GraphSource| -> Vec<(u64, usize)> {
            let adj = Adjacency::build(src, &CategorySet::of(&[CALLS]));
            bfs(&adj, &[n(a)], Walk::Forward, usize::MAX).reached.iter().map(|r| (r.id.0, r.depth)).collect()
        };
        assert_eq!(depths(&g), vec![(b, 1), (c, 2), (d, 3)]);
        let classes = Classes::from_pairs([(n(b), n(c))]);
        let q = Quotient::new(&g, &classes);
        assert_eq!(depths(&q), vec![(b.min(c), 1), (d, 2)]);
    }

    #[test]
    fn empty_classes_is_identity() {
        let mut r = Lcg(0x1D);
        let confidences = [Confidence::Strong, Confidence::Medium, Confidence::Weak];
        let nodes: Vec<NodeId> = (0..40).map(|i| n(1_000 + i * 7)).collect();
        let edges: Vec<Edge> = (0..150)
            .map(|_| {
                let from = nodes[(r.next() % 40) as usize];
                // Now and then an endpoint that is no node (dangling).
                let to = if r.next().is_multiple_of(13) { n(5) } else { nodes[(r.next() % 40) as usize] };
                let conf = confidences[(r.next() % 3) as usize];
                Edge::new(from, to, EdgeCategoryId(1 + (r.next() % 4) as u32), conf)
            })
            .collect();
        let g = ToyGraph { nodes, edges };
        let none = Classes::default();
        let q = Quotient::new(&g, &none);
        assert_eq!(q.node_ids(), g.node_ids());
        assert_eq!(signature(&q), signature(&g));
        assert_eq!(q.collapsed_edges(), 0);
    }

    #[test]
    fn quotient_nodes_keep_first_occurrence() {
        // A repeated inner id and two members of one class: each kept once,
        // at its first position.
        let g = ToyGraph { nodes: vec![n(8), n(3), n(8), n(6), n(2)], edges: Vec::new() };
        let classes = Classes::from_pairs([(n(6), n(3))]);
        let q = Quotient::new(&g, &classes);
        assert_eq!(q.node_ids(), vec![n(8), n(3), n(2)]);
    }
}
