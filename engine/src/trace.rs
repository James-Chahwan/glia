//! **cross_stack_trace** (P3, LD.4a): follow a feature forward across service
//! boundaries and answer with the ranked DISTINCT paths it takes, each hop
//! labelled with its mechanism and whether it crossed a service, plus a
//! two-node mode (`to`) for "how does A reach B".
//!
//! Moved here from `answers` by LD.4a; LD.4b extends it with entry flows
//! ([`entry_flows`], below). Module slot declared by L0.2, reached as
//! `glia_engine::trace::<item>`, never flattened into the crate root.
//!
//! # The answer
//!
//! - `hops`: the forward breadth-first tree over the code profile's carry
//!   edges (`algo::reach::bfs` over `Adjacency::carry`), in discovery order,
//!   exactly what `cross_stack_trace` returned before LD.4a. A consumer that
//!   wants the reached SET reads it.
//! - `paths`: the distinct simple paths from the seed, ranked. One-node mode
//!   enumerates MAXIMAL paths: a path ends where no successor off the path
//!   remains, or at `depth` hops. Two-node mode enumerates the directed paths
//!   that end at the target. Enumeration is a depth-first walk over a
//!   successor map built once per call from `MergedGraph::all_edges` (global
//!   edge order, so deterministic), or from the identity quotient's edges in
//!   that same order (DD.11, "Identity" below), with one successor per
//!   distinct target: of two edges `a -> b` the first in edge order speaks,
//!   the one the BFS tree also reports.
//!
//! # Rank key (no scores)
//!
//! `(cross_service_hops desc, distinct mechanisms desc, length desc, the
//! path's qname sequence asc, its NodeId sequence asc)`. The first
//! `max_paths` are kept (`0` keeps every one); `rank` is the 1-based position.
//!
//! # cross_service
//!
//! A hop crosses a service when its two ends sit in different repos
//! (`cross_repo`, the pre-LD.4a value, carried alongside) OR, when the
//! build's `arch::default_keying` is `ProjectRoots` (one repo with manifest
//! roots below its top level), when `arch::service_of` keys the two ends
//! differently: a manifest-rooted monorepo's `web -> api` hop is cross-service
//! exactly when `glia arch` draws that link. `TopLevelDir` keying is never
//! used for the flag: it would call `src/ -> lib/` cross-service in a plain
//! app. An end is keyed by its located file; an end with none is keyed by the
//! owner segment of its qname (`POST /users @api`, read with
//! `endpoint::split_owner`); an end with neither falls back to the repo
//! comparison.
//!
//! # Blind (FH.9)
//!
//! A trace's `blind` and each entry flow's hold the mechanisms it read (pair
//! categories and boundary kinds on its hops) that `health::boundary_health`
//! rates BLIND or DEGRADED: a path that ends at an unpaired ENDPOINT says the
//! HTTP pairing it could not cross reaches few clients here. One
//! `[blind] surface=trace|flows ..` line per answer.
//!
//! # Bounds
//!
//! Path enumeration is exponential in the worst case, so the walk stops after
//! [`EXPANSION_BUDGET`] path extensions and says so in `truncated` and in the
//! marker, never silently. Two-node mode prunes with a backward walk from the
//! target: a node is entered only when the target is still reachable within
//! the hops left.
//!
//! # Seeds
//!
//! The feature (and `to`) resolve by exact qname (`resolved_by = "qname"`),
//! then exact name (`"name"`) — `answers::resolve_seed` without a scope — then
//! `find`'s top hit when its tier is `exact_ci`, `qname_suffix`,
//! `name_prefix` or `name_word` (`"find"`; never a substring or subsequence
//! tier). Otherwise the answer is an `unknown_symbol` absence with find's
//! nearest qnames as suggestions (`"none"`).
//!
//! The entry-flow tier (LD.4b, one-node mode only): when the feature resolves
//! to a node no carry edge leaves (a dead end — `orders` naming the
//! QUEUE_PRODUCER a handler sends on), or resolves to nothing, and an entry
//! flow's key matches the feature's slug, the seed is that entry instead
//! (`"entry_flow"`). A key matches when it equals the slug, else when either
//! contains the other; among matches, an equal key beats a containing one,
//! then the larger reach within `depth`, then key, qname and id ascending.
//! Only entries reaching at least one node count. Keys come from names alone,
//! so only the matching entries are walked. A hit a carry edge leaves is never
//! replaced. This is the repo-graph wrapper's `nodes_for_feature` fallback,
//! now explainable.
//!
//! The concept tier (GB.1): when that entry-flow tier matched no key, and the
//! feature did not name an exact qname (an exact qname is the node itself, and
//! its dead end stays the answer), the seed is the entry of the top
//! [`concept_flows`] hit over the trace's own depth and identity option
//! (`"concept"`): the flow that TOUCHES the word (`IssueComment` reaching the
//! `issue_comments` table) rather than one named by it. It walks every entry
//! flow, so it runs only where the answer before it was an absence; with no
//! hit the LD.4a answer stands. The `[trace]` marker then reads
//! `resolved_by=concept concept_tier=<tier>`, after the search's own
//! `[concept] .. surface=trace` line. With [`TraceOptions::semantic`] (GB.3)
//! and a feature that resolved to no node, the concept search may answer at
//! its `semantic` tier (module doc "The semantic tier"): the seed is then
//! that flow's entry, `resolved_by = "concept_semantic"` (heuristic), the
//! marker `resolved_by=concept_semantic concept_tier=semantic`.
//!
//! # Entry flows
//!
//! [`entry_flows`] (LD.4b, the wrapper's `_build_flows`): one [`EntryFlow`]
//! per entry point (`CODE_PROFILE.tables.entry`, roles included, and since
//! HF.1 every node carrying an ENTRYPOINT cell — exactly the set liveness
//! seeds from) that reaches anything: its forward BFS tree over the
//! carry edges within `depth`, as the same located [`TraceHop`]s `hops` holds,
//! the services it touches and the mechanisms it uses. One carry index and one
//! [`Locator`] per call serve every entry. Structural edges (DEFINES,
//! CONTAINS, IMPORTS) are not flow, so no flow pulls in a whole module.
//!
//! `services`: each node's service in first-seen order, the entry's first —
//! the repo's label, or in a manifest-rooted monorepo (`ProjectRoots`) the
//! `glia arch` service its file sits in. `TopLevelDir` keying counts the repo
//! as one service, for the reason `cross_service` above never uses it.
//! `cross_service` is `services.len() > 1`.
//!
//! fired_on marker, one line per [`entry_flows`] call:
//! `[flows] entries=<n> flows=<kept> cross_service=<n> depth<=<d>` — grep
//! `[flows] entries=`. It follows LD.6's `[live] annotate surface=flows` line.
//! HF.1: when an entry carries an ENTRYPOINT cell, the next line is
//! `[flows] declared entries=<d> new=<n> sources=(config=<c> main_guard=<m>
//! host=<h> script=<s> other=<o>)`: `d` declared entries, `n` of them the
//! entry table alone would not admit, by the cell's `source` (`config` an
//! overlay `[entrypoints]` pattern, `host:*` a host, `shebang` / `script_arg`
//! a script) — grep `[flows] declared entries=`.
//!
//! # Concept search
//!
//! [`concept_flows`] (GB.1, §F item 6): the entry flows that touch a word,
//! never only the flows named by it (`cache` finds the flows that read a
//! CACHE). A word is a run of letters and digits split again at each
//! lower -> upper case step, lower-cased, two characters or more
//! (`IssueComment` -> issue, comment); a query word matches a fact word when
//! they are equal or one is the other plus `s` / `es` (no stemmer). Per flow
//! (one [`entry_flows`] walk), a word's best match is the least (tier, depth)
//! over the entry (depth 0) and the hops in BFS order, the tiers best first
//! ([`ConceptTier`]):
//!
//! 1. `entry`: the entry's key or name;
//! 2. `hop_name`: a hop node's name (a DATA_ENTITY, DATABASE, CACHE,
//!    BLOB_STORE or SEARCH_INDEX hop by its own name too);
//! 3. `hop_qname`: a word of a node's qname;
//! 4. `kind`: a node's kind name (`CACHE`, `QUEUE_PRODUCER` -> queue,
//!    producer);
//! 5. `mechanism`: the hop's edge category (`ACCESSES_DATA` -> accesses,
//!    data);
//! 6. `data_entity`: the name of a data node of those five kinds a node has
//!    an ACCESSES_DATA / USES edge to, read one hop out, never walked;
//! 7. `external_host`: a host of a node stamped ORIGIN `external` (CJ.3,
//!    the reader `effects` and `serves` share);
//! 8. `doc`: the heading of a DOC_SECTION with a DOCUMENTS edge into a node
//!    (the heading only, never the section's text: a typed link, not text
//!    similarity).
//!
//! The entry node's own qname, kind, data, host and doc facts count at depth
//! 0. A multi-word query matches a flow when EVERY word matches; the row's
//! tier is the weakest of the words' best tiers, its `hop` / `depth` the
//! deepest of their matches (`None` / 0 when that is the entry). Rows rank by
//! (tier, depth asc, reach desc, key, entry qname), then the flow's position
//! in [`entry_flows`]' order, never a NodeId; `limit` keeps the first. Facts
//! are computed once per shown node per call. An empty answer is a FACT
//! `no_match` absence. Lexical and typed only: the one similarity tier is
//! GB.3's fallback below, after every lexical tier missed.
//!
//! # The semantic tier
//!
//! GB.3 (ruling FB-7-FUSED: only the blend FB.6b's gate passed). When the
//! lexical tiers above match NO flow, [`ConceptOptions::semantic`] holds a
//! [`SemanticSource`] and [`ConceptOptions::rank_cache`] knows exactly one
//! repo root over a one-repo merge, the search runs `find --files`' order
//! ([`find_files`]: `LX[IDR,FUSE[T1s,SEMp:t2]]`, or the repo's calibrated
//! `SELf2t` pick) and keeps the files with a `sem` score, numbered 1.. in that
//! order. A flow matches at tier `semantic` (heuristic) when its entry
//! (depth 0) or a hop sits in such a file; its match is its least (file rank,
//! depth) node, the entry before the hops in BFS order. Rows rank by (file
//! rank, depth, reach desc, key, entry qname), then the flow's position: the
//! fused file order decides, never a node's similarity. Each row's `matched`
//! is one entry, `<query> -> fused file #<r> <path> (no lexical match)`, its
//! `hop` / `depth` the matched node's, and [`ConceptHit::similarity`] that
//! node's cosine to the prefixed query (4 dp; absent for a node with no
//! vector), an annotation. Presence stays lexical: a lexical row anywhere
//! means no semantic row, and the semantic tier never seeds a walk of its
//! own (every flow is an entry's own walk; it only picks among them). An
//! [`Answer`] holds an absence only with no row, so the lexical miss travels
//! in the tier itself and in each row's `matched`; with no semantic row
//! either, the answer is the lexical `no_match` absence unchanged.
//!
//! fired_on marker, one line per semantic attempt (a source set, the lexical
//! tiers empty), before the `[concept]` line:
//! `[flows] semantic query=<q> files=<f> flows=<n> top_file_rank=<r> not_embedded=<x>`
//! (`files`: fused files with a `sem` score; `flows`: semantic rows before
//! the limit; `top_file_rank`: the first row's file rank, `-` with none;
//! `not_embedded`: rows whose matched node has no vector) — grep
//! `^\[flows\] semantic query=`. A guard that fails (no cache, several
//! repos, an embedder error) prints `files=0 flows=0`.
//!
//! fired_on marker, one line per search:
//! `[concept] word=<w> flows=<f> hits=<h> tiers=<tier:n,..> surface=<s> ms=<t>`
//! (`tiers=-` with no hit; `hits` before the limit; `ms` the search's wall
//! time, flows walk included) — grep `^\[concept\] word=`. The search walks
//! the flows quietly: no `[flows]` / `[live]` / `[blind]` line of its own.
//!
//! # Trace marker
//!
//! fired_on marker, one line per [`cross_stack_trace`] call:
//! `[trace] seed=<query> resolved_by=<r> paths=<n> expanded=<n> truncated=<bool>`
//! (`resolved_by=concept` is followed by ` concept_tier=<tier>`; two-node mode
//! appends `to=<query> directed=<true|false|->`, `-` when no
//! path was found) — grep `[trace] seed=`.
//! The LD.6 `[live] annotate surface=trace` line still prints once per call.
//!
//! # Identity (DD.11)
//!
//! Both answers walk the identity quotient. One [`IdentityView`] per call,
//! from [`TraceOptions::collapse`] (trace) or the `collapse` argument
//! (flows). With walk classes (a message, contract op, SCIP symbol, or a
//! package and its project, found in several places) the carry index, the
//! successor map and so the BFS tree, the path enumeration and every entry
//! flow run over [`IdentityView::quotient`]: a class is ONE node, so a path
//! that reaches one copy continues through every copy's successors at zero
//! hops, and two copies' identical paths are one distinct path. Without a
//! walk class (collapse off, no SAME_AS edge, only `package_name` classes)
//! the walk is the HEAD walk, byte for byte.
//!
//! - **Seeds.** The seed, the target and every entry walk from their walk
//!   node ([`IdentityView::walk_rep`]). A target in the seed's walk class is
//!   the zero-length path, as `seed == target` is. `seed` / `target` still
//!   locate the nodes the queries resolved to. Two entries in one walk class
//!   are one flow; its `entry` is the class's display member when that is one
//!   of the entries, else the first of them in entry order.
//! - **Hops.** A walk node that stands for a walk class shows the class's
//!   display member (content-chosen, [`IdentityView::display`]; the walk
//!   node's id is an internal handle) in `from_qname` and `to_*`; any other
//!   walk node shows itself. `cross_service` / `cross_repo` compare the shown
//!   ends, and the rank key's qname sequence is the shown nodes'. A hop into
//!   a FACT / DERIVED class (walked or not: one `package:npm:react` reached
//!   in one repo also carries it) carries `to_identity` with every other
//!   member in `also_in`. A walk class's `to_live` holds when ANY member is
//!   live (DD.10's rule).
//! - **The two-node fallback** (`MergedGraph::shortest_path`, the undirected
//!   last resort) stays on the raw graph; its hops show the raw nodes.
//!
//! fired_on, one line per answer and only when the view has a collapse
//! class, after the `[trace] seed=` / `[flows] entries=` line:
//! `[identity] collapse surface=trace|flows classes=<c> walk_classes=<w> folded=<f> rows=<n>`.
//! `rows` is the `[live] annotate` rows count (trace: the BFS tree's hops;
//! flows: every flow's hops), `folded` the class members those rows (and,
//! for flows, the entries) stand for beyond the one each shows. grep
//! `^\[identity\] collapse surface=trace` / `surface=flows`.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use glia_activation::algo::{Adjacency, CategorySet, GraphSource, Walk, reach};
use glia_code_domain::profile::node_ext;
use glia_code_domain::{cell_type, edge_category, endpoint, node_kind};
use glia_core::{Cell, CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use glia_graph::roles::roles_in;
use glia_graph::{MergedGraph, Reach};

use crate::absence::{self, Absence, Answer, BlindCaveat};
use crate::answers::{Located, Locator, entrypoint_reachable, live_marker};
use crate::arch::{ServiceKeying, default_keying, service_of};
use crate::find::{self, FindFilesOptions, FindOptions, FoundNode, find_files};
use crate::health;
use crate::identity::{Collapse, IdentityView, RowIdentity};
use crate::profile::CODE_PROFILE;
use crate::rank::RankCache;
use crate::semantic::SemanticSource;
use crate::serves::external_hosts;

/// `TraceOptions::default().depth`.
pub const DEFAULT_DEPTH: usize = 6;
/// `TraceOptions::default().max_paths`.
pub const DEFAULT_MAX_PATHS: usize = 10;
/// Path extensions the depth-first enumeration may make per call before it
/// stops and reports `truncated`.
pub const EXPANSION_BUDGET: usize = 50_000;

/// The `find` tiers a seed may resolve through when no qname or name matches
/// exactly: the ones that still name the query, never a substring or
/// subsequence match.
const FIND_TIERS: [&str; 4] = ["exact_ci", "qname_suffix", "name_prefix", "name_word"];

/// How far and to what a trace walks. Start from `default()` and set fields:
/// `#[non_exhaustive]` rules out a struct literal outside this crate.
/// Equality compares the two shared handles by pointer.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TraceOptions {
    /// Maximum hops per path, and the depth of the `hops` tree.
    pub depth: usize,
    /// Two-node mode: a qname or name to trace TO.
    pub to: Option<String>,
    /// Keep the first `max_paths` ranked paths; `0` keeps every one.
    pub max_paths: usize,
    /// Identity classes (DD.11, module doc "Identity"): on by default, with
    /// no repo labels; `Collapse::off()` is the per-member walk.
    pub collapse: Collapse,
    /// GB.3: the concept tier's semantic fallback ([`ConceptOptions::semantic`]):
    /// with a source and a [`TraceOptions::rank_cache`], a word no lexical
    /// concept tier matches may still seed from the top `semantic` flow
    /// (`resolved_by` `concept_semantic`). None (the default) is GB.1's
    /// answer byte for byte.
    pub semantic: Option<Arc<SemanticSource>>,
    /// GB.3: the holder's ranking cache the semantic fallback ranks files
    /// through ([`ConceptOptions::rank_cache`]).
    pub rank_cache: Option<Arc<RankCache>>,
}

