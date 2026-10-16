//! DT.6a — a ROUTE registered only in test code leaves the HTTP resolver's
//! route pool, and a product ROUTE is no longer stamped `test_fixture`, over
//! a REAL two-repo build.
//!
//! `bench/substrate-gap/fixtures/go-test-file-routes` (Kina's
//! `accesslog_test.go` / `apperr_test.go` / `refusal_audit_test.go` and
//! examples-glia's `orders_test.go` reduced): a gin `server` whose
//! `api_test.go` registers `GET /boom`, `POST /api/auth/password/reset/:token`
//! and `GET /api/orders` (also registered in `main.go`), a `test/router.go`
//! registering `GET /health-test`, and a product `GET /api/ab_test`; an
//! Angular `web` calling `/api/auth/password/reset/${token}`, `/api/boom`,
//! `/api/ab_test` and `/api/orders`. Before DT.6a the reset and `/api/boom`
//! calls paired the test-only routes (the second through the endpoint-prefix
//! strip), `GET /api/orders` was stamped `test_fixture` by its first POSITION
//! (`api_test.go` sorts first) and `GET /api/ab_test` by the symbol-qname rule
//! (its URL tail reads `_test`). `bench/` is outside the cargo workspace's
//! crates, so the fixture is copied into a tempdir and built there.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, NodeId};
use glia_engine::generate_many;
use glia_engine::merge::{MergeMember, merge_layouts};
use glia_graph::MergedGraph;

fn fixture() -> PathBuf {
    PathBuf::from(format!(
        "{}/../bench/substrate-gap/fixtures/go-test-file-routes",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Copy the fixture's sources (never its key.json or a stray cache).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            if e.file_name() != ".ai" && e.file_name() != ".glia" {
                copy_tree(&src, &dst);
            }
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// The fixture in a tempdir; `declared` adds a `qa/routes.go` registering
/// `GET /qa-only`, the server's `[walk] tests = ["qa/"]` and a web call to
/// `/qa-only` (CJ.4's declared test paths).
fn tree(declared: bool) -> tempfile::TempDir {
    let td = tempfile::tempdir().unwrap();
    copy_tree(&fixture(), td.path());
    if declared {
        let server = td.path().join("server");
        std::fs::create_dir_all(server.join("qa")).unwrap();
        std::fs::write(
            server.join("qa/routes.go"),
            "package qa\n\nimport \"github.com/gin-gonic/gin\"\n\n\
             // Register is the QA harness's router.\n\
             func Register(r *gin.Engine) {\n\tr.GET(\"/qa-only\", func(c *gin.Context) {})\n}\n",
        )
        .unwrap();
        std::fs::create_dir_all(server.join(".glia")).unwrap();
        std::fs::write(
            server.join(".glia/overlay.toml"),
            "version = 1\n[walk]\ntests = [\"qa/\"]\n",
        )
        .unwrap();
        std::fs::write(
            td.path().join("web/src/app/qa.service.ts"),
            "import { Injectable } from '@angular/core';\n\
             import { HttpClient } from '@angular/common/http';\n\n\
             @Injectable({ providedIn: 'root' })\n\
             export class QaService {\n  constructor(private http: HttpClient) {}\n  \
             qa() {\n    return this.http.get('/qa-only');\n  }\n}\n",
        )
        .unwrap();
    }
    td
}

fn build_in(root: &Path) -> MergedGraph {
    let repos: Vec<String> = ["server", "web"]
        .iter()
        .map(|d| root.join(d).to_string_lossy().into_owned())
        .collect();
    generate_many(&repos).expect("go-test-file-routes builds").merged
}

/// A layout merge of the two repos, each built (and persisted under its own
/// `.glia/graph/`) alone first.
fn merge_in(root: &Path) -> MergedGraph {
    let members: Vec<MergeMember> = ["server", "web"]
        .iter()
        .map(|d| MergeMember::Repo { name: d.to_string(), root: root.join(d) })
        .collect();
    merge_layouts(&members).expect("the layout merge").result.merged
}

/// A node's qname without its ` @<project>` owner segment.
fn qname(m: &MergedGraph, id: NodeId) -> Option<String> {
    m.graphs.iter().find_map(|g| {
        g.nav.qname_by_id.get(&id).map(|q| q.split(" @").next().unwrap_or(q).to_string())
    })
}

/// Every HTTP_CALLS edge as `(endpoint qname, route qname)`.
fn http_calls(m: &MergedGraph) -> BTreeSet<(String, String)> {
    m.all_edges()
        .filter(|e| e.category == edge_category::HTTP_CALLS)
        .filter_map(|e| Some((qname(m, e.from)?, qname(m, e.to)?)))
        .collect()
}

/// Every ROUTE's qname, and its ORIGIN provenances over every entry.
fn route_origins(m: &MergedGraph) -> BTreeSet<(String, String)> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ROUTE))
                .flat_map(move |n| {
                    let q = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
                    n.cells.iter().filter(|c| c.kind == cell_type::ORIGIN).filter_map(move |c| {
                        let CellPayload::Json(j) = &c.payload else { return None };
                        let v: serde_json::Value = serde_json::from_str(j).ok()?;
                        Some((q.clone(), v["provenance"].as_str()?.to_string()))
                    })
                })
        })
        .collect()
}

