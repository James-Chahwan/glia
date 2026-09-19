//! **cross_stack_trace** (P3, LD.4a): follow a feature forward across service
//! boundaries and answer with the ranked DISTINCT paths it takes, each hop
//! labelled with its mechanism and whether it crossed a service, plus a
//! two-node mode (`to`) for "how does A reach B".
//!
//! Moved here from `answers` by LD.4a; LD.4b extends it with entry flows
//! ([`entry_flows`], below). Module slot declared by L0.2, reached as
//! `repo_graph_engine::trace::<item>`, never flattened into the crate root.
//!
//! # The answer
//!
//! - `hops`: the forward breadth-first tree over the code profile's carry
//!   edges (`algo::reach::bfs` over `Adjacency::carry`), in discovery order,
//!   exactly what `cross_stack_trace` returned before LD.4a. A consumer that
//!   wants the reached SET reads it.
//! - `paths`: the distinct simple paths from the seed, ranked. One-node mode
//!   enumerates MAXIMAL paths: a path ends where no successor off the path
//!   remains, or at `depth` hops. Two-node mode enumerates the directed paths
//!   that end at the target. Enumeration is a depth-first walk over a
//!   successor map built once per call from `MergedGraph::all_edges` (global
//!   edge order, so deterministic), with one successor per distinct target: of
//!   two edges `a -> b` the first in edge order speaks, the one the BFS tree
//!   also reports.
//!
//! # Rank key (no scores)
//!
//! `(cross_service_hops desc, distinct mechanisms desc, length desc, the
//! path's qname sequence asc, its NodeId sequence asc)`. The first
//! `max_paths` are kept (`0` keeps every one); `rank` is the 1-based position.
//!
//! # cross_service
//!
//! A hop crosses a service when its two ends sit in different repos
//! (`cross_repo`, the pre-LD.4a value, carried alongside) OR, when the
//! build's `arch::default_keying` is `ProjectRoots` (one repo with manifest
//! roots below its top level), when `arch::service_of` keys the two ends
//! differently: a manifest-rooted monorepo's `web -> api` hop is cross-service
//! exactly when `glia arch` draws that link. `TopLevelDir` keying is never
//! used for the flag: it would call `src/ -> lib/` cross-service in a plain
//! app. An end is keyed by its located file; an end with none is keyed by the
//! owner segment of its qname (`POST /users @api`, read with
//! `endpoint::split_owner`); an end with neither falls back to the repo
//! comparison.
//!
//! # Bounds
//!
//! Path enumeration is exponential in the worst case, so the walk stops after
//! [`EXPANSION_BUDGET`] path extensions and says so in `truncated` and in the
//! marker, never silently. Two-node mode prunes with a backward walk from the
//! target: a node is entered only when the target is still reachable within
//! the hops left.
//!
//! # Seeds
//!
//! The feature (and `to`) resolve by exact qname (`resolved_by = "qname"`),
//! then exact name (`"name"`) — `answers::resolve_seed` without a scope — then
//! `find`'s top hit when its tier is `exact_ci`, `qname_suffix`,
//! `name_prefix` or `name_word` (`"find"`; never a substring or subsequence
//! tier). Otherwise the answer is an `unknown_symbol` absence with find's
//! nearest qnames as suggestions (`"none"`).
//!
//! The entry-flow tier (LD.4b, one-node mode only): when the feature resolves
//! to a node no carry edge leaves (a dead end — `orders` naming the
//! QUEUE_PRODUCER a handler sends on), or resolves to nothing, and an entry
//! flow's key matches the feature's slug, the seed is that entry instead
//! (`"entry_flow"`). A key matches when it equals the slug, else when either
//! contains the other; among matches, an equal key beats a containing one,
//! then the larger reach within `depth`, then key, qname and id ascending.
//! Only entries reaching at least one node count. Keys come from names alone,
//! so only the matching entries are walked. A hit a carry edge leaves is never
//! replaced; with no matching key the LD.4a answer stands. This is the
//! repo-graph wrapper's `nodes_for_feature` fallback, now explainable.
//!
//! # Entry flows
//!
//! [`entry_flows`] (LD.4b, the wrapper's `_build_flows`): one [`EntryFlow`]
//! per entry point (`CODE_PROFILE.tables.entry`, roles included — the set
//! liveness seeds from) that reaches anything: its forward BFS tree over the
//! carry edges within `depth`, as the same located [`TraceHop`]s `hops` holds,
//! the services it touches and the mechanisms it uses. One carry index and one
//! [`Locator`] per call serve every entry. Structural edges (DEFINES,
//! CONTAINS, IMPORTS) are not flow, so no flow pulls in a whole module.
//!
//! `services`: each node's service in first-seen order, the entry's first —
//! the repo's label, or in a manifest-rooted monorepo (`ProjectRoots`) the
//! `glia arch` service its file sits in. `TopLevelDir` keying counts the repo
//! as one service, for the reason `cross_service` above never uses it.
//! `cross_service` is `services.len() > 1`.
//!
//! fired_on marker, one line per [`entry_flows`] call:
//! `[flows] entries=<n> flows=<kept> cross_service=<n> depth<=<d>` — grep
//! `[flows] entries=`. It follows LD.6's `[live] annotate surface=flows` line.
//!
//! # Trace marker
//!
//! fired_on marker, one line per [`cross_stack_trace`] call:
//! `[trace] seed=<query> resolved_by=<r> paths=<n> expanded=<n> truncated=<bool>`
//! (two-node mode appends `to=<query> directed=<true|false|->`, `-` when no
//! path was found) — grep `[trace] seed=`.
//! The LD.6 `[live] annotate surface=trace` line still prints once per call.

