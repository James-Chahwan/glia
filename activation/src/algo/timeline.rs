//! Timeline (CD.5a): edge and node validity intervals `[valid_from,
//! invalid_at)`, folded over N consecutive snapshots and their move maps
//! through [`delta`](crate::algo::delta) and keyed by each item's newest
//! identity.
//!
//! Domain-free like the rest of [`crate::algo`]: a snapshot is a caller-built
//! [`DeltaSide`] and a move map is raw `(before, after)` id pairs, never a
//! named node kind or edge category.
//!
//! Snapshots are numbered by revision, `0` for the one given to
//! [`TimelineBuilder::new`] and one more for each [`TimelineBuilder::push`].
//! An item's span is `[from_rev, until_rev)`: present from `from_rev` up to,
//! not including, `until_rev`, which is `None` when the item is still present
//! in the last snapshot.
//!
//! The rules the fold keeps:
//! - An item keeps ONE span across moves. Every push remaps each open span's
//!   key through the moves [`graph_delta`] APPLIED (its `moved_nodes`), never
//!   through the raw move list: a pair the delta rejects would fold two ids
//!   into one. So an edge that survives a file rename is one span keyed by its
//!   newest endpoints, and a node's span records each id it had before in
//!   `prior_ids`.
//! - A span that closes keeps the key the item had when it was last seen: an
//!   edge removed at the same revision its endpoint moved closes under its old
//!   endpoints, which is how the delta keys a removed edge.
//! - An item that comes back after a gap opens a second span; spans are never
//!   bridged.
//! - An edge's identity is `(from, to, category)` ([`EdgeKey`]): a confidence
//!   change is not a new span, the LE.1a rule.
//!
//! The builder holds only the open keys, so its memory is O(V + E) of the
//! widest snapshot pair, not O(N x E). Every map is a `BTreeMap` keyed on raw
//! integers and the output is sorted under a total order, so a timeline is a
//! pure function of its inputs. A snapshot the caller could not build is
//! simply not pushed (the caller records the skip).

use std::cmp::Ordering;
use std::collections::BTreeMap;

use glia_core::NodeId;

use super::delta::{DeltaOptions, DeltaSide, EdgeKey, graph_delta};

/// One validity interval of one edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeSpan {
    /// The edge as last seen: its endpoints remapped through every move up to
    /// `until_rev` (or up to the last snapshot).
    pub key: EdgeKey,
    /// The first revision the edge is present at.
    pub from_rev: u32,
    /// The first revision it is absent from; `None` = present at the last
    /// snapshot.
    pub until_rev: Option<u32>,
}

/// One validity interval of one node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeSpan {
    /// The node's id as last seen.
    pub id: NodeId,
    /// The first revision the node is present at.
    pub from_rev: u32,
    /// The first revision it is absent from; `None` = present at the last
    /// snapshot.
    pub until_rev: Option<u32>,
    /// `(rev, id before)`: each move the span went through, oldest first. At
    /// revision `rev` the node took the next id (or [`Self::id`]).
    pub prior_ids: Vec<(u32, NodeId)>,
}

/// Every span of a run of snapshots. `edges` is sorted by `(from_rev, key,
/// until_rev)` and `nodes` by `(from_rev, id, until_rev)`, raw ids, an open
/// span (`None`) after a closed one.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timeline {
    /// Snapshots folded: the first one plus one per push.
    pub revs: u32,
    pub edges: Vec<EdgeSpan>,
    pub nodes: Vec<NodeSpan>,
    /// Move pairs the deltas applied, summed over every push.
    pub moves_applied: usize,
    /// Move pairs the deltas rejected (`GraphDelta::ignored_moves`), summed:
    /// such a node's span closes and a new one opens under its new id.
    pub moves_ignored: usize,
}

/// Folds consecutive snapshots into a [`Timeline`], two at a time.
#[derive(Clone, Debug)]
pub struct TimelineBuilder {
    /// Open edge spans, keyed as the edge sits in the last snapshot -> from.
    open_edges: BTreeMap<EdgeKey, u32>,
    /// Open node spans, by raw id in the last snapshot -> (from, prior ids).
    open_nodes: BTreeMap<u64, (u32, Vec<(u32, NodeId)>)>,
    closed_edges: Vec<EdgeSpan>,
    closed_nodes: Vec<NodeSpan>,
    /// The last snapshot's revision.
    rev: u32,
    moves_applied: usize,
    moves_ignored: usize,
}

