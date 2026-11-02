//! Who implements or extends X, and what X implements or extends (LD.7c): the
//! type hierarchy over IMPLEMENTS and INHERITS_FROM, transitive by default,
//! each row tiered `fact` / `derived` / `heuristic` by the weakest edge on its
//! path.
//!
//! Before this, "who implements X" meant a blast radius, which mixes the
//! heritage edges with every other carry edge (CALLS, INJECTS, ...) and says
//! neither which rows are implementations nor how sure the graph is of them.
//!
//! # The walk
//!
//! One `algo::reach::bfs` over an [`Adjacency`] of the two heritage
//! categories, from the resolved target: [`HierarchyDirection::Down`] walks
//! the edges backwards (implementors, subclasses, sub-interfaces),
//! [`HierarchyDirection::Up`] forwards (supertypes). The categories are walked
//! alike: C# emits interface-to-interface heritage as IMPLEMENTS, Java and
//! TypeScript as INHERITS_FROM, and `relation` reports what the graph holds.
//! Rows keep BFS discovery order; a dangling edge end (an id no graph names)
//! is walked but never a row.
//!
//! # Tiers
//!
//! A row's tier ([`Tier`], printed lowercase) is the weakest edge tier on its
//! BFS path from the target, and an edge's tier is read from its EVIDENCE by
//! [`why::edge_tier`], the reading `glia why`, `check`
//! and `glia query` share (FD.7b), never from its Confidence: a heritage
//! clause a parser read is `fact` whatever confidence it carries, an
//! inferred graph binding (LD.7b's Go method-set match, Medium) `derived`,
//! a name-only binding (`global_unique`, even Strong) `heuristic`, an edge
//! with no evidence `derived`. It is computed from each reached node's parent
//! during the walk, with an edge's tier read off the first edge in
//! `all_edges` order with its `(from, to, category)`.
//!
//! # METHOD targets
//!
//! A method is answered through its owner, the type that DEFINES it: the
//! owner's hierarchy is walked as above, and every reached type that DEFINES a
//! METHOD of the same name contributes it at that type's depth, relation and
//! tier, `via` naming the type it was reached through. That covers A6.6's
//! method-level IMPLEMENTS pairs (the same name pairing over direct
//! type-level edges), carries them through the hierarchy (a class implementing
//! a sub-interface implements the super-interface's method), adds overrides
//! along INHERITS_FROM, and gives a Go method pair its type-level DERIVED tier
//! (A6.6 stores a Go implicit pair at its type-level edge's Medium since
//! CA.3b, so the owner walk and the pair agree). The method's own
//! method-level edges are walked too, and rows the owner walk did not reach
//! are appended — the only rows for a method with no type owner. Pairing is by
//! name, as A6.6's is: the walk compares no signatures (a Go type-level edge
//! already passed the CA.3b signature gate), and a type that inherits the
//! method from a base outside the walk contributes nothing.
//!
//! # Target resolution
//!
//! Through `find`'s search (the LD.8a handoff), first restricted to the type
//! and method kinds so a Java file MODULE or a same-named function never wins
//! over the declaration; then over every kind, so a query that exactly names
//! some other node walks it (and says `no_edges`) instead of claiming it does
//! not exist. No exact match is `unknown_symbol`, with find's nearest rows as
//! the suggestions.
//!
//! # Precision (HC.4)
//!
//! Each row carries `precision`: the audited precision ([`crate::confidence`])
//! of the weakest link of its path, each heritage edge looked up by the
//! first edge in `all_edges` order with its `(from, to, category)`, as its
//! tier is; named by that edge's stratum, never a product. A METHOD
//! target's rows take their owner row's. It never moves a row or a tier.
//!
//! fired_on marker, once per call:
//! `[implementors] target=<q> direction=<Down|Up> transitive=<b> found=<n> fact=<f> derived=<d> heuristic=<h>`
//! — grep token `[implementors] target=` — then
//! `[implementors] tiers fact=<f> derived=<d> heuristic=<h> source=evidence`
//! (FD.7b), grep token `[implementors] tiers`.
//!
//! Module slot declared by L0.2; its API is reached as
//! `glia_engine::implementors::<item>`, never flattened into the crate
//! root.

use std::collections::{HashMap, HashSet};

use glia_activation::algo::reach::bfs;
use glia_activation::algo::{Adjacency, CategorySet, Walk};
use glia_activation::profile::Tier;
use glia_code_domain::{edge_category, node_kind};
use glia_core::{EdgeCategoryId, NodeId, NodeKindId};
use glia_graph::MergedGraph;