use std::collections::{BTreeMap, HashMap, HashSet};

use repo_graph_activation::algo::{Adjacency, CategorySet, Walk, reach};
use repo_graph_code_domain::{edge_category, endpoint};
use repo_graph_core::{EdgeCategoryId, NodeId, NodeKindId};
use repo_graph_graph::roles::roles_in;
use repo_graph_graph::{MergedGraph, Reach};

use crate::absence::{self, Absence};
use crate::answers::{Located, Locator, entrypoint_reachable, live_marker};
use crate::arch::{ServiceKeying, default_keying, service_of};
use crate::find::{self, FindOptions, FoundNode};
use crate::profile::CODE_PROFILE;

/// `TraceOptions::default().depth`.
pub const DEFAULT_DEPTH: usize = 6;
/// `TraceOptions::default().max_paths`.
pub const DEFAULT_MAX_PATHS: usize = 10;
/// Path extensions the depth-first enumeration may make per call before it
/// stops and reports `truncated`.
pub const EXPANSION_BUDGET: usize = 50_000;

/// The `find` tiers a seed may resolve through when no qname or name matches
/// exactly: the ones that still name the query, never a substring or
/// subsequence match.
const FIND_TIERS: [&str; 4] = ["exact_ci", "qname_suffix", "name_prefix", "name_word"];

/// How far and to what a trace walks. Start from `default()` and set fields:
/// `#[non_exhaustive]` rules out a struct literal outside this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TraceOptions {
    /// Maximum hops per path, and the depth of the `hops` tree.
    pub depth: usize,
    /// Two-node mode: a qname or name to trace TO.
    pub to: Option<String>,
    /// Keep the first `max_paths` ranked paths; `0` keeps every one.
    pub max_paths: usize,
}

impl Default for TraceOptions {
    fn default() -> Self {
        TraceOptions {
            depth: DEFAULT_DEPTH,
            to: None,
            max_paths: DEFAULT_MAX_PATHS,
        }
    }
}

/// One hop in a cross-stack trace: a typed edge from one entity to the next,
/// with the `mechanism` (edge category) and whether it crossed a service
/// boundary. The destination is located.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TraceHop {
    /// In `hops`, the BFS depth; in a path, the hop's 1-based position.
    pub depth: usize,
    /// Edge category name — the mechanism (`CALLS`, `HTTP_CALLS`, `QUEUE_FLOWS`…).
    pub mechanism: &'static str,
    /// The two ends sit in different services: different repos, or (a single
    /// repo keyed by manifest project roots) different `glia arch` services.
    pub cross_service: bool,
    /// The two ends sit in different repos (the pre-LD.4a `cross_service`).
    pub cross_repo: bool,
    pub from_qname: String,
    pub to_qname: String,
    pub to_kind: &'static str,
    /// The destination is reachable from an entrypoint (LD.6, the flag
    /// `BlastAnswer::live` carries): `false` = likely dead.
    pub to_live: bool,
    pub to_file: Option<String>,
    /// 1-based (see [`Located`]).
    pub to_line: Option<i64>,
}

/// One ranked path: its hops in order, and the three facts it ranks by.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TracePath {
    /// 1-based position in the rank order.
    pub rank: usize,
    pub hops: Vec<TraceHop>,
    /// Hops with `cross_service` set.
    pub cross_service_hops: usize,
    /// Distinct hop mechanisms, in first-appearance order along the path.
    pub mechanisms: Vec<&'static str>,
    /// Hop count.
    pub length: usize,
    /// `true` for a path that follows carry edges forward. `false` only for
    /// the two-node fallback: the shortest path over ANY edge category, walked
    /// either way, when no directed carry path exists.
    pub directed: bool,
}

/// The whole trace answer. `absence` is `Some` exactly when `paths` is empty.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct TraceAnswer {
    /// The node the feature resolved to.
    pub seed: Option<Located>,
    /// Two-node mode: the node `to` resolved to.
    pub target: Option<Located>,
    /// How the seed resolved: `qname`, `name`, `find`, `entry_flow` (a dead
    /// end or an unresolved word yielded to the entry flow its key names,
    /// LD.4b) or `none`.
    pub resolved_by: &'static str,
    /// The seed's forward BFS tree over the carry edges, in discovery order.
    pub hops: Vec<TraceHop>,
    /// The ranked distinct paths.
    pub paths: Vec<TracePath>,
    /// The path enumeration hit [`EXPANSION_BUDGET`]: `paths` ranks only the
    /// paths found before it stopped.
    pub truncated: bool,
    pub absence: Option<Absence>,
}

