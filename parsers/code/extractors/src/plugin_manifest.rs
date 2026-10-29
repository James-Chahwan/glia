//! The plugin-manifest extractor (L2, lua-support): one PLUGIN node per
//! routed `<dir>/<manifest>` (the repo's `PluginFamilies` name the manifest,
//! `plugin_families::manifest_family`).
//!
//! The manifest's identity is its **parent directory**, not a TOML field —
//! maki's manifest carries no name (the loader keys plugins by directory), so
//! the node's qname is `plugin:<basename-of-parent-dir> @<family>`, the
//! family's `name` as the LB.8 owner segment so two families' plugins can
//! never collide on a NodeId. The `[permissions]` section is runtime security
//! config, not structure: it is counted for the marker and never parsed into
//! the graph (lua-support ruling 4). A manifest with no parent directory
//! (a root-level `plugin.toml`) names no plugin and mints nothing.
//!
//! The family row travels with the node as a `NavFact::PluginManifest` on the
//! synthetic manifest MODULE, so `resolver:plugin` reads the family name and
//! the manifest basename off the graph and never off a table.
//!
//! fired_on, per routed file with a node minted:
//! `[plugin-manifest] family=<name> plugin=<dir> perms_keys=<n> path=<p>`.

use glia_code_domain::{
    CodeNav, GRAPH_TYPE, NavFact, cell_type, node_kind, plugin_families::PluginFamilies,
};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

/// What one routed manifest yields. `marker` is the fired_on line the route
/// branch pushes into its tally (None: nothing minted, stay silent).
pub struct PluginManifestOut {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    pub marker: Option<String>,
}

/// The shipped entry point: fires only when the basename matches one of the
/// repo's families' `manifest`.
pub fn extract_for_path(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    families: &PluginFamilies,
) -> PluginManifestOut {
    match families.manifest_family(path) {
        Some(family) => extract_for_family(source, path, module_id, repo, family),
        None => PluginManifestOut {
            nodes: Vec::new(),
            edges: Vec::new(),
            nav: CodeNav::default(),
            marker: None,
        },
    }
}

/// The route branch's gate: a manifest basename of one of the repo's
/// families. It must be checked before the package-manifest machinery
/// (`packages::is_manifest_path`), whose dependency + declared-binary shape
/// is the wrong one for a plugin.
pub fn is_routed(path: &str, families: &PluginFamilies) -> bool {
    families.manifest_family(path).is_some()
}

/// The family-parameterised form, so a synthetic row proves the rows are data
/// without shipping a half-supported family.
pub fn extract_for_family(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    family: &glia_code_domain::plugin_families::PluginFamily,
) -> PluginManifestOut {
    let empty = || PluginManifestOut {
        nodes: Vec::new(),
        edges: Vec::new(),
        nav: CodeNav::default(),
        marker: None,
    };
    let Some(dir) = path.rsplit_once('/').map(|(d, _)| d) else {
        return empty();
    };
    let plugin = dir.rsplit('/').next().unwrap_or(dir);
    if plugin.is_empty() {
        return empty();
    }
    let qname = format!("plugin:{} @{}", plugin, family.name);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PLUGIN, &qname);
    let rows = source.lines().count();
    let position = format!(
        "{{\"file\":\"{}\",\"start_line\":0,\"end_line\":{}}}",
        path.replace('\\', "\\\\").replace('"', "\\\""),
        rows.saturating_sub(1)
    );
    let node = Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: vec![
            Cell {
                kind: cell_type::CODE,
                payload: CellPayload::Text(source.to_string()),
            },
            Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(position),
            },
        ],
    };
    let mut nav = CodeNav::default();
    nav.record(id, plugin, &qname, node_kind::PLUGIN, Some(module_id));
    nav.record_fact(
        module_id,
        NavFact::PluginManifest {
            family: family.name.clone(),
            manifest: family.manifest.clone(),
        },
    );
    let marker = format!(
        "[plugin-manifest] family={} plugin={} perms_keys={} path={path}",
        family.name,
        plugin,
        count_permissions_keys(source)
    );
    // The edge list is empty by design: the plugin's DEFINES edges to its
    // tools are `resolver:plugin`'s `same_dir` rule (L3), which pairs on the
    // qname owner tail; the manifest text is what the loader reads, not a
    // structural edge the TOML can assert.
    PluginManifestOut {
        nodes: vec![node],
        edges: Vec::new(),
        nav,
        marker: Some(marker),
    }
}

