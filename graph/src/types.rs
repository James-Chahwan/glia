//! Output types shared by every builder and resolver: the per-repo graph,
//! its symbol index, and the build error.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use glia_code_domain::{CallQualifier, CallSite, CodeNav, UnresolvedRef};
use glia_core::{Edge, Node, NodeId, RepoId, canonical_edge_cmp};

// ============================================================================
// Output graph
// ============================================================================

#[derive(Debug, Clone)]
pub struct RepoGraph {
    pub repo: RepoId,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    pub symbols: SymbolTable,
    /// Call sites left unresolved after cross-file resolution. Kept as a
    /// diagnostic surface (at v0.4.5 they also feed the dense-text `?` sigil).
    pub unresolved_calls: Vec<CallSite>,
    /// `UnresolvedRef`s the resolver couldn't bind. Same diagnostic role as
    /// `unresolved_calls`. v0.4.4 use case: gin route handler refs that point
    /// at packages the parser couldn't link to a known module.
    pub unresolved_refs: Vec<UnresolvedRef>,
    /// v0.4.13b — method ids tagged as property-style (read as `self.x`).
    /// Populated from `FileParse.properties`. Consumed by composition-path
    /// synthesis to filter method→class hops down to syntactically valid
    /// attribute reads.
    pub properties: HashSet<NodeId>,
}

/// Symbol index built during resolution. Everything keyed by node id so
/// consumers never re-parse qnames.
#[derive(Debug, Default, Clone)]
pub struct SymbolTable {
    /// Module qname (`"myapp::users"`) → module node id.
    pub module_by_qname: HashMap<String, NodeId>,
    /// Module node id → (top-level def name → def node id).
    /// Used for `from X import Y` resolution and for module-attribute calls.
    /// A TS / JS re-export barrel MODULE also holds the declarations it
    /// re-exports under their own names (`export * from './user.model'`
    /// passing `User`; DT.1q): each such pair already sits in its declaring
    /// module's table. A renamed or MODULE-valued re-export
    /// (`export { A as B }`, `export * as ns`) is never held here.
    pub module_symbols: HashMap<NodeId, HashMap<String, NodeId>>,
    /// Class node id → (method name → method node id).
    pub class_methods: HashMap<NodeId, HashMap<String, NodeId>>,
    /// INTERFACE node id → (method name → METHOD node id). Kept apart from
    /// `class_methods` on purpose: the HANDLED_BY global fallback
    /// (`unique_global_method`) scans `class_methods`, and an interface method
    /// sharing its implementation's name would turn every such lookup
    /// ambiguous and silently drop the route's HANDLED_BY edge (A6.6).
    /// Build-time only: the store does not persist it, so a graph loaded from
    /// a `.gmap` carries an empty table (resolution is finished before write).
    pub interface_methods: HashMap<NodeId, HashMap<String, NodeId>>,
    /// Module node id → (bound name in that module → target node id).
    /// Populated from resolved imports. Powers cross-file call resolution.
    pub module_import_bindings: HashMap<NodeId, HashMap<String, NodeId>>,
    /// Node id → the file MODULE that declared it, for every node whose
    /// declaring file's nav chain crosses a PACKAGE that another file also
    /// opens (a C# `namespace Shop.Orders` or a braced PHP `namespace
    /// App\Orders { }` in two files, CB.15). Such a PACKAGE is one node with
    /// one nav parent (its first file), so walking `parent_of` from a member
    /// of the second file reaches the wrong file; call resolution reads this
    /// entry instead (`calls::enclosing_home_module`) and resolves through the
    /// member's own file's `use` / `using` bindings. First file wins for an id
    /// two files both declare (a C# partial class). Empty for every language
    /// whose PACKAGEs are per file. Build-time only: the store does not
    /// persist it, so a graph loaded from a `.gmap` carries an empty table.
    pub home_module: HashMap<NodeId, NodeId>,
}

// ============================================================================
// Canonical order (GA.2)
// ============================================================================

/// What one [`RepoGraph::canonicalize`] moved: the entries whose index the
/// sort changed, for the build's `[canonical]` line. Zero when the graph was
/// already canonical.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CanonicalMoves {
    /// Intra edges whose index changed.
    pub edges: usize,
    /// `unresolved_calls` plus `unresolved_refs` entries whose index changed.
    pub unresolved: usize,
}

/// A node as the canonical order reads it: its `(qname, kind)` from the
/// graph's nav, `None` for an id the nav does not name (an endpoint held by
/// another graph), which sorts first.
type NodeKey<'a> = Option<(&'a str, u32)>;

