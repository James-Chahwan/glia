//! Deterministic [`MemSource`] graph builders for the crate's tests, benchmarks and
//! gates, and for engine-side tests that want the same graphs. A test and benchmark
//! kit, not a domain: [`tables`] registers four kinds and six categories with
//! test-local ids that mean nothing outside this module, and no shipped answer reads
//! it. It is public API only so every test target (and DB's engine tests) can share
//! it; it promises nothing beyond 0.5.2's exact pin.
//!
//! Every builder numbers its nodes `0..n`: node `i` is
//! `NodeId::from_parts("rules-test", RepoId(salt), kind, "n<i>")` ([`node_id`]), of
//! kind `KINDS[i % 4]`, rendered `n<i:05>` (`n00042`), with the Int attribute
//! `index` = `i` (attribute 0, [`INDEX`]), so a rule seeds by `index(x, 0)` and rows
//! sort by node number. Randomness is one [`Lcg`] stream per builder, never the
//! clock or an address, so a graph is the same on every run and every machine.
//!
//! - [`lcg_graph`]: `nodes` nodes and `edges` random edges over the named categories
//!   at a FACT / DERIVED / HEURISTIC mix ([`GraphSpec`]).
//! - [`chain`]: `0 -> 1 -> .. -> n-1`, hop `i` at `tiers[i % len]`.
//! - [`clique_ring`]: a ring plus random chords, so one strongly connected component
//!   and an unseeded transitive closure of exactly `n^2` rows.
//! - [`resalted`]: the same rendered graph under new NodeIds with node and edge order
//!   reversed: a checkout at another path (code NodeIds hash a path-derived RepoId,
//!   and edge order follows the ids), for the determinism gate.
//! - [`scaled`]: the R1 spike's `x k` scaler, restated exactly (k copies plus
//!   `edges * k / 100` random cross CALLS edges, [`scaled_size`]), so cost numbers
//!   compare with R1 section 1.4.
//!
//! A builder's tier also sets the edge's confidence (FACT strong, DERIVED medium,
//! HEURISTIC weak), so `edge_conf/4` rows follow the tier. A category name that is no
//! [`CATEGORIES`] row is `EdgeCategoryId(0)`, which no registry names (it renders
//! `#0`).

use glia_activation::algo::GraphSource;
use glia_activation::profile::{DomainTables, EntryRule, Registries};
use glia_core::{Confidence, EdgeCategoryId, NodeId, NodeKindId, RepoId};

use crate::ir::{AttrDecl, ColType};
use crate::source::{FactSource, MemSource};
use crate::value::{Lookup, Tier, Value};

// ============================================================================
// The LCG
// ============================================================================

/// The R1 spike's linear congruential generator: `next_u32` steps the state by
/// `state * 6364136223846793005 + 1442695040888963407` (wrapping) and returns its
/// high bits, `(state >> 33) as u32`. The field is the state; `Lcg(seed)` steps
/// before its first value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lcg(pub u64);

impl Lcg {
    /// The multiplier (Knuth's MMIX).
    pub const MUL: u64 = 6_364_136_223_846_793_005;
    /// The increment.
    pub const INC: u64 = 1_442_695_040_888_963_407;

    /// Step the state, then return its top 31 bits.
    pub fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(Self::MUL).wrapping_add(Self::INC);
        (self.0 >> 33) as u32
    }

    /// The next value modulo `n` (`n > 0`), as an index.
    fn below(&mut self, n: usize) -> usize {
        self.next_u32() as usize % n
    }
}

/// The seed of [`scaled`]'s cross-edge stream: the spike's `0x9E3779B97F4A7C15`.
pub const SCALE_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

// ============================================================================
// The tables
// ============================================================================

/// The graph type of every builder's node ids and of [`tables`].
pub const GRAPH_TYPE: &str = "rules-test";

/// The kit's node kinds, `(name, test-local id)`; node `i` is `KINDS[i % 4]`.
pub const KINDS: [(&str, u32); 4] = [("FUNCTION", 1), ("MODULE", 2), ("ROUTE", 3), ("OBJECT", 4)];

