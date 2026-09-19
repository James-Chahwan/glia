//! LE.9b — `spec_status::spec_status`: implemented / declared_missing /
//! undeclared per route and feature, over the contract ops LE.9a declares and
//! the DOCUMENTS edges A10.2's contract-link pass pairs them with.
//!
//! The first two trees are LE.9a's fixtures, file for file:
//! bench/substrate-gap/fixtures/sdd-quokka-feature and sdd-speckit-features.
//! A key.json cannot assert an answer (it grades nodes, edges and cells), so
//! the report itself is pinned here.

use std::path::Path;

use glia_engine::generate_one;
use glia_engine::spec_status::{
    DECLARED_MISSING, IMPLEMENTED, SpecStatus, SpecStatusRow, UNDECLARED, spec_status,
};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

fn build(root: &Path) -> glia_engine::GenerateResult {
    generate_one(root.to_str().expect("utf-8 temp path")).expect("build")
}

fn status_of(root: &Path, feature: Option<&str>) -> SpecStatus {
    let r = build(root);
    spec_status(&r.merged, &r.repo_labels, feature)
}

/// `(feature, status, method, path)` per row, in answer order.
fn shape(s: &SpecStatus) -> Vec<(Option<&str>, &str, &str, &str)> {
    s.rows
        .iter()
        .map(|r| {
            (
                r.feature.as_deref(),
                r.status,
                r.method.as_str(),
                r.path.as_str(),
            )
        })
        .collect()
}

fn at(l: &Option<glia_engine::Located>) -> Option<(String, i64)> {
    let l = l.as_ref()?;
    Some((l.file.clone()?, l.line?))
}

fn row<'a>(
    s: &'a SpecStatus,
    feature: Option<&str>,
    status: &str,
    path: &str,
) -> &'a SpecStatusRow {
    s.rows
        .iter()
        .find(|r| r.feature.as_deref() == feature && r.status == status && r.path == path)
        .unwrap_or_else(|| panic!("no {status} row for {path} under {feature:?}: {s:#?}"))
}

/// bench/substrate-gap/fixtures/sdd-quokka-feature, verbatim.
fn quokka_tree(root: &Path) {
    write(
        root,
        "features/activities/feature.yaml",
        "name: Activities\n\
         status: complete\n\
         purpose: Hosts create activities; members join or leave them.\n\
         backend_routes:\n\
         \x20 protected:\n\
         \x20   - POST /api/protected/activity   # create\n\
         \x20   - GET  /api/protected/activity/:id\n\
         \x20   - POST /api/protected/activity/:id/leave   # not implemented yet\n\
         frontend_components:\n\
         \x20 - web/src/app/features/activities/activities.component.ts\n\
         mongo_collection: activities\n",
    );
    write(
        root,
        "server/routes.go",
        "package server\n\
         \n\
         import (\n\
         \t\"net/http\"\n\
         \n\
         \t\"github.com/gin-gonic/gin\"\n\
         )\n\
         \n\
         // CreateActivity implements POST /api/protected/activity.\n\
         func CreateActivity(c *gin.Context) {\n\
         \tc.JSON(http.StatusCreated, gin.H{\"id\": \"a1\"})\n\
         }\n\
         \n\
         // GetActivity implements GET /api/protected/activity/:id.\n\
         func GetActivity(c *gin.Context) {\n\
         \tc.JSON(http.StatusOK, gin.H{\"id\": c.Param(\"id\")})\n\
         }\n\
         \n\
         // Health is the liveness probe; no feature declares it.\n\
         func Health(c *gin.Context) {\n\
         \tc.String(http.StatusOK, \"ok\")\n\
         }\n\
         \n\
         // Register wires the routes. POST /api/protected/activity/:id/leave is\n\
         // declared in features/activities/feature.yaml but not implemented yet.\n\
         func Register(r *gin.Engine) {\n\
         \tr.POST(\"/api/protected/activity\", CreateActivity)\n\
         \tr.GET(\"/api/protected/activity/:id\", GetActivity)\n\
         \tr.GET(\"/healthz\", Health)\n\
         }\n",
    );
}

