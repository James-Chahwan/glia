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
//!   with [`ReviewArgs::blast`] (an enabled identity collapse with no repo
//!   labels labelled with the working tree's, DD.10, as `tests_for_delta`
//!   labels its own); the impact rows keep the engine's ranking and
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
//!   change leaves them) evaluated by `check::check_rules_gated` (DT.7c, gated
//!   as `glia check` gates them, with no roots) over the working tree and over
//!   the base. Taking the rules from one side on purpose: a change that
//!   deletes its rule is not rewarded with "resolved" (the rule is simply
//!   absent on both sides then), and a rule the change adds reports what the
//!   base already broke as pre-existing, not new;
//! - `model_breaks` (DT.1p): the inferred client <-> server model breaks of
//!   the same rev pair (`contract_breaks::model_breaks_of`, DT.1o), the
//!   BREAKING rows only (a PR report; a compatible model change is
//!   contract-breaks' business), each blocking or advisory under
//!   [`ReviewArgs::models_min_tier`] (below).
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
//!   one resolved, even when the shortest witness did not change;
//! - rule (DB.3a) and forbid_path (DB.3b): the violation's `subject` under
//!   the rule id (a rule row's head values rendered; a transitive hit's
//!   qname). A rule row may derive from no edge (a negation-only row such as
//!   "every handler is called" has no evidence), and a forbid_path witness
//!   moves whenever a shorter or better-tier path appears, so neither is
//!   identified by its rows: a fresh path into a node the base already
//!   reached is the same violation, not a new one.
//!
//! `new_violations` are the working tree's violations with at least one
//! identity the base lacks, each reduced to its new evidence rows (`count` =
//! the new rows; a no_cycle, rule or forbid_path violation is kept whole, as
//! `check` reports it). `resolved_violations` are the base's violations the
//! same way round. A reduced violation's `tier` is its strongest kept row's
//! and its `blocking` is recomputed from that tier under its rule's
//! `min_tier` (`check::is_blocking`; the working tree's rule of that id, the
//! default for a `reflexion:` divergence), so a fact-broken rule to which a
//! change adds only an overlay edge reports one heuristic, advisory row.
//! The review's `blocking` is whether a NEW violation blocks: one at or above
//! its rule's `min_tier`, never a heuristic one (0.5.2 Q2), or a model row
//! blocks (below). An advisory new violation is listed, and counted in
//! `counts.new_violations`, but does not fail the gate.
//!
//! A rule whose scope matches no node on the BASE side (a directory the
//! change creates) checks nothing there, so its working-tree violations are
//! all new. A rule that checks nothing on the WORKING TREE side (DT.7b /
//! DT.7c, James's ruling 1: listed, not an error; a `from` / `to` / `via` /
//! no_cycle scope no located node sits in) is listed in `unanchored` with its
//! reason and fix, is never blocking and never a rule error, and is compared
//! on neither side: the base's violations of a rule that no longer checks
//! anything are not "resolved". A rule that errors on the WORKING TREE side
//! (an unknown category, a rule text that does not parse) goes to
//! `check_errors` and is compared on neither side either: the rule cannot be
//! evaluated as the change leaves it.
//!
//! # Model breaks (DT.1p)
//!
//! `model_breaks` lists the breaking inferred-model rows; a row blocks when
//! `check::is_blocking(tier, models_min_tier)` holds. The default
//! `models_min_tier` is `fact` (check's vocabulary, `fact` | `derived`, not
//! check's default `derived`): an inferred pairing is advisory (James's
//! ruling 4, 2026-10-17), and every HTTP / GraphQL pairing glia 0.5.2 infers
//! is derived, so by default no model row blocks. The review's `blocking` is
//! a new blocking violation OR a blocking model row.
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
//! read as pre-existing, never as new. A transitive forbid_edge is left out
//! of the index on purpose: check lists every forbid_path violation uncapped
//! (one per hit, sorted by subject), as it lists every rule row, and both
//! are classed by subject. Any other kind (a reflexion divergence) is
//! compared on check's listed rows; when a side lists fewer rows than its
//! `count`, the rows it cannot see are left unclassed and `check_errors` says
//! so. REMOVAL: when
//! `check::check_rules` exposes each violation's full identity (every
//! forbidden edge uncapped, a cycle's member set), read that and delete
//! `RuleIndex`, `Scopes` and `Sub`.
//!
//! # Intent (EB.7, 0.5.3 items 3, 6, 7)
//!
//! [`review_vs_rev`] reads what the PR says it is about: its title
//! ([`ReviewArgs::title`], `glia review --title`), else the subject of the
//! newest non-merge commit of `<base>..HEAD` (redacted as untrusted text),
//! else nothing (no `intent`; the review is exactly the one without EB.7).
//! Leak-free: the PR's own commits (`git rev-list --no-merges
//! <base>..HEAD`) are never text documents, and the ids route through the
//! BASE tree. Over the working tree's candidate files of its repo
//! (`intent::text::Candidates`, path order, the changed files excluded):
//!
//! - `idr`: typed id routing (`intent::idr::route`) of the title at the
//!   base: its docs and trackers (`Tree::Rev`), the base build's PROJECT dirs
//!   and Tier-4 snapshot sections, the base candidates (mapped onto the
//!   working tree's by path; a file new in the working tree has 0), the
//!   history snapshot's renames, the recent scopes of its subjects; the
//!   `[ids]` vocabulary is the working tree's, as check's rules are;
//! - `text`: BM25 of the title's subject over past commit messages
//!   (`intent::text::TextIndex`), zeros and `text: absent` without the
//!   snapshot's messages;
//! - `ppr`: rank_eval's P1 from every node of the changed files, max-pooled
//!   (`rank::rank_files` under `Ranking::head`, over the review's RankCache).
//!
//! `intent.files` are the candidates with any signal in LX[IDR,P1] order
//! ([`INTENT_FUSION_REVIEW`]), at most `max_impact`. The impact rows take
//! LX[IDR, their existing order]: when some file has IDR > 0, rows in such
//! files move first by the file's IDR descending, every other row keeps its
//! relative order after them; text never moves a row, and with no routed id
//! the order is byte-identical to the review without a title. H9s50 and
//! LX[IDR,H9s50] are computed only for the calibrated selection (a fixed
//! H9s50 order loses on a gate snapshot), never shipped as an order here.
//!
//! # Markdown (CC.6b)
//!
//! [`render_markdown`] is the PR report `glia review` prints and pyo3
//! `review_vs_rev(format="markdown")` returns: GitHub / GitLab flavoured and
//! a function of the [`Review`] and [`MarkdownOptions`] alone (two renders
//! are byte-identical; nothing is printed). ``## glia review vs `<base>` ``
//! and a headline of the counts (`**<N> new violation(s)**`, followed by
//! ` (<A> advisory)` only when `A` of them do not block; when `M` model
//! breaks exist it ends ` | <M> model break(s)`, then ` (<A> advisory)` when
//! `A` of them do not block), then `### New violations (blocking)` (only when
//! a blocking one exists: a `#### <rule id> (<kind>, <decl>)` table per
//! violation), `### New violations (advisory: below the rule's min_tier)`
//! (only when an advisory one exists), `### Model breaks (blocking)` and
//! `### Model breaks (advisory: an inferred client <-> server pairing below
//! the models min tier)` (each only when such a row exists: one table `|
//! pairing | via | tier | rule | field | before | after | client | server |`,
//! one line per breaking change; `via` the first exchange's key, ` (+<n>)`
//! for the rest; a site is the side's qname then the `file:line` of the
//! change's field there, else of the side, ` (at <base>)` on a site read
//! from the rev, a dash for none), `### Check errors` (when any: the working
//! tree's rule errors and the rules only partly compared), `### Rules that
//! check nothing` (when any: one bullet per `unanchored` row, its reason,
//! then its `fix:`), `### Tests to run`, `### Impact`, `### Intent` (EB.7,
//! with an intent: `_title (<source>): <title>_`, the routed-ids table `| id
//! | namespace | tier | route | defined at | state | files |` when an id
//! routed, the files table `| # | file | idr | text | ppr | ids | in impact
//! |`, and `_(no history messages: run glia history sync)_` when the text is
//! absent; after the sentence of a review with no graph change), `### Edge changes`
//! (grouped by tier, facts first), `### Changed nodes` and `### Resolved
//! violations` (when any). A rule or forbid_path violation renders its
//! identity first, ``_subject: `<a>`, `<b>` - tier <t> at <file:line>_`` (no
//! ` at` when the subject is unlocated), then its witness table when it has
//! rows. A rule's coverage caveats print ONCE per section (DT.8a's fold,
//! after check): under the rule's first violation there, the caveats of
//! every violation of the rule in that section deduplicated by (category,
//! language, note) in first-seen order, up to 5 lines, `⚠ <category>
//! (<language>): <note>`, a cut list followed by `_(<shown> of <total>
//! caveats)_`; its later violations print none (a rule's negations are
//! static, so every row carried the same list). JSON keeps every
//! violation's own list. [`caveat_fold`] counts what the fold saved.
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
//! rows; the tier split counts every edge row, reconfidenced included;
//! `new=` counts every new violation, advisory ones included, and
//! `blocking=` is the gate (a new violation at or above its rule's
//! `min_tier`, or a blocking model row). The line is otherwise unchanged by
//! DT.1p: the model counts are on the `[model-breaks] base=` line. The
//! component answers print their own lines: one `[delta]` pair (from
//! [`review_vs_rev`] only), one `[diff-impact]`, one `[tests-for]`, two
//! `[check] rules=` lines (the working tree's first, then the base's), and
//! one `[model-breaks] base=<rev> min_tier=<t> ..` line, after the `[check]`
//! lines and before `[review] base=`.
//!
//! EB.7, from [`review_vs_rev`] only, right after `[review] base=`:
//! `[review-intent] base=<rev> title_source=<arg|commit|none>
//! fusion=LX[IDR,P1] ids=<n> idr_files=<i> text=<present|absent>
//! text_files=<t> candidates=<c> reordered=<r>` (`r` the impact rows whose
//! position moved), after the intent's own `[rank-files] .. ranking=P1`,
//! `[intent] text`, `[ids] vocab`, `[id-defs]`, `[id-ns]`, `[id-paths]` and
//! `[idr]` lines (`surface=review`), which a title prints and `none` does
//! not.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use glia_activation::algo::fuse;
use glia_code_domain::snapshots::{read_history, read_history_messages, redact_untrusted};
use glia_core::RepoId;

