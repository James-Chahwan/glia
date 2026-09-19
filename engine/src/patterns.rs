//! Pattern conformance (LE.7a, EXPERIMENTAL): does a route handler follow the
//! call chain the other handlers of its service follow? "38 of 40 handlers go
//! handler > service > repository > db; this one goes handler > repository >
//! db." The answer is an aggregate over every handler of a service, which is
//! exactly what a reader writing the next one does not look at. A divergence
//! is an observation, never a rule and never "wrong": the verdict is
//! `DIVERGENCE`, tier `heuristic` (an explicit rule's `VIOLATION` is LE.8's).
//! Every report carries `experimental: true` and there is no score anywhere:
//! verdicts are counts.
//!
//! # Roles
//!
//! Each node on a handler's chain plays at most one role, the first that
//! applies, each labelled with the SOURCE it came from:
//!
//! 1. `handler` (`edge`): the HANDLED_BY target of a server ROUTE (a
//!    client-router page, `nav::is_nav_route`, is a navigation target, not a
//!    request handler, and is left out);
//! 2. `service` (`kind`): a node whose roles (`roles::roles_in`: its kind or
//!    the ROLE cell LB.3a's fold writes) include SERVICE, a node a SERVICE
//!    CONTAINS (`services.rs`'s overlay shape), or a node whose enclosing
//!    declaration (`CodeNav::parent_of`) is a SERVICE. A SERVICE that owns a
//!    handler is a controller (`@RestController`, `[ApiController]` and a Go
//!    handler struct all classify as SERVICE), so its other methods take no
//!    role from it;
//! 3. `repository` (`edge`): a node holding an ACCESSES_DATA out-edge, or
//!    whose enclosing CLASS / STRUCT holds one;
//! 4. `service` (`edge`): an INJECTS target, or a method of one;
//! 5. the name fallback (`name`, HEURISTIC), only for a FUNCTION / METHOD /
//!    CLASS / STRUCT / INTERFACE no structural rule placed: the name or a
//!    qname segment (split on `:` `/` `.`, innermost first) matching
//!    `(^|_)(repo|repository|repositories|store|dao)$` or
//!    `(Repository|Repo|Dao|Store)$` is a repository,
//!    `(^|_)(service|services)$` or `Service$` a service.
//!
//! Anything else is a helper and does not appear in a signature.
//!
//! # Signature
//!
//! From each handler, ONE breadth-first walk (`algo::reach::bfs`, depth
//! <= `max_depth`) forward over CALLS / USES / INJECTS / ACCESSES_DATA and
//! backward over IMPLEMENTS (an interface method to its implementation,
//! A6.6). Edges with a MODULE end are left out: module-scope data access and
//! a function's use of a module describe the file, not the handler's chain.
//! The chain ends at the FIRST effect sink the walk discovers: a node the
//! domain's sink table (`CODE_PROFILE.tables.effect_sinks`, LE.4d) classifies
//! by its kind together with the category reaching it; a sink is routed to a
//! sink-only twin, so the walk never continues through it. The signature is
//! `handler`, then the roles along the BFS parent path (helpers dropped,
//! consecutive duplicates collapsed), then the sink's class:
//! `handler>service>repository>db`. No sink within the depth is the one
//! signature `handler>(no effect)`: with no sink there is no path to name.
//! The path hops are the evidence, each at LC.3a's site of the first edge
//! with that key (1-based).
//!
//! # Populations and verdicts
//!
//! Handlers are grouped by `arch::service_of` under `arch::default_keying`
//! (the services `glia arch` shows). A handler with ORIGIN provenance
//! `test_fixture`, `generated` or `generated_proto`, one with no located
//! file, and one outside `scope` (strict: its located file under the resolved
//! path) is counted in `excluded` and belongs to no population. A population
//! below `min_support` gets no verdict (`too_small`, counted in
//! `skipped_small`). Otherwise its convention is the most frequent signature
//! that reaches a sink (ties: the lexicographically smallest), declared only
//! when `count * 100 >= min_share_pct * size` (`judged`, verdict
//! `matching/size`), else `no_convention`. Every judged handler whose
//! signature differs is an exception, listed however legitimate it looks (a
//! health check included).
//!
//! `handler>(no effect)` is never the convention, whatever its count: it says
//! the graph followed no chain (an unbound receiver, an unextracted data
//! access), so as a convention it would turn the one handler the graph CAN
//! follow into the divergence. It still counts toward the population's size,
//! and under a declared convention a blind handler is an exception like any
//! other. (Measured on quokka-stack: 58 of its 59 Go handlers are blind,
//! `userRepo := Services.UserRepository()` binds no receiver, and LE.4d's
//! `effects` agrees no sink is reachable from them.)
//!
//! # Delta mode
//!
//! [`pattern_conformance_delta`] takes populations and conventions from the
//! whole after graph and lists as `divergences` only the exceptions a change
//! touched: the handler, or a node on its path, is added, modified or moved
//! in the delta, or a hop of its path is an added edge. [`pattern_conformance`]
//! lists every exception. Populations are ordered by service, exceptions by
//! (signature, handler qname).
//!
//! # Limits
//!
//! Receiver-typed calls (`this.svc.find()`) bind only where Batch C's receiver
//! typing (A6.2a/b/c) does; an unbound chain stops at the handler and lands in
//! `handler>(no effect)`, visible in the report. Go cross-package calls
//! resolve by file stem, so a repository in `repository/repo.go` loses every
//! call into it and its handlers' chains are incomplete.
//!
//! # Security
//!
//! Structural reachability over edges the build holds: no taint or value
//! flow, no auth dimension. Every handler is judged against its peers; no
//! list of routes reaching data is produced.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `glia_engine::patterns::<item>`, never flattened into the
//! crate root.
//!
//! fired_on marker, one line per answer:
//! `[patterns] experimental populations=<P> judged=<J> handlers=<H> divergences=<D> skipped_small=<S> role_sources edge=<E> kind=<K> name=<N>`
//! (grep `^\[patterns\] experimental`), plus in delta mode
//! `[patterns] delta touched_nodes=<T> added_edges=<A> exceptions=<X> divergences=<D>`.