impl Default for TraceOptions {
    fn default() -> Self {
        TraceOptions {
            depth: DEFAULT_DEPTH,
            to: None,
            max_paths: DEFAULT_MAX_PATHS,
            collapse: Collapse::default(),
            semantic: None,
            rank_cache: None,
        }
    }
}

impl PartialEq for TraceOptions {
    fn eq(&self, other: &Self) -> bool {
        self.depth == other.depth
            && self.to == other.to
            && self.max_paths == other.max_paths
            && self.collapse == other.collapse
            && same_arc(&self.semantic, &other.semantic)
            && same_arc(&self.rank_cache, &other.rank_cache)
    }
}

impl Eq for TraceOptions {}

/// Two optional shared handles are equal when both are None or both point at
/// one value.
fn same_arc<T: ?Sized>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        _ => false,
    }
}

/// One hop in a cross-stack trace: a typed edge from one entity to the next,
/// with the `mechanism` (edge category) and whether it crossed a service
/// boundary. The destination is located.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TraceHop {
    /// In `hops`, the BFS depth; in a path, the hop's 1-based position.
    pub depth: usize,
    /// Edge category name — the mechanism (`CALLS`, `HTTP_CALLS`, `QUEUE_FLOWS`…).
    pub mechanism: &'static str,
    /// The two ends sit in different services: different repos, or (a single
    /// repo keyed by manifest project roots) different `glia arch` services.
    pub cross_service: bool,
    /// The two ends sit in different repos (the pre-LD.4a `cross_service`).
    pub cross_repo: bool,
    pub from_qname: String,
    pub to_qname: String,
    pub to_kind: &'static str,
    /// The destination is reachable from an entrypoint (LD.6, the flag
    /// `BlastAnswer::live` carries): `false` = likely dead.
    pub to_live: bool,
    pub to_file: Option<String>,
    /// 1-based (see [`Located`]).
    pub to_line: Option<i64>,
    /// The destination stands for a FACT / DERIVED identity class (DD.11):
    /// its tier, rules and every other member (`also_in`). No JSON key when
    /// `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_identity: Option<RowIdentity>,
}

/// One ranked path: its hops in order, and the three facts it ranks by.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TracePath {
    /// 1-based position in the rank order.
    pub rank: usize,
    pub hops: Vec<TraceHop>,
    /// Hops with `cross_service` set.
    pub cross_service_hops: usize,
    /// Distinct hop mechanisms, in first-appearance order along the path.
    pub mechanisms: Vec<&'static str>,
    /// Hop count.
    pub length: usize,
    /// `true` for a path that follows carry edges forward. `false` only for
    /// the two-node fallback: the shortest path over ANY edge category, walked
    /// either way, when no directed carry path exists.
    pub directed: bool,
}

/// The whole trace answer. `absence` is `Some` exactly when `paths` is empty.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct TraceAnswer {
    /// The node the feature resolved to.
    pub seed: Option<Located>,
    /// Two-node mode: the node `to` resolved to.
    pub target: Option<Located>,
    /// How the seed resolved: `qname`, `name`, `find`, `entry_flow` (a dead
    /// end or an unresolved word yielded to the entry flow its key names,
    /// LD.4b), `concept` (no key named it, and the entry of the top
    /// [`concept_flows`] hit, the flow that touches the word, seeds instead,
    /// GB.1), `concept_semantic` (GB.3: the feature resolved to nothing, no
    /// lexical concept tier matched, and the top [`ConceptTier::Semantic`]
    /// flow's entry seeds it: a heuristic seed) or `none`.
    pub resolved_by: &'static str,
    /// The seed's forward BFS tree over the carry edges, in discovery order.
    pub hops: Vec<TraceHop>,
    /// The ranked distinct paths.
    pub paths: Vec<TracePath>,
    /// The path enumeration hit [`EXPANSION_BUDGET`]: `paths` ranks only the
    /// paths found before it stopped.
    pub truncated: bool,
    pub absence: Option<Absence>,
    /// FH.9: the mechanisms the trace read (the pair categories and boundary
    /// kinds on its hops, paths, seed and target) that health rates BLIND or
    /// DEGRADED; empty, and not serialized, otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blind: Vec<BlindCaveat>,
}

/// One entry point's forward flow (LD.4b): what it reaches over the carry
/// edges within the walk's depth — see the module doc's "Entry flows".
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EntryFlow {
    /// The flow's feature word: the entry's name lower-cased, with spaces and
    /// hyphens as `_` (`POST /orders` -> `post_/orders`), the repo-graph
    /// wrapper's spelling. Not unique: two repos serving `GET /x` give two
    /// rows with one key.
    pub key: String,
    /// The entry point, located.
    pub entry: Located,
    /// Nodes the flow reaches (`hops.len()`), at least 1.
    pub reach: usize,
    /// `services` names more than one service.
    pub cross_service: bool,
    /// Distinct hop mechanisms (edge categories), in first-seen order.
    pub mechanisms: Vec<&'static str>,
    /// The services the flow touches, the entry's first, then in first-seen
    /// order along `hops`.
    pub services: Vec<String>,
    /// The entry's forward BFS tree, in discovery order: the hops a
    /// [`TraceAnswer::hops`] seeded at the entry would hold.
    pub hops: Vec<TraceHop>,
    /// FH.9: the mechanisms this flow read (the entry's kind, its hops' pair
    /// categories and boundary kinds) that health rates BLIND or DEGRADED;
    /// empty, and not serialized, otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blind: Vec<BlindCaveat>,
}

