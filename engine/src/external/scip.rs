//! The SCIP stage (CE.1d): a repo's `.glia/scip-snapshot/` (CE.1a, written by
//! `glia scip import`, read back through `code_domain::snapshots::read_scip`)
//! turns the references a compiler-grade indexer resolved into CALLS / USES
//! edges between existing glia nodes. Its payoff is what the static resolver
//! cannot bind: calls through inferred types, dict / factory dispatch.
//!
//! SCIP is a FACT input, like history: it runs with or without `--no-overlay`.
//! A repo with no snapshot directory is a no-op that prints nothing (the build
//! stays byte-identical); an incomplete snapshot is `read_scip`'s
//! `[scip] snapshot incomplete` line and a no-op. The stage adds edges only,
//! never a node or a node cell, and runs in the single-threaded `Post` stage.
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
//! - EXISTING. A candidate whose `(owner, T, category)` edge glia already has
//!   (any repo graph edge or cross edge) is `confirmed`; one whose `(owner, T)`
//!   pair glia already joins by another category (DEFINES and INHERITS_FROM
//!   included) is `category_differs`: glia's own classification stands and
//!   nothing is added. Otherwise the stage pushes one cross edge, Strong, with
//!   ONE EVIDENCE cell: emitter `scip:<tool>` ([`tool_tag`]), the rule, the
//!   reference's repo-relative file and 0-based line, basis `site`. The
//!   Finalize fill leaves a located evidence alone and the cross-edge sort
//!   orders the edges.
//!
//! Determinism: every output-bearing iteration is over the snapshot's sorted
//! rows, BTreeMaps or sorted Vecs; the existing-edge HashMap is lookup-only.
//!
//! Marker, once per repo with a complete snapshot (the fired_on line):
//!   `[scip] ingest repo=<label> tool=<tool> documents=<d> stale=<s> defs=<n> bound=<b> unbound=<u> ambiguous=<a> refs=<r> imports=<i> unowned=<o> added=<e> (calls=<c> uses=<x>) confirmed=<f> category_differs=<k> self_refs=<z>`
//! where `defs` / `unbound` count definition rows of fresh documents, `bound`
//! / `ambiguous` count symbols, `refs` counts the reference rows of fresh
//! documents whose symbol is bound (`imports`, `unowned` and `self_refs`
//! among them) and `added` / `confirmed` / `category_differs` count distinct
//! `(owner, T, category)` candidates.

use std::collections::{BTreeMap, HashMap};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::snapshots::{ScipDocumentRecord, read_scip, source_hash};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Confidence, Edge, EdgeCategoryId, NodeId, NodeKindId, RepoId};
use glia_graph::MergedGraph;

use super::RepoInputs;
use super::history::{module_files, position};

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

/// One edge the snapshot asserts, before the existing-edge check.
struct Candidate<'a> {
    from: NodeId,
    to: NodeId,
    category: EdgeCategoryId,
    rule: &'static str,
    file: &'a str,
    line: u32,
}

