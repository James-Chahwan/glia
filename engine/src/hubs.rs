//! Hubs (CD.4b): ranked, located hub rows (fan-in utilities, fan-out
//! orchestrators, cross-service connectors) with per-category counts, caller
//! and callee services, HITS scores and liveness; tests and docs are left out
//! by default. The scores are `glia_activation::algo::hubs`. Public slot,
//! reached by module path (`glia_engine::hubs::<item>`).
//!
//! COUNTED EDGES. The domain's `carry_edges` minus TESTS, DOCUMENTS and
//! DEPENDS_ON (test coverage, docs and manifest dependencies are not load),
//! or exactly [`HubArgs::category`] when given (any registered category name,
//! ASCII case ignored; an unknown name is an absence `no_match`).
//!
//! TEST NODES are left out of the index unless [`HubArgs::include_tests`]: a
//! node with ORIGIN provenance `test_fixture` (stamped by the build from the
//! file path and qname, e.g. every `tests::...` qname), or a FUNCTION /
//! METHOD whose name starts with one of the entry rule's named prefixes
//! (`test` / `Test` in `CODE_TABLES.entry`). Their edges are dropped with
//! them, so test fan-out never inflates a production node's fan-in.
//!
//! SCOPE (a path or a project label, resolved once) picks the ROWS: a row is
//! a node located under it (an unlocatable node is kept, as
//! `answers::node_in_scope` keeps it). An edge counts when either end is a
//! row candidate, so a scoped hub keeps the callers it has outside the scope,
//! and their services.
//!
//! THE INDEX is one `Adjacency` over the counted edges, with the nodes in a
//! path-independent order (qname, kind, file, id) and the edges sorted by
//! their ends' positions: node ids hash the repo's path, so an id-ordered
//! index would sum HITS floats in a different order for the same code checked
//! out elsewhere. Degree is `algo::hubs::degree_table` over it, HITS
//! `algo::hubs::hits` ([`HITS_ITERATIONS`] rounds) over the same edges.
//!
//! THRESHOLDS. `p99_in` / `p99_out` are nearest-rank 99th percentiles of the
//! NON-ZERO fan-ins / fan-outs of the row candidates (a node with no counted
//! edge is no evidence of what "high" means, and would lower the bar on a
//! graph full of structure). A node joins the fan-in list at `fan_in >=
//! max(min_degree, p99_in)` ([`utility_hubs`] is that set), the fan-out list
//! likewise; `min_degree` floors the percentile on small graphs. The
//! cross-service list holds nodes whose callers or callees sit in two or more
//! `glia arch` services ([`default_keying`] + [`service_of`] of each
//! neighbour's located file; an unlocated neighbour is skipped, never
//! bucketed). Orders: fan-in / fan-out by that degree descending;
//! cross-service by distinct services (callers and callees together)
//! descending, then fan-in, then fan-out; ties by qname. Each list is cut at
//! [`HubArgs::top`] (`0` keeps every row).
//!
//! LABELS, from the uncut qualifications (so `top` never changes a label):
//! [`UTILITY`] qualifies on fan-in with fan-out <= [`SIDE_MAX`];
//! [`ORCHESTRATOR`] qualifies on fan-out with fan-in <= [`SIDE_MAX`];
//! [`BOTTLENECK`] every other qualifying node (both lists, or one list and
//! more than [`SIDE_MAX`] edges the other way); [`CONNECTOR`] is in the
//! cross-service list only. Every row is tier [`DERIVED`]: counted from
//! observed edges. Dominators stay out (the gated list): a hub is a count,
//! not "every path passes through".
//!
//! fired_on marker, once per answer (row counts are the lists as returned):
//! `[hubs] nodes=<N> edges=<M> p99_in=<a> p99_out=<b> fan_in=<x> fan_out=<y> cross_service=<z> hits_iters=20 surface=<engine|cli|py>`

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_activation::algo::hubs::{DegreeTable, degree_table, hits};
use glia_activation::algo::{Adjacency, CategorySet, GraphSource};
use glia_code_domain::{cell_type, edge_category};
use glia_core::{Cell, CellPayload, Edge, EdgeCategoryId, NodeId, NodeKindId};
use glia_graph::MergedGraph;

