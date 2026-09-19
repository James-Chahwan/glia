//! LG.10: every exported span carries 1-based inclusive lines beside its bytes,
//! and a doc section's Proposition carries the same anchor. POSITION rows are
//! 0-based and end-inclusive; these tests pin the conversion on real parser
//! output (Python, markdown, Solidity) and on a hand-built graph whose files
//! cannot be read, where bytes fall back to 0 but lines still come from the rows.

use std::path::PathBuf;

use engram_core::{Content, Gmap, SpanRef};
use glia_code_domain::{CodeNav, cell_type, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};
use glia_engram_export::{ExportOptions, build_gmap};
use glia_graph::{MergedGraph, RepoGraph};

/// The LG.7 probe fixture: 12, 7 and 18 lines, every file newline-terminated.
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

/// A fresh scratch dir for one test, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-lg10-{}-{test}", std::process::id()));
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

fn content<'g>(gmap: &'g Gmap, key: &str) -> &'g Content {
    let keys: Vec<&str> = gmap.nodes.iter().map(|n| n.key.as_str()).collect();
    let node = gmap.nodes.iter().find(|n| n.key == key);
    &node.unwrap_or_else(|| panic!("no node {key}; keys: {keys:?}")).content
}

fn symbol_span(gmap: &Gmap, key: &str) -> SpanRef {
    match content(gmap, key) {
        Content::Symbol { span, .. } => *span,
        other => panic!("{key}: expected Symbol, got {other:?}"),
    }
}

fn anchor(gmap: &Gmap, key: &str) -> Option<SpanRef> {
    match content(gmap, key) {
        Content::Proposition { span, .. } => *span,
        other => panic!("{key}: expected Proposition, got {other:?}"),
    }
}

fn file_id(gmap: &Gmap, path: &str) -> u32 {
    let hit = gmap.files.iter().find(|(_, p)| p.as_str() == path);
    *hit.unwrap_or_else(|| panic!("{path} not interned: {:?}", gmap.files)).0
}

#[test]
fn fixture_spans_carry_one_based_inclusive_lines() {
    let scratch = Scratch::new("fixture");
    let repo = scratch.0.join("fx");
    scratch.write("fx/svc/orders.py", ORDERS_PY);
    scratch.write("fx/docs/README.md", README_MD);
    scratch.write("fx/contracts/Vault.sol", VAULT_SOL);
    let result = glia_engine::generate_one(repo.to_str().unwrap()).unwrap();
    let (gmap, _, stats) = build_gmap(&result.merged, &repo, &ExportOptions::default());
    let (py, md, sol) = (
        file_id(&gmap, "svc/orders.py"),
        file_id(&gmap, "docs/README.md"),
        file_id(&gmap, "contracts/Vault.sol"),
    );

    // place: rows 6..=8 -> lines 7..=9; bytes cover those whole lines.
    let place = symbol_span(&gmap, "svc::orders::OrderService::place");
    assert_eq!(place, SpanRef { file: py, start: 76, end: 167, start_line: 7, end_line: 9 });
    // deposit: rows 13..=16 -> lines 14..=17.
    let deposit = symbol_span(&gmap, "contracts::Vault::Vault::deposit");
    assert_eq!((deposit.file, deposit.start_line, deposit.end_line), (sol, 14, 17));
    // The module's end row is tree-sitter's point after the trailing newline
    // (row 12 of a 12-line file): the clamp keeps it on the last real line.
    let module = symbol_span(&gmap, "svc::orders");
    assert_eq!(module, SpanRef { file: py, start: 0, end: 234, start_line: 1, end_line: 12 });

    // Doc sections are Propositions anchored the same way (rows end-inclusive
    // since LG.10a): `# Orders` rows 0..=2, `## Placing orders` rows 4..=6.
    // Section keys are `docs::<dir-scoped doc path>::<slug>`.
    let orders = anchor(&gmap, "docs::docs::README::orders");
    assert_eq!(orders, Some(SpanRef { file: md, start: 0, end: 55, start_line: 1, end_line: 3 }));
    let placing = anchor(&gmap, "docs::docs::README::placing-orders");
    assert_eq!(placing, Some(SpanRef { file: md, start: 56, end: 150, start_line: 5, end_line: 7 }));

    // Every positioned node carries lines; both doc sections are anchored.
    assert!(stats.positioned > 0);
    assert_eq!(stats.spans_with_lines, stats.positioned);
    assert_eq!((stats.propositions, stats.propositions_anchored), (2, 2));
    assert_eq!(stats.unreadable_files, 0);
}

fn pos(file: &str, start: u32, end: u32) -> Cell {
    Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(format!(
            r#"{{"file":"{file}","start_line":{start},"end_line":{end}}}"#
        )),
    }
}

fn text(t: &str) -> Cell {
    Cell { kind: cell_type::CODE, payload: CellPayload::Text(t.to_string()) }
}

#[test]
fn unreadable_file_keeps_lines_and_no_position_is_none() {
    // POSITIONs name files that do not exist under the root: bytes fall back
    // to 0/0, lines still come from the rows. No POSITION -> no span at all.
    let scratch = Scratch::new("unreadable");
    let repo = RepoId(1);
    let mut nav = CodeNav::default();
    let mut nodes = Vec::new();
    let rows: [(u64, &str, &str, _, Vec<Cell>); 4] = [
        (1, "f", "app::gone::f", node_kind::FUNCTION, vec![pos("src/gone.py", 2, 3)]),
        (2, "g", "app::nowhere::g", node_kind::FUNCTION, vec![]),
        (
            3,
            "intro",
            "docs::GONE::intro",
            node_kind::DOC_SECTION,
            vec![pos("docs/GONE.md", 0, 0), text("Intro prose.")],
        ),
        (4, "bare", "docs::GONE::bare", node_kind::DOC_SECTION, vec![text("Unplaced prose.")]),
    ];
    for (id, name, qname, kind, cells) in rows {
        nav.record(NodeId(id), name, qname, kind, None);
        nodes.push(Node { id: NodeId(id), repo, confidence: Confidence::Strong, cells });
    }
    let g = RepoGraph {
        repo,
        nodes,
        edges: Vec::new(),
        nav,
        symbols: Default::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    };
    let merged = MergedGraph::new(vec![g]);
    let (gmap, _, stats) = build_gmap(&merged, &scratch.0, &ExportOptions::default());

    // Interned in sorted-path order: docs/GONE.md = 1, src/gone.py = 2.
    let f = symbol_span(&gmap, "app::gone::f");
    assert_eq!(f, SpanRef { file: 2, start: 0, end: 0, start_line: 3, end_line: 4 });
    assert_eq!(symbol_span(&gmap, "app::nowhere::g"), SpanRef::NONE);
    let intro = anchor(&gmap, "docs::GONE::intro");
    assert_eq!(intro, Some(SpanRef { file: 1, start: 0, end: 0, start_line: 1, end_line: 1 }));
    assert_eq!(anchor(&gmap, "docs::GONE::bare"), None);

    assert_eq!((stats.positioned, stats.spans_with_lines), (2, 2));
    assert_eq!((stats.propositions, stats.propositions_anchored), (2, 1));
    assert_eq!(stats.unreadable_files, 2);
}
