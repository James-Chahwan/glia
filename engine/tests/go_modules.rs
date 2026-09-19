//! LA.13: every go.mod of a repo maps Go imports, not only `<root>/go.mod`.
//!
//! The fixture has no go.mod at the walked root and two nested modules whose
//! paths share the prefix `example.com/svc` (the `/`-boundary case):
//! `svc/` (`example.com/svc`) and `svc-b/` (`example.com/svc-b`), both
//! defining `internal/store.Save`. Before LA.13 only the root go.mod was read,
//! so both intra-repo imports of `svc/cmd/main.go` stayed raw library paths:
//! the own-module `store` import could not bind (its tail is ambiguous
//! between the two modules) and both leaked into the IMPORTS cell.

use std::path::Path;

use glia_code_domain::{cell_type, edge_category};
use glia_core::{CellPayload, EdgeCategoryId, NodeId};
use glia_engine::{GenerateResult, ParseCache, generate_one, generate_one_with_cache};

fn write(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

const MAIN_GO: &str = "package main\n\nimport (\n\t\"github.com/google/uuid\"\n\t\"example.com/svc/internal/store\"\n\t\"example.com/svc-b/client\"\n)\n\nfunc main() {\n\t_ = uuid.New()\n\tstore.Save()\n\tclient.Get()\n}\n";

/// The `bench/substrate-gap/fixtures/go-nested-modules` shape.
fn write_fixture(dir: &Path) {
    write(dir, "svc/go.mod", "module example.com/svc\n\ngo 1.22\n");
    write(dir, "svc/cmd/main.go", MAIN_GO);
    write(dir, "svc/internal/store/store.go", "package store\n\nfunc Save() string { return \"svc\" }\n");
    write(dir, "svc-b/go.mod", "module example.com/svc-b\n\ngo 1.22\n");
    write(dir, "svc-b/client/client.go", "package client\n\nfunc Get() string { return \"b\" }\n");
    write(dir, "svc-b/internal/store/store.go", "package store\n\nfunc Save() string { return \"svc-b\" }\n");
}

fn id_of(r: &GenerateResult, qname: &str) -> NodeId {
    let hits: Vec<NodeId> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.iter())
        .filter(|(_, q)| q.as_str() == qname)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(hits.len(), 1, "expected exactly one node {qname}, got {hits:?}");
    hits[0]
}

fn has_edge(r: &GenerateResult, from: &str, to: &str, cat: EdgeCategoryId) -> bool {
    let (f, t) = (id_of(r, from), id_of(r, to));
    r.merged.all_edges().any(|e| e.from == f && e.to == t && e.category == cat)
}

/// The IMPORTS payload of the node `qname` (exactly one cell).
fn imports_cell(r: &GenerateResult, qname: &str) -> String {
    let id = id_of(r, qname);
    let cells: Vec<String> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == cell_type::IMPORTS)
        .map(|c| match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) => s.clone(),
            CellPayload::Bytes(_) => String::from("<bytes>"),
        })
        .collect();
    assert_eq!(cells.len(), 1, "{qname} must carry exactly one IMPORTS cell, got {cells:?}");
    cells.into_iter().next().unwrap()
}

#[test]
fn nested_go_mods_map_internal_imports() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let r = generate_one(tmp.path().to_str().unwrap()).unwrap();

    assert!(
        has_edge(&r, "svc::cmd::main", "svc::internal::store::store", edge_category::IMPORTS),
        "own-module import must bind svc's store package"
    );
    assert!(
        has_edge(&r, "svc::cmd::main", "svc-b::client::client", edge_category::IMPORTS),
        "sibling-module import must bind svc-b's client package"
    );
    assert!(
        has_edge(&r, "svc::cmd::main::main", "svc::internal::store::store::Save", edge_category::CALLS),
        "store.Save() must call svc's Save"
    );
    assert!(
        has_edge(&r, "svc::cmd::main::main", "svc-b::client::client::Get", edge_category::CALLS),
        "client.Get() must call svc-b's Get"
    );
    // Precision: example.com/svc/internal/store is never svc-b's same-named package.
    assert!(!has_edge(
        &r,
        "svc::cmd::main",
        "svc-b::internal::store::store",
        edge_category::IMPORTS
    ));
    assert!(!has_edge(
        &r,
        "svc::cmd::main::main",
        "svc-b::internal::store::store::Save",
        edge_category::CALLS
    ));
}

