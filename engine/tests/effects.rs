//! LE.4d — `effects`: the effect sinks downstream of named nodes, with
//! located witness paths, on a REAL build of a TypeScript service.
//!
//! `svc/orders.ts`: `placeOrder` calls `saveOrder` (an `INSERT INTO orders`),
//! `publishOrder` (a kafkajs `producer.send` to `orders`) and `notify` (reads
//! `NOTIFY_URL`, `axios.post('/api/notify')`); `loadOrder` only SELECTs. The
//! function-level edges come from LE.4a (ACCESSES_DATA with ACCESS_MODE),
//! LE.4b (READS_CONFIG) and LE.4c (USES of the queue producer). The engine
//! prints `[effects] seeds=.. reached=.. effects=..` per answer.

use std::collections::BTreeMap;
use std::path::Path;

use repo_graph_engine::effects::{EffectRow, Effects, EffectsArgs, MAX_SEEDS, effects};
use repo_graph_engine::{GenerateResult, generate_one};

const ORDERS_TS: &str = "import { Kafka } from 'kafkajs';
import axios from 'axios';
import { Pool } from 'pg';
const pool = new Pool();
const kafka = new Kafka({ clientId: 'x', brokers: ['b'] });
const producer = kafka.producer();

export async function saveOrder(order) {
  await pool.query('INSERT INTO orders (id) VALUES ($1)', [order.id]);
  return order;
}

export async function loadOrder(id) {
  const r = await pool.query('SELECT * FROM orders WHERE id = $1', [id]);
  return r.rows[0];
}

export async function publishOrder(order) {
  await producer.send({ topic: 'orders', messages: [{ value: JSON.stringify(order) }] });
}

export async function notify(order) {
  const url = process.env.NOTIFY_URL;
  await axios.post('/api/notify', order);
}

export async function placeOrder(order) {
  await saveOrder(order);
  await publishOrder(order);
  await notify(order);
}

export function label(order) {
  return 'order ' + order.id;
}
";

const ORDERS_APP_TS: &str = "import { Kafka } from 'kafkajs';
const kafka = new Kafka({ clientId: 'orders', brokers: ['b'] });
const producer = kafka.producer();

export async function placeOrder(o) {
  await producer.send({ topic: 'orders.placed', messages: [] });
}
";

const BILLING_APP_TS: &str = "import { Kafka } from 'kafkajs';
import { Pool } from 'pg';
const pool = new Pool();
const kafka = new Kafka({ clientId: 'billing', brokers: ['b'] });
const consumer = kafka.consumer({ groupId: 'billing' });

export async function recordInvoice(o) {
  await pool.query('INSERT INTO invoices (id) VALUES ($1)', [o.id]);
}

export async function start() {
  await consumer.subscribe({ topic: 'orders.placed' });
  await consumer.run({ eachMessage: async (m) => { await recordInvoice(m); } });
}
";

const PLACE: &str = "svc::orders::placeOrder";
const ORDERS: &str = "data_entity:sql:orders";
const PRODUCER: &str = "queue_producer:orders";
const NOTIFY_ENDPOINT: &str = "endpoint:POST:/api/notify";

/// Write `files` under `root`, creating parent dirs.
fn write_all(root: &Path, files: &[(&str, &str)]) {
    for (rel, text) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("fixture dir");
        std::fs::write(&p, text).expect("fixture write");
    }
}

/// A tempdir holding `files`, and its graph.
fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, GenerateResult) {
    let dir = tempfile::tempdir().expect("temp dir");
    write_all(dir.path(), files);
    let g = generate_one(dir.path().to_str().expect("utf-8 temp path")).expect("build");
    (dir, g)
}

fn orders() -> (tempfile::TempDir, GenerateResult) {
    build(&[("svc/orders.ts", ORDERS_TS)])
}

fn run(g: &GenerateResult, seeds: &[&str], args: &EffectsArgs) -> Effects {
    effects(&g.merged, &g.repo_labels, seeds, args).expect("effects answer")
}