/// Bind every definition row of `fresh` (module doc: BIND). Symbol id ->
/// its node and kind.
fn bind_defs(fresh: &[Fresh<'_>], index: &SpanIndex, tally: &mut ScipTally) -> BTreeMap<u32, (NodeId, NodeKindId)> {
    // Symbol -> the distinct nodes its rows bind, in first-seen order.
    let mut seen: BTreeMap<u32, Vec<Span>> = BTreeMap::new();
    for f in fresh {
        for row in &f.doc.defs {
            tally.defs += 1;
            match index.def_at(&f.doc.path, &row.name, row.line) {
                None => tally.unbound += 1,
                Some(span) => {
                    let nodes = seen.entry(row.s).or_default();
                    if !nodes.iter().any(|n| n.id == span.id) {
                        nodes.push(span);
                    }
                }
            }
        }
    }
    let mut bound: BTreeMap<u32, (NodeId, NodeKindId)> = BTreeMap::new();
    for (symbol, nodes) in seen {
        match nodes.as_slice() {
            [one] => {
                bound.insert(symbol, (one.id, one.kind));
            }
            _ => tally.ambiguous += 1,
        }
    }
    tally.bound = bound.len();
    bound
}

/// Every reference row of `fresh` to a bound symbol, as deduped candidates
/// keyed `(owner, T, category)` (module doc: OWNER, EDGES).
fn ref_candidates<'a>(
    fresh: &[Fresh<'a>],
    index: &SpanIndex,
    bound: &BTreeMap<u32, (NodeId, NodeKindId)>,
    tally: &mut ScipTally,
) -> BTreeMap<(u64, u64, u32), Candidate<'a>> {
    let mut out: BTreeMap<(u64, u64, u32), Candidate<'a>> = BTreeMap::new();
    for f in fresh {
        let path = f.doc.path.as_str();
        let module = index.modules.get(path).copied();
        // Built on the document's first owned reference.
        let mut table: Option<Vec<Option<NodeId>>> = None;
        for row in &f.doc.refs {
            let Some(&(target, target_kind)) = bound.get(&row.s) else { continue };
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

    // The categories glia already joins each candidate pair by: one pass over
    // the repo's edges and every cross edge, lookup-only.
    let mut existing: HashMap<(NodeId, NodeId), Vec<EdgeCategoryId>> =
        candidates.values().map(|c| ((c.from, c.to), Vec::new())).collect();
    if !existing.is_empty() {
        let repo_edges = merged.graphs.iter().filter(|g| g.repo == input.repo).flat_map(|g| g.edges.iter());
        for e in repo_edges.chain(merged.cross_edges.iter()) {
            if let Some(categories) = existing.get_mut(&(e.from, e.to)) {
                categories.push(e.category);
            }
        }
    }

    let emitter = format!("{STAGE}:{tool}");
    let mut added: Vec<Edge> = Vec::new();
    for c in candidates.values() {
        let categories = existing.get(&(c.from, c.to)).map_or(&[][..], Vec::as_slice);
        if categories.contains(&c.category) {
            tally.confirmed += 1;
            continue;
        }
        if !categories.is_empty() {
            tally.category_differs += 1;
            continue;
        }
        let evidence = Evidence::emitter(emitter.as_str()).rule(c.rule).at(c.file, c.line);
        added.push(Edge::new(c.from, c.to, c.category, Confidence::Strong).with_cell(evidence.to_cell()));
        if c.category == edge_category::CALLS {
            tally.calls += 1;
        } else {
            tally.uses += 1;
        }
    }
    tally.added = added.len();
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

    #[test]
    fn existing_category_stands_and_the_rest_is_added() {
        let dir = std::env::temp_dir().join(format!("glia_scip_stage_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), A_PY).unwrap();
        let symbols = vec![
            ScipSymbolRecord { id: 0, symbol: "scip-python python app 0.1 `a`/helper().".into(), implements: vec![] },
            ScipSymbolRecord { id: 1, symbol: "scip-python python app 0.1 `a`/run().".into(), implements: vec![] },
        ];
        let rref = |s, line, call| ScipRefRow { s, line, call, write: false, import: false };
        let documents = vec![ScipDocumentRecord {
            path: "a.py".into(),
            language: "python".into(),
            source_hash: source_hash(A_PY.as_bytes()),
            defs: vec![ScipDefRow { s: 1, line: 0, name: "run".into() }, ScipDefRow { s: 0, line: 4, name: "helper".into() }],
            // run calls helper (row 1: glia has USES run -> helper, so the
            // CALLS differs), references it (row 2: confirms that USES), and
            // helper calls run (row 5: added).
            refs: vec![rref(0, 1, true), rref(0, 2, false), rref(1, 5, true)],
        }];
        let meta = ScipMeta::new("scip-python".into(), "0.6.0".into(), String::new(), 0);
        write_scip(&dir, meta, &documents, &symbols).unwrap();

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
        assert_eq!(merged.cross_edges.len(), 1);
        let e = &merged.cross_edges[0];
        assert_eq!((e.from, e.to, e.category), (NodeId(3), NodeId(2), edge_category::CALLS));
        assert_eq!(
            Evidence::of(e),
            Some(Evidence::emitter("scip:scip-python").rule(RULE_CALL).at("a.py", 5))
        );
    }
}
