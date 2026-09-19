//! `effects(A)` (LE.4d): what a node does to the outside world. The effect
//! sinks downstream of one or more named nodes (a DB read or write, a queue
//! produce, an outbound HTTP call, an event emit and the other outbound
//! markers), each with a located witness path, the receivers one flow hop past
//! it, and (for data access) the SQL verb.
//!
//! Search cannot answer it (it is transitive: `placeOrder -> saveOrder ->
//! orders`) and blast radius answers a different question (everything
//! downstream, helpers included). This is the downstream closure filtered to
//! the domain's effect table.
//!
//! # Sinks
//!
//! The table is the domain profile's (`CODE_PROFILE.tables.effect_sinks`,
//! `code_domain::profile::CODE_TABLES`): one class per (target kind, reaching
//! category) pair, first match in table order. A node is a sink only when the
//! walk REACHES it over one of its class's categories: a queue producer the
//! walk enters over USES is a `queue_produce`, the same node entered some
//! other way is not.
//!
//! # Seeds
//!
//! Each name is a qname, a dotted path or an exact simple name, resolved
//! through `find`'s exact tiers to the one node `node_id_by_qname` /
//! `resolve_name` would pick (a name no node has goes to `unresolved`). At
//! most [`MAX_SEEDS`] names: more is an `Err`, never a cut, and there is no
//! wildcard or "every route" mode. A CONFIG_KEY seed is a config flow: the
//! sources of the READS_CONFIG edges into it (the functions reading it, since
//! LE.4b; a module for a key read at module scope) become the walk's seeds,
//! and each row they reach carries `via_config` = the key. Walk seeds are
//! deduplicated and ordered by (qname, id); a node named directly keeps
//! `via_config` unset even when a named key's readers include it.
//!
//! # The walk
//!
//! ONE forward `algo::reach::bfs` from every seed at once, `max_depth` hops,
//! over an [`Adjacency`] of the profile's carry categories minus TESTS,
//! DOCUMENTS, SHARES_SCHEMA and SHARES_DATA_ENTITY (a function's effects are
//! not its tests, its docs, or another repo's copy of a table), and minus the
//! flow categories ([`FLOW`]) unless `cross_service`. Each row's `seed` is
//! the seed whose wave reached it first.
//!
//! A sink is recorded and NOT expanded: the walk's graph routes every edge
//! that makes a sink (its target's kind and its category match a class) to a
//! sink-only twin of the target, which has no out-edge of its own. The twin
//! is a separate node, so the same target entered some other way is walked
//! normally. With `cross_service` the twin carries the target's [`FLOW`]
//! out-edges, so the walk continues into the receiving handler and on (a
//! producer -QUEUE_FLOWS-> consumer -HANDLED_BY-> handler -CALLS-> ...), and
//! each row reports `services_crossed`: how many times `arch::service_of`
//! (under `arch::default_keying`, as `glia arch` keys services) changes along
//! the path, over the placed nodes before the sink (a data entity is placed
//! wherever its first reference sits, so the sink itself never counts).
//!
//! # Rows
//!
//! One row per sink node: class, identity and 1-based location (one LD.1
//! `Locator`), depth, seed, `path` (the BFS parent pointers from the seed,
//! each hop explained by LC.3a's evidence site of the first edge of that
//! (from, to, category) in global edge order), `downstream` (the sink's
//! receivers one [`FLOW`] hop away, located, in edge order, in either mode)
//! and `mode`: the LE.4a ACCESS_MODE cell folded over every edge the walk
//! reaches the sink by (from each expanded node, parallel edges included):
//! read + write is `read_write`, `None` when no reaching edge names a verb.
//! `tier` is always `derived`: an effect is composed from edges. Rows are
//! sorted by (class in table order, depth, qname); ties keep BFS discovery
//! order.
//!
//! Filters apply to the rows, never the walk (a filtered-out sink still stops
//! it): `classes` keeps the named classes, `writes_only` keeps db rows whose
//! mode is `write` / `read_write` plus every other class (a send is a write),
//! `scope` keeps the rows whose sink sits under a path or project label (an
//! unlocatable sink is kept, the A8.3 rule). `counts` holds every class of
//! the table; `writes` counts the kept rows that write.
//!
//! `absence` (LD.8a) is `Some` exactly when `effects` is empty: every name
//! unresolved (`unknown_symbol`), a config key nothing reads, every row
//! filtered away (`no_match`, or the scope's `scope_emptied`), or no sink
//! reached (`no_edges`, mechanisms = the sink table's categories, caveats for
//! the seeds' language when they share one).
//!
//! # Security
//!
//! Structural reachability over edges the build already holds, from nodes the
//! caller names. No auth or middleware dimension, no route enumeration, no
//! value or taint flow: ACCESS_MODE is the SQL verb at the access site.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::effects::<item>`, never flattened into the
//! crate root.
//!
//! fired_on marker, one line per answered call:
//! `[effects] seeds=<S> reached=<R> effects=<E> (db=<a> queue_produce=<b> http_call=<c> event_emit=<d> other=<o>) writes=<W> config_seeds=<C>`
//! — grep `^\[effects\] seeds=`.

