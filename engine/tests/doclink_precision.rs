//! FD.79 (edge audit FD.3c, DOCUMENTS|derived clusters D1 + D2): the doc
//! linker binds a bare mention only when it is identifier-shaped (a call
//! `x()`, an inner `_`, mixed case), never a lone word, a registry name or a
//! placeholder; a test / fixture / bench candidate binds only from a doc under
//! its tree; and an ambiguous tie goes to the copy under the doc's own project
//! root before the least qname.
//!
//! The fixture tree is `bench/substrate-gap/fixtures/doclink-identifier-mentions`,
//! copied into a tempdir and built there; every expect / forbid row of its
//! key.json is checked here, so the engine test and the substrate grade read
//! one source of truth.

use std::path::Path;

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/doclink-identifier-mentions";

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create copy dir");
    let mut entries: Vec<_> = std::fs::read_dir(src)
        .expect("read fixture dir")
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let (from, to) = (e.path(), dst.join(e.file_name()));
        let ty = e.file_type().expect("file type");
        if ty.is_dir() {
            if e.file_name() == ".ai" || e.file_name() == ".glia" {
                continue;
            }
            copy_tree(&from, &to);
        } else if ty.is_file() {
            std::fs::copy(&from, &to).expect("copy file");
        }
    }
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).expect("create dir");
    }
    std::fs::write(p, text).expect("write file");
}

fn build(root: &Path) -> MergedGraph {
    generate_one(&root.to_string_lossy())
        .unwrap_or_else(|e| panic!("build {}: {e}", root.display()))
        .merged
}

/// Every DOCUMENTS edge as (from qname, to qname, confidence, EVIDENCE rule),
/// sorted.
fn documents(m: &MergedGraph) -> Vec<(String, String, String, String)> {
    let mut qname = std::collections::HashMap::new();
    for g in &m.graphs {
        for n in &g.nodes {
            if let Some(q) = g.nav.qname_by_id.get(&n.id) {
                qname.insert(n.id, q.clone());
            }
        }
    }
    let q = |id| qname.get(&id).cloned().unwrap_or_default();
    let mut out: Vec<_> = m
        .all_edges()
        .filter(|e| e.category == edge_category::DOCUMENTS)
        .map(|e| {
            (
                q(e.from),
                q(e.to),
                format!("{:?}", e.confidence),
                Evidence::of(e).and_then(|ev| ev.rule).unwrap_or_default(),
            )
        })
        .collect();
    out.sort();
    out
}

fn has(docs: &[(String, String, String, String)], from: &str, to: &str) -> bool {
    docs.iter().any(|(f, t, _, _)| f == from && t == to)
}

/// Every expect_edges row of the fixture binds; every forbid row does not.
#[test]
fn fixture_rows_hold() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let key: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(src.join("key.json")).expect("read key.json"),
    )
    .expect("parse key.json");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("doclink");
    copy_tree(&src, &root);
    let docs = documents(&build(&root));

    let rows = |k: &str| -> Vec<(String, String)> {
        key[k]
            .as_array()
            .unwrap_or_else(|| panic!("key.json {k}"))
            .iter()
            .map(|r| {
                assert_eq!(r["category"], "DOCUMENTS");
                (
                    r["from"].as_str().unwrap_or_default().to_string(),
                    r["to"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    };
    let expect = rows("expect_edges");
    let forbid = rows("forbid");
    assert_eq!((expect.len(), forbid.len()), (5, 8), "the fixture's rows");
    let missing: Vec<_> = expect.iter().filter(|(f, t)| !has(&docs, f, t)).collect();
    let fired: Vec<_> = forbid.iter().filter(|(f, t)| has(&docs, f, t)).collect();
    assert!(
        missing.is_empty() && fired.is_empty(),
        "missing {missing:?}, forbidden {fired:?}; every edge: {docs:#?}"
    );

    // The five expected rows are the whole answer, each graded as FD.79 says.
    let got: Vec<_> = docs
        .iter()
        .map(|(f, t, c, r)| (f.as_str(), t.as_str(), c.as_str(), r.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "docs::README::lookup",
                "app::registry::ServiceRegistry::lookup",
                "Strong",
                "qualified"
            ),
            (
                "docs::README::lookup",
                "app::registry::lookup_service",
                "Medium",
                "unique"
            ),
            (
                "docs::README::usage",
                "app::config::MAX_RETRIES",
                "Medium",
                "unique"
            ),
            (
                "docs::README::usage",
                "app::registry::ServiceRegistry",
                "Medium",
                "unique"
            ),
            (
                "docs::samples::maps::README::fields",
                "samples::maps::store::MapMarker",
                "Weak",
                "ambiguous"
            ),
        ]
    );
}

/// A test-tree candidate binds from a doc under its tree (tests/ is a project
/// root, so its README is ingested); `meta.json` binds no `json` symbol; a
/// qualified mention with a lowercase tail still binds Strong.
#[test]
fn test_tree_doc_binds_and_lone_words_do_not() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("tree");
    write(&root, "pyproject.toml", "[project]\nname = \"app\"\n");
    write(
        &root,
        "README.md",
        "# App\n\n## Notes\n\nThe config lives in `meta.json`; `Feed.refresh` reloads the feed.\n",
    );
    write(
        &root,
        "app/feed.py",
        "class Feed:\n    def refresh(self):\n        return 1\n\n\ndef json():\n    return {}\n",
    );
    write(
        &root,
        "tests/pyproject.toml",
        "[project]\nname = \"app-tests\"\n",
    );
    write(
        &root,
        "tests/README.md",
        "# Tests\n\n## Helpers\n\nBuild one with `make_registry`.\n",
    );
    write(
        &root,
        "tests/helpers.py",
        "def make_registry():\n    return {}\n",
    );
    let docs = documents(&build(&root));

    let notes: Vec<_> = docs
        .iter()
        .filter(|(f, ..)| f == "docs::README::notes")
        .map(|(_, t, c, r)| (t.as_str(), c.as_str(), r.as_str()))
        .collect();
    assert_eq!(
        notes,
        vec![("app::feed::Feed::refresh", "Strong", "qualified")],
        "{docs:#?}"
    );
    let helpers: Vec<_> = docs
        .iter()
        .filter(|(f, ..)| f.contains("tests") && f.ends_with("::helpers"))
        .map(|(_, t, c, r)| (t.as_str(), c.as_str(), r.as_str()))
        .collect();
    assert_eq!(helpers.len(), 1, "{docs:#?}");
    assert!(helpers[0].0.ends_with("::make_registry"), "{helpers:?}");
    assert_eq!((helpers[0].1, helpers[0].2), ("Medium", "unique"));
}
