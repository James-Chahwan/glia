//! LB.13 acceptance: code files of ONE build group that share a directory and
//! a stem (`src/util.ts` + `src/util.js` in the TS-family graph,
//! `clj/app/core.clj` + `core.cljs` in the Clojure graph, `jvm/shop/Foo.java`
//! + `Foo.kt` in the JVM graph) name their MODULE by file name, as LB.9b does
//! across groups, and a bare import of the stem binds the sibling the
//! importer's language loads (a `.ts` importer `util.ts`, a `.js` importer
//! `util.js`, a JVM Clojure require `core.clj`, a ClojureScript one
//! `core.cljs`).
//!
//! Before LB.13 each pair was ONE MODULE (`src::util`, `clj::app::core`,
//! `jvm::shop::Foo`): both `fmt`s were FUNCTION `src::util::fmt` with two
//! DEFINES edges and CALLS to both files' helpers, and every import landed on
//! the merged module. Built on a tempdir copy of the `module-same-group`
//! fixture.

use std::collections::HashMap;
use std::path::Path;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{EdgeCategoryId, NodeId};
use repo_graph_engine::{generate_one, generate_one_incremental};
use repo_graph_graph::MergedGraph;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/module-same-group";

/// Copy the fixture's sources (not its key.json) into `dst`.
fn copy_fixture(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            std::fs::create_dir_all(&to).expect("mkdir");
            copy_fixture(&from, &to);
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

fn repo_of(tmp: &tempfile::TempDir) -> String {
    tmp.path().join("repo").to_str().expect("utf-8 tempdir").to_string()
}

/// id -> qname over every graph of the build.
fn qnames(merged: &MergedGraph) -> HashMap<NodeId, String> {
    let mut out = HashMap::new();
    for g in &merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            out.insert(*id, q.clone());
        }
    }
    out
}

/// Every MODULE qname, one entry per node record.
fn module_qnames(merged: &MergedGraph) -> Vec<String> {
    let mut out: Vec<String> = merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter_map(move |n| {
                (g.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE))
                    .then(|| g.nav.qname_by_id.get(&n.id).cloned())
                    .flatten()
            })
        })
        .collect();
    out.sort();
    out
}

/// `(from qname, to qname)` of every edge of `category`, one entry per edge.
fn edges(merged: &MergedGraph, category: EdgeCategoryId) -> Vec<(String, String)> {
    let q = qnames(merged);
    let name = |id: &NodeId| q.get(id).cloned().unwrap_or_default();
    let mut out: Vec<(String, String)> = merged
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| (name(&e.from), name(&e.to)))
        .collect();
    out.sort();
    out
}

fn targets_from(merged: &MergedGraph, category: EdgeCategoryId, from: &str) -> Vec<String> {
    edges(merged, category)
        .into_iter()
        .filter(|(f, _)| f == from)
        .map(|(_, t)| t)
        .collect()
}

#[test]
fn same_stem_files_of_one_build_group_keep_separate_identities() {
    let tmp = fixture_copy();
    let merged = generate_one(&repo_of(&tmp)).expect("generate_one").merged;

    // (1) One MODULE per file, named by file name; no stem-form MODULE left.
    let modules = module_qnames(&merged);
    for m in [
        "src::util.ts",
        "src::util.js",
        "clj::app::core.clj",
        "clj::app::core.cljs",
        "jvm::shop::Foo.java",
        "jvm::shop::Foo.kt",
    ] {
        assert!(modules.contains(&m.to_string()), "{m} missing: {modules:?}");
    }
    for m in ["src::util", "clj::app::core", "jvm::shop::Foo"] {
        assert!(!modules.contains(&m.to_string()), "{m} still a MODULE: {modules:?}");
    }
    // The stem stays each MODULE's display name.
    for g in &merged.graphs {
        for (id, qname) in &g.nav.qname_by_id {
            let want = match qname.as_str() {
                "src::util.ts" | "src::util.js" => "util",
                "clj::app::core.clj" | "clj::app::core.cljs" => "core",
                "jvm::shop::Foo.java" | "jvm::shop::Foo.kt" => "Foo",
                _ => continue,
            };
            if g.nav.kind_by_id.get(id) == Some(&node_kind::MODULE) {
                assert_eq!(g.nav.name_by_id.get(id).map(String::as_str), Some(want), "{qname}");
            }
        }
    }

    // (2) Each importer binds the sibling its own language loads, only it.
    for (from, to) in [
        ("src::app", "src::util.ts"),
        ("src::old", "src::util.js"),
        ("clj::app::server", "clj::app::core.clj"),
        ("clj::app::main", "clj::app::core.cljs"),
    ] {
        assert_eq!(targets_from(&merged, edge_category::IMPORTS, from), [to], "{from}");
    }

    // (3) Calls follow the import; each file's fmt / f calls its own helper.
    for (from, to) in [
        ("src::app::render", "src::util.ts::fmt"),
        ("src::old::draw", "src::util.js::fmt"),
        ("src::util.ts::fmt", "src::util.ts::pad"),
        ("src::util.js::fmt", "src::util.js::legacy"),
        ("clj::app::core.clj::f", "clj::app::core.clj::g"),
        ("clj::app::core.cljs::f", "clj::app::core.cljs::h"),
    ] {
        assert_eq!(targets_from(&merged, edge_category::CALLS, from), [to], "{from}");
    }
    // Kotlin's top-level function follows its file's MODULE.
    assert_eq!(
        targets_from(&merged, edge_category::DEFINES, "jvm::shop::Foo.kt"),
        ["jvm::shop::Foo.kt::topLevel"]
    );

    // (4) One DEFINES into each fmt (HEAD: src::util -> src::util::fmt x2).
    let defines = edges(&merged, edge_category::DEFINES);
    for f in ["src::util.ts::fmt", "src::util.js::fmt", "clj::app::core.clj::f"] {
        let into: Vec<&(String, String)> = defines.iter().filter(|(_, t)| t == f).collect();
        assert_eq!(into.len(), 1, "{f}: {into:?}");
    }
}

