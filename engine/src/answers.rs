// ============================================================================
// P3 answer-shaped primitives (handoff v6) — rank + locate + why, in the ENGINE
// so the CLI, pyo3/MCP, and future TUI/3d-viewer all share one implementation.
// ============================================================================

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use glia_activation::algo::{Adjacency, CategorySet, GraphSource, Walk, reach};
use glia_activation::plan::{ActivationPlan, FilterPredicate};
use glia_code_domain::{cell_type, edge_category, endpoint, node_kind};
use glia_code_extractors::queues::is_framework_tag;
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId};
use glia_graph::{MergedGraph, Reach, RepoGraph};

use crate::absence::{self, Absence, Answer};
use crate::find::{self, FindOptions};
use crate::identity::{Collapse, IdentityView, RowIdentity};
use crate::profile::CODE_PROFILE;
use crate::rank::{FileIndex, FileRanking, RankCache, RankedFile, Ranking, SourceKey, order_rows, rank_files};

/// One node in a blast-radius answer: identity, kind, why it is here
/// (`reason`), PPR `score`, `file`:`line`, `live`, the `seed` whose wave
/// reached it, and — for a row that stands for a FACT / DERIVED identity
/// class (DD.10) — the class's `identity`. Serialized straight to pyo3/CLI.
/// Produced by [`blast_radius`], never built by a struct literal outside this
/// crate (LD.9):
///
/// ```compile_fail
/// let _ = glia_engine::BlastAnswer {
///     id: 0,
///     qname: String::new(),
///     name: String::new(),
///     kind: "",
///     reason: "",
///     depth: 0,
///     score: 0.0,
///     live: false,
///     file: None,
///     line: None,
///     seed: String::new(),
///     identity: None,
/// };
/// ```
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct BlastAnswer {
    pub id: u64,
    pub qname: String,
    pub name: String,
    pub kind: &'static str,
    /// Edge category that first put this node in scope ("why it's in the radius").
    pub reason: &'static str,
    pub depth: usize,
    pub score: f64,
    /// Reachable from an entrypoint (route/handler/main/test/component) — `false`
    /// = likely dead. Best-effort; annotated, not filtered, unless `live_only`.
    pub live: bool,
    pub file: Option<String>,
    /// 1-based (see [`Located`]).
    pub line: Option<i64>,
    /// The qname of the seed whose wave reached this node first (LD.5): with
    /// one seed, that seed; with several, the root of the node's parent chain
    /// in the one multi-seed walk.
    pub seed: String,
    /// DD.10: this row stands for a FACT / DERIVED identity class (one
    /// contract op, message, SCIP symbol or package in several places): its
    /// tier, rules, whether the members' contents differ, and every other
    /// member (`also_in`). Absent from the JSON outside a class, so such a
    /// row serialises exactly as before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<RowIdentity>,
}

/// Types whose liveness a live METHOD they declare implies (A7.8): the
/// method's direct `parent_of` must be one of these for the owner step to fire.
const OWNER_KINDS: [glia_core::NodeKindId; 3] =
    [node_kind::CLASS, node_kind::STRUCT, node_kind::INTERFACE];

/// One liveness walk: the live set plus how each part of it got there, for
/// the `[live]` / `[invoked]` markers and the unit tests.
struct LiveWalk {
    live: std::collections::HashSet<NodeId>,
    /// Entries by kind / name.
    by_kind: usize,
    /// Entries only a ROLE cell made (LB.3b).
    by_role: usize,
    /// Entries only an ENTRYPOINT cell made: a declared one (LF.3b) or one a
    /// parser read (DT.5l, a Python `__main__` guard's MODULE).
    by_decl: usize,
    /// Types made live by a live METHOD they declare.
    owners: usize,
    /// Nodes made live by an IMPLEMENTS edge into a live node.
    implementers: usize,
    /// Members first reached over an invoked-member hop from their live
    /// owner (DT.5d): lifecycle hooks, framework-bound members, constructors.
    invoked: usize,
    /// DT.5l: the [`invoked_members`] rows per framework tag, every tag of
    /// `INVOKED_MEMBERS` in first-appearance order (zeros included), and
    /// their total.
    invoked_rows: Vec<(&'static str, usize)>,
    /// DT.5l: METHODs a signature-checked row named whose signature or
    /// siblings refused them ([`invoked_members_counted`]).
    sig_rejected: usize,
    /// Every node in the merged graph.
    total: usize,
}

/// DT.5d: every METHOD a framework or the runtime invokes on an instance of
/// its owner, as `(member, owner, framework)`: a METHOD whose direct
/// `parent_of` is an [`OWNER_KINDS`] node and that
/// [`glia_code_domain::profile::invoked_member_at`] matches, given the
/// owner's roles (`glia_graph::roles::roles_in` of its kind and cells), both
/// names and the extension of the METHOD's POSITION file
/// ([`glia_code_domain::profile::node_ext`]). A METHOD with no POSITION file
/// matches nothing.
///
/// The one matcher [`live_walk`] (the invoked hop) and `gaps` (the
/// follows-its-owner rule) share. Built in `merged.graphs` / `g.nodes` order,
/// each METHOD once (its first instance); the name gate runs before the file
/// and the roles are read, so a graph pays the JSON reads only for the few
/// METHODs a row names. [`invoked_members_counted`] is the same list with the
/// signature rejections counted.
pub(crate) fn invoked_members(merged: &MergedGraph) -> Vec<(NodeId, NodeId, &'static str)> {
    invoked_members_counted(merged).0
}

/// DT.5l: [`invoked_members`], plus how many METHODs a signature-checked row
/// (`go_runtime`) names, with the name, extension and owner gates passing,
/// that no row matched because their own signature is unreadable or not one
/// the row accepts, or their owner lacks a required sibling (the `[invoked]`
/// marker's `sig_rejected=`).
///
/// The DT.5l readers [`invoked_member_at`](glia_code_domain::profile::invoked_member_at)
/// takes, every one read from what the layout persists, never from
/// build-time nav:
/// - `sig`: the CA.3a signature of the METHOD's CODE text
///   ([`glia_parser_go::method_signature`]); a `.go` METHOD only, at most once
///   per METHOD, and only once a signature row's cheap gates passed. A CODE
///   cell that is a `code_span` JSON (its source moved) reads as none.
/// - `sibling_sig(n)`: the same of the owner's first child METHOD (in
///   `children_of` order) whose name is `n`, read only after the member's own
///   signature matched.
/// - `has_base(b)`: the owner's own graph holds an unbound heritage ref
///   (IMPLEMENTS / INHERITS_FROM in `unresolved_refs`) from the owner whose
///   qualifier names `b.name` (`Bare(n)` or `Attribute { name: n, .. }`), the
///   graph declares no CLASS / INTERFACE / STRUCT / ENUM of that name, and
///   the owner's IMPORTS cell lists `b.library` exactly. Both indexes are
///   built per graph on the first base a METHOD reaches, in
///   `unresolved_refs` / `g.nodes` order, and only looked up.
pub(crate) fn invoked_members_counted(
    merged: &MergedGraph,
) -> (Vec<(NodeId, NodeId, &'static str)>, usize) {
    use glia_code_domain::profile::{ExternalBase, INVOKED_MEMBERS, invoked_member_at, node_ext};
    use std::cell::OnceCell;
    let mut out = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut rejected: HashSet<NodeId> = HashSet::new();
    for g in &merged.graphs {
        // The owners' cells, looked up only, filled on first need.
        let mut cells_of: Option<HashMap<NodeId, &[Cell]>> = None;
        // Every node's cells (a sibling METHOD's CODE), filled on first need.
        let all_cells: OnceCell<HashMap<NodeId, &[Cell]>> = OnceCell::new();
        // Owner -> the names its unbound heritage refs give, in ref order.
        let heritage: OnceCell<HashMap<NodeId, Vec<&str>>> = OnceCell::new();
        // The type names this graph declares.
        let declared: OnceCell<HashSet<&str>> = OnceCell::new();
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::METHOD) {
                continue;
            }
            let Some(&owner) = g.nav.parent_of.get(&n.id) else { continue };
            let Some(&owner_kind) = g.nav.kind_by_id.get(&owner) else { continue };
            if !OWNER_KINDS.contains(&owner_kind) {
                continue;
            }
            let name = g.nav.name_by_id.get(&n.id).map(String::as_str).unwrap_or("");
            let owner_name = g.nav.name_by_id.get(&owner).map(String::as_str).unwrap_or("");
            if !INVOKED_MEMBERS.iter().any(|r| r.names_member(owner_name, name)) {
                continue;
            }
            let Some(ext) = node_ext(&n.cells) else { continue };
            let cells = cells_of
                .get_or_insert_with(|| {
                    g.nodes
                        .iter()
                        .filter(|o| {
                            g.nav.kind_by_id.get(&o.id).is_some_and(|k| OWNER_KINDS.contains(k))
                        })
                        .map(|o| (o.id, o.cells.as_slice()))
                        .collect()
                })
                .get(&owner)
                .copied()
                .unwrap_or(&[]);
            let roles = glia_graph::roles::roles_in(Some(owner_kind), cells);
            let is_go = ext.eq_ignore_ascii_case("go");
            let sig = || if is_go { go_code_signature(&n.cells) } else { None };
            let sibling_sig = |sibling: &str| -> Option<String> {
                if !is_go {
                    return None;
                }
                let child = g.nav.children_of.get(&owner)?.iter().find(|c| {
                    g.nav.kind_by_id.get(c) == Some(&node_kind::METHOD)
                        && g.nav.name_by_id.get(c).map(String::as_str) == Some(sibling)
                })?;
                let index = all_cells
                    .get_or_init(|| g.nodes.iter().map(|o| (o.id, o.cells.as_slice())).collect());
                go_code_signature(index.get(child)?)
            };
            let has_base = |b: &ExternalBase| -> bool {
                let refs = heritage.get_or_init(|| unbound_heritage(g));
                if !refs.get(&owner).is_some_and(|names| names.contains(&b.name)) {
                    return false;
                }
                if declared.get_or_init(|| declared_type_names(g)).contains(b.name) {
                    return false;
                }
                imports_library(cells, b.library)
            };
            match invoked_member_at(&roles, owner_name, name, &ext, &sig, &sibling_sig, &has_base)
            {
                Some(framework) => {
                    if seen.insert(n.id) {
                        out.push((n.id, owner, framework));
                    }
                }
                None => {
                    if INVOKED_MEMBERS.iter().any(|r| {
                        r.checks_signature() && r.gates(&roles, owner_name, name, &ext, &has_base)
                    }) {
                        rejected.insert(n.id);
                    }
                }
            }
        }
    }
    let sig_rejected = rejected.iter().filter(|id| !seen.contains(id)).count();
    (out, sig_rejected)
}

/// DT.5l: the CA.3a signature of a Go METHOD's CODE text, or `None` when the
/// node has no CODE text (a `code_span` JSON payload is none).
fn go_code_signature(cells: &[Cell]) -> Option<String> {
    cells.iter().find(|c| c.kind == cell_type::CODE).and_then(|c| match &c.payload {
        CellPayload::Text(code) => glia_parser_go::method_signature(code),
        _ => None,
    })
}

/// DT.5l: per node, the base names of its heritage refs (IMPLEMENTS /
/// INHERITS_FROM) the build left unbound in `g.unresolved_refs`, in ref
/// order: a base no in-repo type took, so a library's.
fn unbound_heritage(g: &RepoGraph) -> HashMap<NodeId, Vec<&str>> {
    use glia_code_domain::CallQualifier;
    let mut out: HashMap<NodeId, Vec<&str>> = HashMap::new();
    for r in &g.unresolved_refs {
        if r.category != edge_category::IMPLEMENTS && r.category != edge_category::INHERITS_FROM {
            continue;
        }
        let name = match &r.qualifier {
            CallQualifier::Bare(n) | CallQualifier::Attribute { name: n, .. } => n.as_str(),
            _ => continue,
        };
        out.entry(r.from).or_default().push(name);
    }
    out
}

/// DT.5l: the names of the CLASS / INTERFACE / STRUCT / ENUM nodes `g`
/// declares, the in-repo types a library base name must not be.
fn declared_type_names(g: &RepoGraph) -> HashSet<&str> {
    const TYPES: [glia_core::NodeKindId; 4] =
        [node_kind::CLASS, node_kind::INTERFACE, node_kind::STRUCT, node_kind::ENUM];
    g.nodes
        .iter()
        .filter(|n| g.nav.kind_by_id.get(&n.id).is_some_and(|k| TYPES.contains(k)))
        .filter_map(|n| g.nav.name_by_id.get(&n.id).map(String::as_str))
        .collect()
}

/// DT.5l: does a node's IMPORTS cell (a JSON array of library names) list
/// `library` exactly?
fn imports_library(cells: &[Cell], library: &str) -> bool {
    cells.iter().filter(|c| c.kind == cell_type::IMPORTS).any(|c| {
        let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
            return false;
        };
        serde_json::from_str::<Vec<String>>(s).is_ok_and(|libs| libs.iter().any(|l| l == library))
    })
}

/// The entrypoint-reachable ("live") node set: every entrypoint plus everything
/// forward-reachable from one along semantic carry edges. A node absent from
/// this set is likely dead code. Conservative (generous entrypoint set) to avoid
/// false-dead flags — the failure mode the handoff warns about.
///
/// Both halves come from the code domain's profile (LD.14b): an entrypoint is
/// what `CODE_PROFILE.tables.entry` says one is — routes, gRPC services and
/// servers, Connect / Twirp procedures, queue consumers, GraphQL resolvers,
/// WS / event handlers, CLI commands, cron jobs (the inbound handler kinds,
/// reconciled by LD.6 and listed by `profile::entry_kinds`), framework
/// components, `main` / `test*` / `Test*` functions and methods, a Go
/// `func init()` (DT.5l: a FUNCTION named `init` in a `.go` file, the
/// extension read through `glia_code_domain::profile::node_ext`), and (LB.3b)
/// any node carrying the COMPONENT role:
/// since the LB.3a fold an Angular `@Component` is a CLASS with ROLE
/// COMPONENT, not a COMPONENT node (DT.5l: so is a Swift `@main` type and a
/// Kotlin Flutter activity). Besides the table, any node carrying an
/// ENTRYPOINT cell ([`is_declared_entry`]) is an entrypoint whatever its kind
/// (LF.3b): the user declared it in `.glia/overlay.toml` `[entrypoints]`, for
/// code the graph cannot see a trigger for (a job a scheduler wires by a
/// string, a plugin hook loaded by reflection, a library's public API), or
/// (DT.5l) the Python parser read it, a script MODULE whose
/// `if __name__ == "__main__":` guard runs it (`source` `main_guard`). The
/// walk carries on `CODE_PROFILE.tables.carry_edges`.
///
/// Three steps besides the forward carry walk (A7.8, DT.5d), each applied as
/// a node is popped from the queue:
/// - **Owner.** A live METHOD makes its owning CLASS / STRUCT / INTERFACE (its
///   direct `parent_of`) live. DEFINES stays out of the carry edges, since
///   pulling in a whole container is the impact fan-out that list exists to
///   prevent. Liveness is a different question: a controller whose action is
///   route-reachable is not dead, and it is the CLASS, not the METHOD, that
///   owns the INJECTS edges to its services. One level, upward only: no climb
///   to MODULE and no walk down to sibling methods.
/// - **Implementer.** A live node makes every node with an IMPLEMENTS edge
///   into it live: the implementing CLASS of a live INTERFACE, and (A6.6's
///   method-level IMPLEMENTS) the implementing METHOD of a live interface
///   METHOD. Interface-typed DI (`UsersController -INJECTS-> IUserService`) is
///   otherwise the canonical false-dead: the real `UserService` has no inbound
///   carry edge. INHERITS_FROM stays forward-only: a live base class does not
///   make its subclasses live.
/// - **Invoked member (DT.5d).** A live owner makes live each of its METHODs
///   a framework or the runtime calls on an instance of it, with no source
///   line to read the call from ([`invoked_members`], the code domain's
///   `profile::INVOKED_MEMBERS` table): Angular lifecycle hooks of a
///   component / directive / pipe / service / guard, the ControlValueAccessor
///   / Validator members of a component or directive, a pipe's `transform`,
///   a service's `intercept`, a guard's or service's `canActivate` family, and the
///   constructor (TS / JS `constructor`, Python `__init__`, Ruby
///   `initialize`, PHP `__construct`, Swift `init`, the METHOD named like
///   its class in Dart, Java, C# and C++). DT.5l adds the Go methods the
///   runtime calls through an interface their signature satisfies (`Error`,
///   `String`, `Unwrap` beside an `Error`, the marshalers, `Scan`, `Value`,
///   `ServeHTTP`, `Len` / `Less` / `Swap` together), an `@main` type's
///   app-delegate callbacks, and the members a library calls on a class that
///   extends or implements its base (Transloco's loader family, Angular's
///   ErrorHandler / Resolve / TitleStrategy / RouteReuseStrategy, dio's
///   interceptors). Only from a live owner, and still no walk down to any
///   other sibling method.
///
/// Every step is a walk-local hop: no edge is stored or returned.
///
/// Prints `[live] profile=P entrypoints=E (kind=K role=R declared=D) owners=O
/// implementers=I invoked=V reached=N/M` once per call — one line per
/// liveness computation, so one per blast-radius query, as `[scope]` /
/// `[locate]` print per query: `P` is the profile's graph type, `kind` counts
/// entries by kind / name, `role` the ones only a ROLE cell made entries
/// (LB.3b), `declared` the ones only an ENTRYPOINT cell made entries (LF.3b),
/// `owners` / `implementers` / `invoked` the nodes each step made live (a
/// member a carry edge reached first counts in none), `N` the live set and
/// `M` every node. Right after it (DT.5l), `[invoked] rows=<n>
/// angular=<a> constructor=<c> flutter=<f> go_runtime=<g> apple_app=<p>
/// transloco=<t> dio=<d> sig_rejected=<r>`: the [`invoked_members`] rows per
/// framework tag, live owner or not, and the METHODs a signature-checked row
/// refused ([`invoked_members_counted`]).
pub fn entrypoint_reachable(merged: &MergedGraph) -> std::collections::HashSet<NodeId> {
    let w = live_walk(merged);
    eprintln!(
        "[live] profile={} entrypoints={} (kind={} role={} declared={}) owners={} implementers={} invoked={} reached={}/{}",
        CODE_PROFILE.tables.graph_type,
        w.by_kind + w.by_role + w.by_decl,
        w.by_kind,
        w.by_role,
        w.by_decl,
        w.owners,
        w.implementers,
        w.invoked,
        w.live.len(),
        w.total
    );
    eprintln!("{}", invoked_marker(&w));
    w.live
}

