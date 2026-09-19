//! Traversal primitives over `RepoGraph` and `MergedGraph`: neighbours, BFS,
//! reverse BFS, parent chains, and spreading activation.
//!
//! Both graphs are [`GraphSource`]s, so the domain-agnostic algorithms in
//! `repo_graph_activation::algo` run over them; the BFS walks here are thin
//! calls into `algo::reach` over a per-call CSR `Adjacency` (LD.15a).

use repo_graph_activation::algo::{Adjacency, CategorySet, GraphSource, Walk, reach};
use repo_graph_core::{Edge, EdgeCategoryId, NodeId, NodeKindId};

use crate::merged::MergedGraph;
use crate::types::RepoGraph;

// ============================================================================
// Traversal primitives
// ============================================================================

impl RepoGraph {
    /// Outgoing neighbours of `id`: `(target, category)` pairs.
    pub fn neighbours(&self, id: NodeId) -> Vec<(NodeId, EdgeCategoryId)> {
        self.edges
            .iter()
            .filter(|e| e.from == id)
            .map(|e| (e.to, e.category))
            .collect()
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

impl MergedGraph {
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
