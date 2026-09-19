//! LG.11: glia's DOCUMENTS edges export as `EdgeKind::Documents` (weight 0.3,
//! unchanged), not folded into `Cooccurs` beside SHARES_* co-location, and
//! `ExportStats::documents_edges` counts them apart from LG.12's NatSpec
//! Documents edges (`natspec_edges`), which never pass through `edge_kind`.

use std::path::{Path, PathBuf};

use engram_core::{EdgeKind, Gmap, GmapEdge};
use glia_engram_export::{ExportOptions, ExportStats, build_gmap};

/// The LG.7 probe fixture: a Python service, a README whose second section
/// names `OrderService.place` and `Vault.deposit` (the doc-linker emits a
/// DOCUMENTS edge to each), and a Solidity vault with one untagged `///` line.
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

/// The same vault with NatSpec tags on `deposit`: `@notice` becomes its doc,
/// the three other tags become facts, each with a Documents edge (LG.12).
const VAULT_TAGGED_SOL: &str = "// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract Vault {
    mapping(address => uint256) public balances;

    /// @notice Deposit ether into the vault
    /// @dev Emits no event
    /// @param who The account credited
    /// @return credited The new balance
    function deposit(address who) public payable returns (uint256 credited) {
        balances[who] += msg.value;
        return balances[who];
    }
}
";

/// The `## Placing orders` section. Doc keys are dir-scoped:
/// `docs::<doc path without extension>::<slug>`, so `docs/README.md` gives
/// `docs::docs::README::..`.
const PLACING: &str = "docs::docs::README::placing-orders";
const PLACE: &str = "svc::orders::OrderService::place";
const DEPOSIT: &str = "contracts::Vault::Vault::deposit";

/// A fresh scratch repo for one test, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-lg11-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Scratch(root)
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// The three-file fixture under `fx/`, with `vault` as the Solidity file.
    fn fixture(&self, vault: &str) -> PathBuf {
        self.write("fx/svc/orders.py", ORDERS_PY);
        self.write("fx/docs/README.md", README_MD);
        self.write("fx/contracts/Vault.sol", vault);
        self.0.join("fx")
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

fn edges_from<'g>(gmap: &'g Gmap, from: &str) -> Vec<&'g GmapEdge> {
    gmap.edges.iter().filter(|e| e.from == from).collect()
}

fn count(gmap: &Gmap, kind: EdgeKind) -> usize {
    gmap.edges.iter().filter(|e| e.kind == kind).count()
}

#[test]
fn doc_links_export_as_documents_not_cooccurs() {
    let scratch = Scratch::new("fixture");
    let repo = scratch.fixture(VAULT_SOL);
    let (gmap, stats) = export(&repo);

    // Exactly the two README -> symbol links leave the section, both
    // Documents at the DOCUMENTS weight (only the kind moved in v6).
    let out = edges_from(&gmap, PLACING);
    let mut to: Vec<&str> = out.iter().map(|e| e.to.as_str()).collect();
    to.sort_unstable();
    assert_eq!(to, [DEPOSIT, PLACE], "edges from {PLACING}: {out:?}");
    for e in &out {
        assert_eq!((e.kind, e.weight), (EdgeKind::Documents, Some(0.3)), "{e:?}");
    }

    // Nothing a doc section emits is folded into co-occurrence any more.
    let folded: Vec<&GmapEdge> = gmap
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Cooccurs && e.from.starts_with("docs::"))
        .collect();
    assert!(folded.is_empty(), "doc edges still exported as Cooccurs: {folded:?}");

    // The untagged `///` line is a notice only: no NatSpec facts, no NatSpec
    // edges, so every Documents edge here is a glia DOCUMENTS edge.
    assert_eq!(stats.natspec_edges, 0);
    assert_eq!(stats.documents_edges, 2);
    assert_eq!(count(&gmap, EdgeKind::Documents), stats.documents_edges);
}

#[test]
fn documents_edges_count_apart_from_natspec_edges() {
    let scratch = Scratch::new("natspec");
    let repo = scratch.fixture(VAULT_TAGGED_SOL);
    let (gmap, stats) = export(&repo);

    // The README links are unchanged by the tags.
    let mut to: Vec<&str> = edges_from(&gmap, PLACING).iter().map(|e| e.to.as_str()).collect();
    to.sort_unstable();
    assert_eq!(to, [DEPOSIT, PLACE]);

    // `@dev`, `@param who`, `@return credited`: three facts, three NatSpec
    // Documents edges, counted in natspec_edges and not in documents_edges.
    assert_eq!(stats.natspec_facts, 3);
    assert_eq!(stats.natspec_edges, 3);
    assert_eq!(stats.documents_edges, 2);
    assert_eq!(count(&gmap, EdgeKind::Documents), stats.documents_edges + stats.natspec_edges);
    let natspec: Vec<&GmapEdge> = gmap
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Documents && e.from.contains("#natspec:"))
        .collect();
    assert_eq!(natspec.len(), stats.natspec_edges);
    assert!(natspec.iter().all(|e| e.to == DEPOSIT && e.weight == Some(1.0)), "{natspec:?}");
}
