//! Activation plan (LD.12a): one pass from PPR to a ranked, filtered and
//! synthesized [`ActivatedView`].
//!
//! On a code graph, activation means more than PPR: ranking tweaks, filters
//! (live-only, a kind cut) and synthesized cells (access paths, key symbols)
//! are part of the answer. Before this module they ran as separate passes in
//! each caller, and the synth passes as separate bins that reloaded the graph.
//! A plan borrows the hooks and runs them in one fixed order:
//!
//! 1. PPR over `g.node_ids()` / `g.edges()` ([`crate::ppr_vector`]);
//! 2. [`DegreeSpecificity`] over the whole score vector, when
//!    `config.node_specificity` is not [`Specificity::None`];
//! 3. the universe: every node with a score above 0 ([`ActivationPlan::run`]),
//!    or the caller's candidates at their score, 0.0 where PPR gave none
//!    ([`ActivationPlan::rank`]);
//! 4. the [`RankingSignal`]s, in registration order;
//! 5. the [`FilterPredicate`]s, in registration order, each one's drops
//!    counted under its name;
//! 6. sort by score descending, ties by node id ascending, then truncate to
//!    `config.top_k`;
//! 7. the [`SynthHook`]s, in registration order.
//!
//! [`crate::activate`] is this pass with no hooks, through the same private
//! functions, so the two cannot drift; the tests hold the pre-plan
//! `activate()` as an oracle and compare f64 bits.
//!
//! Hooks are borrowed trait objects (`&'h dyn ..`), not boxed ones: a plan
//! allocates nothing per hook, and a filter can borrow caller data (the live
//! set). They live on the plan, not on [`ActivationConfig`], which stays
//! `Clone` + `Debug` and buildable with `..Default::default()`.
//!
//! Where hooks live: domain-free ones ([`DegreeSpecificity`]) here, a domain's
//! filters and synths in that domain's crates, and task-shaped grounding (an
//! issue's tokens) in hook structs the driver constructs.
//!
//! `GLIA_ACTIVATION_DEBUG=1` prints one `[activation] plan mode=..` line per
//! pass. It is never printed otherwise: activation runs per query and, in
//! neuropil, per frame.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::OnceLock;

use glia_core::{Edge, NodeId};

use crate::algo::GraphSource;
use crate::{ActivationConfig, Specificity};

// ============================================================================
// Hook traits
// ============================================================================

/// Rescores the universe in place. Runs after the universe is chosen and
/// before any filter. A boost should be additive: a flat score wipes the PPR
/// gradient it is meant to adjust.
pub trait RankingSignal<G: ?Sized> {
    /// Listed in [`ActivatedView::applied`] and the debug line.
    fn name(&self) -> &'static str;
    /// Adjust `scores`, one `(id, score)` per universe node, in any order the
    /// plan hands them; the plan sorts afterwards.
    fn apply(&self, graph: &G, scores: &mut [(NodeId, f64)]);
}

/// Keeps or drops one universe node. Filters run after every signal and
/// before `top_k`, so a dropped node frees its slot for the next one.
pub trait FilterPredicate<G: ?Sized> {
    /// Listed in [`ActivatedView::applied`] and, with its drop count, in
    /// [`ActivatedView::dropped`].
    fn name(&self) -> &'static str;
    /// `score` is the node's score after the signals.
    fn keep(&self, graph: &G, id: NodeId, score: f64) -> bool;
}

/// Derives cells from the final ranked view.
///
/// A hook sees `view.synth` holding every earlier hook's cells, so a hook can
/// build on another's output in the same pass. The plan appends a hook's
/// cells in the order it returns them and never reorders them, so that order
/// is part of the hook's contract and its determinism is the hook's own
/// obligation: iterate no `HashMap` into the output.
pub trait SynthHook<G: ?Sized> {
    /// Listed in [`ActivatedView::applied`] and the debug line.
    fn name(&self) -> &'static str;
    fn synth(&self, graph: &G, view: &ActivatedView) -> Vec<SynthCell>;
}

// ============================================================================
// Output
// ============================================================================

/// One synthesized cell: a derived fact about the activated neighbourhood,
/// with the text a consumer renders.
#[derive(Clone, Debug, PartialEq)]
pub struct SynthCell {
    /// The [`SynthHook::name`] of the hook that emitted it.
    pub hook: &'static str,
    /// The hook's own id for the cell (a summary id, a rank); opaque to the
    /// plan.
    pub id: u64,
    /// What the cell is about, as the hook keys it (a qname, a path key).
    pub key: String,
    /// The node the cell attaches to, when there is one.
    pub anchor: Option<NodeId>,
    pub text: String,
    pub score: f64,
    /// Extra named values (file, line, reason), in the hook's order.
    pub attrs: Vec<(&'static str, String)>,
}

/// What one activation pass produced.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ActivatedView {
    /// `(id, score)`, score descending then id ascending, at most `top_k`.
    pub scores: Vec<(NodeId, f64)>,
    /// Per filter, in registration order: `(name, nodes it dropped)`.
    pub dropped: Vec<(&'static str, usize)>,
    /// Every hook's cells, hooks in registration order, each hook's cells in
    /// its emission order.
    pub synth: Vec<SynthCell>,
    /// Power-iteration rounds PPR ran.
    pub iterations: usize,
    /// Every signal, filter and synth hook that ran, in run order;
    /// `degree_specificity` first when the config asked for it.
    pub applied: Vec<&'static str>,
}

impl ActivatedView {
    /// A view over scores ranked elsewhere, kept in the given order: the
    /// input to [`ActivationPlan::synthesize`].
    pub fn from_ranked(scores: Vec<(NodeId, f64)>) -> Self {
        Self { scores, ..Self::default() }
    }

    /// The ranked node ids, in view order.
    pub fn ids(&self) -> Vec<NodeId> {
        self.scores.iter().map(|(id, _)| *id).collect()
    }

