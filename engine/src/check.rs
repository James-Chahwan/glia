//! **check** (LE.8): the declared architecture rules, evaluated. A rule a
//! human wrote down ("web must not reach into the api's internals", "the api
//! has no import cycles") that the graph breaks is a VIOLATION, with the
//! located edges that break it. This is the CI gate over the rules; `glia
//! cycles` (LE.6b) is the report.
//!
//! The rules are LF.4a's `[[constraint]]` stanzas (and cell-API rules), read
//! only through [`declared_constraints`]: its scopes are already
//! repo-relative paths (`.` = the root), so this module never re-resolves a
//! label.
//!
//! - `forbid_edge {from, to}` (`transitive` false): every DIRECT edge (the
//!   observed fact) whose category is in the rule's set, `from` end in scope
//!   `from` and `to` end in scope `to`.
//!   The set is the rule's `categories` (edge category NAMES), else
//!   [`default_forbid_categories`]: IMPORTS plus the code profile's carry
//!   edges, minus TESTS and DOCUMENTS (a test or a doc legitimately crosses
//!   a boundary). One [`Violation`] per rule; evidence sorted by `(tier,
//!   file, line, category)`, capped at [`MAX_EVIDENCE`] with `count` the
//!   full number.
//! - `no_cycle {scope}`: `categories` empty or exactly `[IMPORTS]` checks the
//!   module import graph (`cycles::module_import_graph`, LE.6b) restricted to
//!   the modules in scope; any other set checks the node-level graph of those
//!   categories restricted to the nodes in scope. One [`Violation`] per
//!   strongly-connected component, its evidence a shortest witness cycle and
//!   its `count` the component's size. An unscoped rule checks the whole
//!   graph.
//! - `invariant {text, rule}` with a `rule` (overlay version 2's Datalog
//!   text, DB.1): evaluated through the rules layer, one [`RULE`]
//!   [`Violation`] per `violation` row (module doc "Rule invariants and
//!   min_tier").
//! - `forbid_edge {from, to, via}` with `transitive` (overlay version 2,
//!   DB.1): every PATH from `from` into `to` through any third scope, a
//!   `via` facade allowed, one [`FORBID_PATH`] [`Violation`] per node of
//!   `to` reached (module doc "Transitive forbid_edge (Q3)").
//! - `invariant {text}` without a `rule`, and any kind this crate cannot
//!   evaluate: listed in [`CheckReport::unchecked`], never dropped.
//!
//! # Tiers (CC.3)
//!
//! Every evidence row carries the tier `why` gives the same edge
//! (`why::tier_of`, one rule for both surfaces): a parser / graph binding the
//! source spells out is [`FACT`]; a resolver- or pass-paired edge, a graph
//! edge inferred below Strong confidence (Go implicit IMPLEMENTS) or one with
//! no location is [`DERIVED`]; an overlay declaration, a git co-change or a
//! name-only guess is [`HEURISTIC`]. The row's `note` says why when the tier
//! is not the stage's plain one, or where an overlay edge was declared.
//!
//! - forbid_edge: the Violation takes its STRONGEST row's tier. One observed
//!   edge proves the forbidden dependency exists; a rule broken only by
//!   resolver-paired edges is derived, only by declared or name-only edges
//!   heuristic. Rows sort tier-first, so the truncation keeps the facts.
//! - no_cycle: the Violation is [`DERIVED`] (the cycle is computed) unless a
//!   hop is [`HEURISTIC`], then heuristic: every hop is needed for the cycle,
//!   so its weakest hop bounds it. A node-level hop is tiered over the real
//!   graph edge; an import hop over the lifted module edge, which carries the
//!   first import's evidence and confidence but no ORIGIN cell (an overlay
//!   import keeps its heuristic tier; its note names the stanza, not who
//!   declared it).
//!
//! - rule: the row's tier, the rules layer's lattice (the min over its
//!   derivations of the max over their inputs), named as `why` names it.
//! - forbid_path: the first of three walks (FACT edges, + DERIVED, all) that
//!   reaches the hit, which is its best path's weakest hop: a HEURISTIC short
//!   path never masks a FACT long one. Each witness row keeps its own edge's
//!   tier.
//!
//! # Transitive forbid_edge (Q3)
//!
//! A `forbid_edge` with `transitive = true` forbids reaching `to` at all: a
//! path from scope `from` through any third scope to its FIRST node in `to`
//! is a violation, unless it enters `to` through a `via` facade, which is
//! never walked through. Seeds are the nodes of `from` outside `to` and the
//! facades, in (qname, id) order; a walk stops at every node of `to` and of
//! a facade (it enters, never leaves), so a node only a facade reaches is no
//! hit. A `via` scope no node sits in is a rule error, like an empty `from`
//! or `to`.
//!
//! IMPORTS and the symbol categories chain separately (0.5.2 Q3): a module
//! import is not a call path. The `imports` chain walks the module import
//! graph (`cycles::module_import_graph`, the graph no_cycle's import rules
//! use; a MODULE is in a scope by its file); the `symbols` chain walks the
//! rule's other categories over the node graph. A rule that NAMES both
//! IMPORTS and CALLS walks one `mixed` chain over every category it names.
//! The default set (no `categories`) is [`default_forbid_categories`], so it
//! walks both separate chains. A node two chains hit is one violation: the
//! stronger tier, then fewer hops, then the imports chain.
//!
//! Each walk is `activation::algo::reach::bfs_levels`, the levelled BFS the
//! rules layer's `reaches` runs, over each edge's `why` tier (a lifted import
//! takes its first import's), so `glia query` and `glia check` tier a path
//! alike. One [`FORBID_PATH`] [`Violation`] per hit, sorted by subject:
//! `subject` the hit's qname, `file` / `line` its location, `tier` its pass's,
//! `count` its hop count, `evidence` one shortest witness at that tier IN
//! PATH ORDER (a witness, not a tier-sorted list): each depth layer is
//! expanded in (qname, id) order, so a hop's parent is the lowest-ranked one,
//! and between parallel edges the hop is the strongest tier, then the
//! smallest category name, then the first in edge order. The hit set and
//! tiers are checkout-stable; two equally short, equally tiered witnesses
//! can differ between checkouts only where qnames tie. A direct forbid_edge
//! keeps its own function and output.
//!
//! fx1 (web/app.py imports shared.util and services.api.public.facade;
//! `page` calls `shared.util.helper`, which calls
//! `services.api.internal.db.query`; `other` calls the facade's `get`, which
//! calls `db.write`), rule `from = "web"`, `to = "services/api"`, `via =
//! ["services/api/public"]`: two hits, `services::api::internal::db` over
//! `web::app -> shared::util -> services::api::internal::db` (imports) and
//! `services::api::internal::db::query` over `web::app::page ->
//! shared::util::helper -> services::api::internal::db::query` (symbols),
//! both FACT with 2 hops. The facade and `db::write` (reached only through
//! it) are not; without `via` the facade and `get` are hits too, and
//! `db::write` still is not (the walk stops at `get`, the first node of
//! `to`).
//!
//! # Rule invariants and min_tier
//!
//! An invariant's `rule` is parsed and analysed at CHECK time (R1 §3), the
//! prelude merged in, and evaluated by `rules::evaluate_rule` over the same
//! `CodeFacts` (DB.2a) this check reads its scopes from, so one `Locator`
//! serves the whole check. Its source name is `<id> (<decl>)` (`cell API`
//! for a rule with no decl), so an error reads `<id>
//! (.glia/overlay.toml:<n>):<line>:<col>: error: ...` with the line and
//! column 1-based within the rule text. A parse error, an analysis error (an
//! unknown node kind / edge category / cell type / predicate, an unsafe
//! variable, an unstratifiable negation), a program that defines no
//! `violation` predicate, and a rule whose entry did not come from
//! `.glia/overlay.toml` (the only place rule text is declared, 0.5.2 Q9: no
//! write path stores one elsewhere) are [`CheckReport::errors`] rows; the
//! rule is not counted checked. Each `violation` row, in the rules layer's
//! rendered order, is one [`RULE`] [`Violation`]: `subject` the head values
//! rendered (a node's qname, a string as is, an int in decimal), `file` /
//! `line` the first node value's location, `tier` the row's, `evidence` the
//! row's minimal derivation's leaf edges located (at most [`MAX_EVIDENCE`],
//! `count` all of them; a row derived from no edge has none), `caveats` the
//! coverage caveats of every edge category the row's negations read (Q5:
//! "no edge" may be a blind extraction). Every row is listed, uncapped.
//!
//! Every [`Violation`] carries `blocking`: whether it fails CI. A rule's
//! `min_tier` (DB.1: `fact` | `derived`, default [`DEFAULT_MIN_TIER`]) is the
//! weakest tier that blocks: [`is_blocking`] is true when the violation's
//! tier is at least as strong as its rule's [`min_tier_of`], and never for a
//! [`HEURISTIC`] violation (0.5.2 Q2: an overlay edge, a co-change or a name
//! guess is reported, never a CI failure). It applies to every kind:
//! forbid_edge, no_cycle and rule alike; a reflexion divergence uses
//! [`DEFAULT_MIN_TIER`] (the model has no per-rule gate). A non-blocking
//! violation is reported in full; `glia check`'s exit code reads `blocking`
//! (DB.3d).
//!
//! SCOPE MEMBERSHIP IS STRICT. A node is in scope `X` only when the file one
//! `Locator` places it in sits under `X` on a path-segment boundary (`web`
//! holds `web/app.py`, not `webhooks/app.py`), or it is a PROJECT whose path
//! does. A node no file places (a SQL table, a queue topic) is in no scope:
//! the keep-unlocatable leniency of `node_in_scope` (right for filtering a
//! blast radius) would accuse every rule that names a data entity's
//! neighbour. A scope no node sits in is a rule error, not a silent pass.
//!
//! Multi-repo: scopes are repo-relative, so a rule is evaluated against every
//! merged repo's nodes at that relative path; a cross-repo edge matches when
//! both ends are in scope in their own repos.
//!
//! Architecture rules only: nothing here carries auth or security semantics.
//!
//! # Reflexion model (check v2)
//!
//! CC.5a stores a reflexion model (Murphy, Notkin and Sullivan) as CONSTRAINT
//! entries of three kinds: `component {name, paths}` and `layer {name, rank,
//! components, strict}` are model DECLARATIONS (not counted in
//! [`CheckReport::rules`], never listed unchecked); `allow {from, to}` is a
//! rule, counted and checked when the model is evaluated. An allow, or a
//! layer, naming a component no entry declares (rejected or orphaned at
//! build, or written through the cell API) is an error for that entry's id,
//! as is a component whose path an equal path of a smaller-named component
//! already owns (possible only through the cell API; the smaller name keeps
//! it).
//!
//! The model is evaluated once per check, only when at least one component is
//! declared ([`CheckReport::reflexion`] is `None` otherwise), as named
//! relations, the stratified-negation Datalog 0.5.2's rule layer compiles the
//! same stanzas into (renaming one needs that translation updated with it).
//! [`ReflexionFacts`] holds them, one field per relation, each derived by one
//! function whose doc comment states its rule:
//!
//! | relation | rule |
//! |---|---|
//! | `component_path(C, Path)` | EDB: each component entry's resolved paths |
//! | `layer_of(C, Rank, Strict)` | EDB: each layer entry's components; rank 0 is the top |
//! | `allow(C1, C2)` | EDB: each allow entry without an error |
//! | `located(N, F)` | EDB: the file one `Locator` places `N` in (strict, as forbid_edge) |
//! | `edge(From, Category, To, E)` | EDB: every edge whose category is checked |
//! | `in_component(N, C)` | `located(N, F), component_path(C, P), under(F, P), not shadowed(F, P)` |
//! | `shadowed(F, P)` | `component_path(_, P2), under(F, P2), longer(P2, P)` |
//! | `dep(C1, C2, E)` | `edge(A, _, B, E), in_component(A, C1), in_component(B, C2), C1 != C2` |
//! | `allowed(C1, C2)` | `allow(C1, C2)` |
//! | | `layer_of(C1, R1, false), layer_of(C2, R2, _), R1 < R2` |
//! | | `layer_of(C1, R1, true), layer_of(C2, R2, _), R2 = R1 + 1` |
//! | `closed()` | `allow(_, _)` ; `layer_of(_, _, _)` |
//! | `convergence(C1, C2)` | `dep(C1, C2, _), allowed(C1, C2)` |
//! | `divergence(C1, C2, E)` | `closed(), dep(C1, C2, E), not allowed(C1, C2)` |
//! | `absence(C1, C2)` | `allow(C1, C2), not dep(C1, C2, _)` |
//! | `unmapped(N, F)` | `located(N, F), not in_component(N, _)` |
//!
//! The same relations are a rules program, `engine/src/rules/reflexion.dl`
//! (DB.8a): `rules::reflexion::derive` feeds it this EDB (an edge as its
//! dedup index `E`, `edge_site(E, A, B)`; a strict flag as `"strict"` /
//! `"lax"`; `R2 = R1 + 1` as a `next_rank(R1, R2)` fact; `dependent(C1, C2)`
//! for `dep(C1, C2, _)`) and reads the relations back into
//! [`ReflexionFacts`]. Both derivations render through one function, and the
//! parity tests (`rules::reflexion_parity`) hold them to byte-identical
//! [`CheckReport`] JSON on seven fixtures, each equal to a golden captured
//! from this native path (`engine/tests/data/reflexion/`). The native
//! functions stay the oracle until DB.8b deletes them.
//!
//! Membership is a pure function of (node file, declared paths): the longest
//! declared path on a path-segment boundary above the file (`web/admin` owns
//! `web/admin/panel.py` over `web`; it does not own `web/admin.py`), a PROJECT
//! by its own path. A node no file places (a queue topic, a SQL table) is in
//! no component and is not unmapped; a cross-service HTTP edge maps through
//! its ENDPOINT's and ROUTE's located files.
//!
//! The checked categories are the union of the allows' `categories` when any
//! sets them, else [`default_forbid_categories`]. What the report says:
//!
//! - the matrix, one [`MatrixCell`] per `dep` pair: [`CONVERGENCE`],
//!   [`DIVERGENCE`], or [`OBSERVED`] in an OPEN model (components only, no
//!   layer and no allow): the map is informative before the rules are
//!   written, and an open model never raises a divergence;
//! - each divergence is also a [`Violation`] (`rule_id`
//!   `reflexion:<C1>-><C2>`, rule_kind [`DIVERGENCE`], `decl` C1's
//!   component, tier its strongest edge's as forbid_edge, evidence located
//!   and tiered like forbid_edge's), so `glia check` exits 1 on it;
//! - each absence (an allow the code never realises) is a
//!   [`ReflexionAbsence`], a [`FACT`] about the graph as built carrying the
//!   coverage caveats (LD.8a) of the checked categories: a blind extraction
//!   looks exactly like an absence. Never a violation;
//! - [`Unmapped`]: the files holding a located MODULE / CLASS / FUNCTION /
//!   METHOD that no component owns.
//!
//! Fired-on marker, one line per call (three when a rule is violated):
//! `[check] rules=<R> checked=<C> violations=<V> (forbid_edge=<F> no_cycle=<N>) unchecked=<U> errors=<E>`,
//! where `V` counts [`Violation`] records (a forbid_edge rule makes at most
//! one, a transitive one one per hit, a no_cycle rule one per cycle, a rule
//! invariant one per row, the reflexion model one per divergent pair) and
//! `F` / `N` split out the direct forbid_edge and no_cycle ones only; when
//! `V` > 0 a second line
//! `[check] tiers fact=<F> derived=<D> heuristic=<H>` splits them by tier
//! (CC.3) — grep `^\[check\] tiers` — and a third
//! `[check] gate blocking=<B> advisory=<A>` splits them by `blocking`
//! (`B` + `A` = `V`) — grep `^\[check\] gate`. When at least one rule
//! invariant or transitive forbid_edge was evaluated, a last line
//! `[rules] check invariant=<I> rows=<R> transitive=<T> hits=<H> (imports=<a> symbols=<b> mixed=<c>)`
//! counts the rule invariants evaluated and their rows, and the transitive
//! rules evaluated, their hits and the hits each chain reported (`a + b + c =
//! H`, DB.3b) — grep `^\[rules\] check`. The rules layer's own `[rules]
//! prelude` / `[rules] eval` lines print during each rule's evaluation,
//! before these. A check
//! with a model prints, before them, `[reflexion] components=<C>
//! closed=<true|false> deps=<P> convergences=<V> divergences=<D>
//! absences=<A> unmapped_files=<U>` (`P` counts dependent component pairs)
//! — grep `^\[reflexion\]`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use glia_activation::algo::cycles::{strongly_connected, witness_cycle};
use glia_activation::algo::reach::{LevelReached, LevelWalk, bfs_levels};
use glia_activation::algo::{Adjacency, CategorySet, GraphSource, Walk};
use glia_code_domain::evidence::Evidence;
use glia_code_domain::external_inputs::{ConstraintKind, ConstraintRule};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Edge, EdgeCategoryId, NodeId, NodeKindId};
use glia_graph::MergedGraph;
use glia_rules::value::Tier;

