//! LE.10d — `glia contracts --fields [--breaking-only]`, driving the real
//! binary over LE.10a / LE.10b's committed substrate-gap fixtures.
//!
//! `--fields` renders LE.10c's `contract_fields` rows after the topic table
//! (with `--json`: an object `{topics, fields}`). Without it, `glia contracts`
//! must print exactly what it printed before this packet: the golden below was
//! captured from the pre-packet binary at the wave HEAD.
//!
//! The `[contract-fields] surface=cli rows=N` stderr line is the fired_on
//! marker; each test relays it so `-- --nocapture | grep '\[contract-fields\]'`
//! sees it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FIXTURES: &str = "bench/substrate-gap/fixtures";

/// The workspace root: paths are passed relative to it, so the rendered
/// heading (`# glia contracts \`<repo>\``) is the same on every machine.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli/ has a parent")
        .to_path_buf()
}

/// `glia contracts <fixture>/<a> --with <fixture>/<b> <flags>`.
fn contracts(fixture: &str, a: &str, b: &str, flags: &[&str]) -> Output {
    let (a, b) = (
        format!("{FIXTURES}/{fixture}/{a}"),
        format!("{FIXTURES}/{fixture}/{b}"),
    );
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .current_dir(workspace())
        .env("GLIA_NO_PERSIST", "1")
        .args(["contracts", a.as_str(), "--with", b.as_str()])
        .args(flags)
        .output()
        .expect("glia runs");
    assert!(
        out.status.success(),
        "glia contracts {fixture} {flags:?} exited {:?}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[contract-fields] ") {
            eprintln!("{line}");
        }
    }
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The `--fields` table's body rows (after the `| pairing |` header and its
/// separator).
fn field_rows(text: &str) -> Vec<&str> {
    text.lines()
        .skip_while(|l| !l.starts_with("| pairing |"))
        .skip(2)
        .take_while(|l| l.starts_with("| "))
        .collect()
}

/// The pre-packet `glia contracts` stdout over xcut-queue-msgtype-go
/// (client --with server), byte for byte.
const XCUT_TOPICS: &str = "# glia contracts `bench/substrate-gap/fixtures/xcut-queue-msgtype-go/client`

| topic | producer type | consumer type | status | where |
|---|---|---|---|---|
| `orders` | `OrderCreated` | `OrderCreated` | match (strong) | server/publisher.go:16 → client/consumer.go:12 |
";

#[test]
fn contracts_without_fields_flag_unchanged() {
    let plain = contracts("xcut-queue-msgtype-go", "client", "server", &[]);
    let text = stdout(&plain);
    assert_eq!(text, XCUT_TOPICS, "no-flag output moved");
    assert!(!text.contains("| pairing |"), "{text}");
    assert!(
        !stderr(&plain).contains("[contract-fields]"),
        "no field diff runs without --fields:\n{}",
        stderr(&plain)
    );

    let with_fields = contracts("xcut-queue-msgtype-go", "client", "server", &["--fields"]);
    let full = stdout(&with_fields);
    assert!(
        full.starts_with(&text),
        "--fields must print the topic table unchanged, then the fields:\n{full}"
    );
    // Both sides name `OrderCreated`, but no schema declares it: no field row.
    assert_eq!(
        &full[text.len()..],
        "\n## field contracts\n\n_(no field contracts)_\n"
    );
    assert!(
        stderr(&with_fields).contains("[contract-fields] surface=cli rows=0"),
        "{}",
        stderr(&with_fields)
    );

    // --json without --fields stays the bare topic array.
    let json = stdout(&contracts(
        "xcut-queue-msgtype-go",
        "client",
        "server",
        &["--json"],
    ));
    let v: serde_json::Value = serde_json::from_str(&json).expect("stdout is JSON");
    assert!(v.is_array(), "{json}");
}

#[test]
fn fields_table_over_proto_drift() {
    let out = contracts("proto-field-drift", "producer", "consumer", &["--fields"]);
    let text = stdout(&out);
    let err = stderr(&out);
    assert!(
        text.contains("| pairing | key | producer | consumer | status | changes |"),
        "{text}"
    );
    let rows = field_rows(&text);
    assert_eq!(rows.len(), 1, "one SHARES_SCHEMA pair: {text}");
    let row = rows[0];
    assert!(
        row.starts_with(
            "| schema_copy | `shop.v1.OrderCreated` | producer/proto/orders.proto:6 (proto) \
             | consumer/proto/orders.proto:6 (proto) | breaking | "
        ),
        "pairing, key, 1-based located sides, status: {row}"
    );
    assert!(
        row.contains("**total_cents: int64 -> int32 (proto_wire_type)**"),
        "the breaking change, in bold: {row}"
    );
    assert!(
        row.contains("labels: map<string,string> -> — (proto_unknown_field)"),
        "a non-breaking change, plain: {row}"
    );
    // The engine's line, then the surface's.
    let engine = err
        .find("[contract-fields] pairs=1 schema_copy=1 ")
        .unwrap_or_else(|| panic!("engine marker missing:\n{err}"));
    let surface = err
        .find("[contract-fields] surface=cli rows=1\n")
        .unwrap_or_else(|| panic!("surface marker missing:\n{err}"));
    assert!(engine < surface, "{err}");
}

