//! FH.24 — contract-breaks reads a stale EVM event subscription as an
//! orphan.
//!
//! `resolver:evm` writes EVENT_FLOWS Solidity event -> `evm_event:`
//! subscription, the reverse of the client -> provider categories, so before
//! FH.24 the orphan pass (which took an edge's `from` as the client and read
//! only HTTP / gRPC / RPC / GraphQL / WS) never saw a subscription whose
//! event the contract renamed. Now an EVENT_FLOWS edge whose EVIDENCE emitter
//! is `resolver:evm` is read from its subscriber end: a Go client that still
//! watches `Released` after the contract renamed it `Settled` (its other
//! members still pairing through `abi_partial`) is exactly one orphan, at the
//! subscription's site. An unchanged contract gives none, and an in-process
//! eventbus EVENT_FLOWS the change removed is no client / provider contract.

use crate::git_fixture;

use git_fixture::GitRepo;
use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_engine::contract_breaks::{ContractBreakArgs, ContractBreaks, contract_breaks_vs_rev};
use glia_engine::generate_one;

const HARDHAT: &str =
    r#"{ "name": "vault-contracts", "private": true, "devDependencies": { "hardhat": "2.22.2" } }"#;

const GO_MOD: &str = "module example.com/vault/client\n\ngo 1.22\n\nrequire (\n\tgithub.com/ethereum/go-ethereum v1.14.11\n)\n";

/// The contract, its payout event named `event`.
fn vault(event: &str) -> String {
    format!(
        r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

contract Vault {{
    address public keeper;
    mapping(address => uint256) public balances;

    event Deposited(address indexed payee, uint256 amount);
    event {event}(address indexed payee, uint256 amount);

    function deposit(address payee) external payable {{
        balances[payee] += msg.value;
        emit Deposited(payee, msg.value);
    }}

    function release(address payee, uint256 amount) external {{
        balances[payee] -= amount;
        payable(payee).transfer(amount);
        emit {event}(payee, amount);
    }}
}}
"#
    )
}

/// The client's JSON ABI: it never changes, so after the rename it still
/// lists `Released`.
const ABI_GO: &str = r#"package chain

// VaultABI is the Vault contract's ABI.
const VaultABI = `[
  {"type": "function", "name": "keeper", "inputs": [], "outputs": [{"name": "", "type": "address", "internalType": "address"}], "stateMutability": "view"},
  {"type": "function", "name": "deposit", "inputs": [{"name": "payee", "type": "address", "internalType": "address"}], "outputs": [], "stateMutability": "payable"},
  {"type": "function", "name": "release", "inputs": [{"name": "payee", "type": "address", "internalType": "address"}, {"name": "amount", "type": "uint256", "internalType": "uint256"}], "outputs": [], "stateMutability": "nonpayable"},
  {"type": "event", "name": "Deposited", "anonymous": false, "inputs": [{"name": "payee", "type": "address", "indexed": true, "internalType": "address"}, {"name": "amount", "type": "uint256", "indexed": false, "internalType": "uint256"}]},
  {"type": "event", "name": "Released", "anonymous": false, "inputs": [{"name": "payee", "type": "address", "indexed": true, "internalType": "address"}, {"name": "amount", "type": "uint256", "indexed": false, "internalType": "uint256"}]}
]`
"#;

/// The client: two calls and the `Released` subscription (line 40).
const VAULT_GO: &str = r#"package chain

import (
	"math/big"
	"strings"

	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/event"
)

// VaultClient pays into and releases from the Vault contract.
type VaultClient struct {
	contract *bind.BoundContract
}

// NewVaultClient binds the Vault contract deployed at addr.
func NewVaultClient(addr common.Address, backend bind.ContractBackend) (*VaultClient, error) {
	parsed, err := abi.JSON(strings.NewReader(VaultABI))
	if err != nil {
		return nil, err
	}
	return &VaultClient{contract: bind.NewBoundContract(addr, parsed, backend, backend, backend)}, nil
}

// Deposit pays into the vault for payee.
func (c *VaultClient) Deposit(opts *bind.TransactOpts, payee common.Address) (*types.Transaction, error) {
	return c.contract.Transact(opts, "deposit", payee)
}

// Release pays amount out of the vault to payee.
func (c *VaultClient) Release(opts *bind.TransactOpts, payee common.Address, amount *big.Int) (*types.Transaction, error) {
	return c.contract.Transact(opts, "release", payee, amount)
}

// WatchReleased subscribes to the vault's Released events.
func (c *VaultClient) WatchReleased(opts *bind.WatchOpts) (event.Subscription, error) {
	_, sub, err := c.contract.WatchLogs(opts, "Released")
	return sub, err
}
"#;

