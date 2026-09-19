//! `blast_radius` — the P3 answer-shaped primitive (handoff v6).

use std::sync::OnceLock;

use repo_graph_activation::algo::{self, Adjacency, Walk};
use repo_graph_activation::plan::{ActivationPlan, FilterPredicate};
use repo_graph_activation::profile::DomainTables;
use repo_graph_core::{EdgeCategoryId, NodeId};

use crate::merged::MergedGraph;

// ============================================================================
// blast_radius — the P3 answer-shaped primitive (handoff v6)
// ============================================================================

/// Which way the blast radius spreads from the seeds.
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

/// One node in a blast radius: reached from `seed` via `reason` at `depth`,
/// ranked by `score`. Produced by `MergedGraph::blast_radius`, never built by
/// a struct literal outside this crate (LD.9):
///
/// ```compile_fail
/// let _ = repo_graph_graph::BlastHit {
///     id: repo_graph_core::NodeId(0),
///     depth: 0,
///     reason: repo_graph_core::EdgeCategoryId(0),
///     score: 0.0,
///     seed: repo_graph_core::NodeId(0),
/// };
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BlastHit {
    pub id: NodeId,
    /// Hops from the nearest seed along carry edges.
    pub depth: usize,
    /// The edge category that FIRST reached this node — the "why it's in scope".
    pub reason: EdgeCategoryId,
    /// PPR score personalised over the seeds (the ranking signal).
    pub score: f64,
    /// The seed whose wave reached this node first (LD.5): the root of its
    /// parent chain in the walk, so with one seed it is always that seed.
    pub seed: NodeId,
}

impl MergedGraph {
    /// The complete, deduped, edge-category-aware, PPR-ranked closure around
    /// `seeds` — the answer `impact`+`activate` composed to, in one pass. Each
    /// hit carries the edge category that first reached it (`reason`), the
    /// seed whose wave reached it (`seed`) and its PPR score; the caller adds
    /// location/kind. Result is sorted by score (desc), then node id (asc)
    /// for determinism.
    ///
    /// Many seeds are ONE walk and ONE ranking (LD.5), never a union of
    /// per-seed answers: the seeds are deduplicated in input order and start
    /// the breadth-first walk together, so each node's `depth` is its hop
    /// count from the NEAREST seed, and PPR is personalised uniformly over
    /// all of them, so every score comes from the same run and compares. A
    /// seed is never a hit, even when another seed's wave reaches it: that
    /// fact is the caller's to report (the engine's `linked_seeds`).
    ///
    /// The domain's `tables` say what to follow and how to rank (LD.14b): the
    /// walk follows `tables.carry_edges` only — semantic dependencies, never
    /// the structural `DEFINES` / `CONTAINS` / `IMPORTS` / `HAS_ATTRIBUTE`
    /// edges that pull in unrelated code through shared containers and
    /// imports (handoff v6 P1, bullet 4: `impact` fanning out through
    /// `imports`) — and the ranking is `tables.activation_config(None)`. The
    /// code domain passes `repo_graph_code_domain::profile::CODE_TABLES`; a
    /// caller that needs another carry set passes its own tables.
    ///
    /// [`Self::blast_radius_filtered`] with no filters.
    pub fn blast_radius(
        &self,
        seeds: &[NodeId],
        reach: Reach,
        max_depth: usize,
        tables: &DomainTables,
    ) -> Vec<BlastHit> {
        self.blast_radius_filtered(seeds, reach, max_depth, tables, &[])
    }

