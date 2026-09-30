//! Go route mounts (CB.20): route-mount prefixes resolved at build time. A
//! provisional mount ROUTE takes every prefix its parameter or field receives
//! through resolved calls (a fixpoint over the parser's mount facts), one ROUTE
//! per mount; an unmounted one keeps its local path. Crate-private:
//! `build_go_passes` binds them after `resolve_go_calls` and before the refs
//! are resolved.
//!
//! The parser (CB.23) names a ROUTE registered on a router group it cannot
//! read in the file (a parameter- or field-held group) with a provisional
//! qname, `<METHOD> <mount:param:<fn>#<i>><path>` or
//! `<METHOD> <mount:field:<owner>.<field>><path>`
//! ([`glia_code_domain::endpoint::mount_route_qname`]), and records two facts
//! on the fn that holds the group:
//!
//! - [`NavFact::MountArg`]: the call on row `line` passes a mount as argument
//!   `arg` of `callee`. The call's resolved CALLS edge (same `from`, its
//!   EVIDENCE site line, the callee's nav name) names the fn whose parameter
//!   receives it;
//! - [`NavFact::FieldMount`]: the fn assigns a mount to a struct field.
//!
//! [`bind`] runs a fixpoint over those facts (every slot, a parameter or a
//! field, collects the prefixes it receives), then re-keys each provisional
//! ROUTE to `<METHOD> <prefix + path>` once per prefix of its slot: the first
//! through [`rename_nodes`], each further one as a clone carrying the
//! provisional's cells, edges and HANDLED_BY refs, so `resolve_refs` binds
//! every copy's handler. A provisional nobody mounts keeps its local path,
//! the qname the parser minted for it before CB.23. Structural identity only
//! (the A11 const-fold family): no value flows to any sink.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::endpoint::{
    canonical_http_path, join_path, parse_mount_route_qname, route_qname, split_owner, with_owner,
};
use glia_code_domain::evidence::Evidence;
use glia_code_domain::{GRAPH_TYPE, Mount, NavFact, UnresolvedRef, edge_category, node_kind};
use glia_core::{Cell, Confidence, Edge, NodeId};

use crate::build::rename_nodes;
use crate::types::RepoGraph;

/// Fixpoint rounds before the pass stops a still-growing mount (a mount loop:
/// `A` hands its group, suffixed, to `B`, which hands it back to `A`).
const MAX_ROUNDS: usize = 16;

/// Prefixes one slot may hold: a helper mounted by N callers mints N ROUTEs
/// per registration, and past this many the slot stops growing.
const MAX_PREFIXES: usize = 32;

/// What [`bind`] did to one Go graph; [`MountStats::marker`] prints it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct MountStats {
    /// Provisional mount ROUTEs found.
    pub(crate) provisional: usize,
    /// Provisionals whose slot received at least one prefix.
    pub(crate) mounted: usize,
    /// ROUTEs cloned for a slot's second and later prefixes.
    pub(crate) extra_mounts: usize,
    /// Provisionals no resolved call mounts: re-keyed to their local path.
    pub(crate) unmounted: usize,
    /// `MountArg` facts read.
    pub(crate) mount_args: usize,
    /// `MountArg` facts no single resolved CALLS edge answers (the call did
    /// not resolve, or two calls of one name sit on its row): skipped.
    pub(crate) unbound_args: usize,
    /// `FieldMount` facts read.
    pub(crate) field_mounts: usize,
    /// Slots a cap stopped: refused a prefix at [`MAX_PREFIXES`], or still
    /// growing after [`MAX_ROUNDS`].
    pub(crate) capped: usize,
}

impl MountStats {
    /// `[go-mounts] provisional=<p> mounted=<m> extra_mounts=<c>
    /// unmounted=<u> mount_args=<a> unbound_args=<b> field_mounts=<f>
    /// capped=<k>`, once per Go graph with a provisional ROUTE.
    pub(crate) fn marker(&self) -> Option<String> {
        (self.provisional > 0).then(|| {
            format!(
                "[go-mounts] provisional={} mounted={} extra_mounts={} unmounted={} \
                 mount_args={} unbound_args={} field_mounts={} capped={}",
                self.provisional,
                self.mounted,
                self.extra_mounts,
                self.unmounted,
                self.mount_args,
                self.unbound_args,
                self.field_mounts,
                self.capped
            )
        })
    }
}

/// Where a mount's prefixes collect: parameter `index` of the fn with that
/// id (receiver excluded), or field `field` of the struct `owner`
/// (`<package dir qname>::<Type>`, as the parser writes it). Keyed by the raw
/// id so the slot table is a BTreeMap (a NodeId is Hash, not Ord).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Slot {
    Param(u64, u32),
    Field(String, String),
}

/// One mount flowing into a slot: a bound `MountArg` or a `FieldMount`.
struct Flow {
    target: Slot,
    mount: Mount,
}

/// A provisional ROUTE: what its qname says and what it carries.
struct Provisional {
    id: NodeId,
    method: String,
    /// The mount with its suffix folded into `path` (always an empty suffix).
    base: Mount,
    /// The mount's suffix joined with the registration's path, canonical.
    path: String,
    owner: Option<String>,
    confidence: Confidence,
    /// The node's cells, its incident edges and its refs as they were before
    /// any re-key: what each clone copies.
    cells: Vec<Cell>,
    edges: Vec<Edge>,
    refs: Vec<UnresolvedRef>,
}

