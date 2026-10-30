//! FH.2a - `health::health` (boundary health) end to end, on small trees.
//!
//! - `paired_app_is_ok`: a gin server serving ten routes and a TS client that
//!   calls each of them: HTTP reaches every client and every route, role
//!   `app`, verdict OK.
//! - `test_site_endpoints_counted_apart` (FH.26): the paired app plus a
//!   `_test.go` helper driving the router through `(method, path)`: its
//!   test-site ENDPOINTs (ORIGIN `test_fixture`, FH.17) are `test_site_producers`,
//!   in neither the producers nor client reach, so HTTP stays OK.
//! - `wrapped_client_is_blind`: the same server, and a client whose calls go
//!   through a gateway wrapper that prefixes a base the server does not serve
//!   (`/gw/..`): twelve ENDPOINTs, none paired, HTTP BLIND
//!   `clients_unpaired`.
//! - `library_has_no_boundary`: a Python package with functions only: role
//!   `library`, every mechanism OK `library`, verdict OK.
//! - `unparsed_code_apart_from_docs`: `.marko` files no parser reads make the
//!   `parse` row BLIND; `.mdx` / `.rst` prose is counted apart and alone never
//!   raises it (FH.1b's correction); `.svelte` / `.astro` files are parsed
//!   (FH.6), so never unparsed.
//! - `workspace_package_import_unbound`: an npm workspace whose app imports two
//!   sibling packages by their manifest names: one with in-repo source binds
//!   (FH.5, IMPORTS rule `workspace_package`), one whose entry names only a
//!   build output with no source is the unresolved row.
//! - `suspicious_endpoint_qnames`: a `data:` URL fetch is a suspicious
//!   ENDPOINT.
//! - `cohort_health` (`#[ignore]`): every repo root named by
//!   `GLIA_HEALTH_REPOS` (`;`-separated), printed for the FH.1 cohort reading.

use std::path::{Path, PathBuf};

