//! LD.4b — `trace::entry_flows`: every entry point's forward flow over the
//! code profile's carry edges, keyed by the repo-graph wrapper's slug, and the
//! entry-flow tier of `cross_stack_trace`'s seed resolution.
//!
//! The fixture is LD.4a's three-service build (see `trace_paths.rs`):
//! `web/checkout.ts` `placeOrder` POSTs `/orders`; `api/app.py`'s
//! `create_order` serves it and calls `save -> audit`, `bill` (POSTs
//! `/charge`) and `enqueue` (`producer.send('orders')`); `billing/app.py`
//! serves `/charge`.
//!
//! Before LD.4b neither `entry_flows` nor `glia flows` existed (the wrapper
//! walked every category from its own `ENTRY_KINDS` in Python), and
//! `glia trace api orders` seeded the dead-end QUEUE_PRODUCER `orders` and
//! printed `_(no outward flow)_`.

use std::path::Path;

use glia_engine::trace::{EntryFlow, TraceOptions, cross_stack_trace, entry_flows};
use glia_engine::{GenerateResult, generate_many, generate_one};

const CHECKOUT_TS: &str = "export async function placeOrder(body: unknown) {\n  return fetch('/orders', { method: 'POST', body: JSON.stringify(body) });\n}\n";
const API_PY: &str = "import requests\nfrom kafka import KafkaProducer\nfrom flask import Flask\n\napp = Flask(__name__)\nproducer = KafkaProducer()\n\n\ndef audit(order):\n    return order\n\n\ndef bill(order):\n    return requests.post('/charge', json=order)\n\n\ndef enqueue(order):\n    producer.send('orders', order)\n\n\ndef save(order):\n    return audit(order)\n\n\n@app.route('/orders', methods=['POST'])\ndef create_order():\n    order = {}\n    save(order)\n    bill(order)\n    enqueue(order)\n    return order\n";
const BILLING_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route('/charge', methods=['POST'])\ndef charge():\n    return {}\n";

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
    std::fs::write(p, text).expect("write fixture file");
}

fn dir(root: &Path, sub: &str) -> String {
    root.join(sub).to_string_lossy().into_owned()
}

/// The three dirs as three repos, with the build's repo labels.
fn three_repos() -> (tempfile::TempDir, GenerateResult) {
    let td = tempfile::tempdir().expect("tempdir");
    write(td.path(), "web/checkout.ts", CHECKOUT_TS);
    write(td.path(), "api/app.py", API_PY);
    write(td.path(), "billing/app.py", BILLING_PY);
    let paths = [
        dir(td.path(), "web"),
        dir(td.path(), "api"),
        dir(td.path(), "billing"),
    ];
    let r = generate_many(&paths).expect("generate_many");
    (td, r)
}

fn flow<'a>(flows: &'a [EntryFlow], kind: &str, qname: &str) -> &'a EntryFlow {
    flows
        .iter()
        .find(|f| f.entry.kind == kind && f.entry.qname == qname)
        .unwrap_or_else(|| panic!("no {kind} `{qname}` flow in {flows:#?}"))
}

#[test]
fn a_route_flow_reaches_billing_and_names_its_services() {
    let (_td, r) = three_repos();
    let flows = entry_flows(&r.merged, &r.repo_labels, 6);

    let orders = flow(&flows, "ROUTE", "POST /orders");
    assert_eq!(orders.key, "post_/orders");
    assert_eq!(orders.services, ["api", "billing"]);
    assert!(orders.cross_service);
    // First-seen order along the BFS tree: HANDLED_BY into the handler, its
    // CALLS, `enqueue` USES the producer it sends on, then the HTTP hop.
    assert_eq!(
        orders.mechanisms,
        ["HANDLED_BY", "CALLS", "USES", "HTTP_CALLS"]
    );
    assert_eq!(orders.reach, orders.hops.len());
    let last = orders.hops.last().expect("the flow has hops");
    assert_eq!(
        (last.mechanism, last.to_qname.as_str()),
        ("HANDLED_BY", "app::charge")
    );
    // The hops are the BFS tree: discovery order, depth never decreasing,
    // every hop leaving the entry or a node an earlier hop reached.
    assert_eq!(
        orders.hops.first().map(|h| h.from_qname.as_str()),
        Some("POST /orders")
    );
    for (i, h) in orders.hops.iter().enumerate() {
        assert!(
            i == 0 || orders.hops[i - 1].depth <= h.depth,
            "{:#?}",
            orders.hops
        );
        assert!(
            h.from_qname == orders.entry.qname
                || orders.hops[..i].iter().any(|p| p.to_qname == h.from_qname),
            "{:#?}",
            orders.hops
        );
    }
    // The one HTTP hop is the only crossing, and it lands in billing.
    let crossings: Vec<(&str, &str)> = orders
        .hops
        .iter()
        .filter(|h| h.cross_service)
        .map(|h| (h.mechanism, h.to_qname.as_str()))
        .collect();
    assert_eq!(crossings, [("HTTP_CALLS", "POST /charge")]);

    let charge = flow(&flows, "ROUTE", "POST /charge");
    assert_eq!(charge.key, "post_/charge");
    assert_eq!(charge.services, ["billing"]);
    assert_eq!(charge.reach, 1);
    assert!(!charge.cross_service);
    assert_eq!(charge.mechanisms, ["HANDLED_BY"]);

    // Every row reaches something, and the rows are ordered by
    // (key, entry qname, entry id).
    assert!(
        flows
            .iter()
            .all(|f| f.reach >= 1 && f.reach == f.hops.len())
    );
    let order: Vec<(&str, &str, u64)> = flows
        .iter()
        .map(|f| (f.key.as_str(), f.entry.qname.as_str(), f.entry.id))
        .collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);

    // `depth` bounds every flow; a depth the entry cannot leave keeps no row.
    let short = entry_flows(&r.merged, &r.repo_labels, 1);
    assert_eq!(flow(&short, "ROUTE", "POST /orders").reach, 1);
    assert!(short.iter().all(|f| f.hops.iter().all(|h| h.depth == 1)));
    assert!(entry_flows(&r.merged, &r.repo_labels, 0).is_empty());
}