fn node_key(nav: &CodeNav, id: NodeId) -> NodeKey<'_> {
    let qname = nav.qname_by_id.get(&id)?;
    let kind = nav.kind_by_id.get(&id).map_or(u32::MAX, |k| k.0);
    Some((qname.as_str(), kind))
}

impl RepoGraph {
    /// Put the graph in its canonical order, returning what moved.
    ///
    /// - `edges` by `(from (qname, kind), to (qname, kind))` read from the
    ///   graph's nav, then [`glia_core::canonical_edge_cmp`] (`from`, `to`,
    ///   `category`, confidence, cells);
    /// - `unresolved_calls` by `(from (qname, kind), line, qualifier)`, then
    ///   the caller's id;
    /// - `unresolved_refs` by `(from (qname, kind), from_module (qname,
    ///   kind), category, line, qualifier)`, then the two ids;
    ///
    /// the qualifier compared as `(variant index, its strings)`.
    ///
    /// Why (GA.2): every builder still emits in its own order (per file, per
    /// pass), so without this a graph's bytes depend on that order, and a
    /// fast path that re-resolves one file's body (appending that file's
    /// edges after everyone else's) could never write what a clean build
    /// writes. The engine calls it at the end of each group build, before the
    /// evidence fill decides its per-edge records (they are indexed by edge
    /// position), and on the region / project / docs graphs; the cross edges
    /// have [`crate::MergedGraph::sort_cross_edges`].
    ///
    /// The order leads with qnames, not ids, because an answer that lists
    /// edges in stored order (a trace's tree, a neighbour list) must not
    /// depend on where the tree is checked out: a [`NodeId`] hashes the
    /// [`RepoId`], a qname does not (DE.1, FD.22). Within one graph a
    /// `(qname, kind)` names one id, so the id compare only orders an endpoint
    /// the nav does not name. The edge order is total, so the result does not
    /// depend on the input order. The unresolved keys are total up to entries
    /// that are equal anyway (one caller, one line, one callee); the sorts are
    /// stable, so such ties keep their parser order.
    pub fn canonicalize(&mut self) -> CanonicalMoves {
        let nav = &self.nav;
        let edge_keys: Vec<(NodeKey<'_>, NodeKey<'_>)> = self
            .edges
            .iter()
            .map(|e| (node_key(nav, e.from), node_key(nav, e.to)))
            .collect();
        let edges = sort_counting(&mut self.edges, |i, a, j, b| {
            edge_keys[i]
                .cmp(&edge_keys[j])
                .then_with(|| canonical_edge_cmp(a, b))
        });
        let calls = sort_counting(&mut self.unresolved_calls, |_, a, _, b| {
            (node_key(nav, a.from), a.line, qualifier_key(&a.qualifier))
                .cmp(&(node_key(nav, b.from), b.line, qualifier_key(&b.qualifier)))
                .then_with(|| a.from.0.cmp(&b.from.0))
        });
        let refs = sort_counting(&mut self.unresolved_refs, |_, a, _, b| {
            unresolved_ref_key(nav, a)
                .cmp(&unresolved_ref_key(nav, b))
                .then_with(|| (a.from.0, a.from_module.0).cmp(&(b.from.0, b.from_module.0)))
        });
        CanonicalMoves {
            edges,
            unresolved: calls + refs,
        }
    }
}

/// A [`CallQualifier`] as `(variant index, first string, second string)`.
/// The match is exhaustive: a new variant must take its own index here.
fn qualifier_key(q: &CallQualifier) -> (u8, &str, &str) {
    match q {
        CallQualifier::Bare(n) => (0, n, ""),
        CallQualifier::SelfMethod(n) => (1, n, ""),
        CallQualifier::SuperMethod(n) => (2, n, ""),
        CallQualifier::Attribute { base, name } => (3, base, name),
        CallQualifier::ComplexReceiver { receiver, name } => (4, receiver, name),
    }
}

/// `unresolved_refs`' leading key: `(from, from_module, category, line,
/// qualifier)`, the nodes by [`node_key`].
type RefKey<'a> = (NodeKey<'a>, NodeKey<'a>, u32, u32, (u8, &'a str, &'a str));

fn unresolved_ref_key<'a>(nav: &'a CodeNav, r: &'a UnresolvedRef) -> RefKey<'a> {
    (
        node_key(nav, r.from),
        node_key(nav, r.from_module),
        r.category.0,
        r.line,
        qualifier_key(&r.qualifier),
    )
}