impl TimelineBuilder {
    /// Open a span at revision 0 for every node and edge of `first`.
    pub fn new(first: &DeltaSide<'_>) -> Self {
        // DeltaSide's lists are private to algo::delta: a delta from an empty
        // side reports every item of `first` as added, deduplicated.
        let d = graph_delta(&DeltaSide::default(), first, &DeltaOptions::default());
        Self {
            open_edges: d.added_edges.into_iter().map(|k| (k, 0)).collect(),
            open_nodes: d.added_nodes.into_iter().map(|id| (id.0, (0, Vec::new()))).collect(),
            closed_edges: Vec::new(),
            closed_nodes: Vec::new(),
            rev: 0,
            moves_applied: 0,
            moves_ignored: 0,
        }
    }

    /// Fold the next snapshot. `prev` must be the snapshot folded last (the
    /// `first` given to [`Self::new`], or the previous push's `next`); `moves`
    /// are `(id in prev, id in next)` pairs, filtered by the rules on
    /// [`DeltaOptions::moves`].
    pub fn push(&mut self, prev: &DeltaSide<'_>, next: &DeltaSide<'_>, moves: &[(NodeId, NodeId)]) {
        self.rev = self.rev.saturating_add(1);
        let rev = self.rev;
        let d = graph_delta(prev, next, &DeltaOptions { content_cells: &[], moves });
        self.moves_applied += d.moved_nodes.len();
        self.moves_ignored += d.ignored_moves;

        // Close under prev-side keys, which is how the delta keys a removal
        // and how the open spans are held.
        for key in d.removed_edges {
            if let Some(from_rev) = self.open_edges.remove(&key) {
                self.closed_edges.push(EdgeSpan { key, from_rev, until_rev: Some(rev) });
            }
        }
        for id in d.removed_nodes {
            if let Some((from_rev, prior_ids)) = self.open_nodes.remove(&id.0) {
                self.closed_nodes.push(NodeSpan { id, from_rev, until_rev: Some(rev), prior_ids });
            }
        }

        // Carry what stays open onto next-side ids. A fresh map, not an
        // in-place rename, so a swap or a chain cannot overwrite a span.
        if !d.moved_nodes.is_empty() {
            let moved: BTreeMap<u64, u64> = d.moved_nodes.iter().map(|&(b, a)| (b.0, a.0)).collect();
            let remap = |id: NodeId| NodeId(moved.get(&id.0).copied().unwrap_or(id.0));
            self.open_edges = std::mem::take(&mut self.open_edges)
                .into_iter()
                .map(|(k, from_rev)| (EdgeKey { from: remap(k.from), to: remap(k.to), category: k.category }, from_rev))
                .collect();
            self.open_nodes = std::mem::take(&mut self.open_nodes)
                .into_iter()
                .map(|(id, (from_rev, mut prior_ids))| match moved.get(&id) {
                    Some(&to) => {
                        prior_ids.push((rev, NodeId(id)));
                        (to, (from_rev, prior_ids))
                    }
                    None => (id, (from_rev, prior_ids)),
                })
                .collect();
        }

        // `or_insert`: a key already open means `prev` was not the last
        // snapshot folded; the older span is kept rather than restarted.
        for key in d.added_edges {
            self.open_edges.entry(key).or_insert(rev);
        }
        for id in d.added_nodes {
            self.open_nodes.entry(id.0).or_insert((rev, Vec::new()));
        }
    }