/// One final ROUTE of a provisional.
struct Final {
    id: NodeId,
    /// With the LB.4a owner, when the provisional had one.
    qname: String,
    /// Without it: the display name every owner-qualified ROUTE carries.
    name: String,
}

/// Re-key every provisional mount ROUTE of the Go graph `g` (see the module
/// doc). `refs` are the graph's pending refs, not yet resolved: a renamed
/// ROUTE's refs move with it, and each clone gets a copy of the provisional's.
/// A no-op on a graph with no provisional ROUTE.
///
/// Deterministic: provisionals in `g.nodes` order, facts by scope in
/// `g.nodes` order (never `nav_facts` map order), BTree slots, finals sorted;
/// clones append to `g.nodes` / `g.edges` / `refs` in that order.
pub(crate) fn bind(g: &mut RepoGraph, refs: &mut Vec<UnresolvedRef>) -> MountStats {
    let mut stats = MountStats::default();
    let provisionals = collect_provisionals(g, refs);
    if provisionals.is_empty() {
        return stats;
    }
    stats.provisional = provisionals.len();

    let fn_ids = param_fn_ids(g, &provisionals);
    let flows = collect_flows(g, &mut stats);
    let slots = fixpoint(&flows, &fn_ids, &mut stats);

    // Finals per provisional, sorted; empty slot -> the local path.
    let mut plans: Vec<Vec<Final>> = Vec::with_capacity(provisionals.len());
    for p in &provisionals {
        let prefixes = slot_of(&p.base, &fn_ids).and_then(|s| slots.get(&s));
        let paths: BTreeSet<String> = match prefixes {
            Some(set) if !set.is_empty() => {
                stats.mounted += 1;
                set.iter()
                    .map(|m| canonical_http_path(&join_path(m, &p.path)).into_owned())
                    .collect()
            }
            _ => {
                stats.unmounted += 1;
                BTreeSet::from([canonical_http_path(&p.path).into_owned()])
            }
        };
        stats.extra_mounts += paths.len().saturating_sub(1);
        let owner = p.owner.as_deref().unwrap_or("");
        plans.push(
            paths
                .into_iter()
                .map(|path| {
                    let name = route_qname(&p.method, &path);
                    let qname = with_owner(&name, owner);
                    let id = NodeId::from_parts(GRAPH_TYPE, g.repo, node_kind::ROUTE, &qname);
                    Final { id, qname, name }
                })
                .collect(),
        );
    }

    // The first final: one batched rename (merges into an existing ROUTE of
    // that id, as two registrations of one full path do).
    let renames: Vec<(NodeId, NodeId)> = provisionals
        .iter()
        .zip(&plans)
        .filter_map(|(p, finals)| finals.first().map(|f| (p.id, f.id)))
        .collect();
    let absorbed = rename_nodes(g, &mut [], refs, &renames);
    let remap: HashMap<NodeId, NodeId> = renames.iter().copied().collect();
    for (p, finals) in provisionals.iter().zip(&plans) {
        if let Some(f) = finals.first()
            && !absorbed.contains(&p.id)
        {
            g.nav.qname_by_id.insert(f.id, f.qname.clone());
            g.nav.name_by_id.insert(f.id, f.name.clone());
        }
    }

    // Every further final: a clone of the provisional as it was.
    let mut at: HashMap<NodeId, usize> =
        g.nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    for (p, finals) in provisionals.iter().zip(&plans) {
        for f in finals.iter().skip(1) {
            clone_route(g, refs, &mut at, &remap, p, f);
        }
    }
    stats
}

/// Every ROUTE whose qname (owner stripped) parses as a provisional mount
/// qname, in `g.nodes` order, with its cells, incident edges and refs.
fn collect_provisionals(g: &RepoGraph, refs: &[UnresolvedRef]) -> Vec<Provisional> {
    let mut out: Vec<Provisional> = Vec::new();
    for n in &g.nodes {
        if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ROUTE) {
            continue;
        }
        let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
            continue;
        };
        let (bare, owner) = split_owner(qname);
        let Some((method, base, path)) = parse_mount_route_qname(bare) else {
            continue;
        };
        out.push(Provisional {
            id: n.id,
            method,
            base,
            path,
            owner: owner.map(str::to_string),
            confidence: n.confidence,
            cells: n.cells.clone(),
            edges: Vec::new(),
            refs: Vec::new(),
        });
    }
    if out.is_empty() {
        return out;
    }
    let index: HashMap<NodeId, usize> = out.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    for e in &g.edges {
        let from = index.get(&e.from).copied();
        let to = index.get(&e.to).copied();
        if let Some(i) = from {
            out[i].edges.push(e.clone());
        }
        if let Some(i) = to
            && Some(i) != from
        {
            out[i].edges.push(e.clone());
        }
    }
    for r in refs {
        if let Some(&i) = index.get(&r.from) {
            out[i].refs.push(r.clone());
        }
    }
    out
}

