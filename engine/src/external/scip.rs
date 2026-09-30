//! The SCIP stage (CE.1d): a repo's `.glia/scip-snapshot/` (CE.1a, written by
//! `glia scip import`, read back through `code_domain::snapshots::read_scip`)
//! turns the references a compiler-grade indexer resolved into CALLS / USES
//! edges between existing glia nodes. Its payoff is what the static resolver
//! cannot bind: calls through inferred types, dict / factory dispatch.
//!
//! SCIP is a FACT input, like history: it runs with or without `--no-overlay`.
//! A repo with no snapshot directory is a no-op that prints nothing (the build
//! stays byte-identical); an incomplete snapshot is `read_scip`'s
//! `[scip] snapshot incomplete` line and a no-op. The stage adds edges and
//! re-stamps the EVIDENCE cell and confidence of edges glia bound weakly
//! (CE.1e); it never adds a node or a node cell, never removes an edge, and
//! runs in the single-threaded `Post` stage.
//!
//! - STALE. A document whose file (`<root>/<path>`) is unreadable, or no longer
//!   hashes (`code_domain::snapshots::source_hash`) to the bytes the import
//!   read, is skipped whole: an index never places facts on code edited since.
//!   The build slices no source text: the importer stored the identifier at
//!   every definition and the call flag of every reference.
//! - SPANS. Every node of the repo with a POSITION cell (the first one, 0-based
//!   rows, `end_line` inclusive, a missing end read as the start) and a nav
//!   kind in [`DEF_KINDS`] or an owner kind is indexed by file, each list
//!   sorted innermost first by `(end - start, start, id)`, so ties break by
//!   NodeId.
//! - BIND. A definition row binds the innermost [`DEF_KINDS`] span of its file
//!   that holds its line and whose nav name equals the row's name EXACTLY; none
//!   is `unbound`. A symbol whose rows bind two different nodes is `ambiguous`
//!   and dropped, never guessed.
//! - OWNER. A reference's owner is the innermost FUNCTION / METHOD span that
//!   holds its line, else the innermost CLASS / STRUCT / INTERFACE / ENUM span,
//!   else the file's MODULE (several MODULEs on one file: the smallest NodeId);
//!   a line past the file's end, or a file with no MODULE, is `unowned`.
//! - EDGES. Per reference to a bound symbol `T`: an import row is counted
//!   (`imports`) and emits nothing (the parser's IMPORTS are already FACT);
//!   `T` a FUNCTION / METHOD and the row a call is CALLS rule [`RULE_CALL`];
//!   `T` callable and no call is USES [`RULE_CALLABLE_REF`]; any other `T` is
//!   USES [`RULE_REFERENCE`] ([`RULE_REFERENCE_WRITE`] on a write). An owner
//!   that is `T` itself keeps only a CALLS (recursion); a self USES is counted
//!   `self_refs` and dropped. Candidates dedup on `(owner, T, category)`,
//!   keeping the smallest `(file, line)`: documents come in path order and
//!   rows in line order (`read_scip` rejects any other), so the first seen
//!   wins.
//! - HERITAGE (CE.1e). Per symbol `A` bound to node `a` and per id `B` of its
//!   `implements` (the index's `is_implementation` relationships) bound to a
//!   node `b != a`, one `(a, b)` relationship: IMPLEMENTS when both are
//!   FUNCTION / METHOD (a method implementing an interface method), or `b` is
//!   an INTERFACE and `a` a CLASS / STRUCT / ENUM; INHERITS_FROM for any other
//!   pair of type kinds (an interface extending an interface included, glia's
//!   own LD.7a shape). Any other pair of kinds (an attribute implementing an
//!   interface property, say) is counted `other_kinds` and adds nothing.
//!   Relationships dedup on `(a, b, category)`, located at `A`'s first
//!   definition row.
//! - EXISTING. Relationships go first, then reference candidates, each
//!   against every repo graph edge and cross edge between its two nodes. A
//!   triple glia already has goes through CONFIRM (a reference candidate
//!   also counts `confirmed`). Otherwise, a reference candidate whose
//!   `(owner, T)` pair glia already joins by any other category (DEFINES and
//!   INHERITS_FROM included, and a heritage edge a relationship just added)
//!   and a relationship whose pair glia joins by the OTHER heritage category
//!   count `category_differs`: glia's own classification stands and nothing is
//!   added. Otherwise the stage pushes one cross edge, Strong, with ONE
//!   EVIDENCE cell: emitter `scip:<tool>` ([`tool_tag`]), the rule
//!   ([`RULE_IMPLEMENTATION`] for a relationship), the repo-relative file and
//!   0-based line, basis `site`. The Finalize fill leaves a located evidence
//!   alone and the cross-edge sort orders the edges.
//! - CONFIRM (CE.1e). Each glia edge on a confirmed triple, by its EVIDENCE:
//!   none, or an emitter stage outside [`RESTAMP_STAGES`] (resolver, pass,
//!   docs, overlay, history, scip), is `confirmed_other` and untouched: a
//!   layout merge drops and recomputes resolver / pass edges by emitter, so a
//!   re-stamped one would come back twice. Otherwise the evidence is settled
//!   as the Finalize fill will leave it (a file-less one placed from its
//!   endpoints, [`settle`]) and tiered by `why::tier_of`: `fact` is
//!   `confirmed_fact` and untouched; `heuristic` or `derived` (a name-only
//!   rule, an inferred below-Strong `graph` binding, no location) is
//!   `upgraded`: its ONE EVIDENCE cell is replaced (`evidence::attach`) by
//!   emitter `scip:<tool>`, rule `confirms:<old emitter>[/<old rule>]`, at the
//!   edge's own site when the index binds its target there, else at the
//!   triple's, basis `site`, and its confidence becomes Strong. Every glia
//!   edge on one triple is judged alone. Re-stamps are collected with their
//!   [`EdgePos`], sorted, and applied after every lookup.
//! - CONTRADICT (CE.1e). A glia edge of the repo (its graphs' edges, and the
//!   cross edges leaving a repo node) that is not re-stamped, is CALLS / USES /
//!   INHERITS_FROM / IMPLEMENTS, tiers `heuristic`, has a settled site
//!   `(file, line)` in a fresh document, and whose `to` the index binds to some
//!   symbol, is `contradicted` when the index binds, at that exact row,
//!   another node and not `to` ([`site_targets`]: call references for CALLS,
//!   references for USES, what `from`'s definition there implements for
//!   heritage). Counted and printed, never removed or changed.
//!
//! Determinism: every output-bearing iteration is over the snapshot's sorted
//! rows, BTreeMaps or sorted Vecs; the existing-edge HashMap is lookup-only,
//! re-stamps apply in [`EdgePos`] order and contradiction lines sort by
//! `(file, line)` then text.
//!
//! Markers, once per repo with a complete snapshot, in this order (the first
//! two are the fired_on lines):
//!   `[scip] ingest repo=<label> tool=<tool> documents=<d> stale=<s> defs=<n> bound=<b> unbound=<u> ambiguous=<a> refs=<r> imports=<i> unowned=<o> added=<e> (calls=<c> uses=<x>) confirmed=<f> category_differs=<k> self_refs=<z>`
//! where `defs` / `unbound` count definition rows of fresh documents, `bound`
//! / `ambiguous` count symbols, `refs` counts the reference rows of fresh
//! documents whose symbol is bound (`imports`, `unowned` and `self_refs`
//! among them), `added` / `confirmed` count distinct `(owner, T, category)`
//! candidates and `category_differs` those candidates plus relationships;
//!   `[scip] confirm repo=<label> upgraded=<u> confirmed_fact=<f> confirmed_other=<o> relationships=<r> added_implements=<i> added_inherits=<h> contradicted=<c> other_kinds=<k>`
//! where `upgraded` / `confirmed_fact` / `confirmed_other` / `contradicted`
//! count glia edges and `relationships` / `added_*` / `other_kinds` distinct
//! relationship pairs; then at most [`MAX_CONTRADICTION_LINES`] of
//!   `[scip] contradicts <from qname> -[<CAT> <emitter>[/<rule>]]-> <to qname> at <file>:<line+1>; index says <qname>`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::evidence::{self, Basis, Evidence, Location};
use glia_code_domain::snapshots::{ScipDocumentRecord, ScipSymbolRecord, read_scip, source_hash};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Confidence, Edge, EdgeCategoryId, NodeId, NodeKindId, RepoId};
use glia_graph::MergedGraph;