use glia_activation::algo::cycles::strongly_connected;
use glia_activation::algo::delta::EdgeKey;
use glia_activation::algo::{Adjacency, CategorySet, GraphSource};
use glia_code_domain::evidence::Evidence;
use glia_code_domain::external_inputs::{ConstraintKind, ConstraintRule};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Edge, EdgeCategoryId, NodeId};
use glia_graph::MergedGraph;

use crate::answers::{BlastAnswer, BlastOptions, BlastRadius, Locator, in_scope, project_roots};
use crate::check::{
    self, CheckOptions, CheckReport, DEFAULT_MIN_TIER, DERIVED, EMPTY_SCOPE, FACT, FORBID_EDGE,
    FORBID_PATH, HEURISTIC, NO_CYCLE, RULE, Unanchored, Violation, ViolationEdge,
    check_rules_gated, default_forbid_categories,
};
use crate::contract_breaks::{
    MODEL_MIN_TIERS, ModelBreak, ModelBreakArgs, ModelSite, check_models_min_tier, model_breaks_of,
};
use crate::contract_fields::FieldSide;
use crate::coverage::CoverageNote;
use crate::cycles::module_import_graph;
use crate::delta::{RevDelta, graph_delta_vs_rev};
use crate::diff_impact::{ChangedNode, diff_impact_from_delta_on};
use crate::external::declared::declared_constraints;
use crate::git_rev::{self, grep_words_at};
use crate::identity::Collapse;
use crate::intent::defs::{Namespaces, Tree, read_defs};
use crate::intent::idr::{
    Idr, IdrInputs, RoutedId, comment_part, recent_scopes, recent_subjects, route,
};
use crate::intent::ids::IdVocab;
use crate::intent::paths::{PathResolver, rename_pairs};
use crate::intent::text::{Candidates, TextIndex, exclude_range, subject};
use crate::rank::{FileIndex, RankCache, Ranking, rank_files};
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
    /// An enabled `collapse` with no repo labels takes the working tree's
    /// (`rev.after.repo_labels`), so `also_in` rows name the repo.
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
    /// The weakest pairing tier at which a breaking inferred-model row blocks
    /// (`contract_breaks::MODEL_MIN_TIERS`; module docs "Model breaks"):
    /// `fact` (the default: every pairing glia 0.5.2 infers is derived, so
    /// its rows are advisory) or `derived`. [`review_vs_rev`] refuses any
    /// other value before building; [`review`] judges one as `fact`.
    pub models_min_tier: &'static str,
    /// 0.5.3 (EB.7): the PR title, the intent [`review_vs_rev`] ranks by
    /// (module docs "Intent"). `None` (the default) takes the subject of the
    /// newest non-merge commit of `<base>..HEAD`, and no intent when there is
    /// none. [`review`] over a caller's delta reads no title.
    pub title: Option<String>,
}

