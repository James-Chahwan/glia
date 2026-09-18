//! RPC stack resolver — RPC call sites → the procedures they name (A10.10).
//!
//! `RPC_CALL` (`rpc_call:<router>.<procedure>`) pairs to `RPC_PROCEDURE`
//! (`rpc:<router>.<procedure>`) on an EXACT procedure path. tRPC is the only
//! emitter today (`repo_graph_code_extractors::trpc`); Connect / Twirp reuse the
//! category once their extraction lands, keeping the `rpc:<service>.<method>`
//! qname convention.
//!
//! Deliberately NOT the bidirectional substring rule `GraphQLStackResolver`
//! uses (which pairs `usequery` to `query`), and no suffix or router-prefix
//! fallback either: a call whose path names no declared procedure gets no edge.
//! The known miss is a cross-file sub-router mounted under a key other than its
//! const-derived namespace (`people: userRouter` elsewhere): the procedure reads
//! `rpc:user.*` while the client calls `people.*`. The fix for that is mount
//! resolution in the extractor, not fuzz here.

use std::collections::HashSet;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Edge, NodeId};

use super::{CrossGraphResolver, build_kind_index, weakest};
use crate::merged::MergedGraph;

pub struct RpcStackResolver;

impl CrossGraphResolver for RpcStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let index = build_kind_index(&merged.graphs, node_kind::RPC_PROCEDURE, "rpc:");
        let procedures: usize = index.values().map(Vec::len).sum();
        let (mut calls, mut paired) = (0usize, 0usize);
        // A node id shared by two graphs indexes twice; one edge per pair.
        let mut seen: HashSet<(NodeId, NodeId)> = HashSet::new();
        let mut edges = Vec::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::RPC_CALL) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(path) = qname.strip_prefix("rpc_call:") else { continue };
                calls += 1;
                let Some(targets) = index.get(path) else { continue };
                paired += 1;
                for t in targets {
                    if seen.insert((n.id, t.id)) {
                        edges.push(Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::RPC_CALLS,
                            confidence: weakest(n.confidence, t.confidence),
                        });
                    }
                }
            }
        }
        // A10.10 fired_on marker, once per resolve. Silent on a build with no RPC.
        if calls > 0 || procedures > 0 {
            eprintln!(
                "[trpc-link] calls={calls} paired={paired} procedures={procedures} edges={}",
                edges.len()
            );
        }
        merged.cross_edges.extend(edges);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RepoGraph, SymbolTable};
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE};
    use repo_graph_core::{Confidence, Node, NodeKindId, RepoId};

    /// One repo holding `(kind, qname, confidence)` nodes, nav name = the path.
    fn graph(repo: RepoId, nodes: &[(NodeKindId, &str, Confidence)]) -> RepoGraph {
        let mut nav = CodeNav::default();
        let mut out = Vec::new();
        for &(kind, qname, confidence) in nodes {
            let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, qname);
            let name = qname.split_once(':').map_or(qname, |(_, p)| p);
            nav.record(id, name, qname, kind, None);
            out.push(Node { id, repo, confidence, cells: vec![] });
        }
        RepoGraph {
            repo,
            nodes: out,
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    fn id(repo: RepoId, kind: NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, kind, qname)
    }

    fn rpc_edges(merged: &MergedGraph) -> Vec<(NodeId, NodeId, Confidence)> {
        merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::RPC_CALLS)
            .map(|e| (e.from, e.to, e.confidence))
            .collect()
    }

    const PROC: NodeKindId = node_kind::RPC_PROCEDURE;
    const CALL: NodeKindId = node_kind::RPC_CALL;
    use repo_graph_core::Confidence::{Medium, Strong};

    #[test]
    fn pairs_a_call_to_its_procedure_across_repos() {
        let (server, client) = (RepoId::from_canonical("test://srv"), RepoId::from_canonical("test://web"));
        let g_srv = graph(server, &[(PROC, "rpc:user.list", Strong), (PROC, "rpc:user.byId", Strong)]);
        let g_web = graph(client, &[(CALL, "rpc_call:user.list", Medium)]);
        let mut merged = MergedGraph::new(vec![g_srv, g_web]);
        merged.run(&RpcStackResolver);
        assert_eq!(
            rpc_edges(&merged),
            vec![(
                id(client, CALL, "rpc_call:user.list"),
                id(server, PROC, "rpc:user.list"),
                Medium, // weakest of the two ends
            )],
            "exactly one call -> procedure edge; the sibling `user.byId` is untouched",
        );
    }

    #[test]
    fn exact_path_only_no_prefix_suffix_or_substring_fallback() {
        let (server, client) = (RepoId::from_canonical("test://srv"), RepoId::from_canonical("test://web"));
        let g_srv = graph(server, &[(PROC, "rpc:user.list", Strong)]);
        let g_web = graph(
            client,
            &[
                (CALL, "rpc_call:user.missing", Strong), // same router, unknown procedure
                (CALL, "rpc_call:user", Strong),         // router prefix of a real path
                (CALL, "rpc_call:list", Strong),         // bare suffix of a real path
                (CALL, "rpc_call:admin.user.list", Strong), // real path as a suffix
            ],
        );
        let mut merged = MergedGraph::new(vec![g_srv, g_web]);
        merged.run(&RpcStackResolver);
        assert!(rpc_edges(&merged).is_empty(), "{:?}", rpc_edges(&merged));
    }

    #[test]
    fn blast_radius_crosses_the_rpc_hop() {
        let (server, client) = (RepoId::from_canonical("test://srv"), RepoId::from_canonical("test://web"));
        let g_srv = graph(server, &[(PROC, "rpc:user.list", Strong)]);
        let g_web = graph(client, &[(CALL, "rpc_call:user.list", Strong)]);
        let mut merged = MergedGraph::new(vec![g_srv, g_web]);
        merged.run(&RpcStackResolver);
        let hits = merged.blast_radius(
            id(client, CALL, "rpc_call:user.list"),
            crate::blast::Reach::Forward,
            2,
            None,
        );
        let proc_id = id(server, PROC, "rpc:user.list");
        let hit = hits.iter().find(|h| h.id == proc_id).map(|h| (h.depth, h.reason));
        assert_eq!(hit, Some((1, edge_category::RPC_CALLS)), "RPC_CALLS must carry blast radius");
    }

    #[test]
    fn pairs_within_one_repo_and_emits_one_edge_per_pair() {
        // A T3 app keeps router and page in one repo; the same procedure id
        // appearing in two graphs of that repo must still yield one edge.
        let r = RepoId::from_canonical("test://t3");
        let g1 = graph(r, &[(PROC, "rpc:post.create", Strong), (CALL, "rpc_call:post.create", Strong)]);
        let g2 = graph(r, &[(PROC, "rpc:post.create", Strong)]);
        let mut merged = MergedGraph::new(vec![g1, g2]);
        merged.run(&RpcStackResolver);
        assert_eq!(
            rpc_edges(&merged),
            vec![(id(r, CALL, "rpc_call:post.create"), id(r, PROC, "rpc:post.create"), Strong)],
        );
    }
}
