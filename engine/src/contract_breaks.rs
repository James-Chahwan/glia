//! Contract breaks against a git rev (CC.8a): "did my change break my
//! clients?". LE.10c's `contract_fields` compares a producer against a
//! consumer at one point in time; this compares every contract against ITSELF
//! across the working tree's change, and lists the clients the change left
//! without a provider. CC.8c builds the rev pair as two multi-repo merges so
//! clients in other repos (`--with`) are reported too.
//!
//! [`contract_breaks_vs_rev`] builds the rev delta once
//! (`delta::graph_delta_vs_rev`, LE.1b); [`contract_breaks`] answers from an
//! already built [`RevDelta`].
//!
//! CLIENTS IN OTHER REPOS (CC.8c). [`contract_breaks_vs_rev_with`] builds
//! each side as one multi-repo merge: the provider's working tree plus the
//! client repos, then the provider's rev (materialised as the delta does,
//! built under the working tree's identity) plus the SAME client repos, each
//! at its own tree on both sides, so only the provider moves. The clients'
//! NodeIds are identical across the pair, so every client edge the delta
//! removed was removed by the provider's change: a client in another repo
//! whose route, operation or channel went away is an orphan like one inside
//! the provider. The cost is building every client twice (each side's parse
//! cache is loaded from the client's sidecar, so the second build reparses
//! nothing). A client whose path is the provider's, or repeats an earlier
//! client, is built once, as the provider or as that client.
//!
//! PAIRING. The holders are LE.10c's: every MESSAGE_TYPE (proto, Avro, JSON
//! Schema) and every OpenAPI / AsyncAPI contract op, read on both sides with
//! their SCHEMA_FIELDS. A Pact interaction is the CONSUMER's contract and a
//! feature spec is no API: both are skipped. The rev is built under the
//! working tree's identity, so an unchanged qname is the SAME NodeId: pairing
//! by id is tier `fact`. A before id the delta moved (LB.6) pairs through the
//! move: tier `derived`, change `moved`. So does a holder whose spec file git
//! renamed (FD.25): a spec moved with `git mv` keeps its op qnames but not its
//! ids, and its MODULE (named by file) never moves, so the delta pairs none
//! of it. Before judging, a before-only holder and an after-only holder with
//! one `(repo, kind, key)` (the key [`same_as::contract_op_key`]'s display
//! for an op, the qualified name for a message), each the only one-sided
//! holder of that identity, pair through the move when the before holder's
//! file is a rename source (`git diff -M --name-status <base>`, git's default
//! 50% similarity) whose target holds the after holder: tier `derived`,
//! change `moved`, listed only when the payloads differ. A spec moved and
//! rewritten past git's threshold stays `removed` + `added`. fired_on, once
//! per [`contract_breaks_vs_rev`] / [`contract_breaks_vs_rev_with`] run:
//! `[contract-breaks] moved_files=<renames read> paired_ops=<pairs made>`.
//! A before-only holder is `removed`
//! (breaking: whoever used it loses it), an after-only one `added`
//! (compatible), both tier `fact`. A pair whose payloads are equal is
//! `identical`: counted, never listed.
//!
//! EVOLUTION RULES (old = before, new = after; every [`FieldChange`] reads
//! `producer` = before, `consumer` = after):
//! - proto: LE.10c's wire rules (symmetric), except that a field retired
//!   under `reserved` is `field_removed_reserved`, and a field number dropped
//!   WITHOUT `reserved` is `field_removed_unreserved` (compatible, and the
//!   row's note): old readers survive it, a later reuse of the number would
//!   not.
//! - Avro: schema resolution with writer = old, reader = new (`backward`, the
//!   default), writer = new, reader = old (`forward`), or both (`full`, where
//!   a change breaking either direction breaks).
//! - OpenAPI `request` (what clients send): a field the new side requires that
//!   the old side lacked or left optional is `new_required_request_field`
//!   (breaking); a type the new side no longer accepts every old value of is
//!   `request_type_changed` (breaking), else `request_type_widened`; a field
//!   only the old side declares is `request_field_removed`, one only the new
//!   side declares optionally `request_field_added` (both compatible).
//! - OpenAPI `response:<code>` (what clients read): a section the new side
//!   lacks is `response_removed`; a field it lacks `response_field_removed`; a
//!   field that stops being required `response_field_now_optional`; a type the
//!   old side does not accept every new value of `response_type_changed` (all
//!   breaking). A new field or section is `response_field_added` /
//!   `response_added`, a narrowed type `response_type_narrowed` (compatible).
//! - AsyncAPI `payload[:<name>]`: an old field the new side lacks is
//!   `payload_field_removed`, any type change `payload_type_changed`
//!   (breaking); a new field is `payload_field_added` (compatible).
//!
//! As in LE.10c, nothing is guessed: a field absent from a TRUNCATED side is
//! never reported removed or added (the row is `unknown`, note `truncated`),
//! an unresolvable `$ref` is an `unknown` change, a format that differs across
//! the pair is `unknown` (`format_mismatch`), and a message type with no
//! SCHEMA_FIELDS on one side (an enum, a fixed) is `unknown`. A contract op
//! with no SCHEMA_FIELDS declares no body field: the extractor read the op, so
//! it compares as an empty schema.
//!
//! ORPHANED CLIENTS. Every HTTP_CALLS / GRPC_CALLS / RPC_CALLS /
//! GRAPHQL_CALLS / WS_CONNECTS edge the delta removed whose client end still
//! exists in the after graph (directly or through a move) with NO edge of that
//! category left: `target_removed` (tier `fact`) when the before target is a
//! removed node, else `pairing_lost` (tier `derived`: the target survives and
//! the resolver no longer pairs them, a path / verb / host change). The client
//! is located in the after graph, the target named from the before graph.
//! One oriented reading joins them (FH.24): an EVENT_FLOWS edge whose
//! EVIDENCE emitter is `resolver:evm` runs Solidity event -> `evm_event:`
//! subscription, so its client is the `to` end and its provider the `from`
//! end; a stale subscription (the contract renamed or removed its event) is
//! an orphan located at the subscription's site. Any other EVENT_FLOWS (the
//! in-process eventbus) is no client / provider contract and is not read, and
//! [`ContractBreaks::blind`] judges the client categories alone.
//!
//! ORDER. Schema rows by (status: breaking < unknown < compatible, kind, key),
//! `models` rows breaking first, then by (pairing, direction, client, server)
//! (the child module's order), orphans by (category, client, target);
//! `breaking` is the breaking schema rows plus the orphans plus the BLOCKING
//! model rows, `advisory` the breaking model rows that do not block. An
//! answer with no row in any of the three lists carries an [`Absence`]
//! (reason `no_match`, FACT) saying why.
//!
//! INFERRED MODELS (DT.1o, DT.1p). The child module `models` judges the
//! contracts nobody declared: every client <-> server model exchange the
//! graph pairs (DT.1m's `http_model` call site against handler, DT.1n's
//! `graphql_selection` operation against schema type), the server's change
//! against what its client reads or sends and the client's against what is
//! served, one row per (client model, server model) pair with located field
//! lines. [`model_breaks`] answers from a [`RevDelta`] alone; every
//! contract-breaks answer folds the same rows into [`ContractBreaks::models`]
//! (the single-repo pair and the `--with` merge alike, so a typed client in
//! another repo is judged too). THE GATE: a breaking model row blocks when
//! `check::is_blocking(tier, models_min_tier)` holds, check's vocabulary
//! ([`MODEL_MIN_TIERS`]: `fact` | `derived`; a heuristic pairing never
//! blocks). THE DEFAULT is `fact`, not check's `derived`: an inferred pairing
//! is advisory (James's ruling 4, 2026-10-17), and every HTTP / GraphQL
//! pairing glia 0.5.2 infers is derived, so by default a model row is listed
//! and counted in `advisory`, never in `breaking`. THE FLAG is
//! [`ContractBreakArgs::models_min_tier`] (`glia contract-breaks
//! --models-min-tier`, pyo3 `models_min_tier=`); `derived` makes a derived
//! pairing's break fail the gate. `breaking_only` drops the compatible model
//! rows as it drops compatible schema rows. See `contract_breaks/models.rs`.
//!
//! NAMES (FH.7a). The child module `names` is the style-agnostic fallback
//! for the exchanges no declared or inferred model covers: [`name_breaks`]
//! reads the names the change removed or renamed on the server side of each
//! paired exchange (case-normalised, rename pairs read within a hunk) and
//! lists every located client site, in any repo of the build, still using
//! one, HEURISTIC (DERIVED for a rename over a fact pairing), blocking only
//! under min tier `heuristic`. See `contract_breaks/names.rs`.
//!
//! NAMES IN THE ANSWER (FH.7b). [`contract_breaks_vs_rev`] and
//! [`contract_breaks_vs_rev_with`] run [`name_breaks`] over the same working
//! tree and build (a `--with` client repo is walked too) and list its rows as
//! [`ContractBreaks::names`]. The gate is
//! [`ContractBreakArgs::models_min_tier`], which also takes `heuristic`
//! ([`NAME_MIN_TIERS`]): a name row blocks only under `heuristic` (it is then
//! counted in `breaking`), else it is counted in `advisory`. Under
//! `heuristic` the model rows are judged as under `derived` (a heuristic
//! model pairing never blocks, the models' own rule). [`contract_breaks`]
//! over a caller's delta reads the working tree from the delta's one repo
//! root; a delta of several repos names no provider, so it lists no name row
//! and prints `[name-breaks] skipped reason=multi_repo roots=<n>`.
//!
//! THE SHARED-SCHEMA FOLD (FH.7b). One change to a schema many operations
//! reference (`$ref` to a shared error body) is one row per operation before
//! the fold, each with the same field changes. After sorting, an `operation`
//! or `channel` row with at least one field change folds into the first row
//! of its group, the group being every row with the same (status, kind,
//! format, change, tier, note) and the same field changes, in order and in
//! every field (section, field, change, producer, consumer, rule, breaking):
//! the kept row (the least key) lists every folded key in
//! [`SchemaChange::operations`], itself first, in row order. A row that
//! folds nothing keeps `operations` empty (not serialised), so an answer with
//! no shared change is byte-identical to the unfolded one. `breaking` still
//! counts every breaking contract (each folded operation once), so the exit
//! code and the count are the fold's to keep; only the listing shrinks.
//! Removed and added contracts (no field change) and messages never fold.
//!
//! Language-level public API (cargo-semver-checks parity) stays out: the
//! graph has no visibility or signature cell.
//!
//! BLIND (FH.9): `breaking = 0` over a mechanism that pairs few clients is
//! no all-clear, so [`ContractBreaks::blind`] lists each client mechanism the
//! orphan search reads (HTTP, gRPC, RPC, GraphQL, WebSocket) that
//! `health::boundary_health` rates BLIND or DEGRADED on the working tree's
//! graph, with its measure (`clients paired 7% of 14`); `[blind]
//! surface=contract_breaks ..` once per answer, after the `[model-breaks]`
//! line.
//!
//! Fired-on marker, one per answer:
//! `[contract-breaks] base=<rev> pairs=<P> breaking=<B> compatible=<C> unknown=<U> removed=<R> added=<A> orphaned_clients=<O>`
//! (`pairs` counts every paired contract, identical ones included; the
//! status counts are schema rows before `breaking_only` filters them, so the
//! line is unchanged by the model rows), then the models' own
//! `[model-breaks] base=<rev> min_tier=<t> exchanges=<E> .. unjudged=<U>`
//! line (`contract_breaks/models.rs`), then, for an answer that read the
//! names (FH.7b), FH.7a's `[name-breaks] base=<rev> ..` line (printed before
//! the two above, by the name pass) and
//! `[name-breaks] answer rows=<k> blocking=<b> advisory=<a> min_tier=<t> folded_schema_rows=<s>-><t> surface=contract_breaks`
//! (`s` the schema rows before the fold, `t` after it, both before
//! `breaking_only`), and, after all, once per
//! [`contract_breaks_vs_rev_with`] call given at least one client:
//! `[contract-breaks] clients=<given> built_twice=<distinct clients built on
//! both sides>`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use glia_activation::algo::delta::GraphDelta;
use glia_activation::profile::Tier;
use glia_code_domain::evidence::Evidence;
use glia_code_domain::{edge_category, same_as};
use glia_core::{Edge, EdgeCategoryId, NodeId};
use glia_graph::MergedGraph;

