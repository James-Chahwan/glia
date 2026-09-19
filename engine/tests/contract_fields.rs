//! LE.10c acceptance: `contract_fields` diffs the DECLARED fields of both
//! sides of every contract pairing the graph already holds — two repos' copies
//! of one message (SHARES_SCHEMA), a queue topic whose sides are typed, an
//! AsyncAPI channel's publish / subscribe ops, and an OpenAPI op against a
//! Pact interaction — and judges each difference by that format's own
//! compatibility rules.
//!
//! The trees are LE.10a / LE.10b's committed substrate-gap fixtures, built
//! with `generate_many` over the dirs their key.json names, so the rows are
//! exactly what `glia contracts --fields` (LE.10d) will print. Variants are
//! written to a temp dir.

use std::path::{Path, PathBuf};

use glia_engine::contract_fields::{FieldChange, FieldDiffRow, contract_fields};
use glia_engine::generate_many;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
}

/// The repo paths a fixture's key.json `dirs` names, in order.
fn fixture_dirs(name: &str) -> Vec<String> {
    let root = fixture(name);
    let key: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("key.json")).unwrap()).unwrap();
    key["dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| root.join(d.as_str().unwrap()).to_str().unwrap().to_string())
        .collect()
}

fn rows_for(paths: &[String]) -> Vec<FieldDiffRow> {
    contract_fields(&generate_many(paths).unwrap().merged)
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Copy a fixture's files into `dst` so a variant can edit one of them.
fn copy_tree(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&to).unwrap();
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}

fn change<'a>(row: &'a FieldDiffRow, field: &str, rule: &str) -> &'a FieldChange {
    row.changes
        .iter()
        .find(|c| c.field == field && c.rule == rule)
        .unwrap_or_else(|| panic!("no {rule} change for `{field}` in {row:#?}"))
}

fn only_row<'a>(rows: &'a [FieldDiffRow], pairing: &str) -> &'a FieldDiffRow {
    let of: Vec<&FieldDiffRow> = rows.iter().filter(|r| r.pairing == pairing).collect();
    assert_eq!(of.len(), 1, "one {pairing} row expected: {rows:#?}");
    of[0]
}

// ---- (1) proto copies ----------------------------------------------------

#[test]
fn proto_copy_drift() {
    let rows = rows_for(&fixture_dirs("proto-field-drift"));
    assert_eq!(
        rows.len(),
        1,
        "one SHARES_SCHEMA pair, rules symmetric: one row: {rows:#?}"
    );
    let r = only_row(&rows, "schema_copy");
    assert_eq!(r.key, "shop.v1.OrderCreated");
    assert_eq!(r.status, "breaking");
    assert_eq!(r.tier, "derived");
    assert_eq!(r.producer.format, "proto");
    assert_eq!(r.consumer.format, "proto");
    assert_eq!(r.producer.qname, "message:proto:shop.v1.OrderCreated");
    assert_ne!(r.producer.repo_id, r.consumer.repo_id);
    assert_eq!(r.producer.file.as_deref(), Some("proto/orders.proto"));
    assert_eq!(
        r.producer.line,
        Some(6),
        "1-based: `message OrderCreated` is line 6"
    );

    let t = change(r, "total_cents", "proto_wire_type");
    assert_eq!(t.change, "type");
    assert_eq!(t.section, "fields");
    assert!(t.breaking);
    // The row keeps SHARES_SCHEMA's orientation: edge.from is the first
    // repo's copy (the resolver pairs in build order), the producer dir.
    assert_eq!(
        (t.producer.as_deref(), t.consumer.as_deref()),
        (Some("int64"), Some("int32"))
    );
    for f in ["labels", "card_token", "wallet_id"] {
        let u = change(r, f, "proto_unknown_field");
        assert!(!u.breaking, "{u:#?}");
    }
    // Nothing else differs: order_id and sku are the same number, name, type.
    assert_eq!(r.changes.len(), 4, "{r:#?}");
    assert_eq!(r.note, None);
}

// ---- (2) avro copies: reader / writer direction --------------------------

