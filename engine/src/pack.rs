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
//!    [`PackArgs::seeds`] rows, [`PackArgs::scope`] applied, identity collapse
//!    off: every matching member of a SAME_AS class seeds). A seed found by
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
//!    edges). The PPR runs over the [`PackArgs::rank_cache`]'s `PprIndex`
//!    for that config (bit-identical to a fresh walk; a transient cache when
//!    none is given). **Ranking (EA.6, EA.6b).** Before the cut, the
//!    neighbours are reordered over the measured file ranking
//!    ([`rank_files`], [`PackArgs::ranking`]) of the seeds' files: under the
//!    default `H10` each file that co-changed with the seeds' (the history
//!    snapshot under a repo root the cache knows) lifts its best (first PPR)
//!    row to the front, by the file ranking, and every other row keeps the
//!    score order (ruling EA-2b: pack lifts FILES, not all their rows, so one
//!    big co-changed file cannot crowd the others out of the cut); `PT*` /
//!    `B0` group the other rows by file ([`order_rows`]' tier 1). With `Ranking::head()`, no snapshot or no seed
//!    file, nothing moves. Every candidate carries an integer weight: a seed
//!    `2_000_000_000 - rank`, a neighbour `round(s_i * 1e9)` where `s_i` is
//!    the i-th largest neighbour score (the 0.5.2 weight profile, assigned
//!    down the ranked order, so an unmoved order is the 0.5.2 pack byte for
//!    byte; scores lie in [0, 1], so every seed outweighs every neighbour).
//!    Floats stop there: every comparison below is integer.
//!    **Selection (EC.9, ruling EC-9b).** Pack never reads the repo's
//!    calibration record (`glia history calibrate`): the record's sets are
//!    scored by the impact replay's file recall, not by pack's budget cut and
//!    file roll-up, and the measured pick lowered pack's recall@10 on every
//!    gate snapshot, so step 2b applies [`PackArgs::ranking`] alone and a
//!    repo with a record packs byte-identically to one without.
//!    [`Pack::selection`] still says what ranked it: the shape is the seeds'
//!    distinct located files (k1 / k2 / k3 / half), the set SELh0 when
//!    `ranking` ranks co-change first, else SELs0; the default ranking (H10)
//!    reads source `fallback` with reason `pack_uncalibrated`, any other
//!    ranking source `explicit`, both with 0 test commits. The candidates
//!    themselves are always the PPR walk above (a preset keeps its weights
//!    there); a `B0` / `H10[B0]` ranking orders their files by carry-BFS
//!    depth from the seed files' nodes, then file degree, and ignores the
//!    preset's weights.
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
//! fired_on markers, one line each per call (the first two only once seeds
//! are resolved): `[select] surface=pack set=<SELh0|SELs0>
//! shape=<k1|k2|k3|half> pick=<name> source=<fallback|explicit>
//! reason=<pack_uncalibrated|-> test_commits=0` — grep `^\[select\] surface=pack`; then
//! `[rank] surface=pack ranking=<P1|PT*|B0|H10|H10[PT*]|H10[B0]>
//! seed_files=<s> cochange_files=<c> files=0 lifted=<l>
//! closure_index=<hit|built> lifted_files=<f> max_file_share=<m>/<k>` (or
//! `[rank] surface=pack ranking=<r> files=0 reason=no_seed_file
//! closure_index=<..>`), where `lifted` counts the lifted rows (one per
//! lifted file), `lifted_files` those inside the cut, and `max_file_share`
//! the most of the `k` cut neighbours any one file holds (EA.6b) —
//! grep `^\[rank\] surface=pack`; then
//! `[pack] query=<q> seeds=<S> candidates=<C> packed=<P> (full=<F> preview=<V>
//! outline=<O> qname=<Q>) tokens=<U>/<B> bytes=<N> bpt=<x.y> rerenders=<R>`
//! (one line) — grep `^\[pack\] query=`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use glia_activation::{Direction, Specificity};
use glia_code_domain::profile::CODE_TABLES;
use glia_core::NodeId;
use glia_graph::MergedGraph;
use glia_projection_text::ladder::{Fidelity, Pick, render_node, render_pack, rendered_as};

