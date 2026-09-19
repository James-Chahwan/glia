//! LA.33 acceptance: a queue consumer whose handler is passed as a callback
//! is HANDLED_BY that handler, on REAL builds of the committed fixtures.
//!
//! `grade.py` reads the installed wheel, so it cannot see this change until
//! the end-of-wave rebuild; these tests build the fixtures from the working
//! tree and grade them against their own `key.json` (every `expect_edges` row
//! present, every `forbid` row absent, qnames matched exactly after the
//! grader's `/` -> `::` normalisation). Run with `-- --nocapture` to see the
//! `[queue-callbacks]` marker.

use std::path::Path;

use repo_graph_code_domain::evidence::Evidence;
use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Edge, NodeId};
use repo_graph_engine::{BlastOptions, blast_radius, generate_one};
use repo_graph_graph::{MergedGraph, Reach};

fn fixture(name: &str) -> String {
    format!(
        "{}/../bench/substrate-gap/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn build(path: &str) -> MergedGraph {
    generate_one(path).expect("fixture builds").merged
}

/// Every node id whose qname is exactly `qname`.
fn ids(m: &MergedGraph, qname: &str) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .filter(move |(_, q)| *q == qname)
                .map(|(id, _)| *id)
        })
        .collect();
    out.sort_by_key(|id| id.0);
    out.dedup();
    out
}

/// The HANDLED_BY edges from `from` to `to` (qnames), across the merge.
fn handled_by<'a>(m: &'a MergedGraph, from: &str, to: &str) -> Vec<&'a Edge> {
    let (f, t) = (ids(m, from), ids(m, to));
    m.all_edges()
        .filter(|e| {
            e.category == edge_category::HANDLED_BY && f.contains(&e.from) && t.contains(&e.to)
        })
        .collect()
}

/// A key.json name as the grader normalises it (`svc/worker/x` -> `svc::worker::x`).
fn norm(s: &str) -> String {
    s.replace('/', "::")
}

