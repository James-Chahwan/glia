//! Domain-agnostic graph algorithms (LD.15a): the home of reachability
//! ([`reach`]), and next of graph delta (LE.1) and cycles (LE.6).
//!
//! This crate depends on `core` only, so nothing here can name a domain's
//! node kind or edge category: an algorithm takes the categories it follows
//! as a [`CategorySet`], or a domain's [`DomainTables`] through
//! [`Adjacency::carry`]. A graph type opts in by implementing
//! [`GraphSource`]; the algorithms then run over an [`Adjacency`], a CSR
//! index built once per query in O(V + E), instead of scanning the whole edge
//! list for every node they visit.
//!
//! The index preserves the edge-scan walks it replaced exactly (the parity
//! rules on [`Adjacency`]), so a first-reach category, a discovery order or a
//! depth never changes when a caller moves onto it; `reach`'s tests hold the
//! old scan loops as the oracle.

use std::collections::HashMap;
use std::sync::OnceLock;

use repo_graph_core::{Edge, EdgeCategoryId, NodeId};

use crate::profile::DomainTables;

pub mod reach;

/// A graph the algorithms can index: its node ids and its edges, each in a
/// stable order. Results that depend on order (a BFS's discovery order, the
/// category that first reaches a node) follow these orders.
pub trait GraphSource {
    /// Every node id, in a stable order. A repeated id counts once, at its
    /// first occurrence.
    fn node_ids(&self) -> Vec<NodeId>;

    /// Every edge, in a stable order. An endpoint need not be one of
    /// [`Self::node_ids`]: an edge to an id that is no node is still walked,
    /// and that id reached.
    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_>;
}

/// The edge categories a walk follows: every category, or a listed set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategorySet {
    /// Sorted by id, deduplicated; unused when `all`.
    sorted: Vec<EdgeCategoryId>,
    all: bool,
}

impl CategorySet {
    /// Every category.
    pub fn all() -> Self {
        Self { sorted: Vec::new(), all: true }
    }

    /// Exactly the categories in `c` (none when `c` is empty).
    pub fn of(c: &[EdgeCategoryId]) -> Self {
        let mut sorted = c.to_vec();
        sorted.sort_unstable_by_key(|c| c.0);
        sorted.dedup();
        Self { sorted, all: false }
    }

    pub fn contains(&self, c: EdgeCategoryId) -> bool {
        self.all || self.sorted.binary_search_by_key(&c.0, |x| x.0).is_ok()
    }
}

/// Which way a walk follows an edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walk {
    /// `from -> to`: what a node reaches.
    Forward,
    /// `to -> from`: what reaches a node.
    Backward,
    /// Both ways, one pass over a node's edges in edge order: an edge leaving
    /// the node is walked forward, an edge entering it backward, and a
    /// self-loop once, forward.
    Both,
}

/// One edge at one of its endpoints: the edge's position among the kept
/// edges (their global order), the node at its other end, and its category.
#[derive(Clone, Copy, Debug)]
struct Inc {
    edge: u32,
    other: u32,
    category: EdgeCategoryId,
}

/// A CSR index over a [`GraphSource`]'s edges of the kept categories: each
/// node's outgoing and incoming edges as a contiguous slice.
///
/// Parity rules — each keeps a behaviour of the edge-scan walks this replaced:
/// 1. Dangling endpoints are indexed. The dense index is the node ids first,
///    then every kept edge's endpoint that is no node, in first-seen edge
///    order, so an edge to such an id is still walked.
/// 2. Each incidence list is in global edge order, so neighbours are visited
///    in the order a scan of the edge list meets them.
/// 3. [`Walk::Both`] merges a node's out- and in-list by edge position, and
///    takes an edge on both (a self-loop) once, as forward.
/// 4. A repeated node id is indexed once, at its first occurrence.
///
/// Only lookups touch the id map, never its iteration order.
#[derive(Clone, Debug, Default)]
pub struct Adjacency {
    ids: Vec<NodeId>,
    index: HashMap<NodeId, u32>,
    out_start: Vec<u32>,
    out: Vec<Inc>,
    in_start: Vec<u32>,
    inn: Vec<Inc>,
    kept: usize,
}

impl Adjacency {
    /// Index `g`'s edges whose category `keep` contains.
    pub fn build<G: GraphSource + ?Sized>(g: &G, keep: &CategorySet) -> Self {
        Self::build_capped(g, keep, u32::MAX as usize)
    }

    /// Index `g`'s edges of the domain's reachability categories
    /// (`tables.carry_edges`).
    pub fn carry<G: GraphSource + ?Sized>(g: &G, tables: &DomainTables) -> Self {
        Self::build(g, &CategorySet::of(tables.carry_edges))
    }