/// A NestJS in-process event: the service emits `topic`, the listener
/// handles `order.created`.
const NEST_PKG: &str = r#"{ "name": "orders", "dependencies": { "@nestjs/common": "^10.0.0", "@nestjs/event-emitter": "^2.0.0" } }"#;

fn orders_service(topic: &str) -> String {
    format!(
        r#"import {{ Injectable }} from "@nestjs/common";
import {{ EventEmitter2 }} from "@nestjs/event-emitter";

@Injectable()
export class OrdersService {{
  constructor(private eventEmitter: EventEmitter2) {{}}

  create(id: string) {{
    this.eventEmitter.emit("{topic}", {{ id }});
  }}
}}
"#
    )
}

const AUDIT_LISTENER: &str = r#"import { Injectable } from "@nestjs/common";
import { OnEvent } from "@nestjs/event-emitter";

@Injectable()
export class AuditListener {
  @OnEvent("order.created")
  onCreated(payload: { id: string }) {}
}
"#;

/// The EVM stack at P (the contract emits `Released`), committed.
fn evm_repo() -> GitRepo {
    let repo = GitRepo::init();
    repo.write("contracts/package.json", HARDHAT);
    repo.write("contracts/contracts/Vault.sol", &vault("Released"));
    repo.write("client/go.mod", GO_MOD);
    repo.write("client/chain/abi.go", ABI_GO);
    repo.write("client/chain/vault.go", VAULT_GO);
    repo.commit("P");
    repo
}

fn breaks(repo: &GitRepo) -> ContractBreaks {
    contract_breaks_vs_rev(repo.path(), "HEAD", &ContractBreakArgs::default())
        .unwrap_or_else(|e| panic!("contract breaks vs HEAD: {e}"))
}

type Row = (
    &'static str,
    String,
    String,
    Option<String>,
    Option<i64>,
    &'static str,
);

fn orphans(b: &ContractBreaks) -> Vec<Row> {
    b.orphaned_clients
        .iter()
        .map(|o| {
            (
                o.category,
                o.client_qname.clone(),
                o.target_qname.clone(),
                o.file.clone(),
                o.line,
                o.reason,
            )
        })
        .collect()
}

#[test]
fn the_subscription_pairs_from_the_event_at_p() {
    // The premise: at P resolver:evm writes event -> subscription.
    let repo = evm_repo();
    let r = generate_one(repo.path()).expect("the stack at P builds");
    let flows = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::EVENT_FLOWS)
        .filter(|e| Evidence::of(e).is_some_and(|ev| ev.emitter == "resolver:evm"))
        .count();
    assert_eq!(flows, 1, "the Released subscription pairs its event");
}

#[test]
fn a_renamed_event_orphans_its_stale_subscription() {
    let repo = evm_repo();
    repo.write("contracts/contracts/Vault.sol", &vault("Settled"));
    let b = breaks(&repo);
    let rows = orphans(&b);
    assert_eq!(rows.len(), 1, "{b:#?}");
    let (category, client, target, file, line, reason) = rows[0].clone();
    assert_eq!(category, "EVENT_FLOWS", "{b:#?}");
    assert!(client.ends_with(".Released"), "{client}");
    assert!(client.starts_with("evm_event:"), "{client}");
    assert!(target.ends_with("Released"), "{target}");
    assert_eq!(file.as_deref(), Some("client/chain/vault.go"), "{b:#?}");
    assert_eq!(line, Some(40), "{b:#?}");
    assert!(
        reason == "target_removed" || reason == "pairing_lost",
        "{reason}"
    );
    assert!(b.breaking >= 1, "an orphan breaks: {b:#?}");
}

#[test]
fn an_unchanged_contract_orphans_nothing() {
    let repo = evm_repo();
    let b = breaks(&repo);
    assert_eq!(orphans(&b), Vec::<Row>::new(), "{b:#?}");
}

#[test]
fn a_removed_eventbus_flow_is_no_orphan() {
    let repo = GitRepo::init();
    repo.write("package.json", NEST_PKG);
    repo.write("src/orders.service.ts", &orders_service("order.created"));
    repo.write("src/audit.listener.ts", AUDIT_LISTENER);
    repo.commit("P");
    // The premise: at P the eventbus pairs the emit with the listener.
    let r = generate_one(repo.path()).expect("the service builds");
    let bus = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::EVENT_FLOWS)
        .filter(|e| Evidence::of(e).is_some_and(|ev| ev.emitter == "resolver:eventbus"))
        .count();
    assert_eq!(bus, 1, "one eventbus flow at P");
    // The emit moves to another topic: the listener survives, unpaired.
    repo.write("src/orders.service.ts", &orders_service("order.placed"));
    let b = breaks(&repo);
    assert_eq!(orphans(&b), Vec::<Row>::new(), "{b:#?}");
}