/// DT.5l: `[invoked] rows=<n> <framework>=<count>.. sig_rejected=<r>`, every
/// framework tag of `INVOKED_MEMBERS` in first-appearance order, zeros
/// included.
fn invoked_marker(w: &LiveWalk) -> String {
    let total: usize = w.invoked_rows.iter().map(|(_, n)| n).sum();
    let tags: Vec<String> = w.invoked_rows.iter().map(|(t, n)| format!("{t}={n}")).collect();
    format!("[invoked] rows={total} {} sig_rejected={}", tags.join(" "), w.sig_rejected)
}

/// Did the user declare this node an entrypoint (LF.3b)? True when it
/// carries an ENTRYPOINT cell, which the build's `[entrypoints]` stage writes
/// from `.glia/overlay.toml` and the `.gmap` persists. Liveness seeds from it
/// beside `CODE_PROFILE.tables.entry`. DT.5l: the Python parser writes one
/// too, on a MODULE with an `if __name__ == "__main__":` guard (`source`
/// `main_guard`); it counts as declared alike.
pub(crate) fn is_declared_entry(cells: &[Cell]) -> bool {
    cells.iter().any(|c| c.kind == cell_type::ENTRYPOINT)
}

/// What the liveness walk follows (LD.15b): the carry edges, plus the two
/// A7.8 steps and the DT.5d invoked-member step as walk-local hops the walk
/// takes like edges — never stored, never returned. `hops` holds every owner
/// hop (METHOD -> its owning type, tagged DEFINES), then every implementer
/// hop (interface -> implementer, an IMPLEMENTS edge reversed, in edge
/// order), then every invoked hop (owner -> invoked member, tagged DEFINES,
/// in [`invoked_members`] order), and the carry edges follow them. The index
/// keeps each node's outgoing list in that global order, so a node popped by
/// the walk takes its owner hop, then its implementer hops, then its invoked
/// hops, then its carry edges — the order the per-node queue loop this
/// replaced applied the first two.
struct LiveSource<'a> {
    merged: &'a MergedGraph,
    hops: Vec<Edge>,
    carry: CategorySet,
}

impl GraphSource for LiveSource<'_> {
    fn node_ids(&self) -> Vec<NodeId> {
        GraphSource::node_ids(self.merged)
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(
            self.hops
                .iter()
                .chain(self.merged.all_edges().filter(|e| self.carry.contains(e.category))),
        )
    }
}

/// A walk-local hop of [`LiveSource`].
fn live_hop(from: NodeId, to: NodeId, category: glia_core::EdgeCategoryId) -> Edge {
    Edge { from, to, category, confidence: Confidence::Strong, cells: Vec::new() }
}

/// The walk behind [`entrypoint_reachable`]: one `algo::reach::bfs` from the
/// entrypoints over a [`LiveSource`] index, built once per call in O(V + E)
/// (LD.15b) instead of scanning every edge for every live node. Which step
/// made a node live is read off the hop that reached it: a node reached from
/// `p` is `p`'s owner when `p`'s owner hop names it (that hop is first in
/// `p`'s list, so it always wins), else an implementer when an implementer
/// hop `p -> it` exists (those precede every carry edge), else an invoked
/// member when it was reached over DEFINES (no carry edge is one) and an
/// invoked hop `p -> it` exists, else a carry edge.
/// Every iteration is over a `Vec` (`merged.graphs`, `g.nodes`, the edges);
/// the maps are only looked up, never iterated, so the counts are
/// deterministic.
fn live_walk(merged: &MergedGraph) -> LiveWalk {
    let t = &CODE_PROFILE.tables;
    let mut invoked_rows: Vec<(&'static str, usize)> = Vec::new();
    for r in glia_code_domain::profile::INVOKED_MEMBERS {
        if !invoked_rows.iter().any(|(tag, _)| *tag == r.framework) {
            invoked_rows.push((r.framework, 0));
        }
    }
    let mut w = LiveWalk {
        live: HashSet::new(),
        by_kind: 0,
        by_role: 0,
        by_decl: 0,
        owners: 0,
        implementers: 0,
        invoked: 0,
        invoked_rows,
        sig_rejected: 0,
        total: 0,
    };
    let mut owner_of: HashMap<NodeId, NodeId> = HashMap::new();
    // Methods with an owner, in node order, once each.
    let mut owned: Vec<NodeId> = Vec::new();
    let mut seeds: Vec<NodeId> = Vec::new();
    for g in &merged.graphs {
        w.total += g.nodes.len();
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            if kind == Some(node_kind::METHOD)
                && let Some(&p) = g.nav.parent_of.get(&n.id)
                && g.nav.kind_by_id.get(&p).is_some_and(|k| OWNER_KINDS.contains(k))
                && owner_of.insert(n.id, p).is_none()
            {
                owned.push(n.id);
            }
            let name = g.nav.name_by_id.get(&n.id).map(String::as_str).unwrap_or("");
            let ext = || glia_code_domain::profile::node_ext(&n.cells);
            let seeded = if t.entry.is_entry_at(kind, name, &[], ext) {
                &mut w.by_kind
            } else if t.entry.is_entry_at(
                kind,
                name,
                &glia_graph::roles::roles_in(kind, &n.cells),
                ext,
            ) {
                &mut w.by_role
            } else if is_declared_entry(&n.cells) {
                &mut w.by_decl
            } else {
                continue;
            };
            if w.live.insert(n.id) {
                *seeded += 1;
                seeds.push(n.id);
            }
        }
    }

    let mut hops: Vec<Edge> = owned
        .iter()
        .filter_map(|m| owner_of.get(m).map(|&p| live_hop(*m, p, edge_category::DEFINES)))
        .collect();
    // Reverse IMPLEMENTS: interface (or interface method) -> its implementers.
    let mut implementer_hops: HashSet<(NodeId, NodeId)> = HashSet::new();
    for e in merged.all_edges() {
        if e.category == edge_category::IMPLEMENTS {
            implementer_hops.insert((e.to, e.from));
            hops.push(live_hop(e.to, e.from, edge_category::IMPLEMENTS));
        }
    }
    // DT.5d: live owner -> each member a framework or the runtime invokes.
    let mut invoked_hops: HashSet<(NodeId, NodeId)> = HashSet::new();
    let (members, sig_rejected) = invoked_members_counted(merged);
    w.sig_rejected = sig_rejected;
    for (member, owner, framework) in members {
        if let Some((_, n)) = w.invoked_rows.iter_mut().find(|(tag, _)| *tag == framework) {
            *n += 1;
        }
        if invoked_hops.insert((owner, member)) {
            hops.push(live_hop(owner, member, edge_category::DEFINES));
        }
    }
    let src = LiveSource { merged, hops, carry: CategorySet::of(t.carry_edges) };
    let adj = Adjacency::build(&src, &CategorySet::all());
    for r in reach::bfs(&adj, &seeds, Walk::Forward, usize::MAX).reached {
        w.live.insert(r.id);
        if owner_of.get(&r.parent) == Some(&r.id) {
            w.owners += 1;
        } else if implementer_hops.contains(&(r.parent, r.id)) {
            w.implementers += 1;
        } else if r.via == edge_category::DEFINES && invoked_hops.contains(&(r.parent, r.id)) {
            w.invoked += 1;
        }
    }
    w
}

/// The node a user-supplied qname or bare name means (LA.14). Candidates are
/// the nodes whose qname is exactly `q`, or — only when none is — whose simple
/// name is. With a `scope` (a path or a project label, see [`resolve_scope`])
/// and more than one candidate, the ones LOCATED under the scope are preferred:
/// `blast-radius handle --scope services/api` means the api `handle`, not the
/// busier web one whose whole radius the scope would then filter away.
///
/// Scope is a preference, never a filter: when no candidate is located under
/// it, the pick is the unscoped one, so `blast-radius shared_fn --scope
/// services/api` still answers "what in services/api uses this shared fn".
/// An unlocatable candidate does not count as in scope here — the
/// keep-unlocatable rule of [`node_in_scope`] is about not DROPPING answers,
/// and preferring a node nobody can place over one placed in scope would undo
/// the preference. Ties inside either set go through
/// [`MergedGraph::pick_primary`]. `scope = None` is exactly
/// `node_id_by_qname(q).or_else(|| resolve_name(q))`.
pub fn resolve_seed(merged: &MergedGraph, q: &str, scope: Option<&str>) -> Option<NodeId> {
    let by_qname = merged.qnames_exact(q);
    let candidates = if by_qname.is_empty() { merged.names_exact(q) } else { by_qname };
    let primary = merged.pick_primary(&candidates);
    let Some(raw) = scope.filter(|_| candidates.len() > 1) else { return primary };
    let path = resolve_scope(merged, raw);
    let loc = Locator::new(merged);
    let inside: Vec<NodeId> = candidates
        .iter()
        .copied()
        .filter(|&id| scope_file_of(&loc, id).is_some_and(|f| in_scope(&f, &path)))
        .collect();
    let pick = merged.pick_primary(&inside).or(primary);
    if let Some(id) = pick.filter(|&id| Some(id) != primary) {
        // fired_on marker (LA.14): only when the scope changed the pick.
        eprintln!(
            "[scope] seed '{q}' -> {} ({} of {} candidates in scope {path})",
            loc.locate(id).qname,
            inside.len(),
            candidates.len()
        );
    }
    pick
}

/// What a [`blast_radius`] walks and keeps. Start from `default()` (both
/// directions, depth 4, every row, dead rows flagged not dropped, no scope)
/// and set fields: `#[non_exhaustive]` rules out a struct literal outside
/// this crate.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BlastOptions {
    /// Which way the radius spreads from the seeds.
    pub direction: Reach,
    /// Maximum hops along carry edges, counted from the nearest seed.
    pub depth: usize,
    /// Keep the first `top_k` rows, cut after `scope`; `None` keeps all.
    pub top_k: Option<usize>,
    /// Drop the rows no entry point reaches ([`entrypoint_reachable`])
    /// instead of flagging them `live: false`.
    pub live_only: bool,
    /// A repo-relative path or a project label (A8.3 / A8.6): keeps the rows
    /// located under it, BEFORE the `top_k` cut, and prefers an in-scope
    /// candidate when a bare name is ambiguous ([`resolve_seed`]).
    pub scope: Option<String>,
    /// DD.10: identity classes (Q17). On by default: a walk class (messages,
    /// contract ops, SCIP symbols, a package and its project) is one node in
    /// the walk and the ranking, and a FACT / DERIVED class is one row with
    /// its `identity`. [`Collapse::off`] is the raw per-member answer.
    pub collapse: Collapse,
    /// 0.5.3 (EA.5): the measured file ranking that orders the rows
    /// ([`crate::rank::rank_files`] from the seeds' files, then
    /// [`crate::rank::order_rows`]) and fills [`BlastRadius::files`]. The
    /// default is `H10` (co-change first, P1 under it); with no history
    /// snapshot, or under [`Ranking::head`], the rows keep the 0.5.2 order.
    /// The closure's own PPR scores never change with it.
    pub ranking: Ranking,
    /// 0.5.3 (EA.5): the caller's held [`RankCache`] (its repo roots name the
    /// history snapshots co-change reads). `None` builds a transient cache
    /// with no repo roots and no labels (so no co-change, and `repo#<n>` in
    /// `files`), per call.
    pub rank_cache: Option<Arc<RankCache>>,
    /// 0.5.3 (EA.5): how many files [`BlastRadius::files`] lists.
    pub top_files: usize,
}

impl Default for BlastOptions {
    fn default() -> Self {
        BlastOptions {
            direction: Reach::Both,
            depth: 4,
            top_k: None,
            live_only: false,
            scope: None,
            collapse: Collapse::default(),
            ranking: Ranking::default(),
            rank_cache: None,
            top_files: 10,
        }
    }
}

/// One seed of a [`BlastRadius`], located, with the other seeds one carry
/// edge away from it in the walk's direction (`linked_seeds`, by qname, in
/// seed order): with `Forward` the seeds it reaches in one hop, with
/// `Backward` the seeds that reach it in one hop, with `Both` either. A seed
/// is never a result row, so this is where "one changed symbol calls another"
/// is kept — the fact a union of per-seed answers drops.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct BlastSeed {
    /// What the caller asked for: the query as given, or the display string
    /// an id-seeded caller passed.
    pub query: String,
    pub id: u64,
    pub qname: String,
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based (see [`Located`]).
    pub line: Option<i64>,
    pub linked_seeds: Vec<String>,
    /// DD.10: the seed's FACT / DERIVED identity class, when it is in one:
    /// its class-mates (`also_in`), which a walk class makes part of the seed
    /// itself (never rows). Absent from the JSON outside a class.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<RowIdentity>,
}

/// A multi-seed blast radius (LD.5): the seeds, the queries that named no
/// node, the ranked located rows (each with the `seed` whose wave reached it
/// first), and — exactly when `results` is empty — why (LD.8a).
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct BlastRadius {
    /// One per distinct node, in query order.
    pub seeds: Vec<BlastSeed>,
    /// Queries no node answers to, in query order, each once. Never an error.
    pub unresolved: Vec<String>,
    pub results: Vec<BlastAnswer>,
    /// 0.5.3 (EA.5): the measured file ranking from the seeds' files
    /// ([`BlastOptions::ranking`]), best first, at most
    /// [`BlastOptions::top_files`], seed files never listed: a file is listed
    /// whether or not the closure reached it (`rows` counts the answer's rows
    /// in it, 0 when none). Under `scope` only files under it are listed.
    /// Empty when no seed has a candidate file.
    pub files: Vec<RankedFile>,
    /// `Some` iff `results` is empty.
    pub absence: Option<Absence>,
}

/// **blast_radius** (P3, LD.5): the complete, deduped, edge-category-aware,
/// PPR-ranked, LOCATED closure around every node `queries` names — the
/// answer `find`→`impact`→`activate`→`read×N` collapses to, for a whole
/// change set in one call.
///
/// Each query resolves through [`resolve_seed`] under `opts.scope` (qname,
/// then name; the scope a preference among ambiguous candidates, LA.14). A
/// query that names no node goes to `unresolved` and the rest still answer;
/// two queries naming one node are one seed, the first query kept. Then
/// [`blast_radius_seeded`] runs ONE walk and ONE PPR over all the seeds:
/// depths from the nearest seed, scores from one run so they compare, each
/// row attributed to the seed whose wave reached it first (seeds expand in
/// query order), no seed ever a row, and `linked_seeds` naming the seeds one
/// carry edge apart.
///
/// Rows: `scope` keeps those located under it BEFORE the `top_k` cut (A8.3;
/// a row with no locatable file — DOC_SPACE, a handler-less route — is KEPT,
/// and `scope` narrows WITHIN a repo: under a multi-repo merge each repo's
/// paths are relative to its OWN root); `live` is [`entrypoint_reachable`]
/// and `live_only` drops the dead rows inside the ranking plan (LD.12c);
/// each row is located through one [`Locator`] (LD.1).
///
/// An empty answer carries its [`Absence`]: `unknown_symbol` when every query
/// is unresolved (suggestions for the first), `no_edges` when the seeds
/// resolved but no carry edge leads anywhere (mechanisms from the seeds'
/// kinds; a MODULE / PACKAGE seed is told that structural edges are not
/// carry edges), `no_match` when `scope`, `live_only`, `top_k = Some(0)` or
/// `depth = 0` removed every row.
///
/// Identity (DD.10, `opts.collapse`, on by default): a walk class (messages,
/// contract ops, SCIP symbols, a package and its project) is one node in the
/// walk and the PPR, and a FACT / DERIVED class is one row carrying its
/// `identity`, folded before `scope` and `top_k`; without a SAME_AS edge the
/// answer is unchanged byte for byte. The rules are on `radius`.
///
/// fired_on marker, once per answer:
/// `[blast] seeds=S unresolved=U reached=R linked_seeds=L walk=W` — `R` is
/// the closure the walk reached (before `live_only`, `scope` and `top_k`;
/// a walk class counts once), `L` the seed-to-seed links. Grep token
/// `[blast] seeds=`. With a collapse class, also
/// `[identity] collapse surface=blast_radius classes=<c> walk_classes=<w> folded=<f> rows=<n>`.
pub fn blast_radius(merged: &MergedGraph, queries: &[&str], opts: &BlastOptions) -> BlastRadius {
    let scope = opts.scope.as_deref();
    let mut seeds: Vec<(String, NodeId)> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    for &q in queries {
        match resolve_seed(merged, q, scope) {
            Some(id) => seeds.push((q.to_string(), id)),
            None if !unresolved.iter().any(|u| u == q) => unresolved.push(q.to_string()),
            None => {}
        }
    }
    if unresolved.is_empty() {
        return blast_radius_seeded(merged, &seeds, opts);
    }
    radius(merged, &seeds, unresolved, opts, "blast_radius")
}

