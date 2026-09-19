//! LD.4a — `glia trace`, driving the real binary over the packet's
//! three-service build (`web/checkout.ts` POSTs `/orders` to `api/app.py`,
//! whose `create_order` bills `billing/app.py` over POST `/charge`), merged
//! with `--with`.
//!
//! The `[trace] seed=` stderr line is the LD.4a fired_on marker; asserting it
//! here makes it a tested contract, and relaying it lets
//! `cargo test -p glia-cli --test trace_cli -- --nocapture 2>&1 | grep '^\[trace\] seed='`
//! show it.
//!
//! LD.4b: `glia flows` over the same build (its `[flows] entries=` marker is
//! relayed the same way), and the entry-flow tier that seeds `glia trace`
//! with a route when the feature word names a dead end.

use std::path::PathBuf;
use std::process::{Command, Output};

const CHECKOUT_TS: &str = "export async function placeOrder(body: unknown) {\n  return fetch('/orders', { method: 'POST', body: JSON.stringify(body) });\n}\n";
const API_PY: &str = "import requests\nfrom kafka import KafkaProducer\nfrom flask import Flask\n\napp = Flask(__name__)\nproducer = KafkaProducer()\n\n\ndef audit(order):\n    return order\n\n\ndef bill(order):\n    return requests.post('/charge', json=order)\n\n\ndef enqueue(order):\n    producer.send('orders', order)\n\n\ndef save(order):\n    return audit(order)\n\n\n@app.route('/orders', methods=['POST'])\ndef create_order():\n    order = {}\n    save(order)\n    bill(order)\n    enqueue(order)\n    return order\n";
const BILLING_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route('/charge', methods=['POST'])\ndef charge():\n    return {}\n";