use crate::absence::{self, Answer};
use crate::answers::{Locator, entrypoint_reachable, live_marker};
use crate::confidence::{self, Annotated, Precision, Tally};
use crate::find::{self, FindOptions, FoundNode};
use crate::why;

/// The categories the walk follows, by `edge_category::name` spelling: the
/// absence's mechanisms and the caveat rows it carries.
const MECHANISMS: &[&str] = &["IMPLEMENTS", "INHERITS_FROM"];

/// The type declarations heritage edges join. A METHOD's owner is one of
/// these; a target resolves among these plus METHOD first.
const TYPE_KINDS: [NodeKindId; 9] = [
    node_kind::CLASS,
    node_kind::INTERFACE,
    node_kind::STRUCT,
    node_kind::ENUM,
    node_kind::SERVICE,
    node_kind::COMPONENT,
    node_kind::DIRECTIVE,
    node_kind::PIPE,
    node_kind::GUARD,
];

/// Which way [`implementors`] walks the hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HierarchyDirection {
    /// Implementors, subclasses and sub-interfaces: what reaches the target.
    Down,
    /// Supertypes: what the target implements or extends.
    Up,
}

/// One type (or method) in the target's hierarchy, located.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct Implementor {
    pub id: u64,
    pub qname: String,
    pub name: String,
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based (LD.1's `Locator`).
    pub line: Option<i64>,
    /// Reachable from an entrypoint (LD.6): `false` = likely dead.
    pub live: bool,
    /// `IMPLEMENTS` | `INHERITS_FROM`: the category of the edge that entered
    /// this row (for a METHOD target's rows, the edge that entered its owner).
    pub relation: &'static str,
    /// Hops from the target; 1 = direct.
    pub depth: usize,
    /// The node this row was reached through, when `depth > 1` (for a METHOD
    /// target's rows, the type the owner walk reached it through).
    pub via: Option<String>,
    /// `fact` | `derived` | `heuristic` ([`Tier::as_str`]): the weakest
    /// edge on the path, each edge read from its EVIDENCE (module doc).
    pub tier: &'static str,
    /// HC.4: the audited precision of the path's weakest link (module doc
    /// "Precision"); never read by the walk, the order or the tier. Not
    /// serialised once a surface drops it (`--no-precision`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<Precision>,
}

/// A walked row before it is located.
#[derive(Clone)]
struct Row {
    id: NodeId,
    depth: usize,
    relation: EdgeCategoryId,
    parent: NodeId,
    tier: Tier,
    /// The weakest link of the path's audited precisions (HC.4).
    precision: Option<Precision>,
}

/// An edge's tier and audited precision, keyed by `(from, to, category)`.
type Tiers = HashMap<(NodeId, NodeId, EdgeCategoryId), (Tier, Precision)>;

/// Who implements or extends `qname` ([`HierarchyDirection::Down`]), or what
/// it implements or extends (`Up`); every level when `transitive`, else the
/// direct ones. See the module doc for the walk, the tiers and METHOD
/// targets.
///
/// An empty answer's absence is `unknown_symbol` (nothing is named `qname`;
/// find's nearest qnames as suggestions) or `no_edges` (caveats narrowed to
/// the target's language). Each row's `live` is read off one
/// [`entrypoint_reachable`] walk; [`implementors_with_live`] takes the set.
pub fn implementors(
    merged: &MergedGraph,
    qname: &str,
    direction: HierarchyDirection,
    transitive: bool,
) -> Answer<Implementor> {
    implementors_with_live(
        merged,
        &entrypoint_reachable(merged),
        qname,
        direction,
        transitive,
    )
}

/// [`implementors`] over a live set the caller already holds (pyo3's
/// `PyGraph` caches one per graph): `live` must be [`entrypoint_reachable`]
/// of `merged`.
pub fn implementors_with_live(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    qname: &str,
    direction: HierarchyDirection,
    transitive: bool,
) -> Answer<Implementor> {
    let q = qname.trim();
    let answer = match resolve(merged, q) {
        Err(near) => {
            live_marker("implementors", 0, 0);
            Answer::from_results(Vec::new(), || {
                absence::unknown_symbol(merged, "implementors", qname, MECHANISMS, &near)
            })
        }
        Ok(target) => walk(merged, live, qname, target, direction, transitive),
    };
    let count = |t: Tier| {
        answer
            .results
            .iter()
            .filter(|r| r.tier == t.as_str())
            .count()
    };
    let (fact, derived, heuristic) = (
        count(Tier::Fact),
        count(Tier::Derived),
        count(Tier::Heuristic),
    );
    eprintln!(
        "[implementors] target={q} direction={direction:?} transitive={transitive} found={} fact={fact} derived={derived} heuristic={heuristic}",
        answer.results.len(),
    );
    eprintln!(
        "[implementors] tiers fact={fact} derived={derived} heuristic={heuristic} source=evidence"
    );
    answer
}