use crate::coverage::{CoverageNote, caveats_for};
use crate::cycles::{ModuleImports, module_import_graph};
use crate::external::declared::declared_constraints;
use crate::profile::CODE_PROFILE;
use crate::rules::facts::{edge_tier_of, located_edge, tier_name};
use crate::rules::{CodeFacts, evaluate_rule};
use crate::why::tier_of;

/// [`Violation::rule_kind`] of a forbidden-edge rule.
pub const FORBID_EDGE: &str = "forbid_edge";
/// [`Violation::rule_kind`] of a no-cycle rule.
pub const NO_CYCLE: &str = "no_cycle";
/// [`Violation::rule_kind`] of one `violation` row of an invariant's `rule`
/// (module doc "Rule invariants and min_tier").
pub const RULE: &str = "rule";
/// [`Violation::rule_kind`] of one hit of a transitive forbid_edge rule: a
/// path from scope `from` through a third scope into `to` (DB.3b).
pub const FORBID_PATH: &str = "forbid_path";
/// The `min_tier` of a rule that sets none, and of a reflexion divergence:
/// a [`FACT`] or [`DERIVED`] violation blocks (0.5.2 Q2).
pub const DEFAULT_MIN_TIER: &str = DERIVED;
/// The `output` a rule invariant's program defines: its violation rows.
const RULE_OUTPUT: &str = "violation";
/// The only `ConstraintRule::source` whose `rule` is evaluated: rule text is
/// declared in `.glia/overlay.toml` and nowhere else (0.5.2 Q9).
const RULE_SOURCE: &str = "overlay";
/// [`Violation::severity`]: an explicit human rule is broken (an observed
/// convention that is broken is a DIVERGENCE, LE.7's business).
pub const VIOLATION: &str = "VIOLATION";
/// A tier (module doc "Tiers"): read at a site the source spells out.
pub const FACT: &str = "fact";
/// A tier: paired or inferred by the build (a resolver, a pass, a graph
/// binding below Strong confidence), or a computed cycle.
pub const DERIVED: &str = "derived";
/// A tier: declared by a person or a model, co-change in git, or a name-only
/// guess.
pub const HEURISTIC: &str = "heuristic";
/// Evidence rows kept per [`Violation`]; `count` keeps the full number.
pub const MAX_EVIDENCE: usize = 100;
/// [`Violation::rule_kind`] of a reflexion-model divergence, and the
/// [`MatrixCell::status`] of a dependency a closed model does not allow.
pub const DIVERGENCE: &str = "divergence";
/// [`MatrixCell::status`]: a dependency the model allows.
pub const CONVERGENCE: &str = "convergence";
/// [`MatrixCell::status`] in an open model (components only): the dependency
/// is reported, not judged.
pub const OBSERVED: &str = "observed";
/// Unmapped files listed in [`Unmapped::sample`]; `files` keeps the full
/// number.
pub const MAX_UNMAPPED_SAMPLE: usize = 50;

/// One edge that breaks a rule: a forbidden edge, or a hop of a witness cycle.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ViolationEdge {
    pub from_qname: String,
    pub to_qname: String,
    /// The edge category name (`IMPORTS`, `CALLS`, ...).
    pub category: &'static str,
    /// Where the edge is asserted: its EVIDENCE site (LC.3a), else the `from`
    /// node's location.
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// The `<stage>:<name>` that put the edge in the graph (`graph:calls`),
    /// when the edge carries EVIDENCE.
    pub emitter: Option<String>,
    /// [`FACT`] | [`DERIVED`] | [`HEURISTIC`]: the tier `why` gives this
    /// edge (module doc "Tiers").
    pub tier: &'static str,
    /// Why the tier is not the emitter stage's plain one, or where an overlay
    /// edge was declared; `why`'s row note.
    pub note: Option<String>,
}

/// One broken rule: all of a forbid_edge rule's edges, one hit of a
/// transitive forbid_edge, one cycle of a no_cycle rule, one `violation` row
/// of a rule invariant, or one divergent component pair of the reflexion
/// model.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct Violation {
    /// The rule's id; `reflexion:<C1>-><C2>` for a divergence.
    pub rule_id: String,
    /// [`FORBID_EDGE`], [`NO_CYCLE`], [`RULE`], [`FORBID_PATH`] or
    /// [`DIVERGENCE`].
    pub rule_kind: &'static str,
    /// `.glia/overlay.toml:<line>` of the stanza (a divergence: of C1's
    /// `[[component]]`); `None` for a cell-API rule.
    pub decl: Option<String>,
    /// Always [`VIOLATION`].
    pub severity: &'static str,
    /// forbid_edge and divergence: the strongest evidence row's tier.
    /// no_cycle: [`DERIVED`], or [`HEURISTIC`] when a hop of the witness is
    /// (module doc "Tiers"). rule: the row's tier. forbid_path: the first
    /// tiered pass that reached the hit (module doc "Transitive forbid_edge
    /// (Q3)").
    pub tier: &'static str,
    /// Whether it fails CI: `tier` is at least as strong as its rule's
    /// `min_tier` and is not [`HEURISTIC`] ([`is_blocking`]; module doc
    /// "Rule invariants and min_tier"). A non-blocking violation is advisory.
    pub blocking: bool,
    /// forbid_edge and divergence: the edges, all of them (the evidence lists
    /// at most [`MAX_EVIDENCE`]). no_cycle: the nodes (modules, for an import
    /// rule) in the cycle's strongly-connected component; the evidence is
    /// one shortest cycle through it. rule: the row's witness leaf edges,
    /// all of them. forbid_path: the witness's hops, its evidence in path
    /// order.
    pub count: usize,
    pub evidence: Vec<ViolationEdge>,
    /// rule: the row's head values rendered (a node's qname, a string as is,
    /// an int in decimal); forbid_path: the hit's qname. Empty (and not
    /// serialised) for forbid_edge, no_cycle and divergence, which their edge
    /// rows and member sets identify.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subject: Vec<String>,
    /// The first node of `subject`, located; `None` (not serialised) when it
    /// has none or no file places it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    /// rule: the coverage caveats of every edge category the row's negations
    /// read (Q5). Empty (not serialised) otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub caveats: Vec<CoverageNote>,
}

/// Every declared rule, evaluated.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct CheckReport {
    /// CONSTRAINT rules read: every entry but the reflexion model's component
    /// and layer declarations (an allow is a rule).
    pub rules: usize,
    /// forbid_edge (direct and transitive) + no_cycle rules and rule
    /// invariants evaluated, plus the allows of an evaluated model (a rule with an error is not: a rule
    /// invariant whose text does not parse or analyse is an error, not
    /// checked).
    pub checked: usize,
    /// Ids of the rules no graph query evaluates (an `invariant` without a
    /// `rule`, a kind this crate cannot evaluate), in rule order.
    pub unchecked: Vec<String>,
    /// `(entry id, message)`: a rule that could not be evaluated (an unknown
    /// edge category, a scope or `via` facade no node sits in, an allow
    /// naming an undeclared component, a rule invariant's parse / analysis /
    /// evaluation error) is skipped; a model declaration in error (a layer naming an undeclared
    /// component, a component on a path another owns) keeps the rest of its
    /// entry. Rules first, in rule order; then the model's.
    pub errors: Vec<(String, String)>,
    /// Sorted by rule id; a rule's cycles in the order of their first member,
    /// a transitive rule's hits by subject, a rule invariant's rows in the
    /// rules layer's rendered order. A closed
    /// reflexion model's divergences are here too.
    pub violations: Vec<Violation>,
    /// The reflexion model, evaluated (module doc "Reflexion model"); `None`
    /// when no component is declared.
    pub reflexion: Option<Reflexion>,
}