/// [`blast_radius`] from node ids the caller already holds — `(display
/// query, id)` per seed — so a caller with ids (LE.2's diff impact: changed
/// nodes, two of which can share a qname) never re-resolves them by name.
/// Everything past resolution is here: [`blast_radius`] only resolves its
/// queries, then lands in this answer (or, when some query is unresolved, in
/// the same answer with `unresolved` filled).
pub(crate) fn blast_radius_seeded(
    merged: &MergedGraph,
    seeds: &[(String, NodeId)],
    opts: &BlastOptions,
) -> BlastRadius {
    blast_radius_seeded_on(merged, seeds, opts, "blast_radius")
}

/// [`blast_radius_seeded`] for an outer answer (`diff_impact`, `review`):
/// `surface` names it in the `[rank]` marker.
pub(crate) fn blast_radius_seeded_on(
    merged: &MergedGraph,
    seeds: &[(String, NodeId)],
    opts: &BlastOptions,
    surface: &'static str,
) -> BlastRadius {
    radius(merged, seeds, Vec::new(), opts, surface)
}

/// The one multi-seed answer behind [`blast_radius`] and
/// [`blast_radius_seeded`] (and so behind `diff_impact` and `review`).
///
/// DD.10, identity: one [`IdentityView`] of `merged` under `opts.collapse`.
/// Without a collapse class (collapse off, or no SAME_AS edge) it is empty
/// and this is the HEAD answer byte for byte. With one:
///
/// - **Seeds** are distinct by walk node: two seeds in one walk class are one
///   seed, the first query kept, and the class-mates of a seed are the seed
///   itself, never rows. Each seed carries its class (`identity`).
/// - **Walk**: with walk classes, the walk and the PPR run over the view's
///   quotient ([`glia_graph::blast_radius_over`]), so a class is one node: a
///   change reaching one SCIP-identical copy reaches the other copy's callers
///   at zero hops, and a carry edge inside a class (a SHARES_SCHEMA pair that
///   is also one class) is dropped, so the pair is walked once. Without walk
///   classes the walk is the HEAD walk. `reached` counts walk nodes.
/// - **Liveness** stays the raw graph's [`entrypoint_reachable`] set; a walk
///   class is live (kept by `live_only`, flagged `live`) when ANY member is.
/// - **Rows**: a walk class's row shows the class's display member (chosen by
///   content, [`IdentityView::display`]); any other hit shows the node the
///   walk reached. Every row of a FACT / DERIVED class carries `identity`
///   (`also_in` = every member but the shown one), and the rows are folded
///   ([`IdentityView::fold`]) BEFORE `scope` and `top_k`, so a non-walk class
///   (one `package:npm:react` reached in two repos) keeps its best-scored row
///   and never spends two `top_k` slots. `scope` reads the shown node's file.
///
/// 0.5.3 (EA.5), ranking: the closure is ranked over `opts.rank_cache`'s
/// PPR index of the walk's source (the quotient or the merged graph) under
/// [`glia_graph::closure_config`], scores unchanged. The measured file
/// ranking ([`crate::rank::rank_files`] under `opts.ranking`, from the
/// display seeds' files in seed order) reorders the rows through
/// [`crate::rank::order_rows`] BEFORE the identity fold, `scope` and `top_k`,
/// and its first `top_files` files (under `scope`) are `files`. fired_on,
/// right after `[blast] seeds=`, once per answer: `[rank] surface=<s>
/// ranking=<harness name> seed_files=<s> cochange_files=<c> files=<f>
/// lifted=<l> closure_index=<hit|built>` (`lifted`: rows placed earlier
/// than in the closure's order), or `[rank] surface=<s> files=0
/// reason=no_seed_file` when no seed has a candidate file.
///
/// fired_on, beside the `[blast] seeds=` line and only when the view has a
/// collapse class: `[identity] collapse surface=blast_radius classes=<c>
/// walk_classes=<w> folded=<f> rows=<n>`, `f` the rows the fold dropped, `n`
/// the answer's rows.
fn radius(
    merged: &MergedGraph,
    seeds: &[(String, NodeId)],
    unresolved: Vec<String>,
    opts: &BlastOptions,
    surface: &'static str,
) -> BlastRadius {
    let view = IdentityView::new(merged, &opts.collapse);
    // One seed per walk node (a node, or a whole walk class), the first
    // display query kept.
    let mut distinct: HashSet<NodeId> = HashSet::new();
    let seeds: Vec<(&str, NodeId)> = seeds
        .iter()
        .filter(|(_, id)| distinct.insert(view.walk_rep(*id)))
        .map(|(q, id)| (q.as_str(), *id))
        .collect();
    let ids: Vec<NodeId> = seeds.iter().map(|&(_, id)| id).collect();
    let walk_seeds: Vec<NodeId> = ids.iter().map(|&id| view.walk_rep(id)).collect();

    let mut out: Vec<BlastAnswer> = Vec::new();
    let mut reached = 0usize;
    let mut dead = 0usize;
    let mut folded = 0usize;
    let mut kept_in_scope = 0usize;
    let mut blast_seeds: Vec<BlastSeed> = Vec::with_capacity(seeds.len());
    let mut ranked_files: Vec<RankedFile> = Vec::new();
    let mut rank_line: Option<String> = None;
    if !ids.is_empty() {
        let loc = Locator::new(merged);
        let live = entrypoint_reachable(merged);
        let quotient = view.quotient(merged);
        // EA.5: the caller's RankCache, or a transient one with no repo
        // roots (no co-change) and no labels: `opts.collapse.repo_labels` is
        // empty under `Collapse::off()`, and a label is a file tie-break, so
        // reading it would make the collapse switch move `files` with no
        // class in the graph (identity_collapse_blast's byte-identity). A
        // labelled answer passes a cache. Guarded before any index is read
        // (EA.2).
        let cache = opts.rank_cache.clone().unwrap_or_else(|| {
            Arc::new(RankCache::new("answer", &BTreeMap::new(), &BTreeMap::new()))
        });
        cache.guard(merged);
        // The closure's own PPR over a cached index (item 5): scores are the
        // unindexed walk's in f64 bits (EA.1).
        let closure_cfg = glia_graph::closure_config(&CODE_PROFILE.tables, opts.direction);
        let built_before = cache.stats().built;
        let closure_ix = match &quotient {
            Some(q) => cache.ppr_index(q, SourceKey::Quotient, &closure_cfg),
            None => cache.ppr_index(merged, SourceKey::Merged, &closure_cfg),
        };
        let closure_how = if cache.stats().built > built_before { "built" } else { "hit" };
        // The measured file ranking from the DISPLAY seeds' files, in seed
        // order (the harness's order, so C0 sums as it does); a seed whose
        // node maps to no candidate file seeds the ranking by its node.
        let files = cache.files(merged);
        let mut seed_files: Vec<u32> = Vec::new();
        let mut extra: Vec<NodeId> = Vec::new();
        for &id in &ids {
            match files.file_of_node(id) {
                Some(f) if !seed_files.contains(&f) => seed_files.push(f),
                Some(_) => {}
                None => extra.push(id),
            }
        }
        let fr: Option<FileRanking> = (!seed_files.is_empty())
            .then(|| rank_files(merged, &cache, &seed_files, &extra, opts.ranking));
        // A walk class is live when any member is: the live set mapped onto
        // the walk's nodes (the raw set itself when nothing is contracted).
        let (walk_live, class_reps): (Cow<'_, HashSet<NodeId>>, HashSet<NodeId>) =
            if quotient.is_some() {
                let mapped = live.iter().map(|&n| view.walk_rep(n)).collect();
                (Cow::Owned(mapped), walk_class_reps(merged, &view))
            } else {
                (Cow::Borrowed(&live), HashSet::new())
            };
        // `live_only` is a filter on the ranking plan: it drops rows from the
        // ranked closure and keeps the rest in order, so scope and `top_k`
        // below see what the post-sort filter they replaced saw.
        let live_filter = LiveFilter::new(&walk_live);
        let hits = match &quotient {
            Some(q) => blast_walk(q, &walk_seeds, opts, &live_filter, &closure_ix),
            None => blast_walk(merged, &walk_seeds, opts, &live_filter, &closure_ix),
        };
        dead = live_filter.dropped.get();
        reached = hits.len() + dead;

        let linked = linked_seeds(merged, &ids, opts);
        for &(query, id) in &seeds {
            let at = loc.locate(id);
            blast_seeds.push(BlastSeed {
                query: query.to_string(),
                id: id.0,
                qname: at.qname,
                kind: at.kind,
                file: at.file,
                line: at.line,
                linked_seeds: Vec::new(),
                identity: view.row_identity(id, id),
            });
        }
        for (i, links) in linked.iter().enumerate() {
            blast_seeds[i].linked_seeds =
                links.iter().map(|&j| blast_seeds[j].qname.clone()).collect();
        }
        // A hit's `seed` is a walk node: the seed itself, or its class's
        // representative.
        let seed_qname: HashMap<NodeId, &str> = walk_seeds
            .iter()
            .zip(&blast_seeds)
            .map(|(&id, s)| (id, s.qname.as_str()))
            .collect();

        out = hits
            .iter()
            .map(|h| {
                // A walk class is one node whose id is its representative,
                // an internal handle: the row shows the class's display
                // member. Any other hit is the node the walk reached.
                let shown = if class_reps.contains(&h.id) { view.display(h.id) } else { h.id };
                let at = loc.locate(shown);
                BlastAnswer {
                    id: shown.0,
                    qname: at.qname,
                    name: at.name,
                    kind: at.kind,
                    reason: edge_category::name(h.reason),
                    depth: h.depth,
                    score: h.score,
                    live: walk_live.contains(&h.id),
                    file: at.file,
                    line: at.line,
                    seed: seed_qname.get(&h.seed).map(|q| q.to_string()).unwrap_or_default(),
                    identity: view.row_identity(h.id, shown),
                }
            })
            .collect();
        // EA.5: the file ranking's row order, BEFORE the fold, scope and the
        // cut, so each sees the final order. None is the identity (always
        // under Ranking::head() and with no co-change).
        let mut lifted = 0usize;
        if let Some(fr) = &fr {
            let row_files: Vec<Option<u32>> =
                out.iter().map(|a| files.file_of_node(NodeId(a.id))).collect();
            if let Some(perm) = order_rows(&row_files, &seed_files, fr) {
                lifted = perm.iter().enumerate().filter(|&(to, &from)| to < from).count();
                let mut slots: Vec<Option<BlastAnswer>> = out.into_iter().map(Some).collect();
                out = perm.iter().filter_map(|&i| slots.get_mut(i).and_then(Option::take)).collect();
            }
        }
        // Fold BEFORE scope and the cut: a class never spends two slots.
        let ranked = out.len();
        out = view.fold(out, |a| NodeId(a.id), |a, identity| a.identity = Some(identity));
        folded = ranked - out.len();
        // Scope BEFORE the cut: filtering after `truncate` would spend the
        // budget on out-of-scope nodes and return fewer (or zero) in-scope
        // answers.
        out = apply_scope(&loc, out, opts.scope.as_deref(), |a| NodeId(a.id), "blast_radius");
        kept_in_scope = out.len();
        if let Some(k) = opts.top_k {
            out.truncate(k);
        }
        // EA.5: the ranked file list, rows counted over the final rows.
        if let Some(fr) = &fr {
            let mut rows_per_file: BTreeMap<u32, usize> = BTreeMap::new();
            for a in &out {
                if let Some(f) = files.file_of_node(NodeId(a.id)) {
                    *rows_per_file.entry(f).or_default() += 1;
                }
            }
            ranked_files = top_files(merged, &files, fr, opts, &rows_per_file);
            let cochange_files = fr
                .c0
                .iter()
                .enumerate()
                .filter(|&(f, &c)| c > 0.0 && !seed_files.contains(&(f as u32)))
                .count();
            rank_line = Some(format!(
                "[rank] surface={surface} ranking={} seed_files={} cochange_files={cochange_files} files={} lifted={lifted} closure_index={closure_how}",
                opts.ranking.harness_name(),
                seed_files.len(),
                ranked_files.len(),
            ));
        }
    }

    let links: usize = blast_seeds.iter().map(|s| s.linked_seeds.len()).sum();
    // fired_on marker (LD.5).
    eprintln!(
        "[blast] seeds={} unresolved={} reached={reached} linked_seeds={links} walk={:?}",
        blast_seeds.len(),
        unresolved.len(),
        opts.direction
    );
    // fired_on marker (EA.5), once per answer, after the `[blast]` line.
    match rank_line {
        Some(line) => eprintln!("{line}"),
        None => eprintln!("[rank] surface={surface} files=0 reason=no_seed_file"),
    }
    // fired_on marker (DD.10), only with a collapse class.
    view.collapse_marker("blast_radius", out.len() + folded, out.len());

    let absence = out.is_empty().then(|| {
        blast_absence(merged, &blast_seeds, &unresolved, opts, reached, dead, kept_in_scope)
    });
    BlastRadius {
        seeds: blast_seeds,
        unresolved,
        results: out,
        files: ranked_files,
        absence,
    }
}

/// [`BlastRadius::files`]: the first `opts.top_files` files of `fr`'s order.
/// Under `opts.scope` a file outside it is skipped (the rows' rule,
/// [`in_scope`] on the repo-relative path) and the walk continues down the
/// order; `rank` is then the place in the listed files.
fn top_files(
    merged: &MergedGraph,
    files: &FileIndex,
    fr: &FileRanking,
    opts: &BlastOptions,
    rows_per_file: &BTreeMap<u32, usize>,
) -> Vec<RankedFile> {
    let Some(raw) = opts.scope.as_deref() else {
        return fr.top(files, opts.top_files, rows_per_file);
    };
    let path = resolve_scope(merged, raw);
    let mut listed: Vec<RankedFile> = fr
        .top(files, usize::MAX, rows_per_file)
        .into_iter()
        .filter(|f| in_scope(&f.file, &path))
        .take(opts.top_files)
        .collect();
    for (i, f) in listed.iter_mut().enumerate() {
        f.rank = i + 1;
    }
    listed
}

/// One blast walk over `g` (the merged graph, or its identity quotient):
/// `opts`' direction and depth over the code domain's carry edges, with
/// `live` as the plan's filter under `live_only`.
fn blast_walk<G: GraphSource + ?Sized>(
    g: &G,
    seeds: &[NodeId],
    opts: &BlastOptions,
    live: &LiveFilter<'_>,
    index: &glia_activation::PprIndex,
) -> Vec<glia_graph::BlastHit> {
    let filters: Vec<&dyn FilterPredicate<G>> = if opts.live_only { vec![live] } else { Vec::new() };
    glia_graph::blast_radius_over_with(
        g,
        seeds,
        opts.direction,
        opts.depth,
        &CODE_PROFILE.tables,
        &filters,
        Some(index),
    )
}

/// The representatives of `view`'s walk classes: the ids a walk over its
/// quotient reaches for a class. Every member of a walk class is an endpoint
/// of a SAME_AS cross edge (the view reads its classes from those), and each
/// class has a member other than its representative, which maps to it.
fn walk_class_reps(merged: &MergedGraph, view: &IdentityView) -> HashSet<NodeId> {
    merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::SAME_AS)
        .flat_map(|e| [e.from, e.to])
        .filter_map(|id| {
            let rep = view.walk_rep(id);
            (rep != id).then_some(rep)
        })
        .collect()
}

/// Why a [`blast_radius`] came back empty. `reached` is the walk's closure,
/// `dead` the rows `live_only` dropped from it, `kept_in_scope` the rows
/// `scope` kept.
fn blast_absence(
    merged: &MergedGraph,
    seeds: &[BlastSeed],
    unresolved: &[String],
    opts: &BlastOptions,
    reached: usize,
    dead: usize,
    kept_in_scope: usize,
) -> Absence {
    const P: &str = "blast_radius";
    if seeds.is_empty() {
        let Some(first) = unresolved.first() else {
            let note = "no seed was given, so nothing is in radius".to_string();
            return absence::empty(merged, P, "", "no_match", note, &[], None);
        };
        let near_opts = FindOptions {
            top_k: absence::SUGGESTIONS,
            ..FindOptions::default()
        };
        let near = find::search(merged, first, &near_opts).rows;
        return absence::unknown_symbol(merged, P, first, &[], &near);
    }
    let query = seeds.iter().map(|s| s.query.as_str()).collect::<Vec<_>>().join(", ");
    let named = seeds.iter().map(|s| format!("`{}`", s.qname)).collect::<Vec<_>>().join(", ");
    let named = if seeds.len() == 1 { named } else { format!("any of {named}") };
    if opts.depth == 0 {
        let note = format!("depth 0 follows no edge, so nothing is in radius of {named}");
        return absence::empty(merged, P, &query, "no_match", note, &[], None);
    }
    if reached == 0 {
        let mut mechanisms: Vec<&'static str> = Vec::new();
        for s in seeds {
            let kind = merged
                .graphs
                .iter()
                .find_map(|g| g.nav.kind_by_id.get(&NodeId(s.id)).copied());
            for m in kind.map(absence::mechanisms_for_kind).unwrap_or(&[]) {
                if !mechanisms.contains(m) {
                    mechanisms.push(m);
                }
            }
        }
        let how = match opts.direction {
            Reach::Forward => "leaves",
            Reach::Backward => "reaches",
            _ => "touches",
        };
        let mut note = format!("no carry edge {how} {named} in this graph");
        let containers: Vec<String> = seeds
            .iter()
            .filter(|s| {
                s.kind == node_kind::name(node_kind::MODULE)
                    || s.kind == node_kind::name(node_kind::PACKAGE)
            })
            .map(|s| format!("`{}`", s.qname))
            .collect();
        if !containers.is_empty() {
            note.push_str(&format!(
                "; {} {}: structural IMPORTS/CONTAINS/DEFINES are not carry edges, so seed a symbol inside it",
                containers.join(", "),
                absence::plural(containers.len(), "is a container", "are containers")
            ));
        }
        let files: BTreeSet<&str> = seeds.iter().filter_map(|s| s.file.as_deref()).collect();
        let seed_file = if files.len() == 1 { files.first().copied() } else { None };
        return absence::empty(merged, P, &query, "no_edges", note, &mechanisms, seed_file);
    }
    if dead == reached {
        let note = format!(
            "{reached} {} in radius of {named}, none reachable from an entry point (live_only)",
            absence::plural(reached, "node", "nodes")
        );
        return absence::empty(merged, P, &query, "no_match", note, &[], None);
    }
    if kept_in_scope == 0
        && let Some(s) = opts.scope.as_deref()
    {
        return absence::scope_emptied(merged, P, &query, reached - dead, s);
    }
    let note = format!(
        "top_k 0 keeps none of the {kept_in_scope} {} in radius of {named}",
        absence::plural(kept_in_scope, "row", "rows")
    );
    absence::empty(merged, P, &query, "no_match", note, &[], None)
}