use crate::absence::{self, Absence, BlindCaveat};
use crate::answers::Locator;
use crate::build::{BuildOptions, GenerateResult, generate_many_as};
use crate::contract_fields::{
    Diff, Field, FieldChange, FieldSide, Gap, Holder, Schema, TypeChange, UNKNOWN, ancestors,
    avro_rules, collect_holders, is_ref, json_type_change, proto_rules, qualified_name,
};
use crate::delta::{RevDelta, graph_delta_vs_rev, located_delta_with};
use crate::git_rev;
use crate::health;

mod models;
mod names;

pub use models::{
    MODEL_MIN_TIERS, ModelBreak, ModelBreakArgs, ModelBreaks, ModelChange, ModelExchange,
    ModelSite, model_breaks,
};
// DT.1p: review folds the same judge into its own answer.
pub(crate) use models::model_breaks_of;
pub use names::{
    CLIENT_DEPTH, ChangedName, GENERIC, NAME_MIN_TIERS, NameBreak, NameBreakArgs, NameBreaks,
    SERVER_DEPTH, name_breaks,
};

/// The accepted [`ContractBreakArgs::avro_mode`] values, default first.
pub const AVRO_MODES: &[&str] = &["backward", "forward", "full"];

/// How [`contract_breaks`] judges and lists. Built outside the crate from
/// `Default` plus field assignment.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct ContractBreakArgs {
    /// Avro's direction ([`AVRO_MODES`]): `backward` (the default: the new
    /// schema reads what the old one wrote), `forward` (the old schema reads
    /// what the new one writes) or `full` (both). [`contract_breaks_vs_rev`]
    /// refuses any other value; [`contract_breaks`] judges one as `full`, the
    /// strictest.
    pub avro_mode: &'static str,
    /// List only `breaking` schema and model rows. [`ContractBreaks::breaking`],
    /// [`ContractBreaks::advisory`] and the orphaned clients are the same
    /// either way.
    pub breaking_only: bool,
    /// The weakest pairing tier at which a breaking inferred-model row blocks
    /// ([`NAME_MIN_TIERS`]; module docs INFERRED MODELS and NAMES IN THE
    /// ANSWER): `fact` (the default: every pairing glia 0.5.2 infers is
    /// derived, so its rows are advisory), `derived`, or `heuristic` (FH.7b:
    /// the name rows block too, the model rows are judged as under
    /// `derived`). [`contract_breaks_vs_rev`] refuses any other value;
    /// [`contract_breaks`] judges one as `fact`.
    pub models_min_tier: &'static str,
}

impl Default for ContractBreakArgs {
    fn default() -> Self {
        ContractBreakArgs {
            avro_mode: "backward",
            breaking_only: false,
            models_min_tier: MODEL_MIN_TIERS[0],
        }
    }
}

/// One contract whose declaration changed between the rev and the working tree.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct SchemaChange {
    /// `message` (a MESSAGE_TYPE), `operation` (an OpenAPI op) or `channel`
    /// (an AsyncAPI op).
    pub kind: &'static str,
    /// The message's qualified name (`shop.Order`), `METHOD path` for an
    /// operation, `<action> <channel>` for a channel op.
    pub key: String,
    /// The SCHEMA_FIELDS format (`proto`, `avro`, `openapi`, `asyncapi`),
    /// else the ORIGIN source; the after side's when it has one.
    pub format: String,
    /// The holder in the rev's graph (`None` for an added contract).
    pub before: Option<FieldSide>,
    /// The holder in the working tree's graph (`None` for a removed contract).
    pub after: Option<FieldSide>,
    /// `breaking` | `compatible` | `unknown` (`identical` pairs are counted,
    /// never listed).
    pub status: &'static str,
    /// `modified` | `removed` | `added` | `moved`.
    pub change: &'static str,
    /// `fact` (paired by identity, or one-sided) or `derived` (paired through a move).
    pub tier: &'static str,
    /// Why a row is `unknown` without a change saying so (`format_mismatch`,
    /// `before_no_fields`, `truncated`, ...), or `field_removed_unreserved`.
    pub note: Option<&'static str>,
    /// Every declared difference, `producer` = before, `consumer` = after.
    pub changes: Vec<FieldChange>,
    /// FH.7b: the keys of every operation (or channel) whose row folded into
    /// this one, this row's own key first (module docs THE SHARED-SCHEMA
    /// FOLD): one shared-schema change, listed once. Empty, and not
    /// serialised, when nothing folded.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<String>,
}

/// A client the change left without a provider.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct OrphanedClient {
    /// The client node in the working tree's graph.
    pub client_qname: String,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// The edge category it lost (`HTTP_CALLS`, `GRPC_CALLS`, ...).
    pub category: &'static str,
    /// The provider it was paired with at the rev, named from the rev's graph.
    pub target_qname: String,
    /// `target_removed` | `pairing_lost`.
    pub reason: &'static str,
    /// `fact` for `target_removed`, `derived` for `pairing_lost`.
    pub tier: &'static str,
}

/// The contract breaks of the working tree against `base` (module docs).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ContractBreaks {
    /// The rev as given.
    pub base: String,
    pub schemas: Vec<SchemaChange>,
    /// The inferred client <-> server model rows (module docs INFERRED
    /// MODELS), the compatible ones left out under `breaking_only`.
    pub models: Vec<ModelBreak>,
    pub orphaned_clients: Vec<OrphanedClient>,
    /// FH.7b: the client sites still using a name the change removed or
    /// renamed on the server side of a paired exchange ([`name_breaks`],
    /// module docs NAMES IN THE ANSWER), in its order. Empty, and not
    /// serialised, when there is none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<NameBreak>,
    /// Breaking contracts (each operation of a folded schema row once) plus
    /// orphaned clients plus blocking model rows plus blocking name rows: the
    /// CI gate (above 0 fails it).
    pub breaking: usize,
    /// Breaking model rows below the models min tier plus name rows that do
    /// not block: listed, never in `breaking`.
    pub advisory: usize,
    /// `Some` exactly when `schemas`, `models`, `orphaned_clients` and
    /// `names` are all empty.
    pub absence: Option<Absence>,
    /// FH.9: the client mechanisms the answer reads (the orphan search's
    /// HTTP, gRPC, RPC, GraphQL and WebSocket: the orphan search and the
    /// model exchanges) that health rates BLIND or DEGRADED on the working
    /// tree's graph: a `0 breaking` over a mechanism that pairs few clients
    /// says so. Empty, and not serialized, otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blind: Vec<BlindCaveat>,
}

/// The client -> provider edge categories an orphan is looked for in (and
/// the mechanisms [`ContractBreaks::blind`] judges).
const CLIENT_CATEGORIES: [EdgeCategoryId; 5] = [
    edge_category::HTTP_CALLS,
    edge_category::GRPC_CALLS,
    edge_category::RPC_CALLS,
    edge_category::GRAPHQL_CALLS,
    edge_category::WS_CONNECTS,
];

/// The EVIDENCE emitter whose EVENT_FLOWS edges are read as client ->
/// provider contracts, subscriber end first (module docs ORPHANED CLIENTS).
const EVM_EMITTER: &str = "resolver:evm";

const PRIMITIVE: &str = "contract_breaks";