use std::collections::{BTreeMap, HashMap, HashSet};

use glia_activation::algo::delta::{EdgeKey, GraphDelta};
use glia_activation::algo::{Adjacency, CategorySet, GraphSource, Walk, reach};
use glia_code_domain::endpoint::split_owner;
use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Edge, EdgeCategoryId, NodeId, NodeKindId};
use glia_graph::MergedGraph;
use glia_graph::nav::{is_nav_route, nav_route_path};
use glia_graph::roles::roles_in;

use crate::answers::{Locator, in_scope, resolve_scope};
use crate::arch::{default_keying, service_of};
use crate::profile::CODE_PROFILE;

/// [`PatternArgs::default`]'s `min_support`.
pub const DEFAULT_MIN_SUPPORT: usize = 5;
/// [`PatternArgs::default`]'s `min_share_pct`.
pub const DEFAULT_MIN_SHARE_PCT: usize = 75;
/// [`PatternArgs::default`]'s `max_depth`.
pub const DEFAULT_MAX_DEPTH: usize = 6;

/// The role names.
pub const HANDLER: &str = "handler";
pub const SERVICE: &str = "service";
pub const REPOSITORY: &str = "repository";

/// The signature's end when no sink is reached.
pub const NO_EFFECT: &str = "(no effect)";

const EDGE: &str = "edge";
const KIND: &str = "kind";
const NAME: &str = "name";

/// Categories the walk follows forward; IMPLEMENTS is followed backward.
const FORWARD: [EdgeCategoryId; 4] = [
    edge_category::CALLS,
    edge_category::USES,
    edge_category::INJECTS,
    edge_category::ACCESSES_DATA,
];

/// Handler provenances that never join a population.
const EXCLUDED_PROVENANCE: [&str; 3] = ["test_fixture", "generated", "generated_proto"];

/// Kinds the name fallback may place.
const NAMED_KINDS: [NodeKindId; 5] = [
    node_kind::FUNCTION,
    node_kind::METHOD,
    node_kind::CLASS,
    node_kind::STRUCT,
    node_kind::INTERFACE,
];

/// A twin id is the real id XOR this, stepped until it is no id of the graph.
const TWIN_SALT: u64 = 0x7a77_e44c_0f0e_a11d;
const TWIN_STEP: u64 = 0x9e37_79b9_7f4a_7c15;

/// How [`pattern_conformance`] groups and judges. Start from `default()` and
/// set fields: `#[non_exhaustive]` rules out a struct literal outside this
/// crate.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct PatternArgs {
    /// Smallest population that gets a verdict ([`DEFAULT_MIN_SUPPORT`]).
    pub min_support: usize,
    /// Share (percent) of a population the most frequent signature needs to
    /// be declared its convention ([`DEFAULT_MIN_SHARE_PCT`]).
    pub min_share_pct: usize,
    /// Hops of each handler's walk ([`DEFAULT_MAX_DEPTH`]).
    pub max_depth: usize,
    /// Keep only handlers located under this path or project label.
    pub scope: Option<String>,
}

impl Default for PatternArgs {
    fn default() -> Self {
        PatternArgs {
            min_support: DEFAULT_MIN_SUPPORT,
            min_share_pct: DEFAULT_MIN_SHARE_PCT,
            max_depth: DEFAULT_MAX_DEPTH,
            scope: None,
        }
    }
}

/// One hop of a handler's path: its ends, category and the 1-based site the
/// edge was asserted at (LC.3a evidence), when recorded.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct PatternHop {
    pub from_qname: String,
    pub to_qname: String,
    pub category: &'static str,
    /// Walked against the edge: IMPLEMENTS, interface method to implementation.
    pub backward: bool,
    pub site_file: Option<String>,
    /// 1-based.
    pub site_line: Option<i64>,
}

