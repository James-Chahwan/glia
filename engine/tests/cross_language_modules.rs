//! LB.9b acceptance: code files of two build groups that share a directory
//! and a stem (`api/user.py` + `api/user.ts`) name their MODULE by file name
//! (`api::user.py`, `api::user.ts`), every symbol under it follows, and each
//! language's bare-path imports (`from api.user import validate`,
//! `import './user'`) still bind through its own graph's alias.
//!
//! Before LB.9b both files were MODULE `api::user` and both `validate`s were
//! FUNCTION `api::user::validate`: one NodeId per pair, two node records in
//! the merged graph (the Python and TypeScript groups are separate
//! RepoGraphs), the two call graphs merged, and `tests::test_user` paired the
//! shared MODULE once per record. Built on a tempdir copy of the
//! `module-cross-language` fixture.

use std::collections::HashMap;
use std::path::Path;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{EdgeCategoryId, NodeId};
use repo_graph_engine::{generate_one, generate_one_incremental};
use repo_graph_graph::MergedGraph;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/module-cross-language";

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

/// Every MODULE qname with one entry per node record.
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

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_string(), b.to_string())
}

#[test]
fn same_stem_files_of_two_build_groups_keep_separate_identities() {
    let tmp = fixture_copy();
    let merged = generate_one(&repo_of(&tmp)).expect("generate_one").merged;

    // (1) The two MODULEs are named by file name; no `api::user` remains.
    let modules = module_qnames(&merged);
    assert!(modules.contains(&"api::user.py".to_string()), "{modules:?}");
    assert!(modules.contains(&"api::user.ts".to_string()), "{modules:?}");
    assert!(!modules.contains(&"api::user".to_string()), "{modules:?}");

    // (2) Each language's caller reaches exactly its own file's validate.
    assert_eq!(
        targets_from(&merged, edge_category::CALLS, "api::main::run"),
        ["api::user.py::validate"]
    );
    assert_eq!(
        targets_from(&merged, edge_category::CALLS, "api::index::run"),
        ["api::user.ts::validate"]
    );

    // (3) The bare-path imports bind through each graph's alias.
    let imports = edges(&merged, edge_category::IMPORTS);
    assert!(imports.contains(&pair("api::main", "api::user.py")), "{imports:?}");
    assert!(imports.contains(&pair("api::index", "api::user.ts")), "{imports:?}");

    // (4) The Python test pairs the Python module once, never the TS one.
    let tests: Vec<(String, String)> = edges(&merged, edge_category::TESTS)
        .into_iter()
        .filter(|(f, _)| f == "tests::test_user")
        .collect();
    assert_eq!(tests, [pair("tests::test_user", "api::user.py")]);

    // (5) No NodeId is a node record in two graphs (HEAD: the MODULE,
    // validate and helper were).
    let mut records: HashMap<NodeId, usize> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            *records.entry(n.id).or_default() += 1;
        }
    }
    let q = qnames(&merged);
    let mut shared: Vec<&str> = records
        .iter()
        .filter(|(_, n)| **n > 1)
        .filter_map(|(id, _)| q.get(id).map(String::as_str))
        .collect();
    shared.sort_unstable();
    assert!(shared.is_empty(), "ids with two node records: {shared:?}");

    // The stem stays each MODULE's display name.
    for g in &merged.graphs {
        for (id, qname) in &g.nav.qname_by_id {
            if qname == "api::user.py" || qname == "api::user.ts" {
                assert_eq!(g.nav.name_by_id.get(id).map(String::as_str), Some("user"));
            }
        }
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

#[test]
fn a_sibling_arriving_requalifies_cached_parses() {
    // (6) incremental == clean, cold and warm; then a third group's same-stem
    // file arrives and the warm build still equals a clean one.
    let tmp = fixture_copy();
    let repo = repo_of(&tmp);
    let clean = generate_one(&repo).expect("generate_one").merged;
    let cold = generate_one_incremental(&repo).expect("cold incremental").merged;
    let warm = generate_one_incremental(&repo).expect("warm incremental").merged;
    let clean_bytes = store_bytes(&clean, &tmp.path().join("clean"));
    assert_eq!(store_bytes(&cold, &tmp.path().join("cold")), clean_bytes, "cold vs clean");
    assert_eq!(store_bytes(&warm, &tmp.path().join("warm")), clean_bytes, "warm vs clean");

    // `api/user.rb`: the key already had two groups, so .py and .ts keep
    // their names; a Ruby file joins them as `api::user.rb`.
    std::fs::write(
        Path::new(&repo).join("api/user.rb"),
        "def validate(x)\n  x\nend\n",
    )
    .expect("write user.rb");
    let warm2 = generate_one_incremental(&repo).expect("warm incremental + rb").merged;
    let clean2 = generate_one(&repo).expect("clean + rb").merged;
    assert!(module_qnames(&clean2).contains(&"api::user.rb".to_string()));
    assert_eq!(
        store_bytes(&warm2, &tmp.path().join("warm2")),
        store_bytes(&clean2, &tmp.path().join("clean2")),
        "incremental vs clean after api/user.rb arrived"
    );

    // Dropping the TypeScript file leaves .py and .rb qualified; dropping the
    // Ruby one too makes api/user.py plain `api::user` again, and its cached
    // parse (still `api::user.py`) must be rejected, not replayed.
    std::fs::remove_file(Path::new(&repo).join("api/user.ts")).expect("rm user.ts");
    std::fs::remove_file(Path::new(&repo).join("api/index.ts")).expect("rm index.ts");
    std::fs::remove_file(Path::new(&repo).join("api/user.rb")).expect("rm user.rb");
    let warm3 = generate_one_incremental(&repo).expect("warm incremental, py alone").merged;
    let clean3 = generate_one(&repo).expect("clean, py alone").merged;
    let modules = module_qnames(&clean3);
    assert!(modules.contains(&"api::user".to_string()), "{modules:?}");
    assert!(!modules.contains(&"api::user.py".to_string()), "{modules:?}");
    assert_eq!(
        store_bytes(&warm3, &tmp.path().join("warm3")),
        store_bytes(&clean3, &tmp.path().join("clean3")),
        "incremental vs clean after the TypeScript and Ruby siblings left"
    );

    // And back: the TypeScript files return, and api/user.py's cached parse
    // (now plain `api::user`) must be rejected for `api::user.py`.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    for f in ["api/user.ts", "api/index.ts"] {
        std::fs::copy(src.join(f), Path::new(&repo).join(f)).expect("restore ts file");
    }
    let warm4 = generate_one_incremental(&repo).expect("warm incremental, ts back").merged;
    let clean4 = generate_one(&repo).expect("clean, ts back").merged;
    assert!(module_qnames(&clean4).contains(&"api::user.py".to_string()));
    assert_eq!(
        store_bytes(&warm4, &tmp.path().join("warm4")),
        store_bytes(&clean4, &tmp.path().join("clean4")),
        "incremental vs clean after the TypeScript siblings returned"
    );
    assert_eq!(store_bytes(&clean4, &tmp.path().join("clean4b")), clean_bytes);
}