/// The contract breaks of the working tree at `repo_path` against git rev
/// `base` (module docs). `Err` on an unknown [`ContractBreakArgs::avro_mode`]
/// or [`ContractBreakArgs::models_min_tier`] (checked before anything is
/// built) and on `delta::graph_delta_vs_rev`'s
/// errors: not a directory, not a git work tree, an unknown rev, a failed
/// build. Like the delta, it saves the working tree's parse-cache sidecar,
/// never a layout.
pub fn contract_breaks_vs_rev(
    repo_path: &str,
    base: &str,
    args: &ContractBreakArgs,
) -> Result<ContractBreaks, String> {
    check_mode(args)?;
    let rev = graph_delta_vs_rev(repo_path, base)?;
    let repo = Path::new(repo_path);
    let renames = git_rev::declared_renames(repo, &git_rev::resolve_rev(repo, base)?);
    let names = name_breaks(base, repo_path, &rev.after, &name_args(args))?.rows;
    Ok(breaks_of(
        &rev.answer.base,
        &rev.before.merged,
        &rev.after,
        &rev.delta,
        Some(&renames),
        names,
        args,
    ))
}

/// [`contract_breaks_vs_rev`] with the client repos at `clients` built beside
/// the provider on both sides (module docs, CLIENTS IN OTHER REPOS), so a
/// client in another repo that lost its provider is an orphan. No clients is
/// exactly [`contract_breaks_vs_rev`]. `Err` as that does, and when a client
/// is not a directory (checked, with the mode and the rev, before anything is
/// built). Saves the parse-cache sidecar of the provider's working tree and of
/// every client (`<repo>/.glia/graph/parse_cache.bin`), as an incremental
/// multi-repo build does; never a layout. The provider's rev is materialised
/// into a temp dir that is removed before this returns, on every path.
pub fn contract_breaks_vs_rev_with(
    repo_path: &str,
    base: &str,
    clients: &[String],
    args: &ContractBreakArgs,
) -> Result<ContractBreaks, String> {
    if clients.is_empty() {
        return contract_breaks_vs_rev(repo_path, base, args);
    }
    check_mode(args)?;
    let repo = Path::new(repo_path);
    if !repo.is_dir() {
        return Err(format!("not a directory: {repo_path}"));
    }
    let distinct = distinct_clients(repo_path, clients)?;
    let marker = || {
        eprintln!(
            "[contract-breaks] clients={} built_twice={}",
            clients.len(),
            distinct.len()
        );
    };
    if distinct.is_empty() {
        // Every client was the provider itself: the single-repo answer.
        let answer = contract_breaks_vs_rev(repo_path, base, args)?;
        marker();
        return Ok(answer);
    }
    let rev = git_rev::resolve_rev(repo, base)?;
    let mut pairs: Vec<(String, String)> = vec![(repo_path.to_string(), repo_path.to_string())];
    pairs.extend(distinct.iter().map(|c| (c.clone(), c.clone())));
    let opts = BuildOptions::default();
    let after = generate_many_as(&pairs, &opts)?;
    let tree = git_rev::materialize_rev(repo, &rev)?;
    let tmp = tree
        .dir
        .path()
        .to_str()
        .ok_or_else(|| format!("temp dir is not valid UTF-8: {}", tree.dir.path().display()))?;
    pairs[0].0 = tmp.to_string();
    let before = generate_many_as(&pairs, &opts)?;
    drop(tree);
    let renames = git_rev::declared_renames(repo, &rev);
    let (delta, _, _) = located_delta_with(&before.merged, &after.merged, &renames);
    let names = name_breaks(&rev.given, repo_path, &after, &name_args(args))?.rows;
    let answer = breaks_of(
        &rev.given,
        &before.merged,
        &after,
        &delta,
        Some(&renames),
        names,
        args,
    );
    marker();
    Ok(answer)
}

/// `Err` naming a mode outside [`AVRO_MODES`] or a models min tier outside
/// [`NAME_MIN_TIERS`] (contract-breaks' gate also takes `heuristic`, FH.7b):
/// ``unknown models min tier `<x>`: expected one of fact, derived,
/// heuristic``.
fn check_mode(args: &ContractBreakArgs) -> Result<(), String> {
    if !AVRO_MODES.contains(&args.avro_mode) {
        return Err(format!(
            "unknown avro mode `{}`: expected one of {}",
            args.avro_mode,
            AVRO_MODES.join(", ")
        ));
    }
    if NAME_MIN_TIERS.contains(&args.models_min_tier) {
        return Ok(());
    }
    Err(format!(
        "unknown models min tier `{}`: expected one of {}",
        args.models_min_tier,
        NAME_MIN_TIERS.join(", ")
    ))
}

/// The name pass's gate (module docs NAMES IN THE ANSWER): the models min
/// tier when it is one of [`NAME_MIN_TIERS`], else `fact`.
fn name_args(args: &ContractBreakArgs) -> NameBreakArgs {
    let mut out = NameBreakArgs::default();
    if let Some(&t) = NAME_MIN_TIERS.iter().find(|&&t| t == args.models_min_tier) {
        out.min_tier = t;
    }
    out
}

