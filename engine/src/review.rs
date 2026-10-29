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
//!   are cut to `max_impact` (`counts.impact` is the uncut length), its seeds
//!   to `max_seeds` (`counts.seeds` uncut), and `impact.truncated` counts
//!   both cuts (FH.8; the JSON of `changed` and `impact` is compact, as
//!   `diff_impact::DiffImpact` documents: FH.8b, under any cap below
//!   `usize::MAX` a `changed` row that repeats a shown seed is `{id,
//!   change}` and a seed's `also_in` past three members is folded; the CLI's
//!   `--full` / pyo3's `full=True` lift every cap). FH.8c, the overview
//!   answer: under a cap the impact carries `areas` (`answers::Area`, every
//!   row by sub-project or directory) and slim rows, `max_impact` defaults to
//!   `answers::DEFAULT_OVERVIEW_ROWS` (25), `offset` skips the first impact
//!   rows, a cut names its next page (`impact.truncated.next`), `changed` is
//!   cut to `max_seeds` (`counts.nodes_changed` uncut), and the markdown adds
//!   `### Impact areas` after `### Impact` when the impact rows were cut;
//! - `tests`: `tests_for::tests_for_delta` (LE.3b / CC.1) with
//!   [`ReviewArgs::tests`], cut to `max_tests` (`counts.tests` is the uncut
//!   length; `test_files` is recomputed over the kept rows, so it stays "the
//!   distinct files of `tests`"; FH.8d: 25 by default, it was 50);
//! - `edges`: every added, removed and reconfidenced edge of the delta, the
//!   first edge with that key on its side (the working tree for added and
//!   reconfidenced, the base for removed), tiered by `why::tier_of` exactly as
//!   `glia why` and `glia check` tier it, and located like `glia delta`'s rows
//!   (the EVIDENCE site, 1-based). Sorted by (tier: fact, derived, heuristic;
//!   change: added, removed, reconfidenced; category; from; to), so a
//!   reviewer reads the observed facts first;
//! - the rules: `declared_constraints` of the WORKING TREE (the rules as the
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
//! new. So `RuleIndex` evaluates forbid_edge and no_cycle to identity level
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
//! `intent.files` are the candidates with any signal in the intent fusion's
//! order, at most `max_impact`. The fusion (EC.11) is set `SELt2`'s pick (EC.12) for
//! the shape of the changed files (`Shape::from_seed_files`) from the repo's
//! `.glia/calibration-snapshot/` (`glia history calibrate`), ranked through
//! the same bank the replay ranks (`calibrate::fusions::seeded_keys`, the
//! extra signals PT*, C0, W5 / M computed only when the pick reads them),
//! else `LX[IDR,P1]` ([`INTENT_FUSION_REVIEW`], byte-identical to EB.7) with
//! the reason; [`Review::selection_intent`] says which. The impact rows take
//! LX[IDR, their existing order]: when some file has IDR > 0, rows in such
//! files move first by the file's IDR descending, every other row keeps its
//! relative order after them; text never moves a row, and with no routed id
//! the order is byte-identical to the review without a title; the
//! calibrated pick orders `intent.files` only. A fixed H9s50 order loses on
//! a gate snapshot, so text fusions ship only as a repo's calibrated pick.
//!
//! # Name breaks (FH.7b)
//!
//! [`Review::name_breaks`] is `contract_breaks::name_breaks` over the same
//! working tree and rev (FH.7a, the style-agnostic drift fallback): every
//! client site still using a name the change removed or renamed on the
//! server side of a paired exchange. Always ADVISORY here: review's
//! [`ReviewArgs::models_min_tier`] takes `fact` | `derived` only, and a name
//! row blocks only under `heuristic` (`glia contract-breaks
//! --models-min-tier heuristic` is the gate that can fail on one), so
//! [`Review::blocking`] never moves for a name row. [`review_vs_rev`] reads
//! its repo path; [`review`] over a caller's delta reads the delta's one repo
//! root (`contract_breaks::name_breaks_of`). fired_on once per review:
//! FH.7a's `[name-breaks] base=<rev> ..` line
//! (printed by the name pass, before `[review] base=`) and, after the
//! `[review] base=` line and the intent and conventions lines,
//! `[name-breaks] answer rows=<k> blocking=0 advisory=<k> min_tier=<t>
//! folded_schema_rows=0->0 surface=review`
//! (review lists no schema row, so it folds none; `rows` is the uncut
//! count).
//!
//! FH.8d, the overview: `name_breaks` keeps [`ReviewArgs::max_names`] rows
//! ([`DEFAULT_MAX_NAMES`], 25; `usize::MAX` under the CLI's `--full` /
//! pyo3's `full=True`), `counts.name_breaks` stays the uncut total and
//! `name_breaks_truncated` counts the rest. A cut keeps the folded rows
//! first, then by tier, file and line; an uncut list keeps `name_breaks`'
//! order. A folded row (FH.7d) renders `<N> sites in <M> files (e.g. a, b,
//! c)` in place of its `file:line` ([`name_break_sites`]), here and in the
//! CLI's contract-breaks `## names` table. The JSON goes through
//! [`review_json`], whose `[answer-size] surface=review ..` line adds
//! `names=<n> names_shown=<k> tests_shown=<t>` before `bytes=`.
//!
//! # Conventions (FB.12b)
//!
//! A change inside a scope is checked against its conventions: [`Review::conventions`]
//! lists every scope-level convention (an overlay `[[convention]]` or a
//! cell-API CONV entry of kind `convention`, FB.12a) of the working tree that
//! governs a changed row's file (a file under one of its `paths`, `.` every
//! file), with the changed files it governs, longest matching path first,
//! then `(source, id)`. Tier `declared` for an overlay one, `heuristic` for
//! an API one. A convention is not a rule: it never blocks and never moves an
//! impact row. fired_on, after the `[review] base=` line, only when one is
//! listed: `[review] conventions=<n>` - grep `^\[review\] conventions=`.
//!
//! # Blind (FH.9)
//!
//! `blind` lists the mechanisms the review read (its impact's, before the row
//! cap, plus `http` / `graphql`, whose model exchanges it judges) that
//! `health::boundary_health` rates BLIND or DEGRADED on the working tree's
//! graph; the markdown prints `blind: http (clients paired 7% of 14)` as its
//! own block under the headline, and `[blind] surface=review ..` fires once
//! per review.
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
//! from the rev, a dash for none), `### Name breaks (advisory)` (FH.7b, when
//! a name row exists: `| name | tier | at | position | client | server |`,
//! the name `` `old` -> `new` `` for a rename, `at` the client site's
//! `file:line` in the working tree; the headline ends ` | <N> name break(s)`), `### Check errors` (when any: the working
//! tree's rule errors and the rules only partly compared), `### Rules that
//! check nothing` (when any: one bullet per `unanchored` row, its reason,
//! then its `fix:`), `### Tests to run`, `### Impact`, `### Intent` (EB.7,
//! with an intent: `_title (<source>): <title>_`, then `_ranked by <fusion>
//! (calibrated on <t> commits, shape <k>)_` or `_ranked by <fusion>
//! (uncalibrated: <reason>)_` (EC.11), the routed-ids table `| id
//! | namespace | tier | route | defined at | state | files |` when an id
//! routed, the files table `| # | file | idr | text | ppr | ids | in impact
//! |`, and `_(no history messages: run glia history sync)_` when the text is
//! absent; after the sentence of a review with no graph change),
//! `### Conventions` (FB.12b, when one governs a changed file: `| convention
//! | scope | files changed | source |`, the source `<source>/<id> (<tier>)`
//! then ` at <decl>` for an overlay one), `### Edge changes`
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
//! fusion=<pick> ids=<n> idr_files=<i> text=<present|absent>
//! text_files=<t> candidates=<c> reordered=<r>` (`r` the impact rows whose
//! position moved), after the intent's own `[rank-files] .. ranking=P1`,
//! `[intent] text`, `[ids] vocab`, `[id-defs]`, `[id-ns]`, `[id-paths]` and
//! `[idr]` lines (`surface=review`) and EC.11's `[select]
//! surface=review_intent set=SELt2 shape=<k> pick=<name>
//! source=calibrated|fallback reason=<r|-> test_commits=<n>` (with `[work]
//! head ..` when the pick reads W5 / M), which a title prints and `none`
//! does not.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};

