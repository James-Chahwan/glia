//! LF.2c — `gaps::gaps_report` and `gaps::overlay_delta` over the review's
//! two probes: r1 (a hand-rolled `request()` wrapper whose `fetch(path)` is an
//! `<unresolved>` sink, and a flask API nobody pairs to) and m1 (a `${…}`
//! base that fans out to two services' `GET /users`, and a gateway-prefixed
//! `/orders-svc/orders` that pairs with nothing). A key.json grades nodes,
//! edges and cells, not an answer, so the report itself is pinned here.

use std::path::{Path, PathBuf};

use repo_graph_engine::gaps::{
    AMBIGUOUS_ENDPOINT, CATEGORIES, COCHANGE_NO_EDGE, DEAD_SYMBOL, FACT, GapRow, GapsOptions,
    GapsReport, HEURISTIC, ORPHANED_CELL, ORPHANED_RULE, REDUNDANT_RULE, TAG_ONLY_QUEUE,
    UNPAIRED_ENDPOINT, UNPAIRED_ROUTE, UNRESOLVED_ENDPOINT, WRAPPED_SINK, gaps_report,
    overlay_delta,
};
use repo_graph_engine::{GenerateResult, generate_many, generate_one};

const CLIENT_TS: &str = "export function request(method: string, path: string) {\n  return fetch(path, { method });\n}\n\nexport async function loadUsers() {\n  return request('GET', '/users');\n}\n";
const API_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\", methods=[\"GET\"])\ndef list_users():\n    return []\n\n\n@app.route(\"/orders\", methods=[\"GET\"])\ndef list_orders():\n    return []\n";
const M1_WEB_TS: &str = "import axios from 'axios';\n\nconst GATEWAY = process.env.GATEWAY_URL;\n\nexport async function listUsers() {\n  return axios.get(`${GATEWAY}/users`);\n}\n\nexport async function listOrders() {\n  return axios.get('/orders-svc/orders');\n}\n";
const M1_ORDERS_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\", methods=[\"GET\"])\ndef orders_users():\n    return []\n\n\n@app.route(\"/orders\", methods=[\"GET\"])\ndef orders_list():\n    return []\n";
const M1_BILLING_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\", methods=[\"GET\"])\ndef billing_users():\n    return []\n";
/// The HTTP_CALLS stanza that pairs r1's `<unresolved>` sink by hand.
const PAIR_STANZA: &str = "version = 1\n\n[[edge]]\nfrom = \"endpoint:GET:<unresolved>\"\nto = \"GET /users\"\ncategory = \"HTTP_CALLS\"\n";

/// A fresh, empty temp dir for one test.
fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("glia_lf2c_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

fn s(p: &Path) -> String {
    p.to_str().expect("utf-8 temp path").to_string()
}

/// Probe r1 as ONE root holding `web/` and `api/`.
fn r1_single(tag: &str) -> PathBuf {
    let root = tmp(tag);
    write(&root, "web/src/client.ts", CLIENT_TS);
    write(&root, "api/app.py", API_PY);
    root
}

/// Probe r1 as two repos, `web` and `api`, under one parent.
fn r1_pair(tag: &str) -> (PathBuf, PathBuf) {
    let root = tmp(tag);
    write(&root, "web/src/client.ts", CLIENT_TS);
    write(&root, "api/app.py", API_PY);
    (root.join("web"), root.join("api"))
}

fn roots_of(r: &GenerateResult) -> Vec<(u64, PathBuf)> {
    r.repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect()
}

fn report(r: &GenerateResult) -> GapsReport {
    gaps_report(&r.merged, &roots_of(r), &GapsOptions::default()).expect("known categories")
}

fn rows<'a>(rep: &'a GapsReport, category: &str) -> Vec<&'a GapRow> {
    rep.rows.iter().filter(|r| r.category == category).collect()
}

/// `(category, count)` in report order, for an exact-counts assertion.
fn counts(rep: &GapsReport) -> Vec<(&'static str, usize)> {
    CATEGORIES.iter().map(|c| (*c, rep.count(c))).collect()
}

