//! **cycles** (LE.6b): the loops that take production down, then the import
//! cycles that make a codebase hard to cut.
//!
//! Three passes, each over its own index so a module import never counts as
//! a runtime path:
//!
//! 1. **Node-level service loops** (kind `event`). The strongly-connected
//!    components of the flow graph ([`FLOW_CAUSAL`]: the cross-service
//!    mechanisms plus the HANDLED_BY / CALLS / USES / INJECTS glue that carries
//!    a handler to the next publish). A component whose members sit in two or
//!    more `glia arch` services is a loop. Its witness is the shortest cycle
//!    through its best crossing edge (one joining two services: a queue /
//!    event hop first, then another cross-service mechanism, then any), so the
//!    witness itself leaves a service and comes back: `event_loop` when it
//!    rides a QUEUE_FLOWS / EVENT_FLOWS hop, else `call_loop` (synchronous
//!    cross-service recursion). Tier `derived`: every hop is an observed edge,
//!    only the loop is computed.
//! 2. **Service-level possible loops** (kind `event`). The QUEUE_FLOWS /
//!    EVENT_FLOWS links of [`service_map_with`] as a service graph; a service
//!    component no node-level loop already covers (same service set) is a
//!    `possible_loop`, tier `heuristic`: each service publishes to and
//!    consumes from the others, but no handler-to-producer path is in the
//!    graph. The witness is the service links, located at their producers.
//! 3. **Import cycles** (kind `import`). [`module_import_graph`] projects every
//!    IMPORTS edge onto the modules at its two ends; each component is an
//!    `import_cycle` located at the import sites (the edge's EVIDENCE line,
//!    LC.3b).
//!
//! Services are keyed exactly as `glia arch` keys them ([`default_keying`] +
//! [`service_of`] over the node's located file, the repo labels the build
//! recorded), so a single repo's `orders/` and `billing/` are two services.
//! Every row is located through one [`Locator`]; lines are 1-based.
//!
//! Fired-on marker, one line per call:
//! `[cycles] event_loops=<E> call_loops=<C> possible=<P> import_cycles=<I> (sccs=<K> nodes=<N>)`,
//! where `sccs` / `nodes` count the non-trivial components of the node-level
//! indexes that ran (flow and module-import) and their members, before the
//! service and scope filters.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use repo_graph_activation::algo::cycles::{strongly_connected, witness_cycle};
use repo_graph_activation::algo::reach::bfs;
use repo_graph_activation::algo::{Adjacency, CategorySet, GraphSource, Walk};
use repo_graph_code_domain::evidence::Evidence;
use repo_graph_code_domain::{edge_category, endpoint, node_kind};
use repo_graph_core::{Confidence, Edge, EdgeCategoryId, NodeId};
use repo_graph_graph::{MergedGraph, channel_of};

use crate::answers::{Locator, in_scope, resolve_scope};
use crate::arch::{
    FLOW_MECHANISMS, ServiceKeying, ServiceLink, default_keying, service_map_with, service_of,
};

/// [`CycleArgs::kinds`] value: node-level service loops and service-level
/// possible loops.
pub const EVENT: &str = "event";
/// [`CycleArgs::kinds`] value: module import cycles.
pub const IMPORT: &str = "import";
/// [`kinds_for`] input selecting both kinds.
pub const ALL: &str = "all";

/// [`CycleRow::kind`]: a node-level cross-service loop through a queue / event hop.
pub const EVENT_LOOP: &str = "event_loop";
/// [`CycleRow::kind`]: a node-level cross-service loop with no queue / event hop.
pub const CALL_LOOP: &str = "call_loop";
/// [`CycleRow::kind`]: a service-level loop with no node-level path behind it.
pub const POSSIBLE_LOOP: &str = "possible_loop";
/// [`CycleRow::kind`]: modules that import each other.
pub const IMPORT_CYCLE: &str = "import_cycle";

/// [`CycleRow::tier`]: every hop is an observed edge.
pub const DERIVED: &str = "derived";
/// [`CycleRow::tier`]: inferred from service links, not from a node path.
pub const HEURISTIC: &str = "heuristic";

/// The categories a runtime loop can travel: the cross-service mechanisms and
/// the in-service glue from a handler to the next publish or call.
const FLOW_CAUSAL: &[EdgeCategoryId] = &[
    edge_category::QUEUE_FLOWS,
    edge_category::EVENT_FLOWS,
    edge_category::HTTP_CALLS,
    edge_category::GRPC_CALLS,
    edge_category::RPC_CALLS,
    edge_category::WS_CONNECTS,
    edge_category::GRAPHQL_CALLS,
    edge_category::HANDLED_BY,
    edge_category::CALLS,
    edge_category::USES,
    edge_category::INJECTS,
];