/// The model judge's gate under a contract-breaks min tier: `heuristic`
/// judges the model rows as `derived` (a heuristic model pairing never
/// blocks); any other spelling passes through (the judge reads anything but
/// `derived` as `fact`).
fn model_min_tier(min_tier: &'static str) -> &'static str {
    if min_tier == Tier::Heuristic.as_str() {
        Tier::Derived.as_str()
    } else {
        min_tier
    }
}

/// The name rows of a caller's delta (module docs NAMES IN THE ANSWER):
/// the working tree is `worktree`, else the after build's one repo root,
/// gated at `min_tier` (one of [`NAME_MIN_TIERS`], else `fact`). A build of
/// several repos with no `worktree` names no provider: no row, and
/// `[name-breaks] skipped reason=multi_repo roots=<n>` (`no_root` for a build
/// with none). A failed name pass (git) is no row and
/// `[name-breaks] skipped reason=error: <message>`: the answers over a
/// delta return no `Err`.
pub(crate) fn name_breaks_of(
    base: &str,
    worktree: Option<&str>,
    after: &GenerateResult,
    min_tier: &'static str,
) -> Vec<NameBreak> {
    let mut args = NameBreakArgs::default();
    if NAME_MIN_TIERS.contains(&min_tier) {
        args.min_tier = min_tier;
    }
    let none = |reason: String| {
        eprintln!("[name-breaks] skipped reason={reason}");
        Vec::new()
    };
    let root = match (worktree, after.repo_roots.len()) {
        (Some(w), _) => w.to_string(),
        (None, 1) => match after.repo_roots.values().next() {
            Some(r) => r.clone(),
            None => return none("no_root".to_string()),
        },
        (None, 0) => return none("no_root".to_string()),
        (None, n) => return none(format!("multi_repo roots={n}")),
    };
    match name_breaks(base, &root, after, &args) {
        Ok(b) => b.rows,
        Err(e) => none(format!("error: {e}")),
    }
}

/// `Err` naming a models min tier outside [`MODEL_MIN_TIERS`]: `unknown
/// models min tier `<x>`: expected one of fact, derived`. Review's message
/// (CLI and pyo3); contract-breaks' gate also takes `heuristic` (FH.7b,
/// `check_mode`).
pub(crate) fn check_models_min_tier(tier: &str) -> Result<(), String> {
    if MODEL_MIN_TIERS.contains(&tier) {
        return Ok(());
    }
    Err(format!(
        "unknown models min tier `{tier}`: expected one of {}",
        MODEL_MIN_TIERS.join(", ")
    ))
}

/// The client paths to build, as given and in argument order, without the
/// provider and without repeats (compared by canonical path, so `./web` and
/// `web/` are one client). `Err` names the first client that is not a
/// directory.
fn distinct_clients(repo_path: &str, clients: &[String]) -> Result<Vec<String>, String> {
    let canonical = |p: &str| -> Option<PathBuf> {
        Path::new(p)
            .is_dir()
            .then(|| std::fs::canonicalize(p).ok())
            .flatten()
    };
    let mut seen: Vec<PathBuf> = canonical(repo_path).into_iter().collect();
    let mut out = Vec::new();
    for c in clients {
        let Some(at) = canonical(c) else {
            return Err(format!("not a directory: {c}"));
        };
        if !seen.contains(&at) {
            seen.push(at);
            out.push(c.clone());
        }
    }
    Ok(out)
}

/// The contract breaks of an already computed rev delta (module docs).
/// Nothing is built; `base` is `rev.answer.base`. The name rows read the
/// delta's one repo root as the working tree (module docs NAMES IN THE
/// ANSWER).
pub fn contract_breaks(rev: &RevDelta, args: &ContractBreakArgs) -> ContractBreaks {
    let names = name_breaks_of(&rev.answer.base, None, &rev.after, name_args(args).min_tier);
    breaks_of(
        &rev.answer.base,
        &rev.before.merged,
        &rev.after,
        &rev.delta,
        None,
        names,
        args,
    )
}

/// The answer over a rev pair (module docs): `before` is the rev's graph,
/// `after_build` the working tree's build, `delta` the id-level delta
/// `after - before`. One body for the single-repo and the `--with` pair.
/// `renames` is git's `(old path, new path)` list for the rev (module docs,
/// FD.25); `None` pairs through the delta's moves only and prints no
/// `moved_files=` marker. `names` is the name pass's rows over the same
/// pair, gated at `name_args(args)` (module docs NAMES IN THE ANSWER).
fn breaks_of(
    base: &str,
    before: &MergedGraph,
    after_build: &GenerateResult,
    delta: &GraphDelta,
    renames: Option<&[(String, String)]>,
    names: Vec<NameBreak>,
    args: &ContractBreakArgs,
) -> ContractBreaks {
    let after = &after_build.merged;
    let (old, new) = (contract_holders(before), contract_holders(after));
    let (then, now) = (Locator::new(before), Locator::new(after));
    let moved: HashMap<u64, u64> = delta.moved_nodes.iter().map(|&(b, a)| (b.0, a.0)).collect();
    let old_ids: BTreeSet<u64> = old.keys().copied().collect();
    let new_ids: BTreeSet<u64> = new.keys().copied().collect();
    let mut through = moved.clone();
    if let Some(renames) = renames {
        let file_of = |loc: &Locator<'_>, id: u64| loc.locate(NodeId(id)).file;
        let spec_moves = renamed_holders(
            &one_sided(&old, &old_ids, &new_ids, &moved, |id| file_of(&then, id)),
            &one_sided(&new, &new_ids, &old_ids, &invert(&moved), |id| {
                file_of(&now, id)
            }),
            renames,
        );
        eprintln!(
            "[contract-breaks] moved_files={} paired_ops={}",
            renames.len(),
            spec_moves.len()
        );
        through.extend(spec_moves);
    }
    let pairing = pair(&old_ids, &new_ids, &through);
    let mode = AvroMode::parse(args.avro_mode);

    let side = |loc: &Locator<'_>, id: u64, h: &Holder| {
        let at = loc.locate(NodeId(id));
        FieldSide {
            repo_id: h.repo,
            qname: at.qname,
            format: h.format(),
            file: at.file,
            line: at.line,
        }
    };
    let mut rows: Vec<SchemaChange> = Vec::new();
    for &(b, a, through_move) in &pairing.pairs {
        let (o, n) = (&old[&b], &new[&a]);
        let Some((status, note, changes)) = judge_pair(o, n, mode) else {
            continue;
        };
        rows.push(SchemaChange {
            kind: kind_of(n),
            key: key_of(n),
            format: format_of(Some(n), o),
            before: Some(side(&then, b, o)),
            after: Some(side(&now, a, n)),
            status,
            change: if through_move { "moved" } else { "modified" },
            tier: if through_move {
                Tier::Derived.as_str()
            } else {
                Tier::Fact.as_str()
            },
            note,
            changes,
            operations: Vec::new(),
        });
    }
    for &b in &pairing.removed {
        let o = &old[&b];
        rows.push(SchemaChange {
            kind: kind_of(o),
            key: key_of(o),
            format: format_of(None, o),
            before: Some(side(&then, b, o)),
            after: None,
            status: "breaking",
            change: "removed",
            tier: Tier::Fact.as_str(),
            note: None,
            changes: Vec::new(),
            operations: Vec::new(),
        });
    }
    for &a in &pairing.added {
        let n = &new[&a];
        rows.push(SchemaChange {
            kind: kind_of(n),
            key: key_of(n),
            format: format_of(Some(n), n),
            before: None,
            after: Some(side(&now, a, n)),
            status: "compatible",
            change: "added",
            tier: Tier::Fact.as_str(),
            note: None,
            changes: Vec::new(),
            operations: Vec::new(),
        });
    }
    rows.sort_by(|x, y| row_key(x).cmp(&row_key(y)));

    let removed_nodes: HashSet<u64> = delta.removed_nodes.iter().map(|id| id.0).collect();
    let present: HashSet<u64> = after
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(|n| n.id.0))
        .collect();
    let served: HashSet<(u64, u32)> = after
        .all_edges()
        .filter_map(|e| client_end(e).map(|client| (client, e.category.0)))
        .collect();
    // The before graph's evm event flows, by (from, to): a removed edge key
    // carries no cells, so its emitter is read from the edge it was.
    let evm_events: HashSet<(u64, u64)> = before
        .all_edges()
        .filter(|e| is_evm_event_flow(e))
        .map(|e| (e.from.0, e.to.0))
        .collect();
    let lost: Vec<RemovedCall> = delta
        .removed_edges
        .iter()
        .filter_map(|k| {
            if CLIENT_CATEGORIES.contains(&k.category) {
                Some(RemovedCall {
                    client: k.from.0,
                    target: k.to.0,
                    category: k.category.0,
                })
            } else if k.category == edge_category::EVENT_FLOWS
                && evm_events.contains(&(k.from.0, k.to.0))
            {
                Some(RemovedCall {
                    client: k.to.0,
                    target: k.from.0,
                    category: k.category.0,
                })
            } else {
                None
            }
        })
        .collect();
    let evm_lost = lost
        .iter()
        .filter(|c| c.category == edge_category::EVENT_FLOWS.0)
        .count();
    let mut orphans: Vec<OrphanedClient> =
        orphaned(&lost, &moved, &present, &served, &removed_nodes)
            .into_iter()
            .map(|o| {
                let at = now.locate(NodeId(o.client));
                let target_removed = o.reason == "target_removed";
                OrphanedClient {
                    client_qname: at.qname,
                    file: at.file,
                    line: at.line,
                    category: edge_category::name(EdgeCategoryId(o.category)),
                    target_qname: then.locate(NodeId(o.target)).qname,
                    reason: o.reason,
                    tier: if target_removed {
                        Tier::Fact.as_str()
                    } else {
                        Tier::Derived.as_str()
                    },
                }
            })
            .collect();
    orphans.sort_by(|x, y| {
        (x.category, &x.client_qname, &x.target_qname).cmp(&(
            y.category,
            &y.client_qname,
            &y.target_qname,
        ))
    });
    orphans.dedup_by(|x, y| {
        (x.category, &x.client_qname, &x.target_qname)
            == (y.category, &y.client_qname, &y.target_qname)
    });
    if evm_lost > 0 {
        eprintln!(
            "[contract-breaks] evm event subscriptions lost={evm_lost} orphaned={}",
            orphans
                .iter()
                .filter(|o| o.category == edge_category::name(edge_category::EVENT_FLOWS))
                .count(),
        );
    }

    let count = |f: &dyn Fn(&SchemaChange) -> bool| rows.iter().filter(|r| f(r)).count();
    let (n_breaking, n_compatible, n_unknown) = (
        count(&|r| r.status == "breaking"),
        count(&|r| r.status == "compatible"),
        count(&|r| r.status == "unknown"),
    );
    let (n_removed, n_added) = (
        count(&|r| r.change == "removed"),
        count(&|r| r.change == "added"),
    );
    let base = base.to_string();
    eprintln!(
        "[contract-breaks] base={base} pairs={} breaking={n_breaking} compatible={n_compatible} unknown={n_unknown} removed={n_removed} added={n_added} orphaned_clients={}",
        pairing.pairs.len(),
        orphans.len(),
    );
    let judged = model_breaks_of(
        &base,
        before,
        after_build,
        delta,
        &ModelBreakArgs {
            min_tier: model_min_tier(args.models_min_tier),
        },
    );
    let mut models = judged.rows;
    let names_blocking = names.iter().filter(|r| r.blocking).count();
    let names_advisory = names.len() - names_blocking;
    let breaking = n_breaking + orphans.len() + judged.blocking + names_blocking;
    let advisory = judged.advisory + names_advisory;
    let unfolded = rows.len();
    let mut rows = fold_shared(rows);
    eprintln!(
        "[name-breaks] answer rows={} blocking={names_blocking} advisory={names_advisory} min_tier={} folded_schema_rows={unfolded}->{} surface={PRIMITIVE}",
        names.len(),
        name_args(args).min_tier,
        rows.len(),
    );
    let listed = rows.len();
    let listed_models = models.len();
    if args.breaking_only {
        rows.retain(|r| r.status == "breaking");
        models.retain(|r| r.status == "breaking");
    }
    let absence = (rows.is_empty() && models.is_empty() && orphans.is_empty() && names.is_empty()).then(|| {
        let mut note = if old.is_empty() && new.is_empty() && lost.is_empty() {
            format!(
                "no contract (OpenAPI / AsyncAPI op or message type) in either graph and no client edge removed vs {base}"
            )
        } else if listed > 0 {
            format!(
                "{} contract {} vs {base}: {listed} compatible or unknown {} left out by breaking_only, and no client lost its provider",
                pairing.pairs.len(),
                absence::plural(pairing.pairs.len(), "pair", "pairs"),
                absence::plural(listed, "row", "rows"),
            )
        } else {
            format!(
                "{} contract {} vs {base}, none with a changed declaration; no contract added or removed, and no client lost its provider",
                pairing.pairs.len(),
                absence::plural(pairing.pairs.len(), "pair", "pairs"),
            )
        };
        // Byte-identical to the schema-only note when no model exchange
        // was judged (the CC.8a-c pins).
        if judged.judged > 0 {
            let what = if listed_models > 0 {
                format!(
                    "none breaking ({listed_models} compatible {} left out by breaking_only)",
                    absence::plural(listed_models, "row", "rows"),
                )
            } else {
                "none changed what a client reads or sends".to_string()
            };
            note.push_str(&format!(
                "; {} client <-> server model {} judged, {what}",
                judged.judged,
                absence::plural(judged.judged, "exchange", "exchanges"),
            ));
        }
        let mechanisms: Vec<&'static str> = CLIENT_CATEGORIES.iter().map(|&c| edge_category::name(c)).collect();
        let mut a = absence::empty(after, PRIMITIVE, &format!("rev {base}"), "no_match", note, &mechanisms, None);
        a.unparsed_files = after_build.parse_errors.len();
        a
    });
    let read = health::mechanisms_read(
        CLIENT_CATEGORIES.iter().map(|&c| edge_category::name(c)),
        std::iter::empty::<&str>(),
    );
    ContractBreaks {
        base,
        schemas: rows,
        models,
        orphaned_clients: orphans,
        names,
        breaking,
        advisory,
        absence,
        blind: health::blind_caveats(after, PRIMITIVE, &read),
    }
}

// ---- the shared-schema fold (FH.7b) -----------------------------------------