use glia_activation::algo::fuse;
use glia_activation::profile::Tier;
use glia_code_domain::snapshots::{
    HistoryMessage, HistorySnapshot, read_history, read_history_messages, redact_untrusted,
};
use glia_core::RepoId;

use glia_activation::algo::cycles::strongly_connected;
use glia_activation::algo::delta::EdgeKey;
use glia_activation::algo::{Adjacency, CategorySet, GraphSource};
use glia_code_domain::evidence::Evidence;
use glia_code_domain::external_inputs::{ConstraintKind, ConstraintRule};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Edge, EdgeCategoryId, NodeId};
use glia_graph::MergedGraph;
use glia_projection_text::ladder::note_tier;

use crate::absence::BlindCaveat;
use crate::answers::{
    AnswerSize, BlastAnswer, BlastOptions, BlastRadius, DEFAULT_ANSWER_SEEDS,
    DEFAULT_OVERVIEW_ROWS, Locator, SizedAnswer, in_scope, project_roots,
};
use crate::calibrate::fusions::{
    AnswerInputs, Signals, answer_signals, needs, seeded_keys, seeded_order,
};
use crate::calibrate::record::{Selection, Shape, explicit, log_selection, select};
use crate::calibrate::sets::{REVIEW_CANDIDATES, REVIEW_FALLBACK, REVIEW_SET};
use crate::check::{
    self, CheckOptions, CheckReport, DEFAULT_MIN_TIER, EMPTY_SCOPE, FORBID_EDGE, FORBID_PATH,
    NO_CYCLE, RULE, RuleMdOptions, Unanchored, Violation, ViolationEdge, check_rules_gated,
    default_forbid_categories,
};
use crate::contract_breaks::{
    MODEL_MIN_TIERS, ModelBreak, ModelBreakArgs, ModelSite, NameBreak, check_models_min_tier,
    model_breaks_of, name_breaks_of,
};
use crate::contract_fields::FieldSide;
use crate::cycles::module_import_graph;
use crate::delta::{RevDelta, graph_delta_vs_rev};
use crate::diff_impact::{ChangedNode, diff_impact_from_delta_on, fold_seed_rows};
use crate::external::declared::{conventions, conventions_for, declared_constraints, node_repos};
use crate::git_rev;
use crate::health;
use crate::intent::defs::{Tree, read_defs};
use crate::intent::idr::{
    Idr, IdrInputs, RoutedId, ids_by_file, recent_scopes, recent_subjects, route,
};
use crate::intent::ids::IdVocab;
use crate::intent::paths::{PathResolver, rename_pairs};
use crate::intent::text::{Candidates, TextAbsent, TextIndex, exclude_range, subject};
use crate::rank::{FileIndex, RankCache, Ranking, rank_files};
use crate::tests_for::{TestsFor, TestsForArgs, tests_for_delta};
use crate::why::tier_of;

/// [`ReviewArgs::default`]'s `max_impact`: FH.8c's overview,
/// [`DEFAULT_OVERVIEW_ROWS`] (it was 50).
pub const DEFAULT_MAX_IMPACT: usize = DEFAULT_OVERVIEW_ROWS;
/// [`ReviewArgs::default`]'s `max_tests`: FH.8d, the overview's
/// [`DEFAULT_OVERVIEW_ROWS`] (it was 50).
pub const DEFAULT_MAX_TESTS: usize = DEFAULT_OVERVIEW_ROWS;
/// FH.8d: [`ReviewArgs::default`]'s `max_names`, the name rows a review
/// keeps (module docs "Name breaks").
pub const DEFAULT_MAX_NAMES: usize = DEFAULT_OVERVIEW_ROWS;

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
    /// FH.8d: name rows kept ([`DEFAULT_MAX_NAMES`]); `counts.name_breaks`
    /// is uncut and `name_breaks_truncated` counts the rows past the cut,
    /// which keeps folded rows first (module docs "Name breaks").
    /// `usize::MAX` (the CLI's `--full`) keeps every row in
    /// `contract_breaks::name_breaks`' order.
    pub max_names: usize,
    /// FH.8: impact seeds kept ([`DEFAULT_ANSWER_SEEDS`], FH.8b; it was
    /// `DEFAULT_ANSWER_ROWS`); `counts.seeds` is uncut. What this and
    /// `max_impact` cut is `impact.truncated` (`{results, seeds}`, absent
    /// when nothing was cut); `usize::MAX` for both (the CLI's `--full`)
    /// keeps every row. FH.8c: also the `changed` rows kept
    /// (`counts.nodes_changed` is uncut).
    pub max_seeds: usize,
    /// FH.8c: impact rows skipped before the `max_impact` cut, after the
    /// ranking and the intent's reorder (`impact.truncated.offset`); 0 (the
    /// default) skips none.
    pub offset: usize,
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
            max_names: DEFAULT_MAX_NAMES,
            max_seeds: DEFAULT_ANSWER_SEEDS,
            offset: 0,
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
    /// FH.7b: the name rows; FH.8d: uncut by `max_names`. Not serialised
    /// when 0.
    #[serde(skip_serializing_if = "is_zero")]
    pub name_breaks: usize,
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
    /// The ranked impact of the changed nodes, `results` cut to `max_impact`
    /// and `seeds` to `max_seeds`, `truncated` counting both cuts (FH.8).
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
    /// FH.7b: the client sites still using a name the change removed or
    /// renamed on the server side of a paired exchange (module docs "Name
    /// breaks"), `name_breaks`' order. Advisory: never in `blocking`. Not
    /// serialised when empty. FH.8d: cut to `max_names`, folded rows first
    /// when the cut drops a row.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub name_breaks: Vec<NameBreak>,
    /// FH.8d: the name rows the `max_names` cut left out
    /// (`counts.name_breaks` is the uncut total). Not serialised when 0.
    #[serde(skip_serializing_if = "is_zero")]
    pub name_breaks_truncated: usize,
    /// `(rule id, message)`: the working tree's rule errors, plus a rule a
    /// side lists too few rows of to classify (module docs).
    pub check_errors: Vec<(String, String)>,
    /// The working tree's rules that check nothing (`check::EMPTY_SCOPE`
    /// rows, reason and fix; module docs "New and resolved violations"):
    /// never blocking, never a rule error. Not serialised when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unanchored: Vec<Unanchored>,
    /// FB.12b: the conventions governing the changed files (module docs
    /// "Conventions"): never blocking, a convention is not a rule. Not
    /// serialised when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conventions: Vec<ReviewConvention>,
    /// A new BLOCKING violation exists (one at or above its rule's
    /// `min_tier`, never a heuristic one: `Violation::blocking`, module docs
    /// "New and resolved violations"), or a blocking model row
    /// (`ModelBreak::blocking`, module docs "Model breaks"). The CI gate.
    pub blocking: bool,
    /// 0.5.3 (EB.7): what the PR says it is about (module docs "Intent"):
    /// its title, the ids it routes, and the files ranked by the intent
    /// fusion. Only from [`review_vs_rev`] with a title or a commit in
    /// `<base>..HEAD`; not serialised when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<ReviewIntent>,
    /// 0.5.3 (EC.11): the intent's ranking choice, set `SELt2` (EC.12) at the shape of
    /// the changed files: the repo's calibrated pick, else the fallback
    /// `LX[IDR,P1]` with the reason. Present exactly when `intent` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_intent: Option<Selection>,
    /// FH.9: the mechanisms the review read (its impact's,
    /// `health::blast_mechanisms` before the row cap, plus `http` and
    /// `graphql`, the model exchanges it judges) that health rates BLIND or
    /// DEGRADED on the working tree's graph; the markdown prints them as one
    /// `blind: ..` line under the headline. Empty, and not serialized,
    /// otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blind: Vec<BlindCaveat>,
}

