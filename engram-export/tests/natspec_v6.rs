//! LG.12: NatSpec tags as engram facts. A Solidity symbol with LA.8's
//! DOC_TAGS cell exports its `@notice` (else its `@title`) as `Symbol.doc`,
//! every other tag as a `natspec` Proposition keyed `<symbol>#natspec:<tag>..`
//! with a weight-1.0 Documents edge to the symbol, and `@inheritdoc <Base>` as
//! a Documents edge from the base function to the override. Symbols without a
//! natspec DOC_TAGS cell keep the DOC-cell path byte for byte.

use std::path::{Path, PathBuf};

use engram_core::{Content, EdgeKind, Gmap, GmapEdge, GmapNode, SpanRef};
use glia_engram_export::{ExportOptions, ExportStats, build_gmap};

const IVAULT_SOL: &str = "// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

interface IVault {
    /// @notice Withdraw funds.
    function withdraw(uint256 amount) external;
}
";

const VAULT_SOL: &str = "// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import \"./IVault.sol\";

/// @title A simple vault
/// @notice Holds deposits for users
/// @dev Uses a mapping for balances
/// @dev Owner-free
contract Vault is IVault {
    // Balances per account.
    mapping(address => uint256) public balances;

    /// @notice Deposit ether into the vault
    /// @dev Emits no event
    /// @param who The account credited
    /// @return credited The new balance
    function deposit(address who) public payable returns (uint256 credited) {
        balances[who] += msg.value;
        return balances[who];
    }

    /// @inheritdoc IVault
    /// @author Someone
    function withdraw(uint256 amount) external {}
}
";

const ORDERS_PY: &str = r#""""Shop orders: placing and checking."""

import json


class OrderService:
    def place(self, sku):
        """Place one order."""
        return self.validate(sku)

    def validate(self, sku):
        return bool(json.dumps(sku))
"#;

const VAULT: &str = "contracts::Vault::Vault";
const DEPOSIT: &str = "contracts::Vault::Vault::deposit";
const WITHDRAW: &str = "contracts::Vault::Vault::withdraw";
const IWITHDRAW: &str = "contracts::IVault::IVault::withdraw";

/// A fresh scratch repo for one test, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-lg12-{}-{test}", std::process::id()));
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

fn export(repo: &Path) -> (Gmap, ExportStats) {
    let result = glia_engine::generate_one(repo.to_str().unwrap()).unwrap();
    let (gmap, _, stats) = build_gmap(&result.merged, repo, &ExportOptions::default());
    (gmap, stats)
}

fn node<'g>(gmap: &'g Gmap, key: &str) -> &'g GmapNode {
    let keys: Vec<&str> = gmap.nodes.iter().map(|n| n.key.as_str()).collect();
    let hit = gmap.nodes.iter().find(|n| n.key == key);
    hit.unwrap_or_else(|| panic!("no node {key}; keys: {keys:?}"))
}

fn index(gmap: &Gmap, key: &str) -> usize {
    let hit = gmap.nodes.iter().position(|n| n.key == key);
    hit.unwrap_or_else(|| panic!("no node {key}"))
}

fn doc_and_span(gmap: &Gmap, key: &str) -> (Option<String>, SpanRef) {
    match &node(gmap, key).content {
        Content::Symbol { doc, span, .. } => (doc.clone(), *span),
        other => panic!("{key}: expected Symbol, got {other:?}"),
    }
}

fn documents_into<'g>(gmap: &'g Gmap, to: &str) -> Vec<&'g GmapEdge> {
    gmap.edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Documents && e.to == to)
        .collect()
}

/// The facts of `symbol`, in emission order: `(key suffix, text)`.
fn facts(gmap: &Gmap, symbol: &str) -> Vec<(String, String)> {
    let prefix = format!("{symbol}#");
    gmap.nodes
        .iter()
        .filter_map(|n| {
            let suffix = n.key.strip_prefix(&prefix)?;
            match &n.content {
                Content::Proposition { text, .. } => Some((suffix.to_string(), text.clone())),
                other => panic!("{}: expected Proposition, got {other:?}", n.key),
            }
        })
        .collect()
}

