//! The review report (CC.6a, + CC.6b): one `RevDelta` against a git rev ->
//! the changed nodes, their ranked impact, the tests to run, every added /
//! removed edge with its `why` tier, and the working tree's rules checked on
//! both sides (new vs resolved violations); CC.6b adds the markdown PR-report
//! renderer the CLI command and pyo3 `review_vs_rev` share. Public slot,
//! reached by module path (`glia_engine::review::<item>`).
//!
//! A PR reviewer asks four questions at once: what changed, what does it
//! reach, which tests prove it, and did it break a declared rule. [`review`]
//! answers all four from ONE [`RevDelta`] (one working-tree build and one rev
//! build); [`review_vs_rev`] is that call on a fresh
//! [`graph_delta_vs_rev`]. Nothing here builds a graph.
//!
//! # The parts
//!
//! - `changed` + `impact`: `diff_impact::diff_impact_from_delta` (LE.2 / CC.1)
//!   with [`ReviewArgs::blast`]; the impact rows keep the engine's ranking and
//!   are cut to `max_impact` (`counts.impact` is the uncut length);
//! - `tests`: `tests_for::tests_for_delta` (LE.3b / CC.1) with
//!   [`ReviewArgs::tests`], cut to `max_tests` (`counts.tests` is the uncut
//!   length; `test_files` is recomputed over the kept rows, so it stays "the
//!   distinct files of `tests`");
//! - `edges`: every added, removed and reconfidenced edge of the delta, the
//!   first edge with that key on its side (the working tree for added and
//!   reconfidenced, the base for removed), tiered by `why::tier_of` exactly as
//!   `glia why` and `glia check` tier it, and located like `glia delta`'s rows
//!   (the EVIDENCE site, 1-based). Sorted by (tier: fact, derived, heuristic;
//!   change: added, removed, reconfidenced; category; from; to), so a
//!   reviewer reads the observed facts first;
//! - the rules: [`declared_constraints`] of the WORKING TREE (the rules as the
//!   change leaves them) evaluated by `check::check_rules` over the working
//!   tree and over the base. Taking the rules from one side on purpose: a
//!   change that deletes its rule is not rewarded with "resolved" (the rule is
//!   simply absent on both sides then), and a rule the change adds reports
//!   what the base already broke as pre-existing, not new.
//!
//! # New and resolved violations
//!
//! A violation's IDENTITY is qname-based, so a node that keeps its qname
//! matches across the two builds, and a renamed end reads as one resolved +
//! one new (what a reviewer should see):
//!
//! - forbid_edge (and any per-edge kind, such as CC.5b's reflexion
//!   divergences): one identity per forbidden edge, `(from qname, category,
//!   to qname)` under the rule id;
//! - no_cycle: the strongly-connected component's sorted member qnames (the
//!   witness is one cycle through it; the members are its identity). A module
//!   joining an existing cycle therefore makes the grown cycle NEW and the old
//!   one resolved, even when the shortest witness did not change.
//!
//! `new_violations` are the working tree's violations with at least one
//! identity the base lacks, each reduced to its new evidence rows (`count` =
//! the new rows; a no_cycle violation is kept whole, its `count` the
//! component's size, as `check` reports it). `resolved_violations` are the
//! base's violations the same way round. `blocking` is `!new_violations
//! .is_empty()`.
//!
//! A rule that errors on the BASE side (its scope matches no node there: a
//! directory the change creates) counts as no violation there, so its
//! working-tree violations are all new. A rule that errors on the WORKING
//! TREE side goes to `check_errors` and is compared on neither side: the rule
//! cannot be evaluated as the change leaves it.
//!
//! ## Why an index beside `check_rules` (stopgap)
//!
//! `check::Violation` lists at most `check::MAX_EVIDENCE` rows and a no_cycle
//! violation carries its witness, not its members. Compared row for row, a
//! rule the base already broke more than `MAX_EVIDENCE` times (the brownfield
//! case a PR gate exists for) would misclass edges beyond the cut: one
//! removed edge lets a hidden old one into the listed rows and it reads as
//! new. So [`RuleIndex`] evaluates forbid_edge and no_cycle to identity level
//! over each side, uncapped, with check's rules (strict scope membership
//! through one `Locator`, the rule's categories or
//! `check::default_forbid_categories`, the module import graph for an
//! import-only no_cycle) and its row shape (EVIDENCE site else the `from`
//! node's location, `why::tier_of`). Every row identity check lists is
//! unioned into the index, so a drift between the two can only make an edge
//! read as pre-existing, never as new. Any other kind is compared on check's
//! listed rows; when a side lists fewer rows than its `count`, the rows it
//! cannot see are left unclassed and `check_errors` says so. REMOVAL: when
//! `check::check_rules` exposes each violation's full identity (every
//! forbidden edge uncapped, a cycle's member set), read that and delete
//! `RuleIndex`, `Scopes` and `Sub`.
//!
//! # Markdown (CC.6b)
//!
//! [`render_markdown`] is the PR report `glia review` prints and pyo3
//! `review_vs_rev(format="markdown")` returns: GitHub / GitLab flavoured and
//! a function of the [`Review`] and [`MarkdownOptions`] alone (two renders
//! are byte-identical; nothing is printed). ``## glia review vs `<base>` ``
//! and a headline of the counts, then `### New violations (blocking)` (only
//! when one exists: a `#### <rule id> (<kind>, <decl>)` table per
//! violation), `### Check errors` (when any: the working tree's rule errors
//! and the rules only partly compared), `### Tests to run`, `### Impact`,
//! `### Edge changes` (grouped by tier, facts first), `### Changed nodes` and
//! `### Resolved violations` (when any).
//!
//! Every location is `file:line`, 1-based; a removed edge or node and a
//! resolved violation's row are marked `(at <base>)`, where they are
//! located. Each table keeps [`MarkdownOptions::max_rows`] rows and a cut
//! one is followed by `_(<shown> of <total>)_`, the total uncut by
//! `max_impact` / `max_tests` (a violation's: its `count`, beyond the
//! `check::MAX_EVIDENCE` rows it lists). One escape serves every cell: text
//! backslash-escapes `\`, `|`, backticks, `*`, `<` and a `_` that is not
//! inside a word (`__init__.py`); a code span escapes `|` and widens its
//! fence past any backtick run; a line break is a space. A review with no
//! graph change renders the impact absence's note as a sentence: `No graph
//! change vs <base>.`
//!
//! # Markers
//!
//! Once per review:
//! `[review] base=<rev> changed=<C> seeds=<S> impact=<I> tests=<T> untested=<U> edges +<a> -<r> (fact=<F> derived=<D> heuristic=<H>) violations new=<N> resolved=<R> blocking=<true|false>`
//! — grep `^\[review\] base=`. `edges +a -r` counts the added and removed
//! rows; the tier split counts every edge row, reconfidenced included. The
//! component answers print their own lines: one `[delta]` pair (from
//! [`review_vs_rev`] only), one `[diff-impact]`, one `[tests-for]`, and two
//! `[check] rules=` lines (the working tree's first, then the base's).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_activation::algo::cycles::strongly_connected;
use glia_activation::algo::delta::EdgeKey;
use glia_activation::algo::{Adjacency, CategorySet, GraphSource};
use glia_code_domain::evidence::Evidence;
use glia_code_domain::external_inputs::{ConstraintKind, ConstraintRule};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Edge, EdgeCategoryId, NodeId};
use glia_graph::MergedGraph;