use std::collections::{BTreeMap, HashMap, HashSet};

use repo_graph_activation::algo::{Adjacency, CategorySet, GraphSource, Walk, reach};
use repo_graph_code_domain::evidence::Evidence;
use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, Edge, EdgeCategoryId, NodeId, NodeKindId};
use repo_graph_graph::MergedGraph;

use crate::absence::{self, Absence};
use crate::answers::{Locator, in_scope, resolve_scope};
use crate::arch::{default_keying, service_of};
use crate::coverage::ext_to_language;
use crate::find::{self, FindOptions, FoundNode};
use crate::profile::CODE_PROFILE;

/// [`EffectsArgs::default`]'s `max_depth`.
pub const DEFAULT_MAX_DEPTH: usize = 8;

/// The most names one answer takes; more is an error.
pub const MAX_SEEDS: usize = 64;

/// The flow categories past a sink: one hop names its receivers
/// (`downstream`); with `cross_service` the walk continues along them.
pub const FLOW: [EdgeCategoryId; 7] = [
    edge_category::QUEUE_FLOWS,
    edge_category::HTTP_CALLS,
    edge_category::EVENT_FLOWS,
    edge_category::GRPC_CALLS,
    edge_category::RPC_CALLS,
    edge_category::WS_CONNECTS,
    edge_category::GRAPHQL_CALLS,
];

/// Carry categories the walk leaves out (module docs).
const NOT_EFFECTS: [EdgeCategoryId; 4] = [
    edge_category::TESTS,
    edge_category::DOCUMENTS,
    edge_category::SHARES_SCHEMA,
    edge_category::SHARES_DATA_ENTITY,
];

/// The data-access class, the one whose rows `writes_only` reads the mode of.
pub const DB: &str = "db";

/// The primitive name in the `[absence]` marker.
const PRIMITIVE: &str = "effects";

const DERIVED: &str = "derived";

/// A twin id is the real id XOR this, stepped until it is no id of the graph.
const TWIN_SALT: u64 = 0x5eed_e44e_c75f_0a1d;
const TWIN_STEP: u64 = 0x9e37_79b9_7f4a_7c15;

/// How [`effects`] walks and what it keeps. Start from `default()` and set
/// fields: `#[non_exhaustive]` rules out a struct literal outside this crate.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct EffectsArgs {
    /// Hops of the forward walk ([`DEFAULT_MAX_DEPTH`]).
    pub max_depth: usize,
    /// Keep only these classes (names of the sink table, any case).
    pub classes: Option<Vec<String>>,
    /// Keep db rows that write, and every other class.
    pub writes_only: bool,
    /// Continue past each sink along its flow edges into the receivers.
    pub cross_service: bool,
    /// Keep only rows whose sink sits under this path or project label.
    pub scope: Option<String>,
}

impl Default for EffectsArgs {
    fn default() -> Self {
        EffectsArgs {
            max_depth: DEFAULT_MAX_DEPTH,
            classes: None,
            writes_only: false,
            cross_service: false,
            scope: None,
        }
    }
}

