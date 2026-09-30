//! Service-split suggestions by minimum cut (CD.2b, + CD.2c): the module or
//! community quotient of a scope bisected by Stoer-Wagner (the ratio-best
//! phase cut above a balance floor, recursive to N parts), cut edges located
//! at their evidence sites and each part diffed against `glia arch`; CD.2c
//! adds the anchored s-t cut between two seeds or scopes and the blockers
//! (shared-write data entities, cycles between parts). The cuts are
//! `glia_activation::algo::cut`. Public slot, reached by module path
//! (`glia_engine::splits::<item>`). Nothing is stored.
//!
//! THE NODES. Every node under [`SplitArgs::scope`] (a path or a project
//! label, resolved once; an unlocatable node is kept, as
//! `answers::node_in_scope` keeps it), read from the first graph listing it,
//! in the canonical order (qname, kind, file, repo label, id) that makes the
//! answer independent of the repo's identity key. Data nodes (the kinds of the
//! domain's `db` effect sink: tables, databases, caches, ...) stay out: they
//! are the shared resources a split has to settle, never code that moves. A
//! weighted edge is any edge between two kept nodes whose category weighs
//! more than 0 in the domain's `community_weights` (CD.1c); a node with no
//! weighted edge stays out.
//!
//! THE UNITS. What a team can move. `module` (the default): each node's unit
//! is the first MODULE up its graph's parent chain (the node itself when it is
//! one). A node with none (a ROUTE, an ENDPOINT, a queue topic) takes the unit
//! of its heaviest weighted neighbour that has one, ties to the lowest unit
//! index, in breadth-first layers from the moduled nodes, so a node two hops
//! out (an ENDPOINT calling a ROUTE) follows the neighbour the layer before
//! placed; a node no layer reaches stays out. Units are ordered by (module
//! qname, file, repo label, id). `community`: the node's CD.1d community
//! (`communities::partition_of`, seeded, gamma 1.0), numbered by first member
//! in canonical order and labelled by that member's qname. The unit graph is
//! the weighted quotient (`WeightedGraph::from_pairs` over the unit pairs).
//!
//! THE CUT. `algo::cut::stoer_wagner` on the unit graph. Raw global minimum
//! cuts peel a leaf, so among its phase cuts only those whose smaller side
//! holds at least [`SplitArgs::min_share`] of the part's NODES are kept, and
//! the least ratio weight / min(nodes A, nodes B) wins (exact `u128` cross
//! products; ties to the lower weight, then the earlier phase), then refined
//! by single-unit moves that strictly lower that ratio above the floor (a
//! phase cut is one merged super-vertex, and Stoer-Wagner merges a leaf that
//! hangs off the cluster its phases start from into the OTHER cluster, so the
//! best phase cut can carry it on the wrong side). When no phase clears the
//! floor the global minimum cut is taken, unrefined, and the answer is not
//! `balanced`. For more than two parts, the part with the most nodes (among
//! parts of two or more units; ties to the lower id) is bisected next on its
//! induced unit subgraph, until [`SplitArgs::parts`] parts or no part has two
//! units. Parts are then numbered by nodes descending, then smallest unit
//! label.
//!
//! THE ANSWER. `cut_weight` sums every weighted edge between two parts;
//! `global_min_weight` is the whole scope's Stoer-Wagner minimum, so a gap
//! between the two is the balance the floor bought. `cut_edges` lists those
//! edges, heaviest first (then category, from qname, to qname), each located
//! at its EVIDENCE site (0-based there, 1-based here: LD.1) with the
//! evidence's basis, else at its from node's declaration (basis `from_node`).
//! Per part: its units and nodes, its MODULE labels (the units themselves
//! under `module`; the modules its members sit in under `community`, so a
//! community part can name a module another part names too), a label (the
//! modules' longest common `::` prefix, else the first segment holding most
//! nodes), the `glia arch` service histogram of its located members, its
//! entrypoints and its [`TOP_MEMBERS`] members of highest weighted degree
//! inside the part. `arch` diffs each part against the services:
//! `aligned` (one service, found in no other part), `splits_service` (one
//! service another part shares: the cut runs inside an existing service),
//! `spans_services` (two or more), `unplaced` (no located member). Every part
//! is tier [`HEURISTIC`]: a suggested cut, never a verdict.
//!
//! ANCHORED (CD.2c). With [`SplitArgs::source`] or [`SplitArgs::sink`] set
//! the mode is `st`: "separate this from that at the least coupling", a
//! minimum s-t cut (`algo::cut::min_st_cut`, Dinic) on the same unit graph,
//! which Stoer-Wagner cannot answer. Each side is read as a path (or project
//! label, `answers::resolve_scope`) when a located member sits under it, and
//! is then every unit with such a member (`services/payments`, `orders`,
//! `orders/api.py`); otherwise as the node it names (`answers::resolve_seed`,
//! preferring one under the scope), whose unit is its own or, for a node
//! outside the quotient, its enclosing MODULE's. Both sides must be set, name
//! a unit and share none. The answer has two parts: part 0 is the source side
//! (the residual source-reachable units), part 1 the rest, whatever their
//! sizes; [`SplitArgs::parts`] is not read. `global_min_weight` is still the
//! scope's Stoer-Wagner minimum, and `balanced` says the smaller side holds
//! at least [`SplitArgs::min_share`] of the nodes. Everything else (cut
//! edges, parts, the arch diff) is the global mode's.
//!
//! THE BLOCKERS (CD.2c), computed in both modes over the final parts. What
//! stops a split once the cut is drawn:
//! - `shared_writes`: a data node (the `db` kinds above, which never join
//!   the quotient) that members of two or more parts access over
//!   ACCESSES_DATA, where at least two parts write it or may. Per part the
//!   LE.4a ACCESS_MODE cells of its accessing edges fold: read + write is
//!   `read_write`; an edge with no cell is `unknown`, which never reads as a
//!   read (read + unknown stays `unknown`; write + unknown is `write`, a
//!   write is known). A part counts as writing when its mode is not `read`.
//!   `writers` are the members whose access is not a known read, by part
//!   then qname, the first [`MAX_WRITERS`]. Tier [`DERIVED`] when no part's
//!   mode is `unknown`, else [`HEURISTIC`]. Rows sort by parts, then writers,
//!   descending, then entity qname. The SQL verb at the access site is all
//!   that is read: nothing here follows a value.
//! - `cycles`: the parts as a directed graph, one synthetic node per part
//!   and one edge per direction any carry edge (the domain's
//!   `carry_edges`) between two members of two parts runs, fed to
//!   `algo::cycles::strongly_connected`. Each non-trivial component is a
//!   [`PartCycle`]: its parts, and per direction inside it the heaviest such
//!   edge (community weight, then the cut-edge order), located as a cut edge
//!   is. Tier [`DERIVED`]: every witness is an edge the graph holds.
//!
//! EMPTY. An unknown quotient name, fewer than two units, or more than
//! [`MAX_UNITS`] (Stoer-Wagner is O(V E log V) and its phase sides hold
//! O(V^2) ids: CD.2a measured ~5 s and ~50 MB at 5,000 units, over a minute
//! and 800 MB at 20,000) is an absence `no_match` through `absence::empty`,
//! with no cut; so, in the `st` mode, is a side not set, a side that names no
//! unit, or two sides sharing a unit (the note names the side).
//!
//! fired_on marker, once per call:
//! `[splits] mode=<global|st> quotient=<module|community> units=<U> parts=<P> cut_weight=<W> global_min=<G> balanced=<true|false> cut_edges=<E> shared_writes=<S> part_cycles=<C> surface=<engine|cli|py>`
//! (`cut_edges` is `cut_edges_total`; `quotient=none` when the name was
//! refused).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_activation::algo::community::{CommunityOptions, Resolution, WeightedGraph};
use glia_activation::algo::cut::{min_st_cut, stoer_wagner};
use glia_activation::algo::cycles::strongly_connected;
use glia_activation::algo::{Adjacency, CategorySet, GraphSource};
use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, Confidence, Edge, NodeId, NodeKindId};
use glia_graph::MergedGraph;
use glia_graph::roles::roles_in;

use crate::absence::{self, Absence};
use crate::answers::{Located, Locator, in_scope, is_declared_entry, resolve_scope, resolve_seed};
use crate::arch::{default_keying, service_of};
use crate::communities::{self, partition_of};
use crate::profile::CODE_PROFILE;