use crate::absence::{self, Absence};
use crate::answers::{Locator, in_scope, resolve_scope};
use crate::calibrate::{COCHANGE_SET, STRUCTURE_SET, Selection, Shape, explicit, log_selection};
use crate::find::{FindOptions, find_nodes};
use crate::identity::Collapse;
use crate::rank::{FileRanking, RankBase, RankCache, Ranking, SourceKey, order_rows, rank_files};

/// `PackArgs::default().budget_tokens`.
pub const DEFAULT_BUDGET_TOKENS: usize = 8000;
/// `PackArgs::default().bytes_per_token_x10`: 3.7 bytes per token (module doc).
pub const DEFAULT_BYTES_PER_TOKEN_X10: u32 = 37;
/// `PackArgs::default().seeds`.
pub const DEFAULT_SEEDS: usize = 5;
/// `PackArgs::default().candidates`.
pub const DEFAULT_CANDIDATES: usize = 200;
/// The `reason` of a default-ranked pack's selection (ruling EC-9b: pack
/// never reads the calibration record).
const PACK_UNCALIBRATED: &str = "pack_uncalibrated";

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
    /// The file ranking the neighbours are ordered by (step 2b, EA.6);
    /// `Ranking::default()` is `H10` (co-change first), `Ranking::head()` the
    /// 0.5.2 order. Without a history snapshot the two give the same pack.
    pub ranking: Ranking,
    /// The ranking indexes to reuse across calls (EA.2). `None`: a transient
    /// cache that knows no repo root, so no co-change is read. Pack never
    /// reads the cache's calibration record (ruling EC-9b).
    pub rank_cache: Option<Arc<RankCache>>,
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
            ranking: Ranking::default(),
            rank_cache: None,
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
    /// EC.9: the ranking that ordered the neighbours and why: `fallback`
    /// (reason `pack_uncalibrated`, ruling EC-9b) for the default ranking,
    /// else `explicit`; never `calibrated`. `None` for an empty pack (an
    /// absence); skipped in JSON then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<Selection>,
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
    // Not one of the collapsing answers (DD.9): every member of an identity
    // class that matches is its own seed, as before the fold.
    let opts = FindOptions {
        top_k: args.seeds.max(1),
        scope: args.scope.clone(),
        collapse: Collapse::off(),
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
    // Step 2a (EC.9, ruling EC-9b): the selection naming the ranking applied.
    let (selection, ranking) = select_for(&loc, &seed_ids, args);
    log_selection("pack", &selection);
    let cache = args.rank_cache.clone().unwrap_or_else(|| {
        Arc::new(RankCache::new("answer", &BTreeMap::new(), &BTreeMap::new()))
    });
    cache.guard(merged);
    let mut cfg = CODE_TABLES.activation_config(args.preset.as_deref());
    cfg.direction = Direction::Undirected;
    cfg.top_k = usize::MAX;
    // The cached index gives MergedGraph::activate's scores bit for bit (same
    // node list, same edge order, EA.1); the edges are read only for a
    // degree specificity, which the code tables never set.
    debug_assert!(matches!(cfg.node_specificity, Specificity::None));
    let built_before = cache.stats().built;
    let index = cache.ppr_index(merged, SourceKey::Merged, &cfg);
    let closure_index = if cache.stats().built > built_before {
        "built"
    } else {
        "hit"
    };
    let mut scored = if matches!(cfg.node_specificity, Specificity::None) {
        glia_activation::activate_indexed(&index, &[], &seed_ids, &cfg).scores
    } else {
        let edges: Vec<glia_core::Edge> = merged.all_edges().cloned().collect();
        glia_activation::activate_indexed(&index, &edges, &seed_ids, &cfg).scores
    };
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
    // Step 2b: the 0.5.2 weight profile, in today's (score desc) order; the
    // measured file ranking then permutes the rows, and neighbour i of the
    // new order takes weight i of the profile (`transplant`).
    let profile: Vec<f64> = scored.iter().map(|(_, s)| *s).collect();
    let ranked = rank_rows(merged, &cache, &seed_ids, &mut scored, ranking);
    scored.truncate(args.candidates.saturating_sub(seeds.len()));
    rank_marker(&ranked, &scored, closure_index);

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
    for ((id, _), score) in scored.iter().zip(&profile) {
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
        selection: Some(selection),
        absence: None,
    };
    marker(&pack, seeds.len());
    pack
}