/// Stable-sort `v` by `cmp` (handed each entry with its index before the
/// sort, so a caller can read keys it computed per index) and return how many
/// entries changed index. An already sorted list costs one scan.
fn sort_counting<T>(v: &mut Vec<T>, cmp: impl Fn(usize, &T, usize, &T) -> Ordering) -> usize {
    if (1..v.len()).all(|i| cmp(i - 1, &v[i - 1], i, &v[i]) != Ordering::Greater) {
        return 0;
    }
    let mut order: Vec<usize> = (0..v.len()).collect();
    order.sort_by(|&a, &b| cmp(a, &v[a], b, &v[b]));
    let moved = order.iter().enumerate().filter(|(i, j)| i != *j).count();
    let mut slots: Vec<Option<T>> = v.drain(..).map(Some).collect();
    v.extend(
        order
            .iter()
            .filter_map(|&i| slots.get_mut(i).and_then(Option::take)),
    );
    moved
}

#[cfg(test)]
mod canonical_tests {
    use super::*;
    use glia_code_domain::{cell_type, edge_category, node_kind};
    use glia_core::{Cell, CellPayload, Confidence, EdgeCategoryId};

    /// Node ids whose numeric order is the reverse of their qname order, so
    /// a sort by id and a sort by qname disagree.
    const NODES: [(u64, &str); 5] = [
        (50, "app"),
        (40, "app::a"),
        (30, "app::b"),
        (20, "app::c"),
        (10, "app::d"),
    ];

    fn graph(edges: Vec<Edge>, calls: Vec<CallSite>, refs: Vec<UnresolvedRef>) -> RepoGraph {
        let mut nav = CodeNav::default();
        for (id, q) in NODES {
            nav.qname_by_id.insert(NodeId(id), q.to_string());
            let kind = if q == "app" {
                node_kind::MODULE
            } else {
                node_kind::FUNCTION
            };
            nav.kind_by_id.insert(NodeId(id), kind);
        }
        RepoGraph {
            repo: RepoId(7),
            nodes: Vec::new(),
            edges,
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: calls,
            unresolved_refs: refs,
            properties: HashSet::new(),
        }
    }

    fn edge(from: u64, to: u64, cat: EdgeCategoryId, line: Option<&str>) -> Edge {
        let mut e = Edge::new(NodeId(from), NodeId(to), cat, Confidence::Strong);
        if let Some(l) = line {
            e.cells.push(Cell {
                kind: cell_type::EVIDENCE,
                payload: CellPayload::Json(format!("{{\"line\":{l}}}")),
            });
        }
        e
    }

    fn site(from: u64, line: u32, q: CallQualifier) -> CallSite {
        CallSite {
            from: NodeId(from),
            qualifier: q,
            line,
        }
    }

    fn uref(
        from: u64,
        module: u64,
        cat: EdgeCategoryId,
        line: u32,
        q: CallQualifier,
    ) -> UnresolvedRef {
        UnresolvedRef {
            from: NodeId(from),
            from_module: NodeId(module),
            qualifier: q,
            category: cat,
            line,
        }
    }

    fn bare(n: &str) -> CallQualifier {
        CallQualifier::Bare(n.to_string())
    }

    fn sample() -> (Vec<Edge>, Vec<CallSite>, Vec<UnresolvedRef>) {
        let edges = vec![
            edge(30, 10, edge_category::CALLS, Some("4")),
            edge(50, 40, edge_category::DEFINES, None),
            edge(30, 10, edge_category::CALLS, Some("2")),
            edge(50, 30, edge_category::DEFINES, None),
            edge(40, 30, edge_category::USES, Some("9")),
            edge(50, 40, edge_category::CALLS, None),
            // An endpoint the nav does not name sorts first.
            edge(40, 99, edge_category::CALLS, None),
        ];
        let calls = vec![
            site(10, 3, bare("b")),
            site(
                10,
                3,
                CallQualifier::Attribute {
                    base: "x".into(),
                    name: "a".into(),
                },
            ),
            site(40, 9, CallQualifier::SelfMethod("m".into())),
            site(
                10,
                1,
                CallQualifier::ComplexReceiver {
                    receiver: "f()".into(),
                    name: "g".into(),
                },
            ),
            site(10, 3, bare("a")),
            site(10, 3, CallQualifier::SuperMethod("a".into())),
        ];
        let refs = vec![
            uref(20, 50, edge_category::HANDLED_BY, 2, bare("h")),
            uref(20, 50, edge_category::INHERITS_FROM, 2, bare("h")),
            uref(20, 40, edge_category::HANDLED_BY, 8, bare("h")),
            uref(40, 50, edge_category::USES, 0, bare("z")),
        ];
        (edges, calls, refs)
    }

    fn qn(g: &RepoGraph, id: NodeId) -> &str {
        g.nav.qname_by_id.get(&id).map_or("?", String::as_str)
    }