/// Every fn qname a Param mount names (a provisional's base, a fact's mount)
/// -> the id of the first FUNCTION / METHOD node with that qname, in
/// `g.nodes` order.
fn param_fn_ids(g: &RepoGraph, provisionals: &[Provisional]) -> BTreeMap<String, NodeId> {
    fn want<'a>(wanted: &mut BTreeSet<&'a str>, m: &'a Mount) {
        if let Mount::Param { fn_qname, .. } = m {
            wanted.insert(fn_qname.as_str());
        }
    }
    let mut wanted: BTreeSet<&str> = BTreeSet::new();
    for p in provisionals {
        want(&mut wanted, &p.base);
    }
    for facts in g.nav.nav_facts.values() {
        for f in facts {
            if let NavFact::MountArg { mount, .. } | NavFact::FieldMount { mount, .. } = f {
                want(&mut wanted, mount);
            }
        }
    }
    let mut ids: BTreeMap<String, NodeId> = BTreeMap::new();
    if wanted.is_empty() {
        return ids;
    }
    for n in &g.nodes {
        let kind = g.nav.kind_by_id.get(&n.id);
        if kind != Some(&node_kind::FUNCTION) && kind != Some(&node_kind::METHOD) {
            continue;
        }
        if let Some(q) = g.nav.qname_by_id.get(&n.id)
            && wanted.contains(q.as_str())
        {
            ids.entry(q.clone()).or_insert(n.id);
        }
    }
    ids
}

/// The slot a mount reads: `None` for a Const (it holds its prefix itself)
/// and for a Param whose fn is not a node of this graph.
fn slot_of(mount: &Mount, fn_ids: &BTreeMap<String, NodeId>) -> Option<Slot> {
    match mount {
        Mount::Const(_) => None,
        Mount::Param {
            fn_qname, index, ..
        } => fn_ids.get(fn_qname).map(|id| Slot::Param(id.0, *index)),
        Mount::Field { owner, field, .. } => Some(Slot::Field(owner.clone(), field.clone())),
    }
}

/// Every mount fact as a flow into its slot, scopes in `g.nodes` order and
/// each scope's facts in record order. A `MountArg` flows into parameter
/// `arg` of the one fn its call site's CALLS edge reaches, matched on
/// `(caller, EVIDENCE site line, callee nav name)`; no such edge, or two
/// distinct targets for that key, leaves it unbound.
fn collect_flows(g: &RepoGraph, stats: &mut MountStats) -> Vec<Flow> {
    let callers: HashSet<NodeId> = g
        .nav
        .nav_facts
        .iter()
        .filter(|(_, facts)| facts.iter().any(|f| matches!(f, NavFact::MountArg { .. })))
        .map(|(scope, _)| *scope)
        .collect();
    let mut calls: HashMap<(NodeId, u32, &str), Vec<NodeId>> = HashMap::new();
    if !callers.is_empty() {
        for e in &g.edges {
            if e.category != edge_category::CALLS || !callers.contains(&e.from) {
                continue;
            }
            let Some(line) = Evidence::of(e).and_then(|ev| ev.line) else {
                continue;
            };
            let Some(name) = g.nav.name_by_id.get(&e.to) else {
                continue;
            };
            let targets = calls.entry((e.from, line, name.as_str())).or_default();
            if !targets.contains(&e.to) {
                targets.push(e.to);
            }
        }
    }

    let mut flows: Vec<Flow> = Vec::new();
    for n in &g.nodes {
        let Some(facts) = g.nav.nav_facts.get(&n.id) else {
            continue;
        };
        for f in facts {
            match f {
                NavFact::MountArg {
                    line,
                    callee,
                    arg,
                    mount,
                } => {
                    stats.mount_args += 1;
                    match calls
                        .get(&(n.id, *line, callee.as_str()))
                        .map(Vec::as_slice)
                    {
                        Some([to]) => flows.push(Flow {
                            target: Slot::Param(to.0, *arg),
                            mount: mount.clone(),
                        }),
                        _ => stats.unbound_args += 1,
                    }
                }
                NavFact::FieldMount {
                    owner,
                    field,
                    mount,
                } => {
                    stats.field_mounts += 1;
                    flows.push(Flow {
                        target: Slot::Field(owner.clone(), field.clone()),
                        mount: mount.clone(),
                    });
                }
                _ => {}
            }
        }
    }
    flows
}

/// The prefixes `mount` stands for, given the slots so far: a Const is its
/// prefix; a Param / Field is each prefix of its slot, then its suffix. An
/// empty suffix adds nothing (`join_path("/api", "")` would be `/api/`).
fn resolve(
    mount: &Mount,
    slots: &BTreeMap<Slot, BTreeSet<String>>,
    fn_ids: &BTreeMap<String, NodeId>,
) -> BTreeSet<String> {
    let suffix = match mount {
        Mount::Const(p) => return BTreeSet::from([p.clone()]),
        Mount::Param { suffix, .. } | Mount::Field { suffix, .. } => suffix,
    };
    let Some(prefixes) = slot_of(mount, fn_ids).and_then(|s| slots.get(&s)) else {
        return BTreeSet::new();
    };
    prefixes
        .iter()
        .map(|m| {
            if suffix.is_empty() {
                m.clone()
            } else {
                join_path(m, suffix)
            }
        })
        .collect()
}