/// One hop of a witness path: its ends, category and the 1-based site the
/// edge was asserted at (LC.3a evidence), when recorded.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct EffectHop {
    pub from_qname: String,
    pub to_qname: String,
    pub category: &'static str,
    pub site_file: Option<String>,
    /// 1-based.
    pub site_line: Option<i64>,
}

/// A receiver one flow hop past a sink (a queue consumer, the ROUTE an
/// endpoint calls, ...).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct EffectTarget {
    pub qname: String,
    pub kind: &'static str,
    /// The flow category, e.g. `QUEUE_FLOWS`.
    pub category: &'static str,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
}

/// One effect: the sink node, its class, how it is reached and what lies past
/// it (module docs).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct EffectRow {
    /// The sink table's class, e.g. `db`, `queue_produce`, `http_call`.
    pub class: &'static str,
    pub qname: String,
    pub name: String,
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// `read` | `write` | `read_write` (ACCESS_MODE, folded); `None` when no
    /// reaching edge names a verb.
    pub mode: Option<String>,
    pub depth: usize,
    /// The seed qname whose wave reached the sink first.
    pub seed: String,
    /// The CONFIG_KEY seed whose reader `seed` is, when the seed came from one.
    pub via_config: Option<String>,
    pub services_crossed: usize,
    pub downstream: Vec<EffectTarget>,
    /// From the seed to the sink, one hop per edge.
    pub path: Vec<EffectHop>,
    /// Always `derived`.
    pub tier: &'static str,
}

/// [`effects`]'s answer.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct Effects {
    /// The walk's seeds (config keys expanded to their readers), ordered by
    /// (qname, id).
    pub seeds: Vec<String>,
    pub effects: Vec<EffectRow>,
    /// Kept rows per class, every class of the sink table present.
    pub counts: BTreeMap<&'static str, usize>,
    /// Kept rows that write: db rows with mode `write` / `read_write`, and
    /// every row of another class.
    pub writes: usize,
    /// Names given that no node has.
    pub unresolved: Vec<String>,
    /// LD.8a; `Some` iff `effects` is empty.
    pub absence: Option<Absence>,
}