/// The asynchronous mechanisms: a witness holding one is an `event_loop`, and
/// only their service links make a `possible_loop`.
const ASYNC_FLOWS: &[EdgeCategoryId] = &[edge_category::QUEUE_FLOWS, edge_category::EVENT_FLOWS];

/// Parent hops walked from a node to its enclosing MODULE; a guard against a
/// malformed `parent_of` cycle, far above any real nesting depth.
const MAX_PARENT_STEPS: usize = 64;

const POSSIBLE_NOTE: &str = "each service both publishes to and consumes from the others, but no consumer-handler -> producer path is in the graph (callback registration and cross-module dispatch may be unextracted)";

/// What [`cycles`] reports. Built outside this crate by `Default` plus field
/// assignment.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct CycleArgs {
    /// [`EVENT`] and / or [`IMPORT`] ([`ALL`] counts as both); any other
    /// value selects nothing ([`kinds_for`] validates user input). Default
    /// both.
    pub kinds: Vec<String>,
    /// Keep a row only when every member sits under this path or project
    /// label (the A8.3 / A8.6 scope; an unlocatable member never excludes).
    pub scope: Option<String>,
    /// Members listed per row; [`CycleRow::size`] keeps the full count. 0
    /// lists every member. Default 50.
    pub max_members: usize,
}

impl Default for CycleArgs {
    fn default() -> Self {
        Self {
            kinds: vec![EVENT.to_string(), IMPORT.to_string()],
            scope: None,
            max_members: 50,
        }
    }
}

impl CycleArgs {
    fn wants(&self, kind: &str) -> bool {
        self.kinds.iter().any(|k| k == kind || k == ALL)
    }
}

/// [`CycleArgs::kinds`] for a user's `event` / `import` / `all`; any other
/// value is an error naming the three.
pub fn kinds_for(kind: &str) -> Result<Vec<String>, String> {
    match kind {
        EVENT | IMPORT => Ok(vec![kind.to_string()]),
        ALL => Ok(vec![EVENT.to_string(), IMPORT.to_string()]),
        other => Err(format!(
            "unknown cycle kind `{other}` (expected {EVENT}, {IMPORT} or {ALL})"
        )),
    }
}

/// One hop of a witness cycle.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct CycleHop {
    /// A node qname; a service id on a `possible_loop` hop.
    pub from_qname: String,
    pub to_qname: String,
    /// The edge category (`EVENT_FLOWS`, `CALLS`, `IMPORTS`, ...).
    pub category: &'static str,
    /// The topic / route / service a cross-service hop travels over; `None`
    /// on the glue and import hops.
    pub channel: Option<String>,
    /// Where the hop is asserted: the edge's EVIDENCE site, else its `from`
    /// node; a `possible_loop` hop is located at the link's producer.
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
}

/// One cycle.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct CycleRow {
    /// [`EVENT_LOOP`], [`CALL_LOOP`], [`POSSIBLE_LOOP`] or [`IMPORT_CYCLE`].
    pub kind: &'static str,
    /// [`DERIVED`] or [`HEURISTIC`].
    pub tier: &'static str,
    /// The `glia arch` services the members sit in, sorted.
    pub services: Vec<String>,
    /// The cross-service mechanisms on the witness (else its categories),
    /// sorted.
    pub mechanisms: Vec<&'static str>,
    /// The topics / routes the witness travels over, sorted.
    pub channels: Vec<String>,
    /// Members in the component (every one, whatever `max_members` lists).
    pub size: usize,
    /// Node qnames (service ids on a `possible_loop`), sorted, capped at
    /// [`CycleArgs::max_members`].
    pub members: Vec<String>,
    /// A cycle through the component: each hop's `to` is the next hop's
    /// `from`, the last hop returns to the first `from`. A node-level loop's
    /// witness starts on the crossing edge it runs through.
    pub witness: Vec<CycleHop>,
    pub note: Option<&'static str>,
}

