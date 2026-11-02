//! HQ.4 — one Solidity contract name declared in two files, built for real.
//!
//! `bench/substrate-gap/fixtures/solidity-twin-contracts` copies one interface
//! into two packages (`vault/` and `lending/`), the shape zora-protocol's
//! shared and per-package interface copies have. DT.10a keys an EVM procedure
//! by its ABI name, so the build holds ONE `evm:IPriceOracle.latestPrice`
//! (one POSITION, the first-merged file's) HANDLED_BY each copy's METHOD. The
//! evidence fill used to site both edges at that one POSITION, so the edge to
//! the other copy read as asserted in the wrong file. Each edge now records
//! its own site, which the fill never overwrites: `why` reads each copy's
//! HANDLED_BY at its own file and line.
//!
//! `bench/` is outside the cargo workspace's crates, so the fixture is copied
//! into a tempdir (never its key.json) and built there.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_engine::generate_one;
use glia_engine::why::why_edge;

const PROC: &str = "evm:IPriceOracle.latestPrice";
const VAULT: &str = "vault::src::IPriceOracle::IPriceOracle::latestPrice";
const LENDING: &str = "lending::src::IPriceOracle::IPriceOracle::latestPrice";

fn fixture() -> PathBuf {
    PathBuf::from(format!(
        "{}/../bench/substrate-gap/fixtures/solidity-twin-contracts",
        env!("CARGO_MANIFEST_DIR")
    ))
}

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

/// The `why` site of the one HANDLED_BY `PROC -> to`: (file, 1-based line, basis).
fn site(m: &glia_graph::MergedGraph, to: &str) -> (String, Option<i64>, Option<&'static str>) {
    let a = why_edge(m, PROC, to, Some("HANDLED_BY")).unwrap_or_else(|e| panic!("why: {e}"));
    assert!(a.found, "{PROC} -HANDLED_BY-> {to}: {a:#?}");
    assert_eq!(a.edges.len(), 1, "{a:#?}");
    let row = &a.edges[0];
    assert_eq!(row.emitter.as_deref(), Some("parser:solidity"));
    assert_eq!(row.tier, "fact");
    let s = row.site.as_ref().expect("a sited edge");
    (s.file.clone(), s.line, row.basis)
}

#[test]
fn twin_procedure_handled_by_edges_keep_their_own_files() {
    let td = tempfile::tempdir().expect("tempdir");
    copy_tree(&fixture(), td.path());
    let r = generate_one(td.path().to_str().expect("utf-8 tempdir"))
        .expect("solidity-twin-contracts builds");
    let m = &r.merged;

    // One procedure, two HANDLED_BY edges, two distinct EVIDENCE files.
    let files: BTreeSet<String> = m
        .all_edges()
        .filter(|e| e.category == edge_category::HANDLED_BY)
        .filter(|e| {
            m.graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&e.from))
                .is_some_and(|q| q == PROC)
        })
        .map(|e| {
            Evidence::of(e)
                .and_then(|ev| ev.file)
                .expect("a sited HANDLED_BY")
        })
        .collect();
    assert_eq!(
        files.into_iter().collect::<Vec<_>>(),
        ["lending/src/IPriceOracle.sol", "vault/src/IPriceOracle.sol"]
    );

    assert_eq!(
        site(m, VAULT),
        (
            "vault/src/IPriceOracle.sol".to_string(),
            Some(6),
            Some("site")
        )
    );
    assert_eq!(
        site(m, LENDING),
        (
            "lending/src/IPriceOracle.sol".to_string(),
            Some(8),
            Some("site")
        )
    );
}