use crate::absence::{self, Absence};
use crate::answers::{Locator, entrypoint_reachable, in_scope, resolve_scope};
use crate::arch::{ServiceKeying, default_keying, service_of};
use crate::profile::CODE_PROFILE;

/// Rows per list by default.
pub const DEFAULT_TOP: usize = 20;
/// The degree floor under the p99 thresholds.
pub const DEFAULT_MIN_DEGREE: u32 = 5;
/// HITS rounds (a fixed count: reproducible, no convergence test).
pub const HITS_ITERATIONS: u32 = 20;
/// The most edges the other way a [`UTILITY`] / [`ORCHESTRATOR`] may have.
pub const SIDE_MAX: u32 = 3;
/// Per-category counts kept per row.
pub const BY_CATEGORY_TOP: usize = 5;

/// [`HubRow::label`]: many callers, few callees.
pub const UTILITY: &str = "utility";
/// [`HubRow::label`]: many callees, few callers.
pub const ORCHESTRATOR: &str = "orchestrator";
/// [`HubRow::label`]: load both ways.
pub const BOTTLENECK: &str = "bottleneck";
/// [`HubRow::label`]: joins services without a high degree.
pub const CONNECTOR: &str = "connector";
/// [`HubRow::tier`] of every row.
pub const DERIVED: &str = "derived";
/// [`HubArgs::surface`] when the engine is called directly.
pub const SURFACE_ENGINE: &str = "engine";

const PRIMITIVE: &str = "hubs";
/// Carry edges that are not load: test coverage, docs, manifest dependencies.
const UNCOUNTED: [EdgeCategoryId; 3] = [
    edge_category::TESTS,
    edge_category::DOCUMENTS,
    edge_category::DEPENDS_ON,
];
/// p99, per mille.
const P99_MILLI: u64 = 990;

/// What [`hubs`] ranks. Start from `default()` and set fields.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct HubArgs {
    /// Rows under this path or project label only.
    pub scope: Option<String>,
    /// Rows per list ([`DEFAULT_TOP`]); `0` keeps every qualifying row.
    pub top: usize,
    /// Count only this edge category (a registered name, e.g. `CALLS`).
    pub category: Option<String>,
    /// The floor under the p99 thresholds ([`DEFAULT_MIN_DEGREE`]).
    pub min_degree: u32,
    /// Keep test nodes and their edges in the index.
    pub include_tests: bool,
    /// Who asked, for the marker: [`SURFACE_ENGINE`], `cli` or `py`.
    pub surface: &'static str,
}

impl Default for HubArgs {
    fn default() -> Self {
        HubArgs {
            scope: None,
            top: DEFAULT_TOP,
            category: None,
            min_degree: DEFAULT_MIN_DEGREE,
            include_tests: false,
            surface: SURFACE_ENGINE,
        }
    }
}

/// One hub.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct HubRow {
    pub qname: String,
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// [`UTILITY`], [`ORCHESTRATOR`], [`BOTTLENECK`] or [`CONNECTOR`].
    pub label: &'static str,
    /// Counted edges in.
    pub fan_in: u32,
    /// Counted edges out.
    pub fan_out: u32,
    /// `(category, in, out)`, the [`BY_CATEGORY_TOP`] busiest categories by
    /// in + out, then by name.
    pub by_category: Vec<(&'static str, u32, u32)>,
    /// Distinct `glia arch` services of the located callers, sorted.
    pub caller_services: Vec<String>,
    /// Distinct `glia arch` services of the located callees, sorted.
    pub callee_services: Vec<String>,
    /// HITS authority (unit L2 norm over the index).
    pub authority: f64,
    /// HITS hub score (unit L2 norm over the index).
    pub hub: f64,
    /// Reachable from an entrypoint.
    pub live: bool,
    /// Always [`DERIVED`].
    pub tier: &'static str,
}

/// The hubs answer: three ranked lists and the thresholds behind them.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct HubsAnswer {
    pub fan_in: Vec<HubRow>,
    pub fan_out: Vec<HubRow>,
    pub cross_service: Vec<HubRow>,
    /// Row candidates: nodes in scope, tests left out unless asked for.
    pub nodes: usize,
    /// Counted edges indexed.
    pub edges: usize,
    /// Nearest-rank p99 of the candidates' non-zero fan-ins.
    pub p99_in: u32,
    /// Nearest-rank p99 of the candidates' non-zero fan-outs.
    pub p99_out: u32,
    /// `Some` iff all three lists are empty.
    pub absence: Option<Absence>,
}

