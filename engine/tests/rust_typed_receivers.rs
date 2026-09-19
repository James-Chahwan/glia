//! LA.35a: a Rust method call on a value (`r.find()`) binds through the
//! value's type. The parser records struct field types (`field_types`) and,
//! per fn, the types of its parameters and `let`s (`local_types`); the
//! generic receiver pass (graph/src/calls.rs, A6.2a) reads the caller's
//! locals before the enclosing type's fields. `x.m()` on a lowercase value is
//! a ComplexReceiver, so it is never read as the path call `x::m()`.
//!
//! Run with `-- --nocapture` to see the `[recv]` marker's `rust=` tokens.
//!
//! `Service::cached -> Cache::lookup` (an `impl` in another file than its
//! type) is LA.35b's row and is not asserted here.

use std::collections::HashMap;

use repo_graph_code_domain::edge_category;
use repo_graph_core::NodeId;
use repo_graph_engine::{GenerateResult, generate_one};

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/rust-typed-receivers"
    )
    .to_string()
}

/// Every CALLS edge of the build as `(from qname, to qname)`, sorted, with
/// duplicates kept (one edge per call site).
fn calls(r: &GenerateResult) -> Vec<(String, String)> {
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
    }
    let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
    let mut out: Vec<(String, String)> = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::CALLS)
        .map(|e| (name(&e.from), name(&e.to)))
        .collect();
    out.sort();
    out
}

fn count(calls: &[(String, String)], from: &str, to: &str) -> usize {
    calls.iter().filter(|(f, t)| f == from && t == to).count()
}

#[test]
fn typed_receivers_bind_through_the_receiver_type() {
    let r = generate_one(&fixture()).expect("fixture builds");
    let calls = calls(&r);
    for (from, to, form) in [
        ("src::lib::Service::get", "src::repo::Repo::find", "self.repo.find() on field repo: Repo"),
        ("src::lib::Service::via_param", "src::repo::Repo::find", "r.find() on parameter r: &Repo"),
        ("src::lib::Service::via_local", "src::repo::Repo::find", "let r = Repo::new()"),
        ("src::lib::Service::via_typed", "src::repo::Repo::find", "let r: Repo = .."),
        ("src::lib::free", "src::repo::Repo::find", "free-fn parameter r: &mut Repo"),
        ("src::lib::sized", "src::repo::Index::size", "parameter repo: &repo::Index"),
        (
            "src::lib::Service::unit_value",
            "src::repo::Repo::find",
            "CONTROL: Repo.find(5) stays an Attribute on the unit-struct value",
        ),
    ] {
        assert_eq!(count(&calls, from, to), 1, "{form}: {from} -> {to}; CALLS = {calls:?}");
    }
}

#[test]
fn a_value_receiver_is_never_a_field_or_a_path() {
    let r = generate_one(&fixture()).expect("fixture builds");
    let calls = calls(&r);
    // `let repo = index()` has no type the parser can read; it shadows the
    // field `repo: Repo`, so `repo.find(9)` binds nothing.
    assert_eq!(
        count(&calls, "src::lib::Service::shadowed", "src::repo::Repo::find"),
        0,
        "the untyped local shadows the field; CALLS = {calls:?}"
    );
    assert_eq!(
        count(&calls, "src::lib::Service::shadowed", "src::repo::index"),
        1,
        "the bare call index() still binds; CALLS = {calls:?}"
    );
    // `repo.size()` is a method call on the parameter, not the path call
    // `repo::size()` into the imported module `repo`.
    assert_eq!(
        count(&calls, "src::lib::sized", "src::repo::size"),
        0,
        "a method call on a value must not bind the module fn; CALLS = {calls:?}"
    );
}
