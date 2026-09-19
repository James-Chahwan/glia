//! LE.4a acceptance: data access is attributed to the function that runs the
//! statement, with an ACCESS_MODE edge cell, on a REAL build.
//!
//! `grade.py` reads the installed wheel and has no edge-cell vocabulary, so
//! the modes are gated here. The TypeScript source is the committed fixture
//! `bench/substrate-gap/fixtures/xcut-data-access-fn`, written to a tempdir so
//! the build owns its directory. Run with `-- --nocapture` to see the
//! `[data-access]` marker.

use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, Edge, NodeId, NodeKindId};
use glia_engine::{ParseCache, generate_one, generate_one_with_cache};
use glia_graph::MergedGraph;
use std::path::Path;

const ORDERS_TS: &str =
    include_str!("../../bench/substrate-gap/fixtures/xcut-data-access-fn/svc/orders.ts");

/// A Go function whose parser already emits `Archive -> orders` (the GORM
/// `db.Table("orders")` site) and whose raw `DELETE FROM orders` is an
/// extractor site in the same function.
const ARCHIVE_GO: &str = r#"package store

import "gorm.io/gorm"

func Archive(db *gorm.DB, id int) error {
	db.Table("orders").Where("id = ?", id).Update("archived", true)
	return db.Exec("DELETE FROM orders WHERE archived = true").Error
}
"#;

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

/// The id of the node of `kind` whose qname is exactly `qname`.
fn node_id(m: &MergedGraph, kind: NodeKindId, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .filter(move |(id, q)| {
                    q.as_str() == qname && g.nav.kind_by_id.get(id) == Some(&kind)
                })
                .map(|(id, _)| *id)
        })
        .next()
        .unwrap_or_else(|| panic!("no {qname} node of kind {kind:?}"))
}

/// Every ACCESSES_DATA edge `from -> to`.
fn access_edges(m: &MergedGraph, from: NodeId, to: NodeId) -> Vec<&Edge> {
    m.all_edges()
        .filter(|e| e.from == from && e.to == to && e.category == edge_category::ACCESSES_DATA)
        .collect()
}

/// The ACCESS_MODE text of `e`, if it carries one.
fn mode(e: &Edge) -> Option<String> {
    e.cell(cell_type::ACCESS_MODE).map(|c| match &c.payload {
        CellPayload::Text(t) => t.clone(),
        other => panic!("ACCESS_MODE must be Text, got {other:?}"),
    })
}

/// The one ACCESSES_DATA edge `from -> to`, asserting there is exactly one.
fn one_edge<'a>(m: &'a MergedGraph, from: NodeId, to: NodeId, what: &str) -> &'a Edge {
    let edges = access_edges(m, from, to);
    assert_eq!(edges.len(), 1, "{what}: {edges:?}");
    edges[0]
}

#[test]
fn raw_sql_access_is_rehomed_to_the_function_with_its_mode() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "svc/orders.ts", ORDERS_TS);
    let m = build(tmp.path());

    let module = node_id(&m, node_kind::MODULE, "svc::orders");
    let orders = node_id(&m, node_kind::DATA_ENTITY, "data_entity:sql:orders");
    let audit = node_id(&m, node_kind::DATA_ENTITY, "data_entity:sql:audit_log");
    let func = |name: &str| node_id(&m, node_kind::FUNCTION, &format!("svc::orders::{name}"));

    for (name, want) in [
        ("saveOrder", "write"),
        ("loadOrder", "read"),
        ("archiveOrder", "read_write"),
    ] {
        let e = one_edge(&m, func(name), orders, name);
        assert_eq!(mode(e).as_deref(), Some(want), "{name}");
        let ev = Evidence::of(e).expect("the re-homed edge is stamped");
        assert_eq!(ev.emitter, "extractor:data_entities", "{name}");
        assert_eq!(ev.basis, Basis::Site, "{name}");
        assert_eq!(ev.file.as_deref(), Some("svc/orders.ts"), "{name}");
        let line = ev.line.expect("site line") as usize;
        assert!(
            ORDERS_TS
                .lines()
                .nth(line)
                .is_some_and(|l| l.contains("orders")),
            "{name}: evidence line {line} is the statement's"
        );
    }
    assert!(
        access_edges(&m, module, orders).is_empty(),
        "every orders statement is inside a function: no module edge"
    );
    assert!(
        access_edges(&m, func("audit"), audit).is_empty(),
        "the statement sits at module scope, not in the function that runs it"
    );
    let kept = one_edge(&m, module, audit, "module-scope AUDIT_SQL");
    assert_eq!(mode(kept), None, "a module edge carries no ACCESS_MODE");
}

#[test]
fn a_parser_edge_takes_the_mode_instead_of_a_duplicate() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "store/archive.go", ARCHIVE_GO);
    let m = build(tmp.path());

    let archive = node_id(&m, node_kind::FUNCTION, "store::archive::Archive");
    let orders = node_id(&m, node_kind::DATA_ENTITY, "data_entity:sql:orders");
    let e = one_edge(
        &m,
        archive,
        orders,
        "GORM site + raw DELETE in one function",
    );
    assert_eq!(mode(e).as_deref(), Some("write"));
    let emitter = Evidence::of(e).map(|ev| ev.emitter).unwrap_or_default();
    assert!(
        emitter.starts_with("parser:"),
        "the parser's edge keeps its own evidence, got {emitter}"
    );
    let module_edges = m
        .all_edges()
        .filter(|e| {
            e.to == orders
                && e.category == edge_category::ACCESSES_DATA
                && m.graphs
                    .iter()
                    .any(|g| g.nav.kind_by_id.get(&e.from) == Some(&node_kind::MODULE))
        })
        .count();
    assert_eq!(module_edges, 0, "the raw statement is inside Archive");
}

#[test]
fn cached_parses_carry_the_same_rehomed_edges() {
    // The re-home runs inside the per-file extractors and is cached with the
    // parse: a warm build that replays one file and reparses the other must
    // store exactly what a clean build stores.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    write(&repo, "svc/orders.ts", ORDERS_TS);
    write(&repo, "store/archive.go", ARCHIVE_GO);
    let repo_s = repo.to_str().unwrap();

    let mut cache = ParseCache::new();
    generate_one_with_cache(repo_s, &mut cache).unwrap();
    write(
        &repo,
        "store/archive.go",
        &ARCHIVE_GO.replace("id int", "id int64"),
    );
    let warm = generate_one_with_cache(repo_s, &mut cache).unwrap().merged;
    assert!(cache.stats.reused > 0 && cache.stats.reparsed > 0);
    let clean = generate_one(repo_s).unwrap().merged;
    assert_eq!(
        store_bytes(&warm, &tmp.path().join("warm")),
        store_bytes(&clean, &tmp.path().join("clean")),
        "incremental vs clean with re-homed data access"
    );
    let orders = node_id(&clean, node_kind::DATA_ENTITY, "data_entity:sql:orders");
    let save = node_id(&clean, node_kind::FUNCTION, "svc::orders::saveOrder");
    assert_eq!(
        mode(one_edge(&warm, save, orders, "warm")).as_deref(),
        Some("write")
    );
}

fn store_bytes(m: &MergedGraph, dir: &Path) -> Vec<(String, Vec<u8>)> {
    glia_store::write_merged_sharded(m, dir).unwrap();
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}