/// [`SplitArgs::parts`] by default.
pub const DEFAULT_PARTS: usize = 2;
/// [`SplitArgs::parts`] at most; a larger value is read as this.
pub const MAX_PARTS: usize = 8;
/// [`SplitArgs::min_share`] by default.
pub const DEFAULT_MIN_SHARE: f64 = 0.1;
/// [`SplitArgs::seed`] by default: the communities default.
pub const DEFAULT_SEED: u64 = communities::DEFAULT_SEED;
/// [`SplitArgs::max_cut_edges`] by default.
pub const DEFAULT_MAX_CUT_EDGES: usize = 50;
/// Units a cut runs over at most (see the module doc).
pub const MAX_UNITS: usize = 5_000;
/// Module labels listed per part at most.
pub const MAX_MODULES: usize = 20;
/// Top members listed per part at most.
pub const TOP_MEMBERS: usize = 10;
/// [`SplitAnswer::tier`]; a [`SharedWrite`] with an `unknown` mode.
pub const HEURISTIC: &str = "heuristic";
/// A [`SharedWrite`] whose every mode is known; every [`PartCycle`].
pub const DERIVED: &str = "derived";
/// [`SharedWrite::writers`] listed at most.
pub const MAX_WRITERS: usize = 5;
/// [`SplitArgs::surface`] when the engine is called directly.
pub const SURFACE_ENGINE: &str = "engine";

const PRIMITIVE: &str = "splits";
const MODE_GLOBAL: &str = "global";
const MODE_ST: &str = "st";
/// A part's access mode when no accessing edge carries one it can fold.
const UNKNOWN: &str = "unknown";
/// Units a side's `share` absence note names at most.
const MAX_NOTED_UNITS: usize = 5;
const QUOTIENT_MODULE: &str = "module";
const QUOTIENT_COMMUNITY: &str = "community";
/// The effect-sink class whose kinds are data nodes.
const DATA_CLASS: &str = "db";
/// A parent chain longer than this is read as having no MODULE.
const MAX_PARENT_STEPS: usize = 64;
/// Gamma 1.0 in the thousandths `communities` holds it in.
const GAMMA_ONE: Resolution = Resolution {
    num: 1000,
    den: 1000,
};

/// What [`splits`] cuts and how. Start from `default()` and set fields.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct SplitArgs {
    /// Nodes under this path or project label only.
    pub scope: Option<String>,
    /// Parts wanted ([`DEFAULT_PARTS`]), read into `2..=`[`MAX_PARTS`].
    pub parts: usize,
    /// `module` (default; empty reads as it) or `community`.
    pub quotient: String,
    /// The smaller side's least share of a part's nodes ([`DEFAULT_MIN_SHARE`]),
    /// read into `0..=0.5`; not a finite number reads as the default.
    pub min_share: f64,
    /// Seeds the community quotient ([`DEFAULT_SEED`]).
    pub seed: u64,
    /// Cut edges listed ([`DEFAULT_MAX_CUT_EDGES`]); 0 lists every one.
    pub max_cut_edges: usize,
    /// Who asked, for the marker: [`SURFACE_ENGINE`], `cli` or `py` (empty
    /// reads as [`SURFACE_ENGINE`]).
    pub surface: &'static str,
    /// The s-t mode's source side: a path or project label, else a node's
    /// qname or name (see the module doc). Either of `source` / `sink` set
    /// selects the s-t mode, which needs both.
    pub source: Option<String>,
    /// The s-t mode's sink side, read as [`Self::source`] is.
    pub sink: Option<String>,
}

impl Default for SplitArgs {
    fn default() -> Self {
        SplitArgs {
            scope: None,
            parts: DEFAULT_PARTS,
            quotient: QUOTIENT_MODULE.to_string(),
            min_share: DEFAULT_MIN_SHARE,
            seed: DEFAULT_SEED,
            max_cut_edges: DEFAULT_MAX_CUT_EDGES,
            surface: SURFACE_ENGINE,
            source: None,
            sink: None,
        }
    }
}

/// One proposed part.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct SplitPart {
    /// 0-based: nodes descending, then smallest unit label.
    pub id: u32,
    pub nodes: usize,
    pub units: usize,
    /// The modules' common prefix (see the module doc).
    pub label: String,
    /// The first [`MAX_MODULES`] MODULE labels, sorted.
    pub modules: Vec<String>,
    /// `(glia arch service, located members)`, count descending, then name.
    pub services: Vec<(String, usize)>,
    /// Members that are entrypoints.
    pub entries: usize,
    /// The [`TOP_MEMBERS`] members of highest weighted degree inside the part.
    pub top_members: Vec<Located>,
}

/// One edge between two parts.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct CutEdge {
    pub from_qname: String,
    pub to_qname: String,
    pub category: &'static str,
    pub from_part: u32,
    pub to_part: u32,
    /// The category's community weight.
    pub weight: u32,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// How the location was obtained: the evidence's basis, or `from_node`.
    pub basis: Option<&'static str>,
}

/// One part against the `glia arch` services.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct ArchDiff {
    pub part: u32,
    /// The part's services, count descending, then name.
    pub services: Vec<String>,
    /// `aligned` | `splits_service` | `spans_services` | `unplaced`.
    pub verdict: &'static str,
}

/// A data node two or more parts write, or may write (see the module doc).
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct SharedWrite {
    pub entity: Located,
    /// The entity's node kind (`DATA_ENTITY`, `DATABASE`, ...).
    pub kind: &'static str,
    /// The parts whose members access it, ascending.
    pub parts: Vec<u32>,
    /// Per part in `parts`: `read` | `write` | `read_write` | `unknown`.
    pub modes: Vec<(u32, &'static str)>,
    /// The first [`MAX_WRITERS`] members whose access is not a known read, by
    /// part then qname.
    pub writers: Vec<Located>,
    /// Members whose access is not a known read.
    pub writers_total: usize,
    /// [`DERIVED`] when no mode is `unknown`, else [`HEURISTIC`].
    pub tier: &'static str,
}

/// Parts that depend on each other both ways: a distributed cycle once split.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct PartCycle {
    /// Two or more parts, ascending.
    pub parts: Vec<u32>,
    /// Per direction between two of `parts`, the heaviest carry edge that way,
    /// by (from part, to part).
    pub witness: Vec<CutEdge>,
    /// Always [`DERIVED`].
    pub tier: &'static str,
}

/// The splits answer.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct SplitAnswer {
    /// `global`, or `st` when a source or sink was given.
    pub mode: &'static str,
    /// `module` or `community` (`none` when the name was refused).
    pub quotient: &'static str,
    /// Units in scope.
    pub units: usize,
    /// In id order; empty iff `absence`. Under `st`, part 0 is the source
    /// side.
    pub parts: Vec<SplitPart>,
    /// Summed community weight of every edge between two parts.
    pub cut_weight: u64,
    /// The whole scope's Stoer-Wagner minimum cut weight.
    pub global_min_weight: u64,
    /// `global`: every bisection cleared the balance floor. `st`: the
    /// smaller side holds at least [`SplitArgs::min_share`] of the nodes.
    pub balanced: bool,
    /// Edges between two parts.
    pub cut_edges_total: usize,
    /// The first [`SplitArgs::max_cut_edges`] of them.
    pub cut_edges: Vec<CutEdge>,
    pub arch: Vec<ArchDiff>,
    /// Data nodes two or more parts write (see the module doc).
    pub shared_writes: Vec<SharedWrite>,
    /// Parts that depend on each other both ways.
    pub cycles: Vec<PartCycle>,
    /// Always [`HEURISTIC`].
    pub tier: &'static str,
    pub absence: Option<Absence>,
}

