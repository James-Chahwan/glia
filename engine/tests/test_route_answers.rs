//! DT.6b — test-file routes leave the route answers: `glia gaps` withholds an
//! `unpaired_route` row for a ROUTE only test code registers (ORIGIN
//! `test_fixture`, DT.6a), never names one as an unpaired endpoint's
//! `suffix_of=` candidate, and proposes no `suspected_edge` into one; the
//! rules prelude's `unhandled_route` excludes them, its absence and caveats
//! still those of HANDLED_BY alone.
//!
//! The tree is DT.6a's `bench/substrate-gap/fixtures/go-test-file-routes`
//! (read only, copied into a tempdir): a gin `server` whose `api_test.go`
//! registers `GET /boom`, `POST /api/auth/password/reset/:token` and
//! `GET /api/orders` (also in `main.go`), a `test/router.go` registering
//! `GET /health-test`, and the product routes `GET /api/orders`,
//! `POST /api/orders`, `GET /api/ab_test`; an Angular `web` calling the reset
//! path, `/api/boom`, `/api/ab_test` and `/api/orders`. After DT.6a alone the
//! three test-only routes were `unpaired_route` rows, the `/api/boom` endpoint
//! read `suffix_of=GET /boom (server)` / `route_prefix|edge`, two
//! `suspected_edge` rows proposed HTTP_CALLS into `GET /boom` and the reset
//! route (score 0.70, target_unpaired=yes), and `unhandled_route` listed the
//! three test routes (their handlers are closures).

use std::path::{Path, PathBuf};
use std::process::Command;

use glia_engine::gaps::{
    GapRow, GapsOptions, GapsReport, SUSPECTED_EDGE, UNPAIRED_ENDPOINT, UNPAIRED_ROUTE, gaps_report,
};
use glia_engine::merge::{MergeMember, merge_layouts};
use glia_engine::rules::{QueryAnswer, QueryOptions, QueryValue, query};
use glia_engine::{CoverageNote, GenerateResult, coverage_report, generate_many};
use glia_graph::MergedGraph;

/// The three routes only test code registers.
const TEST_ONLY: [&str; 3] = [
    "GET /boom",
    "GET /health-test",
    "POST /api/auth/password/reset/:token",
];

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

/// What a variant of the fixture adds.
#[derive(Clone, Copy)]
enum Variant {
    /// The fixture as it is.
    Plain,
    /// CJ.4's declared test paths: a `qa/routes.go` registering
    /// `GET /qa-only`, the server's `[walk] tests = ["qa/"]` and a web call to
    /// `/qa-only` (as engine/tests/test_file_routes.rs builds it).
    Declared,
    /// A product route whose handler is a closure: `GET /status` in
    /// `status.go`, a route with no handler that is no test route.
    Unhandled,
}

/// The fixture in a tempdir, with `variant`'s additions.
fn tree(variant: Variant) -> tempfile::TempDir {
    let td = tempfile::tempdir().unwrap();
    copy_tree(&fixture(), td.path());
    let server = td.path().join("server");
    match variant {
        Variant::Plain => {}
        Variant::Declared => {
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
        Variant::Unhandled => {
            std::fs::write(
                server.join("status.go"),
                "package main\n\nimport \"github.com/gin-gonic/gin\"\n\n\
                 // RegisterStatus mounts the liveness probe.\n\
                 func RegisterStatus(r *gin.Engine) {\n\tr.GET(\"/status\", func(c *gin.Context) {})\n}\n",
            )
            .unwrap();
        }
    }
    td
}

fn build_in(root: &Path) -> GenerateResult {
    let repos: Vec<String> = ["server", "web"]
        .iter()
        .map(|d| root.join(d).to_string_lossy().into_owned())
        .collect();
    generate_many(&repos).expect("go-test-file-routes builds")
}

/// A layout merge of the two repos, each built (and persisted under its own
/// `.glia/graph/`) alone first: the merge runs its passes with an empty
/// build context, so it reads the members' ORIGIN stamps.
fn merge_in(root: &Path) -> GenerateResult {
    let members: Vec<MergeMember> = ["server", "web"]
        .iter()
        .map(|d| MergeMember::Repo {
            name: d.to_string(),
            root: root.join(d),
        })
        .collect();
    merge_layouts(&members).expect("the layout merge").result
}

/// The whole report (no cut), the repos' roots given so the root
/// categories are computed too.
fn report(r: &GenerateResult) -> GapsReport {
    let roots: Vec<(u64, PathBuf)> = r
        .repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect();
    let mut o = GapsOptions::default();
    o.top_k_per_category = Some(2000);
    gaps_report(&r.merged, &roots, &o).expect("no category filter")
}

fn of<'r>(rep: &'r GapsReport, category: &str) -> Vec<&'r GapRow> {
    rep.rows.iter().filter(|r| r.category == category).collect()
}

