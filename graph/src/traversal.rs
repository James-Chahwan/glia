//! Traversal primitives over `RepoGraph` and `MergedGraph`: neighbours, BFS,
//! reverse BFS, reachability, shortest paths, parent chains, and spreading
//! activation.
//!
//! Both graphs are [`GraphSource`]s, so the domain-agnostic algorithms in
//! `repo_graph_activation::algo` run over them; the BFS walks here are thin
//! calls into `algo::reach` over a per-call CSR `Adjacency` (LD.15a). The
//! `MergedGraph` walks (LD.3a) see every repo's edges AND `cross_edges`, so a
//! walk from a client function crosses HTTP_CALLS / QUEUE_FLOWS into the repo
//! that serves it; the `RepoGraph` walks see one repo x language graph only.

use std::collections::HashMap;

use repo_graph_activation::algo::reach::Reached;
use repo_graph_activation::algo::{Adjacency, CategorySet, GraphSource, Walk, reach};
use repo_graph_core::{Edge, EdgeCategoryId, NodeId, NodeKindId};

use crate::blast::Reach;
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

// ============================================================================
// Traversal primitives
// ============================================================================

impl RepoGraph {
    /// One-hop neighbours of `id` over this graph's edges, in edge order:
    /// `(other end, category, the way the edge was walked)`. `Forward` lists
    /// outgoing edges, `Backward` incoming ones, `Both` both (a self-loop
    /// once, as `Forward`). Cross-repo edges live on [`MergedGraph`]; use
    /// [`MergedGraph::neighbours`] to see them.
    pub fn neighbours(&self, id: NodeId, reach: Reach) -> Vec<(NodeId, EdgeCategoryId, Reach)> {
        neighbours_in(self.edges.iter(), id, reach, &CategorySet::all())
    }

    /// Node ids reachable from `start` following edges in `follow` up to
    /// `max_depth`. Start node excluded.
    pub fn bfs(
        &self,
        start: NodeId,
        follow: &[EdgeCategoryId],
        max_depth: usize,
    ) -> Vec<NodeId> {
        let adj = Adjacency::build(self, &CategorySet::of(follow));
        reach_ids(reach::bfs(&adj, &[start], Walk::Forward, max_depth))
    }

    /// Backward BFS — node ids that can reach `sink` by following edges in
    /// `follow` in the reverse direction, up to `max_depth` hops. Sink
    /// excluded.
    ///
    /// Joern-flavored data-flow primitive. Pair it with `reachable_by` to ask
    /// "which of these candidate sources can reach this sink?" without
    /// materialising the full predecessor frontier when only a small set is
    /// of interest.
    ///
    /// Cost is O(V + E) per call: it builds a CSR `Adjacency` over this
    /// graph's edges of the `follow` categories, then walks it. A caller
    /// issuing many walks over one graph builds the `Adjacency` once and
    /// calls `repo_graph_activation::algo::reach` directly.
    pub fn predecessors(
        &self,
        sink: NodeId,
        follow: &[EdgeCategoryId],
        max_depth: usize,
    ) -> Vec<NodeId> {
        let adj = Adjacency::build(self, &CategorySet::of(follow));
        reach_ids(reach::bfs(&adj, &[sink], Walk::Backward, max_depth))
    }

    /// Joern-style `reachableBy` — the subset of `sources` that can reach
    /// `sink` through reverse traversal along `follow` within `max_depth`
    /// hops. Returns nodes in stable iteration order over `sources` so
    /// callers can rely on the result for ranking, not just membership.
    ///
    /// When `sources.len() << total predecessors`, prefer this over
    /// `predecessors` + manual intersection — it short-circuits the BFS as
    /// soon as every candidate is hit.
    pub fn reachable_by(
        &self,
        sink: NodeId,
        sources: &[NodeId],
        follow: &[EdgeCategoryId],
        max_depth: usize,
    ) -> Vec<NodeId> {
        if sources.is_empty() {
            return Vec::new();
        }
        let adj = Adjacency::build(self, &CategorySet::of(follow));
        reach::reachable_by(&adj, sink, sources, max_depth)
    }