/// The effects of the nodes `qnames` name (module docs). `repo_labels` keys
/// services as `glia arch` does (`GenerateResult::repo_labels`).
///
/// Errors: no name given, more than [`MAX_SEEDS`] names, an unknown or empty
/// class filter.
///
/// Cost: one find search per name, O(V) node indexes and one `Locator`, one
/// O(E) walk-graph build and `Adjacency`, one BFS, one O(E) scan for the
/// modes, hop evidence and receivers.
pub fn effects(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    qnames: &[&str],
    args: &EffectsArgs,
) -> Result<Effects, String> {
    let names: Vec<&str> = {
        let mut seen = HashSet::new();
        qnames
            .iter()
            .map(|q| q.trim())
            .filter(|q| !q.is_empty() && seen.insert(*q))
            .collect()
    };
    if names.is_empty() {
        return Err("effects: no seed qname given".to_string());
    }
    if names.len() > MAX_SEEDS {
        return Err(format!(
            "effects: {} seed names, more than the {MAX_SEEDS} one answer takes; name fewer",
            names.len()
        ));
    }
    let tables = &CODE_PROFILE.tables;
    let keep_classes = class_filter(args.classes.as_deref())?;

    let index = NodeIndex::build(merged);
    let loc = Locator::new(merged);

    // Resolve the names; expand config keys to their readers.
    let opts = FindOptions {
        top_k: absence::SUGGESTIONS,
        ..FindOptions::default()
    };
    let mut named: Vec<NodeId> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut near: Option<Vec<FoundNode>> = None;
    for q in &names {
        let rows = find::search(merged, q, &opts).rows;
        match rows.first().filter(|r| find::is_exact(r)) {
            Some(r) => named.push(NodeId(r.id)),
            None => {
                unresolved.push(q.to_string());
                near.get_or_insert(rows);
            }
        }
    }
    let config_keys: Vec<NodeId> = named
        .iter()
        .copied()
        .filter(|id| index.kind(*id) == Some(node_kind::CONFIG_KEY))
        .collect();
    let mut via_config: HashMap<NodeId, NodeId> = HashMap::new();
    if !config_keys.is_empty() {
        let keys: HashSet<NodeId> = config_keys.iter().copied().collect();
        let mut readers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for e in merged.all_edges() {
            if e.category == edge_category::READS_CONFIG && keys.contains(&e.to) {
                readers.entry(e.to).or_default().push(e.from);
            }
        }
        // The first named key a reader reads, in name order.
        for key in &config_keys {
            for r in readers.get(key).into_iter().flatten() {
                via_config.entry(*r).or_insert(*key);
            }
        }
    }
    let mut seeds: Vec<(String, NodeId)> = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    let direct = named.iter().filter(|id| !config_keys.contains(id));
    for &id in direct.chain(via_config.keys()) {
        if seen.insert(id) {
            seeds.push((loc.locate(id).qname, id));
        }
    }
    for id in named.iter() {
        via_config.remove(id);
    }
    seeds.sort_by(|a, b| (&a.0, a.1.0).cmp(&(&b.0, b.1.0)));
    let seed_ids: Vec<NodeId> = seeds.iter().map(|(_, id)| *id).collect();

    // The walk.
    // Every FLOW category is a carry category (a unit test pins it).
    let walk: Vec<EdgeCategoryId> = tables
        .carry_edges
        .iter()
        .copied()
        .filter(|c| !NOT_EFFECTS.contains(c) && (args.cross_service || !FLOW.contains(c)))
        .collect();
    let graph = WalkGraph::build(merged, &index, &walk, args.cross_service);
    let adj = Adjacency::build(&graph, &CategorySet::of(&walk));
    let bfs = reach::bfs(&adj, &seed_ids, Walk::Forward, args.max_depth);
    let parent: HashMap<NodeId, (NodeId, EdgeCategoryId)> = bfs
        .reached
        .iter()
        .map(|r| (r.id, (r.parent, r.via)))
        .collect();
    let is_seed: HashSet<NodeId> = seed_ids.iter().copied().collect();

    // Each sink twin reached: its real node, class and the chain back to its
    // seed, as real `(from, to, category)` hops.
    struct Hit {
        real: NodeId,
        sink: Sink,
        depth: usize,
        seed: NodeId,
        hops: Vec<(NodeId, NodeId, EdgeCategoryId)>,
    }
    let mut hits: Vec<Hit> = Vec::new();
    let mut expanded: HashSet<NodeId> = is_seed.clone();
    for r in &bfs.reached {
        let Some(&(real, sink)) = graph.twins.get(&r.id) else {
            if r.depth < args.max_depth {
                expanded.insert(r.id);
            }
            continue;
        };
        let mut hops = Vec::new();
        let mut cur = r.id;
        // Every parent was discovered before its child and the chain ends at
        // a seed, so this walks at most `reached.len()` steps.
        while !is_seed.contains(&cur) {
            let Some(&(p, via)) = parent.get(&cur) else {
                break;
            };
            hops.push((graph.real(p), graph.real(cur), via));
            cur = p;
        }
        hops.reverse();
        hits.push(Hit {
            real,
            sink,
            depth: r.depth,
            seed: cur,
            hops,
        });
    }

    // One scan: hop evidence, the reaching edges' modes, the receivers.
    let sink_of: HashMap<NodeId, Sink> = hits.iter().map(|h| (h.real, h.sink)).collect();
    let wanted: HashSet<(NodeId, NodeId, EdgeCategoryId)> =
        hits.iter().flat_map(|h| h.hops.iter().copied()).collect();
    let mut first: HashMap<(NodeId, NodeId, EdgeCategoryId), &Edge> = HashMap::new();
    let mut modes: HashMap<NodeId, Option<&'static str>> = HashMap::new();
    let mut receivers: HashMap<NodeId, Vec<(NodeId, EdgeCategoryId)>> = HashMap::new();
    for e in merged.all_edges() {
        let key = (e.from, e.to, e.category);
        if wanted.contains(&key) {
            first.entry(key).or_insert(e);
        }
        if let Some(&sink) = sink_of.get(&e.to)
            && expanded.contains(&e.from)
            && graph.sink_for(e.to, e.category) == Some(sink)
        {
            let m = modes.entry(e.to).or_insert(None);
            *m = fold_mode(*m, mode_of(e));
        }
        if FLOW.contains(&e.category) && sink_of.contains_key(&e.from) {
            let v = receivers.entry(e.from).or_default();
            if !v.contains(&(e.to, e.category)) {
                v.push((e.to, e.category));
            }
        }
    }

    let keying = default_keying(merged);
    let service = |id: NodeId| -> Option<String> {
        let file = loc.file_of(id)?;
        let repo = index.repo(id)?;
        Some(service_of(&file, repo, &keying, repo_labels))
    };
    let mut qnames_of: HashMap<NodeId, String> = HashMap::new();
    let mut qname = |id: NodeId| {
        qnames_of
            .entry(id)
            .or_insert_with(|| loc.locate(id).qname)
            .clone()
    };
    let mut rows: Vec<(usize, EffectRow)> = Vec::new();
    for h in &hits {
        let at = loc.locate(h.real);
        let path: Vec<EffectHop> = h
            .hops
            .iter()
            .map(|&(from, to, cat)| {
                let (site_file, site_line) = first
                    .get(&(from, to, cat))
                    .map(|e| site_of(&loc, e))
                    .unwrap_or((None, None));
                EffectHop {
                    from_qname: qname(from),
                    to_qname: qname(to),
                    category: edge_category::name(cat),
                    site_file,
                    site_line,
                }
            })
            .collect();
        // The placed nodes before the sink, in path order.
        let mut services: Vec<String> = Vec::new();
        for id in h.hops.iter().map(|&(from, _, _)| from) {
            if let Some(s) = service(id) {
                services.push(s);
            }
        }
        let services_crossed = services.windows(2).filter(|w| w[0] != w[1]).count();
        let downstream: Vec<EffectTarget> = receivers
            .get(&h.real)
            .into_iter()
            .flatten()
            .map(|&(to, cat)| {
                let r = loc.locate(to);
                EffectTarget {
                    qname: r.qname,
                    kind: r.kind,
                    category: edge_category::name(cat),
                    file: r.file,
                    line: r.line,
                }
            })
            .collect();
        let row = EffectRow {
            class: h.sink.1,
            qname: at.qname,
            name: at.name,
            kind: at.kind,
            file: at.file,
            line: at.line,
            mode: modes.get(&h.real).copied().flatten().map(str::to_string),
            depth: h.depth,
            seed: qname(h.seed),
            via_config: via_config.get(&h.seed).map(|k| qname(*k)),
            services_crossed,
            downstream,
            path,
            tier: DERIVED,
        };
        rows.push((h.sink.0, row));
    }
    rows.sort_by(|a, b| (a.0, a.1.depth, &a.1.qname).cmp(&(b.0, b.1.depth, &b.1.qname)));
    let reached_rows = rows.len();
    let mut rows: Vec<EffectRow> = rows.into_iter().map(|(_, r)| r).collect();

    // Filters: class, writes, then scope.
    if let Some(keep) = &keep_classes {
        rows.retain(|r| keep.contains(&r.class));
    }
    if args.writes_only {
        rows.retain(is_write);
    }
    let before_scope = rows.len();
    if let Some(raw) = args.scope.as_deref() {
        let scope = resolve_scope(merged, raw);
        rows.retain(|r| r.file.as_deref().is_none_or(|f| in_scope(f, &scope)));
    }

    let mut counts: BTreeMap<&'static str, usize> =
        tables.effect_sinks.iter().map(|s| (s.class, 0)).collect();
    for r in &rows {
        *counts.entry(r.class).or_insert(0) += 1;
    }
    let writes = rows.iter().filter(|r| is_write(r)).count();
    let count = |c: &str| counts.get(c).copied().unwrap_or(0);
    let named_classes = [DB, "queue_produce", "http_call", "event_emit"];
    let other = rows
        .len()
        .saturating_sub(named_classes.iter().map(|c| count(c)).sum::<usize>());
    eprintln!(
        "[effects] seeds={} reached={} effects={} (db={} queue_produce={} http_call={} event_emit={} other={other}) writes={writes} config_seeds={}",
        seeds.len(),
        bfs.reached.len(),
        rows.len(),
        count(DB),
        count("queue_produce"),
        count("http_call"),
        count("event_emit"),
        config_keys.len()
    );

    let query = names.join(", ");
    let absence = if !rows.is_empty() {
        None
    } else if named.is_empty() {
        let first = unresolved
            .first()
            .map(String::as_str)
            .unwrap_or(query.as_str());
        let mechanisms = sink_mechanisms();
        Some(absence::unknown_symbol(
            merged,
            PRIMITIVE,
            first,
            &mechanisms,
            near.as_deref().unwrap_or(&[]),
        ))
    } else if seeds.is_empty() {
        let keys: Vec<String> = config_keys
            .iter()
            .map(|k| format!("`{}`", qname(*k)))
            .collect();
        let note = format!(
            "no node reads {} in this graph (no READS_CONFIG edge into it)",
            keys.join(", ")
        );
        Some(absence::empty(
            merged,
            PRIMITIVE,
            &query,
            "no_edges",
            note,
            &[edge_category::name(edge_category::READS_CONFIG)],
            None,
        ))
    } else if before_scope > 0
        && let Some(raw) = args.scope.as_deref()
    {
        Some(absence::scope_emptied(
            merged,
            PRIMITIVE,
            &query,
            before_scope,
            raw,
        ))
    } else if reached_rows > 0 {
        let what = match (&keep_classes, args.writes_only) {
            (Some(k), true) => format!("of class {} that writes", k.join(", ")),
            (Some(k), false) => format!("of class {}", k.join(", ")),
            _ => "that writes".to_string(),
        };
        let note = format!(
            "{reached_rows} {} reached, none {what}",
            absence::plural(reached_rows, "effect", "effects")
        );
        Some(absence::empty(
            merged,
            PRIMITIVE,
            &query,
            "no_match",
            note,
            &[],
            None,
        ))
    } else {
        let listed: Vec<String> = seeds.iter().map(|(q, _)| format!("`{q}`")).collect();
        let note = format!(
            "no effect sink is reachable from {} within {} {}",
            listed.join(", "),
            args.max_depth,
            absence::plural(args.max_depth, "hop", "hops")
        );
        let mechanisms = sink_mechanisms();
        let seed_file = one_language_file(&loc, &seed_ids);
        Some(absence::empty(
            merged,
            PRIMITIVE,
            &query,
            "no_edges",
            note,
            &mechanisms,
            seed_file.as_deref(),
        ))
    };

    Ok(Effects {
        seeds: seeds.into_iter().map(|(q, _)| q).collect(),
        effects: rows,
        counts,
        writes,
        unresolved,
        absence,
    })
}