/// Per seed (by index into `ids`), the other seeds one carry edge away in
/// `opts.direction`, in seed order: one pass over every edge. Empty at depth
/// 0, which follows no edge.
fn linked_seeds(merged: &MergedGraph, ids: &[NodeId], opts: &BlastOptions) -> Vec<Vec<usize>> {
    let mut links: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); ids.len()];
    if ids.len() < 2 || opts.depth == 0 {
        return links.into_iter().map(|s| s.into_iter().collect()).collect();
    }
    let index: HashMap<NodeId, usize> = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let carry = CategorySet::of(CODE_PROFILE.tables.carry_edges);
    let (fwd, bwd) = match opts.direction {
        Reach::Forward => (true, false),
        Reach::Backward => (false, true),
        _ => (true, true),
    };
    for e in merged.all_edges() {
        if !carry.contains(e.category) || e.from == e.to {
            continue;
        }
        let (Some(&from), Some(&to)) = (index.get(&e.from), index.get(&e.to)) else {
            continue;
        };
        if fwd {
            links[from].insert(to);
        }
        if bwd {
            links[to].insert(from);
        }
    }
    links.into_iter().map(|s| s.into_iter().collect()).collect()
}

/// `live_only` as an activation-plan filter (LD.12c): keeps a node only when
/// it is in the live set it borrows — [`entrypoint_reachable`], mapped onto
/// the walk's nodes when it runs over an identity quotient (DD.10: a walk
/// class is kept when any member is live). Its drops are counted
/// under `live` on the `[activation] plan` debug line, and in `dropped`, so an
/// answer `live_only` emptied says so (LD.5).
struct LiveFilter<'a> {
    live: &'a HashSet<NodeId>,
    dropped: std::cell::Cell<usize>,
}

impl<'a> LiveFilter<'a> {
    fn new(live: &'a HashSet<NodeId>) -> Self {
        LiveFilter { live, dropped: std::cell::Cell::new(0) }
    }
}

/// Over any graph source: the blast walk runs over the merged graph or its
/// identity quotient (DD.10), whose nodes are walk nodes.
impl<G: ?Sized> FilterPredicate<G> for LiveFilter<'_> {
    fn name(&self) -> &'static str {
        "live"
    }

    fn keep(&self, _: &G, id: NodeId, _: f64) -> bool {
        let keep = self.live.contains(&id);
        if !keep {
            self.dropped.set(self.dropped.get() + 1);
        }
        keep
    }
}

/// LD.6's fired_on line, once per answer that carries `live` flags:
/// `[live] annotate surface=<s> rows=<n> live=<l> entry_kinds=<k>` — `s` is
/// the answer (`resolve`, `docs`, `find`, `trace`; pyo3 prints `nodes_json`
/// in the same format), `n` its rows, `l` the rows flagged live, `k` the
/// entry table's kinds. Grep token `[live] annotate`.
pub(crate) fn live_marker(surface: &str, rows: usize, live: usize) {
    eprintln!(
        "[live] annotate surface={surface} rows={rows} live={live} entry_kinds={}",
        CODE_PROFILE.tables.entry.kinds.len()
    );
}

/// One located node in a `resolve` answer: identity + kind + PPR relevance +
/// `live` + `file`:`line`. (No `reason`/`depth` — `resolve` locates seeds, it
/// doesn't walk.)
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct LocatedNode {
    pub id: u64,
    pub qname: String,
    pub name: String,
    pub kind: &'static str,
    pub score: f64,
    /// Reachable from an entrypoint (LD.6, the flag [`BlastAnswer::live`]
    /// carries): `false` = likely dead.
    pub live: bool,
    pub file: Option<String>,
    /// 1-based (see [`Located`]).
    pub line: Option<i64>,
}

/// **resolve** (P3): a failure/change signal (stacktrace, diff, test id, or
/// `auto`) → the ranked, LOCATED nodes it points at, in one call. Resolution
/// order is preserved (stacktrace frames stay in order); `score` is PPR
/// relevance seeded by the whole resolved set, so shared-context nodes rank up.
/// `kind` ∈ {`stacktrace`, `test`, `diff`, `auto`}.
///
/// `scope` (A8.3) filters the SEEDS before PPR, not the rendered result, so the
/// scores genuinely change: `resolve_frame`/`resolve_file` match POSITION files
/// by BASENAME ONLY, so an unscoped frame naming `utils.py` seeds every
/// `utils.py` in the monorepo and those bogus seeds then shape the ranking of
/// the real one. A node with no locatable file is KEPT (see [`node_in_scope`]).
/// Any golden-output test on this function must pass `scope = None`.
///
/// LD.8a: an empty answer carries its [`Absence`](crate::absence::Absence).
/// `no_signal_match` when the signal resolved to no node — its note counts
/// the frames / added lines / paths / test ids the signal held, and it names
/// no mechanism, because resolution is by POSITION file and line or by name,
/// not by edges. `no_match` when `scope` (or `top_k = Some(0)`) removed every
/// resolved node.
///
/// Each row's `live` (LD.6) is read off one [`entrypoint_reachable`] walk,
/// run once per call; [`resolve_signal_located_with_live`] takes the set
/// instead.
pub fn resolve_signal_located(
    merged: &MergedGraph,
    text: &str,
    kind: &str,
    top_k: Option<usize>,
    scope: Option<&str>,
) -> Answer<LocatedNode> {
    resolve_signal_located_with_live(merged, &entrypoint_reachable(merged), text, kind, top_k, scope)
}

/// [`resolve_signal_located`] over a live set the caller already holds
/// (pyo3's `PyGraph` computes it once per graph): `live` must be
/// [`entrypoint_reachable`] of `merged`.
pub fn resolve_signal_located_with_live(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    text: &str,
    kind: &str,
    top_k: Option<usize>,
    scope: Option<&str>,
) -> Answer<LocatedNode> {
    let seeds = merged.resolve_signal(text, kind);
    let resolved = seeds.len();
    let loc = Locator::new(merged);
    // Pre-PPR: `activate` below must only see in-scope seeds.
    let seeds = apply_scope(&loc, seeds, scope, |id| *id, "resolve");
    if seeds.is_empty() {
        live_marker("resolve", 0, 0);
        return Answer::from_results(Vec::new(), || match scope {
            Some(s) if resolved > 0 => absence::scope_emptied(merged, "resolve", text, resolved, s),
            _ => absence::empty(
                merged,
                "resolve",
                text,
                "no_signal_match",
                signal_note(text, kind),
                &[],
                None,
            ),
        });
    }
    let mut config = CODE_PROFILE.tables.activation_config(None);
    config.direction = glia_activation::Direction::Undirected;
    config.top_k = usize::MAX;
    // The seeds ranked by PPR seeded at all of them (LD.12c): the plan scores
    // each seed, 0.0 where PPR gave it none; the rows keep resolution order.
    // One map over the view, not `score_of` per seed: a changed-file list can
    // seed thousands of nodes, and `score_of` is a linear scan.
    let view = ActivationPlan::new(config).rank(merged, &seeds, &seeds);
    let scores: HashMap<NodeId, f64> = view.scores.into_iter().collect();
    let mut out: Vec<LocatedNode> = seeds
        .iter()
        .map(|id| {
            let at = loc.locate(*id);
            LocatedNode {
                id: id.0,
                qname: at.qname,
                name: at.name,
                kind: at.kind,
                score: scores.get(id).copied().unwrap_or(0.0),
                live: live.contains(id),
                file: at.file,
                line: at.line,
            }
        })
        .collect();
    let kept = out.len();
    if let Some(k) = top_k {
        out.truncate(k);
    }
    live_marker("resolve", out.len(), out.iter().filter(|r| r.live).count());
    Answer::from_results(out, || {
        let note = format!(
            "top_k 0 kept none of the {kept} resolved {}",
            absence::plural(kept, "node", "nodes")
        );
        absence::empty(merged, "resolve", text, "no_match", note, &[], None)
    })
}

/// The `no_signal_match` note: what the signal held, counted with the shapes
/// the graph crate's resolver reads.
///
/// MIRRORS `graph/src/signal.rs` (`sniff_signal_kind`, `parse_stack_frames`,
/// `parse_diff_frames` and the plain changed-file-list fallback), which are
/// private there, while `MergedGraph::resolve_signal` returns node ids only
/// and reports no input count. Removal: when the graph crate exposes a signal
/// inventory, call it and delete this, [`sniff_kind`], [`frames_in_line`] and
/// [`added_lines`].
fn signal_note(text: &str, kind: &str) -> String {
    let (kind, auto) = if kind == "auto" {
        (sniff_kind(text), " (auto-detected)")
    } else {
        (kind, "")
    };
    let (n, one, many) = match kind {
        "stacktrace" => (
            text.lines().map(frames_in_line).sum(),
            "stack frame",
            "stack frames",
        ),
        "diff" => match added_lines(text) {
            0 => (
                text.lines()
                    .filter(|l| !l.trim().is_empty() && l.contains('.'))
                    .count(),
                "path",
                "paths",
            ),
            n => (n, "added line", "added lines"),
        },
        "test" => (
            text.split_whitespace().filter(|t| t.contains("::")).count(),
            "test id",
            "test ids",
        ),
        other => {
            return format!(
                "`{other}` is not a signal kind (stacktrace, test, diff or auto), so nothing was resolved"
            );
        }
    };
    format!(
        "the {kind} signal{auto} held {n} {}; none resolved to a node in this graph",
        absence::plural(n, one, many)
    )
}

/// MIRRORS `sniff_signal_kind` (see [`signal_note`]).
fn sniff_kind(text: &str) -> &'static str {
    if text.contains("+++ ") || text.contains("--- a/") || text.contains("\n@@ ") {
        return "diff";
    }
    if (text.contains("File \"") && text.contains("line "))
        || text.contains(".go:")
        || text.contains("\n  at ")
    {
        return "stacktrace";
    }
    let t = text.trim();
    if t.contains("::") && !t.chars().any(char::is_whitespace) {
        return "test";
    }
    "stacktrace"
}

/// Frames `parse_stack_frames` reads from one line: a Python `File "x", line
/// N` frame, else every `path.ext:N` token (see [`signal_note`]).
fn frames_in_line(line: &str) -> usize {
    if let Some(rest) = line.trim_start().strip_prefix("File \"")
        && let Some(end) = rest.find('"')
        && let Some(at) = rest[end..].find("line ")
        && rest[end + at + 5..].starts_with(|c: char| c.is_ascii_digit())
    {
        return 1;
    }
    line.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',')
        .filter(|tok| {
            let mut parts = tok.split(':');
            let path = parts.next().unwrap_or("");
            !path.is_empty()
                && path.contains('.')
                && !path.ends_with('.')
                && parts
                    .next()
                    .is_some_and(|n| n.starts_with(|c: char| c.is_ascii_digit()))
        })
        .count()
}

/// Added lines `parse_diff_frames` turns into frames: `+` lines under a
/// `+++ ` header that names a file, not `/dev/null` (see [`signal_note`]).
fn added_lines(text: &str) -> usize {
    let mut in_file = false;
    let mut n = 0;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("+++ ") {
            let p = p.split('\t').next().unwrap_or(p).trim();
            in_file = p != "/dev/null";
        } else if in_file && line.starts_with('+') {
            n += 1;
        }
    }
    n
}

/// **governing_docs** (P3 payoff, tier-4): the doc sections that DOCUMENTS a
/// symbol — "what are the rules for X?" — located, in one call. Direct
/// `doc --DOCUMENTS--> symbol` predecessors (the conservative, precise linker
/// signal). Reuses `LocatedNode` (score = 0; docs aren't PPR-ranked here).
///
/// `scope` (A8.3) keeps only the doc sections whose own POSITION file lives
/// under that repo-relative path. A section with no locatable file is KEPT (see
/// [`node_in_scope`]).
///
/// The symbol resolves through find's search (the LD.3b handoff): its first
/// row, when that row is an exact match (`exact_qname` / `exact_name`). Those
/// tiers are ordered by `pick_primary`'s key, so it is the node
/// `node_id_by_qname` / `resolve_name` return; a dotted or slashed query
/// (`app.helper`) also finds its `::` qname, as find does.
///
/// LD.8a: never an error. An empty answer's absence is `unknown_symbol`
/// (find's nearest qnames as suggestions), `no_edges` (no DOCUMENTS edge
/// reaches the symbol; caveats narrowed to the symbol's language), or
/// `no_match` (`scope` removed every section).
///
/// Each row's `live` (LD.6) is read off one [`entrypoint_reachable`] walk,
/// run once per call; [`governing_docs_with_live`] takes the set instead.
pub fn governing_docs(
    merged: &MergedGraph,
    qname: &str,
    scope: Option<&str>,
) -> Answer<LocatedNode> {
    governing_docs_with_live(merged, &entrypoint_reachable(merged), qname, scope)
}

/// [`governing_docs`] over a live set the caller already holds (pyo3's
/// `PyGraph` computes it once per graph): `live` must be
/// [`entrypoint_reachable`] of `merged`.
pub fn governing_docs_with_live(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    qname: &str,
    scope: Option<&str>,
) -> Answer<LocatedNode> {
    // The rows are DOC_SECTIONs; the table names the edge a section reaches a
    // symbol through.
    let mechanisms = absence::mechanisms_for_kind(node_kind::DOC_SECTION);
    let near_opts = FindOptions {
        top_k: absence::SUGGESTIONS,
        ..FindOptions::default()
    };
    let near = find::search(merged, qname, &near_opts).rows;
    let Some(target) = near
        .first()
        .filter(|r| find::is_exact(r))
        .map(|r| NodeId(r.id))
    else {
        live_marker("docs", 0, 0);
        return Answer::from_results(Vec::new(), || {
            absence::unknown_symbol(merged, "governing_docs", qname, mechanisms, &near)
        });
    };
    let loc = Locator::new(merged);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for e in merged.all_edges() {
        if e.to == target
            && e.category == edge_category::DOCUMENTS
            && seen.insert(e.from)
        {
            let at = loc.locate(e.from);
            out.push(LocatedNode {
                id: e.from.0,
                qname: at.qname,
                name: at.name,
                kind: at.kind,
                score: 0.0,
                live: live.contains(&e.from),
                file: at.file,
                line: at.line,
            });
        }
    }
    let documented = out.len();
    let out = apply_scope(&loc, out, scope, |d| NodeId(d.id), "governing_docs");
    live_marker("docs", out.len(), out.iter().filter(|r| r.live).count());
    Answer::from_results(out, || match scope {
        Some(s) if documented > 0 => {
            absence::scope_emptied(merged, "governing_docs", qname, documented, s)
        }
        _ => {
            let at = loc.locate(target);
            let note = format!("no DOCUMENTS edge reaches `{}` in this graph", at.qname);
            absence::empty(
                merged,
                "governing_docs",
                qname,
                "no_edges",
                note,
                mechanisms,
                at.file.as_deref(),
            )
        }
    })
}

/// Identity + location of one node, shared by every answer record.
///
/// **`line` is 1-based** — the first line of the node's span as an editor
/// shows it; `None` when no tier places the node. This is the ONE line
/// convention of every answer record (`blast_radius`,
/// `cross_stack_trace`, `resolve_signal_located`, `governing_docs`,
/// `message_contracts`) and of the pyo3 `nodes_json` span. POSITION cells keep
/// storing 0-based tree-sitter rows; [`Locator::locate`] converts, once, at its
/// single exit. A record builder copies `line` as-is and never adds 1 again.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Located {
    pub id: u64,
    pub name: String,
    pub qname: String,
    pub kind: &'static str,
    pub file: Option<String>,
    pub line: Option<i64>,
}

/// A node index over one [`MergedGraph`], built once in O(V) so an answer with
/// R located rows costs O(V + R) instead of the O(R × V) of a per-row scan.
/// Build one per answer and pass it down; never build one per row.
///
/// Placement has three tiers, first hit wins:
///
/// 1. the node's first POSITION cell ([`position_of`]) — every parsed entity,
///    and the go / ts_routes ROUTEs (a POSITION per registration);
/// 2. ROUTE / ENDPOINT only (A3.6): the ENDPOINT_HIT cell or a JSON
///    ROUTE_METHOD cell ([`endpoint::http_node_span`]), which carry the call
///    or registration site;
/// 3. ROUTE only (A3.6): the POSITION of the handler the route is HANDLED_BY,
///    for the parsers whose ROUTE_METHOD is the bare verb. "Where is this
///    route" is answered by its handler.
///
/// All three produce 0-based ROWS internally; only [`Locator::locate`] turns a
/// row into a 1-based `line`. `None` for nodes no tier places (DOC_SPACE, a
/// handler-less Django/Rails route).
pub struct Locator<'a> {
    merged: &'a MergedGraph,
    /// NodeId → index of the FIRST graph (in `merged.graphs` order) whose nav
    /// names it. Built with `or_insert` over graphs in Vec order, so the
    /// per-graph HashMap iteration order cannot change the winner.
    first_graph: HashMap<NodeId, usize>,
    /// Per graph: NodeId → index of its FIRST node in `g.nodes` with that id.
    node_at: Vec<HashMap<NodeId, usize>>,
}