/// Suggest where `args.scope` of the code graph splits into
/// [`SplitArgs::parts`] services, or, with a source or sink, where it
/// separates the two at the least coupling; with the blockers either way.
/// `repo_labels` name the services exactly as `glia arch` names them
/// (`GenerateResult::repo_labels`).
pub fn splits(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    args: &SplitArgs,
) -> SplitAnswer {
    let mode = mode_of(args);
    let Some(quotient) = quotient_name(&args.quotient) else {
        let note = format!(
            "no split quotient is named `{}`; use module or community",
            args.quotient.trim()
        );
        return finish(empty(merged, args, mode, "none", 0, note), args);
    };
    let loc = Locator::new(merged);
    let scope = args.scope.as_deref().map(|s| resolve_scope(merged, s));
    let q = if quotient == QUOTIENT_MODULE {
        module_quotient(merged, &loc, repo_labels, scope.as_deref())
    } else {
        community_quotient(merged, &loc, repo_labels, scope.as_deref(), args.seed)
    };
    let units = q.unit_labels.len();
    if !(2..=MAX_UNITS).contains(&units) {
        let under = args
            .scope
            .as_deref()
            .map(|s| format!(" under `{s}`"))
            .unwrap_or_default();
        let note = if units < 2 {
            format!("fewer than two units in scope: {units} {quotient} unit(s){under}")
        } else {
            format!(
                "too many units ({units}) for Stoer-Wagner (at most {MAX_UNITS}); narrow --scope"
            )
        };
        return finish(empty(merged, args, mode, quotient, units, note), args);
    }

    let mut unit_nodes = vec![0usize; units];
    for &u in &q.unit {
        unit_nodes[u as usize] += 1;
    }
    let pairs: Vec<(u32, u32, u64)> = q
        .edges
        .iter()
        .map(|&(a, b, w, _)| (q.unit[a as usize], q.unit[b as usize], u64::from(w)))
        .filter(|&(a, b, _)| a != b)
        .collect();
    let unit_graph = WeightedGraph::from_pairs(units, &pairs);
    let cut = if mode == MODE_ST {
        match anchored(merged, &q, &unit_graph, &unit_nodes, args) {
            Ok(cut) => cut,
            Err(note) => {
                return finish(empty(merged, args, mode, quotient, units, note), args);
            }
        }
    } else {
        let cut = cut_units(
            &unit_graph,
            &unit_nodes,
            args.parts.clamp(2, MAX_PARTS),
            min_share(args.min_share),
        );
        UnitCut {
            parts: number_parts(cut.parts, &unit_nodes, &q.unit_labels),
            ..cut
        }
    };
    let parts = cut.parts;
    let mut part_of_unit = vec![0u32; units];
    for (p, us) in parts.iter().enumerate() {
        for &u in us {
            part_of_unit[u as usize] = p as u32;
        }
    }
    let part: Vec<u32> = q.unit.iter().map(|&u| part_of_unit[u as usize]).collect();

    let (cut_weight, cut_edges_total, cut_edges) = cut_edges(&q, &part, &loc, args.max_cut_edges);
    let split_parts = describe_parts(merged, &q, &part, &parts, &loc, repo_labels);
    let arch = arch_diff(&split_parts);
    let shared_writes = shared_writes(merged, &q, &part, &loc);
    let cycles = part_cycles(merged, &q, &part, parts.len(), &loc);
    finish(
        SplitAnswer {
            mode,
            quotient,
            units,
            parts: split_parts,
            cut_weight,
            global_min_weight: cut.global_min,
            balanced: cut.balanced,
            cut_edges_total,
            cut_edges,
            arch,
            shared_writes,
            cycles,
            tier: HEURISTIC,
            absence: None,
        },
        args,
    )
}

/// `st` when a source or a sink is set, else `global`.
fn mode_of(args: &SplitArgs) -> &'static str {
    if args.source.is_some() || args.sink.is_some() {
        MODE_ST
    } else {
        MODE_GLOBAL
    }
}

/// The quotient `name` names (ASCII case and surrounding space ignored; empty
/// is `module`); `None` for an unknown name.
fn quotient_name(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "" | "module" | "modules" => Some(QUOTIENT_MODULE),
        "community" | "communities" => Some(QUOTIENT_COMMUNITY),
        _ => None,
    }
}

/// `share` read into `0..=0.5`; not a finite number reads as the default.
fn min_share(share: f64) -> f64 {
    if share.is_finite() {
        share.clamp(0.0, 0.5)
    } else {
        DEFAULT_MIN_SHARE
    }
}

/// A node that may join the quotient.
struct Candidate<'g> {
    id: NodeId,
    repo: u64,
    qname: &'g str,
    kind: Option<NodeKindId>,
    file: Option<String>,
    entry: bool,
    /// The node's community (`community` quotient only).
    community: u32,
}

/// A weighted edge between two members: `(from, to, community weight, the
/// graph edge)`.
type Link<'g> = (u32, u32, u32, &'g Edge);

/// The scope's nodes grouped into units: what the cut runs over.
struct Quotient<'g> {
    /// Kept nodes, canonical order.
    members: Vec<Candidate<'g>>,
    /// Unit of each member.
    unit: Vec<u32>,
    /// The MODULE label of each member, when it sits in one.
    module: Vec<Option<&'g str>>,
    unit_labels: Vec<&'g str>,
    /// Weighted edges between two members.
    edges: Vec<Link<'g>>,
}

/// The data-node kinds: the domain's `db` effect sink.
fn data_kinds() -> &'static [NodeKindId] {
    CODE_PROFILE
        .tables
        .effect_sinks
        .iter()
        .find(|s| s.class == DATA_CLASS)
        .map_or(&[], |s| s.kinds)
}

/// The index of the first graph listing each node.
fn first_graphs(merged: &MergedGraph) -> HashMap<NodeId, usize> {
    let mut out = HashMap::new();
    for (gi, g) in merged.graphs.iter().enumerate() {
        for n in &g.nodes {
            out.entry(n.id).or_insert(gi);
        }
    }
    out
}

/// `(module id, its qname, its repo)`: the first MODULE up `id`'s parent chain
/// in the first graph listing `id`, `id` itself when it is one.
fn module_of<'g>(
    merged: &'g MergedGraph,
    first: &HashMap<NodeId, usize>,
    id: NodeId,
) -> Option<(NodeId, &'g str, u64)> {
    let g = merged.graphs.get(*first.get(&id)?)?;
    let mut cur = id;
    for _ in 0..MAX_PARENT_STEPS {
        if g.nav.kind_by_id.get(&cur) == Some(&node_kind::MODULE) {
            let qname = g.nav.qname_by_id.get(&cur).map_or("", String::as_str);
            return Some((cur, qname, g.repo.0));
        }
        cur = *g.nav.parent_of.get(&cur)?;
    }
    None
}

/// The in-scope nodes, once each (first graph wins), in canonical order.
fn scope_nodes<'g>(
    merged: &'g MergedGraph,
    loc: &Locator<'g>,
    labels: &BTreeMap<u64, String>,
    scope: Option<&str>,
) -> Vec<Candidate<'g>> {
    let entry_rule = &CODE_PROFILE.tables.entry;
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut out = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if !seen.insert(n.id) {
                continue;
            }
            let file = loc.file_of(n.id);
            if let (Some(s), Some(f)) = (scope, file.as_deref())
                && !in_scope(f, s)
            {
                continue;
            }
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            let name = g.nav.name_by_id.get(&n.id).map_or("", String::as_str);
            out.push(Candidate {
                id: n.id,
                repo: g.repo.0,
                qname: g.nav.qname_by_id.get(&n.id).map_or("", String::as_str),
                kind,
                file,
                entry: entry_rule.is_entry(kind, name, &roles_in(kind, &n.cells))
                    || is_declared_entry(&n.cells),
                community: 0,
            });
        }
    }
    let label = |repo: u64| labels.get(&repo).map_or("", String::as_str);
    out.sort_by(|a, b| {
        a.qname
            .cmp(b.qname)
            .then(a.kind.map(|k| k.0).cmp(&b.kind.map(|k| k.0)))
            .then(a.file.cmp(&b.file))
            .then(label(a.repo).cmp(label(b.repo)))
            .then(a.id.0.cmp(&b.id.0))
    });
    out
}

/// The non-data candidates with a weighted edge, in their order, and those
/// edges between them (graphs first, then cross edges; self-loops left out).
fn weighted<'g>(
    merged: &'g MergedGraph,
    cands: Vec<Candidate<'g>>,
) -> (Vec<Candidate<'g>>, Vec<Link<'g>>) {
    let data = data_kinds();
    let cands: Vec<Candidate<'g>> = cands
        .into_iter()
        .filter(|c| !c.kind.is_some_and(|k| data.contains(&k)))
        .collect();
    let at: HashMap<NodeId, u32> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id, i as u32))
        .collect();
    let tables = &CODE_PROFILE.tables;
    let mut edges = Vec::new();
    for e in merged.all_edges() {
        let w = tables.community_weight(e.category);
        if w == 0 {
            continue;
        }
        if let (Some(&a), Some(&b)) = (at.get(&e.from), at.get(&e.to))
            && a != b
        {
            edges.push((a, b, w, e));
        }
    }
    let mut linked = vec![false; cands.len()];
    for &(a, b, _, _) in &edges {
        linked[a as usize] = true;
        linked[b as usize] = true;
    }
    retain(cands, edges, &linked)
}

