//! LA.30b: Java enum constants become ATTRIBUTE nodes (HAS_ATTRIBUTE from the
//! ENUM), enum-body members are walked like a class body, a constant's class
//! body contributes METHODs under that constant, and constant references
//! become USES edges — proven on a REAL build of the `java-enum-members`
//! fixture.
//!
//! Before LA.30b `visit_type_decl` walked an `enum_body` with the class-body
//! arms, which match none of its `enum_constant` / `enum_body_declarations`
//! children, so an enum produced its ENUM node and nothing else. A bare
//! constant inside the enum resolves in the parse (`pick -> RED`); the
//! imported `Color.pick()` and `Color.GREEN` ride LA.30a (a CALLS site and a
//! USES `Attribute` ref the graph crate binds against the ENUM). Run with
//! `-- --nocapture` to see the `[java-enums]` and `[resolve] enum ...` markers.

use std::collections::HashMap;

use glia_code_domain::{edge_category, node_kind};
use glia_core::{EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::generate_one;

const PKG: &str = "src::main::java::com::shop";

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/java-enum-members"
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

fn q(tail: &str) -> String {
    format!("{PKG}::{tail}")
}

impl Built {
    fn has_node(&self, kind: NodeKindId, qname: &str) -> bool {
        self.nodes.iter().any(|(k, q)| *k == kind && q == qname)
    }

    fn has_edge(&self, from: &str, to: &str, cat: EdgeCategoryId) -> bool {
        self.edges.iter().any(|(f, t, c)| f == from && t == to && *c == cat)
    }

    fn of(&self, cat: EdgeCategoryId) -> Vec<(&str, &str)> {
        self.edges
            .iter()
            .filter(|(_, _, c)| *c == cat)
            .map(|(f, t, _)| (f.as_str(), t.as_str()))
            .collect()
    }
}

#[test]
fn constants_are_attributes_of_the_enum() {
    let b = build();
    let color = q("Color");
    assert!(b.has_node(node_kind::ENUM, &color), "ENUM missing; nodes = {:?}", b.nodes);
    for c in ["RED", "GREEN", "BLUE"] {
        let cq = q(&format!("Color::{c}"));
        assert!(b.has_node(node_kind::ATTRIBUTE, &cq), "ATTRIBUTE {cq} missing");
        assert!(b.has_edge(&color, &cq, edge_category::HAS_ATTRIBUTE), "HAS_ATTRIBUTE {cq}");
    }
}

#[test]
fn enum_body_and_constant_body_methods_are_nodes() {
    let b = build();
    let color = q("Color");
    for m in ["Color", "label", "pick", "warm", "isRed"] {
        let mq = q(&format!("Color::{m}"));
        assert!(b.has_node(node_kind::METHOD, &mq), "METHOD {mq} missing");
        assert!(b.has_edge(&color, &mq, edge_category::DEFINES), "DEFINES {mq}");
    }
    let green = q("Color::GREEN");
    let body_label = q("Color::GREEN::label");
    assert!(b.has_node(node_kind::METHOD, &body_label), "constant-body METHOD missing");
    assert!(b.has_edge(&green, &body_label, edge_category::DEFINES));
    assert!(!b.has_edge(&color, &body_label, edge_category::DEFINES));
}

#[test]
fn calls_into_the_enum_resolve() {
    let b = build();
    let calls = b.of(edge_category::CALLS);
    for (from, to) in [
        (q("Color::warm"), q("Color::isRed")),
        (q("web::Picker::choose"), q("Color::pick")),
    ] {
        assert!(
            calls.contains(&(from.as_str(), to.as_str())),
            "CALLS {from} -> {to} missing; CALLS = {calls:?}"
        );
    }
}

#[test]
fn constant_references_are_uses_edges_same_file_and_imported() {
    let b = build();
    let uses = b.of(edge_category::USES);
    let want = [
        (q("Color::isRed"), q("Color::RED")),
        (q("Color::pick"), q("Color::RED")),
        (q("web::Picker::green"), q("Color::GREEN")),
    ];
    for (from, to) in &want {
        assert!(
            uses.contains(&(from.as_str(), to.as_str())),
            "USES {from} -> {to} missing; USES = {uses:?}"
        );
    }
    assert_eq!(uses.len(), want.len(), "exactly the three constant reads; USES = {uses:?}");
}

#[test]
fn only_the_named_constant_and_never_the_override() {
    let b = build();
    assert!(!b.has_edge(&q("Color::pick"), &q("Color::GREEN"), edge_category::USES));
    assert!(!b.has_edge(&q("web::Picker::lbl"), &q("Color::GREEN::label"), edge_category::CALLS));
}
