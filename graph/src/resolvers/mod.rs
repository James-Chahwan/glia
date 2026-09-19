//! Cross-graph resolvers — one module per mechanism. Each owns its own
//! matching rule and emits the cross-repo edges for that mechanism; the
//! shared index builder, target type, confidence helper and cross-repo pair
//! emitter live here.

use std::collections::HashMap;

use repo_graph_code_domain::endpoint::split_owner;
use repo_graph_code_domain::evidence::Evidence;
use repo_graph_core::{Cell, Confidence, Edge, EdgeCategoryId, NodeId, NodeKindId, RepoId};

use crate::merged::MergedGraph;
use crate::types::RepoGraph;

mod http;
mod grpc;
mod rpc;
mod queue;
mod graphql;
mod websocket;
mod eventbus;
mod shared_schema;
mod db;
mod message_schema;
mod cron;
mod config;
mod iac;
mod package;
mod cli;

pub use http::HttpStackResolver;
pub use grpc::GrpcStackResolver;
pub use rpc::RpcStackResolver;
pub use queue::QueueStackResolver;
pub use graphql::GraphQLStackResolver;
pub use websocket::WebSocketStackResolver;
pub use eventbus::EventBusResolver;
pub use shared_schema::SharedSchemaResolver;
pub use db::DbResolver;
pub use message_schema::MessageSchemaResolver;
pub use cron::CronResolver;
pub use config::ConfigResolver;
pub use iac::IacResolver;
pub use package::PackageResolver;
pub use cli::CliInvocationResolver;
pub use http::normalise_http_path;
// A10.2 — the route index + tiers 1-4 for passes that pair a DECLARED path
// (a contract operation) with the ROUTE serving it.
pub use http::{HttpRouteMatcher, RouteMatch};
// LF.2d — gateway route mounts from `.glia/overlay.toml` `[[route_prefix]]`,
// and the http resolver bound to them (the engine's http Resolve pass).
pub use http::{MountedHttpResolver, RouteMounts};
// A9.1 — `cross_links` reads the HTTP channel label off the same parse the
// resolver uses; crate-internal only, the public surface is the facade.
pub(crate) use http::parse_endpoint_qname;

/// Emits edges that cross `RepoGraph` boundaries. v0.4.10 will add
/// `GraphQLResolver`, `GrpcResolver`, `QueueResolver`, etc. against the same
/// trait. Each resolver owns its own matching rule — path normalisation,
/// schema-name matching, queue-topic matching, etc.
pub trait CrossGraphResolver {
    fn resolve(&self, merged: &mut MergedGraph);
}

/// Index entry for the name-keyed resolvers (gRPC services, queue topics,
/// GraphQL resolvers, WS handlers, event handlers, CLI commands).
#[derive(Clone, Copy)]
struct ServiceTarget {
    id: NodeId,
    confidence: Confidence,
}

// ============================================================================
// Shared index builder
// ============================================================================

/// `kind` nodes keyed by their qname minus `prefix` and minus the LB.8 owner
/// segment (`graphql_resolver:getUser @services/users` keys as `getUser`), so
/// every owner of one channel lands under the channel's key. Kinds that are
/// never owned carry no segment and key exactly as before.
fn build_kind_index(
    graphs: &[RepoGraph],
    kind: NodeKindId,
    prefix: &str,
) -> HashMap<String, Vec<ServiceTarget>> {
    let mut index: HashMap<String, Vec<ServiceTarget>> = HashMap::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&kind) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(stripped) = qname.strip_prefix(prefix) else { continue };
            let key = split_owner(stripped).0;
            index
                .entry(key.to_string())
                .or_default()
                .push(ServiceTarget {
                    id: n.id,
                    confidence: n.confidence,
                });
        }
    }
    index
}

pub(crate) fn weakest(a: Confidence, b: Confidence) -> Confidence {
    fn rank(c: Confidence) -> u8 {
        match c {
            Confidence::Strong => 2,
            Confidence::Medium => 1,
            Confidence::Weak => 0,
        }
    }
    if rank(a) <= rank(b) { a } else { b }
}

