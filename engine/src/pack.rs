//! Context packing to a token budget (CC.4b): the answer to "give me the
//! context for X in N tokens", assembled here so every consumer (the CLI, the
//! pyo3 wrapper, an agent skill) gets the same ranked, located, budgeted text
//! instead of cutting a dense map at a character count.
//!
//! Public slot, reached by module path (`glia_engine::pack::<item>`).
//!
//! # The pipeline
//!
//! 1. **Seeds.** [`pack`] resolves the query through [`find_nodes`] (its first
//!    [`PackArgs::seeds`] rows, [`PackArgs::scope`] applied). A seed found by
//!    `exact_qname` / `exact_name` / `exact_ci` / `qname_suffix` is tier `fact`,
//!    one found by a fuzzier find tier `heuristic`, and `matched` names the tier.
//!    When any row is a fact, the heuristic rows are not seeds: fuzzy seeds
//!    stand in only for a query that names nothing exactly. No row: an empty
//!    pack carrying find's own absence (LD.8a). [`pack_ids`]
//!    takes the seeds (a diff-impact's, a trace's) as ids: tier `fact`, no
//!    `matched`.
//! 2. **Candidates.** One personalised PageRank from the seeds with the code
//!    domain's weights (`CODE_TABLES.activation_config(preset)`), walked
//!    UNDIRECTED: a context is a node's callers and its callees, and the
//!    domain's forward default reaches nothing from a leaf. The seeds come
//!    first, in seed order; the rest follow by (score desc, id asc), are
//!    scoped (a node with no locatable file is kept, A8.3) BEFORE the cut to
//!    [`PackArgs::candidates`], and are tier `derived` (a walk over extracted
//!    edges). Every candidate carries an integer weight: a seed
//!    `2_000_000_000 - rank`, a neighbour `round(score * 1e9)` (scores lie in
//!    [0, 1], so every seed outweighs every neighbour). Floats stop there:
//!    every comparison below is integer.
//! 3. **Greedy climb.** Rung values are Qname 1, Outline 3, Preview 6,
//!    Full 10; a node's cost at a rung is [`estimate_tokens`] of its
//!    [`render_node`] text plus one separator byte. A rung that renders as a
//!    lower one ([`rendered_as`]: Full and Preview of a node with no CODE text
//!    render its Outline) is not offered. Each step moves one candidate from
//!    its rung (or from absent, value 0) to any HIGHER rung (jumps allowed,
//!    so a cheap first rung never traps a node), and the step taken is the one
//!    with the largest `weight * value gained / tokens added` among the steps
//!    that keep the running cost inside the budget (compared by u128
//!    cross-multiplication; a step that adds no tokens beats any that does;
//!    ties go to the better rank, then the lower rung). A step that gains
//!    nothing (a neighbour whose weight rounds to 0) is never taken. It stops
//!    when no step fits. **Seed ceiling:** a neighbour never climbs above the
//!    best rung a seed holds, and while no seed is packed no neighbour is; a
//!    seed already at its own top rung (it can climb no further) lifts that
//!    cap. So a pack never shows a neighbour in more detail than the thing it
//!    was asked about.
//! 4. **Re-render.** The picks, in rank order, are rendered by
//!    [`render_pack`]; the title (`context for <query>`), the section headers
//!    and the `## links` lines are in no node's cost, so while the measured
//!    text is over budget the pick with the smallest `weight * value / cost`
//!    moves down one rung (Qname -> removed; a seed only when the ceiling
//!    still holds after the move, ties to the worse rank) and the pack is
//!    rendered again. Each move lowers the total rung count, so the loop ends
//!    within 4 x picks renders.
//! 5. **Manifest.** The packed nodes in rank order, each with its rung (what
//!    [`rendered_as`] says it renders at), its own token cost, its 1-based
//!    rank, tier, reason and find tier, located through one [`Locator`]
//!    (1-based lines, LD.1). A budget that holds not even the smallest pack
//!    (the title and one bare qname) gives no nodes and an absence
//!    `budget_too_small` naming that smallest cost.
//!
//! # Tokens
//!
//! No tokenizer is linked (none reproduces Claude's anyway): a token is
//! estimated as [`DEFAULT_BYTES_PER_TOKEN_X10`] / 10 = 3.7 bytes, the lowest
//! bytes-per-token measured with a local code BPE over Rust (3.93), Python
//! (4.18), TypeScript (4.17) and Go (3.73) sources, so the estimate
//! over-counts on each of them. Minified JS or long base64 literals can fall
//! below 3.7: the rate is per call ([`PackArgs::bytes_per_token_x10`]) and the
//! pack reports its exact `bytes`, so a caller with a real tokenizer re-counts.
//!
//! # Cost and determinism
//!
//! One `find` (with its liveness walk), one PPR over the whole merged graph
//! and one O(V + E) [`MergedGraph::subset`] to the candidates per call; every
//! render runs over that subset, which renders byte-identically to the whole
//! graph (same first-holding graph per node, same nav rows, every edge between
//! two candidates) at O(candidates) per node lookup. The climb is at most
//! 4 x candidates steps of 4 x candidates comparisons. Query-time, no pool,
//! no `HashMap` order reaches the output: the pack is byte-stable.
//!
//! fired_on marker, one line per call:
//! `[pack] query=<q> seeds=<S> candidates=<C> packed=<P> (full=<F> preview=<V>
//! outline=<O> qname=<Q>) tokens=<U>/<B> bytes=<N> bpt=<x.y> rerenders=<R>`
//! (one line) — grep `^\[pack\] query=`.

