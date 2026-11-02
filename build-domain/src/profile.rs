//! The build-target domain's profile: [`BUILD_TABLES`] (the data every
//! domain-agnostic layer reads), [`BUILD_PASSES`] (its build passes, run by
//! activation's `PassRegistry`) and [`BUILD_PROFILE`] (the two together).
//!
//! The graph a parser hands these passes is already resolved: a build
//! system's query output names every edge's ends by label, so there is no
//! Resolve stage. `role_cells` reads each target's rule class into a ROLE,
//! `sort_edges` is the determinism sort.

use glia_activation::passes::{PassRegistry, PassSpec, Stage};
use glia_activation::profile::{DomainProfile, DomainTables, EntryRule, NamedEntry, Registries};
use glia_core::{CellPayload, canonical_edge_cmp};

use crate::graph::{BuildGraph, GRAPH_TYPE};
use crate::registry::cell_type::{self as ct, ATTRS, ROLE};
use crate::registry::edge_category::{self as ec, DEPENDS_ON, HAS_SRC, IN_PACKAGE, RUNTIME_DEP};
use crate::registry::node_kind::{self as nk, TARGET};

/// The ROLE words a target can carry, in the order [`role_of`] tries them.
pub const ROLES: [&str; 4] = ["test", "binary", "alias", "library"];

/// The domain's tables.
///
/// Entries are the targets something runs: a TARGET whose ROLE is `binary`
/// or `test`. Build nodes have no name of their own (a label is an address,
/// not a name), so the entry rule's name is the target's ROLE
/// ([`BuildGraph::is_entry`] passes it): one named row, kind TARGET, exact
/// `binary` / `test`. A library no binary or test reaches over the carry
/// edges is a dead target.
///
/// Reachability and blast radius follow DEPENDS_ON, RUNTIME_DEP and HAS_SRC
/// (IN_PACKAGE is structure: a package's other targets are not affected by
/// one of them). No effects, presets or identity rules.
pub const BUILD_TABLES: DomainTables = DomainTables {
    graph_type: GRAPH_TYPE,
    registries: Registries {
        node_kinds: nk::ALL,
        edge_categories: ec::ALL,
        cell_types: ct::ALL,
    },
    entry: EntryRule {
        kinds: &[],
        roles: &[],
        named: &[NamedEntry {
            kinds: &[TARGET],
            exact: &["binary", "test"],
            prefixes: &[],
            exts: &[],
        }],
    },
    carry_edges: &[DEPENDS_ON, RUNTIME_DEP, HAS_SRC],
    effect_sinks: &[],
    activation_weights: &[
        (DEPENDS_ON, 3.0),
        (RUNTIME_DEP, 1.0),
        (HAS_SRC, 2.0),
        (IN_PACKAGE, 0.5),
    ],
    activation_presets: &[],
    community_weights: &[(DEPENDS_ON, 3), (HAS_SRC, 2), (RUNTIME_DEP, 1)],
    identity_rules: &[],
};

/// The domain's build passes, over a parsed [`BuildGraph`], no build context.
pub const BUILD_PASSES: PassRegistry<BuildGraph> = PassRegistry::new(&[
    PassSpec {
        name: "role_cells",
        stage: Stage::Post,
        after: &[],
        populates: &[ROLE],
        run: role_cells,
    },
    PassSpec {
        name: "sort_edges",
        stage: Stage::Finalize,
        after: &[],
        populates: &[],
        run: sort_edges,
    },
]);

/// The domain's whole profile. `BUILD_PROFILE.run_passes` prints
/// `[passes] domain=build-targets resolve=0 post=1 finalize=1`.
pub static BUILD_PROFILE: DomainProfile<BuildGraph> = DomainProfile {
    tables: BUILD_TABLES,
    passes: BUILD_PASSES,
};

/// A target's ROLE from its rule class: a class ending `_test` is a `test`
/// (Bazel requires every test rule's class to end so), one ending `_binary`
/// a `binary`, `alias` an `alias`, anything else a `library`. `testonly`
/// never makes a test: a testonly library is a test helper, built for tests
/// but never run as one, so it stays a `library` (and a testonly binary a
/// `binary`).
pub fn role_of(rule_class: &str) -> &'static str {
    if rule_class.ends_with("_test") {
        "test"
    } else if rule_class.ends_with("_binary") {
        "binary"
    } else if rule_class == "alias" {
        "alias"
    } else {
        "library"
    }
}

/// Post: every TARGET whose ATTRS names a `rule_class` gets a ROLE Text cell
/// ([`role_of`]), replacing one it has. A target without one gets none.
fn role_cells(g: &mut BuildGraph, _: &()) {
    for node in &mut g.nodes {
        if g.labels.get(&node.id.0).map(|l| l.kind) != Some(TARGET) {
            continue;
        }
        let rule_class = node
            .cells
            .iter()
            .find(|c| c.kind == ATTRS)
            .and_then(|c| match &c.payload {
                CellPayload::Json(s) => serde_json::from_str::<serde_json::Value>(s).ok(),
                _ => None,
            })
            .and_then(|v| v.get("rule_class")?.as_str().map(role_of));
        let Some(role) = rule_class else {
            continue;
        };
        let payload = CellPayload::Text(role.to_string());
        match node.cells.iter_mut().find(|c| c.kind == ROLE) {
            Some(cell) => cell.payload = payload,
            None => node.cells.push(glia_core::Cell {
                kind: ROLE,
                payload,
            }),
        }
    }
}

/// Finalize: edges in the canonical order (`from`, `to`, `category`, ...).
fn sort_edges(g: &mut BuildGraph, _: &()) {
    g.edges.sort_by(canonical_edge_cmp);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_and_profile_validate_in_the_build_block() {
        assert_eq!(BUILD_TABLES.validate(), Ok(()));
        assert_eq!(BUILD_TABLES.validate_blocks(true), Ok(()));
        assert_eq!(BUILD_PROFILE.validate(), Ok(()));
        assert_eq!(
            BUILD_PROFILE.cell_populators(),
            [("role_cells", &[ROLE][..])]
        );
        for (id, _) in nk::ALL {
            assert_eq!(glia_core::ids::block_of(id.0), glia_core::ids::BUILD);
        }
        assert_eq!(TARGET.0, 0x0002_0001);
        assert_eq!(crate::registry::cell_type::VIA.0, 0x0002_0004);
    }

    #[test]
    fn roles_from_rule_class() {
        for (class, role) in [
            ("cc_test", "test"),
            ("go_test", "test"),
            ("cc_binary", "binary"),
            ("go_binary", "binary"),
            ("alias", "alias"),
            ("cc_library", "library"),
            ("go_library", "library"),
            ("test_suite", "library"),
            ("filegroup", "library"),
        ] {
            assert_eq!(role_of(class), role, "{class}");
            assert!(ROLES.contains(&role));
        }
        let entry = |role: &str| BUILD_TABLES.entry.is_entry(Some(TARGET), role, &[]);
        assert!(entry("binary") && entry("test"));
        assert!(!entry("library") && !entry("alias"));
        assert!(!BUILD_TABLES.entry.is_entry(Some(nk::SOURCE), "binary", &[]));
    }
}