    /// Every span: the closed ones plus the ones still open at the last
    /// snapshot (`until_rev: None`), sorted (see [`Timeline`]).
    pub fn finish(self) -> Timeline {
        let mut edges = self.closed_edges;
        edges.extend(self.open_edges.into_iter().map(|(key, from_rev)| EdgeSpan { key, from_rev, until_rev: None }));
        edges.sort_unstable_by(|a, b| {
            (a.from_rev, a.key).cmp(&(b.from_rev, b.key)).then_with(|| until_order(a.until_rev, b.until_rev))
        });
        let mut nodes = self.closed_nodes;
        nodes.extend(self.open_nodes.into_iter().map(|(id, (from_rev, prior_ids))| NodeSpan {
            id: NodeId(id),
            from_rev,
            until_rev: None,
            prior_ids,
        }));
        nodes.sort_unstable_by(|a, b| {
            (a.from_rev, a.id.0).cmp(&(b.from_rev, b.id.0)).then_with(|| until_order(a.until_rev, b.until_rev))
        });
        let t = Timeline {
            revs: self.rev.saturating_add(1),
            edges,
            nodes,
            moves_applied: self.moves_applied,
            moves_ignored: self.moves_ignored,
        };
        if super::algo_debug() {
            let closed_e = t.edges.iter().filter(|s| s.until_rev.is_some()).count();
            let closed_n = t.nodes.iter().filter(|s| s.until_rev.is_some()).count();
            eprintln!(
                "[algo] timeline revs={} edge_spans={} (closed {closed_e}) node_spans={} (closed {closed_n}) moves_applied={} moves_ignored={}",
                t.revs,
                t.edges.len(),
                t.nodes.len(),
                t.moves_applied,
                t.moves_ignored,
            );
        }
        t
    }
}

