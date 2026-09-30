//! `glia gaps` (LF.2c): the ranked blind-spot report an overlay agent works
//! from, and [`overlay_delta`], the measurement that decides whether an
//! overlay edit is kept.
//!
//! Glia owns the report and the measurement; the agent loop that writes
//! `.glia/overlay.toml` from the report belongs to the consumer (repo-graph).
//! The report is read-only: no node, edge or cell is added or changed.
//!
//! [`gaps_report`] lists one [`GapRow`] per gap, in [`CATEGORIES`] order.
//! Each row names the overlay section that could repair it (`suggest`) and
//! how sure the report is (`tier`: [`FACT`] is read straight off the edges,
//! [`HEURISTIC`] can be legitimate):
//!
//! | category | what | tier | suggest |
//! |---|---|---|---|
//! | `unpaired_endpoint` | an ENDPOINT (not `<unresolved>`) with no outgoing HTTP_CALLS | fact; heuristic when its ENDPOINT_HIT records a `host` (it may be a third-party API) | `constants` for a `${…}` / `/{}` path, `route_prefix\|edge` when a route of its method (or `ANY`) has a path that is a proper suffix of it (`detail` names it), else `edge` |
//! | `ambiguous_endpoint` | an ENDPOINT whose HTTP_CALLS targets span >= 2 repos or >= 2 project roots | fact | `constants\|route_prefix` |
//! | `unresolved_endpoint` | `endpoint:<M>:<unresolved>` with no outgoing HTTP_CALLS; `detail` names its owner (the CALLS / USES predecessor) | fact | `wrapper` |
//! | `wrapped_sink` | an `unresolved_endpoint` whose every owner is named like a declared http `[[wrapper]]` (the owner's last qname segment equals the stanza's `call` after its last `.` / `::`, in the owner's repo): the sink is real, its identity now lives at the wrapper's call sites (LF.2e mints them); informational | fact | `none` |
//! | `unpaired_route` | a ROUTE that is not a client-router page (ORIGIN `nav_route`, `graph::nav::is_nav_route`) with no incoming HTTP_CALLS | heuristic (a public API is legitimately uncalled in-stack) | `route_prefix\|edge` |
//! | `tag_only_queue` | a QUEUE_PRODUCER / QUEUE_CONSUMER whose topic is a framework tag (`queues::is_framework_tag`) | fact | `constants\|wrapper` |
//! | `dead_symbol` | a FUNCTION / METHOD / CLASS outside `entrypoint_reachable`, with no incoming carry edge, not ORIGIN `test_fixture` | heuristic | `entrypoints` |
//! | `cochange_no_edge` | a file pair git history says changes together (a CO_CHANGES edge, LF.5b) that no static link joins ([`cochange_gaps`]); the row is the first file's MODULE, `detail` names the other file, the counts and the languages | heuristic (co-change is history, not proof of coupling) | `edge` |
//! | `orphaned_rule` | a qname-bearing overlay stanza (`[[edge]]` from / to, `[[constraint]]` / `[[decision]]` / `[[note]]` anchor, `[entrypoints]` qname) that binds no node | fact | `remove` |
//! | `redundant_rule` | an `[[edge]]` equal to an edge whose ORIGIN is not the overlay (the extractor caught up) | fact | `remove` |
//! | `orphaned_cell` | a `.glia/cells.jsonl` / `.glia/vectors.jsonl` row whose qname + hint bind nothing | fact | `glia cell ls --check --rekey` |
//!
//! `kind` is the node kind for a graph row, the overlay section for a rule
//! row and the cell type for a cell row. `line` is 1-based (LD.1): through
//! [`Locator`] for a node, the stanza's header line for a rule, the row's line
//! for a sidecar row.
//!
//! `wrapped_sink` and the last three categories read each repo's files (its
//! `.glia/overlay.toml`, its sidecars), so they need its root: with no roots
//! they are not computed and are listed in [`GapsReport::skipped`] instead of
//! `counts`, and an `<unresolved>` sink stays an `unresolved_endpoint`. Every qname binds through
//! `graph::cells::QnameIndex`, as the stage that applies it binds it: an
//! `[[edge]]` side as the overlay edge stage (`external::overlay`) does, the
//! exact qname in the stanza's own repo first, else in any repo of the build;
//! a `[[constraint]]` / `[[decision]]` / `[[note]]` anchor in its own repo
//! only, as declared knowledge (`external::declared`) does; a sidecar row in
//! its own repo, with its kind and hint, as the sidecar stage does. An
//! `[entrypoints]` qname (applied by LF.3b) is orphaned when no repo of the
//! build has it (`<prefix>::*`: no qname under `<prefix>::`). A declared
//! stanza with no anchor binds a scope's PROJECT, which is not a qname: the
//! `[declared] orphaned ... scope=` stderr line reports that case.
//!
//! Rows are sorted by (category order, file, line, qname, detail); a
//! category is cut to `top_k_per_category` after sorting, and `counts` holds
//! the totals before the cut. Every iteration is over a `Vec` or a `BTree*`,
//! and the maps are only looked up, so two builds of one tree report the same
//! bytes.
//!
//! Markers (the fired_on lines):
//! - `[gaps] rows=R (unpaired_endpoint=N ... orphaned_cell=N) surface=<cli|py|engine>`
//!   once per report; `R` is the rows returned, each `N` a category's total
//!   (`skipped` for a category not computed);
//! - `[cochange] pairs=P linked=L gaps=G (direct=D bridged=B) surface=<coverage|gaps>`
//!   once per co-change audit: per [`cochange_gaps`] call (`coverage`, the
//!   `glia coverage` section) and per report that computes
//!   `cochange_no_edge` (`gaps`);
//! - `[overlay] <rules> rules, +<M> edges, orphans <K>→<J>` once per
//!   [`overlay_delta`], the review's accept-loop line.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::external_inputs::{CELLS_FILE, CellRow, VECTORS_FILE, VectorRow};
use glia_code_domain::glia_config::{self, LoadedConfig, OVERLAY_FILE, WrapperKind};
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_code_extractors::queues::is_framework_tag;
use glia_core::{Cell, CellPayload, EdgeCategoryId, NodeId, NodeKindId, RepoId};
use glia_graph::MergedGraph;
use glia_graph::cells::{CellTarget, QnameIndex};
use glia_graph::nav::{is_nav_route, nav_route_path};
use glia_graph::normalise_http_path;

use crate::answers::{Locator, entrypoint_reachable};
use crate::build::{BuildOptions, generate_many_opts, generate_one_opts};
use crate::external::signals;
use crate::profile::CODE_PROFILE;

pub const UNPAIRED_ENDPOINT: &str = "unpaired_endpoint";
pub const AMBIGUOUS_ENDPOINT: &str = "ambiguous_endpoint";
pub const UNRESOLVED_ENDPOINT: &str = "unresolved_endpoint";
pub const WRAPPED_SINK: &str = "wrapped_sink";
pub const UNPAIRED_ROUTE: &str = "unpaired_route";
pub const TAG_ONLY_QUEUE: &str = "tag_only_queue";
pub const DEAD_SYMBOL: &str = "dead_symbol";
pub const COCHANGE_NO_EDGE: &str = "cochange_no_edge";
pub const ORPHANED_RULE: &str = "orphaned_rule";
pub const REDUNDANT_RULE: &str = "redundant_rule";
pub const ORPHANED_CELL: &str = "orphaned_cell";