use super::RepoInputs;
use super::history::{module_files, position};
use crate::why;

/// The EVIDENCE stage of every edge this module adds: emitter `scip:<tool>`.
pub(crate) const STAGE: &str = "scip";

/// A reference to a FUNCTION / METHOD followed by a call paren.
pub(crate) const RULE_CALL: &str = "call_site";
/// A reference to a FUNCTION / METHOD that is not a call (a method value).
pub(crate) const RULE_CALLABLE_REF: &str = "callable_ref";
/// A reference to any other definition.
pub(crate) const RULE_REFERENCE: &str = "reference";
/// ... that the index marks a write access.
pub(crate) const RULE_REFERENCE_WRITE: &str = "reference_write";
/// A heritage edge added from the index's `is_implementation` relationship.
pub(crate) const RULE_IMPLEMENTATION: &str = "implementation";
/// The rule prefix of a glia edge the index confirmed:
/// `confirms:<old emitter>[/<old rule>]` (`why::tier_of` reads it back).
pub(crate) const RULE_CONFIRMS: &str = "confirms:";

/// The emitter stages whose edges a confirmation may re-stamp: the ones a
/// layout merge keeps once (`merge::is_recomputed` drops `resolver:` /
/// `pass:` edges and recomputes them by emitter).
const RESTAMP_STAGES: [&str; 3] = ["parser", "extractor", "graph"];

/// The categories a contradiction is looked for on.
const CONTRADICT_CATEGORIES: [EdgeCategoryId; 4] =
    [edge_category::CALLS, edge_category::USES, edge_category::INHERITS_FROM, edge_category::IMPLEMENTS];

/// Most `[scip] contradicts` lines one repo prints; the marker counts all.
const MAX_CONTRADICTION_LINES: usize = 20;

/// The node kinds a definition row may bind.
const DEF_KINDS: [NodeKindId; 8] = [
    node_kind::FUNCTION,
    node_kind::METHOD,
    node_kind::CLASS,
    node_kind::STRUCT,
    node_kind::INTERFACE,
    node_kind::ENUM,
    node_kind::ATTRIBUTE,
    node_kind::STATE_VAR,
];

/// Owners of a reference, tried first; also the targets a call binds as CALLS.
const CALLABLE_KINDS: [NodeKindId; 2] = [node_kind::FUNCTION, node_kind::METHOD];

/// Owners of a reference no callable span holds.
const TYPE_KINDS: [NodeKindId; 4] = [node_kind::CLASS, node_kind::STRUCT, node_kind::INTERFACE, node_kind::ENUM];

/// What one repo's snapshot did to its graph: the marker's counts (module doc).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct ScipTally {
    pub(super) tool: String,
    pub(super) documents: usize,
    pub(super) stale: usize,
    pub(super) defs: usize,
    pub(super) bound: usize,
    pub(super) unbound: usize,
    pub(super) ambiguous: usize,
    pub(super) refs: usize,
    pub(super) imports: usize,
    pub(super) unowned: usize,
    pub(super) self_refs: usize,
    pub(super) added: usize,
    pub(super) calls: usize,
    pub(super) uses: usize,
    pub(super) confirmed: usize,
    pub(super) category_differs: usize,
    pub(super) upgraded: usize,
    pub(super) confirmed_fact: usize,
    pub(super) confirmed_other: usize,
    pub(super) relationships: usize,
    pub(super) other_kinds: usize,
    pub(super) added_implements: usize,
    pub(super) added_inherits: usize,
    pub(super) contradicted: usize,
}

/// One located node: its POSITION rows (0-based, inclusive), id and kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    start: u32,
    end: u32,
    id: NodeId,
    kind: NodeKindId,
}

impl Span {
    fn holds(&self, line: u32) -> bool {
        self.start <= line && line <= self.end
    }

    /// Innermost first; ties break by NodeId.
    fn key(&self) -> (u32, u32, u64) {
        (self.end - self.start, self.start, self.id.0)
    }
}

