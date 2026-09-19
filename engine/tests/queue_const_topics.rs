//! LA.4 (programme A11.7) acceptance: queue topics named by a constant fold
//! through the repo const table after the parse cache, on REAL builds.
//!
//! `grade.py` reads the installed wheel, so it cannot see this change until the
//! end-of-wave rebuild; these tests build the committed fixtures from the
//! working tree. The cache test is the reason the fold runs post-cache: a
//! cache-served file must fold against the CURRENT table, never the one its
//! parse was cached under.

use std::collections::BTreeSet;
use std::path::Path;

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, NodeId, NodeKindId};
use glia_engine::{GenerateResult, generate_many, generate_many_incremental};
use glia_graph::MergedGraph;

fn fixture(name: &str, dirs: &[&str]) -> GenerateResult {
    let root = format!(
        "{}/../bench/substrate-gap/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let dirs: Vec<String> = dirs.iter().map(|d| format!("{root}/{d}")).collect();
    generate_many(&dirs).expect("fixture builds")
}

/// Every qname of `kind`, across graphs.
fn qnames(m: &MergedGraph, kind: NodeKindId) -> BTreeSet<String> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .kind_by_id
                .iter()
                .filter(move |(_, k)| **k == kind)
                .filter_map(move |(id, _)| g.nav.qname_by_id.get(id).cloned())
        })
        .collect()
}

fn node_id(m: &MergedGraph, kind: NodeKindId, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .find_map(|g| {
            g.nav.qname_by_id.iter().find_map(|(id, q)| {
                (q == qname && g.nav.kind_by_id.get(id) == Some(&kind)).then_some(*id)
            })
        })
        .unwrap_or_else(|| panic!("no {qname} node of kind {kind:?}"))
}

fn flows(m: &MergedGraph, from: &str, to: &str) -> bool {
    let (Some(f), Some(t)) = (
        m.graphs.iter().find_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .find_map(|(id, q)| (q == from).then_some(*id))
        }),
        m.graphs.iter().find_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .find_map(|(id, q)| (q == to).then_some(*id))
        }),
    ) else {
        return false;
    };
    m.all_edges()
        .any(|e| e.from == f && e.to == t && e.category == edge_category::QUEUE_FLOWS)
}

/// Every stored POSITION payload on `id`, across graphs.
fn positions(m: &MergedGraph, id: NodeId) -> Vec<String> {
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == cell_type::POSITION)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) => Some(j.clone()),
            _ => None,
        })
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn const_topics_fold_and_pair() {
    let r = fixture("xcut-queue-const-topic", &["producer", "consumer"]);
    let m = &r.merged;
    // Exactly the two constant-named topics: no `audit-log` borrowed from a
    // lower-case binding in another file, no `topic` minted from the
    // parameter, and no sentinel left behind by the parameter site.
    assert_eq!(
        qnames(m, node_kind::QUEUE_PRODUCER),
        set(&["queue_producer:orders", "queue_producer:payments"])
    );
    assert_eq!(
        qnames(m, node_kind::QUEUE_CONSUMER),
        set(&["queue_consumer:orders", "queue_consumer:payments"])
    );
    assert!(flows(m, "queue_producer:orders", "queue_consumer:orders"));
    assert!(flows(
        m,
        "queue_producer:payments",
        "queue_consumer:payments"
    ));
    assert!(!flows(
        m,
        "queue_producer:orders",
        "queue_consumer:payments"
    ));
    // The folded node keeps its call-site provenance (0-indexed send line).
    let orders = node_id(m, node_kind::QUEUE_PRODUCER, "queue_producer:orders");
    assert_eq!(
        positions(m, orders),
        vec![r#"{"file":"publish.ts","start_line":7,"end_line":7}"#.to_string()]
    );
    // Shaped like a literal node: POSITION, CODE, then the IMPORTS cell last.
    let cells: Vec<_> = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == orders)
        .flat_map(|n| n.cells.iter().map(|c| c.kind))
        .collect();
    assert_eq!(cells.first(), Some(&cell_type::POSITION));
    assert_eq!(cells.last(), Some(&cell_type::IMPORTS));
    assert_eq!(
        cells.iter().filter(|k| **k == cell_type::IMPORTS).count(),
        1
    );
}

#[test]
fn java_constants_fold_on_both_sides() {
    let r = fixture("xcut-queue-const-topic-java", &["orders", "billing"]);
    let m = &r.merged;
    assert!(flows(m, "queue_producer:orders", "queue_consumer:orders"));
    let all: Vec<String> = qnames(m, node_kind::QUEUE_PRODUCER)
        .into_iter()
        .chain(qnames(m, node_kind::QUEUE_CONSUMER))
        .collect();
    assert!(
        all.iter().all(|q| !q.contains("unresolved:")),
        "no sentinel survives: {all:?}"
    );
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

const PUBLISH_TS: &str = "import { Kafka } from 'kafkajs';\nimport { ORDERS_TOPIC } from './topics';\n\nconst producer = new Kafka({ brokers: [] }).producer();\n\nexport async function publish(): Promise<void> {\n  await producer.send({ topic: ORDERS_TOPIC, messages: [] });\n}\n";

#[test]
fn cache_served_parse_folds_against_the_current_table() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("producer");
    write(
        &repo,
        "topics.ts",
        "export const ORDERS_TOPIC = 'orders';\n",
    );
    write(&repo, "publish.ts", PUBLISH_TS);
    let dirs = [repo.to_str().unwrap().to_string()];

    let cold = generate_many_incremental(&dirs).unwrap();
    assert_eq!(
        qnames(&cold.merged, node_kind::QUEUE_PRODUCER),
        set(&["queue_producer:orders"])
    );
    let warm = generate_many_incremental(&dirs).unwrap();
    assert_eq!(
        qnames(&warm.merged, node_kind::QUEUE_PRODUCER),
        set(&["queue_producer:orders"]),
        "a cache-served publish.ts still folds"
    );

    // Only topics.ts changes: publish.ts is served from the parse cache, and
    // must fold against the new binding, exactly as a clean build does.
    write(
        &repo,
        "topics.ts",
        "export const ORDERS_TOPIC = 'orders.v2';\n",
    );
    let edited = generate_many_incremental(&dirs).unwrap();
    let clean = generate_many(&dirs).unwrap();
    assert_eq!(
        qnames(&edited.merged, node_kind::QUEUE_PRODUCER),
        set(&["queue_producer:orders.v2"])
    );
    assert_eq!(
        qnames(&edited.merged, node_kind::QUEUE_PRODUCER),
        qnames(&clean.merged, node_kind::QUEUE_PRODUCER)
    );
}
