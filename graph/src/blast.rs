//! `blast_radius` — the P3 answer-shaped primitive (handoff v6).

use std::sync::OnceLock;

use repo_graph_activation::algo::{self, Adjacency, Walk};
use repo_graph_activation::profile::DomainTables;
use repo_graph_core::{EdgeCategoryId, NodeId};

use crate::merged::MergedGraph;

// ============================================================================
// blast_radius — the P3 answer-shaped primitive (handoff v6)
// ============================================================================

/// Which way the blast radius spreads from the seed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Reach {
    /// Downstream — what this node affects (follow edges from→to).
    Forward,
    /// Upstream — what affects this node (follow edges to→from).
    Backward,
    /// Both directions (neighbourhood).
    Both,
}

/// One node in a blast radius: reached via `reason` at `depth`, ranked by `score`.
/// Produced by `MergedGraph::blast_radius`, never built by a struct literal
/// outside this crate (LD.9):
///
/// ```compile_fail
/// let _ = repo_graph_graph::BlastHit {
///     id: repo_graph_core::NodeId(0),
///     depth: 0,
///     reason: repo_graph_core::EdgeCategoryId(0),
///     score: 0.0,
/// };
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BlastHit {
    pub id: NodeId,
    /// Hops from the seed along carry edges.
    pub depth: usize,
    /// The edge category that FIRST reached this node — the "why it's in scope".
    pub reason: EdgeCategoryId,
    /// PPR score seeded at the target (the ranking signal).
    pub score: f64,
}

impl MergedGraph {
    /// The complete, deduped, edge-category-aware, PPR-ranked closure around
    /// `seed` — the answer `impact`+`activate` composed to, in one pass. Each
    /// hit carries the edge category that first reached it (`reason`) and its
    /// PPR score; the caller adds location/kind. Result is sorted by score
    /// (desc), then node id (asc) for determinism.
    ///
    /// The domain's `tables` say what to follow and how to rank (LD.14b): the
    /// walk follows `tables.carry_edges` only — semantic dependencies, never
    /// the structural `DEFINES` / `CONTAINS` / `IMPORTS` / `HAS_ATTRIBUTE`
    /// edges that pull in unrelated code through shared containers and
    /// imports (handoff v6 P1, bullet 4: `impact` fanning out through
    /// `imports`) — and the ranking is `tables.activation_config(None)`. The
    /// code domain passes `repo_graph_code_domain::profile::CODE_TABLES`; a
    /// caller that needs another carry set passes its own tables.
    pub fn blast_radius(
        &self,
        seed: NodeId,
        reach: Reach,
        max_depth: usize,
        tables: &DomainTables,
    ) -> Vec<BlastHit> {
        use repo_graph_activation::Direction;
        use std::collections::HashMap;

        // First-reach (depth, reason) per node: a BFS over the carry edges'
        // CSR index (LD.15b), so depth is the shortest carry-path and `reason`
        // the category of the edge that first put the node in scope - exactly
        // what the per-node edge scan it replaced returned.
        let walk = match reach {
            Reach::Forward => Walk::Forward,
            Reach::Backward => Walk::Backward,
            Reach::Both => Walk::Both,
        };
        let adj = Adjacency::carry(self, tables);
        let b = algo::reach::bfs(&adj, &[seed], walk, max_depth);
        if reach_debug() {
            // fired_on marker (LD.15b), a cost diagnostic: `scanned` is the
            // incidences the walk examined, at most two per kept edge.
            eprintln!(
                "[reach] blast walk={walk:?} depth<={max_depth} reached={} scanned={} index_nodes={} kept={}",
                b.reached.len(),
                b.scanned,
                adj.len(),
                adj.kept_edges()
            );
        }
        if b.reached.is_empty() {
            return Vec::new();
        }
        let first: HashMap<NodeId, (usize, EdgeCategoryId)> =
            b.reached.iter().map(|r| (r.id, (r.depth, r.via))).collect();

        // Rank the closure by PPR seeded at the target.
        let mut config = tables.activation_config(None);
        config.direction = match reach {
            Reach::Forward => Direction::Forward,
            Reach::Backward => Direction::Backward,
            Reach::Both => Direction::Undirected,
        };
        config.top_k = usize::MAX; // score the whole closure, no truncation
        let scores: HashMap<NodeId, f64> =
            self.activate(&[seed], &config).scores.into_iter().collect();

        let mut hits: Vec<BlastHit> = first
            .into_iter()
            .map(|(id, (depth, reason))| BlastHit {
                id,
                depth,
                reason,
                score: scores.get(&id).copied().unwrap_or(0.0),
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.id.0.cmp(&b.id.0))
        });
        hits
    }
}

