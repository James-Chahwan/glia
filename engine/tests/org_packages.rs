//! FA.22: org-internal package routing. An importer's IMPORT_BINDINGS row
//! (FA.21) naming the package of a sibling PROJECT's `manifest_name` SAME_AS
//! class gets one IMPORTS cross edge (EVIDENCE `resolver:org_packages`, rule
//! `org_import`, Confidence::Medium) to the module the import resolves to in
//! that project: npm by the index / src/index convention, Go by the package
//! directory's file. Joint build, a monorepo (the build's own binding is kept,
//! never doubled), an undeclared dependency (nothing) and a layout merge of
//! the single-repo layouts (the joint build's edges).
//!
//! FA.23: the importer's unresolved call sites and references through the
//! names those rows bound become CALLS (rule `org_call`) / USES (`org_ref`)
//! cross edges to the sibling's declarations, for npm (named and namespace
//! imports, a barrel, an ambiguous barrel, package.json `main` / `exports`),
//! Go, cargo and pypi; a layout merge draws the joint build's edges, blast
//! radius walks them into the importing repo, and a one-repo Cargo workspace
//! (the Rust build binds its own crates) gains none.

use std::path::Path;

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_core::{Confidence, NodeId};
use glia_engine::merge::{MergeMember, merge_layouts, read_workspace};
use glia_engine::persist::persist_result;
use glia_engine::{BlastOptions, GenerateResult, blast_radius, generate_many};
use glia_graph::{MergedGraph, Reach};

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

/// (from qname, to qname, category, rule, file, 0-based line) of every
/// org_call / org_ref edge, checking each one's confidence and emitter.
fn org_bound(m: &MergedGraph) -> Vec<(String, String, String, String, String, u32)> {
    let mut out: Vec<_> = m
        .cross_edges
        .iter()
        .filter_map(|e| {
            let ev = Evidence::of(e)?;
            matches!(ev.rule.as_deref(), Some("org_call" | "org_ref")).then_some((e, ev))
        })
        .map(|(e, ev)| {
            assert_eq!(e.confidence, Confidence::Medium);
            assert_eq!(ev.emitter, "resolver:org_packages");
            let category = if e.category == edge_category::CALLS {
                "CALLS"
            } else if e.category == edge_category::USES {
                "USES"
            } else {
                "OTHER"
            };
            (
                qname(m, e.from),
                qname(m, e.to),
                category.to_string(),
                ev.rule.unwrap_or_default(),
                ev.file.unwrap_or_default(),
                ev.line.unwrap_or(u32::MAX),
            )
        })
        .collect();
    out.sort();
    out
}

fn row(
    from: &str,
    to: &str,
    category: &str,
    rule: &str,
    file: &str,
    line: u32,
) -> (String, String, String, String, String, u32) {
    (
        from.to_string(),
        to.to_string(),
        category.to_string(),
        rule.to_string(),
        file.to_string(),
        line,
    )
}

/// Every org_* edge (import, call or reference).
fn any_org(m: &MergedGraph) -> usize {
    m.cross_edges
        .iter()
        .filter(|e| {
            Evidence::of(e)
                .and_then(|ev| ev.rule)
                .is_some_and(|r| r.starts_with("org_"))
        })
        .count()
}

#[test]
fn npm_and_go_calls_bind_to_the_siblings_declarations() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, lib) = (tmp.path().join("app"), tmp.path().join("lib"));
    write_app(&app, true);
    write_lib(&lib);
    // A namespace import beside the named one, and a by-value reference.
    write(
        &app,
        "src/ns.ts",
        "import * as lib from '@acme/lib';\nimport { greet } from '@acme/lib';\nexport function hello() {\n  return lib.greet('y');\n}\nexport const handlers = [greet];\n",
    );
    let r = build(&[&app, &lib]);
    assert_eq!(
        org_bound(&r.merged),
        [
            row(
                "go::main::Run",
                "go::text::text::Upper",
                "CALLS",
                "org_call",
                "go/main.go",
                5
            ),
            row(
                "src::main::run",
                "src::index::greet",
                "CALLS",
                "org_call",
                "src/main.ts",
                1
            ),
            row(
                "src::ns::handlers",
                "src::index::greet",
                "USES",
                "org_ref",
                "src/ns.ts",
                5
            ),
            row(
                "src::ns::hello",
                "src::index::greet",
                "CALLS",
                "org_call",
                "src/ns.ts",
                3
            ),
        ]
    );
    // The sites stay unresolved in the importer's own graph: the cross edge
    // is the binding, so a per-repo graph is the same with or without a
    // sibling.
    let unresolved = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| &g.unresolved_calls)
        .filter(|c| qname(&r.merged, c.from) == "src::main::run")
        .count();
    assert_eq!(unresolved, 1);
}

