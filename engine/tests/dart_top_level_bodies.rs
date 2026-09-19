//! LA.37a: a top-level Dart function, getter or setter owns the
//! `function_body` that tree-sitter-dart 0.1.0 puts BESIDE its signature -
//! proven on a REAL build of the `dart-top-level-bodies` fixture.
//!
//! The body's calls and client ENDPOINTs are credited to the FUNCTION under
//! LA.34's local scope (a parameter named like a top-level function binds
//! nothing), a getter / setter pair is one FUNCTION owning both bodies, an
//! `external` function has no body and cannot steal its neighbour's, and the
//! FUNCTION's POSITION spans signature + body.

use std::collections::HashMap;

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, NodeId};
use repo_graph_engine::generate_one;

const FIXTURE: &str = "fixtures/dart-top-level-bodies";

fn bench(rel: &str) -> String {
    format!("{}/../bench/substrate-gap/{rel}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn top_level_bodies_are_walked_under_local_scope() {
    let r = generate_one(&bench(FIXTURE)).expect("fixture builds");
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
    }
    let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
    let mut calls: Vec<(String, String)> = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::CALLS)
        .map(|e| (name(&e.from), name(&e.to)))
        .collect();
    calls.sort();

    for (from, to) in [
        ("lib::app::loadUsers", "endpoint:GET:/users"),
        ("lib::app::loadUsers", "lib::app::helper"),
        ("lib::app::banner", "lib::app::describeAll"),
        ("lib::app::commented", "lib::app::helper"),
        ("lib::app::afterNative", "lib::app::helper"),
        ("lib::app::level", "lib::app::helper"),
        ("lib::app::level", "lib::app::store"),
    ] {
        assert!(
            calls.iter().any(|(f, t)| f == from && t == to),
            "expected CALLS {from} -> {to}; CALLS = {calls:?}"
        );
    }
    // `apply`'s parameter `helper` shadows the top-level function, and the
    // body-less `native` owns no body at all.
    for from in ["lib::app::apply", "lib::app::native"] {
        assert!(
            !calls.iter().any(|(f, _)| f == from),
            "{from} must have no CALLS; CALLS = {calls:?}"
        );
    }
}

#[test]
fn getter_setter_pair_is_one_function_and_cells_span_the_body() {
    let r = generate_one(&bench(FIXTURE)).expect("fixture builds");
    let mut level: Vec<NodeId> = Vec::new();
    let mut load_users: Option<NodeId> = None;
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            if g.nav.kind_by_id.get(id) != Some(&node_kind::FUNCTION) {
                continue;
            }
            match q.as_str() {
                "lib::app::level" => level.push(*id),
                "lib::app::loadUsers" => load_users = Some(*id),
                _ => {}
            }
        }
    }
    assert_eq!(level.len(), 1, "one FUNCTION lib::app::level");
    let level_id = level[0];
    let level_nodes = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == level_id)
        .count();
    assert_eq!(level_nodes, 1, "the getter / setter pair is one Node");
    let defines = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::DEFINES && e.to == level_id)
        .count();
    assert_eq!(defines, 1, "exactly one DEFINES edge into lib::app::level");

    let load_users = load_users.expect("FUNCTION lib::app::loadUsers");
    let cell = |kind| {
        r.merged
            .graphs
            .iter()
            .flat_map(|g| g.nodes.iter())
            .filter(|n| n.id == load_users)
            .flat_map(|n| n.cells.iter())
            .find(|c| c.kind == kind)
            .map(|c| match &c.payload {
                CellPayload::Json(j) | CellPayload::Text(j) => j.clone(),
                CellPayload::Bytes(_) => String::new(),
            })
            .unwrap_or_default()
    };
    let position = cell(cell_type::POSITION);
    assert!(
        position.contains(r#""end_line":7"#),
        "POSITION ends at the body's closing brace: {position}"
    );
    let code = cell(cell_type::CODE);
    assert!(
        code.contains("client.get('/users')"),
        "CODE covers signature + body: {code}"
    );
}
