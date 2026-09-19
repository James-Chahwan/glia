//! Config resolver — CONFIG_KEY nodes with the same qname across repos.

use std::collections::{HashMap, HashSet};

use glia_code_domain::{edge_category, node_kind};
use glia_core::{Confidence, Edge, NodeId, RepoId};

use super::{CrossGraphResolver, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// ConfigResolver — pairs CONFIG_KEY nodes with the same qname across repos.
// Same env-var name consumed/defined in 2+ services → SHARES_CONFIG edge.
// Useful drift signal (key renamed in one place but not another) and a
// substrate query "which services depend on DB_URL?".
// ============================================================================

pub struct ConfigResolver;

impl CrossGraphResolver for ConfigResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::CONFIG_KEY) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                index
                    .entry(qname.clone())
                    .or_default()
                    .push((n.id, g.repo, n.confidence));
            }
        }
        for refs in index.values() {
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
                            category: edge_category::SHARES_CONFIG,
                            confidence: weakest(refs[i].2, refs[j].2),
                            cells: Vec::new(),
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
    use glia_code_domain::{CodeNav, GRAPH_TYPE};
    use glia_core::Node;
    use crate::types::{RepoGraph, SymbolTable};

    fn graph_with_config(repo_id: RepoId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::CONFIG_KEY, qname);
        let mut nav = CodeNav::default();
        nav.record(
            id,
            qname.rsplit(':').next().unwrap_or(qname),
            qname,
            node_kind::CONFIG_KEY,
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
    fn config_resolver_pairs_same_key_across_repos() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_config(repo_a, "config:env:DATABASE_URL");
        let g_b = graph_with_config(repo_b, "config:env:DATABASE_URL");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&ConfigResolver);
        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_CONFIG)
            .collect();
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn config_resolver_does_not_pair_distinct_keys() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_config(repo_a, "config:env:DATABASE_URL");
        let g_b = graph_with_config(repo_b, "config:env:API_KEY");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&ConfigResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_CONFIG)
        );
    }
}
