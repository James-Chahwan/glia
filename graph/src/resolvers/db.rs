//! DB resolver — services touching the same Table / Collection / NodeLabel,
//! and (A13.3) services reaching the same external data provider.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, NodeId, NodeKindId, RepoId};

use super::{CrossGraphResolver, emit_cross_repo_pairs};
use crate::merged::MergedGraph;

/// The five provider kinds `extractors::data_sources` emits, one per
/// `DataSourceKind` variant. Nodes of these kinds carry the global qname
/// `data_source:<provider>`, so two services reaching the same Redis converge
/// on the same key in different repos.
const DATA_SOURCE_KINDS: [NodeKindId; 5] = [
    node_kind::DATABASE,
    node_kind::CACHE,
    node_kind::BLOB_STORE,
    node_kind::SEARCH_INDEX,
    node_kind::EMAIL_SERVICE,
];

/// Distinct repos a single provider may join before the pairing is dropped.
/// Provider nodes are the "framework-tag fallback is pairable" shape: every
/// service in a 40-service stack imports `redis`, and an unbounded pairing
/// would emit ~800 all-to-all edges of near-zero information. Above the cap we
/// emit NOTHING for that provider rather than a fan-out nobody can use.
const MAX_DATA_SOURCE_FANOUT: usize = 8;

// ============================================================================
// DbResolver — joins services that touch the same Table / Collection /
// NodeLabel by indexing DATA_ENTITY nodes by their full qname
// (`data_entity:<flavor>:<name>`) and pairing nodes that live in different
// repos. Mirrors SharedSchemaResolver's pairwise-pair shape; the qname's
// flavor segment ensures `users` (SQL table) and `User` (Mongoose model)
// don't collide.
// ============================================================================

pub struct DbResolver;

