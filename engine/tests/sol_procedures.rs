//! DT.10a — a Solidity contract's external surface is live, over a REAL build.
//!
//! `bench/substrate-gap/fixtures/solidity-abi-procedures`: an `IEscrow`
//! interface, a `Fees` library and an `Escrow` contract whose `deposit` /
//! `release` (two overloads) are external / public, whose `_credit` is
//! internal and whose `seller` / `deposits` are public state. Before DT.10a
//! no node stood for what an ABI client can call, no entry kind reached a
//! contract, and `Escrow`, `deposit` and `release` were dead_symbol rows.
//! Every reachable member is now an RPC_PROCEDURE `evm:<Contract>.<member>`
//! (an entry kind) HANDLED_BY its METHOD / STATE_VAR, so liveness reaches the
//! functions, their contract (the A7.8 owner hop, and the implementer hop from
//! the live `IEscrow`) and what they call, and leaves the unused library
//! function dead. `bench/` is outside the cargo workspace's crates, so the
//! fixture is copied into a tempdir (the ts_config_alias.rs way).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use glia_code_domain::{edge_category, evm, node_kind};
use glia_core::{NodeId, NodeKindId};
use glia_engine::{entrypoint_reachable, generate_one};
use glia_graph::MergedGraph;

fn fixture() -> PathBuf {
    PathBuf::from(format!(
        "{}/../bench/substrate-gap/fixtures/solidity-abi-procedures",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Copy the fixture's sources (never its key.json or a stray cache).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            if e.file_name() != ".ai" && e.file_name() != ".glia" {
                copy_tree(&src, &dst);
            }
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

fn build() -> (tempfile::TempDir, MergedGraph) {
    let td = tempfile::tempdir().unwrap();
    copy_tree(&fixture(), td.path());
    let m = generate_one(td.path().to_str().unwrap()).expect("solidity-abi-procedures builds").merged;
    (td, m)
}

/// The id of the node of `kind` named `qname`, in whichever graph holds it.
fn id_of(m: &MergedGraph, kind: NodeKindId, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .find_map(|g| {
            g.nav.qname_by_id.iter().find_map(|(id, q)| {
                (q == qname && g.nav.kind_by_id.get(id) == Some(&kind)).then_some(*id)
            })
        })
        .unwrap_or_else(|| panic!("no {qname}"))
}

fn qname_of(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

#[test]
fn contract_surface_is_live_and_library_stays_dead() {
    let (_td, m) = build();
    let live = entrypoint_reachable(&m);
    for (kind, qname) in [
        (node_kind::METHOD, "contracts::Escrow::Escrow::deposit"),
        (node_kind::METHOD, "contracts::Escrow::Escrow::release"),
        (node_kind::CLASS, "contracts::Escrow::Escrow"),
        (node_kind::METHOD, "contracts::Escrow::Escrow::_credit"),
        (node_kind::INTERFACE, "contracts::Escrow::IEscrow"),
        (node_kind::RPC_PROCEDURE, "evm:Escrow.deposit"),
    ] {
        assert!(live.contains(&id_of(&m, kind, qname)), "{qname} should be live");
    }
    let cut = id_of(&m, node_kind::METHOD, "contracts::Escrow::Fees::cut");
    assert!(!live.contains(&cut), "an unused library function stays unreached");
}

#[test]
fn procedures_are_handled_by_their_members() {
    let (_td, m) = build();
    let procedures: BTreeSet<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .kind_by_id
                .iter()
                .filter(|(_, k)| **k == node_kind::RPC_PROCEDURE)
                .filter_map(|(id, _)| g.nav.qname_by_id.get(id).cloned())
        })
        .collect();
    assert_eq!(
        procedures.into_iter().collect::<Vec<_>>(),
        [
            "evm:Escrow.deposit",
            "evm:Escrow.deposits",
            "evm:Escrow.release",
            "evm:Escrow.seller",
            "evm:IEscrow.deposit",
        ]
    );
    let handled: BTreeSet<(String, String)> = m
        .all_edges()
        .filter(|e| e.category == edge_category::HANDLED_BY)
        .map(|e| (qname_of(&m, e.from), qname_of(&m, e.to)))
        .filter(|(from, _)| evm::split_procedure(from).is_some())
        .collect();
    let want: BTreeSet<(String, String)> = [
        ("evm:Escrow.deposit", "contracts::Escrow::Escrow::deposit"),
        ("evm:Escrow.deposits", "contracts::Escrow::Escrow::deposits"),
        ("evm:Escrow.release", "contracts::Escrow::Escrow::release"),
        ("evm:Escrow.seller", "contracts::Escrow::Escrow::seller"),
        ("evm:IEscrow.deposit", "contracts::Escrow::IEscrow::deposit"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(handled, want);

    // The facts survive the build: both overloads' arities on one node.
    let release = id_of(&m, node_kind::RPC_PROCEDURE, "evm:Escrow.release");
    let cells: Vec<_> = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter(|n| n.id == release))
        .flat_map(|n| n.cells.iter().cloned())
        .collect();
    let facts = evm::read_procedure_cell(&cells).expect("evm CODE cell");
    assert_eq!(facts.arities, [1, 2]);
    assert_eq!(facts.mutability, evm::NONPAYABLE);
}
