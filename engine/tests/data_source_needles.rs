//! FD.73 acceptance: a data-source name inside a JNI type descriptor, or a
//! capitalised needle inside a longer word, is no data access, on a REAL
//! build.
//!
//! The sources are the committed fixture
//! `bench/substrate-gap/fixtures/data-source-jni-descriptor-words`, written to
//! a tempdir so the build owns its directory. The forbid rows are the edge
//! audit's cluster A1 (FD.3c): a jnigen Dart binding's raw-string descriptor
//! and its C twin split across adjacent literals (data_source:sqlite and,
//! through the `database/sql` prefix of `android/database/sqlite`,
//! data_source:database_sql), `Redistribution` in a licence string
//! (data_source:redis) and `Prismatic` in a UI string (data_source:prisma).
//! The controls keep their edges. Run with `-- --nocapture` to see the
//! `[data-sources] refused ..` marker.

use glia_code_domain::edge_category;
use glia_engine::generate_one;
use glia_graph::MergedGraph;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

const FIXTURE: &str = "data-source-jni-descriptor-words";

macro_rules! fixture_file {
    ($rel:literal) => {
        (
            $rel,
            include_str!(concat!(
                "../../bench/substrate-gap/fixtures/data-source-jni-descriptor-words/",
                $rel
            )),
        )
    };
}

const FILES: &[(&str, &str)] = &[
    fixture_file!("lib/health_connect.dart"),
    fixture_file!("src/health_connect/health_connect.c"),
    fixture_file!("lib/licence_page.dart"),
    fixture_file!("lib/banner.dart"),
    fixture_file!("lib/store.dart"),
    fixture_file!("lib/cache.dart"),
    fixture_file!("go/repo.go"),
    fixture_file!("jvm/Cache.java"),
    fixture_file!("pubspec.yaml"),
];

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn build(root: &Path) -> MergedGraph {
    generate_one(root.to_str().unwrap())
        .expect("repo builds")
        .merged
}

/// Every ACCESSES_DATA edge of the build as a sorted (from qname, to qname)
/// set, read through every graph's nav.
fn accesses(m: &MergedGraph) -> BTreeSet<(String, String)> {
    let qname: HashMap<_, _> = m
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.iter())
        .map(|(id, q)| (*id, q.clone()))
        .collect();
    m.graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(m.cross_edges.iter())
        .filter(|e| e.category == edge_category::ACCESSES_DATA)
        .filter_map(|e| Some((qname.get(&e.from)?.clone(), qname.get(&e.to)?.clone())))
        .collect()
}

#[test]
fn descriptor_and_word_needles_are_no_access() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(FIXTURE);
    for (rel, body) in FILES {
        write(&root, rel, body);
    }
    let edges = accesses(&build(&root));

    let forbid = [
        ("lib::health_connect", "data_source:sqlite"),
        ("lib::health_connect", "data_source:database_sql"),
        (
            "src::health_connect::health_connect.c",
            "data_source:sqlite",
        ),
        (
            "src::health_connect::health_connect.c",
            "data_source:database_sql",
        ),
        ("lib::licence_page", "data_source:redis"),
        ("lib::banner", "data_source:prisma"),
    ];
    let hit: Vec<_> = forbid
        .iter()
        .filter(|(f, t)| edges.contains(&(f.to_string(), t.to_string())))
        .collect();
    assert!(hit.is_empty(), "phantom ACCESSES_DATA edges: {hit:?}");

    // No edge at all leaves a descriptor or a prose file.
    let from_phantom: Vec<_> = edges
        .iter()
        .filter(|(f, _)| {
            matches!(
                f.as_str(),
                "lib::health_connect"
                    | "src::health_connect::health_connect.c"
                    | "lib::licence_page"
                    | "lib::banner"
            )
        })
        .collect();
    assert!(from_phantom.is_empty(), "{from_phantom:?}");

    for (f, t) in [
        ("lib::store", "data_source:sqlite"),
        ("lib::cache", "data_source:redis"),
        ("go::repo", "data_source:database_sql"),
        ("jvm::Cache", "data_source:redis"),
    ] {
        assert!(
            edges.contains(&(f.to_string(), t.to_string())),
            "control edge {f} -> {t} missing; edges: {edges:?}"
        );
    }
}