#[test]
fn avro_reader_rules() {
    let rows = rows_for(&fixture_dirs("avro-field-drift"));
    // The rules differ by direction, so each direction is its own row.
    assert_eq!(rows.len(), 2, "{rows:#?}");
    assert!(
        rows.iter()
            .all(|r| r.pairing == "schema_copy" && r.key == "com.shop.OrderPlaced")
    );
    assert_eq!(
        rows[0].producer.repo_id, rows[1].consumer.repo_id,
        "the two rows are the two directions"
    );

    // Writer = the long / coupon copy, reader = the int / channel copy.
    let breaking = rows
        .iter()
        .find(|r| r.status == "breaking")
        .unwrap_or_else(|| panic!("one direction breaks: {rows:#?}"));
    let t = change(breaking, "totalCents", "avro_type_changed");
    assert_eq!(
        (t.producer.as_deref(), t.consumer.as_deref()),
        (Some("long"), Some("int"))
    );
    assert!(t.breaking);
    let ch = change(breaking, "channel", "avro_reader_field_no_default");
    assert!(ch.breaking);
    assert_eq!(ch.change, "consumer_only");
    assert!(!change(breaking, "coupon", "avro_writer_field_ignored").breaking);

    // Writer = the int / channel copy, reader = the long / coupon copy.
    let other = rows.iter().find(|r| r.status != "breaking").unwrap();
    assert_eq!(other.status, "compatible", "{other:#?}");
    let p = change(other, "totalCents", "avro_promotion");
    assert_eq!(
        (p.producer.as_deref(), p.consumer.as_deref()),
        (Some("int"), Some("long"))
    );
    assert!(!p.breaking);
    assert!(!change(other, "channel", "avro_writer_field_ignored").breaking);
    // `coupon` has a default, so the reader fills it in.
    assert!(!change(other, "coupon", "avro_reader_field_default").breaking);
}

// ---- (3) OpenAPI provider vs Pact consumer -------------------------------

#[test]
fn openapi_vs_pact() {
    let rows = rows_for(&fixture_dirs("openapi-pact-fields"));
    let r = only_row(&rows, "route");
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(r.key, "POST /orders");
    assert_eq!(r.producer.format, "openapi");
    assert_eq!(r.consumer.format, "pact");
    assert_eq!(r.producer.qname, "contract::openapi::POST:/orders");
    // LB.12: a contract op is scoped by its file's directory + stem.
    assert_eq!(
        r.consumer.qname,
        "contract::pacts::web-orders::POST:/orders"
    );
    assert_eq!(r.status, "compatible", "{r:#?}");
    assert_eq!(
        r.changes.len(),
        1,
        "only giftWrap differs; coupon is optional: {r:#?}"
    );
    let g = change(r, "giftWrap", "undeclared_request_field");
    assert_eq!(g.section, "request");
    assert_eq!(g.change, "consumer_only");
    assert_eq!(g.consumer.as_deref(), Some("boolean"));
    assert!(!g.breaking);
}

