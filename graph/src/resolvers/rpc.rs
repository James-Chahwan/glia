//! RPC stack resolver — RPC call sites → the procedures they name (A10.10).
//!
//! `RPC_CALL` (`rpc_call:<path>`) pairs to `RPC_PROCEDURE` (`rpc:<path>`) on an
//! EXACT path. Three stacks emit the pair:
//!
//! - tRPC (`glia_code_extractors::trpc`): `rpc:<router>.<procedure>`;
//! - Connect and Twirp (LA.17, `grpc::extract_proto_rpc_nodes` in
//!   `glia_code_extractors`): `rpc:<proto package>.<Service>.<Method>`,
//!   the package omitted when the `.proto` declares none — Connect's own route
//!   is `/<package>.<Service>/<Method>`, so the package keeps two same-named
//!   services apart.
//!
//! All three pair here, on the exact path, and the `[trpc-link]` marker counts
//! every family's calls and procedures, not only tRPC's.
//!
//! Deliberately NOT the bidirectional substring rule `GraphQLStackResolver`
//! uses (which pairs `usequery` to `query`), and no suffix or router-prefix
//! fallback either: a call whose path names no declared procedure gets no edge.
//! The known miss is a cross-file sub-router mounted under a key other than its
//! const-derived namespace (`people: userRouter` elsewhere): the procedure reads
//! `rpc:user.*` while the client calls `people.*`. The fix for that is mount
//! resolution in the extractor, not fuzz here.
//!
//! OWNERS (LB.8b). Under a nested project root both sides carry LB.4a's
//! ` @<project path>` owner (`rpc:user.list @services/users`,
//! `rpc_call:user.list @apps/web`). They are network RPC sides like
//! GRPC_SERVER / GRPC_CLIENT, so the owner is stripped on both ends (the
//! procedure index is `build_kind_index`, owner-free since LB.8) and every
//! call pairs every same-path procedure, whichever project holds it.
//!
//! HOST NARROWING (CB.24). A project that builds its tRPC client on a literal
//! base URL (`httpBatchLink({ url: "http://catalog-svc/api/trpc" })`) has that
//! authority stamped on its RPC_CALLs as an ENDPOINT_HIT `hosts` by the
//! engine's client-host graft. A call matching two or more procedures keeps
//! only those of the project or repo the host names (`host::SideNarrowing`,
//! A11.4 / LB.4b's rule: no host, an unknown one or a named scope with no
//! procedure keeps every pair). Such a pair's evidence rule is `host`; every
//! other pair keeps the engine's emitter-only stamp. `narrowed-by-host` on the
//! `[trpc-link]` line counts calls a host narrowed.

use std::collections::HashSet;

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Edge, NodeId};