    /// [`Self::build`] with the dense-index limit as a parameter, so the
    /// over-limit path is testable. Over `cap` nodes or kept edges the index
    /// is empty (every walk from it reaches nothing) and a warning is printed.
    fn build_capped<G: GraphSource + ?Sized>(g: &G, keep: &CategorySet, cap: usize) -> Self {
        let mut ids: Vec<NodeId> = Vec::new();
        let mut index: HashMap<NodeId, u32> = HashMap::new();
        let mut intern = |id: NodeId, ids: &mut Vec<NodeId>| -> Option<u32> {
            if let Some(&ix) = index.get(&id) {
                return Some(ix);
            }
            if ids.len() >= cap {
                return None;
            }
            let ix = ids.len() as u32;
            index.insert(id, ix);
            ids.push(id);
            Some(ix)
        };
        for id in g.node_ids() {
            if intern(id, &mut ids).is_none() {
                return Self::over_limit("nodes", cap);
            }
        }
        let nodes = ids.len();
        let mut total = 0usize;
        let mut kept: Vec<(u32, u32, EdgeCategoryId)> = Vec::new();
        for e in g.edges() {
            total += 1;
            if !keep.contains(e.category) {
                continue;
            }
            if kept.len() >= cap {
                return Self::over_limit("kept edges", cap);
            }
            let (Some(from), Some(to)) = (intern(e.from, &mut ids), intern(e.to, &mut ids)) else {
                return Self::over_limit("nodes", cap);
            };
            kept.push((from, to, e.category));
        }

        let n = ids.len();
        let (out_start, out) = csr(n, &kept, |&(from, to, c), k| (from, Inc { edge: k, other: to, category: c }));
        let (in_start, inn) = csr(n, &kept, |&(from, to, c), k| (to, Inc { edge: k, other: from, category: c }));
        if algo_debug() {
            eprintln!(
                "[algo] adjacency nodes={nodes} dangling={} kept={} of {total} edges",
                n - nodes,
                kept.len()
            );
        }
        Self { ids, index, out_start, out, in_start, inn, kept: kept.len() }
    }

    fn over_limit(what: &str, cap: usize) -> Self {
        eprintln!("[algo] adjacency: more than {cap} {what} - u32 index limit, returning an empty index");
        Self { out_start: vec![0], in_start: vec![0], ..Self::default() }
    }

    /// Indexed ids: the nodes plus the dangling edge endpoints.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Edges of the kept categories.
    pub fn kept_edges(&self) -> usize {
        self.kept
    }

    /// The id at dense index `ix` (`ix < len()`).
    pub fn id(&self, ix: u32) -> NodeId {
        self.ids[ix as usize]
    }

    /// The dense index of `id`, when it is a node or a kept edge's endpoint.
    pub fn index_of(&self, id: NodeId) -> Option<u32> {
        self.index.get(&id).copied()
    }

    /// Kept edges leaving `ix`, in edge order.
    fn outgoing(&self, ix: u32) -> &[Inc] {
        let i = ix as usize;
        &self.out[self.out_start[i] as usize..self.out_start[i + 1] as usize]
    }

    /// Kept edges entering `ix`, in edge order.
    fn incoming(&self, ix: u32) -> &[Inc] {
        let i = ix as usize;
        &self.inn[self.in_start[i] as usize..self.in_start[i + 1] as usize]
    }
}

/// Counting-sort `kept` into a CSR list keyed by the endpoint `key` picks.
/// Rows are filled in kept order, so every row stays in edge order.
fn csr(
    n: usize,
    kept: &[(u32, u32, EdgeCategoryId)],
    key: impl Fn(&(u32, u32, EdgeCategoryId), u32) -> (u32, Inc),
) -> (Vec<u32>, Vec<Inc>) {
    let mut start = vec![0u32; n + 1];
    for (k, e) in kept.iter().enumerate() {
        start[key(e, k as u32).0 as usize + 1] += 1;
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut cursor = start.clone();
    let blank = Inc { edge: 0, other: 0, category: EdgeCategoryId(0) };
    let mut list = vec![blank; kept.len()];
    for (k, e) in kept.iter().enumerate() {
        let (row, inc) = key(e, k as u32);
        let slot = &mut cursor[row as usize];
        list[*slot as usize] = inc;
        *slot += 1;
    }
    (start, list)
}

/// `GLIA_ALGO_DEBUG=1` turns on the `[algo] adjacency` line, read once.
fn algo_debug() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("GLIA_ALGO_DEBUG").is_ok_and(|v| v == "1"))
}

