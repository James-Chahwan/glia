//! Package resolver — PACKAGE_DEP nodes with the same qname across repos.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, NodeId, RepoId};

use super::{CrossGraphResolver, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// PackageResolver — pairs PACKAGE_DEP nodes with the same qname
// (`package:<ecosystem>:<name>`) across repos. Surfaces the "two services
// depend on the same package" signal that would otherwise need an external
// SCA tool. Cross-language reachability (the differentiator vs Endor / Snyk
// / Socket.dev) lives in v0.5+ — this resolver only emits the dependency
// substrate; per-symbol reachability layers on top of it.
// ============================================================================

pub struct PackageResolver;

impl CrossGraphResolver for PackageResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::PACKAGE_DEP) {
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
                            category: edge_category::SHARES_DEPENDENCY,
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

    fn graph_with_package(repo_id: RepoId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::PACKAGE_DEP, qname);
        let mut nav = CodeNav::default();
        nav.record(
            id,
            qname.rsplit(':').next().unwrap_or(qname),
            qname,
            node_kind::PACKAGE_DEP,
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
    fn package_resolver_pairs_same_dep_across_repos() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_package(repo_a, "package:npm:react");
        let g_b = graph_with_package(repo_b, "package:npm:react");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&PackageResolver);
        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_DEPENDENCY)
            .collect();
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn package_resolver_keeps_ecosystems_separate() {
        // Same package name, different ecosystems → no pair.
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_package(repo_a, "package:npm:requests");
        let g_b = graph_with_package(repo_b, "package:pypi:requests");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&PackageResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DEPENDENCY)
        );
    }
}
