//! LB.10a acceptance: every C/C++ file is its own MODULE, named by its full
//! file name (`src::Widget.h`, `src::Widget.cpp`), a quoted `#include` binds
//! that exact file, a `.h` is parsed as C++ unless the C grammar parses it
//! with fewer errors, declarations inside include guards are visited, and a
//! bodiless class / struct specifier (a forward declaration) mints no type.
//!
//! Before LB.10a the header / implementation pair was ONE MODULE `src::Widget`
//! with a self-IMPORTS edge, every include-guarded header was empty (so no
//! CLASS Widget), and `class Gadget;` minted CLASS `src::main::Gadget`. Built
//! on a tempdir copy of the `cpp-header-impl-files` fixture.
//!
//! LB.10b (`types_take_their_cpp_name`, on `cpp-namespace-qnames`): a type a
//! header declares is named by its C++ name - the namespace path
//! (`shop::Cart`), or the header's directory in the global namespace
//! (`src::Widget`) - a source file's type keeps the file scope
//! (`src::Widget.cpp::Local`), and an out-of-line member definition is a
//! METHOD, bound to its class when the same file defines it.
//!
//! LB.10c (`out_of_line_members_meet_their_class`, on
//! `cpp-out-of-line-members`): an out-of-line member defined in another file
//! than its class joins the class its header declares at graph build (one
//! METHOD under one CLASS, `this->x()` resolves, the defining file's static
//! helper stays callable), and a namespace-qualified definition is a
//! FUNCTION of its file.

use std::collections::HashMap;
use std::path::Path;

use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/cpp-header-impl-files";
const NAMESPACE_FIXTURE: &str = "../bench/substrate-gap/fixtures/cpp-namespace-qnames";

/// Copy the fixture's sources (not its key.json, not a stray `.ai` cache)
/// into `dst`.
fn copy_fixture(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            if entry.file_name() != ".ai" {
                std::fs::create_dir_all(&to).expect("mkdir");
                copy_fixture(&from, &to);
            }
        } else if entry.file_name() != "key.json" {
            std::fs::copy(&from, &to).expect("copy fixture file");
        }
    }
}

fn fixture_copy() -> tempfile::TempDir {
    copy_of(FIXTURE)
}

fn copy_of(fixture: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("repo")).expect("mkdir repo");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture);
    copy_fixture(&src, &tmp.path().join("repo"));
    tmp
}

fn build(tmp: &tempfile::TempDir) -> MergedGraph {
    let repo = tmp.path().join("repo");
    let res = generate_one(repo.to_str().expect("utf-8 tempdir")).expect("generate_one");
    assert!(res.parse_errors.is_empty(), "{:?}", res.parse_errors);
    res.merged
}

/// id -> (qname, name, kind) over every graph of the build.
fn nav(merged: &MergedGraph) -> HashMap<NodeId, (String, String, NodeKindId)> {
    let mut out = HashMap::new();
    for g in &merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            let name = g.nav.name_by_id.get(id).cloned().unwrap_or_default();
            let Some(kind) = g.nav.kind_by_id.get(id).copied() else { continue };
            out.insert(*id, (q.clone(), name, kind));
        }
    }
    out
}

/// Every qname of `kind` with a node record, sorted.
fn qnames_of(merged: &MergedGraph, kind: NodeKindId) -> Vec<String> {
    let nav = nav(merged);
    let mut out: Vec<String> = merged
        .graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .filter_map(|n| nav.get(&n.id))
        .filter(|(_, _, k)| *k == kind)
        .map(|(q, _, _)| q.clone())
        .collect();
    out.sort();
    out
}

/// Every node name of `kind` with a node record.
fn names_of(merged: &MergedGraph, kind: NodeKindId) -> Vec<String> {
    let nav = nav(merged);
    merged
        .graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .filter_map(|n| nav.get(&n.id))
        .filter(|(_, _, k)| *k == kind)
        .map(|(_, name, _)| name.clone())
        .collect()
}

