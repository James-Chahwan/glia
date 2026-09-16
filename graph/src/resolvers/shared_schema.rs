//! Shared-schema resolver — the same schema-ish type imported by 2+ repos.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, NodeId, NodeKindId, RepoId};

use super::{CrossGraphResolver, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// SharedSchemaResolver — detects shared imports across repos
// ============================================================================

pub struct SharedSchemaResolver;

impl CrossGraphResolver for SharedSchemaResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut import_index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                    continue;
                }
                if let Some(children) = g.nav.children_of.get(&n.id) {
                    for &child in children {
                        if let Some(qname) = g.nav.qname_by_id.get(&child)
                            && is_schema_type(qname, g.nav.kind_by_id.get(&child).copied())
                        {
                            import_index
                                .entry(g.nav.name_by_id.get(&child).cloned().unwrap_or_default())
                                .or_default()
                                .push((child, g.repo, n.confidence));
                        }
                    }
                }
            }
        }

        for refs in import_index.values() {
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
                            category: edge_category::SHARES_SCHEMA,
                            confidence: weakest(
                                refs[i].2,
                                refs[j].2,
                            ),
                        });
                    }
                }
            }
        }
    }
}

fn is_schema_type(qname: &str, kind: Option<NodeKindId>) -> bool {
    let schema_hints = [
        "Schema", "Validator", "Type", "Model", "Entity", "DTO",
        "Input", "Output", "Params", "Request", "Response",
    ];
    let is_type_kind = matches!(
        kind,
        Some(k) if k == node_kind::CLASS || k == node_kind::INTERFACE || k == node_kind::STRUCT
    );
    is_type_kind && schema_hints.iter().any(|h| qname.contains(h))
}