/// The class names of the sink table, in table order.
pub fn effect_classes() -> Vec<&'static str> {
    CODE_PROFILE
        .tables
        .effect_sinks
        .iter()
        .map(|s| s.class)
        .collect()
}

/// The requested classes as table spellings; `None` keeps every class. An
/// empty list or a name the table has not is an error listing the valid ones.
fn class_filter(classes: Option<&[String]>) -> Result<Option<Vec<&'static str>>, String> {
    let Some(asked) = classes else {
        return Ok(None);
    };
    let valid = effect_classes();
    if asked.iter().all(|c| c.trim().is_empty()) {
        return Err(format!(
            "effects: the class filter is empty; valid: {}",
            valid.join(", ")
        ));
    }
    let mut keep: Vec<&'static str> = Vec::new();
    for c in asked.iter().map(|c| c.trim()).filter(|c| !c.is_empty()) {
        let Some(hit) = valid.iter().find(|v| v.eq_ignore_ascii_case(c)) else {
            return Err(format!(
                "unknown effect class `{c}`; valid: {}",
                valid.join(", ")
            ));
        };
        if !keep.contains(hit) {
            keep.push(hit);
        }
    }
    Ok(Some(keep))
}

/// A write: a db row whose mode writes, or a row of any other class.
fn is_write(r: &EffectRow) -> bool {
    r.class != DB || matches!(r.mode.as_deref(), Some("write" | "read_write"))
}