/// What makes two sorted schema rows one shared change (module docs THE
/// SHARED-SCHEMA FOLD): `None` for a row that never folds (a message, or no
/// field change).
fn fold_key(r: &SchemaChange) -> Option<String> {
    if r.kind == "message" || r.changes.is_empty() {
        return None;
    }
    let changes: Vec<String> = r
        .changes
        .iter()
        .map(|c| {
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{:?}\u{1f}{:?}\u{1f}{}\u{1f}{}",
                c.section, c.field, c.change, c.producer, c.consumer, c.rule, c.breaking
            )
        })
        .collect();
    Some(format!(
        "{}\u{1e}{}\u{1e}{}\u{1e}{}\u{1e}{}\u{1e}{:?}\u{1e}{}",
        r.status,
        r.kind,
        r.format,
        r.change,
        r.tier,
        r.note,
        changes.join("\u{1d}")
    ))
}

/// The rows, sorted, with every shared-schema change folded into its first
/// row (module docs THE SHARED-SCHEMA FOLD). Order is kept: a kept row stays
/// where it was, a folded one leaves. The map is looked up, never iterated.
fn fold_shared(rows: Vec<SchemaChange>) -> Vec<SchemaChange> {
    let mut first: BTreeMap<String, usize> = BTreeMap::new();
    let mut out: Vec<SchemaChange> = Vec::with_capacity(rows.len());
    for r in rows {
        let Some(key) = fold_key(&r) else {
            out.push(r);
            continue;
        };
        match first.get(&key) {
            Some(&at) => {
                let kept = &mut out[at];
                if kept.operations.is_empty() {
                    kept.operations.push(kept.key.clone());
                }
                kept.operations.push(r.key);
            }
            None => {
                first.insert(key, out.len());
                out.push(r);
            }
        }
    }
    out
}

// ---- holders and their pairing ---------------------------------------------

/// Every MESSAGE_TYPE and every OpenAPI / AsyncAPI op of `m` (module docs).
fn contract_holders(m: &MergedGraph) -> BTreeMap<u64, Holder> {
    collect_holders(m)
        .into_iter()
        .filter(|(_, h)| {
            h.message || matches!(h.origin_str("source"), Some("openapi" | "asyncapi"))
        })
        .collect()
}

fn kind_of(h: &Holder) -> &'static str {
    if h.message {
        "message"
    } else if h.origin_str("method").is_some() {
        "operation"
    } else {
        "channel"
    }
}

/// A row's key: a message's fully-qualified name; an op's
/// `same_as::contract_op_key` display (`METHOD path`, `[action ]channel`), the
/// one key the identity resolver's contract_op class uses too; else the qname.
fn key_of(h: &Holder) -> String {
    if h.message {
        return qualified_name(&h.qname).to_string();
    }
    if let Some(k) = h.origin.as_ref().and_then(same_as::contract_op_key) {
        return k.display;
    }
    h.qname.clone()
}

/// `primary`'s format when it names one, else `fallback`'s.
fn format_of(primary: Option<&Holder>, fallback: &Holder) -> String {
    primary
        .map(Holder::format)
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| fallback.format())
}

type RowKey<'a> = (
    u8,
    &'static str,
    &'a str,
    &'static str,
    Option<&'a str>,
    Option<&'a str>,
);

fn row_key(r: &SchemaChange) -> RowKey<'_> {
    let rank = match r.status {
        "breaking" => 0,
        "unknown" => 1,
        _ => 2,
    };
    (
        rank,
        r.kind,
        r.key.as_str(),
        r.change,
        r.before.as_ref().map(|s| s.qname.as_str()),
        r.after.as_ref().map(|s| s.qname.as_str()),
    )
}

/// Before holders paired with after holders, and the one-sided rest.
#[derive(Debug, Default, PartialEq)]
struct Pairing {
    /// `(before id, after id, paired through a move)`, by before id.
    pairs: Vec<(u64, u64, bool)>,
    removed: Vec<u64>,
    added: Vec<u64>,
}

/// A one-sided holder's identity: `(repo, kind, key)` (module docs, FD.25).
type HolderIdentity = (u64, &'static str, String);

/// `other -> this` for a `this -> other` move map.
fn invert(moved: &HashMap<u64, u64>) -> HashMap<u64, u64> {
    moved.iter().map(|(&b, &a)| (a, b)).collect()
}

/// The holders of one side neither paired by id (`ids` absent from `other`)
/// nor through a delta move (`moved`, keyed by this side's id), grouped by
/// identity: `identity -> [(id, file)]`, ids ascending.
fn one_sided(
    holders: &BTreeMap<u64, Holder>,
    ids: &BTreeSet<u64>,
    other: &BTreeSet<u64>,
    moved: &HashMap<u64, u64>,
    file_of: impl Fn(u64) -> Option<String>,
) -> BTreeMap<HolderIdentity, Vec<(u64, Option<String>)>> {
    let mut out: BTreeMap<HolderIdentity, Vec<(u64, Option<String>)>> = BTreeMap::new();
    for &id in ids {
        if other.contains(&id) || moved.contains_key(&id) {
            continue;
        }
        let h = &holders[&id];
        out.entry((h.repo, kind_of(h), key_of(h)))
            .or_default()
            .push((id, file_of(id)));
    }
    out
}

/// `before id -> after id` for every identity held by exactly one removed and
/// exactly one added holder whose files git paired as a rename (module docs,
/// FD.25).
fn renamed_holders(
    removed: &BTreeMap<HolderIdentity, Vec<(u64, Option<String>)>>,
    added: &BTreeMap<HolderIdentity, Vec<(u64, Option<String>)>>,
    renames: &[(String, String)],
) -> BTreeMap<u64, u64> {
    let renamed: BTreeSet<(&str, &str)> = renames
        .iter()
        .map(|(o, n)| (o.as_str(), n.as_str()))
        .collect();
    let mut out = BTreeMap::new();
    for (identity, olds) in removed {
        let (Some([(a, Some(new_file))]), [(b, Some(old_file))]) =
            (added.get(identity).map(Vec::as_slice), olds.as_slice())
        else {
            continue;
        };
        if renamed.contains(&(old_file.as_str(), new_file.as_str())) {
            out.insert(*b, *a);
        }
    }
    out
}

/// Moves first (a swap pairs each id with the other's), then the same id on
/// both sides; an after holder is taken once.
fn pair(old: &BTreeSet<u64>, new: &BTreeSet<u64>, moved: &HashMap<u64, u64>) -> Pairing {
    let mut taken: BTreeSet<u64> = BTreeSet::new();
    let mut through: BTreeMap<u64, u64> = BTreeMap::new();
    for &b in old {
        if let Some(&a) = moved.get(&b)
            && new.contains(&a)
            && taken.insert(a)
        {
            through.insert(b, a);
        }
    }
    let mut out = Pairing::default();
    for &b in old {
        if let Some(&a) = through.get(&b) {
            out.pairs.push((b, a, true));
        } else if new.contains(&b) && taken.insert(b) {
            out.pairs.push((b, b, false));
        } else {
            out.removed.push(b);
        }
    }
    out.added = new.iter().filter(|a| !taken.contains(a)).copied().collect();
    out
}

// ---- judging one pair --------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum AvroMode {
    Backward,
    Forward,
    Full,
}

impl AvroMode {
    fn parse(s: &str) -> AvroMode {
        match s {
            "backward" => AvroMode::Backward,
            "forward" => AvroMode::Forward,
            _ => AvroMode::Full,
        }
    }
}

/// A listed verdict: status, note, changes.
type Verdict = (&'static str, Option<&'static str>, Vec<FieldChange>);

/// `None` when the pair is identical: equal payloads, or no declared
/// difference the rules see (a reordered declaration).
fn judge_pair(old: &Holder, new: &Holder, mode: AvroMode) -> Option<Verdict> {
    let (mut a, mut b) = (old.payloads.clone(), new.payloads.clone());
    a.sort();
    b.sort();
    if a == b {
        return None;
    }
    let op = !old.message && !new.message;
    let (o, n) = match (old.schema(), new.schema()) {
        (Ok(o), Ok(n)) => (o, n),
        // A contract op without SCHEMA_FIELDS declares no body field.
        (Ok(o), Err(Gap::NoFields)) if op => {
            let n = empty_like(&o);
            (o, n)
        }
        (Err(Gap::NoFields), Ok(n)) if op => (empty_like(&n), n),
        (o, n) => return Some(("unknown", Some(gap_note(o.err(), n.err())), Vec::new())),
    };
    if o.format != n.format {
        return Some(("unknown", Some("format_mismatch"), Vec::new()));
    }
    let d = match o.format.as_str() {
        "proto" => proto_evolution(&o, &n),
        "avro" => avro_evolution(&o, &n, mode),
        "openapi" => openapi_evolution(&o, &n),
        "asyncapi" => asyncapi_evolution(&o, &n),
        _ => return Some(("unknown", Some("unsupported_format"), Vec::new())),
    };
    verdict(d)
}

fn empty_like(s: &Schema) -> Schema {
    Schema {
        format: s.format.clone(),
        sections: BTreeMap::new(),
        reserved: Vec::new(),
        truncated: false,
    }
}

fn gap_note(before: Option<Gap>, after: Option<Gap>) -> &'static str {
    match (before, after) {
        (Some(Gap::NoFields), _) => "before_no_fields",
        (_, Some(Gap::NoFields)) => "after_no_fields",
        (Some(Gap::Conflicting), _) | (_, Some(Gap::Conflicting)) => "conflicting_fields",
        _ => "unparsable_fields",
    }
}

/// LE.10c's status rule over one diff; `None` when nothing differs.
fn verdict(d: Diff) -> Option<Verdict> {
    let status = if d.changes.iter().any(|c| c.breaking) {
        "breaking"
    } else if d.suppressed || d.changes.iter().any(|c| c.change == UNKNOWN) {
        "unknown"
    } else if d.changes.is_empty() {
        return None;
    } else {
        "compatible"
    };
    let note = if d.suppressed {
        Some("truncated")
    } else if d
        .changes
        .iter()
        .any(|c| c.rule == "field_removed_unreserved")
    {
        Some("field_removed_unreserved")
    } else {
        None
    };
    Some((status, note, d.changes))
}