/// Every fact of `symbol` is a `natspec` Proposition anchored on the symbol's
/// span, sharing its concept, hinted `<symbol hint>#<suffix>`, emitted right
/// after the symbol, with exactly one weight-1.0 Documents edge to it.
fn assert_fact_shape(gmap: &Gmap, symbol: &str) {
    let sym = node(gmap, symbol);
    let (_, span) = doc_and_span(gmap, symbol);
    let at = index(gmap, symbol);
    for (i, (suffix, _)) in facts(gmap, symbol).iter().enumerate() {
        let key = format!("{symbol}#{suffix}");
        let fact = node(gmap, &key);
        assert_eq!(
            index(gmap, &key),
            at + 1 + i,
            "{key} not right after its symbol"
        );
        assert_eq!(fact.provenance.as_deref(), Some("natspec"), "{key}");
        match &fact.content {
            Content::Proposition { span: s, .. } => assert_eq!(*s, Some(span), "{key}"),
            _ => unreachable!(),
        }
        assert_eq!(fact.concept_hint, sym.concept_hint, "{key}");
        let hint = sym.identity_hint.as_ref().map(|h| format!("{h}#{suffix}"));
        assert!(hint.is_some(), "{symbol} is located, so it has a hint");
        assert_eq!(fact.identity_hint, hint, "{key}");
        let out: Vec<&GmapEdge> = gmap.edges.iter().filter(|e| e.from == key).collect();
        assert_eq!(out.len(), 1, "{key}: {out:?}");
        assert_eq!(
            (out[0].kind, out[0].to.as_str()),
            (EdgeKind::Documents, symbol),
            "{key}"
        );
        assert_eq!(out[0].weight, Some(1.0), "{key}");
    }
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn natspec_tags_become_facts_and_documents_edges() {
    let scratch = Scratch::new("facts");
    scratch.write("contracts/IVault.sol", IVAULT_SOL);
    scratch.write("contracts/Vault.sol", VAULT_SOL);
    scratch.write("svc/orders.py", ORDERS_PY);
    let (gmap, _) = export(&scratch.0);

    // deposit: the @notice alone is the doc; every other tag is a fact.
    assert_eq!(
        doc_and_span(&gmap, DEPOSIT).0.as_deref(),
        Some("Deposit ether into the vault")
    );
    assert_eq!(
        facts(&gmap, DEPOSIT),
        pairs(&[
            ("natspec:dev", "deposit @dev: Emits no event"),
            (
                "natspec:param:who",
                "deposit @param who: The account credited"
            ),
            (
                "natspec:return:credited",
                "deposit @return credited: The new balance"
            ),
        ])
    );
    assert_fact_shape(&gmap, DEPOSIT);

    // The contract: @notice is the doc, @title is a fact, and a repeated @dev
    // is numbered in source order.
    assert_eq!(
        doc_and_span(&gmap, VAULT).0.as_deref(),
        Some("Holds deposits for users")
    );
    assert_eq!(
        facts(&gmap, VAULT),
        pairs(&[
            ("natspec:title", "Vault @title: A simple vault"),
            ("natspec:dev", "Vault @dev: Uses a mapping for balances"),
            ("natspec:dev:1", "Vault @dev: Owner-free"),
        ])
    );
    assert_fact_shape(&gmap, VAULT);

    // withdraw: no @notice / @title, so no doc (never the collapsed DOC text);
    // @author is a fact; @inheritdoc is an edge, not a fact.
    assert_eq!(doc_and_span(&gmap, WITHDRAW).0, None);
    assert_eq!(
        facts(&gmap, WITHDRAW),
        pairs(&[("natspec:author", "withdraw @author: Someone")])
    );
    assert_fact_shape(&gmap, WITHDRAW);

    // @inheritdoc IVault: the base function documents the override. Its only
    // other Documents edge is its @author fact.
    let into: Vec<&str> = documents_into(&gmap, WITHDRAW)
        .iter()
        .map(|e| e.from.as_str())
        .collect();
    assert_eq!(
        into,
        [format!("{WITHDRAW}#natspec:author").as_str(), IWITHDRAW]
    );
    let inherit = documents_into(&gmap, WITHDRAW)[1];
    assert_eq!(inherit.weight, Some(1.0));

    // The interface function: @notice only, so a doc and no facts.
    assert_eq!(
        doc_and_span(&gmap, IWITHDRAW).0.as_deref(),
        Some("Withdraw funds.")
    );
    assert!(facts(&gmap, IWITHDRAW).is_empty());

    // A plain `//` comment is DOC without DOC_TAGS: it stays the doc (LA.8).
    let balances = format!("{VAULT}::balances");
    assert_eq!(
        doc_and_span(&gmap, &balances).0.as_deref(),
        Some("Balances per account.")
    );
    assert!(facts(&gmap, &balances).is_empty());

    // Nothing else carries a natspec provenance or a `#natspec:` key.
    let natspec_nodes = gmap
        .nodes
        .iter()
        .filter(|n| n.provenance.as_deref() == Some("natspec"));
    assert_eq!(natspec_nodes.count(), 7);
    assert_eq!(
        gmap.nodes
            .iter()
            .filter(|n| n.key.contains("#natspec:"))
            .count(),
        7
    );
}

#[test]
fn natspec_stats_count_symbols_facts_edges_and_inheritdoc() {
    let scratch = Scratch::new("stats");
    scratch.write("contracts/IVault.sol", IVAULT_SOL);
    scratch.write("contracts/Vault.sol", VAULT_SOL);
    scratch.write("svc/orders.py", ORDERS_PY);
    let (gmap, stats) = export(&scratch.0);
    // Vault, deposit, withdraw and IVault.withdraw carry a natspec DOC_TAGS
    // cell; 3 + 3 + 1 facts; 7 tag edges + 1 inheritdoc edge.
    assert_eq!(stats.natspec_symbols, 4);
    assert_eq!(stats.natspec_facts, 7);
    assert_eq!(stats.natspec_edges, 8);
    assert_eq!(
        (
            stats.natspec_inheritdoc_resolved,
            stats.natspec_inheritdoc_unresolved
        ),
        (1, 0)
    );
    assert_eq!(
        gmap.edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Documents)
            .count(),
        8
    );
    // The totals include the facts and their edges.
    assert_eq!(
        (stats.nodes, stats.edges),
        (gmap.nodes.len(), gmap.edges.len())
    );
}