/// Every category the sink table reaches its sinks by, deduplicated, in table
/// order: the mechanisms an empty answer depended on.
fn sink_mechanisms() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for s in CODE_PROFILE.tables.effect_sinks {
        for c in s.via {
            let n = edge_category::name(*c);
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// A seed file to narrow an absence's caveats by: the first located seed's,
/// when every located seed is in that one language; `None` (every language
/// in the graph) otherwise.
fn one_language_file(loc: &Locator<'_>, seeds: &[NodeId]) -> Option<String> {
    let files: Vec<String> = seeds.iter().filter_map(|id| loc.file_of(*id)).collect();
    let first = files.first()?;
    let lang = ext_to_language(first)?;
    files
        .iter()
        .all(|f| ext_to_language(f) == Some(lang))
        .then(|| first.clone())
}

/// The LC.3a site an edge was asserted at, 1-based: its evidence file (the
/// caller's file when only the line was recorded) and line.
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

/// The ACCESS_MODE an edge carries (LE.4a), as its `'static` spelling.
fn mode_of(e: &Edge) -> Option<&'static str> {
    let cell = e.cell(cell_type::ACCESS_MODE)?;
    let (CellPayload::Text(t) | CellPayload::Json(t)) = &cell.payload else {
        return None;
    };
    ["read", "write", "read_write"]
        .into_iter()
        .find(|m| *m == t.as_str())
}

/// Fold two modes: equal stays, read + write is `read_write`, `None` (the
/// verb unknown) adds nothing.
fn fold_mode(a: Option<&'static str>, b: Option<&'static str>) -> Option<&'static str> {
    match (a, b) {
        (None, m) | (m, None) => m,
        (Some(x), Some(y)) if x == y => Some(x),
        _ => Some("read_write"),
    }
}

