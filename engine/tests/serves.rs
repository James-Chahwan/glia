//! LD.8b — `serves`: who serves a channel. An HTTP `METHOD /path` goes through
//! the HTTP resolver's own route matcher (placeholder folding, API-prefix
//! tiers, the method-agnostic `ANY` fallback) and names the tier that matched;
//! a queue topic finds its `queue_consumer:<topic>` nodes. An empty answer is
//! a FACT-tier `unserved_channel` absence with its caveat rows and the near
//! misses an agent asks for next (the route under another verb, the parent
//! path's routes; the topic's producers).

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::node_kind;
use glia_core::NodeId;
use glia_engine::generate_many;
use glia_engine::serves::serves;
use glia_graph::MergedGraph;

const API: &str = "from flask import Flask\nfrom kafka import KafkaProducer\n\napp = Flask(__name__)\nproducer = KafkaProducer()\n\n\ndef audit(order):\n    return order\n\n\ndef publish(order):\n    producer.send('orders', order)\n\n\ndef save(order):\n    return audit(order)\n\n\n@app.route('/orders', methods=['POST'])\ndef create_order():\n    order = {}\n    save(order)\n    publish(order)\n    return order\n";
const WEB: &str = "export async function placeOrder(body: unknown) {\n  return fetch('/orders', { method: 'POST', body: JSON.stringify(body) });\n}\n";
const WORKER: &str = "from kafka import KafkaConsumer\n\nconsumer = KafkaConsumer('orders')\n\n\ndef ship(msg):\n    return msg\n\n\ndef handle():\n    for msg in consumer:\n        ship(msg)\n";

/// web -> api over HTTP, api -> worker over the `orders` topic, built as one
/// merge of three repos.
fn orders_stack() -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (dir, file, src) in [
        ("api", "app.py", API),
        ("web", "checkout.ts", WEB),
        ("worker", "consume.py", WORKER),
    ] {
        let d = tmp.path().join(dir);
        std::fs::create_dir_all(&d).expect("repo dir");
        std::fs::write(d.join(file), src).expect("write source");
    }
    let paths: Vec<String> = ["web", "api", "worker"]
        .iter()
        .map(|d| tmp.path().join(d).to_string_lossy().into_owned())
        .collect();
    let merged = generate_many(&paths).expect("generate_many").merged;
    (tmp, merged)
}

/// The ROUTE serving `path` under `method`, found by kind and path whatever
/// owner segment (LB.4a) or qname shape the build gave it.
fn route_id(m: &MergedGraph, method: &str, path: &str) -> NodeId {
    let want = format!("{method} {path}");
    let mut hits: Vec<NodeId> = Vec::new();
    for g in &m.graphs {
        for n in &g.nodes {
            let is_route = g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ROUTE);
            let q = g
                .nav
                .qname_by_id
                .get(&n.id)
                .map(String::as_str)
                .unwrap_or("");
            if is_route && split_owner(q).0 == want && !hits.contains(&n.id) {
                hits.push(n.id);
            }
        }
    }
    assert_eq!(hits.len(), 1, "one `{want}` ROUTE in the build");
    hits[0]
}

fn qname_of(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .expect("the id is a node of the build")
}