/// One entry point's forward flow (LD.4b): what it reaches over the carry
/// edges within the walk's depth — see the module doc's "Entry flows".
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EntryFlow {
    /// The flow's feature word: the entry's name lower-cased, with spaces and
    /// hyphens as `_` (`POST /orders` -> `post_/orders`), the repo-graph
    /// wrapper's spelling. Not unique: two repos serving `GET /x` give two
    /// rows with one key.
    pub key: String,
    /// The entry point, located.
    pub entry: Located,
    /// Nodes the flow reaches (`hops.len()`), at least 1.
    pub reach: usize,
    /// `services` names more than one service.
    pub cross_service: bool,
    /// Distinct hop mechanisms (edge categories), in first-seen order.
    pub mechanisms: Vec<&'static str>,
    /// The services the flow touches, the entry's first, then in first-seen
    /// order along `hops`.
    pub services: Vec<String>,
    /// The entry's forward BFS tree, in discovery order: the hops a
    /// [`TraceAnswer::hops`] seeded at the entry would hold.
    pub hops: Vec<TraceHop>,
}

/// **cross_stack_trace** (P3): the ranked paths a feature takes across the
/// stack — see the module doc. Never an error: an unknown feature, an unknown
/// `to`, a dead end and an unreachable target are absences.
///
/// Deliberately takes NO `scope` (A8.3): a trace's entire value is that it
/// crosses service/directory boundaries, so filtering its hops would delete
/// the answer.
///
/// Each hop's `to_live` (LD.6) is read off one [`entrypoint_reachable`] walk,
/// run once per call; [`cross_stack_trace_with_live`] takes the set instead.
pub fn cross_stack_trace(merged: &MergedGraph, feature: &str, opts: &TraceOptions) -> TraceAnswer {
    cross_stack_trace_with_live(merged, &entrypoint_reachable(merged), feature, opts)
}

