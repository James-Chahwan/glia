//! LA.36a: Swift calls on self bind the enclosing type's own method — proven on
//! a REAL build of the `swift-self-calls` fixture.
//!
//! tree-sitter-swift's `navigation_suffix` spans the dot, so before LA.36a the
//! parser read `self.helper()` as `SelfMethod(".helper")` and no Swift self
//! call ever bound. Covers `self.m()` in a class, a struct and an enum,
//! `self?.m()` inside a `[weak self]` closure and `Self.m()`. The top-level
//! `helper` is a same-name decoy: `self.helper()` must never bind it. Run with
//! `GLIA_SWIFT_DEBUG=1 ... -- --nocapture` to see the `[swift-calls]` marker.

use std::collections::HashMap;

use glia_code_domain::{edge_category, node_kind};
use glia_core::{NodeId, NodeKindId};
use glia_engine::generate_one;

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/swift-self-calls"
    )
    .to_string()
}

/// Every CALLS edge of the build as `(from qname, to qname, to kind)`, sorted
/// by qnames.
fn calls() -> Vec<(String, String, Option<NodeKindId>)> {
    let r = generate_one(&fixture()).expect("fixture builds");
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    let mut kind: HashMap<NodeId, NodeKindId> = HashMap::new();
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
        for (id, k) in &g.nav.kind_by_id {
            kind.insert(*id, *k);
        }
    }
    let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
    let mut out: Vec<(String, String, Option<NodeKindId>)> = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::CALLS)
        .map(|e| (name(&e.from), name(&e.to), kind.get(&e.to).copied()))
        .collect();
    out.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    out
}

#[test]
fn self_calls_bind_the_enclosing_types_method() {
    let calls = calls();
    for (from, to, why) in [
        ("Sources::Shop::Widget::run", "Sources::Shop::Widget::helper", "self.helper() in a class"),
        (
            "Sources::Shop::Widget::later",
            "Sources::Shop::Widget::refresh",
            "self?.refresh() in a [weak self] closure",
        ),
        ("Sources::Shop::Widget::build", "Sources::Shop::Widget::make", "Self.make()"),
        ("Sources::Shop::Part::weight", "Sources::Shop::Part::base", "self.base() in a struct"),
        ("Sources::Shop::Mode::label", "Sources::Shop::Mode::describe", "self.describe() in an enum"),
    ] {
        assert!(
            calls
                .iter()
                .any(|(f, t, k)| f == from && t == to && *k == Some(node_kind::METHOD)),
            "{why}: expected CALLS {from} -> METHOD {to}; CALLS = {calls:?}"
        );
    }
}

#[test]
fn self_call_never_binds_the_same_named_free_function() {
    let calls = calls();
    assert!(
        !calls.iter().any(|(f, t, _)| f == "Sources::Shop::Widget::run"
            && t == "Sources::Shop::Shapes::helper"),
        "self.helper() must not bind the top-level FUNCTION helper; CALLS = {calls:?}"
    );
    assert!(
        calls.iter().any(|(f, t, k)| f == "Sources::Shop::Widget::later"
            && t == "Sources::Shop::Shapes::schedule"
            && *k == Some(node_kind::FUNCTION)),
        "control: the bare schedule {{ }} call binds the free function; CALLS = {calls:?}"
    );
}
