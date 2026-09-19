//! LA.13b: a Go package is a directory. The engine gives every Go file its
//! own MODULE (the qname is the file path, `internal/store/store.go` ->
//! `internal::store::store`), so these tests parse each file the way the
//! engine does and check that an import binds the imported directory and that
//! calls resolve across all of a package's files.

use std::path::PathBuf;

use glia_core::{EdgeCategoryId, NodeId, RepoId};
use glia_graph::{RepoGraph, build_go};
use glia_parser_go::{FileParse, GRAPH_TYPE, edge_category, node_kind, parse_file};

const MODULE_PREFIX: &str = "example.com/app";

fn repo() -> RepoId {
    RepoId::from_canonical("test://go_package_dir")
}

/// The engine's `path_to_qname`: drop `.go`, `/` -> `::`.
fn parse(rel: &str, src: &str) -> FileParse {
    let qname = rel.strip_suffix(".go").unwrap_or(rel).replace('/', "::");
    parse_file(src, rel, &qname, MODULE_PREFIX, repo()).unwrap()
}

fn fixture_parses() -> Vec<FileParse> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("bench/substrate-gap/fixtures/go-package-multifile-calls");
    ["cmd/main.go", "internal/store/store.go", "internal/store/load.go"]
        .iter()
        .map(|rel| parse(rel, &std::fs::read_to_string(root.join(rel)).unwrap()))
        .collect()
}

fn module(qname: &str) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, qname)
}
fn func(qname: &str) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, qname)
}

fn has_edge(g: &RepoGraph, from: NodeId, to: NodeId, category: EdgeCategoryId) -> bool {
    g.edges.iter().any(|e| e.from == from && e.to == to && e.category == category)
}

fn targets(g: &RepoGraph, from: NodeId, category: EdgeCategoryId) -> Vec<NodeId> {
    g.edges.iter().filter(|e| e.from == from && e.category == category).map(|e| e.to).collect()
}

/// The fixture: `store.Save()` (in `store.go`, the representative file) is
/// the control; `store.Load()` sits in the package's other file and
/// `helper()` is a same-package bare call from `store.go` into `load.go`.
#[test]
fn calls_resolve_across_the_files_of_a_package() {
    let g = build_go(repo(), fixture_parses()).unwrap();
    let main = func("cmd::main::main");
    let save = func("internal::store::store::Save");
    let load = func("internal::store::load::Load");
    let helper = func("internal::store::load::helper");

    assert!(has_edge(&g, main, save, edge_category::CALLS), "control: main -> Save");
    assert!(has_edge(&g, main, load, edge_category::CALLS), "main -> Load in a sibling file");
    assert!(has_edge(&g, save, helper, edge_category::CALLS), "Save -> helper across files");
    assert!(g.unresolved_calls.is_empty(), "leftover: {:?}", g.unresolved_calls);

    // One IMPORTS edge, to the file named after the directory.
    assert_eq!(
        targets(&g, module("cmd::main"), edge_category::IMPORTS),
        vec![module("internal::store::store")]
    );
}

/// HEAD bound an import whose path matched no file through the repo-wide
/// tail fallback, to a same-named module in ANOTHER directory
/// (`legacy/store.go`). The package directory now wins: the import binds a
/// file of `internal/store/` (the first by qname, no file is named after the
/// dir) and its calls land there.
#[test]
fn import_binds_its_own_directory_not_a_same_named_module_elsewhere() {
    let parses = vec![
        parse(
            "cmd/main.go",
            "package main\n\nimport \"example.com/app/internal/store\"\n\nfunc main() {\n\tstore.Save()\n\tstore.Load()\n}\n",
        ),
        parse("internal/store/load.go", "package store\n\nfunc Load() string { return \"b\" }\n"),
        parse("internal/store/save.go", "package store\n\nfunc Save() string { return \"a\" }\n"),
        parse("legacy/store.go", "package legacy\n\nfunc Save() string { return \"old\" }\n"),
    ];
    let g = build_go(repo(), parses).unwrap();
    let main = func("cmd::main::main");
    assert_eq!(
        targets(&g, module("cmd::main"), edge_category::IMPORTS),
        vec![module("internal::store::load")]
    );
    assert_eq!(
        targets(&g, main, edge_category::CALLS),
        vec![func("internal::store::save::Save"), func("internal::store::load::Load")]
    );
    assert!(!has_edge(&g, main, func("legacy::store::Save"), edge_category::CALLS));
}