/// `(class, qname, mode, depth)` per row, in answer order.
fn shape(a: &Effects) -> Vec<(&'static str, String, Option<String>, usize)> {
    a.effects
        .iter()
        .map(|r| (r.class, r.qname.clone(), r.mode.clone(), r.depth))
        .collect()
}

fn row<'a>(a: &'a Effects, qname: &str) -> &'a EffectRow {
    a.effects
        .iter()
        .find(|r| r.qname == qname)
        .unwrap_or_else(|| panic!("no row {qname} in {:#?}", a.effects))
}

/// `(from, category, to)` per hop.
fn hops(r: &EffectRow) -> Vec<(String, &'static str, String)> {
    r.path
        .iter()
        .map(|h| (h.from_qname.clone(), h.category, h.to_qname.clone()))
        .collect()
}

fn s(x: &str) -> String {
    x.to_string()
}

#[test]
fn place_order_effects() {
    let (_dir, g) = orders();
    let a = run(&g, &[PLACE], &EffectsArgs::default());
    assert_eq!(
        shape(&a),
        [
            ("db", s(ORDERS), Some(s("write")), 2),
            ("queue_produce", s(PRODUCER), None, 2),
            ("http_call", s(NOTIFY_ENDPOINT), None, 2),
        ],
        "{a:#?}"
    );
    assert_eq!(a.seeds, [PLACE]);
    assert!(a.absence.is_none() && a.unresolved.is_empty());

    let db = row(&a, ORDERS);
    assert_eq!(
        hops(db),
        [
            (s(PLACE), "CALLS", s("svc::orders::saveOrder")),
            (s("svc::orders::saveOrder"), "ACCESSES_DATA", s(ORDERS)),
        ]
    );
    // Each hop is located at the site that asserted it, 1-based.
    assert_eq!(
        (db.path[0].site_file.as_deref(), db.path[0].site_line),
        (Some("svc/orders.ts"), Some(28)),
        "{:#?}",
        db.path
    );
    assert_eq!(
        db.path[1].site_file.as_deref(),
        Some("svc/orders.ts"),
        "{:#?}",
        db.path
    );
    assert_eq!((db.seed.as_str(), db.via_config.as_deref()), (PLACE, None));
    assert_eq!(
        (db.tier, db.services_crossed, db.kind),
        ("derived", 0, "DATA_ENTITY")
    );

    let q = row(&a, PRODUCER);
    assert_eq!(
        hops(q),
        [
            (s(PLACE), "CALLS", s("svc::orders::publishOrder")),
            (s("svc::orders::publishOrder"), "USES", s(PRODUCER)),
        ]
    );
    assert_eq!(
        (q.file.as_deref(), q.line),
        (Some("svc/orders.ts"), Some(19))
    );
    // No consumer in the build: nothing downstream.
    assert!(q.downstream.is_empty(), "{:#?}", q.downstream);

    let h = row(&a, NOTIFY_ENDPOINT);
    assert_eq!(
        hops(h)[1],
        (s("svc::orders::notify"), "CALLS", s(NOTIFY_ENDPOINT))
    );
    assert_eq!(
        (h.file.as_deref(), h.line),
        (Some("svc/orders.ts"), Some(24))
    );

    let counts: BTreeMap<&str, usize> = a.counts.iter().map(|(k, v)| (*k, *v)).collect();
    assert_eq!(counts.get("db"), Some(&1));
    assert_eq!(counts.get("queue_produce"), Some(&1));
    assert_eq!(counts.get("http_call"), Some(&1));
    assert_eq!(
        counts.get("event_emit"),
        Some(&0),
        "every class of the table is counted"
    );
    assert_eq!(a.writes, 3, "a write, a send and a call");
}

#[test]
fn load_is_read() {
    let (_dir, g) = orders();
    let a = run(&g, &["svc::orders::loadOrder"], &EffectsArgs::default());
    assert_eq!(shape(&a), [("db", s(ORDERS), Some(s("read")), 1)], "{a:#?}");
    assert_eq!(a.writes, 0);
}

