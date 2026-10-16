//! DT.10h — contracts end to end, over REAL builds of one stack.
//!
//! `bench/substrate-gap/fixtures/evm-shop-stack` is examples-glia's
//! storefront reduced to its contract paths plus the Kina wallet / indexer /
//! signer shapes, in ONE repo with the nested projects `web` (Angular, ethers),
//! `api` (Go, go-ethereum) and `contracts` (hardhat, Solidity `Escrow` and a
//! `Vault` decoy that declares `release`, `approve` and `Released` too). Each
//! DT.10 packet graded one reader on its own fixture; this test pins the
//! whole chain at once - extract (DT.10c-k), anchor, the post-cache ABI join
//! (DT.10g), `resolver:evm` (DT.10b) and liveness - under the incremental
//! parse cache, plus what the answers say about it: `why` tiers each pairing
//! (abi derived, name_hint heuristic), an unpaired client's absence carries
//! the EVM caveats (engine/src/coverage.rs RPC_CALLS / EVENT_FLOWS rows), and
//! `contract-breaks` lists the clients a deleted Solidity function orphans.
//!
//! `bench/` is outside the cargo workspace's crates, so the fixture is copied
//! into a tempdir (never its key.json) and built there.

mod git_fixture;

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use git_fixture::GitRepo;
use glia_code_domain::{edge_category, node_kind};
use glia_core::{EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::contract_breaks::{ContractBreakArgs, contract_breaks_vs_rev};
use glia_engine::why::{EdgeWhy, why_edge};
use glia_engine::{GenerateResult, entrypoint_reachable, generate_one, generate_one_incremental};
use glia_graph::MergedGraph;

const FIXTURE: &str = "evm-shop-stack";

fn bench_fixture(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "{}/../bench/substrate-gap/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Copy a fixture's sources (never its key.json or a stray `.ai/` / `.glia/`
/// cache).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for e in std::fs::read_dir(from).expect("read fixture dir").flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            if e.file_name() != ".ai" && e.file_name() != ".glia" {
                copy_tree(&src, &dst);
            }
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).expect("copy fixture file");
        }
    }
}

fn stack() -> (tempfile::TempDir, GenerateResult) {
    let td = tempfile::tempdir().expect("tempdir");
    copy_tree(&bench_fixture(FIXTURE), td.path());
    let r =
        generate_one(td.path().to_str().expect("utf-8 tempdir")).expect("evm-shop-stack builds");
    (td, r)
}

fn key() -> serde_json::Value {
    let text = std::fs::read_to_string(bench_fixture(FIXTURE).join("key.json")).expect("key.json");
    serde_json::from_str(&text).expect("key.json parses")
}

fn field<'a>(row: &'a serde_json::Value, name: &str) -> &'a str {
    row.get(name).and_then(|v| v.as_str()).unwrap_or_default()
}

fn category(name: &str) -> EdgeCategoryId {
    edge_category::ALL
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(id, _)| *id)
        .unwrap_or_else(|| panic!("unknown edge category {name}"))
}

fn kind(name: &str) -> NodeKindId {
    node_kind::ALL
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(id, _)| *id)
        .unwrap_or_else(|| panic!("unknown node kind {name}"))
}

fn qname(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// Every distinct node id of `kind` whose qname is `q`, across graphs, in
/// graph order.
fn ids(m: &MergedGraph, kind: NodeKindId, q: &str) -> Vec<NodeId> {
    let mut seen: HashSet<NodeId> = HashSet::new();
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter(move |n| {
                g.nav.kind_by_id.get(&n.id) == Some(&kind)
                    && g.nav.qname_by_id.get(&n.id).is_some_and(|x| x == q)
            })
        })
        .map(|n| n.id)
        .filter(|id| seen.insert(*id))
        .collect()
}