/// Grade `name` against its own key.json; returns the build.
fn grade(name: &str) -> MergedGraph {
    let root = fixture(name);
    let key: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(&root).join("key.json")).expect("key.json"),
    )
    .expect("key.json parses");
    let m = build(&root);
    let rows = |field: &str| -> Vec<(String, String)> {
        key[field]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|r| r["category"] == "HANDLED_BY")
                    .map(|r| {
                        (
                            norm(r["from"].as_str().unwrap_or_default()),
                            norm(r["to"].as_str().unwrap_or_default()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let expect = rows("expect_edges");
    let forbid = rows("forbid");
    assert!(!expect.is_empty() && !forbid.is_empty(), "{name}: key rows");
    for (from, to) in &expect {
        assert_eq!(
            handled_by(&m, from, to).len(),
            1,
            "{name}: expected exactly one {from} -HANDLED_BY-> {to}"
        );
    }
    for (from, to) in &forbid {
        assert!(
            handled_by(&m, from, to).is_empty(),
            "{name}: forbidden {from} -HANDLED_BY-> {to}"
        );
    }
    m
}

#[test]
fn ts_fixture_binds_every_callback_shape() {
    let m = grade("xcut-queue-consumer-callbacks");
    // LE.4c's owner edge to the subscribing function stays beside the handler.
    assert_eq!(
        handled_by(&m, "queue_consumer:payments", "svc::worker::start").len(),
        1
    );
    // `this.handle.bind(this)` is bound in-file, with the extractor's evidence
    // at the `run(` call (0-based line 7 of refunds.ts).
    let direct = handled_by(
        &m,
        "queue_consumer:refunds",
        "svc::refunds::RefundsConsumer::handle",
    );
    let ev = Evidence::of(direct[0]).expect("evidence");
    assert_eq!(ev.emitter, "extractor:queue_callbacks");
    assert_eq!(ev.rule.as_deref(), Some("self"));
    assert_eq!(
        (ev.file.as_deref(), ev.line),
        (Some("svc/refunds.ts"), Some(7))
    );
    // Names go through the graph builder's resolve_refs: the imported handler
    // binds through the import binding, the same-file one through the module.
    let rule = |from: &str, to: &str| {
        Evidence::of(handled_by(&m, from, to)[0]).and_then(|ev| ev.rule.map(|r| (ev.emitter, r)))
    };
    assert_eq!(
        rule("queue_consumer:orders", "svc::handlers::onOrder"),
        Some(("graph:refs".to_string(), "import_binding".to_string()))
    );
    assert_eq!(
        rule("queue_consumer:payments", "svc::worker::handlePayment"),
        Some(("graph:refs".to_string(), "module_symbol".to_string()))
    );
    // Nothing the callbacks named is left unresolved.
    let consumers: Vec<NodeId> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .kind_by_id
                .iter()
                .filter(|(_, k)| **k == node_kind::QUEUE_CONSUMER)
                .map(|(id, _)| *id)
        })
        .collect();
    assert!(
        m.graphs
            .iter()
            .flat_map(|g| &g.unresolved_refs)
            .all(|r| !consumers.contains(&r.from)),
        "every callback ref bound"
    );
}

#[test]
fn go_fixture_binds_functions_literals_and_method_values() {
    let m = grade("go-nats-consumer-callbacks");
    let direct = handled_by(&m, "queue_consumer:refunds", "main::Worker::handle");
    let ev = Evidence::of(direct[0]).expect("evidence");
    assert_eq!(
        (ev.emitter.as_str(), ev.rule.as_deref()),
        ("extractor:queue_callbacks", Some("method_value"))
    );
    assert_eq!(ev.line, Some(15), "the Subscribe call's 0-based line");
}

#[test]
fn blast_radius_continues_from_the_consumer_into_its_handler() {
    // The consumer's forward closure now reaches the handler's own callee.
    let m = build(&fixture("xcut-queue-consumer-callbacks"));
    let mut forward = BlastOptions::default();
    forward.direction = Reach::Forward;
    let blast = blast_radius(&m, &["queue_consumer:payments"], &forward);
    assert!(blast.unresolved.is_empty(), "seed resolves");
    let reached: Vec<&str> = blast.results.iter().map(|b| b.qname.as_str()).collect();
    assert!(reached.contains(&"svc::worker::handlePayment"), "{reached:?}");
    assert!(reached.contains(&"svc::worker::settle"), "{reached:?}");
}

#[test]
fn a_folded_topic_rebinds_its_callback_and_drops_the_sentinel() {
    // The topic is a same-file constant: the per-file parse mints the
    // `unresolved:kafka` sentinel and binds the callback to it; LA.4's
    // post-cache fold swaps it for `queue_consumer:orders`, which must carry
    // the callback, and nothing may still name the sentinel.
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = "import { Kafka } from 'kafkajs';\n\nconst ORDERS_TOPIC = 'orders';\nconst consumer = new Kafka({ brokers: [] }).consumer({ groupId: 'g' });\n\nexport async function onOrder({ message }) { return message; }\n\nexport async function start() {\n  await consumer.subscribe({ topic: ORDERS_TOPIC });\n  await consumer.run({ eachMessage: onOrder });\n}\n";
    std::fs::write(tmp.path().join("worker.ts"), src).expect("write");
    let m = build(tmp.path().to_str().expect("utf-8 path"));
    assert_eq!(
        handled_by(&m, "queue_consumer:orders", "worker::onOrder").len(),
        1
    );
    assert_eq!(
        handled_by(&m, "queue_consumer:orders", "worker::start").len(),
        1,
        "LE.4c's owner edge is re-anchored on the folded id"
    );
    assert!(ids(&m, "queue_consumer:unresolved:kafka").is_empty());
    let sentinel = repo_graph_core::NodeId::from_parts(
        repo_graph_code_domain::GRAPH_TYPE,
        m.graphs[0].repo,
        node_kind::QUEUE_CONSUMER,
        "queue_consumer:unresolved:kafka",
    );
    assert!(
        m.all_edges()
            .all(|e| e.from != sentinel && e.to != sentinel),
        "no edge names the sentinel"
    );
    assert!(
        m.graphs
            .iter()
            .flat_map(|g| &g.unresolved_refs)
            .all(|r| r.from != sentinel),
        "no ref names the sentinel"
    );
}