#[test]
fn openapi_vs_pact_missing_required_field() {
    let tmp = tempfile::tempdir().unwrap();
    copy_tree(&fixture("openapi-pact-fields"), tmp.path());
    let pact = tmp.path().join("web/pacts/web-orders.json");
    let body = std::fs::read_to_string(&pact).unwrap();
    let edited = body.replace(r#""quantity": 2, "#, "");
    assert_ne!(body, edited, "the variant must drop quantity");
    std::fs::write(&pact, edited).unwrap();

    let paths: Vec<String> = ["provider", "web"]
        .iter()
        .map(|d| tmp.path().join(d).to_str().unwrap().to_string())
        .collect();
    let rows = rows_for(&paths);
    let r = only_row(&rows, "route");
    assert_eq!(r.status, "breaking", "{r:#?}");
    let q = change(r, "quantity", "missing_required_field");
    assert_eq!(q.change, "producer_only");
    assert_eq!(q.producer.as_deref(), Some("integer"));
    assert!(q.breaking);
    assert!(!change(r, "giftWrap", "undeclared_request_field").breaking);
}

// ---- (4) AsyncAPI channel ------------------------------------------------

#[test]
fn asyncapi_channel() {
    let rows = rows_for(&fixture_dirs("asyncapi-payload-fields"));
    let r = only_row(&rows, "channel");
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(r.key, "orders.placed");
    assert_eq!(
        r.producer.qname,
        "contract::asyncapi::publish:orders.placed"
    );
    assert_eq!(
        r.consumer.qname,
        "contract::asyncapi::subscribe:orders.placed"
    );
    assert_eq!(r.status, "breaking");
    let c = change(r, "currency", "consumer_field_not_produced");
    assert_eq!(c.section, "payload");
    assert!(c.breaking);
    assert!(!change(r, "totalCents", "producer_extra_field").breaking);
    assert_eq!(
        r.changes.len(),
        2,
        "orderId is on both sides with one type: {r:#?}"
    );
}

// ---- (5) queue topic with typed sides ------------------------------------

/// A Go NATS publisher of `pb.<ty>` on `topic` (message_contracts.rs shape).
fn go_publisher(topic: &str, ty: &str) -> String {
    format!(
        "package svc\n\nimport (\n\t\"github.com/nats-io/nats.go\"\n\t\"google.golang.org/protobuf/proto\"\n\n\tpb \"example.com/demo/gen\"\n)\n\n\
         // Publish marshals a {ty} and publishes it.\n\
         func Publish(nc *nats.Conn, id string) error {{\n\
         \tdata, err := proto.Marshal(&pb.{ty}{{Id: id}})\n\
         \tif err != nil {{\n\t\treturn err\n\t}}\n\
         \treturn nc.Publish(\"{topic}\", data)\n}}\n"
    )
}

/// A Go NATS subscriber that decodes `topic` as `pb.<ty>`.
fn go_subscriber(topic: &str, ty: &str) -> String {
    format!(
        "package worker\n\nimport (\n\t\"github.com/nats-io/nats.go\"\n\t\"google.golang.org/protobuf/proto\"\n\n\tpb \"example.com/demo/gen\"\n)\n\n\
         // Subscribe decodes every message as a {ty}.\n\
         func Subscribe(nc *nats.Conn) (*nats.Subscription, error) {{\n\
         \treturn nc.Subscribe(\"{topic}\", func(m *nats.Msg) {{\n\
         \t\tevt := &pb.{ty}{{}}\n\
         \t\t_ = proto.Unmarshal(m.Data, evt)\n\
         \t}})\n}}\n"
    )
}

const PROTO_PRODUCER: &str = "syntax = \"proto3\";\npackage shop.v1;\n\n\
message OrderCreated {\n  string id = 1;\n  int64 total_cents = 2;\n  string currency = 3;\n}\n";

const PROTO_CONSUMER: &str = "syntax = \"proto3\";\npackage shop.v1;\n\n\
message OrderCreated {\n  string id = 1;\n  int64 total_cents = 2;\n  reserved 3;\n  string note = 4;\n}\n";

#[test]
fn topic_row() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = tmp.path().join("svc");
    let worker = tmp.path().join("worker");
    write(
        &svc,
        "publisher.go",
        &go_publisher("orders", "OrderCreated"),
    );
    write(&svc, "proto/orders.proto", PROTO_PRODUCER);
    write(
        &worker,
        "consumer.go",
        &go_subscriber("orders", "OrderCreated"),
    );
    write(&worker, "proto/orders.proto", PROTO_CONSUMER);

    let paths = vec![
        svc.to_str().unwrap().to_string(),
        worker.to_str().unwrap().to_string(),
    ];
    let rows = rows_for(&paths);
    let r = only_row(&rows, "topic");
    assert_eq!(r.key, "orders");
    assert_eq!(r.producer.qname, "message:proto:shop.v1.OrderCreated");
    assert_eq!(r.consumer.qname, "message:proto:shop.v1.OrderCreated");
    assert_ne!(r.producer.repo_id, r.consumer.repo_id);
    assert_eq!(r.status, "breaking", "{r:#?}");
    // The producer still writes #3, which the consumer reserved.
    let res = change(r, "currency", "proto_reserved_reused");
    assert_eq!(res.change, "reserved");
    assert!(res.breaking);
    let n = change(r, "note", "proto_unknown_field");
    assert_eq!(n.change, "consumer_only");
    assert!(!n.breaking);
    assert_eq!(r.changes.len(), 2, "{r:#?}");
    // The same two copies also share a schema: their schema_copy row exists
    // alongside, keyed by the message, not the topic.
    assert_eq!(only_row(&rows, "schema_copy").key, "shop.v1.OrderCreated");
}