/// A sink table row: its index (the row sort key) and its class.
type Sink = (usize, &'static str);

/// Each node's kind and repo, from the first graph (in `merged.graphs` order)
/// that names it, as `Locator` reads them.
struct NodeIndex {
    at: HashMap<NodeId, (NodeKindId, u64)>,
}

impl NodeIndex {
    fn build(merged: &MergedGraph) -> Self {
        let mut at: HashMap<NodeId, (NodeKindId, u64)> = HashMap::new();
        for g in &merged.graphs {
            for (id, kind) in &g.nav.kind_by_id {
                at.entry(*id).or_insert((*kind, g.repo.0));
            }
        }
        NodeIndex { at }
    }

    fn kind(&self, id: NodeId) -> Option<NodeKindId> {
        self.at.get(&id).map(|(k, _)| *k)
    }

    fn repo(&self, id: NodeId) -> Option<u64> {
        self.at.get(&id).map(|(_, r)| *r)
    }
}

/// The walk's graph (module docs): the merge's edges of the walk categories,
/// with every edge that makes a sink routed to the target's sink-only twin;
/// with `cross_service`, each twin also carries its target's flow out-edges.
struct WalkGraph<'a> {
    index: &'a NodeIndex,
    nodes: Vec<NodeId>,
    edges: Vec<Edge>,
    /// Twin id -> (the real sink node, its sink table index).
    twins: HashMap<NodeId, (NodeId, Sink)>,
}

