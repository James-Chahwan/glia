//! `MergedGraph` — the multi-repo container plus its node-lookup surface
//! (name / qname / span resolution, subsetting, clustering).

use std::collections::{BTreeMap, HashMap, HashSet};

use repo_graph_code_domain::endpoint::split_owner;
use repo_graph_code_domain::{CodeNav, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId};

use crate::resolvers::{CrossGraphResolver, parse_endpoint_qname, weakest};
use crate::types::{RepoGraph, SymbolTable};

// ============================================================================
// Cross-graph resolution (v0.4.4b)
// ============================================================================

/// A bundle of per-repo `RepoGraph`s plus edges that cross repo boundaries.
///
/// Per-repo graphs stay owned and addressable by their `RepoId`. Cross-edges
/// sit on the merged container so the per-repo graphs remain round-trippable
/// through the v0.4.5 rkyv store without the intra-repo edge list being
/// polluted by cross-repo references that only make sense once multiple repos
/// are in scope.
#[derive(Debug, Default)]
pub struct MergedGraph {
    pub graphs: Vec<RepoGraph>,
    pub cross_edges: Vec<Edge>,
}

impl MergedGraph {
    pub fn new(graphs: Vec<RepoGraph>) -> Self {
        Self {
            graphs,
            cross_edges: Vec::new(),
        }
    }

    pub fn run<R: CrossGraphResolver>(&mut self, resolver: &R) {
        resolver.resolve(self);
    }