#[test]
fn flows_follow_carry_edges_never_structure() {
    let (_td, r) = three_repos();
    let flows = entry_flows(&r.merged, &r.repo_labels, 6);
    assert!(!flows.is_empty());
    for f in &flows {
        for h in &f.hops {
            assert_ne!(h.to_kind, "MODULE", "{} reached a MODULE: {h:#?}", f.key);
            assert!(
                !["DEFINES", "CONTAINS", "IMPORTS"].contains(&h.mechanism),
                "{} walked {h:#?}",
                f.key
            );
        }
    }
    // Control: the handler IS contained and defined — a walk over every
    // category (the wrapper's) would have followed those edges.
    let create_order = r
        .merged
        .node_id_by_qname("app::create_order")
        .expect("create_order is a node");
    let structural = r.merged.all_edges().any(|e| {
        (e.from == create_order || e.to == create_order)
            && [
                glia_code_domain::edge_category::DEFINES,
                glia_code_domain::edge_category::CONTAINS,
            ]
            .contains(&e.category)
    });
    assert!(structural, "the fixture's handler has no structural edge");
}

#[test]
fn colliding_keys_are_all_kept() {
    let td = tempfile::tempdir().expect("tempdir");
    let serve = |f: &str| {
        format!(
            "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route('/x', methods=['GET'])\ndef {f}():\n    return {{}}\n"
        )
    };
    write(td.path(), "a/app.py", &serve("x_a"));
    write(td.path(), "b/app.py", &serve("x_b"));
    let r = generate_many(&[dir(td.path(), "a"), dir(td.path(), "b")]).expect("generate_many");
    let flows = entry_flows(&r.merged, &r.repo_labels, 6);
    let x: Vec<&EntryFlow> = flows.iter().filter(|f| f.key == "get_/x").collect();
    assert_eq!(x.len(), 2, "{flows:#?}");
    assert_ne!(x[0].entry.id, x[1].entry.id);
    assert!(
        (x[0].entry.qname.as_str(), x[0].entry.id) < (x[1].entry.qname.as_str(), x[1].entry.id),
        "{x:#?}"
    );
    let mut handlers: Vec<&str> = x
        .iter()
        .filter_map(|f| f.hops.last().map(|h| h.to_qname.as_str()))
        .collect();
    handlers.sort();
    assert_eq!(handlers, ["app::x_a", "app::x_b"]);
    let mut services: Vec<&[String]> = x.iter().map(|f| f.services.as_slice()).collect();
    services.sort();
    assert_eq!(services, [["a".to_string()], ["b".to_string()]]);
}

#[test]
fn a_dead_end_feature_word_yields_to_its_entry_flow() {
    let td = tempfile::tempdir().expect("tempdir");
    write(td.path(), "api/app.py", API_PY);
    let m = generate_one(&dir(td.path(), "api"))
        .expect("generate_one")
        .merged;

    // `orders` names the QUEUE_PRODUCER, which no carry edge leaves; the
    // entry keyed `post_/orders` contains the word.
    let a = cross_stack_trace(&m, "orders", &TraceOptions::default());
    assert_eq!(a.resolved_by, "entry_flow");
    let seed = a.seed.as_ref().expect("orders resolves");
    assert_eq!((seed.kind, seed.qname.as_str()), ("ROUTE", "POST /orders"));
    assert!(a.absence.is_none(), "{:?}", a.absence);
    assert_eq!(
        a.hops.first().map(|h| (h.mechanism, h.to_qname.as_str())),
        Some(("HANDLED_BY", "app::create_order"))
    );
    assert!(!a.paths.is_empty());

    // The key itself, which names no node, resolves the same way.
    let k = cross_stack_trace(&m, "post_/orders", &TraceOptions::default());
    assert_eq!(k.resolved_by, "entry_flow");
    assert_eq!(k.seed.map(|s| s.qname), Some("POST /orders".to_string()));

    // A hit with an outgoing carry edge is never replaced ...
    let live = cross_stack_trace(&m, "app::save", &TraceOptions::default());
    assert_eq!(live.resolved_by, "qname");
    assert_eq!(live.seed.map(|s| s.qname), Some("app::save".to_string()));
    // ... nor is a dead end no entry key matches: that stays an absence.
    let dead = cross_stack_trace(&m, "app::audit", &TraceOptions::default());
    assert_eq!(dead.resolved_by, "qname");
    assert_eq!(
        dead.absence.map(|w| w.reason),
        Some("no_edges"),
        "app::audit is a dead end"
    );
    // ... nor does two-node mode re-seed: its dead end is a path question.
    let mut to = TraceOptions::default();
    to.to = Some("app::create_order".to_string());
    let two = cross_stack_trace(&m, "orders", &to);
    assert_eq!(two.resolved_by, "name");
    assert_eq!(
        two.seed.map(|s| s.qname),
        Some("queue_producer:orders".to_string())
    );
}
