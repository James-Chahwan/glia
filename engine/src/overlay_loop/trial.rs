//! Trying a candidate overlay (CE.3c): what would `.glia/overlay.toml` plus
//! these stanzas change, and which stanza changed what.
//!
//! [`try_candidate`] merges the candidate into the primary repo's overlay
//! text ([`merge`], CE.3b) and builds the tree with each variant through
//! `BuildOptions::with_overlay_text`: BASE (the file as it is, or
//! `version = 1` when there is none), WITH (base + every stanza) and, with
//! [`TryOptions::leave_one_out`], WITHOUT(s) for each graph stanza s (base +
//! every stanza but s; a `[[constraint]]` stanza is judged on WITH alone,
//! "Rules (DB.9b)"). Each build is counted whole ([`graph_counts`], CE.3a),
//! its graph-category gap ids read off [`gaps_report`] with no roots (the
//! root categories read the file on disk, not the overlay a build applied),
//! and, when it declares a rule, its rules checked ("Rules (DB.9b)").
//!
//! - The report's totals are `base` and `with`, their `delta` and
//!   [`verdict`]`(base, with)`; `closed` is every gap a stanza links
//!   (`# gap:` comments, CE.3b) that is in BASE and not in WITH.
//! - A stanza's `marginal` is WITH minus WITHOUT(s): its effect with every
//!   other stanza present, so an interdependent pair (a constant a wrapper
//!   needs) shows the pair's effect on both halves, and a stanza that only
//!   repeats another shows none. Its `closes` are its linked gaps in BASE,
//!   not in WITH and back in WITHOUT(s): closed by it, not by a sibling. Its
//!   verdict is [`verdict`]`(without, with)`, upgraded to [`KEEP`] when it
//!   closes a gap it was written for and no gap category rose.
//!
//! COST. An overlay never changes a parse (wrappers and constant pins run
//! post-cache, the rest after the passes), so every variant reuses every
//! cached parse. One repo: one [`ParseCache`] is loaded from the sidecar,
//! threaded through every build and saved once at the end (LC.11: not
//! rewritten when unchanged), so the parse cost is paid at most once.
//! Several repos (`--with`): each build is `generate_many_opts(.., true, ..)`,
//! which loads and saves each repo's sidecar per build (LC.11 makes the
//! repeated saves no-ops). A variant whose text equals one already built (a
//! stanza already in the base: its WITHOUT is WITH) reuses that build, so
//! `builds` counts the distinct overlay texts built: `2 +` the graph stanzas
//! at most (a `[[constraint]]` stanza costs no build).
//!
//! Read-only for everything but the parse cache: no layout is persisted (a
//! build given `overlay_text` never may be) and `.glia/overlay.toml` is only
//! read. Stanzas are reported in candidate order and every map is a
//! `BTreeMap`; the gap-id sets are only looked up, so two tries of one tree
//! report the same bytes.
//!
//! # Rules (DB.9b)
//!
//! Every build whose graph declares a rule (`declared_constraints`, the read
//! `glia check` makes) is checked once (`check::check_rules`), and its
//! violations are kept as [`RuleHit`]s keyed `(rule_id, rule_kind, key)`, so
//! two builds compare by identity, never by a count. A build that declares
//! none is not checked: a rule-free try prints no `[check]` line and its
//! report serialises exactly as before DB.9b. A rule-bearing build prints
//! check's own `[check]` (and `[rules]`, `[reflexion]`) markers, once per
//! build, like every other build marker.
//!
//! | violation | one hit per | key |
//! |---|---|---|
//! | `forbid_edge`, `divergence` | evidence row | `<from_qname> -[<category>]-> <to_qname>` |
//! | | the rows past `check::MAX_EVIDENCE` | `<n> row(s) past check::MAX_EVIDENCE` |
//! | `rule`, `forbid_path` | violation | its `subject`, joined by `, ` |
//! | `no_cycle` | violation | its witness's sorted distinct qnames, joined by `, `, + ` (<count> members)` |
//! | any other kind | violation | its rows' `from -[category]-> to`, joined by `; ` |
//!
//! A row's hit takes the row's tier and location and blocks by
//! `check::is_blocking(row tier, the rule's min_tier)` (a divergence:
//! `check::DEFAULT_MIN_TIER`); every other hit takes its violation's. The
//! first hit per key wins. The overflow hit stands in for the rows check cut
//! (its list is tier-sorted, so a heuristic overlay edge sorts past it): a
//! move among them changes the count in its key. A `no_cycle` component is
//! identified by its witness plus its member count (check's violation carries
//! no member set), so a grown or shrunk cycle changes its key and a re-chosen
//! witness reads as a move, which demotes to review and hides none. Identity
//! is textual: two repos holding one qname hold one hit.
//!
//! A [`RulesDelta`] between two builds lists the hits one has and the other
//! lacks (each list at most [`MAX_RULE_HITS`], the totals count all), the
//! added hits that block, and the rules erroring only after.
//!
//! A `[[constraint]]` stanza adds a CONSTRAINT cell on an anchor node that
//! exists and nothing else, so its WITHOUT build would have WITH's counts and
//! gap ids exactly: it gets no build, an empty `marginal` and `closes`, and a
//! [`RuleTrial`] over BASE and WITH, first match wins:
//!
//! | status | when |
//! |---|---|
//! | [`RULE_DUPLICATE`] | merge placed it `AlreadyPresent`, or BASE declares a rule of its id (a cell-API rule, an anchored base stanza) |
//! | [`RULE_ERROR`] | WITH's check reports an error for its id (`error` = the message, newlines flattened) |
//! | [`RULE_ORPHANED`] | WITH declares no rule of its id: no `anchor` and no PROJECT at its scope, or an anchor that binds nothing, so `glia check` never reads it |
//! | [`RULE_VIOLATED`] | WITH holds a hit of its id |
//! | [`RULE_HOLDS`] | otherwise |
//!
//! Its `empty_scopes` are the string scopes of the positive `in_scope(_, "s")`
//! (resolved as the builtin resolves it) and `under(_, "p")` literals of its
//! `rule` text that hold no node on WITH: such a rule may hold vacuously. A
//! negated literal never is (`!in_scope(f, "legacy")` holds for every node).
//!
//! | verdict | a `[[constraint]]` stanza |
//! |---|---|
//! | [`KEEP`] | holds, no empty scope: true of the code as it is, and `glia check` keeps it true |
//! | [`REVIEW`] | holds with an empty scope (a misspelt scope, or one meant to stay empty), or violated (the rule or the code is wrong; a blocking violation would make `glia check` exit 1, a heuristic one never) |
//! | [`DROP`] | error (`glia check` would exit 2), orphaned (never read), duplicate (adds nothing) |
//!
//! A graph stanza keeps its graph verdict, except that [`KEEP`] becomes
//! [`REVIEW`] when its `rules` (WITHOUT(it) -> WITH) adds or resolves a hit
//! or makes a rule error: an overlay stanza that adds or hides a declared
//! rule's violation needs a person, as an `allow` does.
//!
//! The report's rule part ([`RulesReport`], present when BASE or WITH
//! evaluated a rule or the candidate holds a `[[constraint]]`):
//! `graph_verdict` is [`verdict`]`(base, with)`, [`KEEP`] demoted to
//! [`REVIEW`] when the BASE -> WITH delta moves a hit (or an error) of a rule
//! the candidate does not newly declare; `rule_verdict` is `None` without a
//! `[[constraint]]` stanza, else [`KEEP`] when every rule trial keeps,
//! [`DROP`] when every one drops, [`REVIEW`] otherwise. [`TryReport::verdict`]
//! is `graph_verdict` alone when `rule_verdict` is `None`, else
//! [`REVIEW`] when either part is review, else [`KEEP`] when either keeps,
//! else [`DROP`] (a rule-only candidate's graph part is [`DROP`], so its
//! verdict is its rule part). The rule trials need only BASE and WITH, so
//! they are judged without leave-one-out too (then counted in
//! [`RulesReport::statuses`] only).
//!
//! Marker (the fired_on line), once per try:
//! `[overlay] try repo=<primary> stanzas=<n> builds=<b> verdicts keep=<k> review=<r> drop=<d> gaps <G0>→<G1> closed=<c> surface=<cli|engine>`,
//! `G` the sum of [`GraphCounts::gaps_by_category`] of BASE and WITH. When the
//! try has a rule part, a second line right after it:
//! `[overlay] try-rules repo=<primary> rules=<R> (holds=<h> violated=<v> error=<e> orphaned=<o> duplicate=<u>) empty_scopes=<s> violations +<a> -<r> blocking=<b> errors=<n> surface=<cli|engine>`,
//! `R` the candidate's `[[constraint]]` stanzas by status, `s` those holding
//! with an empty scope, the rest the BASE -> WITH [`RulesDelta`] (grep
//! `^\[overlay\] try-rules`; it does not start with `[overlay] try `). The
//! `[overlay] candidate` marker is the caller's, which knows the file
//! ([`super::report_candidate`]).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde::Serialize;