const ORDERS_SPEC: &str = "openapi: 3.0.3\n\
info:\n\
\x20 title: Orders\n\
\x20 version: 1.0.0\n\
paths:\n\
\x20 /orders:\n\
\x20   get:\n\
\x20     operationId: listOrders\n\
\x20     summary: List the caller's orders\n\
\x20     responses:\n\
\x20       '200':\n\
\x20         description: ok\n\
\x20   post:\n\
\x20     operationId: createOrder\n\
\x20     summary: Place an order\n\
\x20     responses:\n\
\x20       '201':\n\
\x20         description: created\n";

const ADMIN_SPEC: &str = "openapi: 3.0.3\n\
info:\n\
\x20 title: Admin\n\
\x20 version: 1.0.0\n\
paths:\n\
\x20 /orders:\n\
\x20   get:\n\
\x20     operationId: adminListOrders\n\
\x20     summary: List every order (admin view)\n\
\x20     responses:\n\
\x20       '200':\n\
\x20         description: ok\n\
\x20 /admin/purge:\n\
\x20   post:\n\
\x20     operationId: purge\n\
\x20     summary: Purge cancelled orders\n\
\x20     responses:\n\
\x20       '204':\n\
\x20         description: purged\n";

const FLASK_APP: &str = "\"\"\"Flask service implementing the spec-kit features under specs/.\"\"\"\n\
from flask import Flask\n\
\n\
app = Flask(__name__)\n\
\n\
\n\
@app.get(\"/orders\")\n\
def list_orders():\n\
\x20   return {\"orders\": []}\n\
\n\
\n\
@app.get(\"/health\")\n\
def health():\n\
\x20   return {\"ok\": True}\n";

/// bench/substrate-gap/fixtures/sdd-speckit-features, verbatim.
fn speckit_tree(root: &Path) {
    write(root, "specs/001-orders/contracts/openapi.yaml", ORDERS_SPEC);
    write(root, "specs/002-admin/contracts/openapi.yaml", ADMIN_SPEC);
    write(root, "app/main.py", FLASK_APP);
}

#[test]
fn quokka_feature() {
    let dir = tempfile::tempdir().expect("tempdir");
    quokka_tree(dir.path());
    let s = status_of(dir.path(), None);
    let activities = Some("activities");
    assert_eq!(
        shape(&s),
        vec![
            (activities, IMPLEMENTED, "POST", "/api/protected/activity"),
            (
                activities,
                IMPLEMENTED,
                "GET",
                "/api/protected/activity/:id"
            ),
            (
                activities,
                DECLARED_MISSING,
                "POST",
                "/api/protected/activity/:id/leave"
            ),
            (None, UNDECLARED, "GET", "/healthz"),
        ],
        "{s:#?}"
    );

    let create = row(&s, activities, IMPLEMENTED, "/api/protected/activity");
    assert_eq!(create.source, Some("feature_yaml"));
    // An exact (method, path) pairing, on a route no client calls: the http
    // pass demotes such a route to Medium and the DOCUMENTS edge never rises
    // above its route.
    assert_eq!(create.pairing, Some("exact"));
    assert_eq!(create.confidence, Some("medium"));
    assert_eq!(
        at(&create.decl),
        Some(("features/activities/feature.yaml".into(), 6))
    );
    assert_eq!(
        create.handler.as_ref().map(|h| h.name.as_str()),
        Some("CreateActivity")
    );
    assert_eq!(at(&create.handler), Some(("server/routes.go".into(), 10)));
    assert_eq!(
        at(&create.route),
        Some(("server/routes.go".into(), 27)),
        "the registration"
    );

    let get = row(&s, activities, IMPLEMENTED, "/api/protected/activity/:id");
    assert_eq!(at(&get.handler), Some(("server/routes.go".into(), 15)));
    assert_eq!(
        at(&get.decl),
        Some(("features/activities/feature.yaml".into(), 7))
    );

    let leave = row(
        &s,
        activities,
        DECLARED_MISSING,
        "/api/protected/activity/:id/leave",
    );
    assert_eq!(
        at(&leave.decl),
        Some(("features/activities/feature.yaml".into(), 8))
    );
    assert!(leave.route.is_none() && leave.handler.is_none() && leave.confidence.is_none());

    let health = row(&s, None, UNDECLARED, "/healthz");
    assert_eq!(at(&health.handler), Some(("server/routes.go".into(), 20)));
    assert!(health.decl.is_none() && health.source.is_none());

    assert_eq!(s.governed_services, vec!["server".to_string()]);
    assert_eq!(s.ungoverned_routes, 0);
    let t = &s.by_feature["activities"];
    assert_eq!((t.declared, t.implemented, t.declared_missing), (3, 2, 1));
    assert_eq!(
        s.summary(),
        "features=1 declared=3 implemented=2 declared_missing=1 undeclared=1 ungoverned=0"
    );

    // Byte-stable: a second call serialises identically.
    let again = status_of(dir.path(), None);
    assert_eq!(
        serde_json::to_string(&s).expect("serialises"),
        serde_json::to_string(&again).expect("serialises")
    );
}

