//! Shared-schema resolver — the same schema-ish type imported by 2+ repos.

use std::collections::{HashMap, HashSet};

use glia_code_domain::{edge_category, node_kind};
use glia_core::{Confidence, Edge, NodeId, NodeKindId, RepoId};

use super::{CrossGraphResolver, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// SharedSchemaResolver — detects shared imports across repos
// ============================================================================

pub struct SharedSchemaResolver;

impl CrossGraphResolver for SharedSchemaResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut import_index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        let mut total = 0usize;
        let mut in_package = 0usize;
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                    continue;
                }
                // A type can hang off the file MODULE directly (Python / TS /
                // Go) or off a namespace PACKAGE inside it (C# and PHP
                // `namespace`, Ruby `module`) — for those the DEFINES edge runs
                // from the PACKAGE, so a MODULE-only walk was structurally
                // blind to every namespaced contract type. Descend exactly ONE
                // level: a type nested inside a type is not a shared-schema
                // shape, and deeper recursion would inflate the O(n^2) pairwise
                // emission below without adding a real contract.
                let mut scopes: Vec<(NodeId, bool)> = vec![(n.id, false)];
                if let Some(children) = g.nav.children_of.get(&n.id) {
                    for &c in children {
                        if g.nav.kind_by_id.get(&c) == Some(&node_kind::PACKAGE) {
                            scopes.push((c, true));
                        }
                    }
                }
                for (scope, nested) in scopes {
                    let Some(children) = g.nav.children_of.get(&scope) else {
                        continue;
                    };
                    for &child in children {
                        if let Some(qname) = g.nav.qname_by_id.get(&child)
                            && is_schema_type(qname, g.nav.kind_by_id.get(&child).copied())
                        {
                            total += 1;
                            if nested {
                                in_package += 1;
                            }
                            // Confidence stays the enclosing MODULE's, exactly
                            // as before, so `weakest()` pairing is unchanged
                            // for types that were already indexed.
                            import_index
                                .entry(g.nav.name_by_id.get(&child).cloned().unwrap_or_default())
                                .or_default()
                                .push((child, g.repo, n.confidence));
                        }
                    }
                }
            }
        }

        // Collect, sort, THEN push. `import_index` is a HashMap and `.values()`
        // is hash order, so emitting straight into `cross_edges` makes the
        // cross-edge sequence differ run to run — which reshuffles the store's
        // shards and flaps engine/tests/byte_identical.rs (audit 2026-06-10
        // #5). Tolerable while C#/PHP contributed nothing; not once the index
        // is widened.
        let mut emitted: Vec<Edge> = Vec::new();
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
                        emitted.push(Edge {
                            from: refs[i].0,
                            to: refs[j].0,
                            category: edge_category::SHARES_SCHEMA,
                            confidence: weakest(refs[i].2, refs[j].2),
                            cells: Vec::new(),
                        });
                    }
                }
            }
        }
        emitted.sort_by_key(|e| (e.from.0, e.to.0));
        if !emitted.is_empty() {
            eprintln!(
                "[schema] indexed {total} schema types ({in_package} in namespace packages), {} cross-repo pairs",
                emitted.len()
            );
        }
        merged.cross_edges.extend(emitted);
    }
}