impl<'a> Locator<'a> {
    /// Index every graph of `merged`. Prints the LD.1 fired_on marker once per
    /// process (a Locator is built per answer, so a per-build line would flood
    /// a long-running MCP server's stderr).
    pub fn new(merged: &'a MergedGraph) -> Self {
        let mut first_graph: HashMap<NodeId, usize> = HashMap::new();
        let mut node_at: Vec<HashMap<NodeId, usize>> = Vec::with_capacity(merged.graphs.len());
        for (gi, g) in merged.graphs.iter().enumerate() {
            for id in g.nav.qname_by_id.keys() {
                first_graph.entry(*id).or_insert(gi);
            }
            let mut at: HashMap<NodeId, usize> = HashMap::with_capacity(g.nodes.len());
            for (ni, n) in g.nodes.iter().enumerate() {
                at.entry(n.id).or_insert(ni);
            }
            node_at.push(at);
        }
        static BUILT: std::sync::Once = std::sync::Once::new();
        BUILT.call_once(|| {
            eprintln!(
                "[locate] locator built: nodes={} graphs={} line_base=1",
                first_graph.len(),
                merged.graphs.len()
            );
        });
        Locator { merged, first_graph, node_at }
    }

    /// Identity + 1-based location of `id`. An id no graph names comes back as
    /// `qname: "(unknown:<id>)"`, `kind: "UNKNOWN"`, unlocated. A node named
    /// by a graph's nav but absent from its node list keeps its name and qname
    /// and is unlocated.
    pub fn locate(&self, id: NodeId) -> Located {
        let Some((gi, g)) = self.graph_of(id) else {
            return Located {
                id: id.0,
                name: String::new(),
                qname: format!("(unknown:{})", id.0),
                kind: "UNKNOWN",
                file: None,
                line: None,
            };
        };
        let name = g.nav.name_by_id.get(&id).cloned().unwrap_or_default();
        let qname = g.nav.qname_by_id.get(&id).cloned().unwrap_or_default();
        let kind_id = g.nav.kind_by_id.get(&id).copied();
        let (file, row) = self.place(gi, g, id, &qname);
        Located {
            id: id.0,
            name,
            qname,
            kind: kind_id.map(node_kind::name).unwrap_or("UNKNOWN"),
            file,
            // THE conversion: a 0-based stored row becomes a 1-based line here
            // and nowhere else. Saturating, so a corrupt i64::MAX row cannot
            // overflow-panic a debug build.
            line: row.map(|r| r.saturating_add(1)),
        }
    }

    /// The repo-relative file [`Locator::locate`] reports for `id` — what the
    /// A8.3 scope filter keys on — without cloning the name and qname.
    pub(crate) fn file_of(&self, id: NodeId) -> Option<String> {
        let (gi, g) = self.graph_of(id)?;
        let qname = g.nav.qname_by_id.get(&id).map(String::as_str).unwrap_or("");
        self.place(gi, g, id, qname).0
    }

    fn graph_of(&self, id: NodeId) -> Option<(usize, &'a RepoGraph)> {
        let gi = *self.first_graph.get(&id)?;
        Some((gi, self.merged.graphs.get(gi)?))
    }

    fn node(&self, gi: usize, g: &'a RepoGraph, id: NodeId) -> Option<&'a Node> {
        g.nodes.get(*self.node_at.get(gi)?.get(&id)?)
    }

    /// `(file, 0-based row)` through the three tiers.
    fn place(
        &self,
        gi: usize,
        g: &'a RepoGraph,
        id: NodeId,
        qname: &str,
    ) -> (Option<String>, Option<i64>) {
        let Some(n) = self.node(gi, g, id) else {
            return (None, None);
        };
        let placed = position_of(n);
        if placed.0.is_some() {
            return placed;
        }
        let kind_id = g.nav.kind_by_id.get(&id).copied();
        let is_route = kind_id == Some(node_kind::ROUTE);
        if !is_route && kind_id != Some(node_kind::ENDPOINT) {
            return placed;
        }
        if let Some((f, l)) = endpoint::http_node_span(&n.cells) {
            log_http_locate_once("cell", qname);
            return (Some(f), l);
        }
        if is_route && let Some((f, l)) = self.handler_position(gi, g, id) {
            log_http_locate_once("handled_by", qname);
            return (Some(f), l);
        }
        placed
    }

    /// Tier 3: the POSITION of the first handler (in edge order) that `route`
    /// is HANDLED_BY and that carries a file. Confined to the route's own repo
    /// graph — a ROUTE's HANDLED_BY never crosses repos — so it costs one scan
    /// of that repo's edges, and only for a route tiers 1–2 could not place.
    /// The handler is found through the node index, not a node scan.
    fn handler_position(
        &self,
        gi: usize,
        g: &'a RepoGraph,
        route: NodeId,
    ) -> Option<(String, Option<i64>)> {
        g.edges
            .iter()
            .filter(|e| e.from == route && e.category == edge_category::HANDLED_BY)
            .find_map(|e| match position_of(self.node(gi, g, e.to)?) {
                (Some(f), l) => Some((f, l)),
                (None, _) => None,
            })
    }
}

/// [`Located`] for one node — a one-off [`Locator`]. O(V) per call: a caller
/// locating more than one node builds a `Locator` once and calls
/// [`Locator::locate`] instead.
pub fn locate_node(merged: &MergedGraph, id: NodeId) -> Located {
    Locator::new(merged).locate(id)
}

/// `(file, start_line)` from a node's FIRST parseable POSITION cell;
/// `(None, None)` when it has none. `start_line` is the stored 0-based row.
///
/// FIRST POSITION WINS — A2.8, and it is load-bearing, not cosmetic. A node
/// can carry MORE than one POSITION cell: `merge_parses` appends the cells of
/// every `FileParse` that minted the same NodeId, which is the normal case
/// for a queue topic two files publish to. Returning on the first parseable
/// cell is what keeps `blast_radius` / `trace` / `resolve` on the same file
/// as `passes::position_file` and `projection_text::node_position`, both of
/// which return the first. Scanning on would let the LAST file parsed win.
fn position_of(n: &Node) -> (Option<String>, Option<i64>) {
    for c in &n.cells {
        if c.kind != cell_type::POSITION {
            continue;
        }
        if let CellPayload::Json(s) | CellPayload::Text(s) = &c.payload
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(s)
        {
            let file = v.get("file").and_then(|f| f.as_str()).map(String::from);
            let line = v.get("start_line").and_then(serde_json::Value::as_i64);
            return (file, line);
        }
    }
    (None, None)
}

/// The A3.6 fired_on marker, once per process PER TIER: [`Locator::locate`]
/// runs inside the P3 answer loops, so a per-call line would flood stderr,
/// while one line per process would hide
/// whichever tier fired second. `source` is `cell` (tier 2) or `handled_by`
/// (tier 3), so a run prints at most two lines.
fn log_http_locate_once(source: &'static str, qname: &str) {
    static CELL: std::sync::Once = std::sync::Once::new();
    static HANDLED_BY: std::sync::Once = std::sync::Once::new();
    let once = if source == "cell" { &CELL } else { &HANDLED_BY };
    once.call_once(|| {
        eprintln!("[locate] http span fallback fired: source={source} node={qname}");
    });
}

// ============================================================================
// A8.3 — `scope`: restrict a P3 answer to one part of a monorepo.
//
// Two rules that are load-bearing rather than cosmetic:
//   1. the filter runs BEFORE `truncate(top_k)` (blast_radius) and BEFORE
//      `activate` (resolve), so it changes the RANKING, not just the display;
//   2. a node with NO locatable file is KEPT. Dropping unlocatable nodes would
//      silently delete every DOC_SPACE (and, before A3.6 located them, every
//      ENDPOINT / ROUTE) from a scoped answer and destroy the cross-service
//      result these primitives exist for. The `[scope]` marker reports the
//      kept-unlocatable count so that is visible.
// ============================================================================

/// The repo-relative path a node should be scoped by: exactly the file
/// [`Locator::locate`] reports. Since A3.6 that places ENDPOINT nodes by their
/// ENDPOINT_HIT call site and ROUTE nodes by their JSON ROUTE_METHOD cell or
/// their HANDLED_BY handler, so an HTTP node is scoped where it is defined
/// instead of being kept as unlocatable. `None` only for the nodes no tier
/// places (DOC_SPACE, a handler-less route) — those fall under the
/// keep-unlocatable rule above.
fn scope_file_of(loc: &Locator<'_>, id: NodeId) -> Option<String> {
    loc.file_of(id)
}

/// True when `file` lives under `scope`. Prefix match on a `/` boundary only,
/// so `scope = "services/ap"` does NOT match `services/api/handler.py`. Both
/// sides are normalised by trimming a leading `./` or `/` and a trailing `/`;
/// an empty scope matches everything, and so does `.` — the path the ROOT
/// project resolves to (A8.6), which otherwise matched nothing at all.
pub(crate) fn in_scope(file: &str, scope: &str) -> bool {
    let f = file.trim_start_matches("./").trim_start_matches('/');
    let s = scope.trim_start_matches("./").trim_matches('/');
    if s.is_empty() || s == "." {
        return true;
    }
    f == s || f.starts_with(&format!("{s}/"))
}

/// Would this node survive `scope`? `scope = None` is always true, and so is a
/// node with no locatable file (the keep-unlocatable policy above). Public so
/// the pyo3 `find_nodes_by_qname` surface applies exactly the same rule as the
/// three scoped primitives rather than re-deriving a path guess in Python.
///
/// `scope` narrows WITHIN a repo: under a multi-repo merge each repo's POSITION
/// paths are relative to its OWN root, so a path that was passed as a separate
/// `--with` repo will not match as a scope.
///
/// `scope` here is a PATH. A caller holding a user-supplied scope that may be
/// a project label resolves it ONCE with [`resolve_scope`] before looping —
/// this runs per node, and resolving per node would walk the graph N times.
///
/// Builds a one-off [`Locator`] when `scope` is set: O(V) per call, the same
/// order as the node scan it replaced. A caller filtering many nodes builds
/// one `Locator` and filters on its `file` instead.
pub fn node_in_scope(merged: &MergedGraph, id: NodeId, scope: Option<&str>) -> bool {
    let Some(s) = scope else { return true };
    match scope_file_of(&Locator::new(merged), id) {
        Some(f) => in_scope(&f, s),
        None => true,
    }
}

/// One shared applier so the scoped call sites cannot drift apart. `scope =
/// None` is a strict no-op: the input is returned untouched and no marker is
/// emitted, so no existing answer, ranking or cell value changes.
///
/// A8.6: the scope is label-resolved HERE, once per call, so all three scoped
/// primitives — and the pyo3 / CLI surfaces over them — accept `@shop/web` as
/// well as `apps/web` without any of them knowing about labels. Resolution is
/// a pass-through for anything that is not a project, so every A8.3 path
/// scope behaves exactly as before.
fn apply_scope<T>(
    loc: &Locator<'_>,
    items: Vec<T>,
    scope: Option<&str>,
    id_of: impl Fn(&T) -> NodeId,
    what: &str,
) -> Vec<T> {
    let Some(raw) = scope else { return items };
    let resolved = resolve_scope(loc.merged, raw);
    let s = resolved.as_str();
    let before = items.len();
    let mut unlocatable = 0usize;
    let out: Vec<T> = items
        .into_iter()
        .filter(|it| match scope_file_of(loc, id_of(it)) {
            Some(f) => in_scope(&f, s),
            None => {
                unlocatable += 1;
                true
            }
        })
        .collect();
    eprintln!(
        "[scope] {what} scope={s}: {before} -> {} (unlocatable={unlocatable})",
        out.len()
    );
    out
}

// ============================================================================
// A8.6 — project roots as the `scope` vocabulary.
//
// A8.3's scope is a repo-relative path, which is exactly what a human or an
// agent does not know in a monorepo: you know the service is `@shop/web`, not
// that it lives at `apps/web`. A8.5 put both in the graph as PROJECT anchors;
// this reads them back. Nothing is cached and nothing rides on a side field,
// so a freshly generated graph and one reopened from a `.gmap` answer alike.
// ============================================================================

/// One manifest-rooted sub-project: a PROJECT anchor (A8.5) decoded from its
/// ORIGIN cell. `path` is repo-relative, `.` for the repo root.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProjectInfo {
    pub qname: String,
    pub label: String,
    pub ecosystem: String,
    pub manifest: String,
    pub path: String,
}

/// Every PROJECT anchor in the graph, sorted by `path` (then `qname`, for the
/// several `.` roots of a `--with` merge) so callers and tests see a stable
/// order. A node whose ORIGIN cell is missing or will not parse is SKIPPED,
/// not fatal — the same degrade-don't-fail rule `locate_node` applies to a
/// bad POSITION cell.
pub fn project_roots(merged: &MergedGraph) -> Vec<ProjectInfo> {
    let mut out = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::PROJECT) {
                continue;
            }
            let origin = n.cells.iter().filter(|c| c.kind == cell_type::ORIGIN).find_map(|c| {
                match &c.payload {
                    CellPayload::Json(s) | CellPayload::Text(s) => {
                        serde_json::from_str::<serde_json::Value>(s).ok()
                    }
                    CellPayload::Bytes(_) => None,
                }
            });
            let Some(v) = origin.filter(serde_json::Value::is_object) else { continue };
            let field = |k: &str| v.get(k).and_then(serde_json::Value::as_str).map(String::from);
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            // The walker records the root as `""`; `.` is what the qname says
            // and what a human types.
            let path = field("path")
                .or_else(|| qname.strip_prefix("project:").map(String::from))
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| ".".to_string());
            let label = field("label")
                .or_else(|| g.nav.name_by_id.get(&n.id).cloned())
                .unwrap_or_default();
            out.push(ProjectInfo {
                qname,
                label,
                ecosystem: field("ecosystem").unwrap_or_default(),
                manifest: field("manifest").unwrap_or_default(),
                path,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.qname.cmp(&b.qname)));
    out
}

/// Turn a user-supplied scope into a repo-relative path.
///
/// 1. A project's own path (`apps/web`, or `.`) is returned UNCHANGED — so
///    this is idempotent: resolving an already-resolved scope is a no-op, and
///    a path wins over a label that happens to spell the same string.
/// 2. A project's full qname (`project:apps/web`) resolves to its path.
/// 3. An exact `label` match (`@shop/web`) resolves to its path. When several
///    projects share the label (two Maven modules with one artifactId), the
///    lexicographically smallest path wins and a `[scope] ambiguous` warning
///    names all of them — never a panic, never a silent pick.
/// 4. Anything else passes through untouched: a literal path scope.
///
/// O(nodes) per call; the scoped primitives call it once, not per node.
pub fn resolve_scope(merged: &MergedGraph, scope: &str) -> String {
    let roots = project_roots(merged);
    if roots.iter().any(|p| p.path == scope) {
        return scope.to_string();
    }
    if let Some(p) = roots.iter().find(|p| p.qname == scope) {
        eprintln!("[scope] resolved qname '{scope}' -> {}", p.path);
        return p.path.clone();
    }
    // `roots` is sorted by path, so the first match is the smallest path.
    let hits: Vec<&ProjectInfo> = roots.iter().filter(|p| p.label == scope).collect();
    let Some(first) = hits.first() else {
        return scope.to_string();
    };
    if hits.len() > 1 {
        let paths: Vec<&str> = hits.iter().map(|p| p.path.as_str()).collect();
        eprintln!(
            "[scope] ambiguous label '{scope}' matches {} projects ({}); using {}",
            hits.len(),
            paths.join(", "),
            first.path
        );
    }
    // fired_on marker (A8.6). Label path only — a literal path scope never
    // prints it, so it cannot be confused with A8.3's `[scope] <what>` line.
    eprintln!("[scope] resolved label '{scope}' -> {}", first.path);
    first.path.clone()
}

// ============================================================================
// A12.2 — `message_contracts`: every queue topic, both sides, located, with a
// match / mismatch / unknown verdict on the payload type (A12.1's
// MESSAGE_TYPE cell).
//
// Pairing is NOT re-derived here. A producer and a consumer are a pair exactly
// when `QueueStackResolver` emitted a QUEUE_FLOWS edge between them, so the
// report inherits every rule the resolver applies — the unpairable framework
// tag (A2.3), the broker-family gate and the wildcard dialects (A2.7) — and
// cannot disagree with what `trace` / `blast_radius` walk. Topics are the
// queue qnames as the extractors minted them (A2.5 task identity, A2.6 folded
// URL/ARN topics), stripped of their `queue_producer:` / `queue_consumer:`
// prefix and nothing else.
//
// Every queue node the resolver left unpaired still gets ONE one-sided row.
// That is the unpaired-topic signal, and it is what stops a framework-tag
// topic fanning out into an N×M cross product: 3 tag producers + 4 tag
// consumers are 7 rows, never 12.
//
// fired_on marker, one line per call that produced rows:
//   `... 2>&1 | grep '^\[contracts\] topics='`
// ============================================================================

const PRODUCER_PREFIX: &str = "queue_producer:";
const CONSUMER_PREFIX: &str = "queue_consumer:";

const NOTE_TAG: &str = "topic is a framework tag, not a literal — no trustworthy pairing";
const NOTE_NO_COUNTERPART: &str = "no counterpart for this topic";
const NOTE_FAMILY_SPLIT: &str =
    "a counterpart names this topic but the resolver did not pair them (broker family differs)";
