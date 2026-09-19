//! Graph delta (LE.1a): what a change did to the GRAPH between two snapshots:
//! nodes added / removed / modified / moved, and edges added / removed /
//! reconfidenced.
//!
//! Domain-free like the rest of [`crate::algo`]: a side is a caller-built list
//! of core [`Node`]s and [`Edge`]s ([`DeltaSide`]), and the cell kinds that
//! count as content, plus the move map, arrive in [`DeltaOptions`]. The engine
//! builds both sides from its graphs and names the rows (LE.1b).
//!
//! Each rule below exists because graphs repeat themselves:
//! - One NodeId can sit in several per-language graphs of one repo, so a node's
//!   content is folded across its copies into the SET of its Text payloads
//!   under a content kind. Neither copy order nor copy count can mark it
//!   modified.
//! - Only `CellPayload::Text` is content. A Json or Bytes payload under a
//!   content kind is positional or structured metadata (the queue and cron
//!   extractors put `sites: [{file, line}]` JSON under CODE). Comparing it
//!   would mark every such node below an edited line as modified.
//! - Edges repeat (a node folded from two graphs is walked twice), so they are
//!   compared as a set keyed by `(from, to, category)` that keeps the strongest
//!   confidence seen. Edge cells are not compared: an edit above a call site
//!   shifts the evidence line of every edge below it.
//! - Before-side ids are remapped through the move map first, so a moved
//!   file's nodes and edges are reported as moved, not as removed + added.
//!
//! Every ordered map is keyed on the raw integers (the id newtypes are Hash,
//! not Ord), and every output list comes out in that order, so the delta is a
//! pure function of its inputs. Payloads are borrowed, never cloned.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use repo_graph_core::{CellPayload, CellTypeId, Confidence, Edge, EdgeCategoryId, Node, NodeId};

/// One snapshot of a graph: its nodes (a repeated id is one node, see the
/// module docs) and its edges, borrowed.
#[derive(Clone, Debug, Default)]
pub struct DeltaSide<'a> {
    nodes: Vec<&'a Node>,
    edges: Vec<&'a Edge>,
}

impl<'a> DeltaSide<'a> {
    pub fn new(nodes: impl IntoIterator<Item = &'a Node>, edges: impl IntoIterator<Item = &'a Edge>) -> Self {
        Self { nodes: nodes.into_iter().collect(), edges: edges.into_iter().collect() }
    }
}

/// An edge's identity across snapshots: `(from, to, category)`, without its
/// confidence or cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EdgeKey {
    pub from: NodeId,
    pub to: NodeId,
    pub category: EdgeCategoryId,
}

impl From<&Edge> for EdgeKey {
    fn from(e: &Edge) -> Self {
        Self { from: e.from, to: e.to, category: e.category }
    }
}

/// By raw `(from, to, category)`, because the id newtypes have no `Ord`.
impl Ord for EdgeKey {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.from.0, self.to.0, self.category.0).cmp(&(o.from.0, o.to.0, o.category.0))
    }
}

impl PartialOrd for EdgeKey {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// What counts as a change.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeltaOptions<'a> {
    /// Cell kinds whose Text payloads are a node's content. A node whose set of
    /// such payloads differs between the sides is modified. When this list is
    /// empty, no node is modified.
    pub content_cells: &'a [CellTypeId],
    /// `(before, after)` ids of nodes that moved. A pair applies when `before`
    /// is a before-side node, `after` is a different after-side node, neither
    /// end is already in an earlier pair, and the before side does not name
    /// `after` itself (as a node or an edge endpoint) unless `after` also moves
    /// away (a swap or a chain). Anything else would fold two before ids into
    /// one. Pairs that do not apply are counted in [`GraphDelta::ignored_moves`].
    pub moves: &'a [(NodeId, NodeId)],
}