fn one(m: &MergedGraph, kind_name: &str, q: &str) -> NodeId {
    let found = ids(m, kind(kind_name), q);
    assert_eq!(found.len(), 1, "one {kind_name} `{q}`: {found:?}");
    found.into_iter().next().expect("one id")
}

/// `(from qname, to qname)` of every edge of `category`, sorted.
fn edges(m: &MergedGraph, category: EdgeCategoryId) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = m
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| (qname(m, e.from), qname(m, e.to)))
        .collect();
    out.sort();
    out
}

/// The one `category` edge `why` explains from `from` to `to`.
fn why_one(m: &MergedGraph, from: &str, to: &str, category: &str) -> EdgeWhy {
    let a =
        why_edge(m, from, to, Some(category)).unwrap_or_else(|e| panic!("why {from} -> {to}: {e}"));
    assert!(a.found, "{from} -{category}-> {to}: {a:#?}");
    assert_eq!(a.edges.len(), 1, "{a:#?}");
    a.edges.into_iter().next().expect("one edge")
}

/// The `.gmap` layout bytes of a build, file by file.
fn gmap(td: &Path, r: &GenerateResult, name: &str) -> Vec<(String, Vec<u8>)> {
    let out = td.join(name);
    glia_store::write_merged_sharded(&r.merged, &out).expect("write .gmap");
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&out)
        .expect("read layout")
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).expect("read"),
            )
        })
        .collect();
    files.sort();
    files
}

#[test]
fn every_key_row_holds_and_no_forbid_matches() {
    let (_td, r) = stack();
    let m = &r.merged;
    let key = key();
    let rows = |name: &str| {
        key.get(name)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };

    let expect_nodes = rows("expect_nodes");
    assert_eq!(expect_nodes.len(), 7);
    for row in &expect_nodes {
        one(m, field(row, "kind"), field(row, "name"));
    }
    let expect_edges = rows("expect_edges");
    assert_eq!(expect_edges.len(), 11);
    for row in &expect_edges {
        let (from, to, cat) = (field(row, "from"), field(row, "to"), field(row, "category"));
        let pair = (from.to_string(), to.to_string());
        let n = edges(m, category(cat))
            .iter()
            .filter(|e| **e == pair)
            .count();
        assert_eq!(n, 1, "{from} -{cat}-> {to}: {:#?}", edges(m, category(cat)));
    }

    let forbid = rows("forbid");
    assert_eq!(forbid.len(), 6);
    for row in &forbid {
        if row.get("from").is_some() {
            let (from, to, cat) = (field(row, "from"), field(row, "to"), field(row, "category"));
            let pair = (from.to_string(), to.to_string());
            assert!(
                !edges(m, category(cat)).contains(&pair),
                "forbidden {from} -{cat}-> {to}"
            );
            continue;
        }
        let k = kind(field(row, "kind"));
        let all = m
            .graphs
            .iter()
            .flat_map(|g| {
                g.nodes
                    .iter()
                    .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&k))
            })
            .count();
        match row.get("max_nodes").and_then(|v| v.as_u64()) {
            Some(max) => assert!(all as u64 <= max, "{} {all} > {max}", field(row, "kind")),
            None => assert!(
                ids(m, k, field(row, "name")).is_empty(),
                "forbidden {}",
                field(row, "name")
            ),
        }
    }

    // The RPC_CALL nodes in full: the five calls, ERC20's the unmatched one
    // (Vault declares approve but not balanceOf, and the ABI lists both).
    let calls: BTreeSet<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter_map(move |n| {
                (g.nav.kind_by_id.get(&n.id) == Some(&node_kind::RPC_CALL))
                    .then(|| g.nav.qname_by_id.get(&n.id).cloned())
                    .flatten()
            })
        })
        .collect();
    assert_eq!(
        calls.into_iter().collect::<Vec<_>>(),
        [
            "evm_call:abi:ERC20_ABI.approve",
            "evm_call:abi:ESCROW_ABI.deposit",
            "evm_call:abi:WALLET_ESCROW_ABI.confirm",
            "evm_call:abi:shopEscrowABI.confirm",
            "evm_call:recv:escrow.release",
        ]
    );
    assert_eq!(
        edges(m, edge_category::RPC_CALLS),
        [
            (
                "evm_call:abi:ESCROW_ABI.deposit".into(),
                "evm:Escrow.deposit".into()
            ),
            (
                "evm_call:abi:WALLET_ESCROW_ABI.confirm".into(),
                "evm:Escrow.confirm".into()
            ),
            (
                "evm_call:abi:shopEscrowABI.confirm".into(),
                "evm:Escrow.confirm".into()
            ),
            (
                "evm_call:recv:escrow.release".into(),
                "evm:Escrow.release".into()
            ),
        ],
        "four calls pair, ERC20_ABI.approve pairs nothing"
    );
}

