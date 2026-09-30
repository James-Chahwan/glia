//! Hotspots (CC.10a): code that changes often AND sits where much depends on
//! it. Each row joins a churn rank (git history) with a centrality rank (one
//! global PageRank) and shows both, with the population they range over;
//! there is no composite score to over-read. Public slot, reached by module
//! path (`glia_engine::hotspots::<item>`).
//!
//! POPULATIONS, one per level, read through `external::signals` (CC.2):
//! - `module`: MODULE nodes whose churn ATTN (`source: git`, LF.5b) counts
//!   `commits >= min_churn`; churn is `commits`, the tie-break
//!   `lines_added + lines_deleted`;
//! - `symbol`: FUNCTION / METHOD / CLASS nodes whose blame ATTN
//!   (`source: git-blame`) counts `span_changes >= min_churn`; churn is
//!   `span_changes`, the tie-break the newest blame time.
//!
//! A node with ORIGIN provenance `test_fixture`, `generated` or
//! `generated_proto` is left out unless `include_tests` (test code churns
//! with the code it tests and would crowd the list). `scope` (a path or a
//! project label, resolved once) keeps the nodes located under it; an
//! unlocatable node is kept, as `answers::node_in_scope` keeps it.
//!
//! CENTRALITY is ONE `MergedGraph::activate` per answer, and only when a
//! population is non-empty: every node is a seed (a uniform restart, i.e.
//! classic PageRank), damping [`DAMPING`], direction Forward (importance flows
//! to what is called, imported, used), over the domain's
//! [`CENTRALITY_PRESET`] weights (`code_domain::profile::CODE_TABLES`: DEFINES,
//! CONTAINS, DOCUMENTS and TESTS weigh 0 there, CO_CHANGES weighs 0 in the
//! base). A symbol's centrality is its PageRank; a module's is its own plus
//! that of every node whose nearest MODULE up `nav.parent_of` is it (a module
//! is as central as what it defines).
//!
//! RANKS, per population: competition ranking (1 = highest; equal keys share
//! the lower rank: 1, 2, 2, 4). Churn ranks by (churn, tie-break) descending,
//! centrality by PageRank descending, compared with `f64::total_cmp`, so the
//! order is total and platform-stable. Rows order by (churn rank x centrality
//! rank, the larger of the two, qname, node id), ascending; the product only
//! orders and is never emitted, and no float reaches the output. The list is
//! cut to `top` per level (`0` = every ranked row). Every row is tier
//! `heuristic`: history is not a code reference and centrality is a model.
//!
//! An answer with no row carries an [`Absence`]: `no_history` when no node
//! carries history ATTN (the note says to run `glia history sync`), else
//! `no_match`, naming the threshold and what was left out.
//!
//! fired_on marker, once per answer (shown / ranked per level; `head` is the
//! newest history time, `-` without history):
//! `[hotspots] modules=<m>/<M> symbols=<s>/<S> pagerank_iterations=<i> head=<t>`

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use glia_activation::{Direction, Specificity};
use glia_code_domain::profile::CODE_TABLES;
use glia_code_domain::{cell_type, node_kind};
use glia_core::{Cell, CellPayload, NodeId, NodeKindId};
use glia_graph::{MergedGraph, RepoGraph};

use crate::absence::{self, Absence};
use crate::answers::{Locator, in_scope, resolve_scope};
use crate::external::signals;

/// [`HotspotArgs::level`]: module rows only.
pub const LEVEL_MODULE: &str = "module";
/// [`HotspotArgs::level`]: symbol rows only.
pub const LEVEL_SYMBOL: &str = "symbol";
/// [`HotspotArgs::level`]: both lists (the default).
pub const LEVEL_BOTH: &str = "both";
/// Every level, for a surface's possible values.
pub const LEVELS: [&str; 3] = [LEVEL_MODULE, LEVEL_SYMBOL, LEVEL_BOTH];
/// Rows per level by default.
pub const DEFAULT_TOP: usize = 20;
/// Smallest churn (module commits / symbol span changes) that ranks.
pub const DEFAULT_MIN_CHURN: u32 = 2;
/// The domain's activation preset centrality is computed over.
pub const CENTRALITY_PRESET: &str = "centrality";
/// Classic PageRank's damping.
pub const DAMPING: f64 = 0.85;
/// The tier of every row.
pub const HEURISTIC: &str = "heuristic";

const PRIMITIVE: &str = "hotspots";
const SYMBOL_KINDS: [NodeKindId; 3] = [node_kind::FUNCTION, node_kind::METHOD, node_kind::CLASS];
const EXCLUDED_PROVENANCE: [&str; 3] = ["test_fixture", "generated", "generated_proto"];
/// A `parent_of` chain longer than this is treated as a cycle.
const MAX_PARENT_STEPS: usize = 256;