use glia_code_domain::external_inputs::{ConstraintKind, ConstraintRule};
use glia_code_domain::glia_config::OVERLAY_FILE;
use glia_core::NodeId;
use glia_graph::MergedGraph;
use glia_rules::ir::{Literal, Term};
use glia_rules::parser::parse_program;

use super::writer::{Location, Merged, StanzaRef, merge, parse_candidate};
use crate::answers::resolve_scope;
use crate::build::{
    BuildOptions, GenerateResult, generate_many_opts, generate_one_with_cache_opts,
};
use crate::cache::ParseCache;
use crate::check::{
    self, DIVERGENCE, FORBID_EDGE, FORBID_PATH, NO_CYCLE, RULE, Violation, ViolationEdge,
};
use crate::external::declared::declared_constraints;
use crate::gaps::{
    DROP, GapsOptions, GraphCounts, KEEP, REVIEW, gaps_report, graph_counts, verdict,
};
use crate::rules::CodeFacts;

/// The BASE overlay of a repo with no `.glia/overlay.toml` (what [`merge`]
/// starts a new file from).
const EMPTY_OVERLAY: &str = "version = 1\n";

/// The candidate section of a rule stanza ([`StanzaRef::section`]).
const CONSTRAINT: &str = "constraint";

/// The rules-layer builtins whose second argument names a scope
/// (`rules::facts`): `in_scope(Node, Scope)`, label-resolved, and
/// `under(Path, Prefix)`, literal.
const IN_SCOPE: &str = "in_scope";
const UNDER: &str = "under";