/// Every category, in report order.
pub const CATEGORIES: [&str; 11] = [
    UNPAIRED_ENDPOINT,
    AMBIGUOUS_ENDPOINT,
    UNRESOLVED_ENDPOINT,
    WRAPPED_SINK,
    UNPAIRED_ROUTE,
    TAG_ONLY_QUEUE,
    DEAD_SYMBOL,
    COCHANGE_NO_EDGE,
    ORPHANED_RULE,
    REDUNDANT_RULE,
    ORPHANED_CELL,
];

/// The categories that read a repo's files, so need its root.
const ROOT_CATEGORIES: [&str; 4] = [WRAPPED_SINK, ORPHANED_RULE, REDUNDANT_RULE, ORPHANED_CELL];

/// The categories [`overlay_delta`] counts as orphans: a client-side sink the
/// overlay exists to pair. A `wrapped_sink` is not one: its identity lives at
/// the wrapper's call sites, so declaring the `[[wrapper]]` takes the sink out
/// of the `with` count (those call sites count instead, when they pair with
/// nothing).
const ORPHAN_CATEGORIES: [&str; 3] = [UNPAIRED_ENDPOINT, UNRESOLVED_ENDPOINT, TAG_ONLY_QUEUE];

/// Read straight off the graph's edges.
pub const FACT: &str = "fact";
/// Likely a gap, but can be legitimate (an uncalled public API, a
/// third-party host, a symbol called only from outside the build).
pub const HEURISTIC: &str = "heuristic";

/// Callers / owners named in one `detail` before the rest are counted.
const MAX_NAMED: usize = 5;

/// One gap. See the module table for what each category means.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GapRow {
    pub category: &'static str,
    pub qname: String,
    /// Node kind (graph rows), overlay section (rule rows) or cell type (cell rows).
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    pub detail: String,
    /// The overlay section (or command) that could repair it.
    pub suggest: &'static str,
    /// [`FACT`] or [`HEURISTIC`].
    pub tier: &'static str,
}

/// [`gaps_report`]'s answer.
#[derive(Serialize, Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GapsReport {
    /// Per computed category, the total before `top_k_per_category`.
    pub counts: BTreeMap<&'static str, usize>,
    /// Categories not computed: the root-reading ones when no root was given.
    pub skipped: Vec<&'static str>,
    pub rows: Vec<GapRow>,
}

impl GapsReport {
    /// Total gaps of `category` (0 when it was skipped or found none).
    pub fn count(&self, category: &str) -> usize {
        self.counts.get(category).copied().unwrap_or(0)
    }
}

/// How [`gaps_report`] runs. Made with `Default` plus field assignment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GapsOptions {
    /// Keep at most this many rows per category (after sorting).
    pub top_k_per_category: Option<usize>,
    /// Keep only this category's rows; must be one of [`CATEGORIES`].
    pub category: Option<String>,
    /// The `surface=` of the `[gaps]` marker (`cli`, `py`); empty = `engine`.
    pub surface: &'static str,
}

/// [`overlay_delta`]'s answer: one tree built without and then with its
/// overlay sections.
#[derive(Serialize, Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct OverlayDelta {
    /// Overlay stanzas the loader kept, over every repo: `[constants]` keys,
    /// `[[route_prefix]]`, `[[wrapper]]` and `[[edge]]` stanzas.
    pub rules: usize,
    pub edges_without: usize,
    pub edges_with: usize,
    /// Per edge category, edges with minus edges without; zero deltas left out.
    pub added_by_category: BTreeMap<&'static str, i64>,
    /// `unpaired_endpoint + unresolved_endpoint + tag_only_queue`.
    pub orphans_without: usize,
    pub orphans_with: usize,
}

/// The ranked blind-spot report of `merged`. `roots` are `(RepoId.0, repo
/// root)` pairs (a build's `GenerateResult::repo_roots`); empty skips the
/// root-reading categories. Errs only on an unknown `opts.category`.
pub fn gaps_report(
    merged: &MergedGraph,
    roots: &[(u64, PathBuf)],
    opts: &GapsOptions,
) -> Result<GapsReport, String> {
    if let Some(c) = opts.category.as_deref()
        && !CATEGORIES.contains(&c)
    {
        return Err(format!(
            "unknown gaps category `{c}` (one of: {})",
            CATEGORIES.join(", ")
        ));
    }
    let skipped: Vec<&'static str> = if roots.is_empty() {
        ROOT_CATEGORIES.to_vec()
    } else {
        Vec::new()
    };
    let wanted: Vec<&'static str> = CATEGORIES
        .iter()
        .copied()
        .filter(|c| !skipped.contains(c))
        .collect();
    let wrapped = if wanted.contains(&WRAPPED_SINK) {
        declared_wrappers(roots)
    } else {
        Wrapped::new()
    };
    let mut rows = collect_rows(merged, roots, &wanted, &wrapped);
    rows.sort_by(row_order);

    let mut counts: BTreeMap<&'static str, usize> = wanted.iter().map(|c| (*c, 0)).collect();
    for r in &rows {
        *counts.entry(r.category).or_default() += 1;
    }
    let mut taken: BTreeMap<&'static str, usize> = BTreeMap::new();
    rows.retain(|r| {
        if opts.category.as_deref().is_some_and(|c| c != r.category) {
            return false;
        }
        let n = taken.entry(r.category).or_default();
        *n += 1;
        opts.top_k_per_category.is_none_or(|k| *n <= k)
    });

    let report = GapsReport {
        counts,
        skipped,
        rows,
    };
    let per: Vec<String> = CATEGORIES
        .iter()
        .map(|c| match report.counts.get(c) {
            Some(n) => format!("{c}={n}"),
            None => format!("{c}=skipped"),
        })
        .collect();
    let surface = if opts.surface.is_empty() {
        "engine"
    } else {
        opts.surface
    };
    eprintln!(
        "[gaps] rows={} ({}) surface={surface}",
        report.rows.len(),
        per.join(" ")
    );
    Ok(report)
}

/// Build `repo_paths` twice through `generate_*_opts` — the overlay off, then
/// on — and measure what the overlay changed: edges per category, and the
/// orphan count ([`ORPHAN_CATEGORIES`]) before and after. The `with` count
/// reads the repos' `[[wrapper]]` stanzas, so a wrapper's `<unresolved>` sink
/// is a `wrapped_sink` there and not an orphan; the `without` build applies
/// no overlay, so it stays an `unresolved_endpoint`. One path builds
/// through `generate_one_opts` (what `glia gaps <repo>` reads), several
/// through `generate_many_opts`. Doubles the build cost by design: the
/// extraction-only build is the only consistent "without" view (an overlay
/// section that re-keys nodes cannot be undone by filtering edges).
/// `incremental` reuses each repo's parse cache (and saves it).
///
/// Prints `[overlay] <rules> rules, +<M> edges, orphans <K>→<J>`.
pub fn overlay_delta(repo_paths: &[String], incremental: bool) -> Result<OverlayDelta, String> {
    if repo_paths.is_empty() {
        return Err("overlay_delta: no repo paths".to_string());
    }
    let build = |overlay: bool| {
        let opts = BuildOptions::default().with_overlay(overlay);
        match repo_paths {
            [one] => generate_one_opts(one, incremental, &opts),
            many => generate_many_opts(many, incremental, &opts),
        }
    };
    let without = build(false)?;
    let with = build(true)?;

    let rules = repo_paths
        .iter()
        .filter_map(|p| glia_config::load(Path::new(p)))
        .map(|cfg| overlay_rule_count(&cfg))
        .sum();
    let per_without = edges_by_category(&without.merged);
    let per_with = edges_by_category(&with.merged);
    let mut added_by_category = BTreeMap::new();
    for (_, name) in edge_category::ALL {
        let d =
            per_with.get(name).copied().unwrap_or(0) - per_without.get(name).copied().unwrap_or(0);
        if d != 0 {
            added_by_category.insert(*name, d);
        }
    }
    let orphans = |m: &MergedGraph, wrapped: &Wrapped| {
        collect_rows(m, &[], &ORPHAN_CATEGORIES, wrapped).len()
    };
    let with_roots: Vec<(u64, PathBuf)> = with
        .repo_roots
        .iter()
        .map(|(r, p)| (*r, PathBuf::from(p)))
        .collect();
    let delta = OverlayDelta {
        rules,
        edges_without: without.total_edges,
        edges_with: with.total_edges,
        added_by_category,
        orphans_without: orphans(&without.merged, &Wrapped::new()),
        orphans_with: orphans(&with.merged, &declared_wrappers(&with_roots)),
    };
    let added = i64::try_from(delta.edges_with).unwrap_or(i64::MAX)
        - i64::try_from(delta.edges_without).unwrap_or(i64::MAX);
    eprintln!(
        "[overlay] {} rules, {added:+} edges, orphans {}→{}",
        delta.rules, delta.orphans_without, delta.orphans_with
    );
    Ok(delta)
}