use crate::answers::{BlastOptions, BlastRadius, Locator, in_scope, project_roots};
use crate::check::{
    self, CheckReport, DERIVED, FACT, FORBID_EDGE, HEURISTIC, NO_CYCLE, Violation, ViolationEdge,
    check_rules, default_forbid_categories,
};
use crate::cycles::module_import_graph;
use crate::delta::{RevDelta, graph_delta_vs_rev};
use crate::diff_impact::{ChangedNode, diff_impact_from_delta};
use crate::external::declared::declared_constraints;
use crate::tests_for::{TestsFor, TestsForArgs, tests_for_delta};
use crate::why::tier_of;

/// [`ReviewArgs::default`]'s `max_impact`.
pub const DEFAULT_MAX_IMPACT: usize = 50;
/// [`ReviewArgs::default`]'s `max_tests`.
pub const DEFAULT_MAX_TESTS: usize = 50;

/// What a [`review`] walks and keeps. Start from `default()` and set fields:
/// `#[non_exhaustive]` rules out a struct literal outside this crate.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct ReviewArgs {
    /// The impact radius: `BlastOptions::default()` (both directions, depth 4).
    pub blast: BlastOptions,
    /// The tests walk: `TestsForArgs::default()`.
    pub tests: TestsForArgs,
    /// Impact rows kept ([`DEFAULT_MAX_IMPACT`]); `counts.impact` is uncut.
    /// The cut never adds an `absence`: `impact.absence` is the uncut
    /// answer's, so 0 keeps no row and still no absence when rows exist.
    pub max_impact: usize,
    /// Test rows kept ([`DEFAULT_MAX_TESTS`]); `counts.tests` is uncut and
    /// `tests.absence` is the uncut answer's, as for `max_impact`.
    pub max_tests: usize,
}

impl Default for ReviewArgs {
    fn default() -> Self {
        ReviewArgs {
            blast: BlastOptions::default(),
            tests: TestsForArgs::default(),
            max_impact: DEFAULT_MAX_IMPACT,
            max_tests: DEFAULT_MAX_TESTS,
        }
    }
}

/// One edge the change added, removed or reconfidenced, with the tier `why`
/// gives it. `change` is `added` | `removed` | `reconfidenced`; the row is read
/// from the base graph for `removed`, the working tree's for the others.
/// `site_file` / `site_line` (1-based) and `emitter` are the edge's EVIDENCE
/// (LC.3a); `note` is `why`'s row note (why the tier is not the emitter
/// stage's plain one, or where an overlay edge was declared).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ReviewEdge {
    pub change: &'static str,
    pub from_qname: String,
    pub to_qname: String,
    pub category: &'static str,
    /// `fact` | `derived` | `heuristic`.
    pub tier: &'static str,
    pub note: Option<String>,
    pub site_file: Option<String>,
    /// 1-based.
    pub site_line: Option<i64>,
    pub emitter: Option<String>,
}

/// The review's numbers, every one uncut by `max_impact` / `max_tests`.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone, Default)]
pub struct ReviewCounts {
    /// `changed.len()`: the delta's node rows plus the edge-endpoint seeds.
    pub nodes_changed: usize,
    /// The impact radius's seeds.
    pub seeds: usize,
    pub edges_added: usize,
    pub edges_removed: usize,
    pub impact: usize,
    pub tests: usize,
    /// The tests walk's seeds no test case reaches.
    pub untested_seeds: usize,
    pub new_violations: usize,
    pub resolved_violations: usize,
    /// Every edge row by tier; a tier no row has is absent.
    pub edges_by_tier: BTreeMap<&'static str, usize>,
}

