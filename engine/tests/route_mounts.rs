//! GD.40 acceptance: include-style route mounts, Django first. A urlconf
//! mounted under a prefix BY REFERENCE to its module (`path("api/",
//! include("app.urls"))`, a nested include, a spread of an imported
//! `urlpatterns`) gives each of its routes the full path, located at its
//! `path(..)` call; the include() line mints no ROUTE; and a client calling
//! the full path pairs.
//!
//! The probe tree is the substrate-gap fixture `py-django-include`, copied
//! into a tempdir (server and client as two repos, the fixture's `dirs`);
//! the edge cases (two prefixes, a cycle, an unresolved and a dynamic
//! include) are written inline.
//!
//! GD.41, Python second: Flask `register_blueprint(bp, url_prefix=..)` and
//! FastAPI `include_router(<mod>.router, prefix=..)` across files, a two-level
//! router-into-router chain and a router subclass (`ServiceRouter(prefix=..)`)
//! read as a router, on the fixtures `py-flask-register-blueprint` and
//! `py-fastapi-include-router`, whose clients call non-`/api` prefixes so only
//! the mount pairs them.
//!
//! GD.42, TypeScript third: Express `app.use('/p', <router>)` (an inline
//! `require(..)`, a default import, a same-file router), Hono `app.route('/p',
//! <sub>)` over chained sub-apps (documenso's shape), and NestJS
//! `setGlobalPrefix('p', { exclude })` with `RouterModule.register([{ path,
//! module }])`, on the fixtures `ts-express-use-mount`, `ts-hono-route-mount`
//! and `ts-nest-global-prefix`, whose clients call `/svc/..` prefixes.

use std::path::Path;

use glia_code_domain::{edge_category, evidence, node_kind};
use glia_core::NodeId;
use glia_engine::{generate_many, generate_one};
use glia_graph::MergedGraph;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../bench/substrate-gap/fixtures/py-django-include"
);

/// Copy `rel` files of the fixture (or `files` written inline) under `dir`.
fn write(dir: &Path, files: &[(&str, &str)]) {
    for (rel, src) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, src).expect("write");
    }
}

fn fixture_file(rel: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURE}/{rel}")).expect("fixture file")
}

/// A GD.41 fixture's `server/` files and `client/web/api.ts`, built as two
/// repos.
fn fixture_pair(fixture: &str, server: &[&str]) -> (tempfile::TempDir, MergedGraph) {
    let dir = format!(
        "{}/../bench/substrate-gap/fixtures/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    let read = |rel: &str| std::fs::read_to_string(format!("{dir}/{rel}")).expect("fixture file");
    let td = tempfile::tempdir().expect("tempdir");
    for rel in server {
        write(
            &td.path().join("server"),
            &[(rel, &read(&format!("server/{rel}")))],
        );
    }
    write(
        &td.path().join("client"),
        &[("web/api.ts", &read("client/web/api.ts"))],
    );
    let repos: Vec<String> = ["server", "client"]
        .iter()
        .map(|d| td.path().join(d).to_string_lossy().into_owned())
        .collect();
    let merged = generate_many(&repos).expect("the fixture builds").merged;
    (td, merged)
}

/// The fixture's server and client, built as two repos.
fn probe() -> (tempfile::TempDir, MergedGraph) {
    let td = tempfile::tempdir().expect("tempdir");
    let server = [
        "proj/urls.py",
        "app/urls/__init__.py",
        "app/urls/issue.py",
        "app/urls/nested.py",
        "app/auth_urls.py",
    ];
    for rel in server {
        write(
            &td.path().join("server"),
            &[(rel, &fixture_file(&format!("server/{rel}")))],
        );
    }
    write(
        &td.path().join("client"),
        &[("web/api.ts", &fixture_file("client/web/api.ts"))],
    );
    let repos: Vec<String> = ["server", "client"]
        .iter()
        .map(|d| td.path().join(d).to_string_lossy().into_owned())
        .collect();
    let merged = generate_many(&repos).expect("the probe builds").merged;
    (td, merged)
}

fn build_one(files: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph) {
    let td = tempfile::tempdir().expect("tempdir");
    write(td.path(), files);
    let root = td.path().to_string_lossy().into_owned();
    let merged = generate_one(&root).expect("generate_one").merged;
    (td, merged)
}

/// Every ROUTE qname of `m`, sorted.
fn routes(m: &MergedGraph) -> Vec<String> {
    let mut out: Vec<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ROUTE))
                .filter_map(|n| g.nav.qname_by_id.get(&n.id).cloned())
        })
        .collect();
    out.sort();
    out
}

/// The `file:line` (0-based) of route `qname`.
fn position(m: &MergedGraph, qname: &str) -> Option<String> {
    let id = m.node_id_by_qname(qname)?;
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .find_map(|n| evidence::locate(&n.cells))
        .map(|(file, line)| format!("{file}:{}", line.unwrap_or(u32::MAX)))
}

