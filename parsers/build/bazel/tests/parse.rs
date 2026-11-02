//! HT.5: `parse_targets`, snapshot rows -> the build-target graph.
//!
//! The rows are bazelbuild/examples' cpp-tutorial/stage3 (bazel 9.3.1) and
//! go-tutorial/stage3 (bazel 7.4.0), as glia-snapshots' `targets_sync`
//! normalises the real `bazel query '//...:*'` output under
//! snapshots/tests/data/bazel/ (Apache-2.0, see that directory's NOTICE),
//! written inline: no test data is shared across crates.

use glia_build_domain::BuildGraph;
use glia_build_domain::cell_type::{ATTRS, LOCATION, VIA};
use glia_build_domain::edge_category::{DEPENDS_ON, HAS_SRC, IN_PACKAGE, RUNTIME_DEP};
use glia_build_domain::node_kind::{EXTERNAL, PACKAGE, SOURCE, TARGET};
use glia_build_domain::snapshot::{KIND_GENERATED_FILE, KIND_RULE, KIND_SOURCE_FILE, TargetRow};
use glia_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId, RepoId};
use glia_parser_bazel::{ATTR_EDGES, parse, parse_targets};

fn repo() -> RepoId {
    RepoId::from_canonical("build://bazel-examples")
}

fn source(ws: &str, label: &str, file: &str) -> TargetRow {
    let mut r = TargetRow::new(ws, label, KIND_SOURCE_FILE);
    r.file = Some(file.to_string());
    r.line = Some(0);
    r
}

fn rule(
    ws: &str,
    label: &str,
    class: &str,
    at: (&str, u32),
    labels: &[(&str, &[&str])],
) -> TargetRow {
    let mut r = TargetRow::new(ws, label, KIND_RULE);
    r.rule_class = Some(class.to_string());
    r.file = Some(at.0.to_string());
    r.line = Some(at.1);
    r.testonly = class.ends_with("_test");
    for (attr, ls) in labels {
        r.labels
            .insert(attr.to_string(), ls.iter().map(|s| s.to_string()).collect());
    }
    r
}

fn generated(ws: &str, label: &str, at: (&str, u32)) -> TargetRow {
    let mut r = TargetRow::new(ws, label, KIND_GENERATED_FILE);
    r.file = Some(at.0.to_string());
    r.line = Some(at.1);
    r
}

/// cpp-tutorial/stage3 as workspace `cpp`: 3 rules, 7 source files, 2
/// outputs.
fn cpp() -> Vec<TargetRow> {
    let mut time = rule(
        "cpp",
        "//lib:hello-time",
        "cc_library",
        ("cpp/lib/BUILD", 2),
        &[
            ("hdrs", &["//lib:hello-time.h"]),
            ("srcs", &["//lib:hello-time.cc"]),
        ],
    );
    time.visibility = vec!["//main:__pkg__".to_string()];
    vec![
        source("cpp", "//lib:BUILD", "cpp/lib/BUILD"),
        time,
        source("cpp", "//lib:hello-time.cc", "cpp/lib/hello-time.cc"),
        source("cpp", "//lib:hello-time.h", "cpp/lib/hello-time.h"),
        source("cpp", "//main:BUILD", "cpp/main/BUILD"),
        rule(
            "cpp",
            "//main:hello-greet",
            "cc_library",
            ("cpp/main/BUILD", 3),
            &[
                ("hdrs", &["//main:hello-greet.h"]),
                ("srcs", &["//main:hello-greet.cc"]),
            ],
        ),
        source("cpp", "//main:hello-greet.cc", "cpp/main/hello-greet.cc"),
        source("cpp", "//main:hello-greet.h", "cpp/main/hello-greet.h"),
        rule(
            "cpp",
            "//main:hello-world",
            "cc_binary",
            ("cpp/main/BUILD", 9),
            &[
                (
                    "deps",
                    &[
                        "//lib:hello-time",
                        "//main:hello-greet",
                        "@rules_cc//:link_extra_lib",
                    ],
                ),
                ("srcs", &["//main:hello-world.cc"]),
            ],
        ),
        source("cpp", "//main:hello-world.cc", "cpp/main/hello-world.cc"),
        generated("cpp", "//main:hello-world.dwp", ("cpp/main/BUILD", 9)),
        generated("cpp", "//main:hello-world.stripped", ("cpp/main/BUILD", 9)),
    ]
}