#[test]
fn no_contract_edge_touches_the_vault_decoy() {
    let (_td, r) = stack();
    let m = &r.merged;
    // The one RPC_CALLS / EVENT_FLOWS edge at Vault is its own `emit`
    // (release -> Released, the Solidity parser's): no client pairs it.
    let touching: Vec<(&str, String, String)> =
        [edge_category::RPC_CALLS, edge_category::EVENT_FLOWS]
            .into_iter()
            .flat_map(|cat| {
                edges(m, cat)
                    .into_iter()
                    .filter(|(from, to)| from.contains("Vault") || to.contains("Vault"))
                    .map(move |(from, to)| (edge_category::name(cat), from, to))
            })
            .collect();
    assert_eq!(
        touching,
        [(
            "EVENT_FLOWS",
            "contracts::contracts::Vault::Vault::release".to_string(),
            "contracts::contracts::Vault::Vault::Released".to_string(),
        )]
    );
    // The decoy's procedures exist, so the pick between Escrow and Vault was
    // made, not skipped.
    one(m, "RPC_PROCEDURE", "evm:Vault.release");
    one(m, "RPC_PROCEDURE", "evm:Vault.approve");
}

#[test]
fn why_tiers_each_pairing_and_absences_carry_the_evm_caveats() {
    let (_td, r) = stack();
    let m = &r.merged;
    // ABI-identified: derived, emitter resolver:evm, rule abi (DT.10b's
    // handoff), on both sides of the channel.
    for (from, to, cat) in [
        (
            "evm_call:abi:ESCROW_ABI.deposit",
            "evm:Escrow.deposit",
            "RPC_CALLS",
        ),
        (
            "evm_call:abi:WALLET_ESCROW_ABI.confirm",
            "evm:Escrow.confirm",
            "RPC_CALLS",
        ),
        (
            "evm_call:abi:shopEscrowABI.confirm",
            "evm:Escrow.confirm",
            "RPC_CALLS",
        ),
        (
            "contracts::contracts::Escrow::Escrow::Released",
            "evm_event:abi:escrowABI.Released",
            "EVENT_FLOWS",
        ),
        (
            "contracts::contracts::Escrow::Escrow::Deposited",
            "evm_event:abi:escrowABI.Deposited",
            "EVENT_FLOWS",
        ),
    ] {
        let w = why_one(m, from, to, cat);
        assert_eq!(
            (w.tier, w.emitter.as_deref(), w.rule.as_deref()),
            ("derived", Some("resolver:evm"), Some("abi")),
            "{from} -> {to}: {w:#?}"
        );
    }
    // A receiver named like the contract, no ABI: heuristic.
    let w = why_one(
        m,
        "evm_call:recv:escrow.release",
        "evm:Escrow.release",
        "RPC_CALLS",
    );
    assert_eq!(
        (
            w.tier,
            w.emitter.as_deref(),
            w.rule.as_deref(),
            w.note.as_deref()
        ),
        (
            "heuristic",
            Some("resolver:evm"),
            Some("name_hint"),
            Some("name-matched contract (name_hint)")
        ),
        "{w:#?}"
    );
    assert_eq!(w.confidence, "weak", "{w:#?}");

    // The unpaired ERC20 call: an absence whose RPC_CALLS caveat states the
    // EVM reading.
    let a = why_edge(
        m,
        "evm_call:abi:ERC20_ABI.approve",
        "evm:Vault.approve",
        Some("RPC_CALLS"),
    )
    .expect("both sides name a node");
    assert!(!a.found, "{a:#?}");
    let absence = a.absence.expect("no edge, an absence");
    let rpc: Vec<_> = absence
        .caveats
        .iter()
        .filter(|c| c.edge_category == "RPC_CALLS")
        .collect();
    assert_eq!(rpc.len(), 1, "{:#?}", absence.caveats);
    assert!(rpc[0].note.contains("EVM contract call"), "{}", rpc[0].note);
    assert!(
        rpc[0]
            .note
            .contains("never pairs on the function name alone"),
        "{}",
        rpc[0].note
    );

    // The decoy's same-named event: an EVENT_FLOWS absence names the contract
    // event subscriptions.
    let a = why_edge(
        m,
        "contracts::contracts::Vault::Vault::Released",
        "evm_event:abi:escrowABI.Released",
        Some("EVENT_FLOWS"),
    )
    .expect("both sides name a node");
    assert!(!a.found, "{a:#?}");
    let absence = a.absence.expect("no edge, an absence");
    let ev: Vec<_> = absence
        .caveats
        .iter()
        .filter(|c| c.edge_category == "EVENT_FLOWS")
        .collect();
    assert_eq!(ev.len(), 1, "{:#?}", absence.caveats);
    assert!(
        ev[0].note.contains("contract event subscription"),
        "{}",
        ev[0].note
    );
}