#[test]
fn npm_barrel_reexport_binds_and_an_ambiguous_barrel_binds_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, lib) = (tmp.path().join("app"), tmp.path().join("lib"));
    write_app(&app, true);
    write(&lib, "package.json", r#"{"name":"@acme/lib"}"#);
    write(&lib, "src/index.ts", "export * from './greet';\n");
    write(
        &lib,
        "src/greet.ts",
        "export function greet(n: string) { return 'hi ' + n; }\n",
    );
    let r = build(&[&app, &lib]);
    assert_eq!(
        org_bound(&r.merged),
        [row(
            "src::main::run",
            "src::greet::greet",
            "CALLS",
            "org_call",
            "src/main.ts",
            1
        )]
    );

    // Two star sources both declaring `greet`: the barrel's level names two
    // declarations, so nothing binds.
    write(
        &lib,
        "src/index.ts",
        "export * from './greet';\nexport * from './other';\n",
    );
    write(
        &lib,
        "src/other.ts",
        "export function greet(n: string) { return 'yo ' + n; }\n",
    );
    let r = build(&[&app, &lib]);
    assert!(
        org_bound(&r.merged).is_empty(),
        "{:?}",
        org_bound(&r.merged)
    );
    // The npm import still routes (the Go one has no sibling module here).
    assert_eq!(
        org_imports(&r.merged)
            .iter()
            .map(|i| (i.0.as_str(), i.1.as_str()))
            .collect::<Vec<_>>(),
        [("src::main", "src::index")]
    );
}

#[test]
fn npm_routes_through_package_json_main_and_exports() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, lib) = (tmp.path().join("app"), tmp.path().join("lib"));
    write_app(&app, true);
    write(
        &app,
        "src/fmt.ts",
        "import { shout } from '@acme/lib/fmt';\nexport function loud() { return shout('x'); }\n",
    );
    // `main` names the build output: mapped back to its source, a file the
    // index / src/index convention would never find.
    write(
        &lib,
        "package.json",
        r#"{"name":"@acme/lib","main":"dist/entry.js","exports":{"./fmt":"./dist/text/fmt.js"}}"#,
    );
    write(
        &lib,
        "src/entry.ts",
        "export function greet(n: string) { return 'hi ' + n; }\n",
    );
    write(
        &lib,
        "src/text/fmt.ts",
        "export function shout(s: string) { return s; }\n",
    );
    let r = build(&[&app, &lib]);
    let imports: Vec<(String, String)> = org_imports(&r.merged)
        .into_iter()
        .filter(|i| i.0.starts_with("src::"))
        .map(|i| (i.0, i.1))
        .collect();
    assert_eq!(
        imports,
        [
            ("src::fmt".to_string(), "src::text::fmt".to_string()),
            ("src::main".to_string(), "src::entry".to_string()),
        ]
    );
    assert_eq!(
        org_bound(&r.merged),
        [
            row(
                "src::fmt::loud",
                "src::text::fmt::shout",
                "CALLS",
                "org_call",
                "src/fmt.ts",
                1
            ),
            row(
                "src::main::run",
                "src::entry::greet",
                "CALLS",
                "org_call",
                "src/main.ts",
                1
            ),
        ]
    );
}

/// A Cargo crate `app` depending on `org-lib` (the sibling `lib`).
fn write_cargo(app: &Path, lib: &Path) {
    write(
        app,
        "Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\norg-lib = \"1\"\n",
    );
    write(
        app,
        "src/lib.rs",
        "use org_lib::util::helper;\nuse org_lib::util;\n\npub fn run() -> u8 {\n    helper() + util::helper()\n}\n",
    );
    write(
        lib,
        "Cargo.toml",
        "[package]\nname = \"org-lib\"\nversion = \"0.1.0\"\n",
    );
    write(lib, "src/lib.rs", "pub mod util;\n");
    write(lib, "src/util.rs", "pub fn helper() -> u8 {\n    1\n}\n");
}

/// A Python project `papp` depending on the distribution `org-lib` (the
/// sibling `plib`, top-level package `org_lib`).
fn write_pypi(app: &Path, lib: &Path) {
    write(
        app,
        "pyproject.toml",
        "[project]\nname = \"papp\"\ndependencies = [\"org-lib\"]\n",
    );
    write(
        app,
        "main.py",
        "from org_lib.util import helper\nimport org_lib.util as u\n\n\ndef run():\n    return helper() + u.helper()\n",
    );
    write(lib, "pyproject.toml", "[project]\nname = \"org-lib\"\n");
    write(lib, "org_lib/__init__.py", "");
    write(lib, "org_lib/util.py", "def helper():\n    return 1\n");
}

