//! Graph delta of a git rev against the working tree (LE.1b): "what did my
//! uncommitted change, or my branch since `<rev>`, do to the graph?"
//!
//! [`graph_delta_vs_rev`] builds both sides and diffs them:
//!
//! 1. the working tree, incrementally, exactly as
//!    [`crate::generate_one_incremental`] does: load the parse-cache sidecar,
//!    build, save it (a save failure is logged, not fatal);
//! 2. the rev, materialised read-only into a temp dir (`git_rev`) and built
//!    under the WORKING TREE's identity with the same in-memory cache: the
//!    NodeIds match and every file the rev shares with the working tree is a
//!    cache hit. The cache is not saved after this build, so the sidecar keeps
//!    the working-tree state and the next incremental build reparses nothing;
//! 3. [`located_delta_with`]: LB.6's move detection (git's renames as the
//!    declared tier) feeds the domain-free `activation::algo::delta` (LE.1a),
//!    and every row is named and located (1-based lines): removed rows and the
//!    before end of a move against the BEFORE graph, all others against the
//!    AFTER graph.
//!
//! REGION nodes (and every edge touching one) are dropped from both sides and
//! counted: they summarise ignored, vendored and nested-repo directories as
//! they sit on disk, which a materialised rev cannot reproduce (an untracked,
//! gitignored `node_modules/` exists in the working tree only).
//!
//! A node's content is its [`CONTENT_CELLS`]: its CODE text and its declared
//! SCHEMA_FIELDS (CC.8a), so a contract op or message type whose fields
//! changed while its CODE line did not is a `modified` row. SCHEMA_FIELDS is
//! canonical JSON (LE.10a / LE.10b); the domain-free LE.1a delta compares Text
//! payloads only (it skips the positional JSON the queue and cron extractors
//! keep under CODE), so [`side`] hands it each SCHEMA_FIELDS payload as Text.
//!
//! A watch update (DC.7b) that rebuilt a few graphs takes the scoped variant,
//! [`located_delta_scoped`] (DC.6): the same rows and counts, folding only the
//! ids of the rebuilt graphs (every copy) and the keys of their intra edges
//! plus every cross edge, with move detection only when the walked file set
//! changed.
//!
//! Fired-on markers, one of each per delta:
//! `[delta] materialized <n> files from <rev> (gitlinks_skipped=.. symlinks=.. skipped_symlinks=.. snapshots=..)`
//! and `[delta] base=<rev> files: reused=R reparsed=P evicted=E | nodes +A -D ~M >V | edges +a -r ~c | regions_excluded=X ignored_moves=I`;
//! per scoped delta `[delta] scoped graphs before=<b> after=<a> ids=<i> keys=<k> moves=<on|off>`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use glia_activation::algo::delta::{DeltaOptions, DeltaSide, EdgeKey, GraphDelta, graph_delta};
use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, CellTypeId, Confidence, Edge, Node, NodeId};
use glia_graph::MergedGraph;
use glia_graph::identity::detect_moves_with;

use crate::answers::{Located, Locator};
use crate::build::{GenerateResult, generate_one_as};
use crate::cache::ParseCache;
use crate::git_rev;

/// One node row: `change` is `added`, `removed`, `modified` or `moved`.
/// `side` names the graph the row was located in: `before` for a removed
/// node, `after` for every other. A moved node carries its before id and
/// qname; a modified node that also moved carries them too.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct DeltaNode {
    pub change: &'static str,
    pub id: u64,
    pub before_id: Option<u64>,
    pub qname: String,
    pub before_qname: Option<String>,
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    pub side: &'static str,
}

/// One edge row: `change` is `added`, `removed` or `reconfidenced`.
/// `confidence` is the edge's confidence on its row's side (before for a
/// removed edge); `was_confidence` is the before confidence of a
/// reconfidenced one. The evidence (LC.3) is read from the edge on that side:
/// `site_file` / `site_line` (1-based) and `basis`, which says whether the
/// line is the asserting construct (`site`) or an endpoint's declaration
/// (`from_node` / `to_node`), plus the `emitter`.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct DeltaEdge {
    pub change: &'static str,
    pub from_qname: String,
    pub to_qname: String,
    pub category: &'static str,
    pub confidence: &'static str,
    pub was_confidence: Option<&'static str>,
    pub site_file: Option<String>,
    pub site_line: Option<i64>,
    pub basis: Option<&'static str>,
    pub emitter: Option<String>,
}

/// The file-level work of the rev build against the working tree's parse
/// cache (LA.12's `CacheDiff` of that build): files served from the cache,
/// files parsed because the rev holds another version (or the working tree
/// has none), and working-tree files the rev does not hold.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone, Default)]
pub struct DeltaFiles {
    pub reused: usize,
    pub reparsed: usize,
    pub evicted: usize,
}

