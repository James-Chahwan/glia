//! LE.4d — `glia effects`, driving the real binary over a TypeScript service
//! whose `placeOrder` writes a table, sends to a queue and calls an HTTP
//! endpoint through three helpers, and over a two-service tree where the send
//! is consumed by billing, which writes its own table.
//!
//! The `[effects]` stderr line is the LE.4d fired_on marker; asserting it here
//! makes its counts a tested contract, and relaying it lets
//! `cargo test -p glia-cli --test effects_cli -- --nocapture 2>&1 | grep '^\[effects\]'`
//! show it.

use std::path::PathBuf;
use std::process::{Command, Output};

const ORDERS_TS: &str = "import { Kafka } from 'kafkajs';\nimport axios from 'axios';\nimport { Pool } from 'pg';\nconst pool = new Pool();\nconst kafka = new Kafka({ clientId: 'x', brokers: ['b'] });\nconst producer = kafka.producer();\n\nexport async function saveOrder(order) {\n  await pool.query('INSERT INTO orders (id) VALUES ($1)', [order.id]);\n  return order;\n}\n\nexport async function publishOrder(order) {\n  await producer.send({ topic: 'orders', messages: [{ value: JSON.stringify(order) }] });\n}\n\nexport async function notify(order) {\n  const url = process.env.NOTIFY_URL;\n  await axios.post('/api/notify', order);\n}\n\nexport async function placeOrder(order) {\n  await saveOrder(order);\n  await publishOrder(order);\n  await notify(order);\n}\n";
const ORDERS_APP_TS: &str = "import { Kafka } from 'kafkajs';\nconst kafka = new Kafka({ clientId: 'orders', brokers: ['b'] });\nconst producer = kafka.producer();\n\nexport async function placeOrder(o) {\n  await producer.send({ topic: 'orders.placed', messages: [] });\n}\n";
const BILLING_APP_TS: &str = "import { Kafka } from 'kafkajs';\nimport { Pool } from 'pg';\nconst pool = new Pool();\nconst kafka = new Kafka({ clientId: 'billing', brokers: ['b'] });\nconst consumer = kafka.consumer({ groupId: 'billing' });\n\nexport async function recordInvoice(o) {\n  await pool.query('INSERT INTO invoices (id) VALUES ($1)', [o.id]);\n}\n\nexport async function start() {\n  await consumer.subscribe({ topic: 'orders.placed' });\n  await consumer.run({ eachMessage: async (m) => { await recordInvoice(m); } });\n}\n";

/// A per-test directory under the system temp dir, removed on drop (the cli
/// crate has no `tempfile` dev-dependency).
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str, files: &[(&str, &str)]) -> Self {
        let root =
            std::env::temp_dir().join(format!("glia-effects-cli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, text) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
            std::fs::write(p, text).expect("write fixture file");
        }
        Fixture(root)
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `glia <args>` with persistence off; relay the `[effects]` marker lines.
fn glia(args: &[&str]) -> (Output, Vec<String>) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let markers: Vec<String> = stderr
        .lines()
        .filter(|l| l.starts_with("[effects]"))
        .map(str::to_string)
        .collect();
    for m in &markers {
        eprintln!("{m}");
    }
    (out, markers)
}

#[test]
fn place_order_table_and_marker() {
    let fx = Fixture::new("place", &[("svc/orders.ts", ORDERS_TS)]);
    let (out, markers) = glia(&["effects", &fx.path(), "svc::orders::placeOrder"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        markers,
        [
            "[effects] seeds=1 reached=7 effects=3 (db=1 queue_produce=1 http_call=1 event_emit=0 other=0) writes=3 config_seeds=0"
        ]
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("| class | mode | sink | location | depth | via | downstream |"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "| db | write | `data_entity:sql:orders` | — | 2 | `placeOrder` -[CALLS]-> `saveOrder` -[ACCESSES_DATA]-> `data_entity:sql:orders` | — |"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("| queue_produce | — | `queue_producer:orders` | svc/orders.ts:14 | 2 |"),
        "{stdout}"
    );
    assert!(
        stdout.contains("| http_call | — | `endpoint:POST:/api/notify` | svc/orders.ts:19 | 2 |"),
        "{stdout}"
    );

    // --json is the engine's answer; --writes-only and --class filter it.
    let (out, _) = glia(&[
        "effects",
        &fx.path(),
        "svc::orders::placeOrder",
        "--class",
        "db,http_call",
        "--json",
    ]);
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let classes: Vec<&str> = v["effects"]
        .as_array()
        .expect("effects")
        .iter()
        .filter_map(|r| r["class"].as_str())
        .collect();
    assert_eq!(classes, ["db", "http_call"], "{v}");
    assert_eq!(v["effects"][0]["mode"], "write", "{v}");
    assert_eq!(v["effects"][0]["path"][0]["site_line"], 23, "{v}");
    assert!(v["absence"].is_null(), "{v}");

    // A config key seeds from its reader.
    let (out, markers) = glia(&["effects", &fx.path(), "config:env:NOTIFY_URL", "--json"]);
    assert!(out.status.success(), "{out:?}");
    assert!(markers[0].ends_with("config_seeds=1"), "{markers:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(
        v["effects"][0]["via_config"], "config:env:NOTIFY_URL",
        "{v}"
    );
    assert_eq!(v["effects"][0]["seed"], "svc::orders::notify", "{v}");
}

#[test]
fn cross_service_column_and_errors() {
    let fx = Fixture::new(
        "cross",
        &[
            ("orders/app.ts", ORDERS_APP_TS),
            ("billing/app.ts", BILLING_APP_TS),
        ],
    );
    let (out, markers) = glia(&[
        "effects",
        &fx.path(),
        "orders::app::placeOrder",
        "--cross-service",
    ]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        markers[0].contains("effects=2 (db=1 queue_produce=1"),
        "{markers:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("| class | mode | sink | location | depth | crossed | via | downstream |"),
        "{stdout}"
    );
    assert!(
        stdout.contains("| db | write | `data_entity:sql:invoices` | — | 4 | 1 |"),
        "{stdout}"
    );

    // Unknown node: an answer with its absence, exit 0.
    let (out, _) = glia(&["effects", &fx.path(), "orders::app::nope"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("> FACT: no node has the qname or name `orders::app::nope` in this graph"),
        "{stdout}"
    );

    // An unknown class is a usage error.
    let (out, _) = glia(&[
        "effects",
        &fx.path(),
        "orders::app::placeOrder",
        "--class",
        "disk",
    ]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error: unknown effect class `disk`; valid: db,"),
        "{stderr}"
    );
}
