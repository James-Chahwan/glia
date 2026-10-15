//! LA.36a: Swift calls on self bind the enclosing type's own method — proven on
//! a REAL build of the `swift-self-calls` fixture.
//!
//! tree-sitter-swift's `navigation_suffix` spans the dot, so before LA.36a the
//! parser read `self.helper()` as `SelfMethod(".helper")` and no Swift self
//! call ever bound. Covers `self.m()` in a class, a struct and an enum,
//! `self?.m()` inside a `[weak self]` closure and `Self.m()`. The top-level
//! `helper` is a same-name decoy: `self.helper()` must never bind it. Run with
//! `GLIA_SWIFT_DEBUG=1 ... -- --nocapture` to see the `[swift-calls]` marker.
//!
//! DF.3: parameter and local receivers, on a real build of the
//! `swift-local-receivers` fixture (`Store` declared in two targets).

use std::collections::HashMap;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{edge_category, node_kind};
use glia_core::{NodeId, NodeKindId};
use glia_engine::generate_one;

fn fixture(name: &str) -> String {
    format!("{}/../bench/substrate-gap/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Every CALLS edge of the `swift-self-calls` build as `(from qname, to
/// qname, to kind)`, sorted by qnames.
fn calls() -> Vec<(String, String, Option<NodeKindId>)> {
    calls_with_rule("swift-self-calls")
        .into_iter()
        .map(|(from, to, kind, _)| (from, to, kind))
        .collect()
}

/// Every CALLS edge of the build of fixture `name` as `(from qname, to
/// qname, to kind, EVIDENCE rule)`, sorted by qnames.
fn calls_with_rule(name: &str) -> Vec<(String, String, Option<NodeKindId>, String)> {
    let r = generate_one(&fixture(name)).expect("fixture builds");
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
    let mut out: Vec<(String, String, Option<NodeKindId>, String)> = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::CALLS)
        .map(|e| {
            let rule = Evidence::of(e).and_then(|ev| ev.rule).unwrap_or_default();
            (name(&e.from), name(&e.to), kind.get(&e.to).copied(), rule)
        })
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

/// DF.3: `store.save(1)` on a method parameter, `s.save(2)` / `t.load()` /
/// `u.load()` / `o?.save(3)` on locals (`let s = Store()`, `let t: Store`,
/// `var u = Store.init()`, `let o: Store?`) and `store.save(4)` on a free
/// function's `_ store` parameter bind the App target's `Store` members
/// (rule `local_receiver`, or `receiver_type` where the generic pass already
/// resolves the type), and none reaches the Admin target's `Store`.
#[test]
fn params_and_locals_bind_through_the_target() {
    let calls = calls_with_rule("swift-local-receivers");
    let into_store: Vec<_> = calls
        .iter()
        .filter(|(f, t, _, _)| {
            f.starts_with("Sources::App::Service::")
                && t.contains("::Store::")
                && t != "Sources::App::Store::make"
        })
        .collect();
    for (from, to) in [
        ("Sources::App::Service::run", "Sources::App::Store::save"),
        ("Sources::App::Service::local", "Sources::App::Store::save"),
        ("Sources::App::Service::local", "Sources::App::Store::load"),
        ("Sources::App::Service::freeRun", "Sources::App::Store::save"),
    ] {
        assert!(
            into_store.iter().any(|(f, t, k, rule)| f == from
                && t == to
                && *k == Some(node_kind::METHOD)
                && (rule == "local_receiver" || rule == "receiver_type")),
            "expected CALLS {from} -> METHOD {to} (local_receiver / receiver_type); CALLS = {calls:?}"
        );
    }
    // One edge per call site: run 1, local 4 (`s.save`, `t.load`, `u.load`,
    // `o?.save`), freeRun 1.
    assert_eq!(into_store.len(), 6, "one edge per call site; CALLS = {calls:?}");
    assert!(
        !calls.iter().any(|(_, t, _, _)| t.starts_with("Sources::Admin::")),
        "nothing binds the Admin target's Store; CALLS = {calls:?}"
    );
}