/// [`cross_stack_trace`] over a live set the caller already holds (pyo3's
/// `PyGraph` computes it once per graph): `live` must be
/// [`entrypoint_reachable`] of `merged`.
pub fn cross_stack_trace_with_live(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    feature: &str,
    opts: &TraceOptions,
) -> TraceAnswer {
    let mut seed = resolve(merged, feature);
    let adj = Adjacency::carry(merged, &CODE_PROFILE.tables);
    let succ = Successors::carry(merged);
    // The entry-flow tier (LD.4b, module doc): one-node mode, a dead end or
    // nothing resolved, and an entry key naming the feature.
    if opts.to.is_none()
        && !seed.id.is_some_and(|id| leaves(&succ, id))
        && let Some(entry) = entry_seed(merged, &adj, feature, opts.depth)
    {
        seed = Resolved {
            id: Some(entry),
            by: "entry_flow",
            near: Vec::new(),
        };
    }
    let Some(seed_id) = seed.id else {
        live_marker("trace", 0, 0);
        let absence = absence::unknown_symbol(merged, "trace", feature, &[], &seed.near);
        marker(feature, seed.by, 0, 0, false, opts, None);
        return TraceAnswer {
            seed: None,
            target: None,
            resolved_by: seed.by,
            hops: Vec::new(),
            paths: Vec::new(),
            truncated: false,
            absence: Some(absence),
        };
    };

    let loc = Locator::new(merged);
    let mut sides = Sides::new(merged, &loc);

    // The BFS tree (LD.15b), unchanged: each reached node is one hop, in
    // discovery order, from the node that first reached it.
    let hops: Vec<TraceHop> = reach::bfs(&adj, &[seed_id], Walk::Forward, opts.depth)
        .reached
        .iter()
        .map(|r| sides.hop(live, r.depth, r.parent, r.id, r.via))
        .collect();
    live_marker(
        "trace",
        hops.len(),
        hops.iter().filter(|h| h.to_live).count(),
    );

    let seed_at = loc.locate(seed_id);

    let Some(to_query) = opts.to.as_deref() else {
        let walk = enumerate(&succ, &mut sides, seed_id, None, opts);
        let paths = materialize(&succ, &mut sides, live, &walk.kept);
        let absence = paths.is_empty().then(|| {
            let note = if opts.depth == 0 && leaves(&succ, seed_id) {
                format!("depth 0 follows no edge from `{}`", seed_at.qname)
            } else {
                format!("no carry edge leaves `{}` in this graph", seed_at.qname)
            };
            absence::empty(
                merged,
                "trace",
                feature,
                "no_edges",
                note,
                mechanisms_of(merged, seed_id),
                seed_at.file.as_deref(),
            )
        });
        marker(
            feature,
            seed.by,
            paths.len(),
            walk.expanded,
            walk.truncated,
            opts,
            None,
        );
        return TraceAnswer {
            seed: Some(seed_at),
            target: None,
            resolved_by: seed.by,
            hops,
            paths,
            truncated: walk.truncated,
            absence,
        };
    };

    // Two-node mode.
    let target = resolve(merged, to_query);
    let Some(target_id) = target.id else {
        let absence = absence::unknown_symbol(merged, "trace", to_query, &[], &target.near);
        marker(feature, seed.by, 0, 0, false, opts, None);
        return TraceAnswer {
            seed: Some(seed_at),
            target: None,
            resolved_by: seed.by,
            hops,
            paths: Vec::new(),
            truncated: false,
            absence: Some(absence),
        };
    };
    let target_at = loc.locate(target_id);

    let (mut paths, expanded, truncated) = if seed_id == target_id {
        let zero = TracePath {
            rank: 1,
            hops: Vec::new(),
            cross_service_hops: 0,
            mechanisms: Vec::new(),
            length: 0,
            directed: true,
        };
        (vec![zero], 0, false)
    } else {
        // Hops left to the target, from a backward walk over the same carry
        // index: the enumeration never enters a node the target is out of
        // reach from.
        let mut to_target: HashMap<NodeId, usize> =
            reach::bfs(&adj, &[target_id], Walk::Backward, opts.depth)
                .reached
                .iter()
                .map(|r| (r.id, r.depth))
                .collect();
        to_target.insert(target_id, 0);
        let walk = enumerate(
            &succ,
            &mut sides,
            seed_id,
            Some((target_id, &to_target)),
            opts,
        );
        (
            materialize(&succ, &mut sides, live, &walk.kept),
            walk.expanded,
            walk.truncated,
        )
    };
    if paths.is_empty()
        && let Some(steps) = merged.shortest_path(seed_id, target_id, Reach::Both, None, opts.depth)
    {
        paths.push(undirected_path(&mut sides, live, &steps));
    }
    let directed = paths.first().map(|p| p.directed);
    let absence = paths.is_empty().then(|| {
        let mut mechanisms: Vec<&'static str> = Vec::new();
        for id in [seed_id, target_id] {
            for m in mechanisms_of(merged, id) {
                if !mechanisms.contains(m) {
                    mechanisms.push(m);
                }
            }
        }
        let note = format!(
            "no path from `{}` to `{}` within {} hops, over carry edges forward or any edge either way",
            seed_at.qname, target_at.qname, opts.depth
        );
        absence::empty(merged, "trace", feature, "no_edges", note, &mechanisms, seed_at.file.as_deref())
    });
    marker(
        feature,
        seed.by,
        paths.len(),
        expanded,
        truncated,
        opts,
        directed,
    );
    TraceAnswer {
        seed: Some(seed_at),
        target: Some(target_at),
        resolved_by: seed.by,
        hops,
        paths,
        truncated,
        absence,
    }
}

/// **entry_flows** (LD.4b): every entry point's forward flow — see the module
/// doc's "Entry flows". `repo_labels` is the build's
/// (`GenerateResult::repo_labels`); `services` names repos by it. Rows are
/// ordered by (key, entry qname, entry id) and every one is kept, so two
/// entries with one key are two rows. Entries reaching nothing within `depth`
/// have no row (`depth = 0` keeps none).
///
/// Each hop's `to_live` is read off one [`entrypoint_reachable`] walk, run
/// once per call; [`entry_flows_with_live`] takes the set instead.
pub fn entry_flows(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    depth: usize,
) -> Vec<EntryFlow> {
    entry_flows_with_live(merged, &entrypoint_reachable(merged), repo_labels, depth)
}