/// The role a node on a path plays and where the role came from: `edge`,
/// `kind` or `name` (the HEURISTIC fallback).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct RoleSource {
    pub qname: String,
    pub role: &'static str,
    pub source: &'static str,
}

/// A handler whose signature differs from its population's convention.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct Divergence {
    /// Always `DIVERGENCE`: an observed convention, not a declared rule.
    pub verdict: &'static str,
    /// Always `heuristic`.
    pub tier: &'static str,
    pub service: String,
    /// The handler's qname.
    pub handler: String,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// From the first ROUTE (in edge order) HANDLED_BY this handler.
    pub route_method: Option<String>,
    pub route_path: Option<String>,
    pub signature: String,
    pub convention: String,
    /// Handlers of the population holding the convention.
    pub matching: usize,
    /// The population's size.
    pub population: usize,
    /// From the handler to the sink; empty for `handler>(no effect)`.
    pub path: Vec<PatternHop>,
    /// The handler, then each role node of the path, in path order.
    pub role_sources: Vec<RoleSource>,
}

/// The handlers of one service and its verdict.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct Population {
    pub service: String,
    /// Always `handler`.
    pub role: &'static str,
    pub size: usize,
    /// `judged` | `no_convention` | `too_small`.
    pub status: &'static str,
    pub convention: Option<String>,
    /// Handlers holding the convention; 0 when none is declared.
    pub matching: usize,
    /// `matching/size` when judged.
    pub verdict: Option<String>,
    /// Every signature with its count, by count (descending) then signature.
    pub signatures: Vec<(String, usize)>,
    /// Role -> source -> distinct nodes playing it on the population's paths
    /// (the handlers themselves are not counted).
    pub role_sources: BTreeMap<&'static str, BTreeMap<&'static str, usize>>,
    /// Every handler off the convention, by (signature, handler qname).
    pub exceptions: Vec<Divergence>,
}

/// [`pattern_conformance`]'s answer.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct PatternReport {
    /// Always `true`.
    pub experimental: bool,
    pub delta_mode: bool,
    /// Handlers in a population.
    pub handlers: usize,
    /// Populations with a declared convention.
    pub judged: usize,
    /// Populations below `min_support`.
    pub skipped_small: usize,
    /// Handlers left out, by reason: a provenance, `unplaced`, `out_of_scope`.
    pub excluded: BTreeMap<&'static str, usize>,
    /// Source -> distinct non-handler role nodes on every path (`edge`,
    /// `kind` and `name` always present).
    pub role_sources: BTreeMap<&'static str, usize>,
    pub populations: Vec<Population>,
    /// Whole graph: every exception. Delta mode: the touched ones.
    pub divergences: Vec<Divergence>,
}

/// Pattern conformance over the whole graph (module docs). `repo_labels` keys
/// services as `glia arch` does (`GenerateResult::repo_labels`).
///
/// Cost: O(V + E) indexes and one `Adjacency`, then one BFS per handler
/// (bounded by `max_depth`), one O(E) scan for the exceptions' hop evidence.
pub fn pattern_conformance(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    args: &PatternArgs,
) -> PatternReport {
    report(merged, repo_labels, args, None)
}

/// Pattern conformance with `after`'s populations and conventions, listing
/// only the divergences `delta` (LE.1a, `after`'s ids) touched (module docs).
pub fn pattern_conformance_delta(
    after: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    delta: &GraphDelta,
    args: &PatternArgs,
) -> PatternReport {
    report(after, repo_labels, args, Some(delta))
}

/// One placed handler and its signature.
struct Member {
    handler: NodeId,
    route: NodeId,
    sig: Signature,
}