impl SizedAnswer for Review {
    /// FH.8b: the impact radius's seeds (`[answer-size] surface=review ..`).
    fn answer_size(&self) -> AnswerSize {
        self.impact.size()
    }
}

/// FB.12b: a convention governing at least one changed file (module docs
/// "Conventions").
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ReviewConvention {
    pub id: String,
    /// `overlay` or `api`.
    pub source: String,
    /// `declared` (overlay / adr) or `heuristic` (a cell-API writer).
    pub tier: &'static str,
    pub scope: String,
    pub text: String,
    /// `.glia/overlay.toml:<line>` of an overlay convention.
    pub decl: Option<String>,
    /// The writer's `by`, when it named one.
    pub by: Option<String>,
    /// The changed files it governs, sorted.
    pub files: Vec<String>,
}

/// FB.12b: the working tree's conventions governing the changed rows' files
/// (a removed row's file too), one per `(repo, source, id)`, by the longest
/// path that matched one of them, longest first, then `(source, id, repo)`.
/// A changed row's repo is its node's in the working tree, else the base's.
fn review_conventions(
    before: &MergedGraph,
    after: &MergedGraph,
    changed: &[ChangedNode],
) -> Vec<ReviewConvention> {
    let all = conventions(after);
    if all.is_empty() {
        return Vec::new();
    }
    let (now, then) = (node_repos(after), node_repos(before));
    let files: BTreeSet<(u64, &str)> = changed
        .iter()
        .filter_map(|n| {
            let id = NodeId(n.id);
            let repo = now.get(&id).or_else(|| then.get(&id))?;
            Some((*repo, n.file.as_deref()?))
        })
        .collect();
    type Row<'c> = (
        usize,
        BTreeSet<String>,
        &'c crate::external::declared::Convention,
    );
    let mut by: BTreeMap<(u64, &str, &str), Row<'_>> = BTreeMap::new();
    for (repo, file) in &files {
        let normal = file.trim_start_matches("./").trim_start_matches('/');
        for c in conventions_for(&all, *repo, file) {
            let len = c
                .paths
                .iter()
                .filter(|p| in_scope(normal, p))
                .map(|p| if p == "." { 0 } else { p.len() })
                .max()
                .unwrap_or(0);
            let e = by
                .entry((c.repo, c.source.as_str(), c.id.as_str()))
                .or_insert_with(|| (len, BTreeSet::new(), c));
            e.0 = e.0.max(len);
            e.1.insert(normal.to_string());
        }
    }
    let mut rows: Vec<Row<'_>> = by.into_values().collect();
    rows.sort_by(|(la, _, a), (lb, _, b)| {
        lb.cmp(la)
            .then_with(|| (&a.source, &a.id, a.repo).cmp(&(&b.source, &b.id, b.repo)))
    });
    rows.into_iter()
        .map(|(_, files, c)| ReviewConvention {
            id: c.id.clone(),
            source: c.source.clone(),
            tier: note_tier(&c.source),
            scope: c.scope.clone(),
            text: c.text.clone(),
            decl: c.decl.clone(),
            by: c.by.clone(),
            files: files.into_iter().collect(),
        })
        .collect()
}

/// The review of the working tree's change against git rev `base` (module
/// docs): one `graph_delta_vs_rev`, then [`review`]. `Err` on an unknown
/// [`ReviewArgs::models_min_tier`] (``unknown models min tier `<x>`: expected
/// one of fact, derived``, before anything is built), on the delta's errors
/// (not a directory, not a git work tree, an unknown rev, a failed build) and
/// on `tests_for_delta`'s (more than `tests_for::MAX_SEEDS` seeds). Like the
/// delta, it saves the working tree's parse-cache sidecar, never a layout.
pub fn review_vs_rev(repo_path: &str, base: &str, args: &ReviewArgs) -> Result<Review, String> {
    check_models_min_tier(args.models_min_tier)?;
    let rev = graph_delta_vs_rev(repo_path, base)?;
    let ask = intent_request(Path::new(repo_path), base, args.title.as_deref());
    review_with_intent(&rev, args, ask, Some(repo_path))
}

/// The review of an already computed rev delta (module docs). Nothing is
/// built. `Err` beyond `tests_for::MAX_SEEDS` seeds only. No intent: a
/// delta carries no repo path to read a title or a base tree from
/// ([`ReviewArgs::title`] is read by [`review_vs_rev`] only).
pub fn review(rev: &RevDelta, args: &ReviewArgs) -> Result<Review, String> {
    review_with_intent(rev, args, IntentAsk::Off, None)
}