/// A row's qname without its ` @<project>` owner segment.
fn bare(q: &str) -> &str {
    q.split(" @").next().unwrap_or(q)
}

fn route_rows(rep: &GapsReport) -> Vec<&str> {
    of(rep, UNPAIRED_ROUTE)
        .iter()
        .map(|r| bare(&r.qname))
        .collect()
}

/// `(qname, suggest, detail)` of every unpaired_endpoint row.
fn endpoint_rows(rep: &GapsReport) -> Vec<(&str, &str, &str)> {
    of(rep, UNPAIRED_ENDPOINT)
        .iter()
        .map(|r| (bare(&r.qname), r.suggest, r.detail.as_str()))
        .collect()
}

/// No row of any category is a test route or names one in its detail or
/// draft.
fn assert_no_test_route_named(rep: &GapsReport) {
    for r in &rep.rows {
        assert!(
            !TEST_ONLY.contains(&bare(&r.qname)),
            "a test route is a row: {r:?}"
        );
        for t in TEST_ONLY.iter().chain(["GET /qa-only"].iter()) {
            assert!(!r.detail.contains(&format!("suffix_of={t} ")), "{r:?}");
            assert!(!r.detail.contains(&format!("`{t}`")), "{r:?}");
            assert!(
                !r.draft
                    .as_deref()
                    .unwrap_or("")
                    .contains(&format!("\"{t}\"")),
                "{r:?}"
            );
        }
    }
}

#[test]
fn gaps_withholds_test_routes() {
    let td = tree(Variant::Plain);
    let rep = report(&build_in(td.path()));
    // DT.6a alone: the three test-only routes were rows beside it.
    assert_eq!(route_rows(&rep), ["POST /api/orders"]);
    assert_eq!(rep.count(UNPAIRED_ROUTE), 1);
    // The client calls stay gaps of the product (a client calls a route only
    // a test registers), but `/api/boom` is no longer told its route is
    // `GET /boom`: DT.6a alone read `suffix_of=GET /boom (server)`,
    // `route_prefix|edge`.
    assert_eq!(
        endpoint_rows(&rep),
        [
            (
                "endpoint:POST:/api/auth/password/reset/${…}",
                "constants",
                "no HTTP_CALLS target; caller=src::app::auth.service::AuthService::reset",
            ),
            (
                "endpoint:GET:/api/boom",
                "edge",
                "no HTTP_CALLS target; caller=src::app::auth.service::AuthService::boom",
            ),
        ]
    );
    // DT.6a alone: two rows into `POST /api/auth/password/reset/:token` and
    // `GET /boom`, score=0.70, target_unpaired=yes.
    assert!(
        of(&rep, SUSPECTED_EDGE).is_empty(),
        "{:?}",
        of(&rep, SUSPECTED_EDGE)
    );
    assert_eq!(rep.count(SUSPECTED_EDGE), 0);
    assert_no_test_route_named(&rep);
}

#[test]
fn a_declared_tests_route_is_withheld_too() {
    let td = tree(Variant::Declared);
    let rep = report(&build_in(td.path()));
    assert_eq!(route_rows(&rep), ["POST /api/orders"]);
    let qa: Vec<_> = endpoint_rows(&rep)
        .into_iter()
        .filter(|(q, ..)| q.contains("qa-only"))
        .collect();
    assert_eq!(
        qa,
        [(
            "endpoint:GET:/qa-only",
            "edge",
            "no HTTP_CALLS target; caller=src::app::qa.service::QaService::qa"
        )]
    );
    assert!(
        of(&rep, SUSPECTED_EDGE).is_empty(),
        "{:?}",
        of(&rep, SUSPECTED_EDGE)
    );
    assert_no_test_route_named(&rep);
}

/// The route rows, the endpoint rows and the suspected rows of a joint
/// build and of a layout merge of the same tree are one answer.
#[test]
fn a_layout_merge_withholds_the_same_rows() {
    let joint = tree(Variant::Plain);
    let built = report(&build_in(joint.path()));
    let td = tree(Variant::Plain);
    let merged = report(&merge_in(td.path()));
    assert_eq!(route_rows(&merged), route_rows(&built));
    assert_eq!(endpoint_rows(&merged), endpoint_rows(&built));
    assert_eq!(merged.count(SUSPECTED_EDGE), 0);
    assert_no_test_route_named(&merged);
}

fn ask(m: &MergedGraph, text: &str) -> QueryAnswer {
    query(m, text, &QueryOptions::default()).unwrap_or_else(|e| panic!("{}", e.rendered))
}

