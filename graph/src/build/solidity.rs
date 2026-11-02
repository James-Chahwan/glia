//! HQ.3: Solidity emits of an event the emitting contract does not declare.
//!
//! The Solidity parser draws `emit E(..)` of an event its own contract body
//! declares as a direct EVENT_FLOWS edge. Any other emit is an EVENT_FLOWS
//! [`UnresolvedRef`] at the emit's row: `Bare(E)` for an unqualified
//! `emit E(..)` (in Solidity, an event a base contract or interface
//! declares) and `Attribute { base: I, name: E }` for a qualified
//! `emit I.E(..)` (the event `E` of the type `I`, a base or not).
//! [`resolve_emit_refs`] binds them after the stitch, once `resolve_refs`
//! has drawn the heritage edges:
//!
//! - `Bare(E)`: Solidity's own lookup, level by level up the emitting
//!   contract's bases (every INHERITS_FROM / IMPLEMENTS edge whose two ends
//!   are CLASS or INTERFACE nodes: contract `is` interface is IMPLEMENTS,
//!   contract `is` contract and interface `is` interface INHERITS_FROM),
//!   from its direct bases, at most [`MAX_INHERIT_LEVELS`] levels, each base
//!   visited once: the first level at which a base has an EVENT_EMITTER child
//!   named `E` decides. One distinct event binds; two (two bases at one level
//!   each declaring `E`) are ambiguous and bind nothing; none (an event a base
//!   outside the repo declares, an OpenZeppelin ERC20 `Transfer`) stays
//!   unresolved. Rule `inherited_event`.
//! - `Attribute { I, E }`: the type `I` is the emitting contract itself, else
//!   the first base named `I` on the same walk, else the one CLASS /
//!   INTERFACE / PACKAGE (a library) the graph names `I` (two are ambiguous);
//!   its EVENT_EMITTER child `E` binds. Rule `qualified_event`.
//!
//! Evidence: emitter `graph:solidity`, at the emit's row. A heritage edge
//! bound by a name alone ([`NAME_ONLY_RULES`]; at 0.5.7 every cross-file
//! Solidity heritage edge is `graph:refs` `global_unique`, since a Solidity
//! `import "./X.sol"` binds no names) vouches only for a name match, so an
//! event reached through one, or a qualified type found by unique name, is
//! `inherited_event_by_name` / `qualified_event_by_name`, which `why` reads
//! as heuristic. An emit never binds by its event name alone.
//!
//! Deterministic: refs are bound in their recorded order, every map is only
//! probed or read into sorted, deduped lists, and every outcome is
//! order-free (a set of distinct targets per level).
//!
//! fired_on, once per graph holding an EVENT_FLOWS ref:
//! `[sol-events] bound=<b> (inherited=<i> qualified=<q> by_name=<n>)
//! ambiguous=<a> unbound=<u>`; `by_name` counts the bound edges whose rule is
//! a `_by_name` one.

use std::collections::{BTreeMap, BTreeSet};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{CallQualifier, UnresolvedRef, edge_category, node_kind};
use glia_core::{NodeId, NodeKindId};

use crate::calls::{MAX_INHERIT_LEVELS, push_edge};
use crate::types::RepoGraph;

/// The emitter of every edge this pass draws.
const EMITTER: &str = "graph:solidity";

/// `(emitter, rule)` pairs that bind a heritage edge by a name alone: the
/// graph-stage rows of the engine's `why::NAME_ONLY_RULES` (the engine
/// cannot be a dependency of this crate), so an edge `why` reads heuristic
/// makes the event it leads to a `_by_name` binding here.
const NAME_ONLY_RULES: &[(&str, &str)] = &[
    ("graph:refs", "global_unique"),
    ("graph:refs", "global_unique_method"),
    ("graph:imports", "tail_unique"),
    ("graph:rust_paths", "unique_in_crate"),
    ("graph:rust_paths", "tail_unique"),
];

/// What [`resolve_emit_refs`] did to one graph, for the `[sol-events]`
/// marker.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct EmitBinds {
    /// `emit E(..)` refs bound through the contract's bases.
    pub(crate) inherited: usize,
    /// `emit I.E(..)` refs bound to `I`'s event.
    pub(crate) qualified: usize,
    /// The bound edges whose rule is a `_by_name` one.
    pub(crate) by_name: usize,
    /// Refs that found two distinct events (or types) at one step.
    pub(crate) ambiguous: usize,
    /// Refs that found nothing in the graph.
    pub(crate) unbound: usize,
}