#[test]
fn liveness_reaches_the_called_contract_and_the_indexer() {
    let (_td, r) = stack();
    let m = &r.merged;
    let live: HashSet<NodeId> = entrypoint_reachable(m);
    for (k, q) in [
        ("METHOD", "contracts::contracts::Escrow::Escrow::deposit"),
        ("METHOD", "contracts::contracts::Escrow::Escrow::confirm"),
        ("METHOD", "contracts::contracts::Escrow::Escrow::release"),
        ("CLASS", "contracts::contracts::Escrow::Escrow"),
        ("METHOD", "api::internal::indexer::indexer::Indexer::Handle"),
    ] {
        assert!(live.contains(&one(m, k, q)), "{k} {q} is live");
    }
    // examples-glia's "truly unused" row: nothing calls Settler.Release or
    // Payout, so no carry edge enters them and they stay unreached, though
    // Release's own call pairs Escrow.release.
    let carry: Vec<EdgeCategoryId> = glia_code_domain::profile::CODE_TABLES.carry_edges.to_vec();
    for q in [
        "api::internal::chain::escrow::Settler::Release",
        "api::internal::chain::escrow::Settler::Payout",
    ] {
        let id = one(m, "METHOD", q);
        let incoming: Vec<(String, &str)> = m
            .all_edges()
            .filter(|e| e.to == id && carry.contains(&e.category))
            .map(|e| (qname(m, e.from), edge_category::name(e.category)))
            .collect();
        assert!(incoming.is_empty(), "{q}: {incoming:?}");
        assert!(!live.contains(&id), "{q} stays unreached");
    }
}