/// go-tutorial/stage3 as workspace `go`: 3 rules, 5 source files.
fn go() -> Vec<TargetRow> {
    let mut lib = rule(
        "go",
        "//fortune:fortune",
        "go_library",
        ("go/fortune/BUILD", 2),
        &[("srcs", &["//fortune:fortune.go"])],
    );
    lib.visibility = vec!["//visibility:public".to_string()];
    vec![
        source("go", "//:BUILD", "go/BUILD"),
        rule(
            "go",
            "//:print_fortune",
            "go_binary",
            ("go/BUILD", 2),
            &[
                ("deps", &["//fortune:fortune"]),
                ("srcs", &["//:print_fortune.go"]),
            ],
        ),
        source("go", "//:print_fortune.go", "go/print_fortune.go"),
        source("go", "//fortune:BUILD", "go/fortune/BUILD"),
        lib,
        source("go", "//fortune:fortune.go", "go/fortune/fortune.go"),
        rule(
            "go",
            "//fortune:fortune_test",
            "go_test",
            ("go/fortune/BUILD", 9),
            &[
                ("embed", &["//fortune:fortune"]),
                ("srcs", &["//fortune:fortune_test.go"]),
            ],
        ),
        source(
            "go",
            "//fortune:fortune_test.go",
            "go/fortune/fortune_test.go",
        ),
    ]
}

fn both() -> Vec<TargetRow> {
    let mut rows = cpp();
    rows.extend(go());
    rows
}

fn id(g: &BuildGraph, kind: NodeKindId, ws: &str, label: &str) -> NodeId {
    g.id(kind, ws, label)
        .unwrap_or_else(|| panic!("no {kind:?} node {ws:?} {label}"))
}

/// The VIA of the one `category` edge `from -> to`.
fn via(g: &BuildGraph, from: NodeId, to: NodeId, category: EdgeCategoryId) -> String {
    let edges: Vec<_> = g
        .edges
        .iter()
        .filter(|e| e.from == from && e.to == to && e.category == category)
        .collect();
    assert_eq!(edges.len(), 1, "one edge per (from, to, category)");
    BuildGraph::via(edges[0]).join(",")
}

fn json(g: &BuildGraph, node: NodeId, t: glia_core::CellTypeId) -> Option<String> {
    match g.cell(node, t)? {
        CellPayload::Json(s) => Some(s.clone()),
        other => panic!("not a Json cell: {other:?}"),
    }
}

fn count(g: &BuildGraph, category: EdgeCategoryId) -> usize {
    g.edges.iter().filter(|e| e.category == category).count()
}

#[test]
fn attribute_table_names_every_edge_kind() {
    let of = |attr: &str| ATTR_EDGES.iter().find(|(a, _)| *a == attr).map(|(_, c)| *c);
    for attr in [
        "deps",
        "embed",
        "actual",
        "exports",
        "runtime_deps",
        "implementation_deps",
    ] {
        assert_eq!(of(attr), Some(DEPENDS_ON), "{attr}");
    }
    assert_eq!(of("data"), Some(RUNTIME_DEP));
    for attr in ["srcs", "hdrs", "textual_hdrs"] {
        assert_eq!(of(attr), Some(HAS_SRC), "{attr}");
    }
    assert_eq!(of("malloc"), None);
    assert_eq!(ATTR_EDGES.len(), 10);
}

#[test]
fn both_workspaces_parse_to_counted_nodes_and_edges() {
    let parsed = parse(repo(), &both());
    let s = parsed.stats();
    // cpp: 3 rules; go: 3 rules.
    assert_eq!(s.targets, 6);
    // cpp: //lib, //main; go: //, //fortune.
    assert_eq!(s.packages, 4);
    // cpp: 7 source files + 2 outputs; go: 5 source files.
    assert_eq!(s.sources, 14);
    // cpp's @rules_cc//:link_extra_lib.
    assert_eq!(s.externals, 1);
    assert_eq!(s.dangling, 0);
    assert_eq!(s.other_label_attrs, 0);
    let g = &parsed.graph;
    assert_eq!(g.ids_of(TARGET).len(), 6);
    assert_eq!(g.ids_of(PACKAGE).len(), 4);
    assert_eq!(g.ids_of(SOURCE).len(), 14);
    assert_eq!(g.ids_of(EXTERNAL).len(), 1);
    assert_eq!(g.nodes.len(), 25);
    // DEPENDS_ON: hello-world -> hello-time, hello-greet, link_extra_lib;
    // print_fortune -> fortune; fortune_test -> fortune.
    assert_eq!(count(g, DEPENDS_ON), 5);
    // HAS_SRC: cpp 2 + 2 + 1; go 1 + 1 + 1.
    assert_eq!(count(g, HAS_SRC), 8);
    assert_eq!(count(g, RUNTIME_DEP), 0);
    // One per TARGET and SOURCE.
    assert_eq!(count(g, IN_PACKAGE), 20);
    assert_eq!(s.edges, 33);
    assert_eq!(g.edges.len(), 33);
}