/// `members` where `keep` holds, and the edges between two kept members,
/// re-indexed.
fn retain<'g>(
    members: Vec<Candidate<'g>>,
    edges: Vec<Link<'g>>,
    keep: &[bool],
) -> (Vec<Candidate<'g>>, Vec<Link<'g>>) {
    let mut new_ix = vec![u32::MAX; members.len()];
    let mut next = 0u32;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            new_ix[i] = next;
            next += 1;
        }
    }
    let members = members
        .into_iter()
        .zip(keep)
        .filter(|(_, k)| **k)
        .map(|(m, _)| m)
        .collect();
    let edges = edges
        .into_iter()
        .filter_map(|(a, b, w, e)| {
            let (a, b) = (new_ix[a as usize], new_ix[b as usize]);
            (a != u32::MAX && b != u32::MAX).then_some((a, b, w, e))
        })
        .collect();
    (members, edges)
}

/// The `module` quotient of `scope` (already resolved).
fn module_quotient<'g>(
    merged: &'g MergedGraph,
    loc: &Locator<'g>,
    labels: &BTreeMap<u64, String>,
    scope: Option<&str>,
) -> Quotient<'g> {
    let (members, edges) = weighted(merged, scope_nodes(merged, loc, labels, scope));
    let first = first_graphs(merged);
    let direct: Vec<Option<(NodeId, &'g str, u64)>> = members
        .iter()
        .map(|m| module_of(merged, &first, m.id))
        .collect();

    // The units: distinct modules, by (qname, file, repo label, id).
    let label = |repo: u64| labels.get(&repo).map_or("", String::as_str);
    let mut keys: Vec<(&'g str, Option<String>, &str, NodeId)> = Vec::new();
    let mut listed: HashSet<NodeId> = HashSet::new();
    for &(id, qname, repo) in direct.iter().flatten() {
        if listed.insert(id) {
            keys.push((qname, loc.file_of(id), label(repo), id));
        }
    }
    keys.sort_by(|a, b| {
        a.0.cmp(b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(b.2))
            .then(a.3.0.cmp(&b.3.0))
    });
    let unit_at: HashMap<NodeId, u32> = keys
        .iter()
        .enumerate()
        .map(|(u, k)| (k.3, u as u32))
        .collect();
    let unit_labels: Vec<&'g str> = keys.iter().map(|k| k.0).collect();
    let mut unit: Vec<Option<u32>> = direct
        .iter()
        .map(|d| d.and_then(|(id, _, _)| unit_at.get(&id).copied()))
        .collect();
    attach(&mut unit, &edges);

    let keep: Vec<bool> = unit.iter().map(Option::is_some).collect();
    let (members, edges) = retain(members, edges, &keep);
    let unit: Vec<u32> = unit.into_iter().flatten().collect();
    let module = unit
        .iter()
        .map(|&u| Some(unit_labels[u as usize]))
        .collect();
    Quotient {
        members,
        unit,
        module,
        unit_labels,
        edges,
    }
}

/// Give each node with no unit the unit of its heaviest weighted neighbour
/// that has one (ties to the lower unit), in breadth-first layers from the
/// nodes that have one: a layer reads only the units of the layers before
/// it, so the visiting order inside a layer cannot change a choice.
fn attach(unit: &mut [Option<u32>], edges: &[Link<'_>]) {
    let n = unit.len();
    let mut pairs: Vec<(u32, u32, u64)> = Vec::with_capacity(edges.len() * 2);
    for &(a, b, w, _) in edges {
        pairs.push((a, b, u64::from(w)));
        pairs.push((b, a, u64::from(w)));
    }
    pairs.sort_unstable_by_key(|&(a, b, _)| (a, b));
    let mut nbrs: Vec<Vec<(u32, u64)>> = vec![Vec::new(); n];
    for (a, b, w) in pairs {
        let row = &mut nbrs[a as usize];
        match row.last_mut() {
            Some(last) if last.0 == b => last.1 = last.1.saturating_add(w),
            _ => row.push((b, w)),
        }
    }
    let mut frontier: Vec<u32> = (0..n as u32)
        .filter(|&i| unit[i as usize].is_some())
        .collect();
    while !frontier.is_empty() {
        let layer: BTreeSet<u32> = frontier
            .iter()
            .flat_map(|&f| nbrs[f as usize].iter().map(|&(x, _)| x))
            .filter(|&x| unit[x as usize].is_none())
            .collect();
        let placed: Vec<(u32, u32)> = layer
            .iter()
            .filter_map(|&x| {
                nbrs[x as usize]
                    .iter()
                    .filter_map(|&(y, w)| unit[y as usize].map(|u| (w, u)))
                    .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
                    .map(|(_, u)| (x, u))
            })
            .collect();
        for &(x, u) in &placed {
            unit[x as usize] = Some(u);
        }
        frontier = placed.into_iter().map(|(x, _)| x).collect();
    }
}

/// The `community` quotient of `scope` (already resolved): CD.1d's partition
/// of the same scope, data and edgeless nodes dropped, communities
/// renumbered by first member.
fn community_quotient<'g>(
    merged: &'g MergedGraph,
    loc: &Locator<'g>,
    labels: &BTreeMap<u64, String>,
    scope: Option<&str>,
    seed: u64,
) -> Quotient<'g> {
    let mut opts = CommunityOptions::default();
    opts.seed = seed;
    opts.resolution = GAMMA_ONE;
    let sp = partition_of(merged, loc, labels, scope, &opts, None);
    // The view's ids are ranks: rank r is `sp.nodes[r]`.
    let cands: Vec<Candidate<'g>> = sp
        .nodes
        .iter()
        .enumerate()
        .map(|(r, n)| Candidate {
            id: n.id,
            repo: n.repo,
            qname: n.qname,
            kind: n.kind,
            file: n.file.clone(),
            entry: n.entry,
            community: sp
                .view
                .index_of(NodeId(r as u64))
                .and_then(|ix| sp.partition.membership.get(ix as usize).copied())
                .unwrap_or(u32::MAX),
        })
        .collect();
    let (members, edges) = weighted(merged, cands);
    let keep: Vec<bool> = members.iter().map(|m| m.community != u32::MAX).collect();
    let (members, edges) = retain(members, edges, &keep);

    let first = first_graphs(merged);
    let mut renumber: HashMap<u32, u32> = HashMap::new();
    let mut unit_labels: Vec<&'g str> = Vec::new();
    let mut unit = Vec::with_capacity(members.len());
    for m in &members {
        let u = *renumber.entry(m.community).or_insert_with(|| {
            unit_labels.push(m.qname);
            (unit_labels.len() - 1) as u32
        });
        unit.push(u);
    }
    let module = members
        .iter()
        .map(|m| module_of(merged, &first, m.id).map(|(_, q, _)| q))
        .collect();
    Quotient {
        members,
        unit,
        module,
        unit_labels,
        edges,
    }
}

/// Units grouped into parts by recursive bisection.
struct UnitCut {
    parts: Vec<Vec<u32>>,
    global_min: u64,
    balanced: bool,
}

/// Bisect the unit graph until `want` parts or no part has two units (see
/// the module doc).
fn cut_units(g: &WeightedGraph, unit_nodes: &[usize], want: usize, share: f64) -> UnitCut {
    let mut parts: Vec<Vec<u32>> = vec![(0..g.len() as u32).collect()];
    let (mut global_min, mut balanced) = (None, true);
    let nodes = |p: &[u32]| p.iter().map(|&u| unit_nodes[u as usize]).sum::<usize>();
    while parts.len() < want {
        let next = parts
            .iter()
            .enumerate()
            .filter(|(_, p)| p.len() >= 2)
            .max_by(|(i, a), (j, b)| nodes(a).cmp(&nodes(b)).then(j.cmp(i)))
            .map(|(i, _)| i);
        let Some(i) = next else { break };
        let Some(b) = bisect(g, unit_nodes, &parts[i], share) else {
            break;
        };
        global_min.get_or_insert(b.min);
        balanced &= b.balanced;
        parts[i] = b.a;
        parts.push(b.b);
    }
    UnitCut {
        parts,
        global_min: global_min.unwrap_or(0),
        balanced,
    }
}