impl EmitBinds {
    /// The `[sol-events] bound=` line, `None` when the graph held no
    /// EVENT_FLOWS ref.
    pub(crate) fn marker(&self) -> Option<String> {
        let bound = self.inherited + self.qualified;
        (bound + self.ambiguous + self.unbound > 0).then(|| {
            format!(
                "[sol-events] bound={bound} (inherited={} qualified={} by_name={}) \
                 ambiguous={} unbound={}",
                self.inherited, self.qualified, self.by_name, self.ambiguous, self.unbound
            )
        })
    }
}

/// One step of a lookup: a single answer (with whether it rests on a
/// name-only binding), two or more distinct answers, or nothing.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    One(NodeId, bool),
    Ambiguous,
    Nothing,
}

/// A type's direct bases, each with whether the edge to it is a name-only
/// binding (several edges to one base: name-only only when every one is).
type Bases = BTreeMap<u64, Vec<(NodeId, bool)>>;

/// Bind the graph's EVENT_FLOWS refs (module docs) and print the
/// `[sol-events] bound=` line once when it held any.
pub(crate) fn resolve_emit_refs(g: &mut RepoGraph) {
    let binds = bind_emit_refs(g);
    if let Some(line) = binds.marker() {
        eprintln!("{line}");
    }
}