/// One file's spans, each list innermost first.
#[derive(Debug, Default)]
struct FileSpans {
    /// [`DEF_KINDS`] spans by nav name.
    defs: BTreeMap<String, Vec<Span>>,
    /// [`CALLABLE_KINDS`] and [`TYPE_KINDS`] spans.
    owners: Vec<Span>,
}

/// The repo's located nodes by repo-relative file, and its MODULEs.
struct SpanIndex {
    files: BTreeMap<String, FileSpans>,
    modules: BTreeMap<String, NodeId>,
}

impl SpanIndex {
    fn build(merged: &MergedGraph, repo: RepoId) -> Self {
        let mut files: BTreeMap<String, FileSpans> = BTreeMap::new();
        for g in merged.graphs.iter().filter(|g| g.repo == repo) {
            for n in &g.nodes {
                let Some(&kind) = g.nav.kind_by_id.get(&n.id) else { continue };
                let is_def = DEF_KINDS.contains(&kind);
                let is_owner = CALLABLE_KINDS.contains(&kind) || TYPE_KINDS.contains(&kind);
                if !is_def && !is_owner {
                    continue;
                }
                let Some((file, Some(s0), e0)) = position(n) else { continue };
                let (Ok(start), Ok(end)) = (u32::try_from(s0), u32::try_from(e0.unwrap_or(s0).max(s0))) else {
                    continue;
                };
                let span = Span { start, end, id: n.id, kind };
                let fs = files.entry(file).or_default();
                if is_def && let Some(name) = g.nav.name_by_id.get(&n.id) {
                    fs.defs.entry(name.clone()).or_default().push(span);
                }
                if is_owner {
                    fs.owners.push(span);
                }
            }
        }
        for fs in files.values_mut() {
            for spans in fs.defs.values_mut() {
                innermost_first(spans);
            }
            innermost_first(&mut fs.owners);
        }
        SpanIndex { files, modules: module_files(merged, repo) }
    }

    /// The innermost [`DEF_KINDS`] span of `file` named `name` holding `line`.
    fn def_at(&self, file: &str, name: &str, line: u32) -> Option<Span> {
        self.files.get(file)?.defs.get(name)?.iter().find(|s| s.holds(line)).copied()
    }
}

/// Sort innermost first and drop a node listed twice (one id in two graphs).
fn innermost_first(spans: &mut Vec<Span>) {
    spans.sort_by_key(Span::key);
    spans.dedup_by_key(|s| s.id.0);
}

/// Line -> owner for one file of `lines` rows (module doc: a callable span
/// first, else a type span). Written tier by tier, types then callables, each
/// outermost first, so the last write to a row is its innermost span of the
/// strongest tier. O(sum of the owner spans' lengths), clamped to the file.
fn owner_table(owners: &[Span], lines: usize) -> Vec<Option<NodeId>> {
    let mut table: Vec<Option<NodeId>> = vec![None; lines];
    for tier in [&TYPE_KINDS[..], &CALLABLE_KINDS[..]] {
        for s in owners.iter().rev().filter(|s| tier.contains(&s.kind)) {
            let lo = s.start as usize;
            if lo >= lines {
                continue;
            }
            let hi = (s.end as usize).min(lines - 1);
            for slot in &mut table[lo..=hi] {
                *slot = Some(s.id);
            }
        }
    }
    table
}

/// The emitter name of `tool` (the meta's indexer name): lowercased, every
/// char outside `[a-z0-9_-]` as `_`; an empty name is `index`.
pub(crate) fn tool_tag(tool: &str) -> String {
    let tag: String = tool
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    if tag.is_empty() { "index".to_string() } else { tag }
}

/// A fresh document and its row count (`\n`s + 1).
struct Fresh<'a> {
    doc: &'a ScipDocumentRecord,
    lines: usize,
}

/// One edge the snapshot asserts (a reference candidate or a heritage
/// relationship), before the existing-edge check.
struct Candidate<'a> {
    from: NodeId,
    to: NodeId,
    category: EdgeCategoryId,
    rule: &'static str,
    file: &'a str,
    line: u32,
}

/// A bound symbol: its node and kind, and its first definition row (the
/// documents in path order, rows in line order).
#[derive(Debug, Clone, Copy)]
struct Bound<'a> {
    id: NodeId,
    kind: NodeKindId,
    file: &'a str,
    line: u32,
}

/// Bind every definition row of `fresh` (module doc: BIND). Symbol id ->
/// its node, kind and first definition row.
fn bind_defs<'a>(fresh: &[Fresh<'a>], index: &SpanIndex, tally: &mut ScipTally) -> BTreeMap<u32, Bound<'a>> {
    // Symbol -> the distinct nodes its rows bind, in first-seen order.
    let mut seen: BTreeMap<u32, Vec<Bound<'a>>> = BTreeMap::new();
    for f in fresh {
        let doc: &'a ScipDocumentRecord = f.doc;
        for row in &doc.defs {
            tally.defs += 1;
            match index.def_at(&doc.path, &row.name, row.line) {
                None => tally.unbound += 1,
                Some(span) => {
                    let nodes = seen.entry(row.s).or_default();
                    if !nodes.iter().any(|n| n.id == span.id) {
                        nodes.push(Bound { id: span.id, kind: span.kind, file: doc.path.as_str(), line: row.line });
                    }
                }
            }
        }
    }
    let mut bound: BTreeMap<u32, Bound<'a>> = BTreeMap::new();
    for (symbol, nodes) in seen {
        match nodes.as_slice() {
            [one] => {
                bound.insert(symbol, *one);
            }
            _ => tally.ambiguous += 1,
        }
    }
    tally.bound = bound.len();
    bound
}

/// The heritage category of `a` implementing or extending `b` (module doc:
/// HERITAGE); `None` for a pair of kinds that is no heritage shape.
fn heritage_category(a: NodeKindId, b: NodeKindId) -> Option<EdgeCategoryId> {
    let callable = |k: NodeKindId| CALLABLE_KINDS.contains(&k);
    let is_type = |k: NodeKindId| TYPE_KINDS.contains(&k);
    if callable(a) && callable(b) {
        Some(edge_category::IMPLEMENTS)
    } else if is_type(a) && is_type(b) {
        Some(if b == node_kind::INTERFACE && a != node_kind::INTERFACE {
            edge_category::IMPLEMENTS
        } else {
            edge_category::INHERITS_FROM
        })
    } else {
        None
    }
}