/// The kit's edge categories, `(name, test-local id)`.
pub const CATEGORIES: [(&str, u32); 6] =
    [("CALLS", 1), ("IMPORTS", 2), ("USES", 3), ("HANDLED_BY", 4), ("TESTS", 5), ("NEXT", 6)];

/// Attribute 0 of every builder's source: `index(Node, int)`, node `i`'s number.
pub const INDEX: AttrDecl = AttrDecl { name: "index", ty: ColType::Int };

static KIND_REGISTRY: [(NodeKindId, &str); 4] = {
    let mut out = [(NodeKindId(0), ""); 4];
    let mut i = 0;
    while i < KINDS.len() {
        out[i] = (NodeKindId(KINDS[i].1), KINDS[i].0);
        i += 1;
    }
    out
};

static CATEGORY_REGISTRY: [(EdgeCategoryId, &str); 6] = {
    let mut out = [(EdgeCategoryId(0), ""); 6];
    let mut i = 0;
    while i < CATEGORIES.len() {
        out[i] = (EdgeCategoryId(CATEGORIES[i].1), CATEGORIES[i].0);
        i += 1;
    }
    out
};

/// CALLS and USES (`CATEGORIES[0]`, `CATEGORIES[2]`).
static CARRY: [EdgeCategoryId; 2] =
    [EdgeCategoryId(CATEGORIES[0].1), EdgeCategoryId(CATEGORIES[2].1)];

/// The kit's domain tables: graph type [`GRAPH_TYPE`], the [`KINDS`] and
/// [`CATEGORIES`] registries (no cell types), carry edges CALLS and USES, and an
/// empty entry rule, effect sinks, weights, presets and identity rules.
pub fn tables() -> DomainTables {
    DomainTables {
        graph_type: GRAPH_TYPE,
        registries: Registries {
            node_kinds: &KIND_REGISTRY,
            edge_categories: &CATEGORY_REGISTRY,
            cell_types: &[],
        },
        entry: EntryRule { kinds: &[], roles: &[], named: &[] },
        carry_edges: &CARRY,
        effect_sinks: &[],
        activation_weights: &[],
        activation_presets: &[],
        community_weights: &[],
        identity_rules: &[],
    }
}

/// The [`CATEGORIES`] id of `name`; `EdgeCategoryId(0)` for any other name.
pub fn category_id(name: &str) -> EdgeCategoryId {
    let id = CATEGORIES.iter().find(|(n, _)| *n == name).map_or(0, |(_, id)| *id);
    EdgeCategoryId(id)
}

/// The kind of node `i`: `KINDS[i % 4]`.
fn kind_of(i: u32) -> NodeKindId {
    NodeKindId(KINDS[i as usize % KINDS.len()].1)
}

/// Node `i`'s id in a builder's graph under `salt` ([`resalted`] mints others).
/// [`lcg_graph`] uses [`GraphSpec::salt`]; [`chain`] and [`clique_ring`] use 0.
pub fn node_id(salt: u64, i: u32) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, RepoId(salt), kind_of(i), &format!("n{i}"))
}

/// A builder's confidence for an edge at `tier`.
fn confidence(tier: Tier) -> Confidence {
    match tier {
        Tier::Fact => Confidence::Strong,
        Tier::Derived => Confidence::Medium,
        Tier::Heuristic => Confidence::Weak,
    }
}

/// A source over [`tables`] holding nodes `0..n` under `salt` (module doc), with
/// [`INDEX`] declared; returns their ids in node order.
fn numbered(n: u32, salt: u64) -> (MemSource, Vec<NodeId>) {
    let mut g = MemSource::new(tables());
    g.attr(INDEX);
    let mut ids = Vec::with_capacity(n as usize);
    for i in 0..n {
        let id = node_id(salt, i);
        g.node(id, kind_of(i), format!("n{i:05}")).set_attr(id, 0, Value::Int(i64::from(i)));
        ids.push(id);
    }
    (g, ids)
}

// ============================================================================
// The builders
// ============================================================================