/// [`entry_flows`] over a live set the caller already holds (pyo3's `PyGraph`
/// computes it once per graph): `live` must be [`entrypoint_reachable`] of
/// `merged`.
pub fn entry_flows_with_live(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    repo_labels: &BTreeMap<u64, String>,
    depth: usize,
) -> Vec<EntryFlow> {
    let entries = entries(merged);
    let loc = Locator::new(merged);
    let mut sides = Sides::new(merged, &loc);
    let adj = Adjacency::carry(merged, &CODE_PROFILE.tables);
    let mut flows: Vec<EntryFlow> = Vec::new();
    for e in &entries {
        let reached = reach::bfs(&adj, &[e.id], Walk::Forward, depth).reached;
        if reached.is_empty() {
            continue;
        }
        let hops: Vec<TraceHop> = reached
            .iter()
            .map(|r| sides.hop(live, r.depth, r.parent, r.id, r.via))
            .collect();
        let mut mechanisms: Vec<&'static str> = Vec::new();
        for h in &hops {
            if !mechanisms.contains(&h.mechanism) {
                mechanisms.push(h.mechanism);
            }
        }
        let mut services: Vec<String> = Vec::new();
        for id in std::iter::once(e.id).chain(reached.iter().map(|r| r.id)) {
            if let Some(s) = sides.service_label(id, repo_labels)
                && !services.contains(&s)
            {
                services.push(s);
            }
        }
        flows.push(EntryFlow {
            key: slug(e.name),
            entry: loc.locate(e.id),
            reach: hops.len(),
            cross_service: services.len() > 1,
            mechanisms,
            services,
            hops,
        });
    }
    flows.sort_by(|a, b| {
        a.key
            .cmp(&b.key)
            .then_with(|| a.entry.qname.cmp(&b.entry.qname))
            .then_with(|| a.entry.id.cmp(&b.entry.id))
    });
    let (rows, live_rows) = flows.iter().fold((0, 0), |(n, l), f| {
        (
            n + f.hops.len(),
            l + f.hops.iter().filter(|h| h.to_live).count(),
        )
    });
    live_marker("flows", rows, live_rows);
    eprintln!(
        "[flows] entries={} flows={} cross_service={} depth<={depth}",
        entries.len(),
        flows.len(),
        flows.iter().filter(|f| f.cross_service).count()
    );
    flows
}

/// An entry key or a feature word as a slug: lower-cased, spaces and hyphens
/// as `_` (the repo-graph wrapper's spelling).
fn slug(s: &str) -> String {
    s.to_lowercase().replace([' ', '-'], "_")
}

/// One entry point: the node, its name and qname.
struct Entry<'a> {
    id: NodeId,
    name: &'a str,
    qname: &'a str,
}

/// Every entry point of `merged` — `CODE_PROFILE.tables.entry` over kind,
/// name and roles, the rule liveness seeds from — in graph then node order,
/// each id once.
fn entries(merged: &MergedGraph) -> Vec<Entry<'_>> {
    let rule = &CODE_PROFILE.tables.entry;
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut out: Vec<Entry<'_>> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            let name = g
                .nav
                .name_by_id
                .get(&n.id)
                .map(String::as_str)
                .unwrap_or("");
            let entry = rule.is_entry(kind, name, &[])
                || rule.is_entry(kind, name, &roles_in(kind, &n.cells));
            if entry && seen.insert(n.id) {
                let qname = g
                    .nav
                    .qname_by_id
                    .get(&n.id)
                    .map(String::as_str)
                    .unwrap_or("");
                out.push(Entry {
                    id: n.id,
                    name,
                    qname,
                });
            }
        }
    }
    out
}

/// The entry-flow tier's pick for `feature` (module doc): the entry whose key
/// matches the feature's slug — equal before containing, then the larger reach
/// within `depth`, then key, qname and id ascending — among the entries that
/// reach at least one node. Only the matching entries are walked.
fn entry_seed(
    merged: &MergedGraph,
    adj: &Adjacency,
    feature: &str,
    depth: usize,
) -> Option<NodeId> {
    let word = slug(feature.trim());
    if word.is_empty() {
        return None;
    }
    // (tier, reach, key, qname, id): tier 0 = equal key, 1 = containment.
    let mut best: Option<(u8, usize, String, &str, NodeId)> = None;
    for e in entries(merged) {
        let key = slug(e.name);
        if key.is_empty() {
            continue;
        }
        let tier = if key == word {
            0
        } else if key.contains(&word) || word.contains(&key) {
            1
        } else {
            continue;
        };
        let reach = reach::bfs(adj, &[e.id], Walk::Forward, depth).reached.len();
        if reach == 0 {
            continue;
        }
        let better = match &best {
            None => true,
            Some((t, r, k, q, id)) => {
                (
                    tier,
                    std::cmp::Reverse(reach),
                    key.as_str(),
                    e.qname,
                    e.id.0,
                ) < (*t, std::cmp::Reverse(*r), k.as_str(), *q, id.0)
            }
        };
        if better {
            best = Some((tier, reach, key, e.qname, e.id));
        }
    }
    best.map(|(.., id)| id)
}

/// The LD.4a fired_on line.
fn marker(
    q: &str,
    by: &str,
    paths: usize,
    expanded: usize,
    truncated: bool,
    opts: &TraceOptions,
    directed: Option<bool>,
) {
    let two_node = match (&opts.to, directed) {
        (Some(to), Some(d)) => format!(" to={to} directed={d}"),
        (Some(to), None) => format!(" to={to} directed=-"),
        (None, _) => String::new(),
    };
    eprintln!(
        "[trace] seed={q} resolved_by={by} paths={paths} expanded={expanded} truncated={truncated}{two_node}"
    );
}

/// A seed query, resolved: the node (if any), how, and find's nearest rows
/// (filled only when the exact tiers missed; they become the suggestions).
struct Resolved {
    id: Option<NodeId>,
    by: &'static str,
    near: Vec<FoundNode>,
}

