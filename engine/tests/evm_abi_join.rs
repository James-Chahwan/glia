//! DT.10g — the cross-file ABI join is post-cache, over REAL builds.
//!
//! One repo holds a `web/` project whose `src/pay.ts` builds an ethers
//! contract from `SHOP_ABI`, imported from `./abi`, and a `contracts/` project
//! with the Solidity `Escrow` and `Vault` contracts; `src/legacy-abi.ts`
//! exports a Vault-shaped `SHOP_ABI` that pay.ts does not import. The join
//! fills the client cells from abi.ts, so `resolver:evm` pairs them to Escrow
//! by ABI (rule `abi`). An edit to abi.ts alone must re-join the cache-served
//! pay.ts: the warm incremental build follows the edit and writes the same
//! `.gmap` bytes as a clean build of the edited tree.

use std::path::Path;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{edge_category, evm};
use glia_core::NodeId;
use glia_engine::{
    GenerateResult, ParseCache, generate_one, generate_one_incremental, generate_one_with_cache,
};
use glia_graph::MergedGraph;

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    std::fs::write(path, text).expect("write fixture file");
}

/// The tree: the fixture's TS half, Escrow with a third function so an
/// edited ABI can still name it (`refund`).
fn shop(dir: &Path) {
    write(
        dir,
        "contracts/package.json",
        "{ \"name\": \"escrow-contracts\", \"private\": true }\n",
    );
    write(
        dir,
        "contracts/contracts/Escrow.sol",
        "// SPDX-License-Identifier: MIT\npragma solidity ^0.8.24;\n\ncontract Escrow {\n    \
         function deposit(bytes32 orderId) external payable {}\n\n    \
         function release(bytes32 orderId) external {}\n\n    \
         function refund(bytes32 orderId) external {}\n}\n",
    );
    write(
        dir,
        "contracts/contracts/Vault.sol",
        "// SPDX-License-Identifier: MIT\npragma solidity ^0.8.24;\n\ncontract Vault {\n    \
         function release(bytes32 id) external {}\n\n    \
         function withdraw(uint256 amount) external {}\n}\n",
    );
    write(
        dir,
        "web/package.json",
        "{ \"name\": \"web\", \"dependencies\": { \"ethers\": \"^6.13.0\" } }\n",
    );
    abi(
        dir,
        &[
            "function deposit(bytes32 orderId) payable",
            "function release(bytes32 orderId)",
        ],
    );
    write(
        dir,
        "web/src/legacy-abi.ts",
        "export const SHOP_ABI = [\n  'function release(bytes32 id)',\n  'function withdraw(uint256 amount)',\n];\n",
    );
    write(
        dir,
        "web/src/pay.ts",
        "import { Contract, type Signer } from 'ethers';\nimport { SHOP_ABI } from './abi';\n\n\
         export async function payAndRelease(address: string, signer: Signer, id: string) {\n  \
         const shop = new Contract(address, SHOP_ABI, signer);\n  \
         await shop.deposit(id, { value: 1n });\n  \
         return shop.release(id);\n}\n",
    );
}

/// web/src/abi.ts exporting `SHOP_ABI` with these fragments.
fn abi(dir: &Path, fragments: &[&str]) {
    let body: String = fragments.iter().map(|f| format!("  '{f}',\n")).collect();
    write(
        dir,
        "web/src/abi.ts",
        &format!("export const SHOP_ABI = [\n{body}];\n"),
    );
}

fn qname(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// `(from, to, rule)` of every RPC_CALLS edge, sorted.
fn rpc_calls(r: &GenerateResult) -> Vec<(String, String, String)> {
    let m = &r.merged;
    let mut out: Vec<(String, String, String)> = m
        .all_edges()
        .filter(|e| e.category == edge_category::RPC_CALLS)
        .map(|e| {
            let rule = Evidence::of(e).and_then(|ev| ev.rule).unwrap_or_default();
            (qname(m, e.from), qname(m, e.to), rule)
        })
        .collect();
    out.sort();
    out
}

/// The evm client hits of the node `q`, over every graph copy.
fn hits(r: &GenerateResult, q: &str) -> Vec<evm::ClientHit> {
    r.merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.qname_by_id.get(&n.id).is_some_and(|x| x == q))
        })
        .flat_map(|n| evm::read_client_hits(&n.cells))
        .collect()
}

fn row(from: &str, to: &str) -> (String, String, String) {
    (from.into(), to.into(), "abi".into())
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
fn an_abi_edit_rejoins_a_cache_served_client() {
    let td = tempfile::tempdir().expect("tempdir");
    let repo = td.path().join("shop");
    shop(&repo);
    let path = repo.to_str().expect("utf-8 tempdir");

    // Cold: the client is joined to abi.ts, never to legacy-abi.ts.
    let cold = generate_one_incremental(path).expect("cold incremental");
    assert_eq!(
        rpc_calls(&cold),
        [
            row("evm_call:abi:SHOP_ABI.deposit", "evm:Escrow.deposit"),
            row("evm_call:abi:SHOP_ABI.release", "evm:Escrow.release"),
        ],
        "identified by the imported ABI; the Vault-shaped legacy export is not followed"
    );
    let release = hits(&cold, "evm_call:abi:SHOP_ABI.release");
    assert_eq!(release.len(), 1, "{release:?}");
    assert_eq!(release[0].abi_from.as_deref(), Some("web/src/abi.ts"));
    assert_eq!(
        release[0].abi_ref,
        Some(evm::AbiRef {
            name: "SHOP_ABI".into(),
            from: "./abi".into()
        })
    );
    let mut mem = ParseCache::new();
    generate_one_with_cache(path, &mut mem).expect("cold in-memory");

    // Edit abi.ts alone: `release` leaves the ABI, `refund` joins it.
    abi(
        &repo,
        &[
            "function deposit(bytes32 orderId) payable",
            "function refund(bytes32 orderId)",
        ],
    );
    let warm = generate_one_incremental(path).expect("warm incremental");
    let warm_mem = generate_one_with_cache(path, &mut mem).expect("warm in-memory");
    let diff = mem.last_diff().expect("the warm build recorded a diff");
    assert!(
        diff.reused.iter().any(|p| p == "web/src/pay.ts"),
        "pay.ts is cache-served: {diff:?}"
    );
    assert_eq!(
        diff.reparsed,
        ["web/src/abi.ts"],
        "only the declaring file is reparsed"
    );
    assert_eq!(
        rpc_calls(&warm),
        [row("evm_call:abi:SHOP_ABI.deposit", "evm:Escrow.deposit")],
        "the cache-served release call keeps no RPC_CALLS: the joined ABI no longer lists it (not_in_abi)"
    );
    assert_eq!(
        hits(&warm, "evm_call:abi:SHOP_ABI.release")[0].facts.abi,
        Some(vec!["f:deposit/1".to_string(), "f:refund/1".to_string()]),
        "the cell carries the edited ABI"
    );

    // A clean build of the edited tree writes the same bytes.
    let clean = generate_one(path).expect("clean");
    let clean_bytes = gmap(td.path(), &clean, "clean");
    assert_eq!(
        gmap(td.path(), &warm, "warm"),
        clean_bytes,
        "warm incremental == clean"
    );
    assert_eq!(
        gmap(td.path(), &warm_mem, "warm_mem"),
        clean_bytes,
        "cache-served == clean"
    );
}