    /// All cross-repo edges plus each per-repo graph's intra edges. Used by
    /// consumers that want a single iterator over the whole merged graph.
    pub fn all_edges(&self) -> impl Iterator<Item = &Edge> + '_ {
        self.graphs
            .iter()
            .flat_map(|g| g.edges.iter())
            .chain(self.cross_edges.iter())
    }

    /// G20 — visit every node of `kind` across every contained repo. Iteration
    /// order is per-repo, in repo insertion order. The callback receives the
    /// `(NodeId, &Node)` pair so consumers can read cells / confidence without
    /// a second lookup.
    pub fn for_each_node_of_kind<F: FnMut(NodeId, &Node)>(&self, kind: NodeKindId, mut f: F) {
        for g in &self.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id).copied() == Some(kind) {
                    f(n.id, n);
                }
            }
        }
    }

    /// G20 — collect every node of `kind` into a `Vec<NodeId>`. Equivalent to
    /// `for_each_node_of_kind` but materialised; convenient for callers that
    /// need to hold the list while doing other graph work.
    pub fn nodes_of_kind(&self, kind: NodeKindId) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.for_each_node_of_kind(kind, |id, _| out.push(id));
        out
    }

    /// G20 — build a pre-computed kind → `Vec<NodeId>` index spanning every
    /// repo in the merged graph. Built lazily by callers that want to do many
    /// kind-scoped walks (e.g. neuropil's OpenAPI catalog touches both
    /// `ROUTE` and `ENDPOINT`); cache the return on the caller side.
    pub fn kind_index(&self) -> std::collections::HashMap<NodeKindId, Vec<NodeId>> {
        let mut out: std::collections::HashMap<NodeKindId, Vec<NodeId>> =
            std::collections::HashMap::new();
        for g in &self.graphs {
            for n in &g.nodes {
                if let Some(&kind) = g.nav.kind_by_id.get(&n.id) {
                    out.entry(kind).or_default().push(n.id);
                }
            }
        }
        out
    }

    /// Total degree (incoming + outgoing) of `id` across both intra- and
    /// cross-repo edges. The second key of [`Self::pick_primary`].
    fn degree(&self, id: NodeId) -> usize {
        self.all_edges()
            .filter(|e| e.from == id || e.to == id)
            .count()
    }

    /// Is `id` a declaration rather than a container or anchor? False for
    /// MODULE, PACKAGE, PROJECT, REGION and DOC_SPACE (and for an id no graph's
    /// nav knows); true for every other kind. The kind is read from the first
    /// graph whose nav has the id. The first key of [`Self::pick_primary`].
    fn is_declaration(&self, id: NodeId) -> bool {
        const CONTAINERS: &[NodeKindId] = &[
            node_kind::MODULE,
            node_kind::PACKAGE,
            node_kind::PROJECT,
            node_kind::REGION,
            node_kind::DOC_SPACE,
        ];
        self.graphs
            .iter()
            .find_map(|g| g.nav.kind_by_id.get(&id).copied())
            .is_some_and(|k| !CONTAINERS.contains(&k))
    }

    /// Deterministically choose the "primary" node among identically-keyed
    /// candidates (nodes sharing a simple name or a qname). Framework role
    /// overlays no longer reach here: the build-time fold (`roles`, LB.3a)
    /// merges each one into its declaration. The cases that remain are
    /// bare-name collisions (two `User`s in different modules), suffix matches
    /// across repos, and a declaration sharing its file module's qname.
    ///
    /// Rule, as one key `(is_declaration, degree, Reverse(id))`:
    /// 1. a declaration beats a MODULE / PACKAGE / PROJECT / REGION / DOC_SPACE
    ///    container — `blast-radius Foo` means the symbol, not the file that
    ///    happens to share its key, however many IMPORTS the file carries;
    /// 2. then the highest total degree — the node that participates in the
    ///    graph is what traversal, `impact` and span resolution want, not an
    ///    edgeless marker;
    /// 3. ties break on the lowest `NodeId`.
    ///
    /// Every key is stable across processes, so the choice never rides on
    /// `HashMap` iteration order — that randomness was the root of the
    /// intermittent-empty `impact` / `trace` results.
    pub fn pick_primary(&self, candidates: &[NodeId]) -> Option<NodeId> {
        match candidates {
            [] => None,
            [only] => Some(*only),
            many => many.iter().copied().max_by_key(|&id| {
                (self.is_declaration(id), self.degree(id), std::cmp::Reverse(id.0))
            }),
        }
    }

    /// Resolve a simple name (`"GroupsComponent"`) to a single `NodeId`,
    /// deterministically. When several nodes share the name, a declaration
    /// beats a container, then the highest-degree one wins (see
    /// [`Self::pick_primary`]). `None` if no node carries it.
    pub fn resolve_name(&self, name: &str) -> Option<NodeId> {
        self.pick_primary(&self.names_exact(name))
    }

    /// Every node whose simple name is exactly `name`, sorted by `NodeId` — the
    /// [`Self::qnames_exact`] twin. Use when the candidate set matters (the
    /// engine's scoped seed prefers the in-scope ones before
    /// [`Self::pick_primary`]); use [`Self::resolve_name`] for the single
    /// primary node.
    pub fn names_exact(&self, name: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        for g in &self.graphs {
            for (id, n) in &g.nav.name_by_id {
                if n == name {
                    out.push(*id);
                }
            }
        }
        out.sort_by_key(|id| id.0);
        out
    }

    /// G22 — resolve a full qualified name to a single `NodeId` across every
    /// repo. Returns `None` if no node carries this exact qname. NodeIds aren't
    /// stable across rebuilds; qnames are, so this is the canonical re-keying
    /// path for view-state persistence (e.g. `.neuropil/view_state.json`).
    ///
    /// When more than one node shares the qname (a declaration and its file
    /// module, or two repos in a merge), the pick is deterministic — see
    /// [`Self::pick_primary`].
    pub fn node_id_by_qname(&self, qname: &str) -> Option<NodeId> {
        self.pick_primary(&self.qnames_exact(qname))
    }

    /// Every node whose qname matches `qname` exactly, sorted by `NodeId` for a
    /// stable, reproducible iteration order. Use when each match matters (e.g.
    /// `impact` walks them all); use [`Self::node_id_by_qname`] for the single
    /// primary node.
    pub fn qnames_exact(&self, qname: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        for g in &self.graphs {
            for (id, qn) in &g.nav.qname_by_id {
                if qn == qname {
                    out.push(*id);
                }
            }
        }
        out.sort_by_key(|id| id.0);
        out
    }

    /// Every node whose qname *contains* `pattern`, sorted by `NodeId`. Backs
    /// the pyo3 `find_nodes_by_qname` substring search; sorting keeps the result
    /// reproducible across processes.
    pub fn qnames_containing(&self, pattern: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        for g in &self.graphs {
            for (id, qn) in &g.nav.qname_by_id {
                if qn.contains(pattern) {
                    out.push(*id);
                }
            }
        }
        out.sort_by_key(|id| id.0);
        out
    }

    /// A new `MergedGraph` containing only `keep` nodes, the edges whose both
    /// endpoints are kept, and matching nav entries — structural glue for scoped
    /// projection (WP-C / GR-3: render the top-K from `activate`, not the whole
    /// graph). `symbols` / unresolved / `properties` are dropped (rendering and
    /// read paths don't need them). Cross-benefit: any consumer can scope a view.
    pub fn subset(&self, keep: &[NodeId]) -> MergedGraph {
        let keep: HashSet<NodeId> = keep.iter().copied().collect();
        let graphs = self
            .graphs
            .iter()
            .map(|g| {
                let nodes: Vec<Node> =
                    g.nodes.iter().filter(|n| keep.contains(&n.id)).cloned().collect();
                let edges: Vec<Edge> = g
                    .edges
                    .iter()
                    .filter(|e| keep.contains(&e.from) && keep.contains(&e.to))
                    .cloned()
                    .collect();
                let mut nav = CodeNav::default();
                for (id, v) in &g.nav.name_by_id {
                    if keep.contains(id) {
                        nav.name_by_id.insert(*id, v.clone());
                    }
                }
                for (id, v) in &g.nav.qname_by_id {
                    if keep.contains(id) {
                        nav.qname_by_id.insert(*id, v.clone());
                    }
                }
                for (id, v) in &g.nav.kind_by_id {
                    if keep.contains(id) {
                        nav.kind_by_id.insert(*id, *v);
                    }
                }
                for (id, p) in &g.nav.parent_of {
                    if keep.contains(id) && keep.contains(p) {
                        nav.parent_of.insert(*id, *p);
                    }
                }
                for (id, kids) in &g.nav.children_of {
                    if keep.contains(id) {
                        let kept: Vec<NodeId> =
                            kids.iter().copied().filter(|k| keep.contains(k)).collect();
                        if !kept.is_empty() {
                            nav.children_of.insert(*id, kept);
                        }
                    }
                }
                RepoGraph {
                    repo: g.repo,
                    nodes,
                    edges,
                    nav,
                    symbols: SymbolTable::default(),
                    unresolved_calls: Vec::new(),
                    unresolved_refs: Vec::new(),
                    properties: HashSet::new(),
                }
            })
            .collect();
        let cross_edges = self
            .cross_edges
            .iter()
            .filter(|e| keep.contains(&e.from) && keep.contains(&e.to))
            .cloned()
            .collect();
        MergedGraph { graphs, cross_edges }
    }

    /// G19 — resolve an OTLP-style dotted span name (`myservice.handlers.users.list_users`)
    /// to a `NodeId`. First tries an exact match against the qname (after
    /// converting `.` to `::`); then a suffix match so spans rooted at a
    /// package the parser doesn't see still bind to the method node.
    /// When several nodes share the qname suffix, the pick is deterministic
    /// (declaration first, then highest-degree, see [`Self::pick_primary`]) —
    /// consumers that need a specific repo should still disambiguate by repo.
    pub fn resolve_span(&self, span_name: &str) -> Option<NodeId> {
        let normalised = span_name.replace('.', "::");
        if let Some(id) = self.node_id_by_qname(&normalised) {
            return Some(id);
        }
        let suffix = format!("::{normalised}");
        let mut matches = Vec::new();
        for g in &self.graphs {
            for (id, qn) in &g.nav.qname_by_id {
                if qn.ends_with(&suffix) {
                    matches.push(*id);
                }
            }
        }
        self.pick_primary(&matches)
    }
}

