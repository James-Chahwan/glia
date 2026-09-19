//! ROLE cells and the build-time role fold (LB.3a).
//!
//! Framework extractors classify a declaration by minting a *parallel* node
//! with the SAME qname: `services.rs` a SERVICE over a CLASS / STRUCT (over an
//! Elixir `defmodule` PACKAGE since LA.21a),
//! `angular.rs` a COMPONENT / SERVICE / DIRECTIVE / PIPE / GUARD over a CLASS,
//! `react.rs` a COMPONENT / HOOK and `vue.rs` a COMPONENT / COMPOSABLE over a
//! FUNCTION. Two nodes per declaration split everything that should land on
//! one: the overlay was recorded after the declaration in its module's nav, so
//! it won `module_symbols` and every INJECTS / heritage / bare-name reference
//! bound to an edgeless, unlocatable marker instead of the class.
//!
//! [`fold_role_overlays`] runs inside `merge_parses`, before any builder's
//! symbol table, and folds each overlay into its declaration: the declaration
//! keeps its kind and `NodeId` and gains ONE ROLE cell
//! (`cell_type::ROLE`, `{"roles":["COMPONENT","SERVICE"]}`, names from
//! `node_kind::ALL`, sorted by id). An overlay with no same-qname declaration
//! (a Vue SFC component named after its file) stays as a standalone role node.
//!
//! [`roles_in`] is the ONE reader: every consumer that asks "is this a
//! component / service / hook" reads roles through it, never through the kind
//! alone, because a folded node carries its declaration kind. Filled by
//! LB.3a; extended by LA.21a.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{CallSite, UnresolvedRef, cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, EdgeCategoryId, NodeId, NodeKindId};

use crate::types::RepoGraph;

const ROLE_KIND_TABLE: [NodeKindId; 7] = [
    node_kind::COMPONENT,
    node_kind::HOOK,
    node_kind::SERVICE,
    node_kind::DIRECTIVE,
    node_kind::PIPE,
    node_kind::GUARD,
    node_kind::COMPOSABLE,
];

/// The framework-role node kinds, in id order. A node of one of these kinds
/// that shares its qname with a declaration is folded into it at build time.
pub const ROLE_KINDS: &[NodeKindId] = &ROLE_KIND_TABLE;

/// The kinds an overlay can fold into, most preferred first. Among several
/// same-qname candidates the earliest kind here wins, then the earliest node.
/// PACKAGE is last (LA.21a): it is the base only when nothing else shares the
/// qname — an Elixir `defmodule`, the one PACKAGE `services.rs` classifies.
const BASE_PRIORITY: &[NodeKindId] = &[
    node_kind::CLASS,
    node_kind::STRUCT,
    node_kind::FUNCTION,
    node_kind::INTERFACE,
    node_kind::ENUM,
    node_kind::METHOD,
    node_kind::PACKAGE,
];

/// Roles a node plays: its own kind when that is a role kind, plus every entry
/// of its ROLE cell(s). Sorted by kind id, deduped. The ONE reader every
/// consumer uses (engine liveness, pyo3, neuropil).
///
/// The payload is parsed by hand (this crate stays off serde_json); a name
/// `node_kind::ALL` does not know, or a malformed payload, contributes nothing.
pub fn roles_in(kind: Option<NodeKindId>, cells: &[Cell]) -> Vec<NodeKindId> {
    let mut out: Vec<NodeKindId> = Vec::new();
    if let Some(k) = kind
        && ROLE_KINDS.contains(&k)
    {
        out.push(k);
    }
    for c in cells.iter().filter(|c| c.kind == cell_type::ROLE) {
        let text = match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) => s.as_str(),
            CellPayload::Bytes(_) => continue,
        };
        out.extend(parse_role_names(text));
    }
    out.sort_by_key(|k| k.0);
    out.dedup();
    out
}

