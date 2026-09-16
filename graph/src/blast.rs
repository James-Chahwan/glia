//! `blast_radius` — the P3 answer-shaped primitive (handoff v6).

use repo_graph_code_domain::edge_category;
use repo_graph_core::{Edge, EdgeCategoryId, NodeId};

use crate::activation::code_activation_defaults;
use crate::merged::MergedGraph;

// ============================================================================
// blast_radius — the P3 answer-shaped primitive (handoff v6)
// ============================================================================

/// Which way the blast radius spreads from the seed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reach {
    /// Downstream — what this node affects (follow edges from→to).
    Forward,
    /// Upstream — what affects this node (follow edges to→from).
    Backward,
    /// Both directions (neighbourhood).
    Both,
}

/// One node in a blast radius: reached via `reason` at `depth`, ranked by `score`.
#[derive(Clone, Debug)]
pub struct BlastHit {
    pub id: NodeId,
    /// Hops from the seed along carry edges.
    pub depth: usize,
    /// The edge category that FIRST reached this node — the "why it's in scope".
    pub reason: EdgeCategoryId,
    /// PPR score seeded at the target (the ranking signal).
    pub score: f64,
}

/// Edge categories that carry blast radius — semantic dependencies only.
/// Structural edges (`DEFINES`/`CONTAINS`/`IMPORTS`/`HAS_ATTRIBUTE`) are
/// EXCLUDED: they pull in unrelated code via shared containers/imports, the
/// exact noise the handoff v6 P1 "edge-category-aware traversal" calls out
/// (bullet 4 — `impact` fanning out through `imports`).
pub fn blast_carry_edges() -> Vec<EdgeCategoryId> {
    use edge_category as ec;
    vec![
        ec::CALLS,
        ec::USES,
        ec::HTTP_CALLS,
        ec::GRPC_CALLS,
        ec::GRAPHQL_CALLS,
        ec::QUEUE_FLOWS,
        ec::WS_CONNECTS,
        ec::EVENT_FLOWS,
        ec::CLI_INVOKES,
        ec::HANDLED_BY,
        ec::INJECTS,
        ec::ACCESSES_DATA,
        ec::TESTS,
        ec::DOCUMENTS,
        ec::IMPLEMENTS,
        ec::INHERITS_FROM,
        ec::RETURNS_TYPE,
        ec::SHARES_SCHEMA,
        ec::SHARES_DATA_ENTITY,
        ec::INFRA_REFERENCES,
        ec::DEPENDS_ON,
        ec::SCHEDULES,
        ec::READS_CONFIG,
        ec::DEFINES_CONFIG,
    ]
}

impl MergedGraph {
    /// The complete, deduped, edge-category-aware, PPR-ranked closure around
    /// `seed` — the answer `impact`+`activate` composed to, in one pass. Each
    /// hit carries the edge category that first reached it (`reason`) and its
    /// PPR score; the caller adds location/kind. Result is sorted by score
    /// (desc), then node id (asc) for determinism.
    ///
    /// `follow` overrides the carry set; `None` uses [`blast_carry_edges`]
    /// (semantic edges only — no structural import/contain noise).
    pub fn blast_radius(
        &self,
        seed: NodeId,
        reach: Reach,
        max_depth: usize,
        follow: Option<&[EdgeCategoryId]>,
    ) -> Vec<BlastHit> {
        use repo_graph_activation::Direction;
        use std::collections::{HashMap, HashSet, VecDeque};

        let carry = follow.map(|f| f.to_vec()).unwrap_or_else(blast_carry_edges);
        let allow: HashSet<EdgeCategoryId> = carry.iter().copied().collect();
        let edges: Vec<&Edge> = self.all_edges().collect();

        let fwd = reach != Reach::Backward;
        let bwd = reach != Reach::Forward;

        // First-reach (depth, reason) per node — BFS so depth is the shortest
        // carry-path and `reason` is the edge that first put the node in scope.
        let mut first: HashMap<NodeId, (usize, EdgeCategoryId)> = HashMap::new();
        let mut visited: HashSet<NodeId> = HashSet::from([seed]);
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(seed, 0)]);
        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for e in &edges {
                if !allow.contains(&e.category) {
                    continue;
                }
                let next = if fwd && e.from == node {
                    Some(e.to)
                } else if bwd && e.to == node {
                    Some(e.from)
                } else {
                    None
                };
                if let Some(next) = next {
                    if visited.insert(next) {
                        first.insert(next, (depth + 1, e.category));
                        queue.push_back((next, depth + 1));
                    }
                }
            }
        }
        if first.is_empty() {
            return Vec::new();
        }

        // Rank the closure by PPR seeded at the target.
        let mut config = code_activation_defaults();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use crate::test_support::{flow_graph, repo};
    use crate::types::{RepoGraph, SymbolTable};
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
    use repo_graph_core::{Confidence, Node};

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
                Edge { from: x, to: y, category: edge_category::CALLS, confidence: Confidence::Strong },
                Edge { from: x, to: z, category: edge_category::IMPORTS, confidence: Confidence::Strong },
            ],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        };
        let merged = MergedGraph::new(vec![g]);
        let hits = merged.blast_radius(x, Reach::Forward, 4, None);
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
        let hits = merged.blast_radius(a, Reach::Forward, 5, None);
        let ids: Vec<NodeId> = hits.iter().map(|h| h.id).collect();
        assert!(ids.contains(&b) && ids.contains(&c));
        assert!(!ids.contains(&d), "d is upstream of c, not in forward radius of a");
        // Scores are sorted descending (ranking is real).
        for w in hits.windows(2) {
            assert!(w[0].score >= w[1].score, "hits must be score-sorted");
        }
    }
}