/// Hits a [`RulesDelta`] list or a [`RuleTrial`] keeps; the totals count all.
pub const MAX_RULE_HITS: usize = 20;
/// [`RuleTrial::status`]: WITH declares the rule and holds no hit of it.
pub const RULE_HOLDS: &str = "holds";
/// [`RuleTrial::status`]: WITH holds a hit of the rule.
pub const RULE_VIOLATED: &str = "violated";
/// [`RuleTrial::status`]: WITH's check reports an error for the rule.
pub const RULE_ERROR: &str = "error";
/// [`RuleTrial::status`]: WITH declares no rule of its id (orphaned at build).
pub const RULE_ORPHANED: &str = "orphaned";
/// [`RuleTrial::status`]: the rule is already in the base.
pub const RULE_DUPLICATE: &str = "duplicate";

/// How [`try_candidate`] runs. Made with `Default` plus field assignment.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TryOptions {
    /// Build base + candidate minus each graph stanza, one build per stanza,
    /// to attribute each stanza's effect (default `true`). `false` builds
    /// only BASE and WITH: the totals, the verdict, `closed` and the rule
    /// part, and no [`TryReport::stanzas`] (nothing attributes an effect to
    /// one stanza).
    pub leave_one_out: bool,
    /// The `surface=` of the `[overlay] try` and `[gaps]` markers (`cli`,
    /// `py`); empty = `engine`.
    pub surface: &'static str,
}

impl Default for TryOptions {
    fn default() -> Self {
        Self {
            leave_one_out: true,
            surface: "",
        }
    }
}

/// Per name, `after - before`, for node kinds, edge categories and graph gap
/// categories; zero entries left out.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphDelta {
    /// Per node-kind name.
    pub nodes: BTreeMap<&'static str, i64>,
    /// Per edge-category name.
    pub edges: BTreeMap<&'static str, i64>,
    /// Per graph gap category (`gaps::CATEGORIES` minus the root ones).
    pub gaps: BTreeMap<&'static str, i64>,
}

impl GraphDelta {
    /// `after - before`, category by category.
    pub fn between(before: &GraphCounts, after: &GraphCounts) -> Self {
        Self {
            nodes: diff(&before.nodes_by_kind, &after.nodes_by_kind),
            edges: diff(&before.edges_by_category, &after.edges_by_category),
            gaps: diff(&before.gaps_by_category, &after.gaps_by_category),
        }
    }

    /// No category moved.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty() && self.gaps.is_empty()
    }
}

/// One violation of a declared rule, as two builds are compared (module doc
/// "Rules (DB.9b)").
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct RuleHit {
    /// The violated rule's id (`reflexion:<C1>-><C2>` for a divergence).
    pub rule_id: String,
    /// `check`'s `rule_kind`: `forbid_edge`, `no_cycle`, `rule`,
    /// `forbid_path` or `divergence`.
    pub rule_kind: &'static str,
    /// The hit's identity within its rule (module doc table).
    pub key: String,
    /// `fact` | `derived` | `heuristic`.
    pub tier: &'static str,
    /// Whether it fails `glia check` (`check::is_blocking`).
    pub blocking: bool,
    /// Where: the evidence row's site, or the violation's subject; `None`
    /// for an overflow hit or an unlocated one.
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
}

/// The hits one build has and the other lacks, and the rules erroring only
/// after.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct RulesDelta {
    /// Hits after has and before lacks, by `(rule_id, rule_kind, key)`, at
    /// most [`MAX_RULE_HITS`].
    pub added: Vec<RuleHit>,
    /// Every added hit.
    pub added_total: usize,
    /// Hits before has and after lacks, at most [`MAX_RULE_HITS`].
    pub resolved: Vec<RuleHit>,
    /// Every resolved hit.
    pub resolved_total: usize,
    /// The added hits that block.
    pub blocking: usize,
    /// `(rule id, message)` of each rule after's check reports an error for
    /// and before's does not, sorted by id.
    pub errors: Vec<(String, String)>,
}

impl RulesDelta {
    /// No hit moved and no rule started erroring.
    pub fn is_empty(&self) -> bool {
        self.added_total == 0 && self.resolved_total == 0 && self.errors.is_empty()
    }

    /// `before -> after`; `None` is a build that declares no rule.
    fn between(before: Option<&RuleState>, after: Option<&RuleState>) -> Self {
        let none = RuleState::default();
        let (b, a) = (before.unwrap_or(&none), after.unwrap_or(&none));
        let added: Vec<&RuleHit> = a
            .hits
            .iter()
            .filter(|(k, _)| !b.hits.contains_key(*k))
            .map(|(_, h)| h)
            .collect();
        let resolved: Vec<&RuleHit> = b
            .hits
            .iter()
            .filter(|(k, _)| !a.hits.contains_key(*k))
            .map(|(_, h)| h)
            .collect();
        Self {
            blocking: added.iter().filter(|h| h.blocking).count(),
            added_total: added.len(),
            resolved_total: resolved.len(),
            added: added.into_iter().take(MAX_RULE_HITS).cloned().collect(),
            resolved: resolved.into_iter().take(MAX_RULE_HITS).cloned().collect(),
            errors: a
                .errors
                .iter()
                .filter(|(id, _)| !b.errors.contains_key(*id))
                .map(|(id, msg)| (id.clone(), msg.clone()))
                .collect(),
        }
    }
}