    /// [`Self::blast_radius`] with `filters` run over the ranked closure
    /// (LD.12c): the closure is ranked by one
    /// [`ActivationPlan::rank`] — PPR seeded at `seeds`, every closure node at
    /// its score or 0.0 where PPR gave it none, sorted by score (desc) then
    /// node id (asc) — and each filter drops nodes from it in registration
    /// order. A filter only removes rows, so the kept rows are the unfiltered
    /// answer's rows in the same order. Nothing is truncated here: a caller's
    /// `top_k` cut comes after its own post-steps.
    ///
    /// A hit's `seed` is read off the walk's parent chain in discovery order:
    /// a node reached from a seed belongs to that seed, any other node to its
    /// parent's seed (a parent is always discovered before its child). The
    /// walk expands the seeds in input order and each node's edges in edge
    /// order (LD.15a), so a node two seeds reach at the same depth belongs to
    /// the one listed first — never to a `HashMap` iteration order.
    ///
    /// `GLIA_ACTIVATION_DEBUG=1` prints the plan's
    /// `[activation] plan mode=rank .. filters=[..] universe=N kept=N dropped=[..]`
    /// line, `universe` being the closure.
    pub fn blast_radius_filtered(
        &self,
        seeds: &[NodeId],
        reach: Reach,
        max_depth: usize,
        tables: &DomainTables,
        filters: &[&dyn FilterPredicate<MergedGraph>],
    ) -> Vec<BlastHit> {
        use repo_graph_activation::Direction;
        use std::collections::{HashMap, HashSet};

        // Deduplicated, first occurrence kept: the walk's start order and the
        // PPR personalisation (uniform over distinct seeds).
        let mut is_seed: HashSet<NodeId> = HashSet::with_capacity(seeds.len());
        let seeds: Vec<NodeId> = seeds.iter().copied().filter(|s| is_seed.insert(*s)).collect();

        // First-reach (depth, reason) per node: a BFS over the carry edges'
        // CSR index (LD.15b), so depth is the shortest carry-path from the
        // nearest seed and `reason` the category of the edge that first put
        // the node in scope - exactly what the per-node edge scan it replaced
        // returned.
        let walk = match reach {
            Reach::Forward => Walk::Forward,
            Reach::Backward => Walk::Backward,
            Reach::Both => Walk::Both,
        };
        let adj = Adjacency::carry(self, tables);
        let b = algo::reach::bfs(&adj, &seeds, walk, max_depth);
        if reach_debug() {
            // fired_on marker (LD.15b), a cost diagnostic: `scanned` is the
            // incidences the walk examined, at most two per kept edge.
            eprintln!(
                "[reach] blast walk={walk:?} seeds={} depth<={max_depth} reached={} scanned={} index_nodes={} kept={}",
                seeds.len(),
                b.reached.len(),
                b.scanned,
                adj.len(),
                adj.kept_edges()
            );
        }
        if b.reached.is_empty() {
            return Vec::new();
        }
        // (depth, reason, seed) per reached node, in discovery order: the
        // parent is a seed, or was reached (and attributed) earlier.
        let mut first: HashMap<NodeId, (usize, EdgeCategoryId, NodeId)> =
            HashMap::with_capacity(b.reached.len());
        for r in &b.reached {
            let seed = if is_seed.contains(&r.parent) {
                r.parent
            } else {
                first.get(&r.parent).map_or(r.parent, |&(_, _, s)| s)
            };
            first.insert(r.id, (r.depth, r.via, seed));
        }
        let closure: Vec<NodeId> = b.reached.iter().map(|r| r.id).collect();

        // Rank the closure by one PPR over every seed, then filter it: one
        // plan. A closure node PPR never scored stays in at 0.0.
        let mut config = tables.activation_config(None);
        config.direction = match reach {
            Reach::Forward => Direction::Forward,
            Reach::Backward => Direction::Backward,
            Reach::Both => Direction::Undirected,
        };
        config.top_k = usize::MAX; // score the whole closure, no truncation
        let mut plan = ActivationPlan::new(config);
        for f in filters {
            plan = plan.filter(*f);
        }
        let view = plan.rank(self, &seeds, &closure);

        // The view's order (score desc, id asc) is the answer's order.
        view.scores
            .into_iter()
            .filter_map(|(id, score)| {
                first
                    .get(&id)
                    .map(|&(depth, reason, seed)| BlastHit { id, depth, reason, score, seed })
            })
            .collect()
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
        let hits = merged.blast_radius(&[x], Reach::Forward, 4, &CODE_TABLES);
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
        let hits = merged.blast_radius(&[a], Reach::Forward, 5, &CODE_TABLES);
        let ids: Vec<NodeId> = hits.iter().map(|h| h.id).collect();
        assert!(ids.contains(&b) && ids.contains(&c));
        assert!(!ids.contains(&d), "d is upstream of c, not in forward radius of a");
        // Scores are sorted descending (ranking is real).
        for w in hits.windows(2) {
            assert!(w[0].score >= w[1].score, "hits must be score-sorted");
        }
    }

    /// Drops one named node: the filter half of an `ActivationPlan`.
    struct DropOne(NodeId);

    impl FilterPredicate<MergedGraph> for DropOne {
        fn name(&self) -> &'static str {
            "drop"
        }