impl Default for ReviewArgs {
    fn default() -> Self {
        ReviewArgs {
            blast: BlastOptions::default(),
            tests: TestsForArgs::default(),
            max_impact: DEFAULT_MAX_IMPACT,
            max_tests: DEFAULT_MAX_TESTS,
            models_min_tier: MODEL_MIN_TIERS[0],
            title: None,
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
    /// `model_breaks.len()`: the breaking inferred-model rows, advisory ones
    /// included.
    pub model_breaks: usize,
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
    /// new rows; advisory ones (`blocking` false) included.
    pub new_violations: Vec<Violation>,
    /// The base's violations the working tree does not have, reduced to their
    /// resolved rows.
    pub resolved_violations: Vec<Violation>,
    /// The breaking inferred client <-> server model rows of the change
    /// (module docs "Model breaks"), `contract_breaks`' order; advisory ones
    /// (`blocking` false) included.
    pub model_breaks: Vec<ModelBreak>,
    /// `(rule id, message)`: the working tree's rule errors, plus a rule a
    /// side lists too few rows of to classify (module docs).
    pub check_errors: Vec<(String, String)>,
    /// The working tree's rules that check nothing (`check::EMPTY_SCOPE`
    /// rows, reason and fix; module docs "New and resolved violations"):
    /// never blocking, never a rule error. Not serialised when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unanchored: Vec<Unanchored>,
    /// A new BLOCKING violation exists (one at or above its rule's
    /// `min_tier`, never a heuristic one: `Violation::blocking`, module docs
    /// "New and resolved violations"), or a blocking model row
    /// (`ModelBreak::blocking`, module docs "Model breaks"). The CI gate.
    pub blocking: bool,
    /// 0.5.3 (EB.7): what the PR says it is about (module docs "Intent"):
    /// its title, the ids it routes, and the files ranked LX[IDR,P1]. Only
    /// from [`review_vs_rev`] with a title or a commit in `<base>..HEAD`;
    /// not serialised when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<ReviewIntent>,
}

/// The review of the working tree's change against git rev `base` (module
/// docs): one `graph_delta_vs_rev`, then [`review`]. `Err` on an unknown
/// [`ReviewArgs::models_min_tier`] (`unknown models min tier `<x>`: expected
/// one of fact, derived`, before anything is built), on the delta's errors
/// (not a directory, not a git work tree, an unknown rev, a failed build) and
/// on `tests_for_delta`'s (more than `tests_for::MAX_SEEDS` seeds). Like the
/// delta, it saves the working tree's parse-cache sidecar, never a layout.
pub fn review_vs_rev(repo_path: &str, base: &str, args: &ReviewArgs) -> Result<Review, String> {
    check_models_min_tier(args.models_min_tier)?;
    let rev = graph_delta_vs_rev(repo_path, base)?;
    let ask = intent_request(Path::new(repo_path), base, args.title.as_deref());
    review_with_intent(&rev, args, ask)
}

/// The review of an already computed rev delta (module docs). Nothing is
/// built. `Err` beyond `tests_for::MAX_SEEDS` seeds only. No intent: a
/// delta carries no repo path to read a title or a base tree from
/// ([`ReviewArgs::title`] is read by [`review_vs_rev`] only).
pub fn review(rev: &RevDelta, args: &ReviewArgs) -> Result<Review, String> {
    review_with_intent(rev, args, IntentAsk::Off)
}

/// [`review`] with the intent section [`review_vs_rev`] asked for.
fn review_with_intent(rev: &RevDelta, args: &ReviewArgs, ask: IntentAsk) -> Result<Review, String> {
    let (before, after) = (&rev.before.merged, &rev.after.merged);

    // DD.10: an enabled collapse with no repo labels takes the working
    // tree's, as `tests_for_delta` does (DD.12), so an impact row's
    // `also_in` names the repo by its label, not `repo#<id % 1000>`.
    let labelled;
    let blast = if args.blast.collapse.enabled
        && args.blast.collapse.repo_labels.is_empty()
        && !rev.after.repo_labels.is_empty()
    {
        let mut with = args.blast.clone();
        with.collapse = Collapse::with_labels(&rev.after.repo_labels);
        labelled = with;
        &labelled
    } else {
        &args.blast
    };
    // EB.7: one RankCache for the impact walk and the intent's P1, built as
    // `diff_impact_from_delta_on` builds its transient one (so the impact
    // rows are unchanged by it).
    let cached;
    let blast = if blast.rank_cache.is_none() {
        let mut with = blast.clone();
        with.rank_cache = Some(Arc::new(RankCache::new(
            "answer",
            &rev.after.repo_roots,
            &rev.after.repo_labels,
        )));
        cached = with;
        &cached
    } else {
        blast
    };
    let di = diff_impact_from_delta_on(rev, blast, "review");
    let mut impact = di.impact;
    let (intent, intent_line) = match blast.rank_cache.as_deref() {
        Some(cache) => review_intent(rev, cache, &mut impact, args.max_impact, ask),
        None => (None, None),
    };
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
    // DT.7c's gated form, as glia check gates: a scope no located node sits
    // in is an EMPTY_SCOPE row of `unanchored`, not a rule error.
    let gated = CheckOptions::default();
    let now_report = check_rules_gated(after, &rules, &gated);
    let then_report = check_rules_gated(before, &rules, &gated);
    let now_index = RuleIndex::build(after, &now, &rules);
    let then_index = RuleIndex::build(before, &then, &rules);
    let gates = min_tiers(&rules);
    let mut check_errors = now_report.errors.clone();
    let unanchored: Vec<Unanchored> = now_report
        .unanchored
        .iter()
        .filter(|u| u.status == EMPTY_SCOPE)
        .cloned()
        .collect();
    // Compared on neither side: the working tree's rule errors and its rules
    // that check nothing (module docs).
    let now_errors: HashSet<&str> = now_report
        .errors
        .iter()
        .map(|(id, _)| id.as_str())
        .chain(unanchored.iter().map(|u| u.rule_id.as_str()))
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
        &gates,
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
        &gates,
        "resolved",
        &mut check_errors,
    );

    let mut model_breaks = model_breaks_of(
        &rev.answer.base,
        before,
        &rev.after,
        &rev.delta,
        &ModelBreakArgs {
            min_tier: args.models_min_tier,
        },
    )
    .rows;
    model_breaks.retain(|m| m.status == "breaking");

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
        model_breaks: model_breaks.len(),
        edges_by_tier,
    };
    let answer = Review {
        base: rev.answer.base.clone(),
        counts,
        changed: di.changed,
        impact,
        tests,
        edges,
        blocking: new_violations.iter().any(|v| v.blocking)
            || model_breaks.iter().any(|m| m.blocking),
        new_violations,
        resolved_violations,
        model_breaks,
        check_errors,
        unanchored,
        intent,
    };
    marker(&answer);
    if let Some(line) = intent_line {
        eprintln!("{line}");
    }
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
// intent (module docs "Intent", EB.7)
// ============================================================================

/// The fusion that orders `intent.files` and moves the impact rows (EB.7):
/// the typed links first (IDR), then rank_eval's P1 (module docs "Intent").
/// The calibrated selection (EC.11) keeps it as its uncalibrated fallback.
pub const INTENT_FUSION_REVIEW: &str = "LX[IDR,P1]";

/// What the PR says it is about (module docs "Intent").
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ReviewIntent {
    /// The title ranked by: the caller's, or the newest commit subject of
    /// `<base>..HEAD` (redacted as untrusted text).
    pub title: String,
    /// `arg` | `commit`.
    pub title_source: &'static str,
    /// The fusion `files` and the impact order follow: [`INTENT_FUSION_REVIEW`].
    pub fusion: &'static str,
    /// Every id (or conventional-commit scope) the title routed, read at the
    /// base, unresolved ones included with their reason.
    pub ids: Vec<RoutedId>,
    /// The candidate files with any signal, `fusion`'s order, at most
    /// `max_impact`; the changed files are never listed.
    pub files: Vec<IntentFile>,
    /// `present` | `absent`: whether the history snapshot's messages were read.
    pub text: &'static str,
    /// The per-candidate signals the measured fusions read (crate API for
    /// the calibrated selection).
    #[serde(skip)]
    pub(crate) signals: IntentSignals,
}

impl ReviewIntent {
    /// [`ReviewIntent::files`] from the signals: the candidates with any
    /// signal in [`ReviewIntent::fusion`]'s order (the first of
    /// [`IntentSignals::fusions`]), at most `max`.
    fn listed(
        &self,
        paths: &[String],
        ids_of: &[Vec<String>],
        with_rows: &BTreeSet<usize>,
        max: usize,
    ) -> Vec<IntentFile> {
        let s = &self.signals;
        let [(fusion, order), _, _] = s.fusions();
        debug_assert_eq!(fusion, self.fusion);
        order
            .into_iter()
            .filter(|&i| s.idr[i] > 0.0 || s.text[i] > 0.0 || s.ppr[i] > 0.0)
            .take(max)
            .enumerate()
            .map(|(k, i)| IntentFile {
                rank: k + 1,
                file: paths.get(i).cloned().unwrap_or_default(),
                idr: s.idr[i],
                text: s.text[i],
                ppr: s.ppr[i],
                ids: ids_of.get(i).cloned().unwrap_or_default(),
                in_impact: with_rows.contains(&i),
            })
            .collect()
    }
}

/// One file of [`ReviewIntent::files`].
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct IntentFile {
    /// 1-based.
    pub rank: usize,
    pub file: String,
    /// Typed id routing at the base (`intent::idr`): named + cited weight.
    pub idr: f64,
    /// BM25 over past commit messages (`intent::text`), 0 when absent.
    pub text: f64,
    /// rank_eval's P1 from the changed files, max-pooled.
    pub ppr: f64,
    /// The routed ids that reached the file, in title order.
    pub ids: Vec<String>,
    /// An impact row sits in the file (the uncut impact).
    pub in_impact: bool,
}

/// The signals per candidate (one repo's [`Candidates`], path order) the
/// review's intent fuses; `excluded` are the changed files, `tie` the
/// candidate position.
#[derive(Debug, Clone, Default)]
pub(crate) struct IntentSignals {
    pub(crate) idr: Vec<f64>,
    pub(crate) text: Vec<f64>,
    pub(crate) ppr: Vec<f64>,
    pub(crate) excluded: Vec<bool>,
    pub(crate) tie: Vec<u32>,
}

impl IntentSignals {
    /// The three measured fusions, rank_eval `r3_bank_seed`'s arithmetic:
    /// LX[IDR,P1] (shipped), H9s50 = `blend(0.5, minmax(text), minmax(ppr))`
    /// and LX[IDR,H9s50] (raw IDR first). The last two are candidates for the
    /// calibrated selection (items 4 / 8), never a fixed order here.
    pub(crate) fn fusions(&self) -> [(&'static str, Vec<usize>); 3] {
        let (tie, ex) = (&self.tie, &self.excluded);
        let h9 = fuse::blend(
            0.5,
            &fuse::minmax(&self.text, ex),
            &fuse::minmax(&self.ppr, ex),
        );
        [
            (
                INTENT_FUSION_REVIEW,
                fuse::lx(&self.idr, &[&self.ppr], tie, ex),
            ),
            ("H9s50", fuse::order(&h9, tie, ex)),
            ("LX[IDR,H9s50]", fuse::lx(&self.idr, &[&h9], tie, ex)),
        ]
    }
}

/// Whether and how a review computes its intent.
enum IntentAsk {
    /// [`review`] over a caller's delta: no intent, no marker.
    Off,
    /// No title and no commit in `<base>..HEAD`: no intent, the marker says so.
    NoTitle,
    Title(IntentIn),
}

/// The intent's inputs [`review_vs_rev`] reads from the repo.
struct IntentIn {
    title: String,
    source: &'static str,
    repo: PathBuf,
    /// The base commit, full (a `Tree::Rev` needs one).
    base_sha: String,
    /// The PR's own commits (`base..HEAD`), never text documents.
    exclude: BTreeSet<String>,
}

/// The title and the base of a [`review_vs_rev`] (module docs "Intent").
fn intent_request(repo: &Path, base: &str, title: Option<&str>) -> IntentAsk {
    let Ok(rev) = git_rev::resolve_rev(repo, base) else {
        return IntentAsk::NoTitle;
    };
    let exclude = exclude_range(repo, &rev.sha, "HEAD").unwrap_or_default();
    let (title, source) = match title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => (t.to_string(), "arg"),
        None => match newest_subject(repo, &rev.sha) {
            Some(s) => (s, "commit"),
            None => return IntentAsk::NoTitle,
        },
    };
    IntentAsk::Title(IntentIn {
        title,
        source,
        repo: repo.to_path_buf(),
        base_sha: rev.sha,
        exclude,
    })
}

/// `git log -1 --no-merges --format=%s <base>..HEAD`, redacted
/// (`redact_untrusted`); None when the range has no commit or git fails.
fn newest_subject(repo: &Path, base_sha: &str) -> Option<String> {
    let range = format!("{base_sha}..HEAD");
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .args([
            "log",
            "-1",
            "--no-merges",
            "--no-show-signature",
            "--no-color",
            "--format=%s",
            &range,
            "--",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let subject = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!subject.is_empty()).then(|| redact_untrusted(&subject).0)
}

/// The intent of a review and its `[review-intent]` line (module docs
/// "Intent"); moves `impact.results` (uncut) typed-links-first.
fn review_intent(
    rev: &RevDelta,
    cache: &RankCache,
    impact: &mut BlastRadius,
    max_files: usize,
    ask: IntentAsk,
) -> (Option<ReviewIntent>, Option<String>) {
    let x = match ask {
        IntentAsk::Off => return (None, None),
        IntentAsk::NoTitle => {
            let files = cache.files(&rev.after.merged);
            let n = primary_repo(rev, &files).map_or(0, |r| Candidates::of(&files, r).len());
            let line = format!(
                "[review-intent] base={} title_source=none fusion={INTENT_FUSION_REVIEW} ids=0 idr_files=0 text=absent text_files=0 candidates={n} reordered=0",
                rev.answer.base
            );
            return (None, Some(line));
        }
        IntentAsk::Title(x) => x,
    };
    let (before, after) = (&rev.before.merged, &rev.after.merged);
    let files = cache.files(after);
    let primary = primary_repo(rev, &files);
    let label = primary
        .and_then(|r| rev.after.repo_labels.get(&r).cloned())
        .unwrap_or_else(|| "-".to_string());
    let cands = primary
        .map(|r| Candidates::of(&files, r))
        .unwrap_or_default();
    let n = cands.len();

    // The changed files: the impact's seeds' files (EA.5's seed rule).
    let mut seed_files: Vec<u32> = Vec::new();
    let mut extra: Vec<NodeId> = Vec::new();
    for s in &impact.seeds {
        match files.file_of_node(NodeId(s.id)) {
            Some(f) if !seed_files.contains(&f) => seed_files.push(f),
            Some(_) => {}
            None => extra.push(NodeId(s.id)),
        }
    }
    // P1: rank_eval's base PPR from every node of the changed files,
    // max-pooled per file (`rank_files` under `Ranking::head`).
    let ppr: Vec<f64> = if seed_files.is_empty() && extra.is_empty() {
        vec![0.0; n]
    } else {
        let fr = rank_files(after, cache, &seed_files, &extra, Ranking::head());
        cands
            .files
            .iter()
            .map(|&f| fr.keys.get(f as usize).map_or(0.0, |k| k[0]))
            .collect()
    };
    let excluded: Vec<bool> = cands.files.iter().map(|f| seed_files.contains(f)).collect();

    // The title's BM25 text over past messages, the PR's own left out.
    let query = subject(&x.title).to_string();
    let (text, text_state) = match TextIndex::load(&x.repo, &cands, &x.exclude) {
        Ok(idx) => {
            idx.report(&label, "review", &query);
            (idx.scores(&query), "present")
        }
        Err(why) => {
            why.report(&label, "review");
            (vec![0.0; n], "absent")
        }
    };

    // IDR, read at the base: its docs and trackers, its Tier-4 sections, its
    // candidates; the rules (`[ids]`) are the working tree's.
    let tree = Tree::Rev {
        repo: &x.repo,
        sha: x.base_sha.clone(),
    };
    let base_repo = primary
        .filter(|r| rev.before.repo_roots.contains_key(r))
        .or_else(|| rev.before.repo_roots.keys().next().copied())
        .or(primary);
    let base_files = FileIndex::of(before, &rev.before.repo_labels);
    let base_cands = base_repo
        .map(|r| Candidates::of(&base_files, r))
        .unwrap_or_default();
    let projects = project_roots(before);
    let dirs: Vec<&str> = projects
        .iter()
        .map(|p| p.path.as_str())
        .filter(|p| *p != ".")
        .collect();
    let vocab = IdVocab::for_root(&x.repo);
    let snap = read_history(&x.repo);
    let msgs = snap
        .as_ref()
        .and_then(|s| read_history_messages(&x.repo, s));
    let renames = snap.as_ref().map(rename_pairs).unwrap_or_default();
    let defs = read_defs(&tree, &dirs, &vocab, base_repo.map(|r| (before, RepoId(r))));
    let ns = defs.namespaces(&vocab.local_words);
    let inputs = IdrInputs {
        defs,
        ns,
        paths: PathResolver::new(&base_cands, &renames),
    };
    let side = inputs
        .side(&base_cands, &vocab, &tree)
        .labelled(&label, "review");
    let recent = match (&snap, &msgs) {
        (Some(s), Some(m)) => recent_scopes(
            &recent_subjects(s, m, &base_cands, &x.exclude),
            &inputs.ns,
            &vocab,
        ),
        _ => Vec::new(),
    };
    let routed = route(&x.title, &side, &recent);
    let base_ids = ids_by_file(&routed, &inputs, &vocab, &tree, &base_cands);
    let mut idr = vec![0.0; n];
    let mut ids_of: Vec<Vec<String>> = vec![Vec::new(); n];
    for (i, p) in cands.paths.iter().enumerate() {
        if let Some(b) = base_cands.position(p) {
            idr[i] = routed.idr.get(b).copied().unwrap_or(0.0);
            ids_of[i] = base_ids.get(b).cloned().unwrap_or_default();
        }
    }

    let signals = IntentSignals {
        idr,
        text,
        ppr,
        excluded,
        tie: (0..n as u32).collect(),
    };

    // LX[IDR, the existing order]: rows in id-routed files first, by the
    // file's IDR descending, then their previous place; text moves nothing.
    let row_pos: Vec<Option<usize>> = impact
        .results
        .iter()
        .map(|b| b.file.as_deref().and_then(|p| cands.position(p)))
        .collect();
    let mut reordered = 0usize;
    if signals.idr.iter().any(|&v| v > 0.0) {
        let key: Vec<f64> = row_pos
            .iter()
            .map(|p| p.map_or(0.0, |i| signals.idr[i]))
            .collect();
        let mut ix: Vec<usize> = (0..key.len()).collect();
        ix.sort_by(|&a, &b| key[b].total_cmp(&key[a]).then(a.cmp(&b)));
        reordered = ix.iter().enumerate().filter(|&(p, &i)| p != i).count();
        if reordered > 0 {
            let mut old: Vec<Option<BlastAnswer>> = std::mem::take(&mut impact.results)
                .into_iter()
                .map(Some)
                .collect();
            impact.results = ix.iter().filter_map(|&i| old[i].take()).collect();
        }
    }
    let with_rows: BTreeSet<usize> = row_pos.iter().flatten().copied().collect();

    let mut intent = ReviewIntent {
        title: x.title,
        title_source: x.source,
        fusion: INTENT_FUSION_REVIEW,
        ids: routed.ids.clone(),
        files: Vec::new(),
        text: text_state,
        signals,
    };
    intent.files = intent.listed(&cands.paths, &ids_of, &with_rows, max_files);
    let nz = |v: &[f64]| v.iter().filter(|&&w| w > 0.0).count();
    let line = format!(
        "[review-intent] base={} title_source={} fusion={} ids={} idr_files={} text={text_state} text_files={} candidates={n} reordered={reordered}",
        rev.answer.base,
        intent.title_source,
        intent.fusion,
        intent.ids.len(),
        nz(&intent.signals.idr),
        nz(&intent.signals.text),
    );
    (Some(intent), Some(line))
}

/// The working tree's primary repo: the one repo of a review's build (its
/// first root), else the first candidate file's.
fn primary_repo(rev: &RevDelta, files: &FileIndex) -> Option<u64> {
    rev.after
        .repo_roots
        .keys()
        .next()
        .copied()
        .or_else(|| (!files.is_empty()).then(|| files.repo(0)))
}

/// Per base candidate, the routed ids that reached it, in title order: an id
/// whose defining rows (its lineage class included) name the file, an id a
/// comment of the file cites (one `git grep` at the base over the cited
/// files, the comment's mentions matched against the class's member ids; the
/// display does not repeat route's namespace check), and, when only the
/// conventional-commit scope route ran, every scope for every routed file.
fn ids_by_file(
    routed: &Idr,
    inputs: &IdrInputs,
    vocab: &IdVocab,
    tree: &Tree,
    cands: &Candidates,
) -> Vec<Vec<String>> {
    let n = cands.len();
    let mut out: Vec<Vec<String>> = vec![Vec::new(); n];
    let add = |out: &mut Vec<Vec<String>>, f: usize, id: &str| {
        if let Some(v) = out.get_mut(f)
            && !v.iter().any(|x| x == id)
        {
            v.push(id.to_string());
        }
    };
    let mut members_of: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for r in &routed.ids {
        let Some(rns) = &r.ns else { continue };
        let mut mids: BTreeSet<String> = BTreeSet::new();
        for k in inputs.ns.expand(&Namespaces::key(rns, &r.id)) {
            let Some((mns, mid)) = k.rsplit_once('|') else {
                continue;
            };
            mids.insert(mid.to_string());
            for &ri in inputs
                .ns
                .rows
                .get(&(mns.to_string(), mid.to_string()))
                .into_iter()
                .flatten()
            {
                let Some(row) = inputs.defs.rows.get(ri) else {
                    continue;
                };
                for (f, _, _) in inputs.paths.block(&row.names) {
                    if routed.named.get(f).is_some_and(|&w| w > 0.0) {
                        add(&mut out, f, &r.id);
                    }
                }
            }
        }
        members_of.push((r.id.as_str(), mids));
    }
    let cited: Vec<String> = (0..n)
        .filter(|&f| routed.cited.get(f).is_some_and(|&w| w > 0.0))
        .map(|f| cands.paths[f].clone())
        .collect();
    if !cited.is_empty() {
        let words: Vec<String> = members_of
            .iter()
            .flat_map(|(_, m)| m.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for (p, l) in grep_words_at(tree, &words, &cited).unwrap_or_default() {
            let Some(f) = inputs.paths.map(&p) else {
                continue;
            };
            if !routed.cited.get(f).is_some_and(|&w| w > 0.0) {
                continue;
            }
            let Some(c) = comment_part(&l) else { continue };
            let said: BTreeSet<String> = vocab.mentions(c).into_iter().map(|m| m.id).collect();
            for (id, mids) in &members_of {
                if mids.iter().any(|m| said.contains(m)) {
                    add(&mut out, f, id);
                }
            }
        }
    }
    if members_of.is_empty() {
        for r in routed.ids.iter().filter(|r| r.id.starts_with("scope:")) {
            for f in 0..n {
                if routed.idr.get(f).is_some_and(|&w| w > 0.0) {
                    add(&mut out, f, &r.id);
                }
            }
        }
    }
    out
}

/// `### Intent` (module docs "Markdown"): the title, the routed ids, the
/// ranked files.
fn intent_md(blocks: &mut Vec<String>, r: &Review, max: usize) {
    let Some(i) = &r.intent else { return };
    blocks.push("### Intent".to_string());
    blocks.push(format!("_title ({}): {}_", i.title_source, text(&i.title)));
    let opt = |s: Option<&str>| s.map_or_else(|| DASH.to_string(), text);
    push_table(
        blocks,
        &[
            "id",
            "namespace",
            "tier",
            "route",
            "defined at",
            "state",
            "files",
        ],
        i.ids.iter().map(|d| {
            vec![
                code(&d.id),
                opt(d.ns.as_deref()),
                opt(d.tier),
                text(d.route),
                d.defined_at
                    .as_ref()
                    .map_or_else(|| DASH.to_string(), |(p, l)| at(Some(p), Some(*l))),
                opt(d.state),
                (d.named_files + d.cited_files).to_string(),
            ]
        }),
        i.ids.len(),
        max,
    );
    if i.files.is_empty() {
        blocks.push(none(None));
    }
    push_table(
        blocks,
        &["#", "file", "idr", "text", "ppr", "ids", "in impact"],
        i.files.iter().map(|f| {
            let ids: Vec<String> = f.ids.iter().map(|x| code(x)).collect();
            vec![
                f.rank.to_string(),
                text(&f.file),
                format!("{:.2}", f.idr),
                format!("{:.3}", f.text),
                format!("{:.4}", f.ppr),
                if ids.is_empty() {
                    DASH.to_string()
                } else {
                    ids.join(", ")
                },
                yes_no(f.in_impact),
            ]
        }),
        i.files.len(),
        max,
    );
    if i.text == "absent" {
        blocks.push("_(no history messages: run glia history sync)_".to_string());
    }
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
    let (blocking, advisory): (Vec<&Violation>, Vec<&Violation>) =
        r.new_violations.iter().partition(|v| v.blocking);
    let advisory_note = if advisory.is_empty() {
        String::new()
    } else {
        format!(" ({} advisory)", advisory.len())
    };
    let (model_blocking, model_advisory): (Vec<&ModelBreak>, Vec<&ModelBreak>) =
        r.model_breaks.iter().partition(|m| m.blocking);
    // DT.1p: only with a model break, so every pre-DT.1p headline holds.
    let model_note = match (r.model_breaks.len(), model_advisory.len()) {
        (0, _) => String::new(),
        (m, 0) => format!(" | {m} model break(s)"),
        (m, a) => format!(" | {m} model break(s) ({a} advisory)"),
    };
    let mut blocks: Vec<String> = vec![format!(
        "## glia review vs {}\n**{} new violation(s)**{} | {} resolved | {} changed nodes | impact {} | tests {} ({} seeds untested) | edges +{} -{}{model_note}",
        code(&r.base),
        c.new_violations,
        advisory_note,
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
        && r.resolved_violations.is_empty()
        && r.model_breaks.is_empty();
    if unchanged {
        let note = r.impact.absence.as_ref().map_or_else(
            || format!("no graph change vs {}", r.base),
            |a| a.note.clone(),
        );
        blocks.push(sentence(&note));
        intent_md(&mut blocks, r, max);
        check_errors_md(&mut blocks, r, max);
        unanchored_md(&mut blocks, r);
        return finish(blocks);
    }
    if !blocking.is_empty() {
        blocks.push("### New violations (blocking)".to_string());
        violations_md(&mut blocks, &blocking, max, None);
    }
    if !advisory.is_empty() {
        blocks.push("### New violations (advisory: below the rule's min_tier)".to_string());
        violations_md(&mut blocks, &advisory, max, None);
    }
    if !model_blocking.is_empty() {
        blocks.push("### Model breaks (blocking)".to_string());
        model_breaks_md(&mut blocks, &model_blocking, &r.base, max);
    }
    if !model_advisory.is_empty() {
        blocks.push(
            "### Model breaks (advisory: an inferred client <-> server pairing below the models min tier)"
                .to_string(),
        );
        model_breaks_md(&mut blocks, &model_advisory, &r.base, max);
    }
    check_errors_md(&mut blocks, r, max);
    unanchored_md(&mut blocks, r);

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
    intent_md(&mut blocks, r, max);

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
        let resolved: Vec<&Violation> = r.resolved_violations.iter().collect();
        violations_md(&mut blocks, &resolved, max, Some(&r.base));
    }
    finish(blocks)
}

/// What the markdown's caveat fold (module docs "Markdown", DT.8a's after
/// check) printed and saved. `glia review` reports it as `[review] caveats
/// rules=<r> lines=<l> folded=<f>`.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaveatFold {
    /// Caveat blocks printed: one per (section, rule) with a caveat.
    pub rules: usize,
    /// `⚠` lines those blocks printed.
    pub lines: usize,
    /// The lines one block per violation (the pre-fold layout) would have
    /// printed beyond `lines`.
    pub folded: usize,
}

/// The caveat fold of [`render_markdown`]'s three violation sections (new
/// blocking, new advisory, resolved): the same plan the renderer prints, so
/// the counts are its own.
pub fn caveat_fold(r: &Review) -> CaveatFold {
    let (blocking, advisory): (Vec<&Violation>, Vec<&Violation>) =
        r.new_violations.iter().partition(|v| v.blocking);
    let resolved: Vec<&Violation> = r.resolved_violations.iter().collect();
    let mut fold = CaveatFold::default();
    for section in [&blocking, &advisory, &resolved] {
        for (v, block) in section.iter().zip(caveat_plan(section)) {
            let per_row = if by_subject(v) {
                v.caveats.len().min(MAX_CAVEAT_LINES)
            } else {
                0
            };
            let lines = block.len().min(MAX_CAVEAT_LINES);
            if lines > 0 {
                fold.rules += 1;
            }
            fold.lines += lines;
            fold.folded += per_row;
        }
    }
    fold.folded = fold.folded.saturating_sub(fold.lines);
    fold
}

/// A rule or forbid_path violation: identified by subject, and the kinds
/// that carry coverage caveats.
fn by_subject(v: &Violation) -> bool {
    v.rule_kind == RULE || v.rule_kind == FORBID_PATH
}

/// The caveat block each violation of one section prints (module docs
/// "Markdown"): a rule's first violation there takes every caveat of the
/// rule's violations in the section, deduplicated by (category, language,
/// note) in first-seen order; every other violation takes none.
fn caveat_plan<'v>(section: &[&'v Violation]) -> Vec<Vec<&'v CoverageNote>> {
    let mut first: HashSet<&str> = HashSet::new();
    section
        .iter()
        .map(|v| {
            if !by_subject(v) || !first.insert(v.rule_id.as_str()) {
                return Vec::new();
            }
            let mut seen: HashSet<(&str, &str, &str)> = HashSet::new();
            section
                .iter()
                .filter(|o| o.rule_id == v.rule_id && by_subject(o))
                .flat_map(|o| o.caveats.iter())
                .filter(|c| seen.insert((c.edge_category, c.language, c.note)))
                .collect()
        })
        .collect()
}