/// **cross_stack_trace** (P3): the ranked paths a feature takes across the
/// stack — see the module doc. Never an error: an unknown feature, an unknown
/// `to`, a dead end and an unreachable target are absences.
///
/// Deliberately takes NO `scope` (A8.3): a trace's entire value is that it
/// crosses service/directory boundaries, so filtering its hops would delete
/// the answer.
///
/// Each hop's `to_live` (LD.6) is read off one [`entrypoint_reachable`] walk,
/// run once per call; [`cross_stack_trace_with_live`] takes the set instead.
pub fn cross_stack_trace(merged: &MergedGraph, feature: &str, opts: &TraceOptions) -> TraceAnswer {
    cross_stack_trace_with_live(merged, &entrypoint_reachable(merged), feature, opts)
}

/// [`cross_stack_trace`] over a live set the caller already holds (pyo3's
/// `PyGraph` computes it once per graph): `live` must be
/// [`entrypoint_reachable`] of `merged`.
pub fn cross_stack_trace_with_live(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    feature: &str,
    opts: &TraceOptions,
) -> TraceAnswer {
    let mut answer = trace_answer(merged, live, feature, opts);
    answer.blind = health::blind_caveats(merged, "trace", &trace_mechanisms(&answer));
    answer
}

/// FH.9: the mechanisms a trace read: every pair category on its hops and
/// paths, and every boundary kind among their destinations, the seed and the
/// target (a trace that stops at an unpaired ENDPOINT read HTTP).
fn trace_mechanisms(a: &TraceAnswer) -> Vec<&'static str> {
    let hops = || {
        a.hops
            .iter()
            .chain(a.paths.iter().flat_map(|p| p.hops.iter()))
    };
    health::mechanisms_read(
        hops().map(|h| h.mechanism),
        hops()
            .map(|h| h.to_kind)
            .chain(a.seed.iter().map(|s| s.kind))
            .chain(a.target.iter().map(|t| t.kind)),
    )
}

/// [`cross_stack_trace_with_live`]'s answer before its `blind` rows.
fn trace_answer(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    feature: &str,
    opts: &TraceOptions,
) -> TraceAnswer {
    let mut seed = resolve(merged, feature);
    // DD.11: the walk runs over the identity quotient when the view has
    // walk classes (module doc "Identity").
    let view = IdentityView::new(merged, &opts.collapse);
    let quotient = view.quotient(merged);
    let walk = walk_live(&view, live, quotient.is_some());
    let (adj, succ) = match &quotient {
        Some(q) => (
            Adjacency::carry(q, &CODE_PROFILE.tables),
            Successors::carry_over(q),
        ),
        None => (
            Adjacency::carry(merged, &CODE_PROFILE.tables),
            Successors::carry(merged),
        ),
    };
    // The entry-flow tier (LD.4b, module doc): one-node mode, a dead end or
    // nothing resolved, and an entry key naming the feature; else the concept
    // tier (GB.1): the top entry flow touching the feature.
    let mut concept: Option<ConceptTier> = None;
    if opts.to.is_none() && !seed.id.is_some_and(|id| leaves(&succ, view.walk_rep(id))) {
        if let Some(entry) = entry_seed(merged, &view, &adj, feature, opts.depth) {
            seed = Resolved {
                id: Some(entry),
                by: "entry_flow",
                near: Vec::new(),
            };
        } else if seed.by != "qname"
            && let Some((entry, tier)) =
                concept_seed(merged, live, feature, opts, seed.id.is_none())
        {
            seed = Resolved {
                id: Some(entry),
                by: if tier == ConceptTier::Semantic {
                    "concept_semantic"
                } else {
                    "concept"
                },
                near: Vec::new(),
            };
            concept = Some(tier);
        }
    }
    let Some(seed_id) = seed.id else {
        live_marker("trace", 0, 0);
        let absence = absence::unknown_symbol(merged, "trace", feature, &[], &seed.near);
        marker(feature, (seed.by, concept), 0, 0, false, opts, None);
        view.collapse_marker("trace", 0, 0);
        return TraceAnswer {
            seed: None,
            target: None,
            resolved_by: seed.by,
            hops: Vec::new(),
            paths: Vec::new(),
            truncated: false,
            absence: Some(absence),
            blind: Vec::new(),
        };
    };
    let walk_seed = view.walk_rep(seed_id);

    let loc = Locator::new(merged);
    let mut sides = Sides::new(merged, &loc, &view, &walk);

    // The BFS tree (LD.15b), unchanged: each reached node is one hop, in
    // discovery order, from the node that first reached it.
    let reached = reach::bfs(&adj, &[walk_seed], Walk::Forward, opts.depth).reached;
    let hops: Vec<TraceHop> = reached
        .iter()
        .map(|r| sides.hop(r.depth, r.parent, r.id, r.via))
        .collect();
    live_marker(
        "trace",
        hops.len(),
        hops.iter().filter(|h| h.to_live).count(),
    );
    let folded: usize = reached.iter().map(|r| walk_folded(&view, r.id)).sum();
    let collapse_marker = || view.collapse_marker("trace", hops.len() + folded, hops.len());

    let seed_at = loc.locate(seed_id);

    let Some(to_query) = opts.to.as_deref() else {
        let walked = enumerate(&succ, &mut sides, walk_seed, None, opts);
        let paths = materialize(&succ, &mut sides, &walked.kept);
        let absence = paths.is_empty().then(|| {
            let note = if opts.depth == 0 && leaves(&succ, walk_seed) {
                format!("depth 0 follows no edge from `{}`", seed_at.qname)
            } else {
                format!("no carry edge leaves `{}` in this graph", seed_at.qname)
            };
            absence::empty(
                merged,
                "trace",
                feature,
                "no_edges",
                note,
                mechanisms_of(merged, seed_id),
                seed_at.file.as_deref(),
            )
        });
        marker(
            feature,
            (seed.by, concept),
            paths.len(),
            walked.expanded,
            walked.truncated,
            opts,
            None,
        );
        collapse_marker();
        return TraceAnswer {
            seed: Some(seed_at),
            target: None,
            resolved_by: seed.by,
            hops,
            paths,
            truncated: walked.truncated,
            absence,
            blind: Vec::new(),
        };
    };

    // Two-node mode.
    let target = resolve(merged, to_query);
    let Some(target_id) = target.id else {
        let absence = absence::unknown_symbol(merged, "trace", to_query, &[], &target.near);
        marker(feature, (seed.by, concept), 0, 0, false, opts, None);
        collapse_marker();
        return TraceAnswer {
            seed: Some(seed_at),
            target: None,
            resolved_by: seed.by,
            hops,
            paths: Vec::new(),
            truncated: false,
            absence: Some(absence),
            blind: Vec::new(),
        };
    };
    let target_at = loc.locate(target_id);
    let walk_target = view.walk_rep(target_id);

    // A target in the seed's walk class is the seed itself.
    let (mut paths, expanded, truncated) = if walk_seed == walk_target {
        let zero = TracePath {
            rank: 1,
            hops: Vec::new(),
            cross_service_hops: 0,
            mechanisms: Vec::new(),
            length: 0,
            directed: true,
        };
        (vec![zero], 0, false)
    } else {
        // Hops left to the target, from a backward walk over the same carry
        // index: the enumeration never enters a node the target is out of
        // reach from.
        let mut to_target: HashMap<NodeId, usize> =
            reach::bfs(&adj, &[walk_target], Walk::Backward, opts.depth)
                .reached
                .iter()
                .map(|r| (r.id, r.depth))
                .collect();
        to_target.insert(walk_target, 0);
        let walked = enumerate(
            &succ,
            &mut sides,
            walk_seed,
            Some((walk_target, &to_target)),
            opts,
        );
        (
            materialize(&succ, &mut sides, &walked.kept),
            walked.expanded,
            walked.truncated,
        )
    };
    // The undirected last resort walks the raw graph (module doc).
    if paths.is_empty()
        && let Some(steps) = merged.shortest_path(seed_id, target_id, Reach::Both, None, opts.depth)
    {
        paths.push(undirected_path(&mut sides, live, &steps));
    }
    let directed = paths.first().map(|p| p.directed);
    let absence = paths.is_empty().then(|| {
        let mut mechanisms: Vec<&'static str> = Vec::new();
        for id in [seed_id, target_id] {
            for m in mechanisms_of(merged, id) {
                if !mechanisms.contains(m) {
                    mechanisms.push(m);
                }
            }
        }
        let note = format!(
            "no path from `{}` to `{}` within {} hops, over carry edges forward or any edge either way",
            seed_at.qname, target_at.qname, opts.depth
        );
        absence::empty(merged, "trace", feature, "no_edges", note, &mechanisms, seed_at.file.as_deref())
    });
    marker(
        feature,
        (seed.by, concept),
        paths.len(),
        expanded,
        truncated,
        opts,
        directed,
    );
    collapse_marker();
    TraceAnswer {
        seed: Some(seed_at),
        target: Some(target_at),
        resolved_by: seed.by,
        hops,
        paths,
        truncated,
        absence,
        blind: Vec::new(),
    }
}

/// **entry_flows** (LD.4b): every entry point's forward flow — see the module
/// doc's "Entry flows". `repo_labels` is the build's
/// (`GenerateResult::repo_labels`); `services` names repos by it. Rows are
/// ordered by (key, entry qname, entry id) and every one is kept, so two
/// entries with one key are two rows. Entries reaching nothing within `depth`
/// have no row (`depth = 0` keeps none).
///
/// Identity classes collapse (DD.11, module doc "Identity") with
/// `repo_labels` naming the `also_in` repos: two entries in one walk class
/// are one flow, and a walk class is one node of every flow.
///
/// Each hop's `to_live` is read off one [`entrypoint_reachable`] walk, run
/// once per call; [`entry_flows_with_live`] takes the set instead.
pub fn entry_flows(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    depth: usize,
) -> Vec<EntryFlow> {
    entry_flows_with_live(
        merged,
        &entrypoint_reachable(merged),
        repo_labels,
        depth,
        &Collapse::with_labels(repo_labels),
    )
}

/// [`entry_flows`] over a live set the caller already holds (pyo3's `PyGraph`
/// computes it once per graph): `live` must be [`entrypoint_reachable`] of
/// `merged`. `collapse` is the identity option (DD.11); `Collapse::off()`
/// is one flow per entry over the raw graph, the pre-DD.11 answer.
pub fn entry_flows_with_live(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    repo_labels: &BTreeMap<u64, String>,
    depth: usize,
    collapse: &Collapse,
) -> Vec<EntryFlow> {
    walk_flows(merged, live, repo_labels, depth, collapse, true)
        .into_iter()
        .map(|w| w.flow)
        .collect()
}

/// One walked entry flow with the nodes its rows show: the entry and, per
/// hop, the node the hop's `to_*` fields name ([`show`] of the walk node).
/// Concept search (GB.1) reads those nodes' facts.
struct WalkedFlow {
    flow: EntryFlow,
    entry_id: NodeId,
    hop_ids: Vec<NodeId>,
}