/// Row counts by change, plus the REGION nodes left out of both sides and
/// the move pairs LE.1a did not apply.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone, Default)]
pub struct DeltaCounts {
    pub nodes_added: usize,
    pub nodes_removed: usize,
    pub nodes_modified: usize,
    pub nodes_moved: usize,
    pub edges_added: usize,
    pub edges_removed: usize,
    pub edges_reconfidenced: usize,
    pub regions_excluded: usize,
    pub moves_ignored: usize,
}

/// The located delta of `base` against the working tree. Node rows are
/// sorted by `(change, qname, id)`, edge rows by
/// `(change, category, from_qname, to_qname)`.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct GraphDeltaAnswer {
    pub base: String,
    pub files: DeltaFiles,
    pub counts: DeltaCounts,
    pub nodes: Vec<DeltaNode>,
    pub edges: Vec<DeltaEdge>,
}

/// [`graph_delta_vs_rev`]'s result: the answer plus both builds and the raw
/// id-level delta, for a caller that goes on to query either side (LE.2's
/// diff impact walks the after graph from the changed nodes).
#[non_exhaustive]
pub struct RevDelta {
    pub answer: GraphDeltaAnswer,
    pub before: GenerateResult,
    pub after: GenerateResult,
    pub delta: GraphDelta,
}

/// The cell kinds whose payloads are a node's content in every graph delta:
/// its CODE text and its declared SCHEMA_FIELDS. One list, so every caller of
/// [`side`] and `graph_delta` compares the same thing.
pub(crate) const CONTENT_CELLS: &[CellTypeId] = &[cell_type::CODE, cell_type::SCHEMA_FIELDS];

/// What [`side`] borrows beyond the graph: the REGION ids it drops, and a
/// Text copy of every node that carries a JSON SCHEMA_FIELDS payload.
pub(crate) struct SideParts {
    regions: BTreeSet<u64>,
    /// Nodes holding only their [`CONTENT_CELLS`] cells, each SCHEMA_FIELDS
    /// payload as Text, in graph order.
    schema_copies: Vec<Node>,
    /// `(graph index, node index)` of every node in `schema_copies`.
    replaced: BTreeSet<(usize, usize)>,
}

impl SideParts {
    /// The REGION ids dropped from this side.
    pub(crate) fn regions(&self) -> &BTreeSet<u64> {
        &self.regions
    }
}

/// Collect [`SideParts`] for `m`: the REGION ids, and the Text copies of the
/// nodes whose SCHEMA_FIELDS payload is JSON (every writer's shape, LE.10a /
/// LE.10b). The copies keep CODE exactly as it was, JSON included, so the
/// delta still skips a positional CODE payload.
pub(crate) fn side_parts(m: &MergedGraph) -> SideParts {
    side_parts_in(m, None)
}

/// [`side_parts`] with the Text copies made only for the nodes whose id is in
/// `ids` (every node when `None`). The REGION ids still come from every graph,
/// so a scoped delta reports the full `regions_excluded`.
fn side_parts_in(m: &MergedGraph, ids: Option<&HashSet<u64>>) -> SideParts {
    let mut regions: BTreeSet<u64> = BTreeSet::new();
    let mut schema_copies: Vec<Node> = Vec::new();
    let mut replaced: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (gi, g) in m.graphs.iter().enumerate() {
        for (ni, n) in g.nodes.iter().enumerate() {
            if g.nav.kind_by_id.get(&n.id) == Some(&node_kind::REGION) {
                regions.insert(n.id.0);
                continue;
            }
            if ids.is_some_and(|s| !s.contains(&n.id.0)) {
                continue;
            }
            let json_schema = n
                .cells
                .iter()
                .any(|c| c.kind == cell_type::SCHEMA_FIELDS && matches!(c.payload, CellPayload::Json(_)));
            if !json_schema {
                continue;
            }
            let cells = n
                .cells
                .iter()
                .filter(|c| CONTENT_CELLS.contains(&c.kind))
                .map(|c| match &c.payload {
                    CellPayload::Json(j) if c.kind == cell_type::SCHEMA_FIELDS => {
                        Cell { kind: c.kind, payload: CellPayload::Text(j.clone()) }
                    }
                    _ => c.clone(),
                })
                .collect();
            schema_copies.push(Node { id: n.id, repo: n.repo, confidence: n.confidence, cells });
            replaced.insert((gi, ni));
        }
    }
    SideParts { regions, schema_copies, replaced }
}