/// `GLIA_ALGO_DEBUG=1` turns on the `[reach] blast` line, read once - the
/// variable that turns on `repo_graph_activation::algo`'s `[algo] adjacency`
/// line, whose reader is private to that crate.
fn reach_debug() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("GLIA_ALGO_DEBUG").is_ok_and(|v| v == "1"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use crate::test_support::{flow_graph, repo};
    use crate::types::{RepoGraph, SymbolTable};
    use repo_graph_code_domain::profile::CODE_TABLES;
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
    use repo_graph_core::{Confidence, Edge, Node};

    #[test]
    fn blast_radius_follows_semantic_edges_not_imports() {
        // x --CALLS--> y   (semantic: in the radius)
        // x --IMPORTS--> z (structural: EXCLUDED — the P1 imports-fanout noise)
        let r = repo();
        let x = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::x");
        let y = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::y");
        let z = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "m::z");
        let mk = |id| Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] };
        let mut nav = CodeNav::default();
        nav.record(x, "x", "m::x", node_kind::FUNCTION, None);
        nav.record(y, "y", "m::y", node_kind::FUNCTION, None);
        nav.record(z, "z", "m::z", node_kind::MODULE, None);
        let g = RepoGraph {
            repo: r,
            nodes: vec![mk(x), mk(y), mk(z)],
            edges: vec![
                Edge { from: x, to: y, category: edge_category::CALLS, confidence: Confidence::Strong, cells: Vec::new() },
                Edge { from: x, to: z, category: edge_category::IMPORTS, confidence: Confidence::Strong, cells: Vec::new() },
            ],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        };
        let merged = MergedGraph::new(vec![g]);
        let hits = merged.blast_radius(x, Reach::Forward, 4, &CODE_TABLES);
        // y is reached via CALLS; z (imports-only) is NOT in the radius.
        let ids: Vec<NodeId> = hits.iter().map(|h| h.id).collect();
        assert!(ids.contains(&y), "CALLS target must be in radius");
        assert!(!ids.contains(&z), "IMPORTS target must NOT be in radius");
        let yh = hits.iter().find(|h| h.id == y).unwrap();
        assert_eq!(yh.reason, edge_category::CALLS, "reason = the reaching edge");
        assert_eq!(yh.depth, 1);
    }

    #[test]
    fn blast_radius_forward_closure_and_ranking() {
        // a→b→c, d→c. Forward from a reaches {b,c}; d (inbound to c) does not.
        let merged = MergedGraph::new(vec![flow_graph()]);
        let a = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::a");
        let b = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::b");
        let c = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::c");
        let d = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::d");
        let hits = merged.blast_radius(a, Reach::Forward, 5, &CODE_TABLES);
        let ids: Vec<NodeId> = hits.iter().map(|h| h.id).collect();
        assert!(ids.contains(&b) && ids.contains(&c));
        assert!(!ids.contains(&d), "d is upstream of c, not in forward radius of a");
        // Scores are sorted descending (ranking is real).
        for w in hits.windows(2) {
            assert!(w[0].score >= w[1].score, "hits must be score-sorted");
        }
    }
}