/// A `[[constraint]]` stanza's rule judged on WITH's graph (module doc
/// "Rules (DB.9b)").
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct RuleTrial {
    /// The stanza's `id`.
    pub rule_id: String,
    /// [`RULE_HOLDS`], [`RULE_VIOLATED`], [`RULE_ERROR`], [`RULE_ORPHANED`]
    /// or [`RULE_DUPLICATE`].
    pub status: &'static str,
    /// WITH's hits of this rule.
    pub violations: usize,
    /// Those hits per tier.
    pub tiers: BTreeMap<&'static str, usize>,
    /// Those hits that block.
    pub blocking: usize,
    /// Those hits, at most [`MAX_RULE_HITS`].
    pub hits: Vec<RuleHit>,
    /// [`RULE_ERROR`]: check's message, newlines flattened to `; `.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The scopes its rule's positive `in_scope` / `under` literals name that
    /// hold no node on WITH, once each, in text order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub empty_scopes: Vec<String>,
}

/// The try's rule part: present when a build checked a rule or the candidate
/// holds a `[[constraint]]` (module doc "Rules (DB.9b)").
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct RulesReport {
    /// BASE -> WITH.
    pub delta: RulesDelta,
    /// The candidate's rule trials per status; zero statuses left out.
    pub statuses: BTreeMap<&'static str, usize>,
    /// `gaps::verdict(base, with)`, `keep` demoted to `review` when the delta
    /// moves a rule the candidate does not newly declare.
    pub graph_verdict: &'static str,
    /// The rule trials' verdict; `None` without a `[[constraint]]` stanza.
    pub rule_verdict: Option<&'static str>,
}

/// One stanza of the candidate: a graph stanza judged by leave-one-out, a
/// `[[constraint]]` stanza by its rule on WITH.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct StanzaTrial {
    /// The stanza's handle ([`StanzaRef`]'s display: `wrapper#1`,
    /// `constants.GATEWAY`, `entrypoints#2`, `constraint#1`).
    pub stanza: String,
    /// The gap ids its `# gap:` comments link, in order.
    pub gaps: Vec<String>,
    /// The linked gaps it closes: in BASE, not in WITH, back in WITHOUT(it).
    /// Empty for a `[[constraint]]` stanza.
    pub closes: Vec<String>,
    /// WITH minus WITHOUT(it); empty for a `[[constraint]]` stanza.
    pub marginal: GraphDelta,
    /// A graph stanza: `gaps::verdict(without, with)` (`keep` / `review` /
    /// `drop`), `keep` when `closes` is non-empty and no gap category rose,
    /// and `review` for a `keep` whose `rules` is not empty. A
    /// `[[constraint]]` stanza: its rule's verdict (module doc).
    pub verdict: &'static str,
    /// A `[[constraint]]` stanza's rule trial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleTrial>,
    /// A graph stanza: the declared rules' hits WITHOUT(it) -> WITH.
    #[serde(skip_serializing_if = "RulesDelta::is_empty")]
    pub rules: RulesDelta,
}

/// [`try_candidate`]'s answer.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct TryReport {
    /// One per candidate stanza, in candidate order; empty without
    /// [`TryOptions::leave_one_out`].
    pub stanzas: Vec<StanzaTrial>,
    /// The build with the overlay as it is.
    pub base: GraphCounts,
    /// The build with every candidate stanza merged in.
    pub with: GraphCounts,
    /// WITH minus BASE.
    pub delta: GraphDelta,
    /// `gaps::verdict(base, with)` for a try with no rule part; else the
    /// rule part's graph and rule verdicts combined (module doc "Rules
    /// (DB.9b)").
    pub verdict: &'static str,
    /// The gap ids the stanzas link that are in BASE and not in WITH, in
    /// candidate order, deduped.
    pub closed: Vec<String>,
    /// Builds run: the distinct overlay texts (at most `2 +` the graph
    /// stanzas: a `[[constraint]]` stanza costs no build).
    pub builds: usize,
    /// The declared rules, checked; `None` when no build evaluated a rule
    /// and the candidate holds no `[[constraint]]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<RulesReport>,
}