/// `(from qname, to qname)` of every edge of `category`, sorted.
fn edges(merged: &MergedGraph, category: EdgeCategoryId) -> Vec<(String, String)> {
    let nav = nav(merged);
    let q = |id: &NodeId| nav.get(id).map(|(q, _, _)| q.clone()).unwrap_or_default();
    let mut out: Vec<(String, String)> = merged
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| (q(&e.from), q(&e.to)))
        .collect();
    out.sort();
    out
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_string(), b.to_string())
}

#[test]
fn header_and_impl_are_two_modules() {
    let tmp = fixture_copy();
    let merged = build(&tmp);

    // (1) One MODULE per file, named by its full file name.
    let modules = qnames_of(&merged, node_kind::MODULE);
    for want in ["src::Widget.h", "src::Widget.cpp", "src::main.cpp", "c::point.h", "c::point.c"] {
        assert!(modules.contains(&want.to_string()), "{want} missing from {modules:?}");
    }
    for gone in ["src::Widget", "c::point", "src::main"] {
        assert!(!modules.contains(&gone.to_string()), "{gone} still a MODULE: {modules:?}");
    }
    // The stem stays each MODULE's display name.
    let nav = nav(&merged);
    for (q, name, kind) in nav.values() {
        if *kind == node_kind::MODULE && q.starts_with("src::Widget.") {
            assert_eq!(name, "Widget", "{q}");
        }
    }

    // (2) Each quoted include binds the header FILE; no self-import remains.
    let imports = edges(&merged, edge_category::IMPORTS);
    for (from, to) in [
        ("src::Widget.cpp", "src::Widget.h"),
        ("src::main.cpp", "src::Widget.h"),
        ("c::point.c", "c::point.h"),
    ] {
        assert!(imports.contains(&pair(from, to)), "{from} -> {to} missing: {imports:?}");
    }
    assert!(imports.iter().all(|(f, t)| f != t), "self-IMPORTS: {imports:?}");
    // `<stdlib.h>` is an angle include: no edge.
    assert_eq!(imports.len(), 3, "{imports:?}");
    // Every IMPORTS edge is a located site (LC.3b evidence, carried through).
    for e in merged.all_edges().filter(|e| e.category == edge_category::IMPORTS) {
        let ev = Evidence::of(e).expect("IMPORTS edge carries EVIDENCE");
        assert_eq!(ev.basis, Basis::Site, "{ev:?}");
    }

    // (3) The include-guarded header is walked and read as C++: CLASS Widget
    // with its inline method; the C header's struct is there too. (LB.10b:
    // a header's global types take its directory, not its file, as scope.)
    assert!(names_of(&merged, node_kind::CLASS).contains(&"Widget".to_string()));
    let methods = qnames_of(&merged, node_kind::METHOD);
    assert!(methods.contains(&"src::Widget::helper".to_string()), "{methods:?}");
    let structs = qnames_of(&merged, node_kind::STRUCT);
    assert_eq!(structs, ["c::point"], "the declaration's `struct point` mints nothing");
    // A header misread by the C grammar turns `class Widget {..}` into a
    // FUNCTION named Widget.
    assert!(!names_of(&merged, node_kind::FUNCTION).contains(&"Widget".to_string()));

    // (4) `class Gadget;` is a forward declaration: no node at all.
    assert!(
        nav.values().all(|(_, name, _)| name != "Gadget"),
        "a node named Gadget: {:?}",
        nav.values().filter(|(_, n, _)| n == "Gadget").collect::<Vec<_>>()
    );

    // (5) Free functions follow their file's MODULE; `Widget::run` is an
    // out-of-line member, a METHOD at the header class's qname (LB.10b).
    let functions = qnames_of(&merged, node_kind::FUNCTION);
    for want in ["c::point.c::point_new", "src::main.cpp::main"] {
        assert!(functions.contains(&want.to_string()), "{want} missing: {functions:?}");
    }
    assert!(methods.contains(&"src::Widget::run".to_string()), "{methods:?}");

    // (6) The IMPORTS-cell local filter still knows every quoted include is
    // the repo's own header: the file-named MODULE keeps its stem as nav
    // name, so the index holds the bare `c::point` / `src::Widget`.
    for module in ["c::point.c", "src::main.cpp", "src::Widget.cpp"] {
        let id = nav
            .iter()
            .find(|(_, (q, _, k))| q == module && *k == node_kind::MODULE)
            .map(|(id, _)| *id)
            .unwrap_or_else(|| panic!("MODULE {module}"));
        let cells: Vec<&str> = merged
            .graphs
            .iter()
            .flat_map(|g| &g.nodes)
            .filter(|n| n.id == id)
            .flat_map(|n| &n.cells)
            .filter(|c| c.kind == cell_type::IMPORTS)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) | CellPayload::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(cells, ["[]"], "{module}");
    }
}