fn is_heritage(c: EdgeCategoryId) -> bool {
    c == edge_category::IMPLEMENTS || c == edge_category::INHERITS_FROM
}

/// Every `is_implementation` relationship between two bound symbols, as
/// deduped candidates keyed `(a, b, category)` (module doc: HERITAGE).
fn relationships<'a>(
    symbols: &[ScipSymbolRecord],
    bound: &BTreeMap<u32, Bound<'a>>,
    tally: &mut ScipTally,
) -> BTreeMap<(u64, u64, u32), Candidate<'a>> {
    let mut out: BTreeMap<(u64, u64, u32), Candidate<'a>> = BTreeMap::new();
    let mut other: BTreeSet<(u64, u64)> = BTreeSet::new();
    for sym in symbols {
        let Some(a) = bound.get(&sym.id) else { continue };
        for b in sym.implements.iter().filter_map(|id| bound.get(id)) {
            if a.id == b.id {
                continue;
            }
            let Some(category) = heritage_category(a.kind, b.kind) else {
                other.insert((a.id.0, b.id.0));
                continue;
            };
            let at = (a.file, a.line);
            out.entry((a.id.0, b.id.0, category.0))
                .and_modify(|c| {
                    if at < (c.file, c.line) {
                        (c.file, c.line) = at;
                    }
                })
                .or_insert(Candidate {
                    from: a.id,
                    to: b.id,
                    category,
                    rule: RULE_IMPLEMENTATION,
                    file: a.file,
                    line: a.line,
                });
        }
    }
    tally.relationships = out.len();
    tally.other_kinds = other.len();
    out
}

/// What the index binds at one row of a fresh document: a reference's target
/// (`call` when a call paren follows), or a symbol the row's definition
/// implements (`heritage`, `from` its definer).
#[derive(Debug, Clone, Copy)]
struct SiteRow {
    node: NodeId,
    from: Option<NodeId>,
    call: bool,
    heritage: bool,
}

/// file -> 0-based line -> the rows there.
type SiteRows<'a> = BTreeMap<&'a str, BTreeMap<u32, Vec<SiteRow>>>;

/// Every non-import reference to a bound symbol, and every bound symbol a
/// bound definition implements, by `(file, line)`.
fn site_rows<'a>(fresh: &[Fresh<'a>], symbols: &[ScipSymbolRecord], bound: &BTreeMap<u32, Bound<'a>>) -> SiteRows<'a> {
    let mut out: SiteRows<'a> = BTreeMap::new();
    for f in fresh {
        let doc: &'a ScipDocumentRecord = f.doc;
        let lines = out.entry(doc.path.as_str()).or_default();
        for row in doc.refs.iter().filter(|r| !r.import) {
            if let Some(b) = bound.get(&row.s) {
                lines.entry(row.line).or_default().push(SiteRow {
                    node: b.id,
                    from: None,
                    call: row.call,
                    heritage: false,
                });
            }
        }
        for row in &doc.defs {
            let (Some(a), Some(sym)) = (bound.get(&row.s), symbols.get(row.s as usize)) else { continue };
            for b in sym.implements.iter().filter_map(|id| bound.get(id)) {
                lines.entry(row.line).or_default().push(SiteRow {
                    node: b.id,
                    from: Some(a.id),
                    call: false,
                    heritage: true,
                });
            }
        }
    }
    out
}

/// The rows at `(file, line)`; empty for a stale or unknown document.
fn rows_at<'s>(sites: &'s SiteRows<'_>, file: &str, line: u32) -> &'s [SiteRow] {
    sites.get(file).and_then(|lines| lines.get(&line)).map_or(&[], Vec::as_slice)
}

/// The nodes `rows` bind for an edge `from -[category]->`: call references
/// for CALLS, every reference for USES, what `from`'s definition there
/// implements for IMPLEMENTS / INHERITS_FROM.
fn site_targets(rows: &[SiteRow], from: NodeId, category: EdgeCategoryId) -> impl Iterator<Item = NodeId> + '_ {
    let heritage = is_heritage(category);
    rows.iter()
        .filter(move |r| {
            if heritage {
                r.heritage && r.from == Some(from)
            } else {
                !r.heritage && (r.call || category != edge_category::CALLS)
            }
        })
        .map(|r| r.node)
}

/// Where a glia edge sits: lookup and mutation only, never output order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum EdgePos {
    /// `merged.graphs[graph].edges[edge]`.
    Intra { graph: usize, edge: usize },
    /// `merged.cross_edges[i]`.
    Cross(usize),
    /// An edge this stage pushed: it joins its pair for the later
    /// category checks and is never re-stamped.
    Added,
}

fn edge_at(merged: &MergedGraph, pos: EdgePos) -> Option<&Edge> {
    match pos {
        EdgePos::Intra { graph, edge } => merged.graphs.get(graph)?.edges.get(edge),
        EdgePos::Cross(i) => merged.cross_edges.get(i),
        EdgePos::Added => None,
    }
}

fn edge_at_mut(merged: &mut MergedGraph, pos: EdgePos) -> Option<&mut Edge> {
    match pos {
        EdgePos::Intra { graph, edge } => merged.graphs.get_mut(graph)?.edges.get_mut(edge),
        EdgePos::Cross(i) => merged.cross_edges.get_mut(i),
        EdgePos::Added => None,
    }
}

/// Node -> its location and kind, first graph first: the Finalize fill's view
/// of an endpoint (`passes::fill_evidence_sites`).
type NodeAt = HashMap<NodeId, (Option<Location>, Option<NodeKindId>)>;

fn node_at(merged: &MergedGraph) -> NodeAt {
    let mut at = NodeAt::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            at.entry(n.id).or_insert_with(|| (evidence::locate(&n.cells), g.nav.kind_by_id.get(&n.id).copied()));
        }
    }
    at
}

/// `ev` as the Finalize fill will leave it on `e`: a file-less evidence placed
/// from the endpoints' locations (`Evidence::fill`), so a stage that runs
/// before the fill tiers an edge as `why` will. The fill's LB.9b fallback
/// (an unlocated non-code MODULE's file) is not replayed: it applies only when
/// neither endpoint is located, and a triple's target is always a located,
/// bound node.
fn settle(ev: &Evidence, e: &Edge, at: &NodeAt) -> Evidence {
    let mut ev = ev.clone();
    if ev.file.is_none() {
        let from = at.get(&e.from);
        let to = at.get(&e.to);
        ev.fill(e.category, to.and_then(|t| t.1), from.and_then(|f| f.0.as_ref()), to.and_then(|t| t.0.as_ref()));
    }
    ev
}