/// One declared component of the reflexion model.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ComponentSummary {
    pub name: String,
    /// Its paths as stored: resolved, repo-relative, `.` = the root; every
    /// entry's paths when several entries declare the name (a merge).
    pub paths: Vec<String>,
    /// The layer it sits in, if any.
    pub layer: Option<String>,
    /// Nodes `in_component` places in it.
    pub nodes: usize,
    /// `.glia/overlay.toml:<line>` of its `[[component]]`; `None` for a
    /// cell-API component.
    pub decl: Option<String>,
}

/// One dependent component pair: the `dep` edges from `from` into `to`.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct MatrixCell {
    pub from: String,
    pub to: String,
    /// The distinct edges (an edge two graphs both hold is one).
    pub edges: usize,
    /// [`CONVERGENCE`], [`DIVERGENCE`] or [`OBSERVED`] (an open model).
    pub status: &'static str,
    /// The strongest edge's tier: one fact edge proves the dependency.
    pub tier: &'static str,
    /// What allows it: an allow id, else `layer:<upper>><lower>`; `None`
    /// when nothing does.
    pub allowed_by: Option<String>,
}

/// An allow the code never realises: no checked edge from `from` into `to`.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ReflexionAbsence {
    pub from: String,
    pub to: String,
    /// The allow's id.
    pub rule_id: String,
    /// The allow's `.glia/overlay.toml:<line>`; `None` for a cell-API allow.
    pub decl: Option<String>,
    /// Always [`FACT`]: a fact about the graph as built.
    pub tier: &'static str,
    /// The coverage caveats of the checked categories for the languages
    /// present: a blind extraction looks exactly like an absence.
    pub caveats: Vec<CoverageNote>,
}

/// The code no component owns.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone, Default)]
pub struct Unmapped {
    /// Files holding a located MODULE / CLASS / FUNCTION / METHOD that maps to
    /// no component.
    pub files: usize,
    /// Those MODULE / CLASS / FUNCTION / METHOD nodes.
    pub nodes: usize,
    /// Distinct checked-category edges between an unmapped located node (of
    /// any kind) and a node in a component, either direction.
    pub edges_to_mapped: usize,
    /// The first [`MAX_UNMAPPED_SAMPLE`] of those files, sorted.
    pub sample: Vec<String>,
}

/// The reflexion model, evaluated (module doc "Reflexion model").
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct Reflexion {
    /// Any allow or layer is declared: dependencies are judged.
    pub closed: bool,
    /// Sorted by name.
    pub components: Vec<ComponentSummary>,
    /// Sorted by `(from, to)`.
    pub matrix: Vec<MatrixCell>,
    /// Sorted by `(from, to, rule_id)`.
    pub absences: Vec<ReflexionAbsence>,
    pub unmapped: Unmapped,
    /// Convergent pairs.
    pub convergences: usize,
    /// Divergent pairs (each one [`Violation`]).
    pub divergences: usize,
}

/// The categories a forbid_edge rule without `categories` checks: IMPORTS
/// plus `CODE_PROFILE`'s carry edges, minus TESTS and DOCUMENTS.
pub fn default_forbid_categories() -> Vec<EdgeCategoryId> {
    let mut out = vec![edge_category::IMPORTS];
    for c in CODE_PROFILE.tables.carry_edges {
        if *c != edge_category::TESTS && *c != edge_category::DOCUMENTS && !out.contains(c) {
            out.push(*c);
        }
    }
    out
}

/// Evaluate every rule [`declared_constraints`] reads from `merged`.
pub fn check(merged: &MergedGraph) -> CheckReport {
    check_rules(merged, &declared_constraints(merged))
}

/// Evaluate `rules` (as [`declared_constraints`] returns them: anchor node,
/// rule) against `merged`, whichever graph they were read from: a review
/// checks the working tree's rules against the base graph. Emits the marker.
pub(crate) fn check_rules(merged: &MergedGraph, rules: &[(NodeId, ConstraintRule)]) -> CheckReport {
    check_rules_with(merged, rules, native_derive)
}

/// [`check_rules`] with the reflexion model's relations derived by `derive`
/// (DB.8a: the native derivation, or the rules layer's, held to the same
/// report byte for byte).
pub(crate) fn check_rules_with(
    merged: &MergedGraph,
    rules: &[(NodeId, ConstraintRule)],
    derive: DeriveModel,
) -> CheckReport {
    // Component and layer entries declare the model; every other entry, an
    // allow included, is a rule.
    let (decls, rules): (Vec<&ConstraintRule>, Vec<&ConstraintRule>) = rules
        .iter()
        .map(|(_, r)| r)
        .partition(|r| is_model_decl(&r.kind));
    let mut report = CheckReport {
        rules: rules.len(),
        checked: 0,
        unchecked: Vec::new(),
        errors: Vec::new(),
        violations: Vec::new(),
        reflexion: None,
    };
    let mut stats = RuleStats::default();
    if !rules.is_empty() || !decls.is_empty() {
        let mut ctx = Ctx::new(merged);
        let mut model = ModelEdb::declare(&decls);
        for rule in &rules {
            if let ConstraintKind::Allow { from, to } = &rule.kind {
                match model.admit_allow(rule, from, to) {
                    Ok(()) => report.checked += 1,
                    Err(msg) => report.errors.push((rule.id.clone(), msg)),
                }
                continue;
            }
            match evaluate(&mut ctx, rule) {
                Outcome::Unchecked => report.unchecked.push(rule.id.clone()),
                Outcome::Error(msg) => report.errors.push((rule.id.clone(), msg)),
                Outcome::Checked(mut found) => {
                    report.checked += 1;
                    if is_rule_invariant(rule) {
                        stats.invariant += 1;
                        stats.rows += found.len();
                    }
                    if is_transitive(rule) {
                        stats.transitive += 1;
                        stats.hits += found.len();
                    }
                    let min_tier = min_tier_of(rule);
                    for v in &mut found {
                        v.blocking = is_blocking(v.tier, min_tier);
                    }
                    report.violations.extend(found);
                }
            }
        }
        [stats.imports, stats.symbols, stats.mixed] = ctx.path_hits;
        report.errors.append(&mut model.errors);
        if !model.components.is_empty() {
            let checked = model.checked();
            match derive(&ctx.facts, merged, model, checked) {
                Ok(facts) => {
                    let (reflexion, mut divergences) = render_model(&ctx, facts);
                    // The model has no per-rule gate.
                    for v in &mut divergences {
                        v.blocking = is_blocking(v.tier, DEFAULT_MIN_TIER);
                    }
                    report.violations.extend(divergences);
                    report.reflexion = Some(reflexion);
                }
                Err(msg) => report.errors.push(("reflexion".to_string(), msg)),
            }
        }
    }
    // Stable: a rule's cycles keep their order.
    report.violations.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    let of = |k: &str| {
        report
            .violations
            .iter()
            .filter(|v| v.rule_kind == k)
            .count()
    };
    eprintln!(
        "[check] rules={} checked={} violations={} (forbid_edge={} no_cycle={}) unchecked={} errors={}",
        report.rules,
        report.checked,
        report.violations.len(),
        of(FORBID_EDGE),
        of(NO_CYCLE),
        report.unchecked.len(),
        report.errors.len(),
    );
    if !report.violations.is_empty() {
        let tier = |t: &str| report.violations.iter().filter(|v| v.tier == t).count();
        eprintln!(
            "[check] tiers fact={} derived={} heuristic={}",
            tier(FACT),
            tier(DERIVED),
            tier(HEURISTIC),
        );
        let blocking = report.violations.iter().filter(|v| v.blocking).count();
        eprintln!(
            "[check] gate blocking={} advisory={}",
            blocking,
            report.violations.len() - blocking,
        );
    }
    if stats.invariant + stats.transitive > 0 {
        eprintln!(
            "[rules] check invariant={} rows={} transitive={} hits={} (imports={} symbols={} mixed={})",
            stats.invariant,
            stats.rows,
            stats.transitive,
            stats.hits,
            stats.imports,
            stats.symbols,
            stats.mixed,
        );
    }
    report
}

/// What the `[rules] check` marker counts (module doc): the rule invariants
/// evaluated and their `violation` rows; the transitive forbid_edge rules
/// evaluated and their hits (one [`FORBID_PATH`] violation each), split by
/// the [`Chain`] that reported each hit (`imports + symbols + mixed = hits`).
#[derive(Default)]
struct RuleStats {
    invariant: usize,
    rows: usize,
    transitive: usize,
    hits: usize,
    imports: usize,
    symbols: usize,
    mixed: usize,
}

/// An invariant with Datalog `rule` text: evaluated by [`rule_invariant`].
fn is_rule_invariant(rule: &ConstraintRule) -> bool {
    matches!(rule.kind, ConstraintKind::Invariant { rule: Some(_), .. })
}

/// A forbid_edge with `transitive`: evaluated by [`forbid_path`].
fn is_transitive(rule: &ConstraintRule) -> bool {
    matches!(
        rule.kind,
        ConstraintKind::ForbidEdge {
            transitive: true,
            ..
        }
    )
}

/// The weakest tier that makes `rule`'s violations block: its `min_tier`
/// (`fact` | `derived`, validated at load by DB.1), else
/// [`DEFAULT_MIN_TIER`]. Any other spelling, possible only in a hand-edited
/// cell, reads as the default.
pub(crate) fn min_tier_of(rule: &ConstraintRule) -> &'static str {
    match rule.min_tier.as_deref() {
        Some(FACT) => FACT,
        Some(DERIVED) => DERIVED,
        _ => DEFAULT_MIN_TIER,
    }
}

/// Whether a violation of `tier` fails CI under `min_tier`: the tier is at
/// least as strong as `min_tier`, and it is never [`HEURISTIC`] (0.5.2 Q2:
/// a heuristic violation is reported, never blocking, whatever the gate).
pub(crate) fn is_blocking(tier: &str, min_tier: &str) -> bool {
    tier != HEURISTIC && tier_rank(tier) <= tier_rank(min_tier)
}

/// Sort rank of a tier, strongest first; an unknown spelling sorts last.
fn tier_rank(t: &str) -> u8 {
    match t {
        FACT => 0,
        DERIVED => 1,
        HEURISTIC => 2,
        _ => 3,
    }
}

enum Outcome {
    Unchecked,
    Error(String),
    Checked(Vec<Violation>),
}

fn evaluate(ctx: &mut Ctx<'_>, rule: &ConstraintRule) -> Outcome {
    // Only the kinds this module evaluates reach the category check: an
    // invariant's categories are never read. An invariant without a Datalog
    // `rule` is listed unchecked: prose has no query.
    match &rule.kind {
        ConstraintKind::ForbidEdge { .. } | ConstraintKind::NoCycle { .. } => {}
        ConstraintKind::Invariant {
            rule: Some(text), ..
        } => {
            return match rule_invariant(ctx, rule, text) {
                Ok(found) => Outcome::Checked(found),
                Err(e) => Outcome::Error(e),
            };
        }
        _ => return Outcome::Unchecked,
    }
    let categories = match categories_of(&rule.categories) {
        Ok(c) => c,
        Err(e) => return Outcome::Error(e),
    };
    let result = match &rule.kind {
        ConstraintKind::ForbidEdge {
            from,
            to,
            transitive,
            via,
        } => {
            let named = !categories.is_empty();
            let cats = if named {
                categories
            } else {
                default_forbid_categories()
            };
            if *transitive {
                forbid_path(ctx, rule, (from, to, via), &cats, named)
            } else {
                forbid_edge(ctx, rule, from, to, &cats).map(|v| v.into_iter().collect())
            }
        }
        ConstraintKind::NoCycle { scope } => no_cycle(ctx, rule, scope.as_deref(), &categories),
        _ => return Outcome::Unchecked,
    };
    match result {
        Ok(found) => Outcome::Checked(found),
        Err(e) => Outcome::Error(e),
    }
}

/// The rule's category names as ids; an unregistered name is an error that
/// names every one of them.
fn categories_of(names: &[String]) -> Result<Vec<EdgeCategoryId>, String> {
    let mut ids = Vec::with_capacity(names.len());
    let mut unknown = Vec::new();
    for n in names {
        match edge_category::ALL
            .iter()
            .find(|(_, name)| *name == n.as_str())
        {
            Some((id, _)) if !ids.contains(id) => ids.push(*id),
            Some(_) => {}
            None => unknown.push(format!("`{n}`")),
        }
    }
    if unknown.is_empty() {
        Ok(ids)
    } else {
        Err(format!(
            "unknown edge categor{} {} (the rule is skipped; name registered edge categories)",
            if unknown.len() == 1 { "y" } else { "ies" },
            unknown.join(", ")
        ))
    }
}