/// Both sides typed by a message only one repo declares: nothing to compare
/// on the other side, so the topic row is unknown, never a guess.
#[test]
fn topic_row_with_one_unresolved_side_is_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = tmp.path().join("svc");
    let worker = tmp.path().join("worker");
    write(
        &svc,
        "publisher.go",
        &go_publisher("orders", "OrderCreated"),
    );
    write(&svc, "proto/orders.proto", PROTO_PRODUCER);
    write(
        &worker,
        "consumer.go",
        &go_subscriber("orders", "OrderCreated"),
    );

    let paths = vec![
        svc.to_str().unwrap().to_string(),
        worker.to_str().unwrap().to_string(),
    ];
    let rows = rows_for(&paths);
    let r = only_row(&rows, "topic");
    assert_eq!(r.status, "unknown", "{r:#?}");
    assert!(r.changes.is_empty());
    assert_eq!(r.note, Some("consumer_message_type_unresolved"));
}

// ---- (6) identical / unknown ---------------------------------------------

#[test]
fn identical_copies_are_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    write(&a, "orders.proto", PROTO_PRODUCER);
    write(&b, "proto/orders.proto", PROTO_PRODUCER);
    let paths = vec![
        a.to_str().unwrap().to_string(),
        b.to_str().unwrap().to_string(),
    ];
    let rows = rows_for(&paths);
    let r = only_row(&rows, "schema_copy");
    assert_eq!(r.status, "identical", "{r:#?}");
    assert!(r.changes.is_empty());
    assert_eq!(r.note, None);
}

#[test]
fn missing_fields_is_unknown() {
    // An enum has no SCHEMA_FIELDS cell (LE.10a), so two copies of one are a
    // pair whose fields are unknown — never "identical".
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    let src = "syntax = \"proto3\";\npackage shop.v1;\n\nenum Status {\n  STATUS_UNSPECIFIED = 0;\n  PAID = 1;\n}\n";
    write(&a, "status.proto", src);
    write(&b, "status.proto", src);
    let paths = vec![
        a.to_str().unwrap().to_string(),
        b.to_str().unwrap().to_string(),
    ];
    let rows = rows_for(&paths);
    let r = only_row(&rows, "schema_copy");
    assert_eq!(r.key, "shop.v1.Status");
    assert_eq!(r.status, "unknown", "{r:#?}");
    assert!(r.changes.is_empty());
    assert_eq!(r.note, Some("no_fields"));
}

/// An external `$ref` is unknown, not a type mismatch: the subscriber's
/// children of a field the publisher only names by reference are not
/// "not produced".
#[test]
fn external_ref_is_unknown_not_a_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let orders = tmp.path().join("orders");
    let billing = tmp.path().join("billing");
    write(
        &orders,
        "asyncapi.yaml",
        "asyncapi: 2.6.0\ninfo:\n  title: Orders\n  version: 1.0.0\nchannels:\n  orders.placed:\n    publish:\n      message:\n        payload:\n          type: object\n          properties:\n            orderId:\n              type: string\n            total:\n              $ref: './common.yaml#/Money'\n",
    );
    write(
        &billing,
        "asyncapi.yaml",
        "asyncapi: 2.6.0\ninfo:\n  title: Billing\n  version: 1.0.0\nchannels:\n  orders.placed:\n    subscribe:\n      message:\n        payload:\n          type: object\n          properties:\n            orderId:\n              type: string\n            total:\n              type: object\n              properties:\n                amount:\n                  type: integer\n",
    );
    let paths = vec![
        orders.to_str().unwrap().to_string(),
        billing.to_str().unwrap().to_string(),
    ];
    let rows = rows_for(&paths);
    let r = only_row(&rows, "channel");
    assert_eq!(r.status, "unknown", "{r:#?}");
    let t = change(r, "total", "unresolved_ref");
    assert_eq!(t.change, "unknown");
    assert!(!t.breaking);
    assert!(
        !r.changes
            .iter()
            .any(|c| c.field == "total.amount" && c.breaking),
        "a child under an unresolved ref is not a missing field: {r:#?}"
    );
}

#[test]
fn report_is_deterministic() {
    let dirs = fixture_dirs("proto-field-drift");
    let a = serde_json::to_string(&rows_for(&dirs)).unwrap();
    let b = serde_json::to_string(&rows_for(&dirs)).unwrap();
    assert_eq!(a, b, "two builds serialise identically");
}