/// Exact qname, then exact name (`answers::resolve_seed(merged, q, None)` is
/// exactly these two), then find's top hit in one of [`FIND_TIERS`].
fn resolve(merged: &MergedGraph, q: &str) -> Resolved {
    if let Some(id) = merged.node_id_by_qname(q) {
        return Resolved {
            id: Some(id),
            by: "qname",
            near: Vec::new(),
        };
    }
    if let Some(id) = merged.resolve_name(q) {
        return Resolved {
            id: Some(id),
            by: "name",
            near: Vec::new(),
        };
    }
    let opts = FindOptions {
        top_k: absence::SUGGESTIONS,
        ..FindOptions::default()
    };
    let near = find::search(merged, q, &opts).rows;
    let hit = near
        .first()
        .filter(|r| FIND_TIERS.contains(&r.r#match))
        .map(|r| NodeId(r.id));
    match hit {
        Some(id) => Resolved {
            id: Some(id),
            by: "find",
            near,
        },
        None => Resolved {
            id: None,
            by: "none",
            near,
        },
    }
}

/// The edge categories an answer about `id` depends on
/// (`absence::mechanisms_for_kind` of the kind the first graph naming it
/// records); none for an id no graph names.
fn mechanisms_of(merged: &MergedGraph, id: NodeId) -> &'static [&'static str] {
    let kind: Option<NodeKindId> = merged
        .graphs
        .iter()
        .find_map(|g| g.nav.kind_by_id.get(&id).copied());
    kind.map(absence::mechanisms_for_kind).unwrap_or(&[])
}

/// Does any carry edge leave `id`?
fn leaves(succ: &Successors, id: NodeId) -> bool {
    succ.index
        .get(&id)
        .is_some_and(|&ix| !succ.out[ix as usize].is_empty())
}

/// Per-node facts the hops need, computed once per node per call: the repo
/// (the pre-LD.4a `repo_of` map, a later graph overwriting an earlier one),
/// the qname, and the service key under `ProjectRoots` keying.
struct Sides<'a> {
    loc: &'a Locator<'a>,
    repo_of: HashMap<NodeId, u64>,
    qname_of: HashMap<NodeId, &'a str>,
    keying: ServiceKeying,
    /// `service_of` keys with no repo-label prefix: only equality matters.
    no_labels: BTreeMap<u64, String>,
    service: HashMap<NodeId, Option<String>>,
}

impl<'a> Sides<'a> {
    fn new(merged: &'a MergedGraph, loc: &'a Locator<'a>) -> Self {
        let mut repo_of: HashMap<NodeId, u64> = HashMap::new();
        let mut qname_of: HashMap<NodeId, &'a str> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                repo_of.insert(n.id, g.repo.0);
            }
            for (id, q) in &g.nav.qname_by_id {
                qname_of.entry(*id).or_insert(q.as_str());
            }
        }
        Sides {
            loc,
            repo_of,
            qname_of,
            keying: default_keying(merged),
            no_labels: BTreeMap::new(),
            service: HashMap::new(),
        }
    }

    fn qname(&self, id: NodeId) -> &'a str {
        self.qname_of.get(&id).copied().unwrap_or("")
    }

    /// `(cross_service, cross_repo)` for a hop `from -> to`.
    fn cross(&mut self, from: NodeId, to: NodeId) -> (bool, bool) {
        let cross_repo = self.repo_of.get(&from) != self.repo_of.get(&to);
        if cross_repo || !matches!(self.keying, ServiceKeying::ProjectRoots(_)) {
            return (cross_repo, cross_repo);
        }
        let a = self.service_key(from);
        let b = self.service_key(to);
        (matches!((a, b), (Some(a), Some(b)) if a != b), false)
    }

    /// The `glia arch` service `id` belongs to: keyed by its located file,
    /// else by its qname's owner segment; `None` when it has neither.
    fn service_key(&mut self, id: NodeId) -> Option<String> {
        if let Some(k) = self.service.get(&id) {
            return k.clone();
        }
        let file = self
            .loc
            .file_of(id)
            .or_else(|| endpoint::split_owner(self.qname(id)).1.map(str::to_string));
        let repo = self.repo_of.get(&id).copied().unwrap_or_default();
        let key = file.map(|f| service_of(&f, repo, &self.keying, &self.no_labels));
        self.service.insert(id, key.clone());
        key
    }

    /// The service `id` counts toward in an entry flow's `services`: under
    /// `ProjectRoots` keying the `glia arch` service of [`Self::service_key`]
    /// (one repo, so `service_of` never prefixes it with a label), else its
    /// repo's label from `labels` (`service_of`'s `repo<id>` when the build
    /// has none). `None` for a node no graph holds, or one `service_key`
    /// cannot place.
    fn service_label(&mut self, id: NodeId, labels: &BTreeMap<u64, String>) -> Option<String> {
        let repo = *self.repo_of.get(&id)?;
        if matches!(self.keying, ServiceKeying::ProjectRoots(_)) {
            return self.service_key(id);
        }
        Some(service_of("", repo, &ServiceKeying::PerRepo, labels))
    }

    /// One located hop.
    fn hop(
        &mut self,
        live: &HashSet<NodeId>,
        depth: usize,
        from: NodeId,
        to: NodeId,
        via: EdgeCategoryId,
    ) -> TraceHop {
        let (cross_service, cross_repo) = self.cross(from, to);
        let at = self.loc.locate(to);
        TraceHop {
            depth,
            mechanism: edge_category::name(via),
            cross_service,
            cross_repo,
            from_qname: self.loc.locate(from).qname,
            to_qname: at.qname,
            to_kind: at.kind,
            to_live: live.contains(&to),
            to_file: at.file,
            to_line: at.line,
        }
    }
}

