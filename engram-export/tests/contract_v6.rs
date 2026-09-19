//! The engram-core v6 contract (LG.7), pinned from glia's side: the wire layout
//! glia writes and Engram reads, and export bytes that are a pure function of
//! the graph (so `content_digest` over them is a content address, which the
//! `--since` diff chain keys on).
//!
//! bincode 1.x is positional and not self-describing, so these tests assert raw
//! bytes, not just round-trips: an enum variant is its u32 LE index, a struct is
//! its fields in declaration order, a map is a u64 LE length then its pairs.

use std::collections::BTreeMap;
use std::path::PathBuf;

use engram_core::{
    Content, EdgeKind, GMAP_FORMAT_VERSION, GmapDiff, GmapEdge, GmapNode, NodeChange, SpanRef,
    content_digest, content_key, encodable_text,
};
use glia_engram_export::{ExportOptions, build_gmap, export_engram_gmap};

/// The probe fixture: a 12-line Python service, a 7-line README whose second
/// section names two symbols (so the doc-linker emits DOCUMENTS edges), and an
/// 18-line Solidity contract with one untagged `///` line.
const ORDERS_PY: &str = r#""""Shop orders: placing and checking."""

import json


class OrderService:
    def place(self, sku):
        """Place one order."""
        return self.validate(sku)

    def validate(self, sku):
        return bool(json.dumps(sku))
"#;

const README_MD: &str = "# Orders

Orders are placed through the order service.

## Placing orders

Call `OrderService.place` with a sku; payment settles via `Vault.deposit`.
";

const VAULT_SOL: &str = "// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract Vault {
    mapping(address => uint256) public balances;

    // Balances are credited on deposit and never
    // debited here: withdrawals live in a separate
    // settlement contract that reads this mapping.
    //
    // Keep this contract free of owner logic.

    /// Deposit ether into the vault
    function deposit(address who) public payable {
        balances[who] += msg.value;
        return;
    }
}
";