/// The review of the working tree's change against `base` (module docs).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct Review {
    /// The git rev, as given.
    pub base: String,
    pub counts: ReviewCounts,
    /// `diff_impact`'s changed rows: the delta's node rows, then the
    /// edge-endpoint seeds.
    pub changed: Vec<ChangedNode>,
    /// The ranked impact of the changed nodes, `results` cut to `max_impact`.
    pub impact: BlastRadius,
    /// The tests to run, `tests` cut to `max_tests`.
    pub tests: TestsFor,
    /// Every added / removed / reconfidenced edge, facts first.
    pub edges: Vec<ReviewEdge>,
    /// The working tree's violations the base does not have, reduced to their
    /// new rows.
    pub new_violations: Vec<Violation>,
    /// The base's violations the working tree does not have, reduced to their
    /// resolved rows.
    pub resolved_violations: Vec<Violation>,
    /// `(rule id, message)`: the working tree's rule errors, plus a rule a
    /// side lists too few rows of to classify (module docs).
    pub check_errors: Vec<(String, String)>,
    /// A new violation exists.
    pub blocking: bool,
}

/// The review of the working tree's change against git rev `base` (module
/// docs): one `graph_delta_vs_rev`, then [`review`]. `Err` on its errors (not
/// a directory, not a git work tree, an unknown rev, a failed build) and on
/// `tests_for_delta`'s (more than `tests_for::MAX_SEEDS` seeds). Like the
/// delta, it saves the working tree's parse-cache sidecar, never a layout.
pub fn review_vs_rev(repo_path: &str, base: &str, args: &ReviewArgs) -> Result<Review, String> {
    let rev = graph_delta_vs_rev(repo_path, base)?;
    review(&rev, args)
}

/// The review of an already computed rev delta (module docs). Nothing is
/// built. `Err` beyond `tests_for::MAX_SEEDS` seeds only.
pub fn review(rev: &RevDelta, args: &ReviewArgs) -> Result<Review, String> {
    let (before, after) = (&rev.before.merged, &rev.after.merged);

    let di = diff_impact_from_delta(rev, &args.blast);
    let mut impact = di.impact;
    let impact_total = impact.results.len();
    impact.results.truncate(args.max_impact);

    let mut tests = tests_for_delta(rev, &args.tests)?;
    // `omitted` is what a caller's `args.tests.limit` cut (CC.9a): the
    // count stays the uncut total either way.
    let tests_total = tests.tests.len() + tests.omitted;
    if tests.tests.len() > args.max_tests {
        tests.tests.truncate(args.max_tests);
        let files: BTreeSet<&str> = tests
            .tests
            .iter()
            .filter_map(|t| t.file.as_deref())
            .collect();
        tests.test_files = files.into_iter().map(str::to_string).collect();
    }

    let (then, now) = (Locator::new(before), Locator::new(after));
    let edges = review_edges(rev, &then, &now);

    let rules = declared_constraints(after);
    let now_report = check_rules(after, &rules);
    let then_report = check_rules(before, &rules);
    let now_index = RuleIndex::build(after, &now, &rules);
    let then_index = RuleIndex::build(before, &then, &rules);
    let mut check_errors = now_report.errors.clone();
    let now_errors: HashSet<&str> = now_report
        .errors
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    let new_violations = compare(
        Side {
            report: &now_report,
            index: &now_index,
            name: "working tree",
        },
        Side {
            report: &then_report,
            index: &then_index,
            name: "base",
        },
        &now_errors,
        "new",
        &mut check_errors,
    );
    let resolved_violations = compare(
        Side {
            report: &then_report,
            index: &then_index,
            name: "base",
        },
        Side {
            report: &now_report,
            index: &now_index,
            name: "working tree",
        },
        &now_errors,
        "resolved",
        &mut check_errors,
    );

    let mut edges_by_tier: BTreeMap<&'static str, usize> = BTreeMap::new();
    for e in &edges {
        *edges_by_tier.entry(e.tier).or_insert(0) += 1;
    }
    let counts = ReviewCounts {
        nodes_changed: di.changed.len(),
        seeds: impact.seeds.len(),
        edges_added: edges.iter().filter(|e| e.change == ADDED).count(),
        edges_removed: edges.iter().filter(|e| e.change == REMOVED).count(),
        impact: impact_total,
        tests: tests_total,
        untested_seeds: tests.untested.len(),
        new_violations: new_violations.len(),
        resolved_violations: resolved_violations.len(),
        edges_by_tier,
    };
    let answer = Review {
        base: rev.answer.base.clone(),
        counts,
        changed: di.changed,
        impact,
        tests,
        edges,
        blocking: !new_violations.is_empty(),
        new_violations,
        resolved_violations,
        check_errors,
    };
    marker(&answer);
    Ok(answer)
}

fn marker(r: &Review) {
    let c = &r.counts;
    let tier = |t: &str| c.edges_by_tier.get(t).copied().unwrap_or(0);
    eprintln!(
        "[review] base={} changed={} seeds={} impact={} tests={} untested={} edges +{} -{} (fact={} derived={} heuristic={}) violations new={} resolved={} blocking={}",
        r.base,
        c.nodes_changed,
        c.seeds,
        c.impact,
        c.tests,
        c.untested_seeds,
        c.edges_added,
        c.edges_removed,
        tier(FACT),
        tier(DERIVED),
        tier(HEURISTIC),
        c.new_violations,
        c.resolved_violations,
        r.blocking,
    );
}

// ============================================================================
// markdown (module docs "Markdown")
// ============================================================================

/// [`MarkdownOptions::default`]'s `max_rows`.
pub const DEFAULT_MARKDOWN_ROWS: usize = 20;