#[test]
fn the_requalified_file_counts_as_reparsed() {
    // The cache diff classifies by content hash; a parse rejected for its
    // MODULE qname is reported reparsed, the file that really was.
    let tmp = fixture_copy();
    let repo = repo_of(&tmp);
    let mut cache = repo_graph_engine::ParseCache::new();
    repo_graph_engine::generate_one_with_cache(&repo, &mut cache).expect("cold");
    std::fs::remove_file(Path::new(&repo).join("api/user.ts")).expect("rm user.ts");
    repo_graph_engine::generate_one_with_cache(&repo, &mut cache).expect("warm");
    let diff = cache.last_diff().expect("a recorded diff");
    assert!(diff.reparsed.contains(&"api/user.py".to_string()), "{diff:?}");
    assert!(!diff.reused.contains(&"api/user.py".to_string()), "{diff:?}");
    assert!(diff.evicted.contains(&"api/user.ts".to_string()), "{diff:?}");
}

/// LB.9b (the LC.3a / LC.3b handoff): a non-code MODULE carries no POSITION,
/// so an edge between it and an unlocated node (a manifest DEPENDS_ON a
/// package) ended at basis `none`. The fill pass now places it at the
/// module's file, basis `file`; edges an endpoint locates keep their basis.
#[test]
fn synthetic_module_edges_are_placed_at_their_file() {
    use repo_graph_code_domain::evidence::{Basis, Evidence};

    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("web")).expect("mkdir web");
    std::fs::write(
        repo.join("web/package.json"),
        r#"{"name": "web", "dependencies": {"react": "18.0.0", "lodash": "4.17.21"}}"#,
    )
    .expect("write package.json");
    std::fs::write(repo.join("web/app.ts"), "export function app() { return 1; }\n")
        .expect("write app.ts");
    let merged = generate_one(repo.to_str().expect("utf-8 tempdir"))
        .expect("generate_one")
        .merged;
    let q = qnames(&merged);
    let depends: Vec<Evidence> = merged
        .all_edges()
        .filter(|e| e.category == edge_category::DEPENDS_ON)
        .filter(|e| q.get(&e.from).map(String::as_str) == Some("web::package.json"))
        .map(|e| Evidence::of(e).expect("evidence"))
        .collect();
    assert_eq!(depends.len(), 2, "{depends:?}");
    for ev in &depends {
        assert_eq!(
            (ev.file.as_deref(), ev.line, ev.basis),
            (Some("web/package.json"), None, Basis::File),
            "{ev:?}"
        );
    }
    // A located code edge is untouched: app.ts's DEFINES keeps its node basis.
    let defines: Vec<Evidence> = merged
        .all_edges()
        .filter(|e| e.category == edge_category::DEFINES)
        .filter(|e| q.get(&e.from).map(String::as_str) == Some("web::app"))
        .map(|e| Evidence::of(e).expect("evidence"))
        .collect();
    assert!(!defines.is_empty());
    assert!(defines.iter().all(|ev| ev.basis == Basis::ToNode), "{defines:?}");
}