/// What one asserted triple met among glia's edges.
enum Outcome {
    /// Glia has the triple: every edge on it went through CONFIRM.
    Confirmed,
    /// Glia joins the pair by a blocking category: nothing added.
    Differs,
    /// Pushed as a new cross edge.
    Added,
}

/// The existing-edge check and the confirm rule (module doc: EXISTING,
/// CONFIRM). Reads the graph only; the re-stamps and new edges it collects are
/// applied by [`scip_edges`] after every lookup.
struct Confirm<'c, 'a> {
    merged: &'c MergedGraph,
    emitter: &'c str,
    at: &'c NodeAt,
    sites: &'c SiteRows<'a>,
    /// Pair -> glia's edges between the two, in graph-then-cross scan order.
    existing: HashMap<(NodeId, NodeId), Vec<(EdgeCategoryId, EdgePos)>>,
    restamp: Vec<(EdgePos, Evidence)>,
    added: Vec<Edge>,
}

impl Confirm<'_, '_> {
    /// Confirm `c` when glia has its triple; else add it unless glia joins the
    /// pair by a category `blocks` accepts.
    fn confirm_or_add(&mut self, c: &Candidate<'_>, blocks: fn(EdgeCategoryId) -> bool, tally: &mut ScipTally) -> Outcome {
        let (same, blocked) = {
            let entries = self.existing.get(&(c.from, c.to)).map_or(&[][..], Vec::as_slice);
            let same: Option<Vec<EdgePos>> = entries.iter().any(|(k, _)| *k == c.category).then(|| {
                entries.iter().filter(|(k, p)| *k == c.category && *p != EdgePos::Added).map(|(_, p)| *p).collect()
            });
            (same, entries.iter().any(|(k, _)| blocks(*k)))
        };
        if let Some(positions) = same {
            for pos in positions {
                self.confirm(pos, c, tally);
            }
            return Outcome::Confirmed;
        }
        if blocked {
            return Outcome::Differs;
        }
        let ev = Evidence::emitter(self.emitter).rule(c.rule).at(c.file, c.line);
        self.added.push(Edge::new(c.from, c.to, c.category, Confidence::Strong).with_cell(ev.to_cell()));
        self.existing.entry((c.from, c.to)).or_default().push((c.category, EdgePos::Added));
        Outcome::Added
    }

    /// The confirm rule on the glia edge at `pos`, which carries `c`'s triple.
    fn confirm(&mut self, pos: EdgePos, c: &Candidate<'_>, tally: &mut ScipTally) {
        let Some(e) = edge_at(self.merged, pos) else { return };
        let Some(old) = Evidence::of(e) else {
            tally.confirmed_other += 1;
            return;
        };
        let stage = old.emitter.split(':').next().unwrap_or("");
        if !RESTAMP_STAGES.contains(&stage) {
            tally.confirmed_other += 1;
            return;
        }
        let settled = settle(&old, e, self.at);
        if why::tier_of(Some(&settled), e).0 == why::FACT {
            tally.confirmed_fact += 1;
            return;
        }
        // The edge's own site when the index binds its target there (two
        // call sites stay two sites), else the triple's.
        let own = match (&settled.file, settled.line, settled.basis) {
            (Some(file), Some(line), Basis::Site)
                if site_targets(rows_at(self.sites, file, line), e.from, e.category).any(|n| n == e.to) =>
            {
                Some((file.clone(), line))
            }
            _ => None,
        };
        let (file, line) = own.unwrap_or_else(|| (c.file.to_string(), c.line));
        let rule = match &old.rule {
            Some(r) => format!("{RULE_CONFIRMS}{}/{r}", old.emitter),
            None => format!("{RULE_CONFIRMS}{}", old.emitter),
        };
        self.restamp.push((pos, Evidence::emitter(self.emitter).rule(rule).at(file, line)));
        tally.upgraded += 1;
    }
}

/// The qname of `id` in the first graph that names it.
fn qname_of(merged: &MergedGraph, id: NodeId) -> String {
    merged
        .graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id))
        .cloned()
        .unwrap_or_else(|| format!("#{}", id.0))
}

/// The `[scip] contradicts` lines of the repo's heuristic edges the index
/// binds elsewhere (module doc: CONTRADICT), sorted by `(file, line)` then
/// text. `restamped` edges are confirmed, never contradicted.
fn contradictions(
    merged: &MergedGraph,
    repo: RepoId,
    at: &NodeAt,
    sites: &SiteRows<'_>,
    known: &HashSet<NodeId>,
    restamped: &HashSet<EdgePos>,
) -> Vec<(String, u32, String)> {
    let repo_nodes: HashSet<NodeId> = merged
        .graphs
        .iter()
        .filter(|g| g.repo == repo)
        .flat_map(|g| g.nav.kind_by_id.keys().copied())
        .collect();
    let intra = merged
        .graphs
        .iter()
        .enumerate()
        .filter(|(_, g)| g.repo == repo)
        .flat_map(|(gi, g)| g.edges.iter().enumerate().map(move |(ei, e)| (EdgePos::Intra { graph: gi, edge: ei }, e)));
    let cross = merged
        .cross_edges
        .iter()
        .enumerate()
        .filter(|(_, e)| repo_nodes.contains(&e.from))
        .map(|(i, e)| (EdgePos::Cross(i), e));
    let mut out: Vec<(String, u32, String)> = Vec::new();
    for (pos, e) in intra.chain(cross) {
        if !CONTRADICT_CATEGORIES.contains(&e.category) || !known.contains(&e.to) || restamped.contains(&pos) {
            continue;
        }
        let Some(ev) = Evidence::of(e) else { continue };
        let ev = settle(&ev, e, at);
        if why::tier_of(Some(&ev), e).0 != why::HEURISTIC {
            continue;
        }
        let (Some(file), Some(line), Basis::Site) = (&ev.file, ev.line, ev.basis) else { continue };
        let targets: Vec<NodeId> = site_targets(rows_at(sites, file, line), e.from, e.category).collect();
        let Some(&says) = targets.first() else { continue };
        if targets.contains(&e.to) {
            continue;
        }
        let rule = ev.rule.as_deref().map(|r| format!("/{r}")).unwrap_or_default();
        let text = format!(
            "[scip] contradicts {} -[{} {}{rule}]-> {} at {file}:{}; index says {}",
            qname_of(merged, e.from),
            edge_category::name(e.category),
            ev.emitter,
            qname_of(merged, e.to),
            u64::from(line) + 1,
            qname_of(merged, says),
        );
        out.push((file.clone(), line, text));
    }
    out.sort();
    out
}