/// Rank the nodes of `merged` by structural load. `repo_labels` name the
/// services exactly as `glia arch` names them (`GenerateResult::repo_labels`).
pub fn hubs(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    args: &HubArgs,
) -> HubsAnswer {
    let loc = Locator::new(merged);
    let Some(index) = HubIndex::build(merged, &loc, args) else {
        let name = args.category.as_deref().unwrap_or_default();
        let note =
            format!("no edge category is named `{name}`; pass a registered name such as CALLS");
        let absence = absence::empty(merged, PRIMITIVE, &query(args), "no_match", note, &[], None);
        let answer = HubsAnswer {
            fan_in: Vec::new(),
            fan_out: Vec::new(),
            cross_service: Vec::new(),
            nodes: 0,
            edges: 0,
            p99_in: 0,
            p99_out: 0,
            absence: Some(absence),
        };
        marker(&answer, args);
        return answer;
    };

    let (fan_in_set, t_in) = utility_hubs(&index, args.min_degree);
    let p99_in = index.p99(&index.degree.inn);
    let p99_out = index.p99(&index.degree.out);
    let t_out = args.min_degree.max(p99_out);
    let services = Services::fold(merged, &index, repo_labels);
    let scores = hits(&index.adj, HITS_ITERATIONS);

    let (inn, out) = (&index.degree.inn, &index.degree.out);
    let in_q = |ix: usize| fan_in_set.contains(&index.adj.id(ix as u32).0);
    let out_q = |ix: usize| out[ix] >= t_out;
    let cross_q = |ix: usize| services.callers[ix].len() >= 2 || services.callees[ix].len() >= 2;
    let candidates: Vec<usize> = (0..index.known).filter(|&ix| index.scoped[ix]).collect();

    let mut by_in: Vec<usize> = candidates.iter().copied().filter(|&ix| in_q(ix)).collect();
    by_in.sort_by(|&a, &b| inn[b].cmp(&inn[a]).then(a.cmp(&b)));
    let mut by_out: Vec<usize> = candidates.iter().copied().filter(|&ix| out_q(ix)).collect();
    by_out.sort_by(|&a, &b| out[b].cmp(&out[a]).then(a.cmp(&b)));
    let mut by_cross: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&ix| cross_q(ix))
        .collect();
    by_cross.sort_by(|&a, &b| {
        services
            .distinct(b)
            .cmp(&services.distinct(a))
            .then(inn[b].cmp(&inn[a]))
            .then(out[b].cmp(&out[a]))
            .then(a.cmp(&b))
    });
    for list in [&mut by_in, &mut by_out, &mut by_cross] {
        if args.top > 0 {
            list.truncate(args.top);
        }
    }

    let live = if by_in.is_empty() && by_out.is_empty() && by_cross.is_empty() {
        HashSet::new()
    } else {
        entrypoint_reachable(merged)
    };
    let row = |ix: usize| -> HubRow {
        let id = index.adj.id(ix as u32);
        let at = loc.locate(id);
        let (fi, fo) = (inn[ix], out[ix]);
        HubRow {
            qname: at.qname,
            kind: at.kind,
            file: at.file,
            line: at.line,
            label: label(in_q(ix), out_q(ix), fi, fo),
            fan_in: fi,
            fan_out: fo,
            by_category: by_category(&index.degree, ix),
            caller_services: services.names(&services.callers[ix]),
            callee_services: services.names(&services.callees[ix]),
            authority: scores.authority[ix],
            hub: scores.hub[ix],
            live: live.contains(&id),
            tier: DERIVED,
        }
    };
    let fan_in: Vec<HubRow> = by_in.iter().map(|&ix| row(ix)).collect();
    let fan_out: Vec<HubRow> = by_out.iter().map(|&ix| row(ix)).collect();
    let cross_service: Vec<HubRow> = by_cross.iter().map(|&ix| row(ix)).collect();

    let absence = (fan_in.is_empty() && fan_out.is_empty() && cross_service.is_empty())
        .then(|| no_rows(merged, args, &index, t_in, t_out));
    let answer = HubsAnswer {
        fan_in,
        fan_out,
        cross_service,
        nodes: candidates.len(),
        edges: index.adj.kept_edges(),
        p99_in,
        p99_out,
        absence,
    };
    marker(&answer, args);
    answer
}