/// Per-call node facts: one [`CodeFacts`] (DB.2a) over the merged graph,
/// owning the check's one [`Locator`](crate::answers::Locator), which holds
/// every node's located file, the PROJECT paths and the members of each
/// scope a rule names (each computed once); a rule invariant evaluates over
/// the same facts.
struct Ctx<'a> {
    merged: &'a MergedGraph,
    facts: CodeFacts<'a>,
    qnames: HashMap<NodeId, String>,
    /// Transitive forbid_edge hits so far, by [`Chain`] (the `[rules]
    /// check` marker's `imports=` / `symbols=` / `mixed=`).
    path_hits: [usize; 3],
}

impl<'a> Ctx<'a> {
    fn new(merged: &'a MergedGraph) -> Self {
        Ctx {
            merged,
            facts: CodeFacts::new(merged),
            qnames: HashMap::new(),
            path_hits: [0; 3],
        }
    }

    /// The nodes strictly in `scope` (`CodeFacts::members`: literal, no label
    /// resolution), or the error that names the empty scope.
    fn scope(&self, scope: &str, role: &str) -> Result<Rc<HashSet<NodeId>>, String> {
        let set = self.facts.members(scope);
        if set.is_empty() {
            return Err(format!(
                "{role} scope `{scope}` matches no located node (no file sits under it and no PROJECT is at it)"
            ));
        }
        Ok(set)
    }

    fn qname(&mut self, id: NodeId) -> String {
        if let Some(q) = self.qnames.get(&id) {
            return q.clone();
        }
        let q = self.facts.locator().locate(id).qname;
        self.qnames.insert(id, q.clone());
        q
    }

    /// One located evidence row: at the edge's EVIDENCE site when it names a
    /// file, else at the `from` node (`rules::facts::located_edge`, the row
    /// the rules layer locates a witness edge as). `(tier, note)` is
    /// `why::tier_of` over the edge, computed by the caller that holds it.
    fn row(
        &self,
        from: NodeId,
        category: EdgeCategoryId,
        to: NodeId,
        ev: Option<&Evidence>,
        tier: (&'static str, Option<String>),
    ) -> ViolationEdge {
        located_edge(self.facts.locator(), from, category, to, ev, tier)
    }
}

/// A [`Violation`] of `(rule id, decl)`, its evidence capped at
/// [`MAX_EVIDENCE`]. `blocking` is stamped by [`check_rules`] from the rule's
/// `min_tier`; `subject` / `file` / `line` / `caveats` are a rule row's,
/// set by [`rule_invariant`].
fn violation(
    (rule_id, decl): (String, Option<String>),
    kind: &'static str,
    tier: &'static str,
    count: usize,
    mut evidence: Vec<ViolationEdge>,
) -> Violation {
    evidence.truncate(MAX_EVIDENCE);
    Violation {
        rule_id,
        rule_kind: kind,
        decl,
        severity: VIOLATION,
        tier,
        blocking: false,
        count,
        evidence,
        subject: Vec::new(),
        file: None,
        line: None,
        caveats: Vec::new(),
    }
}

/// `(id, decl)` of a rule, for [`violation`].
fn id_decl(rule: &ConstraintRule) -> (String, Option<String>) {
    (rule.id.clone(), rule.decl.clone())
}

// ============================================================================
// rule invariants (module doc "Rule invariants and min_tier")
// ============================================================================

/// `text`, an invariant's Datalog `rule`, evaluated through the rules layer
/// over the check's facts: one [`RULE`] [`Violation`] per `violation` row,
/// in rendered order, or the rendered error (`<id> (<decl>):<line>:<col>:
/// error: ...`) that skips the rule. A rule whose entry is not the overlay's
/// is an error too (module doc).
fn rule_invariant(
    ctx: &Ctx<'_>,
    rule: &ConstraintRule,
    text: &str,
) -> Result<Vec<Violation>, String> {
    if rule.source != RULE_SOURCE {
        return Err(format!(
            "a `rule` is evaluated only when declared in .glia/overlay.toml (0.5.2 Q9); this entry's source is `{}` (the rule is skipped)",
            rule.source
        ));
    }
    let source_name = format!(
        "{} ({})",
        rule.id,
        rule.decl.as_deref().unwrap_or("cell API")
    );
    let rows = evaluate_rule(ctx.merged, &ctx.facts, &source_name, text, RULE_OUTPUT)?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let mut v = violation(id_decl(rule), RULE, r.tier, r.witness_edges, r.witness);
            v.subject = r.subject;
            v.file = r.file;
            v.line = r.line;
            v.caveats = r.caveats;
            v
        })
        .collect())
}

/// Strongest tier first (so the [`MAX_EVIDENCE`] cut keeps the facts), then
/// located rows by (file, line, category); qnames break the rest.
fn sort_rows(rows: &mut [ViolationEdge]) {
    rows.sort_by(|a, b| {
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
    });
}

/// The dedup key of an edge as reported (an edge two graphs both hold is one
/// row).
pub(crate) fn edge_site(e: &Edge, ev: Option<&Evidence>) -> EdgeSite {
    (
        e.from,
        e.category.0,
        e.to,
        ev.and_then(|x| x.file.clone()),
        ev.and_then(|x| x.line),
    )
}

// ============================================================================
// forbid_edge
// ============================================================================

/// One forbidden edge as reported: `(from, category, to, file, 0-based line)`.
/// An edge two graphs both hold is one row.
pub(crate) type EdgeSite = (NodeId, u32, NodeId, Option<String>, Option<u32>);

fn forbid_edge(
    ctx: &mut Ctx<'_>,
    rule: &ConstraintRule,
    from: &str,
    to: &str,
    cats: &[EdgeCategoryId],
) -> Result<Option<Violation>, String> {
    let from_set = ctx.scope(from, "from")?;
    let to_set = ctx.scope(to, "to")?;
    let merged = ctx.merged;
    let mut seen: HashSet<EdgeSite> = HashSet::new();
    let mut rows: Vec<ViolationEdge> = Vec::new();
    for e in merged.all_edges() {
        if !cats.contains(&e.category) || !from_set.contains(&e.from) || !to_set.contains(&e.to) {
            continue;
        }
        let ev = Evidence::of(e);
        if !seen.insert(edge_site(e, ev.as_ref())) {
            continue;
        }
        let tier = tier_of(ev.as_ref(), e);
        rows.push(ctx.row(e.from, e.category, e.to, ev.as_ref(), tier));
    }
    if rows.is_empty() {
        return Ok(None);
    }
    sort_rows(&mut rows);
    // Sorted tier-first: the first row holds the strongest tier.
    let tier = rows.first().map_or(FACT, |r| r.tier);
    let count = rows.len();
    Ok(Some(violation(
        id_decl(rule),
        FORBID_EDGE,
        tier,
        count,
        rows,
    )))
}

// ============================================================================
// transitive forbid_edge (module doc "Transitive forbid_edge (Q3)")
// ============================================================================

/// The tiered passes of a transitive walk: FACT edges, + DERIVED, all
/// ([`Tier::level`] 0, 1, 2).
const LEVELS: u8 = 3;
/// An empty slot of a [`HopIndex`].
const NO_EDGE: u32 = u32::MAX;

/// Which walk reported a transitive hit. The order is the tie-break between
/// two chains hitting one node at one tier and depth: imports first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Chain {
    /// IMPORTS over the module import graph (`cycles::module_import_graph`).
    Imports = 0,
    /// The rule's other categories over the node graph.
    Symbols = 1,
    /// A rule naming both IMPORTS and CALLS: every category over the node
    /// graph, one walk.
    Mixed = 2,
}

