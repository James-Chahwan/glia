//! FH.23 — `resolver:evm`'s `abi_partial` rule over REAL builds.
//!
//! A client whose human-readable ABI drifted from its contract on one
//! member (here `withdrawFunds`, which the contract renamed `withdraw`) is
//! covered by no decl, so the `abi` rule identifies nothing. Before FH.23 the
//! whole client went unmatched, the members that still fit with it;
//! `abi_partial` pairs it to the one decl holding a strict majority (at least
//! two) of its entries, heuristic tier, and the stale member's call is the
//! one client left unpaired, which contract-breaks names at its site. A
//! client covering only one of three entries stays unmatched, a tie between
//! two decls the ABI's name does not break stays ambiguous, and a covering
//! ABI still reads rule `abi`.

use crate::git_fixture;

use git_fixture::GitRepo;
use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_core::{EdgeCategoryId, NodeId};
use glia_engine::contract_breaks::{ContractBreakArgs, contract_breaks_vs_rev};
use glia_engine::generate_one;
use glia_engine::why::why_edge;
use glia_graph::MergedGraph;

const HARDHAT: &str =
    r#"{ "name": "shop-contracts", "private": true, "devDependencies": { "hardhat": "2.22.2" } }"#;
const WEB: &str = r#"{ "name": "web", "dependencies": { "ethers": "^6.13.0" } }"#;

/// Escrow before the rename (`withdrawFunds`) or after it (`withdraw`), plus
/// the Vault / Safe pair a tied client sees.
fn escrow(withdraw: &str) -> String {
    format!(
        r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

contract Escrow {{
    mapping(bytes32 => uint256) public deposits;

    event Deposited(bytes32 indexed orderId, uint256 amount);

    function deposit(bytes32 orderId) external payable {{
        deposits[orderId] += msg.value;
        emit Deposited(orderId, msg.value);
    }}

    function {withdraw}(uint256 amount) external {{
        payable(msg.sender).transfer(amount);
    }}
}}
"#
    )
}

const LOCKERS: &str = r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

contract Vault {
    function lock(uint256 amount) external {}
    function unlock(uint256 amount) external {}
}

contract Safe {
    function lock(uint256 amount) external {}
    function unlock(uint256 amount) external {}
}
"#;

/// The stale client: its ABI still lists `withdrawFunds`.
const PAY: &str = r#"import { Contract } from 'ethers';

const ESCROW_ABI = [
  'function deposit(bytes32 orderId) payable',
  'function withdrawFunds(uint256 amount)',
  'event Deposited(bytes32 indexed orderId, uint256 amount)',
];

export async function pay(address: string, signer: any, orderId: string): Promise<void> {
  const escrow = new Contract(address, ESCROW_ABI, signer);
  await escrow.deposit(orderId);
  await escrow.withdrawFunds(1);
  escrow.on('Deposited', (id: string, amount: bigint) => console.log(id, amount));
}
"#;

/// Three more clients: one entry of three held, a tie, a covering ABI.
const OTHERS: &str = r#"import { Contract } from 'ethers';

const ODD_ABI = [
  'function deposit(bytes32 orderId) payable',
  'function refund(bytes32 orderId)',
  'function settle(bytes32 orderId)',
];

const LOCKER_ABI = [
  'function lock(uint256 amount)',
  'function unlock(uint256 amount)',
  'function lockAll()',
];

const PAYOUT_ABI = [
  'function deposit(bytes32 orderId) payable',
  'event Deposited(bytes32 indexed orderId, uint256 amount)',
];

export async function others(address: string, signer: any, orderId: string): Promise<void> {
  const odd = new Contract(address, ODD_ABI, signer);
  await odd.deposit(orderId);
  const locker = new Contract(address, LOCKER_ABI, signer);
  await locker.lock(1);
  const payout = new Contract(address, PAYOUT_ABI, signer);
  await payout.deposit(orderId);
}
"#;

/// The repo at P (the contract still declares `withdrawFunds`), committed.
fn repo_at_p() -> GitRepo {
    let repo = GitRepo::init();
    repo.write("contracts/package.json", HARDHAT);
    repo.write("contracts/contracts/Escrow.sol", &escrow("withdrawFunds"));
    repo.write("contracts/contracts/Lockers.sol", LOCKERS);
    repo.write("web/package.json", WEB);
    repo.write("web/src/pay.ts", PAY);
    repo.write("web/src/others.ts", OTHERS);
    repo.commit("P");
    repo
}