fn report(
    merged: &MergedGraph,
    labels: &BTreeMap<u64, String>,
    args: &PatternArgs,
    delta: Option<&GraphDelta>,
) -> PatternReport {
    let facts = Facts::build(merged);
    let loc = Locator::new(merged);
    let walk = WalkGraph::build(merged, &facts);
    let adj = Adjacency::build(&walk, &CategorySet::all());
    let keying = default_keying(merged);
    let scope = args.scope.as_deref().map(|s| resolve_scope(merged, s));

    let mut excluded: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut groups: BTreeMap<String, Vec<Member>> = BTreeMap::new();
    for &(handler, route) in &facts.handlers {
        let Some(fact) = facts.at.get(&handler) else {
            // A HANDLED_BY target no graph's nav names: nothing places it.
            *excluded.entry("unplaced").or_insert(0) += 1;
            continue;
        };
        if let Some(p) = EXCLUDED_PROVENANCE
            .into_iter()
            .find(|p| has_provenance(fact.cells, p))
        {
            *excluded.entry(p).or_insert(0) += 1;
            continue;
        }
        let Some(file) = loc.file_of(handler) else {
            *excluded.entry("unplaced").or_insert(0) += 1;
            continue;
        };
        if scope.as_deref().is_some_and(|s| !in_scope(&file, s)) {
            *excluded.entry("out_of_scope").or_insert(0) += 1;
            continue;
        }
        let service = service_of(&file, fact.repo, &keying, labels);
        let sig = signature(&adj, &walk, &facts, handler, args.max_depth);
        groups.entry(service).or_default().push(Member {
            handler,
            route,
            sig,
        });
    }

    // Verdicts: per population, its signature counts, convention and the
    // members off it.
    struct Judged {
        service: String,
        status: &'static str,
        convention: Option<String>,
        matching: usize,
        signatures: Vec<(String, usize)>,
        exceptions: Vec<usize>,
    }
    let mut judged_rows: Vec<Judged> = Vec::new();
    for (service, members) in &groups {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for m in members {
            *counts.entry(m.sig.text.as_str()).or_insert(0) += 1;
        }
        let mut signatures: Vec<(String, usize)> =
            counts.iter().map(|(s, n)| (s.to_string(), *n)).collect();
        signatures.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let size = members.len();
        // A blind chain is never the convention (module docs).
        let top = signatures
            .iter()
            .find(|(s, _)| !s.ends_with(NO_EFFECT))
            .cloned();
        let (status, convention, matching) = match top {
            _ if size < args.min_support => ("too_small", None, 0),
            Some((sig, n)) if n.saturating_mul(100) >= args.min_share_pct.saturating_mul(size) => {
                ("judged", Some(sig), n)
            }
            _ => ("no_convention", None, 0),
        };
        let mut exceptions: Vec<usize> = match &convention {
            Some(c) => (0..size).filter(|&i| members[i].sig.text != *c).collect(),
            None => Vec::new(),
        };
        exceptions.sort_by(|&a, &b| {
            (&members[a].sig.text, facts.qname(members[a].handler))
                .cmp(&(&members[b].sig.text, facts.qname(members[b].handler)))
        });
        judged_rows.push(Judged {
            service: service.clone(),
            status,
            convention,
            matching,
            signatures,
            exceptions,
        });
    }

    // One scan: the first edge per exception hop key, for its evidence site.
    let wanted: HashSet<EdgeKey> = judged_rows
        .iter()
        .zip(groups.values())
        .flat_map(|(j, members)| j.exceptions.iter().map(move |&i| &members[i]))
        .flat_map(|m| m.sig.hops.iter().map(Hop::key))
        .collect();
    let mut first: HashMap<EdgeKey, &Edge> = HashMap::new();
    if !wanted.is_empty() {
        for e in merged.all_edges() {
            let key = EdgeKey::from(e);
            if wanted.contains(&key) {
                first.entry(key).or_insert(e);
            }
        }
    }

    // Delta mode: what the change touched.
    let touched: Option<(HashSet<NodeId>, HashSet<EdgeKey>)> = delta.map(|d| {
        let nodes = d
            .added_nodes
            .iter()
            .chain(&d.modified_nodes)
            .copied()
            .chain(d.moved_nodes.iter().map(|&(_, after)| after))
            .collect();
        (nodes, d.added_edges.iter().copied().collect())
    });
    let is_touched = |m: &Member| -> bool {
        let Some((nodes, edges)) = &touched else {
            return true;
        };
        nodes.contains(&m.handler)
            || m.sig.hops.iter().any(|h| {
                nodes.contains(&h.from) || nodes.contains(&h.to) || edges.contains(&h.key())
            })
    };

    let mut qnames: HashMap<NodeId, String> = HashMap::new();
    let mut qname = |id: NodeId| -> String {
        qnames
            .entry(id)
            .or_insert_with(|| loc.locate(id).qname)
            .clone()
    };
    let mut populations: Vec<Population> = Vec::new();
    let mut divergences: Vec<Divergence> = Vec::new();
    let mut all_roles: HashMap<NodeId, &'static str> = HashMap::new();
    let mut exception_count = 0usize;
    for (j, members) in judged_rows.into_iter().zip(groups.values()) {
        let mut role_nodes: BTreeMap<u64, (&'static str, &'static str)> = BTreeMap::new();
        for m in members {
            for &(id, role, source) in &m.sig.roles {
                role_nodes.insert(id.0, (role, source));
                all_roles.insert(id, source);
            }
        }
        let mut role_sources: BTreeMap<&'static str, BTreeMap<&'static str, usize>> =
            BTreeMap::new();
        for &(role, source) in role_nodes.values() {
            *role_sources
                .entry(role)
                .or_default()
                .entry(source)
                .or_insert(0) += 1;
        }
        let size = members.len();
        let mut exceptions: Vec<Divergence> = Vec::new();
        for &i in &j.exceptions {
            let m = &members[i];
            let at = loc.locate(m.handler);
            let (route_method, route_path) = facts
                .at
                .get(&m.route)
                .map(|r| route_of(r.qname, r.cells))
                .unwrap_or((None, None));
            let path = m
                .sig
                .hops
                .iter()
                .map(|h| {
                    let (site_file, site_line) = first
                        .get(&h.key())
                        .map(|e| site_of(&loc, e))
                        .unwrap_or((None, None));
                    PatternHop {
                        from_qname: qname(h.from),
                        to_qname: qname(h.to),
                        category: edge_category::name(h.category),
                        backward: h.backward,
                        site_file,
                        site_line,
                    }
                })
                .collect();
            let mut roles = vec![RoleSource {
                qname: at.qname.clone(),
                role: HANDLER,
                source: EDGE,
            }];
            roles.extend(m.sig.roles.iter().map(|&(id, role, source)| RoleSource {
                qname: qname(id),
                role,
                source,
            }));
            let d = Divergence {
                verdict: "DIVERGENCE",
                tier: "heuristic",
                service: j.service.clone(),
                handler: at.qname,
                file: at.file,
                line: at.line,
                route_method,
                route_path,
                signature: m.sig.text.clone(),
                convention: j.convention.clone().unwrap_or_default(),
                matching: j.matching,
                population: size,
                path,
                role_sources: roles,
            };
            if is_touched(m) {
                divergences.push(d.clone());
            }
            exceptions.push(d);
        }
        exception_count += exceptions.len();
        populations.push(Population {
            service: j.service,
            role: HANDLER,
            size,
            status: j.status,
            verdict: (j.status == "judged").then(|| format!("{}/{size}", j.matching)),
            convention: j.convention,
            matching: j.matching,
            signatures: j.signatures,
            role_sources,
            exceptions,
        });
    }

    let mut role_sources: BTreeMap<&'static str, usize> =
        [EDGE, KIND, NAME].into_iter().map(|s| (s, 0)).collect();
    for &source in all_roles.values() {
        *role_sources.entry(source).or_insert(0) += 1;
    }
    let handlers: usize = populations.iter().map(|p| p.size).sum();
    let judged = populations.iter().filter(|p| p.status == "judged").count();
    let skipped_small = populations
        .iter()
        .filter(|p| p.status == "too_small")
        .count();
    let count = |s: &str| role_sources.get(s).copied().unwrap_or(0);
    eprintln!(
        "[patterns] experimental populations={} judged={judged} handlers={handlers} divergences={} skipped_small={skipped_small} role_sources edge={} kind={} name={}",
        populations.len(),
        divergences.len(),
        count(EDGE),
        count(KIND),
        count(NAME)
    );
    if let Some((nodes, edges)) = &touched {
        eprintln!(
            "[patterns] delta touched_nodes={} added_edges={} exceptions={exception_count} divergences={}",
            nodes.len(),
            edges.len(),
            divergences.len()
        );
    }

    PatternReport {
        experimental: true,
        delta_mode: delta.is_some(),
        handlers,
        judged,
        skipped_small,
        excluded,
        role_sources,
        populations,
        divergences,
    }
}