/// What [`lcg_graph`] builds.
#[derive(Clone, Debug)]
pub struct GraphSpec {
    pub nodes: u32,
    pub edges: u32,
    /// The categories edges are drawn from, by [`CATEGORIES`] name. Empty: no edges.
    pub categories: Vec<&'static str>,
    /// Percent FACT / DERIVED / HEURISTIC. A roll in `0..100` below the FACT share is
    /// FACT, below FACT + DERIVED is DERIVED, else HEURISTIC, so shares summing under
    /// 100 leave the rest HEURISTIC.
    pub tier_mix: [u8; 3],
    /// The LCG's initial state.
    pub seed: u64,
    /// The RepoId the node ids hash ([`node_id`]).
    pub salt: u64,
}

/// `spec.nodes` numbered nodes and `spec.edges` random edges. Each edge takes four
/// draws of one `Lcg(spec.seed)` stream, in order: its source and target (`% nodes`),
/// its category (`% categories.len()`) and its tier roll (`% 100`). Self-loops and
/// repeated edges stay, as in a real graph. No nodes or no categories: no edges.
pub fn lcg_graph(spec: &GraphSpec) -> MemSource {
    let (mut g, ids) = numbered(spec.nodes, spec.salt);
    let cats: Vec<EdgeCategoryId> = spec.categories.iter().map(|c| category_id(c)).collect();
    if ids.is_empty() || cats.is_empty() {
        return g;
    }
    let [fact, derived, _] = spec.tier_mix.map(u32::from);
    let mut rng = Lcg(spec.seed);
    for _ in 0..spec.edges {
        let from = ids[rng.below(ids.len())];
        let to = ids[rng.below(ids.len())];
        let category = cats[rng.below(cats.len())];
        let roll = rng.next_u32() % 100;
        let tier = if roll < fact {
            Tier::Fact
        } else if roll < fact + derived {
            Tier::Derived
        } else {
            Tier::Heuristic
        };
        g.edge(from, to, category, confidence(tier), tier);
    }
    g
}

/// `0 -> 1 -> .. -> n-1` over `category` (salt 0): hop `i` (from node `i`) at
/// `tiers[i % tiers.len()]`; every hop DERIVED when `tiers` is empty, the tier a
/// domain that says nothing gets.
pub fn chain(n: u32, category: &str, tiers: &[Tier]) -> MemSource {
    let (mut g, ids) = numbered(n, 0);
    let cat = category_id(category);
    for (i, hop) in ids.windows(2).enumerate() {
        let tier = if tiers.is_empty() { Tier::Derived } else { tiers[i % tiers.len()] };
        g.edge(hop[0], hop[1], cat, confidence(tier), tier);
    }
    g
}

/// The ring `0 -> 1 -> .. -> n-1 -> 0`, then `chords` random edges (source and
/// target each `% n` of one `Lcg(seed)` stream), all over `category` at FACT (salt
/// 0). Strongly connected, so every node reaches every node, itself included: an
/// unseeded transitive closure is exactly `n^2` rows. `n = 1` is one self-loop plus
/// the chords on it; `n = 0` is empty.
pub fn clique_ring(n: u32, chords: u32, category: &str, seed: u64) -> MemSource {
    let (mut g, ids) = numbered(n, 0);
    if ids.is_empty() {
        return g;
    }
    let cat = category_id(category);
    let fact = confidence(Tier::Fact);
    for (i, &from) in ids.iter().enumerate() {
        g.edge(from, ids[(i + 1) % ids.len()], cat, fact, Tier::Fact);
    }
    let mut rng = Lcg(seed);
    for _ in 0..chords {
        let from = ids[rng.below(ids.len())];
        let to = ids[rng.below(ids.len())];
        g.edge(from, to, cat, fact, Tier::Fact);
    }
    g
}