/// The whole store, file by file, for a byte comparison.
fn store_bytes(merged: &MergedGraph, dir: &Path) -> Vec<(String, Vec<u8>)> {
    glia_store::write_merged_sharded(merged, dir).expect("write_merged_sharded");
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .expect("read store dir")
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                std::fs::read(e.path()).expect("read shard"),
            )
        })
        .collect();
    out.sort();
    out
}

/// The C/C++ rule is a pure function of the walked file list, so a cold and
/// a warm incremental build write the same store as a clean one.
#[test]
fn incremental_builds_match_a_clean_build() {
    let tmp = fixture_copy();
    let repo = tmp.path().join("repo");
    let repo = repo.to_str().expect("utf-8 tempdir");
    let clean = build(&tmp);
    let cold = glia_engine::generate_one_incremental(repo).expect("cold").merged;
    let warm = glia_engine::generate_one_incremental(repo).expect("warm").merged;
    let clean_bytes = store_bytes(&clean, &tmp.path().join("clean"));
    assert_eq!(store_bytes(&cold, &tmp.path().join("cold")), clean_bytes, "cold vs clean");
    assert_eq!(store_bytes(&warm, &tmp.path().join("warm")), clean_bytes, "warm vs clean");
}

/// LB.10b on `cpp-namespace-qnames`: header types by C++ name, source-file
/// types file-local, an in-file out-of-line member bound to its class (so
/// `this->b()` resolves), the others provisional METHODs at the header
/// rule's qname.
#[test]
fn types_take_their_cpp_name() {
    let tmp = copy_of(NAMESPACE_FIXTURE);
    let merged = build(&tmp);
    let classes = qnames_of(&merged, node_kind::CLASS);
    let structs = qnames_of(&merged, node_kind::STRUCT);
    let methods = qnames_of(&merged, node_kind::METHOD);

    for want in ["src::Widget", "shop::Cart", "src::Widget.cpp::Local"] {
        assert!(classes.contains(&want.to_string()), "CLASS {want} missing: {classes:?}");
    }
    assert!(structs.contains(&"src::Point".to_string()), "{structs:?}");
    for want in [
        "src::Widget::helper",
        "shop::Cart::tax",
        "src::Widget.cpp::Local::a",
        "src::Widget.cpp::Local::b",
        "src::Widget::run",
        "shop::Cart::total",
    ] {
        assert!(methods.contains(&want.to_string()), "METHOD {want} missing: {methods:?}");
    }
    for q in classes.iter().chain(&structs) {
        assert!(
            !q.starts_with("src::Widget.h::") && !q.starts_with("include::shop::cart.hpp::"),
            "a header type scoped by its file: {q}"
        );
    }
    // No out-of-line definition is left a FUNCTION.
    let functions = qnames_of(&merged, node_kind::FUNCTION);
    assert!(functions.is_empty(), "{functions:?}");

    let calls = edges(&merged, edge_category::CALLS);
    assert!(
        calls.contains(&pair("src::Widget.cpp::Local::a", "src::Widget.cpp::Local::b")),
        "{calls:?}"
    );
    // The in-file-bound member hangs off its class: DEFINES Local -> a.
    let defines = edges(&merged, edge_category::DEFINES);
    assert!(
        defines.contains(&pair("src::Widget.cpp::Local", "src::Widget.cpp::Local::a")),
        "{defines:?}"
    );
    // The defining file keeps its DEFINES after LB.10c joins them to the
    // header class, which DEFINES them too.
    assert!(defines.contains(&pair("src::Widget.cpp", "src::Widget::run")), "{defines:?}");
    assert!(defines.contains(&pair("src::cart.cpp::shop", "shop::Cart::total")), "{defines:?}");
    assert!(defines.contains(&pair("src::Widget", "src::Widget::run")), "{defines:?}");
    assert!(defines.contains(&pair("shop::Cart", "shop::Cart::total")), "{defines:?}");
}