#[test]
fn reports_the_probe_r1_shapes() {
    let root = r1_single("r1");
    let r = generate_one(&s(&root)).expect("build");
    let rep = report(&r);
    std::fs::remove_dir_all(&root).ok();

    // loadUsers is exported but nothing in the build calls it: the one dead head.
    assert_eq!(
        counts(&rep),
        [
            (UNPAIRED_ENDPOINT, 0),
            (AMBIGUOUS_ENDPOINT, 0),
            (UNRESOLVED_ENDPOINT, 1),
            (WRAPPED_SINK, 0),
            (UNPAIRED_ROUTE, 2),
            (TAG_ONLY_QUEUE, 0),
            (DEAD_SYMBOL, 1),
            (COCHANGE_NO_EDGE, 0),
            (ORPHANED_RULE, 0),
            (REDUNDANT_RULE, 0),
            (ORPHANED_CELL, 0),
        ],
        "{rep:#?}"
    );
    assert!(
        rep.skipped.is_empty(),
        "a root was given: {:?}",
        rep.skipped
    );

    let unresolved = rows(&rep, UNRESOLVED_ENDPOINT);
    let u = unresolved[0];
    assert_eq!(u.qname, "endpoint:GET:<unresolved>");
    assert_eq!(u.detail, "owner=web::src::client::request");
    assert_eq!((u.suggest, u.tier, u.kind), ("wrapper", FACT, "ENDPOINT"));
    assert_eq!(
        (u.file.as_deref(), u.line),
        (Some("web/src/client.ts"), Some(2)),
        "the fetch line, 1-based"
    );

    let routes: Vec<(&str, Option<i64>)> = rows(&rep, UNPAIRED_ROUTE)
        .iter()
        .map(|r| (r.qname.as_str(), r.line))
        .collect();
    assert_eq!(
        routes,
        [("GET /users", Some(7)), ("GET /orders", Some(12))],
        "sorted by file, line"
    );
    let users = rows(&rep, UNPAIRED_ROUTE)[0];
    assert_eq!(
        (users.suggest, users.tier),
        ("route_prefix|edge", HEURISTIC)
    );
    assert!(
        users.detail.ends_with("handler=api::app::list_users"),
        "{}",
        users.detail
    );

    let dead = rows(&rep, DEAD_SYMBOL);
    assert_eq!(dead[0].qname, "web::src::client::loadUsers");
    assert_eq!((dead[0].suggest, dead[0].tier), ("entrypoints", HEURISTIC));
}

#[test]
fn reports_the_probe_m1_fanout() {
    let root = tmp("m1");
    write(&root, "web/src/api.ts", M1_WEB_TS);
    write(&root, "orders/app.py", M1_ORDERS_PY);
    write(&root, "billing/app.py", M1_BILLING_PY);
    let paths = ["web", "orders", "billing"].map(|d| s(&root.join(d)));
    let r = generate_many(&paths).expect("build");
    let rep = report(&r);
    std::fs::remove_dir_all(&root).ok();

    let amb = rows(&rep, AMBIGUOUS_ENDPOINT);
    assert_eq!(amb.len(), 1, "{rep:#?}");
    assert_eq!(amb[0].qname, "endpoint:GET:${…}/users");
    assert_eq!(
        amb[0].detail,
        "targets=2 repos=2 projects=0: GET /users (billing), GET /users (orders)"
    );
    assert_eq!(
        (amb[0].suggest, amb[0].tier),
        ("constants|route_prefix", FACT)
    );

    let unpaired = rows(&rep, UNPAIRED_ENDPOINT);
    assert_eq!(unpaired.len(), 1, "{rep:#?}");
    let u = unpaired[0];
    assert_eq!(u.qname, "endpoint:GET:/orders-svc/orders");
    assert_eq!(
        u.suggest, "route_prefix|edge",
        "orders' GET /orders is a suffix of the path"
    );
    assert_eq!(
        u.tier, FACT,
        "no host recorded: nothing says it is third-party"
    );
    assert_eq!(
        u.detail,
        "no HTTP_CALLS target; caller=src::api::listOrders; suffix_of=GET /orders (orders)"
    );
    // Only orders' GET /orders is left uncalled: both GET /users are paired.
    let routes: Vec<&str> = rows(&rep, UNPAIRED_ROUTE)
        .iter()
        .map(|r| r.qname.as_str())
        .collect();
    assert_eq!(routes, ["GET /orders"]);
}