/// One bisection of a part: its two sides, the part's global minimum cut
/// weight, and whether the chosen cut cleared the floor.
struct Bisection {
    a: Vec<u32>,
    b: Vec<u32>,
    min: u64,
    balanced: bool,
}

/// Bisect `part` (sorted unit indices, two or more) on its induced subgraph:
/// the ratio-best phase cut above the floor, else the global minimum.
fn bisect(g: &WeightedGraph, unit_nodes: &[usize], part: &[u32], share: f64) -> Option<Bisection> {
    let mut pairs = Vec::new();
    for (li, &u) in part.iter().enumerate() {
        for &(v, w) in g.neighbours(u) {
            if v > u
                && let Ok(lj) = part.binary_search(&v)
            {
                pairs.push((li as u32, lj as u32, w));
            }
        }
    }
    let sub = WeightedGraph::from_pairs(part.len(), &pairs);
    let cut = stoer_wagner(&sub)?;
    let local_nodes = |l: u32| unit_nodes[part[l as usize] as usize];
    let total: usize = (0..part.len() as u32).map(local_nodes).sum();
    let floor = share * total as f64;
    // (phase, weight, smaller side's nodes)
    let mut best: Option<(usize, u64, usize)> = None;
    for (j, ph) in cut.phases.iter().enumerate() {
        let side: usize = ph.side.iter().map(|&l| local_nodes(l)).sum();
        let small = side.min(total - side);
        if small == 0 || (small as f64) < floor {
            continue;
        }
        let better = best.is_none_or(|(_, bw, bs)| {
            let (lhs, rhs) = (
                u128::from(ph.weight) * bs as u128,
                u128::from(bw) * small as u128,
            );
            lhs < rhs || (lhs == rhs && ph.weight < bw)
        });
        if better {
            best = Some((j, ph.weight, small));
        }
    }
    let in_side: Vec<bool> = match best {
        Some((j, _, _)) => {
            let mut s = vec![false; part.len()];
            for &l in &cut.phases[j].side {
                s[l as usize] = true;
            }
            let nodes: Vec<usize> = (0..part.len() as u32).map(local_nodes).collect();
            refine(&sub, &nodes, &mut s, floor);
            s
        }
        None => cut.side.clone(),
    };
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for (l, &u) in part.iter().enumerate() {
        if in_side[l] == in_side[0] {
            a.push(u)
        } else {
            b.push(u)
        }
    }
    Some(Bisection {
        a,
        b,
        min: cut.weight,
        balanced: best.is_some(),
    })
}

/// Move one unit at a time across the cut `side` while a move strictly
/// lowers the ratio weight / min(nodes A, nodes B) and leaves both sides at or
/// above `floor` nodes: the best move first (ties to the lower weight, then
/// the lower unit), at most one move per unit of the part.
///
/// Why: a phase cut is the members of one merged super-vertex, and the last
/// vertex a phase adds merges into the one added before it. A leaf unit
/// hanging off the cluster the phases start from is added after the other
/// cluster, so it merges into that one and every later phase cut carries it
/// on the wrong side. One move puts it back.
fn refine(sub: &WeightedGraph, nodes: &[usize], side: &mut [bool], floor: f64) {
    let n = side.len();
    let total: usize = nodes.iter().sum();
    let mut in_true: usize = (0..n).filter(|&l| side[l]).map(|l| nodes[l]).sum();
    // Per unit: its weight to the `true` side, and to the `false` side.
    let (mut to_true, mut to_false) = (vec![0u64; n], vec![0u64; n]);
    let mut weight = 0u64;
    for l in 0..n {
        for &(m, w) in sub.neighbours(l as u32) {
            let m = m as usize;
            if side[m] {
                to_true[l] += w;
            } else {
                to_false[l] += w;
            }
            if l < m && side[l] != side[m] {
                weight += w;
            }
        }
    }
    for _ in 0..n {
        let small = in_true.min(total - in_true);
        // (unit, weight after, smaller side after)
        let mut best: Option<(usize, u64, usize)> = None;
        for l in 0..n {
            let (own, other) = if side[l] {
                (to_true[l], to_false[l])
            } else {
                (to_false[l], to_true[l])
            };
            let after = weight.saturating_sub(other).saturating_add(own);
            let moved = if side[l] {
                in_true - nodes[l]
            } else {
                in_true + nodes[l]
            };
            let after_small = moved.min(total - moved);
            if after_small == 0 || (after_small as f64) < floor {
                continue;
            }
            let lowers =
                u128::from(after) * (small as u128) < u128::from(weight) * (after_small as u128);
            let beats = best.is_none_or(|(_, bw, bs)| {
                let (lhs, rhs) = (
                    u128::from(after) * bs as u128,
                    u128::from(bw) * after_small as u128,
                );
                lhs < rhs || (lhs == rhs && after < bw)
            });
            if lowers && beats {
                best = Some((l, after, after_small));
            }
        }
        let Some((l, after, _)) = best else { break };
        let was_true = side[l];
        side[l] = !was_true;
        in_true = if was_true {
            in_true - nodes[l]
        } else {
            in_true + nodes[l]
        };
        for &(m, w) in sub.neighbours(l as u32) {
            let m = m as usize;
            if was_true {
                to_true[m] = to_true[m].saturating_sub(w);
                to_false[m] += w;
            } else {
                to_false[m] = to_false[m].saturating_sub(w);
                to_true[m] += w;
            }
        }
        weight = after;
    }
}

/// The s-t cut between the two sides of `args` on the unit graph `g`: part 0
/// the source side, part 1 the rest (see the module doc). `Err` is the
/// absence note.
fn anchored(
    merged: &MergedGraph,
    q: &Quotient<'_>,
    g: &WeightedGraph,
    unit_nodes: &[usize],
    args: &SplitArgs,
) -> Result<UnitCut, String> {
    let (Some(source), Some(sink)) = (args.source.as_deref(), args.sink.as_deref()) else {
        let missing = if args.source.is_none() {
            "source"
        } else {
            "sink"
        };
        return Err(format!(
            "an s-t split needs both a source and a sink; the {missing} is not set"
        ));
    };
    let first = first_graphs(merged);
    let side = |which: &str, raw: &str| {
        side_units(merged, q, &first, raw, args.scope.as_deref()).ok_or_else(|| {
            format!(
                "{which} `{}` names no unit in scope: no member is located under it as a path, and no node it names has a unit",
                raw.trim()
            )
        })
    };
    let sources = side("source", source)?;
    let sinks = side("sink", sink)?;
    let shared: Vec<&str> = sources
        .iter()
        .filter(|u| sinks.binary_search(u).is_ok())
        .map(|&u| q.unit_labels[u as usize])
        .collect();
    if !shared.is_empty() {
        let more = shared.len().saturating_sub(MAX_NOTED_UNITS);
        let tail = if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        };
        return Err(format!(
            "source `{}` and sink `{}` share {} unit(s): {}{tail}",
            source.trim(),
            sink.trim(),
            shared.len(),
            shared[..shared.len().min(MAX_NOTED_UNITS)].join(", ")
        ));
    }
    let Some(st) = min_st_cut(g, &sources, &sinks) else {
        return Err(format!(
            "no s-t cut separates source `{}` from sink `{}`",
            source.trim(),
            sink.trim()
        ));
    };
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for (u, &on_source) in st.source_side.iter().enumerate() {
        if on_source {
            a.push(u as u32);
        } else {
            b.push(u as u32);
        }
    }
    let total: usize = unit_nodes.iter().sum();
    let side_nodes: usize = a.iter().map(|&u| unit_nodes[u as usize]).sum();
    let small = side_nodes.min(total - side_nodes);
    let balanced = small > 0 && small as f64 >= min_share(args.min_share) * total as f64;
    Ok(UnitCut {
        parts: vec![a, b],
        global_min: stoer_wagner(g).map_or(0, |c| c.weight),
        balanced,
    })
}