#[test]
fn a_test_module_pairs_its_own_languages_sibling() {
    let tmp = fixture_copy();
    let repo = repo_of(&tmp);
    std::fs::write(
        Path::new(&repo).join("src/util.test.ts"),
        "import { fmt } from './util';\n\nexport function checks() {\n  return fmt(1);\n}\n",
    )
    .expect("write util.test.ts");
    let merged = generate_one(&repo).expect("generate_one").merged;
    assert_eq!(targets_from(&merged, edge_category::TESTS, "src::util.test"), ["src::util.ts"]);
    assert_eq!(targets_from(&merged, edge_category::IMPORTS, "src::util.test"), ["src::util.ts"]);
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

#[test]
fn incremental_builds_agree_as_a_sibling_leaves() {
    // (5) incremental == clean, cold and warm; then util.js leaves the warm
    // tree, util.ts's cached parse (still `src::util.ts`) is rejected, and
    // the warm build still equals a clean one.
    let tmp = fixture_copy();
    let repo = repo_of(&tmp);
    let clean = generate_one(&repo).expect("generate_one").merged;
    let cold = generate_one_incremental(&repo).expect("cold incremental").merged;
    let warm = generate_one_incremental(&repo).expect("warm incremental").merged;
    let clean_bytes = store_bytes(&clean, &tmp.path().join("clean"));
    assert_eq!(store_bytes(&cold, &tmp.path().join("cold")), clean_bytes, "cold vs clean");
    assert_eq!(store_bytes(&warm, &tmp.path().join("warm")), clean_bytes, "warm vs clean");

    std::fs::remove_file(Path::new(&repo).join("src/util.js")).expect("rm util.js");
    let warm2 = generate_one_incremental(&repo).expect("warm incremental, ts alone").merged;
    let clean2 = generate_one(&repo).expect("clean, ts alone").merged;
    let modules = module_qnames(&clean2);
    assert!(modules.contains(&"src::util".to_string()), "{modules:?}");
    assert!(!modules.contains(&"src::util.ts".to_string()), "{modules:?}");
    assert_eq!(targets_from(&clean2, edge_category::IMPORTS, "src::old"), ["src::util"]);
    assert_eq!(
        store_bytes(&warm2, &tmp.path().join("warm2")),
        store_bytes(&clean2, &tmp.path().join("clean2")),
        "incremental vs clean after src/util.js left"
    );

    // And back: util.js returns and util.ts's cached parse (now plain
    // `src::util`) must be rejected for `src::util.ts`.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    std::fs::copy(src.join("src/util.js"), Path::new(&repo).join("src/util.js"))
        .expect("restore util.js");
    let warm3 = generate_one_incremental(&repo).expect("warm incremental, js back").merged;
    let clean3 = generate_one(&repo).expect("clean, js back").merged;
    assert_eq!(
        store_bytes(&warm3, &tmp.path().join("warm3")),
        store_bytes(&clean3, &tmp.path().join("clean3")),
        "incremental vs clean after src/util.js returned"
    );
    assert_eq!(store_bytes(&clean3, &tmp.path().join("clean3b")), clean_bytes);
}