    /// `id`'s score, or `None` when it is not in the view. A linear scan.
    pub fn score_of(&self, id: NodeId) -> Option<f64> {
        self.scores.iter().find(|(n, _)| *n == id).map(|(_, s)| *s)
    }

    /// The cells one hook emitted, in its emission order.
    pub fn cells_of(&self, hook: &str) -> impl Iterator<Item = &SynthCell> {
        self.synth.iter().filter(move |c| c.hook == hook)
    }
}

// ============================================================================
// Built-in signal and graph
// ============================================================================

/// Degree-based node specificity: [`Specificity::Idf`] divides a score by
/// `1 + degree` (rare nodes up), [`Specificity::InverseIdf`] multiplies it by
/// `ln(2 + degree)` (hubs up), [`Specificity::None`] leaves it. A node's
/// degree is the number of edge endpoints equal to it, over every edge of the
/// graph whatever its category or weight (a self-loop counts twice).
///
/// A plan applies it automatically, over the whole score vector before the
/// universe is chosen, when `config.node_specificity` is not `None`.
/// Registering it as a signal as well applies it a second time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DegreeSpecificity(pub Specificity);

impl<G: GraphSource + ?Sized> RankingSignal<G> for DegreeSpecificity {
    fn name(&self) -> &'static str {
        DEGREE_SPECIFICITY
    }

    fn apply(&self, graph: &G, scores: &mut [(NodeId, f64)]) {
        specificity(self.0, graph.edges(), scores);
    }
}

const DEGREE_SPECIFICITY: &str = "degree_specificity";

/// Scale each score by its node's degree. The arithmetic is the pre-plan
/// `activate()`'s: `degree as f64`, then `/= 1.0 + d` or `*= (1.0 + d).ln_1p()`.
fn specificity<'e>(spec: Specificity, edges: impl Iterator<Item = &'e Edge>, scores: &mut [(NodeId, f64)]) {
    if spec == Specificity::None || scores.is_empty() {
        return;
    }
    // Keyed by the scored ids only; looked up, never iterated.
    let mut degree: HashMap<NodeId, usize> = scores.iter().map(|&(id, _)| (id, 0)).collect();
    for edge in edges {
        if let Some(d) = degree.get_mut(&edge.from) {
            *d += 1;
        }
        if let Some(d) = degree.get_mut(&edge.to) {
            *d += 1;
        }
    }
    for (id, score) in scores.iter_mut() {
        let d = degree.get(id).copied().unwrap_or(0) as f64;
        match spec {
            Specificity::Idf => *score /= 1.0 + d,
            Specificity::InverseIdf => *score *= (1.0 + d).ln_1p(),
            Specificity::None => {}
        }
    }
}

/// A graph given as two borrowed lists: [`crate::activate`]'s own input.
#[derive(Clone, Copy, Debug)]
pub struct SliceGraph<'a> {
    pub nodes: &'a [NodeId],
    pub edges: &'a [Edge],
}

impl GraphSource for SliceGraph<'_> {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.to_vec()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

// ============================================================================
// The plan
// ============================================================================

/// An [`ActivationConfig`] plus the hooks one pass runs, borrowed for `'h`.
/// Built with [`Self::new`] and the `signal` / `filter` / `synth` builders;
/// each kind of hook runs in the order it was added.
pub struct ActivationPlan<'h, G: ?Sized> {
    pub config: ActivationConfig,
    signals: Vec<&'h dyn RankingSignal<G>>,
    filters: Vec<&'h dyn FilterPredicate<G>>,
    synths: Vec<&'h dyn SynthHook<G>>,
}

impl<G: ?Sized> fmt::Debug for ActivationPlan<'_, G> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pass = self.pass();
        f.debug_struct("ActivationPlan")
            .field("config", &self.config)
            .field("signals", &pass.signal_names())
            .field("filters", &pass.filter_names())
            .field("synths", &pass.synth_names())
            .finish()
    }
}

impl<'h, G: ?Sized> ActivationPlan<'h, G> {
    fn pass(&self) -> Pass<'_, 'h, G> {
        Pass { config: &self.config, signals: &self.signals, filters: &self.filters, synths: &self.synths }
    }
}

impl<'h, G: GraphSource + ?Sized> ActivationPlan<'h, G> {
    /// A plan with no hooks: [`Self::run`] then returns what
    /// [`crate::activate`] returns.
    pub fn new(config: ActivationConfig) -> Self {
        Self { config, signals: Vec::new(), filters: Vec::new(), synths: Vec::new() }
    }

    /// Add a ranking signal, run after the ones already added.
    pub fn signal(mut self, s: &'h dyn RankingSignal<G>) -> Self {
        self.signals.push(s);
        self
    }

    /// Add a filter, run after the ones already added.
    pub fn filter(mut self, f: &'h dyn FilterPredicate<G>) -> Self {
        self.filters.push(f);
        self
    }

    /// Add a synth hook, run after the ones already added.
    pub fn synth(mut self, h: &'h dyn SynthHook<G>) -> Self {
        self.synths.push(h);
        self
    }

    /// The whole pass over the nodes PPR reaches (score above 0), as
    /// [`crate::activate`] ranks them. No seeds or no nodes: an empty view,
    /// no hook run.
    pub fn run(&self, g: &G, seeds: &[NodeId]) -> ActivatedView {
        self.pass().execute(g, seeds, Universe::Reached, "run")
    }

    /// The whole pass over `candidates` (first occurrence kept): each at its
    /// PPR score, 0.0 where PPR gave it none (a node PPR never reaches, or an
    /// id that is no node). That is the lookup blast-radius and resolve do
    /// over `activate()`'s scores. No seeds or no nodes: an empty view, no
    /// hook run.
    pub fn rank(&self, g: &G, seeds: &[NodeId], candidates: &[NodeId]) -> ActivatedView {
        self.pass().execute(g, seeds, Universe::Candidates(candidates), "rank")
    }