const NOTE_NO_PRODUCER_TYPE: &str = "producer side has no detectable message type";
const NOTE_NO_CONSUMER_TYPE: &str = "consumer side has no detectable message type";
const NOTE_NO_TYPES: &str = "neither side has a detectable message type";
const NOTE_CONFLICT: &str =
    "a side carries conflicting message types across its call sites — see types_seen";
const NOTE_PRIMITIVE: &str =
    "a side carries a primitive payload (a serialised body) — the types are not comparable";
const NOTE_PRIMITIVE_MATCH: &str =
    "both sides carry the same primitive payload — the real schema is serialised inside it";
const NOTE_FILE_WINDOW: &str =
    "a side's type was read from the whole file, not beside the call site";

/// Payload type names that mean "serialised body", not "schema". A12.1 keeps
/// them on purpose — by the Kafka `<Key, Value>` convention the last type
/// argument IS the value type — so they are weighed here: a primitive never
/// supports a `mismatch` (a `string` body is routinely decoded into a struct)
/// and only ever a weak `match`. Heuristic; extend as fixtures accumulate.
const PRIMITIVE_PAYLOADS: &[&str] = &[
    "string", "String", "str", "Null", "Ignore", "byte", "bytes", "Bytes", "object", "Object",
];

fn is_primitive_payload(ty: &str) -> bool {
    PRIMITIVE_PAYLOADS.contains(&ty)
}

/// One side of a message contract: the queue node, where it lives, and the
/// payload type it says it carries.
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct MessageContractSide {
    pub node_id: u64,
    /// `RepoId.0`; `GenerateResult::repo_labels` maps it to a human label.
    pub repo_id: u64,
    pub qname: String,
    /// The topic as THIS node names it. It differs from the row's `topic` only
    /// for a wildcard subscriber (`orders.*`) the resolver matched by pattern.
    pub topic: String,
    /// Qualified name of the parent MODULE — the file the call site is in.
    pub module: Option<String>,
    /// The node's own POSITION (A2.8: first call site in the first file that
    /// uses the topic), else the parent MODULE's. `line` is 1-based (see
    /// [`Located`]).
    pub file: Option<String>,
    pub line: Option<i64>,
    /// The best MESSAGE_TYPE `type`: beside-the-call-site before whole-file,
    /// schema before primitive, then first-seen.
    pub message_type: Option<String>,
    /// The type as written (`pb.OrderCreated`).
    pub message_type_raw: Option<String>,
    /// `"generic"` | `"struct_literal"`.
    pub form: Option<String>,
    /// `"near"` | `"file"`.
    pub window: Option<String>,
    /// Every distinct `type` across the node's MESSAGE_TYPE cells, first-seen
    /// order. A topic used from N files carries up to N cells.
    pub types_seen: Vec<String>,
    /// More than one distinct non-primitive type was read beside this node's
    /// call sites (whole-file guesses count only when there is no near one).
    pub conflicting: bool,
}

/// One row of the contract report.
///
/// `status` is `"match"`, `"mismatch"` or `"unknown"`. `confidence` is
/// `"strong"` or `"weak"` for a verdict and `"none"` for `unknown`. `note` says
/// why whenever the row is anything other than a strong verdict.
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct MessageContractRow {
    pub topic: String,
    pub topic_is_tag: bool,
    /// The consumer subscribes by a wildcard pattern the resolver matched (A2.7).
    pub pattern: bool,
    pub producer: Option<MessageContractSide>,
    pub consumer: Option<MessageContractSide>,
    pub status: &'static str,
    pub confidence: &'static str,
    pub note: Option<&'static str>,
}

/// A queue node folded across every graph it appears in. One NodeId can sit in
/// several per-language graphs of one repo — the id hashes repo + kind +
/// qname, not language — and each copy carries its own files' cells.
struct QueueNodeAcc<'a> {
    producer: bool,
    repo: u64,
    /// The full qname, LB.8 owner segment included, so a side still says
    /// which project it is.
    qname: &'a str,
    /// The bare topic: the owner segment is not part of the channel.
    topic: &'a str,
    parent: Option<NodeId>,
    types: Vec<&'a Cell>,
}

/// One MESSAGE_TYPE cell, parsed. A cell that is not JSON or has no `type`
/// parses to nothing — never a panic.
struct ParsedType {
    ty: String,
    raw: Option<String>,
    form: Option<String>,
    window: Option<String>,
}

impl ParsedType {
    fn near(&self) -> bool {
        self.window.as_deref() == Some("near")
    }
}

fn parse_message_type(c: &Cell) -> Option<ParsedType> {
    let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
        return None;
    };
    let v = serde_json::from_str::<serde_json::Value>(s).ok()?;
    let field = |k: &str| v.get(k).and_then(serde_json::Value::as_str).map(String::from);
    Some(ParsedType {
        ty: field("type").filter(|t| !t.is_empty())?,
        raw: field("raw"),
        form: field("form"),
        window: field("window"),
    })
}

fn collect_queue_nodes(merged: &MergedGraph) -> BTreeMap<u64, QueueNodeAcc<'_>> {
    let mut out: BTreeMap<u64, QueueNodeAcc<'_>> = BTreeMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let (producer, prefix) = match g.nav.kind_by_id.get(&n.id) {
                Some(k) if *k == node_kind::QUEUE_PRODUCER => (true, PRODUCER_PREFIX),
                Some(k) if *k == node_kind::QUEUE_CONSUMER => (false, CONSUMER_PREFIX),
                _ => continue,
            };
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(topic) = endpoint::split_owner(qname).0.strip_prefix(prefix) else { continue };
            let acc = out.entry(n.id.0).or_insert_with(|| QueueNodeAcc {
                producer,
                repo: n.repo.0,
                qname,
                topic,
                parent: None,
                types: Vec::new(),
            });
            if acc.parent.is_none() {
                acc.parent = g.nav.parent_of.get(&n.id).copied();
            }
            acc.types
                .extend(n.cells.iter().filter(|c| c.kind == cell_type::MESSAGE_TYPE));
        }
    }
    out
}

fn contract_side(loc: &Locator<'_>, id: u64, acc: &QueueNodeAcc<'_>) -> MessageContractSide {
    let parsed: Vec<ParsedType> = acc.types.iter().filter_map(|c| parse_message_type(c)).collect();
    // `min_by_key` keeps the FIRST of equal keys, so first-seen breaks ties.
    let best = parsed
        .iter()
        .min_by_key(|t| (!t.near(), is_primitive_payload(&t.ty)));
    let mut types_seen: Vec<String> = Vec::new();
    for t in &parsed {
        if !types_seen.contains(&t.ty) {
            types_seen.push(t.ty.clone());
        }
    }
    // Conflict is judged on the near reads when there are any: a whole-file
    // guess is too weak to contradict a type read beside the call site.
    let any_near = parsed.iter().any(ParsedType::near);
    let mut schemas: Vec<&str> = Vec::new();
    for t in parsed.iter().filter(|t| t.near() || !any_near) {
        if !is_primitive_payload(&t.ty) && !schemas.contains(&t.ty.as_str()) {
            schemas.push(&t.ty);
        }
    }

    let mut at = loc.locate(NodeId(id));
    if at.file.is_none()
        && let Some(p) = acc.parent
    {
        at = loc.locate(p);
    }
    let (file, line) = (at.file, at.line);
    let module = acc
        .parent
        .and_then(|p| {
            loc.merged.graphs.iter().find_map(|g| g.nav.qname_by_id.get(&p).cloned())
        });

    MessageContractSide {
        node_id: id,
        repo_id: acc.repo,
        qname: acc.qname.to_string(),
        topic: acc.topic.to_string(),
        module,
        file,
        line,
        message_type: best.map(|t| t.ty.clone()),
        message_type_raw: best.and_then(|t| t.raw.clone()),
        form: best.and_then(|t| t.form.clone()),
        window: best.and_then(|t| t.window.clone()),
        types_seen,
        conflicting: schemas.len() > 1,
    }
}

/// `(status, confidence, note)` for one resolver-made pair.
fn contract_verdict(
    p: &MessageContractSide,
    c: &MessageContractSide,
) -> (&'static str, &'static str, Option<&'static str>) {
    const UNKNOWN: (&str, &str) = ("unknown", "none");
    let (pt, ct) = match (p.message_type.as_deref(), c.message_type.as_deref()) {
        (Some(pt), Some(ct)) => (pt, ct),
        (None, None) => return (UNKNOWN.0, UNKNOWN.1, Some(NOTE_NO_TYPES)),
        (None, Some(_)) => return (UNKNOWN.0, UNKNOWN.1, Some(NOTE_NO_PRODUCER_TYPE)),
        (Some(_), None) => return (UNKNOWN.0, UNKNOWN.1, Some(NOTE_NO_CONSUMER_TYPE)),
    };
    if p.conflicting || c.conflicting {
        return (UNKNOWN.0, UNKNOWN.1, Some(NOTE_CONFLICT));
    }
    let primitive = is_primitive_payload(pt) || is_primitive_payload(ct);
    let whole_file = p.window.as_deref() != Some("near") || c.window.as_deref() != Some("near");
    if pt == ct {
        let note = if primitive {
            Some(NOTE_PRIMITIVE_MATCH)
        } else {
            whole_file.then_some(NOTE_FILE_WINDOW)
        };
        return ("match", if note.is_some() { "weak" } else { "strong" }, note);
    }
    if primitive {
        return (UNKNOWN.0, UNKNOWN.1, Some(NOTE_PRIMITIVE));
    }
    let note = whole_file.then_some(NOTE_FILE_WINDOW);
    ("mismatch", if whole_file { "weak" } else { "strong" }, note)
}

/// Every queue topic in `merged` as producer/consumer contract rows — one per
/// resolver-made pair, plus one one-sided row per queue node the resolver left
/// unpaired. Sorted by (topic, producer file/id, consumer file/id), so two
/// calls over the same graph serialise byte-identically.
///
/// Read-only: no node, edge or cell is added or changed. A graph whose
/// resolvers never ran has no QUEUE_FLOWS edges, so every row is one-sided.
pub fn message_contracts(merged: &MergedGraph) -> Vec<MessageContractRow> {
    let nodes = collect_queue_nodes(merged);
    let is_tag_id = |id: &u64| nodes.get(id).is_some_and(|a| is_framework_tag(a.topic));
    // A node folded from two graphs is walked twice by the resolver, so its
    // edges can repeat: the set dedups them. Tags are refused even if some
    // future resolver pairs them.
    let pairs: BTreeSet<(u64, u64)> = merged
        .all_edges()
        .filter(|e| e.category == edge_category::QUEUE_FLOWS)
        .map(|e| (e.from.0, e.to.0))
        .filter(|(f, t)| {
            nodes.get(f).is_some_and(|a| a.producer)
                && nodes.get(t).is_some_and(|a| !a.producer)
                && !is_tag_id(f)
                && !is_tag_id(t)
        })
        .collect();
    let loc = Locator::new(merged);
    let sides: BTreeMap<u64, MessageContractSide> = nodes
        .iter()
        .map(|(id, acc)| (*id, contract_side(&loc, *id, acc)))
        .collect();

    let mut rows: Vec<MessageContractRow> = Vec::new();
    let mut paired: BTreeSet<u64> = BTreeSet::new();
    for (p, c) in &pairs {
        let (Some(ps), Some(cs)) = (sides.get(p), sides.get(c)) else { continue };
        paired.insert(*p);
        paired.insert(*c);
        let (status, confidence, note) = contract_verdict(ps, cs);
        rows.push(MessageContractRow {
            topic: ps.topic.clone(),
            topic_is_tag: false,
            pattern: ps.topic != cs.topic,
            producer: Some(ps.clone()),
            consumer: Some(cs.clone()),
            status,
            confidence,
            note,
        });
    }

    // Which directions name each topic, to tell "nobody on the other side"
    // from "somebody the resolver refused" (the family gate).
    let mut directions: HashMap<&str, (bool, bool)> = HashMap::new();
    for acc in nodes.values() {
        let d = directions.entry(acc.topic).or_default();
        if acc.producer {
            d.0 = true;
        } else {
            d.1 = true;
        }
    }
    for (id, acc) in &nodes {
        if paired.contains(id) {
            continue;
        }
        let Some(side) = sides.get(id) else { continue };
        let tag = is_framework_tag(acc.topic);
        let note = if tag {
            NOTE_TAG
        } else {
            let (has_p, has_c) = directions.get(acc.topic).copied().unwrap_or_default();
            if (acc.producer && has_c) || (!acc.producer && has_p) {
                NOTE_FAMILY_SPLIT
            } else {
                NOTE_NO_COUNTERPART
            }
        };
        let (producer, consumer) = if acc.producer {
            (Some(side.clone()), None)
        } else {
            (None, Some(side.clone()))
        };
        rows.push(MessageContractRow {
            topic: acc.topic.to_string(),
            topic_is_tag: tag,
            pattern: false,
            producer,
            consumer,
            status: "unknown",
            confidence: "none",
            note: Some(note),
        });
    }

    fn side_key(s: &Option<MessageContractSide>) -> Option<(Option<&str>, u64)> {
        s.as_ref().map(|s| (s.file.as_deref(), s.node_id))
    }
    rows.sort_by(|a, b| {
        (a.topic.as_str(), side_key(&a.producer), side_key(&a.consumer)).cmp(&(
            b.topic.as_str(),
            side_key(&b.producer),
            side_key(&b.consumer),
        ))
    });

    if !rows.is_empty() {
        let count = |s: &str| rows.iter().filter(|r| r.status == s).count();
        let topics: BTreeSet<(&str, bool)> =
            rows.iter().map(|r| (r.topic.as_str(), r.topic_is_tag)).collect();
        let tag_topics = topics.iter().filter(|(_, tag)| *tag).count();
        eprintln!(
            "[contracts] topics={} match={} mismatch={} unknown={} literal={} tag={} rows={}",
            topics.len(),
            count("match"),
            count("mismatch"),
            count("unknown"),
            topics.len() - tag_topics,
            tag_topics,
            rows.len()
        );
    }
    rows
}

#[cfg(test)]
mod locate_tests {
    use super::locate_node;
    use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
    use glia_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};
    use glia_graph::{MergedGraph, RepoGraph, SymbolTable};

    /// A2.8 — FIRST POSITION WINS, and `position_of` returning on the first
    /// parseable cell is what makes it so. `merge_parses` appends the cells of every `FileParse` that
    /// minted the same NodeId, so a queue topic published from two files
    /// carries TWO POSITION cells. Without the break the LAST file parsed won,
    /// which disagreed with `passes::position_file` and
    /// `projection_text::node_position` — both of which return the first.
    ///
    /// A3.6 refactors these lines: this test is the contract. If it starts
    /// failing with `b.go`, first-wins was reverted.
    #[test]
    fn locate_node_uses_first_position() {
        let repo = RepoId::from_canonical("test://locate");
        let id = NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::QUEUE_PRODUCER,
            "queue_producer:orders",
        );
        let pos = |file: &str, line: u32| Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":{line},"end_line":{line}}}"#
            )),
        };
        let mut nav = CodeNav::default();
        nav.record(
            id,
            "orders",
            "queue_producer:orders",
            node_kind::QUEUE_PRODUCER,
            None,
        );
        let g = RepoGraph {
            repo,
            nodes: vec![Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: vec![pos("a.go", 4), pos("b.go", 11)],
            }],
            edges: vec![],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let at = locate_node(&MergedGraph::new(vec![g]), id);
        assert_eq!(at.id, id.0);
        assert_eq!(at.name, "orders");
        assert_eq!(at.qname, "queue_producer:orders");
        assert_eq!(at.kind, "QUEUE_PRODUCER");
        assert_eq!(at.file.as_deref(), Some("a.go"));
        // Stored row 4 (0-based) is reported as line 5 (1-based, LD.1).
        assert_eq!(at.line, Some(5));
    }

    /// A3.6 tier 3 — a bare-verb ROUTE borrows the POSITION of the handler it
    /// is HANDLED_BY. The first handler (edge order) that carries a file
    /// wins, a handler with no POSITION is skipped, and the handler's own
    /// POSITION keeps A2.8's first-wins. A route with no handler, and a
    /// non-HTTP node carrying an HTTP-shaped cell, stay unlocated.
    #[test]
    fn bare_verb_route_is_located_by_its_first_positioned_handler() {
        use glia_code_domain::edge_category;
        use glia_core::Edge;
        let repo = RepoId::from_canonical("test://locate-route");
        let id = |kind, q: &str| NodeId::from_parts(GRAPH_TYPE, repo, kind, q);
        let pos = |file: &str, line: u32| Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":{line},"end_line":{line}}}"#
            )),
        };
        let verb = Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Text("GET".into()),
        };
        let hit = Cell {
            kind: cell_type::ENDPOINT_HIT,
            payload: CellPayload::Json(r#"{"file":"x.ts","line":3}"#.into()),
        };
        let route = id(node_kind::ROUTE, "GET /users");
        let orphan = id(node_kind::ROUTE, "ANY /posts");
        let bare = id(node_kind::FUNCTION, "app::no_span");
        let handler = id(node_kind::FUNCTION, "app::list_users");
        let func_with_hit = id(node_kind::FUNCTION, "app::odd");
        let node = |id, cells| Node { id, repo, confidence: Confidence::Strong, cells };
        let mut nav = CodeNav::default();
        for (nid, q, k) in [
            (route, "GET /users", node_kind::ROUTE),
            (orphan, "ANY /posts", node_kind::ROUTE),
            (bare, "app::no_span", node_kind::FUNCTION),
            (handler, "app::list_users", node_kind::FUNCTION),
            (func_with_hit, "app::odd", node_kind::FUNCTION),
        ] {
            nav.record(nid, q, q, k, None);
        }
        let handled_by = |to| Edge {
            from: route,
            to,
            category: edge_category::HANDLED_BY,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        };
        let g = RepoGraph {
            repo,
            nodes: vec![
                node(route, vec![verb.clone()]),
                node(orphan, vec![verb]),
                node(bare, vec![]),
                node(handler, vec![pos("app.py", 6), pos("other.py", 40)]),
                node(func_with_hit, vec![hit]),
            ],
            edges: vec![handled_by(bare), handled_by(handler)],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let m = MergedGraph::new(vec![g]);
        let at = |nid| {
            let at = locate_node(&m, nid);
            (at.file, at.line)
        };
        // The handler's stored row 6 is line 7 (LD.1).
        assert_eq!(at(route), (Some("app.py".to_string()), Some(7)));
        assert_eq!(at(orphan), (None, None), "no handler, no span");
        assert_eq!(at(func_with_hit), (None, None), "tiers 2-3 are HTTP-only");
    }

    /// LD.1 parity: the Locator index keeps HEAD's scan semantics. The FIRST
    /// graph (Vec order) whose nav names an id owns it, even when a later graph
    /// carries a POSITION for the same id; a nav entry with no node in that
    /// graph keeps its name and qname and is unlocated; an id no graph names is
    /// `(unknown:<id>)`.
    #[test]
    fn locator_keeps_the_first_graph_rule() {
        use super::Locator;
        let repo = RepoId::from_canonical("test://locate-parity");
        let id = |q: &str| NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, q);
        let pos = |file: &str, line: u32| Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":{line},"end_line":{line}}}"#
            )),
        };
        let (shared, nav_only) = (id("m::shared"), id("m::nav_only"));
        let graph = |nodes: Vec<Node>, named: &[NodeId]| {
            let mut nav = CodeNav::default();
            for nid in named {
                let q = if *nid == shared { "m::shared" } else { "m::nav_only" };
                nav.record(*nid, q, q, node_kind::FUNCTION, None);
            }
            RepoGraph {
                repo,
                nodes,
                edges: vec![],
                nav,
                symbols: SymbolTable::default(),
                unresolved_calls: vec![],
                unresolved_refs: vec![],
                properties: Default::default(),
            }
        };
        let node = |nid, cells| Node { id: nid, repo, confidence: Confidence::Strong, cells };
        let m = MergedGraph::new(vec![
            graph(vec![node(shared, vec![pos("a.py", 2)])], &[shared, nav_only]),
            graph(
                vec![node(shared, vec![pos("b.py", 9)]), node(nav_only, vec![pos("c.py", 5)])],
                &[shared, nav_only],
            ),
        ]);
        let loc = Locator::new(&m);
        let at = loc.locate(shared);
        assert_eq!((at.file.as_deref(), at.line), (Some("a.py"), Some(3)), "first graph wins");
        let at = loc.locate(nav_only);
        assert_eq!(at.qname, "m::nav_only");
        assert_eq!((at.file, at.line), (None, None), "nav-only in the owning graph: unlocated");
        let ghost = id("m::ghost");
        let at = loc.locate(ghost);
        assert_eq!(at.qname, format!("(unknown:{})", ghost.0));
        assert_eq!((at.kind, at.name.as_str(), at.line), ("UNKNOWN", "", None));
        assert_eq!(loc.file_of(shared).as_deref(), Some("a.py"), "file_of agrees with locate");
        assert_eq!(locate_node(&m, shared), loc.locate(shared), "the one-off wrapper agrees");
    }
}

