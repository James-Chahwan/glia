//! LB.11a acceptance: a Go ROUTE is one node per (method, path), qname
//! `<METHOD> <path>` like every other server parser, so each route node is
//! HANDLED_BY only its own handler and an HTTP call reaches only the route of
//! its method.
//!
//! Before LB.11a the Go parser keyed a route on its path alone
//! (`route:/users`) and stacked one ROUTE_METHOD cell per verb on it, so gin's
//! `GET /users` + `POST /users` were ONE node HANDLED_BY listUsers AND
//! createUser. The HTTP resolver paired per method, but the edge landed on the
//! shared node, so a GET call traced into the POST handler (quokka-stack:
//! a DELETE of `/activity/:id` traced into the GET and PATCH handlers).
//!
//! The sources are the substrate-gap fixture `go-route-per-method`, copied
//! into a tempdir so the build never writes next to the fixture.

use std::collections::BTreeSet;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::NodeId;
use repo_graph_engine::generate_many;
use repo_graph_engine::trace::{TraceHop, TraceOptions, cross_stack_trace};
use repo_graph_graph::MergedGraph;

const FIXTURE: &str = "../../bench/substrate-gap/fixtures/go-route-per-method";
const GO_MOD: &str =
    include_str!("../../bench/substrate-gap/fixtures/go-route-per-method/api/go.mod");
const MAIN_GO: &str =
    include_str!("../../bench/substrate-gap/fixtures/go-route-per-method/api/main.go");
const API_TS: &str =
    include_str!("../../bench/substrate-gap/fixtures/go-route-per-method/web/api.ts");

fn build() -> (tempfile::TempDir, MergedGraph) {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path();
    for (rel, src) in [("api/go.mod", GO_MOD), ("api/main.go", MAIN_GO), ("web/api.ts", API_TS)] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("file has a parent")).expect("mkdir");
        std::fs::write(&path, src).unwrap_or_else(|e| panic!("write {FIXTURE}/{rel}: {e}"));
    }
    // The key's dir order: web first, then api.
    let paths: Vec<String> = ["web", "api"]
        .iter()
        .map(|d| root.join(d).to_string_lossy().into_owned())
        .collect();
    let merged = generate_many(&paths).expect("generate_many").merged;
    (td, merged)
}

fn id(m: &MergedGraph, qname: &str) -> NodeId {
    m.node_id_by_qname(qname)
        .unwrap_or_else(|| panic!("no node `{qname}` in the fixture graph"))
}

fn qname_of(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_else(|| format!("<unnamed {}>", id.0))
}

/// Qnames of every node `from` reaches over one `category` edge, intra-repo
/// or cross-repo, sorted.
fn targets(m: &MergedGraph, from: NodeId, category: repo_graph_core::EdgeCategoryId) -> Vec<String> {
    let set: BTreeSet<String> = m
        .graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(m.cross_edges.iter())
        .filter(|e| e.from == from && e.category == category)
        .map(|e| qname_of(m, e.to))
        .collect();
    set.into_iter().collect()
}

/// Every ROUTE qname in the build, sorted.
fn route_qnames(m: &MergedGraph) -> Vec<String> {
    let set: BTreeSet<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .kind_by_id
                .iter()
                .filter(|(_, k)| **k == node_kind::ROUTE)
                .filter_map(|(id, _)| g.nav.qname_by_id.get(id).cloned())
        })
        .collect();
    set.into_iter().collect()
}

#[test]
fn go_routes_are_one_node_per_method_and_path() {
    let (_td, m) = build();
    assert_eq!(
        route_qnames(&m),
        [
            "ANY /health",
            "DELETE /users/:id",
            "GET /users",
            "GET /users/:id",
            "POST /users",
        ],
        "one ROUTE per (method, path), no path-only `route:` node"
    );
    assert!(m.node_id_by_qname("route:/users").is_none());
    assert!(m.node_id_by_qname("route:/users/:id").is_none());
}

#[test]
fn each_route_is_handled_by_only_its_own_handler() {
    let (_td, m) = build();
    for (route, handler) in [
        ("GET /users", "main::listUsers"),
        ("POST /users", "main::createUser"),
        ("GET /users/:id", "main::getUser"),
        ("DELETE /users/:id", "main::deleteUser"),
        ("ANY /health", "main::health"),
    ] {
        assert_eq!(
            targets(&m, id(&m, route), edge_category::HANDLED_BY),
            [handler],
            "{route} HANDLED_BY"
        );
    }
}

#[test]
fn http_calls_reach_only_the_route_of_their_method() {
    let (_td, m) = build();
    for (endpoint, route) in [
        ("endpoint:GET:/users", "GET /users"),
        ("endpoint:POST:/users", "POST /users"),
        ("endpoint:DELETE:/users/${…}", "DELETE /users/:id"),
    ] {
        assert_eq!(
            targets(&m, id(&m, endpoint), edge_category::HTTP_CALLS),
            [route],
            "{endpoint} HTTP_CALLS"
        );
    }
}

/// The trace BFS tree from `feature`, 4 hops deep.
fn trace_hops(m: &MergedGraph, feature: &str) -> Vec<TraceHop> {
    let mut opts = TraceOptions::default();
    opts.depth = 4;
    let answer = cross_stack_trace(m, feature, &opts);
    assert!(answer.seed.is_some(), "{feature} resolves");
    answer.hops
}

#[test]
fn a_get_trace_never_reaches_the_post_handler() {
    let (_td, m) = build();
    let hops = trace_hops(&m, "loadUsers");
    let reached: Vec<&str> = hops.iter().map(|h| h.to_qname.as_str()).collect();
    assert!(reached.contains(&"GET /users"), "{reached:?}");
    assert!(reached.contains(&"main::listUsers"), "{reached:?}");
    for wrong in ["main::createUser", "POST /users", "main::deleteUser", "main::getUser"] {
        assert!(!reached.contains(&wrong), "loadUsers reached {wrong}: {reached:?}");
    }

    let hops = trace_hops(&m, "dropUser");
    let reached: Vec<&str> = hops.iter().map(|h| h.to_qname.as_str()).collect();
    assert!(reached.contains(&"main::deleteUser"), "{reached:?}");
    assert!(!reached.contains(&"main::getUser"), "a DELETE never traces into GET: {reached:?}");
}