/// The fan-in hub set of `index`, the nodes [`hubs`] lists as fan-in hubs
/// before the `top` cut: the ids of the row candidates with `fan_in >=
/// max(min_degree, p99_in)`, and that threshold. CD.4e (duplicate flows)
/// ignores these nodes, which every flow reaches; it builds the index with
/// `HubIndex::build(merged, &loc, &args)` from `HubArgs::default()` with its
/// scope set, and passes [`DEFAULT_MIN_DEGREE`].
pub(crate) fn utility_hubs(index: &HubIndex, min_degree: u32) -> (BTreeSet<u64>, u32) {
    let threshold = min_degree.max(index.p99(&index.degree.inn));
    let set = (0..index.known)
        .filter(|&ix| index.scoped[ix] && index.degree.inn[ix] >= threshold)
        .map(|ix| index.adj.id(ix as u32).0)
        .collect();
    (set, threshold)
}

/// The counted-edge index one answer ranks over. Dense ids `0..known` are the
/// indexed nodes in canonical order; the rest are edge endpoints that are no
/// node, which are never rows.
pub(crate) struct HubIndex {
    adj: Adjacency,
    degree: DegreeTable,
    /// Indexed nodes (the dense ids below this are nodes).
    known: usize,
    /// Per node: a row candidate (under the scope).
    scoped: Vec<bool>,
    /// Per node: the repo of its first graph.
    repo: Vec<u64>,
    /// Per node: its located file.
    file: Vec<Option<String>>,
    /// Every indexed edge as dense `(from, to)`, in index edge order.
    pairs: Vec<(u32, u32)>,
    /// The counted categories' names, for an absence.
    mechanisms: Vec<&'static str>,
    /// Test nodes left out.
    tests_left_out: usize,
}

/// One node while the index is built.
struct NodeInfo<'g> {
    id: NodeId,
    repo: u64,
    qname: &'g str,
    kind: u32,
    file: Option<String>,
    scoped: bool,
}

/// The nodes and edges handed to `Adjacency::build`, in canonical order.
struct HubSource<'g> {
    nodes: Vec<NodeId>,
    edges: Vec<&'g Edge>,
}

impl GraphSource for HubSource<'_> {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter().copied())
    }
}