/// One walked hop, real ids: `backward` hops ran `from` (an interface
/// method) to `to` (its implementation) against an IMPLEMENTS edge.
#[derive(Clone, Copy)]
struct Hop {
    from: NodeId,
    to: NodeId,
    category: EdgeCategoryId,
    backward: bool,
}

impl Hop {
    /// The key of the graph edge the hop walked.
    fn key(&self) -> EdgeKey {
        let (from, to) = if self.backward {
            (self.to, self.from)
        } else {
            (self.from, self.to)
        };
        EdgeKey {
            from,
            to,
            category: self.category,
        }
    }
}

/// A handler's signature, the hops of its path and the role nodes after the
/// handler, in path order.
struct Signature {
    text: String,
    hops: Vec<Hop>,
    roles: Vec<(NodeId, &'static str, &'static str)>,
}

/// The walk from `handler` to its first sink (module docs).
fn signature(
    adj: &Adjacency,
    walk: &WalkGraph,
    facts: &Facts<'_>,
    handler: NodeId,
    max_depth: usize,
) -> Signature {
    let bfs = reach::bfs(adj, &[handler], Walk::Forward, max_depth);
    let Some(end) = bfs.reached.iter().find(|r| walk.twins.contains_key(&r.id)) else {
        return Signature {
            text: format!("{HANDLER}>{NO_EFFECT}"),
            hops: Vec::new(),
            roles: Vec::new(),
        };
    };
    let parent: HashMap<NodeId, (NodeId, EdgeCategoryId)> = bfs
        .reached
        .iter()
        .map(|r| (r.id, (r.parent, r.via)))
        .collect();
    let mut hops: Vec<Hop> = Vec::new();
    let mut cur = end.id;
    // Every parent was discovered before its child and the chain ends at the
    // handler, so this walks at most `reached.len()` steps.
    while cur != handler {
        let Some(&(p, via)) = parent.get(&cur) else {
            break;
        };
        hops.push(Hop {
            from: walk.real(p),
            to: walk.real(cur),
            category: via,
            backward: via == edge_category::IMPLEMENTS,
        });
        cur = p;
    }
    hops.reverse();
    let class = walk.twins.get(&end.id).map_or(NO_EFFECT, |&(_, c)| c);
    // The nodes between the handler and the sink.
    let roles: Vec<(NodeId, &'static str, &'static str)> = hops
        .iter()
        .take(hops.len().saturating_sub(1))
        .filter_map(|h| facts.role_of(h.to).map(|(r, s)| (h.to, r, s)))
        .collect();
    let mut parts: Vec<&str> = vec![HANDLER];
    for (_, role, _) in &roles {
        if parts.last() != Some(role) {
            parts.push(role);
        }
    }
    parts.push(class);
    Signature {
        text: parts.join(">"),
        hops,
        roles,
    }
}