/// Every cycle `args` asks for: node-level `event_loop` / `call_loop` rows,
/// then `possible_loop`, then `import_cycle`; within a kind by size
/// descending, then first member. `repo_labels` name the services exactly as
/// `glia arch` passes `GenerateResult::repo_labels`.
pub fn cycles(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    args: &CycleArgs,
) -> Vec<CycleRow> {
    let loc = Locator::new(merged);
    let mut ctx = Ctx::new(merged, &loc, repo_labels);
    let mut stats = (0usize, 0usize);
    let mut rows: Vec<Pending> = Vec::new();
    if args.wants(EVENT) {
        let node_level = flow_loops(&mut ctx, &mut stats);
        let covered: HashSet<Vec<String>> =
            node_level.iter().map(|p| p.row.services.clone()).collect();
        rows.extend(node_level);
        rows.extend(possible_loops(&ctx, &covered));
    }
    if args.wants(IMPORT) {
        rows.extend(import_cycles(&mut ctx, &mut stats));
    }

    if let Some(raw) = args.scope.as_deref() {
        let s = resolve_scope(merged, raw);
        let before = rows.len();
        rows.retain(|p| {
            p.scope_ids
                .iter()
                .all(|id| loc.file_of(*id).is_none_or(|f| in_scope(&f, &s)))
        });
        eprintln!("[scope] cycles scope={s}: {before} -> {}", rows.len());
    }

    let mut rows: Vec<CycleRow> = rows.into_iter().map(|p| p.row).collect();
    rows.sort_by(|a, b| {
        kind_rank(a.kind)
            .cmp(&kind_rank(b.kind))
            .then_with(|| b.size.cmp(&a.size))
            .then_with(|| a.members.cmp(&b.members))
    });
    if args.max_members > 0 {
        for r in &mut rows {
            r.members.truncate(args.max_members);
        }
    }
    let count = |k: &str| rows.iter().filter(|r| r.kind == k).count();
    eprintln!(
        "[cycles] event_loops={} call_loops={} possible={} import_cycles={} (sccs={} nodes={})",
        count(EVENT_LOOP),
        count(CALL_LOOP),
        count(POSSIBLE_LOOP),
        count(IMPORT_CYCLE),
        stats.0,
        stats.1,
    );
    rows
}

fn kind_rank(kind: &str) -> u8 {
    match kind {
        EVENT_LOOP => 0,
        CALL_LOOP => 1,
        POSSIBLE_LOOP => 2,
        _ => 3,
    }
}

/// A row with the nodes the scope filter checks: the members of a node-level
/// row, the link ends of a `possible_loop`.
struct Pending {
    row: CycleRow,
    scope_ids: Vec<NodeId>,
}

/// Per-call node facts: identity through the one [`Locator`], and the `glia
/// arch` service, computed once per node.
struct Ctx<'a> {
    merged: &'a MergedGraph,
    loc: &'a Locator<'a>,
    labels: &'a BTreeMap<u64, String>,
    keying: ServiceKeying,
    /// The repo of each node's first graph (in `merged.graphs` order), the
    /// repo `service_map` keys it under.
    repo_of: HashMap<NodeId, u64>,
    service: HashMap<NodeId, Option<String>>,
}

impl<'a> Ctx<'a> {
    fn new(
        merged: &'a MergedGraph,
        loc: &'a Locator<'a>,
        labels: &'a BTreeMap<u64, String>,
    ) -> Self {
        let mut repo_of: HashMap<NodeId, u64> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                repo_of.entry(n.id).or_insert(g.repo.0);
            }
        }
        Ctx {
            merged,
            loc,
            labels,
            keying: default_keying(merged),
            repo_of,
            service: HashMap::new(),
        }
    }

    /// The `glia arch` service of `id`: keyed by its located file, else by
    /// its qname's owner segment (LB.4a); `None` when it has neither.
    fn service(&mut self, id: NodeId) -> Option<String> {
        if let Some(s) = self.service.get(&id) {
            return s.clone();
        }
        let file = self.loc.file_of(id).or_else(|| {
            let q = self.loc.locate(id).qname;
            endpoint::split_owner(&q).1.map(str::to_string)
        });
        let repo = self.repo_of.get(&id).copied().unwrap_or_default();
        let s = file.map(|f| service_of(&f, repo, &self.keying, self.labels));
        self.service.insert(id, s.clone());
        s
    }

    /// Sorted distinct services of `ids`.
    fn services_of(&mut self, ids: &[NodeId]) -> Vec<String> {
        let set: BTreeSet<String> = ids.iter().filter_map(|id| self.service(*id)).collect();
        set.into_iter().collect()
    }

    /// `ids` with their qnames, sorted by qname (then id). Qname order, not id
    /// order, breaks every tie that picks a witness: a node id hashes the
    /// repo's identity, so the same code checked out elsewhere would rotate an
    /// id-ordered witness.
    fn by_qname(&self, ids: &[NodeId]) -> Vec<(String, NodeId)> {
        let mut qs: Vec<(String, NodeId)> = ids
            .iter()
            .map(|id| (self.loc.locate(*id).qname, *id))
            .collect();
        qs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
        qs
    }

    /// Member qnames, sorted.
    fn member_qnames(&self, ids: &[NodeId]) -> Vec<String> {
        self.by_qname(ids).into_iter().map(|(q, _)| q).collect()
    }
}

