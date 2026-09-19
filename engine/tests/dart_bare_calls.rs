//! LA.34: an unqualified Dart call inside a class member follows Dart's lexical
//! scope — proven on a REAL build of the `dart-bare-calls` fixture and the
//! committed `matrix/dart/calls` probe.
//!
//! Dart resolves a bare `f(..)` against the member's locals first, then the
//! enclosing class's own members (instance and static), then library scope.
//! The graph's Bare arm only knows library scope, so the parser classifies:
//! a member name becomes a SelfMethod, a local emits nothing. The fixture's
//! top-level `add` shares a member's name and its `param` method takes a
//! parameter named `add`: neither may bind the top-level function.

use std::collections::HashMap;

use glia_code_domain::{edge_category, node_kind};
use glia_core::{NodeId, NodeKindId};
use glia_engine::generate_one;

fn bench(rel: &str) -> String {
    format!("{}/../bench/substrate-gap/{rel}", env!("CARGO_MANIFEST_DIR"))
}

/// Every CALLS edge of a build as `(from qname, to qname, to kind)`, sorted.
fn calls(dir: &str) -> Vec<(String, String, Option<NodeKindId>)> {
    let r = generate_one(&bench(dir)).expect("fixture builds");
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
fn bare_calls_follow_dart_lexical_scope() {
    let calls = calls("fixtures/dart-bare-calls");
    let want = [
        ("lib::calc::Calc::total", "lib::calc::Calc::add", node_kind::METHOD),
        ("lib::calc::Calc::useStatic", "lib::calc::Calc::make", node_kind::METHOD),
        ("lib::calc::Calc::viaTop", "lib::calc::helper", node_kind::FUNCTION),
    ];
    for (from, to, k) in want {
        assert!(
            calls.iter().any(|(f, t, tk)| f == from && t == to && *tk == Some(k)),
            "expected CALLS {from} -> {to}; CALLS = {calls:?}"
        );
    }
    // The three above are the file's only CALLS: nothing binds from `shadow`
    // or `param`, and `total` binds nothing else.
    assert_eq!(calls.len(), 3, "{calls:?}");
}

#[test]
fn members_and_locals_shadow_library_and_class_names() {
    let calls = calls("fixtures/dart-bare-calls");
    for (from, to, why) in [
        ("lib::calc::Calc::total", "lib::calc::add", "the member add shadows the top-level add"),
        ("lib::calc::Calc::shadow", "lib::calc::Calc::twice", "the local closure twice shadows the member"),
        ("lib::calc::Calc::param", "lib::calc::Calc::add", "the parameter add shadows the member"),
        ("lib::calc::Calc::param", "lib::calc::add", "the parameter add shadows the top-level add"),
    ] {
        assert!(
            !calls.iter().any(|(f, t, _)| f == from && t == to),
            "{why}: CALLS {from} -> {to} must not exist; CALLS = {calls:?}"
        );
    }
}

#[test]
fn matrix_dart_calls_probe_binds_the_self_call() {
    let calls = calls("matrix/dart/calls");
    assert!(
        calls
            .iter()
            .any(|(f, t, k)| f == "calc::Calc::total"
                && t == "calc::Calc::add"
                && *k == Some(node_kind::METHOD)),
        "expected CALLS calc::Calc::total -> calc::Calc::add; CALLS = {calls:?}"
    );
}