/// [`resolve_emit_refs`] without its stderr line: takes the EVENT_FLOWS refs
/// out of `g.unresolved_refs` (the others keep their order), draws an edge
/// per bound ref and puts every ref that bound nothing back, in its order.
pub(crate) fn bind_emit_refs(g: &mut RepoGraph) -> EmitBinds {
    let mut binds = EmitBinds::default();
    if !g
        .unresolved_refs
        .iter()
        .any(|r| r.category == edge_category::EVENT_FLOWS)
    {
        return binds;
    }
    let (emits, rest): (Vec<UnresolvedRef>, Vec<UnresolvedRef>) =
        std::mem::take(&mut g.unresolved_refs)
            .into_iter()
            .partition(|r| r.category == edge_category::EVENT_FLOWS);
    g.unresolved_refs = rest;
    let bases = bases_of(g);
    let mut types: Option<BTreeMap<String, Vec<NodeId>>> = None;
    let mut edges: Vec<(NodeId, NodeId, &'static str, u32)> = Vec::new();
    let mut kept: Vec<UnresolvedRef> = Vec::new();
    for r in emits {
        let owner = g.nav.parent_of.get(&r.from).copied();
        let found = match (&r.qualifier, owner) {
            (CallQualifier::Bare(event), Some(owner)) => {
                Some((inherited_event(g, &bases, owner, event), true))
            }
            (CallQualifier::Attribute { base, name }, Some(owner)) => {
                let index = types.get_or_insert_with(|| types_by_name(g));
                Some((qualified_event(g, &bases, index, owner, base, name), false))
            }
            _ => None,
        };
        match found {
            Some((Found::One(event, by_name), inherited)) => {
                let rule = match (inherited, by_name) {
                    (true, false) => "inherited_event",
                    (true, true) => "inherited_event_by_name",
                    (false, false) => "qualified_event",
                    (false, true) => "qualified_event_by_name",
                };
                if inherited {
                    binds.inherited += 1;
                } else {
                    binds.qualified += 1;
                }
                binds.by_name += usize::from(by_name);
                edges.push((r.from, event, rule, r.line));
            }
            Some((Found::Ambiguous, _)) => {
                binds.ambiguous += 1;
                kept.push(r);
            }
            Some((Found::Nothing, _)) | None => {
                binds.unbound += 1;
                kept.push(r);
            }
        }
    }
    for (from, to, rule, line) in edges {
        let ev = Evidence::emitter(EMITTER).rule(rule).line(line);
        push_edge(g, from, to, edge_category::EVENT_FLOWS, ev);
    }
    g.unresolved_refs.extend(kept);
    binds
}

/// True for the kinds a Solidity heritage list joins: a contract (CLASS)
/// and an interface (INTERFACE).
fn is_heritage_end(g: &RepoGraph, id: &NodeId) -> bool {
    matches!(
        g.nav.kind_by_id.get(id).copied(),
        Some(k) if k == node_kind::CLASS || k == node_kind::INTERFACE
    )
}

/// True when `e`'s EVIDENCE names a binding by a name alone (or names
/// nothing, so it vouches for nothing).
fn name_only(e: &glia_core::Edge) -> bool {
    match Evidence::of(e) {
        Some(ev) => NAME_ONLY_RULES
            .iter()
            .any(|&(em, rule)| em == ev.emitter && ev.rule.as_deref() == Some(rule)),
        None => true,
    }
}

/// Every CLASS / INTERFACE -> its direct bases, from the INHERITS_FROM and
/// IMPLEMENTS edges whose two ends are CLASS or INTERFACE nodes, keyed by
/// the subclass id, each list sorted by id and deduped (a base reached by a
/// fact edge and a name-only edge is reached by a fact).
fn bases_of(g: &RepoGraph) -> Bases {
    let mut map: Bases = BTreeMap::new();
    for e in &g.edges {
        if (e.category == edge_category::INHERITS_FROM || e.category == edge_category::IMPLEMENTS)
            && e.from != e.to
            && is_heritage_end(g, &e.from)
            && is_heritage_end(g, &e.to)
        {
            map.entry(e.from.0).or_default().push((e.to, name_only(e)));
        }
    }
    for bases in map.values_mut() {
        bases.sort_unstable_by_key(|(b, by_name)| (b.0, *by_name));
        // Sorted with `false` first, so the kept copy of a base is its
        // fact-edge one when it has one.
        bases.dedup_by_key(|(b, _)| b.0);
    }
    map
}

/// The levels of `owner`'s bases, nearest first: each level's distinct
/// bases with whether every path to it crossed a name-only edge, each base
/// in the first level that reaches it, `owner` never.
fn base_levels(bases: &Bases, owner: NodeId) -> Vec<Vec<(NodeId, bool)>> {
    let mut visited: BTreeSet<u64> = BTreeSet::new();
    visited.insert(owner.0);
    let mut levels = Vec::new();
    let mut level: Vec<(NodeId, bool)> = bases.get(&owner.0).cloned().unwrap_or_default();
    for _ in 0..MAX_INHERIT_LEVELS {
        level.retain(|(b, _)| !visited.contains(&b.0));
        if level.is_empty() {
            break;
        }
        for (b, _) in &level {
            visited.insert(b.0);
        }
        let mut next: BTreeMap<u64, (NodeId, bool)> = BTreeMap::new();
        for (b, by_name) in &level {
            for (n, edge_by_name) in bases.get(&b.0).into_iter().flatten() {
                if visited.contains(&n.0) {
                    continue;
                }
                let via = *by_name || *edge_by_name;
                next.entry(n.0)
                    .and_modify(|(_, flag)| *flag = *flag && via)
                    .or_insert((*n, via));
            }
        }
        levels.push(std::mem::take(&mut level));
        level = next.into_values().collect();
    }
    levels
}

/// The EVENT_EMITTER children of `ty` named `name`, distinct, sorted.
fn events_named(g: &RepoGraph, ty: NodeId, name: &str) -> Vec<NodeId> {
    let mut hits: Vec<NodeId> = g
        .nav
        .children_of
        .get(&ty)
        .into_iter()
        .flatten()
        .filter(|c| {
            g.nav.kind_by_id.get(c) == Some(&node_kind::EVENT_EMITTER)
                && g.nav.name_by_id.get(c).map(String::as_str) == Some(name)
        })
        .copied()
        .collect();
    hits.sort_unstable_by_key(|h| h.0);
    hits.dedup();
    hits
}

/// `emit E(..)` from a member of `owner`: the first level of `owner`'s bases
/// at which a base declares the event `E` decides.
fn inherited_event(g: &RepoGraph, bases: &Bases, owner: NodeId, event: &str) -> Found {
    for level in base_levels(bases, owner) {
        let mut hits: BTreeMap<u64, (NodeId, bool)> = BTreeMap::new();
        for (base, by_name) in level {
            for e in events_named(g, base, event) {
                hits.entry(e.0)
                    .and_modify(|(_, flag)| *flag = *flag && by_name)
                    .or_insert((e, by_name));
            }
        }
        let mut hits = hits.into_values();
        match (hits.next(), hits.next()) {
            (Some((e, by_name)), None) => return Found::One(e, by_name),
            (Some(_), Some(_)) => return Found::Ambiguous,
            _ => {}
        }
    }
    Found::Nothing
}

/// The CLASS / INTERFACE / PACKAGE nodes of the graph by their name, each
/// list sorted by id and deduped (only its length and sole member are read).
fn types_by_name(g: &RepoGraph) -> BTreeMap<String, Vec<NodeId>> {
    let type_kinds: [NodeKindId; 3] = [node_kind::CLASS, node_kind::INTERFACE, node_kind::PACKAGE];
    let mut map: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
    for (id, kind) in &g.nav.kind_by_id {
        if type_kinds.contains(kind)
            && let Some(name) = g.nav.name_by_id.get(id)
        {
            map.entry(name.clone()).or_default().push(*id);
        }
    }
    for ids in map.values_mut() {
        ids.sort_unstable_by_key(|i| i.0);
        ids.dedup();
    }
    map
}

/// `emit I.E(..)` from a member of `owner`: the type `I` (the owner itself,
/// else the first base named `I` up `owner`'s bases, else the one type the
/// graph names `I`) and its event `E`.
fn qualified_event(
    g: &RepoGraph,
    bases: &Bases,
    types: &BTreeMap<String, Vec<NodeId>>,
    owner: NodeId,
    ty: &str,
    event: &str,
) -> Found {
    let named = |id: &NodeId| g.nav.name_by_id.get(id).map(String::as_str) == Some(ty);
    let mut found = if named(&owner) {
        Found::One(owner, false)
    } else {
        Found::Nothing
    };
    if found == Found::Nothing {
        for level in base_levels(bases, owner) {
            let hits: Vec<(NodeId, bool)> = level.into_iter().filter(|(b, _)| named(b)).collect();
            match hits.as_slice() {
                [] => continue,
                [(b, by_name)] => found = Found::One(*b, *by_name),
                _ => found = Found::Ambiguous,
            }
            break;
        }
    }
    if found == Found::Nothing {
        found = match types.get(ty).map(Vec::as_slice) {
            Some([one]) => Found::One(*one, true),
            Some([_, _, ..]) => Found::Ambiguous,
            _ => Found::Nothing,
        };
    }
    match found {
        Found::One(ty_id, by_name) => match events_named(g, ty_id, event).as_slice() {
            [one] => Found::One(*one, by_name),
            [] => Found::Nothing,
            _ => Found::Ambiguous,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::build_typescript;
    use glia_code_domain::{CodeNav, FileParse, GRAPH_TYPE};
    use glia_core::{Confidence, Node, RepoId};
    use std::collections::HashSet;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://solidity-events")
    }

    /// One parsed Solidity file built by hand: declarations and refs.
    struct Sol {
        module: NodeId,
        nav: CodeNav,
        nodes: Vec<Node>,
        refs: Vec<UnresolvedRef>,
    }

    impl Sol {
        fn new(module_qname: &str) -> Self {
            let mut s = Sol {
                module: NodeId(0),
                nav: CodeNav::default(),
                nodes: vec![],
                refs: vec![],
            };
            s.module = s.add(node_kind::MODULE, module_qname, None);
            s
        }

        fn add(&mut self, kind: NodeKindId, qname: &str, parent: Option<NodeId>) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            self.nav.record(id, name, qname, kind, parent);
            self.nodes.push(Node {
                id,
                repo: repo(),
                confidence: Confidence::Strong,
                cells: vec![],
            });
            id
        }

        /// A contract / interface / library `name` of this file.
        fn ty(&mut self, kind: NodeKindId, name: &str) -> NodeId {
            let q = format!("{}::{name}", self.qname(self.module));
            let m = self.module;
            self.add(kind, &q, Some(m))
        }

        /// A member `name` (of `kind`) of the type `ty`.
        fn member(&mut self, ty: NodeId, kind: NodeKindId, name: &str) -> NodeId {
            let q = format!("{}::{name}", self.qname(ty));
            self.add(kind, &q, Some(ty))
        }

        fn qname(&self, id: NodeId) -> String {
            self.nav.qname_by_id.get(&id).cloned().unwrap_or_default()
        }

        /// `ty is base` (heritage ref, bound by `resolve_refs`).
        fn is(&mut self, ty: NodeId, base: &str, category: glia_core::EdgeCategoryId) {
            self.refs.push(UnresolvedRef {
                from: ty,
                from_module: self.module,
                qualifier: CallQualifier::Bare(base.to_string()),
                category,
                line: 0,
            });
        }

        fn emit(&mut self, from: NodeId, qualifier: CallQualifier, line: u32) {
            self.refs.push(UnresolvedRef {
                from,
                from_module: self.module,
                qualifier,
                category: edge_category::EVENT_FLOWS,
                line,
            });
        }

        fn file(self) -> FileParse {
            FileParse {
                nodes: self.nodes,
                edges: vec![],
                imports: vec![],
                calls: vec![],
                refs: self.refs,
                nav: self.nav,
                properties: HashSet::new(),
            }
        }
    }

    fn bare(e: &str) -> CallQualifier {
        CallQualifier::Bare(e.to_string())
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute {
            base: base.to_string(),
            name: name.to_string(),
        }
    }

    fn build(files: Vec<Sol>) -> (RepoGraph, EmitBinds) {
        let parses = files.into_iter().map(Sol::file).collect();
        let mut g = build_typescript(repo(), parses, |_, _| None).unwrap();
        // The build already ran the pass; a second run reads the refs it
        // left (and binds nothing new).
        let binds = bind_emit_refs(&mut g);
        (g, binds)
    }

    /// The EVENT_FLOWS edges as `(from qname, to qname, rule, line)`, sorted.
    fn flows(g: &RepoGraph) -> Vec<(String, String, String, u32)> {
        let q = |id: &NodeId| g.nav.qname_by_id.get(id).cloned().unwrap_or_default();
        let mut out: Vec<_> = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::EVENT_FLOWS)
            .map(|e| {
                let ev = Evidence::of(e).expect("evidence");
                assert_eq!(ev.emitter, EMITTER);
                (
                    q(&e.from),
                    q(&e.to),
                    ev.rule.unwrap_or_default(),
                    ev.line.unwrap_or(u32::MAX),
                )
            })
            .collect();
        out.sort();
        out
    }

    fn row(from: &str, to: &str, rule: &str, line: u32) -> (String, String, String, u32) {
        (from.into(), to.into(), rule.into(), line)
    }

    /// `contract Guestbook is IGuestbook` across two files (the heritage
    /// binds by unique name) and in one file (by the module's symbols).
    #[test]
    fn an_interface_event_binds_through_implements() {
        let mut i = Sol::new("src::IGuestbook");
        let ig = i.ty(node_kind::INTERFACE, "IGuestbook");
        i.member(ig, node_kind::EVENT_EMITTER, "Signed");
        let mut c = Sol::new("src::Guestbook");
        let gb = c.ty(node_kind::CLASS, "Guestbook");
        c.is(gb, "IGuestbook", edge_category::IMPLEMENTS);
        let sign = c.member(gb, node_kind::METHOD, "sign");
        c.emit(sign, bare("Signed"), 7);
        let (g, _) = build(vec![i, c]);
        assert_eq!(
            flows(&g),
            [row(
                "src::Guestbook::Guestbook::sign",
                "src::IGuestbook::IGuestbook::Signed",
                "inherited_event_by_name",
                7
            )]
        );
        assert!(g.unresolved_refs.is_empty(), "{:?}", g.unresolved_refs);

        // The same shape in one file: the heritage edge is a module-symbol
        // binding, so the event binds as a fact.
        let mut one = Sol::new("src::Guestbook");
        let ig = one.ty(node_kind::INTERFACE, "IGuestbook");
        one.member(ig, node_kind::EVENT_EMITTER, "Signed");
        let gb = one.ty(node_kind::CLASS, "Guestbook");
        one.is(gb, "IGuestbook", edge_category::IMPLEMENTS);
        let sign = one.member(gb, node_kind::METHOD, "sign");
        one.emit(sign, bare("Signed"), 9);
        let (g, _) = build(vec![one]);
        assert_eq!(
            flows(&g),
            [row(
                "src::Guestbook::Guestbook::sign",
                "src::Guestbook::IGuestbook::Signed",
                "inherited_event",
                9
            )]
        );
    }

    /// contract -> base contract -> interface: two levels up.
    #[test]
    fn an_event_two_levels_up_binds() {
        let mut f = Sol::new("src::All");
        let ib = f.ty(node_kind::INTERFACE, "IBook");
        f.member(ib, node_kind::EVENT_EMITTER, "Signed");
        let base = f.ty(node_kind::CLASS, "BookBase");
        f.is(base, "IBook", edge_category::IMPLEMENTS);
        let book = f.ty(node_kind::CLASS, "Book");
        f.is(book, "BookBase", edge_category::INHERITS_FROM);
        let sign = f.member(book, node_kind::METHOD, "sign");
        f.emit(sign, bare("Signed"), 3);
        let (g, _) = build(vec![f]);
        assert_eq!(
            flows(&g),
            [row(
                "src::All::Book::sign",
                "src::All::IBook::Signed",
                "inherited_event",
                3
            )]
        );
    }

    /// A diamond (two bases sharing one interface) reaches one event: one
    /// edge. Two bases at one level each declaring the event: no edge.
    #[test]
    fn a_diamond_binds_once_and_two_declarers_are_ambiguous() {
        let mut f = Sol::new("src::D");
        let ib = f.ty(node_kind::INTERFACE, "IBook");
        f.member(ib, node_kind::EVENT_EMITTER, "Signed");
        let a = f.ty(node_kind::CLASS, "A");
        f.is(a, "IBook", edge_category::IMPLEMENTS);
        let b = f.ty(node_kind::CLASS, "B");
        f.is(b, "IBook", edge_category::IMPLEMENTS);
        let c = f.ty(node_kind::CLASS, "C");
        f.is(c, "A", edge_category::INHERITS_FROM);
        f.is(c, "B", edge_category::INHERITS_FROM);
        let sign = f.member(c, node_kind::METHOD, "sign");
        f.emit(sign, bare("Signed"), 1);
        let (g, _) = build(vec![f]);
        assert_eq!(
            flows(&g),
            [row(
                "src::D::C::sign",
                "src::D::IBook::Signed",
                "inherited_event",
                1
            )]
        );

        let mut f = Sol::new("src::E");
        let ia = f.ty(node_kind::INTERFACE, "IA");
        f.member(ia, node_kind::EVENT_EMITTER, "Signed");
        let ib = f.ty(node_kind::INTERFACE, "IB");
        f.member(ib, node_kind::EVENT_EMITTER, "Signed");
        let c = f.ty(node_kind::CLASS, "C");
        f.is(c, "IA", edge_category::IMPLEMENTS);
        f.is(c, "IB", edge_category::IMPLEMENTS);
        let sign = f.member(c, node_kind::METHOD, "sign");
        f.emit(sign, bare("Signed"), 1);
        let (g, binds) = build(vec![f]);
        assert!(flows(&g).is_empty());
        assert_eq!(binds.ambiguous, 1);
        assert_eq!(g.unresolved_refs.len(), 1);
    }

    /// `emit IGuestbook.Signed(..)` from a contract with no heritage binds the
    /// type of that name, by unique name; through a base it binds as the
    /// base's heritage edge does; a missing event and an external base bind
    /// nothing.
    #[test]
    fn qualified_emits_bind_the_named_type() {
        let mut i = Sol::new("src::IGuestbook");
        let ig = i.ty(node_kind::INTERFACE, "IGuestbook");
        i.member(ig, node_kind::EVENT_EMITTER, "Signed");
        i.member(ig, node_kind::EVENT_EMITTER, "Cleared");
        let mut t = Sol::new("test::GuestbookTest");
        let gt = t.ty(node_kind::CLASS, "GuestbookTest");
        let test = t.member(gt, node_kind::METHOD, "testSign");
        t.emit(test, attr("IGuestbook", "Signed"), 11);
        t.emit(test, attr("IGuestbook", "Missing"), 12);
        t.emit(test, attr("IERC20", "Transfer"), 13);
        t.emit(test, bare("Transfer"), 14);
        let mut f = Sol::new("src::Book");
        let ib = f.ty(node_kind::INTERFACE, "IBook");
        f.member(ib, node_kind::EVENT_EMITTER, "Cleared");
        let book = f.ty(node_kind::CLASS, "Book");
        f.is(book, "IBook", edge_category::IMPLEMENTS);
        f.member(book, node_kind::EVENT_EMITTER, "Opened");
        let clear = f.member(book, node_kind::METHOD, "clear");
        f.emit(clear, attr("IBook", "Cleared"), 4);
        f.emit(clear, attr("Book", "Opened"), 5);
        let (g, binds) = build(vec![i, t, f]);
        assert_eq!(
            flows(&g),
            [
                row(
                    "src::Book::Book::clear",
                    "src::Book::Book::Opened",
                    "qualified_event",
                    5
                ),
                row(
                    "src::Book::Book::clear",
                    "src::Book::IBook::Cleared",
                    "qualified_event",
                    4
                ),
                row(
                    "test::GuestbookTest::GuestbookTest::testSign",
                    "src::IGuestbook::IGuestbook::Signed",
                    "qualified_event_by_name",
                    11
                ),
            ]
        );
        let left: Vec<u32> = g.unresolved_refs.iter().map(|r| r.line).collect();
        assert_eq!(left, [12, 13, 14]);
        assert_eq!(binds.unbound, 3, "the second run reads the three left");
    }

    /// The marker counts each outcome: the build's own pass bound two (both
    /// through the name-only heritage edge), a re-run reads the one left.
    #[test]
    fn the_marker_counts_each_outcome() {
        let mut i = Sol::new("src::IGuestbook");
        let ig = i.ty(node_kind::INTERFACE, "IGuestbook");
        i.member(ig, node_kind::EVENT_EMITTER, "Signed");
        let mut c = Sol::new("src::Guestbook");
        let gb = c.ty(node_kind::CLASS, "Guestbook");
        c.is(gb, "IGuestbook", edge_category::IMPLEMENTS);
        let sign = c.member(gb, node_kind::METHOD, "sign");
        c.emit(sign, bare("Signed"), 1);
        c.emit(sign, attr("IGuestbook", "Signed"), 2);
        c.emit(sign, bare("Transfer"), 3);
        let (g, again) = build(vec![i, c]);
        let rules: Vec<String> = flows(&g).into_iter().map(|r| r.2).collect();
        assert_eq!(
            rules,
            ["inherited_event_by_name", "qualified_event_by_name"]
        );
        assert_eq!(
            again.marker().as_deref(),
            Some("[sol-events] bound=0 (inherited=0 qualified=0 by_name=0) ambiguous=0 unbound=1")
        );
        let first = EmitBinds {
            inherited: 1,
            qualified: 1,
            by_name: 2,
            ambiguous: 0,
            unbound: 1,
        };
        assert_eq!(
            first.marker().as_deref(),
            Some("[sol-events] bound=2 (inherited=1 qualified=1 by_name=2) ambiguous=0 unbound=1")
        );
        assert_eq!(EmitBinds::default().marker(), None);
    }

    /// A same-named STRUCT of the emitting file never takes an emit ref
    /// (`decide_ref`'s EVENT_FLOWS kind guard): the ref stays for this pass.
    #[test]
    fn a_same_named_struct_is_not_an_event() {
        let mut f = Sol::new("src::S");
        f.ty(node_kind::STRUCT, "Signed");
        let ib = f.ty(node_kind::INTERFACE, "IBook");
        f.member(ib, node_kind::EVENT_EMITTER, "Signed");
        let book = f.ty(node_kind::CLASS, "Book");
        f.is(book, "IBook", edge_category::IMPLEMENTS);
        let sign = f.member(book, node_kind::METHOD, "sign");
        f.emit(sign, bare("Signed"), 2);
        let (g, _) = build(vec![f]);
        assert_eq!(
            flows(&g),
            [row(
                "src::S::Book::sign",
                "src::S::IBook::Signed",
                "inherited_event",
                2
            )]
        );
    }
}
