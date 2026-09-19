//! LB.10a acceptance: every C/C++ file is its own MODULE, named by its full
//! file name (`src::Widget.h`, `src::Widget.cpp`), a quoted `#include` binds
//! that exact file, a `.h` is parsed as C++ unless the C grammar parses it
//! with fewer errors, declarations inside include guards are visited, and a
//! bodiless class / struct specifier (a forward declaration) mints no type.
//!
//! Before LB.10a the header / implementation pair was ONE MODULE `src::Widget`
//! with a self-IMPORTS edge, every include-guarded header was empty (so no
//! CLASS Widget), and `class Gadget;` minted CLASS `src::main::Gadget`. Built
//! on a tempdir copy of the `cpp-header-impl-files` fixture. LB.10b / LB.10c
//! add their tests here.

use std::collections::HashMap;
use std::path::Path;

use repo_graph_code_domain::evidence::{Basis, Evidence};
use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use repo_graph_engine::generate_one;
use repo_graph_graph::MergedGraph;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/cpp-header-impl-files";

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
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("repo")).expect("mkdir repo");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
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
    // with its inline method; the C header's struct is there too.
    assert!(names_of(&merged, node_kind::CLASS).contains(&"Widget".to_string()));
    let methods = qnames_of(&merged, node_kind::METHOD);
    assert!(methods.contains(&"src::Widget.h::Widget::helper".to_string()), "{methods:?}");
    let structs = qnames_of(&merged, node_kind::STRUCT);
    assert_eq!(structs, ["c::point.h::point"], "the declaration's `struct point` mints nothing");
    // A header misread by the C grammar turns `class Widget {..}` into a
    // FUNCTION named Widget.
    assert!(!names_of(&merged, node_kind::FUNCTION).contains(&"Widget".to_string()));

    // (4) `class Gadget;` is a forward declaration: no node at all.
    assert!(
        nav.values().all(|(_, name, _)| name != "Gadget"),
        "a node named Gadget: {:?}",
        nav.values().filter(|(_, n, _)| n == "Gadget").collect::<Vec<_>>()
    );

    // (5) Symbols follow their file's MODULE.
    let functions = qnames_of(&merged, node_kind::FUNCTION);
    for want in ["c::point.c::point_new", "src::main.cpp::main", "src::Widget.cpp::Widget::run"] {
        assert!(functions.contains(&want.to_string()), "{want} missing: {functions:?}");
    }

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
    repo_graph_store::write_merged_sharded(merged, dir).expect("write_merged_sharded");
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
    let cold = repo_graph_engine::generate_one_incremental(repo).expect("cold").merged;
    let warm = repo_graph_engine::generate_one_incremental(repo).expect("warm").merged;
    let clean_bytes = store_bytes(&clean, &tmp.path().join("clean"));
    assert_eq!(store_bytes(&cold, &tmp.path().join("cold")), clean_bytes, "cold vs clean");
    assert_eq!(store_bytes(&warm, &tmp.path().join("warm")), clean_bytes, "warm vs clean");
}
