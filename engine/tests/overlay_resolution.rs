//! LF.2d: the overlay sections that change RESOLUTION rather than add edges,
//! over the substrate-gap fixture `xcut-overlay-const-mount` (web, orders,
//! billing):
//!
//! - web's `[constants] GATEWAY = "/orders-svc"` binds the env-read base of
//!   `` axios.get(`${GATEWAY}/users`) ``, so the ENDPOINT re-keys from
//!   `endpoint:GET:${…}/users` to `endpoint:GET:/orders-svc/users`, its
//!   ENDPOINT_HIT records `"overlay":"const:GATEWAY"` and it turns Weak;
//! - orders' `[[route_prefix]] scope = "." prefix = "/orders-svc"` mounts every
//!   orders route under the gateway path, so both gateway calls pair with the
//!   orders routes and never with billing's `GET /users` (whose qname is the
//!   same as orders', which is why this is asserted here, by RepoId, and not
//!   in the fixture's key.json).
//!
//! Without the overlay (`BuildOptions::with_overlay(false)`) the build is the
//! pre-LF.2d shape: the base folds onto BOTH services' `GET /users`, and the
//! gateway path pairs with nothing.

use std::path::Path;

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, Confidence, NodeId, RepoId};
use repo_graph_engine::{BuildOptions, GenerateResult, generate_many_opts};

const FIXTURE: &str = "../bench/substrate-gap/fixtures/xcut-overlay-const-mount";

fn build(overlay: bool) -> GenerateResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let dirs: Vec<String> =
        ["web", "orders", "billing"].iter().map(|d| root.join(d).to_string_lossy().into_owned()).collect();
    generate_many_opts(&dirs, false, &BuildOptions::default().with_overlay(overlay)).expect("fixture builds")
}

/// The RepoId whose label (the path as given) ends in `/<dir>`.
fn repo_of_dir(r: &GenerateResult, dir: &str) -> RepoId {
    let hits: Vec<u64> = r
        .repo_labels
        .iter()
        .filter(|(_, label)| label.ends_with(&format!("/{dir}")) || label.as_str() == dir)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(hits.len(), 1, "one repo is {dir}: {:?}", r.repo_labels);
    RepoId(hits[0])
}

/// Every node of kind `kind` named `qname`, as (id, repo, confidence).
fn nodes(r: &GenerateResult, kind: repo_graph_core::NodeKindId, qname: &str) -> Vec<(NodeId, RepoId, Confidence)> {
    let mut out: Vec<(NodeId, RepoId, Confidence)> = Vec::new();
    for g in &r.merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) == Some(&kind)
                && g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname)
                && !out.iter().any(|(id, _, _)| *id == n.id)
            {
                out.push((n.id, g.repo, n.confidence));
            }
        }
    }
    out
}

/// The one node of `kind` named `qname` in `repo`.
fn node_in(r: &GenerateResult, kind: repo_graph_core::NodeKindId, qname: &str, repo: RepoId) -> NodeId {
    let hits: Vec<NodeId> =
        nodes(r, kind, qname).into_iter().filter(|(_, rp, _)| *rp == repo).map(|(id, _, _)| id).collect();
    assert_eq!(hits.len(), 1, "one {qname} in {repo:?}");
    hits[0]
}

/// The HTTP_CALLS cross edges out of `from`, as (to, confidence).
fn http_calls(r: &GenerateResult, from: NodeId) -> Vec<(NodeId, Confidence)> {
    r.merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::HTTP_CALLS && e.from == from)
        .map(|e| (e.to, e.confidence))
        .collect()
}

/// The ENDPOINT_HIT payloads of `id`, parsed.
fn endpoint_hits(r: &GenerateResult, id: NodeId) -> Vec<serde_json::Value> {
    r.merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter(|n| n.id == id))
        .flat_map(|n| n.cells.iter().filter(|c| c.kind == cell_type::ENDPOINT_HIT))
        .map(|c| match &c.payload {
            CellPayload::Json(s) => serde_json::from_str(s).expect("ENDPOINT_HIT is JSON"),
            other => panic!("ENDPOINT_HIT is JSON, got {other:?}"),
        })
        .collect()
}