/// Emit one edge per cross-repo pair in `refs`, returning how many were added.
/// Same-repo pairs are skipped: a repo's own duplicates are not a cross-service
/// join. `confidence: None` means `weakest(a, b)`; `Some(c)` forces `c` (the
/// DB provider pass forces `Weak` — see [`DbResolver::resolve`]). Shared by
/// the exact-qname pairwise resolvers (`DbResolver`, `MessageSchemaResolver`).
///
/// LC.3c: `ev` is the rule evidence every emitted edge carries (the DB entity
/// and provider passes); `None` leaves the edges bare for LC.3a's
/// emitter-only stamp in the engine (`MessageSchemaResolver`, one rule).
fn emit_cross_repo_pairs(
    refs: &[(NodeId, RepoId, Confidence)],
    category: EdgeCategoryId,
    confidence: Option<Confidence>,
    ev: Option<&Evidence>,
    out: &mut Vec<Edge>,
) -> usize {
    let cell = ev.map(Evidence::to_cell);
    let mut emitted = 0;
    for i in 0..refs.len() {
        for j in (i + 1)..refs.len() {
            if refs[i].1 == refs[j].1 {
                continue;
            }
            let conf = confidence.unwrap_or_else(|| weakest(refs[i].2, refs[j].2));
            let mut edge = Edge::new(refs[i].0, refs[j].0, category, conf);
            if let Some(c) = &cell {
                edge = edge.with_cell(c.clone());
            }
            out.push(edge);
            emitted += 1;
        }
    }
    emitted
}

// ============================================================================
// LC.3c — rule evidence
// ============================================================================

/// The evidence a tiered resolver attaches to a cross edge it emitted:
/// `resolver:<resolver>` plus the rule (tier, pass or branch) that paired it.
/// `resolver` is the pass name the engine's Resolve stage stamps
/// (`engine/src/profile.rs` `CODE_PASSES`, `resolver!`), so an edge reads the same
/// emitter whether the resolver or the stamp attached it; the stamp never
/// overrides an evidence already present. No location: the engine's fill
/// pass places it from the edge's endpoints.
pub(crate) fn rule_evidence(resolver: &str, rule: &str) -> Evidence {
    Evidence::emitter(format!("resolver:{resolver}")).rule(rule)
}

/// One rule of a [`RuleTally`]: how many edges it paired, and its EVIDENCE
/// cell, serialised once.
struct RuleCount {
    rule: &'static str,
    n: usize,
    cell: Option<Cell>,
}

/// Per-resolve tally behind the `[evidence-rules]` fired_on marker, and the
/// source of each edge's rule evidence cell.
///
/// The marker is one line per resolve, printed only when the resolver emitted
/// at least one edge, rules in the resolver's fixed order:
///   `[evidence-rules] resolver=http exact=3 endpoint_prefix=0 any=1 route_prefix=0 base_fold=0 suffix=1`
/// grep: `... 2>&1 | grep '^\[evidence-rules\] resolver='`
pub(crate) struct RuleTally {
    resolver: &'static str,
    rules: Vec<RuleCount>,
}