/// How [`render_markdown`] cuts its tables. Start from `default()` and set
/// fields: `#[non_exhaustive]` rules out a struct literal outside this crate.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct MarkdownOptions {
    /// Rows kept per table ([`DEFAULT_MARKDOWN_ROWS`]); a cut table is
    /// followed by `_(<shown> of <total>)_`. 0 keeps no row, only that line.
    pub max_rows: usize,
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        MarkdownOptions {
            max_rows: DEFAULT_MARKDOWN_ROWS,
        }
    }
}

/// The review as a markdown PR report (module docs "Markdown").
pub fn render_markdown(r: &Review, opts: &MarkdownOptions) -> String {
    let c = &r.counts;
    let max = opts.max_rows;
    let mut blocks: Vec<String> = vec![format!(
        "## glia review vs {}\n**{} new violation(s)** | {} resolved | {} changed nodes | impact {} | tests {} ({} seeds untested) | edges +{} -{}",
        code(&r.base),
        c.new_violations,
        c.resolved_violations,
        c.nodes_changed,
        c.impact,
        c.tests,
        c.untested_seeds,
        c.edges_added,
        c.edges_removed,
    )];
    let unchanged = r.changed.is_empty()
        && r.edges.is_empty()
        && r.new_violations.is_empty()
        && r.resolved_violations.is_empty();
    if unchanged {
        let note = r.impact.absence.as_ref().map_or_else(
            || format!("no graph change vs {}", r.base),
            |a| a.note.clone(),
        );
        blocks.push(sentence(&note));
        check_errors_md(&mut blocks, r, max);
        return finish(blocks);
    }
    if !r.new_violations.is_empty() {
        blocks.push("### New violations (blocking)".to_string());
        for v in &r.new_violations {
            violation_md(&mut blocks, v, max, None);
        }
    }
    check_errors_md(&mut blocks, r, max);

    blocks.push("### Tests to run".to_string());
    if c.tests == 0 {
        blocks.push(none(r.tests.absence.as_ref().map(|a| a.note.as_str())));
    }
    push_table(
        &mut blocks,
        &["test", "tier", "reason", "at", "covers"],
        r.tests.tests.iter().map(|t| {
            let covers: Vec<String> = t.covers.iter().map(|q| code(q)).collect();
            vec![
                code(&t.qname),
                text(t.tier),
                text(t.reason),
                at(t.file.as_deref(), t.line),
                if covers.is_empty() {
                    DASH.to_string()
                } else {
                    covers.join(", ")
                },
            ]
        }),
        c.tests,
        max,
    );

    blocks.push("### Impact".to_string());
    if c.impact == 0 {
        blocks.push(none(r.impact.absence.as_ref().map(|a| a.note.as_str())));
    }
    push_table(
        &mut blocks,
        &["node", "depth", "via", "seed", "at", "live"],
        r.impact.results.iter().map(|b| {
            vec![
                code(&b.qname),
                b.depth.to_string(),
                text(b.reason),
                code(&b.seed),
                at(b.file.as_deref(), b.line),
                yes_no(b.live),
            ]
        }),
        c.impact,
        max,
    );

    blocks.push("### Edge changes".to_string());
    edges_md(&mut blocks, r, max);

    blocks.push("### Changed nodes".to_string());
    if r.changed.is_empty() {
        blocks.push(none(None));
    }
    push_table(
        &mut blocks,
        &["change", "node", "kind", "at", "seed"],
        r.changed.iter().map(|n| {
            let mut loc = at(n.file.as_deref(), n.line);
            if n.change == REMOVED {
                loc.push_str(&format!(" (at {})", text(&r.base)));
            }
            vec![
                text(n.change),
                code(&n.qname),
                text(n.kind),
                loc,
                yes_no(n.seed),
            ]
        }),
        r.changed.len(),
        max,
    );

    if !r.resolved_violations.is_empty() {
        blocks.push("### Resolved violations".to_string());
        for v in &r.resolved_violations {
            violation_md(&mut blocks, v, max, Some(&r.base));
        }
    }
    finish(blocks)
}

/// The blocks, one blank line apart, ending in one newline.
fn finish(blocks: Vec<String>) -> String {
    let mut out = blocks.join("\n\n");
    out.push('\n');
    out
}

/// A missing cell.
const DASH: &str = "—";

/// `_(<note>)_`, or `_(none)_` without one.
fn none(note: Option<&str>) -> String {
    format!("_({})_", note.map_or_else(|| "none".to_string(), flat))
}