/// The target `q` names: an exact find hit among the type and method kinds,
/// else an exact hit of any kind; `Err` carries find's nearest rows.
fn resolve(merged: &MergedGraph, q: &str) -> Result<NodeId, Vec<FoundNode>> {
    let mut kinds = TYPE_KINDS.to_vec();
    kinds.push(node_kind::METHOD);
    let typed = FindOptions {
        top_k: 1,
        kinds: Some(kinds),
        ..FindOptions::default()
    };
    if let Some(r) = find::search(merged, q, &typed)
        .rows
        .first()
        .filter(|r| find::is_exact(r))
    {
        return Ok(NodeId(r.id));
    }
    let any = FindOptions {
        top_k: absence::SUGGESTIONS,
        ..FindOptions::default()
    };
    let near = find::search(merged, q, &any).rows;
    match near.first().filter(|r| find::is_exact(r)) {
        Some(r) => Ok(NodeId(r.id)),
        None => Err(near),
    }
}

fn walk(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    query: &str,
    target: NodeId,
    direction: HierarchyDirection,
    transitive: bool,
) -> Answer<Implementor> {
    let heritage = [edge_category::IMPLEMENTS, edge_category::INHERITS_FROM];
    let adj = Adjacency::build(merged, &CategorySet::of(&heritage));
    let dir = match direction {
        HierarchyDirection::Down => Walk::Backward,
        HierarchyDirection::Up => Walk::Forward,
    };
    let max_depth = if transitive { usize::MAX } else { 1 };

    let mut tier: Tiers = HashMap::new();
    let mut owner_of: HashMap<NodeId, NodeId> = HashMap::new();
    let mut methods_of: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let target_kind = kind_of(merged, target);
    for e in merged.all_edges() {
        if heritage.contains(&e.category) {
            tier.entry((e.from, e.to, e.category))
                .or_insert_with(|| (why::edge_tier(e).0, confidence::of_edge(e)));
        } else if target_kind == Some(node_kind::METHOD)
            && e.category == edge_category::DEFINES
            && kind_of(merged, e.to) == Some(node_kind::METHOD)
            && kind_of(merged, e.from).is_some_and(|k| TYPE_KINDS.contains(&k))
        {
            owner_of.entry(e.to).or_insert(e.from);
            let list = methods_of.entry(e.from).or_default();
            if !list.contains(&e.to) {
                list.push(e.to);
            }
        }
    }
    let hierarchy = |seed: NodeId| rows(&adj, &tier, seed, dir, max_depth);

    let owner = owner_of.get(&target).copied();
    let mut found: Vec<Row> = Vec::new();
    if let (Some(owner), Some(name)) = (owner, name_of(merged, target)) {
        for r in hierarchy(owner) {
            for &m in methods_of.get(&r.id).into_iter().flatten() {
                if m != target && name_of(merged, m) == Some(name) {
                    found.push(Row { id: m, ..r.clone() });
                }
            }
        }
    }
    let seen: HashSet<NodeId> = found.iter().map(|r| r.id).collect();
    found.extend(
        hierarchy(target)
            .into_iter()
            .filter(|r| !seen.contains(&r.id)),
    );

    let loc = Locator::new(merged);
    let results: Vec<Implementor> = found
        .into_iter()
        .filter_map(|r| {
            let at = loc.locate(r.id);
            if at.kind == "UNKNOWN" {
                return None;
            }
            let via = (r.depth > 1)
                .then(|| loc.locate(r.parent))
                .filter(|p| p.kind != "UNKNOWN")
                .map(|p| p.qname);
            Some(Implementor {
                id: at.id,
                qname: at.qname,
                name: at.name,
                kind: at.kind,
                file: at.file,
                line: at.line,
                live: live.contains(&r.id),
                relation: edge_category::name(r.relation),
                depth: r.depth,
                via,
                tier: r.tier.as_str(),
                precision: r.precision,
            })
        })
        .collect();
    live_marker(
        "implementors",
        results.len(),
        results.iter().filter(|r| r.live).count(),
    );
    Answer::from_results(results, || {
        let at = loc.locate(target);
        let what = match (direction, owner.map(|o| loc.locate(o).qname)) {
            (HierarchyDirection::Down, Some(o)) => format!(
                "nothing implements or overrides it: no heritage edge reaches it, and no type that implements or extends its owner `{o}` declares a `{}`",
                at.name
            ),
            (HierarchyDirection::Up, Some(o)) => format!(
                "it implements or overrides nothing: no heritage edge leaves it, and no supertype of its owner `{o}` declares a `{}`",
                at.name
            ),
            (HierarchyDirection::Down, None) => {
                "no IMPLEMENTS / INHERITS_FROM edge reaches it, so nothing implements or extends it"
                    .to_string()
            }
            (HierarchyDirection::Up, None) => {
                "no IMPLEMENTS / INHERITS_FROM edge leaves it, so it has no supertype".to_string()
            }
        };
        let note = format!("`{}` ({}) in this graph: {what}", at.qname, at.kind);
        absence::empty(
            merged,
            "implementors",
            query,
            "no_edges",
            note,
            MECHANISMS,
            at.file.as_deref(),
        )
    })
}