impl RuleTally {
    /// A zeroed tally for `resolver`, reporting `order` in that order.
    pub(crate) fn new(resolver: &'static str, order: &[&'static str]) -> Self {
        Self {
            resolver,
            rules: order
                .iter()
                .map(|&rule| RuleCount {
                    rule,
                    n: 0,
                    cell: None,
                })
                .collect(),
        }
    }

    /// The slot of `rule`. A rule outside the declared order is appended
    /// rather than lost, so the marker still accounts for every edge.
    fn slot(&mut self, rule: &'static str) -> &mut RuleCount {
        let i = match self.rules.iter().position(|r| r.rule == rule) {
            Some(i) => i,
            None => {
                self.rules.push(RuleCount {
                    rule,
                    n: 0,
                    cell: None,
                });
                self.rules.len() - 1
            }
        };
        &mut self.rules[i]
    }

    /// Count one edge paired by `rule`, returning the EVIDENCE cell it
    /// carries ([`rule_evidence`]).
    pub(crate) fn cell(&mut self, rule: &'static str) -> Cell {
        let resolver = self.resolver;
        let slot = self.slot(rule);
        slot.n += 1;
        slot.cell
            .get_or_insert_with(|| rule_evidence(resolver, rule).to_cell())
            .clone()
    }

    /// The evidence for `rule`, for a helper that stamps edges itself
    /// ([`emit_cross_repo_pairs`]); count them with [`RuleTally::add`].
    pub(crate) fn evidence(&self, rule: &'static str) -> Evidence {
        rule_evidence(self.resolver, rule)
    }

    /// Count `n` edges paired by `rule` and stamped elsewhere.
    pub(crate) fn add(&mut self, rule: &'static str, n: usize) {
        self.slot(rule).n += n;
    }

    /// The `[evidence-rules]` line, or `None` when no edge was emitted.
    fn line(&self) -> Option<String> {
        if self.rules.iter().all(|r| r.n == 0) {
            return None;
        }
        let counts: Vec<String> = self
            .rules
            .iter()
            .map(|r| format!("{}={}", r.rule, r.n))
            .collect();
        Some(format!(
            "[evidence-rules] resolver={} {}",
            self.resolver,
            counts.join(" ")
        ))
    }

    /// Print the marker (see [`RuleTally`]); silent with no edge.
    pub(crate) fn report(&self) {
        if let Some(line) = self.line() {
            eprintln!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
    use repo_graph_core::{Confidence, Node};

    /// One `RepoGraph` of bare nodes `(kind, qname)` in repo `tag`, display
    /// name = qname. For the resolver modules' owner-segment tests.
    pub(super) fn channel_graph(tag: &str, nodes: &[(NodeKindId, &str)]) -> RepoGraph {
        let repo = RepoId::from_canonical(&format!("test://{tag}"));
        let mut nav = CodeNav::default();
        let mut out = Vec::new();
        for (kind, q) in nodes {
            let id = NodeId::from_parts(GRAPH_TYPE, repo, *kind, q);
            nav.record(id, q, q, *kind, None);
            out.push(Node { id, repo, confidence: Confidence::Strong, cells: vec![] });
        }
        RepoGraph {
            repo,
            nodes: out,
            edges: vec![],
            symbols: Default::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        }
    }

    /// `(from qname, to qname)` of every `category` cross edge, sorted.
    pub(super) fn cross_pairs(m: &MergedGraph, category: EdgeCategoryId) -> Vec<(String, String)> {
        let q = |id: NodeId| {
            m.graphs.iter().find_map(|g| g.nav.qname_by_id.get(&id).cloned()).unwrap_or_default()
        };
        let mut out: Vec<(String, String)> = m
            .cross_edges
            .iter()
            .filter(|e| e.category == category)
            .map(|e| (q(e.from), q(e.to)))
            .collect();
        out.sort();
        out
    }

    /// LB.8: an owner-qualified qname keys under its bare channel, so both
    /// owners of `getUser` share one index entry; an owner-free one is as
    /// before.
    #[test]
    fn build_kind_index_keys_by_the_owner_free_name() {
        let g = channel_graph(
            "kind-index",
            &[
                (node_kind::GRAPHQL_RESOLVER, "graphql_resolver:getUser @services/users"),
                (node_kind::GRAPHQL_RESOLVER, "graphql_resolver:getUser @services/catalog"),
                (node_kind::GRAPHQL_RESOLVER, "graphql_resolver:listUsers"),
            ],
        );
        let index = build_kind_index(&[g], node_kind::GRAPHQL_RESOLVER, "graphql_resolver:");
        let mut keys: Vec<(&str, usize)> = index.iter().map(|(k, v)| (k.as_str(), v.len())).collect();
        keys.sort();
        assert_eq!(keys, [("getUser", 2), ("listUsers", 1)]);
    }

    /// LC.3c: the marker is silent with no edge, lists every rule in the
    /// declared order (zeroes included), and never loses an undeclared rule.
    #[test]
    fn rule_tally_marker_and_cells() {
        let mut t = RuleTally::new("http", &["exact", "suffix"]);
        assert_eq!(t.line(), None, "silent with no edge");
        t.add("exact", 0);
        assert_eq!(t.line(), None, "a zero add is still no edge");

        let cell = t.cell("suffix");
        assert_eq!(
            cell.payload,
            repo_graph_core::CellPayload::Json(
                r#"{"emitter":"resolver:http","rule":"suffix","basis":"none"}"#.to_string()
            )
        );
        assert_eq!(t.cell("suffix"), cell, "one cell per rule, reused");
        t.add("exact", 3);
        assert_eq!(
            t.line().as_deref(),
            Some("[evidence-rules] resolver=http exact=3 suffix=2")
        );
        t.add("other", 1);
        assert_eq!(
            t.line().as_deref(),
            Some("[evidence-rules] resolver=http exact=3 suffix=2 other=1")
        );
        assert_eq!(t.evidence("exact"), rule_evidence("http", "exact"));
        assert_eq!(
            rule_evidence("db", "provider"),
            Evidence::emitter("resolver:db").rule("provider")
        );
    }

    #[test]
    fn weakest_confidence_is_min_rank() {
        assert_eq!(weakest(Confidence::Strong, Confidence::Strong), Confidence::Strong);
        assert_eq!(weakest(Confidence::Strong, Confidence::Medium), Confidence::Medium);
        assert_eq!(weakest(Confidence::Medium, Confidence::Weak), Confidence::Weak);
        assert_eq!(weakest(Confidence::Weak, Confidence::Strong), Confidence::Weak);
    }
}