/// Step 2a (EC.9, ruling EC-9b): the set is SELh0 when `args.ranking` ranks
/// co-change first, else SELs0; the shape counts the seeds' distinct located
/// files. The ranking is `args.ranking`, never a calibration record's pick:
/// the default ranking is the `fallback` with reason [`PACK_UNCALIBRATED`],
/// any other `explicit`.
fn select_for(loc: &Locator<'_>, ids: &[NodeId], args: &PackArgs) -> (Selection, Ranking) {
    let seed_files: BTreeSet<String> = ids.iter().filter_map(|&id| loc.locate(id).file).collect();
    let shape = Shape::from_seed_files(seed_files.len());
    let set = if args.ranking.cochange_first { COCHANGE_SET } else { STRUCTURE_SET };
    let mut sel = explicit(set, shape, args.ranking.harness_name());
    if args.ranking == Ranking::default() {
        sel.source = "fallback";
        sel.reason = Some(PACK_UNCALIBRATED);
    }
    (sel, args.ranking)
}

/// What step 2b did, for the `[rank] surface=pack` line.
struct RankedRows {
    ranking: Ranking,
    /// None: no seed has a candidate file, the order is kept.
    seed_files: Option<usize>,
    cochange_files: usize,
    /// Lifted rows: one per co-changed file (EA.6b).
    lifted: usize,
    /// Per neighbour row (in the final order), its candidate file and whether
    /// it was lifted (its co-changed file's best row).
    rows: Vec<(Option<u32>, bool)>,
}

/// Step 2b: the measured file ranking of the seeds' files (EA.4) and the
/// permutation [`lift_files`] gives for `scored`'s rows, applied in place.
/// Seed files are the seeds' distinct candidate files in seed order; a seed
/// with no candidate file is an extra seed node. With no seed file, nothing
/// moves.
fn rank_rows(
    merged: &MergedGraph,
    cache: &RankCache,
    seed_ids: &[NodeId],
    scored: &mut Vec<(NodeId, f64)>,
    ranking: Ranking,
) -> RankedRows {
    let files = cache.files(merged);
    let mut seed_files: Vec<u32> = Vec::new();
    let mut extra: Vec<NodeId> = Vec::new();
    for &id in seed_ids {
        match files.file_of_node(id) {
            Some(f) => {
                if !seed_files.contains(&f) {
                    seed_files.push(f);
                }
            }
            None => extra.push(id),
        }
    }
    let row_files: Vec<Option<u32>> = scored.iter().map(|(id, _)| files.file_of_node(*id)).collect();
    if seed_files.is_empty() {
        return RankedRows {
            ranking,
            seed_files: None,
            cochange_files: 0,
            lifted: 0,
            rows: row_files.into_iter().map(|f| (f, false)).collect(),
        };
    }
    let fr = rank_files(merged, cache, &seed_files, &extra, ranking);
    let cochanged = |f: u32| {
        ranking.cochange_first
            && !seed_files.contains(&f)
            && fr.c0.get(f as usize).is_some_and(|&c| c > 0.0)
    };
    let cochange_files = (0..fr.c0.len() as u32).filter(|&f| cochanged(f)).count();
    let (perm, lift) = lift_files(&row_files, &seed_files, &fr, &cochanged);
    let lifted = lift.len();
    let mut taken: Vec<Option<(NodeId, f64)>> = scored.drain(..).map(Some).collect();
    let mut rows = Vec::with_capacity(perm.len());
    for (to, &i) in perm.iter().enumerate() {
        if let Some(row) = taken.get_mut(i).and_then(Option::take) {
            scored.push(row);
            rows.push((row_files.get(i).copied().flatten(), to < lifted));
        }
    }
    RankedRows {
        ranking,
        seed_files: Some(seed_files.len()),
        cochange_files,
        lifted,
        rows,
    }
}