/// `(file, 1-based line)` of an edge's EVIDENCE, when it names a file.
fn evidence_site(ev: Option<&Evidence>) -> Option<(Option<String>, Option<i64>)> {
    let ev = ev?;
    let file = ev.file.clone()?;
    Some((Some(file), ev.line.map(|l| i64::from(l) + 1)))
}

// ============================================================================
// 1. node-level service loops
// ============================================================================

fn flow_loops(ctx: &mut Ctx<'_>, stats: &mut (usize, usize)) -> Vec<Pending> {
    let adj = Adjacency::build(ctx.merged, &CategorySet::of(FLOW_CAUSAL));
    let comps = strongly_connected(&adj);
    stats.0 += comps.len();
    stats.1 += comps.iter().map(Vec::len).sum::<usize>();

    let mut spanning: Vec<(usize, Vec<String>)> = Vec::new();
    for (ci, comp) in comps.iter().enumerate() {
        let services = ctx.services_of(comp);
        if services.len() >= 2 {
            spanning.push((ci, services));
        }
    }
    if spanning.is_empty() {
        return Vec::new();
    }

    // The component's crossing edges: a kept edge between two members whose
    // services are known and differ. The witness runs through the best one
    // (an async hop first, then another cross-service mechanism, then any;
    // ties by qname), so it shows the crossing the row asserts.
    let comp_of: HashMap<NodeId, usize> = spanning
        .iter()
        .flat_map(|(ci, _)| comps[*ci].iter().map(move |id| (*id, *ci)))
        .collect();
    let mut crossing: HashMap<usize, Crossing> = HashMap::new();
    for e in ctx.merged.all_edges() {
        if !FLOW_CAUSAL.contains(&e.category) {
            continue;
        }
        let (Some(&a), Some(&b)) = (comp_of.get(&e.from), comp_of.get(&e.to)) else {
            continue;
        };
        if a != b {
            continue;
        }
        let (Some(sa), Some(sb)) = (ctx.service(e.from), ctx.service(e.to)) else {
            continue;
        };
        if sa == sb {
            continue;
        }
        let rank = if ASYNC_FLOWS.contains(&e.category) {
            0
        } else if FLOW_MECHANISMS.contains(&e.category) {
            1
        } else {
            2
        };
        let cand = Crossing {
            rank,
            from_qname: ctx.loc.locate(e.from).qname,
            to_qname: ctx.loc.locate(e.to).qname,
            edge: (e.from, e.category, e.to),
        };
        if crossing.get(&a).is_none_or(|best| cand.key() < best.key()) {
            crossing.insert(a, cand);
        }
    }

    let mut found: Vec<(usize, Vec<String>, Vec<Hop>)> = Vec::new();
    for (ci, services) in spanning {
        let hops = match crossing.get(&ci) {
            Some(best) => cycle_through(&adj, best.edge),
            None => {
                let mut placed: Vec<(NodeId, String)> = Vec::new();
                for (_, id) in ctx.by_qname(&comps[ci]) {
                    if let Some(s) = ctx.service(id) {
                        placed.push((id, s));
                    }
                }
                crossing_walk(&adj, &placed)
            }
        };
        found.push((ci, services, hops));
    }

    // The first edge (in edge order, the edge the witness walked) behind each
    // hop, for its EVIDENCE site: one scan for every witness at once.
    let want: HashSet<(NodeId, NodeId, u32)> = found
        .iter()
        .flat_map(|(_, _, hops)| hops.iter().map(|&(f, c, t)| (f, t, c.0)))
        .collect();
    let mut sites: HashMap<(NodeId, NodeId, u32), Option<Evidence>> = HashMap::new();
    for e in ctx.merged.all_edges() {
        let key = (e.from, e.to, e.category.0);
        if want.contains(&key) && !sites.contains_key(&key) {
            sites.insert(key, Evidence::of(e));
        }
    }

    let mut out = Vec::new();
    for (ci, services, hops) in found {
        let comp = &comps[ci];
        let witness: Vec<CycleHop> = hops
            .iter()
            .map(|&(f, c, t)| {
                flow_hop(
                    ctx,
                    f,
                    c,
                    t,
                    sites.get(&(f, t, c.0)).and_then(Option::as_ref),
                )
            })
            .collect();
        let is_async = hops.iter().any(|(_, c, _)| ASYNC_FLOWS.contains(c));
        let mut mechanisms: Vec<&'static str> = hops
            .iter()
            .filter(|(_, c, _)| FLOW_MECHANISMS.contains(c))
            .map(|(_, c, _)| edge_category::name(*c))
            .collect();
        if mechanisms.is_empty() {
            mechanisms = hops
                .iter()
                .map(|(_, c, _)| edge_category::name(*c))
                .collect();
        }
        mechanisms.sort_unstable();
        mechanisms.dedup();
        let channels: BTreeSet<String> = witness.iter().filter_map(|h| h.channel.clone()).collect();
        out.push(Pending {
            row: CycleRow {
                kind: if is_async { EVENT_LOOP } else { CALL_LOOP },
                tier: DERIVED,
                services,
                mechanisms,
                channels: channels.into_iter().collect(),
                size: comp.len(),
                members: ctx.member_qnames(comp),
                witness,
                note: None,
            },
            scope_ids: comp.clone(),
        });
    }
    out
}