/// The level constant spelled `s`, if it is one.
pub fn parse_level(s: &str) -> Option<&'static str> {
    LEVELS.into_iter().find(|l| *l == s)
}

/// What [`hotspots`] ranks. Start from `default()` and set fields.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct HotspotArgs {
    /// Rows per level ([`DEFAULT_TOP`]); `0` keeps every ranked row.
    pub top: usize,
    /// [`LEVEL_MODULE`], [`LEVEL_SYMBOL`] or [`LEVEL_BOTH`]; any other value
    /// reads as both.
    pub level: &'static str,
    /// Smallest churn that joins a population ([`DEFAULT_MIN_CHURN`]).
    pub min_churn: u32,
    /// Keep test / generated nodes in the populations.
    pub include_tests: bool,
    /// Keep only nodes under this path or project label.
    pub scope: Option<String>,
}

impl Default for HotspotArgs {
    fn default() -> Self {
        HotspotArgs {
            top: DEFAULT_TOP,
            level: LEVEL_BOTH,
            min_churn: DEFAULT_MIN_CHURN,
            include_tests: false,
            scope: None,
        }
    }
}

/// One ranked module or symbol.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct Hotspot {
    /// [`LEVEL_MODULE`] or [`LEVEL_SYMBOL`].
    pub level: &'static str,
    pub qname: String,
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// Module: commits that touched its file; symbol: distinct blame times in its span.
    pub churn: u32,
    /// Module: lines added + deleted over those commits; `None` for a symbol.
    pub lines_changed: Option<u64>,
    /// Unix seconds of the newest such commit (module) or blamed line (symbol).
    pub last_change: i64,
    pub churn_rank: usize,
    pub centrality_rank: usize,
    /// The population both ranks range over.
    pub ranked: usize,
    /// Always [`HEURISTIC`].
    pub tier: &'static str,
}

/// The hotspot answer: best first per level.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct Hotspots {
    pub modules: Vec<Hotspot>,
    pub symbols: Vec<Hotspot>,
    /// The newest time any history ATTN carries (`signals::history_now`).
    pub history_head: Option<i64>,
    /// `Some` iff both lists are empty.
    pub absence: Option<Absence>,
}

/// A population member before ranking.
struct Candidate {
    id: NodeId,
    churn: u32,
    /// Lines changed (module) or newest blame time (symbol): the churn tie-break.
    tie: i128,
    lines_changed: Option<u64>,
    last_change: i64,
}

/// The two populations and what the scan left out.
#[derive(Default)]
struct Scan {
    modules: Vec<Candidate>,
    symbols: Vec<Candidate>,
    /// Symbol blame cells seen at any churn.
    blame_seen: bool,
    excluded: usize,
    out_of_scope: usize,
}

/// Rank the modules and symbols of `merged` by churn and centrality.
pub fn hotspots(merged: &MergedGraph, args: &HotspotArgs) -> Hotspots {
    let loc = Locator::new(merged);
    let scope = args.scope.as_deref().map(|s| resolve_scope(merged, s));
    let scan = scan(merged, &loc, args, scope.as_deref());
    let head = signals::history_now(merged);

    let (pagerank, iterations) = if scan.modules.is_empty() && scan.symbols.is_empty() {
        (HashMap::new(), 0)
    } else {
        pagerank(merged)
    };
    let module_ids: HashSet<NodeId> = scan.modules.iter().map(|c| c.id).collect();
    let module_centrality = module_centrality(merged, &pagerank, &module_ids);

    let (modules, ranked_modules) = rank(&loc, &scan.modules, LEVEL_MODULE, &module_centrality, args.top);
    let (symbols, ranked_symbols) = rank(&loc, &scan.symbols, LEVEL_SYMBOL, &pagerank, args.top);
    eprintln!(
        "[hotspots] modules={}/{ranked_modules} symbols={}/{ranked_symbols} pagerank_iterations={iterations} head={}",
        modules.len(),
        symbols.len(),
        head.map_or_else(|| "-".to_string(), |t| t.to_string())
    );
    let absence = (modules.is_empty() && symbols.is_empty()).then(|| no_rows(merged, args, &scan, head));
    Hotspots { modules, symbols, history_head: head, absence }
}