/// The kind ids named in a ROLE payload's `"roles":[...]` array.
fn parse_role_names(json: &str) -> Vec<NodeKindId> {
    let key = "\"roles\"";
    let Some(idx) = json.find(key) else {
        return Vec::new();
    };
    let after = &json[idx + key.len()..];
    let Some(rest) = after.trim_start().strip_prefix(':') else {
        return Vec::new();
    };
    let Some(list) = rest.trim_start().strip_prefix('[') else {
        return Vec::new();
    };
    let Some(end) = list.find(']') else {
        return Vec::new();
    };
    list[..end]
        .split(',')
        .filter_map(|tok| {
            let name = tok.trim().strip_prefix('"')?.strip_suffix('"')?;
            node_kind::ALL
                .iter()
                .find(|(_, n)| *n == name)
                .map(|(id, _)| *id)
        })
        .collect()
}

/// The ROLE cell for `roles`: JSON `{"roles":["COMPONENT","SERVICE"]}`, names
/// from `node_kind::ALL`, sorted by id, deduped.
pub(crate) fn role_cell(roles: &[NodeKindId]) -> Cell {
    let mut ids: Vec<NodeKindId> = roles.to_vec();
    ids.sort_by_key(|k| k.0);
    ids.dedup();
    let names: Vec<String> = ids
        .iter()
        .map(|k| node_kind::name(*k))
        .filter(|n| *n != "UNKNOWN")
        .map(|n| format!("\"{n}\""))
        .collect();
    Cell {
        kind: cell_type::ROLE,
        payload: CellPayload::Json(format!("{{\"roles\":[{}]}}", names.join(","))),
    }
}

/// What one [`fold_role_overlays`] call did. `folded[i]` counts overlays of
/// kind `ROLE_KINDS[i]` folded into a declaration; `standalone` counts role
/// nodes left in place because no same-qname declaration exists.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RoleFoldStats {
    pub(crate) folded: [usize; ROLE_KIND_TABLE.len()],
    pub(crate) standalone: usize,
}

impl RoleFoldStats {
    /// True when the graph held any role node, folded or not.
    pub(crate) fn saw_role_nodes(&self) -> bool {
        self.standalone > 0 || self.folded.iter().any(|&n| n > 0)
    }

    /// The fired_on marker line:
    /// `[roles] folded N overlays into declarations (COMPONENT=a HOOK=b ...) standalone=S`.
    pub(crate) fn marker(&self) -> String {
        let total: usize = self.folded.iter().sum();
        let per_kind: Vec<String> = ROLE_KINDS
            .iter()
            .zip(self.folded.iter())
            .map(|(k, n)| format!("{}={n}", node_kind::name(*k)))
            .collect();
        format!(
            "[roles] folded {total} overlays into declarations ({}) standalone={}",
            per_kind.join(" "),
            self.standalone
        )
    }
}