/// One graph as a delta side: every graph's nodes in order and
/// `all_edges()`, minus REGION nodes and every edge with a REGION endpoint,
/// with each node that carries a JSON SCHEMA_FIELDS payload stood in for by
/// its Text copy in `parts` (module docs).
pub(crate) fn side<'a>(m: &'a MergedGraph, parts: &'a SideParts) -> DeltaSide<'a> {
    DeltaSide::new(side_nodes(m, parts, |_| true), side_edges(m, parts.regions(), |_| true))
}

/// Every node of every graph in graph order whose id passes `keep`, minus
/// REGION nodes, with each JSON SCHEMA_FIELDS node stood in for by its Text
/// copy in `parts` (whose copies the caller made for the same `keep`).
fn side_nodes<'a>(
    m: &'a MergedGraph,
    parts: &'a SideParts,
    keep: impl Fn(u64) -> bool + 'a,
) -> impl Iterator<Item = &'a Node> + 'a {
    let regions = &parts.regions;
    m.graphs
        .iter()
        .enumerate()
        .flat_map(|(gi, g)| g.nodes.iter().enumerate().map(move |(ni, n)| (gi, ni, n)))
        .filter(move |&(gi, ni, n)| keep(n.id.0) && !regions.contains(&n.id.0) && !parts.replaced.contains(&(gi, ni)))
        .map(|(_, _, n)| n)
        .chain(parts.schema_copies.iter())
}

/// Every edge of `all_edges()` (in that order) that passes `keep` and has no
/// endpoint in `regions`.
fn side_edges<'a>(m: &'a MergedGraph, regions: &BTreeSet<u64>, keep: impl Fn(&Edge) -> bool) -> Vec<&'a Edge> {
    m.all_edges().filter(|e| !regions.contains(&e.from.0) && !regions.contains(&e.to.0) && keep(e)).collect()
}

/// [`located_delta_with`] with no declared renames: move detection pairs
/// files by body hash, then by basename. The graph-pair entry, also usable on
/// a loaded prior `.gmap`.
pub fn located_delta(before: &MergedGraph, after: &MergedGraph) -> (GraphDelta, Vec<DeltaNode>, Vec<DeltaEdge>) {
    let l = located_delta_counted(before, after, &[]);
    (l.delta, l.nodes, l.edges)
}

/// The delta `after - before` with every row named and located.
/// `renames` are repo-relative `(old_path, new_path)` pairs the caller's VCS
/// declared (`glia_graph::identity::detect_moves_with`'s FACT tier);
/// LB.6's documented limits (a file moved AND renamed AND edited without a
/// declared rename, a symbol renamed inside a moved file) come out as removed
/// + added.
pub fn located_delta_with(
    before: &MergedGraph,
    after: &MergedGraph,
    renames: &[(String, String)],
) -> (GraphDelta, Vec<DeltaNode>, Vec<DeltaEdge>) {
    let l = located_delta_counted(before, after, renames);
    (l.delta, l.nodes, l.edges)
}

/// The graphs of each side a [`located_delta_scoped`] reads (DC.6): indices
/// into `before.graphs` / `after.graphs` of every graph that was rebuilt,
/// vanished (before only) or added (after only). A watch update passes the
/// language groups it rebuilt plus the regions, projects and docs graphs,
/// which it rebuilds every time. `moves` runs move detection; the session
/// sets it whenever the walked file set changed (any add or remove).
pub(crate) struct DeltaScope {
    pub(crate) before: BTreeSet<usize>,
    pub(crate) after: BTreeSet<usize>,
    pub(crate) moves: bool,
}

/// One located delta: the id-level delta, its named and located rows (node
/// rows by `(change, qname, id)`, edge rows by
/// `(change, category, from_qname, to_qname)`) and their counts.
pub(crate) struct LocatedRows {
    pub(crate) delta: GraphDelta,
    pub(crate) nodes: Vec<DeltaNode>,
    pub(crate) edges: Vec<DeltaEdge>,
    pub(crate) counts: DeltaCounts,
}

/// The full delta `after - before` ([`located_delta_with`]) with its
/// [`DeltaCounts`]: every node and edge of both sides, move detection with
/// `renames` as the declared tier.
pub(crate) fn located_delta_counted(
    before: &MergedGraph,
    after: &MergedGraph,
    renames: &[(String, String)],
) -> LocatedRows {
    let pairs = move_pairs(before, after, renames);
    let (old_parts, new_parts) = (side_parts(before), side_parts(after));
    let old_edges = side_edges(before, old_parts.regions(), |_| true);
    let new_edges = side_edges(after, new_parts.regions(), |_| true);
    let old_side = DeltaSide::new(side_nodes(before, &old_parts, |_| true), old_edges.iter().copied());
    let new_side = DeltaSide::new(side_nodes(after, &new_parts, |_| true), new_edges.iter().copied());
    let delta = graph_delta(&old_side, &new_side, &DeltaOptions { content_cells: CONTENT_CELLS, moves: &pairs });
    let regions_excluded = old_parts.regions().union(new_parts.regions()).count();
    locate_rows(before, after, delta, &old_edges, &new_edges, regions_excluded)
}