/// One section's violations, each with its [`caveat_plan`] block.
fn violations_md(
    blocks: &mut Vec<String>,
    section: &[&Violation],
    max: usize,
    located_at: Option<&str>,
) {
    for (v, caveats) in section.iter().zip(caveat_plan(section)) {
        violation_md(blocks, v, &caveats, max, located_at);
    }
}

/// One model-break section (module docs "Markdown"): one table, a line per
/// breaking change of `rows`, cut at `max` with `_(<shown> of <total>)_`.
fn model_breaks_md(blocks: &mut Vec<String>, rows: &[&ModelBreak], base: &str, max: usize) {
    let lines: Vec<Vec<String>> = rows
        .iter()
        .flat_map(|m| {
            let via = match m.exchanges.split_first() {
                Some((first, [])) => code(&first.key),
                Some((first, rest)) => format!("{} (+{})", code(&first.key), rest.len()),
                None => DASH.to_string(),
            };
            m.changes.iter().filter(|c| c.breaking).map(move |c| {
                vec![
                    text(m.pairing),
                    via.clone(),
                    text(m.tier),
                    text(c.rule),
                    code(&c.field),
                    c.before.as_deref().map_or_else(|| DASH.to_string(), text),
                    c.after.as_deref().map_or_else(|| DASH.to_string(), text),
                    model_site(&m.client, c.client.as_ref(), base),
                    model_site(&m.server, c.server.as_ref(), base),
                ]
            })
        })
        .collect();
    let total = lines.len();
    push_table(
        blocks,
        &[
            "pairing", "via", "tier", "rule", "field", "before", "after", "client", "server",
        ],
        lines,
        total,
        max,
    );
}