// ---- proto -----------------------------------------------------------------

/// LE.10c's symmetric wire rules, read as an evolution: a field only the old
/// side declares was removed, compatible on the wire; under the new side's
/// `reserved` that is the documented retirement (`field_removed_reserved`),
/// without it the number is free for an incompatible reuse
/// (`field_removed_unreserved`).
fn proto_evolution(old: &Schema, new: &Schema) -> Diff {
    let mut d = proto_rules(old, new);
    let numbered: HashSet<&str> = old
        .fields("fields")
        .iter()
        .filter(|f| f.number.is_some())
        .map(|f| f.name.as_str())
        .collect();
    for c in &mut d.changes {
        match (c.change, c.rule) {
            ("producer_only", "proto_unknown_field") if numbered.contains(c.field.as_str()) => {
                c.rule = "field_removed_unreserved";
            }
            ("reserved", "proto_reserved_reused") if c.consumer.as_deref() == Some("reserved") => {
                c.rule = "field_removed_reserved";
                c.breaking = false;
            }
            _ => {}
        }
    }
    d
}

// ---- avro ------------------------------------------------------------------

/// Avro resolution in `mode`'s direction(s), every change oriented
/// before -> after.
fn avro_evolution(old: &Schema, new: &Schema, mode: AvroMode) -> Diff {
    let backward = || avro_rules(old, new);
    let forward = || {
        let mut d = avro_rules(new, old);
        d.changes.iter_mut().for_each(flip);
        d
    };
    match mode {
        AvroMode::Backward => backward(),
        AvroMode::Forward => forward(),
        AvroMode::Full => {
            let (mut d, f) = (backward(), forward());
            d.suppressed |= f.suppressed;
            for c in f.changes {
                let dup = d.changes.iter().any(|x| {
                    (
                        x.section.as_str(),
                        x.field.as_str(),
                        x.change,
                        x.rule,
                        x.breaking,
                        &x.producer,
                        &x.consumer,
                    ) == (
                        c.section.as_str(),
                        c.field.as_str(),
                        c.change,
                        c.rule,
                        c.breaking,
                        &c.producer,
                        &c.consumer,
                    )
                });
                if !dup {
                    d.changes.push(c);
                }
            }
            d
        }
    }
}

/// A change judged with the sides swapped, re-oriented before -> after.
fn flip(c: &mut FieldChange) {
    std::mem::swap(&mut c.producer, &mut c.consumer);
    c.change = match c.change {
        "producer_only" => "consumer_only",
        "consumer_only" => "producer_only",
        other => other,
    };
}

// ---- JSON-schema-shaped contracts (OpenAPI, AsyncAPI) -----------------------

/// Which way values cross the contract, for a type change.
#[derive(Clone, Copy, PartialEq)]
enum Flow {
    /// Clients send it (a request): the new type must accept every old value.
    Input,
    /// Clients read it (a response): the old type must accept every new value.
    Output,
    /// Either (an AsyncAPI payload): any type change breaks.
    Both,
}