/// A graph one [`Chain`] walks, with the `why` tier of each of its edges.
trait PathGraph: GraphSource {
    /// The edge's [`Tier::level`]: the first pass that may walk it.
    fn level(&self, e: &Edge) -> u8;
    /// Where the edge is asserted and its `why` tier and note, for its
    /// witness row.
    fn site(&self, e: &Edge) -> (Option<Evidence>, (&'static str, Option<String>));
}

/// The node graph: each edge tiered as the rules layer tiers it (DB.2a,
/// `why::tier_of`), so `glia query`'s `reaches` and `glia check` agree.
impl PathGraph for MergedGraph {
    fn level(&self, e: &Edge) -> u8 {
        edge_tier_of(e).level()
    }

    fn site(&self, e: &Edge) -> (Option<Evidence>, (&'static str, Option<String>)) {
        let ev = Evidence::of(e);
        let tier = tier_of(ev.as_ref(), e);
        (ev, tier)
    }
}

/// The module import graph: a lifted edge is tiered and located by its first
/// import's evidence, as no_cycle's import rules tier it (module doc
/// "Tiers").
impl PathGraph for ModuleImports {
    fn level(&self, e: &Edge) -> u8 {
        word_level(tier_of(self.evidence(e.from, e.to), e).0)
    }

    fn site(&self, e: &Edge) -> (Option<Evidence>, (&'static str, Option<String>)) {
        let ev = self.evidence(e.from, e.to);
        let tier = tier_of(ev, e);
        (ev.cloned(), tier)
    }
}

/// A `why` tier word as a level: `why`'s own fallback (DERIVED) for any other
/// word, as `rules::facts` reads it.
fn word_level(word: &str) -> u8 {
    match word {
        FACT => Tier::Fact.level(),
        HEURISTIC => Tier::Heuristic.level(),
        _ => Tier::Derived.level(),
    }
}

/// The scopes of one transitive rule: `to`, and every node of its `via`
/// facades.
struct PathScopes {
    to: Rc<HashSet<NodeId>>,
    via: HashSet<NodeId>,
}

impl PathScopes {
    /// A node the walk enters but never leaves: in `to` or in a facade.
    fn stops(&self, id: NodeId) -> bool {
        self.to.contains(&id) || self.via.contains(&id)
    }

    /// A node whose reach is a violation: in `to`, in no facade.
    fn is_hit(&self, id: NodeId) -> bool {
        self.to.contains(&id) && !self.via.contains(&id)
    }
}

/// `inner` without the edges that leave a stop node ([`PathScopes::stops`]):
/// a walk ends at the first node of `to` it meets, and never passes a facade.
struct Barred<'g, G: ?Sized> {
    inner: &'g G,
    scopes: &'g PathScopes,
}

impl<G: GraphSource + ?Sized> GraphSource for Barred<'_, G> {
    fn node_ids(&self) -> Vec<NodeId> {
        self.inner.node_ids()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        let scopes = self.scopes;
        Box::new(self.inner.edges().filter(move |e| !scopes.stops(e.from)))
    }
}

/// `(from, category, to)` -> the first kept edge (in edge order) of that key
/// at each level, or [`NO_EDGE`].
type HopIndex = HashMap<(NodeId, EdgeCategoryId, NodeId), [u32; LEVELS as usize]>;

/// One node of `to` a chain reached first in pass `level`, with its shortest
/// witness at that level (hops in path order, located).
struct PathHit {
    node: NodeId,
    level: u8,
    chain: Chain,
    hops: Vec<ViolationEdge>,
}

impl PathHit {
    /// The hit two chains disagree on keeps the stronger tier, then fewer
    /// hops, then imports first.
    fn rank(&self) -> (u8, usize, Chain) {
        (self.level, self.hops.len(), self.chain)
    }
}

/// A transitive forbid_edge (0.5.2 Q3): every node of `to` that a path from
/// `from` reaches, through any third scope, as its first node of `to`; a path
/// through a `via` facade is allowed. One [`FORBID_PATH`] [`Violation`] per
/// hit, sorted by subject (module doc "Transitive forbid_edge (Q3)").
fn forbid_path(
    ctx: &mut Ctx<'_>,
    rule: &ConstraintRule,
    (from, to, via): (&str, &str, &[String]),
    cats: &[EdgeCategoryId],
    named: bool,
) -> Result<Vec<Violation>, String> {
    let from_set = ctx.scope(from, "from")?;
    let to_set = ctx.scope(to, "to")?;
    let mut via_set: HashSet<NodeId> = HashSet::new();
    for v in via {
        via_set.extend(ctx.scope(v, "via")?.iter().copied());
    }
    let scopes = PathScopes {
        to: to_set,
        via: via_set,
    };
    // Sorted by rendered qname, then id: the walk order is checkout-stable.
    let starts: Vec<NodeId> = from_set
        .iter()
        .copied()
        .filter(|id| !scopes.stops(*id))
        .collect();
    let mut keyed: Vec<(String, u64)> =
        starts.into_iter().map(|id| (ctx.qname(id), id.0)).collect();
    keyed.sort_unstable();
    let seeds: Vec<NodeId> = keyed.into_iter().map(|(_, id)| NodeId(id)).collect();

    let merged = ctx.merged;
    let has = |c: EdgeCategoryId| cats.contains(&c);
    let mut found: Vec<PathHit> = Vec::new();
    if named && has(edge_category::IMPORTS) && has(edge_category::CALLS) {
        found.extend(walk_chain(ctx, merged, Chain::Mixed, cats, &seeds, &scopes));
    } else {
        if has(edge_category::IMPORTS) {
            let imports = module_import_graph(merged);
            found.extend(walk_chain(
                ctx,
                &imports,
                Chain::Imports,
                &[edge_category::IMPORTS],
                &seeds,
                &scopes,
            ));
        }
        let symbols: Vec<EdgeCategoryId> = cats
            .iter()
            .copied()
            .filter(|c| *c != edge_category::IMPORTS)
            .collect();
        if !symbols.is_empty() {
            found.extend(walk_chain(
                ctx,
                merged,
                Chain::Symbols,
                &symbols,
                &seeds,
                &scopes,
            ));
        }
    }

    // One violation per node.
    let mut best: HashMap<NodeId, PathHit> = HashMap::new();
    for h in found {
        match best.get(&h.node) {
            Some(held) if held.rank() <= h.rank() => {}
            _ => {
                best.insert(h.node, h);
            }
        }
    }
    let mut out: Vec<(u64, Violation)> = Vec::with_capacity(best.len());
    for h in best.into_values() {
        ctx.path_hits[h.chain as usize] += 1;
        let tier = tier_name(Tier::from_level(h.level).unwrap_or(Tier::Heuristic));
        let count = h.hops.len();
        let mut v = violation(id_decl(rule), FORBID_PATH, tier, count, h.hops);
        let at = ctx.facts.locator().locate(h.node);
        v.subject = vec![at.qname];
        v.file = at.file;
        v.line = at.line;
        out.push((h.node.0, v));
    }
    // A qname two merged repos share breaks its tie by location, then id.
    out.sort_by(|(a_id, a), (b_id, b)| {
        (&a.subject, &a.file, a.line, a_id).cmp(&(&b.subject, &b.file, b.line, b_id))
    });
    Ok(out.into_iter().map(|(_, v)| v).collect())
}

/// One chain of a transitive rule: the levelled multi-source BFS
/// (`activation::algo::reach::bfs_levels`, the walk the rules layer's
/// `reaches` runs) over `graph`'s `cats` edges, barred at the stop nodes, in
/// [`LEVELS`] passes; each reached node of `to` outside the facades is a hit
/// at its first pass's level. Its witness is a shortest path at that level
/// from a seed: one single-level walk per level that holds a hit, each depth
/// layer expanded in (qname, id) order so the parent is the lowest-ranked
/// node one hop up, and each hop the edge from that parent of the strongest
/// tier, then the smallest category name, then first in edge order.
fn walk_chain<G: PathGraph + ?Sized>(
    ctx: &mut Ctx<'_>,
    graph: &G,
    chain: Chain,
    cats: &[EdgeCategoryId],
    seeds: &[NodeId],
    scopes: &PathScopes,
) -> Vec<PathHit> {
    let barred = Barred {
        inner: graph,
        scopes,
    };
    let keep = CategorySet::of(cats);
    let adj = Adjacency::build(&barred, &keep);
    // The kept edges in index order (`Adjacency::build` keeps the global edge
    // order filtered by `keep`), each with its tier level.
    let kept: Vec<&Edge> = barred
        .edges()
        .filter(|e| keep.contains(e.category))
        .collect();
    let levels: Vec<u8> = kept.iter().map(|e| graph.level(e)).collect();
    let starts: Vec<(NodeId, u8)> = seeds.iter().map(|&s| (s, 0)).collect();
    let walk = bfs_levels(
        &adj,
        &starts,
        &levels,
        &LevelWalk::new(Walk::Forward, LEVELS),
    );
    let hits: Vec<&LevelReached> = walk
        .reached
        .iter()
        .filter(|r| scopes.is_hit(r.id))
        .collect();
    if hits.is_empty() {
        return Vec::new();
    }

    // Every node a witness can pass: the started seeds and the reached.
    let mut on = vec![false; adj.len()];
    let mut order: Vec<(String, u64, u32)> = Vec::new();
    for id in walk
        .seeds
        .iter()
        .map(|s| s.0)
        .chain(walk.reached.iter().map(|r| r.id))
    {
        if let Some(ix) = adj.index_of(id) {
            on[ix as usize] = true;
            order.push((ctx.qname(id), id.0, ix));
        }
    }
    order.sort_unstable();
    let mut rank = vec![u32::MAX; adj.len()];
    for (r, (_, _, ix)) in order.iter().enumerate() {
        rank[*ix as usize] = u32::try_from(r).unwrap_or(u32::MAX);
    }
    let walked = |id: NodeId| adj.index_of(id).is_some_and(|ix| on[ix as usize]);
    let mut index: HopIndex = HashMap::new();
    for (k, (e, lvl)) in kept.iter().zip(&levels).enumerate() {
        let slot = usize::from(*lvl);
        if slot >= usize::from(LEVELS) || !(walked(e.from) && walked(e.to)) {
            continue;
        }
        let first = &mut index
            .entry((e.from, e.category, e.to))
            .or_insert([NO_EDGE; LEVELS as usize])[slot];
        if *first == NO_EDGE {
            *first = u32::try_from(k).unwrap_or(NO_EDGE);
        }
    }
    let mut by_name: Vec<EdgeCategoryId> = cats.to_vec();
    by_name.sort_by_key(|c| (edge_category::name(*c), c.0));
    by_name.dedup();

    let mut out = Vec::with_capacity(hits.len());
    for level in 0..LEVELS {
        let at_level: Vec<NodeId> = hits
            .iter()
            .filter(|h| h.level == level)
            .map(|h| h.id)
            .collect();
        if at_level.is_empty() {
            continue;
        }
        // A shortest path at one level is a one-level walk over the edges of
        // that level or stronger (`bfs_levels`' doc).
        let one: Vec<u8> = levels
            .iter()
            .map(|&l| if l <= level { 0 } else { u8::MAX })
            .collect();
        let mut how = LevelWalk::new(Walk::Forward, 1);
        how.parent_rank = Some(&rank);
        let ranked = bfs_levels(&adj, &starts, &one, &how);
        let mut record = vec![usize::MAX; adj.len()];
        for (i, r) in ranked.reached.iter().enumerate() {
            if let Some(ix) = adj.index_of(r.id) {
                record[ix as usize] = i;
            }
        }
        let reached_at = |id: NodeId| {
            adj.index_of(id)
                .and_then(|ix| ranked.reached.get(record[ix as usize]))
        };
        for node in at_level {
            let mut path: Vec<&Edge> = Vec::new();
            let mut at = node;
            // Each parent sits one layer up, so the chain ends at depth 1,
            // on a seed.
            while let Some(r) = reached_at(at) {
                let hop = pick_hop(&kept, &index, &by_name, r.parent, at, level)
                    .or_else(|| kept.get(r.kept_edge as usize).copied());
                let Some(e) = hop else { break };
                path.push(e);
                if r.depth <= 1 || path.len() > adj.len() {
                    break;
                }
                at = r.parent;
            }
            path.reverse();
            let hops = path
                .into_iter()
                .map(|e| {
                    let (ev, tier) = graph.site(e);
                    ctx.row(e.from, e.category, e.to, ev.as_ref(), tier)
                })
                .collect();
            out.push(PathHit {
                node,
                level,
                chain,
                hops,
            });
        }
    }
    out
}

/// The witness hop `from -> to` walkable at `level`: the strongest tier,
/// then the smallest category name (`by_name` order), then the first in edge
/// order, as the rules layer's closure picks a hop between parallel edges.
fn pick_hop<'e>(
    kept: &[&'e Edge],
    index: &HopIndex,
    by_name: &[EdgeCategoryId],
    from: NodeId,
    to: NodeId,
    level: u8,
) -> Option<&'e Edge> {
    for slot in 0..=usize::from(level).min(usize::from(LEVELS) - 1) {
        for c in by_name {
            if let Some(k) = index.get(&(from, *c, to)).map(|s| s[slot])
                && k != NO_EDGE
            {
                return kept.get(k as usize).copied();
            }
        }
    }
    None
}

// ============================================================================
// no_cycle
// ============================================================================

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

/// Where a hop of a sub-graph edge was asserted, and its `why` tier and note,
/// computed from the edge the hop stands for when it is recorded.
type Sites = BTreeMap<(u64, u32, u64), (Option<Evidence>, &'static str, Option<String>)>;

fn no_cycle(
    ctx: &mut Ctx<'_>,
    rule: &ConstraintRule,
    scope: Option<&str>,
    cats: &[EdgeCategoryId],
) -> Result<Vec<Violation>, String> {
    let keep: Option<Rc<HashSet<NodeId>>> = match scope {
        Some(s) => Some(ctx.scope(s, "no_cycle")?),
        None => None,
    };
    let inside = |id: &NodeId| keep.as_ref().is_none_or(|k| k.contains(id));
    let imports_only = cats.is_empty() || cats == [edge_category::IMPORTS];

    let mut sub = Sub {
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    let mut sites: Sites = BTreeMap::new();
    let mut node_seen: HashSet<NodeId> = HashSet::new();
    let mut add_node = |sub: &mut Sub, id: NodeId| {
        if node_seen.insert(id) {
            sub.nodes.push(id);
        }
    };
    if imports_only {
        let imports = module_import_graph(ctx.merged);
        for e in imports.edges() {
            if !(inside(&e.from) && inside(&e.to)) {
                continue;
            }
            add_node(&mut sub, e.from);
            add_node(&mut sub, e.to);
            sub.edges
                .push(Edge::new(e.from, e.to, e.category, e.confidence));
            // The lifted edge carries the first import's evidence and
            // confidence, not its ORIGIN cell (module doc "Tiers").
            sites
                .entry((e.from.0, e.category.0, e.to.0))
                .or_insert_with(|| {
                    let ev = imports.evidence(e.from, e.to);
                    let (tier, note) = tier_of(ev, e);
                    (ev.cloned(), tier, note)
                });
        }
    } else {
        for e in ctx.merged.all_edges() {
            if !(cats.contains(&e.category) && inside(&e.from) && inside(&e.to)) {
                continue;
            }
            add_node(&mut sub, e.from);
            add_node(&mut sub, e.to);
            sub.edges
                .push(Edge::new(e.from, e.to, e.category, e.confidence));
            sites
                .entry((e.from.0, e.category.0, e.to.0))
                .or_insert_with(|| {
                    let ev = Evidence::of(e);
                    let (tier, note) = tier_of(ev.as_ref(), e);
                    (ev, tier, note)
                });
        }
    }

    let adj = Adjacency::build(&sub, &CategorySet::all());
    let mut cycles: Vec<(String, Violation)> = Vec::new();
    for comp in strongly_connected(&adj) {
        // The member first in qname order starts the witness, so the same
        // code checked out elsewhere (other node ids) gives the same cycle.
        let mut by_qname: Vec<(String, NodeId)> =
            comp.iter().map(|id| (ctx.qname(*id), *id)).collect();
        by_qname.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
        let Some((first, start)) = by_qname.first().cloned() else {
            continue;
        };
        let evidence: Vec<ViolationEdge> = witness_cycle(&adj, &comp, start)
            .into_iter()
            .map(|(f, c, t)| {
                // Every witness hop is a sub edge, so it has a site; the
                // fallback only keeps a missing one honest.
                let (ev, tier) = match sites.get(&(f.0, c.0, t.0)) {
                    Some((ev, tier, note)) => (ev.as_ref(), (*tier, note.clone())),
                    None => (None, (DERIVED, Some("no evidence recorded".to_string()))),
                };
                ctx.row(f, c, t, ev, tier)
            })
            .collect();
        // Every hop is needed for the cycle: its weakest hop bounds it.
        let tier = if evidence.iter().any(|e| e.tier == HEURISTIC) {
            HEURISTIC
        } else {
            DERIVED
        };
        cycles.push((
            first,
            violation(id_decl(rule), NO_CYCLE, tier, comp.len(), evidence),
        ));
    }
    cycles.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(cycles.into_iter().map(|(_, v)| v).collect())
}

// ============================================================================
// reflexion model (module doc "Reflexion model")
// ============================================================================

/// A component or layer entry: a model declaration, not a rule.
fn is_model_decl(kind: &ConstraintKind) -> bool {
    matches!(
        kind,
        ConstraintKind::Component { .. } | ConstraintKind::Layer { .. }
    )
}

/// A path as membership compares it: no leading `./` or `/`, no trailing
/// `/`; the root is `.`.
fn norm_path(p: &str) -> String {
    let s = p
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_end_matches('/');
    if s.is_empty() {
        ".".to_string()
    } else {
        s.to_string()
    }
}

/// One declared component: every entry that names it, merged.
pub(crate) struct ComponentDecl {
    pub(crate) name: String,
    /// Stored paths, deduplicated, in entry order.
    pub(crate) paths: Vec<String>,
    /// The first entry's decl and id.
    pub(crate) decl: Option<String>,
    id: String,
}

/// `layer_of(C, Rank, Strict)`, with the layer's name for `allowed_by`.
pub(crate) struct LayerFact {
    pub(crate) name: String,
    pub(crate) rank: u32,
    pub(crate) strict: bool,
}

/// The allows of one component pair, `(id, decl)`, in rule order.
pub(crate) type Allows = Vec<(String, Option<String>)>;

/// The model's base relations (EDB), read from the stored entries. A
/// component is its index in `components` (sorted by name), so every
/// index-keyed collection iterates in name order.
pub(crate) struct ModelEdb {
    pub(crate) components: Vec<ComponentDecl>,
    index: BTreeMap<String, usize>,
    /// `component_path(C, Path)`: each normalised path with its owner.
    pub(crate) component_path: BTreeMap<String, usize>,
    /// `layer_of(C, Rank, Strict)`.
    pub(crate) layer_of: BTreeMap<usize, LayerFact>,
    /// `allow(C1, C2)`: the admitted allows.
    pub(crate) allow: BTreeMap<(usize, usize), Allows>,
    /// The union of the admitted allows' `categories`.
    pub(crate) allow_categories: Vec<EdgeCategoryId>,
    /// `(entry id, message)` of the declarations in error.
    errors: Vec<(String, String)>,
}

impl ModelEdb {
    /// `component_path` and `layer_of` from the component and layer entries
    /// (in [`declared_constraints`] order). Entries naming one component merge
    /// their paths. An equal path two components claim (possible only through
    /// the cell API: the loader and the build reject it in an overlay) goes to
    /// the smaller name, an error on the other. A layer entry naming an
    /// undeclared component, or one another layer holds, leaves it out, an
    /// error on the layer.
    pub(crate) fn declare(decls: &[&ConstraintRule]) -> Self {
        let mut by_name: BTreeMap<String, ComponentDecl> = BTreeMap::new();
        let mut layers: Vec<(&ConstraintRule, &str, u32, &[String], bool)> = Vec::new();
        for r in decls {
            match &r.kind {
                ConstraintKind::Component { name, paths } => {
                    let c = by_name
                        .entry(name.clone())
                        .or_insert_with(|| ComponentDecl {
                            name: name.clone(),
                            paths: Vec::new(),
                            decl: r.decl.clone(),
                            id: r.id.clone(),
                        });
                    for p in paths {
                        if !c.paths.contains(p) {
                            c.paths.push(p.clone());
                        }
                    }
                }
                ConstraintKind::Layer {
                    name,
                    rank,
                    components,
                    strict,
                } => layers.push((r, name, *rank, components, *strict)),
                _ => {}
            }
        }
        let components: Vec<ComponentDecl> = by_name.into_values().collect();
        let index: BTreeMap<String, usize> = components
            .iter()
            .enumerate()
            .map(|(i, c)| (c.name.clone(), i))
            .collect();
        let mut errors = Vec::new();

        // Name order: the first claimant of a path is the smaller name.
        let mut component_path: BTreeMap<String, usize> = BTreeMap::new();
        for (i, c) in components.iter().enumerate() {
            for p in &c.paths {
                match component_path.get(&norm_path(p)) {
                    Some(&owner) if owner != i => errors.push((
                        c.id.clone(),
                        format!(
                            "path `{p}` is also claimed by component `{}`, which keeps it (an equal path goes to the smaller name)",
                            components[owner].name
                        ),
                    )),
                    Some(_) => {}
                    None => {
                        component_path.insert(norm_path(p), i);
                    }
                }
            }
        }

        // Top first; a layer's name breaks a rank tie.
        layers.sort_by(|a, b| (a.2, a.1).cmp(&(b.2, b.1)));
        let mut layer_of: BTreeMap<usize, LayerFact> = BTreeMap::new();
        for (r, name, rank, members, strict) in layers {
            for c in members {
                let Some(&i) = index.get(c) else {
                    errors.push((
                        r.id.clone(),
                        format!(
                            "component `{c}` is not declared (rejected or orphaned at build, or never written); the layer leaves it out"
                        ),
                    ));
                    continue;
                };
                match layer_of.get(&i) {
                    Some(held) if held.name != name => errors.push((
                        r.id.clone(),
                        format!(
                            "component `{c}` already sits in layer `{}`; this layer leaves it out",
                            held.name
                        ),
                    )),
                    Some(_) => {}
                    None => {
                        layer_of.insert(
                            i,
                            LayerFact {
                                name: name.to_string(),
                                rank,
                                strict,
                            },
                        );
                    }
                }
            }
        }
        ModelEdb {
            components,
            index,
            component_path,
            layer_of,
            allow: BTreeMap::new(),
            allow_categories: Vec::new(),
            errors,
        }
    }

    /// Admit one allow into `allow(C1, C2)`, or the rule error that skips
    /// it: a `from` / `to` no component entry declares, a component allowed
    /// to use itself, an unknown edge category.
    pub(crate) fn admit_allow(
        &mut self,
        rule: &ConstraintRule,
        from: &str,
        to: &str,
    ) -> Result<(), String> {
        let (Some(&c1), Some(&c2)) = (self.index.get(from), self.index.get(to)) else {
            let mut unknown: Vec<String> = [from, to]
                .iter()
                .filter(|c| !self.index.contains_key(**c))
                .map(|c| format!("`{c}`"))
                .collect();
            unknown.dedup();
            return Err(format!(
                "allow names undeclared component{} {} (rejected or orphaned at build, or never written); the allow is skipped",
                if unknown.len() == 1 { "" } else { "s" },
                unknown.join(", ")
            ));
        };
        if c1 == c2 {
            return Err(format!(
                "allow lets component `{from}` use itself; a use within one component is no dependency (the allow is skipped)"
            ));
        }
        for c in categories_of(&rule.categories)? {
            if !self.allow_categories.contains(&c) {
                self.allow_categories.push(c);
            }
        }
        // One allow stored in two merged repos is one allow.
        let ids = self.allow.entry((c1, c2)).or_default();
        if !ids.iter().any(|(id, _)| *id == rule.id) {
            ids.push((rule.id.clone(), rule.decl.clone()));
        }
        Ok(())
    }

    /// The categories `edge(From, Category, To, E)` ranges over: the union of
    /// the admitted allows' `categories` when any sets them, else
    /// [`default_forbid_categories`].
    pub(crate) fn checked(&self) -> Vec<EdgeCategoryId> {
        if self.allow_categories.is_empty() {
            default_forbid_categories()
        } else {
            self.allow_categories.clone()
        }
    }
}

/// One distinct `dep` edge, with its `why` tier and note.
pub(crate) struct DepEdge<'m> {
    pub(crate) edge: &'m Edge,
    pub(crate) ev: Option<Evidence>,
    pub(crate) tier: &'static str,
    pub(crate) note: Option<String>,
}

/// The model as named relations (module doc "Reflexion model"): the EDB
/// ([`ModelEdb`]'s, moved in, plus `located` = `CodeFacts::files` and `edge` =
/// the `checked` categories) and every IDB relation, one field each.
/// Components are [`ModelEdb::components`] indices. Two derivations fill it,
/// the native [`ReflexionFacts::derive`] and the rules layer's
/// `rules::reflexion::derive` (engine/src/rules/reflexion.dl, DB.8a), and
/// [`render_model`] reads it, whichever filled it.
pub(crate) struct ReflexionFacts<'m> {
    pub(crate) components: Vec<ComponentDecl>,
    pub(crate) component_path: BTreeMap<String, usize>,
    pub(crate) layer_of: BTreeMap<usize, LayerFact>,
    pub(crate) allow: BTreeMap<(usize, usize), Allows>,
    /// The categories `edge(From, Category, To, E)` ranges over.
    pub(crate) checked: Vec<EdgeCategoryId>,
    /// `in_component(N, C)`.
    pub(crate) in_component: HashMap<NodeId, usize>,
    /// `unmapped(N, F)`.
    pub(crate) unmapped: HashMap<NodeId, String>,
    /// `dep(C1, C2, E)`: each pair's distinct edges, in `all_edges` order.
    pub(crate) dep: BTreeMap<(usize, usize), Vec<DepEdge<'m>>>,
    /// Distinct checked edges between an `unmapped` node and an
    /// `in_component` one, either direction.
    pub(crate) unmapped_edges: usize,
    /// `allowed(C1, C2)`, with what allows it.
    pub(crate) allowed: BTreeMap<(usize, usize), String>,
    /// `closed()`.
    pub(crate) closed: bool,
    /// `convergence(C1, C2)`.
    pub(crate) convergence: BTreeSet<(usize, usize)>,
    /// `divergence(C1, C2, E)`: the pairs; their edges are `dep`'s.
    pub(crate) divergence: BTreeSet<(usize, usize)>,
    /// `absence(C1, C2)`.
    pub(crate) absence: BTreeSet<(usize, usize)>,
}

/// A derivation of the model's relations from its EDB over the `checked`
/// categories ([`ModelEdb::checked`]): [`native_derive`], or the rules
/// layer's `rules::reflexion::derive`, the two DB.8a holds to byte-identical
/// reports. An `Err` is a [`CheckReport::errors`] row `("reflexion", msg)`.
pub(crate) type DeriveModel = for<'f, 'm> fn(
    &'f CodeFacts<'m>,
    &'m MergedGraph,
    ModelEdb,
    Vec<EdgeCategoryId>,
) -> Result<ReflexionFacts<'m>, String>;

/// [`ReflexionFacts::derive`] as a [`DeriveModel`]: it never fails.
pub(crate) fn native_derive<'m>(
    facts: &CodeFacts<'m>,
    merged: &'m MergedGraph,
    edb: ModelEdb,
    checked: Vec<EdgeCategoryId>,
) -> Result<ReflexionFacts<'m>, String> {
    Ok(ReflexionFacts::derive(facts, merged, edb, checked))
}