/// Overlay (inference) entries the loader kept: what `--no-overlay` switches off.
fn overlay_rule_count(cfg: &LoadedConfig) -> usize {
    let c = &cfg.config;
    c.constants.len() + c.route_prefix.len() + c.wrapper.len() + c.edge.len()
}

fn edges_by_category(merged: &MergedGraph) -> BTreeMap<&'static str, i64> {
    let mut out: BTreeMap<&'static str, i64> = BTreeMap::new();
    for e in merged.all_edges() {
        *out.entry(edge_category::name(e.category)).or_default() += 1;
    }
    out
}

fn row_order(a: &GapRow, b: &GapRow) -> std::cmp::Ordering {
    let rank = |c: &str| {
        CATEGORIES
            .iter()
            .position(|x| *x == c)
            .unwrap_or(CATEGORIES.len())
    };
    rank(a.category)
        .cmp(&rank(b.category))
        .then_with(|| a.file.cmp(&b.file))
        .then_with(|| a.line.cmp(&b.line))
        .then_with(|| a.qname.cmp(&b.qname))
        .then_with(|| a.detail.cmp(&b.detail))
}

// ---------------------------------------------------------------------------
// The graph view: one pass over the nodes and one over the edges.
// ---------------------------------------------------------------------------

/// One node, its first instance (graphs, then nodes, in `Vec` order).
struct NodeView<'a> {
    id: NodeId,
    kind: NodeKindId,
    qname: &'a str,
    cells: &'a [Cell],
}

struct View<'a> {
    nodes: Vec<NodeView<'a>>,
    qname_of: HashMap<NodeId, &'a str>,
    repo_of: HashMap<NodeId, u64>,
    /// HTTP_CALLS targets per ENDPOINT, in edge order.
    http_out: HashMap<NodeId, Vec<NodeId>>,
    http_in: HashSet<NodeId>,
    /// Nodes with an incoming edge reachability follows.
    carry_in: HashSet<NodeId>,
    /// CALLS / USES predecessors, in edge order.
    callers: HashMap<NodeId, Vec<NodeId>>,
    /// HANDLED_BY targets, in edge order.
    handlers: HashMap<NodeId, Vec<NodeId>>,
    /// Per repo, the repo-relative dirs of its PROJECT anchors (`.` left out).
    project_roots: BTreeMap<u64, Vec<String>>,
}

impl<'a> View<'a> {
    fn build(merged: &'a MergedGraph) -> Self {
        let mut v = View {
            nodes: Vec::new(),
            qname_of: HashMap::new(),
            repo_of: HashMap::new(),
            http_out: HashMap::new(),
            http_in: HashSet::new(),
            carry_in: HashSet::new(),
            callers: HashMap::new(),
            handlers: HashMap::new(),
            project_roots: BTreeMap::new(),
        };
        for g in &merged.graphs {
            for n in &g.nodes {
                if v.repo_of.contains_key(&n.id) {
                    continue;
                }
                let (Some(kind), Some(qname)) =
                    (g.nav.kind_by_id.get(&n.id), g.nav.qname_by_id.get(&n.id))
                else {
                    continue;
                };
                v.repo_of.insert(n.id, g.repo.0);
                v.qname_of.insert(n.id, qname.as_str());
                if *kind == node_kind::PROJECT
                    && let Some(dir) = qname.strip_prefix("project:")
                    && dir != "."
                    && !dir.is_empty()
                {
                    v.project_roots
                        .entry(g.repo.0)
                        .or_default()
                        .push(dir.to_string());
                }
                v.nodes.push(NodeView {
                    id: n.id,
                    kind: *kind,
                    qname,
                    cells: &n.cells,
                });
            }
        }
        for e in merged.all_edges() {
            let c = e.category;
            if c == edge_category::HTTP_CALLS {
                v.http_out.entry(e.from).or_default().push(e.to);
                v.http_in.insert(e.to);
            }
            if c == edge_category::CALLS || c == edge_category::USES {
                v.callers.entry(e.to).or_default().push(e.from);
            }
            if c == edge_category::HANDLED_BY {
                v.handlers.entry(e.from).or_default().push(e.to);
            }
            if CODE_PROFILE.tables.carries(c) && e.from != e.to {
                v.carry_in.insert(e.to);
            }
        }
        v
    }

    fn qname(&self, id: NodeId) -> &str {
        self.qname_of.get(&id).copied().unwrap_or("?")
    }

    /// `a, b, c (+N more)`: the distinct qnames of `ids`, in order.
    fn named(&self, ids: &[NodeId]) -> Option<String> {
        let mut seen: Vec<&str> = Vec::new();
        for id in ids {
            let q = self.qname(*id);
            if !seen.contains(&q) {
                seen.push(q);
            }
        }
        if seen.is_empty() {
            return None;
        }
        let more = seen.len().saturating_sub(MAX_NAMED);
        let mut s = seen
            .iter()
            .take(MAX_NAMED)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        if more > 0 {
            s.push_str(&format!(" (+{more} more)"));
        }
        Some(s)
    }

    /// The longest PROJECT root of `repo` holding `file`, if any.
    fn project_root(&self, repo: u64, file: Option<&str>) -> Option<&str> {
        let file = file?;
        self.project_roots
            .get(&repo)?
            .iter()
            .filter(|r| file == r.as_str() || file.starts_with(&format!("{r}/")))
            .max_by(|a, b| {
                a.len()
                    .cmp(&b.len())
                    .then_with(|| a.as_str().cmp(b.as_str()))
            })
            .map(String::as_str)
    }
}

/// `(method, path)` of an ENDPOINT qname `endpoint:<M>:<path>[ @owner]`.
fn endpoint_parts(qname: &str) -> Option<(&str, &str)> {
    split_owner(qname)
        .0
        .strip_prefix("endpoint:")?
        .split_once(':')
}

/// The method of a `<METHOD> <path>` ROUTE qname (`ANY` included); `None`
/// for a method-less shape (`route:<path>`, a page).
fn route_method(qname: &str) -> Option<&str> {
    let (m, _) = split_owner(qname).0.split_once(' ')?;
    (!m.is_empty() && m.bytes().all(|b| b.is_ascii_uppercase())).then_some(m)
}