use std::collections::HashSet;

use glia_activation::Direction;
use glia_code_domain::profile::CODE_TABLES;
use glia_core::NodeId;
use glia_graph::MergedGraph;
use glia_projection_text::ladder::{Fidelity, Pick, render_node, render_pack, rendered_as};

use crate::absence::{self, Absence};
use crate::answers::{Locator, in_scope, resolve_scope};
use crate::find::{FindOptions, find_nodes};

/// `PackArgs::default().budget_tokens`.
pub const DEFAULT_BUDGET_TOKENS: usize = 8000;
/// `PackArgs::default().bytes_per_token_x10`: 3.7 bytes per token (module doc).
pub const DEFAULT_BYTES_PER_TOKEN_X10: u32 = 37;
/// `PackArgs::default().seeds`.
pub const DEFAULT_SEEDS: usize = 5;
/// `PackArgs::default().candidates`.
pub const DEFAULT_CANDIDATES: usize = 200;

/// The value of each rung, indexed like [`Fidelity::LADDER`].
const RUNG_VALUE: [u64; 4] = [1, 3, 6, 10];
/// [`Fidelity::LADDER`]'s index of Full, the top rung.
const TOP: usize = 3;
/// A seed's weight is this minus its 1-based rank; a neighbour's is at most
/// [`SCORE_SCALE`], so every seed outweighs every neighbour.
const SEED_WEIGHT: u64 = 2_000_000_000;
/// A neighbour's PPR score (in [0, 1]) times this, rounded, is its weight.
const SCORE_SCALE: f64 = 1e9;
/// The find tiers a seed is a FACT from: the query names it exactly.
const FACT_TIERS: [&str; 4] = ["exact_qname", "exact_name", "exact_ci", "qname_suffix"];
/// How many seed qnames [`pack_ids`] names in its `query` before `+N more`.
const QUERY_NAMES: usize = 3;