/// [`entry_flows_with_live`]'s walk, keeping each flow's shown node ids.
/// `markers`: print the `[live] annotate`, `[blind]`, `[flows] entries=` and
/// `[identity] collapse` lines (`entry_flows`); concept search, which prints
/// its own `[concept]` line, passes `false`.
fn walk_flows(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    repo_labels: &BTreeMap<u64, String>,
    depth: usize,
    collapse: &Collapse,
    markers: bool,
) -> Vec<WalkedFlow> {
    let entries = entries(merged);
    let view = IdentityView::new(merged, collapse);
    let quotient = view.quotient(merged);
    let walk = walk_live(&view, live, quotient.is_some());
    let loc = Locator::new(merged);
    let mut sides = Sides::new(merged, &loc, &view, &walk);
    let adj = match &quotient {
        Some(q) => Adjacency::carry(q, &CODE_PROFILE.tables),
        None => Adjacency::carry(merged, &CODE_PROFILE.tables),
    };
    // One flow per walk node, in first-entry order: the entries of one walk
    // class are one flow.
    let mut groups: Vec<(NodeId, Vec<usize>)> = Vec::new();
    let mut group_of: HashMap<NodeId, usize> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        let w = view.walk_rep(e.id);
        match group_of.get(&w) {
            Some(&g) => groups[g].1.push(i),
            None => {
                group_of.insert(w, groups.len());
                groups.push((w, vec![i]));
            }
        }
    }
    let mut flows: Vec<WalkedFlow> = Vec::new();
    let mut folded = 0usize;
    for (w, members) in &groups {
        let reached = reach::bfs(&adj, &[*w], Walk::Forward, depth).reached;
        if reached.is_empty() {
            continue;
        }
        // The row's entry: the class's display member when it is one of the
        // class's entries, else the first of them (any other walk node shows
        // itself, its one entry).
        let display = show(&view, *w);
        let e = members
            .iter()
            .map(|&i| &entries[i])
            .find(|e| e.id == display)
            .unwrap_or(&entries[members[0]]);
        folded += walk_folded(&view, *w)
            + reached
                .iter()
                .map(|r| walk_folded(&view, r.id))
                .sum::<usize>();
        let hops: Vec<TraceHop> = reached
            .iter()
            .map(|r| sides.hop(r.depth, r.parent, r.id, r.via))
            .collect();
        let mut mechanisms: Vec<&'static str> = Vec::new();
        for h in &hops {
            if !mechanisms.contains(&h.mechanism) {
                mechanisms.push(h.mechanism);
            }
        }
        let mut services: Vec<String> = Vec::new();
        for id in std::iter::once(e.id).chain(reached.iter().map(|r| show(&view, r.id))) {
            if let Some(s) = sides.service_label(id, repo_labels)
                && !services.contains(&s)
            {
                services.push(s);
            }
        }
        let entry = loc.locate(e.id);
        flows.push(WalkedFlow {
            flow: EntryFlow {
                key: slug(e.name),
                reach: hops.len(),
                cross_service: services.len() > 1,
                mechanisms,
                services,
                blind: Vec::new(),
                hops,
                entry,
            },
            entry_id: e.id,
            hop_ids: reached.iter().map(|r| show(&view, r.id)).collect(),
        });
    }
    // FH.9: one health pass for every flow, run only when one read a
    // mechanism; each flow's rows from its own mechanisms.
    let read: Vec<Vec<&'static str>> = flows
        .iter()
        .map(|w| {
            let f = &w.flow;
            health::mechanisms_read(
                f.hops.iter().map(|h| h.mechanism),
                std::iter::once(f.entry.kind).chain(f.hops.iter().map(|h| h.to_kind)),
            )
        })
        .collect();
    let union: Vec<&'static str> = health::all_mechanisms()
        .into_iter()
        .filter(|m| read.iter().any(|r| r.contains(m)))
        .collect();
    let rows = if union.is_empty() {
        Vec::new()
    } else {
        health::boundary_health(merged).mechanisms
    };
    for (w, mechs) in flows.iter_mut().zip(&read) {
        w.flow.blind = health::caveats_in(&rows, mechs);
    }
    if markers {
        health::blind_marker("flows", &union, &health::caveats_in(&rows, &union));
    }
    flows.sort_by(|a, b| {
        let (a, b) = (&a.flow, &b.flow);
        a.key
            .cmp(&b.key)
            .then_with(|| a.entry.qname.cmp(&b.entry.qname))
            .then_with(|| a.entry.id.cmp(&b.entry.id))
    });
    if markers {
        let (rows, live_rows) = flows.iter().fold((0, 0), |(n, l), w| {
            (
                n + w.flow.hops.len(),
                l + w.flow.hops.iter().filter(|h| h.to_live).count(),
            )
        });
        live_marker("flows", rows, live_rows);
        eprintln!(
            "[flows] entries={} flows={} cross_service={} depth<={depth}",
            entries.len(),
            flows.len(),
            flows.iter().filter(|w| w.flow.cross_service).count()
        );
        declared_marker(&entries);
        view.collapse_marker("flows", rows + folded, rows);
    }
    flows
}

// ============================================================================
// Concept search (GB.1)
// ============================================================================

/// `ConceptOptions::default().limit`.
pub const DEFAULT_CONCEPT_LIMIT: usize = 20;

/// How a flow touches a query word, best first (module doc "Concept
/// search"). A row's tier is the weakest of its words' best tiers.
#[derive(serde::Serialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConceptTier {
    /// The entry's key or name.
    Entry,
    /// A hop node's name (a DATA_ENTITY / DATABASE / CACHE .. hop included).
    HopName,
    /// A segment of a hop node's qname.
    HopQname,
    /// A hop node's kind name (`CACHE`, `QUEUE_PRODUCER`).
    Kind,
    /// A hop's edge category (`ACCESSES_DATA`, `QUEUE_FLOWS`).
    Mechanism,
    /// The name of a data node (DATA_ENTITY, DATABASE, CACHE, BLOB_STORE,
    /// SEARCH_INDEX) a hop node has an ACCESSES_DATA / USES edge to, one hop
    /// out, not walked.
    DataEntity,
    /// A host of an ENDPOINT (or client) the hop reaches that is stamped
    /// ORIGIN `external` (CJ.3).
    ExternalHost,
    /// The heading of a DOC_SECTION with a DOCUMENTS edge into a hop node.
    Doc,
    /// GB.3, heuristic: no lexical tier matched any flow, and the flow runs
    /// through a file `find --files`' fused order ranks with a `sem` score
    /// (module doc "The semantic tier"). Never mixed with the tiers above.
    Semantic,
}

impl ConceptTier {
    /// The snake_case name the JSON and the markers print.
    pub fn as_str(self) -> &'static str {
        match self {
            ConceptTier::Entry => "entry",
            ConceptTier::HopName => "hop_name",
            ConceptTier::HopQname => "hop_qname",
            ConceptTier::Kind => "kind",
            ConceptTier::Mechanism => "mechanism",
            ConceptTier::DataEntity => "data_entity",
            ConceptTier::ExternalHost => "external_host",
            ConceptTier::Doc => "doc",
            ConceptTier::Semantic => "semantic",
        }
    }

    const ALL: [ConceptTier; 9] = [
        ConceptTier::Entry,
        ConceptTier::HopName,
        ConceptTier::HopQname,
        ConceptTier::Kind,
        ConceptTier::Mechanism,
        ConceptTier::DataEntity,
        ConceptTier::ExternalHost,
        ConceptTier::Doc,
        ConceptTier::Semantic,
    ];
}

/// What [`concept_flows`] walks and keeps. Start from `default()` and set
/// fields: `#[non_exhaustive]` rules out a struct literal outside this crate.
/// Equality compares the two shared handles by pointer.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ConceptOptions {
    /// Maximum hops each entry flow follows ([`DEFAULT_DEPTH`]).
    pub depth: usize,
    /// Keep the first `limit` ranked rows ([`DEFAULT_CONCEPT_LIMIT`]); `0`
    /// keeps every one.
    pub limit: usize,
    /// Identity classes, as [`entry_flows_with_live`] takes them: on by
    /// default, with no repo labels.
    pub collapse: Collapse,
    /// The surface the `[concept]` marker names (`cli`, `py`, `trace`;
    /// `lib` by default).
    pub surface: &'static str,
    /// GB.3 (ruling FB-7-FUSED): the repo's t2 vectors and the query
    /// embedder. With one and a [`ConceptOptions::rank_cache`] over exactly
    /// one repo, a word the lexical tiers match in no flow gets the
    /// [`ConceptTier::Semantic`] rows (module doc "The semantic tier"). None
    /// (the default) is GB.1's answer byte for byte.
    pub semantic: Option<Arc<SemanticSource>>,
    /// GB.3: the holder's ranking cache (its `FileIndex`, calibration record
    /// and repo root) the semantic tier's fused file order is ranked through.
    pub rank_cache: Option<Arc<RankCache>>,
}

impl Default for ConceptOptions {
    fn default() -> Self {
        ConceptOptions {
            depth: DEFAULT_DEPTH,
            limit: DEFAULT_CONCEPT_LIMIT,
            collapse: Collapse::default(),
            surface: "lib",
            semantic: None,
            rank_cache: None,
        }
    }
}

impl PartialEq for ConceptOptions {
    fn eq(&self, other: &Self) -> bool {
        self.depth == other.depth
            && self.limit == other.limit
            && self.collapse == other.collapse
            && self.surface == other.surface
            && same_arc(&self.semantic, &other.semantic)
            && same_arc(&self.rank_cache, &other.rank_cache)
    }
}

impl Eq for ConceptOptions {}

/// One entry flow that touches the query, with what matched and where.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ConceptHit {
    /// 1-based position in the rank order.
    pub rank: usize,
    /// The flow's key ([`EntryFlow::key`]).
    pub key: String,
    /// The flow's entry, located.
    pub entry: Located,
    /// The weakest of the query words' best tiers.
    pub tier: ConceptTier,
    /// Per query word, in query order, what it matched:
    /// `cache -> kind CACHE`, `orders -> entry POST /orders`.
    pub matched: Vec<String>,
    /// The matching hop: the deepest of the words' matches. `None` when every
    /// word matched the entry itself (depth 0).
    pub hop: Option<TraceHop>,
    /// The matching hop's depth, 0 for the entry.
    pub depth: usize,
    /// Nodes the flow reaches ([`EntryFlow::reach`]).
    pub reach: usize,
    /// The flow's distinct mechanisms ([`EntryFlow::mechanisms`]).
    pub mechanisms: Vec<&'static str>,
    /// The flow's FH.9 caveats, copied from [`EntryFlow::blind`]; empty, and
    /// not serialized, otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blind: Vec<BlindCaveat>,
    /// GB.3: a [`ConceptTier::Semantic`] row's matched node's cosine to the
    /// prefixed query, 4 dp: an annotation, never the order. None (not
    /// serialized) on every lexical row and for a node with no vector.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f64>,
}

/// **concept_flows** (GB.1): the entry flows that touch `word` — by entry
/// name, hop name or qname, node kind, mechanism, data entity, external host
/// or the heading of a doc section documenting a hop — ranked by (tier, the
/// matching hop's depth asc, reach desc, key, entry qname), see the module
/// doc's "Concept search". An empty answer is a FACT `no_match` absence.
///
/// The flows are [`entry_flows`]' (one walk per entry, the identity quotient
/// as `opts.collapse` says); each hop's `to_live` is read off one
/// [`entrypoint_reachable`] walk, run once per call;
/// [`concept_flows_with_live`] takes the set instead.
pub fn concept_flows(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    word: &str,
    opts: &ConceptOptions,
) -> Answer<ConceptHit> {
    concept_flows_with_live(
        merged,
        &entrypoint_reachable(merged),
        repo_labels,
        word,
        opts,
    )
}