/// A graph given as its two lists, for tests.
#[cfg(test)]
pub(crate) struct ToyGraph {
    pub nodes: Vec<NodeId>,
    pub edges: Vec<Edge>,
}

#[cfg(test)]
impl GraphSource for ToyGraph {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

#[cfg(test)]
pub(crate) fn toy_edge(from: u64, to: u64, category: u32) -> Edge {
    Edge {
        from: NodeId(from),
        to: NodeId(to),
        category: EdgeCategoryId(category),
        confidence: repo_graph_core::Confidence::Strong,
        cells: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{EntryRule, Registries};

    const A: EdgeCategoryId = EdgeCategoryId(1);
    const B: EdgeCategoryId = EdgeCategoryId(2);
    const C: EdgeCategoryId = EdgeCategoryId(3);

    const TABLES: DomainTables = DomainTables {
        graph_type: "toy",
        registries: Registries {
            node_kinds: &[],
            edge_categories: &[(A, "A"), (B, "B"), (C, "C")],
            cell_types: &[],
        },
        entry: EntryRule { kinds: &[], roles: &[], named: &[] },
        carry_edges: &[B, A],
        effect_sinks: &[],
        activation_weights: &[],
        activation_presets: &[],
    };

    fn ids(adj: &Adjacency, xs: &[Inc]) -> Vec<(u64, u32)> {
        xs.iter().map(|i| (adj.id(i.other).0, i.category.0)).collect()
    }

    #[test]
    fn carry_uses_table_categories() {
        let g = ToyGraph {
            nodes: vec![NodeId(1), NodeId(2), NodeId(3), NodeId(4)],
            edges: vec![toy_edge(1, 2, 1), toy_edge(1, 3, 3), toy_edge(1, 4, 2), toy_edge(2, 3, 3)],
        };
        let adj = Adjacency::carry(&g, &TABLES);
        assert_eq!(adj.kept_edges(), 2);
        let one = adj.index_of(NodeId(1)).unwrap();
        assert_eq!(ids(&adj, adj.outgoing(one)), vec![(2, 1), (4, 2)]);
        let three = adj.index_of(NodeId(3)).unwrap();
        assert!(adj.incoming(three).is_empty(), "C edges are not carried");
    }

    #[test]
    fn category_set_membership() {
        let s = CategorySet::of(&[C, A, C]);
        assert!(s.contains(A) && s.contains(C) && !s.contains(B));
        assert!(!CategorySet::of(&[]).contains(A));
        assert!(CategorySet::all().contains(EdgeCategoryId(999)));
    }

    #[test]
    fn index_holds_nodes_once_then_dangling_endpoints_in_edge_order() {
        let g = ToyGraph {
            nodes: vec![NodeId(5), NodeId(1), NodeId(5)],
            // 9 and 7 are no node; 8 sits only on a dropped (category 3) edge.
            edges: vec![toy_edge(1, 9, 1), toy_edge(8, 1, 3), toy_edge(7, 5, 2), toy_edge(9, 1, 1)],
        };
        let adj = Adjacency::build(&g, &CategorySet::of(&[A, B]));
        let order: Vec<u64> = (0..adj.len() as u32).map(|i| adj.id(i).0).collect();
        assert_eq!(order, vec![5, 1, 9, 7]);
        assert_eq!(adj.index_of(NodeId(8)), None);
        assert_eq!(adj.kept_edges(), 3);
        let nine = adj.index_of(NodeId(9)).unwrap();
        assert_eq!(ids(&adj, adj.outgoing(nine)), vec![(1, 1)]);
        assert_eq!(ids(&adj, adj.incoming(nine)), vec![(1, 1)]);
    }

    #[test]
    fn over_the_index_limit_is_empty_not_a_panic() {
        let g = ToyGraph {
            nodes: vec![NodeId(1), NodeId(2)],
            edges: vec![toy_edge(1, 2, 1), toy_edge(2, 3, 1)],
        };
        // Two nodes fit a cap of 2; the dangling endpoint 3 does not.
        let adj = Adjacency::build_capped(&g, &CategorySet::all(), 2);
        assert!(adj.is_empty());
        assert_eq!(adj.kept_edges(), 0);
        assert_eq!(adj.index_of(NodeId(1)), None);
        // At a cap of 1 the one node fits; the second kept edge does not.
        let one = ToyGraph { nodes: vec![NodeId(1)], edges: vec![toy_edge(1, 1, 1), toy_edge(1, 1, 1)] };
        assert!(Adjacency::build_capped(&one, &CategorySet::all(), 1).is_empty());
        assert_eq!(Adjacency::build_capped(&g, &CategorySet::all(), 3).len(), 3);
    }
}
