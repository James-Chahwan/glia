//! A16.4 (audit 2026-06-10 #12): a node's IMPORTS cell lists the external
//! libraries its file imports. An import that resolves inside the repo — a
//! sibling Rust module, the repo's own Python package — is not a dependency
//! and must not be listed. An external Go module path must still be.

use std::path::Path;

use glia_code_domain::cell_type;
use glia_core::CellPayload;
use glia_engine::{GenerateResult, ParseCache, generate_one, generate_one_with_cache};

fn write(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// Rust crate with a sibling module, a Python package importing itself, and a
/// Go file with one third-party import.
fn write_fixture(dir: &Path) {
    write(dir, "src/lib.rs", "mod util;\n\nuse crate::util::helper;\n\npub fn run() -> u32 {\n    helper()\n}\n");
    write(dir, "src/util.rs", "pub fn helper() -> u32 {\n    1\n}\n");
    write(dir, "myapp/__init__.py", "");
    write(
        dir,
        "myapp/auth.py",
        "from myapp.users import User\n\n\ndef login(name):\n    return User(name)\n",
    );
    write(dir, "myapp/users.py", "class User:\n    def __init__(self, name):\n        self.name = name\n");
    write(dir, "svc/server.go", "package svc\n\nimport \"github.com/x/y\"\n\nfunc Serve() {\n\ty.Run()\n}\n");
}

/// Every IMPORTS payload of every node, keyed by qname. A node with several
/// entries (a file split across graphs) keeps them all, so a duplicated or
/// missing cell shows up as a wrong count rather than being hidden.
fn imports_by_qname(r: &GenerateResult) -> Vec<(String, Vec<String>)> {
    let mut rows = Vec::new();
    for g in &r.merged.graphs {
        for n in &g.nodes {
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            let cells: Vec<String> = n
                .cells
                .iter()
                .filter(|c| c.kind == cell_type::IMPORTS)
                .map(|c| match &c.payload {
                    CellPayload::Json(s) | CellPayload::Text(s) => s.clone(),
                    CellPayload::Bytes(_) => String::from("<bytes>"),
                })
                .collect();
            rows.push((qname, cells));
        }
    }
    rows.sort();
    rows
}

/// The one IMPORTS payload every node under `prefix` carries. Fails if a node
/// has zero or two cells, or if the file's nodes disagree.
fn file_imports(rows: &[(String, Vec<String>)], prefix: &str) -> String {
    let under: Vec<_> = rows
        .iter()
        .filter(|(q, _)| q == prefix || q.starts_with(&format!("{prefix}::")))
        .collect();
    assert!(!under.is_empty(), "no nodes under {prefix}: {rows:#?}");
    let mut payloads = std::collections::BTreeSet::new();
    for (q, cells) in &under {
        assert_eq!(cells.len(), 1, "{q} must carry exactly one IMPORTS cell, got {cells:?}");
        payloads.insert(cells[0].clone());
    }
    assert_eq!(payloads.len(), 1, "nodes under {prefix} disagree: {payloads:?}");
    payloads.into_iter().next().unwrap()
}

fn assert_filtered(rows: &[(String, Vec<String>)], context: &str) {
    // Rust: `use crate::util::helper` names the sibling module src::util.
    assert_eq!(file_imports(rows, "src::lib"), "[]", "{context}: rust sibling module leaked");
    // Python: `from myapp.users import User` names the repo's own package.
    assert_eq!(file_imports(rows, "myapp::auth"), "[]", "{context}: python own package leaked");
    // Go: a third-party module path is a real dependency.
    assert_eq!(
        file_imports(rows, "svc::server"),
        r#"["github.com/x/y"]"#,
        "{context}: go external import lost"
    );
}

#[test]
fn intra_repo_imports_are_not_listed_as_libraries() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let r = generate_one(tmp.path().to_str().unwrap()).unwrap();
    assert!(r.parse_errors.is_empty(), "{:?}", r.parse_errors);
    assert_filtered(&imports_by_qname(&r), "clean build");
}

/// The filter depends on the whole repo, so it runs after the parse cache: a
/// warm build replays cached parses and must still filter them — to exactly
/// one IMPORTS cell per node, identical to the clean build.
#[test]
fn warm_cache_build_filters_identically() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let repo = tmp.path().to_str().unwrap();
    let clean = imports_by_qname(&generate_one(repo).unwrap());

    let mut cache = ParseCache::new();
    generate_one_with_cache(repo, &mut cache).unwrap();
    let warm = imports_by_qname(&generate_one_with_cache(repo, &mut cache).unwrap());
    assert_filtered(&warm, "warm build");
    assert_eq!(warm, clean, "warm-cache IMPORTS cells differ from the clean build");
}