/// [`concept_flows`] over a live set the caller already holds: `live` must be
/// [`entrypoint_reachable`] of `merged`.
pub fn concept_flows_with_live(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    repo_labels: &BTreeMap<u64, String>,
    word: &str,
    opts: &ConceptOptions,
) -> Answer<ConceptHit> {
    let mut hits = concept_search(merged, live, repo_labels, word, opts);
    if opts.limit > 0 {
        hits.truncate(opts.limit);
    }
    Answer::from_results(hits, || {
        let note = if words(word).is_empty() {
            format!(
                "`{}` holds no word of two or more letters or digits",
                word.trim()
            )
        } else {
            format!(
                "no entry flow touches `{}` by name, kind, mechanism, data entity, host or doc heading",
                word.trim()
            )
        };
        absence::empty(merged, "concept", word, "no_match", note, &[], None)
    })
}

/// Every hit for `word`, ranked, uncut; prints the `[concept]` marker.
fn concept_search(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    repo_labels: &BTreeMap<u64, String>,
    word: &str,
    opts: &ConceptOptions,
) -> Vec<ConceptHit> {
    let started = std::time::Instant::now();
    let query = words(word);
    let flows = if query.is_empty() {
        Vec::new()
    } else {
        walk_flows(merged, live, repo_labels, opts.depth, &opts.collapse, false)
    };
    let facts = NodeFacts::index(merged, &flows);
    let mut ranked: Vec<(ConceptTier, usize, usize, ConceptHit)> = Vec::new();
    for (i, w) in flows.iter().enumerate() {
        if let Some(hit) = match_flow(w, &facts, &query) {
            ranked.push((hit.tier, hit.depth, i, hit));
        }
    }
    // (tier, depth asc, reach desc, key, entry qname), then the flow's
    // position in entry_flows' order: never a NodeId.
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| b.3.reach.cmp(&a.3.reach))
            .then_with(|| a.3.key.cmp(&b.3.key))
            .then_with(|| a.3.entry.qname.cmp(&b.3.entry.qname))
            .then_with(|| a.2.cmp(&b.2))
    });
    // GB.3: no lexical tier matched any flow: the semantic tier, already in
    // its own rank order.
    if ranked.is_empty() && !query.is_empty() && opts.semantic.is_some() {
        ranked = semantic_rows(merged, &flows, word, opts);
    }
    let mut counts = [0usize; ConceptTier::ALL.len()];
    for (t, ..) in &ranked {
        counts[*t as usize] += 1;
    }
    let tiers: Vec<String> = ConceptTier::ALL
        .iter()
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|(t, n)| format!("{}:{n}", t.as_str()))
        .collect();
    eprintln!(
        "[concept] word={} flows={} hits={} tiers={} surface={} ms={}",
        word.trim(),
        flows.len(),
        ranked.len(),
        if tiers.is_empty() {
            "-".to_string()
        } else {
            tiers.join(",")
        },
        opts.surface,
        started.elapsed().as_millis()
    );
    ranked
        .into_iter()
        .enumerate()
        .map(|(i, (.., mut hit))| {
            hit.rank = i + 1;
            hit
        })
        .collect()
}

/// GB.3's semantic tier (module doc "The semantic tier"): the flows through
/// the files `find --files`' fused order ranks with a `sem` score, each at its
/// least (file rank, depth) node, ranked by (file rank, depth, reach desc,
/// key, entry qname, flow position). Prints one `[flows] semantic` line per
/// attempt; empty when there is no source, no cache, not exactly one repo,
/// an embedder error or no such file.
fn semantic_rows(
    merged: &MergedGraph,
    flows: &[WalkedFlow],
    word: &str,
    opts: &ConceptOptions,
) -> Vec<(ConceptTier, usize, usize, ConceptHit)> {
    let q = word.trim();
    let mut files_kept = 0usize;
    let rows = semantic_ranked(merged, flows, q, opts, &mut files_kept).unwrap_or_default();
    let top = rows
        .first()
        .map(|(r, ..)| r.to_string())
        .unwrap_or_else(|| "-".to_string());
    let not_embedded = rows.iter().filter(|(.., h)| h.similarity.is_none()).count();
    eprintln!(
        "[flows] semantic query={q} files={files_kept} flows={} top_file_rank={top} not_embedded={not_embedded}",
        rows.len()
    );
    rows.into_iter()
        .map(|(rank, pos, hit)| (ConceptTier::Semantic, rank, pos, hit))
        .collect()
}

/// [`semantic_rows`]' rows before the marker: `(file rank (1-based), flow
/// position, hit)`, sorted. `files_kept` receives the count of fused files
/// with a `sem` score. None on any guard (module doc).
fn semantic_ranked(
    merged: &MergedGraph,
    flows: &[WalkedFlow],
    q: &str,
    opts: &ConceptOptions,
    files_kept: &mut usize,
) -> Option<Vec<(usize, usize, ConceptHit)>> {
    let src = opts.semantic.clone()?;
    let cache = opts.rank_cache.clone()?;
    let roots: BTreeMap<u64, String> = cache.repo_roots().clone();
    let repos: BTreeSet<u64> = merged.graphs.iter().map(|g| g.repo.0).collect();
    let (&repo, _) = roots.iter().next()?;
    if roots.len() != 1 || repos.len() != 1 || !repos.contains(&repo) {
        return None;
    }
    let fopts = FindFilesOptions {
        top_k: 0,
        surface: "flows",
        rank_cache: Some(cache.clone()),
        select: true,
        semantic: Some(src.clone()),
    };
    let answer = find_files(merged, &roots, q, &fopts).ok()?;
    let files = cache.files(merged);
    // The fused order's files with a `sem` score, 1-based: looked up, never
    // iterated into an output.
    let mut rank_of: HashMap<u32, (usize, String)> = HashMap::new();
    for r in answer.results.into_iter().filter(|r| r.sem.is_some()) {
        if let Some(f) = files.index_of(repo, &r.file) {
            let next = rank_of.len() + 1;
            rank_of.entry(f).or_insert((next, r.file));
        }
    }
    *files_kept = rank_of.len();
    if rank_of.is_empty() {
        return Some(Vec::new());
    }
    let qvec = src.query_vector(q).ok()?;
    let sims: HashMap<NodeId, f64> = src
        .search(&qvec, src.len(), &HashSet::new())
        .into_iter()
        .map(|h| (h.id, h.similarity_rounded()))
        .collect();
    let mut out: Vec<(usize, usize, usize, ConceptHit)> = Vec::new();
    for (pos, w) in flows.iter().enumerate() {
        let f = &w.flow;
        // (file rank, depth, hop index or None for the entry, node), least
        // first; the entry counts at depth 0, then the hops in BFS order.
        let mut best: Option<(usize, usize, Option<usize>, NodeId)> = None;
        let nodes = std::iter::once((0usize, None, w.entry_id)).chain(
            f.hops
                .iter()
                .zip(&w.hop_ids)
                .enumerate()
                .map(|(i, (h, id))| (h.depth, Some(i), *id)),
        );
        for (depth, hop, id) in nodes {
            let Some((rank, _)) = files.file_of_node(id).and_then(|fi| rank_of.get(&fi)) else {
                continue;
            };
            if best.is_none_or(|(r, d, ..)| (*rank, depth) < (r, d)) {
                best = Some((*rank, depth, hop, id));
            }
        }
        let Some((rank, depth, hop, id)) = best else {
            continue;
        };
        let path = files
            .file_of_node(id)
            .and_then(|fi| rank_of.get(&fi))
            .map(|(_, p)| p.as_str())
            .unwrap_or("");
        out.push((
            rank,
            depth,
            pos,
            ConceptHit {
                rank: 0,
                key: f.key.clone(),
                entry: f.entry.clone(),
                tier: ConceptTier::Semantic,
                matched: vec![format!(
                    "{q} -> fused file #{rank} {path} (no lexical match)"
                )],
                hop: hop.map(|i| f.hops[i].clone()),
                depth,
                reach: f.reach,
                mechanisms: f.mechanisms.clone(),
                blind: f.blind.clone(),
                similarity: sims.get(&id).copied(),
            },
        ));
    }
    // (file rank, depth, reach desc, key, entry qname), then the flow's
    // position: never a NodeId, never similarity alone.
    out.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| b.3.reach.cmp(&a.3.reach))
            .then_with(|| a.3.key.cmp(&b.3.key))
            .then_with(|| a.3.entry.qname.cmp(&b.3.entry.qname))
            .then_with(|| a.2.cmp(&b.2))
    });
    Some(
        out.into_iter()
            .map(|(rank, _, pos, hit)| (rank, pos, hit))
            .collect(),
    )
}