#[test]
fn speckit_two_features() {
    let dir = tempfile::tempdir().expect("tempdir");
    speckit_tree(dir.path());
    let s = status_of(dir.path(), None);
    let (orders, admin) = (Some("001-orders"), Some("002-admin"));
    assert_eq!(
        shape(&s),
        vec![
            (orders, IMPLEMENTED, "GET", "/orders"),
            (orders, DECLARED_MISSING, "POST", "/orders"),
            (admin, IMPLEMENTED, "GET", "/orders"),
            (admin, DECLARED_MISSING, "POST", "/admin/purge"),
            (None, UNDECLARED, "GET", "/health"),
        ],
        "{s:#?}"
    );
    // Two features declaring GET /orders: one row each, on the ONE route.
    let a = row(&s, orders, IMPLEMENTED, "/orders");
    let b = row(&s, admin, IMPLEMENTED, "/orders");
    let route_id = |r: &SpecStatusRow| r.route.as_ref().map(|l| l.id);
    assert!(route_id(a).is_some());
    assert_eq!(route_id(a), route_id(b));
    assert_ne!(a.decl.as_ref().map(|l| l.id), b.decl.as_ref().map(|l| l.id));
    assert_eq!(a.source, Some("openapi"));
    assert_eq!(
        a.decl.as_ref().and_then(|l| l.file.as_deref()),
        Some("specs/001-orders/contracts/openapi.yaml")
    );
    assert_eq!(
        a.handler.as_ref().map(|h| h.name.as_str()),
        Some("list_orders")
    );
    assert_eq!(
        at(&row(&s, admin, DECLARED_MISSING, "/admin/purge").decl),
        Some(("specs/002-admin/contracts/openapi.yaml".into(), 14))
    );

    assert_eq!(s.governed_services, vec!["app".to_string()]);
    assert_eq!(s.ungoverned_routes, 0);
    assert_eq!(
        s.by_feature.keys().collect::<Vec<_>>(),
        ["001-orders", "002-admin"]
    );
    for t in s.by_feature.values() {
        assert_eq!((t.declared, t.implemented, t.declared_missing), (2, 1, 1));
    }
    assert_eq!(
        s.summary(),
        "features=2 declared=4 implemented=2 declared_missing=2 undeclared=1 ungoverned=0"
    );
}

/// A repo with no contract files governs nothing: its routes are counted,
/// never reported as undeclared.
#[test]
fn no_specs_no_undeclared() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(dir.path(), "app/main.py", FLASK_APP);
    let s = status_of(dir.path(), None);
    assert!(s.rows.is_empty(), "{s:#?}");
    assert!(s.by_feature.is_empty() && s.governed_services.is_empty());
    assert_eq!(s.ungoverned_routes, 2);
    assert_eq!(
        s.summary(),
        "features=0 declared=0 implemented=0 declared_missing=0 undeclared=0 ungoverned=2"
    );
}