/// The units one side of an s-t split names, sorted: every unit with a
/// member located under `raw` read as a path or project label, else the unit
/// of the node `raw` names (`resolve_seed`, preferring one under `scope`),
/// else of that node's enclosing MODULE. `None` when neither reading names a
/// unit.
fn side_units(
    merged: &MergedGraph,
    q: &Quotient<'_>,
    first: &HashMap<NodeId, usize>,
    raw: &str,
    scope: Option<&str>,
) -> Option<Vec<u32>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let path = resolve_scope(merged, raw);
    let by_path: BTreeSet<u32> = q
        .members
        .iter()
        .zip(&q.unit)
        .filter(|(m, _)| m.file.as_deref().is_some_and(|f| in_scope(f, &path)))
        .map(|(_, &u)| u)
        .collect();
    if !by_path.is_empty() {
        return Some(by_path.into_iter().collect());
    }
    let unit_of = |id: NodeId| q.members.iter().position(|m| m.id == id).map(|i| q.unit[i]);
    let id = resolve_seed(merged, raw, scope)?;
    let unit = unit_of(id)
        .or_else(|| module_of(merged, first, id).and_then(|(module, _, _)| unit_of(module)))?;
    Some(vec![unit])
}

/// The parts in id order: nodes descending, then smallest unit label, then
/// smallest unit.
fn number_parts(mut parts: Vec<Vec<u32>>, unit_nodes: &[usize], labels: &[&str]) -> Vec<Vec<u32>> {
    let key = |p: &Vec<u32>| {
        let nodes: usize = p.iter().map(|&u| unit_nodes[u as usize]).sum();
        let label = p.iter().map(|&u| labels[u as usize]).min().unwrap_or("");
        (std::cmp::Reverse(nodes), label, p.first().copied())
    };
    parts.sort_by(|a, b| key(a).cmp(&key(b)));
    parts
}

/// `(cut weight, cut edges, the first `cap` of them located)`.
fn cut_edges(
    q: &Quotient<'_>,
    part: &[u32],
    loc: &Locator<'_>,
    cap: usize,
) -> (u64, usize, Vec<CutEdge>) {
    let mut crossing: Vec<&Link<'_>> = q
        .edges
        .iter()
        .filter(|(a, b, _, _)| part[*a as usize] != part[*b as usize])
        .collect();
    let weight = crossing
        .iter()
        .fold(0u64, |s, e| s.saturating_add(u64::from(e.2)));
    let total = crossing.len();
    // Stable: equal keys keep graph edge order.
    crossing.sort_by(|x, y| {
        y.2.cmp(&x.2)
            .then(edge_category::name(x.3.category).cmp(edge_category::name(y.3.category)))
            .then(
                q.members[x.0 as usize]
                    .qname
                    .cmp(q.members[y.0 as usize].qname),
            )
            .then(
                q.members[x.1 as usize]
                    .qname
                    .cmp(q.members[y.1 as usize].qname),
            )
    });
    let take = if cap == 0 { usize::MAX } else { cap };
    let listed = crossing
        .into_iter()
        .take(take)
        .map(|&(a, b, w, e)| cut_edge(q, part, loc, (a, b, w, e)))
        .collect();
    (weight, total, listed)
}

/// The edge `e` from member `a` to member `b`, of community weight `w`,
/// located at its evidence site.
fn cut_edge(q: &Quotient<'_>, part: &[u32], loc: &Locator<'_>, link: Link<'_>) -> CutEdge {
    let (a, b, w, e) = link;
    let (file, line, basis) = site_of(loc, e);
    CutEdge {
        from_qname: q.members[a as usize].qname.to_string(),
        to_qname: q.members[b as usize].qname.to_string(),
        category: edge_category::name(e.category),
        from_part: part[a as usize],
        to_part: part[b as usize],
        weight: w,
        file,
        line,
        basis,
    }
}

/// Each member's index in `q`.
fn member_index(q: &Quotient<'_>) -> HashMap<NodeId, u32> {
    q.members
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id, i as u32))
        .collect()
}

/// The kind of `id` in the first graph naming it.
fn kind_of(merged: &MergedGraph, id: NodeId) -> Option<NodeKindId> {
    merged
        .graphs
        .iter()
        .find_map(|g| g.nav.kind_by_id.get(&id).copied())
}

/// The ACCESS_MODE an edge carries (LE.4a), as its `'static` spelling.
fn access_mode(e: &Edge) -> Option<&'static str> {
    let cell = e.cell(cell_type::ACCESS_MODE)?;
    let (CellPayload::Text(t) | CellPayload::Json(t)) = &cell.payload else {
        return None;
    };
    ["read", "write", "read_write"]
        .into_iter()
        .find(|m| *m == t.as_str())
}

/// How one part accesses one data node, over every accessing edge.
#[derive(Default, Clone, Copy)]
struct Access {
    read: bool,
    write: bool,
    /// An edge with no ACCESS_MODE it can read.
    unknown: bool,
}

impl Access {
    fn add(&mut self, mode: Option<&str>) {
        match mode {
            Some("read") => self.read = true,
            Some("write") => self.write = true,
            Some(_) => {
                self.read = true;
                self.write = true;
            }
            None => self.unknown = true,
        }
    }

    /// The folded mode: read + write is `read_write`, and an unknown edge
    /// never reads as a read (see the module doc).
    fn mode(self) -> &'static str {
        match (self.read, self.write, self.unknown) {
            (true, true, _) => "read_write",
            (false, true, _) => "write",
            (true, false, false) => "read",
            (_, false, _) => UNKNOWN,
        }
    }
}

/// Per data node: its parts' accesses, and its possible writers as `(part,
/// qname, member)`.
type DataAccess<'g> = (BTreeMap<u32, Access>, BTreeSet<(u32, &'g str, u32)>);