/// [`located_delta_counted`] restricted to what `scope` can change (DC.6):
/// the same delta, rows and counts, without folding the clean graphs.
///
/// - Ids: D = the id of every node of the scoped graphs of both sides. A side
///   holds every copy, in every graph, of an id in D (the delta folds a node's
///   content over all its copies), minus REGION nodes; JSON SCHEMA_FIELDS
///   become Text as in [`side`].
/// - Edges: K = the key of every intra edge of the scoped graphs of both
///   sides and of every cross edge of both sides (the resolvers and the
///   post-passes write their edges there, so a cross edge between two clean
///   graphs can still change). A side holds every edge of `all_edges()` whose
///   key is in K or has an endpoint in D: the test is a function of the key,
///   so every copy of a kept key is kept (the delta keeps a key's strongest
///   confidence), and every edge a move can remap, or that names a move
///   target, is there. No REGION endpoint.
/// - Moves: `scope.moves` runs `detect_moves_with(before, after, &[])` over
///   both whole graphs; otherwise there are none. A move pairs a MODULE (and
///   its declarations) whose qname vanished with one that appeared, so both
///   ends sit in rebuilt graphs. A same-stem sibling ADD renames an existing
///   file's MODULE (LB.9b / LB.13), which is why an add alone turns it on; with
///   the walked file set unchanged no MODULE qname changes, so no pair exists.
/// - REGION ids are taken from every graph, so `regions_excluded` is the full
///   count; the Locator indexes both whole graphs.
///
/// Exact when no graph outside the scope changed between the sides: its
/// nodes' CONTENT_CELLS and its intra edges with their confidences are equal
/// before and after. A clean group's pre-pass graph is a clone of its last
/// build, and no post-pass writes CODE or SCHEMA_FIELDS, adds an intra edge
/// or reconfidences one (they write node confidence, TEST / ORIGIN / DECISION
/// cells and edge evidence, none of which the delta compares), so this holds
/// on a source-only (LOW) watch update. An update that can change a clean
/// graph (an overlay edit; a SCIP snapshot, which reconfidences intra edges)
/// takes [`located_delta_counted`].
///
/// Prints `[delta] scoped graphs before=<b> after=<a> ids=<i> keys=<k> moves=<on|off>`:
/// the scoped graph count of each side, the non-REGION ids of D, and the
/// distinct edge keys both sides hold.
pub(crate) fn located_delta_scoped(before: &MergedGraph, after: &MergedGraph, scope: &DeltaScope) -> LocatedRows {
    let scoped = |m: &'_ MergedGraph, at: &BTreeSet<usize>| -> Vec<usize> {
        at.iter().copied().filter(|&i| i < m.graphs.len()).collect()
    };
    let (in_before, in_after) = (scoped(before, &scope.before), scoped(after, &scope.after));
    let mut ids: HashSet<u64> = HashSet::new();
    let mut keys: HashSet<EdgeKey> = HashSet::new();
    let graphs = in_before.iter().map(|&i| &before.graphs[i]).chain(in_after.iter().map(|&i| &after.graphs[i]));
    for g in graphs {
        ids.extend(g.nodes.iter().map(|n| n.id.0));
        keys.extend(g.edges.iter().map(EdgeKey::from));
    }
    keys.extend(before.cross_edges.iter().chain(&after.cross_edges).map(EdgeKey::from));
    let in_scope = |e: &Edge| keys.contains(&EdgeKey::from(e)) || ids.contains(&e.from.0) || ids.contains(&e.to.0);

    let pairs = if scope.moves { move_pairs(before, after, &[]) } else { Vec::new() };
    let (old_parts, new_parts) = (side_parts_in(before, Some(&ids)), side_parts_in(after, Some(&ids)));
    let old_edges = side_edges(before, old_parts.regions(), in_scope);
    let new_edges = side_edges(after, new_parts.regions(), in_scope);
    let id_in = |id: u64| ids.contains(&id);
    let old_side = DeltaSide::new(side_nodes(before, &old_parts, id_in), old_edges.iter().copied());
    let new_side = DeltaSide::new(side_nodes(after, &new_parts, id_in), new_edges.iter().copied());
    let delta = graph_delta(&old_side, &new_side, &DeltaOptions { content_cells: CONTENT_CELLS, moves: &pairs });
    let regions: BTreeSet<u64> = old_parts.regions().union(new_parts.regions()).copied().collect();

    let held: HashSet<EdgeKey> = old_edges.iter().chain(&new_edges).map(|&e| EdgeKey::from(e)).collect();
    eprintln!(
        "[delta] scoped graphs before={} after={} ids={} keys={} moves={}",
        in_before.len(),
        in_after.len(),
        ids.iter().filter(|id| !regions.contains(id)).count(),
        held.len(),
        if scope.moves { "on" } else { "off" },
    );
    locate_rows(before, after, delta, &old_edges, &new_edges, regions.len())
}