/// A note as a sentence: one line, capitalised, ending in a full stop.
fn sentence(note: &str) -> String {
    let flat = flat(note);
    let mut chars = flat.chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

fn yes_no(b: bool) -> String {
    if b { "yes" } else { "no" }.to_string()
}

/// A table of at most `max` of `rows` under `header`, then `_(<shown> of
/// <total>)_` when it shows fewer than `total`. No row, no table.
fn push_table<I>(blocks: &mut Vec<String>, header: &[&str], rows: I, total: usize, max: usize)
where
    I: IntoIterator<Item = Vec<String>>,
{
    let rows: Vec<Vec<String>> = rows.into_iter().take(max).collect();
    if !rows.is_empty() {
        let mut t = format!(
            "| {} |\n|{}",
            header.join(" | "),
            "---|".repeat(header.len())
        );
        for row in &rows {
            t.push_str("\n| ");
            t.push_str(&row.join(" | "));
            t.push_str(" |");
        }
        blocks.push(t);
    }
    if rows.len() < total {
        blocks.push(format!("_({} of {total})_", rows.len()));
    }
}

/// One violation: its heading and its evidence rows. A per-edge kind's total
/// is its `count` (the rows beyond `check::MAX_EVIDENCE` included); a
/// no_cycle's rows are one shortest cycle through its `count` members.
/// `located_at`: the rev a resolved violation's rows are located in.
fn violation_md(blocks: &mut Vec<String>, v: &Violation, max: usize, located_at: Option<&str>) {
    blocks.push(match v.decl.as_deref() {
        Some(decl) => format!(
            "#### {} ({}, {})",
            text(&v.rule_id),
            text(v.rule_kind),
            text(decl)
        ),
        None => format!("#### {} ({})", text(&v.rule_id), text(v.rule_kind)),
    });
    let total = if v.rule_kind == NO_CYCLE {
        blocks.push(format!(
            "_(one shortest cycle through the {} members of its strongly-connected component)_",
            v.count
        ));
        v.evidence.len()
    } else {
        v.count.max(v.evidence.len())
    };
    push_table(
        blocks,
        &["category", "from", "to", "at", "tier"],
        v.evidence.iter().map(|e| {
            let mut loc = at(e.file.as_deref(), e.line);
            if let Some(rev) = located_at.filter(|_| e.file.is_some()) {
                loc.push_str(&format!(" (at {})", text(rev)));
            }
            vec![
                text(e.category),
                code(&e.from_qname),
                code(&e.to_qname),
                loc,
                text(e.tier),
            ]
        }),
        total,
        max,
    );
}

/// `### Check errors`: the working tree's rule errors and the rules only
/// partly compared (`Review::check_errors`); nothing when there are none.
fn check_errors_md(blocks: &mut Vec<String>, r: &Review, max: usize) {
    if r.check_errors.is_empty() {
        return;
    }
    blocks.push("### Check errors".to_string());
    push_table(
        blocks,
        &["rule", "message"],
        r.check_errors
            .iter()
            .map(|(id, msg)| vec![text(id), text(msg)]),
        r.check_errors.len(),
        max,
    );
}

/// `### Edge changes`' body: the tier counts, then the first `max` rows
/// grouped by tier under `#### <tier>` (the rows come sorted tier-first).
fn edges_md(blocks: &mut Vec<String>, r: &Review, max: usize) {
    if r.edges.is_empty() {
        blocks.push(none(None));
        return;
    }
    let mut tiers: Vec<(&str, usize)> = r
        .counts
        .edges_by_tier
        .iter()
        .map(|(t, n)| (*t, *n))
        .collect();
    tiers.sort_by_key(|(t, _)| (tier_rank(t), *t));
    let by_tier: Vec<String> = tiers
        .iter()
        .map(|(t, n)| format!("{} {n}", text(t)))
        .collect();
    blocks.push(format!("_by tier: {}_", by_tier.join(", ")));
    let shown = &r.edges[..r.edges.len().min(max)];
    for group in shown.chunk_by(|a, b| a.tier == b.tier) {
        blocks.push(format!("#### {}", text(group[0].tier)));
        push_table(
            blocks,
            &["+/-", "category", "from", "to", "at", "emitter"],
            group.iter().map(|e| {
                let sign = match e.change {
                    ADDED => "+",
                    REMOVED => "-",
                    _ => "~",
                };
                let mut loc = at(e.site_file.as_deref(), e.site_line);
                if e.change == REMOVED && e.site_file.is_some() {
                    loc.push_str(&format!(" (at {})", text(&r.base)));
                }
                vec![
                    sign.to_string(),
                    text(e.category),
                    code(&e.from_qname),
                    code(&e.to_qname),
                    loc,
                    e.emitter.as_deref().map_or_else(|| DASH.to_string(), text),
                ]
            }),
            group.len(),
            max,
        );
    }
    if shown.len() < r.edges.len() {
        blocks.push(format!("_({} of {})_", shown.len(), r.edges.len()));
    }
}

/// `file:line`, `file`, or a dash, as table text.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => text(&format!("{f}:{l}")),
        (Some(f), None) => text(f),
        _ => DASH.to_string(),
    }
}

/// `s` on one line: every line break a space.
fn flat(s: &str) -> String {
    s.replace("\r\n", " ").replace(['\n', '\r'], " ")
}