// ============================================================================
// Service-level view of the cross-repo edges (A9.1)
// ============================================================================

/// Mechanisms whose resolvers emit the pair once, A→B, and never B→A. Which
/// end lands in `from` depends on index order inside a `HashMap` bucket, so
/// `cross_links` normalises them to `from <= to`. Without that the output
/// flaps across processes exactly the way `impact` / `trace` did before
/// `pick_primary`.
const SYMMETRIC: &[EdgeCategoryId] = &[
    edge_category::SHARES_SCHEMA,
    edge_category::SHARES_DATA_ENTITY,
    edge_category::SHARES_CRON_SCHEDULE,
    edge_category::SHARES_CONFIG,
    edge_category::SHARES_INFRA_REF,
    edge_category::SHARES_DEPENDENCY,
];

/// One service-to-service link: every cross-repo edge running between the same
/// pair of keys, over the same mechanism, on the same channel, collapsed into a
/// single row with a count.
///
/// `from` / `to` are whatever the caller's `key_of` returns — repo id, service
/// name, directory, anything. Nothing here assumes `RepoId`, which is precisely
/// the partition key that fails for a monorepo.
///
/// Plain struct on purpose: `graph` carries no serde dependency, so shaping
/// this for the wire is the engine's job.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CrossLink {
    pub from: String,
    pub to: String,
    /// `edge_category::name` of the collapsed edges, e.g. `"HTTP_CALLS"`.
    pub mechanism: &'static str,
    /// The identifying literal the link travels over. See `channel_of`.
    pub channel: String,
    pub count: usize,
    /// Weakest confidence in the bucket — a link is only as trustworthy as its
    /// shakiest member edge.
    pub confidence: Confidence,
    pub example_from_qname: String,
    pub example_to_qname: String,
}

/// The identifying literal a link travels over: a route, a topic, a gRPC
/// service, a schema name.
///
/// Mirrors the qname-prefix vocabulary the cross-graph resolvers key on, so
/// the channel never has to be re-derived from qname prefixes outside this
/// crate. `strip_prefix` rather than `split_once` throughout, so a topic or
/// schedule that itself contains `:` survives intact.
///
/// HTTP qnames carry an LB.4a owner segment (` @<project path>`) when their
/// node sits under a nested project root. The channel is the literal the link
/// travels over, which the owner is not, so it is stripped: the endpoint
/// branch through `parse_endpoint_qname`, the `route:` branch here, and a
/// legacy `<METHOD> <path>` route through its display name, which never
/// carried one.
///
/// Total: falls back to `name`, then to `qname`, so there is no failure mode.
pub fn channel_of(qname: &str, name: &str) -> String {
    if let Some((method, path)) = parse_endpoint_qname(qname) {
        return format!("{method} {path}");
    }
    // Ordered longest-discriminator-first; `grpc_client:` cannot be shadowed
    // by `grpc:` (the fifth byte differs) but the order documents intent.
    const SUFFIX_PREFIXES: &[&str] = &[
        "queue_producer:",
        "queue_consumer:",
        "grpc_client:",
        "grpc:",
        "graphql_op:",
        "graphql_resolver:",
        "ws_client:",
        "event_emit:",
        "cli_invoke:",
        "data_entity:",
        "config:",
        "cron:",
        "infra:",
        "route:",
    ];
    for p in SUFFIX_PREFIXES {
        if let Some(rest) = qname.strip_prefix(p) {
            if *p == "route:" {
                return split_owner(rest).0.to_string();
            }
            return rest.to_string();
        }
    }
    if name.is_empty() {
        qname.to_string()
    } else {
        name.to_string()
    }
}