/// One witness hop: `(from, category, to)`.
type Hop = (NodeId, EdgeCategoryId, NodeId);

/// A candidate crossing edge of a component; the least [`Crossing::key`]
/// carries the witness.
struct Crossing {
    /// 0 a queue / event hop, 1 another cross-service mechanism, 2 any other.
    rank: u8,
    from_qname: String,
    to_qname: String,
    edge: Hop,
}

impl Crossing {
    fn key(&self) -> (u8, &str, &str, u32) {
        (self.rank, &self.from_qname, &self.to_qname, self.edge.1.0)
    }
}

/// A shortest path `from` ⇝ `to` over `adj` as hops, from a forward
/// breadth-first walk (edge order breaks ties); empty when `to` is `from` or
/// unreachable. Between two members of one strongly-connected component every
/// node on it is a member too: it is reached from `from` and reaches `to`.
fn shortest_path(adj: &Adjacency, from: NodeId, to: NodeId) -> Vec<Hop> {
    if from == to {
        return Vec::new();
    }
    let walk = bfs(adj, &[from], Walk::Forward, usize::MAX);
    let parent: HashMap<NodeId, (NodeId, EdgeCategoryId)> = walk
        .reached
        .iter()
        .map(|r| (r.id, (r.parent, r.via)))
        .collect();
    let mut hops = Vec::new();
    let mut at = to;
    while at != from {
        let Some(&(p, via)) = parent.get(&at) else {
            return Vec::new();
        };
        hops.push((p, via, at));
        at = p;
    }
    hops.reverse();
    hops
}

/// The shortest cycle through the edge `x -c-> y`: the edge, then the
/// shortest path back `y` ⇝ `x`. A simple cycle, since a shortest path
/// repeats no node.
fn cycle_through(adj: &Adjacency, (x, c, y): Hop) -> Vec<Hop> {
    let mut hops = vec![(x, c, y)];
    hops.extend(shortest_path(adj, y, x));
    hops
}

/// The witness of a component whose services meet only through unplaced
/// members (no edge joins two known services directly): a closed walk from
/// the first placed member to the first member of another service and back,
/// each leg a shortest path. It may pass a node twice; it always visits two
/// services. `placed` is the component's members that have a service, in
/// qname order.
fn crossing_walk(adj: &Adjacency, placed: &[(NodeId, String)]) -> Vec<Hop> {
    let Some((u, su)) = placed.first() else {
        return Vec::new();
    };
    let Some((v, _)) = placed.iter().find(|(_, s)| s != su) else {
        return Vec::new();
    };
    let mut hops = shortest_path(adj, *u, *v);
    hops.extend(shortest_path(adj, *v, *u));
    hops
}