/// [`review`] with the intent section [`review_vs_rev`] asked for, and the
/// working tree the name breaks read (`None`: the delta's one repo root).
fn review_with_intent(
    rev: &RevDelta,
    args: &ReviewArgs,
    ask: IntentAsk,
    worktree: Option<&str>,
) -> Result<Review, String> {
    let (before, after) = (&rev.before.merged, &rev.after.merged);

    // EB.7: one RankCache for the impact walk and the intent's P1, built as
    // `diff_impact_from_delta_on` builds its transient one (so the impact
    // rows are unchanged by it).
    // `diff_impact_from_delta_on` labels an unlabelled collapse with the
    // working tree's repo labels (DD.10, FD.28).
    let cached;
    let blast = if args.blast.rank_cache.is_none() {
        let mut with = args.blast.clone();
        with.rank_cache = Some(Arc::new(RankCache::new(
            "answer",
            &rev.after.repo_roots,
            &rev.after.repo_labels,
        )));
        cached = with;
        &cached
    } else {
        &args.blast
    };
    let mut di = diff_impact_from_delta_on(rev, blast, "review");
    let mut impact = di.impact;
    let (intent, intent_line) = match blast.rank_cache.as_deref() {
        Some(cache) => review_intent(rev, cache, &mut impact, args.max_impact, ask),
        None => (None, None),
    };
    // FH.9: the impact's mechanisms plus the model exchanges', in health's
    // order.
    let impact_read = health::blast_mechanisms(&impact);
    let read: Vec<&'static str> = health::all_mechanisms()
        .into_iter()
        .filter(|m| impact_read.contains(m) || MODEL_MECHANISMS.contains(m))
        .collect();
    let blind = health::blind_caveats(after, "review", &read);
    let impact_total = impact.rows_total();
    let seeds_total = impact.seeds.len() + impact.truncated.as_ref().map_or(0, |t| t.seeds);
    // FH.8c: a capped review's impact maps every row by area before the cut.
    if args.max_impact != usize::MAX || args.max_seeds != usize::MAX {
        impact.map_areas(after);
    }
    impact.page(args.offset, args.max_impact, args.max_seeds);
    // FH.8b: `changed` folds against the seeds the review's cut kept.
    fold_seed_rows(&mut di.changed, &impact);

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
    // FH.7b: review's tiers are fact | derived, so no name row blocks.
    let mut name_breaks =
        name_breaks_of(&rev.answer.base, worktree, &rev.after, args.models_min_tier);
    let names_total = name_breaks.len();
    let names_blocking = name_breaks.iter().filter(|n| n.blocking).count();
    // FH.8d: the overview keeps `max_names` rows, folded rows first.
    let name_breaks_truncated = cap_name_breaks(&mut name_breaks, args.max_names);

    let mut edges_by_tier: BTreeMap<&'static str, usize> = BTreeMap::new();
    for e in &edges {
        *edges_by_tier.entry(e.tier).or_insert(0) += 1;
    }
    let conventions = review_conventions(before, after, &di.changed);
    let nodes_changed = di.changed.len();
    // FH.8c: `changed` cut with the seeds (`counts.nodes_changed` uncut).
    di.changed.truncate(args.max_seeds);
    let counts = ReviewCounts {
        nodes_changed,
        seeds: seeds_total,
        edges_added: edges.iter().filter(|e| e.change == ADDED).count(),
        edges_removed: edges.iter().filter(|e| e.change == REMOVED).count(),
        impact: impact_total,
        tests: tests_total,
        untested_seeds: tests.untested.len(),
        new_violations: new_violations.len(),
        resolved_violations: resolved_violations.len(),
        model_breaks: model_breaks.len(),
        name_breaks: names_total,
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
        name_breaks,
        name_breaks_truncated,
        check_errors,
        unanchored,
        conventions,
        selection_intent: intent.as_ref().map(|i| i.selection.clone()),
        intent,
        blind,
    };
    marker(&answer);
    if let Some(line) = intent_line {
        eprintln!("{line}");
    }
    if !answer.conventions.is_empty() {
        eprintln!("[review] conventions={}", answer.conventions.len());
    }
    eprintln!(
        "[name-breaks] answer rows={names_total} blocking={names_blocking} advisory={} min_tier={} folded_schema_rows=0->0 surface=review",
        names_total - names_blocking,
        args.models_min_tier,
    );
    Ok(answer)
}

/// The mechanisms `model_breaks` judges exchanges of (FH.9).
const MODEL_MECHANISMS: [&str; 2] = ["http", "graphql"];

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
        tier(Tier::Fact.as_str()),
        tier(Tier::Derived.as_str()),
        tier(Tier::Heuristic.as_str()),
        c.new_violations,
        c.resolved_violations,
        r.blocking,
    );
}

// ============================================================================
// intent (module docs "Intent", EB.7)
// ============================================================================

/// The fusion that orders `intent.files` (EB.7) without a calibration: the
/// typed links first (IDR), then rank_eval's P1 (module docs "Intent").
/// The calibrated selection (EC.11) keeps it as its uncalibrated fallback
/// (`calibrate::REVIEW_FALLBACK`).
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
    /// The fusion `files` follow: set SELt2's pick for the repo (EC.11),
    /// [`INTENT_FUSION_REVIEW`] when uncalibrated.
    pub fusion: String,
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
    /// The ranking choice (serialised as `Review.selection_intent`).
    #[serde(skip)]
    pub(crate) selection: Selection,
}

