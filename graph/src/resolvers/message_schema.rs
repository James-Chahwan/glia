//! Message-schema resolver (A10.7) — services that declare the SAME message
//! contract (a protobuf `message` / `enum`; Avro and JSON Schema flavors once
//! A10.12 emits them).
//!
//! `MESSAGE_TYPE` nodes are per-repo islands until something joins them.
//! `SharedSchemaResolver` cannot: it only looks at CLASS / INTERFACE / STRUCT
//! children whose name carries a `Schema` / `DTO` / `Request` hint, so a proto
//! `message User` is invisible to it. This resolver indexes `MESSAGE_TYPE`
//! nodes on their FULL qname (`message:<flavor>:<qualified name>`) and pairs
//! nodes that live in different repos — `DbResolver`'s exact-qname pairwise
//! shape. The flavor segment keeps `message:proto:User` and
//! `message:avro:User` apart; the proto package keeps `shop.User` and
//! `billing.User` apart. Nothing is normalised: the qname is the contract.

use std::collections::{BTreeMap, HashSet};

use glia_code_domain::{edge_category, node_kind};
use glia_core::{Confidence, NodeId, RepoId};

use super::{CrossGraphResolver, emit_cross_repo_pairs};
use crate::merged::MergedGraph;

pub struct MessageSchemaResolver;

