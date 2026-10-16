//! DF.4b: a Dart plain import (`import 'x.dart';`, no `show`, no `as`)
//! exposes every public top-level name of the library it names, proven on a
//! REAL build of the `dart-plain-import-calls` fixture.
//!
//! `lib/cart.dart` imports `package:shop/src/util.dart` (an own-package URI,
//! resolved since DF.4a) and `src/fmt.dart`, both plainly: its bare calls
//! `formatPrice(..)` / `shout(..)`, the construction `Money()` and the static
//! `Money.zero()` bind into those libraries. `lib/clash.dart` plainly imports
//! two libraries that both declare `shout`, so its `shout('y')` binds
//! neither, and `util.dart`'s private `_hidden` is never a binding.

use std::collections::HashMap;

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_core::NodeId;
use glia_engine::generate_one;

fn fixture() -> String {
    format!("{}/../bench/substrate-gap/fixtures/dart-plain-import-calls", env!("CARGO_MANIFEST_DIR"))
}

/// Every `(from qname, to qname, category, evidence rule)` edge of the build.
fn edges() -> Vec<(String, String, u32, String)> {
    let r = generate_one(&fixture()).expect("fixture builds");
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
    }
    let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
    let mut out: Vec<(String, String, u32, String)> = r
        .merged
        .all_edges()
        .map(|e| {
            let rule = Evidence::of(e).and_then(|ev| ev.rule).unwrap_or_default();
            (name(&e.from), name(&e.to), e.category.0, rule)
        })
        .collect();
    out.sort();
    out
}

fn calls(edges: &[(String, String, u32, String)]) -> Vec<(&str, &str, &str)> {
    edges
        .iter()
        .filter(|(_, _, c, _)| *c == edge_category::CALLS.0)
        .map(|(f, t, _, r)| (f.as_str(), t.as_str(), r.as_str()))
        .collect()
}

#[test]
fn a_plain_import_binds_the_librarys_public_names() {
    let edges = edges();
    assert!(
        edges
            .iter()
            .any(|(f, t, c, _)| f == "lib::cart" && t == "lib::src::util" && *c == edge_category::IMPORTS.0),
        "IMPORTS lib::cart -> lib::src::util (DF.4a's own-package URI); edges = {edges:?}"
    );
    let calls = calls(&edges);
    for (from, to, rule) in [
        ("lib::cart::Cart::total", "lib::src::util::formatPrice", "import_binding"),
        ("lib::cart::Cart::total", "lib::src::util::Money::zero", "attribute"),
        ("lib::cart::Cart::total", "lib::src::fmt::shout", "import_binding"),
        ("lib::cart::main", "lib::src::util::formatPrice", "import_binding"),
        ("lib::cart::main", "lib::src::util::Money", "import_binding"),
    ] {
        assert!(
            calls.contains(&(from, to, rule)),
            "expected CALLS {from} -> {to} (rule {rule}); CALLS = {calls:?}"
        );
    }
}

#[test]
fn an_ambiguous_or_private_name_binds_nothing() {
    let edges = edges();
    let calls = calls(&edges);
    assert!(
        !calls.iter().any(|(f, _, _)| *f == "lib::clash::run"),
        "`shout` is declared by both of clash.dart's plain imports: no CALLS from run; CALLS = {calls:?}"
    );
    assert!(
        !calls.iter().any(|(_, t, _)| *t == "lib::src::util::_hidden"),
        "the private _hidden is never bound; CALLS = {calls:?}"
    );
    // cart.dart's calls are the five above and nothing else.
    let from_cart: Vec<_> = calls.iter().filter(|(f, _, _)| f.starts_with("lib::cart::")).collect();
    assert_eq!(from_cart.len(), 5, "{from_cart:?}");
}