/// A table cell's text (module docs "Markdown"): one line, with `\`, `|`,
/// backticks, `*`, `<` and every `_` not inside a word backslash-escaped, so
/// no character splits the row or opens a span.
fn text(s: &str) -> String {
    let chars: Vec<char> = flat(s).chars().collect();
    let word = |i: Option<usize>| {
        i.and_then(|i| chars.get(i))
            .is_some_and(|c| c.is_alphanumeric())
    };
    let mut out = String::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        let escape = match c {
            '\\' | '|' | '`' | '*' | '<' => true,
            '_' => !(word(i.checked_sub(1)) && word(Some(i + 1))),
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A code span holding `s` in a table cell (module docs "Markdown"): one
/// line, `|` escaped, fenced by one backtick more than its longest run.
fn code(s: &str) -> String {
    if s.is_empty() {
        return DASH.to_string();
    }
    let body = flat(s).replace('|', "\\|");
    let (mut longest, mut run) = (0usize, 0usize);
    for c in body.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest + 1);
    if longest == 0 {
        format!("{fence}{body}{fence}")
    } else {
        format!("{fence} {body} {fence}")
    }
}

// ============================================================================
// edges
// ============================================================================

const ADDED: &str = "added";
const REMOVED: &str = "removed";
const RECONFIDENCED: &str = "reconfidenced";

/// Sort rank of a tier, strongest first; an unknown spelling sorts last.
fn tier_rank(t: &str) -> u8 {
    match t {
        FACT => 0,
        DERIVED => 1,
        HEURISTIC => 2,
        _ => 3,
    }
}

fn change_rank(c: &str) -> u8 {
    match c {
        ADDED => 0,
        REMOVED => 1,
        _ => 2,
    }
}

/// Every edge row of the delta (module docs "The parts").
fn review_edges(rev: &RevDelta, then: &Locator<'_>, now: &Locator<'_>) -> Vec<ReviewEdge> {
    let (old, new) = (
        first_edges(&rev.before.merged),
        first_edges(&rev.after.merged),
    );
    let d = &rev.delta;
    let mut out = Vec::with_capacity(
        d.added_edges.len() + d.removed_edges.len() + d.reconfidenced_edges.len(),
    );
    for key in &d.added_edges {
        out.push(edge_row(ADDED, key, now, new.get(key).copied()));
    }
    for key in &d.removed_edges {
        out.push(edge_row(REMOVED, key, then, old.get(key).copied()));
    }
    for (key, _, _) in &d.reconfidenced_edges {
        out.push(edge_row(RECONFIDENCED, key, now, new.get(key).copied()));
    }
    out.sort_by(|a, b| {
        (
            tier_rank(a.tier),
            change_rank(a.change),
            a.category,
            &a.from_qname,
            &a.to_qname,
        )
            .cmp(&(
                tier_rank(b.tier),
                change_rank(b.change),
                b.category,
                &b.from_qname,
                &b.to_qname,
            ))
    });
    out
}

/// Key -> the first edge (in `all_edges` order) with that key: the edge
/// `glia delta` reads the same row from.
fn first_edges(m: &MergedGraph) -> BTreeMap<EdgeKey, &Edge> {
    let mut out: BTreeMap<EdgeKey, &Edge> = BTreeMap::new();
    for e in m.all_edges() {
        out.entry(EdgeKey::from(e)).or_insert(e);
    }
    out
}

fn edge_row(
    change: &'static str,
    key: &EdgeKey,
    loc: &Locator<'_>,
    edge: Option<&Edge>,
) -> ReviewEdge {
    let ev = edge.and_then(Evidence::of);
    // A delta key always names an edge of its side; the fallback keeps a
    // missing one honest, as `tier_of` does an edge with no evidence.
    let (tier, note) = match edge {
        Some(e) => tier_of(ev.as_ref(), e),
        None => (DERIVED, Some("no evidence recorded".to_string())),
    };
    ReviewEdge {
        change,
        from_qname: loc.locate(key.from).qname,
        to_qname: loc.locate(key.to).qname,
        category: edge_category::name(key.category),
        tier,
        note,
        site_file: ev.as_ref().and_then(|e| e.file.clone()),
        // Evidence lines are 0-based rows; rows here are 1-based (LD.1).
        site_line: ev.as_ref().and_then(|e| e.line).map(|l| i64::from(l) + 1),
        emitter: ev.map(|e| e.emitter),
    }
}

// ============================================================================
// rules: new vs resolved
// ============================================================================

/// One forbidden edge's identity across the two builds.
type Ident = (String, &'static str, String);

fn ident(r: &ViolationEdge) -> Ident {
    (r.from_qname.clone(), r.category, r.to_qname.clone())
}

/// One build's rule evaluation.
#[derive(Clone, Copy)]
struct Side<'a> {
    report: &'a CheckReport,
    index: &'a RuleIndex,
    name: &'static str,
}

impl Side<'_> {
    fn violations_of<'s>(&'s self, rule_id: &'s str) -> impl Iterator<Item = &'s Violation> + 's {
        self.report
            .violations
            .iter()
            .filter(move |v| v.rule_id == rule_id)
    }

    /// Every forbidden-edge identity of `rule_id` on this side: the index's,
    /// plus every row check lists (module docs "Why an index").
    fn edge_idents(&self, rule_id: &str) -> HashSet<Ident> {
        let mut out: HashSet<Ident> = self
            .index
            .forbid
            .get(rule_id)
            .map(|rows| rows.iter().map(ident).collect())
            .unwrap_or_default();
        out.extend(
            self.violations_of(rule_id)
                .flat_map(|v| v.evidence.iter().map(ident)),
        );
        out
    }
}

/// The violations of `this` whose identity `other` lacks, each reduced to its
/// unmatched rows (module docs). `skip`: rule ids never compared (the working
/// tree's rule errors). `verb` names the result in a note.
fn compare(
    this: Side<'_>,
    other: Side<'_>,
    skip: &HashSet<&str>,
    verb: &str,
    notes: &mut Vec<(String, String)>,
) -> Vec<Violation> {
    let mut out: Vec<Violation> = Vec::new();
    for v in &this.report.violations {
        if skip.contains(v.rule_id.as_str()) {
            continue;
        }
        if v.rule_kind == NO_CYCLE {
            let members = this.index.members_of(v);
            let known = other
                .index
                .cycles
                .get(&v.rule_id)
                .is_some_and(|sets| sets.contains(&members))
                || other
                    .violations_of(&v.rule_id)
                    .any(|o| other.index.members_of(o) == members);
            if !known {
                out.push(v.clone());
            }
            continue;
        }
        let known = other.edge_idents(&v.rule_id);
        if v.rule_kind == FORBID_EDGE
            && let Some(rows) = this.index.forbid.get(&v.rule_id)
        {
            // The index's rows plus any row check lists that the index
            // lacks (module docs "Why an index"), in check's order.
            let indexed: HashSet<Ident> = rows.iter().map(ident).collect();
            let listed = v.evidence.iter().filter(|r| !indexed.contains(&ident(r)));
            let mut fresh: Vec<ViolationEdge> = rows
                .iter()
                .chain(listed)
                .filter(|r| !known.contains(&ident(r)))
                .cloned()
                .collect();
            if !fresh.is_empty() {
                fresh.sort_by(row_order);
                out.push(reduced(v, fresh.len(), fresh));
            }
            continue;
        }
        // Any other kind: check's listed rows are all there is.
        let fresh: Vec<ViolationEdge> = v
            .evidence
            .iter()
            .filter(|r| !known.contains(&ident(r)))
            .cloned()
            .collect();
        if fresh.is_empty() {
            continue;
        }
        let partial = other
            .violations_of(&v.rule_id)
            .find(|o| o.count > o.evidence.len());
        if let Some(o) = partial {
            notes.push((
                v.rule_id.clone(),
                format!(
                    "the {} lists {} of {} {} rows (check::MAX_EVIDENCE); {} {} row(s) outside that list are not classed {verb}",
                    other.name,
                    o.evidence.len(),
                    o.count,
                    v.rule_kind,
                    fresh.len(),
                    this.name,
                ),
            ));
            continue;
        }
        if v.count > v.evidence.len() {
            notes.push((
                v.rule_id.clone(),
                format!(
                    "the {} lists {} of {} {} rows (check::MAX_EVIDENCE); only the listed rows are classed {verb}",
                    this.name,
                    v.evidence.len(),
                    v.count,
                    v.rule_kind,
                ),
            ));
        }
        out.push(reduced(v, fresh.len(), fresh));
    }
    out
}

