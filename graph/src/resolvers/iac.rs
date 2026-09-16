//! IaC resolver — INFRA_RESOURCE nodes with the same qname across repos.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, NodeId, RepoId};

use super::{CrossGraphResolver, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// IacResolver — pairs INFRA_RESOURCE nodes with the same qname
// (`infra:<kind>:<name>`) across repos. Captures cross-service
// container/manifest references — image built in repo A consumed by k8s
// manifest in repo B; same Service name appearing in compose for one repo and
// k8s for another (drift signal).
// ============================================================================

pub struct IacResolver;

impl CrossGraphResolver for IacResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::INFRA_RESOURCE) {
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
                            category: edge_category::SHARES_INFRA_REF,
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

    fn graph_with_infra(repo_id: RepoId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::INFRA_RESOURCE, qname);
        let mut nav = CodeNav::default();
        nav.record(
            id,
            qname.rsplit(':').next().unwrap_or(qname),
            qname,
            node_kind::INFRA_RESOURCE,
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
    fn iac_resolver_pairs_same_image_across_repos() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_infra(repo_a, "infra:image:api");
        let g_b = graph_with_infra(repo_b, "infra:image:api");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&IacResolver);
        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_INFRA_REF)
            .collect();
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn iac_resolver_keeps_kinds_separate() {
        // `infra:service:api` vs `infra:deployment:api` — same name, different
        // kind, distinct qnames → no pairing.
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_infra(repo_a, "infra:service:api");
        let g_b = graph_with_infra(repo_b, "infra:deployment:api");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&IacResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_INFRA_REF)
        );
    }
}
