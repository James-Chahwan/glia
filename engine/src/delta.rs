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
//! Fired-on markers, one of each per delta:
//! `[delta] materialized <n> files from <rev> (gitlinks_skipped=.. symlinks=.. skipped_symlinks=.. snapshots=..)`
//! and `[delta] base=<rev> files: reused=R reparsed=P evicted=E | nodes +A -D ~M >V | edges +a -r ~c | regions_excluded=X ignored_moves=I`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    let mut regions: BTreeSet<u64> = BTreeSet::new();
    let mut schema_copies: Vec<Node> = Vec::new();
    let mut replaced: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (gi, g) in m.graphs.iter().enumerate() {
        for (ni, n) in g.nodes.iter().enumerate() {
            if g.nav.kind_by_id.get(&n.id) == Some(&node_kind::REGION) {
                regions.insert(n.id.0);
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
    let regions = &parts.regions;
    let nodes = m
        .graphs
        .iter()
        .enumerate()
        .flat_map(|(gi, g)| g.nodes.iter().enumerate().map(move |(ni, n)| (gi, ni, n)))
        .filter(|&(gi, ni, n)| !regions.contains(&n.id.0) && !parts.replaced.contains(&(gi, ni)))
        .map(|(_, _, n)| n)
        .chain(parts.schema_copies.iter());
    let edges = m.all_edges().filter(|e| !regions.contains(&e.from.0) && !regions.contains(&e.to.0));
    DeltaSide::new(nodes, edges)
}

/// [`located_delta_with`] with no declared renames: move detection pairs
/// files by body hash, then by basename. The graph-pair entry, also usable on
/// a loaded prior `.gmap`.
pub fn located_delta(before: &MergedGraph, after: &MergedGraph) -> (GraphDelta, Vec<DeltaNode>, Vec<DeltaEdge>) {
    let l = locate_delta(before, after, &[]);
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
    let l = locate_delta(before, after, renames);
    (l.delta, l.nodes, l.edges)
}

/// Everything [`graph_delta_vs_rev`] needs from one located delta.
struct LocatedDelta {
    delta: GraphDelta,
    nodes: Vec<DeltaNode>,
    edges: Vec<DeltaEdge>,
    regions_excluded: usize,
}

fn locate_delta(before: &MergedGraph, after: &MergedGraph, renames: &[(String, String)]) -> LocatedDelta {
    let moves = detect_moves_with(before, after, renames);
    let pairs: Vec<(NodeId, NodeId)> = moves.nodes.iter().map(|m| (m.old_id, m.new_id)).collect();
    let (old_parts, new_parts) = (side_parts(before), side_parts(after));
    let (old_side, new_side) = (side(before, &old_parts), side(after, &new_parts));
    let opts = DeltaOptions { content_cells: CONTENT_CELLS, moves: &pairs };
    let delta = graph_delta(&old_side, &new_side, &opts);
    let regions_excluded = old_parts.regions().union(new_parts.regions()).count();

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

    let (old_edges, new_edges) = (first_edges(&before.all_edges().collect::<Vec<_>>()), first_edges(&after.all_edges().collect::<Vec<_>>()));
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
    LocatedDelta { delta, nodes, edges, regions_excluded }
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
    let l = locate_delta(&before.merged, &after.merged, &renames);
    let d = &l.delta;
    let counts = DeltaCounts {
        nodes_added: d.added_nodes.len(),
        nodes_removed: d.removed_nodes.len(),
        nodes_modified: d.modified_nodes.len(),
        nodes_moved: d.moved_nodes.len(),
        edges_added: d.added_edges.len(),
        edges_removed: d.removed_edges.len(),
        edges_reconfidenced: d.reconfidenced_edges.len(),
        regions_excluded: l.regions_excluded,
        moves_ignored: d.ignored_moves,
    };
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
    let answer = GraphDeltaAnswer { base: rev.given, files, counts, nodes: l.nodes, edges: l.edges };
    Ok(RevDelta { answer, before, after, delta: l.delta })
}