/// Fold every role overlay that shares its qname with a declaration into that
/// declaration. Runs on the merged parses before the symbol table is built, so
/// `module_symbols`, INJECTS, CALLS and heritage all resolve to the survivor.
///
/// Walks `g.nodes` in Vec order; every HashMap here is lookup-only, so the
/// result is independent of hasher seeds. Edges are rewritten in place:
/// untouched edges keep their bytes and relative order, and only the rewritten
/// ones are filtered (a self-loop, an exact duplicate, or a CONTAINS that
/// duplicates the base's DEFINES — `services.rs` mints those).
pub(crate) fn fold_role_overlays(
    g: &mut RepoGraph,
    calls: &mut [CallSite],
    refs: &mut [UnresolvedRef],
) -> RoleFoldStats {
    let mut stats = RoleFoldStats::default();

    // (1) + (2): pair each overlay with its base, in node order.
    let mut folds: Vec<(usize, usize, NodeKindId)> = Vec::new();
    {
        let mut by_qname: HashMap<&str, Vec<(usize, NodeKindId)>> = HashMap::new();
        for (i, n) in g.nodes.iter().enumerate() {
            let (Some(q), Some(&k)) = (g.nav.qname_by_id.get(&n.id), g.nav.kind_by_id.get(&n.id))
            else {
                continue;
            };
            by_qname.entry(q.as_str()).or_default().push((i, k));
        }
        for (i, n) in g.nodes.iter().enumerate() {
            let Some(&kind) = g.nav.kind_by_id.get(&n.id) else {
                continue;
            };
            let Some(slot) = ROLE_KINDS.iter().position(|r| *r == kind) else {
                continue;
            };
            let base = g
                .nav
                .qname_by_id
                .get(&n.id)
                .and_then(|q| by_qname.get(q.as_str()))
                .and_then(|group| {
                    group
                        .iter()
                        .filter_map(|&(j, k)| {
                            BASE_PRIORITY.iter().position(|b| *b == k).map(|p| (p, j))
                        })
                        .min()
                });
            match base {
                Some((_, j)) => {
                    stats.folded[slot] += 1;
                    folds.push((i, j, kind));
                }
                None => stats.standalone += 1,
            }
        }
    }
    if folds.is_empty() {
        return stats;
    }

    // (3) cells: one ROLE cell per base (union of roles), then any overlay
    // cell the base does not already carry (same kind AND payload).
    let mut base_order: Vec<usize> = Vec::new();
    let mut roles_of: HashMap<usize, Vec<NodeKindId>> = HashMap::new();
    let mut extra_of: HashMap<usize, Vec<Cell>> = HashMap::new();
    for &(o, b, role) in &folds {
        if !roles_of.contains_key(&b) {
            base_order.push(b);
        }
        let roles = roles_of.entry(b).or_default();
        roles.push(role);
        roles.extend(roles_in(None, &g.nodes[o].cells));
        let extra = extra_of.entry(b).or_default();
        extra.extend(
            g.nodes[o]
                .cells
                .iter()
                .filter(|c| c.kind != cell_type::ROLE)
                .cloned(),
        );
    }
    for b in base_order {
        let node = &mut g.nodes[b];
        let mut roles = roles_of.remove(&b).unwrap_or_default();
        roles.extend(roles_in(None, &node.cells));
        let cell = role_cell(&roles);
        match node.cells.iter().position(|c| c.kind == cell_type::ROLE) {
            Some(at) => {
                node.cells.retain(|c| c.kind != cell_type::ROLE);
                node.cells.insert(at.min(node.cells.len()), cell);
            }
            None => node.cells.push(cell),
        }
        for c in extra_of.remove(&b).unwrap_or_default() {
            if !node.cells.contains(&c) {
                node.cells.push(c);
            }
        }
    }

    // (4) edges.
    let remap: HashMap<NodeId, NodeId> = folds
        .iter()
        .map(|&(o, b, _)| (g.nodes[o].id, g.nodes[b].id))
        .collect();
    let mut rewritten = vec![false; g.edges.len()];
    for (i, e) in g.edges.iter_mut().enumerate() {
        let from = remap.get(&e.from).copied();
        let to = remap.get(&e.to).copied();
        if let Some(f) = from {
            e.from = f;
        }
        if let Some(t) = to {
            e.to = t;
        }
        rewritten[i] = from.is_some() || to.is_some();
    }
    if rewritten.iter().any(|&r| r) {
        let untouched: HashSet<(NodeId, NodeId, EdgeCategoryId)> = g
            .edges
            .iter()
            .zip(&rewritten)
            .filter(|(_, r)| !**r)
            .map(|(e, _)| (e.from, e.to, e.category))
            .collect();
        let defines: HashSet<(NodeId, NodeId)> = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::DEFINES)
            .map(|e| (e.from, e.to))
            .collect();
        let mut seen: HashSet<(NodeId, NodeId, EdgeCategoryId)> = HashSet::new();
        let keep: Vec<bool> = g
            .edges
            .iter()
            .zip(&rewritten)
            .map(|(e, &r)| {
                if !r {
                    return true;
                }
                let key = (e.from, e.to, e.category);
                e.from != e.to
                    && !(e.category == edge_category::CONTAINS && defines.contains(&(e.from, e.to)))
                    && !untouched.contains(&key)
                    && seen.insert(key)
            })
            .collect();
        let mut flags = keep.into_iter();
        g.edges.retain(|_| flags.next().unwrap_or(true));
    }

    // (5) call sites and refs that originate on an overlay.
    for c in calls.iter_mut() {
        if let Some(&b) = remap.get(&c.from) {
            c.from = b;
        }
    }
    for r in refs.iter_mut() {
        if let Some(&b) = remap.get(&r.from) {
            r.from = b;
        }
        if let Some(&b) = remap.get(&r.from_module) {
            r.from_module = b;
        }
    }

    // (6) nav: the overlay disappears; any nav children it had move to the base.
    for &(o, b, _) in &folds {
        let (oid, bid) = (g.nodes[o].id, g.nodes[b].id);
        g.nav.name_by_id.remove(&oid);
        g.nav.qname_by_id.remove(&oid);
        g.nav.kind_by_id.remove(&oid);
        g.nav.parent_of.remove(&oid);
        if let Some(kids) = g.nav.children_of.remove(&oid) {
            for kid in kids {
                if kid == bid || remap.contains_key(&kid) {
                    continue;
                }
                if g.nav.parent_of.get(&kid) == Some(&oid) {
                    g.nav.parent_of.insert(kid, bid);
                }
                let list = g.nav.children_of.entry(bid).or_default();
                if !list.contains(&kid) {
                    list.push(kid);
                }
            }
        }
        if g.properties.remove(&oid) {
            g.properties.insert(bid);
        }
    }
    // An overlay minted by two extractors (angular.rs + services.rs for an
    // `@Injectable`) sits in its parent's children list twice: drop every
    // occurrence. Each list is filtered on its own, so map order is irrelevant.
    let mut emptied: Vec<NodeId> = Vec::new();
    for (parent, kids) in g.nav.children_of.iter_mut() {
        let before = kids.len();
        kids.retain(|k| !remap.contains_key(k));
        if kids.is_empty() && before > 0 {
            emptied.push(*parent);
        }
    }
    for parent in emptied {
        g.nav.children_of.remove(&parent);
    }

    // (7) the overlay nodes themselves.
    g.nodes.retain(|n| !remap.contains_key(&n.id));

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::repo;
    use crate::types::SymbolTable;
    use repo_graph_code_domain::{CallQualifier, CodeNav, GRAPH_TYPE};
    use repo_graph_core::{Confidence, Edge, Node};

    fn node(id: NodeId) -> Node {
        Node {
            id,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![],
        }
    }

    fn edge(from: NodeId, to: NodeId, category: EdgeCategoryId) -> Edge {
        Edge {
            from,
            to,
            category,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        }
    }

    fn empty_graph() -> RepoGraph {
        RepoGraph {
            repo: repo(),
            nodes: vec![],
            edges: vec![],
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    fn id(kind: NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    #[test]
    fn role_cell_is_sorted_by_kind_id_and_deduped() {
        let c = role_cell(&[node_kind::SERVICE, node_kind::COMPONENT, node_kind::SERVICE]);
        assert_eq!(c.kind, cell_type::ROLE);
        assert_eq!(
            c.payload,
            CellPayload::Json(r#"{"roles":["COMPONENT","SERVICE"]}"#.into())
        );
    }

    #[test]
    fn roles_in_reads_own_kind_and_role_cells() {
        let cells = vec![role_cell(&[node_kind::HOOK, node_kind::COMPOSABLE])];
        assert_eq!(
            roles_in(Some(node_kind::FUNCTION), &cells),
            vec![node_kind::HOOK, node_kind::COMPOSABLE]
        );
        // A standalone role node carries no cell: its kind is the role.
        assert_eq!(
            roles_in(Some(node_kind::COMPONENT), &[]),
            vec![node_kind::COMPONENT]
        );
        assert!(roles_in(Some(node_kind::CLASS), &[]).is_empty());
        assert!(roles_in(None, &[]).is_empty());
    }

    #[test]
    fn roles_in_skips_unknown_and_malformed_payloads() {
        let odd = |s: &str| Cell {
            kind: cell_type::ROLE,
            payload: CellPayload::Json(s.into()),
        };
        let cells = vec![
            odd(r#"{ "roles" : [ "SERVICE" , "NOT_A_KIND", 7 ] }"#),
            odd(r#"{"roles":["GUARD""#),
            odd("not json at all"),
            Cell {
                kind: cell_type::ROLE,
                payload: CellPayload::Bytes(vec![1, 2]),
            },
            // Not a ROLE cell: ignored even though its text names a role.
            Cell {
                kind: cell_type::DOC,
                payload: CellPayload::Text(r#"{"roles":["PIPE"]}"#.into()),
            },
        ];
        assert_eq!(roles_in(None, &cells), vec![node_kind::SERVICE]);
    }

    #[test]
    fn marker_names_every_role_kind_in_id_order() {
        let mut s = RoleFoldStats::default();
        assert!(!s.saw_role_nodes());
        s.folded[0] = 1;
        s.folded[2] = 2;
        assert!(s.saw_role_nodes());
        assert_eq!(
            s.marker(),
            "[roles] folded 3 overlays into declarations (COMPONENT=1 HOOK=0 SERVICE=2 \
             DIRECTIVE=0 PIPE=0 GUARD=0 COMPOSABLE=0) standalone=0"
        );
    }

    /// A STRUCT + SERVICE twin (the Go / Rust shape): the overlay's CONTAINS
    /// edge to a method the struct does NOT define moves to the struct; one
    /// the struct already DEFINES is dropped; an untouched edge keeps its slot.
    #[test]
    fn fold_moves_edges_calls_and_refs_to_the_base() {
        let mut g = empty_graph();
        let module = id(node_kind::MODULE, "m");
        let st = id(node_kind::STRUCT, "m::Svc");
        let svc = id(node_kind::SERVICE, "m::Svc");
        let run = id(node_kind::METHOD, "m::Svc::run");
        let stop = id(node_kind::METHOD, "m::Svc::stop");
        let other = id(node_kind::FUNCTION, "m::other");
        g.nav.record(module, "m", "m", node_kind::MODULE, None);
        g.nav
            .record(st, "Svc", "m::Svc", node_kind::STRUCT, Some(module));
        g.nav
            .record(run, "run", "m::Svc::run", node_kind::METHOD, Some(st));
        g.nav
            .record(stop, "stop", "m::Svc::stop", node_kind::METHOD, Some(st));
        g.nav.record(
            other,
            "other",
            "m::other",
            node_kind::FUNCTION,
            Some(module),
        );
        g.nav
            .record(svc, "Svc", "m::Svc", node_kind::SERVICE, Some(module));
        // Recorded twice, as angular.rs + services.rs both do for @Injectable.
        g.nav.children_of.entry(module).or_default().push(svc);
        g.nodes = vec![
            node(module),
            node(st),
            node(run),
            node(stop),
            node(other),
            node(svc),
        ];
        g.nodes[5].cells.push(Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Text("d".into()),
        });
        g.edges = vec![
            edge(module, st, edge_category::DEFINES),
            edge(st, run, edge_category::DEFINES),
            edge(svc, run, edge_category::CONTAINS),
            edge(svc, stop, edge_category::CONTAINS),
            edge(other, svc, edge_category::CALLS),
            edge(other, st, edge_category::CALLS),
            edge(svc, svc, edge_category::USES),
        ];
        let mut calls = vec![CallSite {
            from: svc,
            qualifier: CallQualifier::Bare("x".into()),
        }];
        let mut refs = vec![UnresolvedRef {
            from: svc,
            from_module: module,
            qualifier: CallQualifier::Bare("y".into()),
            category: edge_category::INJECTS,
        }];

        let stats = fold_role_overlays(&mut g, &mut calls, &mut refs);
        assert_eq!(stats.folded, [0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(stats.standalone, 0);

        assert_eq!(
            g.nodes.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![module, st, run, stop, other]
        );
        let survivor = &g.nodes[1];
        assert_eq!(
            roles_in(Some(node_kind::STRUCT), &survivor.cells),
            vec![node_kind::SERVICE]
        );
        assert!(
            survivor.cells.iter().any(|c| c.kind == cell_type::DOC),
            "overlay cell carried over"
        );

        let got: Vec<(NodeId, NodeId, EdgeCategoryId)> =
            g.edges.iter().map(|e| (e.from, e.to, e.category)).collect();
        assert_eq!(
            got,
            vec![
                (module, st, edge_category::DEFINES),
                (st, run, edge_category::DEFINES),
                // svc -CONTAINS-> run dropped (duplicates DEFINES); stop moves.
                (st, stop, edge_category::CONTAINS),
                // other -CALLS-> svc duplicated other -CALLS-> st: dropped.
                (other, st, edge_category::CALLS),
                // svc -USES-> svc became a self-loop: dropped.
            ]
        );
        assert_eq!(calls[0].from, st);
        assert_eq!(refs[0].from, st);
        assert_eq!(refs[0].from_module, module);

        assert!(!g.nav.kind_by_id.contains_key(&svc));
        assert!(!g.nav.qname_by_id.contains_key(&svc));
        assert_eq!(g.nav.children_of[&module], vec![st, other]);
    }

    #[test]
    fn base_priority_prefers_class_then_node_order() {
        let mut g = empty_graph();
        let f = id(node_kind::FUNCTION, "m::X");
        let c = id(node_kind::CLASS, "m::X");
        let comp = id(node_kind::COMPONENT, "m::X");
        let hook = id(node_kind::HOOK, "m::useX");
        let hook_fn = id(node_kind::FUNCTION, "m::useX");
        let hook_comp = id(node_kind::COMPOSABLE, "m::useX");
        for (i, k, q) in [
            (f, node_kind::FUNCTION, "m::X"),
            (c, node_kind::CLASS, "m::X"),
            (comp, node_kind::COMPONENT, "m::X"),
            (hook_fn, node_kind::FUNCTION, "m::useX"),
            (hook, node_kind::HOOK, "m::useX"),
            (hook_comp, node_kind::COMPOSABLE, "m::useX"),
        ] {
            g.nav
                .record(i, q.rsplit("::").next().unwrap_or(q), q, k, None);
            g.nodes.push(node(i));
        }
        let stats = fold_role_overlays(&mut g, &mut [], &mut []);
        assert_eq!(stats.folded, [1, 1, 0, 0, 0, 0, 1]);
        assert_eq!(
            g.nodes.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![f, c, hook_fn]
        );
        assert!(
            g.nodes[0].cells.is_empty(),
            "the FUNCTION loses to the CLASS"
        );
        assert_eq!(
            roles_in(None, &g.nodes[1].cells),
            vec![node_kind::COMPONENT]
        );
        assert_eq!(
            g.nodes[2].cells,
            vec![role_cell(&[node_kind::HOOK, node_kind::COMPOSABLE])],
            "two overlays of one base merge into ONE ROLE cell"
        );
    }

    /// LA.21a: an Elixir `defmodule` is a PACKAGE; its SERVICE overlay folds
    /// into it (no twin), its CONTAINS to a function the module DEFINES is
    /// dropped. PACKAGE loses to any other base kind sharing the qname.
    #[test]
    fn service_overlay_on_package_folds_into_the_package() {
        let mut g = empty_graph();
        let module = id(node_kind::MODULE, "worker");
        let pkg = id(node_kind::PACKAGE, "worker::MyApp.Cache");
        let init = id(node_kind::FUNCTION, "worker::MyApp.Cache::init");
        let svc = id(node_kind::SERVICE, "worker::MyApp.Cache");
        g.nav
            .record(module, "worker", "worker", node_kind::MODULE, None);
        g.nav.record(
            pkg,
            "Cache",
            "worker::MyApp.Cache",
            node_kind::PACKAGE,
            Some(module),
        );
        g.nav.record(
            init,
            "init",
            "worker::MyApp.Cache::init",
            node_kind::FUNCTION,
            Some(pkg),
        );
        g.nav.record(
            svc,
            "MyApp.Cache",
            "worker::MyApp.Cache",
            node_kind::SERVICE,
            Some(module),
        );
        g.nodes = vec![node(module), node(pkg), node(init), node(svc)];
        g.edges = vec![
            edge(module, pkg, edge_category::CONTAINS),
            edge(pkg, init, edge_category::DEFINES),
            edge(svc, init, edge_category::CONTAINS),
        ];

        let stats = fold_role_overlays(&mut g, &mut [], &mut []);
        assert_eq!(stats.folded, [0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(stats.standalone, 0);
        assert_eq!(
            g.nodes.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![module, pkg, init]
        );
        assert_eq!(
            roles_in(Some(node_kind::PACKAGE), &g.nodes[1].cells),
            vec![node_kind::SERVICE]
        );
        assert_eq!(
            g.edges
                .iter()
                .map(|e| (e.from, e.to, e.category))
                .collect::<Vec<_>>(),
            vec![
                (module, pkg, edge_category::CONTAINS),
                (pkg, init, edge_category::DEFINES),
            ]
        );
        assert!(!g.nav.kind_by_id.contains_key(&svc));

        // A CLASS sharing the qname outranks the PACKAGE.
        let mut g = empty_graph();
        let p = id(node_kind::PACKAGE, "m::X");
        let c = id(node_kind::CLASS, "m::X");
        let s = id(node_kind::SERVICE, "m::X");
        for (i, k) in [
            (p, node_kind::PACKAGE),
            (c, node_kind::CLASS),
            (s, node_kind::SERVICE),
        ] {
            g.nav.record(i, "X", "m::X", k, None);
            g.nodes.push(node(i));
        }
        fold_role_overlays(&mut g, &mut [], &mut []);
        assert_eq!(g.nodes.iter().map(|n| n.id).collect::<Vec<_>>(), vec![p, c]);
        assert!(
            g.nodes[0].cells.is_empty(),
            "the PACKAGE loses to the CLASS"
        );
        assert_eq!(roles_in(None, &g.nodes[1].cells), vec![node_kind::SERVICE]);
    }

    #[test]
    fn standalone_overlay_stays_and_is_counted() {
        let mut g = empty_graph();
        let comp = id(node_kind::COMPONENT, "components::UserCard::UserCard");
        g.nav.record(
            comp,
            "UserCard",
            "components::UserCard::UserCard",
            node_kind::COMPONENT,
            None,
        );
        g.nodes.push(node(comp));
        let stats = fold_role_overlays(&mut g, &mut [], &mut []);
        assert_eq!(stats.standalone, 1);
        assert_eq!(stats.folded, [0; 7]);
        assert!(stats.saw_role_nodes());
        assert_eq!(g.nodes.len(), 1);
        assert!(g.nodes[0].cells.is_empty());
        assert_eq!(g.nav.kind_by_id[&comp], node_kind::COMPONENT);
    }
}
