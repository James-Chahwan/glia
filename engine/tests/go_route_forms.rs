//! LA.32a acceptance: the method-bearing Go route registration forms become
//! ROUTEs with a ROUTE_METHOD cell and a HANDLED_BY edge, and every Go ROUTE
//! carries a POSITION cell at its registration call.
//!
//! Before LA.32a gin `r.Handle("PATCH", path, h)`, `r.Any(path, h)`,
//! `r.Match([]string{..}, path, h)` and Go 1.22 ServeMux patterns
//! (`mux.HandleFunc("GET /items/{id}", h)`) emitted no ROUTE at all, and the
//! verb-form `route:/users` carried only its ROUTE_METHOD cells, so every
//! POSITION-only reader (pyo3 `nodes_json`, projection-text, engram-export's
//! identity hints) saw a Go route with no file.
//!
//! The source is the substrate-gap fixture `go-route-registration-forms`,
//! copied into a tempdir so the build never writes next to the fixture. The
//! POSITION is read off the node's cells, never through `locate_node`, whose
//! line base is its own contract.

use repo_graph_code_domain::{cell_type, edge_category};
use repo_graph_core::{CellPayload, NodeId};
use repo_graph_engine::generate_one;
use repo_graph_graph::MergedGraph;

const FIXTURE_GO_MOD: &str =
    include_str!("../../bench/substrate-gap/fixtures/go-route-registration-forms/go.mod");
const FIXTURE_MAIN_GO: &str =
    include_str!("../../bench/substrate-gap/fixtures/go-route-registration-forms/main.go");

fn build() -> (tempfile::TempDir, MergedGraph) {
    let td = tempfile::tempdir().expect("tempdir");
    std::fs::write(td.path().join("go.mod"), FIXTURE_GO_MOD).expect("write go.mod");
    std::fs::write(td.path().join("main.go"), FIXTURE_MAIN_GO).expect("write main.go");
    let root = td.path().to_string_lossy().into_owned();
    let merged = generate_one(&root).expect("generate_one").merged;
    (td, merged)
}

fn route(m: &MergedGraph, qname: &str) -> NodeId {
    m.node_id_by_qname(qname)
        .unwrap_or_else(|| panic!("no node `{qname}` in the fixture graph"))
}

/// Every JSON payload of `kind` on node `id`, in cell order.
fn cells_of(m: &MergedGraph, id: NodeId, kind: repo_graph_core::CellTypeId) -> Vec<String> {
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == kind)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// The `method` of every ROUTE_METHOD cell on `id`, in cell order.
fn methods(m: &MergedGraph, id: NodeId) -> Vec<String> {
    cells_of(m, id, cell_type::ROUTE_METHOD)
        .iter()
        .filter_map(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .filter_map(|v| v.get("method").and_then(|x| x.as_str()).map(String::from))
        .collect()
}

/// The names of the nodes `id` is HANDLED_BY, sorted and deduped.
fn handlers(m: &MergedGraph, id: NodeId) -> Vec<String> {
    let mut out: Vec<String> = m
        .graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(m.cross_edges.iter())
        .filter(|e| e.from == id && e.category == edge_category::HANDLED_BY)
        .filter_map(|e| {
            m.graphs
                .iter()
                .find_map(|g| g.nav.name_by_id.get(&e.to).cloned())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn method_bearing_forms_emit_routes_with_methods_and_handlers() {
    let (_td, m) = build();

    let patch = route(&m, "route:/users/:id");
    assert_eq!(methods(&m, patch), vec!["PATCH".to_string()]);
    assert_eq!(handlers(&m, patch), vec!["patchUser".to_string()]);

    let ping = route(&m, "route:/ping");
    assert_eq!(methods(&m, ping), vec!["ANY".to_string()]);
    assert_eq!(handlers(&m, ping), vec!["anyPing".to_string()]);

    let orders = route(&m, "route:/orders");
    assert_eq!(methods(&m, orders), vec!["GET".to_string(), "POST".to_string()]);
    assert_eq!(handlers(&m, orders), vec!["matchOrders".to_string()]);

    let item = route(&m, "route:/items/{id}");
    assert_eq!(methods(&m, item), vec!["GET".to_string()]);
    assert_eq!(handlers(&m, item), vec!["getItem".to_string()]);

    // Precision: the method string is never a path, a Go 1.22 pattern is never
    // kept verbatim, and the PATCH handler stays on its own path.
    assert!(m.node_id_by_qname("route:/PATCH").is_none());
    assert!(m.node_id_by_qname("route:/GET /items/{id}").is_none());
    let users = route(&m, "route:/users");
    assert_eq!(handlers(&m, users), vec!["createUser".to_string(), "listUsers".to_string()]);
}

#[test]
fn every_go_route_is_positioned_at_its_registration() {
    let (_td, m) = build();

    // Verb form: `r.GET("/users", ..)` on 0-based row 17, then its POST twin
    // on row 18 — first POSITION wins, and it is the first registration.
    let users = route(&m, "route:/users");
    let pos = cells_of(&m, users, cell_type::POSITION);
    assert_eq!(
        pos.first().map(String::as_str),
        Some(r#"{"file":"main.go","start_line":17,"end_line":17}"#),
        "{pos:?}"
    );
    assert_eq!(pos.len(), 2, "one POSITION per registration: {pos:?}");

    // Go 1.22 pattern: `mux.HandleFunc("GET /items/{id}", ..)` on row 23.
    let item = route(&m, "route:/items/{id}");
    assert_eq!(
        cells_of(&m, item, cell_type::POSITION).first().map(String::as_str),
        Some(r#"{"file":"main.go","start_line":23,"end_line":23}"#),
    );

    // Every ROUTE in the graph carries at least one POSITION.
    for q in ["route:/users/:id", "route:/ping", "route:/orders"] {
        let id = route(&m, q);
        assert!(
            !cells_of(&m, id, cell_type::POSITION).is_empty(),
            "{q} has no POSITION cell"
        );
    }
}
