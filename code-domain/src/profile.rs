//! The code domain's profile tables (`CODE_TABLES`): the per-domain dials the
//! domain-agnostic layers read.
//!
//! Module slot declared by L0.1 so its owner edits only this file. Filled by
//! LD.14a; LD.14b, LD.6 and LE.4d extend it.
//!
//! LD.14a: [`CODE_TABLES`] is the code domain's [`DomainTables`] — the data
//! half of the engine's `profile::CODE_PROFILE`, here beside the ids it names
//! so the graph, store and projection crates (which cannot depend on the
//! engine) read the same tables. Every list is the HEAD hardcoding it
//! replaces, with the same members in the same order:
//! - `entry`: `engine::answers::is_entrypoint`;
//! - `carry_edges`: `graph::blast::blast_carry_edges`;
//! - `activation_weights` / `activation_presets`:
//!   `graph::activation::code_activation_defaults` / `code_activation_profile`.
//!
//! The engine's `profile::tests::code_profile_matches_head_tables` pins each
//! against its original until LD.14b switches the consumers and deletes them.
//! `effect_sinks` has no HEAD hardcoding: it is seeded with the four effects
//! LE.4 names, and LE.4 owns its final content.

use repo_graph_activation::profile::{
    ActivationPreset, DomainTables, EntryRule, NamedEntry, Registries,
};

use crate::{GRAPH_TYPE, cell_type, edge_category as ec, node_kind as nk};

/// The code domain's tables.
pub const CODE_TABLES: DomainTables = DomainTables {
    graph_type: GRAPH_TYPE,
    registries: Registries {
        node_kinds: nk::ALL,
        edge_categories: ec::ALL,
        cell_types: cell_type::ALL,
    },
    // Routes, gRPC / WS / event handlers, CLI commands and framework
    // components are externally triggered roots; so are `main` and
    // `test*` / `Test*` functions and methods. A COMPONENT role (LB.3a's fold:
    // an Angular `@Component` is a CLASS carrying ROLE COMPONENT) is an entry
    // exactly as the COMPONENT kind is (LB.3b); the other roles never were.
    entry: EntryRule {
        kinds: &[
            nk::ROUTE,
            nk::GRPC_SERVICE,
            nk::WS_HANDLER,
            nk::EVENT_HANDLER,
            nk::CLI_COMMAND,
            nk::COMPONENT,
        ],
        roles: &[nk::COMPONENT],
        named: &[NamedEntry {
            kinds: &[nk::FUNCTION, nk::METHOD],
            exact: &["main"],
            prefixes: &["test", "Test"],
        }],
    },
    // The semantic edges blast radius and liveness follow. Structural
    // containment / import / define edges stay out (every node would reach
    // everything), and so does SHARES_DATA_SOURCE: a shared database is an
    // operational fact, not a code dependency (see the guard test below).
    carry_edges: &[
        ec::CALLS,
        ec::USES,
        ec::HTTP_CALLS,
        ec::GRPC_CALLS,
        ec::RPC_CALLS,
        ec::GRAPHQL_CALLS,
        ec::QUEUE_FLOWS,
        ec::WS_CONNECTS,
        ec::EVENT_FLOWS,
        ec::CLI_INVOKES,
        // LA.6a: renaming or removing a route breaks every page that links to
        // it, and page flow becomes traceable end to end.
        ec::NAVIGATES_TO,
        ec::HANDLED_BY,
        ec::INJECTS,
        ec::ACCESSES_DATA,
        ec::TESTS,
        ec::DOCUMENTS,
        ec::IMPLEMENTS,
        ec::INHERITS_FROM,
        ec::RETURNS_TYPE,
        ec::SHARES_SCHEMA,
        ec::SHARES_DATA_ENTITY,
        ec::INFRA_REFERENCES,
        ec::DEPENDS_ON,
        ec::SCHEDULES,
        ec::READS_CONFIG,
        ec::DEFINES_CONFIG,
    ],
    // LE.4's four effects: data access, a queue, an outbound call, an event.
    effect_sinks: &[ec::ACCESSES_DATA, ec::QUEUE_FLOWS, ec::HTTP_CALLS, ec::EVENT_FLOWS],
    // `calls` and cross-stack calls highest, flows and handlers next, imports
    // medium, structural edges (`contains`, `defines`) lowest; any category
    // not listed weighs 1.0.
    activation_weights: &[
        (ec::CALLS, 5.0),
        (ec::HTTP_CALLS, 5.0),
        (ec::GRPC_CALLS, 5.0),
        (ec::RPC_CALLS, 5.0),
        (ec::GRAPHQL_CALLS, 5.0),
        (ec::QUEUE_FLOWS, 4.0),
        (ec::WS_CONNECTS, 4.0),
        (ec::EVENT_FLOWS, 4.0),
        (ec::CLI_INVOKES, 3.0),
        (ec::NAVIGATES_TO, 3.0),
        (ec::HANDLED_BY, 4.0),
        (ec::IMPORTS, 3.0),
        (ec::USES, 3.0),
        (ec::SHARES_SCHEMA, 2.0),
        (ec::TESTS, 2.0),
        (ec::INJECTS, 2.0),
        (ec::DEFINES, 1.0),
        (ec::CONTAINS, 1.0),
        (ec::DOCUMENTS, 0.5),
    ],
    // Task-tuned lenses over the base weights (WP-F / GR-5); any other name,
    // `"default"` included, is the base.
    activation_presets: &[
        // What buggy code actually touches: calls, data / config access, tests.
        ActivationPreset {
            name: "repair",
            overrides: &[
                (ec::CALLS, 8.0),
                (ec::USES, 6.0),
                (ec::ACCESSES_DATA, 6.0),
                (ec::READS_CONFIG, 5.0),
                (ec::IMPORTS, 5.0),
                (ec::TESTS, 4.0),
            ],
        },
        // What a reviewer reasons over: calls, tests, implements / inherits,
        // return types.
        ActivationPreset {
            name: "review",
            overrides: &[
                (ec::CALLS, 6.0),
                (ec::TESTS, 6.0),
                (ec::IMPLEMENTS, 5.0),
                (ec::INHERITS_FROM, 5.0),
                (ec::RETURNS_TYPE, 4.0),
            ],
        },
        // The high-level shape: entry points, modules, containment, docs.
        ActivationPreset {
            name: "onboard",
            overrides: &[
                (ec::CONTAINS, 5.0),
                (ec::IMPORTS, 5.0),
                (ec::HANDLED_BY, 6.0),
                (ec::DEFINES, 3.0),
                (ec::DOCUMENTS, 3.0),
            ],
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_tables_validate() {
        assert_eq!(CODE_TABLES.validate(), Ok(()));
        assert_eq!(CODE_TABLES.graph_type, "code");
        assert_eq!(CODE_TABLES.registries.kind_name(nk::ROUTE), Some("ROUTE"));
        assert_eq!(CODE_TABLES.registries.category_name(ec::NAVIGATES_TO), Some("NAVIGATES_TO"));
        assert_eq!(CODE_TABLES.registries.cell_name(cell_type::ORIGIN), Some("ORIGIN"));
    }

    #[test]
    fn shares_data_source_is_not_carried() {
        // REGRESSION GUARD. A shared Postgres is an operational fact, not a
        // code dependency: carrying it would fan every blast radius across
        // every service in the stack. Do not "fix" this by adding the row.
        assert!(
            !CODE_TABLES.carries(ec::SHARES_DATA_SOURCE),
            "SHARES_DATA_SOURCE must stay OUT of CODE_TABLES.carry_edges"
        );
    }
}