use glia_engine::health::{Health, HealthOptions, Role, Verdict, health, health_with};
use glia_engine::{GenerateResult, generate_one};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("glia_fh2a_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

fn roots_of(r: &GenerateResult) -> Vec<(u64, PathBuf)> {
    r.repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect()
}

fn build(root: &Path) -> (GenerateResult, Health) {
    let r = generate_one(root.to_str().expect("utf-8 path")).expect("build");
    let h = health(&r.merged, &roots_of(&r));
    (r, h)
}

/// A gin server serving `GET /api/r<i>` for `i` in `0..n`.
fn gin_server(root: &Path, n: usize) {
    write(
        root,
        "server/go.mod",
        "module example.com/server\n\ngo 1.22\n\nrequire github.com/gin-gonic/gin v1.9.1\n",
    );
    let mut main = String::from(
        "package main\n\nimport \"github.com/gin-gonic/gin\"\n\nfunc handle(c *gin.Context) {\n\tc.JSON(200, gin.H{})\n}\n\nfunc main() {\n\tr := gin.Default()\n",
    );
    for i in 0..n {
        main.push_str(&format!("\tr.GET(\"/api/r{i}\", handle)\n"));
    }
    main.push_str("\tr.Run()\n}\n");
    write(root, "server/main.go", &main);
}

fn web_package(root: &Path) {
    write(
        root,
        "web/package.json",
        "{\"name\":\"web\",\"version\":\"1.0.0\"}\n",
    );
}

#[test]
fn paired_app_is_ok() {
    let root = tmp("paired");
    gin_server(&root, 10);
    web_package(&root);
    let mut api = String::new();
    for i in 0..10 {
        api.push_str(&format!(
            "export async function load{i}() {{\n  return fetch('/api/r{i}');\n}}\n\n"
        ));
    }
    write(&root, "web/src/api.ts", &api);
    let (_, h) = build(&root);
    let http = h.mechanism("http").expect("http row");
    assert_eq!(h.role, Role::App, "{h:#?}");
    assert_eq!(
        (
            http.producers,
            http.consumers,
            http.paired_producers,
            http.reached_consumers
        ),
        (10, 10, 10, 10),
        "{http:#?}"
    );
    assert_eq!((http.verdict, http.reason), (Verdict::Ok, "paired"));
    assert_eq!(http.reach, Some(1.0));
    assert_eq!(h.verdict, Verdict::Ok, "{h:#?}");
    assert_eq!(h.reason, "ok");
    for m in &h.mechanisms[1..] {
        assert_eq!((m.verdict, m.reason), (Verdict::Ok, "absent"), "{m:#?}");
    }
    assert_eq!(h.parse.unparsed_files, 0);
    assert!(h.parse.parsed_files >= 2, "{:?}", h.parse);
    assert!(h.workspace_unresolved.is_empty());
    assert_eq!(h.suspicious.count, 0);
    assert!(h.dead.symbols > 0, "{:?}", h.dead);
    let marker = h.marker("cli");
    assert!(
        marker.starts_with("[health] repo=glia_fh2a_")
            && marker.contains(" role=app http=OK:1.000 grpc=OK:- rpc=OK:- graphql=OK:- queue=OK:- ws=OK:- unparsed=0 workspace_unresolved=0 suspicious=0 dead_share=")
            && marker.ends_with(" docs_unparsed=0 parse=OK verdict=OK surface=cli"),
        "{marker}"
    );
}

/// FH.26: the paired app of [`paired_app_is_ok`] plus a `_test.go` helper
/// that drives the app's own router through `(method, path)`: an inferred
/// wrapper (FH.17) whose twelve test-file sites mint ENDPOINTs stamped ORIGIN
/// `test_fixture` that pair nothing by design. They count as test sites, in
/// neither the producers nor client reach, so HTTP stays OK `paired` over the
/// ten non-test clients.
#[test]
fn test_site_endpoints_counted_apart() {
    let root = tmp("test_sites");
    gin_server(&root, 10);
    web_package(&root);
    let mut api = String::new();
    for i in 0..10 {
        api.push_str(&format!(
            "export async function load{i}() {{\n  return fetch('/api/r{i}');\n}}\n\n"
        ));
    }
    write(&root, "web/src/api.ts", &api);
    let mut tests = String::from(
        "package main\n\nimport (\n\t\"net/http\"\n\t\"net/http/httptest\"\n\t\"testing\"\n\n\t\"github.com/gin-gonic/gin\"\n)\n\nfunc PerformRequest(r http.Handler, method, path string) *httptest.ResponseRecorder {\n\treq, _ := http.NewRequest(method, path, nil)\n\tw := httptest.NewRecorder()\n\tr.ServeHTTP(w, req)\n\treturn w\n}\n\nfunc TestRoutes(t *testing.T) {\n\tr := gin.Default()\n",
    );
    for i in 0..12 {
        tests.push_str(&format!("\tPerformRequest(r, \"GET\", \"/api/t{i}\")\n"));
    }
    tests.push_str("}\n");
    write(&root, "server/api_test.go", &tests);
    let (_, h) = build(&root);
    let http = h.mechanism("http").expect("http row");
    assert_eq!(h.role, Role::App, "{h:#?}");
    assert!(http.test_site_producers >= 12, "{http:#?}");
    assert_eq!(
        (http.producers, http.paired_producers, http.consumers),
        (10, 10, 10),
        "{http:#?}"
    );
    assert_eq!(http.client_reach, Some(1.0), "{http:#?}");
    assert_eq!((http.verdict, http.reason), (Verdict::Ok, "paired"));
    assert!(
        h.marker("").contains(" role=app http=OK:1.000 "),
        "{}",
        h.marker("")
    );
}

#[test]
fn wrapped_client_is_blind() {
    let root = tmp("wrapped");
    gin_server(&root, 12);
    web_package(&root);
    write(
        &root,
        "web/src/gateway.ts",
        "export function gw(path: string) {\n  return fetch('/gw' + path);\n}\n",
    );
    let mut api = String::from("import { gw } from './gateway';\n\n");
    for i in 0..12 {
        api.push_str(&format!(
            "export async function load{i}() {{\n  return fetch('/gw/api/v{i}');\n}}\n\n"
        ));
    }
    write(&root, "web/src/api.ts", &api);
    let (_, h) = build(&root);
    let http = h.mechanism("http").expect("http row");
    assert_eq!(h.role, Role::App);
    assert!(http.producers >= 12, "{http:#?}");
    assert_eq!(http.paired_producers, 0, "{http:#?}");
    assert_eq!(
        (http.verdict, http.reason),
        (Verdict::Blind, "clients_unpaired"),
        "{http:#?}"
    );
    assert_eq!(http.reach, Some(0.0));
    assert_eq!(
        http.measure,
        format!("clients paired 0% of {}", http.producers)
    );
    assert_eq!(h.verdict, Verdict::Blind);
    assert!(
        h.reason
            .starts_with("http: clients_unpaired (clients paired 0% of "),
        "{}",
        h.reason
    );
    let mut opts = HealthOptions::default();
    opts.labels = roots_of(&generate_one(root.to_str().expect("utf-8")).expect("build"))
        .into_iter()
        .map(|(id, _)| (id, "shop".to_string()))
        .collect();
    opts.surface = "py";
    let r = generate_one(root.to_str().expect("utf-8")).expect("build");
    let h2 = health_with(&r.merged, &roots_of(&r), &opts);
    assert_eq!(h2.repos, vec!["shop".to_string()]);
    assert!(
        h2.marker("py")
            .starts_with("[health] repo=shop role=app http=BLIND:0.000 "),
        "{}",
        h2.marker("py")
    );
}

#[test]
fn library_has_no_boundary() {
    let root = tmp("library");
    write(
        &root,
        "pyproject.toml",
        "[project]\nname = \"mathlib\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root,
        "mathlib/__init__.py",
        "def add(a, b):\n    return a + b\n\n\ndef twice(a):\n    return add(a, a)\n",
    );
    write(
        &root,
        "tests/test_add.py",
        "from mathlib import add\n\n\ndef test_add():\n    assert add(1, 2) == 3\n",
    );
    let (_, h) = build(&root);
    assert_eq!(h.role, Role::Library, "{h:#?}");
    for m in &h.mechanisms {
        assert_eq!((m.verdict, m.reason), (Verdict::Ok, "library"), "{m:#?}");
    }
    assert_eq!(h.verdict, Verdict::Ok);
    assert!(h.marker("").contains(" role=library http=OK:- "));
    assert!(h.marker("").ends_with(" surface=engine"));
}

#[test]
fn unparsed_code_apart_from_docs() {
    let root = tmp("marko");
    write(
        &root,
        "package.json",
        "{\"name\":\"app\",\"version\":\"1.0.0\"}\n",
    );
    write(
        &root,
        "src/lib/api.ts",
        "export function hello() {\n  return 1;\n}\n",
    );
    for i in 0..12 {
        write(
            &root,
            &format!("src/components/Page{i}.marko"),
            "class {\n  onCreate() {\n    this.state = { n: 0 };\n  }\n}\n<p>${state.n}</p>\n",
        );
    }
    // FH.6: single-file components the SFC parser reads are code, never
    // unparsed.
    write(
        &root,
        "src/routes/+page.svelte",
        "<script>\n  let n = 0;\n</script>\n<p>{n}</p>\n",
    );
    write(
        &root,
        "src/pages/index.astro",
        "---\nconst n = 0;\n---\n<p>{n}</p>\n",
    );
    for i in 0..30 {
        write(&root, &format!("docs/page{i}.mdx"), "# Page\n\nText.\n");
    }
    write(&root, "docs/guide.rst", "Guide\n=====\n");
    // Gitignored and collapsed trees are not walked.
    write(&root, ".gitignore", "build/\n");
    write(&root, "build/Out.marko", "<p>x</p>\n");
    write(&root, "node_modules/x/Dep.marko", "<p>x</p>\n");
    let (_, h) = build(&root);
    assert_eq!(h.parse.unparsed.get("marko"), Some(&12), "{:?}", h.parse);
    assert_eq!(h.parse.unparsed.get("svelte"), None, "{:?}", h.parse);
    assert_eq!(h.parse.unparsed.get("astro"), None, "{:?}", h.parse);
    assert_eq!(h.parse.unparsed_files, 12);
    assert_eq!(h.parse.docs_unparsed.get("mdx"), Some(&30));
    assert_eq!(h.parse.docs_unparsed.get("rst"), Some(&1));
    assert_eq!(h.parse.docs_unparsed_files, 31);
    assert_eq!(
        (h.parse.verdict, h.parse.reason),
        (Verdict::Blind, "code_unparsed")
    );
    assert_eq!(h.verdict, Verdict::Blind);
    assert!(
        h.reason
            .starts_with("parse: code_unparsed (12 code files unparsed (.marko 12) of ")
    );
    assert!(h.marker("").contains(" unparsed=12 "));
    assert!(
        h.marker("")
            .contains(" docs_unparsed=31 parse=BLIND verdict=BLIND ")
    );

    // Prose alone raises nothing.
    let docs = tmp("docs_only");
    write(&docs, "src/lib.ts", "export const x = 1;\n");
    for i in 0..40 {
        write(&docs, &format!("content/p{i}.mdx"), "# P\n");
    }
    let (_, d) = build(&docs);
    assert_eq!(d.parse.unparsed_files, 0);
    assert_eq!(d.parse.docs_unparsed_files, 40);
    assert_eq!((d.parse.verdict, d.parse.reason), (Verdict::Ok, "parsed"));
    assert_eq!(d.verdict, Verdict::Ok, "{d:#?}");
}

#[test]
fn workspace_package_import_unbound() {
    let root = tmp("workspace");
    write(
        &root,
        "package.json",
        "{\"name\":\"mono\",\"private\":true,\"workspaces\":[\"packages/*\",\"apps/*\"]}\n",
    );
    // FH.5: a package whose entry names only a build output, with no source
    // in the repo, stays unbound; one with source binds and is no row.
    write(
        &root,
        "packages/sdk/package.json",
        "{\"name\":\"@acme/sdk\",\"version\":\"1.0.0\",\"main\":\"dist/index.js\"}\n",
    );
    write(
        &root,
        "packages/sdk/README.md",
        "# sdk\n\nBuilt elsewhere.\n",
    );
    write(
        &root,
        "packages/ui/package.json",
        "{\"name\":\"@acme/ui\",\"version\":\"1.0.0\",\"main\":\"src/index.ts\"}\n",
    );
    write(
        &root,
        "packages/ui/src/index.ts",
        "export function label(id: string) {\n  return id;\n}\n",
    );
    write(
        &root,
        "apps/web/package.json",
        "{\"name\":\"@acme/web\",\"version\":\"1.0.0\",\"dependencies\":{\"@acme/sdk\":\"*\",\"@acme/ui\":\"*\"}}\n",
    );
    write(
        &root,
        "apps/web/src/main.ts",
        "import { getUser } from '@acme/sdk';\nimport { label } from '@acme/ui';\nimport { thing } from 'left-pad';\n\nexport function show() {\n  return label(getUser('1'));\n}\n",
    );
    let (_, h) = build(&root);
    let names: Vec<&str> = h
        .workspace_unresolved
        .iter()
        .map(|w| w.package.as_str())
        .collect();
    assert_eq!(names, vec!["@acme/sdk"], "{:#?}", h.workspace_unresolved);
    let w = &h.workspace_unresolved[0];
    assert_eq!(
        (w.ecosystem.as_str(), w.path.as_str()),
        ("npm", "packages/sdk")
    );
    assert_eq!(w.importers, vec!["apps/web/src/main.ts".to_string()]);
    assert!(h.marker("").contains(" workspace_unresolved=1 "));
}

#[test]
fn suspicious_endpoint_qnames() {
    let root = tmp("suspicious");
    web_package(&root);
    write(
        &root,
        "web/src/img.ts",
        "export async function pixel() {\n  return fetch('data:image/png;base64,iVBORw0KGgo=');\n}\n\nexport async function spaced() {\n  return fetch('/api/a b');\n}\n\nexport async function users() {\n  return fetch('/api/users');\n}\n",
    );
    let (_, h) = build(&root);
    // FH.4a: a data: URL mints no ENDPOINT at all, so the whitespace path is
    // the one suspicious node.
    assert_eq!(h.suspicious.count, 1, "{:#?}", h.suspicious);
    assert!(
        h.suspicious.examples[0].contains("a b"),
        "{:?}",
        h.suspicious
    );
}

#[test]
#[ignore = "reads the repo roots GLIA_HEALTH_REPOS names (FH.1 cohort copies)"]
fn cohort_health() {
    let Ok(list) = std::env::var("GLIA_HEALTH_REPOS") else {
        return;
    };
    for root in list.split(';').filter(|s| !s.is_empty()) {
        let r = generate_one(root).expect("build");
        let t = std::time::Instant::now();
        let h = health(&r.merged, &roots_of(&r));
        println!("  health_ms={}", t.elapsed().as_millis());
        println!("{}", h.marker("cohort"));
        println!("  reason: {}", h.reason);
        println!("  role_basis: {:?}", h.role_basis);
        for m in &h.mechanisms {
            println!(
                "  {} {} {} producers={} consumers={} pairs={} external={} :: {}",
                m.mechanism,
                m.verdict.as_str(),
                m.reason,
                m.producers,
                m.consumers,
                m.pairs,
                m.external_producers,
                m.measure
            );
        }
        println!("  parse: {} ({})", h.parse.reason, h.parse.measure);
        println!("  docs_unparsed: {:?}", h.parse.docs_unparsed);
        println!(
            "  workspace_unresolved: {:?}",
            h.workspace_unresolved
                .iter()
                .map(|w| (&w.package, w.importers.len()))
                .collect::<Vec<_>>()
        );
        println!("  dead: {:?}", h.dead);
    }
}