#[test]
fn deps_embed_and_hdrs_become_edges_with_their_via() {
    let g = parse_targets(repo(), &both());
    let world = id(&g, TARGET, "cpp", "//main:hello-world");
    let time = id(&g, TARGET, "cpp", "//lib:hello-time");
    assert_eq!(via(&g, world, time, DEPENDS_ON), "deps");
    let test = id(&g, TARGET, "go", "//fortune:fortune_test");
    let fortune = id(&g, TARGET, "go", "//fortune:fortune");
    assert_eq!(via(&g, test, fortune, DEPENDS_ON), "embed");
    let greet = id(&g, TARGET, "cpp", "//main:hello-greet");
    let header = id(&g, SOURCE, "cpp", "//main:hello-greet.h");
    assert_eq!(via(&g, greet, header, HAS_SRC), "hdrs");
    let main = id(&g, PACKAGE, "cpp", "//main");
    assert_eq!(via(&g, greet, main, IN_PACKAGE), "");
    assert_eq!(via(&g, header, main, IN_PACKAGE), "");
    let root = id(&g, PACKAGE, "go", "//");
    let bin = id(&g, TARGET, "go", "//:print_fortune");
    assert_eq!(via(&g, bin, root, IN_PACKAGE), "");
}

#[test]
fn one_pair_named_by_two_attributes_is_one_edge() {
    let mut rows = go();
    let test = rows
        .iter_mut()
        .find(|r| r.label == "//fortune:fortune_test")
        .expect("fortune_test");
    test.labels
        .insert("deps".to_string(), vec!["//fortune:fortune".to_string()]);
    let g = parse_targets(repo(), &rows);
    let from = id(&g, TARGET, "go", "//fortune:fortune_test");
    let to = id(&g, TARGET, "go", "//fortune:fortune");
    assert_eq!(via(&g, from, to, DEPENDS_ON), "deps,embed");
}