/// Grow every slot by what its flows carry until none grows, in flow order
/// (a round sees the prefixes an earlier flow of the same round added). A
/// slot holds at most [`MAX_PREFIXES`]. After [`MAX_ROUNDS`] the slots stay
/// as they are, and one more round, run on a copy, names the ones still
/// growing: counted capped with those a full slot refused.
fn fixpoint(
    flows: &[Flow],
    fn_ids: &BTreeMap<String, NodeId>,
    stats: &mut MountStats,
) -> BTreeMap<Slot, BTreeSet<String>> {
    let mut slots: BTreeMap<Slot, BTreeSet<String>> = BTreeMap::new();
    let mut capped: BTreeSet<Slot> = BTreeSet::new();
    let mut grew = !flows.is_empty();
    for _ in 0..MAX_ROUNDS {
        grew = !round(&mut slots, flows, fn_ids, &mut capped).is_empty();
        if !grew {
            break;
        }
    }
    if grew {
        let (mut next, mut refused) = (slots.clone(), BTreeSet::new());
        capped.extend(round(&mut next, flows, fn_ids, &mut refused));
        capped.extend(refused);
    }
    stats.capped = capped.len();
    slots
}

/// One round of [`fixpoint`]: every flow in order adds what its mount
/// resolves to now. Returns the slots that grew; a slot that refused a
/// prefix at [`MAX_PREFIXES`] goes in `full`.
fn round(
    slots: &mut BTreeMap<Slot, BTreeSet<String>>,
    flows: &[Flow],
    fn_ids: &BTreeMap<String, NodeId>,
    full: &mut BTreeSet<Slot>,
) -> BTreeSet<Slot> {
    let mut grew: BTreeSet<Slot> = BTreeSet::new();
    for flow in flows {
        let incoming = resolve(&flow.mount, slots, fn_ids);
        let slot = slots.entry(flow.target.clone()).or_default();
        for p in incoming {
            if slot.contains(&p) {
                continue;
            }
            if slot.len() >= MAX_PREFIXES {
                full.insert(flow.target.clone());
                break;
            }
            slot.insert(p);
            grew.insert(flow.target.clone());
        }
    }
    grew
}

/// Add final `f` of provisional `p` as its own ROUTE: `p`'s cells, a nav
/// record (the parser's ROUTE shape: no parent), a copy of each of `p`'s
/// edges with `f` for `p` (the other end through `remap`, the first-final
/// rename) and a copy of each of `p`'s refs from `f`. An id already in the
/// graph (another provisional's final, or a ROUTE registered with that full
/// path) takes the cells, as `merge_parses` merges a duplicate id, and keeps
/// its nav record.
fn clone_route(
    g: &mut RepoGraph,
    refs: &mut Vec<UnresolvedRef>,
    at: &mut HashMap<NodeId, usize>,
    remap: &HashMap<NodeId, NodeId>,
    p: &Provisional,
    f: &Final,
) {
    match at.get(&f.id) {
        Some(&i) => g.nodes[i].cells.extend(p.cells.iter().cloned()),
        None => {
            at.insert(f.id, g.nodes.len());
            g.nodes.push(glia_core::Node {
                id: f.id,
                repo: g.repo,
                confidence: p.confidence,
                cells: p.cells.clone(),
            });
            g.nav
                .record(f.id, &f.name, &f.qname, node_kind::ROUTE, None);
        }
    }
    let end = |id: NodeId| {
        if id == p.id {
            f.id
        } else {
            remap.get(&id).copied().unwrap_or(id)
        }
    };
    for e in &p.edges {
        let mut copy = e.clone();
        copy.from = end(e.from);
        copy.to = end(e.to);
        g.edges.push(copy);
    }
    for r in &p.refs {
        let mut copy = r.clone();
        copy.from = f.id;
        refs.push(copy);
    }
}

#[cfg(test)]
mod tests {
    //! Synthetic Go graphs: the Go parser supplies the functions and the call
    //! sites (so CALLS edges resolve and carry their real site rows); each
    //! test adds what CB.23's parser half records, the provisional ROUTEs
    //! ([`mount_route_qname`]) and the mount facts
    //! ([`glia_code_domain::CodeNav::record_fact`]). No source here registers
    //! a route or takes a router-typed parameter, so the parser itself records
    //! neither.

    use glia_code_domain::endpoint::mount_route_qname;
    use glia_code_domain::{CallQualifier, FileParse, cell_type};
    use glia_core::{CellPayload, Node, RepoId};