/// `g` as a checkout at another path would give it: every node and edge endpoint
/// under a new id, `NodeId::from_parts(graph_type, RepoId(salt), NodeKindId(0),
/// "<old id as 16 hex digits>")` (one per old id, so even two nodes with one render
/// stay two), the node order and the edge order reversed, and kinds, renders,
/// attributes, cells, confidences, edge cells and tiers unchanged. The rendered graph
/// is the same, so rows compared by rendered value must be too; witness ordinals and
/// id orders legitimately differ. A dangling endpoint renders its id, so it renders
/// differently (no builder here makes one). Builtins are not copied: a
/// `Box<dyn Builtin>` cannot be cloned, so register them on the result.
pub fn resalted(g: &MemSource, salt: u64) -> MemSource {
    let graph_type = g.tables().graph_type;
    let new_id = |id: NodeId| {
        NodeId::from_parts(graph_type, RepoId(salt), NodeKindId(0), &format!("{:016x}", id.0))
    };
    let mut out = MemSource::new(*g.tables());
    let ids: Vec<NodeId> = g.node_ids().into_iter().rev().collect();
    for &id in &ids {
        if let Some(kind) = g.node_kind(id) {
            out.node(new_id(id), kind, g.render_node(id));
        }
    }
    copy_columns(g, &mut out, 1, |_, id| new_id(id));
    let edges: Vec<_> = g.edges().enumerate().collect();
    for (i, e) in edges.into_iter().rev() {
        let mut copy = e.clone();
        copy.from = new_id(e.from);
        copy.to = new_id(e.to);
        out.push_edge(copy, g.edge_tier(i, e));
    }
    out
}

/// Nodes and edges of [`scaled`]`(g, k)` for a graph of `nodes` nodes and `edges`
/// edges: `(nodes * k, edges * k + edges * k / 100)` (saturating), the spike's
/// arithmetic. R1 section 1.4's glia x10 is `scaled_size(18_387, 56_738, 10)` =
/// `(183_870, 573_053)`.
pub fn scaled_size(nodes: usize, edges: usize, k: u32) -> (usize, usize) {
    let copied = edges.saturating_mul(k as usize);
    (nodes.saturating_mul(k as usize), copied.saturating_add(cross_edges(edges, k)))
}

/// The spike's cross-edge count: `edges * k / 100` (saturating).
fn cross_edges(edges: usize, k: u32) -> usize {
    edges.saturating_mul(k as usize) / 100
}

/// The spike's `x k`: `k` copies of `g` plus `edges * k / 100` random cross CALLS
/// edges ([`scaled_size`]).
///
/// Copy `c` of node `v` is `NodeId::from_parts("rules-scaled", RepoId(c),
/// NodeKindId(0), "<v as 16 hex digits>")` with `v`'s kind, render, attributes and
/// cells unchanged (renders repeat across copies, so a scope keyed on the render
/// spans every copy, as in the spike). Nodes are added copy `0..k`, each in `g`'s
/// node order; then edges, copy `0..k`, each in `g`'s edge order with its tier,
/// confidence and cells kept (a dangling endpoint maps by the same formula); then the
/// cross edges. Cross edge `j` takes two draws of one `Lcg(`[`SCALE_SEED`]`)` stream,
/// source then target, each `% (n * k)`: a dense index into the scaled node order
/// (copy `c`, node `v` is `c * n + v`). Cross edges are FACT, strong, of `g`'s
/// registered `CALLS` category (`EdgeCategoryId(0)` when it has none). `g`'s tables
/// are kept; builtins are not copied (see [`resalted`]).
pub fn scaled(g: &MemSource, k: u32) -> MemSource {
    let copy_id = |c: u32, id: NodeId| {
        NodeId::from_parts(
            "rules-scaled",
            RepoId(u64::from(c)),
            NodeKindId(0),
            &format!("{:016x}", id.0),
        )
    };
    let mut out = MemSource::new(*g.tables());
    let ids: Vec<NodeId> = g.node_ids();
    let kinds: Vec<Option<NodeKindId>> = ids.iter().map(|id| g.node_kind(*id)).collect();
    let renders: Vec<String> = ids.iter().map(|id| g.render_node(*id)).collect();
    let n = ids.len();
    let mut position: Lookup<NodeId, usize> = Lookup::new();
    for (p, id) in ids.iter().enumerate() {
        position.get_or_insert_with(*id, || p);
    }
    // The scaled node order: copy c, node v at c * n + v.
    let mut dense: Vec<NodeId> = Vec::with_capacity(n.saturating_mul(k as usize));
    for c in 0..k {
        for (p, &v) in ids.iter().enumerate() {
            let id = copy_id(c, v);
            dense.push(id);
            if let Some(kind) = kinds[p] {
                out.node(id, kind, renders[p].clone());
            }
        }
    }
    let endpoint = |c: u32, id: NodeId| match position.get(&id) {
        Some(p) => dense[c as usize * n + p],
        None => copy_id(c, id),
    };
    copy_columns(g, &mut out, k, endpoint);
    let edges: Vec<_> = g.edges().collect();
    let tiers: Vec<Tier> = edges.iter().enumerate().map(|(i, e)| g.edge_tier(i, e)).collect();
    for c in 0..k {
        for (e, tier) in edges.iter().zip(&tiers) {
            let mut copy = (*e).clone();
            copy.from = endpoint(c, e.from);
            copy.to = endpoint(c, e.to);
            out.push_edge(copy, *tier);
        }
    }
    let cross = cross_edges(edges.len(), k);
    if !dense.is_empty() {
        let calls = g
            .tables()
            .registries
            .edge_categories
            .iter()
            .find(|(_, name)| *name == "CALLS")
            .map_or(EdgeCategoryId(0), |(id, _)| *id);
        let mut rng = Lcg(SCALE_SEED);
        for _ in 0..cross {
            let from = dense[rng.below(dense.len())];
            let to = dense[rng.below(dense.len())];
            out.edge(from, to, calls, Confidence::Strong, Tier::Fact);
        }
    }
    out
}