/// `(before, after)` id pairs of every move `detect_moves_with` finds.
fn move_pairs(before: &MergedGraph, after: &MergedGraph, renames: &[(String, String)]) -> Vec<(NodeId, NodeId)> {
    detect_moves_with(before, after, renames).nodes.iter().map(|m| (m.old_id, m.new_id)).collect()
}

/// Name, locate, sort and count `delta`'s rows. `old_edges` / `new_edges` are
/// the edges each side handed the delta, in `all_edges()` order: a row's
/// evidence is read from the first edge with its key there, which is the
/// first in `all_edges()` because a side keeps every copy of a key it holds.
fn locate_rows(
    before: &MergedGraph,
    after: &MergedGraph,
    delta: GraphDelta,
    old_edges: &[&Edge],
    new_edges: &[&Edge],
    regions_excluded: usize,
) -> LocatedRows {
    let (then, now) = (Locator::new(before), Locator::new(after));
    // After id -> before id, for the moves LE.1a applied.
    let was: HashMap<NodeId, NodeId> = delta.moved_nodes.iter().map(|&(b, a)| (a, b)).collect();
    let mut nodes = Vec::with_capacity(
        delta.added_nodes.len() + delta.removed_nodes.len() + delta.modified_nodes.len() + delta.moved_nodes.len(),
    );
    for &id in &delta.added_nodes {
        nodes.push(node_row("added", now.locate(id), None, "after"));
    }
    for &id in &delta.removed_nodes {
        nodes.push(node_row("removed", then.locate(id), None, "before"));
    }
    for &id in &delta.modified_nodes {
        let prior = was.get(&id).map(|&b| then.locate(b));
        nodes.push(node_row("modified", now.locate(id), prior, "after"));
    }
    for &(b, a) in &delta.moved_nodes {
        nodes.push(node_row("moved", now.locate(a), Some(then.locate(b)), "after"));
    }
    nodes.sort_by(|x, y| (x.change, &x.qname, x.id).cmp(&(y.change, &y.qname, y.id)));

    let (old_edges, new_edges) = (first_edges(old_edges), first_edges(new_edges));
    let mut edges = Vec::with_capacity(
        delta.added_edges.len() + delta.removed_edges.len() + delta.reconfidenced_edges.len(),
    );
    for key in &delta.added_edges {
        edges.push(edge_row("added", key, &now, new_edges.get(key).copied(), None));
    }
    for key in &delta.removed_edges {
        edges.push(edge_row("removed", key, &then, old_edges.get(key).copied(), None));
    }
    for (key, was_conf, _) in &delta.reconfidenced_edges {
        edges.push(edge_row("reconfidenced", key, &now, new_edges.get(key).copied(), Some(*was_conf)));
    }
    edges.sort_by(|x, y| {
        (x.change, x.category, &x.from_qname, &x.to_qname).cmp(&(y.change, y.category, &y.from_qname, &y.to_qname))
    });
    let counts = DeltaCounts {
        nodes_added: delta.added_nodes.len(),
        nodes_removed: delta.removed_nodes.len(),
        nodes_modified: delta.modified_nodes.len(),
        nodes_moved: delta.moved_nodes.len(),
        edges_added: delta.added_edges.len(),
        edges_removed: delta.removed_edges.len(),
        edges_reconfidenced: delta.reconfidenced_edges.len(),
        regions_excluded,
        moves_ignored: delta.ignored_moves,
    };
    LocatedRows { delta, nodes, edges, counts }
}

/// Key -> the first edge (in `all_edges` order) with that key.
fn first_edges<'a>(edges: &[&'a Edge]) -> BTreeMap<EdgeKey, &'a Edge> {
    let mut out: BTreeMap<EdgeKey, &'a Edge> = BTreeMap::new();
    for &e in edges {
        out.entry(EdgeKey::from(e)).or_insert(e);
    }
    out
}

fn node_row(change: &'static str, at: Located, prior: Option<Located>, side: &'static str) -> DeltaNode {
    DeltaNode {
        change,
        id: at.id,
        before_id: prior.as_ref().map(|p| p.id),
        qname: at.qname,
        before_qname: prior.map(|p| p.qname),
        kind: at.kind,
        file: at.file,
        line: at.line,
        side,
    }
}

