//! LE.6b — `cycles`: cross-service loops first, then import cycles.
//!
//! - an EventEmitter loop (orders emits `order.placed`, billing handles it and
//!   emits `payment.settled`, orders handles that and re-places the order) is
//!   ONE derived `event_loop` whose witness walks every observed edge;
//! - a kafkajs pair where each service publishes to the other but the orders
//!   handler never republishes is only a `possible_loop` (heuristic);
//! - a Python `a <-> b` import is a 2-module `import_cycle` located at both
//!   import lines.
//!
//! The orders/ and billing/ trees stay manifest-free on purpose: with a
//! manifest in each, LB.8b treats them as separate projects and correctly
//! drops the cross-project in-process EVENT_FLOWS the loop rides on.

use std::collections::{BTreeMap, BTreeSet};

use glia_engine::cycles::{CycleArgs, CycleRow, cycles, kinds_for};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const EV_ORDERS: &str = "import { EventEmitter } from \"events\";\nexport const bus = new EventEmitter();\nexport function placeOrder(o) { bus.emit(\"order.placed\", o); }\nexport function registerOrderHandlers() { bus.on(\"payment.settled\", (p) => { retryOrder(p); }); }\nfunction retryOrder(p) { placeOrder(p); }\n";
const EV_BILLING: &str = "import { EventEmitter } from \"events\";\nexport const bus = new EventEmitter();\nexport function registerBillingHandlers() { bus.on(\"order.placed\", (o) => { settle(o); }); }\nfunction settle(o) { bus.emit(\"payment.settled\", o); }\n";

const KAFKA_ORDERS: &str = "import { Kafka } from \"kafkajs\";\nconst kafka = new Kafka({ clientId: \"orders\", brokers: [\"localhost:9092\"] });\nconst producer = kafka.producer();\nconst consumer = kafka.consumer({ groupId: \"orders\" });\nexport async function placeOrder(o) { await producer.send({ topic: 'orders.placed', messages: [] }); }\nexport async function onPayment(msg) { await markPaid(msg); }\nexport async function markPaid(msg) { return msg; }\nexport async function start() { await consumer.subscribe({ topic: 'payments.settled' }); await consumer.run({ eachMessage: onPayment }); }\n";
const KAFKA_BILLING: &str = "import { Kafka } from \"kafkajs\";\nconst kafka = new Kafka({ clientId: \"billing\", brokers: [\"localhost:9092\"] });\nconst producer = kafka.producer();\nconst consumer = kafka.consumer({ groupId: \"billing\" });\nexport async function settle(order) { await producer.send({ topic: 'payments.settled', messages: [] }); }\nexport async function onOrder(msg) { await settle(msg); }\nexport async function start() { await consumer.subscribe({ topic: 'orders.placed' }); await consumer.run({ eachMessage: onOrder }); }\n";

const PY_A: &str = "from pkg.b import f\n\n\ndef g():\n    return 1\n";
const PY_B: &str = "from pkg.a import g\n\n\ndef f():\n    return 2\n";

/// Build one repo from `(relative path, source)` pairs; the tempdir is
/// returned so it outlives the graph's use.
fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph, BTreeMap<u64, String>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (rel, src) in files {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let r = generate_one(tmp.path().to_str().expect("utf-8 temp path")).expect("generate_one");
    (tmp, r.merged, r.repo_labels)
}

fn evloop() -> (tempfile::TempDir, MergedGraph, BTreeMap<u64, String>) {
    build(&[("orders/app.ts", EV_ORDERS), ("billing/app.ts", EV_BILLING)])
}

fn args(kind: &str, scope: Option<&str>) -> CycleArgs {
    let mut a = CycleArgs::default();
    a.kinds = kinds_for(kind).expect("a known kind");
    a.scope = scope.map(str::to_string);
    a
}

fn of_kind<'a>(rows: &'a [CycleRow], kind: &str) -> Vec<&'a CycleRow> {
    rows.iter().filter(|r| r.kind == kind).collect()
}