fn routes(m: &MergedGraph) -> BTreeSet<String> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ROUTE))
                .filter_map(move |n| g.nav.qname_by_id.get(&n.id).cloned())
        })
        .collect()
}

fn pairs(items: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    items.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

const TEST_ONLY: [&str; 3] = ["GET /boom", "GET /health-test", "POST /api/auth/password/reset/:token"];

#[test]
fn test_only_routes_pair_no_client_and_product_routes_keep_pairing() {
    let td = tree(false);
    let m = build_in(td.path());
    // Every route stays a node; only the pool shrinks.
    assert_eq!(
        routes(&m),
        [
            "GET /api/ab_test",
            "GET /api/orders",
            "GET /boom",
            "GET /health-test",
            "POST /api/auth/password/reset/:token",
            "POST /api/orders",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    );
    // HEAD also paired the reset call and `/api/boom` (through the
    // endpoint-prefix strip) with test-only routes.
    assert_eq!(
        http_calls(&m),
        pairs(&[
            ("endpoint:GET:/api/ab_test", "GET /api/ab_test"),
            ("endpoint:GET:/api/orders", "GET /api/orders"),
        ])
    );
    for (_, to) in http_calls(&m) {
        assert!(!TEST_ONLY.contains(&to.as_str()), "{to} is a test-only route");
    }
}

#[test]
fn only_test_only_routes_are_stamped_test_fixture() {
    let td = tree(false);
    let m = build_in(td.path());
    // HEAD also stamped `GET /api/orders` (first POSITION api_test.go) and
    // `GET /api/ab_test` (the symbol-qname rule).
    assert_eq!(
        route_origins(&m),
        TEST_ONLY.iter().map(|q| (q.to_string(), "test_fixture".to_string())).collect()
    );
}

#[test]
fn a_declared_tests_route_is_tagged_and_excluded() {
    let td = tree(true);
    let m = build_in(td.path());
    let origins = route_origins(&m);
    assert!(origins.contains(&("GET /qa-only".to_string(), "test_fixture".to_string())), "{origins:?}");
    assert_eq!(origins.len(), 4, "{origins:?}");
    assert_eq!(
        http_calls(&m),
        pairs(&[
            ("endpoint:GET:/api/ab_test", "GET /api/ab_test"),
            ("endpoint:GET:/api/orders", "GET /api/orders"),
        ]),
        "the /qa-only call pairs nothing"
    );
}

#[test]
fn a_layout_merge_excludes_the_same_routes() {
    let joint = tree(false);
    let built = build_in(joint.path());
    let td = tree(false);
    let merged = merge_in(td.path());
    assert_eq!(http_calls(&merged), http_calls(&built));
    assert_eq!(route_origins(&merged), route_origins(&built));
}

const CHILD_ENV: &str = "GLIA_DT6A_CHILD_BUILD";

/// The child half of [`build_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`] (`<mode>:<dir>`), in which case it builds (`build`) or
/// layout-merges (`merge`) that tree so its stderr (with `--nocapture`)
/// carries the markers.
#[test]
fn child_build_for_stderr() {
    if let Ok(arg) = std::env::var(CHILD_ENV) {
        match arg.split_once(':') {
            Some(("merge", dir)) => {
                merge_in(Path::new(dir));
            }
            Some((_, dir)) => {
                build_in(Path::new(dir));
            }
            None => panic!("bad {CHILD_ENV}: {arg}"),
        }
    }
}

/// The stderr lines of one fresh build (or merge) of the fixture.
fn build_stderr(mode: &str, declared: bool) -> Vec<String> {
    let td = tree(declared);
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_build_for_stderr", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, format!("{mode}:{}", td.path().display()))
        .output()
        .expect("re-run the test binary");
    assert!(out.status.success(), "child build failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stderr).lines().map(String::from).collect()
}

fn lines_with<'l>(lines: &'l [String], prefix: &str) -> Vec<&'l str> {
    lines.iter().filter(|l| l.starts_with(prefix)).map(String::as_str).collect()
}

#[test]
fn markers_count_the_excluded_routes() {
    let lines = build_stderr("build", false);
    assert_eq!(
        lines_with(&lines, "[http] test routes excluded"),
        vec!["[http] test routes excluded from route index: 3"]
    );
    assert_eq!(
        lines_with(&lines, "[http] routes="),
        vec!["[http] routes=3 endpoints=4 paired=2 (exact=2 any=0 eprefix=0 rprefix=0 base=0 suffix=0)"]
    );
    // HEAD: test_fixture=10 (path=8 qname=2); the qname hit left is the
    // ENDPOINT `endpoint:GET:/api/ab_test`, out of DT.6a's scope.
    let provenance = lines_with(&lines, "[provenance] test_fixture=");
    assert_eq!(provenance.len(), 1, "{provenance:?}");
    assert!(provenance[0].starts_with("[provenance] test_fixture=8 (path=7 qname=1) "), "{provenance:?}");

    // The declared pattern: one more route excluded, counted on the repo's
    // declared line with the other nodes of `qa/routes.go` (its MODULE, the
    // `Register` FUNCTION and the ROUTE, whose only site is no built-in test
    // path), never under `path=`.
    let lines = build_stderr("build", true);
    assert_eq!(
        lines_with(&lines, "[http] test routes excluded"),
        vec!["[http] test routes excluded from route index: 4"]
    );
    let declared = lines_with(&lines, "[provenance] declared tests repo=");
    assert_eq!(declared.len(), 1, "{declared:?}");
    assert!(declared[0].ends_with(" patterns=1 tagged=3"), "{declared:?}");
    let provenance = lines_with(&lines, "[provenance] test_fixture=");
    assert!(provenance[0].starts_with("[provenance] test_fixture=11 (path=7 qname=1) "), "{provenance:?}");

    // A layout merge: each member build, then the merge's own resolve.
    let lines = build_stderr("merge", false);
    let excluded = lines_with(&lines, "[http] test routes excluded");
    assert_eq!(excluded.last(), Some(&"[http] test routes excluded from route index: 3"), "{excluded:?}");
    let http = lines_with(&lines, "[http] routes=");
    assert_eq!(
        http.last(),
        Some(&"[http] routes=3 endpoints=4 paired=2 (exact=2 any=0 eprefix=0 rprefix=0 base=0 suffix=0)"),
        "{http:?}"
    );
}