#[test]
fn overlay_delta_counts_orphans() {
    let (web, api) = r1_pair("delta");
    write(&web, ".glia/overlay.toml", PAIR_STANZA);
    let d = overlay_delta(&[s(&web), s(&api)], false).expect("both builds");
    std::fs::remove_dir_all(web.parent().expect("parent")).ok();

    assert_eq!(d.rules, 1);
    assert_eq!((d.orphans_without, d.orphans_with), (1, 0), "{d:?}");
    assert_eq!(d.added_by_category.get("HTTP_CALLS"), Some(&1), "{d:?}");
    assert_eq!(
        d.added_by_category.len(),
        1,
        "only the stanza's edge moved: {d:?}"
    );
    assert_eq!(d.edges_with, d.edges_without + 1, "{d:?}");
}

#[test]
fn orphaned_rule_is_reported() {
    let (web, api) = r1_pair("orphan");
    write(
        &web,
        ".glia/overlay.toml",
        "version = 1\n\n[[edge]]\nfrom = \"src::client::gone\"\nto = \"GET /users\"\ncategory = \"CALLS\"\n",
    );
    let r = generate_many(&[s(&web), s(&api)]).expect("build");
    let rep = report(&r);
    std::fs::remove_dir_all(web.parent().expect("parent")).ok();

    let orphaned = rows(&rep, ORPHANED_RULE);
    assert_eq!(orphaned.len(), 1, "{rep:#?}");
    let o = orphaned[0];
    assert_eq!((o.qname.as_str(), o.kind), ("src::client::gone", "edge"));
    assert_eq!(
        (o.file.as_deref(), o.line),
        (Some(".glia/overlay.toml"), Some(3))
    );
    assert_eq!(
        o.detail,
        "repo=web edge#1 from=src::client::gone to=GET /users category=CALLS (no node: from)"
    );
    assert_eq!((o.suggest, o.tier), ("remove", FACT));
    assert_eq!(rep.count(REDUNDANT_RULE), 0);
}

/// A stanza the extractor already covers is redundant; stale anchors (one
/// naming a qname only ANOTHER repo has), an entrypoint prefix nothing sits
/// under and a sidecar row naming a gone symbol are orphaned, each at its own
/// line.
#[test]
fn redundant_rule_stale_anchors_and_orphaned_cell() {
    let (web, api) = r1_pair("rot");
    write(
        &web,
        ".glia/overlay.toml",
        "version = 1\n\n[[edge]]\nfrom = \"src::client::loadUsers\"\nto = \"src::client::request\"\ncategory = \"CALLS\"\n\n\
         [[note]]\nanchor = \"src::client::nothing\"\ntext = \"stale\"\n\n\
         [entrypoints]\nqnames = [\"src::client::loadUsers\", \"src::gone::*\"]\n\n\
         [[decision]]\nid = \"d1\"\nanchor = \"app::list_users\"\ntitle = \"flask\"\n",
    );
    write(
        &web,
        ".glia/cells.jsonl",
        "{\"cell\":\"CONV\",\"entry\":{\"id\":\"000001\",\"source\":\"api\",\"text\":\"x\"},\"qname\":\"src::client::request\"}\n\
         {\"cell\":\"CONV\",\"entry\":{\"id\":\"000001\",\"source\":\"api\",\"text\":\"y\"},\"qname\":\"src::client::removed\"}\n",
    );
    let r = generate_many(&[s(&web), s(&api)]).expect("build");
    let rep = report(&r);
    std::fs::remove_dir_all(web.parent().expect("parent")).ok();

    let redundant = rows(&rep, REDUNDANT_RULE);
    assert_eq!(redundant.len(), 1, "{rep:#?}");
    assert_eq!(redundant[0].line, Some(3));
    assert!(
        redundant[0]
            .detail
            .contains("edge#1 src::client::loadUsers -> src::client::request"),
        "{}",
        redundant[0].detail
    );

    let orphaned: Vec<(&str, &str, Option<i64>)> = rows(&rep, ORPHANED_RULE)
        .iter()
        .map(|r| (r.kind, r.qname.as_str(), r.line))
        .collect();
    assert_eq!(
        orphaned,
        [
            ("note", "src::client::nothing", Some(8)),
            ("entrypoint", "src::gone::*", Some(13)),
            // The api repo has it, but a declared anchor binds in its own repo only.
            ("decision", "app::list_users", Some(15)),
        ]
    );

    let cells = rows(&rep, ORPHANED_CELL);
    assert_eq!(cells.len(), 1, "{rep:#?}");
    let c = cells[0];
    assert_eq!((c.qname.as_str(), c.kind), ("src::client::removed", "CONV"));
    assert_eq!(
        (c.file.as_deref(), c.line),
        (Some(".glia/cells.jsonl"), Some(2))
    );
    assert_eq!(c.suggest, "glia cell ls --check --rekey");
}