impl HubIndex {
    /// Index `merged` for `args` (scope, category, include_tests); `None`
    /// when `args.category` names no registered edge category.
    pub(crate) fn build(
        merged: &MergedGraph,
        loc: &Locator<'_>,
        args: &HubArgs,
    ) -> Option<HubIndex> {
        let counted = counted_categories(args.category.as_deref())?;
        let count = CategorySet::of(&counted);
        let scope = args.scope.as_deref().map(|s| resolve_scope(merged, s));

        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut tests: HashSet<NodeId> = HashSet::new();
        let mut infos: Vec<NodeInfo<'_>> = Vec::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if !seen.insert(n.id) {
                    continue;
                }
                let kind = g.nav.kind_by_id.get(&n.id).copied();
                let name = g
                    .nav
                    .name_by_id
                    .get(&n.id)
                    .map(String::as_str)
                    .unwrap_or("");
                if !args.include_tests && is_test_node(&n.cells, kind, name) {
                    tests.insert(n.id);
                    continue;
                }
                let file = loc.file_of(n.id);
                let scoped = match (scope.as_deref(), file.as_deref()) {
                    (Some(s), Some(f)) => in_scope(f, s),
                    _ => true,
                };
                infos.push(NodeInfo {
                    id: n.id,
                    repo: g.repo.0,
                    qname: g
                        .nav
                        .qname_by_id
                        .get(&n.id)
                        .map(String::as_str)
                        .unwrap_or(""),
                    kind: kind.map_or(0, |k| k.0),
                    file,
                    scoped,
                });
            }
        }
        infos.sort_by(|a, b| {
            a.qname
                .cmp(b.qname)
                .then(a.kind.cmp(&b.kind))
                .then(a.file.cmp(&b.file))
                .then(a.id.0.cmp(&b.id.0))
        });
        let pos: HashMap<NodeId, usize> =
            infos.iter().enumerate().map(|(p, i)| (i.id, p)).collect();

        // An edge whose end is no node sorts after every node, by id.
        let key = |id: NodeId| pos.get(&id).map_or((1u8, id.0), |&p| (0u8, p as u64));
        let touches_scope = |id: NodeId| pos.get(&id).is_some_and(|&p| infos[p].scoped);
        let mut edges: Vec<((u8, u64), (u8, u64), u32, &Edge)> = merged
            .all_edges()
            .filter(|e| count.contains(e.category))
            .filter(|e| !tests.contains(&e.from) && !tests.contains(&e.to))
            .filter(|e| touches_scope(e.from) || touches_scope(e.to))
            .map(|e| (key(e.from), key(e.to), e.category.0, e))
            .collect();
        // Stable: same-key edges are interchangeable for every count and score.
        edges.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));

        let source = HubSource {
            nodes: infos.iter().map(|i| i.id).collect(),
            edges: edges.iter().map(|e| e.3).collect(),
        };
        let adj = Adjacency::build(&source, &count);
        let degree = degree_table(&adj, &count);
        let pairs = source
            .edges
            .iter()
            .filter_map(|e| Some((adj.index_of(e.from)?, adj.index_of(e.to)?)))
            .collect();
        Some(HubIndex {
            degree,
            known: infos.len(),
            scoped: infos.iter().map(|i| i.scoped).collect(),
            repo: infos.iter().map(|i| i.repo).collect(),
            file: infos.into_iter().map(|i| i.file).collect(),
            pairs,
            adj,
            mechanisms: counted.iter().map(|&c| edge_category::name(c)).collect(),
            tests_left_out: tests.len(),
        })
    }

    /// Nearest-rank p99 of the non-zero degrees in `degrees` of the row
    /// candidates; 0 when none is non-zero.
    fn p99(&self, degrees: &[u32]) -> u32 {
        let mut xs: Vec<u32> = (0..self.known)
            .filter(|&ix| self.scoped[ix])
            .map(|ix| degrees[ix])
            .filter(|&d| d > 0)
            .collect();
        if xs.is_empty() {
            return 0;
        }
        xs.sort_unstable();
        let rank = (P99_MILLI * xs.len() as u64).div_ceil(1000).max(1);
        xs[(rank - 1) as usize]
    }
}

/// The counted categories: the carry edges minus [`UNCOUNTED`], or exactly
/// the one `category` names (ASCII case ignored); `None` for an unknown name.
fn counted_categories(category: Option<&str>) -> Option<Vec<EdgeCategoryId>> {
    match category {
        None => Some(
            CODE_PROFILE
                .tables
                .carry_edges
                .iter()
                .copied()
                .filter(|c| !UNCOUNTED.contains(c))
                .collect(),
        ),
        Some(name) => {
            let name = name.trim();
            edge_category::ALL
                .iter()
                .find(|(_, n)| n.eq_ignore_ascii_case(name))
                .map(|(c, _)| vec![*c])
        }
    }
}

/// A test node: ORIGIN provenance `test_fixture`, or a FUNCTION / METHOD the
/// entry rule's named prefixes (`test` / `Test`) match.
fn is_test_node(cells: &[Cell], kind: Option<NodeKindId>, name: &str) -> bool {
    provenance_is(cells, "test_fixture")
        || kind.is_some_and(|k| {
            CODE_PROFILE.tables.entry.named.iter().any(|rule| {
                rule.kinds.contains(&k) && rule.prefixes.iter().any(|p| name.starts_with(p))
            })
        })
}

/// An ORIGIN cell names `provenance`.
fn provenance_is(cells: &[Cell], provenance: &str) -> bool {
    let needle = format!("\"provenance\":\"{provenance}\"");
    cells.iter().any(|c| {
        c.kind == cell_type::ORIGIN && matches!(&c.payload, CellPayload::Json(j) | CellPayload::Text(j) if j.contains(&needle))
    })
}

/// Distinct caller / callee services per dense id, as interned service ids.
struct Services {
    names: Vec<String>,
    callers: Vec<Vec<u32>>,
    callees: Vec<Vec<u32>>,
}

