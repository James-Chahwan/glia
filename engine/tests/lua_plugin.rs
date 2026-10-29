//! L1-L3 (lua-support) fired_on proof: the Lua parser, its graph builder, the
//! routed manifest and `resolver:plugin`, over the committed
//! `engine/tests/data/lua-plugin/` fixture (each directory one feature; the
//! ecosystems the shapes come from are named in its README).
//!
//! Graph content is asserted in-process; the `[lua]` / `[imports] lua` /
//! `[lua-calls]` / `[plugin]` markers are greppable in the test's captured
//! stderr (`cargo test -p glia-engine --test lua_plugin -- --nocapture 2> <out>`).

use glia_engine::generate_one;

fn fixture() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/lua-plugin").to_string()
}

/// A node by qname across every language group's graph: the Lua symbols, the
/// routed PLUGIN and the Rust loader live in separate `RepoGraph`s of the one
/// repo.
fn qname_id(r: &glia_engine::GenerateResult, qname: &str) -> glia_core::NodeId {
    r.merged
        .graphs
        .iter()
        .find_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .find(|(_, q)| q.as_str() == qname)
                .map(|(id, _)| *id)
        })
        .unwrap_or_else(|| panic!("no node with qname {qname}"))
}

/// The edge in any group's graph, or a cross edge (a resolver's edges sit in
/// `cross_edges` even when both endpoints share a repo).
fn has_edge(
    r: &glia_engine::GenerateResult,
    from: glia_core::NodeId,
    to: glia_core::NodeId,
    category: glia_core::EdgeCategoryId,
) -> bool {
    r.merged
        .all_edges()
        .any(|e| e.from == from && e.to == to && e.category == category)
}

/// The kind a node was minted with, in whichever group's graph holds it.
fn kind_of(
    r: &glia_engine::GenerateResult,
    id: glia_core::NodeId,
) -> Option<glia_core::NodeKindId> {
    r.merged
        .graphs
        .iter()
        .find_map(|g| g.nav.kind_by_id.get(&id).copied())
}

/// The nav parent, likewise.
fn parent_of(r: &glia_engine::GenerateResult, id: glia_core::NodeId) -> Option<glia_core::NodeId> {
    r.merged
        .graphs
        .iter()
        .find_map(|g| g.nav.parent_of.get(&id).copied())
}

#[test]
fn lua_plugin_fixture_builds_the_bridge() {
    let r = generate_one(&fixture()).expect("fixture builds");
    assert!(
        r.parse_errors.is_empty(),
        "every fixture file parses: {:?}",
        r.parse_errors
    );
    // The three .lua files are MODULEs, named by their `::` paths.
    for qname in [
        "tool-registration::init",
        "tool-registration::helper",
        "lib::maki::tool_view",
    ] {
        let id = qname_id(&r, qname);
        assert_eq!(
            kind_of(&r, id),
            Some(glia_code_domain::node_kind::MODULE),
            "{qname} is a MODULE"
        );
    }

    // `require("helper")` binds same-dir; `require("maki.tool_view")` binds
    // through the lib-root walk (ancestor `tool-registration`'s parent, the
    // repo root, with the family's `lib` root).
    let init = qname_id(&r, "tool-registration::init");
    let helper = qname_id(&r, "tool-registration::helper");
    let tool_view = qname_id(&r, "lib::maki::tool_view");
    assert!(
        has_edge(&r, init, helper, glia_code_domain::edge_category::IMPORTS),
        "same-dir require binds"
    );
    assert!(
        has_edge(
            &r,
            init,
            tool_view,
            glia_code_domain::edge_category::IMPORTS
        ),
        "the lib-path require binds"
    );

    // The registration: one PLUGIN_TOOL, two handler FUNCTIONs, HANDLED_BY
    // each.
    let tool = qname_id(&r, "plugin_tool:greet @maki/tool-registration");
    assert_eq!(
        kind_of(&r, tool),
        Some(glia_code_domain::node_kind::PLUGIN_TOOL)
    );
    let exec = qname_id(&r, "tool-registration::init::greet.execute");
    let restore = qname_id(&r, "tool-registration::init::greet.restore");
    for handler in [exec, restore] {
        assert_eq!(
            kind_of(&r, handler),
            Some(glia_code_domain::node_kind::FUNCTION),
            "a handler closure is a FUNCTION"
        );
        assert!(
            has_edge(
                &r,
                tool,
                handler,
                glia_code_domain::edge_category::HANDLED_BY
            ),
            "PLUGIN_TOOL -HANDLED_BY-> its handler"
        );
        assert!(
            has_edge(&r, init, handler, glia_code_domain::edge_category::DEFINES),
            "the module defines its handler"
        );
    }

    // `helper.greet(..)` inside the execute closure binds the helper module's
    // `greet` through the require-alias rule.
    let greet = qname_id(&r, "tool-registration::helper::greet");
    assert!(
        has_edge(&r, exec, greet, glia_code_domain::edge_category::CALLS),
        "helper.greet binds through the require alias"
    );

    // The bound sample leaves nothing unresolved: the fixture's Lua calls are
    // the alias call alone.
    let unresolved_calls = r
        .merged
        .graphs
        .iter()
        .map(|g| {
            g.unresolved_refs
                .iter()
                .filter(|u| u.category == glia_code_domain::edge_category::CALLS)
                .count()
        })
        .sum::<usize>();
    assert_eq!(unresolved_calls, 0, "the bound sample binds every call");

    // L2: each routed manifest is one PLUGIN node keyed by its directory, the
    // family name as the owner segment, parented by the manifest's MODULE.
    let plugin = qname_id(&r, "plugin:tool-registration @maki");
    assert_eq!(
        kind_of(&r, plugin),
        Some(glia_code_domain::node_kind::PLUGIN),
        "the routed plugin.toml mints PLUGIN"
    );
    let manifest_module = qname_id(&r, "tool-registration::plugin.toml");
    assert_eq!(
        parent_of(&r, plugin),
        Some(manifest_module),
        "the manifest's MODULE parents its PLUGIN"
    );
    qname_id(&r, "plugin:manifest-permissions @maki");

    // L3: same_dir LOADS the tool set off the manifest's directory (ruling
    // 1: the load chain's second hop, on the carried LOADS category); the
    // loader's manifest literal LOADS every plugin of the family (the
    // manifest-name census the PLUGIN facts carry); the embedded plugin file
    // IMPORTS its MODULE.
    assert!(
        has_edge(&r, plugin, tool, glia_code_domain::edge_category::LOADS),
        "resolver:plugin same_dir pairs the plugin with its tool"
    );
    let loader = qname_id(&r, "host-loader::loader::load_all");
    let second = qname_id(&r, "plugin:manifest-permissions @maki");
    assert!(
        has_edge(&r, loader, plugin, glia_code_domain::edge_category::LOADS),
        "the loader's plugin.toml literal LOADS the tool-registration plugin"
    );
    assert!(
        has_edge(&r, loader, second, glia_code_domain::edge_category::LOADS),
        "the loader's literal LOADS the family's other plugin too"
    );
    assert!(
        has_edge(
            &r,
            qname_id(&r, "host-loader::loader"),
            init,
            glia_code_domain::edge_category::IMPORTS
        ),
        "the loader's include_str! imports the embedded module"
    );
}
