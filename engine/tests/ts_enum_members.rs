//! LA.30c: TypeScript `enum` / `const enum` become ENUM nodes with one
//! ATTRIBUTE per member, and `Enum.Member` reads become USES edges — proven on
//! a REAL build of the `ts-enum-members` fixture.
//!
//! Before LA.30c `visit_top` had no `enum_declaration` arm, so a TS enum
//! produced no node at all. A same-file member read resolves in the parse
//! (`warm -> Color::Red`, `firstLocal -> Local::A`); an imported one
//! (`paint::pick -> Color::Green`, `paint::up -> Dir::Up`) is a USES
//! `Attribute` ref the graph crate binds through LA.30a's ENUM member lookup.
//! Run with `-- --nocapture` to see the `[ts-enums]` and
//! `[resolve] enum member uses bound:` markers.

use std::collections::HashMap;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{EdgeCategoryId, NodeId, NodeKindId};
use repo_graph_engine::generate_one;

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/ts-enum-members"
    )
    .to_string()
}

struct Built {
    /// `(kind, qname)` of every node.
    nodes: Vec<(NodeKindId, String)>,
    /// `(from qname, to qname, category)` of every edge.
    edges: Vec<(String, String, EdgeCategoryId)>,
}

fn build() -> Built {
    let r = generate_one(&fixture()).expect("fixture builds");
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    let mut nodes = Vec::new();
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
            if let Some(k) = g.nav.kind_by_id.get(id) {
                nodes.push((*k, q.clone()));
            }
        }
    }
    let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
    let mut edges: Vec<(String, String, EdgeCategoryId)> = r
        .merged
        .all_edges()
        .map(|e| (name(&e.from), name(&e.to), e.category))
        .collect();
    nodes.sort_by(|a, b| a.1.cmp(&b.1));
    edges.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    Built { nodes, edges }
}

impl Built {
    fn has_node(&self, kind: NodeKindId, qname: &str) -> bool {
        self.nodes.iter().any(|(k, q)| *k == kind && q == qname)
    }

    fn has_edge(&self, from: &str, to: &str, cat: EdgeCategoryId) -> bool {
        self.edges.iter().any(|(f, t, c)| f == from && t == to && *c == cat)
    }

    fn uses(&self) -> Vec<(&str, &str)> {
        self.edges
            .iter()
            .filter(|(_, _, c)| *c == edge_category::USES)
            .map(|(f, t, _)| (f.as_str(), t.as_str()))
            .collect()
    }
}

#[test]
fn enums_and_members_are_nodes() {
    let b = build();
    for e in ["Color", "Dir", "Local"] {
        let q = format!("src::color::{e}");
        assert!(b.has_node(node_kind::ENUM, &q), "ENUM {q} missing; nodes = {:?}", b.nodes);
        assert!(b.has_edge("src::color", &q, edge_category::DEFINES), "DEFINES {q}");
    }
    for (e, m) in [
        ("Color", "Red"),
        ("Color", "Green"),
        ("Color", "Blue"),
        ("Dir", "Up"),
        ("Dir", "Down"),
        ("Local", "A"),
        ("Local", "B"),
    ] {
        let enum_q = format!("src::color::{e}");
        let q = format!("{enum_q}::{m}");
        assert!(b.has_node(node_kind::ATTRIBUTE, &q), "ATTRIBUTE {q} missing");
        assert!(b.has_edge(&enum_q, &q, edge_category::HAS_ATTRIBUTE), "HAS_ATTRIBUTE {q}");
    }
}

#[test]
fn member_reads_are_uses_edges_same_file_and_imported() {
    let b = build();
    let uses = b.uses();
    for (from, to) in [
        ("src::color::warm", "src::color::Color::Red"),
        ("src::color::firstLocal", "src::color::Local::A"),
        ("src::paint::pick", "src::color::Color::Green"),
        ("src::paint::up", "src::color::Dir::Up"),
    ] {
        assert!(uses.contains(&(from, to)), "USES {from} -> {to} missing; USES = {uses:?}");
    }
    assert_eq!(uses.len(), 4, "exactly the four member reads; USES = {uses:?}");
}

#[test]
fn only_the_named_member_is_used_and_the_enum_is_not_its_own_member() {
    let b = build();
    assert!(!b.has_edge("src::color::warm", "src::color::Color::Green", edge_category::USES));
    assert!(!b.has_edge("src::paint::pick", "src::color::Color::Red", edge_category::USES));
    assert!(!b.has_node(node_kind::ATTRIBUTE, "src::color::Color::Color"));
}