/// The BFS rows from `seed`, in discovery order, each tiered by the weakest
/// edge on its path: its parent's tier (the seed's is fact) and the entering
/// edge's evidence tier.
fn rows(adj: &Adjacency, tiers: &Tiers, seed: NodeId, dir: Walk, max_depth: usize) -> Vec<Row> {
    let mut tier_of: HashMap<NodeId, Tier> = HashMap::from([(seed, Tier::Fact)]);
    let mut prec_of: HashMap<NodeId, Precision> = HashMap::new();
    bfs(adj, &[seed], dir, max_depth)
        .reached
        .into_iter()
        .map(|r| {
            // Down walks an edge backwards: it runs from the reached node to
            // its parent. Every walked edge is in the map; a miss is heuristic.
            let key = match dir {
                Walk::Backward => (r.id, r.parent, r.via),
                _ => (r.parent, r.id, r.via),
            };
            let found = tiers.get(&key);
            let edge = found.map_or(Tier::Heuristic, |(t, _)| *t);
            let parent = tier_of.get(&r.parent).copied().unwrap_or(Tier::Fact);
            let tier = parent.weakest(edge);
            tier_of.insert(r.id, tier);
            // HC.4: the weakest link of the parent's path and this edge; an
            // edge the map misses has no stratum, so the path reads none.
            let precision = found.and_then(|(_, p)| {
                confidence::weakest(prec_of.get(&r.parent).into_iter().chain([p]))
            });
            if let Some(p) = &precision {
                prec_of.insert(r.id, p.clone());
            }
            Row {
                id: r.id,
                depth: r.depth,
                relation: r.via,
                parent: r.parent,
                tier,
                precision,
            }
        })
        .collect()
}

impl Annotated for Answer<Implementor> {
    fn drop_precision(&mut self) {
        for r in &mut self.results {
            r.precision = None;
        }
    }

    fn precision_tally(&self) -> Tally {
        let mut t = Tally::default();
        for r in &self.results {
            t.add(r.precision.as_ref());
        }
        t
    }
}

/// The kind the first graph (in `merged.graphs` order) naming `id` records.
fn kind_of(merged: &MergedGraph, id: NodeId) -> Option<NodeKindId> {
    merged
        .graphs
        .iter()
        .find_map(|g| g.nav.kind_by_id.get(&id).copied())
}

/// The simple name the first graph naming `id` records.
fn name_of(merged: &MergedGraph, id: NodeId) -> Option<&str> {
    merged
        .graphs
        .iter()
        .find_map(|g| g.nav.name_by_id.get(&id).map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::evidence::{Evidence, attach};
    use glia_core::{Confidence, Edge};

    fn heritage(c: Confidence, ev: Evidence) -> Edge {
        let mut e = Edge::new(NodeId(1), NodeId(2), edge_category::IMPLEMENTS, c);
        attach(&mut e, ev);
        e
    }

    /// FD.7b: the edge tier is why's evidence reading, not the confidence.
    #[test]
    fn an_edge_is_tiered_by_its_evidence() {
        let name_only = heritage(
            Confidence::Strong,
            Evidence::emitter("graph:refs")
                .rule("global_unique")
                .at("a.ts", 3),
        );
        assert_eq!(why::edge_tier(&name_only).0, Tier::Heuristic);
        let parsed = heritage(
            Confidence::Medium,
            Evidence::emitter("parser:typescript")
                .rule("implements")
                .at("a.ts", 3),
        );
        assert_eq!(why::edge_tier(&parsed).0, Tier::Fact);
        assert_eq!(Tier::Fact.weakest(Tier::Derived).as_str(), "derived");
        assert_eq!(
            Tier::Derived.weakest(Tier::Fact),
            Tier::Derived,
            "a row is as weak as its weakest edge"
        );
    }
}
