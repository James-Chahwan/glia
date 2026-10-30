//! FD.81: Go implicit IMPLEMENTS stays inside one Go module.
//!
//! The fixture is `bench/substrate-gap/fixtures/go-implicit-implements-per-module`
//! (microservices-demo's shape), copied to a tempdir: `src/checkoutservice`
//! and `src/frontend` are two Go modules (a go.mod each), each with its own
//! genproto package declaring `CartServiceClient` and `cartServiceClient`
//! over package-local message types (`*AddItemRequest`, `*Cart`, ..). The
//! two copies' signature texts match, but across modules the message types
//! are distinct, so a client of one module never implements the other
//! module's interface, at type level (`method_signature`) or method level
//! (the `same_name` pairs that ride on it). `src/frontend/conn`'s `Price`
//! implements `src/checkoutservice/money`'s `Formatter` across modules
//! through an import, with predeclared-only signatures, and keeps its edge.
//!
//! Every expect / forbid row of the fixture's key.json is asserted, plus the
//! method-level twins of both dropped type pairs.

use std::path::{Path, PathBuf};

use glia_code_domain::edge_category;
use glia_core::NodeId;
use glia_engine::{GenerateResult, generate_one};

const FIXTURE: &str = "../bench/substrate-gap/fixtures/go-implicit-implements-per-module";

/// The fixture's source files (key.json is not part of the tree).
const FILES: [&str; 8] = [
    "src/checkoutservice/go.mod",
    "src/checkoutservice/genproto/demo_grpc.pb.go",
    "src/checkoutservice/genproto/demo.pb.go",
    "src/checkoutservice/money/money.go",
    "src/frontend/go.mod",
    "src/frontend/conn/conn.go",
    "src/frontend/genproto/demo_grpc.pb.go",
    "src/frontend/genproto/demo.pb.go",
];

const CHECKOUT: &str = "src::checkoutservice::genproto::demo_grpc.pb";
const FRONTEND: &str = "src::frontend::genproto::demo_grpc.pb";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
}

fn build() -> GenerateResult {
    let src = fixture();
    let dir = tempfile::tempdir().expect("tempdir");
    for f in FILES {
        let body = std::fs::read_to_string(src.join(f)).unwrap_or_else(|e| panic!("read {f}: {e}"));
        let path = dir.path().join(f);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }
    generate_one(dir.path().to_str().expect("utf-8 path")).expect("build")
}

fn id_of(r: &GenerateResult, qname: &str) -> NodeId {
    let hits: Vec<NodeId> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.iter())
        .filter(|(_, q)| q.as_str() == qname)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one node {qname}, got {hits:?}"
    );
    hits[0]
}

fn implements(r: &GenerateResult, from: &str, to: &str) -> bool {
    let (f, t) = (id_of(r, from), id_of(r, to));
    r.merged
        .all_edges()
        .any(|e| e.from == f && e.to == t && e.category == edge_category::IMPLEMENTS)
}

/// The `(from, to)` rows of one key.json list.
fn rows(key: &serde_json::Value, list: &str) -> Vec<(String, String)> {
    key[list]
        .as_array()
        .unwrap_or_else(|| panic!("key.json has no {list}"))
        .iter()
        .map(|row| {
            assert_eq!(row["category"], "IMPLEMENTS");
            let s = |k: &str| row[k].as_str().expect("qname").to_string();
            (s("from"), s("to"))
        })
        .collect()
}

#[test]
fn implicit_implements_stays_inside_one_go_module() {
    let r = build();
    let key: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture().join("key.json")).expect("key.json"),
    )
    .expect("key.json parses");
    for (from, to) in rows(&key, "expect_edges") {
        assert!(
            implements(&r, &from, &to),
            "expected IMPLEMENTS {from} -> {to}"
        );
    }
    for (from, to) in rows(&key, "forbid") {
        assert!(
            !implements(&r, &from, &to),
            "forbidden IMPLEMENTS {from} -> {to}"
        );
    }
    // Both modules' in-module pairs, type and method level, stay; every
    // cross-module twin of them is gone.
    for (own, other) in [(CHECKOUT, FRONTEND), (FRONTEND, CHECKOUT)] {
        let (client, iface) = (
            format!("{own}::cartServiceClient"),
            format!("{own}::CartServiceClient"),
        );
        assert!(implements(&r, &client, &iface), "{client} -> {iface}");
        let cross = format!("{other}::CartServiceClient");
        assert!(!implements(&r, &client, &cross), "{client} -> {cross}");
        for m in ["AddItem", "GetCart"] {
            let (from, to) = (format!("{client}::{m}"), format!("{iface}::{m}"));
            assert!(implements(&r, &from, &to), "{from} -> {to}");
            let to = format!("{cross}::{m}");
            assert!(!implements(&r, &from, &to), "{from} -> {to}");
        }
    }
    // The cross-module control keeps its method-level pairs too.
    for m in ["Format", "Currency"] {
        let (from, to) = (
            format!("src::frontend::conn::conn::Price::{m}"),
            format!("src::checkoutservice::money::money::Formatter::{m}"),
        );
        assert!(implements(&r, &from, &to), "{from} -> {to}");
    }
}