/// The qnames of every HTTP_CALLS target of an ENDPOINT whose qname
/// contains `client`, sorted.
fn paired(m: &MergedGraph, client: &str) -> Vec<String> {
    let qname = |id: NodeId| {
        m.graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_default()
    };
    let mut out: Vec<String> = m
        .cross_edges
        .iter()
        .chain(m.graphs.iter().flat_map(|g| g.edges.iter()))
        .filter(|e| e.category == edge_category::HTTP_CALLS && qname(e.from).contains(client))
        .map(|e| qname(e.to))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The probe tree: a nested include, a spread import, each route at its
/// full path and its call, no include() ROUTE, every client call paired.
#[test]
fn django_includes_mount_full_paths_and_pair() {
    let (_td, m) = probe();
    assert_eq!(
        routes(&m),
        [
            "ANY /api/v2/things/",
            "ANY /api/workspaces/<str:slug>/issues/",
            "ANY /auth/login/",
        ]
    );
    assert_eq!(
        position(&m, "ANY /api/workspaces/<str:slug>/issues/").as_deref(),
        Some("app/urls/issue.py:3")
    );
    assert_eq!(
        position(&m, "ANY /api/v2/things/").as_deref(),
        Some("app/urls/nested.py:3")
    );
    assert_eq!(
        paired(&m, "/api/workspaces/"),
        ["ANY /api/workspaces/<str:slug>/issues/"]
    );
    assert_eq!(paired(&m, "/api/v2/things/"), ["ANY /api/v2/things/"]);
    assert_eq!(paired(&m, "/auth/login/"), ["ANY /auth/login/"]);
}

const AUTH: &str =
    "from django.urls import path\n\nurlpatterns = [\n    path(\"login/\", login),\n]\n";

/// One urlconf included under two prefixes is two ROUTEs at one call.
#[test]
fn two_includes_of_one_module_are_two_routes() {
    let root = "from django.urls import include, path\n\nurlpatterns = [\n    path(\"api/\", include(\"app.auth_urls\")),\n    path(\"legacy/\", include(\"app.auth_urls\")),\n]\n";
    let (_td, m) = build_one(&[("proj/urls.py", root), ("app/auth_urls.py", AUTH)]);
    assert_eq!(routes(&m), ["ANY /api/login/", "ANY /legacy/login/"]);
    for r in ["ANY /api/login/", "ANY /legacy/login/"] {
        assert_eq!(
            position(&m, r).as_deref(),
            Some("app/auth_urls.py:3"),
            "{r}"
        );
    }
}

/// A cycle stops at the fixpoint's cap and leaves no provisional; an include
/// of a module outside the repo (`django.contrib.admin.urls`) and a dynamic
/// one (`include(router.urls)`) mint nothing; a root's own pattern keeps its
/// local path.
#[test]
fn cycles_unresolved_and_dynamic_includes_mint_no_stray_route() {
    let a = "from django.urls import include, path\n\nurlpatterns = [\n    path(\"a/\", view_a),\n    path(\"b/\", include(\"loop.b\")),\n]\n";
    let b = "from django.urls import include, path\n\nurlpatterns = [\n    path(\"back/\", include(\"loop.a\")),\n]\n";
    let root = "from django.urls import include, path\n\nurlpatterns = [\n    path(\"x/\", include(\"loop.a\")),\n    path(\"admin/\", include(\"django.contrib.admin.urls\")),\n    path(\"r/\", include(router.urls)),\n    path(\"health/\", health),\n]\n";
    let (_td, m) = build_one(&[("proj/urls.py", root), ("loop/a.py", a), ("loop/b.py", b)]);
    let rs = routes(&m);
    assert!(rs.contains(&"ANY /health/".to_string()), "{rs:?}");
    assert!(rs.contains(&"ANY /x/a/".to_string()), "{rs:?}");
    assert!(rs.contains(&"ANY /x/b/back/a/".to_string()), "{rs:?}");
    assert!(
        !rs.iter()
            .any(|r| r.contains("<mount:") || r.contains("admin") || r.contains("/r/")),
        "{rs:?}"
    );
    assert!(rs.iter().all(|r| r.starts_with("ANY /")), "{rs:?}");
}

/// GD.41, Flask: a blueprint mounted cross-file at `/svc` by imported name,
/// one registered by module attribute with no `url_prefix` at its own
/// `/orders`; each client call pairs exactly.
#[test]
fn flask_register_blueprint_mounts_and_pairs() {
    let (_td, m) = fixture_pair(
        "py-flask-register-blueprint",
        &["app.py", "svc/items.py", "svc/orders.py"],
    );
    assert_eq!(
        routes(&m),
        [
            "DELETE /svc/items/<int:item_id>",
            "GET /health",
            "GET /orders/recent",
            "GET /svc/items",
        ]
    );
    assert_eq!(paired(&m, "GET:/svc/items"), ["GET /svc/items"]);
    assert_eq!(
        paired(&m, "DELETE:/svc/items/"),
        ["DELETE /svc/items/<int:item_id>"]
    );
    assert_eq!(paired(&m, "/orders/recent"), ["GET /orders/recent"]);
}

/// GD.41, FastAPI: `users.router` and a router subclass's `flows.router`
/// included into a prefixed package router, itself included by aliased name
/// at `/svc`; each client call pairs exactly.
#[test]
fn fastapi_include_router_chain_mounts_and_pairs() {
    let (_td, m) = fixture_pair(
        "py-fastapi-include-router",
        &[
            "main.py",
            "core/base.py",
            "routers/__init__.py",
            "routers/users.py",
            "routers/flows.py",
        ],
    );
    assert_eq!(
        routes(&m),
        [
            "GET /health",
            "GET /svc/core/users",
            "POST /svc/core/users",
            "POST /svc/core/v1/flows/filter",
        ]
    );
    assert_eq!(paired(&m, "GET:/svc/core/users"), ["GET /svc/core/users"]);
    assert_eq!(paired(&m, "POST:/svc/core/users"), ["POST /svc/core/users"]);
    assert_eq!(
        paired(&m, "/flows/filter"),
        ["POST /svc/core/v1/flows/filter"]
    );
}

/// GD.42, Express: each router at its mount path, the app's own route
/// unchanged, the named handler kept; each client call pairs exactly.
#[test]
fn express_use_mounts_and_pairs() {
    let (_td, m) = fixture_pair(
        "ts-express-use-mount",
        &["app.ts", "routes/users.js", "routes/items.ts"],
    );
    assert_eq!(
        routes(&m),
        [
            "GET /svc/health/ping",
            "GET /svc/items/all",
            "GET /svc/users/list",
            "GET /version",
            "POST /svc/users",
        ]
    );
    assert_eq!(
        position(&m, "GET /svc/users/list").as_deref(),
        Some("routes/users.js:4")
    );
    assert_eq!(paired(&m, "GET:/svc/users/list"), ["GET /svc/users/list"]);
    assert_eq!(paired(&m, "POST:/svc/users"), ["POST /svc/users"]);
    assert_eq!(paired(&m, "GET:/svc/items/all"), ["GET /svc/items/all"]);
    assert_eq!(paired(&m, "GET:/svc/health/ping"), ["GET /svc/health/ping"]);
}

/// GD.42, Hono: chained sub-apps mounted by imported name, two levels deep
/// for the auth app; each client call pairs exactly.
#[test]
fn hono_route_mounts_and_pairs() {
    let (_td, m) = fixture_pair(
        "ts-hono-route-mount",
        &["router.ts", "api/auth.ts", "api/files/files.ts"],
    );
    assert_eq!(
        routes(&m),
        [
            "GET /healthz",
            "GET /svc/auth/session",
            "GET /svc/files/item/:id",
            "POST /svc/auth/sign-out",
            "POST /svc/files/upload-pdf",
        ]
    );
    assert_eq!(
        position(&m, "POST /svc/files/upload-pdf").as_deref(),
        Some("api/files/files.ts:6")
    );
    assert_eq!(
        paired(&m, "POST:/svc/files/upload-pdf"),
        ["POST /svc/files/upload-pdf"]
    );
    assert_eq!(
        paired(&m, "GET:/svc/auth/session"),
        ["GET /svc/auth/session"]
    );
    assert_eq!(
        paired(&m, "POST:/svc/auth/sign-out"),
        ["POST /svc/auth/sign-out"]
    );
}

/// GD.42, NestJS: the global prefix on every controller route, the
/// RouterModule path below it for AdminModule's controller, the excluded
/// `health` at its own path, the decorated handler kept; each client call
/// pairs exactly.
#[test]
fn nest_global_prefix_and_router_module_pair() {
    let (_td, m) = fixture_pair(
        "ts-nest-global-prefix",
        &[
            "src/main.ts",
            "src/app.module.ts",
            "src/admin/admin.module.ts",
            "src/admin/users.controller.ts",
            "src/health/health.module.ts",
            "src/health/health.controller.ts",
        ],
    );
    assert_eq!(
        routes(&m),
        [
            "GET /health",
            "GET /svc/backoffice/users/all",
            "GET /svc/version",
            "POST /svc/backoffice/users",
        ]
    );
    assert_eq!(
        paired(&m, "GET:/svc/backoffice/users/all"),
        ["GET /svc/backoffice/users/all"]
    );
    assert_eq!(
        paired(&m, "POST:/svc/backoffice/users"),
        ["POST /svc/backoffice/users"]
    );
    assert_eq!(paired(&m, "GET:/svc/version"), ["GET /svc/version"]);
    assert_eq!(paired(&m, "GET:/health"), ["GET /health"]);
    let route = m
        .node_id_by_qname("GET /svc/backoffice/users/all")
        .expect("route");
    let handled: Vec<String> = m
        .graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(m.cross_edges.iter())
        .filter(|e| e.from == route && e.category == edge_category::HANDLED_BY)
        .filter_map(|e| {
            m.graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&e.to).cloned())
        })
        .collect();
    assert_eq!(
        handled,
        ["src::admin::users.controller::UsersController::all"]
    );
}