    /// Walk `parent_of` from `id` to the top. Excludes `id` itself.
    pub fn parent_chain(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = id;
        while let Some(parent) = self.nav.parent_of.get(&cur).copied() {
            out.push(parent);
            cur = parent;
        }
        out
    }

    /// Count nodes of a given kind.
    pub fn count_of_kind(&self, kind: NodeKindId) -> usize {
        self.nav
            .kind_by_id
            .values()
            .filter(|k| **k == kind)
            .count()
    }

    /// Spreading activation (PPR) over this repo's graph.
    pub fn activate(
        &self,
        seeds: &[NodeId],
        config: &repo_graph_activation::ActivationConfig,
    ) -> repo_graph_activation::ActivationResult {
        let node_ids: Vec<NodeId> = self.nodes.iter().map(|n| n.id).collect();
        repo_graph_activation::activate(&node_ids, &self.edges, seeds, config)
    }
}

/// A walk's reached ids, in discovery order.
fn reach_ids(walk: reach::Bfs) -> Vec<NodeId> {
    walk.reached.into_iter().map(|r| r.id).collect()
}

/// The walk direction a [`Reach`] names.
fn walk_of(reach: Reach) -> Walk {
    match reach {
        Reach::Forward => Walk::Forward,
        Reach::Backward => Walk::Backward,
        Reach::Both => Walk::Both,
    }
}

/// `None` follows every category, `Some(c)` exactly the listed ones.
fn follow_set(follow: Option<&[EdgeCategoryId]>) -> CategorySet {
    follow.map_or_else(CategorySet::all, CategorySet::of)
}

/// One linear scan of `edges` for those incident to `id` whose category
/// `keep` holds, in edge order. An edge leaving `id` is taken forward when
/// `reach` walks forward; otherwise an edge entering it is taken backward
/// when `reach` walks backward — so under `Both` a self-loop counts once, as
/// forward (the rule `Walk::Both` follows).
fn neighbours_in<'a>(
    edges: impl Iterator<Item = &'a Edge>,
    id: NodeId,
    reach: Reach,
    keep: &CategorySet,
) -> Vec<(NodeId, EdgeCategoryId, Reach)> {
    let fwd = reach != Reach::Backward;
    let bwd = reach != Reach::Forward;
    edges
        .filter(|e| keep.contains(e.category))
        .filter_map(|e| {
            if fwd && e.from == id {
                Some((e.to, e.category, Reach::Forward))
            } else if bwd && e.to == id {
                Some((e.from, e.category, Reach::Backward))
            } else {
                None
            }
        })
        .collect()
}

/// The `[traverse]` fired_on line: one per merged-graph walk (LD.3a).
fn traverse_marker(op: &str, walk: Walk, seeds: usize, reached: usize, index_nodes: usize) {
    eprintln!("[traverse] op={op} walk={walk:?} seeds={seeds} reached={reached} index_nodes={index_nodes}");
}

/// Nodes in `nodes` order, edges in `edges` order.
impl GraphSource for RepoGraph {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.iter().map(|n| n.id).collect()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

/// Each graph's nodes in graph order (the order [`MergedGraph::activate`]
/// hands PPR), edges in [`MergedGraph::all_edges`] order.
impl GraphSource for MergedGraph {
    fn node_ids(&self) -> Vec<NodeId> {
        self.graphs
            .iter()
            .flat_map(|g| g.nodes.iter().map(|n| n.id))
            .collect()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.all_edges())
    }
}

/// Walks over every repo's edges and `cross_edges` (LD.3a). `follow = None`
/// follows every category, `Some(c)` exactly the listed ones. An edge to an id
/// that is no node is still followed, and that id reached (LD.15a parity rule
/// 1). Order is global edge order: the graphs in `Vec` order, then
/// `cross_edges` ([`MergedGraph::all_edges`]).
///
/// `bfs`, `predecessors`, `reachable_by` and `shortest_path` build one
/// `Adjacency` per call, O(V + E), and print one
/// `[traverse] op=<op> walk=<Forward|Backward|Both> seeds=<n> reached=<n> index_nodes=<n>`
/// line to stderr (`reached` is the walk's reached count; for `reachable_by`
/// the sources hit). A caller issuing many walks over one filter builds the
/// `Adjacency` once and calls `repo_graph_activation::algo::reach` directly.
/// `neighbours` is one edge scan with no index and no line: it is called per
/// node.
impl MergedGraph {
    /// One-hop neighbours of `id` over intra-repo and cross edges, in global
    /// edge order: `(other end, category, the way the edge was walked)`.
    /// `Forward` lists outgoing edges, `Backward` incoming ones, `Both` both
    /// (a self-loop once, as `Forward`).
    pub fn neighbours(
        &self,
        id: NodeId,
        reach: Reach,
        follow: Option<&[EdgeCategoryId]>,
    ) -> Vec<(NodeId, EdgeCategoryId, Reach)> {
        neighbours_in(self.all_edges(), id, reach, &follow_set(follow))
    }