/// The carry edges as a successor map over a dense index: each node's
/// distinct successors in global edge order, the first edge to a successor
/// speaking for it.
struct Successors {
    ids: Vec<NodeId>,
    index: HashMap<NodeId, u32>,
    out: Vec<Vec<(u32, EdgeCategoryId)>>,
}

impl Successors {
    fn carry(merged: &MergedGraph) -> Self {
        let keep = CategorySet::of(CODE_PROFILE.tables.carry_edges);
        let mut s = Successors {
            ids: Vec::new(),
            index: HashMap::new(),
            out: Vec::new(),
        };
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        for e in merged.all_edges() {
            if !keep.contains(e.category) {
                continue;
            }
            let from = s.intern(e.from);
            let to = s.intern(e.to);
            if seen.insert((from, to)) {
                s.out[from as usize].push((to, e.category));
            }
        }
        s
    }

    fn intern(&mut self, id: NodeId) -> u32 {
        if let Some(&ix) = self.index.get(&id) {
            return ix;
        }
        let ix = self.ids.len() as u32;
        self.index.insert(id, ix);
        self.ids.push(id);
        self.out.push(Vec::new());
        ix
    }
}

/// A path found by the enumeration, before it is located.
struct Candidate {
    /// Dense indices, seed first.
    nodes: Vec<u32>,
    cats: Vec<EdgeCategoryId>,
    cross_service_hops: usize,
    mechanisms: usize,
}

/// What [`enumerate`] found: the best paths (ranked, cut to `max_paths`),
/// the extensions it made, and whether the budget stopped it.
struct Walked {
    kept: Vec<Candidate>,
    expanded: usize,
    truncated: bool,
}

/// One depth-first frame: the node, the next successor to try, and whether
/// the walk ever extended the path past it.
struct Frame {
    ix: u32,
    cursor: usize,
    extended: bool,
}

/// Enumerate simple paths from `seed` by depth-first search (iterative, so a
/// deep `depth` cannot overflow the stack). One-node mode (`target = None`)
/// records every maximal path; two-node mode records the paths ending at the
/// target and never extends past it, entering only nodes whose hops-to-target
/// (`to_target`) fit in the hops left.
fn enumerate(
    succ: &Successors,
    sides: &mut Sides<'_>,
    seed: NodeId,
    target: Option<(NodeId, &HashMap<NodeId, usize>)>,
    opts: &TraceOptions,
) -> Walked {
    let mut walked = Walked {
        kept: Vec::new(),
        expanded: 0,
        truncated: false,
    };
    let Some(&seed_ix) = succ.index.get(&seed) else {
        return walked;
    };
    let target_ix = target.and_then(|(t, _)| succ.index.get(&t).copied());
    if target.is_some() && target_ix.is_none() {
        return walked;
    }
    // Hops-to-target per dense index, `None` = out of reach.
    let left: Option<Vec<Option<usize>>> =
        target.map(|(_, dist)| succ.ids.iter().map(|id| dist.get(id).copied()).collect());
    let prune_at = if opts.max_paths == 0 {
        usize::MAX
    } else {
        opts.max_paths.saturating_mul(4).max(1024)
    };

    let mut on_path = vec![false; succ.ids.len()];
    let mut frames = vec![Frame {
        ix: seed_ix,
        cursor: 0,
        extended: false,
    }];
    let mut cats: Vec<EdgeCategoryId> = Vec::new();
    // `crossed[i]` = cross-service hops among the first `i` hops.
    let mut crossed: Vec<usize> = vec![0];
    on_path[seed_ix as usize] = true;
    walked.expanded = 1;

    while let Some(fi) = frames.len().checked_sub(1) {
        let len = fi; // hops on the path so far
        let node = frames[fi].ix;
        let mut next = None;
        if len < opts.depth && Some(node) != target_ix {
            let out = &succ.out[node as usize];
            while frames[fi].cursor < out.len() {
                let (to, cat) = out[frames[fi].cursor];
                frames[fi].cursor += 1;
                if on_path[to as usize] {
                    continue;
                }
                if let Some(left) = &left {
                    match left[to as usize] {
                        Some(d) if len + 1 + d <= opts.depth => {}
                        _ => continue,
                    }
                }
                next = Some((to, cat));
                break;
            }
        }
        match next {
            Some((to, cat)) => {
                if walked.expanded >= EXPANSION_BUDGET {
                    walked.truncated = true;
                    break;
                }
                walked.expanded += 1;
                frames[fi].extended = true;
                let (svc, _) = sides.cross(succ.ids[node as usize], succ.ids[to as usize]);
                crossed.push(crossed[len] + usize::from(svc));
                cats.push(cat);
                on_path[to as usize] = true;
                frames.push(Frame {
                    ix: to,
                    cursor: 0,
                    extended: false,
                });
            }
            None => {
                let done = &frames[fi];
                let record = len > 0
                    && match target_ix {
                        Some(t) => done.ix == t,
                        None => !done.extended,
                    };
                if record {
                    let mut seen: Vec<EdgeCategoryId> = Vec::new();
                    for c in &cats {
                        if !seen.contains(c) {
                            seen.push(*c);
                        }
                    }
                    walked.kept.push(Candidate {
                        nodes: frames.iter().map(|f| f.ix).collect(),
                        cats: cats.clone(),
                        cross_service_hops: crossed[len],
                        mechanisms: seen.len(),
                    });
                    if walked.kept.len() >= prune_at {
                        rank(&mut walked.kept, succ, sides, opts.max_paths);
                    }
                }
                on_path[node as usize] = false;
                frames.pop();
                if fi > 0 {
                    cats.pop();
                    crossed.pop();
                }
            }
        }
    }
    rank(&mut walked.kept, succ, sides, opts.max_paths);
    walked
}

