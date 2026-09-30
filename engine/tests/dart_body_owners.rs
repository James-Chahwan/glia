//! LA.37b: every Dart member body is credited to its owner - proven on a REAL
//! build of the `dart-body-owners` fixture.
//!
//! Mixins, named extensions and extension types are CLASS nodes owning their
//! members; an enhanced enum owns its members; an unnamed extension on a type
//! the file declares hangs its members on that type, and one on a foreign
//! type mints nothing. A getter / setter is a METHOD owning its body; a
//! constructor is a METHOD `<T>::<T>` owning its body (CB.9). No CALLS edge
//! starts at an ENDPOINT, and no body is credited to the member declared
//! before it.
//!
//! CB.9, on a real build of the `dart-members` fixture: constructors,
//! factories, operators and abstract members are METHODs, enum constants
//! ATTRIBUTEs under their ENUM; every call comes from a member, none from a
//! CLASS; a named constructor / factory called on its class binds.

use std::collections::{HashMap, HashSet};

use glia_code_domain::{edge_category, node_kind};
use glia_core::NodeId;
use glia_engine::generate_one;

const FIXTURE: &str = "fixtures/dart-body-owners";

fn bench(rel: &str) -> String {
    format!("{}/../bench/substrate-gap/{rel}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn member_bodies_are_credited_to_their_owner() {
    let r = generate_one(&bench(FIXTURE)).expect("fixture builds");
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    let mut kind: HashMap<NodeId, glia_core::NodeKindId> = HashMap::new();
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
        for (id, k) in &g.nav.kind_by_id {
            kind.insert(*id, *k);
        }
    }

    let nodes: HashSet<(glia_core::NodeKindId, &str)> = qname
        .iter()
        .filter_map(|(id, q)| kind.get(id).map(|k| (*k, q.as_str())))
        .collect();
    for (k, q) in [
        (node_kind::CLASS, "lib::app::Greets"),
        (node_kind::METHOD, "lib::app::Greets::greet"),
        (node_kind::CLASS, "lib::app::Shout"),
        (node_kind::METHOD, "lib::app::Shout::shout"),
        (node_kind::METHOD, "lib::app::Color::label"),
        (node_kind::METHOD, "lib::app::Api::status"),
        (node_kind::METHOD, "lib::app::Api::doubled"),
        (node_kind::CLASS, "lib::app::Meters"),
        (node_kind::METHOD, "lib::app::Meters::plus"),
    ] {
        assert!(nodes.contains(&(k, q)), "expected node {q}");
    }
    for q in ["lib::app::String", "lib::app::String::whisper"] {
        assert!(!qname.values().any(|v| v == q), "no phantom {q}");
    }

    let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
    let calls: Vec<(NodeId, String, String)> = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::CALLS)
        .map(|e| (e.from, name(&e.from), name(&e.to)))
        .collect();
    let has = |from: &str, to: &str| calls.iter().any(|(_, f, t)| f == from && t == to);

    for (from, to) in [
        ("lib::app::Api::status", "endpoint:GET:/status"),
        ("lib::app::Api::Api", "endpoint:GET:/boot"),
        ("lib::app::Greets::greet", "lib::app::Greets::hello"),
        ("lib::app::Shout::shout", "lib::app::Shout::twice"),
        ("lib::app::Color::label", "lib::app::Color::describe"),
        ("lib::app::Api::area", "lib::app::Api::compute"),
        ("lib::app::Api::area", "lib::app::Api::store"),
        ("lib::app::Meters::twicePlus", "lib::app::Meters::plus"),
    ] {
        assert!(has(from, to), "expected CALLS {from} -> {to}; CALLS = {calls:?}");
    }

    for (from, to) in [
        ("lib::app::Api::warm", "endpoint:GET:/boot"),
        ("lib::app::Api", "endpoint:GET:/boot"),
        ("lib::app::Api::compute", "lib::app::Api::compute"),
    ] {
        assert!(!has(from, to), "forbidden CALLS {from} -> {to}");
    }
    let from_endpoint: Vec<_> = calls
        .iter()
        .filter(|(from, _, _)| kind.get(from) == Some(&node_kind::ENDPOINT))
        .collect();
    assert!(from_endpoint.is_empty(), "CALLS from an ENDPOINT: {from_endpoint:?}");
}

