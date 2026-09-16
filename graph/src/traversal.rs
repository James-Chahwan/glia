//! Traversal primitives over `RepoGraph` and `MergedGraph`: neighbours, BFS,
//! reverse BFS, parent chains, and spreading activation.

use std::collections::{HashSet, VecDeque};

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
        let allow: HashSet<EdgeCategoryId> = follow.iter().copied().collect();
        let mut visited: HashSet<NodeId> = HashSet::from([start]);
        let mut out = Vec::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(start, 0)]);
        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for e in self.edges.iter().filter(|e| e.from == node) {
                if !allow.contains(&e.category) {
                    continue;
                }
                if visited.insert(e.to) {
                    out.push(e.to);
                    queue.push_back((e.to, depth + 1));
                }
            }
        }
        out
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
    /// Cost is O(E * depth) per call (linear scan over edges per visited
    /// node). For repeated queries against the same graph, build a reverse
    /// adjacency index out-of-band; this primitive is intentionally
    /// index-free so it composes with `MergedGraph::all_edges`.
    pub fn predecessors(
        &self,
        sink: NodeId,
        follow: &[EdgeCategoryId],
        max_depth: usize,
    ) -> Vec<NodeId> {
        let allow: HashSet<EdgeCategoryId> = follow.iter().copied().collect();
        let mut visited: HashSet<NodeId> = HashSet::from([sink]);
        let mut out = Vec::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(sink, 0)]);
        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for e in self.edges.iter().filter(|e| e.to == node) {
                if !allow.contains(&e.category) {
                    continue;
                }
                if visited.insert(e.from) {
                    out.push(e.from);
                    queue.push_back((e.from, depth + 1));
                }
            }
        }
        out
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
        let allow: HashSet<EdgeCategoryId> = follow.iter().copied().collect();
        let target_set: HashSet<NodeId> = sources.iter().copied().collect();
        let mut visited: HashSet<NodeId> = HashSet::from([sink]);
        let mut hit: HashSet<NodeId> = HashSet::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(sink, 0)]);
        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth || hit.len() == target_set.len() {
                if hit.len() == target_set.len() {
                    break;
                }
                continue;
            }
            for e in self.edges.iter().filter(|e| e.to == node) {
                if !allow.contains(&e.category) {
                    continue;
                }
                if visited.insert(e.from) {
                    if target_set.contains(&e.from) {
                        hit.insert(e.from);
                    }
                    queue.push_back((e.from, depth + 1));
                }
            }
        }
        sources.iter().copied().filter(|s| hit.contains(s)).collect()
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
}