const OUT_OF_LINE_FIXTURE: &str = "../bench/substrate-gap/fixtures/cpp-out-of-line-members";

/// The nav parent's (qname, kind) of the node with `qname` and `kind`.
fn nav_parent(merged: &MergedGraph, qname: &str, kind: NodeKindId) -> Option<(String, NodeKindId)> {
    let nav = nav(merged);
    let (id, _) = nav.iter().find(|(_, (q, _, k))| q == qname && *k == kind)?;
    let parent = merged.graphs.iter().find_map(|g| g.nav.parent_of.get(id))?;
    nav.get(parent).map(|(q, _, k)| (q.clone(), *k))
}

/// LB.10c on `cpp-out-of-line-members`: `void Widget::run() {}` in
/// `Widget.cpp` and `int Cart::total() {}` inside `namespace shop {}` of
/// `src/cart.cpp` (the class in `include/shop/cart.hpp`) join the header
/// classes: nav parent the CLASS, `this->helper()` / `this->tax()` bind
/// through it, and `file_helper()` (a static of `Widget.cpp`) still binds
/// from the moved method. `void shop::init() {}` names a namespace: FUNCTION
/// `src::cart.cpp::shop::init`, never the provisional METHOD.
#[test]
fn out_of_line_members_meet_their_class() {
    let tmp = copy_of(OUT_OF_LINE_FIXTURE);
    let merged = build(&tmp);

    assert_eq!(
        nav_parent(&merged, "src::Widget::run", node_kind::METHOD),
        Some(("src::Widget".to_string(), node_kind::CLASS))
    );
    assert_eq!(
        nav_parent(&merged, "shop::Cart::total", node_kind::METHOD),
        Some(("shop::Cart".to_string(), node_kind::CLASS))
    );
    let calls = edges(&merged, edge_category::CALLS);
    for (from, to) in [
        ("src::Widget::run", "src::Widget::helper"),
        ("src::Widget::run", "src::Widget.cpp::file_helper"),
        ("shop::Cart::total", "shop::Cart::tax"),
    ] {
        assert!(calls.contains(&pair(from, to)), "CALLS {from} -> {to} missing: {calls:?}");
    }

    let functions = qnames_of(&merged, node_kind::FUNCTION);
    assert!(functions.contains(&"src::cart.cpp::shop::init".to_string()), "{functions:?}");
    let methods = qnames_of(&merged, node_kind::METHOD);
    assert!(!methods.contains(&"src::shop::init".to_string()), "{methods:?}");
    // No out-of-line member is left named `Q::m`.
    let nav = nav(&merged);
    for (q, name, kind) in nav.values() {
        if *kind == node_kind::METHOD || *kind == node_kind::FUNCTION {
            assert!(!name.contains("::"), "{q} is named {name}");
        }
    }

    // Every edge carries its EVIDENCE cell (LC.3a), the new CLASS -> METHOD
    // DEFINES included.
    for e in merged.all_edges() {
        assert!(Evidence::of(e).is_some(), "edge without EVIDENCE: {e:?}");
    }
    let joined = merged
        .all_edges()
        .filter(|e| e.category == edge_category::DEFINES)
        .filter_map(Evidence::of)
        .filter(|ev| ev.emitter == "graph:cpp_members")
        .count();
    assert_eq!(joined, 2, "Widget -> run, Cart -> total");
}