impl<'m> ReflexionFacts<'m> {
    /// Derive every IDB relation from `edb`, stratum by stratum: membership,
    /// then the edge pass, then the verdicts (each negation reads a relation
    /// already complete).
    fn derive(
        facts: &CodeFacts<'m>,
        merged: &'m MergedGraph,
        edb: ModelEdb,
        checked: Vec<EdgeCategoryId>,
    ) -> Self {
        let mut f = ReflexionFacts {
            components: edb.components,
            component_path: edb.component_path,
            layer_of: edb.layer_of,
            allow: edb.allow,
            checked,
            in_component: HashMap::new(),
            unmapped: HashMap::new(),
            dep: BTreeMap::new(),
            unmapped_edges: 0,
            allowed: BTreeMap::new(),
            closed: false,
            convergence: BTreeSet::new(),
            divergence: BTreeSet::new(),
            absence: BTreeSet::new(),
        };
        f.in_component = in_component_rel(facts, &f.component_path);
        f.unmapped = unmapped_rel(facts, &f.in_component);
        (f.dep, f.unmapped_edges) = dep_rel(merged, &f.checked, &f.in_component, &f.unmapped);
        f.allowed = allowed_rel(&f.allow, &f.layer_of);
        f.closed = closed_rel(&f.allow, &f.layer_of);
        f.convergence = convergence_rel(&f.dep, &f.allowed);
        f.divergence = divergence_rel(f.closed, &f.dep, &f.allowed);
        f.absence = absence_rel(&f.allow, &f.dep);
        f
    }
}