/// Every reference row of `fresh` to a bound symbol, as deduped candidates
/// keyed `(owner, T, category)` (module doc: OWNER, EDGES).
fn ref_candidates<'a>(
    fresh: &[Fresh<'a>],
    index: &SpanIndex,
    bound: &BTreeMap<u32, Bound<'_>>,
    tally: &mut ScipTally,
) -> BTreeMap<(u64, u64, u32), Candidate<'a>> {
    let mut out: BTreeMap<(u64, u64, u32), Candidate<'a>> = BTreeMap::new();
    for f in fresh {
        let path = f.doc.path.as_str();
        let module = index.modules.get(path).copied();
        // Built on the document's first owned reference.
        let mut table: Option<Vec<Option<NodeId>>> = None;
        for row in &f.doc.refs {
            let Some(&Bound { id: target, kind: target_kind, .. }) = bound.get(&row.s) else { continue };
            tally.refs += 1;
            if row.import {
                tally.imports += 1;
                continue;
            }
            let table = table.get_or_insert_with(|| {
                let owners = index.files.get(path).map_or(&[][..], |fs| fs.owners.as_slice());
                owner_table(owners, f.lines)
            });
            let owner = match table.get(row.line as usize) {
                Some(Some(id)) => Some(*id),
                Some(None) => module,
                None => None,
            };
            let Some(owner) = owner else {
                tally.unowned += 1;
                continue;
            };
            let callable = CALLABLE_KINDS.contains(&target_kind);
            let (category, rule) = match (callable, row.call) {
                (true, true) => (edge_category::CALLS, RULE_CALL),
                (true, false) => (edge_category::USES, RULE_CALLABLE_REF),
                (false, _) if row.write => (edge_category::USES, RULE_REFERENCE_WRITE),
                (false, _) => (edge_category::USES, RULE_REFERENCE),
            };
            if owner == target && category != edge_category::CALLS {
                tally.self_refs += 1;
                continue;
            }
            out.entry((owner.0, target.0, category.0)).or_insert(Candidate {
                from: owner,
                to: target,
                category,
                rule,
                file: path,
                line: row.line,
            });
        }
    }
    out
}

/// The SCIP edge stage for one repo (module doc). `None`, with nothing
/// changed, when the repo has no complete snapshot.
pub(super) fn scip_edges(merged: &mut MergedGraph, input: &RepoInputs) -> Option<ScipTally> {
    let snap = read_scip(&input.root)?;
    let tool = tool_tag(&snap.meta.tool);
    let mut tally = ScipTally { tool: tool.clone(), documents: snap.documents.len(), ..ScipTally::default() };

    let mut fresh: Vec<Fresh<'_>> = Vec::new();
    for doc in &snap.documents {
        match std::fs::read(input.root.join(&doc.path)) {
            Ok(bytes) if source_hash(&bytes) == doc.source_hash => {
                let lines = bytes.iter().filter(|&&b| b == b'\n').count() + 1;
                fresh.push(Fresh { doc, lines });
            }
            _ => tally.stale += 1,
        }
    }

    let index = SpanIndex::build(merged, input.repo);
    let bound = bind_defs(&fresh, &index, &mut tally);
    let candidates = ref_candidates(&fresh, &index, &bound, &mut tally);
    let relations = relationships(&snap.symbols, &bound, &mut tally);
    let sites = site_rows(&fresh, &snap.symbols, &bound);

    // Glia's edges between each asserted pair: one pass over the repo's
    // edges and every cross edge, lookup-only.
    let mut existing: HashMap<(NodeId, NodeId), Vec<(EdgeCategoryId, EdgePos)>> =
        candidates.values().chain(relations.values()).map(|c| ((c.from, c.to), Vec::new())).collect();
    if !existing.is_empty() {
        for (gi, g) in merged.graphs.iter().enumerate().filter(|(_, g)| g.repo == input.repo) {
            for (ei, e) in g.edges.iter().enumerate() {
                if let Some(v) = existing.get_mut(&(e.from, e.to)) {
                    v.push((e.category, EdgePos::Intra { graph: gi, edge: ei }));
                }
            }
        }
        for (i, e) in merged.cross_edges.iter().enumerate() {
            if let Some(v) = existing.get_mut(&(e.from, e.to)) {
                v.push((e.category, EdgePos::Cross(i)));
            }
        }
    }

    let emitter = format!("{STAGE}:{tool}");
    let at = node_at(merged);
    let mut stage = Confirm {
        merged: &*merged,
        emitter: emitter.as_str(),
        at: &at,
        sites: &sites,
        existing,
        restamp: Vec::new(),
        added: Vec::new(),
    };
    // Heritage first, so the class-header reference a relationship explains
    // meets its heritage edge and adds no USES beside it.
    for r in relations.values() {
        match stage.confirm_or_add(r, is_heritage, &mut tally) {
            Outcome::Confirmed => {}
            Outcome::Differs => tally.category_differs += 1,
            Outcome::Added if r.category == edge_category::IMPLEMENTS => tally.added_implements += 1,
            Outcome::Added => tally.added_inherits += 1,
        }
    }
    for c in candidates.values() {
        match stage.confirm_or_add(c, |_| true, &mut tally) {
            Outcome::Confirmed => tally.confirmed += 1,
            Outcome::Differs => tally.category_differs += 1,
            Outcome::Added if c.category == edge_category::CALLS => tally.calls += 1,
            Outcome::Added => tally.uses += 1,
        }
    }
    let Confirm { mut restamp, added, .. } = stage;
    tally.added = tally.calls + tally.uses;

    let restamped: HashSet<EdgePos> = restamp.iter().map(|(p, _)| *p).collect();
    let known: HashSet<NodeId> = bound.values().map(|b| b.id).collect();
    let contradicts = contradictions(merged, input.repo, &at, &sites, &known, &restamped);
    tally.contradicted = contradicts.len();

    restamp.sort_by_key(|(p, _)| *p);
    for (pos, ev) in restamp {
        if let Some(e) = edge_at_mut(merged, pos) {
            evidence::attach(e, ev);
            e.confidence = Confidence::Strong;
        }
    }
    merged.cross_edges.extend(added);

    eprintln!(
        "[scip] ingest repo={} tool={} documents={} stale={} defs={} bound={} unbound={} ambiguous={} refs={} imports={} unowned={} added={} (calls={} uses={}) confirmed={} category_differs={} self_refs={}",
        input.label,
        tally.tool,
        tally.documents,
        tally.stale,
        tally.defs,
        tally.bound,
        tally.unbound,
        tally.ambiguous,
        tally.refs,
        tally.imports,
        tally.unowned,
        tally.added,
        tally.calls,
        tally.uses,
        tally.confirmed,
        tally.category_differs,
        tally.self_refs,
    );
    eprintln!(
        "[scip] confirm repo={} upgraded={} confirmed_fact={} confirmed_other={} relationships={} added_implements={} added_inherits={} contradicted={} other_kinds={}",
        input.label,
        tally.upgraded,
        tally.confirmed_fact,
        tally.confirmed_other,
        tally.relationships,
        tally.added_implements,
        tally.added_inherits,
        tally.contradicted,
        tally.other_kinds,
    );
    for (_, _, line) in contradicts.iter().take(MAX_CONTRADICTION_LINES) {
        eprintln!("{line}");
    }
    Some(tally)
}