    /// Breadth-first walk from `seeds` along `reach`, up to `max_depth` hops:
    /// every reached node in discovery order with its depth, the category of
    /// the edge that first reached it and the node it was reached from. The
    /// seeds are not in it; `max_depth = 0` reaches nothing.
    pub fn bfs(
        &self,
        seeds: &[NodeId],
        reach: Reach,
        follow: Option<&[EdgeCategoryId]>,
        max_depth: usize,
    ) -> Vec<Reached> {
        let walk = walk_of(reach);
        let adj = Adjacency::build(self, &follow_set(follow));
        let reached = reach::bfs(&adj, seeds, walk, max_depth).reached;
        traverse_marker("bfs", walk, seeds.len(), reached.len(), adj.len());
        reached
    }

    /// Node ids that reach `sink` within `max_depth` hops, in backward
    /// discovery order. Sink excluded.
    pub fn predecessors(
        &self,
        sink: NodeId,
        follow: Option<&[EdgeCategoryId]>,
        max_depth: usize,
    ) -> Vec<NodeId> {
        let adj = Adjacency::build(self, &follow_set(follow));
        let ids = reach_ids(reach::bfs(&adj, &[sink], Walk::Backward, max_depth));
        traverse_marker("predecessors", Walk::Backward, 1, ids.len(), adj.len());
        ids
    }

    /// The `sources` that reach `sink` within `max_depth` hops, in `sources`
    /// order. The sink itself is never a hit.
    pub fn reachable_by(
        &self,
        sink: NodeId,
        sources: &[NodeId],
        follow: Option<&[EdgeCategoryId]>,
        max_depth: usize,
    ) -> Vec<NodeId> {
        if sources.is_empty() {
            traverse_marker("reachable_by", Walk::Backward, 1, 0, 0);
            return Vec::new();
        }
        let adj = Adjacency::build(self, &follow_set(follow));
        let hits = reach::reachable_by(&adj, sink, sources, max_depth);
        traverse_marker("reachable_by", Walk::Backward, 1, hits.len(), adj.len());
        hits
    }

    /// A shortest path by hop count from `from` to `to` along `reach`, or
    /// `None` when `to` is not reached within `max_depth` hops. Each step is
    /// `(node, category of the edge that entered it)`, `None` for `from`;
    /// `from == to` is the zero-hop path `[(from, None)]`.
    ///
    /// The path follows the BFS parents from `to` back to `from`. A parent is
    /// recorded at first discovery, so the path is a shortest one, and among
    /// equally short paths it is the one global edge order meets first.
    pub fn shortest_path(
        &self,
        from: NodeId,
        to: NodeId,
        reach: Reach,
        follow: Option<&[EdgeCategoryId]>,
        max_depth: usize,
    ) -> Option<Vec<(NodeId, Option<EdgeCategoryId>)>> {
        let walk = walk_of(reach);
        if from == to {
            traverse_marker("shortest_path", walk, 1, 0, 0);
            return Some(vec![(from, None)]);
        }
        let adj = Adjacency::build(self, &follow_set(follow));
        let reached = reach::bfs(&adj, &[from], walk, max_depth).reached;
        traverse_marker("shortest_path", walk, 1, reached.len(), adj.len());
        let entered: HashMap<NodeId, (EdgeCategoryId, NodeId)> =
            reached.iter().map(|r| (r.id, (r.via, r.parent))).collect();
        // Every reached node's parent is `from` (the only seed, never itself
        // reached) or a node discovered before it, so the chain ends at
        // `from` in at most `reached.len()` steps.
        let mut path = Vec::new();
        let mut cur = to;
        while cur != from {
            let &(via, parent) = entered.get(&cur)?;
            path.push((cur, Some(via)));
            cur = parent;
        }
        path.push((from, None));
        path.reverse();
        Some(path)
    }