impl ReviewIntent {
    /// [`ReviewIntent::files`] from the signals: the candidates with any
    /// signal in `order` ([`ReviewIntent::fusion`]'s), at most `max`.
    fn listed(
        &self,
        order: Vec<usize>,
        paths: &[String],
        ids_of: &[Vec<String>],
        with_rows: &BTreeSet<usize>,
        max: usize,
    ) -> Vec<IntentFile> {
        let s = &self.signals;
        listed_order(s, order)
            .into_iter()
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

/// The candidates of `order` the intent table lists: those with a positive
/// signal (IDR, text, P1, or the bank's PT* / C0 / W5 / M), order kept.
fn listed_order(s: &IntentSignals, order: Vec<usize>) -> Vec<usize> {
    let pos = |v: &[f64], i: usize| v.get(i).is_some_and(|&x| x > 0.0);
    order
        .into_iter()
        .filter(|&i| {
            s.idr[i] > 0.0
                || s.text[i] > 0.0
                || s.ppr[i] > 0.0
                || pos(&s.bank.pt, i)
                || pos(&s.bank.c0, i)
                || pos(&s.bank.w5, i)
                || pos(&s.bank.m, i)
        })
        .collect()
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
    /// The calibrated fusion's other signals (PT*, C0, W5, M), filled only
    /// when the pick reads them (EC.11); empty otherwise.
    pub(crate) bank: Signals,
}

impl IntentSignals {
    /// The candidates in the order of the seeded fusion `name`
    /// (`calibrate::fusions::seeded_keys`, rank_eval `r3_bank_seed`'s
    /// arithmetic, raw IDR first in an LX; ties by `tie`), the changed files
    /// left out; None for a name the bank does not rank. LX[IDR,P1] is EB.7's
    /// shipped order; H9s50 = `blend(0.5, minmax(text), minmax(ppr))`.
    pub(crate) fn order(&self, name: &str) -> Option<Vec<usize>> {
        let mut s = self.bank.clone();
        s.p1 = self.ppr.clone();
        s.t1s = self.text.clone();
        s.idr = Some(self.idr.clone());
        let keys = seeded_keys(name, &s, &self.excluded)?;
        Some(seeded_order(&keys, &self.tie, &self.excluded))
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

/// The author time of the newest non-merge commit of `<base>..HEAD` (`git
/// log -1 --no-merges --format=%at`), the "now" W5 / M read (EC.11); None
/// when the range has no commit or git fails.
fn newest_author_time(repo: &Path, base_sha: &str) -> Option<i64> {
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
            "--format=%at",
            &range,
            "--",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
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
    let base_repo = primary
        .filter(|r| rev.before.repo_roots.contains_key(r))
        .or_else(|| rev.before.repo_roots.keys().next().copied())
        .or(primary);
    let history = IntentHistory::from_root(&x.repo, &x.exclude);
    let ranked = intent_rank(&IntentAt {
        after,
        before,
        base_files: Arc::new(FileIndex::of(before, &rev.before.repo_labels)),
        primary,
        base_repo,
        label,
        root: &x.repo,
        base_sha: &x.base_sha,
        title: &x.title,
        cache,
        seed_files: &seed_files,
        extra: &extra,
        history: history.as_ref(),
        exclude: &x.exclude,
        pick: None,
    });
    let IntentRanked {
        cands,
        routed_ids,
        ids_of,
        signals,
        selection,
        order,
        text_state,
    } = ranked;
    let n = cands.len();

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
        fusion: selection.pick.clone(),
        ids: routed_ids,
        files: Vec::new(),
        text: text_state,
        signals,
        selection,
    };
    intent.files = intent.listed(order, &cands.paths, &ids_of, &with_rows, max_files);
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

/// The history an intent ranking reads in place of the repo root's
/// `.glia/history-snapshot/` (EG.7): the snapshot (newest first, as `glia
/// history sync` writes it), its messages sidecar (row i belonging to commit
/// i; None reads as `messages=absent`), the commits no signal may read
/// (`exclude`: review's own PR commits; the rank gate's test commit and every
/// newer one), and four optional overrides a caller with no working tree at
/// the answered revision sets: `now`, the author time W5 / M count back from
/// (review: else the newest commit of `<base>..HEAD`; find: else the newest
/// kept snapshot commit's); `vocab`, the `[ids]` vocabulary (else the root's
/// `.glia/overlay.toml`); `git`, the repository `Tree::Rev` and `git grep`
/// read (else the root); `rev`, the revision `find --files` routes ids at
/// (else the working tree; review always reads its base).
///
/// For the rank gate's harness (`engine/examples/rank_eval.rs`); unstable.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct IntentHistory {
    pub snapshot: HistorySnapshot,
    pub messages: Option<Vec<HistoryMessage>>,
    pub exclude: BTreeSet<String>,
    pub now: Option<i64>,
    pub vocab: Option<IdVocab>,
    pub git: Option<PathBuf>,
    pub rev: Option<String>,
    /// Routed ids per (surface, revision, title), shared by every call over
    /// this history: a rank gate test commit's queries route one subject at
    /// one parent.
    memo: Arc<Mutex<Vec<RouteMemo>>>,
}

/// One routed text of an [`IntentHistory`]'s memo.
#[derive(Debug, Clone)]
pub(crate) struct RouteMemo {
    key: (&'static str, String, String, usize),
    routed: Arc<(Idr, Vec<Vec<String>>)>,
}

impl IntentHistory {
    /// A history with no override.
    pub fn new(
        snapshot: HistorySnapshot,
        messages: Option<Vec<HistoryMessage>>,
        exclude: BTreeSet<String>,
    ) -> IntentHistory {
        IntentHistory {
            snapshot,
            messages,
            exclude,
            now: None,
            vocab: None,
            git: None,
            rev: None,
            memo: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// W5 / M count back from `now` (an author time, seconds).
    pub fn with_now(mut self, now: i64) -> IntentHistory {
        self.now = Some(now);
        self
    }

    /// Route ids with `vocab` instead of the root's.
    pub fn with_vocab(mut self, vocab: IdVocab) -> IntentHistory {
        self.vocab = Some(vocab);
        self
    }

    /// Read trees from the repository `git` (a bare clone serves), and route
    /// `find --files`' ids at revision `rev`.
    pub fn at_rev(mut self, git: impl Into<PathBuf>, rev: impl Into<String>) -> IntentHistory {
        self.git = Some(git.into());
        self.rev = Some(rev.into());
        self
    }

    /// `root`'s own snapshot and messages, `exclude` left out; None without
    /// a complete snapshot.
    pub(crate) fn from_root(root: &Path, exclude: &BTreeSet<String>) -> Option<IntentHistory> {
        let snapshot = read_history(root)?;
        let messages = read_history_messages(root, &snapshot);
        Some(IntentHistory::new(snapshot, messages, exclude.clone()))
    }

    /// The `[ids]` vocabulary: the override, else `root`'s.
    pub(crate) fn vocab_for(history: Option<&IntentHistory>, root: &Path) -> IdVocab {
        history
            .and_then(|h| h.vocab.clone())
            .unwrap_or_else(|| IdVocab::for_root(root))
    }

    /// The memoised route of `key`, else `compute`'s, kept.
    pub(crate) fn routed(
        history: Option<&IntentHistory>,
        key: (&'static str, String, String, usize),
        compute: impl FnOnce() -> (Idr, Vec<Vec<String>>),
    ) -> Arc<(Idr, Vec<Vec<String>>)> {
        let Some(h) = history else {
            return Arc::new(compute());
        };
        let held = h
            .memo
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|m| m.key == key)
            .map(|m| Arc::clone(&m.routed));
        if let Some(r) = held {
            return r;
        }
        let r = Arc::new(compute());
        h.memo
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(RouteMemo {
                key,
                routed: Arc::clone(&r),
            });
        r
    }
}

/// What one intent ranking stands on (review's base and working tree).
struct IntentAt<'a> {
    after: &'a MergedGraph,
    before: &'a MergedGraph,
    /// The base graph's file space.
    base_files: Arc<FileIndex>,
    primary: Option<u64>,
    base_repo: Option<u64>,
    label: String,
    /// The working tree: its `[ids]` and, without `history.git`, its git.
    root: &'a Path,
    base_sha: &'a str,
    title: &'a str,
    cache: &'a RankCache,
    /// The changed files (the cache's file space) and seed nodes in no file.
    seed_files: &'a [u32],
    extra: &'a [NodeId],
    /// None: no history snapshot.
    history: Option<&'a IntentHistory>,
    /// The commits no signal reads.
    exclude: &'a BTreeSet<String>,
    /// A forced SELt2 candidate; None reads the calibration record.
    pick: Option<&'a str>,
}

/// One intent ranking: the candidates, the routed ids, the signals, the
/// selection and the order of the candidates (changed files left out).
struct IntentRanked {
    cands: Candidates,
    routed_ids: Vec<RoutedId>,
    ids_of: Vec<Vec<String>>,
    signals: IntentSignals,
    selection: Selection,
    order: Vec<usize>,
    text_state: &'static str,
}

/// Review's intent ranking (EB.7, EC.11, EC.12): P1, the title's text over
/// past messages, IDR at the base, the SELt2 pick (or `x.pick`) and its
/// order. Every read of the history goes through `x.history`.
fn intent_rank(x: &IntentAt<'_>) -> IntentRanked {
    let (before, after, cache) = (x.before, x.after, x.cache);
    let files = cache.files(after);
    let cands = x
        .primary
        .map(|r| Candidates::of(&files, r))
        .unwrap_or_default();
    let n = cands.len();
    let (seed_files, extra) = (x.seed_files, x.extra);

    // P1: rank_eval's base PPR from every node of the changed files,
    // max-pooled per file (`rank_files` under `Ranking::head`).
    let ppr: Vec<f64> = if seed_files.is_empty() && extra.is_empty() {
        vec![0.0; n]
    } else {
        let fr = rank_files(after, cache, seed_files, extra, Ranking::head());
        cands
            .files
            .iter()
            .map(|&f| fr.keys.get(f as usize).map_or(0.0, |k| k[0]))
            .collect()
    };
    let excluded: Vec<bool> = cands.files.iter().map(|f| seed_files.contains(f)).collect();

    // The title's BM25 text over past messages, the PR's own left out.
    let label = x.label.as_str();
    let query = subject(x.title).to_string();
    let (text, text_state) = match x.history {
        None => {
            TextAbsent::History.report(label, "review");
            (vec![0.0; n], "absent")
        }
        Some(IntentHistory { messages: None, .. }) => {
            TextAbsent::Messages.report(label, "review");
            (vec![0.0; n], "absent")
        }
        Some(
            h @ IntentHistory {
                messages: Some(m), ..
            },
        ) => {
            let idx = TextIndex::build(&h.snapshot, m, &cands, x.exclude);
            idx.report(label, "review", &query);
            (idx.scores(&query), "present")
        }
    };

    // IDR, read at the base: its docs and trackers, its Tier-4 sections, its
    // candidates; the rules (`[ids]`) are the working tree's.
    let git: &Path = x.history.and_then(|h| h.git.as_deref()).unwrap_or(x.root);
    let tree = Tree::Rev {
        repo: git,
        sha: x.base_sha.to_string(),
    };
    let base_cands = x
        .base_repo
        .map(|r| Candidates::of(&x.base_files, r))
        .unwrap_or_default();
    let snap = x.history.map(|h| &h.snapshot);
    let msgs = x.history.and_then(|h| h.messages.as_deref());
    let key = (
        "review",
        format!("{}@{}", git.display(), x.base_sha),
        x.title.to_string(),
        base_cands.len(),
    );
    let vocab = IntentHistory::vocab_for(x.history, x.root);
    let routed = IntentHistory::routed(x.history, key, || {
        let projects = project_roots(before);
        let dirs: Vec<&str> = projects
            .iter()
            .map(|p| p.path.as_str())
            .filter(|p| *p != ".")
            .collect();
        let renames = snap.map(rename_pairs).unwrap_or_default();
        let defs = read_defs(
            &tree,
            &dirs,
            &vocab,
            x.base_repo.map(|r| (before, RepoId(r))),
        );
        let ns = defs.namespaces(&vocab.local_words);
        let inputs = IdrInputs {
            defs,
            ns,
            paths: PathResolver::new(&base_cands, &renames),
        };
        let side = inputs
            .side(&base_cands, &vocab, &tree)
            .labelled(label, "review");
        let recent = match (snap, msgs) {
            (Some(s), Some(m)) => recent_scopes(
                &recent_subjects(s, m, &base_cands, x.exclude),
                &inputs.ns,
                &vocab,
            ),
            _ => Vec::new(),
        };
        let routed = route(x.title, &side, &recent);
        let base_ids = ids_by_file(&routed, &inputs, &vocab, &tree, &base_cands);
        (routed, base_ids)
    });
    let (routed, base_ids) = (&routed.0, &routed.1);
    let mut idr = vec![0.0; n];
    let mut ids_of: Vec<Vec<String>> = vec![Vec::new(); n];
    for (i, p) in cands.paths.iter().enumerate() {
        if let Some(b) = base_cands.position(p) {
            idr[i] = routed.idr.get(b).copied().unwrap_or(0.0);
            ids_of[i] = base_ids.get(b).cloned().unwrap_or_default();
        }
    }

    // EC.11: the intent fusion is set SELt2's (EC.12) calibrated pick at the shape of
    // the changed files, else EB.7's LX[IDR,P1]; its signals come from the
    // same bank the calibration replay ranks through.
    let shape = Shape::from_seed_files(seed_files.len());
    let selection = match x.pick {
        Some(name) => explicit(REVIEW_SET, shape, name),
        None => select(
            cache.calibration().read(),
            REVIEW_SET,
            shape,
            &REVIEW_CANDIDATES,
            REVIEW_FALLBACK,
        ),
    };
    log_selection("review_intent", &selection);
    let now_at = needs(&selection.pick)
        .is_some_and(|w| w.w5 || w.m)
        .then(|| match x.history.and_then(|h| h.now) {
            Some(t) => Some(t),
            None => newest_author_time(git, x.base_sha),
        })
        .flatten();
    let inputs_x = AnswerInputs {
        merged: after,
        cache,
        cands: &cands,
        seed_files,
        extra,
        exclude: x.exclude,
    };
    let history = snap.map(|s| (s, msgs.unwrap_or(&[])));
    let bank = answer_signals(&selection.pick, &inputs_x, now_at, history);
    let signals = IntentSignals {
        idr,
        text,
        ppr,
        excluded,
        tie: (0..n as u32).collect(),
        bank,
    };
    let order = signals.order(&selection.pick).unwrap_or_else(|| {
        fuse::lx(
            &signals.idr,
            &[&signals.ppr],
            &signals.tie,
            &signals.excluded,
        )
    });
    IntentRanked {
        cands,
        routed_ids: routed.ids.clone(),
        ids_of,
        signals,
        selection,
        order,
        text_state,
    }
}

/// The intent file order of a review whose changed files are `changed` (the
/// rank cache's file space), title `title`, base `base_sha`: the candidates
/// in the order review's `### Intent` table lists them (EB.7 / EC.11 /
/// EC.12, uncut), as file indices of `cache.files(merged)`. `merged` is both
/// the base and the working graph; `history` replaces every read of `root`'s
/// history snapshot; `pick: Some(name)` forces that SELt2 candidate (source
/// `explicit`), None reads the cache's calibration record as review does.
/// Prints what review's intent prints (`[intent] text`, `[idr]`, `[select]
/// surface=review_intent ..`) and `[review-intent] seam ..`.
///
/// For the rank gate's harness (`engine/examples/rank_eval.rs`): review
/// routes through the same ranking; unstable.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn intent_files_with(
    merged: &MergedGraph,
    root: &Path,
    base_sha: &str,
    title: &str,
    changed: &[u32],
    cache: &Arc<RankCache>,
    history: &IntentHistory,
    pick: Option<&str>,
) -> Vec<u32> {
    cache.guard(merged);
    let files = cache.files(merged);
    let primary = cache
        .repo_roots()
        .keys()
        .next()
        .copied()
        .or_else(|| (!files.is_empty()).then(|| files.repo(0)));
    let label = primary
        .and_then(|r| cache.repo_labels().get(&r).cloned())
        .unwrap_or_else(|| "-".to_string());
    let ranked = intent_rank(&IntentAt {
        after: merged,
        before: merged,
        base_files: Arc::clone(&files),
        primary,
        base_repo: primary,
        label,
        root,
        base_sha,
        title,
        cache,
        seed_files: changed,
        extra: &[],
        history: Some(history),
        exclude: &history.exclude,
        pick,
    });
    let listed = listed_order(&ranked.signals, ranked.order);
    eprintln!(
        "[review-intent] seam fusion={} candidates={} listed={}",
        ranked.selection.pick,
        ranked.cands.len(),
        listed.len()
    );
    listed.into_iter().map(|i| ranked.cands.files[i]).collect()
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

/// `### Intent` (module docs "Markdown"): the title, the routed ids, the
/// ranked files.
fn intent_md(blocks: &mut Vec<String>, r: &Review, max: usize) {
    let Some(i) = &r.intent else { return };
    blocks.push("### Intent".to_string());
    blocks.push(format!("_title ({}): {}_", i.title_source, text(&i.title)));
    let sel = &i.selection;
    let how = match (sel.source, sel.reason) {
        ("calibrated", _) => format!(
            "calibrated on {} commits, shape {}",
            sel.test_commits, sel.shape
        ),
        (_, Some(reason)) => format!("uncalibrated: {reason}"),
        (source, None) => format!("uncalibrated: {source}"),
    };
    blocks.push(format!("_ranked by {} ({})_", text(&i.fusion), text(&how)));
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
    let mut model_note = match (r.model_breaks.len(), model_advisory.len()) {
        (0, _) => String::new(),
        (m, 0) => format!(" | {m} model break(s)"),
        (m, a) => format!(" | {m} model break(s) ({a} advisory)"),
    };
    // FH.7b: only with a name break, so every earlier headline holds.
    if c.name_breaks > 0 {
        model_note.push_str(&format!(" | {} name break(s)", c.name_breaks));
    }
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
    if let Some(line) = health::blind_line(&r.blind) {
        blocks.push(line);
    }
    let unchanged = r.changed.is_empty()
        && r.edges.is_empty()
        && r.new_violations.is_empty()
        && r.resolved_violations.is_empty()
        && r.model_breaks.is_empty()
        && c.name_breaks == 0;
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
    name_breaks_md(&mut blocks, r, max);
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
    impact_areas_md(&mut blocks, r, max);
    intent_md(&mut blocks, r, max);
    conventions_md(&mut blocks, r, max);

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
        c.nodes_changed,
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
    let mut sink: Vec<String> = Vec::new();
    for section in [&blocking, &advisory, &resolved] {
        section_md(&mut sink, section, &mut fold, 0, None);
    }
    fold
}

/// One section's violations, one `check::render_rule_md` block per run of
/// one rule (the report sorts them by rule id, and a section keeps that
/// order), in review's form ([`RuleMdOptions::default`]); its caveat fold
/// added to `fold`.
fn section_md(
    blocks: &mut Vec<String>,
    section: &[&Violation],
    fold: &mut CaveatFold,
    max: usize,
    located_at: Option<&str>,
) {
    let opts = RuleMdOptions {
        max_rows: max,
        located_at: located_at.map(String::from),
        ..RuleMdOptions::default()
    };
    let mut i = 0;
    while i < section.len() {
        let id = &section[i].rule_id;
        let run = section[i..].iter().take_while(|v| &v.rule_id == id).count();
        blocks.extend(check::rule_md_blocks(&section[i..i + run], fold, &opts));
        i += run;
    }
}

/// One section's violations (module docs "Markdown").
fn violations_md(
    blocks: &mut Vec<String>,
    section: &[&Violation],
    max: usize,
    located_at: Option<&str>,
) {
    section_md(blocks, section, &mut CaveatFold::default(), max, located_at);
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

/// serde's `skip_serializing_if` for a count left out at 0 (FH.7b).
fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// FH.8d: cut `rows` to `max`, returning how many it left out. A cut keeps
/// the folded rows first (each stands for many client sites), then by tier
/// (strongest first), file, line, repo label and normalised name; with
/// nothing to cut the rows keep `contract_breaks::name_breaks`' order.
fn cap_name_breaks(rows: &mut Vec<NameBreak>, max: usize) -> usize {
    if rows.len() <= max {
        return 0;
    }
    let tier = |t: &str| Tier::parse(t).unwrap_or(Tier::Heuristic);
    rows.sort_by(|a, b| {
        (
            a.folded.is_none(),
            tier(a.tier),
            &a.file,
            a.line,
            &a.repo,
            &a.normalised,
        )
            .cmp(&(
                b.folded.is_none(),
                tier(b.tier),
                &b.file,
                b.line,
                &b.repo,
                &b.normalised,
            ))
    });
    let cut = rows.len() - max;
    rows.truncate(max);
    cut
}

/// FH.8d: where a folded name row (FH.7d, `position` `folded`) stands:
/// `<N> sites in <M> files (e.g. a, b, c)` from its `client_sites`,
/// `client_files` and `sample_files`; `None` for a site row, which stands
/// at its own `file:line`. Plain text: the CLI's `## names` table and
/// review's `### Name breaks (advisory)` escape it their own way.
pub fn name_break_sites(n: &NameBreak) -> Option<String> {
    n.folded?;
    let sites = n.client_sites.unwrap_or(1);
    let files = n.client_files.unwrap_or(n.sample_files.len());
    let mut out = format!("{sites} sites in {files} files");
    if !n.sample_files.is_empty() {
        out.push_str(&format!(" (e.g. {})", n.sample_files.join(", ")));
    }
    Some(out)
}

/// FH.8d: the review's JSON, the one serialisation the CLI's `--json` and
/// pyo3's dict read (in place of `answers::answer_json`, whose line it
/// extends). fired_on, stderr, once per call:
/// `[answer-size] surface=review rows=<n> shown=<k> offset=<o> areas=<a>
/// names=<n> names_shown=<k> tests_shown=<t> bytes=<b> seeds=<s>
/// seeds_shown=<t> also_in_folded=<f>` (`rows` / `shown` the impact's,
/// `names` the uncut name rows).
pub fn review_json(review: &Review) -> serde_json::Result<String> {
    let text = serde_json::to_string(review)?;
    eprintln!("{}", answer_size_line(review, text.len()));
    Ok(text)
}

fn answer_size_line(r: &Review, bytes: usize) -> String {
    let size = r.answer_size();
    format!(
        "[answer-size] surface=review rows={} shown={} offset={} areas={} names={} names_shown={} tests_shown={} bytes={bytes} seeds={} seeds_shown={} also_in_folded={}",
        r.counts.impact,
        r.impact.results.len(),
        size.offset,
        size.areas,
        r.counts.name_breaks,
        r.name_breaks.len(),
        r.tests.tests.len(),
        size.seeds,
        size.seeds_shown,
        size.also_in_folded,
    )
}

/// `### Name breaks (advisory)` (FH.7b, module docs "Markdown"; only when a
/// name row exists): one line per client site, cut at `max` and counted
/// against `counts.name_breaks` (FH.8d: the uncut total).
fn name_breaks_md(blocks: &mut Vec<String>, r: &Review, max: usize) {
    if r.counts.name_breaks == 0 {
        return;
    }
    blocks.push("### Name breaks (advisory)".to_string());
    name_breaks_table(blocks, &r.name_breaks, r.counts.name_breaks, max);
}

/// The name rows' table: a site row at its `file:line`, a folded one at
/// [`name_break_sites`].
fn name_breaks_table(blocks: &mut Vec<String>, rows: &[NameBreak], total: usize, max: usize) {
    // A review is one repo's: every site is the working tree's file.
    let lines = rows.iter().map(|n| {
        let name = match &n.renamed_to {
            Some(to) => format!("{} -> {}", code(&n.name), code(to)),
            None => code(&n.name),
        };
        let place = match name_break_sites(n) {
            Some(sites) => text(&sites),
            None => at(Some(&n.file), Some(n.line)),
        };
        vec![
            name,
            text(n.tier),
            place,
            text(n.position),
            code(&n.client),
            code(&n.server),
        ]
    });
    push_table(
        blocks,
        &["name", "tier", "at", "position", "client", "server"],
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

/// `### Conventions` (FB.12b; only when one governs a changed file): `|
/// convention | scope | files changed | source |`, the source cell
/// `<source>/<id>` then ` (<tier>)`, and ` at <decl>` for an overlay one.
fn conventions_md(blocks: &mut Vec<String>, r: &Review, max: usize) {
    if r.conventions.is_empty() {
        return;
    }
    blocks.push("### Conventions".to_string());
    push_table(
        blocks,
        &["convention", "scope", "files changed", "source"],
        r.conventions.iter().map(|c| {
            let mut source = format!("{}/{} ({})", c.source, c.id, c.tier);
            if let Some(decl) = &c.decl {
                source.push_str(&format!(" at {decl}"));
            }
            vec![
                text(&c.text),
                code(&c.scope),
                c.files.len().to_string(),
                text(&source),
            ]
        }),
        r.conventions.len(),
        max,
    );
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
pub(crate) const DASH: &str = "—";

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
pub(crate) fn push_table<I>(
    blocks: &mut Vec<String>,
    header: &[&str],
    rows: I,
    total: usize,
    max: usize,
) where
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

/// FH.8c: `### Impact areas`, only when the impact rows were cut: every
/// impact row by area (`BlastRadius::areas`), then the next page's flags
/// (`impact.truncated.next`): review's `--offset`, and for one area
/// `glia diff-impact --scope`, the same impact (review takes no scope).
fn impact_areas_md(blocks: &mut Vec<String>, r: &Review, max: usize) {
    let impact = &r.impact;
    let Some(next) = impact.truncated.as_ref().and_then(|t| t.next.as_ref()) else {
        return;
    };
    if impact.areas.is_empty() {
        return;
    }
    blocks.push("### Impact areas".to_string());
    push_table(
        blocks,
        &["area", "rows", "files", "top file", "scope"],
        impact.areas.iter().map(|a| {
            vec![
                text(&a.area),
                a.rows.to_string(),
                a.files.to_string(),
                a.top_file.as_deref().map_or_else(|| DASH.to_string(), text),
                a.scope.as_deref().map_or_else(|| DASH.to_string(), code),
            ]
        }),
        impact.areas.len() + impact.truncated.as_ref().map_or(0, |t| t.areas),
        max,
    );
    // review takes no --scope: an area's rows are diff-impact's `--scope`.
    let scope = next
        .scope
        .as_deref()
        .map(|s| {
            format!(
                ", or one area: `glia diff-impact --base {} --scope {s}`",
                r.base
            )
        })
        .unwrap_or_default();
    blocks.push(format!(
        "_next: `--offset {}`{scope}; `--full` lists every row_",
        next.offset
    ));
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
pub(crate) fn at(file: Option<&str>, line: Option<i64>) -> String {
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
pub(crate) fn text(s: &str) -> String {
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
pub(crate) fn code(s: &str) -> String {
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
    Tier::ALL
        .into_iter()
        .find(|x| x.as_str() == t)
        .map_or(3, Tier::level)
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
        None => (Tier::Derived, Some("no evidence recorded".to_string())),
    };
    let tier = tier.as_str();
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
    let tier = tier.as_str();
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
            bank: Signals::default(),
        };
        let o = |n: &str| s.order(n).unwrap_or_else(|| panic!("{n}"));
        assert_eq!(o(INTENT_FUSION_REVIEW), [1, 3, 2, 0]);
        assert_eq!(o("H9s50"), [0, 2, 1, 3]);
        assert_eq!(o("LX[IDR,H9s50]"), [1, 3, 0, 2]);
        assert!(s.order("B0").is_none());
        // ties go by candidate position
        let flat = IntentSignals {
            idr: vec![0.0; 3],
            text: vec![0.0; 3],
            ppr: vec![0.0; 3],
            excluded: vec![false; 3],
            tie: vec![0, 1, 2],
            bank: Signals::default(),
        };
        for n in [INTENT_FUSION_REVIEW, "H9s50", "LX[IDR,H9s50]"] {
            assert_eq!(flat.order(n), Some(vec![0, 1, 2]), "{n}");
        }
    }

    /// A name row at `file:line`; `folded` makes it FH.7d's aggregate.
    fn name_row(file: &str, line: i64, tier: &'static str, folded: bool) -> NameBreak {
        NameBreak {
            name: "id".to_string(),
            normalised: "id".to_string(),
            renamed_to: Some("ticketId".to_string()),
            repo: "app".to_string(),
            file: file.to_string(),
            line,
            position: if folded { "folded" } else { "property" },
            client: "web::rows::pick".to_string(),
            server: "main::Ticket".to_string(),
            tier,
            blocking: false,
            folded: folded.then_some("generic"),
            client_sites: folded.then_some(7),
            client_files: folded.then_some(4),
            sample_files: if folded {
                vec!["web/a.ts".into(), "web/b.ts".into(), "web/c.ts".into()]
            } else {
                Vec::new()
            },
        }
    }

    fn places(rows: &[NameBreak]) -> Vec<(String, i64)> {
        rows.iter().map(|n| (n.file.clone(), n.line)).collect()
    }

    /// FH.8d: a cut keeps folded rows first, then tier, file, line, and
    /// counts what it dropped; no cut keeps the answer's order.
    #[test]
    fn the_name_cap_keeps_folded_rows_first_and_counts_the_rest() {
        let rows = vec![
            name_row("web/a.ts", 3, "heuristic", false),
            name_row("web/b.ts", 9, "derived", false),
            name_row("web/z.ts", 1, "heuristic", true),
            name_row("web/a.ts", 1, "heuristic", false),
        ];
        let mut uncut = rows.clone();
        assert_eq!(cap_name_breaks(&mut uncut, 4), 0);
        assert_eq!(places(&uncut), places(&rows), "no cut, no reorder");
        let mut all = rows.clone();
        assert_eq!(cap_name_breaks(&mut all, usize::MAX), 0);
        assert_eq!(places(&all), places(&rows), "--full keeps the order");
        let mut cut = rows.clone();
        assert_eq!(cap_name_breaks(&mut cut, 3), 1);
        assert_eq!(
            places(&cut),
            vec![
                ("web/z.ts".to_string(), 1),
                ("web/b.ts".to_string(), 9),
                ("web/a.ts".to_string(), 1),
            ]
        );
        let mut none = rows;
        assert_eq!(cap_name_breaks(&mut none, 0), 4);
        assert!(none.is_empty());
    }

    /// FH.8d: a folded row reads as its counts and samples, a site row as
    /// nothing (it stands at its own file:line).
    #[test]
    fn a_folded_name_row_names_its_sites() {
        assert_eq!(
            name_break_sites(&name_row("web/z.ts", 1, "heuristic", true)).as_deref(),
            Some("7 sites in 4 files (e.g. web/a.ts, web/b.ts, web/c.ts)")
        );
        assert_eq!(
            name_break_sites(&name_row("web/a.ts", 3, "heuristic", false)),
            None
        );
    }

    /// FH.8d: review's `### Name breaks (advisory)` table prints a folded
    /// row's counts in its `at` cell and counts the cut against the uncut
    /// total.
    #[test]
    fn the_name_table_prints_a_folded_rows_counts() {
        let rows = vec![
            name_row("web/z.ts", 1, "heuristic", true),
            name_row("web/a.ts", 3, "heuristic", false),
        ];
        let mut blocks = Vec::new();
        name_breaks_table(&mut blocks, &rows, 30, 20);
        let table = &blocks[0];
        assert!(
            table.contains(
                "| `id` -> `ticketId` | heuristic | 7 sites in 4 files (e.g. web/a.ts, web/b.ts, web/c.ts) | folded | "
            ),
            "{table}"
        );
        assert!(
            table.contains("| `id` -> `ticketId` | heuristic | web/a.ts:3 | property | "),
            "{table}"
        );
        assert!(!table.contains("web/z.ts:1"), "{table}");
        assert_eq!(blocks[1], "_(2 of 30)_");
    }
}