#[test]
fn cells_locate_targets_and_sources_and_describe_targets() {
    let g = parse_targets(repo(), &both());
    let world = id(&g, TARGET, "cpp", "//main:hello-world");
    assert_eq!(
        json(&g, world, LOCATION).as_deref(),
        Some(r#"{"file":"cpp/main/BUILD","line":9}"#)
    );
    assert_eq!(
        json(&g, world, ATTRS).as_deref(),
        Some(
            r#"{"rule_class":"cc_binary","tags":[],"testonly":false,"visibility":[],"workspace":"cpp"}"#
        )
    );
    let fortune = id(&g, TARGET, "go", "//fortune:fortune");
    assert_eq!(
        json(&g, fortune, ATTRS).as_deref(),
        Some(
            r#"{"rule_class":"go_library","tags":[],"testonly":false,"visibility":["//visibility:public"],"workspace":"go"}"#
        )
    );
    let test = id(&g, TARGET, "go", "//fortune:fortune_test");
    assert!(
        json(&g, test, ATTRS)
            .expect("ATTRS")
            .contains(r#""testonly":true"#)
    );
    let cc = id(&g, SOURCE, "cpp", "//main:hello-world.cc");
    assert_eq!(
        json(&g, cc, LOCATION).as_deref(),
        Some(r#"{"file":"cpp/main/hello-world.cc","line":0}"#)
    );
    assert_eq!(json(&g, cc, ATTRS), None);
    let package = id(&g, PACKAGE, "cpp", "//main");
    assert!(g.node(package).expect("package").cells.is_empty());
    // Only edges into a node's HAS_SRC / DEPENDS_ON carry VIA; IN_PACKAGE none.
    assert!(
        g.edges
            .iter()
            .filter(|e| e.category == IN_PACKAGE)
            .all(|e| e.cells.iter().all(|c| c.kind != VIA))
    );
}

#[test]
fn an_at_label_is_external_never_dangling() {
    let parsed = parse(repo(), &cpp());
    let g = &parsed.graph;
    let world = id(g, TARGET, "cpp", "//main:hello-world");
    let ext = id(g, EXTERNAL, "cpp", "@rules_cc//:link_extra_lib");
    assert_eq!(via(g, world, ext, DEPENDS_ON), "deps");
    assert_eq!(json(g, ext, ATTRS), None, "an @ label is not dangling");
    assert_eq!(json(g, ext, LOCATION), None);

    // A bzlmod canonical name is external too.
    let mut rows = cpp();
    rows[1].labels.insert(
        "deps".to_string(),
        vec!["@@rules_cc+//cc:compiler".to_string()],
    );
    let parsed = parse(repo(), &rows);
    let time = id(&parsed.graph, TARGET, "cpp", "//lib:hello-time");
    let canonical = id(&parsed.graph, EXTERNAL, "cpp", "@@rules_cc+//cc:compiler");
    assert_eq!(via(&parsed.graph, time, canonical, DEPENDS_ON), "deps");
    assert_eq!(parsed.stats().externals, 2);
    assert_eq!(parsed.stats().dangling, 0);
}

#[test]
fn an_unknown_main_workspace_label_is_a_dangling_external() {
    let mut rows = go();
    let bin = rows
        .iter_mut()
        .find(|r| r.label == "//:print_fortune")
        .expect("print_fortune");
    bin.labels
        .insert("data".to_string(), vec!["//x:y".to_string()]);
    let parsed = parse(repo(), &rows);
    let g = &parsed.graph;
    let from = id(g, TARGET, "go", "//:print_fortune");
    let dangling = id(g, EXTERNAL, "go", "//x:y");
    assert_eq!(
        json(g, dangling, ATTRS).as_deref(),
        Some(r#"{"dangling":true}"#)
    );
    assert_eq!(via(g, from, dangling, RUNTIME_DEP), "data");
    assert_eq!(
        g.id(PACKAGE, "go", "//x"),
        None,
        "no package for a dangling label"
    );
    assert_eq!(parsed.stats().dangling, 1);
    assert_eq!(parsed.stats().externals, 1);
}

#[test]
fn a_narrowed_snapshot_still_reads_srcs_as_sources() {
    // `bazel query //...` (rules only): no source-file or output rows.
    let rules: Vec<TargetRow> = both().into_iter().filter(|r| r.kind == KIND_RULE).collect();
    let parsed = parse(repo(), &rules);
    let g = &parsed.graph;
    let s = parsed.stats();
    // Every srcs / hdrs label of the six rules: cpp 5, go 3.
    assert_eq!(s.sources, 8);
    assert_eq!(s.dangling, 0);
    let time = id(g, TARGET, "cpp", "//lib:hello-time");
    let header = id(g, SOURCE, "cpp", "//lib:hello-time.h");
    assert_eq!(via(g, time, header, HAS_SRC), "hdrs");
    assert_eq!(
        json(g, header, LOCATION).as_deref(),
        Some(r#"{"file":"cpp/lib/hello-time.h","line":0}"#)
    );
    let root_src = id(g, SOURCE, "go", "//:print_fortune.go");
    assert_eq!(
        json(g, root_src, LOCATION).as_deref(),
        Some(r#"{"file":"go/print_fortune.go","line":0}"#)
    );
    let lib = id(g, PACKAGE, "cpp", "//lib");
    assert_eq!(via(g, header, lib, IN_PACKAGE), "");
}

#[test]
fn an_attribute_outside_the_table_is_counted_and_dropped() {
    let mut rows = cpp();
    rows[1]
        .labels
        .insert("malloc".to_string(), vec!["//tools:malloc".to_string()]);
    let parsed = parse(repo(), &rows);
    assert_eq!(parsed.stats().other_label_attrs, 1);
    assert_eq!(parsed.graph.id(EXTERNAL, "cpp", "//tools:malloc"), None);
    assert_eq!(parsed.stats().edges, parse(repo(), &cpp()).stats().edges);
}

#[test]
fn two_workspaces_never_collide_on_one_label() {
    let mut rows = go();
    let mut other: Vec<TargetRow> = go()
        .into_iter()
        .map(|mut r| {
            r.workspace = "go2".to_string();
            r.file = r.file.map(|f| f.replacen("go/", "go2/", 1));
            r
        })
        .collect();
    rows.append(&mut other);
    let parsed = parse(repo(), &rows);
    assert_eq!(parsed.stats().targets, 6);
    assert_ne!(
        id(&parsed.graph, TARGET, "go", "//fortune:fortune"),
        id(&parsed.graph, TARGET, "go2", "//fortune:fortune")
    );
}

#[test]
fn shuffled_rows_give_an_equal_graph() {
    let rows = both();
    let first = parse_targets(repo(), &rows);
    let mut reversed = rows.clone();
    reversed.reverse();
    let mut rotated = rows.clone();
    rotated.rotate_left(7);
    for other in [reversed, rotated] {
        assert_eq!(parse_targets(repo(), &other), first);
    }
    // Nodes in (kind, key) order.
    let keys: Vec<(u32, String)> = first
        .nodes
        .iter()
        .map(|n| {
            let l = first.node_label(n.id).expect("label");
            (
                l.kind.0,
                glia_build_domain::node_key(&l.workspace, &l.label),
            )
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    // Edges in the canonical order.
    let mut edges = first.edges.clone();
    edges.sort_by(glia_core::canonical_edge_cmp);
    assert_eq!(edges, first.edges);
}

#[test]
fn an_empty_snapshot_is_an_empty_graph() {
    let parsed = parse(repo(), &[]);
    assert!(parsed.graph.nodes.is_empty() && parsed.graph.edges.is_empty());
    assert_eq!(parsed.stats().targets, 0);
}