    /// Only the synth hooks, over a view built elsewhere
    /// ([`ActivatedView::from_ranked`]): cells are appended after the ones the
    /// view already holds, which every hook sees.
    pub fn synthesize(&self, g: &G, view: &mut ActivatedView) {
        let pass = self.pass();
        let added = pass.synthesize(g, view);
        if activation_debug() {
            let n = view.scores.len();
            let marker = Marker {
                mode: "synthesize",
                signals: Vec::new(),
                filters: Vec::new(),
                synth: pass.synth_names(),
                universe: n,
                kept: n,
                dropped: &[],
                synth_cells: added,
            };
            eprintln!("{marker}");
        }
    }
}

/// Which nodes the view ranks.
#[derive(Clone, Copy)]
enum Universe<'c> {
    /// Every node PPR gave a score above 0.
    Reached,
    /// These ids, at their score or 0.0.
    Candidates(&'c [NodeId]),
}

/// One pass's config and hooks, borrowed: what [`ActivationPlan`] and
/// [`crate::activate`] share.
struct Pass<'a, 'h, G: ?Sized> {
    config: &'a ActivationConfig,
    signals: &'a [&'h dyn RankingSignal<G>],
    filters: &'a [&'h dyn FilterPredicate<G>],
    synths: &'a [&'h dyn SynthHook<G>],
}

impl<G: ?Sized> Pass<'_, '_, G> {
    fn signal_names(&self) -> Vec<&'static str> {
        self.signals.iter().map(|s| s.name()).collect()
    }

    fn filter_names(&self) -> Vec<&'static str> {
        self.filters.iter().map(|f| f.name()).collect()
    }

    fn synth_names(&self) -> Vec<&'static str> {
        self.synths.iter().map(|h| h.name()).collect()
    }
}

impl<G: GraphSource + ?Sized> Pass<'_, '_, G> {
    fn execute(&self, g: &G, seeds: &[NodeId], universe: Universe<'_>, mode: &'static str) -> ActivatedView {
        let node_ids = g.node_ids();
        if node_ids.is_empty() || seeds.is_empty() {
            if activation_debug() {
                let marker = Marker {
                    mode,
                    signals: Vec::new(),
                    filters: Vec::new(),
                    synth: Vec::new(),
                    universe: 0,
                    kept: 0,
                    dropped: &[],
                    synth_cells: 0,
                };
                eprintln!("{marker}");
            }
            return ActivatedView::default();
        }
        let config = self.config;
        let (vector, iterations) = crate::ppr_vector(&node_ids, g.edges(), seeds, config);
        let mut applied: Vec<&'static str> = Vec::new();

        // Over the whole vector, before the universe: an underflow to 0 must
        // still drop a node from the `Reached` universe, as it always has.
        let mut scored: Vec<(NodeId, f64)> = node_ids.iter().copied().zip(vector).collect();
        if config.node_specificity != Specificity::None {
            specificity(config.node_specificity, g.edges(), &mut scored);
            applied.push(DEGREE_SPECIFICITY);
        }

        let mut ranked = match universe {
            Universe::Reached => {
                scored.retain(|(_, s)| *s > 0.0);
                scored
            }
            Universe::Candidates(candidates) => candidate_scores(scored, candidates),
        };
        let universe_len = ranked.len();

        for s in self.signals {
            s.apply(g, &mut ranked);
            applied.push(s.name());
        }
        let mut dropped: Vec<(&'static str, usize)> = Vec::with_capacity(self.filters.len());
        for f in self.filters {
            let before = ranked.len();
            ranked.retain(|&(id, score)| f.keep(g, id, score));
            dropped.push((f.name(), before - ranked.len()));
            applied.push(f.name());
        }
        sort_ranked(&mut ranked);
        ranked.truncate(config.top_k);

        let mut view = ActivatedView { scores: ranked, dropped, synth: Vec::new(), iterations, applied };
        let added = self.synthesize(g, &mut view);
        if activation_debug() {
            let mut signals = Vec::with_capacity(self.signals.len() + 1);
            if config.node_specificity != Specificity::None {
                signals.push(DEGREE_SPECIFICITY);
            }
            signals.extend(self.signal_names());
            let marker = Marker {
                mode,
                signals,
                filters: self.filter_names(),
                synth: self.synth_names(),
                universe: universe_len,
                kept: view.scores.len(),
                dropped: &view.dropped,
                synth_cells: added,
            };
            eprintln!("{marker}");
        }
        view
    }

    /// Run the synth hooks over `view`; the number of cells they added.
    fn synthesize(&self, g: &G, view: &mut ActivatedView) -> usize {
        let before = view.synth.len();
        for h in self.synths {
            let cells = h.synth(g, view);
            view.synth.extend(cells);
            view.applied.push(h.name());
        }
        view.synth.len() - before
    }
}

/// Each candidate (first occurrence) at its score when that is above 0, else
/// 0.0: the lookup callers did over `activate()`'s positive scores, so a NaN
/// or a zero reads as 0.0 here too.
fn candidate_scores(scored: Vec<(NodeId, f64)>, candidates: &[NodeId]) -> Vec<(NodeId, f64)> {
    // A repeated node id: the last occurrence carries PPR's score (the index
    // PPR built is last-wins; an earlier occurrence scores 0).
    let at: HashMap<NodeId, f64> = scored.into_iter().collect();
    let mut seen: HashSet<NodeId> = HashSet::with_capacity(candidates.len());
    candidates
        .iter()
        .filter(|id| seen.insert(**id))
        .map(|&id| {
            let s = at.get(&id).copied().filter(|s| *s > 0.0).unwrap_or(0.0);
            (id, s)
        })
        .collect()
}

/// Score descending (`partial_cmp`, a NaN compares Equal), then node id
/// ascending, so an exact tie at the `top_k` boundary never resolves by input
/// order (audit 2026-06-10).
fn sort_ranked(ranked: &mut [(NodeId, f64)]) {
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.0.cmp(&b.0.0))
    });
}