/// The rule names and verdicts of one section kind.
struct Evolution {
    flow: Flow,
    /// A field only the old side declares.
    removed: (&'static str, bool),
    /// A field only the new side declares, optional.
    added: (&'static str, bool),
    /// A field only the new side declares, required.
    added_required: (&'static str, bool),
    /// A field both declare that became required.
    now_required: (&'static str, bool),
    /// A field both declare that stopped being required.
    now_optional: (&'static str, bool),
    type_changed: &'static str,
    type_compatible: &'static str,
}

const REQUEST: Evolution = Evolution {
    flow: Flow::Input,
    removed: ("request_field_removed", false),
    added: ("request_field_added", false),
    added_required: ("new_required_request_field", true),
    now_required: ("new_required_request_field", true),
    now_optional: ("request_field_now_optional", false),
    type_changed: "request_type_changed",
    type_compatible: "request_type_widened",
};

const RESPONSE: Evolution = Evolution {
    flow: Flow::Output,
    removed: ("response_field_removed", true),
    added: ("response_field_added", false),
    added_required: ("response_field_added", false),
    now_required: ("response_field_now_required", false),
    now_optional: ("response_field_now_optional", true),
    type_changed: "response_type_changed",
    type_compatible: "response_type_narrowed",
};

const PAYLOAD: Evolution = Evolution {
    flow: Flow::Both,
    removed: ("payload_field_removed", true),
    added: ("payload_field_added", false),
    added_required: ("payload_field_added", false),
    now_required: ("payload_field_now_required", false),
    now_optional: ("payload_field_now_optional", true),
    type_changed: "payload_type_changed",
    type_compatible: "payload_type_changed",
};

/// OpenAPI: `request` by [`REQUEST`]; each `response:<code>` by [`RESPONSE`],
/// a whole response section on one side only judged as a section. Any other
/// section is compared as a response.
fn openapi_evolution(old: &Schema, new: &Schema) -> Diff {
    let mut d = Diff::default();
    let empty: Vec<Field> = Vec::new();
    for sec in section_names(old, new) {
        let (of, nf) = (old.sections.get(sec), new.sections.get(sec));
        if sec == "request" {
            evolve_section(
                &mut d,
                sec,
                (of.unwrap_or(&empty), old.truncated),
                (nf.unwrap_or(&empty), new.truncated),
                &REQUEST,
            );
            continue;
        }
        match (of, nf) {
            (Some(of), Some(nf)) => evolve_section(
                &mut d,
                sec,
                (of, old.truncated),
                (nf, new.truncated),
                &RESPONSE,
            ),
            (Some(_), None) if new.truncated => d.suppressed = true,
            (Some(_), None) => d.push(
                (sec, ""),
                "section",
                (Some("declared"), None),
                "response_removed",
                true,
            ),
            (None, Some(_)) if old.truncated => d.suppressed = true,
            (None, Some(_)) => d.push(
                (sec, ""),
                "section",
                (None, Some("declared")),
                "response_added",
                false,
            ),
            (None, None) => {}
        }
    }
    d
}

/// AsyncAPI: every `payload[:<name>]` section by [`PAYLOAD`], a section on
/// one side only as all of its fields.
fn asyncapi_evolution(old: &Schema, new: &Schema) -> Diff {
    let mut d = Diff::default();
    let empty: Vec<Field> = Vec::new();
    for sec in section_names(old, new) {
        let (of, nf) = (old.sections.get(sec), new.sections.get(sec));
        evolve_section(
            &mut d,
            sec,
            (of.map_or(&empty[..], Vec::as_slice), old.truncated),
            (nf.map_or(&empty[..], Vec::as_slice), new.truncated),
            &PAYLOAD,
        );
    }
    d
}

fn section_names<'a>(old: &'a Schema, new: &'a Schema) -> BTreeSet<&'a str> {
    old.sections
        .keys()
        .chain(new.sections.keys())
        .map(String::as_str)
        .collect()
}

/// One section, old fields `of` vs new fields `nf`, each with its side's
/// `truncated` flag, by `ev`. LE.10c's `json_section` guards, read as an
/// evolution: a whole-body `$ref` or a field under a `$ref` that differs is
/// `unknown`; a field absent from a truncated side is never reported; only
/// the topmost of a removed or added subtree is.
fn evolve_section(
    d: &mut Diff,
    sec: &str,
    (of, ot): (&[Field], bool),
    (nf, nt): (&[Field], bool),
    ev: &Evolution,
) {
    let o_by: HashMap<&str, &Field> = of.iter().map(|f| (f.name.as_str(), f)).collect();
    let n_by: HashMap<&str, &Field> = nf.iter().map(|f| (f.name.as_str(), f)).collect();
    let o_root = o_by.get("$ref").map(|f| f.ty.as_deref());
    let n_root = n_by.get("$ref").map(|f| f.ty.as_deref());
    if (o_root.is_some() || n_root.is_some()) && o_root != n_root {
        d.push(
            (sec, "$ref"),
            UNKNOWN,
            (o_root.flatten(), n_root.flatten()),
            "unresolved_ref",
            false,
        );
        return;
    }
    let refs: HashSet<&str> = of
        .iter()
        .chain(nf)
        .filter(|f| is_ref(f.ty.as_deref()))
        .map(|f| f.name.as_str())
        .collect();
    let ty_of = |by: &HashMap<&str, &Field>, name: &str| {
        by.get(name)
            .and_then(|f| f.ty.as_deref())
            .map(str::to_string)
    };
    let mut unknown: BTreeSet<String> = BTreeSet::new();
    let mut ref_unknown = |d: &mut Diff, name: &str| {
        if unknown.insert(name.to_string()) {
            let (a, b) = (ty_of(&o_by, name), ty_of(&n_by, name));
            d.push(
                (sec, name),
                UNKNOWN,
                (a.as_deref(), b.as_deref()),
                "unresolved_ref",
                false,
            );
        }
    };
    // Old and new names reported absent from the other side.
    let (mut gone, mut arrived): (HashSet<&str>, HashSet<&str>) = (HashSet::new(), HashSet::new());

    for f in of {
        let name = f.name.as_str();
        if let Some(r) = ancestors(name).find(|a| refs.contains(a)) {
            if ty_of(&o_by, r) != ty_of(&n_by, r) {
                ref_unknown(d, r);
            }
            continue;
        }
        let Some(g) = n_by.get(name) else {
            if ancestors(name).any(|a| gone.contains(a)) {
                continue;
            }
            if nt {
                d.suppressed = true;
                continue;
            }
            gone.insert(name);
            let (rule, breaking) = ev.removed;
            d.push(
                (sec, name),
                "producer_only",
                (f.ty.as_deref(), None),
                rule,
                breaking,
            );
            continue;
        };
        let (a, b) = (f.ty.as_deref(), g.ty.as_deref());
        if a != b {
            if is_ref(a) || is_ref(b) {
                ref_unknown(d, name);
            } else {
                type_change(d, sec, name, (a, b), ev);
            }
        }
        if f.required != g.required {
            let (label, (rule, breaking)) = if g.required {
                (("optional", "required"), ev.now_required)
            } else {
                (("required", "optional"), ev.now_optional)
            };
            d.push(
                (sec, name),
                "required",
                (Some(label.0), Some(label.1)),
                rule,
                breaking,
            );
        }
    }
    for g in nf {
        let name = g.name.as_str();
        if o_by.contains_key(name)
            || ancestors(name).any(|a| refs.contains(a) || arrived.contains(a))
        {
            continue;
        }
        if ot {
            d.suppressed = true;
            continue;
        }
        arrived.insert(name);
        let (rule, breaking) = if g.required {
            ev.added_required
        } else {
            ev.added
        };
        d.push(
            (sec, name),
            "consumer_only",
            (None, g.ty.as_deref()),
            rule,
            breaking,
        );
    }
}

/// A field both sides declare with different types, by `ev.flow`.
fn type_change(
    d: &mut Diff,
    sec: &str,
    name: &str,
    (a, b): (Option<&str>, Option<&str>),
    ev: &Evolution,
) {
    let (old, new) = (a.unwrap_or("any"), b.unwrap_or("any"));
    // `(from, to)` for json_type_change: every `from` value must be a `to` value.
    let judged = match ev.flow {
        Flow::Input => json_type_change(old, new),
        Flow::Output => json_type_change(new, old),
        Flow::Both if old == "any" || new == "any" => Some(TypeChange::Untyped),
        Flow::Both => Some(TypeChange::Changed),
    };
    match judged {
        Some(TypeChange::Changed) => d.push((sec, name), "type", (a, b), ev.type_changed, true),
        Some(TypeChange::Untyped) => d.push((sec, name), UNKNOWN, (a, b), "untyped", false),
        // Widened, or the accepting side takes any value.
        Some(TypeChange::Widened) | None => {
            d.push((sec, name), "type", (a, b), ev.type_compatible, false)
        }
    }
}

// ---- orphaned clients --------------------------------------------------------

/// One client -> provider edge the delta removed, as before-side ids.
struct RemovedCall {
    client: u64,
    target: u64,
    category: u32,
}

/// An orphan before it is located: the client's after id.
#[derive(Debug, PartialEq)]
struct Orphan {
    client: u64,
    target: u64,
    category: u32,
    reason: &'static str,
}

/// Whether `e` is an EVENT_FLOWS edge `resolver:evm` paired (Solidity event
/// -> `evm_event:` subscription).
fn is_evm_event_flow(e: &Edge) -> bool {
    e.category == edge_category::EVENT_FLOWS
        && Evidence::of(e).is_some_and(|ev| ev.emitter == EVM_EMITTER)
}

/// The client end of a client -> provider edge: `from` for the
/// [`CLIENT_CATEGORIES`], `to` (the subscription) for an evm event flow,
/// `None` for any other edge.
fn client_end(e: &Edge) -> Option<u64> {
    if CLIENT_CATEGORIES.contains(&e.category) {
        Some(e.from.0)
    } else if is_evm_event_flow(e) {
        Some(e.to.0)
    } else {
        None
    }
}

/// The removed calls whose client survives (`present`, directly or through
/// `moved`) with no edge of the call's category left (`served`, after ids).
fn orphaned(
    lost: &[RemovedCall],
    moved: &HashMap<u64, u64>,
    present: &HashSet<u64>,
    served: &HashSet<(u64, u32)>,
    removed_nodes: &HashSet<u64>,
) -> Vec<Orphan> {
    let mut out = Vec::new();
    for call in lost {
        let client = moved.get(&call.client).copied().unwrap_or(call.client);
        if !present.contains(&client) || served.contains(&(client, call.category)) {
            continue;
        }
        let reason = if removed_nodes.contains(&call.target) {
            "target_removed"
        } else {
            "pairing_lost"
        };
        out.push(Orphan {
            client,
            target: call.target,
            category: call.category,
            reason,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(message: bool, payload: &str) -> Holder {
        Holder {
            repo: 1,
            message,
            qname: "q".to_string(),
            payloads: vec![payload.to_string()],
            origin: None,
        }
    }

    fn schema(payload: &str) -> Schema {
        holder(false, payload)
            .schema()
            .unwrap_or_else(|_| panic!("parses: {payload}"))
    }

    fn rules(d: &Diff) -> Vec<(&str, &str, &str, bool)> {
        d.changes
            .iter()
            .map(|c| (c.field.as_str(), c.change, c.rule, c.breaking))
            .collect()
    }

    fn field_change(section: &str, field: &str, breaking: bool) -> FieldChange {
        FieldChange {
            section: section.to_string(),
            field: field.to_string(),
            change: if breaking {
                "producer_only"
            } else {
                "consumer_only"
            },
            producer: breaking.then(|| "string".to_string()),
            consumer: (!breaking).then(|| "string".to_string()),
            rule: if breaking {
                "response_field_removed"
            } else {
                "response_field_added"
            },
            breaking,
        }
    }

    fn op_row(kind: &'static str, key: &str, changes: Vec<FieldChange>) -> SchemaChange {
        SchemaChange {
            kind,
            key: key.to_string(),
            format: "openapi".to_string(),
            before: None,
            after: None,
            status: "breaking",
            change: "modified",
            tier: Tier::Fact.as_str(),
            note: None,
            changes,
            operations: Vec::new(),
        }
    }

    /// FH.7b: one shared-schema change (the same field changes on many
    /// operations) folds into its first row, which lists every folded key;
    /// a row whose changes differ in any field (here the section), a
    /// message and a removed op (no field change) never fold, and order is
    /// kept.
    #[test]
    fn shared_schema_rows_fold_into_their_first_row() {
        let shared = || {
            vec![
                field_change("response:400", "id", true),
                field_change("response:400", "messageId", false),
            ]
        };
        let rows = vec![
            op_row("operation", "DELETE /a", shared()),
            op_row("operation", "GET /a", shared()),
            op_row(
                "operation",
                "GET /b",
                vec![
                    field_change("response:401", "id", true),
                    field_change("response:401", "messageId", false),
                ],
            ),
            op_row("message", "shop.Order", shared()),
            op_row("message", "shop.Other", shared()),
            op_row("operation", "GET /gone", Vec::new()),
            op_row("operation", "GET /gone2", Vec::new()),
            op_row("operation", "POST /a", shared()),
        ];
        let folded = fold_shared(rows);
        let keys: Vec<(&str, Vec<&str>)> = folded
            .iter()
            .map(|r| {
                (
                    r.key.as_str(),
                    r.operations.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                ("DELETE /a", vec!["DELETE /a", "GET /a", "POST /a"]),
                ("GET /b", vec![]),
                ("shop.Order", vec![]),
                ("shop.Other", vec![]),
                ("GET /gone", vec![]),
                ("GET /gone2", vec![]),
            ]
        );
        // A row that folded nothing serialises as before: no `operations`.
        let one = serde_json::to_value(&folded[1]).expect("json");
        assert!(one.get("operations").is_none(), "{one}");
        let many = serde_json::to_value(&folded[0]).expect("json");
        assert_eq!(many["operations"][2], "POST /a", "{many}");

        let mut other_status = op_row("operation", "PUT /a", shared());
        other_status.status = "unknown";
        let kept = fold_shared(vec![op_row("operation", "GET /a", shared()), other_status]);
        assert_eq!(kept.len(), 2, "a different status never folds");
    }

    /// FH.7b: contract-breaks' gate takes `heuristic` (the name rows'), the
    /// model judge reads it as `derived`, and the name pass gets it as given.
    #[test]
    fn heuristic_min_tier_gates_names_and_judges_models_as_derived() {
        let mut args = ContractBreakArgs::default();
        assert_eq!(name_args(&args).min_tier, "fact");
        assert_eq!(model_min_tier(args.models_min_tier), "fact");
        args.models_min_tier = "heuristic";
        assert!(check_mode(&args).is_ok());
        assert_eq!(name_args(&args).min_tier, "heuristic");
        assert_eq!(model_min_tier(args.models_min_tier), "derived");
        args.models_min_tier = "derived";
        assert_eq!(model_min_tier(args.models_min_tier), "derived");
        args.models_min_tier = "certain";
        assert_eq!(
            check_mode(&args),
            Err(
                "unknown models min tier `certain`: expected one of fact, derived, heuristic"
                    .to_string()
            )
        );
        assert_eq!(name_args(&args).min_tier, "fact");
    }

    #[test]
    fn renamed_holders_pair_one_to_one_through_a_git_rename() {
        type Side = BTreeMap<HolderIdentity, Vec<(u64, Option<String>)>>;
        let id = |key: &str| (7u64, "operation", key.to_string());
        let at = |f: &str| Some(f.to_string());
        let removed: Side = [
            (id("GET /a"), vec![(1, at("api/a.yaml"))]),
            (id("GET /b"), vec![(2, at("api/a.yaml"))]),
            // Two removed holders of one identity: never paired.
            (
                id("GET /c"),
                vec![(3, at("api/a.yaml")), (4, at("api/a.yaml"))],
            ),
            // Its file was not renamed to the added holder's.
            (id("GET /d"), vec![(5, at("api/d.yaml"))]),
        ]
        .into();
        let added: Side = [
            (id("GET /a"), vec![(11, at("spec/a.yaml"))]),
            (id("GET /b"), vec![(12, at("spec/a.yaml"))]),
            (id("GET /c"), vec![(13, at("spec/a.yaml"))]),
            (id("GET /d"), vec![(15, at("spec/d.yaml"))]),
        ]
        .into();
        let renames = vec![("api/a.yaml".to_string(), "spec/a.yaml".to_string())];
        let got = renamed_holders(&removed, &added, &renames);
        assert_eq!(got, [(1, 11), (2, 12)].into());
    }

    #[test]
    fn pairing_follows_moves_then_identity() {
        let old: BTreeSet<u64> = [1, 2, 3, 4].into();
        let new: BTreeSet<u64> = [1, 2, 5, 6].into();
        // 1 and 2 swap ids; 3 moves to 5; 4 is gone; 6 is new.
        let moved: HashMap<u64, u64> = [(1, 2), (2, 1), (3, 5)].into();
        let p = pair(&old, &new, &moved);
        assert_eq!(p.pairs, vec![(1, 2, true), (2, 1, true), (3, 5, true)]);
        assert_eq!((p.removed, p.added), (vec![4], vec![6]));
        let p = pair(&old, &new, &HashMap::new());
        assert_eq!(p.pairs, vec![(1, 1, false), (2, 2, false)]);
        assert_eq!((p.removed, p.added), (vec![3, 4], vec![5, 6]));
    }

    #[test]
    fn orphans_by_reason() {
        let lost = [
            RemovedCall {
                client: 10,
                target: 20,
                category: 1,
            },
            RemovedCall {
                client: 11,
                target: 21,
                category: 1,
            },
            RemovedCall {
                client: 12,
                target: 22,
                category: 1,
            },
            RemovedCall {
                client: 13,
                target: 23,
                category: 1,
            },
            RemovedCall {
                client: 14,
                target: 24,
                category: 1,
            },
        ];
        // 10: target removed; 11 moved to 31, target survives; 12 is gone;
        // 13 still calls something; 14's target survives and nothing pairs it.
        let moved: HashMap<u64, u64> = [(11, 31)].into();
        let present: HashSet<u64> = [10, 31, 13, 14, 21, 23, 24].into();
        let served: HashSet<(u64, u32)> = [(13, 1)].into();
        let removed: HashSet<u64> = [20, 12, 22].into();
        let got = orphaned(&lost, &moved, &present, &served, &removed);
        assert_eq!(
            got,
            vec![
                Orphan {
                    client: 10,
                    target: 20,
                    category: 1,
                    reason: "target_removed"
                },
                Orphan {
                    client: 31,
                    target: 21,
                    category: 1,
                    reason: "pairing_lost"
                },
                Orphan {
                    client: 14,
                    target: 24,
                    category: 1,
                    reason: "pairing_lost"
                },
            ]
        );
    }

    #[test]
    fn an_evm_event_flow_is_read_from_its_subscriber_end() {
        let flow = |emitter: &str| {
            let mut e = Edge::new(
                NodeId(1),
                NodeId(2),
                edge_category::EVENT_FLOWS,
                glia_core::Confidence::Strong,
            );
            e.cells
                .push(Evidence::emitter(emitter).rule("abi").to_cell());
            e
        };
        let call = Edge::new(
            NodeId(3),
            NodeId(4),
            edge_category::RPC_CALLS,
            glia_core::Confidence::Strong,
        );
        // Event 1 -> subscription 2: the subscription is the client.
        assert_eq!(client_end(&flow("resolver:evm")), Some(2));
        assert_eq!(client_end(&call), Some(3));
        // An eventbus flow is no client / provider contract.
        assert_eq!(client_end(&flow("resolver:eventbus")), None);
        let bare = Edge::new(
            NodeId(1),
            NodeId(2),
            edge_category::EVENT_FLOWS,
            glia_core::Confidence::Strong,
        );
        assert_eq!(client_end(&bare), None);

        // The oriented removed edge: subscription 2 lost event 1, which the
        // change removed; subscription 5 is still served by another event.
        let ev = edge_category::EVENT_FLOWS.0;
        let lost = [
            RemovedCall {
                client: 2,
                target: 1,
                category: ev,
            },
            RemovedCall {
                client: 5,
                target: 6,
                category: ev,
            },
        ];
        let present: HashSet<u64> = [2, 5, 6].into();
        let served: HashSet<(u64, u32)> = [(5, ev)].into();
        let removed: HashSet<u64> = [1].into();
        let got = orphaned(&lost, &HashMap::new(), &present, &served, &removed);
        assert_eq!(
            got,
            vec![Orphan {
                client: 2,
                target: 1,
                category: ev,
                reason: "target_removed"
            }]
        );
    }

    #[test]
    fn truncated_side_never_reports_absence() {
        let old = schema(
            r#"{"format":"openapi","response:200":[{"name":"id","type":"string"},{"name":"total","type":"number"}]}"#,
        );
        let new = schema(
            r#"{"format":"openapi","response:200":[{"name":"id","type":"string"}],"truncated":true}"#,
        );
        let d = openapi_evolution(&old, &new);
        assert!(d.suppressed && d.changes.is_empty());
        assert_eq!(
            verdict(d).map(|v| (v.0, v.1)),
            Some(("unknown", Some("truncated")))
        );
        // A whole response section missing from a truncated side is not "removed" either.
        let new = schema(
            r#"{"format":"openapi","request":[{"name":"x","type":"string"}],"truncated":true}"#,
        );
        assert!(openapi_evolution(&old, &new).suppressed);
    }

    #[test]
    fn nested_removal_reports_the_topmost_field_and_refs_are_unknown() {
        let old = schema(
            r##"{"format":"openapi","response:200":[{"name":"customer","type":"object"},{"name":"customer.name","type":"string"},{"name":"lines","type":"#/components/schemas/Line"}]}"##,
        );
        let new = schema(
            r##"{"format":"openapi","response:200":[{"name":"lines","type":"#/components/schemas/Line2"}]}"##,
        );
        let d = openapi_evolution(&old, &new);
        assert_eq!(
            rules(&d),
            [
                ("customer", "producer_only", "response_field_removed", true),
                ("lines", "unknown", "unresolved_ref", false)
            ]
        );
    }

    #[test]
    fn type_changes_follow_the_flow() {
        let old = schema(
            r#"{"format":"openapi","request":[{"name":"n","type":"integer"}],"response:200":[{"name":"n","type":"integer"},{"name":"s","type":"string|null"}]}"#,
        );
        let new = schema(
            r#"{"format":"openapi","request":[{"name":"n","type":"number"}],"response:200":[{"name":"n","type":"number"},{"name":"s","type":"string"}]}"#,
        );
        let d = openapi_evolution(&old, &new);
        assert_eq!(
            rules(&d),
            [
                ("n", "type", "request_type_widened", false),
                ("n", "type", "response_type_changed", true),
                ("s", "type", "response_type_narrowed", false),
            ]
        );
        let old = schema(
            r#"{"format":"asyncapi","payload":[{"name":"n","type":"integer"},{"name":"gone","type":"string"}]}"#,
        );
        let new = schema(
            r#"{"format":"asyncapi","payload":[{"name":"n","type":"number"},{"name":"new","type":"string"}]}"#,
        );
        assert_eq!(
            rules(&asyncapi_evolution(&old, &new)),
            [
                ("n", "type", "payload_type_changed", true),
                ("gone", "producer_only", "payload_field_removed", true),
                ("new", "consumer_only", "payload_field_added", false),
            ]
        );
    }

    #[test]
    fn response_field_now_optional_breaks() {
        let old = schema(
            r#"{"format":"openapi","response:200":[{"name":"id","type":"string","required":true}]}"#,
        );
        let new = schema(r#"{"format":"openapi","response:200":[{"name":"id","type":"string"}]}"#);
        assert_eq!(
            rules(&openapi_evolution(&old, &new)),
            [("id", "required", "response_field_now_optional", true)]
        );
    }

    #[test]
    fn identical_and_gapped_pairs() {
        let a = holder(
            true,
            r#"{"format":"proto","fields":[{"name":"id","type":"string","number":1}]}"#,
        );
        assert!(judge_pair(&a, &a, AvroMode::Backward).is_none());
        let enum_like = Holder {
            payloads: Vec::new(),
            ..holder(true, "")
        };
        assert_eq!(
            judge_pair(&a, &enum_like, AvroMode::Backward).map(|v| (v.0, v.1)),
            Some(("unknown", Some("after_no_fields")))
        );
        let avro = holder(
            true,
            r#"{"format":"avro","fields":[{"name":"id","type":"string"}]}"#,
        );
        assert_eq!(
            judge_pair(&a, &avro, AvroMode::Backward).map(|v| (v.0, v.1)),
            Some(("unknown", Some("format_mismatch")))
        );
        // A reordered declaration differs in bytes, not in the contract.
        let two = holder(
            true,
            r#"{"format":"proto","fields":[{"name":"id","type":"string","number":1},{"name":"t","type":"int64","number":2}]}"#,
        );
        let swapped = holder(
            true,
            r#"{"format":"proto","fields":[{"name":"t","type":"int64","number":2},{"name":"id","type":"string","number":1}]}"#,
        );
        assert!(judge_pair(&two, &swapped, AvroMode::Backward).is_none());
        // An op that loses its only body lists that as a removal.
        let op = holder(
            false,
            r#"{"format":"openapi","response:200":[{"name":"id","type":"string"}]}"#,
        );
        let bare = Holder {
            payloads: Vec::new(),
            ..holder(false, "")
        };
        let v = judge_pair(&op, &bare, AvroMode::Backward).expect("a verdict");
        assert_eq!(v.0, "breaking");
        assert_eq!(v.2.first().map(|c| c.rule), Some("response_removed"));
    }

    #[test]
    fn avro_modes_parse_strictly() {
        assert_eq!(AvroMode::parse("backward"), AvroMode::Backward);
        assert_eq!(AvroMode::parse("forward"), AvroMode::Forward);
        assert_eq!(AvroMode::parse("full"), AvroMode::Full);
        assert_eq!(
            AvroMode::parse("sideways"),
            AvroMode::Full,
            "an unknown mode is judged strictest"
        );
        assert_eq!(ContractBreakArgs::default().avro_mode, AVRO_MODES[0]);
    }
}