fn is_schema_type(qname: &str, kind: Option<NodeKindId>) -> bool {
    // PascalCase hints, matched case-sensitively as substrings of the qname.
    // `Dto` sits beside `DTO` because .NET naming guidelines spell the acronym
    // `OrderDto`, and `contains("DTO")` never matched that — the packet's own
    // C#-DTO use case was blind twice over.
    let schema_hints = [
        "Schema", "Validator", "Type", "Model", "Entity", "DTO", "Dto",
        "Input", "Output", "Params", "Request", "Response",
    ];
    let is_type_kind = matches!(
        kind,
        Some(k) if k == node_kind::CLASS || k == node_kind::INTERFACE || k == node_kind::STRUCT
    );
    is_type_kind && schema_hints.iter().any(|h| qname.contains(h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RepoGraph, SymbolTable};
    use glia_code_domain::{CodeNav, GRAPH_TYPE};
    use glia_core::Node;

    /// One repo shaped like a C# file: MODULE `Models` → PACKAGE `<ns>` →
    /// CLASS `OrderDto`. This is the nesting the resolver used to miss.
    fn graph_with_namespaced_type(repo_id: RepoId, ns: &str, ty: &str) -> RepoGraph {
        let module = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::MODULE, "Models");
        let pkg = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::PACKAGE, ns);
        let qname = format!("{ns}::{ty}");
        let class = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::CLASS, &qname);
        let mut nav = CodeNav::default();
        nav.record(module, "Models", "Models", node_kind::MODULE, None);
        nav.record(pkg, "Contracts", ns, node_kind::PACKAGE, Some(module));
        nav.record(class, ty, &qname, node_kind::CLASS, Some(pkg));
        let node = |id| Node { id, repo: repo_id, confidence: Confidence::Medium, cells: vec![] };
        RepoGraph {
            repo: repo_id,
            nodes: vec![node(module), node(pkg), node(class)],
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    #[test]
    fn shared_schema_sees_package_nested_types() {
        let g_a = graph_with_namespaced_type(RepoId(21), "Shop::Contracts", "OrderDto");
        let g_b = graph_with_namespaced_type(RepoId(22), "Store::Contracts", "OrderDto");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&SharedSchemaResolver);

        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_SCHEMA)
            .collect();
        assert_eq!(
            edges.len(),
            1,
            "expected exactly one cross-repo SHARES_SCHEMA edge for the namespaced DTO"
        );
    }

    #[test]
    fn shared_schema_still_sees_types_directly_under_a_module() {
        // Regression guard: the PACKAGE descent must not cost the flat shape
        // (Python / TS / Go put the type straight under the file MODULE).
        fn flat(repo_id: RepoId, qname: &str) -> RepoGraph {
            let module = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::MODULE, "models");
            let class = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::CLASS, qname);
            let mut nav = CodeNav::default();
            nav.record(module, "models", "models", node_kind::MODULE, None);
            nav.record(class, "UserModel", qname, node_kind::CLASS, Some(module));
            let node = |id| Node { id, repo: repo_id, confidence: Confidence::Strong, cells: vec![] };
            RepoGraph {
                repo: repo_id,
                nodes: vec![node(module), node(class)],
                edges: vec![],
                symbols: SymbolTable::default(),
                nav,
                unresolved_calls: vec![],
                unresolved_refs: vec![],
                properties: HashSet::new(),
            }
        }
        let mut merged =
            MergedGraph::new(vec![flat(RepoId(31), "models::UserModel"), flat(RepoId(32), "models::UserModel")]);
        merged.run(&SharedSchemaResolver);
        assert_eq!(
            merged
                .cross_edges
                .iter()
                .filter(|e| e.category == edge_category::SHARES_SCHEMA)
                .count(),
            1
        );
    }

    #[test]
    fn shared_schema_does_not_descend_two_levels() {
        // A type nested inside a type (MODULE → PACKAGE → CLASS → CLASS) is not
        // a shared-schema shape; only the one-level descent is indexed.
        fn nested(repo_id: RepoId) -> RepoGraph {
            let module = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::MODULE, "Models");
            let pkg = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::PACKAGE, "Ns");
            let outer = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::CLASS, "Ns::Outer");
            let inner =
                NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::CLASS, "Ns::Outer::InnerDto");
            let mut nav = CodeNav::default();
            nav.record(module, "Models", "Models", node_kind::MODULE, None);
            nav.record(pkg, "Ns", "Ns", node_kind::PACKAGE, Some(module));
            nav.record(outer, "Outer", "Ns::Outer", node_kind::CLASS, Some(pkg));
            nav.record(inner, "InnerDto", "Ns::Outer::InnerDto", node_kind::CLASS, Some(outer));
            let node = |id| Node { id, repo: repo_id, confidence: Confidence::Medium, cells: vec![] };
            RepoGraph {
                repo: repo_id,
                nodes: vec![node(module), node(pkg), node(outer), node(inner)],
                edges: vec![],
                symbols: SymbolTable::default(),
                nav,
                unresolved_calls: vec![],
                unresolved_refs: vec![],
                properties: HashSet::new(),
            }
        }
        let mut merged = MergedGraph::new(vec![nested(RepoId(41)), nested(RepoId(42))]);
        merged.run(&SharedSchemaResolver);
        // `Outer` carries no schema hint, so nothing is indexed at all and the
        // doubly-nested `InnerDto` must stay invisible.
        assert_eq!(
            merged
                .cross_edges
                .iter()
                .filter(|e| e.category == edge_category::SHARES_SCHEMA)
                .count(),
            0
        );
    }
}