/// `in_component(N, C) :- located(N, F), component_path(C, P), under(F, P),
/// not shadowed(F, P).` with `shadowed(F, P) :- component_path(_, P2),
/// under(F, P2), longer(P2, P).`: the component of the longest declared
/// path at or above the node's file ([`owner`]). A PROJECT is placed by its
/// own path, as scope membership places it.
fn in_component_rel(
    facts: &CodeFacts<'_>,
    component_path: &BTreeMap<String, usize>,
) -> HashMap<NodeId, usize> {
    let mut out = HashMap::new();
    let projects = facts.projects();
    for (id, file) in facts.files() {
        let place = projects.get(id).or(file.as_ref());
        if let Some(c) = place.and_then(|p| owner(component_path, p)) {
            out.insert(*id, c);
        }
    }
    out
}

/// The unshadowed `component_path` above `path`: walk its path-segment
/// prefixes longest first (`web/admin/panel.py`, `web/admin`, `web`), then
/// the root `.`. One lookup per segment, no scan of the declared paths.
fn owner(component_path: &BTreeMap<String, usize>, path: &str) -> Option<usize> {
    let mut p = norm_path(path);
    loop {
        if let Some(c) = component_path.get(&p) {
            return Some(*c);
        }
        match p.rfind('/') {
            Some(i) => p.truncate(i),
            None => break,
        }
    }
    if p == "." {
        None
    } else {
        component_path.get(".").copied()
    }
}

/// `unmapped(N, F) :- located(N, F), not in_component(N, _).`
fn unmapped_rel(
    facts: &CodeFacts<'_>,
    in_component: &HashMap<NodeId, usize>,
) -> HashMap<NodeId, String> {
    facts
        .files()
        .iter()
        .filter(|(id, _)| !in_component.contains_key(id))
        .filter_map(|(id, f)| f.clone().map(|f| (*id, f)))
        .collect()
}

/// `dep(C1, C2, E) :- edge(A, _, B, E), in_component(A, C1),
/// in_component(B, C2), C1 != C2.` One pass over `all_edges`; an edge two
/// graphs both hold is one `E` (keyed as forbid_edge keys its rows). The same
/// pass counts the distinct checked edges between an `unmapped` node and an
/// `in_component` one.
fn dep_rel<'m>(
    merged: &'m MergedGraph,
    checked: &[EdgeCategoryId],
    in_component: &HashMap<NodeId, usize>,
    unmapped: &HashMap<NodeId, String>,
) -> (BTreeMap<(usize, usize), Vec<DepEdge<'m>>>, usize) {
    let mut dep: BTreeMap<(usize, usize), Vec<DepEdge<'m>>> = BTreeMap::new();
    let mut seen: HashSet<EdgeSite> = HashSet::new();
    let mut unmapped_edges = 0;
    for e in merged.all_edges() {
        if !checked.contains(&e.category) {
            continue;
        }
        let (a, b) = (in_component.get(&e.from), in_component.get(&e.to));
        let pair = match (a, b) {
            (Some(&c1), Some(&c2)) if c1 != c2 => Some((c1, c2)),
            (Some(_), None) if unmapped.contains_key(&e.to) => None,
            (None, Some(_)) if unmapped.contains_key(&e.from) => None,
            _ => continue,
        };
        let ev = Evidence::of(e);
        if !seen.insert(edge_site(e, ev.as_ref())) {
            continue;
        }
        let Some(pair) = pair else {
            unmapped_edges += 1;
            continue;
        };
        let (tier, note) = tier_of(ev.as_ref(), e);
        dep.entry(pair).or_default().push(DepEdge {
            edge: e,
            ev,
            tier,
            note,
        });
    }
    (dep, unmapped_edges)
}

/// `allowed(C1, C2) :- allow(C1, C2).`
/// `allowed(C1, C2) :- layer_of(C1, R1, false), layer_of(C2, R2, _), R1 < R2.`
/// `allowed(C1, C2) :- layer_of(C1, R1, true), layer_of(C2, R2, _), R2 = R1 + 1.`
/// Each allowed pair with what allows it: its smallest allow id, else
/// `layer:<upper>><lower>` (an explicit allow names the decision better).
fn allowed_rel(
    allow: &BTreeMap<(usize, usize), Allows>,
    layer_of: &BTreeMap<usize, LayerFact>,
) -> BTreeMap<(usize, usize), String> {
    let mut out = BTreeMap::new();
    for (&c1, l1) in layer_of {
        for (&c2, l2) in layer_of {
            let below = if l1.strict {
                l1.rank.checked_add(1) == Some(l2.rank)
            } else {
                l1.rank < l2.rank
            };
            if below {
                out.insert((c1, c2), format!("layer:{}>{}", l1.name, l2.name));
            }
        }
    }
    for (pair, ids) in allow {
        if let Some((id, _)) = ids.iter().min_by(|a, b| a.0.cmp(&b.0)) {
            out.insert(*pair, id.clone());
        }
    }
    out
}

/// `closed() :- allow(_, _).` `closed() :- layer_of(_, _, _).`
fn closed_rel(
    allow: &BTreeMap<(usize, usize), Allows>,
    layer_of: &BTreeMap<usize, LayerFact>,
) -> bool {
    !allow.is_empty() || !layer_of.is_empty()
}

/// `convergence(C1, C2) :- dep(C1, C2, _), allowed(C1, C2).`
fn convergence_rel(
    dep: &BTreeMap<(usize, usize), Vec<DepEdge<'_>>>,
    allowed: &BTreeMap<(usize, usize), String>,
) -> BTreeSet<(usize, usize)> {
    dep.keys()
        .filter(|p| allowed.contains_key(p))
        .copied()
        .collect()
}

/// `divergence(C1, C2, E) :- closed(), dep(C1, C2, E), not allowed(C1, C2).`
fn divergence_rel(
    closed: bool,
    dep: &BTreeMap<(usize, usize), Vec<DepEdge<'_>>>,
    allowed: &BTreeMap<(usize, usize), String>,
) -> BTreeSet<(usize, usize)> {
    if !closed {
        return BTreeSet::new();
    }
    dep.keys()
        .filter(|p| !allowed.contains_key(p))
        .copied()
        .collect()
}

/// `absence(C1, C2) :- allow(C1, C2), not dep(C1, C2, _).`
fn absence_rel(
    allow: &BTreeMap<(usize, usize), Allows>,
    dep: &BTreeMap<(usize, usize), Vec<DepEdge<'_>>>,
) -> BTreeSet<(usize, usize)> {
    allow
        .keys()
        .filter(|p| !dep.contains_key(p))
        .copied()
        .collect()
}

/// Render the derived model ([`ReflexionFacts`], from either derivation):
/// the report and one [`DIVERGENCE`] [`Violation`] per divergent pair. Emits
/// the `[reflexion]` marker.
fn render_model(ctx: &Ctx<'_>, facts: ReflexionFacts<'_>) -> (Reflexion, Vec<Violation>) {
    let merged = ctx.merged;
    let name = |c: usize| facts.components[c].name.clone();

    let mut owned = vec![0usize; facts.components.len()];
    for c in facts.in_component.values() {
        owned[*c] += 1;
    }
    let components: Vec<ComponentSummary> = facts
        .components
        .iter()
        .enumerate()
        .map(|(i, c)| ComponentSummary {
            name: c.name.clone(),
            paths: c.paths.clone(),
            layer: facts.layer_of.get(&i).map(|l| l.name.clone()),
            nodes: owned[i],
            decl: c.decl.clone(),
        })
        .collect();

    let strongest = |edges: &[DepEdge<'_>]| {
        edges
            .iter()
            .map(|d| d.tier)
            .min_by_key(|t| tier_rank(t))
            .unwrap_or(FACT)
    };
    let matrix: Vec<MatrixCell> = facts
        .dep
        .iter()
        .map(|(pair, edges)| MatrixCell {
            from: name(pair.0),
            to: name(pair.1),
            edges: edges.len(),
            status: if !facts.closed {
                OBSERVED
            } else if facts.convergence.contains(pair) {
                CONVERGENCE
            } else {
                DIVERGENCE
            },
            tier: strongest(edges),
            allowed_by: facts.allowed.get(pair).cloned(),
        })
        .collect();

    let mut divergences = Vec::with_capacity(facts.divergence.len());
    for pair in &facts.divergence {
        let Some(edges) = facts.dep.get(pair) else {
            continue;
        };
        let mut rows: Vec<ViolationEdge> = edges
            .iter()
            .map(|d| {
                let e = d.edge;
                ctx.row(
                    e.from,
                    e.category,
                    e.to,
                    d.ev.as_ref(),
                    (d.tier, d.note.clone()),
                )
            })
            .collect();
        sort_rows(&mut rows);
        let tier = rows.first().map_or(FACT, |r| r.tier);
        let id = format!("reflexion:{}->{}", name(pair.0), name(pair.1));
        let decl = facts.components[pair.0].decl.clone();
        divergences.push(violation((id, decl), DIVERGENCE, tier, edges.len(), rows));
    }

    let caveats = if facts.absence.is_empty() {
        Vec::new()
    } else {
        let names: Vec<&str> = facts
            .checked
            .iter()
            .map(|c| edge_category::name(*c))
            .collect();
        caveats_for(merged, &names, None)
    };
    let mut absences: Vec<ReflexionAbsence> = Vec::new();
    for pair in &facts.absence {
        let mut ids: Vec<&(String, Option<String>)> =
            facts.allow.get(pair).into_iter().flatten().collect();
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, decl) in ids {
            absences.push(ReflexionAbsence {
                from: name(pair.0),
                to: name(pair.1),
                rule_id: id.clone(),
                decl: decl.clone(),
                tier: FACT,
                caveats: caveats.clone(),
            });
        }
    }

    let unmapped = unmapped_summary(merged, &facts);
    let reflexion = Reflexion {
        closed: facts.closed,
        components,
        matrix,
        absences,
        unmapped,
        convergences: facts.convergence.len(),
        divergences: facts.divergence.len(),
    };
    eprintln!(
        "[reflexion] components={} closed={} deps={} convergences={} divergences={} absences={} unmapped_files={}",
        reflexion.components.len(),
        reflexion.closed,
        facts.dep.len(),
        reflexion.convergences,
        reflexion.divergences,
        reflexion.absences.len(),
        reflexion.unmapped.files,
    );
    (reflexion, divergences)
}