/// A closed span (by its end) before an open one.
fn until_order(a: Option<u32>, b: Option<u32>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algo::{ToyGraph, toy_edge};
    use glia_core::{Confidence, Edge, EdgeCategoryId, Node, RepoId};

    const CAT: u32 = 1;

    /// One snapshot: a [`ToyGraph`] plus the core nodes a [`DeltaSide`]
    /// borrows.
    struct Snap {
        g: ToyGraph,
        nodes: Vec<Node>,
    }

    impl Snap {
        fn side(&self) -> DeltaSide<'_> {
            DeltaSide::new(&self.nodes, &self.g.edges)
        }
    }

    fn snap(nodes: &[u64], edges: &[(u64, u64)]) -> Snap {
        snap_with(nodes, edges.iter().map(|&(f, t)| toy_edge(f, t, CAT)).collect())
    }

    fn snap_with(nodes: &[u64], edges: Vec<Edge>) -> Snap {
        let ids: Vec<NodeId> = nodes.iter().map(|&n| NodeId(n)).collect();
        let nodes = ids
            .iter()
            .map(|&id| Node { id, repo: RepoId(1), confidence: Confidence::Strong, cells: Vec::new() })
            .collect();
        Snap { g: ToyGraph { nodes: ids, edges }, nodes }
    }

    /// Fold `snaps` in order; `moves[i]` is applied on the push into
    /// `snaps[i + 1]`.
    fn fold(snaps: &[Snap], moves: &[&[(u64, u64)]]) -> Timeline {
        let mut b = TimelineBuilder::new(&snaps[0].side());
        for (i, pair) in snaps.windows(2).enumerate() {
            let m: Vec<(NodeId, NodeId)> =
                moves.get(i).copied().unwrap_or(&[]).iter().map(|&(x, y)| (NodeId(x), NodeId(y))).collect();
            b.push(&pair[0].side(), &pair[1].side(), &m);
        }
        b.finish()
    }

    fn key(from: u64, to: u64) -> EdgeKey {
        EdgeKey { from: NodeId(from), to: NodeId(to), category: EdgeCategoryId(CAT) }
    }

    fn espan(from: u64, to: u64, from_rev: u32, until_rev: Option<u32>) -> EdgeSpan {
        EdgeSpan { key: key(from, to), from_rev, until_rev }
    }

    fn nspan(id: u64, from_rev: u32, until_rev: Option<u32>, prior: &[(u32, u64)]) -> NodeSpan {
        NodeSpan { id: NodeId(id), from_rev, until_rev, prior_ids: prior.iter().map(|&(r, n)| (r, NodeId(n))).collect() }
    }

    fn spans_of(t: &Timeline, k: EdgeKey) -> Vec<(u32, Option<u32>)> {
        t.edges.iter().filter(|s| s.key == k).map(|s| (s.from_rev, s.until_rev)).collect()
    }

    #[test]
    fn add_remove_readd() {
        let snaps = [
            snap(&[1, 2], &[(1, 2)]),
            snap(&[1, 2], &[(1, 2)]),
            snap(&[1, 2], &[]),
            snap(&[1, 2], &[(1, 2)]),
        ];
        let t = fold(&snaps, &[]);
        assert_eq!(t.revs, 4);
        assert_eq!(t.edges, vec![espan(1, 2, 0, Some(2)), espan(1, 2, 3, None)]);
        // The nodes never left: one span each.
        assert_eq!(t.nodes, vec![nspan(1, 0, None, &[]), nspan(2, 0, None, &[])]);
        assert_eq!((t.moves_applied, t.moves_ignored), (0, 0));
    }

    #[test]
    fn move_keeps_one_interval() {
        let snaps = [snap(&[5, 7], &[(5, 7)]), snap(&[5, 7], &[(5, 7)]), snap(&[7, 9], &[(9, 7)])];
        let t = fold(&snaps, &[&[], &[(5, 9)]]);
        assert_eq!(t.edges, vec![espan(9, 7, 0, None)]);
        assert_eq!(t.nodes, vec![nspan(7, 0, None, &[]), nspan(9, 0, None, &[(2, 5)])]);
        assert_eq!(t.moves_applied, 1);
        // Without the move map the same snapshots are a removal plus an addition.
        let t = fold(&snaps, &[]);
        assert_eq!(t.edges, vec![espan(5, 7, 0, Some(2)), espan(9, 7, 2, None)]);
        assert_eq!(t.nodes, vec![nspan(5, 0, Some(2), &[]), nspan(7, 0, None, &[]), nspan(9, 2, None, &[])]);
    }

    #[test]
    fn chained_moves() {
        let snaps = [snap(&[5, 7], &[(5, 7), (7, 5)]), snap(&[7, 9], &[(9, 7), (7, 9)]), snap(&[7, 11], &[(11, 7), (7, 11)])];
        let t = fold(&snaps, &[&[(5, 9)], &[(9, 11)]]);
        assert_eq!(t.edges, vec![espan(7, 11, 0, None), espan(11, 7, 0, None)]);
        assert_eq!(t.nodes, vec![nspan(7, 0, None, &[]), nspan(11, 0, None, &[(1, 5), (2, 9)])]);
        assert_eq!(t.moves_applied, 2);
    }

    #[test]
    fn removed_before_move_closes_under_old_key() {
        // At rev 1 node 5 moves to 9: its edge to 7 is dropped in the same
        // change, its edge to 8 survives.
        let snaps = [snap(&[5, 7, 8], &[(5, 7), (5, 8)]), snap(&[7, 8, 9], &[(9, 8)]), snap(&[7, 8], &[])];
        let t = fold(&snaps, &[&[(5, 9)]]);
        assert_eq!(t.edges, vec![espan(5, 7, 0, Some(1)), espan(9, 8, 0, Some(2))]);
        // The node's span closes under the id it had when last seen.
        assert_eq!(t.nodes, vec![nspan(7, 0, None, &[]), nspan(8, 0, None, &[]), nspan(9, 0, Some(2), &[(1, 5)])]);
    }

    #[test]
    fn deterministic_output_order() {
        // Same snapshots, different list orders, repeated items and mixed
        // confidences: equal timelines.
        let a = [snap(&[5, 7, 9], &[(5, 7), (9, 7)]), snap(&[7, 9], &[(9, 7)]), snap(&[5, 7], &[(5, 7)])];
        let weak = |f, t| Edge::new(NodeId(f), NodeId(t), EdgeCategoryId(CAT), Confidence::Weak);
        let b = [
            snap_with(&[9, 7, 5, 7], vec![toy_edge(9, 7, CAT), weak(5, 7), toy_edge(5, 7, CAT)]),
            snap_with(&[9, 7], vec![weak(9, 7), weak(9, 7)]),
            snap_with(&[7, 5], vec![toy_edge(5, 7, CAT)]),
        ];
        // 5 leaves at rev 1; at rev 2, 9 takes id 5.
        let moves: &[&[(u64, u64)]] = &[&[], &[(9, 5)]];
        let (ta, tb) = (fold(&a, moves), fold(&b, moves));
        assert_eq!(ta, tb);
        // Two spans share (from_rev, key): the closed one sorts first.
        assert_eq!(ta.edges, vec![espan(5, 7, 0, Some(1)), espan(5, 7, 0, None)]);
        assert_eq!(ta.nodes, vec![nspan(5, 0, Some(1), &[]), nspan(5, 0, None, &[(2, 9)]), nspan(7, 0, None, &[])]);
        // Sorted by from_rev before key.
        let c = [snap(&[1, 2, 3, 9], &[(3, 9)]), snap(&[1, 2, 3, 9], &[(3, 9), (1, 2), (9, 1)])];
        let t = fold(&c, &[]);
        assert_eq!(t.edges, vec![espan(3, 9, 0, None), espan(1, 2, 1, None), espan(9, 1, 1, None)]);
    }

    #[test]
    fn empty_snapshots() {
        let t = fold(&[snap(&[], &[])], &[]);
        assert_eq!(t, Timeline { revs: 1, ..Timeline::default() });
        let snaps = [snap(&[], &[]), snap(&[1, 2], &[(1, 2)]), snap(&[1, 2], &[(1, 2)])];
        let t = fold(&snaps, &[]);
        assert_eq!(t.revs, 3);
        assert_eq!(t.edges, vec![espan(1, 2, 1, None)]);
        assert_eq!(t.nodes, vec![nspan(1, 1, None, &[]), nspan(2, 1, None, &[])]);
        // Everything leaving closes every span.
        let t = fold(&[snap(&[1, 2], &[(1, 2)]), snap(&[], &[])], &[]);
        assert_eq!(t.edges, vec![espan(1, 2, 0, Some(1))]);
        assert_eq!(t.nodes, vec![nspan(1, 0, Some(1), &[]), nspan(2, 0, Some(1), &[])]);
    }

    #[test]
    fn swap_keeps_both_spans() {
        let snaps = [snap(&[5, 9], &[(5, 9)]), snap(&[5, 9], &[(9, 5)])];
        let t = fold(&snaps, &[&[(5, 9), (9, 5)]]);
        assert_eq!(t.edges, vec![espan(9, 5, 0, None)]);
        assert_eq!(t.nodes, vec![nspan(5, 0, None, &[(1, 9)]), nspan(9, 0, None, &[(1, 5)])]);
        assert_eq!(t.moves_applied, 2);
    }

    #[test]
    fn rejected_move_is_not_applied() {
        // 99 is no node of the next snapshot, so the delta rejects the pair;
        // the raw list is never used to remap.
        let snaps = [snap(&[5, 7], &[(5, 7)]), snap(&[7, 9], &[(9, 7)])];
        let t = fold(&snaps, &[&[(5, 99)]]);
        assert_eq!((t.moves_applied, t.moves_ignored), (0, 1));
        assert_eq!(t.edges, vec![espan(5, 7, 0, Some(1)), espan(9, 7, 1, None)]);
        assert_eq!(t.nodes, vec![nspan(5, 0, Some(1), &[]), nspan(7, 0, None, &[]), nspan(9, 1, None, &[])]);
    }

    #[test]
    fn reconfidenced_edge_is_one_span() {
        let snaps = [
            snap_with(&[1, 2], vec![toy_edge(1, 2, CAT)]),
            snap_with(&[1, 2], vec![Edge::new(NodeId(1), NodeId(2), EdgeCategoryId(CAT), Confidence::Weak)]),
        ];
        assert_eq!(fold(&snaps, &[]).edges, vec![espan(1, 2, 0, None)]);
        // A different category is a different edge.
        let snaps = [snap(&[1, 2], &[(1, 2)]), snap_with(&[1, 2], vec![toy_edge(1, 2, CAT + 1)])];
        let t = fold(&snaps, &[]);
        assert_eq!(spans_of(&t, key(1, 2)), vec![(0, Some(1))]);
        assert_eq!(t.edges.len(), 2);
    }
}