/// The populations of the wanted levels, in graph order, each node once.
fn scan(merged: &MergedGraph, loc: &Locator<'_>, args: &HotspotArgs, scope: Option<&str>) -> Scan {
    let want_modules = args.level != LEVEL_SYMBOL;
    let want_symbols = args.level != LEVEL_MODULE;
    let mut out = Scan::default();
    let mut taken: HashSet<NodeId> = HashSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let Some(&kind) = g.nav.kind_by_id.get(&n.id) else { continue };
            let candidate = if kind == node_kind::MODULE {
                signals::module_churn(&n.cells).filter(|m| want_modules && m.commits >= args.min_churn).map(|m| {
                    let lines = m.lines_added.saturating_add(m.lines_deleted);
                    Candidate {
                        id: n.id,
                        churn: m.commits,
                        tie: i128::from(lines),
                        lines_changed: Some(lines),
                        last_change: m.last,
                    }
                })
            } else if SYMBOL_KINDS.contains(&kind) {
                let blame = signals::symbol_blame(&n.cells);
                out.blame_seen |= blame.is_some();
                blame.filter(|b| want_symbols && b.span_changes >= args.min_churn as usize).map(|b| Candidate {
                    id: n.id,
                    churn: u32::try_from(b.span_changes).unwrap_or(u32::MAX),
                    tie: i128::from(b.last),
                    lines_changed: None,
                    last_change: b.last,
                })
            } else {
                None
            };
            let Some(c) = candidate else { continue };
            if taken.contains(&c.id) {
                continue;
            }
            if !args.include_tests && EXCLUDED_PROVENANCE.iter().any(|p| has_provenance(&n.cells, p)) {
                out.excluded += 1;
                continue;
            }
            if let Some(s) = scope
                && loc.file_of(c.id).is_some_and(|f| !in_scope(&f, s))
            {
                out.out_of_scope += 1;
                continue;
            }
            taken.insert(c.id);
            if kind == node_kind::MODULE {
                out.modules.push(c);
            } else {
                out.symbols.push(c);
            }
        }
    }
    out
}

/// An ORIGIN cell names `provenance`.
fn has_provenance(cells: &[Cell], provenance: &str) -> bool {
    let needle = format!("\"provenance\":\"{provenance}\"");
    cells.iter().any(|c| {
        c.kind == cell_type::ORIGIN
            && matches!(&c.payload, CellPayload::Json(j) | CellPayload::Text(j) if j.contains(&needle))
    })
}

/// Classic PageRank of every node (each id once, graph order, all seeds)
/// over the domain's centrality preset, and the power-iteration rounds.
fn pagerank(merged: &MergedGraph) -> (HashMap<NodeId, f64>, usize) {
    let mut seen: HashSet<NodeId> = HashSet::new();
    let ids: Vec<NodeId> =
        merged.graphs.iter().flat_map(|g| g.nodes.iter().map(|n| n.id)).filter(|id| seen.insert(*id)).collect();
    let mut cfg = CODE_TABLES.activation_config(Some(CENTRALITY_PRESET));
    cfg.damping = DAMPING;
    cfg.direction = Direction::Forward;
    cfg.node_specificity = Specificity::None;
    cfg.top_k = ids.len();
    let result = merged.activate(&ids, &cfg);
    (result.scores.into_iter().collect(), result.iterations)
}

/// Each module of `modules`: its own PageRank plus that of every node whose
/// nearest MODULE ancestor it is, summed in graph order.
fn module_centrality(
    merged: &MergedGraph,
    pagerank: &HashMap<NodeId, f64>,
    modules: &HashSet<NodeId>,
) -> HashMap<NodeId, f64> {
    let score = |id: &NodeId| pagerank.get(id).copied().unwrap_or(0.0);
    let mut sums: HashMap<NodeId, f64> = modules.iter().map(|id| (*id, score(id))).collect();
    if sums.is_empty() {
        return sums;
    }
    let mut seen: HashSet<NodeId> = HashSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if seen.insert(n.id)
                && let Some(m) = nearest_module(g, n.id)
                && let Some(sum) = sums.get_mut(&m)
            {
                *sum += score(&n.id);
            }
        }
    }
    sums
}

/// The first MODULE up `id`'s `parent_of` chain in `g`.
fn nearest_module(g: &RepoGraph, id: NodeId) -> Option<NodeId> {
    let mut cur = id;
    for _ in 0..MAX_PARENT_STEPS {
        let parent = *g.nav.parent_of.get(&cur)?;
        if g.nav.kind_by_id.get(&parent) == Some(&node_kind::MODULE) {
            return Some(parent);
        }
        cur = parent;
    }
    None
}

