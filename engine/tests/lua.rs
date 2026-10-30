//! 0.5.5 Lua (from Brian Ealdwine's glia PR #3): the `lua-pure` and
//! `lua-nvim` substrate fixtures built for real, checking the IMPORTS and
//! CALLS edges and their EVIDENCE rules (`build/lua.rs`), and the
//! `matrix/lua/impl` cell's metatable classes (INHERITS_FROM, inherited calls).

use std::collections::HashMap;

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_core::NodeId;
use glia_engine::generate_one;

fn fixture(name: &str) -> String {
    format!("{}/../bench/substrate-gap/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Every IMPORTS, CALLS and INHERITS_FROM edge of fixture `name`'s build as
/// `"<category> <from qname> -> <to qname> [<rule>]"`, sorted.
fn edges(name: &str) -> Vec<String> {
    let r = generate_one(&fixture(name)).expect("fixture builds");
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
    }
    let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
    let mut out: Vec<String> = r
        .merged
        .all_edges()
        .filter_map(|e| {
            let cat = match e.category {
                c if c == edge_category::CALLS => "CALLS",
                c if c == edge_category::IMPORTS => "IMPORTS",
                c if c == edge_category::INHERITS_FROM => "INHERITS_FROM",
                _ => return None,
            };
            Some((e, cat))
        })
        .map(|(e, cat)| {
            let rule = Evidence::of(e).and_then(|ev| ev.rule).unwrap_or_default();
            format!("{cat} {} -> {} [{rule}]", name(&e.from), name(&e.to))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn lua_pure_requires_and_calls_bind() {
    assert_eq!(
        edges("fixtures/lua-pure"),
        [
            "CALLS main::App.run -> helper::M.greet [require_alias]",
            "CALLS main::App.run -> lib::mathx::M.add [require_alias]",
            "CALLS main::App.run -> main::App.step [module_table]",
            "CALLS main::App.run -> main::log [local_call]",
            "CALLS main::App.run -> strings::init::upper [require_alias]",
            "CALLS main::App.step -> main::App.step.tick [local_call]",
            "CALLS main::App.step.tick -> main::log [local_call]",
            "CALLS main::App.stop -> main::log [local_call]",
            "IMPORTS main -> helper [require_sibling]",
            "IMPORTS main -> lib::mathx [require_root]",
            "IMPORTS main -> strings::init [require_sibling]",
        ]
    );
}

#[test]
fn lua_nvim_runtimepath_requires_and_alias_calls_bind() {
    assert_eq!(
        edges("fixtures/lua-nvim"),
        [
            "CALLS lua::myplug::init::M.setup -> lua::myplug::init::apply [local_call]",
            "CALLS lua::myplug::init::apply -> lua::myplug::config::defaults [require_alias]",
            "CALLS lua::myplug::init::apply -> lua::myplug::util::M.merge [require_alias]",
            "CALLS plugin::myplug -> lua::myplug::init::M.setup [require_alias]",
            "IMPORTS lua::myplug::init -> lua::myplug::config [require_path]",
            "IMPORTS lua::myplug::init -> lua::myplug::util [require_path]",
            "IMPORTS plugin::myplug -> lua::myplug::init [require_root]",
        ]
    );
}

#[test]
fn lua_metatable_classes_inherit_across_files() {
    assert_eq!(
        edges("matrix/lua/impl"),
        [
            "CALLS dog::Dog.bark -> animal::Animal.speak [inherited_method]",
            "CALLS dog::Dog.new -> animal::Animal.new [require_alias]",
            "CALLS main -> dog::Dog.bark [require_alias]",
            "CALLS main -> dog::Dog.new [require_alias]",
            "IMPORTS dog -> animal [require_sibling]",
            "IMPORTS main -> dog [require_sibling]",
            "INHERITS_FROM dog::Dog -> animal::Animal [require_alias]",
        ]
    );
}