#[test]
fn read_and_write_fold_to_read_write() {
    // Two seeds reach `orders`, one reading, one writing: the mode folds over
    // every edge the walk reaches the sink by.
    let (_dir, g) = orders();
    let a = run(
        &g,
        &["svc::orders::loadOrder", "svc::orders::saveOrder"],
        &EffectsArgs::default(),
    );
    assert_eq!(
        shape(&a),
        [("db", s(ORDERS), Some(s("read_write")), 1)],
        "{a:#?}"
    );
    assert_eq!(
        a.seeds,
        ["svc::orders::loadOrder", "svc::orders::saveOrder"]
    );
    assert_eq!(a.writes, 1);
}

#[test]
fn writes_only_drops_reads() {
    let (_dir, g) = orders();
    let mut args = EffectsArgs::default();
    args.writes_only = true;
    let a = run(&g, &["svc::orders::loadOrder"], &args);
    assert!(a.effects.is_empty(), "{a:#?}");
    let absence = a.absence.expect("an emptied answer carries an absence");
    assert_eq!(absence.reason, "no_match");
    assert_eq!(absence.note, "1 effect reached, none that writes");

    // The write and both sends stay.
    let a = run(&g, &[PLACE, "svc::orders::loadOrder"], &args);
    let classes: Vec<&str> = a.effects.iter().map(|r| r.class).collect();
    assert_eq!(classes, ["db", "queue_produce", "http_call"], "{a:#?}");
    assert_eq!(row(&a, ORDERS).mode.as_deref(), Some("read_write"));
}

#[test]
fn config_seed() {
    let (_dir, g) = orders();
    let a = run(&g, &["config:env:NOTIFY_URL"], &EffectsArgs::default());
    assert_eq!(
        shape(&a),
        [("http_call", s(NOTIFY_ENDPOINT), None, 1)],
        "{a:#?}"
    );
    let r = row(&a, NOTIFY_ENDPOINT);
    assert_eq!(r.seed, "svc::orders::notify");
    assert_eq!(r.via_config.as_deref(), Some("config:env:NOTIFY_URL"));
    assert_eq!(a.seeds, ["svc::orders::notify"]);

    // Naming the reader too: it is a seed in its own right, no via_config.
    let a = run(
        &g,
        &["config:env:NOTIFY_URL", "svc::orders::notify"],
        &EffectsArgs::default(),
    );
    assert_eq!(row(&a, NOTIFY_ENDPOINT).via_config, None);
}

#[test]
fn class_filter() {
    let (_dir, g) = orders();
    let mut args = EffectsArgs::default();
    args.classes = Some(vec![s("queue_produce"), s("HTTP_CALL")]);
    let a = run(&g, &[PLACE], &args);
    let got: Vec<(&str, &str)> = a
        .effects
        .iter()
        .map(|r| (r.class, r.qname.as_str()))
        .collect();
    assert_eq!(
        got,
        [("queue_produce", PRODUCER), ("http_call", NOTIFY_ENDPOINT)]
    );
    assert_eq!(a.counts.get("db"), Some(&0));

    args.classes = Some(vec![s("email")]);
    let a = run(&g, &[PLACE], &args);
    assert!(a.effects.is_empty());
    assert_eq!(
        a.absence.expect("absence").note,
        "3 effects reached, none of class email"
    );

    args.classes = Some(vec![s("disk")]);
    let err = effects(&g.merged, &g.repo_labels, &[PLACE], &args).expect_err("unknown class");
    assert!(
        err.starts_with("unknown effect class `disk`; valid: db, email,"),
        "{err}"
    );
}

#[test]
fn scope_keeps_rows_under_it() {
    let (_dir, g) = orders();
    let mut args = EffectsArgs::default();
    args.scope = Some(s("elsewhere"));
    let a = run(&g, &[PLACE], &args);
    // The unlocatable data entity is kept (the A8.3 rule); the located sends
    // are outside the scope.
    assert_eq!(
        shape(&a),
        [("db", s(ORDERS), Some(s("write")), 2)],
        "{a:#?}"
    );

    args.classes = Some(vec![s("http_call")]);
    let a = run(&g, &[PLACE], &args);
    assert!(a.effects.is_empty());
    assert_eq!(
        a.absence.expect("absence").note,
        "1 result outside scope `elsewhere`"
    );
}