fn edge_row(
    change: &'static str,
    key: &EdgeKey,
    loc: &Locator<'_>,
    edge: Option<&Edge>,
    was: Option<Confidence>,
) -> DeltaEdge {
    let ev = edge.and_then(Evidence::of);
    let confidence = edge.map(|e| e.confidence).unwrap_or(Confidence::Weak);
    DeltaEdge {
        change,
        from_qname: loc.locate(key.from).qname,
        to_qname: loc.locate(key.to).qname,
        category: edge_category::name(key.category),
        confidence: confidence_str(confidence),
        was_confidence: was.map(confidence_str),
        site_file: ev.as_ref().and_then(|e| e.file.clone()),
        // Evidence lines are 0-based rows; rows here are 1-based (LD.1).
        site_line: ev.as_ref().and_then(|e| e.line).map(|l| i64::from(l) + 1),
        basis: ev.as_ref().map(|e| basis_str(e.basis)),
        emitter: ev.map(|e| e.emitter),
    }
}

fn confidence_str(c: Confidence) -> &'static str {
    match c {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    }
}

fn basis_str(b: Basis) -> &'static str {
    match b {
        Basis::Site => "site",
        Basis::FromNode => "from_node",
        Basis::ToNode => "to_node",
        Basis::File => "file",
        Basis::None => "none",
    }
}

/// The graph delta of git rev `base` against the working tree at `repo_path`
/// (module docs). `Err` when `repo_path` is not a directory, git is missing,
/// `repo_path` is not in a git work tree, `base` names no commit, or either
/// build fails; the rev is checked before anything is built or saved. The
/// temp checkout is removed before this returns, on every path.
pub fn graph_delta_vs_rev(repo_path: &str, base: &str) -> Result<RevDelta, String> {
    let repo = Path::new(repo_path);
    if !repo.is_dir() {
        return Err(format!("not a directory: {repo_path}"));
    }
    let rev = git_rev::resolve_rev(repo, base)?;
    let mut cache = ParseCache::load(repo_path);
    let after = generate_one_as(repo_path, repo_path, Some(&mut cache))?;
    if let Err(e) = cache.save(repo_path) {
        eprintln!("[incremental] {repo_path}: warning: failed to save parse cache: {e}");
    }
    let tree = git_rev::materialize_rev(repo, &rev)?;
    let tmp = tree
        .dir
        .path()
        .to_str()
        .ok_or_else(|| format!("temp dir is not valid UTF-8: {}", tree.dir.path().display()))?;
    let before = generate_one_as(tmp, repo_path, Some(&mut cache))?;
    drop(tree);
    let files = cache
        .last_diff()
        .map(|d| DeltaFiles { reused: d.reused.len(), reparsed: d.reparsed.len(), evicted: d.evicted.len() })
        .unwrap_or_default();
    let renames = git_rev::declared_renames(repo, &rev);
    let LocatedRows { delta, nodes, edges, counts } = located_delta_counted(&before.merged, &after.merged, &renames);
    eprintln!(
        "[delta] base={} files: reused={} reparsed={} evicted={} | nodes +{} -{} ~{} >{} | edges +{} -{} ~{} | regions_excluded={} ignored_moves={}",
        rev.given,
        files.reused,
        files.reparsed,
        files.evicted,
        counts.nodes_added,
        counts.nodes_removed,
        counts.nodes_modified,
        counts.nodes_moved,
        counts.edges_added,
        counts.edges_removed,
        counts.edges_reconfidenced,
        counts.regions_excluded,
        counts.moves_ignored,
    );
    let answer = GraphDeltaAnswer { base: rev.given, files, counts, nodes, edges };
    Ok(RevDelta { answer, before, after, delta })
}

#[cfg(test)]
mod tests {
    //! DC.6: the scoped delta equals the full delta when the scope holds every
    //! rebuilt graph (plus the regions / projects / docs graphs a watch update
    //! always rebuilds). Each test builds one directory twice, editing between
    //! the builds, so both sides share one RepoId and unmoved nodes keep ids.

    use std::path::{Path, PathBuf};

    use super::*;

    const USERS_PY: &str = "import requests\n\n\ndef list_users():\n    return requests.get(\"/users\")\n\n\ndef count_users():\n    return len(list_users())\n";
    const CLIENT_TS: &str = "export function loadUsers() {\n  return fetch(\"/users\");\n}\n";
    const GO_MAIN: &str = "package main\n\nimport \"net/http\"\n\nfunc items(w http.ResponseWriter, r *http.Request) {}\n\nfunc main() {\n\thttp.HandleFunc(\"/items\", items)\n\tping()\n}\n\nfunc ping() {}\n";
    const ITEMS_TS: &str = "export function loadItems() {\n  return fetch(\"/items\");\n}\n";
    const FLASK_ITEMS: &str = "\n\nfrom flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/items\")\ndef items():\n    return []\n";
    const README: &str = "# demo\n\nThe `list_users` call reads `/users`.\n";

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    fn build(root: &Path) -> MergedGraph {
        crate::generate_one(root.to_str().expect("utf-8 tempdir")).expect("build").merged
    }