/// Try `candidate_text` (a candidate: overlay sections, entrypoints and
/// rules, each stanza linked to its target gaps by `# gap:` comments, CE.3b)
/// against `repo_paths` (the first is the primary repo, whose
/// `.glia/overlay.toml` the candidate would join; the rest build as they
/// are): see the module doc. `Err` when there is no path, the primary's
/// overlay file cannot be read, the candidate is refused
/// ([`parse_candidate`]), it does not merge into the file ([`merge`]), or a
/// build fails.
pub fn try_candidate(
    repo_paths: &[String],
    candidate_text: &str,
    opts: &TryOptions,
) -> Result<TryReport, String> {
    let primary = repo_paths
        .first()
        .ok_or_else(|| "overlay try: no repo paths".to_string())?;
    let base_text = read_overlay(primary)?;
    let cand = parse_candidate(candidate_text)?;
    let merged = merge(base_text.as_deref(), &cand, None)?;
    let surface = if opts.surface.is_empty() {
        "engine"
    } else {
        opts.surface
    };

    let mut trial = Trial::new(repo_paths, surface);
    let base = trial.measure(base_text.as_deref().unwrap_or(EMPTY_OVERLAY), &[])?;
    // The candidate's rules: (ref index, id, duplicate). The non-duplicates
    // are the ones WITH probes for empty scopes.
    let candidate_rules: Vec<(usize, String, bool)> = merged
        .refs
        .iter()
        .enumerate()
        .filter(|(_, (r, _))| r.section == CONSTRAINT)
        .map(|(i, (r, at))| {
            let id = r.key.clone().unwrap_or_default();
            let declared = trial.built[base]
                .rules
                .as_ref()
                .is_some_and(|s| s.ids.contains(&id));
            (i, id, *at == Location::AlreadyPresent || declared)
        })
        .collect();
    let probe: Vec<String> = candidate_rules
        .iter()
        .filter(|(_, _, dup)| !dup)
        .map(|(_, id, _)| id.clone())
        .collect();
    let with = trial.measure(&merged.text, &probe)?;
    let without = if opts.leave_one_out {
        leave_one_out(&mut trial, &merged)?
    } else {
        BTreeMap::new()
    };
    trial.finish();

    let (b, w) = (&trial.built[base], &trial.built[with]);
    let rule_trials: BTreeMap<usize, RuleTrial> = candidate_rules
        .iter()
        .map(|(i, id, dup)| (*i, rule_trial(id, *dup, w.rules.as_ref())))
        .collect();
    let stanzas: Vec<StanzaTrial> = if opts.leave_one_out {
        merged
            .refs
            .iter()
            .enumerate()
            .filter_map(|(i, (r, _))| match rule_trials.get(&i) {
                Some(t) => Some(rule_stanza(r, t)),
                None => without
                    .get(&i)
                    .map(|k| stanza_trial(r, b, w, &trial.built[*k])),
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut seen = BTreeSet::new();
    let closed: Vec<String> = merged
        .refs
        .iter()
        .flat_map(|(r, _)| &r.gaps)
        .filter(|id| b.ids.contains(*id) && !w.ids.contains(*id) && seen.insert(id.as_str()))
        .cloned()
        .collect();

    // The rule part.
    let graph_counts_verdict = verdict(&b.counts, &w.counts);
    let newly_declared: BTreeSet<&str> = probe.iter().map(String::as_str).collect();
    let foreign_move = moved_rules(b.rules.as_ref(), w.rules.as_ref())
        .iter()
        .any(|id| !newly_declared.contains(id.as_str()));
    let graph_verdict = if graph_counts_verdict == KEEP && foreign_move {
        REVIEW
    } else {
        graph_counts_verdict
    };
    let rule_part = (!rule_trials.is_empty()).then(|| {
        let mut verdicts = rule_trials.values().map(rule_verdict);
        let first = verdicts.next().unwrap_or(DROP);
        if verdicts.all(|v| v == first) && first != REVIEW {
            first
        } else {
            REVIEW
        }
    });
    let mut statuses: BTreeMap<&'static str, usize> = BTreeMap::new();
    for t in rule_trials.values() {
        *statuses.entry(t.status).or_insert(0) += 1;
    }
    let evaluated = |m: &Measured| m.rules.as_ref().is_some_and(RuleState::evaluated);
    let rules = (evaluated(b) || evaluated(w) || !rule_trials.is_empty()).then(|| RulesReport {
        delta: RulesDelta::between(b.rules.as_ref(), w.rules.as_ref()),
        statuses,
        graph_verdict,
        rule_verdict: rule_part,
    });

    let report = TryReport {
        delta: GraphDelta::between(&b.counts, &w.counts),
        verdict: rule_part.map_or(graph_verdict, |r| combine(graph_verdict, r)),
        closed,
        builds: trial.built.len(),
        stanzas,
        base: b.counts.clone(),
        with: w.counts.clone(),
        rules,
    };
    let n = |v: &str| report.stanzas.iter().filter(|s| s.verdict == v).count();
    eprintln!(
        "[overlay] try repo={primary} stanzas={} builds={} verdicts keep={} review={} drop={} gaps {}→{} closed={} surface={surface}",
        merged.refs.len(),
        report.builds,
        n(KEEP),
        n(REVIEW),
        n(DROP),
        report.base.total_gaps(),
        report.with.total_gaps(),
        report.closed.len(),
    );
    if let Some(rules) = &report.rules {
        let status = |s: &str| rule_trials.values().filter(|t| t.status == s).count();
        let vacuous = rule_trials
            .values()
            .filter(|t| t.status == RULE_HOLDS && !t.empty_scopes.is_empty())
            .count();
        eprintln!(
            "[overlay] try-rules repo={primary} rules={} (holds={} violated={} error={} orphaned={} duplicate={}) empty_scopes={vacuous} violations +{} -{} blocking={} errors={} surface={surface}",
            rule_trials.len(),
            status(RULE_HOLDS),
            status(RULE_VIOLATED),
            status(RULE_ERROR),
            status(RULE_ORPHANED),
            status(RULE_DUPLICATE),
            rules.delta.added_total,
            rules.delta.resolved_total,
            rules.delta.blocking,
            rules.delta.errors.len(),
        );
    }
    Ok(report)
}

/// The primary repo's `.glia/overlay.toml` text; `None` when there is none.
fn read_overlay(primary: &str) -> Result<Option<String>, String> {
    match std::fs::read(Path::new(primary).join(OVERLAY_FILE)) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| format!("{OVERLAY_FILE}: not valid UTF-8")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{OVERLAY_FILE}: cannot read: {e}")),
    }
}

/// WITHOUT(s) for every merged graph stanza, in candidate order: the
/// stanza's index in [`Merged::refs`] and its build's in [`Trial::built`].
/// A `[[constraint]]` stanza gets none (module doc "Rules (DB.9b)").
fn leave_one_out(trial: &mut Trial<'_>, merged: &Merged) -> Result<BTreeMap<usize, usize>, String> {
    let mut out = BTreeMap::new();
    for (i, (r, _)) in merged.refs.iter().enumerate() {
        if r.section != CONSTRAINT {
            out.insert(i, trial.measure(&merged.without(r)?, &[])?);
        }
    }
    Ok(out)
}

/// One graph stanza judged by WITH against WITHOUT(it).
fn stanza_trial(
    r: &StanzaRef,
    base: &Measured,
    with: &Measured,
    without: &Measured,
) -> StanzaTrial {
    let closes: Vec<String> = r
        .gaps
        .iter()
        .filter(|id| base.ids.contains(*id) && !with.ids.contains(*id) && without.ids.contains(*id))
        .cloned()
        .collect();
    let marginal = GraphDelta::between(&without.counts, &with.counts);
    let graph = if !closes.is_empty() && marginal.gaps.values().all(|d| *d <= 0) {
        KEEP
    } else {
        verdict(&without.counts, &with.counts)
    };
    let rules = RulesDelta::between(without.rules.as_ref(), with.rules.as_ref());
    let verdict = if graph == KEEP && !rules.is_empty() {
        REVIEW
    } else {
        graph
    };
    StanzaTrial {
        stanza: r.to_string(),
        gaps: r.gaps.clone(),
        closes,
        marginal,
        verdict,
        rule: None,
        rules,
    }
}

/// A `[[constraint]]` stanza: no WITHOUT build, its verdict its rule's.
fn rule_stanza(r: &StanzaRef, t: &RuleTrial) -> StanzaTrial {
    StanzaTrial {
        stanza: r.to_string(),
        gaps: r.gaps.clone(),
        closes: Vec::new(),
        marginal: GraphDelta::default(),
        verdict: rule_verdict(t),
        rule: Some(t.clone()),
        rules: RulesDelta::default(),
    }
}

/// The rule `id` of a `[[constraint]]` stanza judged on WITH's rules
/// (`None`: WITH declares none); `duplicate` when the base already holds it.
fn rule_trial(id: &str, duplicate: bool, with: Option<&RuleState>) -> RuleTrial {
    let none = RuleState::default();
    let with = with.unwrap_or(&none);
    let hits: Vec<&RuleHit> = with.hits.values().filter(|h| h.rule_id == id).collect();
    let error = with.errors.get(id);
    let status = if duplicate {
        RULE_DUPLICATE
    } else if error.is_some() {
        RULE_ERROR
    } else if !with.ids.contains(id) {
        RULE_ORPHANED
    } else if !hits.is_empty() {
        RULE_VIOLATED
    } else {
        RULE_HOLDS
    };
    let mut tiers: BTreeMap<&'static str, usize> = BTreeMap::new();
    for h in &hits {
        *tiers.entry(h.tier).or_insert(0) += 1;
    }
    RuleTrial {
        rule_id: id.to_string(),
        status,
        violations: hits.len(),
        tiers,
        blocking: hits.iter().filter(|h| h.blocking).count(),
        hits: hits.into_iter().take(MAX_RULE_HITS).cloned().collect(),
        error: error
            .filter(|_| status == RULE_ERROR)
            .map(|m| m.trim_end().replace('\n', "; ")),
        empty_scopes: with.empty_scopes.get(id).cloned().unwrap_or_default(),
    }
}

/// A `[[constraint]]` stanza's verdict (module doc table).
fn rule_verdict(t: &RuleTrial) -> &'static str {
    match t.status {
        RULE_HOLDS if t.empty_scopes.is_empty() => KEEP,
        RULE_HOLDS | RULE_VIOLATED => REVIEW,
        _ => DROP,
    }
}

/// The candidate's verdict from its graph and rule parts: review when either
/// is, else keep when either is, else drop.
fn combine(graph: &'static str, rule: &'static str) -> &'static str {
    if graph == REVIEW || rule == REVIEW {
        REVIEW
    } else if graph == KEEP || rule == KEEP {
        KEEP
    } else {
        DROP
    }
}