    /// Spreading activation over the full merged graph (all repos + cross edges).
    pub fn activate(
        &self,
        seeds: &[NodeId],
        config: &repo_graph_activation::ActivationConfig,
    ) -> repo_graph_activation::ActivationResult {
        let node_ids: Vec<NodeId> = self
            .graphs
            .iter()
            .flat_map(|g| g.nodes.iter().map(|n| n.id))
            .collect();
        let edges: Vec<Edge> = self.all_edges().cloned().collect();
        repo_graph_activation::activate(&node_ids, &edges, seeds, config)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::test_support::{flow_graph, repo};
    use repo_graph_code_domain::{GRAPH_TYPE, edge_category, node_kind};

    #[test]
    fn predecessors_walks_backward_along_chosen_categories() {
        let g = flow_graph();
        let c = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::c");
        let a = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::a");
        let b = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::b");
        let d = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::d");

        let preds: HashSet<NodeId> = g
            .predecessors(c, &[edge_category::CALLS], 5)
            .into_iter()
            .collect();
        assert_eq!(preds, HashSet::from([a, b, d]));

        // Depth-bounded — depth=1 only finds direct predecessors of c.
        let direct: HashSet<NodeId> = g
            .predecessors(c, &[edge_category::CALLS], 1)
            .into_iter()
            .collect();
        assert_eq!(direct, HashSet::from([b, d]));

        // Wrong category yields nothing — proves filter is enforced.
        let none = g.predecessors(c, &[edge_category::IMPORTS], 5);
        assert!(none.is_empty());
    }

    #[test]
    fn reachable_by_intersects_predecessors_with_sources() {
        let g = flow_graph();
        let c = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::c");
        let a = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::a");
        let d = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::d");
        let absent =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::ghost");

        // a (transitive via b) and d (direct) both reach c. ghost is not in graph.
        let hit = g.reachable_by(c, &[a, d, absent], &[edge_category::CALLS], 5);
        assert_eq!(hit, vec![a, d]);

        // Source order is preserved (d listed first → d listed first).
        let hit_swapped = g.reachable_by(c, &[d, a], &[edge_category::CALLS], 5);
        assert_eq!(hit_swapped, vec![d, a]);

        // Empty sources short-circuits.
        assert!(g.reachable_by(c, &[], &[edge_category::CALLS], 5).is_empty());
    }

    #[test]
    fn graph_sources_keep_node_and_edge_order_and_dangling_targets() {
        let mut g = flow_graph();
        let a = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::a");
        let c = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::c");
        // An edge to an id that is no node is still walked.
        let ghost = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "m::Ghost");
        let to_ghost = Edge {
            from: c,
            to: ghost,
            category: edge_category::CALLS,
            confidence: repo_graph_core::Confidence::Strong,
            cells: Vec::new(),
        };
        g.edges.push(to_ghost.clone());
        assert_eq!(g.bfs(a, &[edge_category::CALLS], 5).last(), Some(&ghost));

        let nodes: Vec<NodeId> = g.nodes.iter().map(|n| n.id).collect();
        assert_eq!(GraphSource::node_ids(&g), nodes);
        let intra = g.edges.len();
        let mut merged = MergedGraph::new(vec![g, flow_graph()]);
        let cross = Edge { from: ghost, to: a, ..to_ghost };
        merged.cross_edges.push(cross.clone());
        let merged_nodes = GraphSource::node_ids(&merged);
        assert_eq!(merged_nodes.len(), 8);
        assert_eq!(merged_nodes[..4], nodes[..]);
        let edges: Vec<&Edge> = GraphSource::edges(&merged).collect();
        assert_eq!(edges.len(), intra + 3 + 1);
        assert_eq!(edges.last().map(|e| (e.from, e.to)), Some((cross.from, cross.to)));
    }
}