#[test]
fn event_emitter_loop_is_a_derived_event_loop() {
    let (_tmp, merged, labels) = evloop();
    let rows = cycles(&merged, &labels, &args("all", None));
    let loops = of_kind(&rows, "event_loop");
    assert_eq!(loops.len(), 1, "{rows:#?}");
    assert_eq!(
        rows.len(),
        1,
        "no possible_loop over the same services, no import cycle: {rows:#?}"
    );
    let row = loops[0];
    assert_eq!(row.tier, "derived");
    assert_eq!(row.services, vec!["billing", "orders"]);
    assert_eq!(row.channels, vec!["order.placed", "payment.settled"]);
    assert_eq!(row.mechanisms, vec!["EVENT_FLOWS"]);
    assert_eq!(row.size, 9);
    assert_eq!(row.members.len(), 9);
    assert!(row.note.is_none());

    let w = &row.witness;
    assert_eq!(w.len(), 9, "{w:#?}");
    // A closed chain: each hop ends where the next begins, the last ends at the start.
    for pair in w.windows(2) {
        assert_eq!(pair[0].to_qname, pair[1].from_qname, "{w:#?}");
    }
    assert_eq!(w[w.len() - 1].to_qname, w[0].from_qname);
    let nodes: BTreeSet<&str> = w.iter().map(|h| h.from_qname.as_str()).collect();
    assert_eq!(nodes.len(), 9, "9 distinct nodes: {nodes:?}");
    let members: BTreeSet<&str> = row.members.iter().map(String::as_str).collect();
    assert_eq!(nodes, members);
    let emitters = nodes
        .iter()
        .filter(|q| q.starts_with("event_emit:"))
        .count();
    let handlers = nodes
        .iter()
        .filter(|q| q.starts_with("event_handle:"))
        .count();
    assert_eq!((emitters, handlers), (2, 2), "{nodes:?}");
    for f in [
        "orders::app::placeOrder",
        "orders::app::registerOrderHandlers",
        "orders::app::retryOrder",
        "billing::app::registerBillingHandlers",
        "billing::app::settle",
    ] {
        assert!(nodes.contains(f), "{f} is on the witness: {nodes:?}");
    }
    let flows: Vec<(&str, Option<&str>)> = w
        .iter()
        .filter(|h| h.category == "EVENT_FLOWS")
        .map(|h| (h.from_qname.as_str(), h.channel.as_deref()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(
        flows,
        vec![
            ("event_emit:order.placed", Some("order.placed")),
            ("event_emit:payment.settled", Some("payment.settled")),
        ]
    );
    // Every hop is located, 1-based, in one of the two sources.
    for h in w {
        let file = h.file.as_deref().expect("hop located");
        assert!(file == "orders/app.ts" || file == "billing/app.ts", "{h:?}");
        assert!(h.line.is_some_and(|l| l >= 1), "{h:?}");
    }
    let calls: BTreeSet<(&str, &str)> = w
        .iter()
        .filter(|h| h.category == "CALLS")
        .map(|h| (h.from_qname.as_str(), h.to_qname.as_str()))
        .collect();
    assert!(
        calls.contains(&("orders::app::retryOrder", "orders::app::placeOrder")),
        "{calls:?}"
    );
}

#[test]
fn kafka_service_loop_without_causal_path_is_possible_only() {
    let (_tmp, merged, labels) = build(&[
        ("orders/app.ts", KAFKA_ORDERS),
        ("billing/app.ts", KAFKA_BILLING),
    ]);
    let rows = cycles(&merged, &labels, &args("all", None));
    assert!(of_kind(&rows, "event_loop").is_empty(), "{rows:#?}");
    assert!(of_kind(&rows, "call_loop").is_empty(), "{rows:#?}");
    let possible = of_kind(&rows, "possible_loop");
    assert_eq!(possible.len(), 1, "{rows:#?}");
    let row = possible[0];
    assert_eq!(row.tier, "heuristic");
    assert_eq!(row.services, vec!["billing", "orders"]);
    assert_eq!(row.members, vec!["billing", "orders"]);
    assert_eq!(row.size, 2);
    assert_eq!(row.channels, vec!["orders.placed", "payments.settled"]);
    assert_eq!(row.mechanisms, vec!["QUEUE_FLOWS"]);
    assert!(
        row.note
            .is_some_and(|n| n.contains("no consumer-handler -> producer path"))
    );
    // The witness is the two service links, each located at its producer.
    let hops: BTreeSet<(&str, &str, Option<&str>)> = row
        .witness
        .iter()
        .map(|h| {
            (
                h.from_qname.as_str(),
                h.to_qname.as_str(),
                h.channel.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        hops,
        BTreeSet::from([
            ("billing", "orders", Some("payments.settled")),
            ("orders", "billing", Some("orders.placed")),
        ])
    );
    for h in &row.witness {
        assert_eq!(h.category, "QUEUE_FLOWS");
        let want = format!("{}/app.ts", h.from_qname);
        assert_eq!(h.file.as_deref(), Some(want.as_str()), "{h:?}");
        assert!(h.line.is_some_and(|l| l >= 1), "{h:?}");
    }
    // `--kind import` never runs the flow analysis.
    assert!(cycles(&merged, &labels, &args("import", None)).is_empty());
}

#[test]
fn python_import_cycle() {
    let (_tmp, merged, labels) = build(&[
        ("pkg/__init__.py", ""),
        ("pkg/a.py", PY_A),
        ("pkg/b.py", PY_B),
    ]);
    let rows = cycles(&merged, &labels, &args("all", None));
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let row = &rows[0];
    assert_eq!((row.kind, row.tier), ("import_cycle", "derived"));
    assert_eq!(row.size, 2);
    assert_eq!(row.members, vec!["pkg::a", "pkg::b"]);
    assert_eq!(row.mechanisms, vec!["IMPORTS"]);
    assert!(row.channels.is_empty());
    assert_eq!(row.services, vec!["pkg"]);
    assert_eq!(row.witness.len(), 2, "{:#?}", row.witness);
    let sites: BTreeSet<(&str, &str, Option<&str>, Option<i64>)> = row
        .witness
        .iter()
        .map(|h| {
            (
                h.from_qname.as_str(),
                h.to_qname.as_str(),
                h.file.as_deref(),
                h.line,
            )
        })
        .collect();
    assert_eq!(
        sites,
        BTreeSet::from([
            ("pkg::a", "pkg::b", Some("pkg/a.py"), Some(1)),
            ("pkg::b", "pkg::a", Some("pkg/b.py"), Some(1)),
        ])
    );
    assert!(
        row.witness
            .iter()
            .all(|h| h.category == "IMPORTS" && h.channel.is_none())
    );
    // `--kind event` never runs the import analysis.
    assert!(cycles(&merged, &labels, &args("event", None)).is_empty());
}

#[test]
fn acyclic_repo_is_empty() {
    let (_tmp, merged, labels) = build(&[
        (
            "orders/app.ts",
            "export function placeOrder(o) { return save(o); }\nfunction save(o) { return o; }\n",
        ),
        (
            "billing/app.py",
            "from billing.util import charge\n\n\ndef bill(o):\n    return charge(o)\n",
        ),
        ("billing/util.py", "def charge(o):\n    return o\n"),
    ]);
    assert!(cycles(&merged, &labels, &args("all", None)).is_empty());
    assert!(kinds_for("loops").is_err(), "an unknown kind is refused");
    assert_eq!(kinds_for("all").expect("all"), vec!["event", "import"]);
}

#[test]
fn scope_excludes_partial_cycles() {
    let (_tmp, merged, labels) = evloop();
    // The loop spans orders/ and billing/: a scope holding only one of them
    // drops it; a scope holding both (the repo root) keeps it.
    assert!(cycles(&merged, &labels, &args("all", Some("orders"))).is_empty());
    assert!(cycles(&merged, &labels, &args("all", Some("billing"))).is_empty());
    assert_eq!(cycles(&merged, &labels, &args("all", Some("."))).len(), 1);

    let (_tmp, merged, labels) = build(&[
        ("pkg/__init__.py", ""),
        ("pkg/a.py", PY_A),
        ("pkg/b.py", PY_B),
    ]);
    assert_eq!(
        cycles(&merged, &labels, &args("import", Some("pkg"))).len(),
        1
    );
    assert!(cycles(&merged, &labels, &args("import", Some("pkg/a.py"))).is_empty());

    // max_members caps the list, never the size.
    let (_tmp, merged, labels) = evloop();
    let mut capped = args("event", None);
    capped.max_members = 3;
    let rows = cycles(&merged, &labels, &capped);
    assert_eq!((rows[0].size, rows[0].members.len()), (9, 3));
}

#[test]
fn deterministic_across_two_builds() {
    let render = || {
        let (_tmp, merged, labels) = evloop();
        let a = serde_json::to_string(&cycles(&merged, &labels, &args("all", None)))
            .expect("serialises");
        let (_tmp, merged, labels) = build(&[
            ("orders/app.ts", KAFKA_ORDERS),
            ("billing/app.ts", KAFKA_BILLING),
        ]);
        let b = serde_json::to_string(&cycles(&merged, &labels, &args("all", None)))
            .expect("serialises");
        (a, b)
    };
    let first = render();
    assert_eq!(first, render());
    assert!(first.0.contains("\"event_loop\"") && first.1.contains("\"possible_loop\""));
}
