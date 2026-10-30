//! FA.22: org-internal package routing. An importer's IMPORT_BINDINGS row
//! (FA.21) naming the package of a sibling PROJECT's `manifest_name` SAME_AS
//! class gets one IMPORTS cross edge (EVIDENCE `resolver:org_packages`, rule
//! `org_import`, Confidence::Medium) to the module the import resolves to in
//! that project: npm by the index / src/index convention, Go by the package
//! directory's file. Joint build, a monorepo (the build's own binding is kept,
//! never doubled), an undeclared dependency (nothing) and a layout merge of
//! the single-repo layouts (the joint build's edges).

use std::path::Path;

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_core::{Confidence, NodeId};
use glia_engine::merge::{MergeMember, merge_layouts, read_workspace};
use glia_engine::persist::persist_result;
use glia_engine::{GenerateResult, generate_many};
use glia_graph::MergedGraph;

fn write(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// The importing repo: an npm package depending on `@acme/lib` (unless
/// `declared` is false) and, in `go/`, a Go module requiring
/// `example.com/acme/lib`.
fn write_app(dir: &Path, declared: bool) {
    let deps = if declared {
        r#","dependencies":{"@acme/lib":"1.0.0"}"#
    } else {
        ""
    };
    write(dir, "package.json", &format!(r#"{{"name":"app"{deps}}}"#));
    write(
        dir,
        "src/main.ts",
        "import { greet } from '@acme/lib';\nexport function run() { return greet('x'); }\n",
    );
    let require = if declared {
        "\nrequire example.com/acme/lib v1.0.0\n"
    } else {
        ""
    };
    write(
        dir,
        "go/go.mod",
        &format!("module example.com/acme/app\n\ngo 1.22\n{require}"),
    );
    write(
        dir,
        "go/main.go",
        "package main\n\nimport \"example.com/acme/lib/text\"\n\nfunc Run() string {\n\treturn text.Upper(\"x\")\n}\n",
    );
}

/// The sibling: npm `@acme/lib` (src/index.ts) and, in `go/`, the Go module
/// `example.com/acme/lib` with package `text`.
fn write_lib(dir: &Path) {
    write(dir, "package.json", r#"{"name":"@acme/lib"}"#);
    write(
        dir,
        "src/index.ts",
        "export function greet(n: string) { return 'hi ' + n; }\n",
    );
    write(dir, "go/go.mod", "module example.com/acme/lib\n\ngo 1.22\n");
    write(
        dir,
        "go/text/text.go",
        "package text\n\nfunc Upper(s string) string {\n\treturn s\n}\n",
    );
}

fn build(dirs: &[&Path]) -> GenerateResult {
    generate_many(
        &dirs
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn qname(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// (from qname, to qname, file, 0-based line) of every org_import edge,
/// checking each one's category, confidence and emitter.
fn org_imports(m: &MergedGraph) -> Vec<(String, String, String, u32)> {
    let mut out: Vec<_> = m
        .cross_edges
        .iter()
        .filter_map(|e| {
            let ev = Evidence::of(e)?;
            (ev.rule.as_deref() == Some("org_import")).then_some((e, ev))
        })
        .map(|(e, ev)| {
            assert_eq!(e.category, edge_category::IMPORTS);
            assert_eq!(e.confidence, Confidence::Medium);
            assert_eq!(ev.emitter, "resolver:org_packages");
            (
                qname(m, e.from),
                qname(m, e.to),
                ev.file.unwrap_or_default(),
                ev.line.unwrap_or(u32::MAX),
            )
        })
        .collect();
    out.sort();
    out
}

/// Every IMPORTS edge (intra or cross) between two qnames.
fn imports_between(m: &MergedGraph, from: &str, to: &str) -> usize {
    m.graphs
        .iter()
        .flat_map(|g| &g.edges)
        .chain(&m.cross_edges)
        .filter(|e| {
            e.category == edge_category::IMPORTS && qname(m, e.from) == from && qname(m, e.to) == to
        })
        .count()
}

fn expected() -> Vec<(String, String, String, u32)> {
    vec![
        (
            "go::main".to_string(),
            "go::text::text".to_string(),
            "go/main.go".to_string(),
            2,
        ),
        (
            "src::main".to_string(),
            "src::index".to_string(),
            "src/main.ts".to_string(),
            0,
        ),
    ]
}

#[test]
fn sibling_repos_route_npm_and_go_imports() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, lib) = (tmp.path().join("app"), tmp.path().join("lib"));
    write_app(&app, true);
    write_lib(&lib);
    let r = build(&[&app, &lib]);
    assert_eq!(org_imports(&r.merged), expected());
    // Edges are only added: each importer reaches its target once.
    assert_eq!(imports_between(&r.merged, "src::main", "src::index"), 1);
    assert_eq!(imports_between(&r.merged, "go::main", "go::text::text"), 1);
}

#[test]
fn undeclared_dependency_routes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, lib, other) = (
        tmp.path().join("app"),
        tmp.path().join("lib"),
        tmp.path().join("other"),
    );
    // `app` declares nothing: no class forms at all.
    write_app(&app, false);
    write_lib(&lib);
    let r = build(&[&app, &lib]);
    assert!(org_imports(&r.merged).is_empty());
    assert_eq!(imports_between(&r.merged, "src::main", "src::index"), 0);

    // `other` declares the packages, so the classes form and route its own
    // imports; `app`, importing them undeclared, still routes nothing.
    write_app(&other, true);
    let r = build(&[&app, &lib, &other]);
    let app_repo = r.merged.graphs[0].repo;
    let repo_of = |id: NodeId| {
        r.merged
            .graphs
            .iter()
            .flat_map(|g| &g.nodes)
            .find(|n| n.id == id)
            .map(|n| n.repo)
    };
    let routed: Vec<Option<_>> = r
        .merged
        .cross_edges
        .iter()
        .filter(|e| Evidence::of(e).and_then(|ev| ev.rule).as_deref() == Some("org_import"))
        .map(|e| repo_of(e.from))
        .collect();
    assert_eq!(routed.len(), 2, "control: `other` routes both imports");
    assert!(
        routed.iter().all(|r| *r != Some(app_repo)),
        "an undeclared import routes nothing"
    );
}

#[test]
fn monorepo_keeps_the_builds_own_binding() {
    let tmp = tempfile::tempdir().unwrap();
    write_app(&tmp.path().join("app"), true);
    write_lib(&tmp.path().join("lib"));
    write(
        tmp.path(),
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["app","lib"]}"#,
    );
    let r = build(&[tmp.path()]);
    // The TS workspace pass and the Go package-directory import bind both
    // in-repo; org routing only fills what the build left unbound, so the
    // importer reaches its target exactly once and no org_import is drawn.
    assert_eq!(
        imports_between(&r.merged, "app::src::main", "lib::src::index"),
        1
    );
    assert_eq!(
        imports_between(&r.merged, "app::go::main", "lib::go::text::text"),
        1
    );
    assert!(org_imports(&r.merged).is_empty());
}

fn gmap(name: &str, dir: &Path) -> MergeMember {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("glia.workspace.json");
    let member = format!(r#"{{"name":"{name}","gmap":"{}"}}"#, dir.display());
    std::fs::write(&path, format!(r#"{{"version":1,"members":[{member}]}}"#)).unwrap();
    read_workspace(&path).unwrap().remove(0)
}

#[test]
fn layout_merge_routes_as_the_joint_build() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, lib) = (tmp.path().join("app"), tmp.path().join("lib"));
    write_app(&app, true);
    write_lib(&lib);
    let (a, b) = (tmp.path().join("out_app"), tmp.path().join("out_lib"));
    let solo_app = build(&[&app]);
    assert!(
        org_imports(&solo_app.merged).is_empty(),
        "control: one repo alone has no sibling"
    );
    persist_result(&solo_app, &a, "test").unwrap();
    persist_result(&build(&[&lib]), &b, "test").unwrap();
    let joint = build(&[&app, &lib]);
    let m = merge_layouts(&[gmap("app", &a), gmap("lib", &b)]).unwrap();
    assert_eq!(org_imports(&m.result.merged), org_imports(&joint.merged));
    assert_eq!(org_imports(&m.result.merged), expected());
}