#[test]
fn breaking_only_filters() {
    // Avro judges reader vs writer: the two directions of one pair differ,
    // so it is two rows, one breaking and one compatible.
    let all = stdout(&contracts(
        "avro-field-drift",
        "producer",
        "consumer",
        &["--fields"],
    ));
    let rows = field_rows(&all);
    assert_eq!(rows.len(), 2, "{all}");
    assert!(rows.iter().any(|r| r.contains(" | compatible | ")), "{all}");

    let out = contracts(
        "avro-field-drift",
        "producer",
        "consumer",
        &["--fields", "--breaking-only"],
    );
    let only = stdout(&out);
    let kept = field_rows(&only);
    assert_eq!(kept.len(), 1, "{only}");
    assert!(kept[0].contains(" | breaking | "), "{only}");
    assert!(
        kept[0].contains("**totalCents: long -> int (avro_type_changed)**"),
        "{only}"
    );
    // A `|` inside a union type is escaped, not a column break.
    assert!(kept[0].contains("coupon: null\\|string -> —"), "{only}");
    assert!(
        stderr(&out).contains("[contract-fields] surface=cli rows=1"),
        "the marker counts the rows printed:\n{}",
        stderr(&out)
    );

    // Nothing breaking: the empty line names the filter.
    let none = stdout(&contracts(
        "openapi-pact-fields",
        "provider",
        "web",
        &["--fields", "--breaking-only"],
    ));
    assert!(
        none.ends_with("_(no breaking field contracts)_\n"),
        "{none}"
    );

    // --breaking-only is a --fields filter; alone it is a usage error.
    let bare = Command::new(env!("CARGO_BIN_EXE_glia"))
        .current_dir(workspace())
        .args([
            "contracts",
            "bench/substrate-gap/fixtures/avro-field-drift/producer",
            "--breaking-only",
        ])
        .output()
        .expect("glia runs");
    assert_eq!(bare.status.code(), Some(2), "{}", stderr(&bare));
    assert!(stderr(&bare).contains("--fields"), "{}", stderr(&bare));
}

#[test]
fn fields_json_has_topics_and_fields() {
    let out = contracts(
        "proto-field-drift",
        "producer",
        "consumer",
        &["--fields", "--json"],
    );
    let text = stdout(&out);
    assert!(text.starts_with("{\"topics\":"), "topics first: {text}");
    let v: serde_json::Value = serde_json::from_str(&text).expect("stdout is JSON");
    let obj = v.as_object().expect("an object, not an array");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["fields", "topics"], "{text}");
    assert_eq!(
        obj["topics"],
        serde_json::json!([]),
        "no queue here: {text}"
    );
    let fields = obj["fields"].as_array().expect("fields is an array");
    assert_eq!(fields.len(), 1, "{text}");
    let row = &fields[0];
    assert_eq!(row["pairing"], "schema_copy");
    assert_eq!(row["status"], "breaking");
    assert_eq!(row["producer"]["line"], 6, "1-based");
    assert!(
        row["changes"]
            .as_array()
            .expect("changes")
            .iter()
            .any(|c| c["field"] == "total_cents" && c["rule"] == "proto_wire_type"),
        "{text}"
    );

    // `topics` is exactly the no-flag --json array.
    let plain = stdout(&contracts(
        "xcut-queue-msgtype-go",
        "client",
        "server",
        &["--json"],
    ));
    let both = stdout(&contracts(
        "xcut-queue-msgtype-go",
        "client",
        "server",
        &["--fields", "--json"],
    ));
    let plain: serde_json::Value = serde_json::from_str(&plain).expect("JSON");
    let both: serde_json::Value = serde_json::from_str(&both).expect("JSON");
    assert_eq!(both["topics"], plain, "topics carried through unchanged");
    assert_eq!(both["fields"], serde_json::json!([]));
}
