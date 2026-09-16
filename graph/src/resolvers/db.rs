//! DB resolver — services touching the same Table / Collection / NodeLabel.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, NodeId, RepoId};

use super::{CrossGraphResolver, weakest};
use crate::merged::MergedGraph;

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
            for i in 0..refs.len() {
                for j in (i + 1)..refs.len() {
                    if refs[i].1 != refs[j].1 {
                        merged.cross_edges.push(Edge {
                            from: refs[i].0,
                            to: refs[j].0,
                            category: edge_category::SHARES_DATA_ENTITY,
                            confidence: weakest(refs[i].2, refs[j].2),
                        });
                    }
                }
            }
        }
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