/// Ranked, ordered and cut rows of one population, and its size.
fn rank(
    loc: &Locator<'_>,
    population: &[Candidate],
    level: &'static str,
    centrality: &HashMap<NodeId, f64>,
    top: usize,
) -> (Vec<Hotspot>, usize) {
    let located: Vec<_> = population.iter().map(|c| loc.locate(c.id)).collect();
    let pr: Vec<f64> = population.iter().map(|c| centrality.get(&c.id).copied().unwrap_or(0.0)).collect();
    let total = |a: usize, b: usize| {
        located[a].qname.cmp(&located[b].qname).then(population[a].id.0.cmp(&population[b].id.0))
    };
    let churn_rank = competition_ranks(
        population.len(),
        |a, b| {
            let (x, y) = (&population[a], &population[b]);
            y.churn.cmp(&x.churn).then(y.tie.cmp(&x.tie))
        },
        total,
    );
    let centrality_rank = competition_ranks(population.len(), |a, b| pr[b].total_cmp(&pr[a]), total);

    let mut order: Vec<usize> = (0..population.len()).collect();
    let key = |i: usize| {
        let (c, p) = (churn_rank[i], centrality_rank[i]);
        ((c as u128) * (p as u128), c.max(p))
    };
    order.sort_by(|&a, &b| key(a).cmp(&key(b)).then_with(|| total(a, b)));
    if top > 0 {
        order.truncate(top);
    }
    let rows = order
        .into_iter()
        .map(|i| {
            let (c, at) = (&population[i], &located[i]);
            Hotspot {
                level,
                qname: at.qname.clone(),
                kind: at.kind,
                file: at.file.clone(),
                line: at.line,
                churn: c.churn,
                lines_changed: c.lines_changed,
                last_change: c.last_change,
                churn_rank: churn_rank[i],
                centrality_rank: centrality_rank[i],
                ranked: population.len(),
                tier: HEURISTIC,
            }
        })
        .collect();
    (rows, population.len())
}

/// Competition ranks of `0..n`: sorted by `key` (Less first, `total` breaking
/// ties), a member whose `key` equals its predecessor's shares its rank, and
/// the next distinct key ranks at its position (1, 2, 2, 4).
fn competition_ranks(
    n: usize,
    key: impl Fn(usize, usize) -> Ordering,
    total: impl Fn(usize, usize) -> Ordering,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| key(a, b).then_with(|| total(a, b)));
    let mut ranks = vec![0; n];
    for (pos, &i) in order.iter().enumerate() {
        ranks[i] = match pos.checked_sub(1).map(|p| order[p]) {
            Some(prev) if key(prev, i) == Ordering::Equal => ranks[prev],
            _ => pos + 1,
        };
    }
    ranks
}

/// Why no row came back: no history at all, or none past the filters.
fn no_rows(merged: &MergedGraph, args: &HotspotArgs, scan: &Scan, head: Option<i64>) -> Absence {
    let scope = args.scope.as_deref().map(|s| format!(" scope={s}")).unwrap_or_default();
    let query = format!(
        "level={} min_churn={} include_tests={}{scope}",
        args.level, args.min_churn, args.include_tests
    );
    if head.is_none() {
        let note = "no git-history ATTN in the graph; run `glia history sync <repo>` (add --blame for symbol rows) and rebuild".to_string();
        return absence::empty(merged, PRIMITIVE, &query, "no_history", note, &[], None);
    }
    let mut missing = Vec::new();
    if args.level != LEVEL_SYMBOL {
        missing.push(format!("no MODULE has >= {} commits", args.min_churn));
    }
    if args.level != LEVEL_MODULE {
        let hint = if scan.blame_seen {
            ""
        } else {
            " (no blame ATTN: run `glia history sync <repo> --blame` for symbol rows)"
        };
        missing.push(format!("no FUNCTION / METHOD / CLASS has >= {} blame span changes{hint}", args.min_churn));
    }
    let note = format!(
        "{} in the history window ({} test / generated and {} out-of-scope nodes left out)",
        missing.join("; "),
        scan.excluded,
        scan.out_of_scope
    );
    absence::empty(merged, PRIMITIVE, &query, "no_match", note, &[], None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranks_of(keys: &[u32]) -> Vec<usize> {
        competition_ranks(keys.len(), |a, b| keys[b].cmp(&keys[a]), |a, b| a.cmp(&b))
    }

    #[test]
    fn competition_ranking_shares_the_lower_rank() {
        assert_eq!(ranks_of(&[5, 9, 5, 1]), [2, 1, 2, 4]);
        assert_eq!(ranks_of(&[3, 3, 3]), [1, 1, 1]);
        assert_eq!(ranks_of(&[]), Vec::<usize>::new());
    }

    #[test]
    fn levels_parse() {
        assert_eq!(parse_level("module"), Some(LEVEL_MODULE));
        assert_eq!(parse_level("both"), Some(LEVEL_BOTH));
        assert_eq!(parse_level("modules"), None);
    }
}