#[test]
fn incremental_builds_match_a_clean_build() {
    let td = tempfile::tempdir().expect("tempdir");
    let repo = td.path().join("shop");
    copy_tree(&bench_fixture(FIXTURE), &repo);
    let path = repo.to_str().expect("utf-8 tempdir");
    let cold = generate_one_incremental(path).expect("cold incremental");
    let warm = generate_one_incremental(path).expect("warm incremental");
    let clean = generate_one(path).expect("clean");
    assert_eq!(
        edges(&warm.merged, edge_category::RPC_CALLS).len(),
        4,
        "the cache-served parses pair as the cold ones do"
    );
    assert_eq!(
        edges(&warm.merged, edge_category::EVENT_FLOWS),
        edges(&clean.merged, edge_category::EVENT_FLOWS)
    );
    let clean_bytes = gmap(td.path(), &clean, "clean");
    assert_eq!(
        gmap(td.path(), &cold, "cold"),
        clean_bytes,
        "cold incremental == clean"
    );
    assert_eq!(
        gmap(td.path(), &warm, "warm"),
        clean_bytes,
        "warm incremental == clean"
    );
}

#[test]
fn deleting_a_called_function_orphans_its_clients() {
    let repo = GitRepo::init();
    copy_tree(&bench_fixture(FIXTURE), repo.root());
    repo.commit("evm-shop-stack");
    let escrow = "contracts/contracts/Escrow.sol";
    let text = std::fs::read_to_string(repo.root().join(escrow)).expect("Escrow.sol");
    let confirm = "    function confirm(bytes32 orderId) external {\n        require(deposits[orderId] > 0, \"none\");\n    }\n\n";
    assert!(text.contains(confirm), "{text}");
    repo.write(escrow, &text.replace(confirm, ""));

    let b = contract_breaks_vs_rev(repo.path(), "HEAD", &ContractBreakArgs::default())
        .unwrap_or_else(|e| panic!("contract breaks vs HEAD: {e}"));
    let rows: Vec<(
        String,
        &str,
        String,
        &str,
        &str,
        Option<String>,
        Option<i64>,
    )> = b
        .orphaned_clients
        .iter()
        .map(|o| {
            (
                o.client_qname.clone(),
                o.category,
                o.target_qname.clone(),
                o.reason,
                o.tier,
                o.file.clone(),
                o.line,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                "evm_call:abi:WALLET_ESCROW_ABI.confirm".to_string(),
                "RPC_CALLS",
                "evm:Escrow.confirm".to_string(),
                "target_removed",
                "fact",
                Some("web/src/app/core/services/wallet.service.ts".to_string()),
                Some(21),
            ),
            (
                "evm_call:abi:shopEscrowABI.confirm".to_string(),
                "RPC_CALLS",
                "evm:Escrow.confirm".to_string(),
                "target_removed",
                "fact",
                Some("api/internal/signer/signer.go".to_string()),
                Some(30),
            ),
        ],
        "{b:#?}"
    );
    assert_eq!(b.breaking, 2, "{b:#?}");
    assert!(b.schemas.is_empty(), "{:#?}", b.schemas);
    assert!(b.absence.is_none());
}

#[test]
fn moving_the_contract_or_deleting_the_decoy_orphans_nothing() {
    // A procedure's qname names its contract, not its file, so a moved
    // Escrow.sol keeps every client paired; no client pairs Vault, so
    // deleting it breaks nothing.
    let repo = GitRepo::init();
    copy_tree(&bench_fixture(FIXTURE), repo.root());
    repo.commit("evm-shop-stack");
    std::fs::create_dir_all(repo.root().join("contracts/contracts/shop")).expect("mkdir shop/");
    repo.git_mv(
        "contracts/contracts/Escrow.sol",
        "contracts/contracts/shop/Escrow.sol",
    );
    repo.remove("contracts/contracts/Vault.sol");

    let b = contract_breaks_vs_rev(repo.path(), "HEAD", &ContractBreakArgs::default())
        .unwrap_or_else(|e| panic!("contract breaks vs HEAD: {e}"));
    assert!(b.orphaned_clients.is_empty(), "{b:#?}");
    assert!(b.schemas.is_empty(), "{:#?}", b.schemas);
    assert_eq!(b.breaking, 0);
    assert!(b.absence.is_some(), "nothing broke, an absence says so");
}