/// Group `merged.cross_edges` into `(from, to, mechanism, channel)` buckets.
///
/// Reads `cross_edges` only — a repo's own `g.edges` are internal wiring, not
/// service links. `key_of` decides what a "service" is; returning `None` for a
/// node drops that edge and bumps the second return value (`unplaced`), so a
/// caller can tell "no links" from "nothing was placeable".
///
/// Self-links (`from == to`) are NOT filtered here: that is presentation
/// policy and belongs to the caller.
///
/// Output is sorted and deterministic — `BTreeMap` ordering plus first-write-
/// wins example qnames plus symmetric-mechanism normalisation.
pub fn cross_links(
    merged: &MergedGraph,
    key_of: &dyn Fn(NodeId) -> Option<String>,
) -> (Vec<CrossLink>, usize) {
    // (qname, name) lookup, built by walking `g.nodes` — never by iterating
    // `nav`'s HashMaps, whose order is per-process (CODE_RULES §3).
    let mut meta: HashMap<NodeId, (&str, &str)> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            meta.entry(n.id).or_insert_with(|| {
                (
                    g.nav.qname_by_id.get(&n.id).map(String::as_str).unwrap_or(""),
                    g.nav.name_by_id.get(&n.id).map(String::as_str).unwrap_or(""),
                )
            });
        }
    }

    struct Agg {
        count: usize,
        confidence: Confidence,
        example_from: String,
        example_to: String,
    }

    let mut buckets: BTreeMap<(String, String, &'static str, String), Agg> = BTreeMap::new();
    let mut unplaced = 0usize;

    for e in &merged.cross_edges {
        let (Some(from_key), Some(to_key)) = (key_of(e.from), key_of(e.to)) else {
            unplaced += 1;
            continue;
        };
        let (fq, fname) = meta.get(&e.from).copied().unwrap_or(("", ""));
        let (tq, tname) = meta.get(&e.to).copied().unwrap_or(("", ""));
        // The channel is read off the `from` end; for every symmetric mechanism
        // the resolver paired the two nodes *because* the literal matched, so
        // both ends carry it and the choice is swap-invariant.
        let mut channel = channel_of(fq, fname);
        if channel.is_empty() {
            channel = channel_of(tq, tname);
        }
        let swap = SYMMETRIC.contains(&e.category) && from_key > to_key;
        let (from_key, to_key, example_from, example_to) = if swap {
            (to_key, from_key, tq.to_string(), fq.to_string())
        } else {
            (from_key, to_key, fq.to_string(), tq.to_string())
        };
        buckets
            .entry((from_key, to_key, edge_category::name(e.category), channel))
            .and_modify(|a| {
                a.count += 1;
                a.confidence = weakest(a.confidence, e.confidence);
            })
            .or_insert_with(|| Agg {
                count: 1,
                confidence: e.confidence,
                example_from,
                example_to,
            });
    }

    let links: Vec<CrossLink> = buckets
        .into_iter()
        .map(|((from, to, mechanism, channel), a)| CrossLink {
            from,
            to,
            mechanism,
            channel,
            count: a.count,
            confidence: a.confidence,
            example_from_qname: a.example_from,
            example_to_qname: a.example_to,
        })
        .collect();

    // fired_on marker — proves a real build reached the grouping, not just that
    // the unit tests compile.
    eprintln!(
        "[cross-links] buckets={} unplaced={} cross_edges={}",
        links.len(),
        unplaced,
        merged.cross_edges.len()
    );

    (links, unplaced)
}

/// G17 — derive a stable cluster key for `node_id` at the requested depth.
/// Splits the node's qname on `::` and returns the first `min(depth, n-1)`
/// segments rejoined. Capping at `n-1` guarantees the leaf segment never
/// lands in the key — siblings share the cluster, the node itself doesn't
/// form a singleton bucket.
///
/// Returns the empty string if `node_id` is unknown to `merged`.
pub fn cluster_key_for(node_id: NodeId, depth: usize, merged: &MergedGraph) -> String {
    let qname = match find_qname(node_id, merged) {
        Some(q) => q,
        None => return String::new(),
    };
    let parts: Vec<&str> = qname.split("::").collect();
    if parts.is_empty() {
        return String::new();
    }
    let take = depth.min(parts.len().saturating_sub(1));
    parts[..take].join("::")
}