impl Services {
    /// One pass over the indexed edges: each end's service joins the other
    /// end's callee / caller set. A service is resolved once per node.
    fn fold(merged: &MergedGraph, index: &HubIndex, labels: &BTreeMap<u64, String>) -> Services {
        let n = index.adj.len();
        let keying: ServiceKeying = default_keying(merged);
        let mut memo: Vec<Option<Option<u32>>> = vec![None; n];
        let mut ids: HashMap<String, u32> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        let mut service = |ix: u32| -> Option<u32> {
            let i = ix as usize;
            if let Some(s) = memo[i] {
                return s;
            }
            // Past `known` is an endpoint that is no node: never bucketed.
            let s = (i < index.known)
                .then(|| index.file[i].as_deref())
                .flatten()
                .map(|f| service_of(f, index.repo[i], &keying, labels))
                .map(|name| {
                    *ids.entry(name.clone()).or_insert_with(|| {
                        names.push(name);
                        (names.len() - 1) as u32
                    })
                });
            memo[i] = Some(s);
            s
        };
        let mut callers: Vec<Vec<u32>> = vec![Vec::new(); n];
        let mut callees: Vec<Vec<u32>> = vec![Vec::new(); n];
        for &(from, to) in &index.pairs {
            if let Some(s) = service(from) {
                callers[to as usize].push(s);
            }
            if let Some(s) = service(to) {
                callees[from as usize].push(s);
            }
        }
        for set in callers.iter_mut().chain(callees.iter_mut()) {
            set.sort_unstable();
            set.dedup();
        }
        Services {
            names,
            callers,
            callees,
        }
    }

    /// Sorted names of the interned `ids`.
    fn names(&self, ids: &[u32]) -> Vec<String> {
        let mut out: Vec<String> = ids
            .iter()
            .map(|&s| self.names[s as usize].clone())
            .collect();
        out.sort();
        out
    }

    /// Distinct services on either side of `ix`.
    fn distinct(&self, ix: usize) -> usize {
        let mut all: Vec<u32> = self.callers[ix]
            .iter()
            .chain(&self.callees[ix])
            .copied()
            .collect();
        all.sort_unstable();
        all.dedup();
        all.len()
    }
}

/// The label of a row from its qualifications.
fn label(in_q: bool, out_q: bool, fan_in: u32, fan_out: u32) -> &'static str {
    match (in_q, out_q) {
        (true, false) if fan_out <= SIDE_MAX => UTILITY,
        (false, true) if fan_in <= SIDE_MAX => ORCHESTRATOR,
        (false, false) => CONNECTOR,
        _ => BOTTLENECK,
    }
}

/// `(category, in, out)` at `ix`, busiest [`BY_CATEGORY_TOP`] first.
fn by_category(degree: &DegreeTable, ix: usize) -> Vec<(&'static str, u32, u32)> {
    let mut rows: Vec<(&'static str, u32, u32)> = degree
        .of(ix as u32)
        .map(|(c, out, inn)| (edge_category::name(c), inn, out))
        .collect();
    rows.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)).then(a.0.cmp(b.0)));
    rows.truncate(BY_CATEGORY_TOP);
    rows
}

/// The query an absence names.
fn query(args: &HubArgs) -> String {
    let scope = args
        .scope
        .as_deref()
        .map(|s| format!(" scope={s}"))
        .unwrap_or_default();
    format!(
        "category={} min_degree={} include_tests={}{scope}",
        args.category.as_deref().unwrap_or("carry"),
        args.min_degree,
        args.include_tests
    )
}

/// Why no list has a row.
fn no_rows(
    merged: &MergedGraph,
    args: &HubArgs,
    index: &HubIndex,
    t_in: u32,
    t_out: u32,
) -> Absence {
    let candidates = (0..index.known).filter(|&ix| index.scoped[ix]).count();
    let scope = args
        .scope
        .as_deref()
        .map(|s| format!(" under scope `{s}`"))
        .unwrap_or_default();
    let note = format!(
        "no node{scope} has fan-in >= {t_in} or fan-out >= {t_out}, and none has callers or callees in two services ({candidates} nodes, {} counted edges; {} test nodes left out)",
        index.adj.kept_edges(),
        index.tests_left_out
    );
    absence::empty(
        merged,
        PRIMITIVE,
        &query(args),
        "no_match",
        note,
        &index.mechanisms,
        None,
    )
}