/// How a pack is built. Start from `default()` and set fields:
/// `#[non_exhaustive]` rules out a struct literal outside this crate.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PackArgs {
    /// The most tokens the packed `text` may take, by [`estimate_tokens`].
    pub budget_tokens: usize,
    /// Bytes per token, in tenths (37 = 3.7). Below 10 counts as 10.
    pub bytes_per_token_x10: u32,
    /// The most find rows [`pack`] seeds from (at least 1).
    pub seeds: usize,
    /// The most nodes the pack chooses from, seeds included (every seed is
    /// always one).
    pub candidates: usize,
    /// A `CODE_TABLES` activation preset (`repair`, `review`, `onboard`, ...);
    /// `None` or an unknown name is the base weights.
    pub preset: Option<String>,
    /// A repo-relative path or project label (A8.3 / A8.6): seeds and
    /// neighbours are kept under it; a node with no locatable file is kept.
    /// [`pack_ids`] keeps its explicit seeds regardless.
    pub scope: Option<String>,
}

impl Default for PackArgs {
    fn default() -> Self {
        PackArgs {
            budget_tokens: DEFAULT_BUDGET_TOKENS,
            bytes_per_token_x10: DEFAULT_BYTES_PER_TOKEN_X10,
            seeds: DEFAULT_SEEDS,
            candidates: DEFAULT_CANDIDATES,
            preset: None,
            scope: None,
        }
    }
}

/// One packed node, as the manifest lists it.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct PackedNode {
    pub id: u64,
    pub qname: String,
    /// The code-domain kind name, e.g. `FUNCTION`.
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based (LD.1's [`Locator`]).
    pub line: Option<i64>,
    /// The rung it renders at: `full`, `preview`, `outline` or `qname`.
    pub fidelity: &'static str,
    /// Its own estimated cost at that rung (its text plus one separator byte).
    pub tokens: usize,
    /// 1-based position in the candidate order (seeds first).
    pub rank: usize,
    /// `fact` (an exact seed or an explicit id), `derived` (a neighbour: a
    /// walk over extracted edges) or `heuristic` (a fuzzy find seed).
    pub tier: &'static str,
    /// `seed` or `neighbour`.
    pub reason: &'static str,
    /// The find tier that matched a [`pack`] seed; `None` for a neighbour and
    /// for a [`pack_ids`] seed.
    pub matched: Option<&'static str>,
}

/// A packed context: the text to hand over, and the manifest of what is in it.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct Pack {
    /// The query as asked; for [`pack_ids`], the seeds' qnames.
    pub query: String,
    /// The pack, rendered by the fidelity ladder; empty when nothing is packed.
    pub text: String,
    pub budget_tokens: usize,
    /// [`estimate_tokens`] of `text`: at most `budget_tokens`.
    pub used_tokens: usize,
    /// `text.len()`, for a caller who re-counts with a real tokenizer.
    pub bytes: usize,
    /// The bytes-per-token rate the estimate used, e.g. `"3.7"`.
    pub bytes_per_token: String,
    /// The nodes the pack chose from.
    pub candidates: usize,
    /// The packed nodes, in rank order.
    pub nodes: Vec<PackedNode>,
    /// Candidates left out: `candidates - nodes.len()`.
    pub dropped: usize,
    /// How many times the text was rendered again to fit the budget.
    pub rerenders: usize,
    /// Why `nodes` is empty (`Some` exactly then): find's `no_match`, or
    /// `budget_too_small`.
    pub absence: Option<Absence>,
}

/// Estimated tokens in `bytes` bytes at `bytes_per_token_x10` tenths of a byte
/// per token, rounded up: `ceil(bytes * 10 / x10)`. A rate below 10 (one byte
/// per token) counts as 10, so the estimate never exceeds `bytes`.
pub fn estimate_tokens(bytes: usize, bytes_per_token_x10: u32) -> usize {
    let per = usize::try_from(bytes_per_token_x10.max(10)).unwrap_or(usize::MAX);
    bytes.saturating_mul(10).div_ceil(per)
}