/// The rule ids with a hit in one build and not the other, or erroring only
/// after: every move, not only the listed ones.
fn moved_rules(before: Option<&RuleState>, after: Option<&RuleState>) -> BTreeSet<String> {
    let none = RuleState::default();
    let (b, a) = (before.unwrap_or(&none), after.unwrap_or(&none));
    let one_sided = |x: &RuleState, y: &RuleState| -> Vec<String> {
        x.hits
            .keys()
            .filter(|k| !y.hits.contains_key(*k))
            .map(|(id, _, _)| id.clone())
            .collect()
    };
    let mut out: BTreeSet<String> = one_sided(a, b).into_iter().collect();
    out.extend(one_sided(b, a));
    out.extend(
        a.errors
            .keys()
            .filter(|id| !b.errors.contains_key(*id))
            .cloned(),
    );
    out
}

/// One build's declared rules, checked (module doc "Rules (DB.9b)").
#[derive(Default)]
struct RuleState {
    /// Every hit, by `(rule_id, rule_kind, key)`; the first per key.
    hits: BTreeMap<(String, &'static str, String), RuleHit>,
    /// Check's errors by rule id (several for one id joined by `; `).
    errors: BTreeMap<String, String>,
    /// Every declared rule id (the reflexion model's component / layer
    /// declarations excluded, an erroring rule included).
    ids: BTreeSet<String>,
    /// Per probed rule id, its empty scopes (module doc).
    empty_scopes: BTreeMap<String, Vec<String>>,
}

impl RuleState {
    /// The build evaluated a rule: it declares one, or check reported a hit
    /// (a closed reflexion model's divergence) or an error.
    fn evaluated(&self) -> bool {
        !self.ids.is_empty() || !self.hits.is_empty() || !self.errors.is_empty()
    }
}

/// `merged`'s declared rules checked; `None` (check never called) when it
/// declares none. `probe`: the rule ids whose empty scopes are read.
fn rule_state(merged: &MergedGraph, probe: &[String]) -> Option<RuleState> {
    let rules = declared_constraints(merged);
    if rules.is_empty() {
        return None;
    }
    let report = check::check_rules(merged, &rules);
    let mut by_id: BTreeMap<&str, &ConstraintRule> = BTreeMap::new();
    let mut state = RuleState::default();
    for (_, r) in &rules {
        by_id.entry(r.id.as_str()).or_insert(r);
        if !matches!(
            r.kind,
            ConstraintKind::Component { .. } | ConstraintKind::Layer { .. }
        ) {
            state.ids.insert(r.id.clone());
        }
    }
    for v in &report.violations {
        // The reflexion model has no per-rule gate (check.rs).
        let min = match by_id.get(v.rule_id.as_str()) {
            Some(r) if v.rule_kind != DIVERGENCE => check::min_tier_of(r),
            _ => check::DEFAULT_MIN_TIER,
        };
        for h in violation_hits(v, min) {
            state
                .hits
                .entry((h.rule_id.clone(), h.rule_kind, h.key.clone()))
                .or_insert(h);
        }
    }
    for (id, msg) in report.errors {
        match state.errors.get_mut(&id) {
            Some(m) => {
                m.push_str("; ");
                m.push_str(&msg);
            }
            None => {
                state.errors.insert(id, msg);
            }
        }
    }
    state.empty_scopes = empty_scopes(merged, &rules, probe);
    Some(state)
}

/// `<from> -[<category>]-> <to>`: an evidence row's identity.
fn edge_key(row: &ViolationEdge) -> String {
    format!("{} -[{}]-> {}", row.from_qname, row.category, row.to_qname)
}

/// The hits of one violation (module doc table); `min` is its rule's
/// `min_tier`.
fn violation_hits(v: &Violation, min: &'static str) -> Vec<RuleHit> {
    let hit = |key: String,
               tier: &'static str,
               blocking: bool,
               file: Option<String>,
               line: Option<i64>| RuleHit {
        rule_id: v.rule_id.clone(),
        rule_kind: v.rule_kind,
        key,
        tier,
        blocking,
        file,
        line,
    };
    let first = v.evidence.first();
    let (first_file, first_line) = (
        first.and_then(|r| r.file.clone()),
        first.and_then(|r| r.line),
    );
    match v.rule_kind {
        FORBID_EDGE | DIVERGENCE => {
            let mut out: Vec<RuleHit> = v
                .evidence
                .iter()
                .map(|row| {
                    hit(
                        edge_key(row),
                        row.tier,
                        check::is_blocking(row.tier, min),
                        row.file.clone(),
                        row.line,
                    )
                })
                .collect();
            // check lists at most MAX_EVIDENCE rows, tier-sorted; `count`
            // keeps them all.
            if v.count > v.evidence.len() {
                let tier = v.evidence.last().map_or(v.tier, |r| r.tier);
                out.push(hit(
                    format!(
                        "{} row(s) past check::MAX_EVIDENCE",
                        v.count - v.evidence.len()
                    ),
                    tier,
                    check::is_blocking(tier, min),
                    None,
                    None,
                ));
            }
            out
        }
        RULE | FORBID_PATH => vec![hit(
            v.subject.join(", "),
            v.tier,
            v.blocking,
            v.file.clone(),
            v.line,
        )],
        NO_CYCLE => {
            let members: BTreeSet<&str> = v
                .evidence
                .iter()
                .flat_map(|r| [r.from_qname.as_str(), r.to_qname.as_str()])
                .collect();
            let names: Vec<&str> = members.into_iter().collect();
            let key = format!("{} ({} members)", names.join(", "), v.count);
            vec![hit(key, v.tier, v.blocking, first_file, first_line)]
        }
        _ => {
            let rows: Vec<String> = v.evidence.iter().map(edge_key).collect();
            vec![hit(
                rows.join("; "),
                v.tier,
                v.blocking,
                first_file,
                first_line,
            )]
        }
    }
}

/// Per probed rule id (an invariant with `rule` text whose id is in
/// `probe`; the first such rule of an id), the string scopes of its positive
/// `in_scope` / `under` literals that hold no node of `merged`, once each, in
/// text order. A text that does not parse has none (check made it an error).
fn empty_scopes(
    merged: &MergedGraph,
    rules: &[(NodeId, ConstraintRule)],
    probe: &[String],
) -> BTreeMap<String, Vec<String>> {
    let mut texts: BTreeMap<&str, &str> = BTreeMap::new();
    for (_, r) in rules {
        if let ConstraintKind::Invariant {
            rule: Some(text), ..
        } = &r.kind
            && probe.contains(&r.id)
        {
            texts.entry(r.id.as_str()).or_insert(text.as_str());
        }
    }
    if texts.is_empty() {
        return BTreeMap::new();
    }
    let facts = CodeFacts::new(merged);
    // in_scope's own resolution, once per distinct string.
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for (id, text) in texts {
        let mut found: Vec<String> = Vec::new();
        if let Ok(program) = parse_program(id, text) {
            for lit in program.rules.iter().flat_map(|clause| &clause.body) {
                let Literal::Pos(atom) = lit else {
                    continue;
                };
                let [_, Term::Str { value, .. }] = atom.args.as_slice() else {
                    continue;
                };
                let members = match atom.pred.as_str() {
                    IN_SCOPE => {
                        let path = resolved
                            .entry(value.clone())
                            .or_insert_with(|| resolve_scope(merged, value));
                        facts.members(path)
                    }
                    UNDER => facts.members(value),
                    _ => continue,
                };
                if members.is_empty() && !found.contains(value) {
                    found.push(value.clone());
                }
            }
        }
        out.insert(id.to_string(), found);
    }
    out
}

/// One build, measured.
struct Measured {
    counts: GraphCounts,
    /// The graph-category gap ids (looked up only).
    ids: HashSet<String>,
    /// Its declared rules, checked; `None` when it declares none.
    rules: Option<RuleState>,
}

/// How a variant is built: one repo on one in-memory parse cache, or
/// several repos, each on its own sidecar.
enum Builder<'a> {
    One { repo: &'a str, cache: ParseCache },
    Many(&'a [String]),
}

/// The builds of one try, memoised by overlay text.
struct Trial<'a> {
    builder: Builder<'a>,
    surface: &'static str,
    /// Each built overlay text and its measure, in build order.
    texts: Vec<String>,
    built: Vec<Measured>,
}

impl<'a> Trial<'a> {
    fn new(repo_paths: &'a [String], surface: &'static str) -> Self {
        let builder = match repo_paths {
            [one] => Builder::One {
                repo: one,
                cache: ParseCache::load(one),
            },
            many => Builder::Many(many),
        };
        Self {
            builder,
            surface,
            texts: Vec::new(),
            built: Vec::new(),
        }
    }