/// The qname and kind of every node, and the CALLS / HAS_ATTRIBUTE edges as
/// (category, from qname, to qname), of one fixture's real build.
struct Built {
    nodes: HashSet<(glia_core::NodeKindId, String)>,
    kind_of: HashMap<String, glia_core::NodeKindId>,
    edges: Vec<(glia_core::EdgeCategoryId, String, String)>,
}

fn build(fixture: &str) -> Built {
    let r = generate_one(&bench(fixture)).expect("fixture builds");
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    let mut kind: HashMap<NodeId, glia_core::NodeKindId> = HashMap::new();
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
        for (id, k) in &g.nav.kind_by_id {
            kind.insert(*id, *k);
        }
    }
    let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
    let edges = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::CALLS || e.category == edge_category::HAS_ATTRIBUTE)
        .map(|e| (e.category, name(&e.from), name(&e.to)))
        .collect();
    let kind_of: HashMap<String, glia_core::NodeKindId> = qname
        .iter()
        .filter_map(|(id, q)| kind.get(id).map(|k| (q.clone(), *k)))
        .collect();
    Built {
        nodes: kind_of.iter().map(|(q, k)| (*k, q.clone())).collect(),
        kind_of,
        edges,
    }
}

#[test]
fn constructors_operators_abstract_members_and_enum_constants_are_nodes() {
    let b = build("fixtures/dart-members");
    let m = "lib::src::money";
    for (k, q) in [
        (node_kind::METHOD, "Money::Money"),
        (node_kind::METHOD, "Money::zero"),
        (node_kind::METHOD, "Money::parse"),
        (node_kind::METHOD, "Money::operator+"),
        (node_kind::METHOD, "Repo::load"),
        (node_kind::METHOD, "Repo::save"),
        (node_kind::ATTRIBUTE, "Currency::aud"),
        (node_kind::ATTRIBUTE, "Currency::usd"),
    ] {
        let q = format!("{m}::{q}");
        assert!(b.nodes.contains(&(k, q.clone())), "expected node {q}");
    }
    let has = |cat, from: &str, to: &str| {
        b.edges
            .iter()
            .any(|(c, f, t)| *c == cat && f == &format!("{m}::{from}") && t == &format!("{m}::{to}"))
    };
    for (cat, from, to) in [
        (edge_category::CALLS, "Money::Money", "Money::validate"),
        (edge_category::CALLS, "Money::zero", "round2"),
        (edge_category::CALLS, "Money::operator+", "round2"),
        (edge_category::CALLS, "total", "Money::zero"),
        (edge_category::CALLS, "total", "Money::parse"),
        (edge_category::HAS_ATTRIBUTE, "Currency", "Currency::aud"),
        (edge_category::HAS_ATTRIBUTE, "Currency", "Currency::usd"),
    ] {
        assert!(has(cat, from, to), "expected {cat:?} {from} -> {to}; edges = {:?}", b.edges);
    }
    // A CLASS makes no calls: its members do.
    let from_class: Vec<_> = b
        .edges
        .iter()
        .filter(|(c, f, _)| *c == edge_category::CALLS && b.kind_of.get(f) == Some(&node_kind::CLASS))
        .collect();
    assert!(from_class.is_empty(), "CALLS from a CLASS: {from_class:?}");
    // An abstract member has no body, so no calls.
    assert!(
        !b.edges.iter().any(|(_, f, _)| f.starts_with(&format!("{m}::Repo::"))),
        "{:?}",
        b.edges
    );
}