/// Copy `g`'s attribute columns (same declarations, same indexes) and node cells
/// into `out`, `copies` times: copy `c` of node `v` is `to(c, v)`. `out` already
/// holds every target node (`set_attr` / `cell` skip any other id).
fn copy_columns(
    g: &MemSource,
    out: &mut MemSource,
    copies: u32,
    to: impl Fn(u32, NodeId) -> NodeId,
) {
    let mut rows: Vec<(NodeId, Value)> = Vec::new();
    for (col, decl) in g.attributes().iter().enumerate() {
        out.attr(*decl);
        rows.clear();
        g.attr_rows(col, &mut rows);
        for c in 0..copies {
            for (id, value) in &rows {
                out.set_attr(to(c, *id), col, value.clone());
            }
        }
    }
    let ids = g.node_ids();
    for c in 0..copies {
        for &id in &ids {
            for cell in g.node_cells(id) {
                out.cell(to(c, id), cell.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::catalog_conflicts;
    use glia_activation::algo::cycles::strongly_connected;
    use glia_activation::algo::{Adjacency, CategorySet};
    use glia_core::{Cell, CellPayload, CellTypeId, Edge};

    /// Every edge as (from, to, category, confidence, tier), in edge order.
    fn edge_rows(g: &MemSource) -> Vec<(NodeId, NodeId, EdgeCategoryId, Confidence, Tier)> {
        g.edges()
            .enumerate()
            .map(|(i, e)| (e.from, e.to, e.category, e.confidence, g.edge_tier(i, e)))
            .collect()
    }

    /// Every edge rendered: (render(from), render(to), category id, tier), in edge order.
    fn rendered_edges(g: &MemSource) -> Vec<(String, String, u32, Tier)> {
        g.edges()
            .enumerate()
            .map(|(i, e)| {
                (g.render_node(e.from), g.render_node(e.to), e.category.0, g.edge_tier(i, e))
            })
            .collect()
    }

    /// Every node rendered with its kind and `index` value, in node order.
    fn rendered_nodes(g: &MemSource) -> Vec<(String, Option<NodeKindId>, Option<Value>)> {
        let mut index = Vec::new();
        g.attr_rows(0, &mut index);
        g.node_ids()
            .into_iter()
            .map(|id| {
                let v = index.iter().find(|(n, _)| *n == id).map(|(_, v)| v.clone());
                (g.render_node(id), g.node_kind(id), v)
            })
            .collect()
    }

    fn spec(nodes: u32, edges: u32, seed: u64, salt: u64) -> GraphSpec {
        GraphSpec {
            nodes,
            edges,
            categories: vec!["CALLS", "USES", "IMPORTS"],
            tier_mix: [60, 30, 10],
            seed,
            salt,
        }
    }

    #[test]
    fn tables_are_valid() {
        assert_eq!(tables().validate(), Ok(()));
        let t = tables();
        assert_eq!(t.registries.kind_name(NodeKindId(3)), Some("ROUTE"));
        assert_eq!(t.registries.category_name(EdgeCategoryId(6)), Some("NEXT"));
        assert_eq!(t.carry_edges, &[category_id("CALLS"), category_id("USES")]);
        assert_eq!(category_id("HANDLED_BY"), EdgeCategoryId(4));
        assert_eq!(category_id("NOPE"), EdgeCategoryId(0));
    }

    #[test]
    fn lcg_graph_is_deterministic() {
        let s = spec(500, 10_000, 3, 0);
        let (a, b) = (lcg_graph(&s), lcg_graph(&s));
        assert_eq!(a.node_ids(), b.node_ids());
        assert_eq!(edge_rows(&a), edge_rows(&b));
        assert_eq!(rendered_nodes(&a), rendered_nodes(&b));
        assert_eq!(a.edges().count(), 10_000);
        assert!(catalog_conflicts(&a).is_empty());

        // Node i: id, kind, render and index as the module doc says.
        let ids = a.node_ids();
        assert_eq!(ids.len(), 500);
        assert_eq!(ids[42], node_id(0, 42));
        assert_eq!(ids[42], NodeId::from_parts("rules-test", RepoId(0), NodeKindId(3), "n42"));
        assert_eq!(a.node_kind(ids[42]), Some(NodeKindId(3)));
        assert_eq!(a.render_node(ids[42]), "n00042");
        assert_eq!(rendered_nodes(&a)[42].2, Some(Value::Int(42)));

        // The tier shares land within 2 points of the mix; confidence follows tier.
        let rows = edge_rows(&a);
        for (tier, pct) in [(Tier::Fact, 60), (Tier::Derived, 30), (Tier::Heuristic, 10)] {
            let got = rows.iter().filter(|r| r.4 == tier).count() as i64;
            assert!((got - pct * 100).abs() <= 200, "{tier:?}: {got} of 10000, want {pct}%");
            assert!(rows.iter().filter(|r| r.4 == tier).all(|r| r.3 == confidence(tier)));
        }
        // Every drawn category is one of the spec's, and each one occurs.
        for c in ["CALLS", "USES", "IMPORTS"] {
            assert!(rows.iter().any(|r| r.2 == category_id(c)), "{c}");
        }
        assert!(rows.iter().all(|r| [1, 2, 3].contains(&r.2.0)));

        // Another seed is another graph; another salt only other ids.
        assert_ne!(edge_rows(&lcg_graph(&spec(500, 10_000, 4, 0))), rows);
        let salted = lcg_graph(&spec(500, 10_000, 3, 5));
        assert_eq!(rendered_edges(&salted), rendered_edges(&a));
        assert_ne!(salted.node_ids(), ids);

        // No categories or no nodes: no edges.
        let mut none = spec(10, 50, 1, 0);
        none.categories.clear();
        assert_eq!(lcg_graph(&none).edges().count(), 0);
        assert_eq!(lcg_graph(&spec(0, 50, 1, 0)).edges().count(), 0);
    }

    #[test]
    fn resalted_keeps_the_rendered_graph() {
        let mut g = lcg_graph(&spec(300, 900, 3, 0));
        let ids = g.node_ids();
        let cell = Cell { kind: CellTypeId(9), payload: CellPayload::Text("x".into()) };
        g.cell(ids[7], cell.clone());
        let g2 = resalted(&g, 7);

        let mut old = ids.clone();
        old.sort_unstable_by_key(|id| id.0);
        let new = g2.node_ids();
        assert_eq!(new.len(), ids.len());
        assert!(new.iter().all(|id| old.binary_search_by_key(&id.0, |o| o.0).is_err()));
        assert!(g2.edges().all(|e| {
            old.binary_search_by_key(&e.from.0, |o| o.0).is_err()
                && old.binary_search_by_key(&e.to.0, |o| o.0).is_err()
        }));

        // Node order and edge order reversed, rendered values equal.
        let mut nodes = rendered_nodes(&g);
        nodes.reverse();
        assert_eq!(rendered_nodes(&g2), nodes);
        let mut edges = rendered_edges(&g);
        edges.reverse();
        assert_eq!(rendered_edges(&g2), edges);
        // So the multisets are equal too.
        let (mut a, mut b) = (rendered_edges(&g), rendered_edges(&g2));
        a.sort();
        b.sort();
        assert_eq!(a, b);
        // Every endpoint is a node of g2; cells travel with their node.
        assert!(g2.edges().all(|e| g2.node_kind(e.from).is_some() && g2.node_kind(e.to).is_some()));
        let seven = new.iter().find(|id| g2.render_node(**id) == "n00007");
        assert_eq!(seven.map(|id| g2.node_cells(*id).to_vec()), Some(vec![cell]));
        assert_eq!(g2.attributes(), g.attributes());
        assert_eq!(g2.tables(), g.tables());
        // Deterministic, and another salt is other ids again.
        assert_eq!(resalted(&g, 7).node_ids(), new);
        assert_ne!(resalted(&g, 8).node_ids(), new);
    }

    /// The spike's stream restated from its literal constants.
    fn spike_next(state: &mut u64) -> u32 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*state >> 33) as u32
    }

    #[test]
    fn scaled_matches_the_spike_arithmetic() {
        assert_eq!(scaled_size(1_000, 3_000, 10), (10_000, 30_300));
        // R1 section 1.4 on the 0.5.0 dump: `glia x10: 183870 nodes, 573053 edges`.
        assert_eq!(scaled_size(18_387, 56_738, 10), (183_870, 573_053));
        // The spike binary over a HEAD 22c65ee dump: `glia x10: 257680 nodes, 826755 edges`.
        assert_eq!(scaled_size(25_768, 81_857, 10), (257_680, 826_755));
        assert_eq!(scaled_size(5, 99, 1), (5, 99));

        let g = lcg_graph(&spec(1_000, 3_000, 3, 0));
        let s = scaled(&g, 10);
        let (ids, sids) = (g.node_ids(), s.node_ids());
        assert_eq!(sids.len(), 10_000);
        assert_eq!(s.edges().count(), 30_300);

        // Copy c of node v sits at c * n + v, kind, render and index unchanged.
        for (c, v) in [(0u32, 0usize), (3, 17), (9, 999)] {
            let id = sids[c as usize * 1_000 + v];
            let want = NodeId::from_parts(
                "rules-scaled",
                RepoId(u64::from(c)),
                NodeKindId(0),
                &format!("{:016x}", ids[v].0),
            );
            assert_eq!(id, want);
            assert_eq!(s.node_kind(id), g.node_kind(ids[v]));
            assert_eq!(s.render_node(id), g.render_node(ids[v]));
        }
        let mut index = Vec::new();
        s.attr_rows(0, &mut index);
        assert_eq!(index.len(), 10_000);
        assert_eq!(index[3_017], (sids[3_017], Value::Int(17)));

        // Copy c of edge e joins copy c of its endpoints, tier and confidence kept.
        let (ge, se) = (edge_rows(&g), edge_rows(&s));
        let at = |id: NodeId| ids.iter().position(|x| *x == id);
        for (c, j) in [(0usize, 0usize), (4, 1_234), (9, 2_999)] {
            let (f, t, cat, conf, tier) = ge[j];
            let (sf, st, scat, sconf, stier) = se[c * 3_000 + j];
            assert_eq!((scat, sconf, stier), (cat, conf, tier));
            assert_eq!(Some(sf), at(f).map(|p| sids[c * 1_000 + p]));
            assert_eq!(Some(st), at(t).map(|p| sids[c * 1_000 + p]));
        }

        // The first three cross edges are the reference stream's dense indexes.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for j in 0..3 {
            let from = spike_next(&mut state) as usize % 10_000;
            let to = spike_next(&mut state) as usize % 10_000;
            let (sf, st, cat, conf, tier) = se[30_000 + j];
            assert_eq!((sf, st), (sids[from], sids[to]), "cross edge {j}");
            assert_eq!((cat, conf, tier), (category_id("CALLS"), Confidence::Strong, Tier::Fact));
        }
        let mut lcg = Lcg(SCALE_SEED);
        let mut reference = SCALE_SEED;
        for _ in 0..1_000 {
            assert_eq!(lcg.next_u32(), spike_next(&mut reference));
        }

        // x0 is empty; x1 is a copy plus 1% cross edges.
        assert_eq!(scaled(&g, 0).node_ids().len(), 0);
        assert_eq!(scaled(&g, 0).edges().count(), 0);
        assert_eq!(scaled(&g, 1).edges().count(), 3_030);
    }

    #[test]
    fn scaled_copies_cells_and_dangling_endpoints() {
        let mut g = chain(3, "CALLS", &[Tier::Fact]);
        let ids = g.node_ids();
        let cell = Cell { kind: CellTypeId(1), payload: CellPayload::Text("body".into()) };
        g.cell(ids[1], cell.clone());
        let dangling = NodeId(0xdead);
        let edge = Edge::new(ids[2], dangling, category_id("USES"), Confidence::Medium);
        g.push_edge(edge.with_cell(cell.clone()), Tier::Heuristic);
        let s = scaled(&g, 2);
        let sids = s.node_ids();
        assert_eq!(sids.len(), 6);
        assert_eq!(s.node_cells(sids[1]), std::slice::from_ref(&cell));
        assert_eq!(s.node_cells(sids[4]), std::slice::from_ref(&cell));
        let edges: Vec<&Edge> = s.edges().collect();
        // 3 edges x 2 copies, plus 6 / 100 = 0 cross edges.
        assert_eq!(edges.len(), 6);
        for c in 0..2u32 {
            let e = edges[c as usize * 3 + 2];
            let want = NodeId::from_parts(
                "rules-scaled",
                RepoId(u64::from(c)),
                NodeKindId(0),
                "000000000000dead",
            );
            assert_eq!((e.from, e.to), (sids[c as usize * 3 + 2], want));
            assert_eq!(e.cells, vec![cell.clone()]);
            assert_eq!(s.edge_tier(c as usize * 3 + 2, e), Tier::Heuristic);
        }
    }

    #[test]
    fn clique_ring_is_strongly_connected() {
        let g = clique_ring(60, 120, "CALLS", 1);
        assert_eq!(g.node_ids().len(), 60);
        assert_eq!(g.edges().count(), 180);
        assert!(g.edges().all(|e| e.category == category_id("CALLS")));
        let adj = Adjacency::build(&g, &CategorySet::all());
        let comps = strongly_connected(&adj);
        assert_eq!(comps.len(), 1);
        assert_eq!(comps[0].len(), 60);
        // The ring's hops come first, in node order.
        let ids = g.node_ids();
        let first: Vec<_> = g.edges().take(60).map(|e| (e.from, e.to)).collect();
        let ring: Vec<_> = (0..60).map(|i| (ids[i], ids[(i + 1) % 60])).collect();
        assert_eq!(first, ring);
        assert_eq!(edge_rows(&g), edge_rows(&clique_ring(60, 120, "CALLS", 1)));
        assert_ne!(edge_rows(&g), edge_rows(&clique_ring(60, 120, "CALLS", 2)));

        let one = clique_ring(1, 0, "CALLS", 1);
        assert_eq!(strongly_connected(&Adjacency::build(&one, &CategorySet::all())).len(), 1);
        assert_eq!(clique_ring(0, 5, "CALLS", 1).edges().count(), 0);
    }

    #[test]
    fn chain_tiers_cycle() {
        let g = chain(5, "CALLS", &[Tier::Fact, Tier::Heuristic]);
        let hops = rendered_edges(&g);
        let calls = category_id("CALLS").0;
        assert_eq!(
            hops,
            vec![
                ("n00000".into(), "n00001".into(), calls, Tier::Fact),
                ("n00001".into(), "n00002".into(), calls, Tier::Heuristic),
                ("n00002".into(), "n00003".into(), calls, Tier::Fact),
                ("n00003".into(), "n00004".into(), calls, Tier::Heuristic),
            ]
        );
        assert_eq!(g.node_kind(node_id(0, 4)), Some(NodeKindId(1)));
        let plain = chain(3, "NEXT", &[]);
        assert!(edge_rows(&plain).iter().all(|r| r.4 == Tier::Derived && r.2 == EdgeCategoryId(6)));
        assert_eq!(chain(1, "CALLS", &[Tier::Fact]).edges().count(), 0);
        assert_eq!(chain(0, "CALLS", &[Tier::Fact]).node_ids().len(), 0);
    }
}