#[cfg(test)]
mod tests {
    use glia_code_domain::cell_type;
    use glia_code_domain::snapshots::{ScipDefRow, ScipMeta, ScipRefRow, ScipSymbolRecord, write_scip};
    use glia_core::{Cell, CellPayload, Node};

    use super::*;

    fn span(start: u32, end: u32, id: u64, kind: NodeKindId) -> Span {
        Span { start, end, id: NodeId(id), kind }
    }

    #[test]
    fn tool_tag_is_lowercase_and_safe() {
        assert_eq!(tool_tag("scip-python"), "scip-python");
        assert_eq!(tool_tag("Rust-Analyzer"), "rust-analyzer");
        assert_eq!(tool_tag("scip java 0.9"), "scip_java_0_9");
        assert_eq!(tool_tag("scip:ts"), "scip_ts");
        assert_eq!(tool_tag(""), "index");
    }

    /// A callable span beats a type span that is smaller; within a tier the
    /// innermost wins; rows past a span keep the outer owner.
    #[test]
    fn owner_table_prefers_callables_then_innermost() {
        let mut owners = vec![
            // f: rows 0..=9, a nested class C in it: rows 2..=4, a method m of
            // C: rows 3..=4, a top-level class K: rows 11..=13.
            span(0, 9, 10, node_kind::FUNCTION),
            span(2, 4, 20, node_kind::CLASS),
            span(3, 4, 30, node_kind::METHOD),
            span(11, 13, 40, node_kind::CLASS),
        ];
        innermost_first(&mut owners);
        let t = owner_table(&owners, 15);
        assert_eq!(t[0], Some(NodeId(10)));
        assert_eq!(t[2], Some(NodeId(10)), "the class line is inside f: f owns it");
        assert_eq!(t[3], Some(NodeId(30)));
        assert_eq!(t[5], Some(NodeId(10)));
        assert_eq!(t[10], None);
        assert_eq!(t[12], Some(NodeId(40)));
        assert_eq!(t[14], None);
        // A span running past the file is clamped, not an out-of-range write.
        assert_eq!(owner_table(&[span(1, 99, 7, node_kind::CLASS)], 3), vec![None, Some(NodeId(7)), Some(NodeId(7))]);
    }

    #[test]
    fn equal_spans_break_ties_by_id() {
        let mut owners = vec![span(1, 3, 9, node_kind::FUNCTION), span(1, 3, 4, node_kind::FUNCTION)];
        innermost_first(&mut owners);
        assert_eq!(owner_table(&owners, 4)[2], Some(NodeId(4)));
    }

    fn located(id: u64, repo: RepoId, file: &str, start: u32, end: u32) -> Node {
        Node {
            id: NodeId(id),
            repo,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(format!(r#"{{"file":"{file}","start_line":{start},"end_line":{end}}}"#)),
            }],
        }
    }

    /// One graph: MODULE a.py (1), FUNCTION `run` rows 0..=2 (2), FUNCTION
    /// `helper` rows 4..=5 (3), and a pre-existing USES run -> helper.
    fn graph(repo: RepoId) -> MergedGraph {
        let mut g = glia_graph::RepoGraph {
            repo,
            nodes: Vec::new(),
            edges: vec![Edge::new(NodeId(2), NodeId(3), edge_category::USES, Confidence::Strong)],
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: Default::default(),
        };
        for (id, kind, name, s, e) in [
            (1, node_kind::MODULE, "a", 0, 5),
            (2, node_kind::FUNCTION, "run", 0, 2),
            (3, node_kind::FUNCTION, "helper", 4, 5),
        ] {
            g.nodes.push(located(id, repo, "a.py", s, e));
            g.nav.kind_by_id.insert(NodeId(id), kind);
            g.nav.name_by_id.insert(NodeId(id), name.to_string());
        }
        MergedGraph::new(vec![g])
    }

    const A_PY: &str = "def run():\n    helper()\n    return helper\n\ndef helper():\n    run()\n";