#[cfg(test)]
mod scope_tests {
    use super::in_scope;

    #[test]
    fn in_scope_matches_on_segment_boundaries_only() {
        assert!(in_scope("services/api/handler.py", "services/api"));
        assert!(in_scope("./services/api/handler.py", "/services/api/"));
        assert!(in_scope("services/api", "services/api"));
        // The boundary rule: a prefix that stops mid-segment must not match.
        assert!(!in_scope("services/api/handler.py", "services/ap"));
        assert!(!in_scope("services-api/handler.py", "services"));
        assert!(!in_scope("web/client.py", "services/api"));
        // An empty scope is "everything", not "nothing".
        assert!(in_scope("web/client.py", ""));
        assert!(in_scope("web/client.py", "/"));
        // So is `.` — the root project's path (A8.6).
        assert!(in_scope("web/client.py", "."));
        assert!(in_scope("main.go", "./"));
    }
}

#[cfg(test)]
mod project_scope_tests {
    use super::{project_roots, resolve_scope};
    use glia_code_domain::project_roots::ProjectRoot;
    use glia_core::{CellPayload, RepoId};
    use glia_graph::MergedGraph;

    fn graph(roots: &[ProjectRoot]) -> MergedGraph {
        MergedGraph::new(vec![crate::walk::build_project_graph(roots, RepoId(1))])
    }

    /// Two Maven modules sharing an artifactId: the smallest path wins,
    /// deterministically, whatever order the anchors were emitted in.
    #[test]
    fn ambiguous_label_resolves_to_the_smallest_path() {
        let label = || Some("billing".to_string());
        let m = graph(&[
            ProjectRoot::new("svc/zeta".into(), "maven", "pom.xml", label()),
            ProjectRoot::new("svc/alpha".into(), "maven", "pom.xml", label()),
        ]);
        assert_eq!(resolve_scope(&m, "billing"), "svc/alpha");
        // Both paths still resolve to themselves.
        assert_eq!(resolve_scope(&m, "svc/zeta"), "svc/zeta");
        assert_eq!(resolve_scope(&m, "svc/alpha"), "svc/alpha");
    }

    /// A PROJECT whose ORIGIN will not parse is skipped, not fatal, and the
    /// root's `""` walker path surfaces as `.`.
    #[test]
    fn unparseable_origin_is_skipped_and_root_is_dot() {
        let mut m = graph(&[
            ProjectRoot::new(String::new(), "npm", "package.json", Some("mono".into())),
            ProjectRoot::new("apps/web".into(), "npm", "package.json", Some("web".into())),
        ]);
        let broken = m.graphs[0]
            .nodes
            .iter_mut()
            .find(|n| {
                matches!(&n.cells[0].payload, CellPayload::Json(s) if s.contains("apps/web"))
            })
            .expect("apps/web anchor");
        broken.cells[0].payload = CellPayload::Json("{not json".into());
        let roots = project_roots(&m);
        assert_eq!(roots.len(), 1, "the broken anchor is skipped");
        assert_eq!((roots[0].path.as_str(), roots[0].label.as_str()), (".", "mono"));
        assert_eq!(roots[0].qname, "project:.");
        assert_eq!(resolve_scope(&m, "mono"), ".");
        assert_eq!(resolve_scope(&m, "web"), "web", "a skipped label passes through");
    }
}

#[cfg(test)]
mod contracts_tests {
    use super::{MessageContractSide, contract_side, contract_verdict, message_contracts};
    use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
    use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
    use glia_graph::{MergedGraph, RepoGraph, SymbolTable};

    fn msg(ty: &str, window: &str) -> Cell {
        Cell {
            kind: cell_type::MESSAGE_TYPE,
            payload: CellPayload::Json(format!(
                r#"{{"type":"{ty}","raw":"pb.{ty}","form":"generic","window":"{window}"}}"#
            )),
        }
    }

    /// One queue node per `(kind, topic, cells)`, all in one repo, plus the
    /// given producer→consumer QUEUE_FLOWS cross edges (by index).
    fn graph(nodes: Vec<(bool, &str, Vec<Cell>)>, flows: &[(usize, usize)]) -> (MergedGraph, Vec<NodeId>) {
        let repo = RepoId::from_canonical("test://contracts");
        let mut nav = CodeNav::default();
        let mut out = Vec::new();
        let mut ids = Vec::new();
        for (producer, topic, cells) in nodes {
            let (kind, prefix) = if producer {
                (node_kind::QUEUE_PRODUCER, "queue_producer:")
            } else {
                (node_kind::QUEUE_CONSUMER, "queue_consumer:")
            };
            let qname = format!("{prefix}{topic}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
            nav.record(id, topic, &qname, kind, None);
            out.push(Node { id, repo, confidence: Confidence::Medium, cells });
            ids.push(id);
        }
        let g = RepoGraph {
            repo,
            nodes: out,
            edges: vec![],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let mut merged = MergedGraph::new(vec![g]);
        for (p, c) in flows {
            merged.cross_edges.push(Edge {
                from: ids[*p],
                to: ids[*c],
                category: edge_category::QUEUE_FLOWS,
                confidence: Confidence::Medium,
                cells: Vec::new(),
            });
        }
        (merged, ids)
    }

    fn side_of(merged: &MergedGraph, id: NodeId) -> MessageContractSide {
        let nodes = super::collect_queue_nodes(merged);
        contract_side(&super::Locator::new(merged), id.0, &nodes[&id.0])
    }

    #[test]
    fn whole_file_type_makes_the_verdict_weak() {
        let (m, ids) = graph(
            vec![
                (true, "orders", vec![msg("OrderCreated", "file")]),
                (false, "orders", vec![msg("OrderCreated", "near")]),
            ],
            &[(0, 1)],
        );
        let (p, c) = (side_of(&m, ids[0]), side_of(&m, ids[1]));
        let (status, confidence, note) = contract_verdict(&p, &c);
        assert_eq!((status, confidence), ("match", "weak"));
        assert!(note.is_some_and(|n| n.contains("whole file")));
    }

    #[test]
    fn near_type_beats_an_earlier_whole_file_and_primitive_read() {
        let (m, ids) = graph(
            vec![(
                true,
                "orders",
                vec![msg("string", "near"), msg("Legacy", "file"), msg("OrderCreated", "near")],
            )],
            &[],
        );
        let s = side_of(&m, ids[0]);
        assert_eq!(s.message_type.as_deref(), Some("OrderCreated"));
        assert_eq!(s.types_seen, vec!["string", "Legacy", "OrderCreated"]);
        // `Legacy` is a whole-file guess and `string` a primitive: neither
        // contradicts the near schema read.
        assert!(!s.conflicting);
    }

    #[test]
    fn two_near_schema_types_are_a_conflict_not_a_verdict() {
        let (m, _) = graph(
            vec![
                (true, "orders", vec![msg("OrderCreated", "near"), msg("OrderPlaced", "near")]),
                (false, "orders", vec![msg("OrderCreated", "near")]),
            ],
            &[(0, 1)],
        );
        let rows = message_contracts(&m);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].producer.as_ref().unwrap().conflicting);
        assert_eq!(rows[0].status, "unknown");
        assert!(rows[0].note.is_some_and(|n| n.contains("conflicting")));
    }

    #[test]
    fn malformed_payload_is_no_type_and_no_panic() {
        let bad = Cell {
            kind: cell_type::MESSAGE_TYPE,
            payload: CellPayload::Json("{not json".into()),
        };
        let untyped = Cell {
            kind: cell_type::MESSAGE_TYPE,
            payload: CellPayload::Json(r#"{"raw":"x"}"#.into()),
        };
        let (m, ids) = graph(
            vec![(true, "orders", vec![bad, untyped]), (false, "orders", vec![])],
            &[(0, 1)],
        );
        let rows = message_contracts(&m);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].producer.as_ref().unwrap().message_type, None);
        assert_eq!(rows[0].status, "unknown");
        assert_eq!(rows[0].note, Some(super::NOTE_NO_TYPES));
        assert_eq!(side_of(&m, ids[0]).types_seen, Vec::<String>::new());
    }

    /// Same topic, no QUEUE_FLOWS edge = the resolver refused the pair (the
    /// family gate). The report must say so, not claim nobody is listening,
    /// and must not pair them itself.
    #[test]
    fn unpaired_same_topic_is_named_as_a_family_split() {
        let (m, _) = graph(
            vec![
                (true, "jobs", vec![msg("Job", "near")]),
                (false, "jobs", vec![msg("Job", "near")]),
            ],
            &[],
        );
        let rows = message_contracts(&m);
        assert_eq!(rows.len(), 2, "two one-sided rows, never a pair the resolver refused");
        assert!(rows.iter().all(|r| r.status == "unknown"));
        assert!(rows.iter().all(|r| r.note == Some(super::NOTE_FAMILY_SPLIT)));
    }

    /// A wildcard subscriber the resolver matched keeps its own pattern topic;
    /// the row is keyed by the producer's concrete topic.
    #[test]
    fn pattern_pair_is_keyed_by_the_producer_topic() {
        let (m, _) = graph(
            vec![
                (true, "orders.created", vec![msg("OrderCreated", "near")]),
                (false, "orders.*", vec![msg("OrderCreated", "near")]),
            ],
            &[(0, 1)],
        );
        let rows = message_contracts(&m);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].topic, "orders.created");
        assert!(rows[0].pattern);
        assert_eq!(rows[0].consumer.as_ref().unwrap().topic, "orders.*");
        assert_eq!((rows[0].status, rows[0].confidence), ("match", "strong"));
    }

    /// Belt and braces: even if an edge between two tags existed, the report
    /// refuses to pair them.
    #[test]
    fn tag_edge_is_never_a_pair() {
        let (m, _) = graph(
            vec![
                (true, "unresolved:kafka", vec![msg("A", "near")]),
                (false, "unresolved:kafka", vec![msg("A", "near")]),
            ],
            &[(0, 1)],
        );
        let rows = message_contracts(&m);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.topic_is_tag && r.note == Some(super::NOTE_TAG)));
    }
}

#[cfg(test)]
mod role_live_tests {
    use super::entrypoint_reachable;
    use crate::profile::CODE_PROFILE;
    use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
    use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
    use glia_graph::{MergedGraph, RepoGraph, SymbolTable};

