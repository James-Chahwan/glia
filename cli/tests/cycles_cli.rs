//! LE.6b — `glia cycles`, driving the real binary over an EventEmitter loop
//! (orders/ emits `order.placed`, billing/ handles it and emits
//! `payment.settled`, orders/ handles that and re-places the order) and over
//! an acyclic tree.
//!
//! The `[cycles]` stderr line is the LE.6b fired_on marker; asserting it here
//! makes its counts a tested contract, and relaying it lets
//! `cargo test -p glia-cli --test cycles_cli -- --nocapture 2>&1 | grep '^\[cycles\]'`
//! show it.

use std::path::PathBuf;
use std::process::{Command, Output};

const ORDERS_TS: &str = "import { EventEmitter } from \"events\";\nexport const bus = new EventEmitter();\nexport function placeOrder(o) { bus.emit(\"order.placed\", o); }\nexport function registerOrderHandlers() { bus.on(\"payment.settled\", (p) => { retryOrder(p); }); }\nfunction retryOrder(p) { placeOrder(p); }\n";
const BILLING_TS: &str = "import { EventEmitter } from \"events\";\nexport const bus = new EventEmitter();\nexport function registerBillingHandlers() { bus.on(\"order.placed\", (o) => { settle(o); }); }\nfunction settle(o) { bus.emit(\"payment.settled\", o); }\n";

/// A per-test directory under the system temp dir, removed on drop (the cli
/// crate has no `tempfile` dev-dependency). Manifest-free on purpose: LB.8b
/// drops in-process EVENT_FLOWS between two manifest projects.
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str, files: &[(&str, &str)]) -> Self {
        let root =
            std::env::temp_dir().join(format!("glia-cycles-cli-{}-{tag}", std::process::id()));
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

/// Run `glia <args>` with persistence off; assert exit 0 and relay the
/// `[cycles]` marker lines.
fn glia(args: &[&str]) -> (Output, Vec<String>) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "glia {args:?} exited {:?}\n{stderr}",
        out.status
    );
    let markers: Vec<String> = stderr
        .lines()
        .filter(|l| l.starts_with("[cycles]"))
        .map(str::to_string)
        .collect();
    for m in &markers {
        eprintln!("{m}");
    }
    (out, markers)
}

#[test]
fn event_loop_table_and_marker() {
    let fx = Fixture::new(
        "loop",
        &[("orders/app.ts", ORDERS_TS), ("billing/app.ts", BILLING_TS)],
    );
    let (out, markers) = glia(&["cycles", &fx.path()]);
    assert_eq!(
        markers,
        vec!["[cycles] event_loops=1 call_loops=0 possible=0 import_cycles=0 (sccs=1 nodes=9)"]
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("## cross-service event loops (event_loop): 1"),
        "{text}"
    );
    assert!(
        text.contains("## import cycles (import_cycle): 0"),
        "{text}"
    );
    assert!(
        text.contains("-[EVENT_FLOWS order.placed]-> `event_handle:order.placed`"),
        "the witness chain names the hop and its channel: {text}"
    );

    let (out, _) = glia(&["cycles", &fx.path(), "--json", "--kind", "event"]);
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON rows");
    assert_eq!(rows[0]["kind"], "event_loop");
    assert_eq!(
        rows[0]["services"],
        serde_json::json!(["billing", "orders"])
    );
    assert_eq!(rows[0]["witness"].as_array().map(Vec::len), Some(9));

    let (out, markers) = glia(&["cycles", &fx.path(), "--kind", "import"]);
    assert_eq!(
        markers,
        vec!["[cycles] event_loops=0 call_loops=0 possible=0 import_cycles=0 (sccs=0 nodes=0)"]
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("event loops"),
        "--kind import prints the import table only: {text}"
    );
}

#[test]
fn acyclic_repo_reports_nothing_and_exits_0() {
    let fx = Fixture::new(
        "acyclic",
        &[
            (
                "orders/app.ts",
                "export function placeOrder(o) { return save(o); }\nfunction save(o) { return o; }\n",
            ),
            (
                "billing/app.py",
                "from billing.util import charge\n\n\ndef bill(o):\n    return charge(o)\n",
            ),
            ("billing/util.py", "def charge(o):\n    return o\n"),
        ],
    );
    let (out, markers) = glia(&["cycles", &fx.path()]);
    assert_eq!(
        markers,
        vec!["[cycles] event_loops=0 call_loops=0 possible=0 import_cycles=0 (sccs=0 nodes=0)"]
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.matches("_(none)_").count(), 4, "{text}");
    let (out, _) = glia(&["cycles", &fx.path(), "--json"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "[]");
}