/// What the roles read of one node: from the first graph (in
/// `merged.graphs` order) whose nav names it, as `Locator` does.
struct Fact<'a> {
    kind: NodeKindId,
    repo: u64,
    qname: &'a str,
    name: &'a str,
    parent: Option<NodeId>,
    cells: &'a [Cell],
}

/// The per-node facts and edge-derived sets the roles need.
struct Facts<'a> {
    at: HashMap<NodeId, Fact<'a>>,
    /// `(handler, its first server ROUTE)`, sorted by (qname, id).
    handlers: Vec<(NodeId, NodeId)>,
    handler_set: HashSet<NodeId>,
    /// Owners of a handler (its `parent_of`, or a SERVICE that CONTAINS it).
    /// A SERVICE among them is a controller: its members take no role from it.
    controllers: HashSet<NodeId>,
    /// Nodes a non-controller SERVICE CONTAINS.
    service_contained: HashSet<NodeId>,
    accesses_data: HashSet<NodeId>,
    injected: HashSet<NodeId>,
}

impl<'a> Facts<'a> {
    fn build(merged: &'a MergedGraph) -> Self {
        let mut at: HashMap<NodeId, Fact<'a>> = HashMap::new();
        for g in &merged.graphs {
            let mut cells: HashMap<NodeId, &'a [Cell]> = HashMap::with_capacity(g.nodes.len());
            for n in &g.nodes {
                cells.entry(n.id).or_insert(n.cells.as_slice());
            }
            for (id, kind) in &g.nav.kind_by_id {
                at.entry(*id).or_insert_with(|| Fact {
                    kind: *kind,
                    repo: g.repo.0,
                    qname: g.nav.qname_by_id.get(id).map_or("", String::as_str),
                    name: g.nav.name_by_id.get(id).map_or("", String::as_str),
                    parent: g.nav.parent_of.get(id).copied(),
                    cells: cells.get(id).copied().unwrap_or(&[]),
                });
            }
        }
        let mut f = Facts {
            at,
            handlers: Vec::new(),
            handler_set: HashSet::new(),
            controllers: HashSet::new(),
            service_contained: HashSet::new(),
            accesses_data: HashSet::new(),
            injected: HashSet::new(),
        };
        let mut first_route: HashMap<NodeId, NodeId> = HashMap::new();
        let mut contains: Vec<(NodeId, NodeId)> = Vec::new();
        for e in merged.all_edges() {
            match e.category {
                c if c == edge_category::HANDLED_BY => {
                    let server_route =
                        f.at.get(&e.from)
                            .is_some_and(|r| r.kind == node_kind::ROUTE && !is_nav_route(r.cells));
                    if server_route && !first_route.contains_key(&e.to) {
                        first_route.insert(e.to, e.from);
                        f.handlers.push((e.to, e.from));
                    }
                }
                c if c == edge_category::ACCESSES_DATA => {
                    f.accesses_data.insert(e.from);
                }
                c if c == edge_category::INJECTS => {
                    f.injected.insert(e.to);
                }
                c if c == edge_category::CONTAINS && f.is_service(e.from) => {
                    contains.push((e.from, e.to));
                }
                _ => {}
            }
        }
        f.handler_set = f.handlers.iter().map(|(h, _)| *h).collect();
        for (h, _) in &f.handlers {
            if let Some(p) = f.at.get(h).and_then(|x| x.parent) {
                f.controllers.insert(p);
            }
        }
        for (svc, member) in &contains {
            if f.handler_set.contains(member) {
                f.controllers.insert(*svc);
            }
        }
        f.service_contained = contains
            .iter()
            .filter(|(svc, _)| !f.controllers.contains(svc))
            .map(|(_, member)| *member)
            .collect();
        let mut handlers = std::mem::take(&mut f.handlers);
        handlers.sort_by(|a, b| (f.qname(a.0), a.0.0).cmp(&(f.qname(b.0), b.0.0)));
        f.handlers = handlers;
        f
    }

    fn kind(&self, id: NodeId) -> Option<NodeKindId> {
        self.at.get(&id).map(|f| f.kind)
    }

    fn qname(&self, id: NodeId) -> &str {
        self.at.get(&id).map_or("", |f| f.qname)
    }

    /// The node's own roles (kind or ROLE cell) include SERVICE.
    fn is_service(&self, id: NodeId) -> bool {
        self.at
            .get(&id)
            .is_some_and(|f| roles_in(Some(f.kind), f.cells).contains(&node_kind::SERVICE))
    }