/// The keys declared inside the `[permissions]` section (and any of its
/// compatibility continuations before the next `[section]`): a marker count
/// only, never graph structure.
fn count_permissions_keys(source: &str) -> usize {
    let mut in_permissions = false;
    let mut keys = 0usize;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_permissions = trimmed == "[permissions]";
            continue;
        }
        if in_permissions && !trimmed.is_empty() && !trimmed.starts_with('#') {
            keys += 1;
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::plugin_families::{PluginFamily, seed_families};

    fn repo() -> RepoId {
        RepoId(1)
    }

    fn module(path: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, path)
    }

    fn families() -> PluginFamilies {
        PluginFamilies::new(seed_families())
    }

    #[test]
    fn mints_the_plugin_from_the_directory() {
        let out = extract_for_path(
            "[permissions]\nnet_hosts = [\"api.example.com\"]\n",
            "plugins/question/plugin.toml",
            module("plugins/question/plugin.toml"),
            repo(),
            &families(),
        );
        assert_eq!(out.nodes.len(), 1);
        let node = &out.nodes[0];
        assert_eq!(
            out.nav.qname_by_id.get(&node.id).map(String::as_str),
            Some("plugin:question @maki")
        );
        assert_eq!(out.nav.kind_by_id.get(&node.id), Some(&node_kind::PLUGIN));
        // Strong: the node's existence is read off the tree, not inferred.
        assert_eq!(node.confidence, Confidence::Strong);
        // The manifest text is the CODE cell; the file the POSITION cell.
        assert!(node.cells.iter().any(|c| c.kind == cell_type::CODE));
        assert!(node.cells.iter().any(|c| c.kind == cell_type::POSITION));
        // The family row travels as a NavFact on the manifest MODULE.
        assert_eq!(
            out.nav
                .nav_facts
                .get(&module("plugins/question/plugin.toml")),
            Some(&vec![NavFact::PluginManifest {
                family: "maki".to_string(),
                manifest: "plugin.toml".to_string(),
            }])
        );
        // The marker carries the family, the directory and the perms count.
        let marker = out.marker.expect("a minted manifest fires the marker");
        assert!(marker.contains("family=maki"), "{marker}");
        assert!(marker.contains("plugin=question"), "{marker}");
        assert!(marker.contains("perms_keys=1"), "{marker}");
        // The CODE cell holds the manifest text verbatim.
        let code = node
            .cells
            .iter()
            .find(|c| c.kind == cell_type::CODE)
            .and_then(|c| match &c.payload {
                CellPayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .unwrap();
        assert!(code.starts_with("[permissions]"), "{code}");
    }

    #[test]
    fn counts_permissions_keys_across_sections() {
        let src = "[permissions]\nnet = 1\nshell = true\n\n[compat]\nold = 2\n";
        let out = extract_for_path(
            src,
            "p/plugin.toml",
            module("p/plugin.toml"),
            repo(),
            &families(),
        );
        assert!(
            out.marker.unwrap().contains("perms_keys=2"),
            "only the [permissions] section counts"
        );
        // Comments are not keys.
        let commented = "[permissions]\n# note\nnet = 1\n";
        let out = extract_for_path(
            commented,
            "p/plugin.toml",
            module("p/plugin.toml"),
            repo(),
            &families(),
        );
        assert!(out.marker.unwrap().contains("perms_keys=1"));
    }

    #[test]
    fn ignores_a_non_manifest_basename() {
        let out = extract_for_path(
            "[permissions]\n",
            "p/other.toml",
            module("p/other.toml"),
            repo(),
            &families(),
        );
        assert!(out.nodes.is_empty());
        assert!(out.marker.is_none());
        // The manifest basename must be exact: a suffixed twin is not routed.
        assert!(families().manifest_family("p/plugin.toml.prod").is_none());
    }

    #[test]
    fn a_root_level_manifest_mints_nothing() {
        // No parent directory -> no plugin identity; silent, nothing minted.
        let out = extract_for_path(
            "[permissions]\n",
            "plugin.toml",
            module("plugin.toml"),
            repo(),
            &families(),
        );
        assert!(out.nodes.is_empty());
        assert!(out.marker.is_none());
    }

    #[test]
    fn a_synthetic_family_row_drives_the_extractor() {
        let family = PluginFamily {
            name: "toy".to_string(),
            manifest: "host.toml".to_string(),
            calls: Vec::new(),
            lib_roots: Vec::new(),
        };
        let out = extract_for_family(
            "[permissions]\n",
            "plugins/x/host.toml",
            module("plugins/x/host.toml"),
            repo(),
            &family,
        );
        assert_eq!(
            out.nav.qname_by_id.values().cloned().collect::<Vec<_>>(),
            vec!["plugin:x @toy".to_string()],
            "the shipped row is data, not code"
        );
        // The seed table does not know the toy manifest.
        assert!(
            extract_for_path(
                "[permissions]\n",
                "p/host.toml",
                module("p/host.toml"),
                repo(),
                &families()
            )
            .nodes
            .is_empty()
        );
    }
}
