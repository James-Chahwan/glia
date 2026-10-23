//! LD.5 — `glia blast-radius <REPO> <QNAME>...`: many seeds, one radius.
//!
//! Before LD.5 the command took exactly one qname: `glia blast-radius api
//! app::save app::publish` was clap's `error: unexpected argument
//! 'app::publish' found` (exit 2, measured at the wave HEAD). Now it is one
//! walk and one ranking over both seeds, each row naming its seed, and an
//! unknown name is reported instead of failing the whole answer: exit 3 only
//! when every name is unknown.
//!
//! Drives the real binary. The `[blast] seeds=` stderr line is the LD.5
//! fired_on marker; asserting it here makes it a tested contract.

use std::path::PathBuf;
use std::process::{Command, Output};

const APP_PY: &str = "from flask import Flask\nfrom kafka import KafkaProducer\n\napp = Flask(__name__)\nproducer = KafkaProducer()\n\n\ndef audit(order):\n    return order\n\n\ndef publish(order):\n    producer.send('orders', order)\n\n\ndef save(order):\n    return audit(order)\n\n\n@app.route('/orders', methods=['POST'])\ndef create_order():\n    order = {}\n    save(order)\n    publish(order)\n    return order\n";

/// A fresh `api` repo dir holding only `app.py`, removed on drop.
struct TempRepo(PathBuf);

impl TempRepo {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-ld5-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let api = root.join("api");
        std::fs::create_dir_all(&api).expect("temp dir");
        std::fs::write(api.join("app.py"), APP_PY).expect("write app.py");
        TempRepo(root)
    }

    fn api(&self) -> String {
        self.0.join("api").to_str().expect("temp path is UTF-8").to_string()
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn glia(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_glia"))
        .env("GLIA_NO_PERSIST", "1")
        .args(args)
        .output()
        .expect("glia runs")
}

/// The one `[blast] seeds=` line of a run, relayed so `-- --nocapture 2>&1 |
/// grep '\[blast\] seeds='` sees it.
fn marker(out: &Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().filter(|l| l.starts_with("[blast] seeds=")).collect();
    assert_eq!(lines.len(), 1, "one [blast] line per answer:\n{stderr}");
    eprintln!("{}", lines[0]);
    lines[0].to_string()
}

#[test]
fn several_qnames_are_one_radius() {
    let repo = TempRepo::new("json");
    let api = repo.api();
    let out = glia(&["blast-radius", &api, "app::save", "app::publish", "--json"]);
    assert!(out.status.success(), "exit {:?}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let seeds: Vec<&str> =
        v["seeds"].as_array().expect("seeds").iter().filter_map(|s| s["qname"].as_str()).collect();
    assert_eq!(seeds, ["app::save", "app::publish"], "{v}");
    assert_eq!(v["unresolved"], serde_json::json!([]), "{v}");
    assert!(v["absence"].is_null(), "{v}");
    // EA.5: the answer's keys (sorted: serde_json without preserve_order)
    // gain `files`, the measured file ranking;
    // the seeds' own file is never listed.
    let keys: Vec<&str> = v.as_object().expect("object").keys().map(String::as_str).collect();
    assert_eq!(keys, ["absence", "files", "results", "seeds", "unresolved"], "{v}");
    assert!(v["files"].as_array().expect("files").iter().all(|f| f["file"] != "app.py"), "{v}");
    let rows = v["results"].as_array().expect("results");
    let create = rows.iter().find(|r| r["qname"] == "app::create_order").expect("the shared caller");
    assert_eq!(create["seed"], "app::save", "the first seed's wave reaches it first: {v}");
    assert!(rows.iter().all(|r| r["qname"] != "app::save" && r["qname"] != "app::publish"), "{v}");
    assert!(marker(&out).starts_with("[blast] seeds=2 unresolved=0 "));

    // Backward, the marker names the walk and the whole closure.
    let out = glia(&[
        "blast-radius", &api, "app::save", "app::publish", "--direction", "backward", "--depth", "6",
        "--json",
    ]);
    assert!(out.status.success());
    assert_eq!(marker(&out), "[blast] seeds=2 unresolved=0 reached=2 linked_seeds=0 walk=Backward");
}

#[test]
fn the_table_names_each_rows_seed_and_the_unresolved_queries() {
    let repo = TempRepo::new("table");
    let api = repo.api();
    let out = glia(&["blast-radius", &api, "app::create_order", "app::save", "nope", "--direction", "forward"]);
    assert!(out.status.success(), "a mixed answer exits 0: {:?}", out.status);
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("> unresolved: nope"), "{table}");
    assert!(table.contains("> seed `app::create_order` is one carry edge from: app::save"), "{table}");
    assert!(table.contains("| seed |"), "a seed column with more than one seed:\n{table}");
    assert!(table.contains("`app::audit` | `app::save` |"), "audit is save's:\n{table}");
    assert_eq!(marker(&out), "[blast] seeds=2 unresolved=1 reached=3 linked_seeds=1 walk=Forward");

    // One seed: no seed column, as before LD.5.
    let out = glia(&["blast-radius", &api, "app::save"]);
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("| score | depth | live | via | kind | qname | location |"), "{table}");
    assert!(!table.contains("| seed |"), "{table}");
}

