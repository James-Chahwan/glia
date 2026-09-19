//! Cron resolver — CRON_JOB nodes sharing the full `cron:<schedule>:<target>`
//! qname across repos.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, NodeId, RepoId};

use super::{CrossGraphResolver, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// CronResolver — pairs CRON_JOB nodes that share the full qname
// (`cron:<schedule>:<target>`) across different repos. Surfaces drift /
// accidental duplication of scheduled work; pairing on schedule alone is
// noise (e.g. five unrelated 4am jobs).
// ============================================================================

pub struct CronResolver;

impl CrossGraphResolver for CronResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::CRON_JOB) {
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
                            category: edge_category::SHARES_CRON_SCHEDULE,
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
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE};
    use repo_graph_core::Node;
    use crate::types::{RepoGraph, SymbolTable};

    fn graph_with_cron(repo_id: RepoId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::CRON_JOB, qname);
        let mut nav = CodeNav::default();
        nav.record(
            id,
            qname.split(':').nth(1).unwrap_or(qname),
            qname,
            node_kind::CRON_JOB,
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
    fn cron_resolver_pairs_same_schedule_target_across_repos() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_cron(repo_a, "cron:0 4 * * *:cleanup");
        let g_b = graph_with_cron(repo_b, "cron:0 4 * * *:cleanup");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&CronResolver);
        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_CRON_SCHEDULE)
            .collect();
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn cron_resolver_does_not_pair_on_schedule_alone() {
        // Same schedule, different targets — must NOT pair (no drift).
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_cron(repo_a, "cron:0 4 * * *:cleanup");
        let g_b = graph_with_cron(repo_b, "cron:0 4 * * *:reindex");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&CronResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_CRON_SCHEDULE),
            "different targets at same schedule must not pair"
        );
    }
}
