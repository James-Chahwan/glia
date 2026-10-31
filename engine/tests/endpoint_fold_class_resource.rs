//! GD.29 acceptance: a JS resource client whose URL is a class-held resource
//! (the chatwoot `ApiClient` shape) folds per subclass, on REAL builds of the
//! committed `js-class-resource-client` fixture.
//!
//! `ApiClient` builds `url` in a getter over `baseUrl()` (a `let` the
//! `accountScoped` option extends) and the constructor's `resource`; each
//! subclass hands `super()` a literal resource and options object and calls
//! `axios.<verb>(this.url)` / `` axios.<verb>(`${this.url}/..`) ``, `axios` an
//! ESLint `/* global axios */`. The endpoint fold binds the constructor
//! parameters from the literal `super()` arguments (James 2026-10-31: only a
//! string literal handed to a constructor / super(), never a runtime value),
//! takes the option branches the literal object decides, and keys the
//! ENDPOINT on the folded path, which then pairs the Rails routes.

use std::collections::BTreeSet;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::{GenerateResult, generate_many};
use glia_graph::MergedGraph;

const FIXTURE: &str = "js-class-resource-client";

const NOTIFICATIONS: &str = "endpoint:GET:/api/v1/accounts/${…}/notifications";
const UNREAD: &str = "endpoint:POST:/api/v1/accounts/${…}/notifications/${…}/unread";
const PROFILE_GET: &str = "endpoint:GET:/api/v1/profile";
const PROFILE_PUT: &str = "endpoint:PUT:/api/v1/profile";

fn fixture(dirs: &[&str]) -> GenerateResult {
    let root = format!(
        "{}/../bench/substrate-gap/fixtures/{FIXTURE}",
        env!("CARGO_MANIFEST_DIR")
    );
    let dirs: Vec<String> = dirs.iter().map(|d| format!("{root}/{d}")).collect();
    generate_many(&dirs).expect("fixture builds")
}

/// Every qname of `kind`, across graphs.
fn qnames(m: &MergedGraph, kind: NodeKindId) -> BTreeSet<String> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .kind_by_id
                .iter()
                .filter(move |(_, k)| **k == kind)
                .filter_map(move |(id, _)| g.nav.qname_by_id.get(id).cloned())
        })
        .collect()
}

fn node_id(m: &MergedGraph, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .find_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .find_map(|(id, q)| (q == qname).then_some(*id))
        })
        .unwrap_or_else(|| panic!("no node {qname}: {:?}", qnames(m, node_kind::ENDPOINT)))
}

fn qname_of(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// Every `category` edge, as `(from qname, to qname)`, sorted.
fn edges(m: &MergedGraph, category: EdgeCategoryId) -> BTreeSet<(String, String)> {
    m.all_edges()
        .filter(|e| e.category == category)
        .map(|e| (qname_of(m, e.from), qname_of(m, e.to)))
        .collect()
}

/// The JSON ENDPOINT_HIT payloads of the node `qname`.
fn hits(m: &MergedGraph, qname: &str) -> Vec<String> {
    let id = node_id(m, qname);
    m.graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .filter(|n| n.id == id)
        .flat_map(|n| &n.cells)
        .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) => Some(j.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn subclass_calls_fold_to_the_resource_path() {
    let r = fixture(&["client"]);
    let endpoints = qnames(&r.merged, node_kind::ENDPOINT);
    for q in [NOTIFICATIONS, UNREAD, PROFILE_GET, PROFILE_PUT] {
        assert!(endpoints.contains(q), "{q} missing: {endpoints:?}");
    }
    // The plain subclass passes no options: neither the enterprise nor the
    // accountScoped branch is taken.
    assert!(
        !endpoints.iter().any(|q| q.contains("enterprise")),
        "{endpoints:?}"
    );
    // The control's resource is a runtime value: its call stays unfolded.
    assert!(
        !endpoints
            .iter()
            .any(|q| q.starts_with("endpoint:GET:/api/v1/accounts/") && q.ends_with("/search")),
        "a runtime super() argument must not fold: {endpoints:?}"
    );
    assert!(
        endpoints.iter().any(|q| q.ends_with("/search")),
        "the control's call still mints its unfolded endpoint: {endpoints:?}"
    );
}

#[test]
fn folded_hit_names_its_class_and_source() {
    let r = fixture(&["client"]);
    let profile = hits(&r.merged, PROFILE_GET);
    assert_eq!(profile.len(), 1, "{profile:?}");
    let hit = &profile[0];
    assert!(hit.contains(r#""class_resource":"ProfileAPI""#), "{hit}");
    assert!(hit.contains(r#""folded_from":"<unresolved>""#), "{hit}");
    assert!(hit.contains(r#""path":"/api/v1/profile""#), "{hit}");

    let unread = hits(&r.merged, UNREAD);
    assert_eq!(unread.len(), 1, "{unread:?}");
    assert!(
        unread[0].contains(r#""class_resource":"NotificationsAPI""#),
        "{}",
        unread[0]
    );
    // The folded node is an inference: Weak.
    let id = node_id(&r.merged, UNREAD);
    let weak = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .filter(|n| n.id == id)
        .all(|n| n.confidence == glia_core::Confidence::Weak);
    assert!(weak, "a class-resource fold is Weak");
}

#[test]
fn calls_edge_carries_the_class_resource_rule() {
    let r = fixture(&["client"]);
    let list = "src::api::notifications::NotificationsAPI::list";
    let ep = node_id(&r.merged, NOTIFICATIONS);
    let edge = r
        .merged
        .all_edges()
        .find(|e| e.category == edge_category::CALLS && e.to == ep)
        .unwrap_or_else(|| panic!("no CALLS into {NOTIFICATIONS}"));
    assert_eq!(qname_of(&r.merged, edge.from), list);
    let ev = Evidence::of(edge).expect("evidence");
    assert_eq!(ev.rule.as_deref(), Some("class_resource_fold"), "{ev:?}");
    assert_eq!(ev.emitter, "extractor:endpoint_fold", "{ev:?}");
    assert_eq!(
        ev.file.as_deref(),
        Some("src/api/notifications.js"),
        "{ev:?}"
    );
}

#[test]
fn folded_endpoints_pair_the_rails_routes() {
    let r = fixture(&["client", "server"]);
    let calls = edges(&r.merged, edge_category::HTTP_CALLS);
    for pair in [
        (
            NOTIFICATIONS,
            "ANY /api/v1/accounts/:account_id/notifications",
        ),
        (
            UNREAD,
            "POST /api/v1/accounts/:account_id/notifications/:id/unread",
        ),
        (PROFILE_GET, "ANY /api/v1/profile"),
        (PROFILE_PUT, "ANY /api/v1/profile"),
    ] {
        let pair = (pair.0.to_string(), pair.1.to_string());
        assert!(calls.contains(&pair), "{pair:?} missing: {calls:?}");
    }
}