/// The CD.4b fired_on line.
fn marker(a: &HubsAnswer, args: &HubArgs) {
    eprintln!(
        "[hubs] nodes={} edges={} p99_in={} p99_out={} fan_in={} fan_out={} cross_service={} hits_iters={HITS_ITERATIONS} surface={}",
        a.nodes,
        a.edges,
        a.p99_in,
        a.p99_out,
        a.fan_in.len(),
        a.fan_out.len(),
        a.cross_service.len(),
        args.surface
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_cover_every_qualification() {
        assert_eq!(label(true, false, 40, 0), UTILITY);
        assert_eq!(label(true, false, 40, SIDE_MAX), UTILITY);
        assert_eq!(label(true, false, 40, SIDE_MAX + 1), BOTTLENECK);
        assert_eq!(label(false, true, 0, 30), ORCHESTRATOR);
        assert_eq!(label(false, true, SIDE_MAX + 1, 30), BOTTLENECK);
        assert_eq!(label(true, true, 40, 30), BOTTLENECK);
        assert_eq!(label(false, false, 2, 2), CONNECTOR);
    }

    #[test]
    fn counted_categories_drop_tests_docs_and_manifests() {
        let carry = counted_categories(None).expect("the default set");
        for c in UNCOUNTED {
            assert!(!carry.contains(&c), "{}", edge_category::name(c));
        }
        assert!(
            carry.contains(&edge_category::CALLS) && carry.contains(&edge_category::ACCESSES_DATA)
        );
        assert_eq!(
            counted_categories(Some("calls")),
            Some(vec![edge_category::CALLS])
        );
        assert_eq!(
            counted_categories(Some("DEPENDS_ON")),
            Some(vec![edge_category::DEPENDS_ON])
        );
        assert_eq!(counted_categories(Some("NO_SUCH")), None);
    }

    #[test]
    fn named_test_functions_are_test_nodes() {
        use glia_code_domain::node_kind;
        assert!(is_test_node(&[], Some(node_kind::FUNCTION), "test_login"));
        assert!(is_test_node(&[], Some(node_kind::METHOD), "TestLogin"));
        assert!(!is_test_node(&[], Some(node_kind::FUNCTION), "login"));
        assert!(
            !is_test_node(&[], Some(node_kind::CLASS), "TestLogin"),
            "the named rule is FUNCTION / METHOD only"
        );
        let origin = Cell {
            kind: cell_type::ORIGIN,
            payload: CellPayload::Json(r#"{"provenance":"test_fixture"}"#.to_string()),
        };
        assert!(is_test_node(
            std::slice::from_ref(&origin),
            Some(node_kind::CLASS),
            "Helper"
        ));
    }

    /// `utility_hubs` is the fan-in list [`hubs`] cuts: same threshold, same
    /// ids, before `top`.
    #[test]
    fn utility_hubs_is_the_uncut_fan_in_set() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut callers = String::from("from util.log import log\n\n");
        for i in 0..7 {
            callers.push_str(&format!("\ndef f{i}():\n    return log({i})\n\n"));
        }
        for (rel, src) in [
            ("util/log.py", "def log(m):\n    return m\n".to_string()),
            ("app/calls.py", callers),
        ] {
            let p = tmp.path().join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            std::fs::write(p, src).expect("write");
        }
        let r = crate::generate_one(tmp.path().to_str().expect("utf-8")).expect("generate_one");
        let loc = Locator::new(&r.merged);
        let args = HubArgs::default();
        let index = HubIndex::build(&r.merged, &loc, &args).expect("the default categories");
        let (set, threshold) = utility_hubs(&index, DEFAULT_MIN_DEGREE);
        assert_eq!(
            threshold, 7,
            "p99 of the non-zero fan-ins is log's 7, above the floor"
        );
        let answer = hubs(&r.merged, &r.repo_labels, &args);
        let listed: Vec<&str> = answer.fan_in.iter().map(|h| h.qname.as_str()).collect();
        assert_eq!(listed, vec!["util::log::log"]);
        assert_eq!(set.len(), 1);
        let (floored, t) = utility_hubs(&index, 50);
        assert!(
            floored.is_empty() && t == 50,
            "min_degree floors the percentile"
        );
    }
}
