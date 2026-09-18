//! LB.4a — the owner segment on HTTP qnames, over REAL builds.
//!
//! A ROUTE / ENDPOINT / page node whose file lies under a NESTED project root
//! is qualified with ` @<project path>`. Before it, two services of one repo
//! serving `/health` were ONE `GET /health` node HANDLED_BY both handlers
//! (measured at aa91f3c on the `bench/substrate-gap/fixtures/
//! http-monorepo-owner` layout, which [`write_monorepo`] reproduces: `bench/`
//! is outside the cargo workspace, so the layout is written into a tempdir).

use std::collections::BTreeSet;
use std::path::Path;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{EdgeCategoryId, NodeId, NodeKindId};
use repo_graph_engine::{
    GenerateResult, ParseCache, generate_one, generate_one_incremental, generate_one_with_cache,
    service_map,
};
use repo_graph_graph::MergedGraph;

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn flask(handler: &str) -> String {
    format!(
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
         @app.route(\"/health\")\ndef {handler}():\n    return {{\"ok\": True}}\n"
    )
}

/// services/admin + services/users (flask, each its own pyproject.toml) both
/// serve `/health`; web/ (package.json) calls `http://users-svc:8080/health`.
fn write_monorepo(dir: &Path) {
    write(dir, "services/admin/app.py", &flask("admin_health"));
    write(dir, "services/admin/pyproject.toml", "[project]\nname = \"admin\"\n");
    write(dir, "services/users/app.py", &flask("users_health"));
    write(dir, "services/users/pyproject.toml", "[project]\nname = \"users\"\n");
    write(dir, "web/package.json", "{\"name\": \"web\"}\n");
    write(
        dir,
        "web/api.ts",
        "import axios from 'axios';\n\nconst USERS_API = 'http://users-svc:8080';\n\n\
         export async function ping() {\n  return axios.get(`${USERS_API}/health`);\n}\n",
    );
}

/// Every `(qname, id)` of `kind`, first graph entry per id.
fn qnames_of(m: &MergedGraph, kind: NodeKindId) -> BTreeSet<String> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter_map(move |n| {
                (g.nav.kind_by_id.get(&n.id) == Some(&kind))
                    .then(|| g.nav.qname_by_id.get(&n.id).cloned())
                    .flatten()
            })
        })
        .collect()
}

fn qname(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// `(from qname, to qname)` for every edge of `category`, sorted.
fn edges(m: &MergedGraph, category: EdgeCategoryId) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = m
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| (qname(m, e.from), qname(m, e.to)))
        .collect();
    out.sort();
    out
}

fn build(dir: &Path) -> GenerateResult {
    generate_one(dir.to_str().unwrap()).expect("generate_one")
}

#[test]
fn monorepo_services_keep_their_own_routes() {
    let td = tempfile::tempdir().unwrap();
    write_monorepo(td.path());
    let m = build(td.path()).merged;

    let routes = qnames_of(&m, node_kind::ROUTE);
    assert_eq!(
        routes.iter().map(String::as_str).collect::<Vec<_>>(),
        ["GET /health @services/admin", "GET /health @services/users"],
        "one ROUTE per service, each qualified with its project"
    );
    assert_eq!(
        edges(&m, edge_category::HANDLED_BY),
        [
            (
                "GET /health @services/admin".to_string(),
                "services::admin::app::admin_health".to_string()
            ),
            (
                "GET /health @services/users".to_string(),
                "services::users::app::users_health".to_string()
            ),
        ],
        "each route HANDLED_BY exactly its own handler"
    );
    let endpoints = qnames_of(&m, node_kind::ENDPOINT);
    assert_eq!(
        endpoints.iter().map(String::as_str).collect::<Vec<_>>(),
        ["endpoint:GET:/health @web"]
    );
    // Pairing ignores owners: the client reaches BOTH routes until LB.4b
    // narrows by project.
    assert_eq!(
        edges(&m, edge_category::HTTP_CALLS),
        [
            (
                "endpoint:GET:/health @web".to_string(),
                "GET /health @services/admin".to_string()
            ),
            (
                "endpoint:GET:/health @web".to_string(),
                "GET /health @services/users".to_string()
            ),
        ]
    );
    // Display names are unchanged.
    for g in &m.graphs {
        for (id, q) in &g.nav.qname_by_id {
            if q.starts_with("GET /health @") || q.starts_with("endpoint:GET:/health @") {
                assert_eq!(g.nav.name_by_id.get(id).map(String::as_str), Some("GET /health"));
            }
        }
    }
}

