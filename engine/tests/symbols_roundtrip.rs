//! LC.6: a graph loaded from a `.gmap` layout carries the same
//! `SymbolTable.interface_methods` as the fresh build that wrote it.
//!
//! A6.6 split interface methods out of `class_methods` into their own table
//! (so the HANDLED_BY global fallback cannot see them). Before LC.6 the code
//! section's `SymbolTableStore` had no slot for it and `to_owned_table` filled
//! it from `Default`, so every graph read back from a `.gmap` had an empty
//! interface table while a fresh build had it filled. The fixture is the
//! substrate-gap `csharp-iface-methods` probe: `IUserService.GetById` and its
//! implementation `UserService.GetById`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use repo_graph_code_domain::node_kind;
use repo_graph_core::NodeId;
use repo_graph_engine::generate_one;
use repo_graph_graph::{MergedGraph, RepoGraph};
use repo_graph_store::{read_merged_sharded, write_merged_sharded};

type Table = BTreeMap<u64, BTreeMap<String, u64>>;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures/csharp-iface-methods")
}

/// The fixture's sources copied into `tmp/repo`: the build runs on a scratch
/// tree so nothing is written beside the committed fixture.
fn fixture_copy(tmp: &Path) -> PathBuf {
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    for entry in std::fs::read_dir(fixture_dir()).unwrap().flatten() {
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), repo.join(entry.file_name())).unwrap();
        }
    }
    repo
}

/// A method table in a stable order: `HashMap` iterates randomly per process.
fn sorted(m: &std::collections::HashMap<NodeId, std::collections::HashMap<String, NodeId>>) -> Table {
    m.iter()
        .map(|(k, inner)| (k.0, inner.iter().map(|(s, n)| (s.clone(), n.0)).collect()))
        .collect()
}

/// Build the fixture, write it as a sharded layout and read it back.
fn fresh_and_loaded() -> (MergedGraph, MergedGraph) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = fixture_copy(tmp.path());
    let fresh = generate_one(&repo.to_string_lossy()).unwrap().merged;
    let dir = tmp.path().join("layout");
    write_merged_sharded(&fresh, &dir).unwrap();
    let loaded = read_merged_sharded(&dir).unwrap();
    assert_eq!(loaded.graphs.len(), fresh.graphs.len(), "one loaded graph per shard");
    (fresh, loaded)
}

fn interface_ids(g: &RepoGraph) -> Vec<NodeId> {
    let mut ids: Vec<NodeId> = g
        .nav
        .kind_by_id
        .iter()
        .filter(|(_, k)| **k == node_kind::INTERFACE)
        .map(|(id, _)| *id)
        .collect();
    ids.sort_by_key(|id| id.0);
    ids
}

#[test]
fn interface_methods_survive_the_gmap() {
    let (fresh, loaded) = fresh_and_loaded();
    let names_get_by_id = fresh.graphs.iter().any(|g| {
        g.symbols.interface_methods.values().any(|m| m.contains_key("GetById"))
    });
    assert!(names_get_by_id, "fresh build must index IUserService.GetById in interface_methods");
    for (i, (f, l)) in fresh.graphs.iter().zip(&loaded.graphs).enumerate() {
        assert_eq!(
            sorted(&l.symbols.interface_methods),
            sorted(&f.symbols.interface_methods),
            "graph {i}: loaded interface_methods must equal the fresh build's",
        );
    }
}

/// Sorted at both levels, the interface table cannot make shard bytes flap:
/// two clean builds in one process (each `HashMap` with its own `RandomState`)
/// write identical files. The extra source gives the table several owners
/// with several methods each, so an unsorted level would show.
#[test]
fn interface_table_bytes_are_reproducible() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = fixture_copy(tmp.path());
    std::fs::write(
        repo.join("Repos.cs"),
        "namespace Shop.Repos\n{\n    public interface IOrderRepo\n    {\n        void Save(int id);\n        \
         void Delete(int id);\n        int Count();\n        bool Exists(int id);\n    }\n\n    \
         public interface IAuditLog\n    {\n        void Write(string line);\n        void Flush();\n        \
         string Tail();\n    }\n}\n",
    )
    .unwrap();
    let repo_s = repo.to_string_lossy().into_owned();
    let (out1, out2) = (tmp.path().join("out1"), tmp.path().join("out2"));
    let first = generate_one(&repo_s).unwrap().merged;
    let owners_with_several: usize = first
        .graphs
        .iter()
        .map(|g| g.symbols.interface_methods.values().filter(|m| m.len() > 1).count())
        .sum();
    assert!(owners_with_several >= 2, "the fixture must give several interfaces several methods");
    write_merged_sharded(&first, &out1).unwrap();
    write_merged_sharded(&generate_one(&repo_s).unwrap().merged, &out2).unwrap();
    let files = |d: &Path| -> BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(d)
            .unwrap()
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
            .collect()
    };
    assert_eq!(files(&out1), files(&out2), "two clean builds must write identical layouts");
}

#[test]
fn class_methods_still_exclude_interfaces() {
    let (fresh, loaded) = fresh_and_loaded();
    let mut interfaces_seen = 0usize;
    for (i, (f, l)) in fresh.graphs.iter().zip(&loaded.graphs).enumerate() {
        assert_eq!(
            sorted(&l.symbols.class_methods),
            sorted(&f.symbols.class_methods),
            "graph {i}: class_methods round-trips unchanged",
        );
        for iface in interface_ids(l) {
            interfaces_seen += 1;
            assert!(
                !l.symbols.class_methods.contains_key(&iface),
                "graph {i}: interface {} is keyed in the loaded class_methods",
                iface.0,
            );
            assert!(
                l.symbols.interface_methods.contains_key(&iface),
                "graph {i}: interface {} is missing from the loaded interface_methods",
                iface.0,
            );
        }
    }
    assert!(interfaces_seen > 0, "the fixture declares IUserService: an INTERFACE node must load");
}