/// A model row's side: its qname, then the change's field site there, else
/// the side's own location; ` (at <base>)` on a site read from the rev, a
/// dash for none.
fn model_site(side: &FieldSide, field: Option<&ModelSite>, base: &str) -> String {
    let loc = match field.filter(|s| s.file.is_some()) {
        Some(s) => {
            let mut loc = at(s.file.as_deref(), s.line);
            if s.at_base {
                loc.push_str(&format!(" (at {})", text(base)));
            }
            loc
        }
        None => at(side.file.as_deref(), side.line),
    };
    format!("{} {loc}", code(&side.qname))
}

/// `### Rules that check nothing` (DT.7b / DT.7c): one bullet per
/// [`Review::unanchored`] row, its reason, then its fix; nothing when there
/// are none.
fn unanchored_md(blocks: &mut Vec<String>, r: &Review) {
    if r.unanchored.is_empty() {
        return;
    }
    blocks.push("### Rules that check nothing".to_string());
    blocks.push(
        "_declared, but nothing is evaluated for them; never a blocking violation or a rule error_"
            .to_string(),
    );
    let lines: Vec<String> = r
        .unanchored
        .iter()
        .map(|u| {
            let decl = u.decl.as_deref().unwrap_or("cell API");
            format!(
                "- {} ({}, {}): {}\n  fix: {}",
                code(&u.rule_id),
                text(u.section),
                text(decl),
                text(&u.reason),
                text(&u.fix)
            )
        })
        .collect();
    blocks.push(lines.join("\n"));
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

/// Caveat lines one rule's caveat block lists in markdown.
const MAX_CAVEAT_LINES: usize = 5;

/// One violation: its heading and its evidence rows. A per-edge kind's total
/// is its `count` (the rows beyond `check::MAX_EVIDENCE` included); a
/// no_cycle's rows are one shortest cycle through its `count` members; a rule
/// or forbid_path violation's identity line comes first, then its rows, then
/// `caveats`, its [`caveat_plan`] block (empty for every violation of a rule
/// but the section's first; module docs "Markdown"). `located_at`: the rev a
/// resolved violation's rows are located in.
fn violation_md(
    blocks: &mut Vec<String>,
    v: &Violation,
    caveats: &[&CoverageNote],
    max: usize,
    located_at: Option<&str>,
) {
    blocks.push(match v.decl.as_deref() {
        Some(decl) => format!(
            "#### {} ({}, {})",
            text(&v.rule_id),
            text(v.rule_kind),
            text(decl)
        ),
        None => format!("#### {} ({})", text(&v.rule_id), text(v.rule_kind)),
    });
    let subject_kind = by_subject(v);
    if subject_kind {
        let subject: Vec<String> = v.subject.iter().map(|s| code(s)).collect();
        let mut line = format!(
            "_subject: {} - tier {}",
            if subject.is_empty() {
                DASH.to_string()
            } else {
                subject.join(", ")
            },
            text(v.tier)
        );
        if v.file.is_some() {
            line.push_str(&format!(" at {}", at(v.file.as_deref(), v.line)));
            if let Some(rev) = located_at {
                line.push_str(&format!(" (at {})", text(rev)));
            }
        }
        line.push('_');
        blocks.push(line);
    }
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
    if subject_kind && !caveats.is_empty() {
        let lines: Vec<String> = caveats
            .iter()
            .take(MAX_CAVEAT_LINES)
            .map(|c| {
                format!(
                    "⚠ {} ({}): {}",
                    text(c.edge_category),
                    text(c.language),
                    text(c.note)
                )
            })
            .collect();
        let shown = lines.len();
        blocks.push(lines.join("\n"));
        if shown < caveats.len() {
            blocks.push(format!("_({shown} of {} caveats)_", caveats.len()));
        }
    }
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

/// Rule id -> the `min_tier` its violations block at (`check::min_tier_of`)
/// over the working tree's rules, which both sides are checked against; the
/// first rule of an id wins. A `reflexion:` divergence has no entry: it reads
/// [`DEFAULT_MIN_TIER`], as check gates it.
fn min_tiers(rules: &[(NodeId, ConstraintRule)]) -> HashMap<&str, &'static str> {
    let mut out: HashMap<&str, &'static str> = HashMap::new();
    for (_, rule) in rules {
        out.entry(rule.id.as_str())
            .or_insert_with(|| check::min_tier_of(rule));
    }
    out
}

/// The violations of `this` whose identity `other` lacks, each reduced to its
/// unmatched rows (module docs). `skip`: rule ids never compared (the working
/// tree's rule errors). `gates`: [`min_tiers`], which a reduced violation's
/// `blocking` is recomputed under. `verb` names the result in a note.
fn compare(
    this: Side<'_>,
    other: Side<'_>,
    skip: &HashSet<&str>,
    gates: &HashMap<&str, &'static str>,
    verb: &str,
    notes: &mut Vec<(String, String)>,
) -> Vec<Violation> {
    let mut out: Vec<Violation> = Vec::new();
    for v in &this.report.violations {
        if skip.contains(v.rule_id.as_str()) {
            continue;
        }
        // A rule row and a transitive hit are their subject (module docs):
        // check lists both uncapped, so the other side's own list is whole.
        if v.rule_kind == RULE || v.rule_kind == FORBID_PATH {
            let known = other
                .violations_of(&v.rule_id)
                .any(|o| o.rule_kind == v.rule_kind && o.subject == v.subject);
            if !known {
                out.push(v.clone());
            }
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
                out.push(reduced(v, fresh.len(), fresh, gates));
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
        out.push(reduced(v, fresh.len(), fresh, gates));
    }
    out
}

/// `v` with its evidence narrowed to `rows` (sorted tier-first, as check
/// sorts them) and capped like check's; `count` the full number, the tier the
/// strongest row's, and `blocking` recomputed from that tier under its rule's
/// `min_tier` (`gates`, [`DEFAULT_MIN_TIER`] for a divergence): the full
/// violation's gate read its own strongest row, which the change may not
/// have added.
fn reduced(
    v: &Violation,
    count: usize,
    mut rows: Vec<ViolationEdge>,
    gates: &HashMap<&str, &'static str>,
) -> Violation {
    rows.truncate(check::MAX_EVIDENCE);
    let mut out = v.clone();
    out.tier = rows.first().map_or(v.tier, |r| r.tier);
    let min_tier = gates
        .get(v.rule_id.as_str())
        .copied()
        .unwrap_or(DEFAULT_MIN_TIER);
    out.blocking = check::is_blocking(out.tier, min_tier);
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
                // A transitive rule's violations are forbid_path hits, which
                // `compare` classes by subject from check's own uncapped
                // list (module docs "Why an index"): no rows here.
                ConstraintKind::ForbidEdge {
                    from,
                    to,
                    transitive: false,
                    ..
                } => {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// EB.7 (1c): the three measured fusions on a fixed 5-file set equal
    /// rank_eval `r3_bank_seed`'s arithmetic (file 4 is the changed file).
    /// minmax(text) over files 0..3 = [1, 0, 1/3, 0]; minmax(ppr) over
    /// 0.05..0.4 = [3/7, 1/7, 1, 0]; H9s50 = [0.714, 0.071, 0.667, 0].
    #[test]
    fn fusions_are_the_harness_arithmetic() {
        let s = IntentSignals {
            idr: vec![0.0, 1.0, 0.0, 0.5, 0.0],
            text: vec![3.0, 0.0, 1.0, 0.0, 9.0],
            ppr: vec![0.2, 0.1, 0.4, 0.05, 0.9],
            excluded: vec![false, false, false, false, true],
            tie: vec![0, 1, 2, 3, 4],
        };
        let [(a, lx_p1), (b, h9), (c, lx_h9)] = s.fusions();
        assert_eq!((a, b, c), ("LX[IDR,P1]", "H9s50", "LX[IDR,H9s50]"));
        assert_eq!(lx_p1, [1, 3, 2, 0]);
        assert_eq!(h9, [0, 2, 1, 3]);
        assert_eq!(lx_h9, [1, 3, 0, 2]);
        // ties go by candidate position
        let flat = IntentSignals {
            idr: vec![0.0; 3],
            text: vec![0.0; 3],
            ppr: vec![0.0; 3],
            excluded: vec![false; 3],
            tie: vec![0, 1, 2],
        };
        assert_eq!(
            flat.fusions().map(|(_, o)| o),
            [vec![0, 1, 2], vec![0, 1, 2], vec![0, 1, 2]]
        );
    }
}