    /// The role `id` plays and its source, first rule that applies (module
    /// docs); `None` for a helper.
    fn role_of(&self, id: NodeId) -> Option<(&'static str, &'static str)> {
        if self.handler_set.contains(&id) {
            return Some((HANDLER, EDGE));
        }
        let fact = self.at.get(&id)?;
        let owner = fact.parent;
        let service_owner =
            owner.is_some_and(|o| !self.controllers.contains(&o) && self.is_service(o));
        if self.is_service(id) || self.service_contained.contains(&id) || service_owner {
            return Some((SERVICE, KIND));
        }
        let data_owner = owner.is_some_and(|o| {
            matches!(self.kind(o), Some(k) if k == node_kind::CLASS || k == node_kind::STRUCT)
                && self.accesses_data.contains(&o)
        });
        if self.accesses_data.contains(&id) || data_owner {
            return Some((REPOSITORY, EDGE));
        }
        if self.injected.contains(&id) || owner.is_some_and(|o| self.injected.contains(&o)) {
            return Some((SERVICE, EDGE));
        }
        if NAMED_KINDS.contains(&fact.kind) {
            return name_role(fact.qname, fact.name).map(|r| (r, NAME));
        }
        None
    }
}

/// The name fallback: the name, then the qname's segments innermost first;
/// the first segment that names a role decides.
fn name_role(qname: &str, name: &str) -> Option<&'static str> {
    std::iter::once(name)
        .chain(qname.rsplit([':', '/', '.']))
        .filter(|s| !s.is_empty())
        .find_map(segment_role)
}

/// `(^|_)(repo|repository|repositories|store|dao)$` or
/// `(Repository|Repo|Dao|Store)$` is a repository;
/// `(^|_)(service|services)$` or `Service$` a service.
fn segment_role(seg: &str) -> Option<&'static str> {
    let word = |words: &[&str]| {
        words.iter().any(|w| {
            seg.strip_suffix(w)
                .is_some_and(|rest| rest.is_empty() || rest.ends_with('_'))
        })
    };
    let repo_words = ["repo", "repository", "repositories", "store", "dao"];
    let repo_suffixes = ["Repository", "Repo", "Dao", "Store"];
    if word(&repo_words) || repo_suffixes.iter().any(|s| seg.ends_with(s)) {
        return Some(REPOSITORY);
    }
    if word(&["service", "services"]) || seg.ends_with("Service") {
        return Some(SERVICE);
    }
    None
}

/// An ORIGIN cell names `provenance`.
fn has_provenance(cells: &[Cell], provenance: &str) -> bool {
    let needle = format!("\"provenance\":\"{provenance}\"");
    cells.iter().any(|c| {
        c.kind == cell_type::ORIGIN
            && matches!(&c.payload, CellPayload::Json(j) | CellPayload::Text(j) if j.contains(&needle))
    })
}

/// `(method, path)` of a server ROUTE: `<METHOD> <path>` as named, a legacy
/// `route:<path>` with the verbs of its ROUTE_METHOD cells (`,`-joined in
/// cell order; `None` when it has none).
fn route_of(qname: &str, cells: &[Cell]) -> (Option<String>, Option<String>) {
    let path = nav_route_path(qname).map(str::to_string);
    let (q, _) = split_owner(qname);
    if let Some((m, p)) = q.split_once(' ')
        && p.starts_with('/')
        && !m.is_empty()
        && m.bytes().all(|b| b.is_ascii_uppercase())
    {
        return (Some(m.to_string()), path);
    }
    let mut verbs: Vec<String> = Vec::new();
    for c in cells.iter().filter(|c| c.kind == cell_type::ROUTE_METHOD) {
        let verb = match &c.payload {
            CellPayload::Json(j) => serde_json::from_str::<serde_json::Value>(j)
                .ok()
                .and_then(|v| v.get("method")?.as_str().map(str::to_ascii_uppercase)),
            CellPayload::Text(t) => Some(t.trim().to_ascii_uppercase()),
            CellPayload::Bytes(_) => None,
        };
        if let Some(v) = verb.filter(|v| !v.is_empty() && !verbs.contains(v)) {
            verbs.push(v);
        }
    }
    ((!verbs.is_empty()).then(|| verbs.join(",")), path)
}

/// The LC.3a site an edge was asserted at, 1-based: its evidence file (the
/// source node's file when only the line was recorded) and line.
fn site_of(loc: &Locator<'_>, e: &Edge) -> (Option<String>, Option<i64>) {
    let Some(ev) = Evidence::of(e) else {
        return (None, None);
    };
    let line = ev.line.map(|l| i64::from(l) + 1);
    let file = match ev.file {
        Some(f) => Some(f),
        None if line.is_some() => loc.file_of(e.from),
        None => None,
    };
    match file {
        Some(f) => (Some(f), line),
        None => (None, None),
    }
}