/// A repo whose only manifest is at its root keeps today's qnames exactly.
#[test]
fn single_project_repo_is_untouched() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    write(d, "pyproject.toml", "[project]\nname = \"shop\"\n");
    write(
        d,
        "api/app.py",
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
         @app.route(\"/users\")\ndef list_users():\n    return []\n",
    );
    write(
        d,
        "web/api.ts",
        "import axios from 'axios';\n\nexport async function load() {\n  return axios.get('/users');\n}\n",
    );
    let m = build(d).merged;
    assert_eq!(
        qnames_of(&m, node_kind::ROUTE).into_iter().collect::<Vec<_>>(),
        ["GET /users"]
    );
    assert_eq!(
        qnames_of(&m, node_kind::ENDPOINT).into_iter().collect::<Vec<_>>(),
        ["endpoint:GET:/users"]
    );
    assert_eq!(
        edges(&m, edge_category::HTTP_CALLS),
        [("endpoint:GET:/users".to_string(), "GET /users".to_string())]
    );
}

/// Two SPAs with a page on the same path are two `page:` nodes (LB.4c's
/// namespace, now split by project).
#[test]
fn pages_of_two_apps_split_by_project() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    let app = "import { Routes, Route } from \"react-router-dom\";\n\n\
               function Users() {\n  return <div>users</div>;\n}\n\n\
               export default function App() {\n  return (\n    <Routes>\n      \
               <Route path=\"/users\" element={<Users />} />\n    </Routes>\n  );\n}\n";
    for p in ["apps/admin", "apps/shop"] {
        write(d, &format!("{p}/package.json"), "{\"name\": \"x\"}\n");
        write(d, &format!("{p}/App.tsx"), app);
    }
    let m = build(d).merged;
    let pages: Vec<String> = qnames_of(&m, node_kind::ROUTE)
        .into_iter()
        .filter(|q| q.starts_with("page:"))
        .collect();
    assert_eq!(pages, ["page:/users @apps/admin", "page:/users @apps/shop"]);
}

/// The owner pass runs post-cache, so an incremental build (cold, then warm
/// off the persisted parse cache) writes the same `.gmap` bytes as a clean one.
#[test]
fn incremental_matches_clean() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("mono");
    std::fs::create_dir_all(&repo).unwrap();
    write_monorepo(&repo);
    let path = repo.to_str().unwrap();

    let write_gmap = |r: &GenerateResult, name: &str| {
        let out = td.path().join(name);
        repo_graph_store::write_merged_sharded(&r.merged, &out).expect("write .gmap");
        let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&out)
            .unwrap()
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
            .collect();
        files.sort();
        files
    };
    let cold = generate_one_incremental(path).expect("cold incremental");
    let warm = generate_one_incremental(path).expect("warm incremental");
    // The warm build really is served from the persisted cache.
    let persisted = ParseCache::load(path);
    assert!(persisted.len() >= 3, "every parsed file cached: {}", persisted.len());
    let mut mem = ParseCache::new();
    generate_one_with_cache(path, &mut mem).expect("cold in-memory");
    let warm_mem = generate_one_with_cache(path, &mut mem).expect("warm in-memory");
    assert_eq!(mem.stats.reparsed, 0, "the second in-memory build reparses nothing");
    assert!(mem.stats.reused >= 3);
    let clean = generate_one(path).expect("clean");
    assert!(
        qnames_of(&warm.merged, node_kind::ROUTE).contains("GET /health @services/users"),
        "the warm build qualifies cache-served parses too"
    );
    let clean_bytes = write_gmap(&clean, "clean");
    assert_eq!(write_gmap(&cold, "cold"), clean_bytes, "cold incremental == clean");
    assert_eq!(write_gmap(&warm, "warm"), clean_bytes, "warm incremental == clean");
    assert_eq!(write_gmap(&warm_mem, "warm_mem"), clean_bytes, "cache-served == clean");
}

/// `glia arch` on the monorepo: each service owns its own route. HEAD
/// (measured): admin 1, users 0 — the shared node was placed in admin.
#[test]
fn service_map_counts_one_route_per_service() {
    let td = tempfile::tempdir().unwrap();
    write_monorepo(td.path());
    let r = build(td.path());
    let map = service_map(&r.merged, &r.repo_labels);
    assert_eq!(map.keying, "project_roots");
    let routes = |id: &str| {
        map.services
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.routes)
            .unwrap_or_else(|| panic!("no service {id}"))
    };
    assert_eq!((routes("services/admin"), routes("services/users")), (1, 1));
    let mut links: Vec<(String, String, String)> = map
        .links
        .iter()
        .map(|l| (l.from.clone(), l.to.clone(), l.channel.clone()))
        .collect();
    links.sort();
    assert_eq!(
        links,
        [
            ("web".to_string(), "services/admin".to_string(), "GET /health".to_string()),
            ("web".to_string(), "services/users".to_string(), "GET /health".to_string()),
        ],
        "channels read the owner-free path"
    );
}