/// The words of `s`: runs of letters and digits, split again at each
/// lower-case -> upper-case step (`IssueComment` -> issue, comment),
/// lower-cased, tokens under two characters dropped, each once in order.
fn words(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if cur.chars().count() >= 2 && !out.contains(cur) {
            out.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };
    for c in s.chars() {
        if !c.is_alphanumeric() {
            flush(&mut cur, &mut out);
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower {
            flush(&mut cur, &mut out);
        }
        prev_lower = c.is_lowercase();
        cur.extend(c.to_lowercase());
    }
    flush(&mut cur, &mut out);
    out
}

/// A query word matches a fact word when they are equal or one is the other
/// plus a trailing `s` / `es` (no stemmer).
fn word_matches(q: &str, f: &str) -> bool {
    fn plural_of(a: &str, b: &str) -> bool {
        b.strip_prefix(a)
            .is_some_and(|rest| rest == "s" || rest == "es")
    }
    q == f || plural_of(q, f) || plural_of(f, q)
}

fn any_match(q: &str, ws: &[String]) -> bool {
    ws.iter().any(|f| word_matches(q, f))
}

/// The facts one shown node offers a query word, computed once per node per
/// call (module doc "Concept search").
struct NodeFacts {
    name: String,
    name_words: Vec<String>,
    qname: String,
    qname_words: Vec<String>,
    kind: &'static str,
    kind_words: Vec<String>,
    /// (qname, name words) of each data node it ACCESSES_DATA / USES.
    data: Vec<(String, Vec<String>)>,
    /// (host, words) of each host its external stamp names.
    hosts: Vec<(String, Vec<String>)>,
    /// (heading, words) of each DOC_SECTION that DOCUMENTS it.
    docs: Vec<(String, Vec<String>)>,
}

/// The data kinds a hop's one-hop neighbour is read from (DataEntity tier).
const DATA_KINDS: [NodeKindId; 5] = [
    node_kind::DATA_ENTITY,
    node_kind::DATABASE,
    node_kind::CACHE,
    node_kind::BLOB_STORE,
    node_kind::SEARCH_INDEX,
];

impl NodeFacts {
    /// The facts of every node the flows show (entries and hops): one pass
    /// over the nodes and one over the edges. A HashMap looked up, never
    /// iterated into an output.
    fn index(merged: &MergedGraph, flows: &[WalkedFlow]) -> HashMap<NodeId, NodeFacts> {
        let mut want: HashSet<NodeId> = HashSet::new();
        for w in flows {
            want.insert(w.entry_id);
            want.extend(w.hop_ids.iter().copied());
        }
        let mut out: HashMap<NodeId, NodeFacts> = HashMap::new();
        if want.is_empty() {
            return out;
        }
        // Kind, name and qname: the first graph naming a node.
        let mut kind_of: HashMap<NodeId, NodeKindId> = HashMap::new();
        let mut name_of: HashMap<NodeId, &str> = HashMap::new();
        let mut cells: HashMap<NodeId, Vec<&glia_core::Cell>> = HashMap::new();
        for g in &merged.graphs {
            for (id, k) in &g.nav.kind_by_id {
                kind_of.entry(*id).or_insert(*k);
            }
            for n in &g.nodes {
                if want.contains(&n.id) {
                    cells.entry(n.id).or_default().extend(n.cells.iter());
                    if let Some(name) = g.nav.name_by_id.get(&n.id) {
                        name_of.entry(n.id).or_insert(name.as_str());
                    }
                }
            }
        }
        let qname_of = |id: NodeId| -> &str {
            merged
                .graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&id).map(String::as_str))
                .unwrap_or("")
        };
        let mut data: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut docs: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for e in merged.all_edges() {
            if (e.category == edge_category::ACCESSES_DATA || e.category == edge_category::USES)
                && want.contains(&e.from)
                && kind_of.get(&e.to).is_some_and(|k| DATA_KINDS.contains(k))
            {
                let v = data.entry(e.from).or_default();
                if !v.contains(&e.to) {
                    v.push(e.to);
                }
            } else if e.category == edge_category::DOCUMENTS
                && want.contains(&e.to)
                && kind_of.get(&e.from) == Some(&node_kind::DOC_SECTION)
            {
                let v = docs.entry(e.to).or_default();
                if !v.contains(&e.from) {
                    v.push(e.from);
                }
            }
        }
        let name_for = |id: NodeId, name_of: &HashMap<NodeId, &str>| -> String {
            name_of
                .get(&id)
                .map(|s| s.to_string())
                .or_else(|| {
                    merged
                        .graphs
                        .iter()
                        .find_map(|g| g.nav.name_by_id.get(&id).cloned())
                })
                .unwrap_or_default()
        };
        for &id in &want {
            let name = name_for(id, &name_of);
            let qname = qname_of(id).to_string();
            let kind = kind_of
                .get(&id)
                .map(|k| node_kind::name(*k))
                .unwrap_or("UNKNOWN");
            let data_rows = data
                .get(&id)
                .into_iter()
                .flatten()
                .map(|&d| (qname_of(d).to_string(), words(&name_for(d, &name_of))))
                .collect();
            let doc_rows = docs
                .get(&id)
                .into_iter()
                .flatten()
                .map(|&d| {
                    let heading = name_for(d, &name_of);
                    let ws = words(&heading);
                    (heading, ws)
                })
                .collect();
            let hosts = cells
                .get(&id)
                .and_then(|c| external_hosts(c.iter().copied()))
                .unwrap_or_default()
                .into_iter()
                .map(|h| {
                    let ws = words(&h);
                    (h, ws)
                })
                .collect();
            out.insert(
                id,
                NodeFacts {
                    name_words: words(&name),
                    qname_words: words(&qname),
                    kind_words: words(kind),
                    name,
                    qname,
                    kind,
                    data: data_rows,
                    hosts,
                    docs: doc_rows,
                },
            );
        }
        out
    }

    /// The best tier at which this node matches `q`, and what matched.
    /// `entry`: the node is the flow's entry, whose name is the Entry tier
    /// (matched by the caller), so HopName is skipped. `mechanism` is the
    /// edge category of the hop into it (None for the entry).
    fn best(
        &self,
        q: &str,
        entry: bool,
        mechanism: Option<&'static str>,
    ) -> Option<(ConceptTier, String)> {
        if !entry && any_match(q, &self.name_words) {
            return Some((ConceptTier::HopName, format!("{q} -> name {}", self.name)));
        }
        if any_match(q, &self.qname_words) {
            return Some((
                ConceptTier::HopQname,
                format!("{q} -> qname {}", self.qname),
            ));
        }
        if any_match(q, &self.kind_words) {
            return Some((ConceptTier::Kind, format!("{q} -> kind {}", self.kind)));
        }
        if let Some(m) = mechanism
            && any_match(q, &words(m))
        {
            return Some((ConceptTier::Mechanism, format!("{q} -> mechanism {m}")));
        }
        if let Some((d, _)) = self.data.iter().find(|(_, ws)| any_match(q, ws)) {
            return Some((ConceptTier::DataEntity, format!("{q} -> data entity {d}")));
        }
        if let Some((h, _)) = self.hosts.iter().find(|(_, ws)| any_match(q, ws)) {
            return Some((ConceptTier::ExternalHost, format!("{q} -> host {h}")));
        }
        if let Some((d, _)) = self.docs.iter().find(|(_, ws)| any_match(q, ws)) {
            return Some((ConceptTier::Doc, format!("{q} -> doc {d}")));
        }
        None
    }
}

/// Match one flow against every query word (module doc): each word's best
/// (tier, depth) over the entry and the hops in order; the flow is a hit when
/// every word matched. `None` otherwise.
fn match_flow(
    w: &WalkedFlow,
    facts: &HashMap<NodeId, NodeFacts>,
    query: &[String],
) -> Option<ConceptHit> {
    let f = &w.flow;
    let entry_words = {
        let mut ws = words(&f.key);
        for x in words(&f.entry.name) {
            if !ws.contains(&x) {
                ws.push(x);
            }
        }
        ws
    };
    // Per word: (tier, depth, hop index or None, detail).
    let mut best: Vec<(ConceptTier, usize, Option<usize>, String)> = Vec::new();
    for q in query {
        let mut b: Option<(ConceptTier, usize, Option<usize>, String)> = None;
        let mut offer = |t: ConceptTier, d: usize, hop: Option<usize>, detail: String| {
            if b.as_ref().is_none_or(|(bt, bd, ..)| (t, d) < (*bt, *bd)) {
                b = Some((t, d, hop, detail));
            }
        };
        if any_match(q, &entry_words) {
            offer(
                ConceptTier::Entry,
                0,
                None,
                format!("{q} -> entry {}", f.entry.name),
            );
        } else if let Some((t, detail)) = facts.get(&w.entry_id).and_then(|n| n.best(q, true, None))
        {
            offer(t, 0, None, detail);
        }
        for (i, (h, id)) in f.hops.iter().zip(&w.hop_ids).enumerate() {
            if let Some((t, detail)) = facts
                .get(id)
                .and_then(|n| n.best(q, false, Some(h.mechanism)))
            {
                offer(t, h.depth, Some(i), detail);
            }
        }
        best.push(b?);
    }
    let tier = best.iter().map(|b| b.0).max()?;
    // The deepest word's match speaks for the hop (the first such word).
    let deepest = best
        .iter()
        .enumerate()
        .max_by(|(ia, a), (ib, b)| a.1.cmp(&b.1).then_with(|| ib.cmp(ia)))
        .map(|(_, b)| b)?;
    Some(ConceptHit {
        rank: 0,
        key: f.key.clone(),
        entry: f.entry.clone(),
        tier,
        matched: best.iter().map(|b| b.3.clone()).collect(),
        hop: deepest.2.map(|i| f.hops[i].clone()),
        depth: deepest.1,
        reach: f.reach,
        mechanisms: f.mechanisms.clone(),
        blind: f.blind.clone(),
        similarity: None,
    })
}

/// An entry key or a feature word as a slug: lower-cased, spaces and hyphens
/// as `_` (the repo-graph wrapper's spelling).
fn slug(s: &str) -> String {
    s.to_lowercase().replace([' ', '-'], "_")
}

/// One entry point: the node, its name and qname. Crate-visible so CD.4e's
/// `duplicate_flows` compares exactly the entries `glia flows` lists.
pub(crate) struct Entry<'a> {
    pub(crate) id: NodeId,
    pub(crate) name: &'a str,
    pub(crate) qname: &'a str,
    /// HF.1: the entry table admits it (kind, name, roles).
    table: bool,
    /// HF.1: it carries an ENTRYPOINT cell, by the cell's `source` class.
    declared: Option<DeclSource>,
}

/// HF.1: what declared an entry, from its ENTRYPOINT cell's `source` key
/// (the `[flows] declared entries=` marker's `sources=(..)`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeclSource {
    /// `config`: an overlay `[entrypoints]` pattern (LF.3b).
    Config,
    /// `main_guard`: a script's `if __name__ == "__main__":` guard.
    MainGuard,
    /// `host:<h>`: a host callback, plugin unit or host start (GH.2-GH.20).
    Host,
    /// `shebang` / `script_arg`: a script run by its interpreter (GH.4).
    Script,
    /// Any other or no `source`.
    Other,
}

/// The [`DeclSource`] of the first ENTRYPOINT cell in `cells`, `None`
/// when there is none.
fn decl_source(cells: &[Cell]) -> Option<DeclSource> {
    let c = cells.iter().find(|c| c.kind == cell_type::ENTRYPOINT)?;
    let text = match &c.payload {
        CellPayload::Json(s) | CellPayload::Text(s) => s.as_str(),
        CellPayload::Bytes(_) => return Some(DeclSource::Other),
    };
    let value: Option<serde_json::Value> = serde_json::from_str(text).ok();
    let source = value
        .as_ref()
        .and_then(|v| v.get("source"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    Some(match source {
        "config" => DeclSource::Config,
        "main_guard" => DeclSource::MainGuard,
        "shebang" | "script_arg" => DeclSource::Script,
        s if s.starts_with("host:") => DeclSource::Host,
        _ => DeclSource::Other,
    })
}

/// The `[flows] declared entries=<d> new=<n> sources=(..)` line (HF.1),
/// printed after `[flows] entries=` when an entry carries an ENTRYPOINT
/// cell: `d` such entries, `n` of them the entry table alone would not
/// admit, by [`DeclSource`].
fn declared_marker(entries: &[Entry<'_>]) {
    let declared: Vec<(bool, DeclSource)> = entries
        .iter()
        .filter_map(|e| e.declared.map(|s| (e.table, s)))
        .collect();
    if declared.is_empty() {
        return;
    }
    let count = |s: DeclSource| declared.iter().filter(|(_, d)| *d == s).count();
    eprintln!(
        "[flows] declared entries={} new={} sources=(config={} main_guard={} host={} script={} other={})",
        declared.len(),
        declared.iter().filter(|(table, _)| !table).count(),
        count(DeclSource::Config),
        count(DeclSource::MainGuard),
        count(DeclSource::Host),
        count(DeclSource::Script),
        count(DeclSource::Other),
    );
}

/// Every entry point of `merged`, the set liveness seeds from
/// (`entrypoint_reachable`): `CODE_PROFILE.tables.entry` over kind, name
/// and roles, and (HF.1) every node carrying an ENTRYPOINT cell (an overlay
/// `[entrypoints]` declaration, a `__main__` guard, a host callback, plugin
/// unit or start, a shebang / script_arg script) — in graph then node
/// order, each id once.
pub(crate) fn entries(merged: &MergedGraph) -> Vec<Entry<'_>> {
    let rule = &CODE_PROFILE.tables.entry;
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut out: Vec<Entry<'_>> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            let name = g
                .nav
                .name_by_id
                .get(&n.id)
                .map(String::as_str)
                .unwrap_or("");
            let ext = || node_ext(&n.cells);
            let table = rule.is_entry_at(kind, name, &[], ext)
                || rule.is_entry_at(kind, name, &roles_in(kind, &n.cells), ext);
            let declared = decl_source(&n.cells);
            if (table || declared.is_some()) && seen.insert(n.id) {
                let qname = g
                    .nav
                    .qname_by_id
                    .get(&n.id)
                    .map(String::as_str)
                    .unwrap_or("");
                out.push(Entry {
                    id: n.id,
                    name,
                    qname,
                    table,
                    declared,
                });
            }
        }
    }
    out
}