/// The `host` an ENDPOINT_HIT recorded (engine `endpoint_fold`).
fn endpoint_host(cells: &[Cell]) -> Option<String> {
    cells.iter().find_map(|c| match &c.payload {
        CellPayload::Json(j) if c.kind == cell_type::ENDPOINT_HIT => {
            serde_json::from_str::<serde_json::Value>(j)
                .ok()?
                .get("host")?
                .as_str()
                .map(str::to_string)
        }
        _ => None,
    })
}

/// ORIGIN `provenance` of a node, when it has one.
fn provenance_is(cells: &[Cell], provenance: &str) -> bool {
    let needle = format!("\"provenance\":\"{provenance}\"");
    cells.iter().any(|c| {
        c.kind == cell_type::ORIGIN
            && matches!(&c.payload, CellPayload::Json(j) | CellPayload::Text(j) if j.contains(&needle))
    })
}

/// An edge the overlay stage added (its ORIGIN edge cell is `overlay:<llm|human>`).
fn is_overlay_edge(cells: &[Cell]) -> bool {
    cells.iter().any(|c| {
        c.kind == cell_type::ORIGIN
            && matches!(&c.payload, CellPayload::Json(j) | CellPayload::Text(j)
                        if j.contains("\"provenance\":\"overlay:"))
    })
}

/// The label a repo id is shown with in a `detail`: the root's directory
/// name, else `repo<id>` (the `arch::service_of` fallback).
fn label_of(labels: &BTreeMap<u64, String>, repo: u64) -> String {
    labels
        .get(&repo)
        .cloned()
        .unwrap_or_else(|| format!("repo{repo}"))
}

/// Per repo id, the declared http `[[wrapper]]` callees: the last segment of
/// `call` (after its last `.` / `::`) -> the `call` as written, first stanza
/// first.
type Wrapped = BTreeMap<u64, BTreeMap<String, String>>;

/// The http `[[wrapper]]` stanzas of each root's `.glia/overlay.toml`.
fn declared_wrappers(roots: &[(u64, PathBuf)]) -> Wrapped {
    let mut out = Wrapped::new();
    for (repo, root) in roots {
        let Some(cfg) = glia_config::load(root) else {
            continue;
        };
        for s in &cfg.config.wrapper {
            let w = s.get_ref();
            if w.wrapper_kind() != Some(WrapperKind::Http) {
                continue;
            }
            let name = w.call.rsplit(['.', ':']).next().unwrap_or(&w.call);
            if !name.is_empty() {
                out.entry(*repo)
                    .or_default()
                    .entry(name.to_string())
                    .or_insert_with(|| w.call.clone());
            }
        }
    }
    out
}