/// One located node-level hop: at its edge's EVIDENCE site, else at `from`.
/// A cross-service mechanism hop names its channel, read off the `from` end
/// (the producer / caller) and else the `to` end, as `glia arch` reads it.
fn flow_hop(
    ctx: &Ctx<'_>,
    from: NodeId,
    category: EdgeCategoryId,
    to: NodeId,
    ev: Option<&Evidence>,
) -> CycleHop {
    let f = ctx.loc.locate(from);
    let t = ctx.loc.locate(to);
    let channel = FLOW_MECHANISMS.contains(&category).then(|| {
        let c = channel_of(&f.qname, &f.name);
        if c.is_empty() {
            channel_of(&t.qname, &t.name)
        } else {
            c
        }
    });
    let (file, line) = evidence_site(ev).unwrap_or((f.file, f.line));
    CycleHop {
        from_qname: f.qname,
        to_qname: t.qname,
        category: edge_category::name(category),
        channel,
        file,
        line,
    }
}

// ============================================================================
// 2. service-level possible loops
// ============================================================================

/// The QUEUE_FLOWS / EVENT_FLOWS service links as a graph over synthetic ids:
/// service `i` of the sorted service names is `NodeId(i)`, one edge per link
/// in `service_map` order.
struct ServiceGraph {
    nodes: Vec<NodeId>,
    edges: Vec<Edge>,
}

impl GraphSource for ServiceGraph {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

fn possible_loops(ctx: &Ctx<'_>, covered: &HashSet<Vec<String>>) -> Vec<Pending> {
    let merged = ctx.merged;
    let map = service_map_with(merged, ctx.labels, &ctx.keying);
    let category_of = |l: &ServiceLink| {
        ASYNC_FLOWS
            .iter()
            .copied()
            .find(|c| edge_category::name(*c) == l.mechanism)
    };
    let links: Vec<(&ServiceLink, EdgeCategoryId)> = map
        .links
        .iter()
        .filter_map(|l| category_of(l).map(|c| (l, c)))
        .collect();
    if links.is_empty() {
        return Vec::new();
    }
    let names: Vec<&str> = links
        .iter()
        .flat_map(|(l, _)| [l.from.as_str(), l.to.as_str()])
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .collect();
    let ix = |s: &str| NodeId(names.binary_search(&s).unwrap_or_default() as u64);
    let graph = ServiceGraph {
        nodes: (0..names.len() as u64).map(NodeId).collect(),
        edges: links
            .iter()
            .map(|(l, c)| {
                Edge::new(
                    ix(l.from.as_str()),
                    ix(l.to.as_str()),
                    *c,
                    Confidence::Strong,
                )
            })
            .collect(),
    };
    let adj = Adjacency::build(&graph, &CategorySet::all());

    let mut by_qname: Option<HashMap<&str, NodeId>> = None;
    let mut out = Vec::new();
    for comp in strongly_connected(&adj) {
        let services: Vec<String> = comp
            .iter()
            .map(|id| names[id.0 as usize].to_string())
            .collect();
        if covered.contains(&services) {
            continue;
        }
        let by_qname = by_qname.get_or_insert_with(|| qname_index(merged));
        let mut witness = Vec::new();
        let mut scope_ids = Vec::new();
        // Synthetic ids follow service-name order: the first service starts.
        let Some(&start) = comp.first() else {
            continue;
        };
        for (f, c, t) in witness_cycle(&adj, &comp, start) {
            let Some((link, _)) = links
                .iter()
                .find(|(l, lc)| *lc == c && ix(l.from.as_str()) == f && ix(l.to.as_str()) == t)
            else {
                continue;
            };
            let producer = by_qname.get(link.example_from_qname.as_str()).copied();
            let consumer = by_qname.get(link.example_to_qname.as_str()).copied();
            scope_ids.extend(producer.into_iter().chain(consumer));
            let (file, line) = match producer {
                Some(p) => {
                    let at = ctx.loc.locate(p);
                    (at.file, at.line)
                }
                None => (None, None),
            };
            witness.push(CycleHop {
                from_qname: link.from.clone(),
                to_qname: link.to.clone(),
                category: link.mechanism,
                channel: Some(link.channel.clone()),
                file,
                line,
            });
        }
        let mut mechanisms: Vec<&'static str> = witness.iter().map(|h| h.category).collect();
        mechanisms.sort_unstable();
        mechanisms.dedup();
        let channels: BTreeSet<String> = witness.iter().filter_map(|h| h.channel.clone()).collect();
        out.push(Pending {
            row: CycleRow {
                kind: POSSIBLE_LOOP,
                tier: HEURISTIC,
                services: services.clone(),
                mechanisms,
                channels: channels.into_iter().collect(),
                size: services.len(),
                members: services,
                witness,
                note: Some(POSSIBLE_NOTE),
            },
            scope_ids,
        });
    }
    out
}

/// qname -> node id, the first node (in graph order) carrying it.
fn qname_index(merged: &MergedGraph) -> HashMap<&str, NodeId> {
    let mut idx: HashMap<&str, NodeId> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if let Some(q) = g.nav.qname_by_id.get(&n.id) {
                idx.entry(q.as_str()).or_insert(n.id);
            }
        }
    }
    idx
}