fn find_qname(id: NodeId, merged: &MergedGraph) -> Option<String> {
    for g in &merged.graphs {
        if let Some(q) = g.nav.qname_by_id.get(&id) {
            return Some(q.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{flow_graph, repo};
    use repo_graph_code_domain::{GRAPH_TYPE, edge_category, node_kind};
    use repo_graph_core::{Confidence, RepoId};

    #[test]
    fn subset_keeps_only_requested_nodes_and_internal_edges() {
        // flow_graph: a->b->c, d->c (CALLS).
        let m = MergedGraph::new(vec![flow_graph()]);
        let a = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::a");
        let b = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::b");
        let c = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::c");

        let sub = m.subset(&[a, b]);
        let g = &sub.graphs[0];
        let ids: HashSet<NodeId> = g.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, HashSet::from([a, b]));
        // a->b kept (both in subset); b->c dropped (c excluded).
        assert_eq!(g.edges.len(), 1);
        assert!(g.edges.iter().any(|e| e.from == a && e.to == b));
        // nav filtered to the kept nodes.
        assert!(g.nav.qname_by_id.contains_key(&a));
        assert!(!g.nav.qname_by_id.contains_key(&c));
    }

    /// Reproduces the `impact`/`trace` non-determinism: a framework component
    /// where the `CLASS` carries the edges and a `COMPONENT` marker shares its
    /// name *and* qname but has none. Resolution must always land on the
    /// connected `CLASS`, regardless of `HashMap` iteration order. Hand-built,
    /// so it bypasses the build-time role fold (LB.3a) that removes such twins
    /// from real builds; `pick_primary` must stay deterministic for any
    /// same-key set, and both nodes here are declarations, so degree decides.
    fn dupe_name_graph() -> MergedGraph {
        let r = repo();
        let class = NodeId::from_parts(GRAPH_TYPE, r, node_kind::CLASS, "pkg::Dup");
        let comp = NodeId::from_parts(GRAPH_TYPE, r, node_kind::COMPONENT, "pkg::Dup");
        let run = NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "pkg::Dup::run");
        let mut nav = CodeNav::default();
        nav.record(class, "Dup", "pkg::Dup", node_kind::CLASS, None);
        nav.record(comp, "Dup", "pkg::Dup", node_kind::COMPONENT, None);
        nav.record(run, "run", "pkg::Dup::run", node_kind::METHOD, Some(class));
        let g = RepoGraph {
            repo: r,
            nodes: vec![
                Node { id: class, repo: r, confidence: Confidence::Strong, cells: vec![] },
                Node { id: comp, repo: r, confidence: Confidence::Strong, cells: vec![] },
                Node { id: run, repo: r, confidence: Confidence::Strong, cells: vec![] },
            ],
            // Only the CLASS participates in an edge; the COMPONENT is edgeless.
            edges: vec![Edge {
                from: class,
                to: run,
                category: edge_category::CONTAINS,
                confidence: Confidence::Strong,
            }],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        };
        MergedGraph::new(vec![g])
    }

    #[test]
    fn resolve_name_prefers_connected_node_over_edgeless_marker() {
        let m = dupe_name_graph();
        let class = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "pkg::Dup");
        // Both `resolve_name` (by simple name) and `node_id_by_qname` (exact
        // qname) hit the duplicate; both must choose the connected CLASS.
        assert_eq!(m.resolve_name("Dup"), Some(class));
        assert_eq!(m.node_id_by_qname("pkg::Dup"), Some(class));
    }

    #[test]
    fn pick_primary_is_order_independent() {
        let m = dupe_name_graph();
        let class = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "pkg::Dup");
        let comp = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::COMPONENT, "pkg::Dup");
        // Same candidate set in either order → same winner. This is the
        // property HashMap iteration order used to violate across processes.
        assert_eq!(m.pick_primary(&[class, comp]), Some(class));
        assert_eq!(m.pick_primary(&[comp, class]), Some(class));
        assert_eq!(m.pick_primary(&[]), None);
    }

    /// LB.3a: a declaration beats a same-key container even when the container
    /// has the higher degree. A MODULE `pkg::Dup` with three IMPORTS edges and
    /// a CLASS `pkg::Dup` with one DEFINES edge: degree alone picked the MODULE.
    #[test]
    fn pick_primary_prefers_a_declaration_over_its_file_module() {
        let r = repo();
        let module = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "pkg::Dup");
        let class = NodeId::from_parts(GRAPH_TYPE, r, node_kind::CLASS, "pkg::Dup");
        let run = NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "pkg::Dup::run");
        let deps: Vec<NodeId> = ["a", "b", "c"]
            .iter()
            .map(|q| NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, q))
            .collect();
        let mut nav = CodeNav::default();
        nav.record(module, "Dup", "pkg::Dup", node_kind::MODULE, None);
        nav.record(class, "Dup", "pkg::Dup", node_kind::CLASS, None);
        nav.record(run, "run", "pkg::Dup::run", node_kind::METHOD, Some(class));
        for (d, q) in deps.iter().zip(["a", "b", "c"]) {
            nav.record(*d, q, q, node_kind::MODULE, None);
        }
        let node = |id| Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] };
        let edge = |from, to, category| Edge { from, to, category, confidence: Confidence::Strong };
        let mut nodes = vec![node(module), node(class), node(run)];
        nodes.extend(deps.iter().map(|d| node(*d)));
        let mut edges: Vec<Edge> =
            deps.iter().map(|d| edge(module, *d, edge_category::IMPORTS)).collect();
        edges.push(edge(class, run, edge_category::DEFINES));
        let m = MergedGraph::new(vec![RepoGraph {
            repo: r,
            nodes,
            edges,
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }]);
        assert!(m.degree(module) > m.degree(class), "precondition: the MODULE has more edges");
        assert_eq!(m.pick_primary(&[module, class]), Some(class));
        assert_eq!(m.pick_primary(&[class, module]), Some(class));
        assert_eq!(m.node_id_by_qname("pkg::Dup"), Some(class));
        assert_eq!(m.resolve_name("Dup"), Some(class));
        // An id no nav knows is not a declaration: a known CLASS still wins.
        let ghost = NodeId(0);
        assert_eq!(m.pick_primary(&[ghost, class]), Some(class));
    }

    #[test]
    fn qname_matches_are_sorted_by_node_id() {
        let m = dupe_name_graph();
        let got = m.qnames_exact("pkg::Dup");
        assert_eq!(got.len(), 2, "both same-qname nodes returned");
        let mut want = got.clone();
        want.sort_by_key(|id| id.0);
        assert_eq!(got, want, "qnames_exact must return a stable id-sorted order");

        // Substring search reaches all three (Dup, Dup marker, Dup::run).
        let sub = m.qnames_containing("pkg::Dup");
        assert_eq!(sub.len(), 3);
        let mut want_sub = sub.clone();
        want_sub.sort_by_key(|id| id.0);
        assert_eq!(sub, want_sub);
    }

    // ------------------------------------------------------------------------
    // G17 / G19 / G20 / G22 — MergedGraph helpers
    // ------------------------------------------------------------------------

    fn synth_repo_graph(repo: RepoId, items: &[(&str, NodeKindId)]) -> RepoGraph {
        let mut g = RepoGraph {
            repo,
            nodes: Vec::new(),
            edges: Vec::new(),
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: HashSet::new(),
        };
        for (qname, kind) in items {
            let id = NodeId::from_parts(GRAPH_TYPE, repo, *kind, qname);
            g.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
            let leaf = qname.rsplit("::").next().unwrap_or(qname);
            g.nav.record(id, leaf, qname, *kind, None);
        }
        g
    }

    #[test]
    fn for_each_node_of_kind_walks_every_repo() {
        let r1 = RepoId::from_canonical("test://r1");
        let r2 = RepoId::from_canonical("test://r2");
        let g1 = synth_repo_graph(
            r1,
            &[
                ("a::A", node_kind::CLASS),
                ("a::B", node_kind::CLASS),
                ("a::f", node_kind::FUNCTION),
            ],
        );
        let g2 = synth_repo_graph(
            r2,
            &[
                ("b::C", node_kind::CLASS),
                ("b::g", node_kind::FUNCTION),
            ],
        );
        let merged = MergedGraph::new(vec![g1, g2]);
        let classes = merged.nodes_of_kind(node_kind::CLASS);
        assert_eq!(classes.len(), 3);
        let funcs = merged.nodes_of_kind(node_kind::FUNCTION);
        assert_eq!(funcs.len(), 2);
    }

    #[test]
    fn kind_index_groups_by_kind() {
        let r = RepoId::from_canonical("test://idx");
        let g = synth_repo_graph(
            r,
            &[
                ("m::A", node_kind::CLASS),
                ("m::B", node_kind::CLASS),
                ("m::f", node_kind::FUNCTION),
            ],
        );
        let merged = MergedGraph::new(vec![g]);
        let idx = merged.kind_index();
        assert_eq!(idx.get(&node_kind::CLASS).map(|v| v.len()), Some(2));
        assert_eq!(idx.get(&node_kind::FUNCTION).map(|v| v.len()), Some(1));
    }

    #[test]
    fn node_id_by_qname_resolves_across_repos() {
        let r1 = RepoId::from_canonical("test://r1");
        let r2 = RepoId::from_canonical("test://r2");
        let g1 = synth_repo_graph(r1, &[("a::X", node_kind::CLASS)]);
        let g2 = synth_repo_graph(r2, &[("b::Y", node_kind::CLASS)]);
        let merged = MergedGraph::new(vec![g1, g2]);
        let want_y = NodeId::from_parts(GRAPH_TYPE, r2, node_kind::CLASS, "b::Y");
        assert_eq!(merged.node_id_by_qname("b::Y"), Some(want_y));
        assert_eq!(merged.node_id_by_qname("nope"), None);
    }

    #[test]
    fn resolve_span_matches_dotted_and_suffix() {
        let r = RepoId::from_canonical("test://span");
        let g = synth_repo_graph(
            r,
            &[
                ("svc::handlers::users::list_users", node_kind::FUNCTION),
                ("other::list_users", node_kind::FUNCTION),
            ],
        );
        let merged = MergedGraph::new(vec![g]);
        let exact = merged.resolve_span("svc.handlers.users.list_users");
        assert!(exact.is_some());
        let suffix = merged.resolve_span("handlers.users.list_users");
        assert!(suffix.is_some());
        let miss = merged.resolve_span("nonexistent");
        assert_eq!(miss, None);
    }

    // ------------------------------------------------------------------
    // A9.1 — cross_links / channel_of
    // ------------------------------------------------------------------

    /// Map every node of `g` to `key`; anything unknown stays `None`.
    fn keys_for(pairs: &[(&RepoGraph, &str)]) -> std::collections::HashMap<NodeId, String> {
        let mut m = std::collections::HashMap::new();
        for (g, key) in pairs {
            for n in &g.nodes {
                m.insert(n.id, (*key).to_string());
            }
        }
        m
    }

    fn xedge(from: NodeId, to: NodeId, category: repo_graph_core::EdgeCategoryId) -> Edge {
        Edge { from, to, category, confidence: Confidence::Strong }
    }

    fn id_of(repo: RepoId, kind: NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, kind, qname)
    }

    #[test]
    fn channel_of_reads_every_resolver_prefix() {
        assert_eq!(channel_of("endpoint:GET:/users", "GET /users"), "GET /users");
        // Method is upper-cased by parse_endpoint_qname, path kept verbatim.
        assert_eq!(channel_of("endpoint:get:/users/:id", ""), "GET /users/:id");
        assert_eq!(channel_of("queue_producer:orders.v2", "kafka"), "orders.v2");
        assert_eq!(channel_of("queue_consumer:orders.v2", "kafka"), "orders.v2");
        assert_eq!(channel_of("grpc_client:Greeter", "Greeter"), "Greeter");
        assert_eq!(channel_of("grpc:Greeter", "Greeter"), "Greeter");
        assert_eq!(channel_of("graphql_op:getUser", "getUser"), "getUser");
        assert_eq!(channel_of("graphql_resolver:getUser", "getUser"), "getUser");
        assert_eq!(channel_of("ws_client:/notifications", "ws"), "/notifications");
        assert_eq!(channel_of("event_emit:user.created", "emit"), "user.created");
        assert_eq!(channel_of("cli_invoke:terraform", "run"), "terraform");
        assert_eq!(channel_of("data_entity:sql:users", "users"), "sql:users");
        assert_eq!(channel_of("config:env:DATABASE_URL", "DATABASE_URL"), "env:DATABASE_URL");
        assert_eq!(channel_of("infra:image:api", "api"), "image:api");
        assert_eq!(channel_of("route:/users", "/users"), "/users");
        // LB.4a: the owner segment is not part of the channel.
        assert_eq!(channel_of("route:/users @services/api", "/users"), "/users");
        assert_eq!(channel_of("endpoint:POST:/users @web", "POST /users"), "POST /users");
        assert_eq!(channel_of("GET /users @api", "GET /users"), "GET /users");
        // strip_prefix, not split_once — a schedule full of colons survives.
        assert_eq!(channel_of("cron:0 4 * * *:cleanup", "cleanup"), "0 4 * * *:cleanup");
        // Fallbacks: name, then qname.
        assert_eq!(channel_of("UserDto", "UserDto"), "UserDto");
        assert_eq!(channel_of("a::b::UserDto", ""), "a::b::UserDto");
    }

    #[test]
    fn cross_links_collapses_same_channel() {
        let ra = RepoId::from_canonical("test://cl-a");
        let rb = RepoId::from_canonical("test://cl-b");
        let g_a = synth_repo_graph(ra, &[("endpoint:GET:/users", node_kind::ENDPOINT)]);
        let g_b = synth_repo_graph(
            rb,
            &[("route:/users", node_kind::ROUTE), ("route:/users/", node_kind::ROUTE)],
        );
        let keys = keys_for(&[(&g_a, "a"), (&g_b, "b")]);
        let ep = id_of(ra, node_kind::ENDPOINT, "endpoint:GET:/users");
        let r1 = id_of(rb, node_kind::ROUTE, "route:/users");
        let r2 = id_of(rb, node_kind::ROUTE, "route:/users/");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.cross_edges = vec![
            xedge(ep, r1, edge_category::HTTP_CALLS),
            xedge(ep, r2, edge_category::HTTP_CALLS),
        ];

        let (links, unplaced) = cross_links(&merged, &|id| keys.get(&id).cloned());
        assert_eq!(unplaced, 0);
        assert_eq!(links.len(), 1, "same (from,to,mechanism,channel) must collapse");
        assert_eq!(links[0].from, "a");
        assert_eq!(links[0].to, "b");
        assert_eq!(links[0].mechanism, "HTTP_CALLS");
        assert_eq!(links[0].channel, "GET /users");
        assert_eq!(links[0].count, 2);
        assert_eq!(links[0].confidence, Confidence::Strong);
        assert_eq!(links[0].example_from_qname, "endpoint:GET:/users");
        assert_eq!(links[0].example_to_qname, "route:/users");
    }

    #[test]
    fn cross_links_splits_distinct_channels() {
        let ra = RepoId::from_canonical("test://cl2-a");
        let rb = RepoId::from_canonical("test://cl2-b");
        let g_a = synth_repo_graph(
            ra,
            &[
                ("endpoint:POST:/users", node_kind::ENDPOINT),
                ("endpoint:GET:/users", node_kind::ENDPOINT),
            ],
        );
        let g_b = synth_repo_graph(rb, &[("route:/users", node_kind::ROUTE)]);
        let keys = keys_for(&[(&g_a, "a"), (&g_b, "b")]);
        let post = id_of(ra, node_kind::ENDPOINT, "endpoint:POST:/users");
        let get = id_of(ra, node_kind::ENDPOINT, "endpoint:GET:/users");
        let route = id_of(rb, node_kind::ROUTE, "route:/users");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        // POST first on the wire; the BTreeMap must still sort GET before POST.
        merged.cross_edges = vec![
            xedge(post, route, edge_category::HTTP_CALLS),
            xedge(get, route, edge_category::HTTP_CALLS),
        ];

        let (links, unplaced) = cross_links(&merged, &|id| keys.get(&id).cloned());
        assert_eq!(unplaced, 0);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].channel, "GET /users");
        assert_eq!(links[1].channel, "POST /users");
        assert!(links.iter().all(|l| l.count == 1));
    }

    #[test]
    fn cross_links_normalises_symmetric_and_counts_unplaced() {
        let ra = RepoId::from_canonical("test://cl3-a");
        let rb = RepoId::from_canonical("test://cl3-b");
        let g_a = synth_repo_graph(ra, &[("svc_a::UserDto", node_kind::CLASS)]);
        let g_b = synth_repo_graph(rb, &[("svc_b::UserDto", node_kind::CLASS)]);
        let keys = keys_for(&[(&g_a, "a"), (&g_b, "b")]);
        let a_dto = id_of(ra, node_kind::CLASS, "svc_a::UserDto");
        let b_dto = id_of(rb, node_kind::CLASS, "svc_b::UserDto");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.cross_edges = vec![
            // Emitted b -> a by the resolver; must land a -> b.
            xedge(b_dto, a_dto, edge_category::SHARES_SCHEMA),
            // `from` is unknown to key_of -> dropped, counted.
            xedge(NodeId(0xdead_beef), a_dto, edge_category::SHARES_SCHEMA),
        ];

        let (links, unplaced) = cross_links(&merged, &|id| keys.get(&id).cloned());
        assert_eq!(unplaced, 1);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].from, "a");
        assert_eq!(links[0].to, "b");
        assert_eq!(links[0].mechanism, "SHARES_SCHEMA");
        // Example qnames swap with the keys, so from/to stay consistent.
        assert_eq!(links[0].example_from_qname, "svc_a::UserDto");
        assert_eq!(links[0].example_to_qname, "svc_b::UserDto");
        // Channel is the shared literal; both ends carry it.
        assert_eq!(links[0].channel, "UserDto");
    }

    #[test]
    fn cross_links_folds_confidence_to_weakest_and_keeps_self_links() {
        let r = RepoId::from_canonical("test://cl4");
        let g = synth_repo_graph(
            r,
            &[
                ("queue_producer:orders.v2", node_kind::CLASS),
                ("queue_consumer:orders.v2", node_kind::CLASS),
                ("queue_consumer:orders.v2.dlq", node_kind::CLASS),
            ],
        );
        let keys = keys_for(&[(&g, "mono")]);
        let prod = id_of(r, node_kind::CLASS, "queue_producer:orders.v2");
        let c1 = id_of(r, node_kind::CLASS, "queue_consumer:orders.v2");
        let c2 = id_of(r, node_kind::CLASS, "queue_consumer:orders.v2.dlq");
        let mut merged = MergedGraph::new(vec![g]);
        merged.cross_edges = vec![
            xedge(prod, c1, edge_category::QUEUE_FLOWS),
            Edge {
                from: prod,
                to: c2,
                category: edge_category::QUEUE_FLOWS,
                confidence: Confidence::Weak,
            },
        ];

        let (links, unplaced) = cross_links(&merged, &|id| keys.get(&id).cloned());
        assert_eq!(unplaced, 0);
        // Self-link (mono -> mono) is NOT filtered — that is caller policy.
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].from, "mono");
        assert_eq!(links[0].to, "mono");
        assert_eq!(links[0].channel, "orders.v2");
        assert_eq!(links[0].count, 2);
        assert_eq!(links[0].confidence, Confidence::Weak, "weakest member wins");
    }

    #[test]
    fn cluster_key_for_drops_leaf_segment() {
        let r = RepoId::from_canonical("test://ck");
        let g = synth_repo_graph(
            r,
            &[
                ("svc::users::repo::find_one", node_kind::METHOD),
                ("svc::users::repo", node_kind::CLASS),
                ("solo", node_kind::MODULE),
            ],
        );
        let merged = MergedGraph::new(vec![g]);
        let find_one = NodeId::from_parts(
            GRAPH_TYPE,
            r,
            node_kind::METHOD,
            "svc::users::repo::find_one",
        );
        assert_eq!(cluster_key_for(find_one, 0, &merged), "");
        assert_eq!(cluster_key_for(find_one, 1, &merged), "svc");
        assert_eq!(cluster_key_for(find_one, 2, &merged), "svc::users");
        // Depth caps at parts-1; leaf never lands in the key.
        assert_eq!(
            cluster_key_for(find_one, 99, &merged),
            "svc::users::repo"
        );
        // Single-segment qname → empty key (no parent cluster).
        let solo = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "solo");
        assert_eq!(cluster_key_for(solo, 1, &merged), "");
        // Unknown node id → empty.
        assert_eq!(cluster_key_for(NodeId(0xdeadbeef), 1, &merged), "");
    }
}
