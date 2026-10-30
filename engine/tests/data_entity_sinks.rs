//! FD.74 acceptance: a SQL / Cypher statement literal is a data access only
//! as the argument of a call that runs it, on a REAL build.
//!
//! The tree is the committed fixture
//! `bench/substrate-gap/fixtures/data-entity-statement-sinks`, written to a
//! tempdir so the build owns its directory. File-write content
//! (`std::fs::write`, a `write(dir, rel, body)` helper, `Path.write_text`,
//! `fs.writeFileSync`), a query to glia's in-process GQL evaluator
//! (`query_gql`) and a statement anywhere in a Cargo `tests/` target mint no
//! DATA_ENTITY; a statement a DB client runs (Rust `client.query`, Python
//! `conn.execute` / `session.run`, a Dart StringBuffer builder, a Python test
//! seeding a table) keeps its ACCESSES_DATA edge. Run with `-- --nocapture`
//! to see the `[data-entity] sink-args refused ..` marker.

use std::path::Path;

use glia_code_domain::{edge_category, node_kind};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

macro_rules! fixture {
    ($rel:literal) => {
        (
            $rel,
            include_str!(concat!(
                "../../bench/substrate-gap/fixtures/data-entity-statement-sinks/",
                $rel
            )),
        )
    };
}

const FILES: [(&str, &str); 10] = [
    fixture!("tests/parallel_build.rs"),
    fixture!("tests/seed_db.rs"),
    fixture!("tests/test_query.py"),
    fixture!("tests/test_seed.py"),
    fixture!("src/scaffold.rs"),
    fixture!("src/repo.rs"),
    fixture!("tools/scaffold.py"),
    fixture!("tools/init.js"),
    fixture!("app/repo.py"),
    fixture!("lib/report.dart"),
];

/// The DATA_ENTITY qnames the fixture's key.json forbids.
const FORBID: [&str; 8] = [
    "data_entity:sql:orders",
    "data_entity:sql:ledgers",
    "data_entity:sql:helper_rows",
    "data_entity:sql:parcels",
    "data_entity:sql:manifests",
    "data_entity:sql:pallets",
    "data_entity:graph:FUNCTION",
    "data_entity:graph:ROUTEE",
];

/// The ACCESSES_DATA edges (from qname, DATA_ENTITY qname) key.json expects.
const EXPECT: [(&str, &str); 5] = [
    ("src::repo::load", "data_entity:sql:accounts"),
    ("app::repo::invoices", "data_entity:sql:invoices"),
    ("app::repo::customers", "data_entity:graph:Customer"),
    ("lib::report::report", "data_entity:sql:refunds"),
    (
        "tests::test_seed::test_seed",
        "data_entity:sql:fixtures_seed",
    ),
];

fn build() -> MergedGraph {
    let tmp = tempfile::tempdir().unwrap();
    let root: &Path = tmp.path();
    for (rel, body) in FILES {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    generate_one(root.to_str().unwrap())
        .expect("fixture builds")
        .merged
}

/// Every DATA_ENTITY qname of the build, sorted.
fn entities(m: &MergedGraph) -> Vec<String> {
    let mut out: Vec<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .filter(|(id, _)| g.nav.kind_by_id.get(id) == Some(&node_kind::DATA_ENTITY))
                .map(|(_, q)| q.clone())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every ACCESSES_DATA edge as (from qname, to qname), sorted.
fn accesses(m: &MergedGraph) -> Vec<(String, String)> {
    let qname = |id| {
        m.graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_default()
    };
    let mut out: Vec<(String, String)> = m
        .all_edges()
        .filter(|e| e.category == edge_category::ACCESSES_DATA)
        .map(|e| (qname(e.from), qname(e.to)))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn statement_sinks_are_no_data_access() {
    let m = build();
    let have = entities(&m);
    for q in FORBID {
        assert!(!have.iter().any(|h| h == q), "{q} minted: {have:?}");
    }
    let edges = accesses(&m);
    for (from, to) in EXPECT {
        assert!(
            edges.iter().any(|(f, t)| f == from && t == to),
            "{from} -> {to} missing: {edges:?}"
        );
    }
    // Nothing else is minted: the controls are the fixture's only entities.
    let mut want: Vec<String> = EXPECT.iter().map(|(_, to)| to.to_string()).collect();
    want.sort();
    assert_eq!(have, want);
}