    use super::*;
    use crate::build::build_go_with_mounts;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://go_mounts")
    }

    /// The engine's `path_to_qname`: drop `.go`, `/` -> `::`.
    fn module_qname(rel: &str) -> String {
        rel.strip_suffix(".go").unwrap_or(rel).replace('/', "::")
    }

    fn parse(rel: &str, src: &str) -> FileParse {
        glia_parser_go::parse_file(src, rel, &module_qname(rel), "example.com/app", repo())
            .expect("parse")
    }

    fn id(kind: glia_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn func(qname: &str) -> NodeId {
        id(node_kind::FUNCTION, qname)
    }

    fn route(qname: &str) -> NodeId {
        id(node_kind::ROUTE, qname)
    }

    /// The 0-based row of the first line of `src` holding `needle`.
    fn row(src: &str, needle: &str) -> u32 {
        let at = src
            .lines()
            .position(|l| l.contains(needle))
            .expect("needle");
        u32::try_from(at).expect("row")
    }

    /// A ROUTE the parser (CB.23) would mint on `mount`: the provisional
    /// qname with the LB.4a owner, its display name without it, one
    /// ROUTE_METHOD cell and a Bare HANDLED_BY ref to `handler` on `line`.
    fn provisional(
        fp: &mut FileParse,
        method: &str,
        mount: &Mount,
        local: &str,
        owner: &str,
        handler: &str,
        line: u32,
    ) -> NodeId {
        let module = id(node_kind::MODULE, &file_module(fp));
        let qname = with_owner(&mount_route_qname(method, mount, local), owner);
        let name = split_owner(&qname).0.to_string();
        let rid = route(&qname);
        fp.nodes.push(Node {
            id: rid,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::ROUTE_METHOD,
                payload: CellPayload::Json(format!(r#"{{"method":"{method}","line":{line}}}"#)),
            }],
        });
        fp.nav.record(rid, &name, &qname, node_kind::ROUTE, None);
        fp.refs.push(UnresolvedRef {
            from: rid,
            from_module: module,
            qualifier: CallQualifier::Bare(handler.to_string()),
            category: edge_category::HANDLED_BY,
            line,
        });
        rid
    }

    fn file_module(fp: &FileParse) -> String {
        fp.nodes
            .iter()
            .find(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE))
            .and_then(|n| fp.nav.qname_by_id.get(&n.id))
            .cloned()
            .expect("file module")
    }

    fn mount_arg(fp: &mut FileParse, scope: &str, line: u32, callee: &str, mount: Mount) {
        fp.nav.record_fact(
            func(scope),
            NavFact::MountArg {
                line,
                callee: callee.to_string(),
                arg: 0,
                mount,
            },
        );
    }

    fn param(fn_qname: &str, suffix: &str) -> Mount {
        Mount::Param {
            fn_qname: fn_qname.to_string(),
            index: 0,
            suffix: suffix.to_string(),
        }
    }

    fn konst(p: &str) -> Mount {
        Mount::Const(p.to_string())
    }

    /// Every ROUTE qname, in `g.nodes` order.
    fn routes(g: &RepoGraph) -> Vec<String> {
        g.nodes
            .iter()
            .filter(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ROUTE))
            .filter_map(|n| g.nav.qname_by_id.get(&n.id).cloned())
            .collect()
    }

    fn handlers(g: &RepoGraph, from: NodeId) -> Vec<NodeId> {
        g.edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::HANDLED_BY)
            .map(|e| e.to)
            .collect()
    }

    /// Every provisional is gone: nodes, nav and refs.
    fn assert_no_provisional(g: &RepoGraph) {
        assert!(
            g.nav.qname_by_id.values().all(|q| !q.contains("<mount:")),
            "provisional qname left: {:?}",
            routes(g)
        );
        assert!(g.nav.name_by_id.values().all(|q| !q.contains("<mount:")));
        let ids: HashSet<NodeId> = g.nodes.iter().map(|n| n.id).collect();
        assert!(
            g.nav.kind_by_id.keys().all(|k| ids.contains(k)),
            "nav record without a node"
        );
        assert!(
            g.edges
                .iter()
                .all(|e| ids.contains(&e.from) && ids.contains(&e.to))
        );
        assert!(
            g.unresolved_refs.iter().all(|r| ids.contains(&r.from)),
            "ref from a dropped id"
        );
    }

    const SERVER: &str = "package server

func SetupRouter() {
\tRegisterUser(nil)
}

func RegisterUser(rg any) {
}