/// `init` never binds (Go allows one per file and forbids calling it), even
/// when only one file defines it; two files defining one name (a build-tag
/// pair) bind nothing; an unexported name is not reachable through an
/// import; a METHOD left under its file (receiver type not a parsed struct)
/// never answers a bare call.
#[test]
fn init_ambiguous_unexported_and_methods_never_bind() {
    let parses = vec![
        parse("pkg/a.go", "package pkg\n\nfunc init() {}\n\nfunc A() {}\n"),
        parse("pkg/b.go", "package pkg\n\nfunc init() {}\n"),
        parse("pkg/open_linux.go", "//go:build linux\n\npackage pkg\n\nfunc Open() {}\n"),
        parse("pkg/open_windows.go", "//go:build windows\n\npackage pkg\n\nfunc Open() {}\n"),
        parse(
            "pkg/status.go",
            "package pkg\n\ntype Status int\n\nfunc (s Status) String() string { return \"\" }\n",
        ),
        parse(
            "pkg/c.go",
            "package pkg\n\nfunc C() {\n\tinit()\n\tOpen()\n\tString()\n\tA()\n}\n",
        ),
        parse("solo/x.go", "package solo\n\nfunc init() {}\n"),
        parse("solo/y.go", "package solo\n\nfunc Y() { init() }\n\nfunc hidden() {}\n"),
        parse(
            "cmd/main.go",
            "package main\n\nimport \"example.com/app/solo\"\n\nfunc main() { solo.hidden() }\n",
        ),
    ];
    let g = build_go(repo(), parses).unwrap();
    let inits = [func("pkg::a::init"), func("pkg::b::init"), func("solo::x::init")];
    assert!(
        !g.edges.iter().any(|e| e.category == edge_category::CALLS && inits.contains(&e.to)),
        "no CALLS to any init"
    );
    assert_eq!(
        targets(&g, func("pkg::c::C"), edge_category::CALLS),
        vec![func("pkg::a::A")],
        "only the unique exported A binds"
    );
    assert!(targets(&g, func("cmd::main::main"), edge_category::CALLS).is_empty());
}

/// An import bound to a FILE whose qname is the import path (`a/b.go` for
/// `import ".../a/b"`, with no `a/b/` directory) is not widened to that
/// file's directory: `a/c.go` is not the imported package.
#[test]
fn file_bound_import_is_not_widened_to_its_directory() {
    let parses = vec![
        parse("a/b.go", "package a\n\nfunc B() {}\n"),
        parse("a/c.go", "package a\n\nfunc X() {}\n"),
        parse(
            "cmd/main.go",
            "package main\n\nimport \"example.com/app/a/b\"\n\nfunc main() {\n\tb.B()\n\tb.X()\n}\n",
        ),
    ];
    let g = build_go(repo(), parses).unwrap();
    let main = func("cmd::main::main");
    assert_eq!(targets(&g, module("cmd::main"), edge_category::IMPORTS), vec![module("a::b")]);
    assert_eq!(targets(&g, main, edge_category::CALLS), vec![func("a::b::B")], "B as before, no X");
}

/// `_test.go` files are built only by `go test`: a test file's bare call
/// reaches the package's files, a non-test file never binds into a test file,
/// an importer never sees one, and an import of a directory binds its
/// non-test file even when a test file sorts first (grpc-go's
/// `binarylog/binarylog_end2end_test.go` importing `binarylog`, whose other
/// file is `sink.go`, bound itself without this rule).
#[test]
fn test_files_are_seen_only_by_test_files() {
    let parses = vec![
        parse("store/a_test.go", "package store\n\nfunc TestSave() { Save(); fixture() }\n\nfunc Only() {}\n"),
        parse("store/export_test.go", "package store\n\nfunc fixture() {}\n\nfunc Exported() {}\n"),
        parse("store/store.go", "package store\n\nfunc Save() { Only() }\n"),
        parse(
            "store/zz_ext_test.go",
            "package store_test\n\nimport \"example.com/app/store\"\n\nfunc TestExt() { store.Exported() }\n",
        ),
        parse(
            "cmd/main.go",
            "package main\n\nimport \"example.com/app/store\"\n\nfunc main() { store.Exported() }\n",
        ),
        parse(
            "binlog/b_end2end_test.go",
            "package binlog_test\n\nimport \"example.com/app/binlog\"\n\nfunc TestE() { binlog.Write() }\n",
        ),
        parse("binlog/sink.go", "package binlog\n\nfunc Write() {}\n"),
    ];
    let g = build_go(repo(), parses).unwrap();
    assert_eq!(
        targets(&g, func("store::a_test::TestSave"), edge_category::CALLS),
        vec![func("store::store::Save"), func("store::export_test::fixture")],
        "a test file's bare calls reach test and non-test files"
    );
    assert!(targets(&g, func("store::store::Save"), edge_category::CALLS).is_empty(), "non-test -> test");
    assert!(targets(&g, func("cmd::main::main"), edge_category::CALLS).is_empty(), "importer -> test");
    assert_eq!(
        targets(&g, func("store::zz_ext_test::TestExt"), edge_category::CALLS),
        vec![func("store::export_test::Exported")],
        "an external test package sees its directory's export_test.go"
    );
    assert_eq!(
        targets(&g, module("binlog::b_end2end_test"), edge_category::IMPORTS),
        vec![module("binlog::sink")]
    );
    assert_eq!(
        targets(&g, func("binlog::b_end2end_test::TestE"), edge_category::CALLS),
        vec![func("binlog::sink::Write")]
    );
}

/// Two builds of the same input give the same edge list, in order.
#[test]
fn package_resolution_is_deterministic() {
    let edges = |g: RepoGraph| -> Vec<(NodeId, NodeId, EdgeCategoryId)> {
        g.edges.iter().map(|e| (e.from, e.to, e.category)).collect()
    };
    let first = edges(build_go(repo(), fixture_parses()).unwrap());
    for _ in 0..4 {
        assert_eq!(edges(build_go(repo(), fixture_parses()).unwrap()), first);
    }
}