/// The graph-level difference `after - before`. Every list is sorted by raw
/// id. Ids are after-side ids, except `removed_nodes`, `removed_edges` and the
/// first element of each `moved_nodes` pair, which are before-side ids.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphDelta {
    pub added_nodes: Vec<NodeId>,
    pub removed_nodes: Vec<NodeId>,
    /// On both sides (after the moves are applied) with different content.
    /// Uses the after-side id, and includes a moved node whose content changed.
    pub modified_nodes: Vec<NodeId>,
    /// `(before, after)`: the applied moves, sorted by before id.
    pub moved_nodes: Vec<(NodeId, NodeId)>,
    pub added_edges: Vec<EdgeKey>,
    /// Keyed as the edge sat in `before`, before any move is applied.
    pub removed_edges: Vec<EdgeKey>,
    /// `(key, before, after)`: an edge on both sides whose strongest
    /// confidence changed.
    pub reconfidenced_edges: Vec<(EdgeKey, Confidence, Confidence)>,
    /// `DeltaOptions::moves` pairs that did not apply.
    pub ignored_moves: usize,
}

/// Node id -> its content: sorted, deduplicated `(cell kind, text)`.
type Content<'a> = BTreeMap<u64, Vec<(u32, &'a str)>>;

/// Remapped key -> (strongest confidence, the key as the edge was given).
type EdgeSet = BTreeMap<EdgeKey, (Confidence, EdgeKey)>;

/// The difference between two graph snapshots (see the module docs).
pub fn graph_delta(before: &DeltaSide<'_>, after: &DeltaSide<'_>, opts: &DeltaOptions<'_>) -> GraphDelta {
    let kinds: BTreeSet<u32> = opts.content_cells.iter().map(|k| k.0).collect();
    let (old, new) = (content(before, &kinds), content(after, &kinds));
    let (moves, ignored_moves) = accept_moves(opts.moves, before, &old, &new);
    let remap = |id: u64| moves.get(&id).copied().unwrap_or(id);
    let mut d = GraphDelta { ignored_moves, ..GraphDelta::default() };

    // After id -> the before id it was. The remap is injective (accept_moves).
    let mut was: BTreeMap<u64, u64> = BTreeMap::new();
    for &b in old.keys() {
        let a = remap(b);
        if new.contains_key(&a) {
            was.insert(a, b);
        } else {
            d.removed_nodes.push(NodeId(b));
        }
    }
    for (a, texts) in &new {
        match was.get(a) {
            None => d.added_nodes.push(NodeId(*a)),
            Some(b) if old.get(b) != Some(texts) => d.modified_nodes.push(NodeId(*a)),
            Some(_) => {}
        }
    }
    d.moved_nodes = moves.iter().map(|(&b, &a)| (NodeId(b), NodeId(a))).collect();

    let old_edges = fold_edges(&before.edges, remap);
    let new_edges = fold_edges(&after.edges, |id| id);
    for (key, &(then, given)) in &old_edges {
        match new_edges.get(key) {
            None => d.removed_edges.push(given),
            Some(&(now, _)) if now != then => d.reconfidenced_edges.push((*key, then, now)),
            Some(_) => {}
        }
    }
    // Emitted in remapped order; re-sort under the before-side keys they carry.
    d.removed_edges.sort_unstable();
    d.added_edges = new_edges.keys().filter(|k| !old_edges.contains_key(k)).copied().collect();

    if super::algo_debug() {
        eprintln!(
            "[algo] delta nodes +{} -{} ~{} >{} | edges +{} -{} ~{} | ignored_moves={}",
            d.added_nodes.len(),
            d.removed_nodes.len(),
            d.modified_nodes.len(),
            d.moved_nodes.len(),
            d.added_edges.len(),
            d.removed_edges.len(),
            d.reconfidenced_edges.len(),
            d.ignored_moves,
        );
    }
    d
}

