//! LA.31 acceptance: tRPC procedures and calls are located and owned on a REAL
//! two-dir build of the committed `xcut-trpc` fixture.
//!
//! `grade.py` reads the installed wheel, so it cannot see this change until the
//! end-of-wave rebuild; these tests grade the working tree directly. Every
//! assertion reads the STORED POSITION cell JSON (0-based rows), never
//! `locate_node`, whose return shape and line base LD.1 changes — so this test
//! holds in either landing order.

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::{GenerateResult, generate_many, node_file, service_map};
use glia_graph::MergedGraph;

fn build() -> GenerateResult {
    let root = format!(
        "{}/../bench/substrate-gap/fixtures/xcut-trpc",
        env!("CARGO_MANIFEST_DIR")
    );
    let dirs = [format!("{root}/server"), format!("{root}/client")];
    generate_many(&dirs).expect("fixture builds")
}

/// The id of the node of `kind` whose qname is exactly `qname`.
fn node_id(m: &MergedGraph, kind: NodeKindId, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .filter(move |(id, q)| {
                    q.as_str() == qname && g.nav.kind_by_id.get(id) == Some(&kind)
                })
                .map(|(id, _)| *id)
        })
        .next()
        .unwrap_or_else(|| panic!("no {qname} node of kind {kind:?}"))
}

/// Every stored POSITION payload on `id`, across graphs.
fn positions(m: &MergedGraph, id: NodeId) -> Vec<String> {
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == cell_type::POSITION)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) => Some(j.clone()),
            _ => None,
        })
        .collect()
}

fn has_edge(m: &MergedGraph, from: NodeId, to: NodeId, cat: EdgeCategoryId) -> bool {
    m.all_edges()
        .any(|e| e.from == from && e.to == to && e.category == cat)
}

#[test]
fn trpc_procedures_and_calls_are_located() {
    let r = build();
    let m = &r.merged;
    let list = node_id(m, node_kind::RPC_PROCEDURE, "rpc:user.list");
    let by_id = node_id(m, node_kind::RPC_PROCEDURE, "rpc:user.byId");
    let call = node_id(m, node_kind::RPC_CALL, "rpc_call:user.list");

    assert_eq!(
        positions(m, list),
        vec![r#"{"file":"router.ts","start_line":5,"end_line":5}"#.to_string()],
        "one-line span at the `list:` key (0-based row)"
    );
    assert_eq!(
        positions(m, by_id),
        vec![r#"{"file":"router.ts","start_line":6,"end_line":6}"#.to_string()],
        "the multi-line `byId` chain anchors at its key, not at `.query(`"
    );
    assert_eq!(
        positions(m, call),
        vec![r#"{"file":"users.tsx","start_line":3,"end_line":3}"#.to_string()],
        "one-line span at the `api.user.list.useQuery()` site"
    );
}

#[test]
fn trpc_markers_are_owned_or_module_contained() {
    let r = build();
    let m = &r.merged;
    let list = node_id(m, node_kind::RPC_PROCEDURE, "rpc:user.list");
    let by_id = node_id(m, node_kind::RPC_PROCEDURE, "rpc:user.byId");
    let call = node_id(m, node_kind::RPC_CALL, "rpc_call:user.list");
    let users = node_id(m, node_kind::FUNCTION, "users::Users");
    let users_mod = node_id(m, node_kind::MODULE, "users");
    let router = node_id(m, node_kind::MODULE, "router");

    // Outbound: the component holding the hook call USES the RPC_CALL.
    assert!(has_edge(m, users, call, edge_category::USES));
    assert!(
        !has_edge(m, users_mod, call, edge_category::CONTAINS),
        "an owned marker takes no module fallback edge"
    );

    // Inbound: a module-level router has no enclosing function, so its
    // procedures take the module CONTAINS fallback and no HANDLED_BY.
    for p in [list, by_id] {
        assert!(has_edge(m, router, p, edge_category::CONTAINS));
        assert!(
            !m.all_edges()
                .any(|e| e.from == p && e.category == edge_category::HANDLED_BY),
            "no function encloses a module-level router's keys"
        );
    }

    // A10.10 pairing is untouched, and still exact.
    assert!(has_edge(m, call, list, edge_category::RPC_CALLS));
    assert!(!has_edge(m, call, by_id, edge_category::RPC_CALLS));
}

#[test]
fn arch_places_the_trpc_link() {
    let r = build();
    // Every tRPC node carries its own file now (HEAD: all three unlocated).
    // The build's remaining unlocated node is the React COMPONENT overlay
    // `users::Users`, which LB.3a folds into its FUNCTION; not asserted here.
    for g in &r.merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            if kind == Some(node_kind::RPC_PROCEDURE) || kind == Some(node_kind::RPC_CALL) {
                assert!(
                    node_file(n).is_some(),
                    "{:?} has no file",
                    g.nav.qname_by_id.get(&n.id)
                );
            }
        }
    }
    let map = service_map(&r.merged, &r.repo_labels);
    let links: Vec<(&str, &str, &str)> = map
        .links
        .iter()
        .map(|l| (l.from.as_str(), l.to.as_str(), l.mechanism))
        .collect();
    assert!(
        links
            .iter()
            .any(|(f, t, mech)| *f == "client" && *t == "server" && *mech == "RPC_CALLS"),
        "missing client -> server RPC_CALLS in {links:?}"
    );
}