/// (1) `POST /orders` is served by exactly the flask route, matched on the
/// matcher's exact tier, handled by `create_order`, located and live.
#[test]
fn a_served_route_names_its_tier_and_handler() {
    let (_tmp, m) = orders_stack();
    let route = route_id(&m, "POST", "/orders");
    let a = serves(&m, "POST /orders", "auto").expect("http is a mechanism");
    assert!(a.absence.is_none(), "{:?}", a.absence);
    assert_eq!(a.results.len(), 1, "{:?}", a.results);
    let s = &a.results[0];
    assert_eq!(s.id, route.0);
    assert_eq!(s.qname, qname_of(&m, route));
    assert_eq!(
        (s.kind, s.r#match, s.confidence),
        ("ROUTE", "exact", "strong")
    );
    let handlers: Vec<&str> = s.handlers.iter().map(|h| h.qname.as_str()).collect();
    assert_eq!(handlers, ["app::create_order"]);
    assert_eq!(s.handlers[0].file.as_deref(), Some("app.py"));
    assert_eq!(
        s.handlers[0].line,
        Some(21),
        "1-based line of `@app.route`/`def create_order`"
    );
    assert_eq!(
        s.file.as_deref(),
        Some("app.py"),
        "a route is located at its handler or its own site"
    );
    assert!(s.live, "a ROUTE is an entrypoint");
}

/// (2) No route takes DELETE: an `unserved_channel` FACT whose suggestions
/// name the POST route (the same path under another verb) and whose caveats
/// carry the universal `*` HTTP_CALLS row.
#[test]
fn an_unserved_verb_suggests_the_route_under_another_verb() {
    let (_tmp, m) = orders_stack();
    let post = qname_of(&m, route_id(&m, "POST", "/orders"));
    let a = serves(&m, "DELETE /orders", "auto").expect("http is a mechanism");
    assert!(a.results.is_empty(), "{:?}", a.results);
    let ab = a.absence.expect("an empty answer carries its absence");
    assert_eq!((ab.tier, ab.reason), ("FACT", "unserved_channel"));
    assert_eq!(ab.query, "DELETE /orders");
    assert_eq!(ab.mechanisms, ["HTTP_CALLS", "HANDLED_BY"]);
    assert!(ab.suggestions.contains(&post), "{:?}", ab.suggestions);
    assert!(
        ab.caveats
            .iter()
            .any(|c| c.language == "*" && c.edge_category == "HTTP_CALLS"),
        "{:?}",
        ab.caveats
    );
    assert!(ab.note.contains("DELETE /orders"), "{}", ab.note);
}

/// (3) A bare path means every verb: exactly the POST route, on the exact tier.
#[test]
fn a_bare_path_is_looked_up_under_every_verb() {
    let (_tmp, m) = orders_stack();
    let route = route_id(&m, "POST", "/orders");
    let a = serves(&m, "/orders", "auto").expect("http is a mechanism");
    let got: Vec<(u64, &str)> = a.results.iter().map(|s| (s.id, s.r#match)).collect();
    assert_eq!(got, [(route.0, "exact")]);
}

/// (4) A topic finds its consumer. The module-level `KafkaConsumer` has no
/// handler the extractor can bind, so `handlers` is honestly empty.
#[test]
fn a_topic_is_served_by_its_consumer() {
    let (_tmp, m) = orders_stack();
    let a = serves(&m, "orders", "queue").expect("queue is a mechanism");
    assert!(a.absence.is_none(), "{:?}", a.absence);
    let got: Vec<(&str, &str, &str)> = a
        .results
        .iter()
        .map(|s| (split_owner(&s.qname).0, s.kind, s.r#match))
        .collect();
    assert_eq!(got, [("queue_consumer:orders", "QUEUE_CONSUMER", "exact")]);
    // `auto` reads a channel with no verb and no leading `/` as a topic.
    let auto = serves(&m, "orders", "auto").expect("auto");
    assert_eq!(auto.results.len(), 1);
    assert_eq!(auto.results[0].id, a.results[0].id);
}

/// (5) Nobody consumes `payments`: `unserved_channel` with exactly the three
/// universal QUEUE_FLOWS caveat rows (the build holds no language with its own
/// QUEUE_FLOWS row).
#[test]
fn an_unconsumed_topic_carries_the_queue_caveats() {
    let (_tmp, m) = orders_stack();
    let a = serves(&m, "payments", "queue").expect("queue is a mechanism");
    assert!(a.results.is_empty());
    let ab = a.absence.expect("absence");
    assert_eq!(ab.reason, "unserved_channel");
    assert_eq!(ab.mechanisms, ["QUEUE_FLOWS"]);
    let rows: Vec<(&str, &str)> = ab
        .caveats
        .iter()
        .map(|c| (c.language, c.edge_category))
        .collect();
    assert_eq!(rows, [("*", "QUEUE_FLOWS"); 3], "{:?}", ab.caveats);
    assert!(
        ab.note
            .contains("wildcard subscribers are not matched by serves"),
        "{}",
        ab.note
    );
}

/// A produced-but-unconsumed topic's absence suggests its producer; a topic
/// that differs from a consumed one only by case / separator suggests it.
#[test]
fn queue_near_misses_are_producers_then_look_alike_topics() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let d = tmp.path().join("svc");
    std::fs::create_dir_all(&d).expect("repo dir");
    std::fs::write(
        d.join("bus.py"),
        "from kafka import KafkaConsumer, KafkaProducer\n\nproducer = KafkaProducer()\n\
         consumer = KafkaConsumer('Orders_Created')\n\n\n\
         def emit(x):\n    producer.send('payments', x)\n",
    )
    .expect("write");
    let m = generate_many(&[d.to_string_lossy().into_owned()])
        .expect("build")
        .merged;

    let pay = serves(&m, "payments", "queue").expect("queue");
    let ab = pay.absence.expect("nobody consumes payments");
    let sugg: Vec<&str> = ab.suggestions.iter().map(|s| split_owner(s).0).collect();
    assert_eq!(sugg, ["queue_producer:payments"], "{:?}", ab.suggestions);

    let look = serves(&m, "orders.created", "queue").expect("queue");
    let ab = look
        .absence
        .expect("case and separators differ, so nothing serves it");
    let sugg: Vec<&str> = ab.suggestions.iter().map(|s| split_owner(s).0).collect();
    assert_eq!(
        sugg,
        ["queue_consumer:Orders_Created"],
        "{:?}",
        ab.suggestions
    );
}

/// A framework tag is not a topic, so it is refused rather than matched
/// against the tag-only consumer nodes an unresolved topic leaves.
#[test]
fn a_framework_tag_is_refused() {
    let (_tmp, m) = orders_stack();
    let a = serves(&m, "unresolved:kafka", "queue").expect("queue");
    assert!(a.results.is_empty());
    let ab = a.absence.expect("refused");
    assert!(
        ab.note.contains("a framework tag is not a topic"),
        "{}",
        ab.note
    );
}

/// (6) An unknown mechanism is the one error.
#[test]
fn an_unknown_mechanism_is_an_error() {
    let (_tmp, m) = orders_stack();
    let err = serves(&m, "POST /orders", "smtp").expect_err("smtp is not a mechanism");
    assert!(
        err.contains("smtp") && err.contains("http") && err.contains("queue"),
        "{err}"
    );
}

/// The parent path's routes are near misses too: `/orders/{id}` is unserved,
/// and `/orders` is the path an agent checks next.
#[test]
fn an_unserved_subpath_suggests_the_parent_route() {
    let (_tmp, m) = orders_stack();
    let post = qname_of(&m, route_id(&m, "POST", "/orders"));
    let a = serves(&m, "GET /orders/{id}", "http").expect("http");
    let ab = a.absence.expect("no route serves the sub-path");
    assert_eq!(ab.suggestions, [post]);
}
