//! LB.11b acceptance: a ts_routes ROUTE is one node per (method, path), qname
//! `<METHOD> <path>`, located by a POSITION cell at its registration, and a
//! method-agnostic registration (Express `.all(`, a Next.js Pages Router
//! default export) is `ANY`.
//!
//! Built from the committed substrate-gap fixture `ts-route-per-method` (a TS
//! client in `web/`, an Express server plus a Pages Router API route in
//! `server/`), through `generate_many` exactly as `grade.py` builds it. The
//! grader reads the installed wheel, so this grades the working tree directly.
//! Run with `-- --nocapture` to see the `[ts-routes]` and `[http-qname]`
//! markers.

use std::collections::BTreeSet;

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, EdgeCategoryId, NodeId};
use repo_graph_engine::generate_many;
use repo_graph_graph::MergedGraph;

fn build() -> MergedGraph {
    let root = format!(
        "{}/../bench/substrate-gap/fixtures/ts-route-per-method",
        env!("CARGO_MANIFEST_DIR")
    );
    let dirs: Vec<String> = ["web", "server"].iter().map(|d| format!("{root}/{d}")).collect();
    generate_many(&dirs).expect("fixture builds").merged
}

/// Every ROUTE qname of the merge.
fn route_qnames(m: &MergedGraph) -> BTreeSet<String> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .filter(|(id, _)| g.nav.kind_by_id.get(id) == Some(&node_kind::ROUTE))
                .map(|(_, q)| q.clone())
        })
        .collect()
}

fn id(m: &MergedGraph, qname: &str) -> NodeId {
    m.node_id_by_qname(qname)
        .unwrap_or_else(|| panic!("no node `{qname}`; routes: {:?}", route_qnames(m)))
}

fn qname(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// The qnames `from` reaches over `category`, deduped.
fn targets(m: &MergedGraph, from: &str, category: EdgeCategoryId) -> BTreeSet<String> {
    let from = id(m, from);
    m.all_edges()
        .filter(|e| e.from == from && e.category == category)
        .map(|e| qname(m, e.to))
        .collect()
}

#[test]
fn ts_routes_are_per_method_located_and_any_for_agnostic_registrations() {
    let m = build();

    // One node per (method, path); the path-only shape is gone and `ALL` is
    // never a method token.
    let routes = route_qnames(&m);
    for q in ["GET /users", "POST /users", "GET /users/:id", "DELETE /users/:id", "ANY /health", "ANY /api/status"] {
        assert!(routes.contains(q), "missing ROUTE `{q}` in {routes:?}");
    }
    assert!(
        !routes.iter().any(|q| q.starts_with("route:") || q.starts_with("ALL ")),
        "{routes:?}"
    );

    // PRECISION: each method's node is handled by its own handler only.
    assert_eq!(
        targets(&m, "GET /users", edge_category::HANDLED_BY),
        BTreeSet::from(["app::listUsers".to_string()])
    );
    assert_eq!(
        targets(&m, "POST /users", edge_category::HANDLED_BY),
        BTreeSet::from(["app::createUser".to_string()])
    );

    // Each call pairs with exactly its own method's route; the POST to the
    // Pages Router default export reaches it through the any tier.
    assert_eq!(
        targets(&m, "endpoint:GET:/users", edge_category::HTTP_CALLS),
        BTreeSet::from(["GET /users".to_string()])
    );
    assert_eq!(
        targets(&m, "endpoint:POST:/users", edge_category::HTTP_CALLS),
        BTreeSet::from(["POST /users".to_string()])
    );
    assert_eq!(
        targets(&m, "endpoint:DELETE:/users/${…}", edge_category::HTTP_CALLS),
        BTreeSet::from(["DELETE /users/:id".to_string()])
    );
    assert_eq!(
        targets(&m, "endpoint:POST:/api/status", edge_category::HTTP_CALLS),
        BTreeSet::from(["ANY /api/status".to_string()])
    );

    // Located at the registration: `app.get("/users", listUsers)` is 0-based
    // row 9 of server/app.ts. Read off the node's cells, not through the
    // locator (whose line base LD.1 owns).
    let get_users = id(&m, "GET /users");
    let first_position = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == get_users)
        .flat_map(|n| n.cells.iter())
        .find_map(|c| match &c.payload {
            CellPayload::Json(s) if c.kind == cell_type::POSITION => Some(s.clone()),
            _ => None,
        });
    assert_eq!(
        first_position.as_deref(),
        Some(r#"{"file":"app.ts","start_line":9,"end_line":9}"#)
    );
}