/// Fold a side's nodes into their content: the Text payloads of `kinds` cells,
/// across every copy of an id, sorted and deduplicated.
fn content<'a>(side: &DeltaSide<'a>, kinds: &BTreeSet<u32>) -> Content<'a> {
    let mut out: Content<'a> = BTreeMap::new();
    for &n in &side.nodes {
        let texts = out.entry(n.id.0).or_default();
        for c in &n.cells {
            if let CellPayload::Text(s) = &c.payload
                && kinds.contains(&c.kind.0)
            {
                texts.push((c.kind.0, s.as_str()));
            }
        }
    }
    for texts in out.values_mut() {
        texts.sort_unstable();
        texts.dedup();
    }
    out
}

/// The move pairs that apply (before id -> after id) and how many do not, by
/// the rules on [`DeltaOptions::moves`].
fn accept_moves(pairs: &[(NodeId, NodeId)], before: &DeltaSide<'_>, old: &Content<'_>, new: &Content<'_>) -> (BTreeMap<u64, u64>, usize) {
    let mut moves: BTreeMap<u64, u64> = BTreeMap::new();
    let mut targets: BTreeSet<u64> = BTreeSet::new();
    let mut ignored = 0usize;
    for &(b, a) in pairs {
        let (b, a) = (b.0, a.0);
        if b != a && old.contains_key(&b) && new.contains_key(&a) && !moves.contains_key(&b) && !targets.contains(&a) {
            moves.insert(b, a);
            targets.insert(a);
        } else {
            ignored += 1;
        }
    }
    // Targets the before side also names, as a node or as an edge endpoint.
    let mut named: BTreeSet<u64> = targets.iter().filter(|a| old.contains_key(a)).copied().collect();
    for e in &before.edges {
        for id in [e.from.0, e.to.0] {
            if targets.contains(&id) {
                named.insert(id);
            }
        }
    }
    // Such a target must itself move away, or two before ids would fold into
    // one. Dropping a move can strand another one's target, so repeat.
    loop {
        let clash: Vec<u64> =
            moves.iter().filter(|&(_, a)| named.contains(a) && !moves.contains_key(a)).map(|(&b, _)| b).collect();
        if clash.is_empty() {
            break;
        }
        ignored += clash.len();
        for b in clash {
            moves.remove(&b);
        }
    }
    (moves, ignored)
}

/// Fold edges under `remap`ped endpoints into a set, keeping the strongest
/// confidence of a repeated key and the key as the first copy gave it.
fn fold_edges(edges: &[&Edge], remap: impl Fn(u64) -> u64) -> EdgeSet {
    let mut out = EdgeSet::new();
    for &e in edges {
        let key = EdgeKey { from: NodeId(remap(e.from.0)), to: NodeId(remap(e.to.0)), category: e.category };
        let slot = out.entry(key).or_insert((e.confidence, EdgeKey::from(e)));
        if strength(e.confidence) > strength(slot.0) {
            slot.0 = e.confidence;
        }
    }
    out
}