/// [`Unmapped`] over `unmapped(N, F)`: the files and nodes of its MODULE /
/// CLASS / FUNCTION / METHOD members (one node read once), and the edges the
/// edge pass counted.
fn unmapped_summary(merged: &MergedGraph, facts: &ReflexionFacts<'_>) -> Unmapped {
    const CODE: [NodeKindId; 4] = [
        node_kind::MODULE,
        node_kind::CLASS,
        node_kind::FUNCTION,
        node_kind::METHOD,
    ];
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut files: BTreeSet<&str> = BTreeSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let Some(file) = facts.unmapped.get(&n.id) else {
                continue;
            };
            let code = g
                .nav
                .kind_by_id
                .get(&n.id)
                .is_some_and(|k| CODE.contains(k));
            if code && seen.insert(n.id) {
                files.insert(file);
            }
        }
    }
    Unmapped {
        files: files.len(),
        nodes: seen.len(),
        edges_to_mapped: facts.unmapped_edges,
        sample: files
            .into_iter()
            .take(MAX_UNMAPPED_SAMPLE)
            .map(String::from)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_forbid_set_is_imports_plus_carry_minus_tests_and_docs() {
        let d = default_forbid_categories();
        assert_eq!(d.first(), Some(&edge_category::IMPORTS));
        assert!(d.contains(&edge_category::CALLS));
        assert!(d.contains(&edge_category::HTTP_CALLS));
        assert!(!d.contains(&edge_category::TESTS));
        assert!(!d.contains(&edge_category::DOCUMENTS));
        assert!(
            !d.contains(&edge_category::CONTAINS),
            "structure is no dependency"
        );
        let mut sorted = d.clone();
        sorted.sort_by_key(|c| c.0);
        sorted.dedup();
        assert_eq!(sorted.len(), d.len(), "no category twice");
    }

    #[test]
    fn categories_of_maps_names_and_reports_every_unknown() {
        assert_eq!(
            categories_of(&["CALLS".into(), "IMPORTS".into(), "CALLS".into()]),
            Ok(vec![edge_category::CALLS, edge_category::IMPORTS])
        );
        assert_eq!(categories_of(&[]), Ok(Vec::new()));
        let err = categories_of(&["CALLS".into(), "NOPE".into(), "GONE".into()])
            .expect_err("unknown names");
        assert!(
            err.contains("`NOPE`") && err.contains("`GONE`") && err.contains("categories"),
            "{err}"
        );
        let one = categories_of(&["NOPE".into()]).expect_err("unknown name");
        assert!(one.contains("category `NOPE`"), "{one}");
    }

    #[test]
    fn tier_rank_orders_strongest_first() {
        let mut t = vec![HEURISTIC, "other", FACT, DERIVED];
        t.sort_by_key(|x| tier_rank(x));
        assert_eq!(t, [FACT, DERIVED, HEURISTIC, "other"]);
    }

    /// The one rule a CONSTRAINT payload of one entry holds.
    fn one_rule(entry: &str) -> ConstraintRule {
        let payload = glia_core::CellPayload::Json(format!("[{entry}]"));
        glia_code_domain::external_inputs::parse_constraints(&payload)
            .pop()
            .expect("a well-formed entry")
    }

    #[test]
    fn heuristic_never_blocks_and_min_tier_gates_the_rest() {
        for min in [FACT, DERIVED] {
            assert!(!is_blocking(HEURISTIC, min), "Q2: {min}");
            assert!(is_blocking(FACT, min), "a fact always blocks: {min}");
        }
        assert!(is_blocking(DERIVED, DERIVED));
        assert!(
            !is_blocking(DERIVED, FACT),
            "min_tier = fact: derived is advisory"
        );
        assert!(
            !is_blocking(HEURISTIC, HEURISTIC),
            "no gate admits a heuristic"
        );
        assert!(
            !is_blocking("other", DERIVED),
            "an unknown tier is never stronger"
        );
    }

    #[test]
    fn min_tier_reads_the_rule_or_the_default() {
        let mut r = one_rule(r#"{"id":"r","kind":"no_cycle","source":"overlay"}"#);
        assert_eq!(r.min_tier, None);
        assert_eq!(min_tier_of(&r), DEFAULT_MIN_TIER);
        assert_eq!(DEFAULT_MIN_TIER, DERIVED);
        r.min_tier = Some("fact".into());
        assert_eq!(min_tier_of(&r), FACT);
        r.min_tier = Some("derived".into());
        assert_eq!(min_tier_of(&r), DERIVED);
        r.min_tier = Some("heuristic".into());
        assert_eq!(min_tier_of(&r), DEFAULT_MIN_TIER, "a hand-edited spelling");
    }

    /// A transitive forbid_edge (DB.3b) is evaluated by the path walk, never
    /// the direct one: one forbid_path violation for the module its IMPORTS
    /// chain reaches, beside the direct rule's one forbid_edge violation over
    /// the same scopes; a rule invariant (overlay version 2) is evaluated
    /// too: its prelude predicate has no row here, so it is checked and
    /// clean.
    #[test]
    fn transitive_forbid_and_rule_invariant_are_evaluated() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let overlay = "version = 2

[[constraint]]
id = \"direct\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
categories = [\"IMPORTS\"]

[[constraint]]
id = \"transitive\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
categories = [\"IMPORTS\"]
transitive = true
min_tier = \"fact\"

[[constraint]]
id = \"rule\"
kind = \"invariant\"
scope = \"services/api\"
text = \"every route has a handler\"
rule = \"violation(r) :- unhandled_route(r).\"
";
        for (rel, src) in [
            ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
            (
                "web/app.py",
                "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n",
            ),
            ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
            ("services/api/internal.py", "def charge(o):\n    return o\n"),
            (".glia/overlay.toml", overlay),
        ] {
            let p = tmp.path().join(rel);
            std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
            std::fs::write(p, src).expect("write");
        }
        let r = crate::generate_one(tmp.path().to_str().expect("utf-8")).expect("generate_one");
        let report = check(&r.merged);
        assert_eq!((report.rules, report.checked), (3, 3), "{report:?}");
        assert!(report.unchecked.is_empty(), "{report:?}");
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let ids: Vec<(&str, &str)> = report
            .violations
            .iter()
            .map(|v| (v.rule_id.as_str(), v.rule_kind))
            .collect();
        assert_eq!(
            ids,
            [("direct", FORBID_EDGE), ("transitive", FORBID_PATH)],
            "no route, so the rule has no row"
        );
        assert!(report.violations[0].blocking, "a fact import blocks");
        let path = &report.violations[1];
        assert_eq!(path.subject, ["services::api::internal"], "{path:?}");
        assert_eq!((path.tier, path.count), (FACT, 1), "{path:?}");
        assert!(path.blocking, "a fact path blocks under min_tier = fact");
        assert_eq!(
            path.evidence
                .iter()
                .map(|e| (e.category, e.from_qname.as_str(), e.to_qname.as_str()))
                .collect::<Vec<_>>(),
            [("IMPORTS", "web::app", "services::api::internal")]
        );
    }

    /// The tie-break between two chains hitting one node: the stronger tier,
    /// then fewer hops, then the imports chain.
    #[test]
    fn path_hit_rank_orders_tier_then_hops_then_imports() {
        let hit = |level, hops: usize, chain| PathHit {
            node: NodeId(1),
            level,
            chain,
            hops: (0..hops)
                .map(|_| ViolationEdge {
                    from_qname: String::new(),
                    to_qname: String::new(),
                    category: "CALLS",
                    file: None,
                    line: None,
                    emitter: None,
                    tier: FACT,
                    note: None,
                })
                .collect(),
        };
        assert!(hit(0, 5, Chain::Symbols).rank() < hit(1, 1, Chain::Imports).rank());
        assert!(hit(1, 1, Chain::Symbols).rank() < hit(1, 2, Chain::Imports).rank());
        assert!(hit(1, 2, Chain::Imports).rank() < hit(1, 2, Chain::Symbols).rank());
        assert_eq!(word_level(FACT), 0);
        assert_eq!(word_level(DERIVED), 1);
        assert_eq!(word_level(HEURISTIC), 2);
        assert_eq!(word_level("other"), 1, "why's own fallback");
    }

    /// A barred view drops exactly the edges that leave a stop node (a node
    /// of `to` or a facade), so a walk enters them and goes no further.
    #[test]
    fn barred_drops_the_edges_leaving_a_stop() {
        let calls = edge_category::CALLS;
        let edges = vec![
            Edge::new(NodeId(1), NodeId(2), calls, glia_core::Confidence::Strong),
            Edge::new(NodeId(2), NodeId(3), calls, glia_core::Confidence::Strong),
            Edge::new(NodeId(4), NodeId(5), calls, glia_core::Confidence::Strong),
            Edge::new(NodeId(1), NodeId(4), calls, glia_core::Confidence::Strong),
        ];
        let sub = Sub {
            nodes: (1..=5).map(NodeId).collect(),
            edges,
        };
        let scopes = PathScopes {
            to: Rc::new([NodeId(2), NodeId(3)].into_iter().collect()),
            via: [NodeId(4)].into_iter().collect(),
        };
        let barred = Barred {
            inner: &sub,
            scopes: &scopes,
        };
        let kept: Vec<(u64, u64)> = barred.edges().map(|e| (e.from.0, e.to.0)).collect();
        assert_eq!(kept, [(1, 2), (1, 4)]);
        assert_eq!(barred.node_ids().len(), 5);
        assert!(scopes.is_hit(NodeId(2)) && !scopes.is_hit(NodeId(4)));
        assert!(scopes.stops(NodeId(4)) && !scopes.stops(NodeId(1)));
    }

    /// Rule text is declared only in the overlay (Q9): a `rule` on an entry
    /// of another source (no write path stores one) is an error, never run.
    #[test]
    fn rule_from_another_source_is_an_error() {
        let empty = MergedGraph::new(Vec::new());
        let ctx = Ctx::new(&empty);
        let rule = one_rule(
            r#"{"id":"api-rule","kind":"invariant","source":"api","text":"t","rule":"violation(r) :- node(r, \"ROUTE\")."}"#,
        );
        assert!(is_rule_invariant(&rule), "{rule:?}");
        assert!(matches!(
            evaluate(&mut Ctx::new(&empty), &rule),
            Outcome::Error(_)
        ));
        let err = rule_invariant(&ctx, &rule, "violation(r) :- node(r, \"ROUTE\").")
            .expect_err("not the overlay's");
        assert!(err.contains("source is `api`"), "{err}");
    }

    #[test]
    fn empty_graph_has_no_rules() {
        let r = check(&MergedGraph::new(Vec::new()));
        assert_eq!((r.rules, r.checked), (0, 0));
        assert!(r.violations.is_empty() && r.unchecked.is_empty() && r.errors.is_empty());
        assert!(r.reflexion.is_none());
    }

    fn paths(pairs: &[(&str, usize)]) -> BTreeMap<String, usize> {
        pairs.iter().map(|(p, c)| (norm_path(p), *c)).collect()
    }

    #[test]
    fn owner_is_the_longest_path_on_a_segment_boundary() {
        let cp = paths(&[("web", 0), ("web/admin", 1), ("./services/api/", 2)]);
        assert_eq!(owner(&cp, "web/admin/panel.py"), Some(1));
        assert_eq!(owner(&cp, "web/admin.py"), Some(0), "no segment boundary");
        assert_eq!(owner(&cp, "web/app.py"), Some(0));
        assert_eq!(owner(&cp, "services/api/h.py"), Some(2));
        assert_eq!(owner(&cp, "webhooks/app.py"), None);
        assert_eq!(owner(&cp, "."), None);
        // The root `.` owns what nothing longer does, a PROJECT at `.` too.
        let rooted = paths(&[(".", 3), ("web", 0)]);
        assert_eq!(owner(&rooted, "scripts/tool.py"), Some(3));
        assert_eq!(owner(&rooted, "web/app.py"), Some(0));
        assert_eq!(owner(&rooted, "."), Some(3));
        assert_eq!(norm_path(""), ".");
        assert_eq!(norm_path("/a/b/"), "a/b");
    }

    fn layer(name: &str, rank: u32, strict: bool) -> LayerFact {
        LayerFact {
            name: name.into(),
            rank,
            strict,
        }
    }

    #[test]
    fn allowed_follows_layers_and_allows() {
        // 0 ui (strict, rank 0), 1 core (rank 1), 2 data (rank 2), 3 also in
        // core; 4 has no layer.
        let layers: BTreeMap<usize, LayerFact> = [
            (0, layer("ui", 0, true)),
            (1, layer("core", 1, false)),
            (2, layer("data", 2, false)),
            (3, layer("core", 1, false)),
        ]
        .into_iter()
        .collect();
        let mut allow: BTreeMap<(usize, usize), Allows> = BTreeMap::new();
        allow.insert((0, 2), vec![("z-id".into(), None), ("a-id".into(), None)]);
        allow.insert((4, 0), vec![("up".into(), None)]);
        allow.insert((3, 2), vec![("core-data".into(), None)]);
        let a = allowed_rel(&allow, &layers);
        assert_eq!(a.get(&(0, 1)).map(String::as_str), Some("layer:ui>core"));
        assert_eq!(
            a.get(&(0, 2)).map(String::as_str),
            Some("a-id"),
            "smallest allow id; strict skips data"
        );
        assert_eq!(a.get(&(1, 2)).map(String::as_str), Some("layer:core>data"));
        assert_eq!(
            a.get(&(3, 2)).map(String::as_str),
            Some("core-data"),
            "an allow names it over a layer"
        );
        assert_eq!(a.get(&(4, 0)).map(String::as_str), Some("up"));
        assert!(!a.contains_key(&(1, 3)), "same layer");
        assert!(!a.contains_key(&(2, 1)), "upward");
        assert!(!a.contains_key(&(1, 0)), "upward");
        assert!(closed_rel(&allow, &layers));
        assert!(closed_rel(&BTreeMap::new(), &layers));
        assert!(closed_rel(&allow, &BTreeMap::new()));
        assert!(!closed_rel(&BTreeMap::new(), &BTreeMap::new()));
    }
}