    /// Every emission order of one graph sorts to one form, led by qnames
    /// (never by the ids, which run the other way here); a second call moves
    /// nothing.
    #[test]
    fn any_emission_order_has_one_form() {
        let (edges, calls, refs) = sample();
        let mut base = graph(edges.clone(), calls.clone(), refs.clone());
        let moved = base.canonicalize();
        assert!(moved.edges > 0 && moved.unresolved > 0, "{moved:?}");
        assert_eq!(base.canonicalize(), CanonicalMoves::default());

        let order: Vec<(&str, &str, u32)> = base
            .edges
            .iter()
            .map(|e| (qn(&base, e.from), qn(&base, e.to), e.category.0))
            .collect();
        let want: Vec<(&str, &str, u32)> = vec![
            (
                "app",
                "app::a",
                edge_category::DEFINES.0.min(edge_category::CALLS.0),
            ),
            (
                "app",
                "app::a",
                edge_category::DEFINES.0.max(edge_category::CALLS.0),
            ),
            ("app", "app::b", edge_category::DEFINES.0),
            ("app::a", "?", edge_category::CALLS.0),
            ("app::a", "app::b", edge_category::USES.0),
            ("app::b", "app::d", edge_category::CALLS.0),
            ("app::b", "app::d", edge_category::CALLS.0),
        ];
        assert_eq!(order, want);
        // The two `b -> d` CALLS differ only in their cells: line 2 first.
        let lines: Vec<&str> = base
            .edges
            .iter()
            .filter(|e| e.from == NodeId(30))
            .flat_map(|e| e.cells.iter())
            .map(|c| match &c.payload {
                CellPayload::Json(s) => s.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(lines, vec!["{\"line\":2}", "{\"line\":4}"]);

        let calls_order: Vec<(&str, u32, (u8, &str, &str))> = base
            .unresolved_calls
            .iter()
            .map(|c| (qn(&base, c.from), c.line, qualifier_key(&c.qualifier)))
            .collect();
        assert_eq!(
            calls_order,
            vec![
                ("app::a", 9, (1, "m", "")),
                ("app::d", 1, (4, "f()", "g")),
                ("app::d", 3, (0, "a", "")),
                ("app::d", 3, (0, "b", "")),
                ("app::d", 3, (2, "a", "")),
                ("app::d", 3, (3, "x", "a")),
            ]
        );
        let refs_order: Vec<(&str, &str, u32)> = base
            .unresolved_refs
            .iter()
            .map(|r| (qn(&base, r.from), qn(&base, r.from_module), r.line))
            .collect();
        assert_eq!(
            refs_order,
            vec![
                ("app::a", "app", 0),
                ("app::c", "app", 2),
                ("app::c", "app", 2),
                ("app::c", "app::a", 8),
            ]
        );
        // One `(from, module, line)`: by category id.
        assert!(base.unresolved_refs[1].category.0 < base.unresolved_refs[2].category.0);

        for rot in 1..7 {
            let mut e = edges.clone();
            e.rotate_left(rot);
            e.reverse();
            let mut c = calls.clone();
            c.rotate_left(rot % calls.len());
            let mut r = refs.clone();
            r.rotate_right(rot % refs.len());
            let mut g = graph(e, c, r);
            g.canonicalize();
            assert_eq!(g.edges, base.edges, "rotation {rot}");
            assert_eq!(g.unresolved_calls, base.unresolved_calls, "rotation {rot}");
            assert_eq!(g.unresolved_refs, base.unresolved_refs, "rotation {rot}");
        }
    }

    /// The move count is the number of entries whose index changed.
    #[test]
    fn counts_moved_entries() {
        let by_value = |_: usize, a: &i32, _: usize, b: &i32| a.cmp(b);
        let mut v = vec![1, 2, 3, 5, 4];
        assert_eq!(sort_counting(&mut v, by_value), 2);
        assert_eq!(v, vec![1, 2, 3, 4, 5]);
        let mut v = vec![3, 1, 2];
        assert_eq!(sort_counting(&mut v, by_value), 3);
        let mut v: Vec<i32> = Vec::new();
        assert_eq!(sort_counting(&mut v, by_value), 0);
        // Stable: equal keys keep their order and count as unmoved.
        let mut v = vec![(1, 'b'), (0, 'z'), (1, 'a')];
        assert_eq!(
            sort_counting(&mut v, |_, a: &(i32, char), _, b| a.0.cmp(&b.0)),
            2
        );
        assert_eq!(v, vec![(0, 'z'), (1, 'b'), (1, 'a')]);
    }
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GraphError {
    #[error("module qname collision: {0}")]
    ModuleCollision(String),
}
