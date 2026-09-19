//! LA.30a: an ENUM owns its methods in the generic resolver, exactly like a
//! CLASS / STRUCT — proven on a REAL build of the `enum-owned-methods` fixture
//! (a Rust `impl Tier` self-call beside the identical `impl Ledger` control).
//!
//! Before LA.30a `enclosing_class_or_struct` stopped only at CLASS / STRUCT and
//! `build_symbol_table` indexed only their METHOD children, so
//! `self.weight()` inside `impl Tier` resolved nowhere while the struct twin
//! did. The two forbids pin precision: each self-call binds its OWN type's
//! `weight`, never the same-named method of the other type. Run with
//! `-- --nocapture` to see the `[resolve] enum-owned calls bound:` marker.

use std::collections::HashMap;

use glia_code_domain::edge_category;
use glia_core::NodeId;
use glia_engine::generate_one;

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/enum-owned-methods"
    )
    .to_string()
}

/// Every CALLS edge of the build as `(from qname, to qname)`, sorted.
fn calls() -> Vec<(String, String)> {
    let r = generate_one(&fixture()).expect("fixture builds");
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
    }
    let mut out: Vec<(String, String)> = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::CALLS)
        .map(|e| {
            let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
            (name(&e.from), name(&e.to))
        })
        .collect();
    out.sort();
    out
}

fn has(calls: &[(String, String)], from: &str, to: &str) -> bool {
    calls.iter().any(|(f, t)| f == from && t == to)
}

#[test]
fn enum_impl_self_call_binds_the_enums_own_method() {
    let calls = calls();
    assert!(
        has(&calls, "src::lib::Tier::rank", "src::lib::Tier::weight"),
        "self.weight() inside `impl Tier` must bind Tier::weight; CALLS = {calls:?}"
    );
    assert!(
        has(&calls, "src::lib::Ledger::total", "src::lib::Ledger::weight"),
        "control: the struct twin resolves; CALLS = {calls:?}"
    );
}

#[test]
fn enum_and_struct_self_calls_never_cross() {
    let calls = calls();
    assert!(
        !has(&calls, "src::lib::Tier::rank", "src::lib::Ledger::weight"),
        "the enum's self-call must not bind the struct's weight; CALLS = {calls:?}"
    );
    assert!(
        !has(&calls, "src::lib::Ledger::total", "src::lib::Tier::weight"),
        "indexing ENUM methods must not leak them into a struct's self-calls; CALLS = {calls:?}"
    );
}