/// The data nodes two or more parts write or may write (see the module doc).
fn shared_writes(
    merged: &MergedGraph,
    q: &Quotient<'_>,
    part: &[u32],
    loc: &Locator<'_>,
) -> Vec<SharedWrite> {
    let data = data_kinds();
    let at = member_index(q);
    let mut kinds: HashMap<NodeId, Option<NodeKindId>> = HashMap::new();
    let mut by_entity: HashMap<NodeId, DataAccess<'_>> = HashMap::new();
    for e in merged.all_edges() {
        if e.category != edge_category::ACCESSES_DATA {
            continue;
        }
        let Some(&m) = at.get(&e.from) else {
            continue;
        };
        let kind = *kinds.entry(e.to).or_insert_with(|| kind_of(merged, e.to));
        if !kind.is_some_and(|k| data.contains(&k)) {
            continue;
        }
        let p = part[m as usize];
        let mode = access_mode(e);
        let (parts, writers) = by_entity.entry(e.to).or_default();
        parts.entry(p).or_default().add(mode);
        if mode != Some("read") {
            writers.insert((p, q.members[m as usize].qname, m));
        }
    }
    let mut rows: Vec<SharedWrite> = by_entity
        .into_iter()
        .filter_map(|(entity, (parts, writers))| {
            let modes: Vec<(u32, &'static str)> =
                parts.iter().map(|(&p, a)| (p, a.mode())).collect();
            let writing = modes.iter().filter(|(_, m)| *m != "read").count();
            if modes.len() < 2 || writing < 2 {
                return None;
            }
            let tier = if modes.iter().any(|(_, m)| *m == UNKNOWN) {
                HEURISTIC
            } else {
                DERIVED
            };
            Some(SharedWrite {
                entity: loc.locate(entity),
                kind: kinds
                    .get(&entity)
                    .copied()
                    .flatten()
                    .map_or("UNKNOWN", node_kind::name),
                parts: parts.keys().copied().collect(),
                modes,
                writers: writers
                    .iter()
                    .take(MAX_WRITERS)
                    .map(|&(_, _, m)| loc.locate(q.members[m as usize].id))
                    .collect(),
                writers_total: writers.len(),
                tier,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b.parts
            .len()
            .cmp(&a.parts.len())
            .then(b.writers_total.cmp(&a.writers_total))
            .then(a.entity.qname.cmp(&b.entity.qname))
            .then(a.entity.id.cmp(&b.entity.id))
    });
    rows
}

/// The parts as a graph over synthetic ids: part `p` is `NodeId(p)`, one edge
/// per direction between two parts.
struct PartGraph {
    nodes: Vec<NodeId>,
    edges: Vec<Edge>,
}

impl GraphSource for PartGraph {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

/// `x` goes before `y` in the cut-edge order: heavier, then category name,
/// then from qname, then to qname.
fn heavier(q: &Quotient<'_>, x: &Link<'_>, y: &Link<'_>) -> bool {
    let key = |l: &Link<'_>| {
        (
            std::cmp::Reverse(l.2),
            edge_category::name(l.3.category),
            q.members[l.0 as usize].qname,
            q.members[l.1 as usize].qname,
        )
    };
    key(x) < key(y)
}

/// The cycles between `parts` parts (see the module doc).
fn part_cycles(
    merged: &MergedGraph,
    q: &Quotient<'_>,
    part: &[u32],
    parts: usize,
    loc: &Locator<'_>,
) -> Vec<PartCycle> {
    let tables = &CODE_PROFILE.tables;
    let carry = CategorySet::of(tables.carry_edges);
    let at = member_index(q);
    // (from part, to part) -> the heaviest carry edge that way; the first in
    // edge order wins a full tie.
    let mut heaviest: BTreeMap<(u32, u32), Link<'_>> = BTreeMap::new();
    for e in merged.all_edges() {
        if !carry.contains(e.category) {
            continue;
        }
        let (Some(&a), Some(&b)) = (at.get(&e.from), at.get(&e.to)) else {
            continue;
        };
        let (pa, pb) = (part[a as usize], part[b as usize]);
        if pa == pb {
            continue;
        }
        let link: Link<'_> = (a, b, tables.community_weight(e.category), e);
        let slot = heaviest.entry((pa, pb)).or_insert(link);
        if heavier(q, &link, slot) {
            *slot = link;
        }
    }
    if heaviest.len() < 2 {
        return Vec::new();
    }
    let graph = PartGraph {
        nodes: (0..parts as u64).map(NodeId).collect(),
        edges: heaviest
            .iter()
            .map(|(&(a, b), l)| {
                Edge::new(
                    NodeId(u64::from(a)),
                    NodeId(u64::from(b)),
                    l.3.category,
                    Confidence::Strong,
                )
            })
            .collect(),
    };
    let adj = Adjacency::build(&graph, &CategorySet::all());
    strongly_connected(&adj)
        .into_iter()
        .map(|comp| {
            let ps: Vec<u32> = comp.iter().map(|id| id.0 as u32).collect();
            let witness = heaviest
                .iter()
                .filter(|((a, b), _)| ps.binary_search(a).is_ok() && ps.binary_search(b).is_ok())
                .map(|(_, &l)| cut_edge(q, part, loc, l))
                .collect();
            PartCycle {
                parts: ps,
                witness,
                tier: DERIVED,
            }
        })
        .collect()
}

/// Where `e` is asserted: its EVIDENCE site (the one 0-based to 1-based step
/// of this answer), else its from node's declaration.
fn site_of(loc: &Locator<'_>, e: &Edge) -> (Option<String>, Option<i64>, Option<&'static str>) {
    if let Some(ev) = Evidence::of(e)
        && let Some(file) = ev.file
    {
        return (
            Some(file),
            ev.line.map(|l| i64::from(l) + 1),
            Some(basis_name(ev.basis)),
        );
    }
    let at = loc.locate(e.from);
    let basis = at.file.is_some().then_some("from_node");
    (at.file, at.line, basis)
}

fn basis_name(b: Basis) -> &'static str {
    match b {
        Basis::Site => "site",
        Basis::FromNode => "from_node",
        Basis::ToNode => "to_node",
        Basis::File => "file",
        Basis::None => "none",
    }
}

/// Each part summarised, in id order.
fn describe_parts(
    merged: &MergedGraph,
    q: &Quotient<'_>,
    part: &[u32],
    parts: &[Vec<u32>],
    loc: &Locator<'_>,
    labels: &BTreeMap<u64, String>,
) -> Vec<SplitPart> {
    let keying = default_keying(merged);
    let mut inner = vec![0u64; q.members.len()];
    for &(a, b, w, _) in &q.edges {
        if part[a as usize] == part[b as usize] {
            inner[a as usize] = inner[a as usize].saturating_add(u64::from(w));
            inner[b as usize] = inner[b as usize].saturating_add(u64::from(w));
        }
    }
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); parts.len()];
    for (m, &p) in part.iter().enumerate() {
        members[p as usize].push(m as u32);
    }
    parts
        .iter()
        .zip(members)
        .enumerate()
        .map(|(p, (units, mut ms))| {
            let mut services: BTreeMap<String, usize> = BTreeMap::new();
            // Home label (the member's module, else its own qname) -> members.
            let mut homes: BTreeMap<&str, usize> = BTreeMap::new();
            let mut modules: BTreeSet<&str> = BTreeSet::new();
            let mut entries = 0;
            for &m in &ms {
                let c = &q.members[m as usize];
                if let Some(f) = c.file.as_deref() {
                    *services
                        .entry(service_of(f, c.repo, &keying, labels))
                        .or_default() += 1;
                }
                let module = q.module[m as usize];
                modules.extend(module);
                *homes.entry(module.unwrap_or(c.qname)).or_default() += 1;
                entries += usize::from(c.entry);
            }
            ms.sort_by(|&x, &y| {
                inner[y as usize]
                    .cmp(&inner[x as usize])
                    .then(q.members[x as usize].qname.cmp(q.members[y as usize].qname))
                    .then(x.cmp(&y))
            });
            SplitPart {
                id: p as u32,
                nodes: ms.len(),
                units: units.len(),
                label: label_of(&homes),
                modules: modules
                    .into_iter()
                    .take(MAX_MODULES)
                    .map(String::from)
                    .collect(),
                services: by_count(services),
                entries,
                top_members: ms
                    .iter()
                    .take(TOP_MEMBERS)
                    .map(|&m| loc.locate(q.members[m as usize].id))
                    .collect(),
            }
        })
        .collect()
}

/// The longest `::`-segment prefix every home label (a member's module, else
/// its own qname) shares, else the first segment holding the most members
/// (ties to the smaller string).
fn label_of(homes: &BTreeMap<&str, usize>) -> String {
    let split: Vec<Vec<&str>> = homes.keys().map(|h| h.split("::").collect()).collect();
    let Some(first) = split.first() else {
        return String::new();
    };
    let common = split[1..].iter().fold(first.len(), |c, segs| {
        c.min(
            first
                .iter()
                .zip(segs)
                .take_while(|(a, b)| a == b && !a.is_empty())
                .count(),
        )
    });
    if common > 0 {
        return first[..common].join("::");
    }
    let mut heads: BTreeMap<&str, usize> = BTreeMap::new();
    for (h, n) in homes {
        *heads.entry(h.split("::").next().unwrap_or("")).or_default() += n;
    }
    by_count(heads)
        .into_iter()
        .next()
        .map(|(h, _)| h.to_string())
        .unwrap_or_default()
}

/// A histogram as `(key, count)`, count descending, then key.
fn by_count<K: Ord>(h: BTreeMap<K, usize>) -> Vec<(K, usize)> {
    let mut rows: Vec<(K, usize)> = h.into_iter().collect();
    // Stable, so equal counts keep the map's key order.
    rows.sort_by_key(|r| std::cmp::Reverse(r.1));
    rows
}

/// Each part against the services (see the module doc).
fn arch_diff(parts: &[SplitPart]) -> Vec<ArchDiff> {
    let mut parts_of: BTreeMap<&str, usize> = BTreeMap::new();
    for p in parts {
        for (s, _) in &p.services {
            *parts_of.entry(s.as_str()).or_default() += 1;
        }
    }
    parts
        .iter()
        .map(|p| {
            let services: Vec<String> = p.services.iter().map(|(s, _)| s.clone()).collect();
            let verdict = match services.as_slice() {
                [] => "unplaced",
                [one] if parts_of.get(one.as_str()).copied().unwrap_or(0) > 1 => "splits_service",
                [_] => "aligned",
                _ => "spans_services",
            };
            ArchDiff {
                part: p.id,
                services,
                verdict,
            }
        })
        .collect()
}

/// The query an absence names.
fn query(args: &SplitArgs) -> String {
    let scope = args
        .scope
        .as_deref()
        .map(|s| format!(" scope={s}"))
        .unwrap_or_default();
    let anchor = |name: &str, side: &Option<String>| {
        side.as_deref()
            .map(|s| format!(" {name}={}", s.trim()))
            .unwrap_or_default()
    };
    format!(
        "quotient={} parts={} min_share={} seed={}{scope}{}{}",
        args.quotient.trim(),
        args.parts,
        args.min_share,
        args.seed,
        anchor("source", &args.source),
        anchor("sink", &args.sink),
    )
}

