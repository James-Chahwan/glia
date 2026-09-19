//! IaC resolver — INFRA_RESOURCE nodes with the same qname across repos.

use std::collections::{HashMap, HashSet};

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, Confidence, Edge, Node, NodeId, RepoId};

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

/// The canonical INFRA_RESOURCE qname prefix, shared by the terraform parser
/// and the k8s/compose/Dockerfile extractor (`code_domain::infra::qname`).
const INFRA_PREFIX: &str = "infra:";

/// Which side of the IaC graph a resource came from, derived from its POSITION
/// cell's file extension rather than from confidence or node order. The
/// terraform parser attaches CODE + POSITION cells to every resource; the
/// manifest path (`extractors/src/iac.rs`) attaches none, so "no `.tf`
/// position" means the manifest side.
fn is_terraform_resource(n: &Node) -> bool {
    n.cells.iter().any(|c| {
        c.kind == cell_type::POSITION
            && match &c.payload {
                // `<file>:<start>-<end>` — strip the line range back off.
                CellPayload::Text(t) => {
                    t.rsplit_once(':').map(|(path, _)| path).unwrap_or(t).ends_with(".tf")
                }
                _ => false,
            }
    })
}

impl CrossGraphResolver for IacResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        // fired_on counters. `tf + k8s == resources` is the proof both emitters
        // agree on `infra:<kind>:<name>`: before A13.5 the terraform parser
        // emitted `<module_qname>::<type>.<name>`, so tf was always 0 here and
        // terraform could never pair with anything.
        let (mut resources, mut tf, mut k8s) = (0usize, 0usize, 0usize);
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::INFRA_RESOURCE) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                if qname.starts_with(INFRA_PREFIX) {
                    resources += 1;
                    if is_terraform_resource(n) {
                        tf += 1;
                    } else {
                        k8s += 1;
                    }
                }
                index
                    .entry(qname.clone())
                    .or_default()
                    .push((n.id, g.repo, n.confidence));
            }
        }
        let mut paired = 0usize;
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
                            cells: Vec::new(),
                        });
                        paired += 1;
                    }
                }
            }
        }
        // One line per BUILD, and only when this resolver had anything to say —
        // the `[queues]` house style, so builds with no IaC stay silent.
        if resources > 0 {
            eprintln!("[iac] resources={resources} paired={paired} tf={tf} k8s={k8s}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::{CodeNav, GRAPH_TYPE};
    use glia_core::{Cell, Node};
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
    fn iac_resolver_pairs_terraform_to_k8s_service() {
        // A13.5: `resource "aws_ecs_service" "api"` now emits the SAME qname the
        // k8s `kind: Service` / `metadata.name: api` path emits, so the two
        // finally pair. Before, the terraform side was `main::aws_ecs_service.api`.
        let tf_repo = RepoId(21);
        let k8s_repo = RepoId(22);
        let mut g_tf = graph_with_infra(tf_repo, "infra:service:api");
        // Give the terraform node the POSITION cell its parser attaches, so the
        // tf/k8s split in the fired_on marker is exercised too.
        g_tf.nodes[0].cells = vec![Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Text("infra/main.tf:1-6".to_string()),
        }];
        assert!(is_terraform_resource(&g_tf.nodes[0]));
        let g_k8s = graph_with_infra(k8s_repo, "infra:service:api");
        assert!(!is_terraform_resource(&g_k8s.nodes[0]));
        let mut merged = MergedGraph::new(vec![g_tf, g_k8s]);
        merged.run(&IacResolver);
        assert_eq!(
            merged
                .cross_edges
                .iter()
                .filter(|e| e.category == edge_category::SHARES_INFRA_REF)
                .count(),
            1
        );
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