/// The answer's rows as their first value's bare qname.
fn row_qnames(a: &QueryAnswer) -> Vec<String> {
    a.results
        .iter()
        .map(|r| match r.values.first() {
            Some(QueryValue::Node { qname, .. }) => bare(qname).to_string(),
            other => panic!("a node, not {other:?}"),
        })
        .collect()
}

/// The coverage rows of every present language whose category is HANDLED_BY
/// or `*`: what the query layer caveats a negation of `handled` with.
fn handled_by_caveats(m: &MergedGraph) -> Vec<serde_json::Value> {
    coverage_report(m)
        .into_iter()
        .filter(|c: &CoverageNote| c.edge_category == "*" || c.edge_category == "HANDLED_BY")
        .map(|c| serde_json::to_value(c).expect("a note serialises"))
        .collect()
}

#[test]
fn unhandled_route_skips_test_routes() {
    // The fixture's test routes are closure-handled: no HANDLED_BY. The
    // pre-DT.6b definition, written out, still lists them.
    let td = tree(Variant::Plain);
    let m = build_in(td.path()).merged;
    let old = ask(&m, "x(r) :- node(r, \"ROUTE\"), !handled(r).");
    assert_eq!(row_qnames(&old), TEST_ONLY);
    // The prelude's no longer does; with nothing left, the absence still
    // names HANDLED_BY alone (origin/2 is a cell attribute, it reads no
    // edge).
    let a = ask(&m, "x(r) :- unhandled_route(r).");
    assert!(a.results.is_empty(), "{:?}", row_qnames(&a));
    let absence = a.absence.expect("no product route lacks a handler");
    assert_eq!(absence.mechanisms, ["HANDLED_BY"]);

    // A product route with no handler stays, caveated by HANDLED_BY only.
    let td = tree(Variant::Unhandled);
    let m = build_in(td.path()).merged;
    let a = ask(&m, "x(r) :- unhandled_route(r).");
    assert_eq!(row_qnames(&a), ["GET /status"]);
    let caveats: Vec<serde_json::Value> = a.results[0]
        .caveats
        .iter()
        .map(|c| serde_json::to_value(c).expect("a note serialises"))
        .collect();
    assert_eq!(caveats, handled_by_caveats(&m));
    assert!(a.absence.is_none());
}

const CHILD_ENV: &str = "GLIA_DT6B_CHILD_GAPS";

/// The child half of [`gaps_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`] (a tree's dir), in which case it builds that tree and runs
/// the gaps report so its stderr (with `--nocapture`) carries the markers.
#[test]
fn child_gaps_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        report(&build_in(Path::new(&dir)));
    }
}

/// The stderr lines of one fresh build + gaps report of `variant`'s tree.
fn gaps_stderr(variant: Variant) -> Vec<String> {
    let td = tree(variant);
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "--exact",
            "child_gaps_for_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, td.path())
        .output()
        .expect("re-run the test binary");
    assert!(
        out.status.success(),
        "child build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .map(String::from)
        .collect()
}

fn lines_with<'l>(lines: &'l [String], prefix: &str) -> Vec<&'l str> {
    lines
        .iter()
        .filter(|l| l.starts_with(prefix))
        .map(String::as_str)
        .collect()
}

#[test]
fn markers_count_the_withheld_routes() {
    let lines = gaps_stderr(Variant::Plain);
    assert_eq!(
        lines_with(&lines, "[gaps] unpaired_route rows withheld"),
        vec!["[gaps] unpaired_route rows withheld: 3 (test-file routes)"]
    );
    let rows = lines_with(&lines, "[gaps] rows=");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains(" unpaired_route=1 "), "{rows:?}");
    assert!(rows[0].contains(" suspected_edge=0 "), "{rows:?}");
    // The two client calls are still orphans; no test route is a target.
    assert_eq!(
        lines_with(&lines, "[suspected] "),
        vec!["[suspected] triples=1 orphans=2 candidates=0 kept=0 by="]
    );

    let lines = gaps_stderr(Variant::Declared);
    assert_eq!(
        lines_with(&lines, "[gaps] unpaired_route rows withheld"),
        vec!["[gaps] unpaired_route rows withheld: 4 (test-file routes)"]
    );

    // A product route with no handler and no caller is a row; the three
    // test routes are still withheld.
    let lines = gaps_stderr(Variant::Unhandled);
    assert_eq!(
        lines_with(&lines, "[gaps] unpaired_route rows withheld"),
        vec!["[gaps] unpaired_route rows withheld: 3 (test-file routes)"]
    );
    let rows = lines_with(&lines, "[gaps] rows=");
    assert!(rows[0].contains(" unpaired_route=2 "), "{rows:?}");
}
