//! The code domain's profile tables (`CODE_TABLES`): the per-domain dials the
//! domain-agnostic layers read.
//!
//! Module slot declared by L0.1 so its owner edits only this file. Filled by
//! LD.14a; LD.14b, LD.6 and LE.4d extend it.
//!
//! LD.14a: [`CODE_TABLES`] is the code domain's [`DomainTables`] — the data
//! half of the engine's `profile::CODE_PROFILE`, here beside the ids it names
//! so the graph, store and projection crates (which cannot depend on the
//! engine) read the same tables. Every list is the hardcoding it replaced,
//! with the same members in the same order:
//! - `entry`: the entrypoint predicate in `engine::answers` (LD.6 then
//!   reconciled it with the repo-graph wrapper's `ENTRY_KINDS` and added the
//!   inbound handler kinds both lacked, so it is the one entrypoint set);
//! - `carry_edges`: the blast carry list in `graph::blast`;
//! - `activation_weights` / `activation_presets`: the activation default and
//!   preset functions in `graph::activation`.
//!
//! LD.14b switched every consumer to these tables and deleted the originals:
//! liveness, cross-stack trace and blast radius (`MergedGraph::blast_radius`
//! takes `&CODE_TABLES`), signal resolution, `PyGraph.activate`, and the
//! store / projection research harnesses. The engine's
//! `profile::tests::code_profile_matches_head_tables` pins each list to the
//! original's values as literals. `effect_sinks` had no hardcoding: LD.14a
//! seeded it with four categories.
//!
//! LE.4d: `effect_sinks` is the table `effects` classifies with, one
//! [`EffectSink`] per (target kind, reaching category) class: the data kinds
//! over ACCESSES_DATA, every OUTBOUND marker the anchor pass
//! (`extractors::anchor`) ties to the function that owns it (QUEUE_PRODUCER,
//! EVENT_EMITTER, GRPC_CLIENT, RPC_CALL, WS_CLIENT, GRAPHQL_OPERATION, reached
//! over USES; RPC_CALL also over CALLS) and the outbound HTTP ENDPOINT
//! (reached over the CALLS the HTTP client extractor anchors, or USES).