#[test]
fn tag_only_queue_is_reported() {
    let root = tmp("queue");
    write(
        &root,
        "publish.ts",
        "import { Kafka } from 'kafkajs';\n\nconst kafka = new Kafka({ clientId: 'a', brokers: ['localhost:9092'] });\nconst producer = kafka.producer();\n\nexport async function publishTo(topic: string, body: string): Promise<void> {\n  await producer.send({ topic: topic, messages: [{ value: body }] });\n}\n",
    );
    let r = generate_one(&s(&root)).expect("build");
    let rep = report(&r);
    std::fs::remove_dir_all(&root).ok();

    let tags = rows(&rep, TAG_ONLY_QUEUE);
    assert_eq!(tags.len(), 1, "{rep:#?}");
    assert_eq!(tags[0].qname, "queue_producer:unresolved:kafka");
    assert_eq!(
        tags[0].detail,
        "topic=unresolved:kafka; owner=publish::publishTo"
    );
    assert_eq!((tags[0].suggest, tags[0].tier), ("constants|wrapper", FACT));
}

/// `top_k_per_category` cuts rows after ranking, `category` keeps one
/// category, and `counts` stay the totals; no root skips the four
/// root-reading categories; an unknown category is an error.
#[test]
fn options_cut_rows_not_counts() {
    let root = r1_single("opts");
    let r = generate_one(&s(&root)).expect("build");
    std::fs::remove_dir_all(&root).ok();

    let mut o = GapsOptions::default();
    o.top_k_per_category = Some(1);
    let cut = gaps_report(&r.merged, &roots_of(&r), &o).expect("report");
    assert_eq!(rows(&cut, UNPAIRED_ROUTE).len(), 1);
    assert_eq!(
        rows(&cut, UNPAIRED_ROUTE)[0].qname,
        "GET /users",
        "the first after ranking"
    );
    assert_eq!(
        cut.count(UNPAIRED_ROUTE),
        2,
        "counts are pre-truncation totals"
    );

    let mut o = GapsOptions::default();
    o.category = Some(UNPAIRED_ROUTE.to_string());
    let one = gaps_report(&r.merged, &roots_of(&r), &o).expect("report");
    assert!(
        one.rows.iter().all(|r| r.category == UNPAIRED_ROUTE) && one.rows.len() == 2,
        "{one:#?}"
    );
    assert_eq!(one.count(UNRESOLVED_ENDPOINT), 1);

    let bare = gaps_report(&r.merged, &[], &GapsOptions::default()).expect("report");
    assert_eq!(
        bare.skipped,
        [WRAPPED_SINK, ORPHANED_RULE, REDUNDANT_RULE, ORPHANED_CELL]
    );
    assert!(!bare.counts.contains_key(ORPHANED_RULE));

    let mut o = GapsOptions::default();
    o.category = Some("nope".to_string());
    let err = gaps_report(&r.merged, &[], &o).expect_err("unknown category");
    assert!(err.contains("unknown gaps category `nope`"), "{err}");
}

#[test]
fn deterministic() {
    let (web, api) = r1_pair("det");
    write(
        &web,
        ".glia/overlay.toml",
        "version = 1\n\n[[edge]]\nfrom = \"a::gone\"\nto = \"GET /users\"\ncategory = \"CALLS\"\n",
    );
    let paths = [s(&web), s(&api)];
    let a = serde_json::to_string(&report(&generate_many(&paths).expect("build"))).expect("json");
    let b = serde_json::to_string(&report(&generate_many(&paths).expect("build"))).expect("json");
    std::fs::remove_dir_all(web.parent().expect("parent")).ok();
    assert_eq!(a, b);
    assert!(a.starts_with("{\"counts\":{"), "{a}");
}