/// A Pact interaction is the consumer's expectation of the provider, not a
/// declaration of the provider's surface: it pairs with the route (the
/// DOCUMENTS edge exists) but declares nothing and governs nothing.
#[test]
fn pact_is_not_a_declaration() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "pact.json",
        "{\n  \"consumer\": {\"name\": \"web\"},\n  \"provider\": {\"name\": \"api\"},\n  \
         \"interactions\": [\n    {\n      \"description\": \"a request for users\",\n      \
         \"request\": {\"method\": \"GET\", \"path\": \"/users\"},\n      \
         \"response\": {\"status\": 200}\n    }\n  ],\n  \
         \"metadata\": {\"pactSpecification\": {\"version\": \"3.0.0\"}}\n}\n",
    );
    write(
        dir.path(),
        "app.py",
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
         @app.route(\"/users\")\ndef list_users():\n    return {\"users\": []}\n\n\n\
         @app.route(\"/health\")\ndef health():\n    return {\"ok\": True}\n",
    );
    let r = build(dir.path());
    let merged = &r.merged;
    let qname = |id| {
        merged
            .graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_default()
    };
    let pact_documents_a_route = merged.all_edges().any(|e| {
        e.category == glia_code_domain::edge_category::DOCUMENTS
            && qname(e.from).starts_with("contract::")
            && qname(e.to).ends_with("/users")
    });
    assert!(
        pact_documents_a_route,
        "control: the pact op pairs with the route"
    );

    let s = spec_status(merged, &r.repo_labels, None);
    assert!(s.rows.is_empty(), "{s:#?}");
    assert!(s.governed_services.is_empty());
    assert_eq!(s.ungoverned_routes, 2);
}

#[test]
fn feature_filter() {
    let dir = tempfile::tempdir().expect("tempdir");
    speckit_tree(dir.path());
    let s = status_of(dir.path(), Some("002-admin"));
    let admin = Some("002-admin");
    assert_eq!(
        shape(&s),
        vec![
            (admin, IMPLEMENTED, "GET", "/orders"),
            (admin, DECLARED_MISSING, "POST", "/admin/purge"),
        ],
        "a feature filter drops other features and the feature-less undeclared rows: {s:#?}"
    );
    assert_eq!(s.by_feature.keys().collect::<Vec<_>>(), ["002-admin"]);
    // Governance is a whole-graph fact, not narrowed by the filter.
    assert_eq!(s.governed_services, vec!["app".to_string()]);
    assert_eq!(
        s.summary(),
        "features=1 declared=2 implemented=1 declared_missing=1 undeclared=0 ungoverned=0"
    );

    let none = status_of(dir.path(), Some("003-nothing"));
    assert!(
        none.rows.is_empty() && none.by_feature.is_empty(),
        "{none:#?}"
    );
}

/// No ORIGIN `feature`: the op's feature is the stem of the file declaring it
/// (its POSITION file), so a service-wide `openapi.yaml` is feature `openapi`.
/// The route of another service stays ungoverned.
#[test]
fn file_stem_is_the_fallback_feature() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(dir.path(), "api/openapi.yaml", ORDERS_SPEC);
    write(dir.path(), "api/main.py", FLASK_APP);
    write(
        dir.path(),
        "admin/main.py",
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
         @app.get(\"/purge\")\ndef purge():\n    return {}\n",
    );
    let s = status_of(dir.path(), None);
    let openapi = Some("openapi");
    assert_eq!(
        shape(&s),
        vec![
            (openapi, IMPLEMENTED, "GET", "/orders"),
            (openapi, DECLARED_MISSING, "POST", "/orders"),
            (None, UNDECLARED, "GET", "/health"),
        ],
        "{s:#?}"
    );
    assert_eq!(s.governed_services, vec!["api".to_string()]);
    assert_eq!(
        s.ungoverned_routes, 1,
        "admin's /purge: its service declares nothing"
    );
}