/// `Strong > Medium > Weak`, because `Confidence` has no `Ord`.
fn strength(c: Confidence) -> u8 {
    match c {
        Confidence::Strong => 2,
        Confidence::Medium => 1,
        Confidence::Weak => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_core::{Cell, RepoId};

    const CODE: u32 = 1;
    const POS: u32 = 3;
    const CALLS: EdgeCategoryId = EdgeCategoryId(1);
    const CONTENT: &[CellTypeId] = &[CellTypeId(CODE)];

    fn text(s: &str) -> CellPayload {
        CellPayload::Text(s.to_string())
    }

    fn node(id: u64, cells: Vec<(u32, CellPayload)>) -> Node {
        Node {
            id: NodeId(id),
            repo: RepoId(1),
            confidence: Confidence::Strong,
            cells: cells.into_iter().map(|(k, payload)| Cell { kind: CellTypeId(k), payload }).collect(),
        }
    }

    fn bare(id: u64) -> Node {
        node(id, Vec::new())
    }

    fn code(id: u64, s: &str) -> Node {
        node(id, vec![(CODE, text(s))])
    }

    fn edge(from: u64, to: u64, c: Confidence) -> Edge {
        Edge::new(NodeId(from), NodeId(to), CALLS, c)
    }

    fn key(from: u64, to: u64) -> EdgeKey {
        EdgeKey { from: NodeId(from), to: NodeId(to), category: CALLS }
    }

    fn ids(xs: &[u64]) -> Vec<NodeId> {
        xs.iter().map(|&x| NodeId(x)).collect()
    }

    fn delta(b: (&[Node], &[Edge]), a: (&[Node], &[Edge]), content_cells: &[CellTypeId], moves: &[(u64, u64)]) -> GraphDelta {
        let moves: Vec<(NodeId, NodeId)> = moves.iter().map(|&(x, y)| (NodeId(x), NodeId(y))).collect();
        let opts = DeltaOptions { content_cells, moves: &moves };
        graph_delta(&DeltaSide::new(b.0, b.1), &DeltaSide::new(a.0, a.1), &opts)
    }

    #[test]
    fn identical_sides_have_empty_delta() {
        let nodes = [code(1, "fn a() {}"), code(2, "fn b() {}")];
        let edges = [edge(1, 2, Confidence::Strong)];
        let d = delta((&nodes, &edges), (&nodes, &edges), CONTENT, &[]);
        assert_eq!(d, GraphDelta::default());
    }

    #[test]
    fn added_removed_modified_nodes() {
        let before = [bare(1), code(2, "x")];
        let after = [code(2, "y"), bare(3)];
        let d = delta((&before, &[]), (&after, &[]), CONTENT, &[]);
        assert_eq!(d.added_nodes, ids(&[3]));
        assert_eq!(d.removed_nodes, ids(&[1]));
        assert_eq!(d.modified_nodes, ids(&[2]));
        assert!(d.moved_nodes.is_empty());
    }

    #[test]
    fn position_only_change_is_not_modified() {
        let before = [node(1, vec![(CODE, text("fn a() {}")), (POS, text("a.rs:4"))])];
        let after = [node(1, vec![(CODE, text("fn a() {}")), (POS, text("a.rs:9"))])];
        let d = delta((&before, &[]), (&after, &[]), CONTENT, &[]);
        assert!(d.modified_nodes.is_empty());
        // The same change is a modification when POSITION counts as content.
        let both = [CellTypeId(CODE), CellTypeId(POS)];
        assert_eq!(delta((&before, &[]), (&after, &[]), &both, &[]).modified_nodes, ids(&[1]));
    }

    #[test]
    fn json_payload_under_content_kind_is_ignored() {
        let json = |s: &str| CellPayload::Json(s.to_string());
        let before = [node(1, vec![(CODE, json(r#"{"sites":[{"line":4}]}"#))])];
        let after = [node(1, vec![(CODE, json(r#"{"sites":[{"line":7}]}"#))])];
        assert!(delta((&before, &[]), (&after, &[]), CONTENT, &[]).modified_nodes.is_empty());
        let bytes = |b: &[u8]| CellPayload::Bytes(b.to_vec());
        let before = [node(1, vec![(CODE, bytes(&[1]))])];
        let after = [node(1, vec![(CODE, bytes(&[2]))])];
        assert!(delta((&before, &[]), (&after, &[]), CONTENT, &[]).modified_nodes.is_empty());
    }

    #[test]
    fn edge_added_removed_and_reconfidenced() {
        let nodes = [bare(1), bare(2), bare(3)];
        let before = [edge(1, 2, Confidence::Strong), edge(2, 3, Confidence::Weak)];
        let after = [edge(2, 3, Confidence::Medium), edge(3, 1, Confidence::Strong)];
        let d = delta((&nodes, &before), (&nodes, &after), CONTENT, &[]);
        assert_eq!(d.added_edges, vec![key(3, 1)]);
        assert_eq!(d.removed_edges, vec![key(1, 2)]);
        assert_eq!(d.reconfidenced_edges, vec![(key(2, 3), Confidence::Weak, Confidence::Medium)]);
        assert!(d.added_nodes.is_empty() && d.removed_nodes.is_empty());
    }

    #[test]
    fn duplicate_edges_fold_to_strongest() {
        let nodes = [bare(1), bare(2)];
        let before = [edge(1, 2, Confidence::Weak), edge(1, 2, Confidence::Strong)];
        let after = [edge(1, 2, Confidence::Strong)];
        assert_eq!(delta((&nodes, &before), (&nodes, &after), CONTENT, &[]), GraphDelta::default());
        // Order of the copies does not matter, and the fold keeps the strongest.
        let after = [edge(1, 2, Confidence::Strong), edge(1, 2, Confidence::Medium), edge(1, 2, Confidence::Weak)];
        assert_eq!(delta((&nodes, &before), (&nodes, &after), CONTENT, &[]), GraphDelta::default());
    }

    #[test]
    fn edge_cells_are_not_compared() {
        let nodes = [bare(1), bare(2)];
        let site = |line: &str| Cell { kind: CellTypeId(9), payload: text(line) };
        let before = [edge(1, 2, Confidence::Strong).with_cell(site("a.rs:3"))];
        let after = [edge(1, 2, Confidence::Strong).with_cell(site("a.rs:8"))];
        assert_eq!(delta((&nodes, &before), (&nodes, &after), CONTENT, &[]), GraphDelta::default());
    }

    #[test]
    fn moves_remap_nodes_and_edges() {
        let before = [code(1, "fn a() {}"), bare(2)];
        let after = [code(11, "fn a() {}"), bare(2)];
        let (eb, ea) = ([edge(1, 2, Confidence::Strong)], [edge(11, 2, Confidence::Strong)]);
        let d = delta((&before, &eb), (&after, &ea), CONTENT, &[(1, 11)]);
        assert_eq!(d.moved_nodes, vec![(NodeId(1), NodeId(11))]);
        assert!(d.added_nodes.is_empty() && d.removed_nodes.is_empty() && d.modified_nodes.is_empty());
        assert!(d.added_edges.is_empty() && d.removed_edges.is_empty());
        assert_eq!(d.ignored_moves, 0);
    }

    #[test]
    fn removed_edge_of_moved_node_keeps_before_ids() {
        let before = [bare(1), bare(2)];
        let after = [bare(11), bare(2)];
        let eb = [edge(1, 2, Confidence::Strong)];
        let ea = [edge(2, 11, Confidence::Weak)];
        let d = delta((&before, &eb), (&after, &ea), CONTENT, &[(1, 11)]);
        assert_eq!(d.removed_edges, vec![key(1, 2)]);
        assert_eq!(d.added_edges, vec![key(2, 11)]);
    }

    #[test]
    fn move_with_changed_code_is_moved_and_modified() {
        let before = [code(1, "fn a() {}")];
        let after = [code(11, "fn a() { b() }")];
        let d = delta((&before, &[]), (&after, &[]), CONTENT, &[(1, 11)]);
        assert_eq!(d.moved_nodes, vec![(NodeId(1), NodeId(11))]);
        assert_eq!(d.modified_nodes, ids(&[11]));
        assert!(d.added_nodes.is_empty() && d.removed_nodes.is_empty());
    }

    #[test]
    fn move_with_missing_end_is_ignored_and_counted() {
        let before = [bare(1)];
        let after = [bare(11)];
        // 11 is the real target; 99 is no after-side node.
        let d = delta((&before, &[]), (&after, &[]), CONTENT, &[(1, 99)]);
        assert_eq!(d.ignored_moves, 1);
        assert!(d.moved_nodes.is_empty());
        assert_eq!(d.removed_nodes, ids(&[1]));
        assert_eq!(d.added_nodes, ids(&[11]));
    }

    #[test]
    fn moves_that_would_merge_ids_are_ignored() {
        // An identity pair, a repeated source, and a target the before side
        // keeps: none of them applies.
        let before = [bare(1), bare(2), bare(3)];
        let after = [bare(2), bare(3), bare(4)];
        let d = delta((&before, &[]), (&after, &[]), CONTENT, &[(3, 3), (1, 4), (1, 2), (2, 2)]);
        assert_eq!(d.moved_nodes, vec![(NodeId(1), NodeId(4))]);
        assert_eq!(d.ignored_moves, 3);
        // A swap applies: each target moves away itself.
        let d = delta((&before, &[]), (&after, &[]), CONTENT, &[(2, 3), (3, 2)]);
        assert_eq!(d.moved_nodes, vec![(NodeId(2), NodeId(3)), (NodeId(3), NodeId(2))]);
        assert_eq!(d.ignored_moves, 0);
        // Dropping 2=>3 (3 stays in before) strands 1=>2, whose target 2 then stays too.
        let d = delta((&before, &[]), (&after, &[]), CONTENT, &[(1, 2), (2, 3)]);
        assert!(d.moved_nodes.is_empty());
        assert_eq!(d.ignored_moves, 2);
        assert_eq!(d.removed_nodes, ids(&[1]));
        // A target named only by a before-side edge endpoint is kept too.
        let eb = [edge(2, 4, Confidence::Weak)];
        let d = delta((&before, &eb), (&after, &[]), CONTENT, &[(1, 4)]);
        assert!(d.moved_nodes.is_empty());
        assert_eq!(d.ignored_moves, 1);
    }

    #[test]
    fn node_in_two_copies_folds_order_independent() {
        let copy_a = node(1, vec![(CODE, text("class A {}"))]);
        let copy_b = node(1, vec![(2, text("A"))]);
        let content = [CellTypeId(CODE), CellTypeId(2)];
        let before = [copy_a.clone(), copy_b.clone()];
        let after = [copy_b.clone(), copy_a.clone(), copy_a.clone()];
        let d = delta((&before, &[]), (&after, &[]), &content, &[]);
        assert_eq!(d, GraphDelta::default());
        // A real change in one copy still shows.
        let after = [copy_b, node(1, vec![(CODE, text("class A { x }"))])];
        assert_eq!(delta((&before, &[]), (&after, &[]), &content, &[]).modified_nodes, ids(&[1]));
    }

    #[test]
    fn output_is_sorted() {
        let before = [code(30, "x"), bare(10), code(20, "y"), bare(7), bare(8)];
        let after = [bare(40), code(20, "z"), bare(5), code(30, "w"), bare(12), bare(1)];
        let eb = [edge(30, 20, Confidence::Strong), edge(10, 20, Confidence::Strong), edge(7, 20, Confidence::Strong)];
        let ea = [edge(40, 5, Confidence::Strong), edge(5, 40, Confidence::Strong), edge(20, 30, Confidence::Strong)];
        // 8 => 1 and 7 => 12: before-id order and after-id order disagree.
        let d = delta((&before, &eb), (&after, &ea), CONTENT, &[(8, 1), (7, 12)]);
        assert_eq!(d.added_nodes, ids(&[5, 40]));
        assert_eq!(d.removed_nodes, ids(&[10]));
        assert_eq!(d.modified_nodes, ids(&[20, 30]));
        assert_eq!(d.moved_nodes, vec![(NodeId(7), NodeId(12)), (NodeId(8), NodeId(1))]);
        assert_eq!(d.added_edges, vec![key(5, 40), key(20, 30), key(40, 5)]);
        // 7->20 remaps to 12->20, which sorts after 10->20; the output is in
        // before-key order.
        assert_eq!(d.removed_edges, vec![key(7, 20), key(10, 20), key(30, 20)]);
    }
}