/// `v` with its evidence narrowed to `rows` (sorted tier-first, as check
/// sorts them) and capped like check's; `count` the full number, the tier the
/// strongest row's.
fn reduced(v: &Violation, count: usize, mut rows: Vec<ViolationEdge>) -> Violation {
    rows.truncate(check::MAX_EVIDENCE);
    let mut out = v.clone();
    out.tier = rows.first().map_or(v.tier, |r| r.tier);
    out.count = count;
    out.evidence = rows;
    out
}

// ============================================================================
// the stopgap index (module docs "Why an index beside check_rules")
// ============================================================================

/// forbid_edge and no_cycle evaluated to identity level over one build,
/// uncapped, with `check`'s rules. A rule that errors (an unknown category, a
/// scope no located node sits in) has no entry: no violation on this side.
struct RuleIndex {
    /// Rule id -> every forbidden-edge row, sorted as check sorts them.
    forbid: HashMap<String, Vec<ViolationEdge>>,
    /// Rule id -> each non-trivial strongly-connected component's members.
    cycles: HashMap<String, Vec<BTreeSet<String>>>,
}

impl RuleIndex {
    fn build(merged: &MergedGraph, loc: &Locator<'_>, rules: &[(NodeId, ConstraintRule)]) -> Self {
        let mut index = RuleIndex {
            forbid: HashMap::new(),
            cycles: HashMap::new(),
        };
        if rules.is_empty() {
            return index;
        }
        let mut scopes = Scopes::new(merged, loc);
        for (_, rule) in rules {
            let Some(cats) = category_ids(&rule.categories) else {
                continue;
            };
            match &rule.kind {
                ConstraintKind::ForbidEdge { from, to } => {
                    let cats = if cats.is_empty() {
                        default_forbid_categories()
                    } else {
                        cats
                    };
                    let (Some(from), Some(to)) = (scopes.members(from), scopes.members(to)) else {
                        continue;
                    };
                    index.forbid.insert(
                        rule.id.clone(),
                        forbidden_rows(merged, loc, &from, &to, &cats),
                    );
                }
                ConstraintKind::NoCycle { scope } => {
                    let keep = match scope {
                        Some(s) => match scopes.members(s) {
                            Some(set) => Some(set),
                            None => continue,
                        },
                        None => None,
                    };
                    index.cycles.insert(
                        rule.id.clone(),
                        components(merged, loc, keep.as_ref(), &cats),
                    );
                }
                _ => {}
            }
        }
        index
    }

    /// A no_cycle violation's members: the component whose first member (in
    /// qname order) starts its witness, as check picks it; the witness's own
    /// nodes when no component does (a drift from check).
    fn members_of(&self, v: &Violation) -> BTreeSet<String> {
        let start = v.evidence.first().map(|e| e.from_qname.as_str());
        let found = self
            .cycles
            .get(&v.rule_id)
            .and_then(|sets| sets.iter().find(|s| s.first().map(String::as_str) == start));
        match found {
            Some(set) => set.clone(),
            None => v.evidence.iter().map(|e| e.from_qname.clone()).collect(),
        }
    }
}