#[test]
fn every_query_unknown_exits_3_with_the_absence() {
    let repo = TempRepo::new("unknown");
    let api = repo.api();
    let out = glia(&["blast-radius", &api, "nope", "zilch"]);
    assert_eq!(out.status.code(), Some(3));
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("> unresolved: nope, zilch"), "{table}");
    assert!(table.contains("> FACT: no node has the qname or name `nope`"), "{table}");
    let out = glia(&["blast-radius", &api, "nope", "--json"]);
    assert_eq!(out.status.code(), Some(3), "the JSON answer exits 3 too");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(v["absence"]["reason"], "unknown_symbol", "{v}");
    assert_eq!(v["results"], serde_json::json!([]), "{v}");
}

/// DD.10: the three ops of one contract class (identity-contract-ops: the
/// swagger op, the swaggo op and the feature.yaml op, DERIVED) are one
/// DOC_SECTION row with its `identity` by default and three rows with
/// `--no-collapse`. The `[identity] collapse surface=blast_radius` stderr line
/// is DD.10's fired_on marker, printed beside the unchanged `[blast]` line.
#[test]
fn identity_classes_collapse_unless_no_collapse() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../bench/substrate-gap/fixtures/identity-contract-ops");
    let args = ["blast-radius", fixture, "GET /api/orders", "--direction", "backward", "--depth", "1"];
    let docs = |v: &serde_json::Value| -> Vec<serde_json::Value> {
        v["results"].as_array().expect("results").iter().filter(|r| r["kind"] == "DOC_SECTION").cloned().collect()
    };

    let out = glia(&[&args[..], &["--json"]].concat());
    assert!(out.status.success(), "exit {:?}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let rows = docs(&v);
    assert_eq!(rows.len(), 1, "one row for the class: {v}");
    let id = &rows[0]["identity"];
    assert_eq!(id["tier"], "derived", "{v}");
    assert_eq!(id["rules"], serde_json::json!(["contract_op", "contract_route"]), "{v}");
    assert_eq!(id["also_in"].as_array().map(Vec::len), Some(2), "{v}");
    assert_eq!(marker(&out), "[blast] seeds=1 unresolved=0 reached=1 linked_seeds=0 walk=Backward");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().filter(|l| l.starts_with("[identity] collapse surface=blast_radius")).collect();
    assert_eq!(lines.len(), 1, "one identity line per answer:\n{stderr}");
    eprintln!("{}", lines[0]);
    assert!(lines[0].ends_with(" folded=0 rows=1"), "{}", lines[0]);

    let out = glia(&[&args[..], &["--json", "--no-collapse"]].concat());
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let rows = docs(&v);
    assert_eq!(rows.len(), 3, "one row per op: {v}");
    assert!(rows.iter().all(|r| r.get("identity").is_none()), "{v}");
    assert_eq!(marker(&out), "[blast] seeds=1 unresolved=0 reached=3 linked_seeds=0 walk=Backward");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("[identity] collapse"), "no class, no identity line");

    // The table's location cell names the other places.
    let out = glia(&args);
    assert!(out.status.success());
    let table = String::from_utf8_lossy(&out.stdout);
    let row = table.lines().find(|l| l.contains("| DOC_SECTION |")).expect("the class row");
    assert!(row.contains(" (also in: "), "{table}");
    assert_eq!(table.lines().filter(|l| l.contains("| DOC_SECTION |")).count(), 1, "{table}");
}