        fn keep(&self, _: &MergedGraph, id: NodeId, _: f64) -> bool {
            id != self.0
        }
    }

    fn rows(hits: &[BlastHit]) -> Vec<(NodeId, usize, EdgeCategoryId, u64)> {
        hits.iter().map(|h| (h.id, h.depth, h.reason, h.score.to_bits())).collect()
    }

    #[test]
    fn blast_radius_filtered_is_the_unfiltered_answer_minus_the_dropped_rows() {
        // a→b→c, d→c. Both ways from c reaches {b, d, a}.
        let merged = MergedGraph::new(vec![flow_graph()]);
        let id = |q: &str| NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, q);
        let (b, c) = (id("m::b"), id("m::c"));
        let all = merged.blast_radius(&[c], Reach::Both, 4, &CODE_TABLES);
        assert_eq!(all.len(), 3);
        assert_eq!(
            rows(&merged.blast_radius_filtered(&[c], Reach::Both, 4, &CODE_TABLES, &[])),
            rows(&all),
            "no filter: the blast_radius answer, bit for bit"
        );
        let drop_b = DropOne(b);
        let kept = merged.blast_radius_filtered(&[c], Reach::Both, 4, &CODE_TABLES, &[&drop_b]);
        let expected: Vec<BlastHit> = all.iter().filter(|h| h.id != b).cloned().collect();
        assert_eq!(rows(&kept), rows(&expected), "same rows, same order, b gone");
    }

    #[test]
    fn closure_node_ppr_never_scored_stays_in_at_zero() {
        // x --CALLS--> ghost, an id that is no node: the walk reaches it, PPR
        // (over the nodes) cannot score it, and it stays in the answer at 0.0.
        let r = repo();
        let x = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::x");
        let y = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::y");
        let ghost = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::ghost");
        let mk = |id| Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] };
        let call = |from, to| Edge {
            from,
            to,
            category: edge_category::CALLS,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        };
        let mut nav = CodeNav::default();
        nav.record(x, "x", "m::x", node_kind::FUNCTION, None);
        nav.record(y, "y", "m::y", node_kind::FUNCTION, None);
        let g = RepoGraph {
            repo: r,
            nodes: vec![mk(x), mk(y)],
            edges: vec![call(x, y), call(x, ghost)],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        };
        let merged = MergedGraph::new(vec![g]);
        let hits = merged.blast_radius(&[x], Reach::Forward, 4, &CODE_TABLES);
        let ids: Vec<NodeId> = hits.iter().map(|h| h.id).collect();
        assert_eq!(ids, vec![y, ghost], "y scored, ghost last at 0.0");
        assert!(hits[0].score > 0.0);
        assert_eq!(hits[1].score.to_bits(), 0.0f64.to_bits());
        assert_eq!(hits[1].depth, 1);
    }

    #[test]
    fn multi_seed_bfs_excludes_seeds_and_attributes_hits() {
        // a→b→c, d→c.
        let merged = MergedGraph::new(vec![flow_graph()]);
        let id = |q: &str| NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, q);
        let (a, b, c, d) = (id("m::a"), id("m::b"), id("m::c"), id("m::d"));
        let facts = |hits: &[BlastHit]| {
            let mut v: Vec<(NodeId, usize, NodeId)> = hits.iter().map(|h| (h.id, h.depth, h.seed)).collect();
            v.sort_by_key(|t| t.0.0);
            v
        };
        let sorted = |mut v: Vec<(NodeId, usize, NodeId)>| {
            v.sort_by_key(|t| t.0.0);
            v
        };

        // [a, d] forward: one walk, so c is one hop from d, not two from a;
        // neither seed is a hit. A repeated seed changes nothing.
        let hits = merged.blast_radius(&[a, d], Reach::Forward, 4, &CODE_TABLES);
        assert_eq!(facts(&hits), sorted(vec![(b, 1, a), (c, 1, d)]));
        let again = merged.blast_radius(&[a, a, d, a], Reach::Forward, 4, &CODE_TABLES);
        assert_eq!(rows(&again), rows(&hits), "duplicate seeds are one seed");
        for w in hits.windows(2) {
            assert!(w[0].score >= w[1].score, "one ranking over the union, score-sorted");
        }

        // A seed another seed reaches is never a hit: [a, b] forward is {c},
        // attributed to b, whose wave reached it.
        let hits = merged.blast_radius(&[a, b], Reach::Forward, 4, &CODE_TABLES);
        assert_eq!(facts(&hits), vec![(c, 1, b)]);

        // Attribution follows the parent chain: from a alone, c (via b) is a's.
        let hits = merged.blast_radius(&[a], Reach::Forward, 4, &CODE_TABLES);
        assert_eq!(facts(&hits), sorted(vec![(b, 1, a), (c, 2, a)]));

        // A tie (b and d both reach c in one hop) goes to the seed listed
        // first, in either order.
        let bd = merged.blast_radius(&[b, d], Reach::Forward, 4, &CODE_TABLES);
        assert_eq!(facts(&bd), vec![(c, 1, b)]);
        let db = merged.blast_radius(&[d, b], Reach::Forward, 4, &CODE_TABLES);
        assert_eq!(facts(&db), vec![(c, 1, d)]);

        // Backward from [c]: b and d at depth 1, a at 2 through b, all c's.
        let hits = merged.blast_radius(&[c], Reach::Backward, 4, &CODE_TABLES);
        assert_eq!(facts(&hits), sorted(vec![(a, 2, c), (b, 1, c), (d, 1, c)]));

        // No seeds, no walk.
        assert!(merged.blast_radius(&[], Reach::Both, 4, &CODE_TABLES).is_empty());
    }
}