/// The exact-equality gate for the leak: the fixture key's `contains` row
/// cannot say "nothing else".
#[test]
fn imports_cell_lists_only_third_party() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let r = generate_one(tmp.path().to_str().unwrap()).unwrap();
    assert_eq!(imports_cell(&r, "svc::cmd::main"), r#"["github.com/google/uuid"]"#);
    assert_eq!(imports_cell(&r, "svc::cmd::main::main"), r#"["github.com/google/uuid"]"#);
}

/// Module `example.com/svc` holds `example.com/svc/...` only at a `/`
/// boundary: `example.com/svc-b/client` with no svc-b module in the repo is a
/// library, under a root go.mod as under a nested one. (Before LA.13 the root
/// go.mod's unbounded prefix test mapped it to the local path `-b::client`,
/// dropping it from the cell.)
#[test]
fn prefix_boundary() {
    for root in ["", "svc/"] {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, &format!("{root}go.mod"), "module example.com/svc\n\ngo 1.22\n");
        write(
            d,
            &format!("{root}cmd/main.go"),
            "package main\n\nimport (\n\t\"example.com/svc/util\"\n\t\"example.com/svc-b/client\"\n)\n\nfunc main() {\n\tutil.Do()\n\tclient.Get()\n}\n",
        );
        write(d, &format!("{root}util/util.go"), "package util\n\nfunc Do() {}\n");
        let r = generate_one(d.to_str().unwrap()).unwrap();
        let main = format!("{}cmd::main", root.replace('/', "::"));
        let util = format!("{}util::util", root.replace('/', "::"));
        assert_eq!(
            imports_cell(&r, &main),
            r#"["example.com/svc-b/client"]"#,
            "root={root:?}: the other module path is a library"
        );
        assert!(
            has_edge(&r, &main, &util, edge_category::IMPORTS),
            "root={root:?}: the own-module import binds"
        );
    }
}

/// A go.mod is invisible to every `.go` content hash, so editing one must
/// discard the parse cache: `extra.go` never changes, but once svc-b's module
/// is renamed to the path it imports, that import is intra-repo.
#[test]
fn editing_a_nested_go_mod_discards_the_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write_fixture(d);
    write(
        d,
        "svc/cmd/extra.go",
        "package main\n\nimport \"example.com/bee/client\"\n\nfunc extra() {\n\tclient.Get()\n}\n",
    );
    let path = d.to_str().unwrap();
    let mut cache = ParseCache::new();

    let first = generate_one_with_cache(path, &mut cache).unwrap();
    assert_eq!(
        imports_cell(&first, "svc::cmd::extra"),
        r#"["example.com/bee/client"]"#,
        "no module is example.com/bee yet: a library"
    );
    // Control: an unchanged rebuild reuses every parse.
    generate_one_with_cache(path, &mut cache).unwrap();
    let warm = cache.last_diff().expect("diff").clone();
    assert!(warm.reparsed.is_empty(), "unchanged rebuild reparsed {:?}", warm.reparsed);
    assert!(warm.reused.iter().any(|p| p == "svc/cmd/extra.go"));

    write(d, "svc-b/go.mod", "module example.com/bee\n\ngo 1.22\n");
    let second = generate_one_with_cache(path, &mut cache).unwrap();
    assert_eq!(
        imports_cell(&second, "svc::cmd::extra"),
        "[]",
        "example.com/bee is svc-b's module now: the stale raw import must not survive"
    );
    assert!(has_edge(&second, "svc::cmd::extra", "svc-b::client::client", edge_category::IMPORTS));
    let diff = cache.last_diff().expect("diff");
    assert!(
        diff.reused.is_empty(),
        "a go.mod edit must discard every cached parse, reused {:?}",
        diff.reused
    );
    assert!(diff.reparsed.iter().any(|p| p == "svc/cmd/extra.go"));
    // And the rebuild equals a clean build.
    let clean = generate_one(path).unwrap();
    assert_eq!(imports_cell(&clean, "svc::cmd::extra"), "[]");
    assert_eq!(
        imports_cell(&clean, "svc::cmd::main"),
        imports_cell(&second, "svc::cmd::main")
    );
}