    /// `<tmp>/proj`: python and TS calling one `/users` endpoint, a TS client
    /// of the Go `/items` route (a Go graph outside every scope), a README
    /// naming the python function, a root `package.json` PROJECT and a
    /// `node_modules/` REGION.
    fn repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("proj");
        write(&root, "api/users.py", USERS_PY);
        write(&root, "web/client.ts", CLIENT_TS);
        write(&root, "web/items.ts", ITEMS_TS);
        write(&root, "README.md", README);
        write(&root, "package.json", "{\"name\": \"demo\"}\n");
        write(&root, "svc/main.go", GO_MAIN);
        write(&root, "node_modules/left-pad/index.js", "module.exports = 1;\n");
        (tmp, root)
    }

    /// The graphs holding a MODULE located in a file ending in `ext`.
    fn lang_graphs(m: &MergedGraph, ext: &str) -> BTreeSet<usize> {
        let loc = Locator::new(m);
        let holds = |g: &glia_graph::RepoGraph| {
            g.nodes.iter().any(|n| {
                g.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE)
                    && loc.locate(n.id).file.is_some_and(|f| f.ends_with(ext))
            })
        };
        m.graphs.iter().enumerate().filter(|(_, g)| holds(g)).map(|(i, _)| i).collect()
    }

    /// The regions, projects and docs graphs: a watch update rebuilds them
    /// every time.
    fn always_graphs(m: &MergedGraph) -> BTreeSet<usize> {
        let kinds = [node_kind::REGION, node_kind::PROJECT, node_kind::DOC_SECTION, node_kind::DOC_SPACE];
        let holds = |g: &glia_graph::RepoGraph| {
            g.nodes.iter().any(|n| g.nav.kind_by_id.get(&n.id).is_some_and(|k| kinds.contains(k)))
        };
        m.graphs.iter().enumerate().filter(|(_, g)| holds(g)).map(|(i, _)| i).collect()
    }

    /// The scope a watch update passes when the groups of `exts` were rebuilt.
    /// Asserts it leaves at least one graph out on each side, or the test
    /// would compare the full delta with itself.
    fn scope(before: &MergedGraph, after: &MergedGraph, exts: &[&str], moves: bool) -> DeltaScope {
        let pick = |m: &MergedGraph| {
            let mut s = always_graphs(m);
            assert_eq!(s.len(), 3, "regions, projects and docs graphs");
            for ext in exts {
                let langs = lang_graphs(m, ext);
                assert!(!langs.is_empty(), "no graph holds a {ext} module");
                s.extend(langs);
            }
            assert!(s.len() < m.graphs.len(), "scope {s:?} covers all {} graphs", m.graphs.len());
            s
        };
        DeltaScope { before: pick(before), after: pick(after), moves }
    }

    fn json<T: serde::Serialize>(v: &T) -> serde_json::Value {
        serde_json::to_value(v).expect("serialise")
    }

    /// Scoped == full on the raw delta, every row and every count; returns
    /// the full rows for the caller's own checks.
    fn assert_scoped_equals_full(before: &MergedGraph, after: &MergedGraph, scope: &DeltaScope) -> LocatedRows {
        let full = located_delta_counted(before, after, &[]);
        let scoped = located_delta_scoped(before, after, scope);
        assert_eq!(scoped.delta, full.delta);
        assert_eq!(json(&scoped.nodes), json(&full.nodes));
        assert_eq!(json(&scoped.edges), json(&full.edges));
        assert_eq!(json(&scoped.counts), json(&full.counts));
        assert!(full.counts.regions_excluded > 0, "the node_modules REGION is counted");
        full
    }

    /// The ENDPOINT ids that sit in more than one graph of `m`.
    fn shared_endpoints(m: &MergedGraph) -> BTreeSet<u64> {
        let mut graphs_of: BTreeMap<u64, BTreeSet<usize>> = BTreeMap::new();
        for (gi, g) in m.graphs.iter().enumerate() {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT) {
                    graphs_of.entry(n.id.0).or_default().insert(gi);
                }
            }
        }
        graphs_of.into_iter().filter(|(_, gs)| gs.len() > 1).map(|(id, _)| id).collect()
    }

    #[test]
    fn scoped_equals_full_on_a_body_edit() {
        let (_tmp, root) = repo();
        let before = build(&root);
        assert!(!shared_endpoints(&before).is_empty(), "the /users ENDPOINT sits in the python and TS graphs");
        write(&root, "api/users.py", &USERS_PY.replace("len(list_users())", "len(list_users()) + 0"));
        let after = build(&root);
        let full = assert_scoped_equals_full(&before, &after, &scope(&before, &after, &[".py"], false));
        assert!(full.counts.nodes_modified > 0, "the edited function is modified");
    }

    #[test]
    fn scoped_equals_full_on_add_and_remove() {
        let (_tmp, root) = repo();
        let before = build(&root);
        std::fs::rename(root.join("api/users.py"), root.join("api/people.py")).expect("rename");
        let after = build(&root);
        let full = assert_scoped_equals_full(&before, &after, &scope(&before, &after, &[".py"], true));
        assert!(full.counts.nodes_moved > 0, "the renamed file's nodes are moved");
    }

    #[test]
    fn scoped_equals_full_on_same_stem_add() {
        let (_tmp, root) = repo();
        let before = build(&root);
        write(&root, "api/users.ts", "export function countUsers() {\n  return 0;\n}\n");
        let after = build(&root);
        let full = assert_scoped_equals_full(&before, &after, &scope(&before, &after, &[".py", ".ts"], true));
        assert!(full.counts.nodes_moved > 0, "the python MODULE is renamed by the same-stem add and paired");
        assert!(full.counts.nodes_added > 0, "the TS module is added");
    }

    /// The TS edit takes the TS copy of the shared ENDPOINT away; the python
    /// copy stays. The ENDPOINT id is in D (the TS graph is scoped), and the
    /// scoped side must still hold its python copy, or it would be reported
    /// removed.
    #[test]
    fn scoped_equals_full_when_a_shared_id_changes() {
        let (_tmp, root) = repo();
        let before = build(&root);
        let shared = shared_endpoints(&before);
        assert_eq!(shared.len(), 1, "the /users ENDPOINT sits in the python and TS graphs");
        write(&root, "web/client.ts", &CLIENT_TS.replace("fetch(\"/users\")", "fetch(\"/people\")"));
        let after = build(&root);
        let users = NodeId(*shared.iter().next().expect("one shared id"));
        let holders = |m: &MergedGraph| -> Vec<usize> {
            m.graphs.iter().enumerate().filter(|(_, g)| g.nodes.iter().any(|n| n.id == users)).map(|(i, _)| i).collect()
        };
        let (ts_before, ts_after) = (lang_graphs(&before, ".ts"), lang_graphs(&after, ".ts"));
        assert!(holders(&before).iter().any(|i| ts_before.contains(i)), "the TS graph held a copy");
        assert!(!holders(&after).iter().any(|i| ts_after.contains(i)), "the edit took the TS copy away");
        assert_eq!(holders(&after).len(), 1, "the python copy stays");
        let full = assert_scoped_equals_full(&before, &after, &scope(&before, &after, &[".ts"], false));
        assert!(!full.delta.removed_nodes.contains(&users), "the ENDPOINT is still in the after graph");
        assert!(full.counts.nodes_added > 0, "the /people ENDPOINT is added");
        assert!(full.counts.nodes_modified > 0, "the edited TS function is modified");
    }

    /// The rebuilt python graph adds a `GET /items` route, and the HTTP
    /// resolver moves the TS client's HTTP_CALLS edge off the Go `ANY /items`
    /// route onto it. The removed cross edge has both ends in clean graphs:
    /// only K's "every cross edge" keeps it on the scoped sides.
    #[test]
    fn scoped_equals_full_when_a_clean_cross_edge_changes() {
        let (_tmp, root) = repo();
        let before = build(&root);
        write(&root, "api/users.py", &format!("{USERS_PY}{FLASK_ITEMS}"));
        let after = build(&root);
        let scope = scope(&before, &after, &[".py"], false);
        let full = assert_scoped_equals_full(&before, &after, &scope);
        let held = |m: &MergedGraph, at: &BTreeSet<usize>| -> Vec<NodeId> {
            at.iter().flat_map(|&i| m.graphs[i].nodes.iter().map(|n| n.id)).collect()
        };
        let d: HashSet<NodeId> = held(&before, &scope.before).into_iter().chain(held(&after, &scope.after)).collect();
        let clean: Vec<&EdgeKey> = full
            .delta
            .removed_edges
            .iter()
            .filter(|k| k.category == edge_category::HTTP_CALLS && !d.contains(&k.from) && !d.contains(&k.to))
            .collect();
        assert_eq!(clean.len(), 1, "the TS -> Go HTTP_CALLS edge, both ends clean, is removed");
        assert_eq!(full.counts.edges_added, 3, "DEFINES, HANDLED_BY and the TS -> python HTTP_CALLS");
    }
}