/// Normalise a node's span file id to 0 once it is checked to name `path`:
/// dropping the two .sol files renumbers the interned files.
fn normalised(gmap: &Gmap, n: &GmapNode, path: &str) -> Vec<u8> {
    let mut n = n.clone();
    if let Content::Symbol { span, .. } = &mut n.content {
        assert_eq!(
            gmap.files.get(&span.file).map(String::as_str),
            Some(path),
            "{}",
            n.key
        );
        span.file = 0;
    }
    bincode::serialize(&n).unwrap()
}

#[test]
fn non_natspec_nodes_are_byte_identical() {
    let with = Scratch::new("with-sol");
    with.write("contracts/IVault.sol", IVAULT_SOL);
    with.write("contracts/Vault.sol", VAULT_SOL);
    with.write("svc/orders.py", ORDERS_PY);
    let without = Scratch::new("without-sol");
    without.write("svc/orders.py", ORDERS_PY);
    let (a, _) = export(&with.0);
    let (b, stats_b) = export(&without.0);

    let py = |g: &Gmap| -> Vec<Vec<u8>> {
        let nodes = g.nodes.iter().filter(|n| n.key.starts_with("svc::orders"));
        nodes.map(|n| normalised(g, n, "svc/orders.py")).collect()
    };
    let (pa, pb) = (py(&a), py(&b));
    assert_eq!(pa.len(), 4);
    assert_eq!(pa, pb);
    assert_eq!(
        (
            stats_b.natspec_symbols,
            stats_b.natspec_facts,
            stats_b.natspec_edges
        ),
        (0, 0, 0)
    );
}