/// An answer with no cut: an absence `no_match` with `note`.
fn empty(
    merged: &MergedGraph,
    args: &SplitArgs,
    mode: &'static str,
    quotient: &'static str,
    units: usize,
    note: String,
) -> SplitAnswer {
    let mechanisms: Vec<&'static str> = CODE_PROFILE
        .tables
        .community_weights
        .iter()
        .map(|&(c, _)| edge_category::name(c))
        .collect();
    let absence = absence::empty(
        merged,
        PRIMITIVE,
        &query(args),
        "no_match",
        note,
        &mechanisms,
        None,
    );
    SplitAnswer {
        mode,
        quotient,
        units,
        parts: Vec::new(),
        cut_weight: 0,
        global_min_weight: 0,
        balanced: false,
        cut_edges_total: 0,
        cut_edges: Vec::new(),
        arch: Vec::new(),
        shared_writes: Vec::new(),
        cycles: Vec::new(),
        tier: HEURISTIC,
        absence: Some(absence),
    }
}

/// Print the CD.2b / CD.2c fired_on line and hand the answer back.
fn finish(a: SplitAnswer, args: &SplitArgs) -> SplitAnswer {
    let surface = if args.surface.is_empty() {
        SURFACE_ENGINE
    } else {
        args.surface
    };
    eprintln!(
        "[splits] mode={} quotient={} units={} parts={} cut_weight={} global_min={} balanced={} cut_edges={} shared_writes={} part_cycles={} surface={surface}",
        a.mode,
        a.quotient,
        a.units,
        a.parts.len(),
        a.cut_weight,
        a.global_min_weight,
        a.balanced,
        a.cut_edges_total,
        a.shared_writes.len(),
        a.cycles.len(),
    );
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two 4-cliques of weight 20 joined by 10, and a leaf joined by 5 to
    /// the first: units of 3 nodes, the leaf 2.
    fn barbell() -> (WeightedGraph, Vec<usize>) {
        let mut pairs = Vec::new();
        for base in [0u32, 4] {
            for a in base..base + 4 {
                for b in a + 1..base + 4 {
                    pairs.push((a, b, 20));
                }
            }
        }
        pairs.extend([(1, 4, 5), (2, 5, 5), (3, 8, 5)]);
        let mut nodes = vec![3; 9];
        nodes[8] = 2;
        (WeightedGraph::from_pairs(9, &pairs), nodes)
    }

    #[test]
    fn the_floor_skips_the_leaf_for_the_seam() {
        let (g, nodes) = barbell();
        let all: Vec<u32> = (0..9).collect();
        let b = bisect(&g, &nodes, &all, 0.1).expect("two or more units");
        assert_eq!(b.min, 5, "the global minimum peels the leaf");
        assert!(b.balanced);
        assert_eq!((b.a, b.b), (vec![0, 1, 2, 3, 8], vec![4, 5, 6, 7]));

        // No floor: the leaf's ratio 5 / 2 loses to the seam's 10 / 12 all
        // the same. A floor no phase cut clears falls back to the global
        // minimum, unbalanced.
        let free = bisect(&g, &nodes, &all, 0.0).expect("cut");
        assert_eq!(free.b, vec![4, 5, 6, 7]);
        let strict = bisect(&g, &nodes, &all, 0.5).expect("cut");
        assert!(
            !strict.balanced,
            "13 of 26 on each side: no phase cut has it"
        );
        assert_eq!(strict.b, vec![8], "the global minimum");
    }

    #[test]
    fn refinement_moves_a_leaf_glued_to_the_wrong_side() {
        // The phases start in the first clique, which the leaf (8) hangs
        // off: the seam's phase cut carries the leaf with the second clique
        // (weight 15). One move lowers it to the seam (10).
        let (g, nodes) = barbell();
        let cut = stoer_wagner(&g).expect("nine units");
        assert!(
            !cut.phases
                .iter()
                .any(|p| p.side == [4, 5, 6, 7] || p.side == [0, 1, 2, 3, 8]),
            "no phase cut is the seam: {:?}",
            cut.phases
        );
        let mut side = vec![false, false, false, false, true, true, true, true, true];
        refine(&g, &nodes, &mut side, 2.6);
        assert_eq!(
            side,
            [false, false, false, false, true, true, true, true, false]
        );
        // At the seam no move lowers the ratio.
        let before = side.clone();
        refine(&g, &nodes, &mut side, 2.6);
        assert_eq!(side, before);
        // A move that would breach the floor is never made.
        let mut lopsided = vec![true, false, false, false, false, false, false, false, false];
        refine(&g, &nodes, &mut lopsided, 3.0);
        assert!(lopsided.iter().filter(|&&t| t).count() >= 1);
        let small: usize = (0..9).filter(|&l| lopsided[l]).map(|l| nodes[l]).sum();
        assert!(small.min(26 - small) >= 3);
    }

    #[test]
    fn recursion_splits_the_larger_part_and_numbers_by_nodes() {
        let (g, nodes) = barbell();
        let cut = cut_units(&g, &nodes, 3, 0.1);
        assert_eq!(cut.global_min, 5);
        assert_eq!(cut.parts.len(), 3);
        assert!(cut.parts.iter().all(|p| !p.is_empty()));
        let labels = ["a", "b", "c", "d", "e", "f", "g", "h", "z"];
        let parts = number_parts(cut.parts, &nodes, &labels);
        assert_eq!(parts, [vec![0, 1, 2, 3], vec![4, 5, 6, 7], vec![8]]);
        // Two units, asked for eight parts: stops at two.
        let pair = WeightedGraph::from_pairs(2, &[(0, 1, 3)]);
        assert_eq!(cut_units(&pair, &[1, 1], 8, 0.1).parts.len(), 2);
    }

    #[test]
    fn attachment_follows_the_heaviest_neighbour_layer_by_layer() {
        // 0 and 1 are moduled (units 0 and 1); 2 hangs off both (3 to unit 1,
        // 3 to unit 0: a tie, to the lower unit), 3 hangs off 2 only, 4 off
        // nothing.
        let e = Edge {
            from: NodeId(0),
            to: NodeId(0),
            category: edge_category::CALLS,
            confidence: glia_core::Confidence::Strong,
            cells: Vec::new(),
        };
        let edges = [(1, 2, 3, &e), (0, 2, 3, &e), (2, 3, 1, &e)];
        let mut unit = vec![Some(0), Some(1), None, None, None];
        attach(&mut unit, &edges);
        assert_eq!(unit, [Some(0), Some(1), Some(0), Some(0), None]);
    }

    #[test]
    fn access_modes_fold_and_unknown_never_reads() {
        let fold = |modes: &[Option<&str>]| {
            let mut a = Access::default();
            for &m in modes {
                a.add(m);
            }
            a.mode()
        };
        assert_eq!(fold(&[Some("read")]), "read");
        assert_eq!(fold(&[Some("write"), Some("write")]), "write");
        assert_eq!(fold(&[Some("read"), Some("write")]), "read_write");
        assert_eq!(fold(&[Some("read_write")]), "read_write");
        assert_eq!(fold(&[None]), UNKNOWN);
        assert_eq!(fold(&[Some("read"), None]), UNKNOWN, "never a read");
        assert_eq!(fold(&[Some("write"), None]), "write");
        assert_eq!(fold(&[]), UNKNOWN);
    }

    #[test]
    fn names_and_shares() {
        assert_eq!(quotient_name(" Module "), Some(QUOTIENT_MODULE));
        assert_eq!(quotient_name(""), Some(QUOTIENT_MODULE));
        assert_eq!(quotient_name("community"), Some(QUOTIENT_COMMUNITY));
        assert_eq!(quotient_name("files"), None);
        assert_eq!(min_share(0.9), 0.5);
        assert_eq!(min_share(-1.0), 0.0);
        assert_eq!(min_share(f64::NAN), DEFAULT_MIN_SHARE);
    }

    #[test]
    fn labels_are_the_common_prefix_else_the_heaviest_head() {
        let h = |rows: &[(&'static str, usize)]| rows.iter().copied().collect::<BTreeMap<_, _>>();
        assert_eq!(label_of(&h(&[("a::b::c", 1), ("a::b::d", 2)])), "a::b");
        assert_eq!(
            label_of(&h(&[
                ("orders::api", 3),
                ("orders::cart", 3),
                ("util::fmt", 2)
            ])),
            "orders"
        );
        assert_eq!(label_of(&h(&[("x::a", 1), ("y::b", 1)])), "x");
        assert_eq!(label_of(&BTreeMap::new()), "");
    }
}
