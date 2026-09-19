//! Shared unit-test fixtures. These bodies moved verbatim out of the single
//! `mod tests` block in `lib.rs` when the crate was split into modules; they
//! live here because more than one module's tests use them.

use std::collections::HashSet;

use glia_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use glia_core::{Confidence, Edge, Node, NodeId, RepoId};

use crate::types::{RepoGraph, SymbolTable};

pub(crate) fn repo() -> RepoId {
    RepoId::from_canonical("test://unit")
}

/// Build a 4-node `RepoGraph` by hand: A → B → C plus D → C, all CALLS
/// edges. Used to exercise the reverse-traversal primitives in isolation
/// of any parser quirks.
pub(crate) fn flow_graph() -> RepoGraph {
    let r = repo();
    let a = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::a");
    let b = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::b");
    let c = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::c");
    let d = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::d");
    let nodes = vec![
        Node { id: a, repo: r, confidence: Confidence::Strong, cells: vec![] },
        Node { id: b, repo: r, confidence: Confidence::Strong, cells: vec![] },
        Node { id: c, repo: r, confidence: Confidence::Strong, cells: vec![] },
        Node { id: d, repo: r, confidence: Confidence::Strong, cells: vec![] },
    ];
    let edges = vec![
        Edge { from: a, to: b, category: edge_category::CALLS, confidence: Confidence::Strong, cells: Vec::new() },
        Edge { from: b, to: c, category: edge_category::CALLS, confidence: Confidence::Strong, cells: Vec::new() },
        Edge { from: d, to: c, category: edge_category::CALLS, confidence: Confidence::Strong, cells: Vec::new() },
    ];
    let mut nav = CodeNav::default();
    nav.record(a, "a", "m::a", node_kind::FUNCTION, None);
    nav.record(b, "b", "m::b", node_kind::FUNCTION, None);
    nav.record(c, "c", "m::c", node_kind::FUNCTION, None);
    nav.record(d, "d", "m::d", node_kind::FUNCTION, None);
    RepoGraph {
        repo: r,
        nodes,
        edges,
        symbols: SymbolTable::default(),
        nav,
        unresolved_calls: vec![],
        unresolved_refs: vec![],
        properties: HashSet::new(),
    }
}