fn le32(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn node(key: &str, content: Content) -> GmapNode {
    GmapNode {
        key: key.to_string(),
        content,
        provenance: None,
        concept_hint: None,
        identity_hint: Some(format!("{key}#id")),
    }
}

#[test]
fn v6_constants_and_layout() {
    assert_eq!(GMAP_FORMAT_VERSION, 6);

    // Variant order is wire contract: the 12 pre-v6 kinds keep 0..=11 and
    // Documents is appended at 12.
    let older = [
        EdgeKind::Supersedes,
        EdgeKind::Contradicts,
        EdgeKind::Specializes,
        EdgeKind::Causes,
        EdgeKind::Cooccurs,
        EdgeKind::Imports,
        EdgeKind::Contains,
        EdgeKind::Calls,
        EdgeKind::Implements,
        EdgeKind::Extends,
        EdgeKind::DependsOn,
        EdgeKind::Returns,
    ];
    for (i, kind) in older.iter().enumerate() {
        assert_eq!(bincode::serialize(kind).unwrap(), le32(&[i as u32]), "{kind:?}");
    }
    assert_eq!(bincode::serialize(&EdgeKind::Documents).unwrap(), vec![12, 0, 0, 0]);
    let back: EdgeKind = bincode::deserialize(&[12, 0, 0, 0]).unwrap();
    assert_eq!(back, EdgeKind::Documents);

    // SpanRef: five u32 LE in declaration order, 20 bytes.
    let span = SpanRef { file: 1, start: 2, end: 3, start_line: 4, end_line: 5 };
    let bytes = bincode::serialize(&span).unwrap();
    assert_eq!(bytes.len(), 20);
    assert_eq!(bytes, le32(&[1, 2, 3, 4, 5]));
    assert_eq!(
        SpanRef::bytes(7, 10, 20),
        SpanRef { file: 7, start: 10, end: 20, start_line: 0, end_line: 0 }
    );
    assert_eq!(SpanRef::NONE, SpanRef::bytes(0, 0, 0));

    // FNV-1a 64: offset basis for no input, the published vector for "a".
    assert_eq!(content_digest(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(content_digest(b"a"), 0xaf63_dc4c_8601_ec8c);
}

#[test]
fn proposition_span_round_trips() {
    let anchored = Content::Proposition {
        text: "Orders are placed through the order service.".to_string(),
        span: Some(SpanRef { file: 2, start: 0, end: 57, start_line: 1, end_line: 3 }),
    };
    let unanchored = Content::proposition("Call OrderService.place with a sku.");
    assert_eq!(
        unanchored,
        Content::Proposition { text: "Call OrderService.place with a sku.".to_string(), span: None }
    );
    for c in [&anchored, &unanchored] {
        let bytes = bincode::serialize(c).unwrap();
        // Proposition is still variant 1 of Content.
        assert_eq!(&bytes[..4], &le32(&[1])[..]);
        let back: Content = bincode::deserialize(&bytes).unwrap();
        assert_eq!(&back, c);
    }
    // The anchor is not part of the fact's identity or its encodable text.
    assert_eq!(content_key(&anchored), "Orders are placed through the order service.");
    assert_eq!(encodable_text(&anchored), "Orders are placed through the order service.");
}

#[test]
fn gmap_diff_round_trips_and_leads_with_version() {
    let place = node(
        "svc::orders::OrderService::place",
        Content::Symbol {
            name: "place".to_string(),
            span: SpanRef { file: 3, start: 76, end: 167, start_line: 7, end_line: 9 },
            qname: Some("svc::orders::OrderService::place".to_string()),
            doc: Some("Place one order.".to_string()),
            imports: Some(vec!["json".to_string()]),
        },
    );
    let section = node("docs::README::placing-orders", Content::proposition("Placing orders"));
    let diff = GmapDiff {
        format_version: GMAP_FORMAT_VERSION,
        base_digest: content_digest(b"base"),
        target_digest: content_digest(b"target"),
        added: vec![section],
        removed: vec!["docs::README::orders".to_string()],
        modified: vec![NodeChange {
            prior_key: "api::orders::OrderService::place".to_string(),
            node: place,
            location_only: true,
        }],
        edges_added: vec![GmapEdge {
            from: "docs::README::placing-orders".to_string(),
            kind: EdgeKind::Documents,
            to: "svc::orders::OrderService::place".to_string(),
            weight: Some(0.3),
        }],
        edges_removed: vec![GmapEdge {
            from: "svc::orders::OrderService".to_string(),
            kind: EdgeKind::Contains,
            to: "svc::orders::OrderService::cancel".to_string(),
            weight: Some(1.0),
        }],
        files: BTreeMap::from([(2, "docs/README.md".to_string()), (3, "svc/orders.py".to_string())]),
    };
    let bytes = bincode::serialize(&diff).unwrap();
    // First field on purpose, as in Gmap: a reader gates on it before decoding the rest.
    assert_eq!(&bytes[..4], &6u32.to_le_bytes()[..]);
    let back: GmapDiff = bincode::deserialize(&bytes).unwrap();
    assert_eq!(bincode::serialize(&back).unwrap(), bytes);
    assert_eq!(back.base_digest, content_digest(b"base"));
    assert_eq!(back.target_digest, content_digest(b"target"));
    assert_eq!(back.removed, vec!["docs::README::orders".to_string()]);
    assert_eq!(back.modified[0].prior_key, "api::orders::OrderService::place");
    assert!(back.modified[0].location_only);
    assert_eq!(back.modified[0].node.content, diff.modified[0].node.content);
    assert_eq!(back.added[0].content, diff.added[0].content);
    assert_eq!(back.edges_added[0].kind, EdgeKind::Documents);
    assert_eq!(back.edges_removed[0].kind, EdgeKind::Contains);
    assert_eq!(back.files.get(&3).map(String::as_str), Some("svc/orders.py"));
}

/// A fresh scratch repo for one test, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-lg7-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Scratch(root)
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn export_bytes_are_deterministic() {
    let scratch = Scratch::new("determinism");
    let repo = scratch.0.join("fx");
    scratch.write("fx/svc/orders.py", ORDERS_PY);
    scratch.write("fx/docs/README.md", README_MD);
    scratch.write("fx/contracts/Vault.sol", VAULT_SOL);
    let result = glia_engine::generate_one(repo.to_str().unwrap()).unwrap();
    let opts = ExportOptions::default();

    // Before v6 `Gmap.files` was a HashMap: every build_gmap drew a fresh
    // RandomState, so the trailing files map came out in a different order and
    // six exports of this fixture gave three distinct byte strings.
    let mut runs = Vec::new();
    for _ in 0..8 {
        let (gmap, _, stats) = build_gmap(&result.merged, &repo, &opts);
        assert_eq!(stats.files, 3);
        assert_eq!(gmap.files.len(), 3);
        runs.push(bincode::serialize(&gmap).unwrap());
    }
    for (i, bytes) in runs.iter().enumerate().skip(1) {
        assert_eq!(bytes, &runs[0], "export {i} differs from export 0");
    }
    // The files map is written in id order, which is sorted-path order.
    let (gmap, _, _) = build_gmap(&result.merged, &repo, &opts);
    let paths: Vec<&str> = gmap.files.values().map(String::as_str).collect();
    assert_eq!(paths, ["contracts/Vault.sol", "docs/README.md", "svc/orders.py"]);

    // The written file is those bytes, and stats.digest addresses them.
    let out = scratch.0.join("fx.engram-gmap");
    let stats = export_engram_gmap(&result.merged, &repo, &out, &opts).unwrap();
    let written = std::fs::read(&out).unwrap();
    assert_eq!(written, runs[0]);
    assert_eq!(stats.digest, content_digest(&runs[0]));
    assert_eq!(&written[..4], &GMAP_FORMAT_VERSION.to_le_bytes()[..]);
}