#[test]
fn overlay_constant_and_mount_pair_the_gateway_paths_with_orders_only() {
    let r = build(true);
    let orders = repo_of_dir(&r, "orders");
    let billing = repo_of_dir(&r, "billing");
    let web = repo_of_dir(&r, "web");

    assert!(
        nodes(&r, node_kind::ENDPOINT, "endpoint:GET:${…}/users").is_empty(),
        "the pinned base re-keys the env-built endpoint"
    );
    let users_ep = nodes(&r, node_kind::ENDPOINT, "endpoint:GET:/orders-svc/users");
    assert_eq!(users_ep.len(), 1, "{users_ep:?}");
    let (users_ep, ep_repo, ep_conf) = users_ep[0];
    assert_eq!(ep_repo, web);
    assert_eq!(ep_conf, Confidence::Weak, "an llm-origin pin makes the endpoint Weak");
    let hits = endpoint_hits(&r, users_ep);
    assert!(!hits.is_empty());
    for h in &hits {
        assert_eq!(h["overlay"], "const:GATEWAY", "{h}");
        assert_eq!(h["path"], "/orders-svc/users", "{h}");
    }

    let orders_users = node_in(&r, node_kind::ROUTE, "GET /users", orders);
    let billing_users = node_in(&r, node_kind::ROUTE, "GET /users", billing);
    let orders_list = node_in(&r, node_kind::ROUTE, "GET /orders", orders);

    let users_calls = http_calls(&r, users_ep);
    assert_eq!(users_calls, [(orders_users, Confidence::Weak)], "exactly one pairing, to orders");
    assert!(
        !r.merged.cross_edges.iter().any(|e| e.category == edge_category::HTTP_CALLS && e.to == billing_users),
        "billing's GET /users is never a target"
    );

    let orders_ep = nodes(&r, node_kind::ENDPOINT, "endpoint:GET:/orders-svc/orders");
    assert_eq!(orders_ep.len(), 1, "{orders_ep:?}");
    assert!(
        endpoint_hits(&r, orders_ep[0].0).iter().all(|h| h.get("overlay").is_none()),
        "a literal path no pin folded is not marked"
    );
    assert_eq!(
        http_calls(&r, orders_ep[0].0),
        [(orders_list, Confidence::Weak)],
        "the mount pairs the gateway path at the mount's confidence"
    );
}

#[test]
fn without_the_overlay_the_build_is_the_head_shape() {
    let r = build(false);
    let orders = repo_of_dir(&r, "orders");
    let billing = repo_of_dir(&r, "billing");

    assert!(nodes(&r, node_kind::ENDPOINT, "endpoint:GET:/orders-svc/users").is_empty());
    let base = nodes(&r, node_kind::ENDPOINT, "endpoint:GET:${…}/users");
    assert_eq!(base.len(), 1, "{base:?}");
    assert!(endpoint_hits(&r, base[0].0).iter().all(|h| h.get("overlay").is_none()));
    assert_ne!(base[0].2, Confidence::Weak, "no pin, no overlay confidence");

    let mut fan_out: Vec<u64> = http_calls(&r, base[0].0).into_iter().map(|(to, _)| to.0).collect();
    fan_out.sort_unstable();
    let mut both = vec![
        node_in(&r, node_kind::ROUTE, "GET /users", orders).0,
        node_in(&r, node_kind::ROUTE, "GET /users", billing).0,
    ];
    both.sort_unstable();
    assert_eq!(fan_out, both, "the base folds onto both services' GET /users");

    let gateway = nodes(&r, node_kind::ENDPOINT, "endpoint:GET:/orders-svc/orders");
    assert_eq!(gateway.len(), 1, "{gateway:?}");
    assert!(http_calls(&r, gateway[0].0).is_empty(), "no mount: the gateway path pairs with nothing");
}