/// Edge category NAMES as ids; `None` when one is unregistered (check reports
/// that rule as an error).
fn category_ids(names: &[String]) -> Option<Vec<EdgeCategoryId>> {
    let mut ids: Vec<EdgeCategoryId> = Vec::with_capacity(names.len());
    for n in names {
        let (id, _) = edge_category::ALL
            .iter()
            .find(|(_, name)| *name == n.as_str())?;
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    Some(ids)
}

/// Strict scope membership, as check's: a node is in scope `X` when the file
/// its `Locator` places it in sits under `X`, or it is a PROJECT whose path
/// does.
struct Scopes {
    files: HashMap<NodeId, Option<String>>,
    projects: HashMap<NodeId, String>,
    cache: HashMap<String, Option<HashSet<NodeId>>>,
}

impl Scopes {
    fn new(merged: &MergedGraph, loc: &Locator<'_>) -> Self {
        let path_of: HashMap<String, String> = project_roots(merged)
            .into_iter()
            .map(|p| (p.qname, p.path))
            .collect();
        let mut files: HashMap<NodeId, Option<String>> = HashMap::new();
        let mut projects: HashMap<NodeId, String> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if files.contains_key(&n.id) {
                    continue;
                }
                files.insert(n.id, loc.file_of(n.id));
                if g.nav.kind_by_id.get(&n.id) == Some(&node_kind::PROJECT)
                    && let Some(p) = g.nav.qname_by_id.get(&n.id).and_then(|q| path_of.get(q))
                {
                    projects.insert(n.id, p.clone());
                }
            }
        }
        Scopes {
            files,
            projects,
            cache: HashMap::new(),
        }
    }

    /// The nodes strictly in `scope`; `None` when there are none (a rule
    /// error in check).
    fn members(&mut self, scope: &str) -> Option<HashSet<NodeId>> {
        if !self.cache.contains_key(scope) {
            let set: HashSet<NodeId> = self
                .files
                .iter()
                .filter(|(id, file)| {
                    file.as_deref().is_some_and(|f| in_scope(f, scope))
                        || self.projects.get(id).is_some_and(|p| in_scope(p, scope))
                })
                .map(|(id, _)| *id)
                .collect();
            self.cache
                .insert(scope.to_string(), (!set.is_empty()).then_some(set));
        }
        self.cache.get(scope).cloned().flatten()
    }
}

/// Every direct edge of `cats` from `from` into `to`, one row per
/// `(from, category, to, evidence file, evidence line)` as check dedups them,
/// sorted as check sorts them.
fn forbidden_rows(
    merged: &MergedGraph,
    loc: &Locator<'_>,
    from: &HashSet<NodeId>,
    to: &HashSet<NodeId>,
    cats: &[EdgeCategoryId],
) -> Vec<ViolationEdge> {
    type Site = (NodeId, u32, NodeId, Option<String>, Option<u32>);
    let mut seen: HashSet<Site> = HashSet::new();
    let mut rows: Vec<ViolationEdge> = Vec::new();
    for e in merged.all_edges() {
        if !cats.contains(&e.category) || !from.contains(&e.from) || !to.contains(&e.to) {
            continue;
        }
        let ev = Evidence::of(e);
        let key = (
            e.from,
            e.category.0,
            e.to,
            ev.as_ref().and_then(|x| x.file.clone()),
            ev.as_ref().and_then(|x| x.line),
        );
        if seen.insert(key) {
            rows.push(violation_row(loc, e, ev.as_ref()));
        }
    }
    rows.sort_by(row_order);
    rows
}

/// Check's evidence order: strongest tier first (so the `MAX_EVIDENCE` cut
/// keeps the facts), then located rows by (file, line, category); qnames
/// break the rest.
fn row_order(a: &ViolationEdge, b: &ViolationEdge) -> std::cmp::Ordering {
    (
        tier_rank(a.tier),
        a.file.is_none(),
        &a.file,
        a.line.is_none(),
        a.line,
        a.category,
    )
        .cmp(&(
            tier_rank(b.tier),
            b.file.is_none(),
            &b.file,
            b.line.is_none(),
            b.line,
            b.category,
        ))
        .then_with(|| a.from_qname.cmp(&b.from_qname))
        .then_with(|| a.to_qname.cmp(&b.to_qname))
}

/// One located row, as check's: at the EVIDENCE site when it names a file,
/// else at the `from` node; tiered by `why::tier_of`.
fn violation_row(loc: &Locator<'_>, e: &Edge, ev: Option<&Evidence>) -> ViolationEdge {
    let f = loc.locate(e.from);
    let site = ev.and_then(|x| {
        x.file
            .clone()
            .map(|file| (Some(file), x.line.map(|l| i64::from(l) + 1)))
    });
    let (file, line) = site.unwrap_or((f.file, f.line));
    let (tier, note) = tier_of(ev, e);
    ViolationEdge {
        from_qname: f.qname,
        to_qname: loc.locate(e.to).qname,
        category: edge_category::name(e.category),
        file,
        line,
        emitter: ev.map(|x| x.emitter.clone()),
        tier,
        note,
    }
}

/// A graph restricted to one scope: the kept nodes and the edges between them.
struct Sub {
    nodes: Vec<NodeId>,
    edges: Vec<Edge>,
}

impl GraphSource for Sub {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

/// The member qnames of every non-trivial strongly-connected component check's
/// no_cycle evaluates: the module import graph when `cats` is empty or only
/// IMPORTS, else the node-level graph of `cats`; restricted to `keep` (every
/// node when `None`).
fn components(
    merged: &MergedGraph,
    loc: &Locator<'_>,
    keep: Option<&HashSet<NodeId>>,
    cats: &[EdgeCategoryId],
) -> Vec<BTreeSet<String>> {
    let inside = |id: &NodeId| keep.is_none_or(|k| k.contains(id));
    let mut sub = Sub {
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut add = |sub: &mut Sub, e: &Edge| {
        for id in [e.from, e.to] {
            if seen.insert(id) {
                sub.nodes.push(id);
            }
        }
        sub.edges
            .push(Edge::new(e.from, e.to, e.category, e.confidence));
    };
    if cats.is_empty() || cats == [edge_category::IMPORTS] {
        let imports = module_import_graph(merged);
        for e in imports.edges() {
            if inside(&e.from) && inside(&e.to) {
                add(&mut sub, e);
            }
        }
    } else {
        for e in merged.all_edges() {
            if cats.contains(&e.category) && inside(&e.from) && inside(&e.to) {
                add(&mut sub, e);
            }
        }
    }
    let adj = Adjacency::build(&sub, &CategorySet::all());
    strongly_connected(&adj)
        .into_iter()
        .map(|comp| comp.into_iter().map(|id| loc.locate(id).qname).collect())
        .collect()
}