// ============================================================================
// 3. import cycles
// ============================================================================

/// Every IMPORTS edge projected onto modules: `module(from) -> module(to)`,
/// where `module(n)` is `n` when it is a MODULE, else the nearest MODULE up
/// `nav.parent_of` in the first graph naming `n`. Self-projections are dropped
/// and a module pair keeps one edge, its first import in edge order, whose
/// site locates the hop. LE.8 reuses it.
pub(crate) struct ModuleImports {
    modules: Vec<NodeId>,
    edges: Vec<Edge>,
    /// Parallel to `edges`: the EVIDENCE of the import edge behind each.
    evidence: Vec<Option<Evidence>>,
    at: HashMap<(NodeId, NodeId), usize>,
}

impl ModuleImports {
    /// The EVIDENCE of the first import `from` -> `to` projects (`None` when
    /// the pair is no projected edge or its edge carried none).
    pub(crate) fn evidence(&self, from: NodeId, to: NodeId) -> Option<&Evidence> {
        self.evidence.get(*self.at.get(&(from, to))?)?.as_ref()
    }
}

impl GraphSource for ModuleImports {
    fn node_ids(&self) -> Vec<NodeId> {
        self.modules.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

/// [`ModuleImports`] of `merged`: modules in first-seen edge order.
pub(crate) fn module_import_graph(merged: &MergedGraph) -> ModuleImports {
    // The first graph (in `merged.graphs` order) naming each node; the map's
    // own iteration order cannot change the winner.
    let mut graph_of: HashMap<NodeId, usize> = HashMap::new();
    for (gi, g) in merged.graphs.iter().enumerate() {
        for id in g.nav.kind_by_id.keys() {
            graph_of.entry(*id).or_insert(gi);
        }
    }
    let module_of = |id: NodeId| -> Option<NodeId> {
        let g = merged.graphs.get(*graph_of.get(&id)?)?;
        let mut cur = id;
        for _ in 0..MAX_PARENT_STEPS {
            if g.nav.kind_by_id.get(&cur) == Some(&node_kind::MODULE) {
                return Some(cur);
            }
            cur = *g.nav.parent_of.get(&cur)?;
        }
        None
    };
    let mut out = ModuleImports {
        modules: Vec::new(),
        edges: Vec::new(),
        evidence: Vec::new(),
        at: HashMap::new(),
    };
    let mut seen: HashSet<NodeId> = HashSet::new();
    for e in merged.all_edges() {
        if e.category != edge_category::IMPORTS {
            continue;
        }
        let (Some(a), Some(b)) = (module_of(e.from), module_of(e.to)) else {
            continue;
        };
        if a == b || out.at.contains_key(&(a, b)) {
            continue;
        }
        for m in [a, b] {
            if seen.insert(m) {
                out.modules.push(m);
            }
        }
        out.at.insert((a, b), out.edges.len());
        out.edges
            .push(Edge::new(a, b, edge_category::IMPORTS, e.confidence));
        out.evidence.push(Evidence::of(e));
    }
    out
}

fn import_cycles(ctx: &mut Ctx<'_>, stats: &mut (usize, usize)) -> Vec<Pending> {
    let imports = module_import_graph(ctx.merged);
    let adj = Adjacency::build(&imports, &CategorySet::of(&[edge_category::IMPORTS]));
    let comps = strongly_connected(&adj);
    stats.0 += comps.len();
    stats.1 += comps.iter().map(Vec::len).sum::<usize>();
    let mut out = Vec::new();
    for comp in comps {
        // The member first in qname order starts the witness.
        let Some(&(_, start)) = ctx.by_qname(&comp).first() else {
            continue;
        };
        let witness: Vec<CycleHop> = witness_cycle(&adj, &comp, start)
            .into_iter()
            .map(|(f, c, t)| {
                let from = ctx.loc.locate(f);
                let (file, line) =
                    evidence_site(imports.evidence(f, t)).unwrap_or((from.file, from.line));
                CycleHop {
                    from_qname: from.qname,
                    to_qname: ctx.loc.locate(t).qname,
                    category: edge_category::name(c),
                    channel: None,
                    file,
                    line,
                }
            })
            .collect();
        out.push(Pending {
            row: CycleRow {
                kind: IMPORT_CYCLE,
                tier: DERIVED,
                services: ctx.services_of(&comp),
                mechanisms: vec![edge_category::name(edge_category::IMPORTS)],
                channels: Vec::new(),
                size: comp.len(),
                members: ctx.member_qnames(&comp),
                witness,
                note: None,
            },
            scope_ids: comp,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_for_maps_all_and_refuses_unknown() {
        assert_eq!(kinds_for("event").expect("event"), vec![EVENT]);
        assert_eq!(kinds_for("import").expect("import"), vec![IMPORT]);
        assert_eq!(kinds_for("all").expect("all"), vec![EVENT, IMPORT]);
        let err = kinds_for("loops").expect_err("unknown");
        assert!(err.contains("loops") && err.contains("all"), "{err}");
        let mut a = CycleArgs::default();
        assert!(a.wants(EVENT) && a.wants(IMPORT));
        a.kinds = vec![ALL.to_string()];
        assert!(a.wants(EVENT) && a.wants(IMPORT));
        a.kinds = vec!["loops".to_string()];
        assert!(!a.wants(EVENT) && !a.wants(IMPORT));
    }

    #[test]
    fn empty_graph_has_no_cycles() {
        let merged = MergedGraph::new(Vec::new());
        assert!(cycles(&merged, &BTreeMap::new(), &CycleArgs::default()).is_empty());
        let imports = module_import_graph(&merged);
        assert!(imports.node_ids().is_empty() && imports.edges().next().is_none());
        assert!(imports.evidence(NodeId(1), NodeId(2)).is_none());
    }

    fn toy(edges: &[(u64, u64)]) -> Adjacency {
        let mut nodes: Vec<NodeId> = edges
            .iter()
            .flat_map(|&(a, b)| [NodeId(a), NodeId(b)])
            .collect();
        nodes.dedup();
        let edges = edges
            .iter()
            .map(|&(a, b)| {
                Edge::new(
                    NodeId(a),
                    NodeId(b),
                    edge_category::CALLS,
                    Confidence::Strong,
                )
            })
            .collect();
        Adjacency::build(&ServiceGraph { nodes, edges }, &CategorySet::all())
    }

    fn ids(hops: &[Hop]) -> Vec<(u64, u64)> {
        hops.iter().map(|(f, _, t)| (f.0, t.0)).collect()
    }

    #[test]
    fn cycle_through_an_edge_takes_the_shortest_way_back() {
        // 1 -> 2 -> 3 -> 1, and a longer way back 2 -> 4 -> 5 -> 1.
        let adj = toy(&[(1, 2), (2, 4), (4, 5), (5, 1), (2, 3), (3, 1)]);
        let c = edge_category::CALLS;
        assert_eq!(
            ids(&cycle_through(&adj, (NodeId(1), c, NodeId(2)))),
            vec![(1, 2), (2, 3), (3, 1)]
        );
        assert_eq!(
            ids(&cycle_through(&adj, (NodeId(4), c, NodeId(5)))),
            vec![(4, 5), (5, 1), (1, 2), (2, 4)]
        );
        assert!(shortest_path(&adj, NodeId(1), NodeId(1)).is_empty());
        assert!(
            shortest_path(&adj, NodeId(1), NodeId(9)).is_empty(),
            "an id the index lacks"
        );
    }

    #[test]
    fn crossing_walk_visits_two_services_through_unplaced_nodes() {
        // Services meet only through the unplaced 2 and 4: 1 (a) -> 2 -> 3 (b)
        // -> 4 -> 1. No edge joins a and b directly.
        let adj = toy(&[(1, 2), (2, 3), (3, 4), (4, 1)]);
        let placed = vec![(NodeId(1), "a".to_string()), (NodeId(3), "b".to_string())];
        assert_eq!(
            ids(&crossing_walk(&adj, &placed)),
            vec![(1, 2), (2, 3), (3, 4), (4, 1)]
        );
        assert!(
            crossing_walk(&adj, &placed[..1]).is_empty(),
            "one service: nothing to cross"
        );
        assert!(crossing_walk(&adj, &[]).is_empty());
    }

    #[test]
    fn rows_order_by_kind_then_size() {
        assert!(kind_rank(EVENT_LOOP) < kind_rank(CALL_LOOP));
        assert!(kind_rank(CALL_LOOP) < kind_rank(POSSIBLE_LOOP));
        assert!(kind_rank(POSSIBLE_LOOP) < kind_rank(IMPORT_CYCLE));
    }
}