/// The entry-flow tier's pick for `feature` (module doc): the entry whose key
/// matches the feature's slug — equal before containing, then the larger reach
/// within `depth`, then key, qname and id ascending — among the entries that
/// reach at least one node. Only the matching entries are walked, each from
/// its walk node (DD.11: `adj` may index the identity quotient).
fn entry_seed(
    merged: &MergedGraph,
    view: &IdentityView,
    adj: &Adjacency,
    feature: &str,
    depth: usize,
) -> Option<NodeId> {
    let word = slug(feature.trim());
    if word.is_empty() {
        return None;
    }
    // (tier, reach, key, qname, id): tier 0 = equal key, 1 = containment.
    let mut best: Option<(u8, usize, String, &str, NodeId)> = None;
    for e in entries(merged) {
        let key = slug(e.name);
        if key.is_empty() {
            continue;
        }
        let tier = if key == word {
            0
        } else if key.contains(&word) || word.contains(&key) {
            1
        } else {
            continue;
        };
        let reach = reach::bfs(adj, &[view.walk_rep(e.id)], Walk::Forward, depth)
            .reached
            .len();
        if reach == 0 {
            continue;
        }
        let better = match &best {
            None => true,
            Some((t, r, k, q, id)) => {
                (
                    tier,
                    std::cmp::Reverse(reach),
                    key.as_str(),
                    e.qname,
                    e.id.0,
                ) < (*t, std::cmp::Reverse(*r), k.as_str(), *q, id.0)
            }
        };
        if better {
            best = Some((tier, reach, key, e.qname, e.id));
        }
    }
    best.map(|(.., id)| id)
}

/// The LD.4a fired_on line.
/// `(by, concept)`: how the seed resolved, and the concept tier when `by`
/// is `concept`.
fn marker(
    q: &str,
    (by, concept): (&str, Option<ConceptTier>),
    paths: usize,
    expanded: usize,
    truncated: bool,
    opts: &TraceOptions,
    directed: Option<bool>,
) {
    let two_node = match (&opts.to, directed) {
        (Some(to), Some(d)) => format!(" to={to} directed={d}"),
        (Some(to), None) => format!(" to={to} directed=-"),
        (None, _) => String::new(),
    };
    let tier = concept
        .map(|t| format!(" concept_tier={}", t.as_str()))
        .unwrap_or_default();
    eprintln!(
        "[trace] seed={q} resolved_by={by}{tier} paths={paths} expanded={expanded} truncated={truncated}{two_node}"
    );
}

/// The concept tier's pick for `feature` (GB.1, module doc "Seeds"): the
/// entry of the top [`concept_flows`] hit over the trace's own depth and
/// identity option, with its tier. Walks every entry flow, so it runs only
/// where the LD.4b tier matched no key. `semantic`: GB.3's tier may answer
/// (the feature resolved to no node at all; a lexically resolved dead end
/// never yields to a similarity).
fn concept_seed(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    feature: &str,
    opts: &TraceOptions,
    semantic: bool,
) -> Option<(NodeId, ConceptTier)> {
    let c = ConceptOptions {
        depth: opts.depth,
        collapse: opts.collapse.clone(),
        surface: "trace",
        semantic: opts.semantic.clone().filter(|_| semantic),
        rank_cache: opts.rank_cache.clone(),
        ..ConceptOptions::default()
    };
    concept_search(merged, live, &opts.collapse.repo_labels, feature, &c)
        .first()
        .map(|h| (NodeId(h.entry.id), h.tier))
}

/// A seed query, resolved: the node (if any), how, and find's nearest rows
/// (filled only when the exact tiers missed; they become the suggestions).
struct Resolved {
    id: Option<NodeId>,
    by: &'static str,
    near: Vec<FoundNode>,
}

/// Exact qname, then exact name (`answers::resolve_seed(merged, q, None)` is
/// exactly these two), then find's top hit in one of [`FIND_TIERS`].
fn resolve(merged: &MergedGraph, q: &str) -> Resolved {
    if let Some(id) = merged.node_id_by_qname(q) {
        return Resolved {
            id: Some(id),
            by: "qname",
            near: Vec::new(),
        };
    }
    if let Some(id) = merged.resolve_name(q) {
        return Resolved {
            id: Some(id),
            by: "name",
            near: Vec::new(),
        };
    }
    let opts = FindOptions {
        top_k: absence::SUGGESTIONS,
        ..FindOptions::default()
    };
    let near = find::search(merged, q, &opts).rows;
    let hit = near
        .first()
        .filter(|r| FIND_TIERS.contains(&r.r#match))
        .map(|r| NodeId(r.id));
    match hit {
        Some(id) => Resolved {
            id: Some(id),
            by: "find",
            near,
        },
        None => Resolved {
            id: None,
            by: "none",
            near,
        },
    }
}

/// The edge categories an answer about `id` depends on
/// (`absence::mechanisms_for_kind` of the kind the first graph naming it
/// records); none for an id no graph names.
fn mechanisms_of(merged: &MergedGraph, id: NodeId) -> &'static [&'static str] {
    let kind: Option<NodeKindId> = merged
        .graphs
        .iter()
        .find_map(|g| g.nav.kind_by_id.get(&id).copied());
    kind.map(absence::mechanisms_for_kind).unwrap_or(&[])
}

/// Does any carry edge leave `id`?
fn leaves(succ: &Successors, id: NodeId) -> bool {
    succ.index
        .get(&id)
        .is_some_and(|&ix| !succ.out[ix as usize].is_empty())
}

/// The live set mapped onto the walk's nodes (DD.11): a walk class is live
/// when any member is. `walking`: the answer walks `view`'s quotient;
/// otherwise the raw `live` set itself.
fn walk_live<'a>(
    view: &IdentityView,
    live: &'a HashSet<NodeId>,
    walking: bool,
) -> Cow<'a, HashSet<NodeId>> {
    if walking {
        Cow::Owned(live.iter().map(|&n| view.walk_rep(n)).collect())
    } else {
        Cow::Borrowed(live)
    }
}

/// The node walk node `id` shows: a walk class's representative (an
/// internal handle, DD.1) shows the class's display member, any other walk
/// node itself.
fn show(view: &IdentityView, id: NodeId) -> NodeId {
    if view.walk_members(id).is_empty() {
        id
    } else {
        view.display(id)
    }
}

/// The class members walk node `id` stands for beyond the one it shows.
fn walk_folded(view: &IdentityView, id: NodeId) -> usize {
    view.walk_members(id).len().saturating_sub(1)
}

/// Per-node facts the hops need, computed once per node per call: the repo
/// (the pre-LD.4a `repo_of` map, a later graph overwriting an earlier one),
/// the qname, and the service key under `ProjectRoots` keying. Hops of a walk
/// ([`Self::hop`]) show their ends through [`show`]; the two-node
/// fallback's ([`Self::raw_hop`]) are raw nodes.
struct Sides<'a> {
    loc: &'a Locator<'a>,
    repo_of: HashMap<NodeId, u64>,
    qname_of: HashMap<NodeId, &'a str>,
    keying: ServiceKeying,
    /// `service_of` keys with no repo-label prefix: only equality matters.
    no_labels: BTreeMap<u64, String>,
    service: HashMap<NodeId, Option<String>>,
    view: &'a IdentityView,
    /// The walk's live set ([`walk_live`]).
    walk_live: &'a HashSet<NodeId>,
}

impl<'a> Sides<'a> {
    fn new(
        merged: &'a MergedGraph,
        loc: &'a Locator<'a>,
        view: &'a IdentityView,
        walk_live: &'a HashSet<NodeId>,
    ) -> Self {
        let mut repo_of: HashMap<NodeId, u64> = HashMap::new();
        let mut qname_of: HashMap<NodeId, &'a str> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                repo_of.insert(n.id, g.repo.0);
            }
            for (id, q) in &g.nav.qname_by_id {
                qname_of.entry(*id).or_insert(q.as_str());
            }
        }
        Sides {
            loc,
            repo_of,
            qname_of,
            keying: default_keying(merged),
            no_labels: BTreeMap::new(),
            service: HashMap::new(),
            view,
            walk_live,
        }
    }

    fn qname(&self, id: NodeId) -> &'a str {
        self.qname_of.get(&id).copied().unwrap_or("")
    }

    /// The qname walk node `id` shows (the rank key's).
    fn walk_qname(&self, id: NodeId) -> &'a str {
        self.qname(show(self.view, id))
    }

    /// [`Self::cross`] of a walk hop, over the shown ends.
    fn walk_cross(&mut self, from: NodeId, to: NodeId) -> (bool, bool) {
        self.cross(show(self.view, from), show(self.view, to))
    }

    /// `(cross_service, cross_repo)` for a hop `from -> to`.
    fn cross(&mut self, from: NodeId, to: NodeId) -> (bool, bool) {
        let cross_repo = self.repo_of.get(&from) != self.repo_of.get(&to);
        if cross_repo || !matches!(self.keying, ServiceKeying::ProjectRoots(_)) {
            return (cross_repo, cross_repo);
        }
        let a = self.service_key(from);
        let b = self.service_key(to);
        (matches!((a, b), (Some(a), Some(b)) if a != b), false)
    }

    /// The `glia arch` service `id` belongs to: keyed by its located file,
    /// else by its qname's owner segment; `None` when it has neither.
    fn service_key(&mut self, id: NodeId) -> Option<String> {
        if let Some(k) = self.service.get(&id) {
            return k.clone();
        }
        let file = self
            .loc
            .file_of(id)
            .or_else(|| endpoint::split_owner(self.qname(id)).1.map(str::to_string));
        let repo = self.repo_of.get(&id).copied().unwrap_or_default();
        let key = file.map(|f| service_of(&f, repo, &self.keying, &self.no_labels));
        self.service.insert(id, key.clone());
        key
    }

    /// The service `id` counts toward in an entry flow's `services`: under
    /// `ProjectRoots` keying the `glia arch` service of [`Self::service_key`]
    /// (one repo, so `service_of` never prefixes it with a label), else its
    /// repo's label from `labels` (`service_of`'s `repo<id>` when the build
    /// has none). `None` for a node no graph holds, or one `service_key`
    /// cannot place.
    fn service_label(&mut self, id: NodeId, labels: &BTreeMap<u64, String>) -> Option<String> {
        let repo = *self.repo_of.get(&id)?;
        if matches!(self.keying, ServiceKeying::ProjectRoots(_)) {
            return self.service_key(id);
        }
        Some(service_of("", repo, &ServiceKeying::PerRepo, labels))
    }

    /// One located hop of a walk: `from` and `to` are walk nodes, shown
    /// through [`show`]; `to_live` is the walk's live set.
    fn hop(&mut self, depth: usize, from: NodeId, to: NodeId, via: EdgeCategoryId) -> TraceHop {
        let to_live = self.walk_live.contains(&to);
        let (shown_from, shown_to) = (show(self.view, from), show(self.view, to));
        self.located(depth, shown_from, shown_to, to, via, to_live)
    }

    /// One located hop between two raw nodes (the two-node fallback).
    fn raw_hop(
        &mut self,
        live: &HashSet<NodeId>,
        depth: usize,
        from: NodeId,
        to: NodeId,
        via: EdgeCategoryId,
    ) -> TraceHop {
        self.located(depth, from, to, to, via, live.contains(&to))
    }

    /// The hop `from -> to` between two shown nodes; `class_of` is the node
    /// whose identity class `to` stands for (the walk node, or `to`).
    fn located(
        &mut self,
        depth: usize,
        from: NodeId,
        to: NodeId,
        class_of: NodeId,
        via: EdgeCategoryId,
        to_live: bool,
    ) -> TraceHop {
        let (cross_service, cross_repo) = self.cross(from, to);
        let at = self.loc.locate(to);
        TraceHop {
            depth,
            mechanism: edge_category::name(via),
            cross_service,
            cross_repo,
            from_qname: self.loc.locate(from).qname,
            to_qname: at.qname,
            to_kind: at.kind,
            to_live,
            to_file: at.file,
            to_line: at.line,
            to_identity: self.view.row_identity(class_of, to),
        }
    }
}