/// Sort by the rank key (module doc) and keep the first `max_paths`
/// (`0` keeps all).
fn rank(cands: &mut Vec<Candidate>, succ: &Successors, sides: &Sides<'_>, max_paths: usize) {
    cands.sort_by(|a, b| {
        b.cross_service_hops
            .cmp(&a.cross_service_hops)
            .then_with(|| b.mechanisms.cmp(&a.mechanisms))
            .then_with(|| b.cats.len().cmp(&a.cats.len()))
            .then_with(|| {
                let qa = a.nodes.iter().map(|&ix| sides.qname(succ.ids[ix as usize]));
                let qb = b.nodes.iter().map(|&ix| sides.qname(succ.ids[ix as usize]));
                qa.cmp(qb)
            })
            .then_with(|| {
                let ia = a.nodes.iter().map(|&ix| succ.ids[ix as usize].0);
                let ib = b.nodes.iter().map(|&ix| succ.ids[ix as usize].0);
                ia.cmp(ib)
            })
    });
    if max_paths > 0 {
        cands.truncate(max_paths);
    }
}

/// Locate the kept candidates into ranked [`TracePath`]s.
fn materialize(
    succ: &Successors,
    sides: &mut Sides<'_>,
    live: &HashSet<NodeId>,
    kept: &[Candidate],
) -> Vec<TracePath> {
    kept.iter()
        .enumerate()
        .map(|(i, c)| {
            let hops: Vec<TraceHop> = c
                .nodes
                .windows(2)
                .zip(&c.cats)
                .enumerate()
                .map(|(d, (w, &cat))| {
                    sides.hop(
                        live,
                        d + 1,
                        succ.ids[w[0] as usize],
                        succ.ids[w[1] as usize],
                        cat,
                    )
                })
                .collect();
            path_of(i + 1, hops, true)
        })
        .collect()
}

/// The two-node fallback: `MergedGraph::shortest_path`'s steps as one
/// undirected path, each hop's mechanism the category it traversed.
fn undirected_path(
    sides: &mut Sides<'_>,
    live: &HashSet<NodeId>,
    steps: &[(NodeId, Option<EdgeCategoryId>)],
) -> TracePath {
    // Every step after the first names the category that entered it.
    let hops: Vec<TraceHop> = steps
        .windows(2)
        .filter_map(|w| Some((w[0].0, w[1].0, w[1].1?)))
        .enumerate()
        .map(|(d, (from, to, via))| sides.hop(live, d + 1, from, to, via))
        .collect();
    path_of(1, hops, false)
}

fn path_of(rank: usize, hops: Vec<TraceHop>, directed: bool) -> TracePath {
    let mut mechanisms: Vec<&'static str> = Vec::new();
    for h in &hops {
        if !mechanisms.contains(&h.mechanism) {
            mechanisms.push(h.mechanism);
        }
    }
    TracePath {
        rank,
        cross_service_hops: hops.iter().filter(|h| h.cross_service).count(),
        length: hops.len(),
        mechanisms,
        hops,
        directed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options() {
        let o = TraceOptions::default();
        assert_eq!((o.depth, o.to.as_deref(), o.max_paths), (6, None, 10));
    }
}