/// S: the contract renamed `withdrawFunds` to `withdraw`; the client did not.
fn drift(repo: &GitRepo) {
    repo.write("contracts/contracts/Escrow.sol", &escrow("withdraw"));
}

fn qname(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// `(from, to, rule)` of every `resolver:evm` edge of `category`, sorted.
fn evm_edges(m: &MergedGraph, category: EdgeCategoryId) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = m
        .all_edges()
        .filter(|e| e.category == category)
        .filter_map(|e| {
            let ev = Evidence::read(&e.cells)?;
            (ev.emitter == "resolver:evm").then(|| {
                (
                    qname(m, e.from),
                    qname(m, e.to),
                    ev.rule.unwrap_or_default(),
                )
            })
        })
        .collect();
    out.sort();
    out
}

fn s(a: &str, b: &str, c: &str) -> (String, String, String) {
    (a.into(), b.into(), c.into())
}

#[test]
fn a_stale_entry_leaves_the_members_that_fit_paired() {
    let repo = repo_at_p();
    drift(&repo);
    let r = generate_one(repo.path()).expect("the stale stack builds");
    let m = &r.merged;
    assert_eq!(
        evm_edges(m, edge_category::RPC_CALLS),
        [
            s(
                "evm_call:abi:ESCROW_ABI.deposit",
                "evm:Escrow.deposit",
                "abi_partial"
            ),
            s(
                "evm_call:abi:PAYOUT_ABI.deposit",
                "evm:Escrow.deposit",
                "abi"
            ),
        ],
        "withdrawFunds is stale; ODD_ABI holds 1 of 3; LOCKER_ABI ties Vault and Safe"
    );
    assert_eq!(
        evm_edges(m, edge_category::EVENT_FLOWS),
        [s(
            "contracts::contracts::Escrow::Escrow::Deposited",
            "evm_event:abi:ESCROW_ABI.Deposited",
            "abi_partial"
        )]
    );
    // The pairing is heuristic: check and contract-breaks read it advisory.
    let w = why_edge(
        m,
        "evm_call:abi:ESCROW_ABI.deposit",
        "evm:Escrow.deposit",
        Some("RPC_CALLS"),
    )
    .expect("why the abi_partial pairing");
    assert!(w.found, "{w:#?}");
    assert_eq!(w.edges.len(), 1, "{w:#?}");
    assert_eq!(w.edges[0].tier, "heuristic", "{w:#?}");
}

#[test]
fn contract_breaks_names_the_stale_member_only() {
    let repo = repo_at_p();
    drift(&repo);
    let b = contract_breaks_vs_rev(repo.path(), "HEAD", &ContractBreakArgs::default())
        .unwrap_or_else(|e| panic!("contract breaks vs HEAD: {e}"));
    let rows: Vec<(String, String, Option<String>, Option<i64>)> = b
        .orphaned_clients
        .iter()
        .map(|o| {
            (
                o.client_qname.clone(),
                o.target_qname.clone(),
                o.file.clone(),
                o.line,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [(
            "evm_call:abi:ESCROW_ABI.withdrawFunds".to_string(),
            "evm:Escrow.withdrawFunds".to_string(),
            Some("web/src/pay.ts".to_string()),
            Some(12),
        )],
        "{b:#?}"
    );
}

#[test]
fn a_covering_abi_still_reads_abi() {
    let repo = repo_at_p();
    let r = generate_one(repo.path()).expect("the stack at P builds");
    assert_eq!(
        evm_edges(&r.merged, edge_category::RPC_CALLS),
        [
            s(
                "evm_call:abi:ESCROW_ABI.deposit",
                "evm:Escrow.deposit",
                "abi"
            ),
            s(
                "evm_call:abi:ESCROW_ABI.withdrawFunds",
                "evm:Escrow.withdrawFunds",
                "abi"
            ),
            s(
                "evm_call:abi:PAYOUT_ABI.deposit",
                "evm:Escrow.deposit",
                "abi"
            ),
        ]
    );
}