    /// A snapshot of `A_PY` in a fresh temp dir named after `tag`: `run`
    /// (row 0) and `helper` (row 4) defined, and `refs` of `(symbol, row,
    /// call)` (symbol 0 is helper, 1 is run).
    fn a_py_snapshot(tag: &str, refs: &[(u32, u32, bool)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("glia_scip_{tag}_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), A_PY).unwrap();
        let symbols = vec![
            ScipSymbolRecord { id: 0, symbol: "scip-python python app 0.1 `a`/helper().".into(), implements: vec![] },
            ScipSymbolRecord { id: 1, symbol: "scip-python python app 0.1 `a`/run().".into(), implements: vec![] },
        ];
        let documents = vec![ScipDocumentRecord {
            path: "a.py".into(),
            language: "python".into(),
            source_hash: source_hash(A_PY.as_bytes()),
            defs: vec![ScipDefRow { s: 1, line: 0, name: "run".into() }, ScipDefRow { s: 0, line: 4, name: "helper".into() }],
            refs: refs.iter().map(|&(s, line, call)| ScipRefRow { s, line, call, write: false, import: false }).collect(),
        }];
        let meta = ScipMeta::new("scip-python".into(), "0.6.0".into(), String::new(), 0);
        write_scip(&dir, meta, &documents, &symbols).unwrap();
        dir
    }

    #[test]
    fn existing_category_stands_and_the_rest_is_added() {
        // run calls helper (row 1: glia has USES run -> helper, so the CALLS
        // differs), references it (row 2: confirms that USES), and helper
        // calls run (row 5: added).
        let dir = a_py_snapshot("stage", &[(0, 1, true), (0, 2, false), (1, 5, true)]);
        let repo = RepoId::from_canonical("test://scip-stage");
        let mut merged = graph(repo);
        let input = RepoInputs { repo, root: dir.clone(), label: "stage".into(), config: None };
        let tally = scip_edges(&mut merged, &input).expect("a complete snapshot");
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(
            (tally.defs, tally.bound, tally.refs, tally.added, tally.calls, tally.confirmed, tally.category_differs),
            (2, 2, 3, 1, 1, 1, 1),
            "{tally:?}"
        );
        // The confirmed USES carries no EVIDENCE cell: its emitter is unknown,
        // so it is never re-stamped.
        assert_eq!((tally.confirmed_other, tally.upgraded), (1, 0), "{tally:?}");
        assert_eq!(merged.cross_edges.len(), 1);
        let e = &merged.cross_edges[0];
        assert_eq!((e.from, e.to, e.category), (NodeId(3), NodeId(2), edge_category::CALLS));
        assert_eq!(
            Evidence::of(e),
            Some(Evidence::emitter("scip:scip-python").rule(RULE_CALL).at("a.py", 5))
        );
    }

    /// CE.1e: a resolver's CALLS the index confirms keeps its evidence and
    /// confidence (a merge recomputes it by emitter; a `scip:` re-stamp would
    /// come back twice). No resolver emits CALLS from real sources, so the
    /// edge is built by hand.
    #[test]
    fn resolver_edges_are_never_restamped() {
        let dir = a_py_snapshot("resolver", &[(1, 5, true)]);
        let repo = RepoId::from_canonical("test://scip-resolver");
        let mut merged = graph(repo);
        let ev = Evidence::emitter("resolver:rpc").rule("exact");
        merged
            .cross_edges
            .push(Edge::new(NodeId(3), NodeId(2), edge_category::CALLS, Confidence::Medium).with_cell(ev.to_cell()));
        let before = merged.cross_edges.clone();
        let input = RepoInputs { repo, root: dir.clone(), label: "resolver".into(), config: None };
        let tally = scip_edges(&mut merged, &input).expect("a complete snapshot");
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(merged.cross_edges, before, "a resolver edge was changed");
        assert_eq!(
            (tally.confirmed, tally.confirmed_other, tally.confirmed_fact, tally.upgraded, tally.added),
            (1, 1, 0, 0, 0),
            "{tally:?}"
        );
    }

    /// CE.1e: a name-only graph edge is re-stamped at its own site, keeping
    /// its first emitter and rule in the `confirms:` rule; a located fact is
    /// untouched.
    #[test]
    fn name_only_edge_is_upgraded_and_fact_is_kept() {
        let dir = a_py_snapshot("upgrade", &[(0, 1, true), (1, 5, true)]);
        let repo = RepoId::from_canonical("test://scip-upgrade");
        let mut merged = graph(repo);
        let name_only = Evidence::emitter("graph:refs").rule("global_unique_method").at("a.py", 1);
        let fact = Evidence::emitter("graph:calls").rule("import_binding").at("a.py", 5);
        merged.graphs[0].edges = vec![
            Edge::new(NodeId(2), NodeId(3), edge_category::CALLS, Confidence::Medium).with_cell(name_only.to_cell()),
            Edge::new(NodeId(3), NodeId(2), edge_category::CALLS, Confidence::Strong).with_cell(fact.to_cell()),
        ];
        let input = RepoInputs { repo, root: dir.clone(), label: "upgrade".into(), config: None };
        let tally = scip_edges(&mut merged, &input).expect("a complete snapshot");
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!((tally.upgraded, tally.confirmed_fact, tally.added), (1, 1, 0), "{tally:?}");
        let [up, kept] = merged.graphs[0].edges.as_slice() else { panic!("two edges") };
        assert_eq!(up.confidence, Confidence::Strong);
        assert_eq!(
            Evidence::of(up),
            Some(Evidence::emitter("scip:scip-python").rule("confirms:graph:refs/global_unique_method").at("a.py", 1))
        );
        assert_eq!(Evidence::of(kept), Some(fact));
    }

    #[test]
    fn heritage_category_follows_the_kinds() {
        use edge_category::{IMPLEMENTS, INHERITS_FROM};
        use node_kind::{ATTRIBUTE, CLASS, ENUM, FUNCTION, INTERFACE, METHOD, STRUCT};
        assert_eq!(heritage_category(CLASS, CLASS), Some(INHERITS_FROM));
        assert_eq!(heritage_category(CLASS, INTERFACE), Some(IMPLEMENTS));
        assert_eq!(heritage_category(STRUCT, INTERFACE), Some(IMPLEMENTS));
        assert_eq!(heritage_category(ENUM, INTERFACE), Some(IMPLEMENTS));
        assert_eq!(heritage_category(INTERFACE, INTERFACE), Some(INHERITS_FROM));
        assert_eq!(heritage_category(METHOD, METHOD), Some(IMPLEMENTS));
        assert_eq!(heritage_category(FUNCTION, METHOD), Some(IMPLEMENTS));
        assert_eq!(heritage_category(ATTRIBUTE, ATTRIBUTE), None);
        assert_eq!(heritage_category(CLASS, METHOD), None);
    }
}