use super::host::SideNarrowing;
use super::{CrossGraphResolver, RuleTally, build_kind_index, weakest};
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
        // CB.24: a call's hosts narrow its procedures; the calls narrowed
        // (distinct ids) and the `host` rule's tally.
        let mut narrowing =
            SideNarrowing::new(&merged.graphs, node_kind::RPC_CALL, node_kind::RPC_PROCEDURE);
        let mut narrowed: HashSet<NodeId> = HashSet::new();
        let mut rules = RuleTally::new("rpc", &["host"]);
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::RPC_CALL) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(path) = split_owner(qname).0.strip_prefix("rpc_call:") else { continue };
                calls += 1;
                let Some(targets) = index.get(path) else { continue };
                paired += 1;
                let mut targets = targets.clone();
                let by_host = narrowing.narrow(n.id, &mut targets, |t| t.id);
                if by_host {
                    narrowed.insert(n.id);
                }
                for t in targets {
                    if seen.insert((n.id, t.id)) {
                        let mut edge = Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::RPC_CALLS,
                            confidence: weakest(n.confidence, t.confidence),
                            cells: Vec::new(),
                        };
                        if by_host {
                            edge.cells.push(rules.cell("host"));
                        }
                        edges.push(edge);
                    }
                }
            }
        }
        rules.report();
        // A10.10 fired_on marker, once per resolve. Silent on a build with no RPC.
        if calls > 0 || procedures > 0 {
            eprintln!(
                "[trpc-link] calls={calls} paired={paired} procedures={procedures} edges={} narrowed-by-host={}",
                edges.len(),
                narrowed.len()
            );
        }
        merged.cross_edges.extend(edges);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RepoGraph, SymbolTable};
    use glia_code_domain::{CodeNav, GRAPH_TYPE};
    use glia_core::{Confidence, Node, NodeKindId, RepoId};

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
    use glia_core::Confidence::{Medium, Strong};

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
            &[id(client, CALL, "rpc_call:user.list")],
            crate::blast::Reach::Forward,
            2,
            &glia_code_domain::profile::CODE_TABLES,
        );
        let proc_id = id(server, PROC, "rpc:user.list");
        let hit = hits.iter().find(|h| h.id == proc_id).map(|h| (h.depth, h.reason));
        assert_eq!(hit, Some((1, edge_category::RPC_CALLS)), "RPC_CALLS must carry blast radius");
    }

    /// LB.8b: an owned call reaches every owned procedure of its path, and the
    /// owner never takes part in the key.
    #[test]
    fn rpc_call_owner_is_stripped() {
        let r = RepoId::from_canonical("test://mono");
        let g = graph(
            r,
            &[
                (PROC, "rpc:user.list @services/users", Strong),
                (PROC, "rpc:user.list @services/legacy", Strong),
                (PROC, "rpc:user.byId @services/users", Strong),
                (CALL, "rpc_call:user.list @apps/web", Medium),
            ],
        );
        let mut merged = MergedGraph::new(vec![g]);
        merged.run(&RpcStackResolver);
        let call = id(r, CALL, "rpc_call:user.list @apps/web");
        assert_eq!(
            rpc_edges(&merged),
            vec![
                (call, id(r, PROC, "rpc:user.list @services/users"), Medium),
                (call, id(r, PROC, "rpc:user.list @services/legacy"), Medium),
            ],
            "one edge per same-path procedure, across owners; user.byId untouched"
        );
    }

    /// CB.24: `services/users` and `services/catalog` both serve
    /// `item.list`; the web app's tRPC client names catalog-svc (with a
    /// port), so its call keeps the catalog procedure only, with rule `host`.
    /// The LB.8b owner strip still pairs a hostless call with both.
    #[test]
    fn rpc_host_narrows_to_the_named_project() {
        use super::super::tests::{channel_graph, cross_pairs};
        use glia_code_domain::cell_type;
        use glia_code_domain::evidence::Evidence;
        use glia_core::{Cell, CellPayload};

        let mut g = channel_graph(
            "rpc-host",
            &[
                (node_kind::PROJECT, "project:services/users"),
                (node_kind::PROJECT, "project:services/catalog"),
                (PROC, "rpc:item.list @services/users"),
                (PROC, "rpc:item.list @services/catalog"),
                (CALL, "rpc_call:item.list @apps/web"),
                (CALL, "rpc_call:item.list @apps/admin"),
            ],
        );
        let ids: Vec<NodeId> = g.nodes.iter().map(|n| n.id).collect();
        for (id, label) in ids.iter().zip(["users-svc", "catalog-svc"]) {
            g.nav.name_by_id.insert(*id, label.to_string());
        }
        g.nodes[4].cells.push(Cell {
            kind: cell_type::ENDPOINT_HIT,
            payload: CellPayload::Json(r#"{"via":"rpc","hosts":["catalog-svc:3000"]}"#.into()),
        });
        let mut merged = MergedGraph::new(vec![g]);
        merged.run(&RpcStackResolver);
        let pair = |from: &str, to: &str| (format!("rpc_call:item.list @{from}"), format!("rpc:item.list @{to}"));
        assert_eq!(
            cross_pairs(&merged, edge_category::RPC_CALLS),
            vec![
                pair("apps/admin", "services/catalog"),
                pair("apps/admin", "services/users"),
                pair("apps/web", "services/catalog"),
            ]
        );
        let host_rule: Vec<bool> = merged
            .cross_edges
            .iter()
            .map(|e| Evidence::read(&e.cells).and_then(|ev| ev.rule).as_deref() == Some("host"))
            .collect();
        assert_eq!(host_rule.iter().filter(|h| **h).count(), 1, "only the narrowed pair: {host_rule:?}");
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