impl CrossGraphResolver for DbResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut entity_index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> =
            HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::DATA_ENTITY) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                entity_index
                    .entry(qname.clone())
                    .or_default()
                    .push((n.id, g.repo, n.confidence));
            }
        }
        for refs in entity_index.values() {
            if refs.len() < 2 {
                continue;
            }
            let repos: HashSet<RepoId> = refs.iter().map(|(_, r, _)| *r).collect();
            if repos.len() < 2 {
                continue;
            }
            emit_cross_repo_pairs(
                refs,
                edge_category::SHARES_DATA_ENTITY,
                None,
                &mut merged.cross_edges,
            );
        }

        // ---- A13.3: provider pass ------------------------------------------
        // Keyed on the VERBATIM `data_source:<provider>` qname — providers are
        // a closed vocabulary from the extractor's PATTERNS table, so there is
        // nothing to canonicalise and any normalisation would only collide
        // distinct providers.
        let mut source_index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> =
            HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                let Some(kind) = g.nav.kind_by_id.get(&n.id) else {
                    continue;
                };
                if !DATA_SOURCE_KINDS.contains(kind) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                source_index
                    .entry(qname.clone())
                    .or_default()
                    .push((n.id, g.repo, n.confidence));
            }
        }
        let mut providers = 0usize;
        let mut paired = 0usize;
        let mut skipped_fanout = 0usize;
        let mut emitted: Vec<Edge> = Vec::new();
        for refs in source_index.values() {
            if refs.len() < 2 {
                continue;
            }
            let repos: HashSet<RepoId> = refs.iter().map(|(_, r, _)| *r).collect();
            if repos.len() < 2 {
                continue;
            }
            if repos.len() > MAX_DATA_SOURCE_FANOUT {
                skipped_fanout += 1;
                continue;
            }
            providers += 1;
            // Forced `Weak`, NOT `weakest(a, b)`: the provider nodes are
            // Medium by construction, and `weakest` would hand a substring
            // match on `"Redis"` the same confidence as a resolved call.
            paired += emit_cross_repo_pairs(
                refs,
                edge_category::SHARES_DATA_SOURCE,
                Some(Confidence::Weak),
                &mut emitted,
            );
        }
        // HashMap iteration order is per-process random; sort so the emitted
        // block is byte-stable across runs.
        emitted.sort_by_key(|e| (e.from.0, e.to.0));
        if providers > 0 || skipped_fanout > 0 {
            eprintln!(
                "[db-source] providers={providers} paired={paired} \
                 skipped_fanout={skipped_fanout}"
            );
        }
        merged.cross_edges.extend(emitted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE};
    use repo_graph_core::Node;
    use crate::types::{RepoGraph, SymbolTable};

    /// Build a single-node RepoGraph holding one DATA_ENTITY for `qname` in
    /// `repo_id`. Used to assemble cross-repo fixtures for DbResolver tests.
    fn graph_with_entity(repo_id: RepoId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::DATA_ENTITY, qname);
        let mut nav = CodeNav::default();
        nav.record(
            id,
            qname.rsplit(':').next().unwrap_or(qname),
            qname,
            node_kind::DATA_ENTITY,
            None,
        );
        RepoGraph {
            repo: repo_id,
            nodes: vec![Node {
                id,
                repo: repo_id,
                confidence: Confidence::Medium,
                cells: vec![],
            }],
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    /// Build a single-node RepoGraph holding one provider node of `kind` for
    /// `qname` in `repo_id` — the shape `extractors::data_sources` emits.
    fn graph_with_data_source(repo_id: RepoId, kind: NodeKindId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, kind, qname);
        let mut nav = CodeNav::default();
        nav.record(id, qname.rsplit(':').next().unwrap_or(qname), qname, kind, None);
        RepoGraph {
            repo: repo_id,
            nodes: vec![Node {
                id,
                repo: repo_id,
                confidence: Confidence::Medium,
                cells: vec![],
            }],
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    #[test]
    fn db_resolver_pairs_same_data_source_across_repos() {
        // A13.3 — two services reaching the same Redis. Exactly one edge, and
        // it is WEAK: the provider nodes are Medium, so `weakest(a, b)` would
        // have said Medium and overstated a substring guess.
        let g_a = graph_with_data_source(RepoId(11), node_kind::CACHE, "data_source:redis");
        let g_b = graph_with_data_source(RepoId(12), node_kind::CACHE, "data_source:redis");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);

        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_DATA_SOURCE)
            .collect();
        assert_eq!(edges.len(), 1, "expected one cross-repo SHARES_DATA_SOURCE edge");
        assert_eq!(
            edges[0].confidence,
            Confidence::Weak,
            "provider pairing is a substring guess and must stay Weak"
        );
    }

    #[test]
    fn db_resolver_keeps_providers_separate() {
        // postgres and redis are distinct keys even across repos.
        let g_a = graph_with_data_source(RepoId(11), node_kind::CACHE, "data_source:redis");
        let g_b =
            graph_with_data_source(RepoId(12), node_kind::DATABASE, "data_source:postgres");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DATA_SOURCE),
            "different providers must not pair"
        );
    }

    #[test]
    fn db_resolver_drops_data_source_fanout_above_cap() {
        // 9 distinct repos on one provider is the all-to-all noise shape the
        // cap exists to refuse: nothing at all, not 36 edges.
        let graphs: Vec<RepoGraph> = (0..(MAX_DATA_SOURCE_FANOUT as u64 + 1))
            .map(|i| {
                graph_with_data_source(RepoId(100 + i), node_kind::CACHE, "data_source:redis")
            })
            .collect();
        let mut merged = MergedGraph::new(graphs);
        merged.run(&DbResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DATA_SOURCE),
            "a provider above MAX_DATA_SOURCE_FANOUT must emit nothing"
        );
    }

    #[test]
    fn shares_data_source_is_not_a_blast_carry_edge() {
        // REGRESSION GUARD. A shared Postgres is an operational fact, not a
        // code dependency: carrying it would fan every blast radius across
        // every service in the stack. Do not "fix" this by adding the row.
        assert!(
            !crate::blast::blast_carry_edges().contains(&edge_category::SHARES_DATA_SOURCE),
            "SHARES_DATA_SOURCE must stay OUT of blast_carry_edges()"
        );
    }

    #[test]
    fn db_resolver_pairs_same_entity_across_repos() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_entity(repo_a, "data_entity:sql:users");
        let g_b = graph_with_entity(repo_b, "data_entity:sql:users");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);

        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_DATA_ENTITY)
            .collect();
        assert_eq!(edges.len(), 1, "expected one cross-repo SHARES_DATA_ENTITY edge");
    }

    #[test]
    fn db_resolver_does_not_pair_within_a_single_repo() {
        // Two DATA_ENTITY nodes with the same qname inside one repo would
        // already collapse via NodeId; even if duplicated, no cross-edge.
        let repo_a = RepoId(11);
        let g1 = graph_with_entity(repo_a, "data_entity:sql:users");
        let g2 = graph_with_entity(repo_a, "data_entity:sql:users");
        let mut merged = MergedGraph::new(vec![g1, g2]);
        merged.run(&DbResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DATA_ENTITY),
            "must not emit SHARES_DATA_ENTITY when all matches share the same repo"
        );
    }

    #[test]
    fn db_resolver_keeps_flavors_separate() {
        // A SQL `users` table and a NoSQL `users` collection have different
        // qname flavor segments and must not be joined.
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_entity(repo_a, "data_entity:sql:users");
        let g_b = graph_with_entity(repo_b, "data_entity:nosql:users");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DATA_ENTITY),
            "flavor mismatch must not emit a SHARES_DATA_ENTITY edge"
        );
    }
}