/// [`crate::activate`]: the pass with no hooks over two slices.
pub(crate) fn activate_view(g: &SliceGraph<'_>, seeds: &[NodeId], config: &ActivationConfig) -> ActivatedView {
    Pass { config, signals: &[], filters: &[], synths: &[] }.execute(g, seeds, Universe::Reached, "activate")
}

// ============================================================================
// Debug marker
// ============================================================================

/// `GLIA_ACTIVATION_DEBUG=1` turns on the `[activation] plan` line, read once.
fn activation_debug() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("GLIA_ACTIVATION_DEBUG").is_ok_and(|v| v == "1"))
}

/// The debug line: `[activation] plan mode=<activate|run|rank|synthesize>
/// signals=[..] filters=[..] synth=[..] universe=N kept=N dropped=[name:N,..]
/// synth_cells=N`. `universe` is the ranked set before the filters, `kept` the
/// view after `top_k`, `synth_cells` the cells this pass added.
struct Marker<'a> {
    mode: &'a str,
    signals: Vec<&'a str>,
    filters: Vec<&'a str>,
    synth: Vec<&'a str>,
    universe: usize,
    kept: usize,
    dropped: &'a [(&'a str, usize)],
    synth_cells: usize,
}

impl fmt::Display for Marker<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dropped: Vec<String> = self.dropped.iter().map(|(n, c)| format!("{n}:{c}")).collect();
        write!(
            f,
            "[activation] plan mode={} signals=[{}] filters=[{}] synth=[{}] universe={} kept={} dropped=[{}] synth_cells={}",
            self.mode,
            self.signals.join(","),
            self.filters.join(","),
            self.synth.join(","),
            self.universe,
            self.kept,
            dropped.join(","),
            self.synth_cells,
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

/// The pre-plan `activate()` (activation/src/lib.rs at c8d67bc), verbatim:
/// the oracle `legacy_activate_is_bit_identical` compares the new path to.
#[cfg(test)]
mod oracle {
    use std::collections::HashMap;

    use glia_core::{Edge, NodeId};

    use crate::{ActivationConfig, ActivationResult, Direction, Specificity};

    pub fn activate(
        node_ids: &[NodeId],
        edges: &[Edge],
        seeds: &[NodeId],
        config: &ActivationConfig,
    ) -> ActivationResult {
        let n = node_ids.len();
        if n == 0 || seeds.is_empty() {
            return ActivationResult {
                scores: vec![],
                iterations: 0,
            };
        }

        let id_to_idx: HashMap<NodeId, usize> =
            node_ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();

        // Build adjacency: incoming[i] = [(source_idx, weight)] — who can send
        // activation to node i. out_weight[j] = total outgoing weight from j.
        let mut incoming: Vec<Vec<(usize, f64)>> = vec![vec![]; n];
        let mut out_weight: Vec<f64> = vec![0.0; n];

        for edge in edges {
            let w = config
                .edge_weights
                .get(&edge.category)
                .copied()
                .unwrap_or(1.0);
            if w <= 0.0 {
                continue;
            }

            let from_idx = id_to_idx.get(&edge.from).copied();
            let to_idx = id_to_idx.get(&edge.to).copied();

            match (from_idx, to_idx) {
                (Some(fi), Some(ti)) => match config.direction {
                    Direction::Forward => {
                        incoming[ti].push((fi, w));
                        out_weight[fi] += w;
                    }
                    Direction::Backward => {
                        incoming[fi].push((ti, w));
                        out_weight[ti] += w;
                    }
                    Direction::Undirected => {
                        incoming[ti].push((fi, w));
                        incoming[fi].push((ti, w));
                        out_weight[fi] += w;
                        out_weight[ti] += w;
                    }
                },
                _ => continue,
            }
        }

        // Personalization vector: equal weight on seed nodes.
        let mut personalization = vec![0.0; n];
        let seed_count = seeds
            .iter()
            .filter(|s| id_to_idx.contains_key(s))
            .count();
        if seed_count == 0 {
            return ActivationResult {
                scores: vec![],
                iterations: 0,
            };
        }
        let seed_weight = 1.0 / seed_count as f64;
        for seed in seeds {
            if let Some(&idx) = id_to_idx.get(seed) {
                personalization[idx] = seed_weight;
            }
        }

        // Power iteration: v = (1-d) * p + d * M * v
        let d = config.damping.clamp(0.01, 0.99);
        let mut scores = personalization.clone();
        let mut iterations = 0;

        for _ in 0..config.max_iterations {
            let mut new_scores = vec![0.0; n];

            // Dangling nodes (no outgoing edges) would lose activation into the
            // void. Standard PPR redistributes their mass back to the
            // personalization vector — keeps scores summing to 1.0.
            let dangling_sum: f64 = (0..n)
                .filter(|&j| out_weight[j] == 0.0)
                .map(|j| scores[j])
                .sum();

            for i in 0..n {
                let propagated: f64 = incoming[i]
                    .iter()
                    .map(|&(j, w)| {
                        if out_weight[j] > 0.0 {
                            scores[j] * w / out_weight[j]
                        } else {
                            0.0
                        }
                    })
                    .sum();

                new_scores[i] =
                    (1.0 - d) * personalization[i] + d * (propagated + dangling_sum * personalization[i]);
            }

            let diff: f64 = scores
                .iter()
                .zip(new_scores.iter())
                .map(|(a, b)| (a - b).abs())
                .sum();

            scores = new_scores;
            iterations += 1;

            if diff < config.epsilon {
                break;
            }
        }

        // Node specificity adjustment (post-PPR).
        if config.node_specificity != Specificity::None {
            let mut degree = vec![0usize; n];
            for edge in edges {
                if let Some(&fi) = id_to_idx.get(&edge.from) {
                    degree[fi] += 1;
                }
                if let Some(&ti) = id_to_idx.get(&edge.to) {
                    degree[ti] += 1;
                }
            }

            for i in 0..n {
                let d = degree[i] as f64;
                match config.node_specificity {
                    Specificity::Idf => scores[i] /= 1.0 + d,
                    Specificity::InverseIdf => scores[i] *= (1.0 + d).ln_1p(),
                    Specificity::None => unreachable!(),
                }
            }
        }

        // Collect, sort descending, truncate to top_k.
        let mut result: Vec<(NodeId, f64)> = node_ids
            .iter()
            .zip(scores.iter())
            .filter(|(_, s)| **s > 0.0)
            .map(|(&id, &s)| (id, s))
            .collect();
        // Tiebreak on node id so exact-score ties at the top_k boundary don't
        // resolve by caller-supplied input order (audit 2026-06-10).
        result.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.0.cmp(&b.0.0))
        });
        result.truncate(config.top_k);

        ActivationResult {
            scores: result,
            iterations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActivationResult, Direction, activate};
    use glia_core::{Confidence, EdgeCategoryId};

    fn edge(from: u64, to: u64, category: u32) -> Edge {
        Edge {
            from: NodeId(from),
            to: NodeId(to),
            category: EdgeCategoryId(category),
            confidence: Confidence::Strong,
            cells: Vec::new(),
        }
    }

    fn ids(ns: &[u64]) -> Vec<NodeId> {
        ns.iter().map(|&n| NodeId(n)).collect()
    }

    /// `(id, score bits)`: equality here is f64 bit equality.
    fn bits(scores: &[(NodeId, f64)]) -> Vec<(u64, u64)> {
        scores.iter().map(|(id, s)| (id.0, s.to_bits())).collect()
    }

    /// Knuth's MMIX LCG: deterministic random graphs without a dependency.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
            xs[self.below(xs.len() as u64) as usize]
        }
    }

    /// One random graph and query. Node ids repeat (drawn from 1..=40), edge
    /// endpoints 41..=45 are no node, seeds may be empty or miss the graph,
    /// and categories 0..5 each get a weight of 0.0, a negative, a positive
    /// or none at all.
    struct Case {
        nodes: Vec<NodeId>,
        edges: Vec<Edge>,
        seeds: Vec<NodeId>,
        config: ActivationConfig,
    }

    fn case(rng: &mut Lcg) -> Case {
        let n = rng.below(30);
        let nodes: Vec<NodeId> = (0..n).map(|_| NodeId(1 + rng.below(40))).collect();
        let m = rng.below(90);
        let edges = (0..m)
            .map(|_| edge(1 + rng.below(45), 1 + rng.below(45), rng.below(5) as u32))
            .collect();
        let k = rng.below(4);
        let seeds = (0..k).map(|_| some_id(rng, &nodes)).collect();
        let mut edge_weights = HashMap::new();
        for c in 0..5 {
            let w: Option<f64> = rng.pick(&[None, Some(0.0), Some(-1.0), Some(0.5), Some(1.0), Some(3.0), Some(10.0)]);
            if let Some(w) = w {
                edge_weights.insert(EdgeCategoryId(c), w);
            }
        }
        let config = ActivationConfig {
            damping: rng.pick(&[0.5, 0.85, 0.3, 0.0, 1.5]),
            edge_weights,
            max_iterations: rng.pick(&[0, 1, 7, 100]),
            epsilon: rng.pick(&[1e-6, 1e-12, 0.0]),
            ..Default::default()
        };
        Case { nodes, edges, seeds, config }
    }

    /// Three times in four a node of `nodes` (when there is one), else any id
    /// in 1..=45: most seeds and candidates hit the graph, some miss it.
    fn some_id(rng: &mut Lcg, nodes: &[NodeId]) -> NodeId {
        if !nodes.is_empty() && rng.below(4) != 0 {
            rng.pick(nodes)
        } else {
            NodeId(1 + rng.below(45))
        }
    }

    const DIRECTIONS: [Direction; 3] = [Direction::Forward, Direction::Backward, Direction::Undirected];
    const SPECIFICITIES: [Specificity; 3] = [Specificity::None, Specificity::Idf, Specificity::InverseIdf];
    const TOP_KS: [usize; 3] = [1, 5, usize::MAX];

    /// Every (case, direction, specificity, top_k) of 300 LCG graphs.
    fn for_each_config(seed: u64, mut f: impl FnMut(usize, &Case, &ActivationConfig)) {
        let mut rng = Lcg(seed);
        for i in 0..300 {
            let c = case(&mut rng);
            for direction in DIRECTIONS {
                for node_specificity in SPECIFICITIES {
                    for top_k in TOP_KS {
                        let config = ActivationConfig { direction, node_specificity, top_k, ..c.config.clone() };
                        f(i, &c, &config);
                    }
                }
            }
        }
    }

    fn same(a: &ActivationResult, b: &ActivationResult) -> bool {
        a.iterations == b.iterations && bits(&a.scores) == bits(&b.scores)
    }

    #[test]
    fn legacy_activate_is_bit_identical() {
        let (mut runs, mut nonempty, mut with_specificity) = (0usize, 0usize, 0usize);
        for_each_config(0x1d12a, |i, c, config| {
            let old = oracle::activate(&c.nodes, &c.edges, &c.seeds, config);
            let new = activate(&c.nodes, &c.edges, &c.seeds, config);
            assert!(same(&old, &new), "case {i} {config:?}\nold {:?}\nnew {:?}", old, new);
            runs += 1;
            if !old.scores.is_empty() {
                nonempty += 1;
                if config.node_specificity != Specificity::None {
                    with_specificity += 1;
                }
            }
        });
        assert_eq!(runs, 300 * 27);
        // Not vacuous: most graphs activate something, with and without the
        // specificity step.
        assert!(nonempty > runs / 3, "only {nonempty} of {runs} runs scored a node");
        assert!(with_specificity > runs / 5, "only {with_specificity} specificity runs scored a node");
    }

    #[test]
    fn plan_run_equals_activate_without_hooks() {
        let mut checked = 0usize;
        for_each_config(0x5eed, |i, c, config| {
            let g = SliceGraph { nodes: &c.nodes, edges: &c.edges };
            let view = ActivationPlan::new(config.clone()).run(&g, &c.seeds);
            let old = activate(&c.nodes, &c.edges, &c.seeds, config);
            assert_eq!(bits(&view.scores), bits(&old.scores), "case {i} {config:?}");
            assert_eq!(view.iterations, old.iterations, "case {i}");
            assert!(view.dropped.is_empty() && view.synth.is_empty());
            let ran = !c.nodes.is_empty() && !c.seeds.is_empty();
            let expect: Vec<&str> =
                if ran && config.node_specificity != Specificity::None { vec![DEGREE_SPECIFICITY] } else { vec![] };
            assert_eq!(view.applied, expect, "case {i}");
            checked += usize::from(!view.scores.is_empty());
        });
        assert!(checked > 1000, "only {checked} non-empty views compared");
    }

    #[test]
    fn rank_is_activate_scores_looked_up_then_sorted() {
        // What blast-radius and resolve do over activate() today: the whole
        // positive score list (top_k = MAX) as a map, each candidate at its
        // score or 0.0, sorted, truncated.
        let mut rng = Lcg(0xb1a57);
        let mut checked = 0usize;
        for_each_config(0xb1a57, |i, c, config| {
            let n = rng.below(12);
            let candidates: Vec<NodeId> = (0..n).map(|_| some_id(&mut rng, &c.nodes)).collect();
            let g = SliceGraph { nodes: &c.nodes, edges: &c.edges };
            let view = ActivationPlan::new(config.clone()).rank(&g, &c.seeds, &candidates);
            if c.nodes.is_empty() || c.seeds.is_empty() {
                assert_eq!(view, ActivatedView::default(), "case {i}");
                return;
            }
            let all = ActivationConfig { top_k: usize::MAX, ..config.clone() };
            let at: HashMap<NodeId, f64> = activate(&c.nodes, &c.edges, &c.seeds, &all).scores.into_iter().collect();
            let mut seen = HashSet::new();
            let mut expect: Vec<(NodeId, f64)> = candidates
                .iter()
                .filter(|id| seen.insert(**id))
                .map(|id| (*id, at.get(id).copied().unwrap_or(0.0)))
                .collect();
            sort_ranked(&mut expect);
            expect.truncate(config.top_k);
            assert_eq!(bits(&view.scores), bits(&expect), "case {i} {config:?} candidates {candidates:?}");
            checked += usize::from(view.scores.iter().any(|(_, s)| *s > 0.0));
        });
        assert!(checked > 500, "only {checked} ranked views held a positive score");
    }

    /// Star 1 -> {2, 3, 4, 5}, seed 1: node 1 ranks first.
    fn star() -> (Vec<NodeId>, Vec<Edge>) {
        (ids(&[1, 2, 3, 4, 5]), vec![edge(1, 2, 1), edge(1, 3, 1), edge(1, 4, 1), edge(1, 5, 1)])
    }

    struct DropIds(&'static str, Vec<NodeId>);

    impl<G: ?Sized> FilterPredicate<G> for DropIds {
        fn name(&self) -> &'static str {
            self.0
        }

        fn keep(&self, _: &G, id: NodeId, _: f64) -> bool {
            !self.1.contains(&id)
        }
    }

    #[test]
    fn filters_apply_before_top_k_and_count_drops() {
        let (nodes, edges) = star();
        let g = SliceGraph { nodes: &nodes, edges: &edges };
        let config = ActivationConfig { top_k: 2, ..Default::default() };
        let plain = ActivationPlan::new(config.clone()).run(&g, &[NodeId(1)]);
        assert_eq!(plain.scores[0].0, NodeId(1), "the seed ranks first unfiltered");

        let drop_seed = DropIds("drop_seed", vec![NodeId(1)]);
        let view = ActivationPlan::new(config).filter(&drop_seed).run(&g, &[NodeId(1)]);
        assert_eq!(view.scores.len(), 2, "the dropped node's slot goes to the next one");
        assert!(view.score_of(NodeId(1)).is_none());
        let all = activate(&nodes, &edges, &[NodeId(1)], &ActivationConfig::default());
        assert_eq!(bits(&view.scores), bits(&all.scores[1..3]));
        assert_eq!(view.dropped, vec![("drop_seed", 1)]);
        assert_eq!(view.applied, vec!["drop_seed"]);
    }

    #[test]
    fn filters_run_in_order_and_count_only_their_own_drops() {
        let (nodes, edges) = star();
        let g = SliceGraph { nodes: &nodes, edges: &edges };
        let evens = DropIds("evens", ids(&[2, 4]));
        let low = DropIds("low", ids(&[1, 2, 3]));
        let view = ActivationPlan::new(ActivationConfig::default()).filter(&evens).filter(&low).run(&g, &[NodeId(1)]);
        // `low` never sees 2: `evens` dropped it first.
        assert_eq!(view.dropped, vec![("evens", 2), ("low", 2)]);
        assert_eq!(view.ids(), ids(&[5]));
        let none = DropIds("none", vec![]);
        let view = ActivationPlan::new(ActivationConfig::default()).filter(&none).run(&g, &[NodeId(1)]);
        assert_eq!(view.dropped, vec![("none", 0)], "a filter that drops nothing still reports 0");
    }

    struct AddOne;
    struct Double;

    impl<G: ?Sized> RankingSignal<G> for AddOne {
        fn name(&self) -> &'static str {
            "add_one"
        }

        fn apply(&self, _: &G, scores: &mut [(NodeId, f64)]) {
            scores.iter_mut().for_each(|(_, s)| *s += 1.0);
        }
    }

    impl<G: ?Sized> RankingSignal<G> for Double {
        fn name(&self) -> &'static str {
            "double"
        }

        fn apply(&self, _: &G, scores: &mut [(NodeId, f64)]) {
            scores.iter_mut().for_each(|(_, s)| *s *= 2.0);
        }
    }

    #[test]
    fn signals_run_in_registration_order() {
        let nodes = ids(&[1, 2, 3]);
        let edges = vec![edge(1, 2, 1), edge(2, 3, 1)];
        let g = SliceGraph { nodes: &nodes, edges: &edges };
        let base = ActivationPlan::new(ActivationConfig::default()).run(&g, &[NodeId(1)]);
        let add_then_double =
            ActivationPlan::new(ActivationConfig::default()).signal(&AddOne).signal(&Double).run(&g, &[NodeId(1)]);
        let double_then_add =
            ActivationPlan::new(ActivationConfig::default()).signal(&Double).signal(&AddOne).run(&g, &[NodeId(1)]);
        assert_eq!(add_then_double.applied, vec!["add_one", "double"]);
        assert_eq!(double_then_add.applied, vec!["double", "add_one"]);
        assert_eq!(base.scores.len(), 3);
        for (id, s) in &base.scores {
            assert_eq!(add_then_double.score_of(*id), Some((s + 1.0) * 2.0));
            assert_eq!(double_then_add.score_of(*id), Some(s * 2.0 + 1.0));
        }
        assert_ne!(add_then_double.scores, double_then_add.scores);
    }

    #[test]
    fn degree_specificity_as_a_signal_matches_the_config_step() {
        let nodes = ids(&[1, 2, 3, 4, 5, 6]);
        let edges = vec![edge(1, 2, 1), edge(1, 3, 1), edge(1, 4, 1), edge(1, 5, 1), edge(2, 6, 1), edge(6, 6, 2)];
        let g = SliceGraph { nodes: &nodes, edges: &edges };
        for spec in [Specificity::Idf, Specificity::InverseIdf] {
            let by_config = ActivationPlan::new(ActivationConfig { node_specificity: spec, ..Default::default() })
                .run(&g, &[NodeId(1)]);
            let signal = DegreeSpecificity(spec);
            let by_signal = ActivationPlan::new(ActivationConfig::default()).signal(&signal).run(&g, &[NodeId(1)]);
            assert_eq!(bits(&by_config.scores), bits(&by_signal.scores), "{spec:?}");
            assert_eq!(by_config.applied, vec![DEGREE_SPECIFICITY]);
            assert_eq!(by_signal.applied, vec![DEGREE_SPECIFICITY]);
        }
        // None is a no-op arm, never a panic.
        let mut scores = vec![(NodeId(1), 0.25)];
        RankingSignal::<SliceGraph<'_>>::apply(&DegreeSpecificity(Specificity::None), &g, &mut scores);
        assert_eq!(scores, vec![(NodeId(1), 0.25)]);
    }

    #[test]
    fn rank_keeps_zero_score_candidates() {
        // 1 -> 2 forward from 1: 3 is a node PPR never reaches, 99 no node.
        let nodes = ids(&[1, 2, 3]);
        let edges = vec![edge(1, 2, 1)];
        let g = SliceGraph { nodes: &nodes, edges: &edges };
        let plan = ActivationPlan::new(ActivationConfig::default());
        let view = plan.rank(&g, &[NodeId(1)], &ids(&[99, 3, 2, 3]));
        assert_eq!(view.ids(), ids(&[2, 3, 99]), "reached first, then the 0.0 ties by id; 3 once");
        assert!(view.score_of(NodeId(2)).is_some_and(|s| s > 0.0));
        assert_eq!(view.score_of(NodeId(3)), Some(0.0));
        assert_eq!(view.score_of(NodeId(99)), Some(0.0));
        assert_eq!(view.score_of(NodeId(1)), None, "the seed is no candidate");
        assert!(plan.run(&g, &[NodeId(1)]).score_of(NodeId(3)).is_none(), "run drops what rank keeps");
        // A seed that is no node: PPR gives nothing, every candidate is 0.0.
        let missed = plan.rank(&g, &[NodeId(42)], &ids(&[2, 1]));
        assert_eq!(missed.scores, vec![(NodeId(1), 0.0), (NodeId(2), 0.0)]);
    }

    /// Emits one cell per key, in the given (unsorted) order.
    struct EmitKeys(&'static [&'static str]);

    impl<G: ?Sized> SynthHook<G> for EmitKeys {
        fn name(&self) -> &'static str {
            "emit_keys"
        }

        fn synth(&self, _: &G, view: &ActivatedView) -> Vec<SynthCell> {
            self.0
                .iter()
                .enumerate()
                .map(|(i, k)| SynthCell {
                    hook: "emit_keys",
                    id: i as u64,
                    key: (*k).to_string(),
                    anchor: view.scores.get(i).map(|(id, _)| *id),
                    text: format!("cell {k}"),
                    score: 1.0 / (i + 1) as f64,
                    attrs: vec![("rank", i.to_string())],
                })
                .collect()
        }
    }

    /// Reads `emit_keys`'s cells from the view and joins their keys.
    struct JoinKeys;

    impl<G: ?Sized> SynthHook<G> for JoinKeys {
        fn name(&self) -> &'static str {
            "join_keys"
        }

        fn synth(&self, _: &G, view: &ActivatedView) -> Vec<SynthCell> {
            let keys: Vec<&str> = view.cells_of("emit_keys").map(|c| c.key.as_str()).collect();
            vec![SynthCell {
                hook: "join_keys",
                id: 0,
                key: "joined".to_string(),
                anchor: None,
                text: keys.join(","),
                score: 0.0,
                attrs: vec![("seen_applied", view.applied.join(","))],
            }]
        }
    }

    #[test]
    fn later_synth_sees_earlier_cells_and_order_is_preserved() {
        let (nodes, edges) = star();
        let g = SliceGraph { nodes: &nodes, edges: &edges };
        let emit = EmitKeys(&["c", "a", "b"]);
        let drop_five = DropIds("drop_five", vec![NodeId(5)]);
        let view = ActivationPlan::new(ActivationConfig::default())
            .filter(&drop_five)
            .synth(&emit)
            .synth(&JoinKeys)
            .run(&g, &[NodeId(1)]);
        let keys: Vec<&str> = view.synth.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, vec!["c", "a", "b", "joined"], "emission order kept, hooks in order");
        let joined: Vec<&SynthCell> = view.cells_of("join_keys").collect();
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].text, "c,a,b", "the later hook read the earlier hook's cells");
        assert_eq!(joined[0].attrs, vec![("seen_applied", "drop_five,emit_keys".to_string())]);
        assert_eq!(view.applied, vec!["drop_five", "emit_keys", "join_keys"]);
        assert_eq!(view.synth[0].anchor, Some(view.scores[0].0), "hooks see the final ranked view");
    }

    #[test]
    fn synthesize_appends_to_a_view_ranked_elsewhere() {
        let (nodes, edges) = star();
        let g = SliceGraph { nodes: &nodes, edges: &edges };
        // Deliberately not sorted: from_ranked keeps the caller's order.
        let mut view = ActivatedView::from_ranked(vec![(NodeId(4), 0.1), (NodeId(2), 0.9)]);
        view.synth.push(SynthCell {
            hook: "summary",
            id: 7,
            key: "pre".to_string(),
            anchor: None,
            text: String::new(),
            score: 0.5,
            attrs: vec![],
        });
        let emit = EmitKeys(&["x"]);
        let plan = ActivationPlan::new(ActivationConfig::default()).synth(&emit).synth(&JoinKeys);
        plan.synthesize(&g, &mut view);
        assert_eq!(view.ids(), ids(&[4, 2]), "synthesize never reranks");
        let keys: Vec<&str> = view.synth.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, vec!["pre", "x", "joined"]);
        assert_eq!(view.synth[1].anchor, Some(NodeId(4)));
        assert_eq!(view.applied, vec!["emit_keys", "join_keys"]);
        assert_eq!(view.iterations, 0);
    }

    #[test]
    fn empty_seeds_or_empty_graph_give_empty_view() {
        let (nodes, edges) = star();
        let g = SliceGraph { nodes: &nodes, edges: &edges };
        let emit = EmitKeys(&["a"]);
        let drop_none = DropIds("drop_none", vec![]);
        let plan = ActivationPlan::new(ActivationConfig { node_specificity: Specificity::Idf, ..Default::default() })
            .signal(&AddOne)
            .filter(&drop_none)
            .synth(&emit);
        assert_eq!(plan.run(&g, &[]), ActivatedView::default());
        assert_eq!(plan.rank(&g, &[], &ids(&[1, 2])), ActivatedView::default());
        let empty = SliceGraph { nodes: &[], edges: &edges };
        assert_eq!(plan.run(&empty, &[NodeId(1)]), ActivatedView::default());
        assert_eq!(plan.rank(&empty, &[NodeId(1)], &ids(&[1])), ActivatedView::default());
        // Seeds that miss the graph are a query: the pass runs, over nothing.
        let missed = plan.run(&g, &[NodeId(42)]);
        assert!(missed.scores.is_empty());
        assert_eq!(missed.applied, vec![DEGREE_SPECIFICITY, "add_one", "drop_none", "emit_keys"]);
    }

    #[test]
    fn run_is_deterministic_across_hash_seeds() {
        // Every HashMap gets its own RandomState, so 20 runs in one process
        // exercise 20 hash seeds for the PPR index and the degree map.
        let mut rng = Lcg(0xde7);
        let c = (0..50).map(|_| case(&mut rng)).max_by_key(|c| c.edges.len() * usize::from(!c.nodes.is_empty()));
        let c = c.expect("50 cases");
        let g = SliceGraph { nodes: &c.nodes, edges: &c.edges };
        let seeds: Vec<NodeId> = c.nodes.iter().take(2).copied().collect();
        let drop_first = DropIds("drop_first", c.nodes.iter().take(1).copied().collect());
        let emit = EmitKeys(&["k", "j"]);
        let config = ActivationConfig { node_specificity: Specificity::InverseIdf, top_k: 8, ..Default::default() };
        let plan = ActivationPlan::new(config).filter(&drop_first).synth(&emit);
        let first = plan.run(&g, &seeds);
        assert!(!first.scores.is_empty());
        for _ in 0..20 {
            let again = plan.run(&g, &seeds);
            assert_eq!(bits(&again.scores), bits(&first.scores));
            assert_eq!(again, first);
        }
    }

    #[test]
    fn view_accessors() {
        let mut view = ActivatedView::from_ranked(vec![(NodeId(3), 0.5), (NodeId(1), 0.25)]);
        assert_eq!(view.ids(), ids(&[3, 1]));
        assert_eq!(view.score_of(NodeId(1)), Some(0.25));
        assert_eq!(view.score_of(NodeId(2)), None);
        let cell = |hook: &'static str, key: &str| SynthCell {
            hook,
            id: 0,
            key: key.to_string(),
            anchor: None,
            text: String::new(),
            score: 0.0,
            attrs: vec![],
        };
        view.synth = vec![cell("a", "1"), cell("b", "2"), cell("a", "3")];
        let a: Vec<&str> = view.cells_of("a").map(|c| c.key.as_str()).collect();
        assert_eq!(a, vec!["1", "3"]);
        assert_eq!(view.cells_of("z").count(), 0);
    }

    #[test]
    fn debug_line_shape() {
        let dropped = [("live", 37)];
        let marker = Marker {
            mode: "rank",
            signals: vec![DEGREE_SPECIFICITY],
            filters: vec!["live"],
            synth: vec![],
            universe: 5712,
            kept: 812,
            dropped: &dropped,
            synth_cells: 0,
        };
        assert_eq!(
            marker.to_string(),
            "[activation] plan mode=rank signals=[degree_specificity] filters=[live] synth=[] universe=5712 kept=812 dropped=[live:37] synth_cells=0"
        );
        let plan: ActivationPlan<'_, SliceGraph<'_>> = ActivationPlan::new(ActivationConfig::default()).signal(&AddOne);
        let debug = format!("{plan:?}");
        assert!(debug.contains("signals: [\"add_one\"]"), "{debug}");
    }
}