use repo_graph_activation::profile::{
    ActivationPreset, DomainTables, EffectSink, EntryRule, NamedEntry, Registries,
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
    // The ONE entrypoint set (LD.6): liveness seeds from it, the engine's
    // `profile::entry_kinds()` lists it, and the dense-text `*` sigil reads
    // it. Every kind is an externally triggered inbound handler: routes, gRPC
    // services and servers, Connect / Twirp procedures, queue consumers,
    // GraphQL resolvers, WS / event handlers, CLI commands, cron jobs and
    // framework components; so are `main` and `test*` / `Test*` functions and
    // methods. A COMPONENT role (LB.3a's fold: an Angular `@Component` is a
    // CLASS carrying ROLE COMPONENT) is an entry exactly as the COMPONENT
    // kind is (LB.3b); the other roles never were.
    entry: EntryRule {
        kinds: &[
            nk::ROUTE,
            nk::GRPC_SERVICE,
            nk::GRPC_SERVER,
            nk::RPC_PROCEDURE,
            nk::QUEUE_CONSUMER,
            nk::GRAPHQL_RESOLVER,
            nk::WS_HANDLER,
            nk::EVENT_HANDLER,
            nk::CLI_COMMAND,
            nk::CRON_JOB,
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
    // LE.4d: what a function does to the outside world, as (target kind,
    // reaching category). A category alone misfires: USES also links a
    // function to symbols that are no effect, and a producer with no consumer
    // in the build has no QUEUE_FLOWS edge. The flow edges past a sink
    // (QUEUE_FLOWS, HTTP_CALLS, ...) name its receivers; they are not sinks.
    effect_sinks: &[
        EffectSink {
            class: "db",
            kinds: &[nk::DATA_ENTITY, nk::DATABASE, nk::CACHE, nk::BLOB_STORE, nk::SEARCH_INDEX],
            via: &[ec::ACCESSES_DATA],
        },
        EffectSink { class: "email", kinds: &[nk::EMAIL_SERVICE], via: &[ec::ACCESSES_DATA] },
        EffectSink { class: "queue_produce", kinds: &[nk::QUEUE_PRODUCER], via: &[ec::USES] },
        EffectSink { class: "http_call", kinds: &[nk::ENDPOINT], via: &[ec::CALLS, ec::USES] },
        EffectSink { class: "event_emit", kinds: &[nk::EVENT_EMITTER], via: &[ec::USES] },
        EffectSink {
            class: "rpc_call",
            kinds: &[nk::GRPC_CLIENT, nk::RPC_CALL],
            via: &[ec::USES, ec::CALLS],
        },
        EffectSink { class: "ws_send", kinds: &[nk::WS_CLIENT], via: &[ec::USES] },
        EffectSink { class: "graphql_op", kinds: &[nk::GRAPHQL_OPERATION], via: &[ec::USES] },
    ],
    // `calls` and cross-stack calls highest, flows and handlers next, imports
    // medium, structural edges (`contains`, `defines`) lowest; any category
    // not listed weighs 1.0. CO_CHANGES (LF.5b) weighs 0: a git-history
    // heuristic, it must not reshape any ranking (a 0 edge is skipped
    // outright, so scores are those of the graph without it).
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
        (ec::CO_CHANGES, 0.0),
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

    /// LE.4d: a sink is the (kind, category) pair. The USES a function has on
    /// a queue producer is an effect; the same USES on a plain function, or a
    /// CONTAINS / QUEUE_FLOWS into the producer, is not.
    #[test]
    fn effect_sinks_are_kind_and_category() {
        let class = |k, c| CODE_TABLES.effect_sink(k, c).map(|(_, s)| s.class);
        assert_eq!(class(nk::DATA_ENTITY, ec::ACCESSES_DATA), Some("db"));
        assert_eq!(class(nk::QUEUE_PRODUCER, ec::USES), Some("queue_produce"));
        assert_eq!(class(nk::ENDPOINT, ec::CALLS), Some("http_call"));
        assert_eq!(class(nk::EVENT_EMITTER, ec::USES), Some("event_emit"));
        assert_eq!(class(nk::RPC_CALL, ec::CALLS), Some("rpc_call"));
        assert_eq!(class(nk::FUNCTION, ec::USES), None);
        assert_eq!(class(nk::QUEUE_PRODUCER, ec::CONTAINS), None);
        assert_eq!(class(nk::QUEUE_CONSUMER, ec::QUEUE_FLOWS), None);
        assert_eq!(class(nk::DATA_ENTITY, ec::USES), None);
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

    #[test]
    fn co_changes_is_heuristic_everywhere() {
        // REGRESSION GUARD (LF.5b). Files that change together are coupled,
        // not linked: CO_CHANGES must never carry a blast radius or liveness,
        // and must weigh 0 in every activation preset (an unlisted category
        // would weigh 1.0 and reshape every ranking in a repo with history).
        assert!(!CODE_TABLES.carries(ec::CO_CHANGES), "CO_CHANGES must stay OUT of carry_edges");
        assert!(!CODE_TABLES.effect_sinks.iter().any(|s| s.via.contains(&ec::CO_CHANGES)));
        for preset in [None, Some("repair"), Some("review"), Some("onboard")] {
            let w = CODE_TABLES.activation_config(preset).edge_weights.get(&ec::CO_CHANGES).copied();
            assert_eq!(w, Some(0.0), "preset {preset:?}");
        }
    }

    /// The presets are real lenses (moved from `graph::activation`, LD.14b):
    /// `repair` weighs CALLS above the base, `onboard` weighs CONTAINS above
    /// it, and `"default"` or a name no preset has is exactly the base.
    #[test]
    fn activation_presets_shift_weights() {
        let base = CODE_TABLES.activation_config(None);
        let repair = CODE_TABLES.activation_config(Some("repair"));
        let onboard = CODE_TABLES.activation_config(Some("onboard"));
        assert!(repair.edge_weights[&ec::CALLS] > base.edge_weights[&ec::CALLS]);
        assert!(onboard.edge_weights[&ec::CONTAINS] > base.edge_weights[&ec::CONTAINS]);
        assert_ne!(repair.edge_weights, onboard.edge_weights, "the presets differ");
        for name in ["default", "nonsense"] {
            assert_eq!(
                CODE_TABLES.activation_config(Some(name)).edge_weights,
                base.edge_weights,
                "preset {name:?} is the base"
            );
        }
    }
}