    /// `app::Page` -INJECTS-> `app::Api`, both CLASS; `Page` carries a ROLE
    /// cell with `roles` when given. Returns the graph and `(page, api)`.
    fn page_injects_api(roles: Option<&str>) -> (MergedGraph, NodeId, NodeId) {
        let repo = RepoId::from_canonical("test://role-live");
        let id = |q: &str| NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, q);
        let (page, api) = (id("app::Page"), id("app::Api"));
        let mut nav = CodeNav::default();
        nav.record(page, "Page", "app::Page", node_kind::CLASS, None);
        nav.record(api, "Api", "app::Api", node_kind::CLASS, None);
        let cells = roles
            .map(|r| Cell {
                kind: cell_type::ROLE,
                payload: CellPayload::Json(format!(r#"{{"roles":[{r}]}}"#)),
            })
            .into_iter()
            .collect();
        let node = |nid, cells| Node { id: nid, repo, confidence: Confidence::Strong, cells };
        let g = RepoGraph {
            repo,
            nodes: vec![node(page, cells), node(api, vec![])],
            edges: vec![Edge {
                from: page,
                to: api,
                category: edge_category::INJECTS,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            }],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        (MergedGraph::new(vec![g]), page, api)
    }

    /// LB.3b: a CLASS folded from an `@Component` seeds liveness through its
    /// ROLE cell; without the cell, or with a non-entry role, it does not.
    #[test]
    fn component_role_is_an_entrypoint() {
        let (m, page, api) = page_injects_api(Some(r#""COMPONENT""#));
        let live = entrypoint_reachable(&m);
        assert!(live.contains(&page), "the COMPONENT-role class is an entry");
        assert!(live.contains(&api), "what it injects is live");

        let (m, page, api) = page_injects_api(None);
        let live = entrypoint_reachable(&m);
        assert!(!live.contains(&page) && !live.contains(&api), "no role, no entry");

        let (m, _, api) = page_injects_api(Some(r#""SERVICE""#));
        assert!(!entrypoint_reachable(&m).contains(&api), "SERVICE is not an entry role");

        // The kind / name arms are unchanged and need no roles.
        let entry = &CODE_PROFILE.tables.entry;
        let class = Some(node_kind::CLASS);
        assert!(entry.is_entry(class, "Page", &[node_kind::COMPONENT]));
        assert!(!entry.is_entry(class, "Page", &[node_kind::SERVICE, node_kind::HOOK]));
        assert!(entry.is_entry(Some(node_kind::COMPONENT), "Card", &[]));
        assert!(entry.is_entry(Some(node_kind::FUNCTION), "main", &[]));
        assert!(!entry.is_entry(class, "Page", &[]));
    }
}

#[cfg(test)]
mod live_tests {
    //! A7.8 — liveness past the entry set: a live METHOD makes its owning
    //! type live, a live interface makes its implementers live, and neither
    //! step turns into "everything is live".

    use super::{entrypoint_reachable, live_walk};
    use crate::profile::CODE_PROFILE;
    use glia_code_domain::{
        CallQualifier, CodeNav, FileParse, GRAPH_TYPE, UnresolvedRef, edge_category, node_kind,
    };
    use glia_core::{Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId};
    use glia_graph::roles::roles_in;
    use glia_graph::{MergedGraph, RepoGraph, SymbolTable, build_typescript};

    fn repo() -> RepoId {
        RepoId::from_canonical("test://live")
    }

    /// Hand-built graph: nodes recorded with kind, qname and parent; the simple
    /// name is the last `::` segment of the qname.
    #[derive(Default)]
    struct G {
        nodes: Vec<Node>,
        edges: Vec<Edge>,
        nav: CodeNav,
    }

    impl G {
        fn node(&mut self, kind: NodeKindId, qname: &str, parent: Option<NodeId>) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            self.nav.record(id, name, qname, kind, parent);
            self.nodes.push(Node {
                id,
                repo: repo(),
                confidence: Confidence::Strong,
                cells: vec![],
            });
            id
        }

        fn edge(&mut self, from: NodeId, to: NodeId, category: EdgeCategoryId) {
            self.edges.push(Edge {
                from,
                to,
                category,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
        }

        fn merged(self) -> MergedGraph {
            MergedGraph::new(vec![RepoGraph {
                repo: repo(),
                nodes: self.nodes,
                edges: self.edges,
                nav: self.nav,
                symbols: SymbolTable::default(),
                unresolved_calls: vec![],
                unresolved_refs: vec![],
                properties: Default::default(),
            }])
        }
    }

    /// LB.3 folds the `@Component` overlay into its CLASS; the surviving CLASS
    /// is an entrypoint through its ROLE cell and what it injects is live.
    /// Built through `build_typescript`, so the fold itself runs.
    #[test]
    fn merged_component_seeds_its_injects() {
        let id = |k, q: &str| NodeId::from_parts(GRAPH_TYPE, repo(), k, q);
        let node = |nid| Node {
            id: nid,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![],
        };
        let parse = |nodes, edges, refs, nav| FileParse {
            nodes,
            edges,
            imports: vec![],
            calls: vec![],
            refs,
            nav,
            properties: Default::default(),
        };
        // m.ts: `@Injectable() class UserService {}` (no entry role).
        let (m, svc_class, svc_overlay) = (
            id(node_kind::MODULE, "m"),
            id(node_kind::CLASS, "m::UserService"),
            id(node_kind::SERVICE, "m::UserService"),
        );
        let mut nav = CodeNav::default();
        nav.record(m, "m", "m", node_kind::MODULE, None);
        nav.record(
            svc_class,
            "UserService",
            "m::UserService",
            node_kind::CLASS,
            Some(m),
        );
        nav.record(
            svc_overlay,
            "UserService",
            "m::UserService",
            node_kind::SERVICE,
            Some(m),
        );
        let service = parse(
            vec![node(m), node(svc_class), node(svc_overlay)],
            vec![Edge {
                from: m,
                to: svc_class,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            }],
            vec![],
            nav,
        );
        // n.ts: `@Component() class UsersComponent { constructor(s: UserService) }`.
        let (n, comp_class, comp_overlay) = (
            id(node_kind::MODULE, "n"),
            id(node_kind::CLASS, "n::UsersComponent"),
            id(node_kind::COMPONENT, "n::UsersComponent"),
        );
        let mut nav = CodeNav::default();
        nav.record(n, "n", "n", node_kind::MODULE, None);
        nav.record(
            comp_class,
            "UsersComponent",
            "n::UsersComponent",
            node_kind::CLASS,
            Some(n),
        );
        nav.record(
            comp_overlay,
            "UsersComponent",
            "n::UsersComponent",
            node_kind::COMPONENT,
            Some(n),
        );
        let component = parse(
            vec![node(n), node(comp_class), node(comp_overlay)],
            vec![Edge {
                from: n,
                to: comp_class,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            }],
            vec![UnresolvedRef {
                from: comp_class,
                from_module: n,
                qualifier: CallQualifier::Bare("UserService".into()),
                category: edge_category::INJECTS,
                line: 0,
            }],
            nav,
        );
        let g = build_typescript(repo(), vec![service, component], |_, _| None).expect("build");

        // The fold left one node per declaration, the CLASS, with the role.
        assert!(
            g.nodes
                .iter()
                .all(|x| x.id != comp_overlay && x.id != svc_overlay)
        );
        let comp = g
            .nodes
            .iter()
            .find(|x| x.id == comp_class)
            .expect("the CLASS survives");
        let roles = roles_in(Some(node_kind::CLASS), &comp.cells);
        assert!(CODE_PROFILE.tables.entry.is_entry(
            Some(node_kind::CLASS),
            "UsersComponent",
            &roles
        ));
        assert!(g.edges.iter().any(|e| e.from == comp_class
            && e.to == svc_class
            && e.category == edge_category::INJECTS));

        let w = live_walk(&MergedGraph::new(vec![g]));
        assert!(
            w.live.contains(&comp_class),
            "the folded component is an entry"
        );
        assert!(w.live.contains(&svc_class), "what it injects is live");
        assert_eq!(
            (w.by_kind, w.by_role, w.owners, w.implementers),
            (0, 1, 0, 0)
        );
    }

    /// ROUTE -HANDLED_BY-> METHOD, whose CLASS -INJECTS-> a service: the
    /// method makes the class live, and the class carries the injection.
    #[test]
    fn method_liveness_propagates_to_owning_class() {
        let mut g = G::default();
        let module = g.node(node_kind::MODULE, "app", None);
        let ctrl = g.node(node_kind::CLASS, "app::ReportsController", Some(module));
        let get = g.node(node_kind::METHOD, "app::ReportsController::Get", Some(ctrl));
        let route = g.node(node_kind::ROUTE, "GET /reports", None);
        let svc = g.node(node_kind::CLASS, "app::ReportService", Some(module));
        let point = g.node(node_kind::STRUCT, "app::Point", Some(module));
        let norm = g.node(node_kind::METHOD, "app::Point::norm", Some(point));
        g.edge(module, ctrl, edge_category::DEFINES);
        g.edge(ctrl, get, edge_category::DEFINES);
        g.edge(route, get, edge_category::HANDLED_BY);
        g.edge(ctrl, svc, edge_category::INJECTS);
        g.edge(get, norm, edge_category::CALLS);

        let w = live_walk(&g.merged());
        for (id, what) in [
            (ctrl, "the controller"),
            (svc, "its injected service"),
            (point, "a STRUCT"),
        ] {
            assert!(w.live.contains(&id), "{what} is live");
        }
        assert!(!w.live.contains(&module), "no climb to MODULE");
        assert_eq!((w.by_kind, w.owners, w.implementers), (1, 2, 0));
        assert_eq!(w.total, 7);
    }

    /// The guard against "everything is live": a CLASS with no live method and
    /// no entry role stays dead, and a live method's class does not make its
    /// sibling methods live (upward only).
    #[test]
    fn unrelated_class_stays_dead() {
        let mut g = G::default();
        let ctrl = g.node(node_kind::CLASS, "app::Ctrl", None);
        let get = g.node(node_kind::METHOD, "app::Ctrl::get", Some(ctrl));
        let helper = g.node(node_kind::METHOD, "app::Ctrl::helper", Some(ctrl));
        let route = g.node(node_kind::ROUTE, "GET /x", None);
        let other = g.node(node_kind::CLASS, "app::Unused", None);
        let run = g.node(node_kind::METHOD, "app::Unused::run", Some(other));
        let dep = g.node(node_kind::CLASS, "app::Dep", None);
        g.edge(ctrl, get, edge_category::DEFINES);
        g.edge(ctrl, helper, edge_category::DEFINES);
        g.edge(route, get, edge_category::HANDLED_BY);
        g.edge(other, run, edge_category::DEFINES);
        g.edge(other, dep, edge_category::INJECTS);

        let live = entrypoint_reachable(&g.merged());
        assert!(live.contains(&ctrl));
        for (id, what) in [
            (helper, "a sibling of the live method"),
            (other, "an unrelated class"),
            (run, "its method"),
            (dep, "what it injects"),
        ] {
            assert!(!live.contains(&id), "{what} stays dead");
        }
    }

    /// A class that is live as a route handler DEFINES a method nothing calls:
    /// DEFINES still does not carry, so the method stays dead.
    #[test]
    fn defines_still_does_not_carry() {
        let mut g = G::default();
        let ctrl = g.node(node_kind::CLASS, "app::Ctrl", None);
        let unused = g.node(node_kind::METHOD, "app::Ctrl::unused", Some(ctrl));
        let route = g.node(node_kind::ROUTE, "ANY /api", None);
        g.edge(route, ctrl, edge_category::HANDLED_BY);
        g.edge(ctrl, unused, edge_category::DEFINES);

        let w = live_walk(&g.merged());
        assert!(w.live.contains(&ctrl));
        assert!(!w.live.contains(&unused), "DEFINES is not a carry edge");
        assert_eq!((w.owners, w.implementers), (0, 0));
    }

    /// Interface-typed DI: the controller injects `IRepo` and calls
    /// `IRepo::find`; the implementing class and its `find` are live, and the
    /// implementation's own callees with them. INHERITS_FROM stays forward-only
    /// and an implementer of a dead interface stays dead.
    #[test]
    fn implementer_of_live_interface_is_live() {
        let mut g = G::default();
        let route = g.node(node_kind::ROUTE, "GET /items", None);
        let ctrl = g.node(node_kind::CLASS, "app::Ctrl", None);
        let get = g.node(node_kind::METHOD, "app::Ctrl::get", Some(ctrl));
        let irepo = g.node(node_kind::INTERFACE, "app::IRepo", None);
        let ifind = g.node(node_kind::METHOD, "app::IRepo::find", Some(irepo));
        let repo_c = g.node(node_kind::CLASS, "app::Repo", None);
        let find = g.node(node_kind::METHOD, "app::Repo::find", Some(repo_c));
        let load = g.node(node_kind::METHOD, "app::Repo::load", Some(repo_c));
        let cached = g.node(node_kind::CLASS, "app::CachedRepo", None);
        let idead = g.node(node_kind::INTERFACE, "app::IDead", None);
        let dead_impl = g.node(node_kind::CLASS, "app::DeadImpl", None);
        g.edge(route, get, edge_category::HANDLED_BY);
        g.edge(ctrl, irepo, edge_category::INJECTS);
        g.edge(get, ifind, edge_category::CALLS);
        g.edge(repo_c, irepo, edge_category::IMPLEMENTS);
        g.edge(find, ifind, edge_category::IMPLEMENTS);
        g.edge(find, load, edge_category::CALLS);
        g.edge(cached, repo_c, edge_category::INHERITS_FROM);
        g.edge(dead_impl, idead, edge_category::IMPLEMENTS);

        let w = live_walk(&g.merged());
        for (id, what) in [
            (repo_c, "the implementing class"),
            (find, "the implementing method"),
            (load, "the implementation's callee"),
        ] {
            assert!(w.live.contains(&id), "{what} is live");
        }
        assert!(!w.live.contains(&cached), "INHERITS_FROM is forward-only");
        assert!(!w.live.contains(&idead) && !w.live.contains(&dead_impl));
        // Implementers: Repo (via IRepo) and Repo::find (via IRepo::find).
        // Owners: Ctrl (via get); IRepo and Repo were already live.
        assert_eq!((w.owners, w.implementers), (1, 2));
    }

    /// DT.5d: a live owner makes its framework- and runtime-invoked members
    /// live (hooks, constructors), never its other methods; a member a carry
    /// edge reached first counts as a carry hit; a dead owner's members stay
    /// dead; no edge is added. Three classes: an entry component, the service
    /// it injects and a service nothing injects.
    #[test]
    fn invoked_members_follow_a_live_owner() {
        use glia_code_domain::cell_type;
        use glia_core::{Cell, CellPayload};
        let mut g = G::default();
        let module = g.node(node_kind::MODULE, "app", None);
        let page = g.node(node_kind::CLASS, "app::Page", Some(module));
        let page_init = g.node(node_kind::METHOD, "app::Page::ngOnInit", Some(page));
        let page_ctor = g.node(node_kind::METHOD, "app::Page::constructor", Some(page));
        let page_later = g.node(node_kind::METHOD, "app::Page::ngOnInitLater", Some(page));
        let svc = g.node(node_kind::CLASS, "app::Svc", Some(module));
        let svc_ctor = g.node(node_kind::METHOD, "app::Svc::constructor", Some(svc));
        let svc_destroy = g.node(node_kind::METHOD, "app::Svc::ngOnDestroy", Some(svc));
        let svc_list = g.node(node_kind::METHOD, "app::Svc::list", Some(svc));
        let idle = g.node(node_kind::CLASS, "app::Idle", Some(module));
        let idle_ctor = g.node(node_kind::METHOD, "app::Idle::constructor", Some(idle));
        let idle_destroy = g.node(node_kind::METHOD, "app::Idle::ngOnDestroy", Some(idle));
        g.edge(page, svc, edge_category::INJECTS);
        g.edge(page_init, svc_list, edge_category::CALLS);
        // ngOnInit's call reaches the hook before Svc is popped.
        g.edge(page_init, svc_destroy, edge_category::CALLS);
        let role = |r: &str| Cell {
            kind: cell_type::ROLE,
            payload: CellPayload::Json(format!("{{\"roles\":[\"{r}\"]}}")),
        };
        let at = |f: &str| Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!("{{\"file\":\"{f}\",\"start_line\":1}}")),
        };
        for n in &mut g.nodes {
            let file = if n.id == page || [page_init, page_ctor, page_later].contains(&n.id) {
                "src/app/page.ts"
            } else if n.id == svc || [svc_ctor, svc_destroy, svc_list].contains(&n.id) {
                "src/app/svc.ts"
            } else {
                "src/app/idle.ts"
            };
            n.cells.push(at(file));
            if n.id == page {
                n.cells.push(role("COMPONENT"));
            } else if n.id == svc || n.id == idle {
                n.cells.push(role("SERVICE"));
            }
        }
        let edges_before = g.edges.len();
        let merged = g.merged();

        assert_eq!(
            super::invoked_members(&merged),
            [
                (page_init, page, "angular"),
                (page_ctor, page, "constructor"),
                (svc_ctor, svc, "constructor"),
                (svc_destroy, svc, "angular"),
                (idle_ctor, idle, "constructor"),
                (idle_destroy, idle, "angular"),
            ],
            "node order, the look-alike and the plain method left out"
        );

        let w = live_walk(&merged);
        for (id, what) in [
            (page_init, "the entry component's hook"),
            (page_ctor, "its constructor"),
            (svc_ctor, "the injected service's constructor"),
            (svc_destroy, "its hook"),
            (svc_list, "what the hook calls"),
        ] {
            assert!(w.live.contains(&id), "{what} is live");
        }
        for (id, what) in [
            (page_later, "a hook look-alike"),
            (idle, "a service nothing injects"),
            (idle_ctor, "its constructor"),
            (idle_destroy, "its hook"),
        ] {
            assert!(!w.live.contains(&id), "{what} stays dead");
        }
        // Invoked: Page::ngOnInit, Page::constructor, Svc::constructor;
        // Svc::ngOnDestroy was reached over ngOnInit's CALLS first.
        assert_eq!((w.by_role, w.owners, w.implementers, w.invoked), (1, 0, 0, 3));
        assert_eq!(entrypoint_reachable(&merged), w.live);
        assert_eq!(merged.all_edges().count(), edges_before, "the hop is never stored");
    }

    /// LF.3b: a FUNCTION nothing calls is an entry once it carries an
    /// ENTRYPOINT cell, and what it calls is live with it. A node the table
    /// already makes an entry counts by kind, never twice.
    #[test]
    fn declared_entrypoint_cell_seeds_liveness() {
        use glia_code_domain::cell_type;
        use glia_core::{Cell, CellPayload};
        let mut g = G::default();
        let job = g.node(node_kind::FUNCTION, "app::jobs::nightly_rollup", None);
        let helper = g.node(node_kind::FUNCTION, "app::jobs::summarise", None);
        let undeclared = g.node(node_kind::FUNCTION, "app::jobs::weekly", None);
        let main = g.node(node_kind::FUNCTION, "app::main", None);
        g.edge(job, helper, edge_category::CALLS);
        for n in g.nodes.iter_mut().filter(|n| n.id == job || n.id == main) {
            n.cells.push(Cell {
                kind: cell_type::ENTRYPOINT,
                payload: CellPayload::Json(r#"{"pattern":"app::*","source":"config"}"#.into()),
            });
        }

        let w = live_walk(&g.merged());
        assert!(w.live.contains(&job), "the declared job is an entry");
        assert!(w.live.contains(&helper), "what it calls is live");
        assert!(!w.live.contains(&undeclared), "an undeclared job stays dead");
        assert!(w.live.contains(&main));
        assert_eq!((w.by_kind, w.by_role, w.by_decl), (1, 0, 1));
    }
}

#[cfg(test)]
mod blast_seeded_tests {
    //! LD.5: the id-seeded entry LE.2's diff impact calls. Integration tests
    //! (engine/tests/blast_multi_seed.rs) cannot reach a `pub(crate)` fn.

    use super::{BlastOptions, blast_radius, blast_radius_seeded};
    use crate::build::generate_many;

    const APP_PY: &str = "def audit(order):\n    return order\n\n\ndef save(order):\n    return audit(order)\n\n\ndef create_order():\n    order = {}\n    save(order)\n    return order\n";

    #[test]
    fn seeded_by_id_is_the_query_answer_under_its_display_string() {
        let dir = tempfile::tempdir().expect("tempdir");
        let api = dir.path().join("api");
        std::fs::create_dir_all(&api).expect("api dir");
        std::fs::write(api.join("app.py"), APP_PY).expect("write app.py");
        let m = generate_many(&[api.display().to_string()]).expect("builds").merged;
        let save = m.node_id_by_qname("app::save").expect("app::save");
        let opts = BlastOptions::default();

        let by_query = blast_radius(&m, &["app::save"], &opts);
        let by_id = blast_radius_seeded(&m, &[("x".to_string(), save)], &opts);
        assert!(!by_id.results.is_empty());
        let json = |r: &super::BlastRadius| serde_json::to_value(&r.results).expect("rows");
        assert_eq!(json(&by_id), json(&by_query), "same rows, order, scores and seed");
        assert_eq!(by_id.seeds[0].query, "x", "the display string is kept");
        assert_eq!(by_id.seeds[0].qname, "app::save");
        assert!(by_id.results.iter().all(|r| r.seed == "app::save"), "rows name the qname, not the display");
        assert!(by_id.unresolved.is_empty() && by_id.absence.is_none());

        // One id twice is one seed, the first display kept; no seeds is an
        // empty answer with an absence, never a walk.
        let twice = blast_radius_seeded(&m, &[("a".into(), save), ("b".into(), save)], &opts);
        assert_eq!(twice.seeds.len(), 1);
        assert_eq!(twice.seeds[0].query, "a");
        assert_eq!(json(&twice), json(&by_query));
        let none = blast_radius_seeded(&m, &[], &opts);
        assert!(none.results.is_empty() && none.absence.is_some());
    }
}
