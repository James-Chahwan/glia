//! Trying a candidate overlay (CE.3c): what would `.glia/overlay.toml` plus
//! these stanzas change, and which stanza changed what.
//!
//! [`try_candidate`] merges the candidate into the primary repo's overlay
//! text ([`merge`], CE.3b) and builds the tree with each variant through
//! `BuildOptions::with_overlay_text`: BASE (the file as it is, or
//! `version = 1` when there is none), WITH (base + every stanza) and, with
//! [`TryOptions::leave_one_out`], WITHOUT(s) for each stanza s (base + every
//! stanza but s). Each build is counted whole ([`graph_counts`], CE.3a) and
//! its graph-category gap ids read off [`gaps_report`] with no roots (the
//! root categories read the file on disk, not the overlay a build applied).
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
//! `builds` counts the distinct overlay texts built: `2 + stanzas` at most.
//!
//! Read-only for everything but the parse cache: no layout is persisted (a
//! build given `overlay_text` never may be) and `.glia/overlay.toml` is only
//! read. Stanzas are reported in candidate order and every map is a
//! `BTreeMap`; the gap-id sets are only looked up, so two tries of one tree
//! report the same bytes.
//!
//! Marker (the fired_on line), once per try:
//! `[overlay] try repo=<primary> stanzas=<n> builds=<b> verdicts keep=<k> review=<r> drop=<d> gaps <G0>→<G1> closed=<c> surface=<cli|engine>`,
//! `G` the sum of [`GraphCounts::gaps_by_category`] of BASE and WITH. The
//! `[overlay] candidate` marker is the caller's, which knows the file
//! ([`super::report_candidate`]).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde::Serialize;

use glia_code_domain::glia_config::OVERLAY_FILE;
use glia_graph::MergedGraph;

use super::writer::{Merged, StanzaRef, merge, parse_candidate};
use crate::build::{
    BuildOptions, GenerateResult, generate_many_opts, generate_one_with_cache_opts,
};
use crate::cache::ParseCache;
use crate::gaps::{
    DROP, GapsOptions, GraphCounts, KEEP, REVIEW, gaps_report, graph_counts, verdict,
};

/// The BASE overlay of a repo with no `.glia/overlay.toml` (what [`merge`]
/// starts a new file from).
const EMPTY_OVERLAY: &str = "version = 1\n";

/// How [`try_candidate`] runs. Made with `Default` plus field assignment.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TryOptions {
    /// Build base + candidate minus each stanza, one build per stanza, to
    /// attribute each stanza's effect (default `true`). `false` builds only
    /// BASE and WITH: the totals, the verdict and `closed`, and no
    /// [`TryReport::stanzas`] (nothing attributes an effect to one stanza).
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

/// One stanza of the candidate, judged by leave-one-out.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct StanzaTrial {
    /// The stanza's handle ([`StanzaRef`]'s display: `wrapper#1`,
    /// `constants.GATEWAY`, `entrypoints#2`).
    pub stanza: String,
    /// The gap ids its `# gap:` comments link, in order.
    pub gaps: Vec<String>,
    /// The linked gaps it closes: in BASE, not in WITH, back in WITHOUT(it).
    pub closes: Vec<String>,
    /// WITH minus WITHOUT(it).
    pub marginal: GraphDelta,
    /// `gaps::verdict(without, with)` (`keep` / `review` / `drop`), `keep`
    /// when `closes` is non-empty and no gap category rose.
    pub verdict: &'static str,
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
    /// `gaps::verdict(base, with)`.
    pub verdict: &'static str,
    /// The gap ids the stanzas link that are in BASE and not in WITH, in
    /// candidate order, deduped.
    pub closed: Vec<String>,
    /// Builds run: the distinct overlay texts (at most `2 + stanzas`).
    pub builds: usize,
}

/// Try `candidate_text` (a candidate: overlay sections and entrypoints, each
/// stanza linked to its target gaps by `# gap:` comments, CE.3b) against
/// `repo_paths` (the first is the primary repo, whose `.glia/overlay.toml`
/// the candidate would join; the rest build as they are): see the module
/// doc. `Err` when there is no path, the primary's overlay file cannot be
/// read, the candidate is refused ([`parse_candidate`]), it does not merge
/// into the file ([`merge`]), or a build fails.
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
    let base = trial.measure(base_text.as_deref().unwrap_or(EMPTY_OVERLAY))?;
    let with = trial.measure(&merged.text)?;
    let without = if opts.leave_one_out {
        leave_one_out(&mut trial, &merged)?
    } else {
        Vec::new()
    };
    trial.finish();

    let (b, w) = (&trial.built[base], &trial.built[with]);
    let stanzas: Vec<StanzaTrial> = without
        .iter()
        .map(|(r, i)| stanza_trial(r, b, w, &trial.built[*i]))
        .collect();
    let mut seen = BTreeSet::new();
    let closed: Vec<String> = merged
        .refs
        .iter()
        .flat_map(|(r, _)| &r.gaps)
        .filter(|id| b.ids.contains(*id) && !w.ids.contains(*id) && seen.insert(id.as_str()))
        .cloned()
        .collect();
    let report = TryReport {
        delta: GraphDelta::between(&b.counts, &w.counts),
        verdict: verdict(&b.counts, &w.counts),
        closed,
        builds: trial.built.len(),
        stanzas,
        base: b.counts.clone(),
        with: w.counts.clone(),
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

/// WITHOUT(s) for every merged stanza, in candidate order: the stanza and
/// the index of its build in [`Trial::built`].
fn leave_one_out<'m>(
    trial: &mut Trial<'_>,
    merged: &'m Merged,
) -> Result<Vec<(&'m StanzaRef, usize)>, String> {
    merged
        .refs
        .iter()
        .map(|(r, _)| Ok((r, trial.measure(&merged.without(r)?)?)))
        .collect()
}

/// One stanza judged by WITH against WITHOUT(it).
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
    let verdict = if !closes.is_empty() && marginal.gaps.values().all(|d| *d <= 0) {
        KEEP
    } else {
        verdict(&without.counts, &with.counts)
    };
    StanzaTrial {
        stanza: r.to_string(),
        gaps: r.gaps.clone(),
        closes,
        marginal,
        verdict,
    }
}

/// One build, measured.
struct Measured {
    counts: GraphCounts,
    /// The graph-category gap ids (looked up only).
    ids: HashSet<String>,
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
    /// the build of an equal text) and measure it; the index into `built`.
    fn measure(&mut self, text: &str) -> Result<usize, String> {
        if let Some(i) = self.texts.iter().position(|t| t == text) {
            return Ok(i);
        }
        let opts = BuildOptions::default().with_overlay_text(text.to_string());
        let result: GenerateResult = match &mut self.builder {
            Builder::One { repo, cache } => generate_one_with_cache_opts(repo, cache, &opts)?,
            Builder::Many(paths) => generate_many_opts(paths, true, &opts)?,
        };
        let measured = measure(&result.merged, self.surface)?;
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

/// `merged` counted whole, and the ids of its graph-category gaps.
fn measure(merged: &MergedGraph, surface: &'static str) -> Result<Measured, String> {
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