/// Step 2b's permutation (EA.6b, ruling EA-2b: pack lifts co-changed FILES,
/// not their rows), as row indices in the new order, and the lifted rows.
///
/// - Lifted: each co-changed file's (the same predicate as [`order_rows`]'
///   tier 0: co-change first, C0 > 0, no seed file, a place in `fr.order`)
///   FIRST row in the input (PPR) order, by the file's place.
/// - Every other row, a lifted file's later rows included: the input order
///   for base `P1`; for a grouped base (`PT*` / `B0`), [`order_rows`]' tier 1
///   grouping by the same file order, co-change left out (EA.6's, unchanged).
///
/// So pack's file roll-up (first appearance) reads the H10 file order while
/// the rest of the budget keeps PPR's row order.
fn lift_files(
    row_files: &[Option<u32>],
    seed_files: &[u32],
    fr: &FileRanking,
    cochanged: &dyn Fn(u32) -> bool,
) -> (Vec<usize>, Vec<usize>) {
    let mut pos: Vec<usize> = vec![usize::MAX; fr.keys.len().max(fr.c0.len())];
    for (p, &f) in fr.order.iter().enumerate() {
        if let Some(slot) = pos.get_mut(f as usize) {
            *slot = p;
        }
    }
    let place = |f: u32| pos.get(f as usize).copied().filter(|&p| p != usize::MAX);
    let mut firsts: Vec<(usize, usize)> = Vec::new();
    let mut done: BTreeSet<u32> = BTreeSet::new();
    for (i, f) in row_files.iter().enumerate() {
        if let Some(f) = *f
            && cochanged(f)
            && let Some(p) = place(f)
            && done.insert(f)
        {
            firsts.push((p, i));
        }
    }
    firsts.sort_unstable();
    let lift: Vec<usize> = firsts.into_iter().map(|(_, i)| i).collect();
    let lifted: BTreeSet<usize> = lift.iter().copied().collect();
    let rest: Vec<usize> = (0..row_files.len()).filter(|i| !lifted.contains(i)).collect();
    let rest = if matches!(fr.ranking.base, RankBase::Ppr) {
        rest
    } else {
        let mut tier1 = fr.clone();
        tier1.ranking.cochange_first = false;
        let rest_files: Vec<Option<u32>> = rest.iter().map(|&i| row_files[i]).collect();
        match order_rows(&rest_files, seed_files, &tier1) {
            Some(p) => p.into_iter().map(|j| rest[j]).collect(),
            None => rest,
        }
    };
    let mut perm = lift.clone();
    perm.extend(rest);
    (perm, lift)
}

/// The `[rank] surface=pack` line: once per call, before `[pack] query=`.
/// `lifted_files` counts the lifted rows inside the cut, `max_file_share` the
/// most of the cut any one file holds in the final order (EA.6b).
fn rank_marker(r: &RankedRows, kept: &[(NodeId, f64)], closure_index: &str) {
    let Some(seed_files) = r.seed_files else {
        eprintln!(
            "[rank] surface=pack ranking={} files=0 reason=no_seed_file closure_index={closure_index}",
            r.ranking.harness_name()
        );
        return;
    };
    let mut per_file: BTreeMap<u32, usize> = BTreeMap::new();
    let mut lifted_files = 0usize;
    for (f, lifted) in r.rows.iter().take(kept.len()) {
        if let Some(f) = f {
            *per_file.entry(*f).or_insert(0) += 1;
            lifted_files += usize::from(*lifted);
        }
    }
    let max_share = per_file.values().copied().max().unwrap_or(0);
    eprintln!(
        "[rank] surface=pack ranking={} seed_files={seed_files} cochange_files={} files=0 lifted={} closure_index={closure_index} lifted_files={} max_file_share={max_share}/{}",
        r.ranking.harness_name(),
        r.cochange_files,
        r.lifted,
        lifted_files,
        kept.len(),
    );
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
        selection: None,
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