func Get2FA() {}
";

    /// quokka-stack's shape: `SetupRouter` builds `/api/protected` and passes
    /// it to `RegisterUser`, whose `rg.GET("/user/2fa", ..)` the parser can
    /// only name on its parameter.
    #[test]
    fn param_mount_prefixes_the_route() {
        let mut fp = parse("server/server.go", SERVER);
        mount_arg(
            &mut fp,
            "server::server::SetupRouter",
            row(SERVER, "RegisterUser(nil)"),
            "RegisterUser",
            konst("/api/protected"),
        );
        let reg = param("server::server::RegisterUser", "");
        let old = provisional(
            &mut fp,
            "GET",
            &reg,
            "/user/2fa",
            "",
            "Get2FA",
            row(SERVER, "func RegisterUser"),
        );
        let (g, stats) = build_go_with_mounts(repo(), vec![fp]);

        assert_eq!(routes(&g), vec!["GET /api/protected/user/2fa"]);
        let r = route("GET /api/protected/user/2fa");
        assert!(g.nodes.iter().all(|n| n.id != old));
        assert_eq!(g.nav.name_by_id[&r], "GET /api/protected/user/2fa");
        assert_eq!(g.nav.kind_by_id[&r], node_kind::ROUTE);
        assert!(!g.nav.parent_of.contains_key(&r));
        assert_eq!(handlers(&g, r), vec![func("server::server::Get2FA")]);
        assert_no_provisional(&g);
        assert_eq!(
            stats,
            MountStats {
                provisional: 1,
                mounted: 1,
                mount_args: 1,
                ..MountStats::default()
            }
        );
        assert_eq!(
            stats.marker().as_deref(),
            Some(
                "[go-mounts] provisional=1 mounted=1 extra_mounts=0 unmounted=0 mount_args=1 \
                 unbound_args=0 field_mounts=0 capped=0"
            )
        );
    }

    /// One register function mounted on two groups: two ROUTEs, each with its
    /// own HANDLED_BY (the ref cloned), the ROUTE cells on both.
    #[test]
    fn two_mounts_two_routes() {
        let src = "package api

func Setup() {
\tRegisterUsers(nil)
\tRegisterUsers(nil)
}

func RegisterUsers(rg any) {
}

func ListUsers() {}
";
        let mut fp = parse("api/api.go", src);
        let first = row(src, "RegisterUsers(nil)");
        mount_arg(
            &mut fp,
            "api::api::Setup",
            first,
            "RegisterUsers",
            konst("/api/v2"),
        );
        mount_arg(
            &mut fp,
            "api::api::Setup",
            first + 1,
            "RegisterUsers",
            konst("/api/v1"),
        );
        let reg = param("api::api::RegisterUsers", "");
        provisional(
            &mut fp,
            "GET",
            &reg,
            "/users",
            "",
            "ListUsers",
            row(src, "func RegisterUsers"),
        );
        let (g, stats) = build_go_with_mounts(repo(), vec![fp]);

        // Sorted finals: the first is the renamed node, the second the clone.
        assert_eq!(routes(&g), vec!["GET /api/v1/users", "GET /api/v2/users"]);
        let list = func("api::api::ListUsers");
        for q in ["GET /api/v1/users", "GET /api/v2/users"] {
            let r = route(q);
            assert_eq!(handlers(&g, r), vec![list], "{q}");
            assert_eq!(g.nav.name_by_id[&r], q);
            let n = g.nodes.iter().find(|n| n.id == r).expect("route node");
            assert_eq!(n.cells.len(), 1);
            assert_eq!(n.cells[0].kind, cell_type::ROUTE_METHOD);
        }
        // Each cloned HANDLED_BY carries its EVIDENCE cell, as every route's.
        assert!(
            g.edges
                .iter()
                .filter(|e| e.category == edge_category::HANDLED_BY)
                .all(|e| Evidence::of(e).is_some())
        );
        assert_no_provisional(&g);
        assert_eq!(
            (stats.mounted, stats.extra_mounts, stats.mount_args),
            (1, 1, 2)
        );
    }

    /// `A` mounts `/api` on `B`; `B` hands `rg.Group("/users")` to `C`.
    #[test]
    fn chained_param_mounts() {
        let src = "package chain

func A() {
\tB(nil)
}

func B(rg any) {
\tC(rg)
}

func C(rg any) {
}

func GetUser() {}
";
        let mut fp = parse("chain/chain.go", src);
        mount_arg(
            &mut fp,
            "chain::chain::A",
            row(src, "B(nil)"),
            "B",
            konst("/api"),
        );
        mount_arg(
            &mut fp,
            "chain::chain::B",
            row(src, "C(rg)"),
            "C",
            param("chain::chain::B", "/users"),
        );
        let c = param("chain::chain::C", "");
        provisional(
            &mut fp,
            "GET",
            &c,
            "/:id",
            "",
            "GetUser",
            row(src, "func C"),
        );
        let (g, stats) = build_go_with_mounts(repo(), vec![fp]);

        assert_eq!(routes(&g), vec!["GET /api/users/:id"]);
        assert_eq!(
            handlers(&g, route("GET /api/users/:id")),
            vec![func("chain::chain::GetUser")]
        );
        assert_no_provisional(&g);
        assert_eq!(
            (stats.mounted, stats.mount_args, stats.unbound_args),
            (1, 2, 0)
        );
    }

    /// `main` passes the bare engine to `api.NewServer`, which stores
    /// `r.Group("/admin")` in `s.admin`; a method in ANOTHER file of the
    /// package registers on `s.admin`.
    #[test]
    fn field_mount_across_files() {
        let main = "package main

import \"example.com/app/api\"

func main() {
\tapi.NewServer(nil)
}
";
        let server = "package api

type Server struct {
\tadmin any
}

func NewServer(r any) *Server {
\treturn &Server{}
}
";
        let admin = "package api

func (s *Server) adminRoutes() {
}

func stats() {}
";
        let mut m = parse("cmd/main.go", main);
        mount_arg(
            &mut m,
            "cmd::main::main",
            row(main, "api.NewServer"),
            "NewServer",
            konst(""),
        );
        let mut s = parse("api/server.go", server);
        s.nav.record_fact(
            func("api::server::NewServer"),
            NavFact::FieldMount {
                owner: "api::Server".into(),
                field: "admin".into(),
                mount: param("api::server::NewServer", "/admin"),
            },
        );
        let mut a = parse("api/admin_routes.go", admin);
        let field = Mount::Field {
            owner: "api::Server".into(),
            field: "admin".into(),
            suffix: String::new(),
        };
        provisional(
            &mut a,
            "GET",
            &field,
            "/stats",
            "",
            "stats",
            row(admin, "adminRoutes"),
        );
        let (g, stats) = build_go_with_mounts(repo(), vec![m, s, a]);

        assert_eq!(routes(&g), vec!["GET /admin/stats"]);
        assert_eq!(
            handlers(&g, route("GET /admin/stats")),
            vec![func("api::admin_routes::stats")]
        );
        assert_no_provisional(&g);
        assert_eq!(
            stats,
            MountStats {
                provisional: 1,
                mounted: 1,
                mount_args: 1,
                field_mounts: 1,
                ..MountStats::default()
            }
        );
    }

    /// No call mounts the register function: the ROUTE keeps its local path,
    /// the qname (and so the id) the parser minted before CB.23.
    #[test]
    fn unmounted_keeps_local_path() {
        let src = "package health

func RegisterHealth(rg any) {
}

func Healthz() {}
";
        let mut fp = parse("health/health.go", src);
        let reg = param("health::health::RegisterHealth", "");
        provisional(
            &mut fp,
            "GET",
            &reg,
            "/healthz",
            "",
            "Healthz",
            row(src, "func RegisterHealth"),
        );
        let (g, stats) = build_go_with_mounts(repo(), vec![fp]);

        let head = route("GET /healthz");
        assert_eq!(routes(&g), vec!["GET /healthz"]);
        assert!(g.nodes.iter().any(|n| n.id == head));
        assert_eq!(g.nav.name_by_id[&head], "GET /healthz");
        assert_eq!(handlers(&g, head), vec![func("health::health::Healthz")]);
        assert_no_provisional(&g);
        assert_eq!(
            stats,
            MountStats {
                provisional: 1,
                unmounted: 1,
                ..MountStats::default()
            }
        );
    }

    /// An LB.4a owner (`@turps`) is split off before parsing and put back on
    /// every final; the display name carries none.
    #[test]
    fn owner_suffix_survives() {
        let mut fp = parse("server/server.go", SERVER);
        mount_arg(
            &mut fp,
            "server::server::SetupRouter",
            row(SERVER, "RegisterUser(nil)"),
            "RegisterUser",
            konst("/api/protected"),
        );
        let reg = param("server::server::RegisterUser", "");
        provisional(
            &mut fp,
            "POST",
            &reg,
            "/activity",
            "turps",
            "Get2FA",
            row(SERVER, "func RegisterUser"),
        );
        let (g, _) = build_go_with_mounts(repo(), vec![fp]);

        let r = route("POST /api/protected/activity @turps");
        assert_eq!(routes(&g), vec!["POST /api/protected/activity @turps"]);
        assert_eq!(g.nav.name_by_id[&r], "POST /api/protected/activity");
        assert_eq!(handlers(&g, r), vec![func("server::server::Get2FA")]);
        assert_no_provisional(&g);
    }

    /// `A` hands its group, suffixed, to `B`, and `B` hands it back: the round
    /// cap stops the loop and both slots count as capped.
    #[test]
    fn cap_stops_a_loop() {
        let src = "package loop

func Main() {
\tA(nil)
}

func A(rg any) {
\tB(rg)
}

func B(rg any) {
\tA(rg)
}

func H() {}
";
        let mut fp = parse("loop/loop.go", src);
        mount_arg(
            &mut fp,
            "loop::loop::Main",
            row(src, "A(nil)"),
            "A",
            konst("/api"),
        );
        mount_arg(
            &mut fp,
            "loop::loop::A",
            row(src, "B(rg)"),
            "B",
            param("loop::loop::A", "/b"),
        );
        mount_arg(
            &mut fp,
            "loop::loop::B",
            row(src, "A(rg)"),
            "A",
            param("loop::loop::B", "/a"),
        );
        let a = param("loop::loop::A", "");
        provisional(&mut fp, "GET", &a, "/x", "", "H", row(src, "func A"));
        let (g, stats) = build_go_with_mounts(repo(), vec![fp]);

        let all = routes(&g);
        assert!(all.len() > 1 && all.len() <= MAX_PREFIXES, "{all:?}");
        assert!(
            all.iter()
                .all(|q| q.starts_with("GET /api") && q.ends_with("/x")),
            "{all:?}"
        );
        assert!(all.contains(&"GET /api/x".to_string()));
        assert!(all.contains(&"GET /api/b/a/x".to_string()));
        assert_eq!(stats.capped, 2);
        assert_eq!(stats.extra_mounts, all.len() - 1);
        assert_no_provisional(&g);
        // Every copy is handled by H.
        for q in &all {
            assert_eq!(handlers(&g, route(q)), vec![func("loop::loop::H")], "{q}");
        }
    }

    /// A helper mounted by more callers than a slot holds mints at most
    /// MAX_PREFIXES ROUTEs; the one slot counts as capped.
    #[test]
    fn cap_stops_an_explosion() {
        let calls = MAX_PREFIXES + 8;
        let mut src = String::from("package many\n\nfunc Setup() {\n");
        for _ in 0..calls {
            src.push_str("\tRegister(nil)\n");
        }
        src.push_str("}\n\nfunc Register(rg any) {\n}\n\nfunc H() {}\n");
        let mut fp = parse("many/many.go", &src);
        let first = row(&src, "Register(nil)");
        for i in 0..calls {
            let line = first + u32::try_from(i).expect("row");
            mount_arg(
                &mut fp,
                "many::many::Setup",
                line,
                "Register",
                konst(&format!("/v{i:02}")),
            );
        }
        let reg = param("many::many::Register", "");
        provisional(
            &mut fp,
            "GET",
            &reg,
            "/x",
            "",
            "H",
            row(&src, "func Register"),
        );
        let (g, stats) = build_go_with_mounts(repo(), vec![fp]);

        let all = routes(&g);
        assert_eq!(all.len(), MAX_PREFIXES);
        assert_eq!(all.first().map(String::as_str), Some("GET /v00/x"));
        assert_eq!(stats.capped, 1);
        assert_eq!(stats.mount_args, calls);
        assert_no_provisional(&g);
    }

    /// A fact whose row holds no resolved call of that name binds nothing: it
    /// is counted, and the route stays at its local path.
    #[test]
    fn unbound_arg_is_counted() {
        let mut fp = parse("server/server.go", SERVER);
        let wrong_row = row(SERVER, "func Get2FA");
        mount_arg(
            &mut fp,
            "server::server::SetupRouter",
            wrong_row,
            "RegisterUser",
            konst("/api"),
        );
        mount_arg(
            &mut fp,
            "server::server::SetupRouter",
            row(SERVER, "RegisterUser(nil)"),
            "Elsewhere",
            konst("/api"),
        );
        let reg = param("server::server::RegisterUser", "");
        provisional(
            &mut fp,
            "GET",
            &reg,
            "/me",
            "",
            "Get2FA",
            row(SERVER, "func RegisterUser"),
        );
        let (g, stats) = build_go_with_mounts(repo(), vec![fp]);

        assert_eq!(routes(&g), vec!["GET /me"]);
        assert_eq!(
            (stats.mount_args, stats.unbound_args, stats.unmounted),
            (2, 2, 1)
        );
    }

    /// A mounted route whose full path a body-local registration already has
    /// merges into that ROUTE: one node, both registrations' cells, both
    /// handlers.
    #[test]
    fn merges_into_an_existing_route() {
        let src = "package api

func Setup() {
\tRegister(nil)
}

func Register(rg any) {
}

func Old() {}

func New() {}
";
        let mut fp = parse("api/api.go", src);
        mount_arg(
            &mut fp,
            "api::api::Setup",
            row(src, "Register(nil)"),
            "Register",
            konst("/api"),
        );
        // The body-local registration: a plain ROUTE on the full path.
        provisional(
            &mut fp,
            "GET",
            &konst("/api"),
            "/users",
            "",
            "Old",
            row(src, "func Setup"),
        );
        let reg = param("api::api::Register", "");
        provisional(
            &mut fp,
            "GET",
            &reg,
            "/users",
            "",
            "New",
            row(src, "func Register"),
        );
        let (g, stats) = build_go_with_mounts(repo(), vec![fp]);

        let r = route("GET /api/users");
        assert_eq!(routes(&g), vec!["GET /api/users"]);
        let n = g.nodes.iter().find(|n| n.id == r).expect("route");
        assert_eq!(n.cells.len(), 2, "both registrations' cells");
        assert_eq!(
            handlers(&g, r),
            vec![func("api::api::Old"), func("api::api::New")]
        );
        assert_eq!(g.nav.name_by_id[&r], "GET /api/users");
        assert_no_provisional(&g);
        assert_eq!((stats.provisional, stats.mounted), (1, 1));
    }

    /// Two builds of one input give the same nodes, edges and nav, in order.
    #[test]
    fn mounts_are_deterministic() {
        let build = || {
            let src = "package api

func Setup() {
\tRegister(nil)
\tRegister(nil)
\tRegister(nil)
}

func Register(rg any) {
\tInner(nil)
}

func Inner(rg any) {
}

func H() {}
";
            let mut fp = parse("api/api.go", src);
            let first = row(src, "Register(nil)");
            for (i, p) in ["/c", "/a", "/b"].iter().enumerate() {
                let line = first + u32::try_from(i).expect("row");
                mount_arg(&mut fp, "api::api::Setup", line, "Register", konst(p));
            }
            mount_arg(
                &mut fp,
                "api::api::Register",
                row(src, "Inner(nil)"),
                "Inner",
                param("api::api::Register", "/in"),
            );
            provisional(
                &mut fp,
                "GET",
                &param("api::api::Register", ""),
                "/r",
                "",
                "H",
                row(src, "func Register"),
            );
            provisional(
                &mut fp,
                "POST",
                &param("api::api::Inner", ""),
                "/i",
                "",
                "H",
                row(src, "func Inner"),
            );
            build_go_with_mounts(repo(), vec![fp])
        };
        let (a, sa) = build();
        let (b, sb) = build();
        assert_eq!(sa, sb);
        assert_eq!(a.nodes, b.nodes);
        assert_eq!(a.edges, b.edges);
        assert_eq!(routes(&a), routes(&b));
        assert_eq!(
            routes(&a),
            vec![
                "GET /a/r",
                "POST /a/in/i",
                "GET /b/r",
                "GET /c/r",
                "POST /b/in/i",
                "POST /c/in/i",
            ]
        );
        assert_eq!((sa.provisional, sa.mounted, sa.extra_mounts), (2, 2, 4));
    }
}