impl CrossGraphResolver for MessageSchemaResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        // BTreeMap, not HashMap: pairs are emitted in qname order, so the
        // emitted block — and any per-edge cell a later packet adds — never
        // depends on per-process hash order.
        let mut index: BTreeMap<String, Vec<(NodeId, RepoId, Confidence)>> = BTreeMap::new();
        let mut message_types = 0usize;
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MESSAGE_TYPE) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                message_types += 1;
                index
                    .entry(qname.clone())
                    .or_default()
                    .push((n.id, g.repo, n.confidence));
            }
        }
        let mut shared = 0usize;
        let mut pairs = 0usize;
        for refs in index.values() {
            if refs.len() < 2 {
                continue;
            }
            let repos: HashSet<RepoId> = refs.iter().map(|(_, r, _)| *r).collect();
            if repos.len() < 2 {
                continue;
            }
            shared += 1;
            // One rule (exact qname): no evidence of its own, so LC.3a's
            // engine stamp `resolver:message_schema` is the whole story.
            pairs += emit_cross_repo_pairs(
                refs,
                edge_category::SHARES_SCHEMA,
                None,
                None,
                &mut merged.cross_edges,
            );
        }
        if message_types > 0 {
            eprintln!("[schema-link] message_types={message_types} pairs={pairs} shared={shared}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RepoGraph, SymbolTable};
    use glia_code_domain::{CodeNav, GRAPH_TYPE};
    use glia_core::{Edge, Node};

    /// One repo holding MESSAGE_TYPE nodes with the given qnames. The node id
    /// is salted with `file` so a test can put two nodes with one qname in
    /// the same repo (two vendored copies in two directories of one repo).
    fn graph(repo_id: RepoId, file: &str, qnames: &[&str]) -> RepoGraph {
        let mut nav = CodeNav::default();
        let mut nodes = Vec::new();
        for q in qnames {
            let id = NodeId::from_parts(
                GRAPH_TYPE,
                repo_id,
                node_kind::MESSAGE_TYPE,
                &format!("{file}#{q}"),
            );
            let name = q.rsplit(['.', ':']).next().unwrap_or(q);
            nav.record(id, name, q, node_kind::MESSAGE_TYPE, None);
            nodes.push(Node { id, repo: repo_id, confidence: Confidence::Strong, cells: vec![] });
        }
        RepoGraph {
            repo: repo_id,
            nodes,
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    fn shares_schema(merged: &MergedGraph) -> Vec<&Edge> {
        merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_SCHEMA)
            .collect()
    }

    #[test]
    fn same_message_in_two_repos_pairs_once() {
        let a = graph(RepoId(71), "a/user.proto", &["message:proto:user.User"]);
        let b = graph(RepoId(72), "b/user.proto", &["message:proto:user.User"]);
        let mut merged = MergedGraph::new(vec![a, b]);
        merged.run(&MessageSchemaResolver);
        let edges = shares_schema(&merged);
        assert_eq!(edges.len(), 1, "one cross-repo pair for one shared qname");
        assert_eq!(edges[0].confidence, Confidence::Strong);
    }

    #[test]
    fn same_repo_duplicate_never_pairs_with_itself() {
        // Repo A holds the qname twice (two vendored copies); repo B once.
        // A-internal pairs are skipped: exactly A1-B and A2-B.
        let a1 = graph(RepoId(71), "a/one/user.proto", &["message:proto:user.User"]);
        let a2 = graph(RepoId(71), "a/two/user.proto", &["message:proto:user.User"]);
        let b = graph(RepoId(72), "b/user.proto", &["message:proto:user.User"]);
        let a_ids: HashSet<NodeId> = a1.nodes.iter().chain(&a2.nodes).map(|n| n.id).collect();
        let mut merged = MergedGraph::new(vec![a1, a2, b]);
        merged.run(&MessageSchemaResolver);
        let edges = shares_schema(&merged);
        assert_eq!(edges.len(), 2);
        for e in edges {
            assert!(
                !(a_ids.contains(&e.from) && a_ids.contains(&e.to)),
                "an A-internal SHARES_SCHEMA edge was emitted"
            );
        }
    }

    #[test]
    fn a_lone_repo_emits_nothing() {
        let a1 = graph(RepoId(71), "a/one/user.proto", &["message:proto:user.User"]);
        let a2 = graph(RepoId(71), "a/two/user.proto", &["message:proto:user.User"]);
        let mut merged = MergedGraph::new(vec![a1, a2]);
        merged.run(&MessageSchemaResolver);
        assert!(shares_schema(&merged).is_empty());
    }

    #[test]
    fn flavor_and_package_keep_same_bare_names_apart() {
        // Same bare name `User`, but a different flavor and a different proto
        // package: none of these is the same contract.
        let a = graph(RepoId(71), "a", &["message:proto:shop.User", "message:proto:billing.User"]);
        let b = graph(RepoId(72), "b", &["message:avro:shop.User", "message:proto:crm.User"]);
        let mut merged = MergedGraph::new(vec![a, b]);
        merged.run(&MessageSchemaResolver);
        assert!(shares_schema(&merged).is_empty());
    }

    #[test]
    fn only_message_type_nodes_are_indexed() {
        // A CLASS whose qname happens to spell a message qname is not a
        // declared message contract.
        let a = graph(RepoId(71), "a", &["message:proto:user.User"]);
        let mut b = graph(RepoId(72), "b", &[]);
        let cls = NodeId::from_parts(GRAPH_TYPE, RepoId(72), node_kind::CLASS, "user.User");
        b.nav.record(cls, "User", "message:proto:user.User", node_kind::CLASS, None);
        b.nodes.push(Node { id: cls, repo: RepoId(72), confidence: Confidence::Strong, cells: vec![] });
        let mut merged = MergedGraph::new(vec![a, b]);
        merged.run(&MessageSchemaResolver);
        assert!(shares_schema(&merged).is_empty());
    }

    #[test]
    fn pairs_are_emitted_in_qname_order() {
        let qs = ["message:proto:z.Last", "message:proto:a.First", "message:proto:m.Mid"];
        let mut merged =
            MergedGraph::new(vec![graph(RepoId(71), "a", &qs), graph(RepoId(72), "b", &qs)]);
        merged.run(&MessageSchemaResolver);
        let nav_a = &merged.graphs[0].nav;
        let names: Vec<String> = shares_schema(&merged)
            .iter()
            .map(|e| nav_a.qname_by_id.get(&e.from).cloned().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            ["message:proto:a.First", "message:proto:m.Mid", "message:proto:z.Last"]
        );
    }
}