/// Every row of the `wanted` categories, unsorted. `wrapped` is
/// [`declared_wrappers`] (empty when no wrapper is known).
fn collect_rows(
    merged: &MergedGraph,
    roots: &[(u64, PathBuf)],
    wanted: &[&'static str],
    wrapped: &Wrapped,
) -> Vec<GapRow> {
    let want = |c: &str| wanted.contains(&c);
    let view = View::build(merged);
    let loc = Locator::new(merged);
    let labels = crate::arch::repo_label_map(
        &roots
            .iter()
            .map(|(r, p)| (*r, p.to_string_lossy().into_owned()))
            .collect::<Vec<_>>(),
    );
    let mut rows = Vec::new();

    // Server routes (method, normalised path), for the route_prefix suggestion.
    let routes: Vec<(Option<&str>, String, NodeId)> = view
        .nodes
        .iter()
        .filter(|n| n.kind == node_kind::ROUTE && !is_nav_route(n.cells))
        .filter_map(|n| {
            let path = normalise_http_path(nav_route_path(n.qname)?);
            Some((route_method(n.qname), path, n.id))
        })
        .collect();

    let live = want(DEAD_SYMBOL).then(|| entrypoint_reachable(merged));

    for n in &view.nodes {
        let node_row =
            |category: &'static str, detail: String, suggest: &'static str, tier: &'static str| {
                let at = loc.locate(n.id);
                GapRow {
                    category,
                    qname: n.qname.to_string(),
                    kind: at.kind,
                    file: at.file,
                    line: at.line,
                    detail,
                    suggest,
                    tier,
                }
            };
        match n.kind {
            k if k == node_kind::ENDPOINT => {
                let Some((method, path)) = endpoint_parts(n.qname) else {
                    continue;
                };
                let targets = view.http_out.get(&n.id).map(Vec::as_slice).unwrap_or(&[]);
                let callers = view.callers.get(&n.id).map(Vec::as_slice).unwrap_or(&[]);
                if targets.is_empty() && path == "<unresolved>" {
                    let owner = view.named(callers).unwrap_or_else(|| "(none)".to_string());
                    // Every owner a declared wrapper (in the sink's repo): the
                    // identity lives at the wrapper's call sites.
                    let declared = view.repo_of.get(&n.id).and_then(|r| wrapped.get(r));
                    let calls: Option<Vec<&str>> = declared.and_then(|d| {
                        (!callers.is_empty())
                            .then(|| {
                                callers
                                    .iter()
                                    .map(|c| {
                                        let q = view.qname(*c);
                                        d.get(q.rsplit("::").next().unwrap_or(q))
                                            .map(String::as_str)
                                    })
                                    .collect()
                            })
                            .flatten()
                    });
                    match calls {
                        Some(mut calls) if want(WRAPPED_SINK) => {
                            calls.sort_unstable();
                            calls.dedup();
                            rows.push(node_row(
                                WRAPPED_SINK,
                                format!("owner={owner}; wrapper={}", calls.join(", ")),
                                "none",
                                FACT,
                            ));
                        }
                        Some(_) => {}
                        None if want(UNRESOLVED_ENDPOINT) => {
                            rows.push(node_row(
                                UNRESOLVED_ENDPOINT,
                                format!("owner={owner}"),
                                "wrapper",
                                FACT,
                            ));
                        }
                        None => {}
                    }
                } else if targets.is_empty() {
                    if want(UNPAIRED_ENDPOINT) {
                        let norm = normalise_http_path(path);
                        let suffix = routes
                            .iter()
                            .filter(|(m, r, _)| {
                                m.is_none_or(|m| m == "ANY" || m.eq_ignore_ascii_case(method))
                                    && r != "/"
                                    && norm.len() > r.len()
                                    && norm.ends_with(r.as_str())
                            })
                            .map(|(_, _, id)| *id)
                            .min_by(|a, b| view.qname(*a).cmp(view.qname(*b)).then(a.0.cmp(&b.0)));
                        let suggest = if path.contains("${") || norm.starts_with("/{}") {
                            "constants"
                        } else if suffix.is_some() {
                            "route_prefix|edge"
                        } else {
                            "edge"
                        };
                        let host = endpoint_host(n.cells);
                        let mut detail = vec!["no HTTP_CALLS target".to_string()];
                        if let Some(c) = view.named(callers) {
                            detail.push(format!("caller={c}"));
                        }
                        if let Some(h) = &host {
                            detail.push(format!("host={h}"));
                        }
                        if let Some(r) = suffix {
                            let repo = view.repo_of.get(&r).copied().unwrap_or_default();
                            detail.push(format!(
                                "suffix_of={} ({})",
                                view.qname(r),
                                label_of(&labels, repo)
                            ));
                        }
                        let tier = if host.is_some() { HEURISTIC } else { FACT };
                        rows.push(node_row(
                            UNPAIRED_ENDPOINT,
                            detail.join("; "),
                            suggest,
                            tier,
                        ));
                    }
                } else if want(AMBIGUOUS_ENDPOINT) {
                    let mut buckets: BTreeSet<(u64, Option<String>)> = BTreeSet::new();
                    let mut repos: BTreeSet<u64> = BTreeSet::new();
                    let mut named: BTreeSet<String> = BTreeSet::new();
                    let mut ids: BTreeSet<u64> = BTreeSet::new();
                    for t in targets {
                        let repo = view.repo_of.get(t).copied().unwrap_or_default();
                        let file = loc.file_of(*t);
                        let root = view.project_root(repo, file.as_deref()).map(str::to_string);
                        repos.insert(repo);
                        ids.insert(t.0);
                        buckets.insert((repo, root));
                        named.insert(format!("{} ({})", view.qname(*t), label_of(&labels, repo)));
                    }
                    if buckets.len() >= 2 {
                        let projects = buckets.iter().filter(|(_, r)| r.is_some()).count();
                        let detail = format!(
                            "targets={} repos={} projects={projects}: {}",
                            ids.len(),
                            repos.len(),
                            named.into_iter().collect::<Vec<_>>().join(", ")
                        );
                        rows.push(node_row(
                            AMBIGUOUS_ENDPOINT,
                            detail,
                            "constants|route_prefix",
                            FACT,
                        ));
                    }
                }
            }
            k if k == node_kind::ROUTE
                && want(UNPAIRED_ROUTE)
                && !is_nav_route(n.cells)
                && !view.http_in.contains(&n.id) =>
            {
                let handlers = view.handlers.get(&n.id).map(Vec::as_slice).unwrap_or(&[]);
                let detail = match view.named(handlers) {
                    Some(h) => format!("no incoming HTTP_CALLS; handler={h}"),
                    None => "no incoming HTTP_CALLS".to_string(),
                };
                rows.push(node_row(
                    UNPAIRED_ROUTE,
                    detail,
                    "route_prefix|edge",
                    HEURISTIC,
                ));
            }
            k if k == node_kind::QUEUE_PRODUCER || k == node_kind::QUEUE_CONSUMER => {
                if !want(TAG_ONLY_QUEUE) {
                    continue;
                }
                let Some((_, topic)) = split_owner(n.qname).0.split_once(':') else {
                    continue;
                };
                if is_framework_tag(topic) {
                    let mut owners: Vec<NodeId> =
                        view.callers.get(&n.id).cloned().unwrap_or_default();
                    owners.extend(view.handlers.get(&n.id).into_iter().flatten().copied());
                    let mut detail = format!("topic={topic}");
                    if let Some(o) = view.named(&owners) {
                        detail.push_str(&format!("; owner={o}"));
                    }
                    rows.push(node_row(TAG_ONLY_QUEUE, detail, "constants|wrapper", FACT));
                }
            }
            k if k == node_kind::FUNCTION || k == node_kind::METHOD || k == node_kind::CLASS => {
                if let Some(live) = &live
                    && !live.contains(&n.id)
                    && !view.carry_in.contains(&n.id)
                    && !provenance_is(n.cells, "test_fixture")
                {
                    rows.push(node_row(
                        DEAD_SYMBOL,
                        "no entrypoint reaches it; no incoming call or use".to_string(),
                        "entrypoints",
                        HEURISTIC,
                    ));
                }
            }
            _ => {}
        }
    }

    if want(COCHANGE_NO_EDGE) {
        for p in cochange_audit(merged, "gaps") {
            let at = loc.locate(p.module_a);
            rows.push(GapRow {
                category: COCHANGE_NO_EDGE,
                qname: at.qname,
                kind: at.kind,
                line: at.line,
                detail: cochange_detail(&p.gap, view.qname(p.module_b)),
                file: Some(p.gap.file_a),
                suggest: "edge",
                tier: HEURISTIC,
            });
        }
    }

    if want(ORPHANED_RULE) || want(REDUNDANT_RULE) || want(ORPHANED_CELL) {
        let mut ctx = RootCtx {
            merged,
            view: &view,
            labels: &labels,
            all: None,
            extracted: None,
            want: wanted,
        };
        for (repo, root) in roots {
            ctx.rule_rows(*repo, root, &mut rows);
            if want(ORPHANED_CELL) {
                ctx.cell_rows(*repo, root, &mut rows);
            }
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// The co-change audit (LF.5c): history's pairs the static graph cannot explain.
// ---------------------------------------------------------------------------

/// Hops a static link may take between the two files of a co-change pair: a
/// direct edge is one, a bridge through one or two file-less nodes two or
/// three.
const MAX_LINK_HOPS: usize = 3;

/// Edge categories that never link a co-change pair: CO_CHANGES is the claim
/// under audit; DEFINES / CONTAINS are structure (a module holds its symbols,
/// a project its files), which would join every pair of one package.
pub(crate) const NOT_A_LINK: [EdgeCategoryId; 3] = [
    edge_category::CO_CHANGES,
    edge_category::DEFINES,
    edge_category::CONTAINS,
];

/// One co-changing file pair with no static link (LF.5c): what
/// [`cochange_gaps`] returns and the `cochange_no_edge` rows of
/// [`gaps_report`] carry. HEURISTIC by construction: co-change is history,
/// not proof of coupling, and a pair can be coupled through something no
/// extractor sees (a shared config key read by name, a convention). Never a
/// FACT-tier input.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CochangeGap {
    /// Repo-relative; `file_a < file_b`.
    pub file_a: String,
    pub file_b: String,
    /// Commits of the snapshot window that touched both files (the
    /// CO_CHANGES edge's ATTN `cochanges`).
    pub cochanges: u32,
    /// `cochanges` per mille of the commits of the file that changes less
    /// often (the edge's ATTN `ratio_permille`).
    pub ratio_permille: u32,
    /// The coverage-table language of each file; `None` for an extension that
    /// table does not name.
    pub language_a: Option<&'static str>,
    pub language_b: Option<&'static str>,
    /// The two languages differ (by extension when either is `None`).
    pub cross_language: bool,
    /// Always [`HEURISTIC`].
    pub tier: &'static str,
}

/// One audited gap and the MODULEs its CO_CHANGES edge joins (`module_a` is
/// the one in `file_a`).
struct PairGap {
    gap: CochangeGap,
    module_a: NodeId,
    module_b: NodeId,
}

/// **cochange_gaps** (LF.5c): the file pairs that change together in git
/// history (CO_CHANGES edges, LF.5b) but share no static link — places where
/// THIS repo's graph is likely blind, which the static caveat table of
/// `coverage_report` cannot point at.
///
/// A pair is linked when some path of at most three edges joins a node of
/// one file to a node of the other, either way round, over edges of any
/// category but CO_CHANGES / DEFINES / CONTAINS, where every intermediate
/// node is file-less (no POSITION: ENDPOINT, ROUTE, queue / event nodes,
/// PACKAGE_DEP, ...). A node belongs to every file its POSITION cells name,
/// so one node placed in both files links them too. Cross-service coupling is
/// a static link: a client function -> ENDPOINT -> ROUTE -> handler bridges
/// the client's file to the server's. A path through a third FILE is not a
/// link (that is transitive coupling, not the pair's own).
///
/// Unlinked pairs are sorted by (cochanges desc, file_a, file_b) and cut to
/// `top_k`. Read-only; empty for a graph with no CO_CHANGES edge. Prints the
/// `[cochange] ... surface=coverage` marker.
pub fn cochange_gaps(merged: &MergedGraph, top_k: Option<usize>) -> Vec<CochangeGap> {
    let mut out: Vec<CochangeGap> = cochange_audit(merged, "coverage")
        .into_iter()
        .map(|p| p.gap)
        .collect();
    if let Some(k) = top_k {
        out.truncate(k);
    }
    out
}

/// A `cochange_no_edge` row's `detail`:
/// `with=<file_b> (<qname>); cochanges=N; ratio_permille=R; languages=<a>,<b>`,
/// plus `; cross_language` when the languages differ (`?`: unknown).
fn cochange_detail(g: &CochangeGap, other: &str) -> String {
    let mut s = format!(
        "with={} ({other}); cochanges={}; ratio_permille={}; languages={},{}",
        g.file_b,
        g.cochanges,
        g.ratio_permille,
        g.language_a.unwrap_or("?"),
        g.language_b.unwrap_or("?")
    );
    if g.cross_language {
        s.push_str("; cross_language");
    }
    s
}

/// Where each node sits: the files its POSITION cells name, and the nodes
/// each file holds. Lookup only (no map is iterated), so HashMap order never
/// reaches an answer.
pub(crate) struct FileIndex {
    /// Node id -> (its graph's repo, its files in cell order, deduped); the
    /// first instance of an id (graphs, then nodes, in `Vec` order) wins.
    files_of: HashMap<u64, (u64, Vec<String>)>,
    /// (repo, file) -> the nodes placed in it.
    in_file: HashMap<(u64, String), Vec<u64>>,
}

impl FileIndex {
    fn build(merged: &MergedGraph) -> Self {
        let mut ix = FileIndex {
            files_of: HashMap::new(),
            in_file: HashMap::new(),
        };
        for g in &merged.graphs {
            for n in &g.nodes {
                if ix.files_of.contains_key(&n.id.0) {
                    continue;
                }
                let mut files: Vec<String> = Vec::new();
                for f in n.cells.iter().filter_map(position_file) {
                    if !files.contains(&f) {
                        files.push(f);
                    }
                }
                for f in &files {
                    ix.in_file
                        .entry((g.repo.0, f.clone()))
                        .or_default()
                        .push(n.id.0);
                }
                ix.files_of.insert(n.id.0, (g.repo.0, files));
            }
        }
        ix
    }

    /// `(repo, first file)` of a node that has one.
    pub(crate) fn first_file(&self, id: NodeId) -> Option<(u64, &str)> {
        let (repo, files) = self.files_of.get(&id.0)?;
        Some((*repo, files.first()?.as_str()))
    }

    /// No POSITION names a file for it (an id no graph holds included).
    fn file_less(&self, id: u64) -> bool {
        self.files_of.get(&id).is_none_or(|(_, f)| f.is_empty())
    }

    fn nodes_in(&self, repo: u64, file: &str) -> &[u64] {
        self.in_file
            .get(&(repo, file.to_string()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// The non-empty `file` of a POSITION cell.
fn position_file(c: &Cell) -> Option<String> {
    if c.kind != cell_type::POSITION {
        return None;
    }
    let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
        return None;
    };
    serde_json::from_str::<serde_json::Value>(s)
        .ok()?
        .get("file")?
        .as_str()
        .filter(|f| !f.is_empty())
        .map(str::to_string)
}

/// How a pair's two files are joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Link {
    /// One edge (or one node placed in both files).
    Direct,
    /// Two or three edges through file-less nodes.
    Bridged,
    None,
}

/// Bounded BFS from `a` (one file's nodes) to `b` (the other's) over the
/// undirected link adjacency, expanding only through file-less nodes.
pub(crate) fn link_between(
    adj: &HashMap<u64, Vec<u64>>,
    ix: &FileIndex,
    a: &[u64],
    b: &HashSet<u64>,
) -> Link {
    if a.iter().any(|n| b.contains(n)) {
        return Link::Direct;
    }
    let mut seen: HashSet<u64> = a.iter().copied().collect();
    let mut frontier: Vec<u64> = a.to_vec();
    for hop in 1..=MAX_LINK_HOPS {
        let mut next = Vec::new();
        for u in &frontier {
            for v in adj.get(u).into_iter().flatten() {
                if b.contains(v) {
                    return if hop == 1 {
                        Link::Direct
                    } else {
                        Link::Bridged
                    };
                }
                if hop < MAX_LINK_HOPS && ix.file_less(*v) && seen.insert(*v) {
                    next.push(*v);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Link::None
}

/// The static-link test of the co-change audit (LF.5c), shared with the
/// co-change suggestions (`cochange`, CC.11a): the [`FileIndex`] plus the
/// undirected link adjacency — every edge of `merged` but a self-loop or a
/// [`NOT_A_LINK`] category, both ways round. Built once per answer; lookup
/// only, so HashMap order never reaches an answer.
pub(crate) struct LinkIndex {
    pub(crate) files: FileIndex,
    adj: HashMap<u64, Vec<u64>>,
}

impl LinkIndex {
    pub(crate) fn build(merged: &MergedGraph) -> Self {
        let mut adj: HashMap<u64, Vec<u64>> = HashMap::new();
        for e in merged.all_edges() {
            if e.from == e.to || NOT_A_LINK.contains(&e.category) {
                continue;
            }
            adj.entry(e.from.0).or_default().push(e.to.0);
            adj.entry(e.to.0).or_default().push(e.from.0);
        }
        LinkIndex {
            files: FileIndex::build(merged),
            adj,
        }
    }

    /// How `file_a` and `file_b` of `repo` are joined ([`link_between`] from
    /// `file_a`'s nodes to `file_b`'s): [`Link::None`] when either file holds
    /// no node.
    pub(crate) fn link(&self, repo: u64, file_a: &str, file_b: &str) -> Link {
        let b: HashSet<u64> = self.files.nodes_in(repo, file_b).iter().copied().collect();
        link_between(
            &self.adj,
            &self.files,
            self.files.nodes_in(repo, file_a),
            &b,
        )
    }
}

/// A co-change pair: (repo, file_a, file_b), `file_a < file_b`.
type PairKey = (u64, String, String);

/// A pair's `(cochanges, ratio_permille, MODULE in file_a, MODULE in file_b)`.
type PairCounts = (u32, u32, NodeId, NodeId);

/// Audit every CO_CHANGES pair of `merged`: the unlinked ones, sorted by
/// (cochanges desc, file_a, file_b). Prints the `[cochange]` marker with
/// `surface`.
fn cochange_audit(merged: &MergedGraph, surface: &str) -> Vec<PairGap> {
    let edges: Vec<_> = merged
        .all_edges()
        .filter(|e| e.category == edge_category::CO_CHANGES)
        .collect();
    let (mut direct, mut bridged) = (0usize, 0usize);
    let mut gaps: Vec<PairGap> = Vec::new();
    let mut pairs = 0usize;
    if !edges.is_empty() {
        let links = LinkIndex::build(merged);
        let ix = &links.files;
        // One pair per (repo, file_a, file_b): the edge with the most
        // co-changes (first seen on a tie).
        let mut by_pair: BTreeMap<PairKey, PairCounts> = BTreeMap::new();
        for e in &edges {
            let (Some((repo, fa)), Some((_, fb))) = (ix.first_file(e.from), ix.first_file(e.to))
            else {
                continue;
            };
            if fa == fb {
                continue;
            }
            // The edge's ATTN through the one reader (CC.2); (0, 0) without it.
            let (cochanges, ratio) = signals::pair_counts(&e.cells).map_or((0, 0), |p| {
                (
                    p.cochanges,
                    u32::try_from(p.ratio_permille).unwrap_or(u32::MAX),
                )
            });
            let (fa, fb, ma, mb) = if fa < fb {
                (fa, fb, e.from, e.to)
            } else {
                (fb, fa, e.to, e.from)
            };
            let slot = by_pair
                .entry((repo, fa.to_string(), fb.to_string()))
                .or_insert((cochanges, ratio, ma, mb));
            if cochanges > slot.0 {
                *slot = (cochanges, ratio, ma, mb);
            }
        }
        pairs = by_pair.len();

        for ((repo, fa, fb), (cochanges, ratio_permille, ma, mb)) in by_pair {
            match links.link(repo, &fa, &fb) {
                Link::Direct => direct += 1,
                Link::Bridged => bridged += 1,
                Link::None => {
                    let language_a = crate::coverage::ext_to_language(&fa);
                    let language_b = crate::coverage::ext_to_language(&fb);
                    let cross_language = match (language_a, language_b) {
                        (Some(x), Some(y)) => x != y,
                        _ => Path::new(&fa).extension() != Path::new(&fb).extension(),
                    };
                    gaps.push(PairGap {
                        gap: CochangeGap {
                            file_a: fa,
                            file_b: fb,
                            cochanges,
                            ratio_permille,
                            language_a,
                            language_b,
                            cross_language,
                            tier: HEURISTIC,
                        },
                        module_a: ma,
                        module_b: mb,
                    });
                }
            }
        }
        // Stable: pairs already in (repo, file_a, file_b) order.
        gaps.sort_by(|x, y| {
            y.gap
                .cochanges
                .cmp(&x.gap.cochanges)
                .then_with(|| x.gap.file_a.cmp(&y.gap.file_a))
                .then_with(|| x.gap.file_b.cmp(&y.gap.file_b))
        });
    }
    eprintln!(
        "[cochange] pairs={pairs} linked={} gaps={} (direct={direct} bridged={bridged}) surface={surface}",
        direct + bridged,
        gaps.len()
    );
    gaps
}

// ---------------------------------------------------------------------------
// The root-reading categories: the overlay's and the sidecars' own rot.
// ---------------------------------------------------------------------------

struct RootCtx<'a> {
    merged: &'a MergedGraph,
    view: &'a View<'a>,
    labels: &'a BTreeMap<u64, String>,
    /// The all-repo qname index, built on first use.
    all: Option<QnameIndex>,
    /// `(from, to, category)` -> an instance of it is not an overlay edge.
    /// Built on first use.
    extracted: Option<HashMap<(u64, u64, u32), bool>>,
    want: &'a [&'static str],
}

impl RootCtx<'_> {
    /// Bind `qname` as the overlay edge stage does: the own repo first, then
    /// every repo; several hits bind the smallest NodeId.
    fn bind(&mut self, own: &QnameIndex, qname: &str) -> Option<NodeId> {
        let pick = |t: CellTarget| match t {
            CellTarget::Bound(id) | CellTarget::Ambiguous(id) => Some(id),
            _ => None,
        };
        let merged = self.merged;
        pick(own.resolve(qname, None, None)).or_else(|| {
            pick(
                self.all
                    .get_or_insert_with(|| QnameIndex::build(merged, None))
                    .resolve(qname, None, None),
            )
        })
    }

    /// True when some node of the build has a qname under `prefix::`.
    fn has_descendant(&self, prefix: &str) -> bool {
        let p = format!("{prefix}::");
        self.view.nodes.iter().any(|n| n.qname.starts_with(&p))
    }

    fn rule_rows(&mut self, repo: u64, root: &Path, rows: &mut Vec<GapRow>) {
        let Some(cfg) = glia_config::load(root) else {
            return;
        };
        let label = label_of(self.labels, repo);
        let own = QnameIndex::build(self.merged, Some(RepoId(repo)));
        let orphan = |kind: &'static str, qname: &str, line: u32, detail: String| GapRow {
            category: ORPHANED_RULE,
            qname: qname.to_string(),
            kind,
            file: Some(OVERLAY_FILE.to_string()),
            line: Some(i64::from(line)),
            detail: format!("repo={label} {detail}"),
            suggest: "remove",
            tier: FACT,
        };
        let want_orphans = self.want.contains(&ORPHANED_RULE);
        let want_redundant = self.want.contains(&REDUNDANT_RULE);

        let rejected = rejected_edge_lines(&cfg);
        for (kept, stanza) in cfg.config.edge.iter().enumerate() {
            let decl = stanza.get_ref();
            let line = cfg.line_of(stanza.span());
            let ordinal = 1 + kept + rejected.iter().filter(|l| **l < line).count();
            let from = self.bind(&own, &decl.from);
            let to = self.bind(&own, &decl.to);
            match (from, to) {
                (Some(from), Some(to)) => {
                    let Some(cat) = decl.category_id() else {
                        continue;
                    };
                    if want_redundant && self.is_extracted(from, to, cat.0) {
                        rows.push(GapRow {
                            category: REDUNDANT_RULE,
                            qname: decl.from.clone(),
                            kind: "edge",
                            file: Some(OVERLAY_FILE.to_string()),
                            line: Some(i64::from(line)),
                            detail: format!(
                                "repo={label} edge#{ordinal} {} -> {} category={} (already extracted)",
                                decl.from, decl.to, decl.category
                            ),
                            suggest: "remove",
                            tier: FACT,
                        });
                    }
                }
                (from, to) if want_orphans => {
                    let (missing, side) = match (from, to) {
                        (None, None) => (&decl.from, "from,to"),
                        (None, Some(_)) => (&decl.from, "from"),
                        _ => (&decl.to, "to"),
                    };
                    let detail = format!(
                        "edge#{ordinal} from={} to={} category={} (no node: {side})",
                        decl.from, decl.to, decl.category
                    );
                    rows.push(orphan("edge", missing, line, detail));
                }
                _ => {}
            }
        }
        if !want_orphans {
            return;
        }
        let mut anchors: Vec<(&'static str, &str, u32, String)> = Vec::new();
        for s in &cfg.config.constraint {
            if let Some(a) = s.get_ref().anchor.as_deref() {
                anchors.push((
                    "constraint",
                    a,
                    cfg.line_of(s.span()),
                    format!("constraint {}", s.get_ref().id),
                ));
            }
        }
        for s in &cfg.config.decision {
            if let Some(a) = s.get_ref().anchor.as_deref() {
                anchors.push((
                    "decision",
                    a,
                    cfg.line_of(s.span()),
                    format!("decision {}", s.get_ref().id),
                ));
            }
        }
        for (i, s) in cfg.config.note.iter().enumerate() {
            let id = s
                .get_ref()
                .id
                .clone()
                .unwrap_or_else(|| format!("note#{}", i + 1));
            anchors.push((
                "note",
                s.get_ref().anchor.as_str(),
                cfg.line_of(s.span()),
                format!("note {id}"),
            ));
        }
        // Declared knowledge (LF.4a `external::declared`) binds an anchor in
        // its own repo only, with no cross-repo fallback.
        for (kind, anchor, line, what) in anchors {
            if !matches!(
                own.resolve(anchor, None, None),
                CellTarget::Bound(_) | CellTarget::Ambiguous(_)
            ) {
                rows.push(orphan(
                    kind,
                    anchor,
                    line,
                    format!("{what} anchor={anchor} (no node)"),
                ));
            }
        }
        for q in &cfg.config.entrypoints.qnames {
            let pattern = q.get_ref().as_str();
            let line = cfg.line_of(q.span());
            let found = match pattern.strip_suffix("::*") {
                Some(prefix) => self.has_descendant(prefix),
                None => self.bind(&own, pattern).is_some(),
            };
            if !found {
                rows.push(orphan(
                    "entrypoint",
                    pattern,
                    line,
                    format!("entrypoints qname={pattern} (no node)"),
                ));
            }
        }
    }

    /// Is `(from, to, cat)` an edge of the graph that the overlay did not add?
    fn is_extracted(&mut self, from: NodeId, to: NodeId, cat: u32) -> bool {
        let merged = self.merged;
        let map = self.extracted.get_or_insert_with(|| {
            let mut m: HashMap<(u64, u64, u32), bool> = HashMap::new();
            for e in merged.all_edges() {
                *m.entry((e.from.0, e.to.0, e.category.0)).or_default() |=
                    !is_overlay_edge(&e.cells);
            }
            m
        });
        map.get(&(from.0, to.0, cat)).copied().unwrap_or(false)
    }

    fn cell_rows(&mut self, repo: u64, root: &Path, rows: &mut Vec<GapRow>) {
        let cells_path = root.join(CELLS_FILE);
        let vectors_path = root.join(VECTORS_FILE);
        if !cells_path.is_file() && !vectors_path.is_file() {
            return;
        }
        let label = label_of(self.labels, repo);
        let idx = QnameIndex::build(self.merged, Some(RepoId(repo)));
        let mut check = |file: &'static str,
                         line: u32,
                         qname: &str,
                         kind: Option<&str>,
                         hint: Option<&str>,
                         cell: &'static str| {
            if idx.resolve(qname, kind, hint) != CellTarget::Orphaned {
                return;
            }
            let mut detail = format!("repo={label} cell={cell}");
            if let Some(k) = kind {
                detail.push_str(&format!(" kind={k}"));
            }
            if let Some(h) = hint {
                detail.push_str(&format!(" hint={h}"));
            }
            rows.push(GapRow {
                category: ORPHANED_CELL,
                qname: qname.to_string(),
                kind: cell,
                file: Some(file.to_string()),
                line: Some(i64::from(line)),
                detail,
                suggest: "glia cell ls --check --rekey",
                tier: FACT,
            });
        };
        for (line, r) in numbered_rows::<CellRow>(&cells_path) {
            check(
                CELLS_FILE,
                line,
                &r.qname,
                r.kind.as_deref(),
                r.hint.as_deref(),
                cell_name(&r.cell),
            );
        }
        for (line, r) in numbered_rows::<VectorRow>(&vectors_path) {
            check(
                VECTORS_FILE,
                line,
                &r.qname,
                r.kind.as_deref(),
                r.hint.as_deref(),
                "VECTOR",
            );
        }
    }
}

/// The registry's `&'static` name for a cell type NAME, or `CELL` for one it
/// does not know (the sidecar stage rejects that row; it can still be orphaned).
fn cell_name(name: &str) -> &'static str {
    cell_type::ALL
        .iter()
        .find(|(_, n)| *n == name)
        .map_or("CELL", |(_, n)| *n)
}

/// A JSONL sidecar's rows with their 1-based line, as
/// `external_inputs::read_rows` reads them (a leading BOM stripped, blank
/// lines skipped); a line that does not parse is the sidecar stage's
/// `rejected`, not a gap, and is skipped.
fn numbered_rows<T: DeserializeOwned>(path: &Path) -> Vec<(u32, T)> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let bytes = bytes.strip_prefix("\u{feff}".as_bytes()).unwrap_or(&bytes);
    let mut out = Vec::new();
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        let Ok(l) = std::str::from_utf8(line) else {
            continue;
        };
        let l = l.trim();
        if l.is_empty() {
            continue;
        }
        if let Ok(row) = serde_json::from_str::<T>(l) {
            out.push((u32::try_from(i + 1).unwrap_or(u32::MAX), row));
        }
    }
    out
}

/// The 1-based lines of the `[[edge]]` stanzas the loader dropped, read from
/// its errors (`.glia/overlay.toml:<line>: [[edge]] ... (stanza dropped)`),
/// so a row's `edge#<n>` is the number the overlay edge stage prints. The same
/// read as `external::overlay`'s private helper of that name, which this
/// module cannot reach; if that helper becomes `pub(crate)`, call it instead.
fn rejected_edge_lines(cfg: &LoadedConfig) -> Vec<u32> {
    let prefix = format!("{OVERLAY_FILE}:");
    cfg.errors
        .iter()
        .filter_map(|e| {
            let (line, msg) = e.strip_prefix(prefix.as_str())?.split_once(": ")?;
            msg.starts_with("[[edge]] ")
                .then(|| line.parse().ok())
                .flatten()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::glia_config::parse_str;

    #[test]
    fn endpoint_parts_strip_owner_and_prefix() {
        assert_eq!(
            endpoint_parts("endpoint:GET:/users"),
            Some(("GET", "/users"))
        );
        assert_eq!(
            endpoint_parts("endpoint:GET:<unresolved>"),
            Some(("GET", "<unresolved>"))
        );
        assert_eq!(
            endpoint_parts("endpoint:POST:/a @web"),
            Some(("POST", "/a"))
        );
        assert_eq!(endpoint_parts("GET /users"), None);
        assert_eq!(route_method("GET /users @api"), Some("GET"));
        assert_eq!(route_method("route:/users"), None);
        assert_eq!(route_method("page:/a b"), None);
    }

    #[test]
    fn rejected_lines_match_the_edge_stage() {
        let cfg = parse_str(
            "version = 1\n\n[[edge]]\nfrom = \"a\"\nto = \"b\"\ncategory = \"DEFINES\"\n\n[[edge]]\nfrom = \"a\"\nto = \"b\"\ncategory = \"CALLS\"\n",
        );
        assert_eq!(rejected_edge_lines(&cfg), [3], "{:?}", cfg.errors);
        assert_eq!(overlay_rule_count(&cfg), 1);
    }

    #[test]
    fn unknown_category_is_an_error_and_empty_graph_is_empty() {
        let m = MergedGraph::new(Vec::new());
        let o = GapsOptions {
            category: Some("nope".into()),
            ..GapsOptions::default()
        };
        assert!(gaps_report(&m, &[], &o).is_err());
        let r = gaps_report(&m, &[], &GapsOptions::default()).expect("known categories");
        assert!(r.rows.is_empty());
        assert_eq!(r.skipped, ROOT_CATEGORIES);
        assert_eq!(r.counts.len(), CATEGORIES.len() - ROOT_CATEGORIES.len());
    }

    #[test]
    fn numbered_rows_keep_file_lines() {
        let d = std::env::temp_dir().join(format!("glia_gaps_rows_{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("mkdir");
        let p = d.join("cells.jsonl");
        std::fs::write(
            &p,
            "\u{feff}{\"qname\":\"a\",\"cell\":\"CONV\",\"entry\":{}}\n\nnot json\n{\"qname\":\"b\",\"cell\":\"CONV\",\"entry\":{}}\n",
        )
        .expect("write");
        let rows = numbered_rows::<CellRow>(&p);
        std::fs::remove_dir_all(&d).ok();
        let got: Vec<(u32, &str)> = rows.iter().map(|(l, r)| (*l, r.qname.as_str())).collect();
        assert_eq!(got, [(1, "a"), (4, "b")]);
    }
}