#[test]
fn cargo_and_pypi_calls_route_through_the_crate_root_and_top_package() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs: Vec<_> = ["app", "lib", "papp", "plib"]
        .iter()
        .map(|d| tmp.path().join(d))
        .collect();
    write_cargo(&dirs[0], &dirs[1]);
    write_pypi(&dirs[2], &dirs[3]);
    let r = build(&dirs.iter().map(|d| d.as_path()).collect::<Vec<_>>());
    assert_eq!(
        org_bound(&r.merged),
        [
            row(
                "main::run",
                "org_lib::util::helper",
                "CALLS",
                "org_call",
                "main.py",
                5
            ),
            row(
                "src::lib::run",
                "src::util::helper",
                "CALLS",
                "org_call",
                "src/lib.rs",
                4
            ),
        ]
    );
    let imports: Vec<(String, String)> = org_imports(&r.merged)
        .into_iter()
        .map(|i| (i.0, i.1))
        .collect();
    assert_eq!(
        imports,
        [
            ("main".to_string(), "org_lib::util".to_string()),
            ("src::lib".to_string(), "src::lib".to_string()),
            ("src::lib".to_string(), "src::util".to_string()),
        ]
    );
}

#[test]
fn pypi_distribution_named_otherwise_routes_through_its_package_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, lib) = (tmp.path().join("papp"), tmp.path().join("plib"));
    write(
        &app,
        "pyproject.toml",
        "[project]\nname = \"papp\"\ndependencies = [\"acme-yaml-tools\"]\n",
    );
    write(
        &app,
        "main.py",
        "from ayaml import load\n\n\ndef run():\n    return load('x')\n",
    );
    write(
        &lib,
        "pyproject.toml",
        "[project]\nname = \"acme-yaml-tools\"\n",
    );
    write(
        &lib,
        "src/ayaml/__init__.py",
        "def load(s):\n    return s\n",
    );
    let r = build(&[&app, &lib]);
    assert_eq!(
        org_bound(&r.merged),
        [row(
            "main::run",
            "src::ayaml::__init__::load",
            "CALLS",
            "org_call",
            "main.py",
            4
        )]
    );
}

#[test]
fn layout_merge_binds_calls_as_the_joint_build() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs: Vec<_> = ["app", "lib", "capp", "clib", "papp", "plib"]
        .iter()
        .map(|d| tmp.path().join(d))
        .collect();
    write_app(&dirs[0], true);
    write_lib(&dirs[1]);
    write_cargo(&dirs[2], &dirs[3]);
    write_pypi(&dirs[4], &dirs[5]);
    let mut members = Vec::new();
    for (i, d) in dirs.iter().enumerate() {
        let out = tmp.path().join(format!("out{i}"));
        persist_result(&build(&[d.as_path()]), &out, "test").unwrap();
        members.push(gmap(&format!("m{i}"), &out));
    }
    let joint = build(&dirs.iter().map(|d| d.as_path()).collect::<Vec<_>>());
    let m = merge_layouts(&members).unwrap();
    assert_eq!(org_bound(&m.result.merged), org_bound(&joint.merged));
    assert_eq!(org_bound(&joint.merged).len(), 4);
}

#[test]
fn blast_radius_of_the_siblings_declaration_reaches_the_importer() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, lib) = (tmp.path().join("app"), tmp.path().join("lib"));
    write_app(&app, true);
    write_lib(&lib);
    let r = build(&[&app, &lib]);
    let mut o = BlastOptions::default();
    o.direction = Reach::Backward;
    o.depth = 2;
    let answer = blast_radius(&r.merged, &["src::index::greet"], &o);
    let hit = answer
        .results
        .iter()
        .find(|a| a.qname == "src::main::run")
        .unwrap_or_else(|| {
            panic!(
                "run is in greet's radius: {:?}",
                answer.results.iter().map(|a| &a.qname).collect::<Vec<_>>()
            )
        });
    assert_eq!(hit.reason, "CALLS");
}

#[test]
fn one_repo_cargo_workspace_gains_no_org_edge() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"app\", \"lib\"]\nresolver = \"2\"\n",
    );
    write_cargo(&tmp.path().join("app"), &tmp.path().join("lib"));
    write(
        tmp.path(),
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\norg-lib = { path = \"../lib\" }\n",
    );
    let r = build(&[tmp.path()]);
    // Control: the class forms (the dependency and the crate are one repo's),
    // and the Rust build bound the call through its own crates.
    let classes = r
        .merged
        .cross_edges
        .iter()
        .filter(|e| Evidence::of(e).and_then(|ev| ev.rule).as_deref() == Some("manifest_name"))
        .count();
    assert_eq!(classes, 1, "control: the manifest_name class forms");
    assert_eq!(any_org(&r.merged), 0, "{:?}", org_bound(&r.merged));
}