/// A per-test directory under the system temp dir, removed on drop (the cli
/// crate has no `tempfile` dev-dependency).
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-trace-cli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, text) in [
            ("web/checkout.ts", CHECKOUT_TS),
            ("api/app.py", API_PY),
            ("billing/app.py", BILLING_PY),
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
            std::fs::write(p, text).expect("write fixture file");
        }
        Fixture(root)
    }

    fn repo(&self, sub: &str) -> String {
        self.0.join(sub).to_string_lossy().into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `glia <args>` with persistence off; assert exit 0 and relay the
/// `[trace]` marker lines.
fn glia(args: &[&str]) -> (Output, Vec<String>) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "glia {args:?} exited {:?}\n{stderr}", out.status);
    let markers: Vec<String> = stderr
        .lines()
        .filter(|l| l.starts_with("[trace] seed="))
        .map(str::to_string)
        .collect();
    for m in &markers {
        eprintln!("{m}");
    }
    (out, markers)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn trace_json_answers_ranked_paths() {
    let fx = Fixture::new("json");
    let (web, api, billing) = (fx.repo("web"), fx.repo("api"), fx.repo("billing"));
    let args = [
        "trace", &web, "checkout::placeOrder", "--with", &api, "--with", &billing, "--depth", "8",
        "--json",
    ];
    let (out, markers) = glia(&args);
    assert_eq!(
        markers,
        ["[trace] seed=checkout::placeOrder resolved_by=qname paths=3 expanded=12 truncated=false"]
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let paths = v["paths"].as_array().expect("a `paths` array");
    assert_eq!(paths.len(), 3, "{v}");
    assert_eq!(paths[0]["rank"], 1);
    assert_eq!(paths[0]["length"], 7);
    assert_eq!(paths[0]["cross_service_hops"], 2);
    assert_eq!(paths[0]["mechanisms"], serde_json::json!(["CALLS", "HTTP_CALLS", "HANDLED_BY"]));
    assert_eq!(v["hops"].as_array().map(Vec::len), Some(11), "the BFS tree rides along: {v}");
    assert_eq!(v["resolved_by"], "qname");
    assert_eq!(v["truncated"], false);
    assert!(v["absence"].is_null(), "{v}");

    let (out, _) = glia(&["trace", &web, "placeOrder", "--with", &api, "--with", &billing, "--max-paths", "1", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(v["paths"].as_array().map(Vec::len), Some(1), "{v}");
}

#[test]
fn trace_table_prints_one_block_per_path() {
    let fx = Fixture::new("table");
    let (web, api, billing) = (fx.repo("web"), fx.repo("api"), fx.repo("billing"));
    let (out, _) = glia(&["trace", &web, "placeOrder", "--with", &api, "--with", &billing, "--depth", "8"]);
    let text = stdout(&out);
    assert!(text.contains("### path 1 - 7 hops, 2 cross-service (CALLS, HTTP_CALLS, HANDLED_BY)"), "{text}");
    assert!(text.contains("### path 3 - 5 hops, 1 cross-service (CALLS, HTTP_CALLS, HANDLED_BY)"), "{text}");
    assert_eq!(text.matches("### path ").count(), 3, "{text}");
    assert!(
        text.contains("| 7 | HANDLED_BY |  | `POST /charge` | `app::charge` (FUNCTION) | ● | app.py:7 |"),
        "{text}"
    );

    // Two-node mode: the directed path, then the undirected fallback.
    let (out, markers) = glia(&[
        "trace", &web, "placeOrder", "--with", &api, "--with", &billing, "--to", "app::audit",
    ]);
    let text = stdout(&out);
    assert!(text.contains("# glia trace `placeOrder` → `app::audit` (depth ≤ 6)"), "{text}");
    assert!(text.contains("### path 1 - 5 hops, 1 cross-service"), "{text}");
    assert!(markers[0].ends_with(" to=app::audit directed=true"), "{markers:?}");

    let (out, _) = glia(&[
        "trace", &api, "app::audit", "--with", &web, "--with", &billing, "--to", "placeOrder",
    ]);
    let text = stdout(&out);
    assert!(text.contains("_undirected: "), "{text}");
}

#[test]
fn an_unknown_feature_is_an_absence_with_exit_0() {
    let fx = Fixture::new("absent");
    let api = fx.repo("api");
    let (out, markers) = glia(&["trace", &api, "no::such::thing"]);
    let text = stdout(&out);
    assert!(text.contains("_(nothing resolved)_"), "{text}");
    assert!(text.contains("> FACT: no node has the qname or name `no::such::thing`"), "{text}");
    assert_eq!(markers, ["[trace] seed=no::such::thing resolved_by=none paths=0 expanded=0 truncated=false"]);

    // A dead end no entry key names resolves and says why it is empty.
    let (out, _) = glia(&["trace", &api, "queue_producer:orders", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(v["resolved_by"], "qname");
    assert_eq!(v["seed"]["kind"], "QUEUE_PRODUCER");
    assert_eq!(v["absence"]["reason"], "no_edges", "{v}");
}

#[test]
fn a_dead_end_feature_word_traces_its_entry_flow() {
    let fx = Fixture::new("entry");
    let api = fx.repo("api");
    // LD.4b: `orders` names the QUEUE_PRODUCER no carry edge leaves (through
    // LD.4a: `_(no outward flow)_`); the entry flow keyed `post_/orders`
    // contains the word, so the route seeds the trace.
    let (out, markers) = glia(&["trace", &api, "orders"]);
    assert_eq!(markers, ["[trace] seed=orders resolved_by=entry_flow paths=3 expanded=8 truncated=false"]);
    let text = stdout(&out);
    assert!(!text.contains("_(no outward flow)_"), "{text}");
    assert!(text.contains("| 1 | HANDLED_BY |  | `POST /orders` | `app::create_order` (FUNCTION) |"), "{text}");
    let (out, _) = glia(&["trace", &api, "orders", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(v["resolved_by"], "entry_flow");
    assert_eq!(v["seed"]["kind"], "ROUTE");
    assert_eq!(v["seed"]["qname"], "POST /orders");
    assert!(v["absence"].is_null(), "{v}");
}

/// Run `glia flows <args>` like [`glia`], relaying the `[flows]` marker.
fn flows(args: &[&str]) -> (Output, Vec<String>) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .arg("flows")
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "glia flows {args:?} exited {:?}\n{stderr}", out.status);
    let markers: Vec<String> = stderr
        .lines()
        .filter(|l| l.starts_with("[flows] entries="))
        .map(str::to_string)
        .collect();
    for m in &markers {
        eprintln!("{m}");
    }
    (out, markers)
}

#[test]
fn flows_json_rows_equal_the_engine_answer() {
    let fx = Fixture::new("flows-json");
    let (api, billing) = (fx.repo("api"), fx.repo("billing"));
    let (out, markers) = flows(&[&api, "--with", &billing, "--json"]);
    assert_eq!(markers, ["[flows] entries=2 flows=2 cross_service=1 depth<=6"]);
    let cli: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");

    let r = repo_graph_engine::generate_many(&[api.clone(), billing.clone()]).expect("generate_many");
    let engine = repo_graph_engine::trace::entry_flows(&r.merged, &r.repo_labels, 6);
    let engine = serde_json::to_value(&engine).expect("flows serialise");
    assert_eq!(cli, engine);

    let rows = cli.as_array().expect("a list of flows");
    let keys: Vec<&str> = rows.iter().filter_map(|f| f["key"].as_str()).collect();
    assert_eq!(keys, ["post_/charge", "post_/orders"]);
    assert_eq!(rows[1]["services"], serde_json::json!(["api", "billing"]));
    assert_eq!(rows[1]["cross_service"], true);
}

#[test]
fn flows_table_prints_one_row_per_flow() {
    let fx = Fixture::new("flows-table");
    let (api, billing) = (fx.repo("api"), fx.repo("billing"));
    let (out, _) = flows(&[&api, "--with", &billing]);
    let text = stdout(&out);
    assert!(text.contains(&format!("# glia flows `{api}` (depth ≤ 6)")), "{text}");
    assert!(text.contains("| key | kind | entry | reach | xsvc | mechanisms | location |"), "{text}");
    assert!(
        text.contains("| `post_/charge` | ROUTE | `POST /charge` | 1 |  | HANDLED_BY | app.py:"),
        "{text}"
    );
    assert!(
        text.contains("| `post_/orders` | ROUTE | `POST /orders` | 9 | ✔ | HANDLED_BY, CALLS, USES, HTTP_CALLS | app.py:"),
        "{text}"
    );

    // Nothing reached within 0 hops: no rows, still exit 0.
    let (out, markers) = flows(&[&api, "--depth", "0"]);
    assert!(stdout(&out).contains("_(no entry point reaches anything within 0 hops)_"));
    assert_eq!(markers, ["[flows] entries=1 flows=0 cross_service=0 depth<=0"]);
}