impl<'a> WalkGraph<'a> {
    fn build(
        merged: &'a MergedGraph,
        index: &'a NodeIndex,
        walk: &[EdgeCategoryId],
        cross_service: bool,
    ) -> Self {
        let nodes = GraphSource::node_ids(merged);
        let mut taken: HashSet<NodeId> = nodes.iter().copied().collect();
        for e in merged.all_edges() {
            taken.insert(e.from);
            taken.insert(e.to);
        }
        let mut g = WalkGraph {
            index,
            nodes,
            edges: Vec::new(),
            twins: HashMap::new(),
        };
        let mut twin_of: HashMap<NodeId, NodeId> = HashMap::new();
        let mut order: Vec<NodeId> = Vec::new();
        let walk = CategorySet::of(walk);
        for e in merged.all_edges().filter(|e| walk.contains(e.category)) {
            let to = g.target(e, &mut taken, &mut twin_of, &mut order);
            g.push(e, e.from, to);
        }
        if cross_service {
            let flows: Vec<&Edge> = merged
                .all_edges()
                .filter(|e| FLOW.contains(&e.category))
                .collect();
            // Rounds, so a twin a flow edge makes gets its own flow edges too.
            let mut done = 0;
            while done < order.len() {
                let round: HashSet<NodeId> = order[done..]
                    .iter()
                    .filter_map(|t| g.twins.get(t).map(|(real, _)| *real))
                    .collect();
                done = order.len();
                for e in flows.iter().filter(|e| round.contains(&e.from)) {
                    let Some(&from) = twin_of.get(&e.from) else {
                        continue;
                    };
                    let to = g.target(e, &mut taken, &mut twin_of, &mut order);
                    g.push(e, from, to);
                }
            }
        }
        g.nodes.extend(order);
        g
    }

    /// The sink that reaching `to` over `category` is, if any.
    fn sink_for(&self, to: NodeId, category: EdgeCategoryId) -> Option<Sink> {
        let kind = self.index.kind(to)?;
        CODE_PROFILE
            .tables
            .effect_sink(kind, category)
            .map(|(i, s)| (i, s.class))
    }

    /// `e.to`, or its twin (made on first use) when `e` makes it a sink.
    fn target(
        &mut self,
        e: &Edge,
        taken: &mut HashSet<NodeId>,
        twin_of: &mut HashMap<NodeId, NodeId>,
        order: &mut Vec<NodeId>,
    ) -> NodeId {
        let Some(sink) = self.sink_for(e.to, e.category) else {
            return e.to;
        };
        if let Some(t) = twin_of.get(&e.to) {
            return *t;
        }
        let mut t = e.to.0 ^ TWIN_SALT;
        while taken.contains(&NodeId(t)) {
            t = t.wrapping_add(TWIN_STEP);
        }
        let t = NodeId(t);
        taken.insert(t);
        twin_of.insert(e.to, t);
        self.twins.insert(t, (e.to, sink));
        order.push(t);
        t
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

impl GraphSource for WalkGraph<'_> {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_mode_is_a_join() {
        assert_eq!(fold_mode(None, Some("read")), Some("read"));
        assert_eq!(fold_mode(Some("write"), None), Some("write"));
        assert_eq!(fold_mode(Some("read"), Some("read")), Some("read"));
        assert_eq!(fold_mode(Some("read"), Some("write")), Some("read_write"));
        assert_eq!(
            fold_mode(Some("read_write"), Some("read")),
            Some("read_write")
        );
        assert_eq!(fold_mode(None, None), None);
    }

    #[test]
    fn class_filter_takes_table_names_in_any_case() {
        assert_eq!(class_filter(None), Ok(None));
        let asked = vec![
            "DB".to_string(),
            " http_call ".to_string(),
            "db".to_string(),
        ];
        assert_eq!(
            class_filter(Some(&asked)),
            Ok(Some(vec!["db", "http_call"]))
        );
        let err = class_filter(Some(&["nope".to_string()])).expect_err("unknown class");
        assert!(
            err.starts_with("unknown effect class `nope`; valid: db, email,"),
            "{err}"
        );
        assert!(class_filter(Some(&[])).is_err());
    }

    #[test]
    fn sink_mechanisms_are_edge_category_spellings() {
        let m = sink_mechanisms();
        assert_eq!(m, ["ACCESSES_DATA", "USES", "CALLS"]);
        for name in m {
            assert!(edge_category::ALL.iter().any(|(_, n)| *n == name), "{name}");
        }
    }

    #[test]
    fn flow_categories_are_carried_and_never_sinks() {
        for c in FLOW {
            assert!(CODE_PROFILE.tables.carries(c), "{}", edge_category::name(c));
            assert!(
                !CODE_PROFILE
                    .tables
                    .effect_sinks
                    .iter()
                    .any(|s| s.via.contains(&c)),
                "{} is a flow past a sink, not a sink",
                edge_category::name(c)
            );
        }
    }
}