    /// Build the tree with `text` as the primary repo's overlay (or reuse
    /// the build of an equal text) and measure it, probing the rules `probe`
    /// names for empty scopes; the index into `built`. Only WITH probes, and
    /// its text differs from BASE's unless every stanza is a duplicate (then
    /// it probes nothing), so the memo never hides a probe.
    fn measure(&mut self, text: &str, probe: &[String]) -> Result<usize, String> {
        if let Some(i) = self.texts.iter().position(|t| t == text) {
            return Ok(i);
        }
        let opts = BuildOptions::default().with_overlay_text(text.to_string());
        let result: GenerateResult = match &mut self.builder {
            Builder::One { repo, cache } => generate_one_with_cache_opts(repo, cache, &opts)?,
            Builder::Many(paths) => generate_many_opts(paths, true, &opts)?,
        };
        let measured = measure(&result.merged, self.surface, probe)?;
        self.texts.push(text.to_string());
        self.built.push(measured);
        Ok(self.built.len() - 1)
    }

    /// Save the one-repo parse cache, once (not rewritten when unchanged).
    fn finish(&self) {
        if let Builder::One { repo, cache } = &self.builder
            && let Err(e) = cache.save(repo)
        {
            eprintln!("[incremental] {repo}: warning: failed to save parse cache: {e}");
        }
    }
}

/// `merged` counted whole, the ids of its graph-category gaps, and its
/// declared rules checked.
fn measure(
    merged: &MergedGraph,
    surface: &'static str,
    probe: &[String],
) -> Result<Measured, String> {
    let counts = graph_counts(merged);
    let gaps = gaps_report(
        merged,
        &[],
        &GapsOptions {
            surface,
            ..GapsOptions::default()
        },
    )?;
    Ok(Measured {
        counts,
        ids: gaps.rows.into_iter().map(|r| r.id).collect(),
        rules: rule_state(merged, probe),
    })
}

/// Per key of either map, `after - before`; zero entries left out.
fn diff(
    before: &BTreeMap<&'static str, usize>,
    after: &BTreeMap<&'static str, usize>,
) -> BTreeMap<&'static str, i64> {
    let n = |m: &BTreeMap<&'static str, usize>, k: &str| {
        i64::try_from(m.get(k).copied().unwrap_or(0)).unwrap_or(i64::MAX)
    };
    before
        .keys()
        .chain(after.keys())
        .copied()
        .collect::<BTreeSet<&'static str>>()
        .into_iter()
        .filter_map(|k| {
            let d = n(after, k) - n(before, k);
            (d != 0).then_some((k, d))
        })
        .collect()
}