/// The walk's graph (module docs): the forward categories as they are,
/// IMPLEMENTS reversed, edges with a MODULE end left out, and every edge that
/// makes a sink routed to its target's sink-only twin.
struct WalkGraph {
    edges: Vec<Edge>,
    /// Twin id -> (the real sink node, its sink class).
    twins: HashMap<NodeId, (NodeId, &'static str)>,
}

impl WalkGraph {
    fn build(merged: &MergedGraph, facts: &Facts<'_>) -> Self {
        let mut taken: HashSet<u64> = facts.at.keys().map(|id| id.0).collect();
        for e in merged.all_edges() {
            taken.insert(e.from.0);
            taken.insert(e.to.0);
        }
        let module = |id: NodeId| facts.kind(id) == Some(node_kind::MODULE);
        let mut g = WalkGraph {
            edges: Vec::new(),
            twins: HashMap::new(),
        };
        let mut twin_of: HashMap<NodeId, NodeId> = HashMap::new();
        for e in merged.all_edges() {
            if module(e.from) || module(e.to) {
                continue;
            }
            if e.category == edge_category::IMPLEMENTS {
                g.push(e, e.to, e.from);
                continue;
            }
            if !FORWARD.contains(&e.category) {
                continue;
            }
            let sink = facts
                .kind(e.to)
                .and_then(|k| CODE_PROFILE.tables.effect_sink(k, e.category));
            let to = match sink {
                Some((_, s)) => *twin_of.entry(e.to).or_insert_with(|| {
                    let mut t = e.to.0 ^ TWIN_SALT;
                    while !taken.insert(t) {
                        t = t.wrapping_add(TWIN_STEP);
                    }
                    g.twins.insert(NodeId(t), (e.to, s.class));
                    NodeId(t)
                }),
                None => e.to,
            };
            g.push(e, e.from, to);
        }
        g
    }

    fn push(&mut self, e: &Edge, from: NodeId, to: NodeId) {
        self.edges.push(Edge {
            from,
            to,
            category: e.category,
            confidence: e.confidence,
            cells: Vec::new(),
        });
    }

    /// The real node behind a walk node (a twin's target, else itself).
    fn real(&self, id: NodeId) -> NodeId {
        self.twins.get(&id).map_or(id, |(r, _)| *r)
    }
}

impl GraphSource for WalkGraph {
    /// Only the edges are indexed: a node no walk edge touches reaches nothing.
    fn node_ids(&self) -> Vec<NodeId> {
        Vec::new()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_roles_follow_the_spec_patterns() {
        let r = |q: &str| name_role(q, q.rsplit("::").next().unwrap_or(q));
        assert_eq!(r("service::service::GetUser"), Some(SERVICE));
        assert_eq!(r("app::user_service::find"), Some(SERVICE));
        assert_eq!(r("app::services::find"), Some(SERVICE));
        assert_eq!(r("app::UserService::find"), Some(SERVICE));
        assert_eq!(r("repository::repository::FindUser"), Some(REPOSITORY));
        assert_eq!(r("app::UserRepository::find"), Some(REPOSITORY));
        assert_eq!(r("app::user_repo::find"), Some(REPOSITORY));
        assert_eq!(r("app::OrderDao::find"), Some(REPOSITORY));
        assert_eq!(r("app::store::get"), Some(REPOSITORY));
        // Innermost segment first: the repository inside a services package.
        assert_eq!(r("services::user_repo::find"), Some(REPOSITORY));
        // Angular file naming: `user.service` splits on the dot.
        assert_eq!(r("src/app/user.service::UserApi::get"), Some(SERVICE));
        // No word boundary, no role.
        assert_eq!(r("app::restore::run"), None);
        assert_eq!(r("app::servicer::run"), None);
        assert_eq!(r("core::helpers::FooHelper::run"), None);
    }

    #[test]
    fn route_of_reads_named_and_legacy_routes() {
        assert_eq!(
            route_of("POST /orders", &[]),
            (Some("POST".to_string()), Some("/orders".to_string()))
        );
        assert_eq!(
            route_of("GET /orders @services/api", &[]),
            (Some("GET".to_string()), Some("/orders".to_string()))
        );
        let cells = [
            Cell {
                kind: cell_type::ROUTE_METHOD,
                payload: CellPayload::Json(r#"{"method":"post","file":"a.go"}"#.to_string()),
            },
            Cell {
                kind: cell_type::ROUTE_METHOD,
                payload: CellPayload::Text("get".to_string()),
            },
        ];
        assert_eq!(
            route_of("route:/ws", &cells),
            (Some("POST,GET".to_string()), Some("/ws".to_string()))
        );
        assert_eq!(route_of("route:/ws", &[]), (None, Some("/ws".to_string())));
    }

    #[test]
    fn a_backward_hop_keys_the_implements_edge() {
        let h = Hop {
            from: NodeId(1),
            to: NodeId(2),
            category: edge_category::IMPLEMENTS,
            backward: true,
        };
        assert_eq!(
            h.key(),
            EdgeKey {
                from: NodeId(2),
                to: NodeId(1),
                category: edge_category::IMPLEMENTS
            }
        );
    }
}