#[test]
fn cross_service() {
    let (_dir, g) = build(&[
        ("orders/app.ts", ORDERS_APP_TS),
        ("billing/app.ts", BILLING_APP_TS),
    ]);
    let seed = "orders::app::placeOrder";

    // Without it: the send only, its consumer named downstream.
    let a = run(&g, &[seed], &EffectsArgs::default());
    assert_eq!(
        shape(&a),
        [("queue_produce", s("queue_producer:orders.placed"), None, 1)],
        "{a:#?}"
    );
    let q = &a.effects[0];
    let down: Vec<(&str, &str, &str)> = q
        .downstream
        .iter()
        .map(|t| (t.qname.as_str(), t.kind, t.category))
        .collect();
    assert_eq!(
        down,
        [(
            "queue_consumer:orders.placed",
            "QUEUE_CONSUMER",
            "QUEUE_FLOWS"
        )]
    );
    assert_eq!(q.downstream[0].file.as_deref(), Some("billing/app.ts"));
    assert_eq!(q.services_crossed, 0);

    // With it: on through the consumer into billing's handler and its write.
    let mut args = EffectsArgs::default();
    args.cross_service = true;
    let a = run(&g, &[seed], &args);
    let invoices = row(&a, "data_entity:sql:invoices");
    assert_eq!(
        (
            invoices.class,
            invoices.mode.as_deref(),
            invoices.services_crossed
        ),
        ("db", Some("write"), 1),
        "{a:#?}"
    );
    assert_eq!(invoices.seed, seed);
    assert_eq!(
        hops(invoices)[..2],
        [
            (s(seed), "USES", s("queue_producer:orders.placed")),
            (
                s("queue_producer:orders.placed"),
                "QUEUE_FLOWS",
                s("queue_consumer:orders.placed")
            ),
        ]
    );
    assert_eq!(hops(invoices).last().map(|h| h.1), Some("ACCESSES_DATA"));
    let q = row(&a, "queue_producer:orders.placed");
    assert_eq!((q.depth, q.services_crossed, q.downstream.len()), (1, 0, 1));
}

#[test]
fn seed_cap_errors_at_65() {
    let (_dir, g) = orders();
    let names: Vec<String> = (0..MAX_SEEDS + 1).map(|i| format!("seed{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let err = effects(&g.merged, &g.repo_labels, &refs, &EffectsArgs::default())
        .expect_err("65 names is over the cap");
    assert_eq!(
        err,
        "effects: 65 seed names, more than the 64 one answer takes; name fewer"
    );
    // 64 is fine (none of them resolves: an unknown-symbol absence).
    let a = run(&g, &refs[..MAX_SEEDS], &EffectsArgs::default());
    assert_eq!(a.unresolved.len(), MAX_SEEDS);
    assert_eq!(a.absence.expect("absence").reason, "unknown_symbol");
    let err =
        effects(&g.merged, &g.repo_labels, &[" "], &EffectsArgs::default()).expect_err("no name");
    assert_eq!(err, "effects: no seed qname given");
}

#[test]
fn no_effect_has_absence() {
    let (_dir, g) = orders();
    let a = run(&g, &["svc::orders::label"], &EffectsArgs::default());
    assert!(a.effects.is_empty(), "{a:#?}");
    assert_eq!(a.writes, 0);
    let absence = a.absence.expect("a pure helper has no effect");
    assert_eq!((absence.tier, absence.reason), ("FACT", "no_edges"));
    assert_eq!(absence.mechanisms, ["ACCESSES_DATA", "USES", "CALLS"]);
    assert_eq!(
        absence.note,
        "no effect sink is reachable from `svc::orders::label` within 8 hops"
    );

    // A name no node has.
    let a = run(&g, &["svc::orders::nope", PLACE], &EffectsArgs::default());
    assert_eq!(a.unresolved, ["svc::orders::nope"]);
    assert_eq!(a.effects.len(), 3, "the resolved seed still answers");
    let a = run(&g, &["svc::orders::nope"], &EffectsArgs::default());
    assert_eq!(a.absence.expect("absence").reason, "unknown_symbol");
}