/// The carry edges as a successor map over a dense index: each node's
/// distinct successors in global edge order, the first edge to a successor
/// speaking for it.
struct Successors {
    ids: Vec<NodeId>,
    index: HashMap<NodeId, u32>,
    out: Vec<Vec<(u32, EdgeCategoryId)>>,
}

impl Successors {
    fn carry(merged: &MergedGraph) -> Self {
        Self::carry_over(merged)
    }

    /// The carry successors of any [`GraphSource`] (DD.11: the merged graph,
    /// or its identity quotient), in its edge order. A `MergedGraph`'s edges
    /// are `all_edges()`, so `carry_over(merged)` is the pre-DD.11 map.
    fn carry_over<G: GraphSource + ?Sized>(g: &G) -> Self {
        let keep = CategorySet::of(CODE_PROFILE.tables.carry_edges);
        let mut s = Successors {
            ids: Vec::new(),
            index: HashMap::new(),
            out: Vec::new(),
        };
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        for e in g.edges() {
            if !keep.contains(e.category) {
                continue;
            }
            let from = s.intern(e.from);
            let to = s.intern(e.to);
            if seen.insert((from, to)) {
                s.out[from as usize].push((to, e.category));
            }
        }
        s
    }

    fn intern(&mut self, id: NodeId) -> u32 {
        if let Some(&ix) = self.index.get(&id) {
            return ix;
        }
        let ix = self.ids.len() as u32;
        self.index.insert(id, ix);
        self.ids.push(id);
        self.out.push(Vec::new());
        ix
    }
}

/// A path found by the enumeration, before it is located.
struct Candidate {
    /// Dense indices, seed first.
    nodes: Vec<u32>,
    cats: Vec<EdgeCategoryId>,
    cross_service_hops: usize,
    mechanisms: usize,
}

/// What [`enumerate`] found: the best paths (ranked, cut to `max_paths`),
/// the extensions it made, and whether the budget stopped it.
struct Walked {
    kept: Vec<Candidate>,
    expanded: usize,
    truncated: bool,
}

/// One depth-first frame: the node, the next successor to try, and whether
/// the walk ever extended the path past it.
struct Frame {
    ix: u32,
    cursor: usize,
    extended: bool,
}

/// Enumerate simple paths from `seed` by depth-first search (iterative, so a
/// deep `depth` cannot overflow the stack). One-node mode (`target = None`)
/// records every maximal path; two-node mode records the paths ending at the
/// target and never extends past it, entering only nodes whose hops-to-target
/// (`to_target`) fit in the hops left.
fn enumerate(
    succ: &Successors,
    sides: &mut Sides<'_>,
    seed: NodeId,
    target: Option<(NodeId, &HashMap<NodeId, usize>)>,
    opts: &TraceOptions,
) -> Walked {
    let mut walked = Walked {
        kept: Vec::new(),
        expanded: 0,
        truncated: false,
    };
    let Some(&seed_ix) = succ.index.get(&seed) else {
        return walked;
    };
    let target_ix = target.and_then(|(t, _)| succ.index.get(&t).copied());
    if target.is_some() && target_ix.is_none() {
        return walked;
    }
    // Hops-to-target per dense index, `None` = out of reach.
    let left: Option<Vec<Option<usize>>> =
        target.map(|(_, dist)| succ.ids.iter().map(|id| dist.get(id).copied()).collect());
    let prune_at = if opts.max_paths == 0 {
        usize::MAX
    } else {
        opts.max_paths.saturating_mul(4).max(1024)
    };

    let mut on_path = vec![false; succ.ids.len()];
    let mut frames = vec![Frame {
        ix: seed_ix,
        cursor: 0,
        extended: false,
    }];
    let mut cats: Vec<EdgeCategoryId> = Vec::new();
    // `crossed[i]` = cross-service hops among the first `i` hops.
    let mut crossed: Vec<usize> = vec![0];
    on_path[seed_ix as usize] = true;
    walked.expanded = 1;

    while let Some(fi) = frames.len().checked_sub(1) {
        let len = fi; // hops on the path so far
        let node = frames[fi].ix;
        let mut next = None;
        if len < opts.depth && Some(node) != target_ix {
            let out = &succ.out[node as usize];
            while frames[fi].cursor < out.len() {
                let (to, cat) = out[frames[fi].cursor];
                frames[fi].cursor += 1;
                if on_path[to as usize] {
                    continue;
                }
                if let Some(left) = &left {
                    match left[to as usize] {
                        Some(d) if len + 1 + d <= opts.depth => {}
                        _ => continue,
                    }
                }
                next = Some((to, cat));
                break;
            }
        }
        match next {
            Some((to, cat)) => {
                if walked.expanded >= EXPANSION_BUDGET {
                    walked.truncated = true;
                    break;
                }
                walked.expanded += 1;
                frames[fi].extended = true;
                let (svc, _) = sides.walk_cross(succ.ids[node as usize], succ.ids[to as usize]);
                crossed.push(crossed[len] + usize::from(svc));
                cats.push(cat);
                on_path[to as usize] = true;
                frames.push(Frame {
                    ix: to,
                    cursor: 0,
                    extended: false,
                });
            }
            None => {
                let done = &frames[fi];
                let record = len > 0
                    && match target_ix {
                        Some(t) => done.ix == t,
                        None => !done.extended,
                    };
                if record {
                    let mut seen: Vec<EdgeCategoryId> = Vec::new();
                    for c in &cats {
                        if !seen.contains(c) {
                            seen.push(*c);
                        }
                    }
                    walked.kept.push(Candidate {
                        nodes: frames.iter().map(|f| f.ix).collect(),
                        cats: cats.clone(),
                        cross_service_hops: crossed[len],
                        mechanisms: seen.len(),
                    });
                    if walked.kept.len() >= prune_at {
                        rank(&mut walked.kept, succ, sides, opts.max_paths);
                    }
                }
                on_path[node as usize] = false;
                frames.pop();
                if fi > 0 {
                    cats.pop();
                    crossed.pop();
                }
            }
        }
    }
    rank(&mut walked.kept, succ, sides, opts.max_paths);
    walked
}

/// Sort by the rank key (module doc) and keep the first `max_paths`
/// (`0` keeps all).
fn rank(cands: &mut Vec<Candidate>, succ: &Successors, sides: &Sides<'_>, max_paths: usize) {
    cands.sort_by(|a, b| {
        b.cross_service_hops
            .cmp(&a.cross_service_hops)
            .then_with(|| b.mechanisms.cmp(&a.mechanisms))
            .then_with(|| b.cats.len().cmp(&a.cats.len()))
            .then_with(|| {
                let qa = a
                    .nodes
                    .iter()
                    .map(|&ix| sides.walk_qname(succ.ids[ix as usize]));
                let qb = b
                    .nodes
                    .iter()
                    .map(|&ix| sides.walk_qname(succ.ids[ix as usize]));
                qa.cmp(qb)
            })
            .then_with(|| {
                let ia = a.nodes.iter().map(|&ix| succ.ids[ix as usize].0);
                let ib = b.nodes.iter().map(|&ix| succ.ids[ix as usize].0);
                ia.cmp(ib)
            })
    });
    if max_paths > 0 {
        cands.truncate(max_paths);
    }
}

/// Locate the kept candidates into ranked [`TracePath`]s.
fn materialize(succ: &Successors, sides: &mut Sides<'_>, kept: &[Candidate]) -> Vec<TracePath> {
    kept.iter()
        .enumerate()
        .map(|(i, c)| {
            let hops: Vec<TraceHop> = c
                .nodes
                .windows(2)
                .zip(&c.cats)
                .enumerate()
                .map(|(d, (w, &cat))| {
                    sides.hop(d + 1, succ.ids[w[0] as usize], succ.ids[w[1] as usize], cat)
                })
                .collect();
            path_of(i + 1, hops, true)
        })
        .collect()
}

/// The two-node fallback: `MergedGraph::shortest_path`'s steps as one
/// undirected path, each hop's mechanism the category it traversed. Raw
/// nodes, shown as themselves (DD.11).
fn undirected_path(
    sides: &mut Sides<'_>,
    live: &HashSet<NodeId>,
    steps: &[(NodeId, Option<EdgeCategoryId>)],
) -> TracePath {
    // Every step after the first names the category that entered it.
    let hops: Vec<TraceHop> = steps
        .windows(2)
        .filter_map(|w| Some((w[0].0, w[1].0, w[1].1?)))
        .enumerate()
        .map(|(d, (from, to, via))| sides.raw_hop(live, d + 1, from, to, via))
        .collect();
    path_of(1, hops, false)
}

fn path_of(rank: usize, hops: Vec<TraceHop>, directed: bool) -> TracePath {
    let mut mechanisms: Vec<&'static str> = Vec::new();
    for h in &hops {
        if !mechanisms.contains(&h.mechanism) {
            mechanisms.push(h.mechanism);
        }
    }
    TracePath {
        rank,
        cross_service_hops: hops.iter().filter(|h| h.cross_service).count(),
        length: hops.len(),
        mechanisms,
        hops,
        directed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_camel_steps_and_separators() {
        assert_eq!(words("IssueComment"), ["issue", "comment"]);
        assert_eq!(
            words("data_entity:sql:issue_comments"),
            ["data", "entity", "sql", "issue", "comments"]
        );
        assert_eq!(words("POST /orders/<oid>"), ["post", "orders", "oid"]);
        // One-character tokens drop; an all-caps run is one word.
        assert_eq!(words("a HTTPServer x2"), ["httpserver", "x2"]);
        assert_eq!(words("QUEUE_PRODUCER"), ["queue", "producer"]);
        assert!(words(" - ").is_empty());
    }

    #[test]
    fn a_word_matches_its_plural_by_s_or_es() {
        assert!(word_matches("cache", "caches"));
        assert!(word_matches("caches", "cache"));
        assert!(word_matches("box", "boxes"));
        assert!(word_matches("order", "order"));
        assert!(!word_matches("cache", "caching"));
        assert!(!word_matches("cache", "cachex"));
    }

    #[test]
    fn concept_tiers_order_best_first_and_print_snake_case() {
        let names: Vec<&str> = ConceptTier::ALL.iter().map(|t| t.as_str()).collect();
        assert_eq!(
            names,
            [
                "entry",
                "hop_name",
                "hop_qname",
                "kind",
                "mechanism",
                "data_entity",
                "external_host",
                "doc",
                "semantic"
            ]
        );
        assert!(ConceptTier::ALL.windows(2).all(|w| w[0] < w[1]));
        for t in ConceptTier::ALL {
            assert_eq!(
                serde_json::to_string(&t).expect("json"),
                format!("\"{}\"", t.as_str())
            );
        }
        let o = ConceptOptions::default();
        assert_eq!((o.depth, o.limit, o.surface), (6, 20, "lib"));
    }

    #[test]
    fn default_options() {
        let o = TraceOptions::default();
        assert_eq!((o.depth, o.to.as_deref(), o.max_paths), (6, None, 10));
        assert_eq!(
            o.collapse,
            Collapse::default(),
            "identity collapse on by default"
        );
    }
}