/// The context for `query` in `args.budget_tokens` tokens: seeds from
/// [`find_nodes`], then the pipeline in the module doc. Prints one
/// `[pack] query=...` line.
pub fn pack(merged: &MergedGraph, query: &str, args: &PackArgs) -> Pack {
    let opts = FindOptions {
        top_k: args.seeds.max(1),
        scope: args.scope.clone(),
        ..FindOptions::default()
    };
    let found = find_nodes(merged, query, &opts);
    if let Some(absence) = found.absence {
        return empty(query.to_string(), args, 0, 0, absence);
    }
    // Find ranks tier first, so any fact rows lead; when there are some, the
    // fuzzier rows behind them are not seeds (a seed outweighs every
    // neighbour, and a subsequence match of an exact query is noise).
    let exact = found
        .results
        .iter()
        .any(|r| FACT_TIERS.contains(&r.r#match));
    let seeds = found
        .results
        .iter()
        .filter(|r| !exact || FACT_TIERS.contains(&r.r#match))
        .map(|r| Seed {
            id: NodeId(r.id),
            tier: if FACT_TIERS.contains(&r.r#match) {
                "fact"
            } else {
                "heuristic"
            },
            matched: Some(r.r#match),
        })
        .collect();
    run(merged, query.to_string(), seeds, args)
}

/// [`pack`] from explicit seeds (a diff-impact's, a trace's), each tier
/// `fact`, in the order given; a repeated id counts once. An id no graph
/// holds seeds nothing; with no seed left, the pack is empty with reason
/// `no_match`. `query` is the seeds' qnames.
pub fn pack_ids(merged: &MergedGraph, seeds: &[NodeId], args: &PackArgs) -> Pack {
    let held: HashSet<NodeId> = merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(|n| n.id))
        .collect();
    let mut seen: HashSet<NodeId> = HashSet::with_capacity(seeds.len());
    let unique: Vec<NodeId> = seeds
        .iter()
        .copied()
        .filter(|id| seen.insert(*id))
        .collect();
    let loc = Locator::new(merged);
    let named: Vec<String> = unique
        .iter()
        .take(QUERY_NAMES)
        .map(|id| {
            if held.contains(id) {
                loc.locate(*id).qname
            } else {
                format!("id:{}", id.0)
            }
        })
        .collect();
    let mut query = named.join(", ");
    if unique.len() > QUERY_NAMES {
        query.push_str(&format!(" +{} more", unique.len() - QUERY_NAMES));
    }
    let kept: Vec<Seed> = unique
        .iter()
        .filter(|id| held.contains(id))
        .map(|id| Seed {
            id: *id,
            tier: "fact",
            matched: None,
        })
        .collect();
    if kept.is_empty() {
        let note = if unique.is_empty() {
            "no seed id was given".to_string()
        } else {
            format!(
                "none of the {} seed {} names a node in this graph",
                unique.len(),
                if unique.len() == 1 { "id" } else { "ids" }
            )
        };
        let absence = absence::empty(merged, "pack", &query, "no_match", note, &[], None);
        return empty(query, args, 0, 0, absence);
    }
    run(merged, query, kept, args)
}

/// A seed, before it becomes a candidate.
struct Seed {
    id: NodeId,
    tier: &'static str,
    matched: Option<&'static str>,
}

/// One node the pack may take, with everything the climb and the manifest need.
struct Cand {
    id: NodeId,
    /// Integer weight (module doc, step 2).
    w: u64,
    seed: bool,
    tier: &'static str,
    matched: Option<&'static str>,
    /// Estimated tokens per rung, indexed like [`Fidelity::LADDER`]; `None`
    /// for a rung that renders as a lower one.
    cost: [Option<usize>; 4],
}

impl Cand {
    /// The cost at `level` (`None` = absent = 0).
    fn cost_at(&self, level: Option<usize>) -> usize {
        level.and_then(|l| self.cost[l]).unwrap_or(0)
    }

    /// The highest rung this node renders at.
    fn top(&self) -> Option<usize> {
        (0..RUNG_VALUE.len())
            .rev()
            .find(|&l| self.cost[l].is_some())
    }

    /// The next rung down from `level` that this node renders at, or `None`
    /// (removed) below its lowest.
    fn lower(&self, level: usize) -> Option<usize> {
        (0..level).rev().find(|&l| self.cost[l].is_some())
    }
}

/// The pipeline from step 2 on, over resolved seeds.
fn run(merged: &MergedGraph, query: String, seeds: Vec<Seed>, args: &PackArgs) -> Pack {
    let x10 = args.bytes_per_token_x10.max(10);
    let loc = Locator::new(merged);

    // Step 2: the PPR neighbourhood. `top_k` stays open so the scope filter
    // runs before the cut (A8.3 rule 1); the cut is `args.candidates`.
    let seed_ids: Vec<NodeId> = seeds.iter().map(|s| s.id).collect();
    let mut cfg = CODE_TABLES.activation_config(args.preset.as_deref());
    cfg.direction = Direction::Undirected;
    cfg.top_k = usize::MAX;
    let mut scored = merged.activate(&seed_ids, &cfg).scores;
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.0.cmp(&b.0.0)));
    let mut seen: HashSet<NodeId> = seed_ids.iter().copied().collect();
    scored.retain(|(id, s)| *s > 0.0 && seen.insert(*id));
    if let Some(raw) = args.scope.as_deref() {
        let scope = resolve_scope(merged, raw);
        let before = scored.len();
        let mut unlocatable = 0usize;
        scored.retain(|(id, _)| match loc.file_of(*id) {
            Some(f) => in_scope(&f, &scope),
            None => {
                unlocatable += 1;
                true
            }
        });
        eprintln!(
            "[scope] pack scope={scope}: {before} -> {} (unlocatable={unlocatable})",
            scored.len()
        );
    }
    scored.truncate(args.candidates.saturating_sub(seeds.len()));

    let mut ids: Vec<NodeId> = seed_ids.clone();
    ids.extend(scored.iter().map(|(id, _)| *id));
    let view = merged.subset(&ids);

    let mut cands: Vec<Cand> = Vec::with_capacity(ids.len());
    for (rank, s) in seeds.iter().enumerate() {
        let w = SEED_WEIGHT.saturating_sub(u64::try_from(rank + 1).unwrap_or(u64::MAX));
        if let Some(cost) = costs(&view, s.id, x10) {
            cands.push(Cand {
                id: s.id,
                w,
                seed: true,
                tier: s.tier,
                matched: s.matched,
                cost,
            });
        }
    }
    for (id, score) in &scored {
        // `as` saturates: a score in [0, 1] maps into [0, SCORE_SCALE].
        let w = (score.clamp(0.0, 1.0) * SCORE_SCALE).round() as u64;
        if let Some(cost) = costs(&view, *id, x10) {
            cands.push(Cand {
                id: *id,
                w,
                seed: false,
                tier: "derived",
                matched: None,
                cost,
            });
        }
    }

    // Step 3: the climb; step 4: render, and move picks down until it fits.
    let budget = args.budget_tokens;
    let mut level = climb(&cands, budget);
    let title = format!("context for {query}");
    let mut text = render(&view, &title, &cands, &level);
    let bound = 4 * level.iter().filter(|l| l.is_some()).count();
    let mut rerenders = 0usize;
    while estimate_tokens(text.len(), x10) > budget && rerenders < bound {
        let Some(i) = demotion(&cands, &level) else {
            break;
        };
        level[i] = level[i].and_then(|l| cands[i].lower(l));
        text = render(&view, &title, &cands, &level);
        rerenders += 1;
    }

    // Step 5: the manifest.
    let nodes: Vec<PackedNode> = cands
        .iter()
        .zip(&level)
        .enumerate()
        .filter_map(|(i, (c, l))| {
            let l = (*l)?;
            let at = loc.locate(c.id);
            let asked = Fidelity::LADDER[l];
            Some(PackedNode {
                id: c.id.0,
                qname: at.qname,
                kind: at.kind,
                file: at.file,
                line: at.line,
                fidelity: rendered_as(&view, c.id, asked).unwrap_or(asked).name(),
                tokens: c.cost[l].unwrap_or(0),
                rank: i + 1,
                tier: c.tier,
                reason: if c.seed { "seed" } else { "neighbour" },
                matched: c.matched,
            })
        })
        .collect();

    if nodes.is_empty() {
        let absence = too_small(merged, &view, &query, &title, &cands, args, x10);
        return empty(query, args, seeds.len(), cands.len(), absence);
    }

    let used = estimate_tokens(text.len(), x10);
    let pack = Pack {
        query,
        bytes: text.len(),
        text,
        budget_tokens: budget,
        used_tokens: used,
        bytes_per_token: rate(x10),
        candidates: cands.len(),
        dropped: cands.len() - nodes.len(),
        nodes,
        rerenders,
        absence: None,
    };
    marker(&pack, seeds.len());
    pack
}

/// Each rung's estimated cost for `id` over the candidate `view`: the rung's
/// [`render_node`] text plus one separator byte, `None` for a rung that
/// [`rendered_as`] says renders as a lower one. `None` when no graph holds `id`.
fn costs(view: &MergedGraph, id: NodeId, x10: u32) -> Option<[Option<usize>; 4]> {
    let mut out = [None; 4];
    for (slot, f) in out.iter_mut().zip(Fidelity::LADDER) {
        if rendered_as(view, id, f)? != f {
            continue;
        }
        let text = render_node(view, id, f)?;
        *slot = Some(estimate_tokens(text.len().saturating_add(1), x10));
    }
    Some(out)
}

/// The highest rung a neighbour may hold: the best rung any seed holds, where
/// a seed at its own top rung lifts the cap to Full. `None` (no neighbour
/// allowed) while no seed is packed.
fn seed_ceiling(cands: &[Cand], level: &[Option<usize>]) -> Option<usize> {
    cands
        .iter()
        .zip(level)
        .filter(|(c, _)| c.seed)
        .filter_map(|(c, l)| {
            let l = (*l)?;
            Some(if Some(l) == c.top() { TOP } else { l })
        })
        .max()
}

/// Step 3, the greedy climb: each candidate's rung (index into
/// [`Fidelity::LADDER`]; `None` = not packed) once no step fits `budget`.
fn climb(cands: &[Cand], budget: usize) -> Vec<Option<usize>> {
    let mut level: Vec<Option<usize>> = vec![None; cands.len()];
    let mut running = 0usize;
    loop {
        let ceiling = seed_ceiling(cands, &level);
        // (candidate, target rung, gain, added tokens)
        let mut best: Option<(usize, usize, u128, u128)> = None;
        for (i, c) in cands.iter().enumerate() {
            let from = level[i];
            let from_cost = c.cost_at(from);
            let from_value = from.map_or(0, |l| RUNG_VALUE[l]);
            let start = from.map_or(0, |l| l + 1);
            for (to, &to_value) in RUNG_VALUE.iter().enumerate().skip(start) {
                let Some(to_cost) = c.cost[to] else { continue };
                if !c.seed && ceiling.is_none_or(|cap| to > cap) {
                    continue;
                }
                if running - from_cost + to_cost > budget {
                    continue;
                }
                let gain = u128::from(c.w) * u128::from(to_value - from_value);
                if gain == 0 {
                    continue;
                }
                let added = to_cost.saturating_sub(from_cost) as u128;
                // Strictly better only: iteration order is (rank, rung), so a
                // tie keeps the better rank, then the lower rung.
                let better = best.is_none_or(|(_, _, bg, ba)| gain * ba > bg * added);
                if better {
                    best = Some((i, to, gain, added));
                }
            }
        }
        let Some((i, to, _, _)) = best else { break };
        running = running - cands[i].cost_at(level[i]) + cands[i].cost[to].unwrap_or(0);
        level[i] = Some(to);
    }
    level
}

/// Step 4's choice: the pick to move down one rung, the one with the smallest
/// `weight * value / cost` (ties to the worse rank). A seed qualifies only if,
/// after the move, no neighbour sits above the seeds' ceiling. `None` when
/// nothing is packed.
fn demotion(cands: &[Cand], level: &[Option<usize>]) -> Option<usize> {
    let top_neighbour = cands
        .iter()
        .zip(level)
        .filter(|(c, _)| !c.seed)
        .filter_map(|(_, l)| *l)
        .max();
    let mut best: Option<(usize, u128, u128)> = None;
    for i in (0..cands.len()).rev() {
        let Some(l) = level[i] else { continue };
        let c = &cands[i];
        if c.seed && top_neighbour.is_some() {
            let mut after = level.to_vec();
            after[i] = c.lower(l);
            let cap = seed_ceiling(cands, &after);
            if top_neighbour > cap {
                continue;
            }
        }
        let value = u128::from(c.w) * u128::from(RUNG_VALUE[l]);
        let cost = c.cost_at(Some(l)).max(1) as u128;
        let smaller = best.is_none_or(|(_, bv, bc)| value * bc < bv * cost);
        if smaller {
            best = Some((i, value, cost));
        }
    }
    best.map(|(i, _, _)| i)
}

/// The pack text for the current rungs, picks in rank order.
fn render(view: &MergedGraph, title: &str, cands: &[Cand], level: &[Option<usize>]) -> String {
    let picks: Vec<Pick> = cands
        .iter()
        .zip(level)
        .filter_map(|(c, l)| {
            l.map(|l| Pick {
                id: c.id,
                fidelity: Fidelity::LADDER[l],
            })
        })
        .collect();
    render_pack(view, title, &picks)
}

/// The `budget_too_small` absence: names the smallest pack, the title and the
/// cheapest candidate as a bare qname.
fn too_small(
    merged: &MergedGraph,
    view: &MergedGraph,
    query: &str,
    title: &str,
    cands: &[Cand],
    args: &PackArgs,
    x10: u32,
) -> Absence {
    let cheapest = cands
        .iter()
        .filter(|c| c.cost[0].is_some())
        .min_by_key(|c| (c.cost[0].unwrap_or(usize::MAX), c.id.0));
    let note = match cheapest {
        Some(c) => {
            let text = render_pack(
                view,
                title,
                &[Pick {
                    id: c.id,
                    fidelity: Fidelity::Qname,
                }],
            );
            let qname = render_node(view, c.id, Fidelity::Qname).unwrap_or_default();
            format!(
                "a budget of {} tokens packs no node: the smallest pack, the title and `{qname}` as a bare qname, is {} tokens at {} bytes per token",
                args.budget_tokens,
                estimate_tokens(text.len(), x10),
                rate(x10)
            )
        }
        None => format!(
            "a budget of {} tokens packs no node: there is no candidate to pack",
            args.budget_tokens
        ),
    };
    absence::empty(merged, "pack", query, "budget_too_small", note, &[], None)
}

/// A pack with no nodes, carrying why; prints the marker.
fn empty(
    query: String,
    args: &PackArgs,
    seeds: usize,
    candidates: usize,
    absence: Absence,
) -> Pack {
    let pack = Pack {
        query,
        text: String::new(),
        budget_tokens: args.budget_tokens,
        used_tokens: 0,
        bytes: 0,
        bytes_per_token: rate(args.bytes_per_token_x10.max(10)),
        candidates,
        nodes: Vec::new(),
        dropped: candidates,
        rerenders: 0,
        absence: Some(absence),
    };
    marker(&pack, seeds);
    pack
}

/// `37` -> `"3.7"`.
fn rate(x10: u32) -> String {
    format!("{}.{}", x10 / 10, x10 % 10)
}

/// The CC.4b fired_on marker, once per call.
fn marker(p: &Pack, seeds: usize) {
    let at = |f: Fidelity| p.nodes.iter().filter(|n| n.fidelity == f.name()).count();
    eprintln!(
        "[pack] query={} seeds={seeds} candidates={} packed={} (full={} preview={} outline={} qname={}) tokens={}/{} bytes={} bpt={} rerenders={}",
        p.query.split_whitespace().collect::<Vec<_>>().join(" "),
        p.candidates,
        p.nodes.len(),
        at(Fidelity::Full),
        at(Fidelity::Preview),
        at(Fidelity::Outline),
        at(Fidelity::Qname),
        p.used_tokens,
        p.budget_tokens,
        p.bytes,
        p.bytes_per_token,
        p.rerenders
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(w: u64, seed: bool, cost: [Option<usize>; 4]) -> Cand {
        Cand {
            id: NodeId(w),
            w,
            seed,
            tier: "fact",
            matched: None,
            cost,
        }
    }

    const CODE: [Option<usize>; 4] = [Some(2), Some(5), Some(10), Some(30)];
    const NO_CODE: [Option<usize>; 4] = [Some(2), Some(5), None, None];

    #[test]
    fn climb_fills_the_budget_seed_first() {
        let cands = [
            cand(SEED_WEIGHT - 1, true, CODE),
            cand(500_000_000, false, CODE),
        ];
        // Everything fits: both at the top.
        assert_eq!(climb(&cands, 1000), vec![Some(3), Some(3)]);
        // Nothing fits.
        assert_eq!(climb(&cands, 1), vec![None, None]);
        // The seed climbs to Outline, then Preview (10 of 10); nothing is
        // left for the neighbour.
        assert_eq!(climb(&cands, 10), vec![Some(2), None]);
    }

    #[test]
    fn a_neighbour_never_climbs_above_the_seed() {
        // The seed fits Outline (5) but not Preview (10) inside 9. The cheap
        // neighbour could afford Full (running 5 + 4 = 9), but the ceiling
        // holds it at the seed's Outline.
        let cheap = [Some(1), Some(2), Some(3), Some(4)];
        let cands = [
            cand(SEED_WEIGHT - 1, true, CODE),
            cand(500_000_000, false, cheap),
        ];
        assert_eq!(climb(&cands, 9), vec![Some(1), Some(1)]);
        // No seed packed, no neighbour packed: the seed's cheapest rung is 2.
        assert_eq!(climb(&cands, 1), vec![None, None]);
    }

    #[test]
    fn a_seed_at_its_top_rung_lifts_the_ceiling() {
        // The seed has no CODE text: Outline is its top, so the neighbour may
        // take Full.
        let cands = [
            cand(SEED_WEIGHT - 1, true, NO_CODE),
            cand(500_000_000, false, CODE),
        ];
        assert_eq!(climb(&cands, 100), vec![Some(1), Some(3)]);
    }

    #[test]
    fn a_higher_rung_at_no_extra_cost_wins_and_a_zero_weight_never_packs() {
        // Full costs what Preview does (a short body under a one-line
        // signature): the climb takes Full, never stopping at Preview.
        let flat = [Some(2), Some(5), Some(10), Some(10)];
        let cands = [cand(SEED_WEIGHT - 1, true, flat), cand(0, false, CODE)];
        assert_eq!(climb(&cands, 100), vec![Some(3), None]);
        assert_eq!(climb(&cands, 10), vec![Some(3), None]);
    }

    #[test]
    fn demotion_moves_neighbours_before_seeds() {
        let cands = [
            cand(SEED_WEIGHT - 1, true, CODE),
            cand(900_000_000, false, CODE),
        ];
        let level = [Some(3), Some(3)];
        assert_eq!(demotion(&cands, &level), Some(1));
        // A seed alone moves down.
        assert_eq!(demotion(&cands, &[Some(3), None]), Some(0));
        assert_eq!(demotion(&cands, &[None, None]), None);
    }

    #[test]
    fn rate_prints_tenths() {
        assert_eq!(rate(37), "3.7");
        assert_eq!(rate(10), "1.0");
        assert_eq!(rate(200), "20.0");
    }
}
