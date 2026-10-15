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
//!
//! CD.1c: `community_weights` is the integer table communities and split
//! cuts group by, kept apart from the PPR `activation_weights` so a ranking
//! retune never moves community structure. Code that calls, handles or
//! messages other code binds hardest; DEFINES glues a symbol to its module
//! at 1; every edge that would glue unrelated code through a shared hub (a
//! PROJECT's CONTAINS, a package, a doc, a test, a shared schema / config /
//! entity, git history) is left out, so weighs 0.
//!
//! DD.3: `identity_rules` is the code domain's eight ways two nodes are one
//! thing, one [`IdentityRule`] per [`same_as::RULES`] name in that order:
//! the kinds each pairs, the key they share, the tier (FACT, DERIVED or
//! HEURISTIC) and whether a class is contracted in walks. Three consumers
//! read the rows: the identity resolver (which kinds a rule pairs), the tier
//! of a SAME_AS edge, and the answers' collapse (FACT and DERIVED classes
//! collapse, HEURISTIC never). SAME_AS itself weighs 0 in PPR and is in no
//! carry or community list: identity reaches a walk or a ranking only through
//! the quotient (`glia_activation::algo::classes`, DD.1), never as an edge.

use glia_activation::profile::{
    ActivationPreset, DomainTables, EffectSink, EntryRule, IdentityRule, IdentityTier, NamedEntry,
    Registries,
};
use glia_core::NodeKindId;

use crate::{GRAPH_TYPE, cell_type, edge_category as ec, node_kind as nk, same_as};

/// The definitions the SCIP build stage binds a definition row to
/// (`engine/src/external/scip.rs` `DEF_KINDS`, in its order): what a SCIP
/// symbol can name on either side of a `scip_symbol` class.
const SCIP_DEF_KINDS: &[NodeKindId] = &[
    nk::FUNCTION,
    nk::METHOD,
    nk::CLASS,
    nk::STRUCT,
    nk::INTERFACE,
    nk::ENUM,
    nk::ATTRIBUTE,
    nk::STATE_VAR,
];

/// The declarations a vendored copy is matched by (`body_hash`,
/// `path_suffix`): a file's MODULE and the code declared in it.
const VENDORED_KINDS: &[NodeKindId] = &[
    nk::MODULE,
    nk::FUNCTION,
    nk::METHOD,
    nk::CLASS,
    nk::STRUCT,
    nk::INTERFACE,
    nk::ENUM,
];

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
    // outright, so scores are those of the graph without it). SAME_AS (DD.3)
    // weighs 0 too: identity reaches a ranking only through the quotient, and
    // a SAME_AS star at 1.0 would conduct across every repo sharing a package.
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
        (ec::SAME_AS, 0.0),
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
        // CC.10a: what much depends on, for one global PageRank (hotspots).
        // Every module DEFINES / CONTAINS what it declares, a heavily tested
        // function is not more depended on and a doc is no dependant, so
        // none of them makes code central.
        ActivationPreset {
            name: "centrality",
            overrides: &[
                (ec::DEFINES, 0.0),
                (ec::CONTAINS, 0.0),
                (ec::DOCUMENTS, 0.0),
                (ec::TESTS, 0.0),
            ],
        },
    ],
    // CD.1c: every category of `ec::ALL` in its order, weighted or left out
    // (weight 0) with the reason. The guard test below pins the left-out set.
    community_weights: &[
        (ec::DEFINES, 1),
        // CONTAINS out: a PROJECT / directory contains everything under it.
        (ec::IMPORTS, 1),
        (ec::CALLS, 4),
        (ec::USES, 2),
        // DOCUMENTS out: one doc names many unrelated symbols.
        // TESTS out: a test suite touches whatever it tests, across modules.
        (ec::INJECTS, 3),
        (ec::HANDLED_BY, 4),
        (ec::HTTP_CALLS, 3),
        (ec::GRPC_CALLS, 3),
        (ec::QUEUE_FLOWS, 3),
        (ec::GRAPHQL_CALLS, 3),
        (ec::WS_CONNECTS, 3),
        (ec::EVENT_FLOWS, 3),
        // SHARES_SCHEMA out: a shared schema pairs services, not their code.
        (ec::CLI_INVOKES, 2),
        (ec::ACCESSES_DATA, 2),
        (ec::HAS_ATTRIBUTE, 1),
        (ec::INHERITS_FROM, 2),
        (ec::RETURNS_TYPE, 1),
        // SHARES_DATA_ENTITY out: a shared table is an operational fact.
        (ec::SCHEDULES, 2),
        // SHARES_CRON_SCHEDULE out: one schedule string, no code link.
        (ec::READS_CONFIG, 1),
        (ec::DEFINES_CONFIG, 1),
        // SHARES_CONFIG out: one config key read in two places, no code link.
        (ec::INFRA_REFERENCES, 1),
        // SHARES_INFRA_REF out: one infra name in two places, no code link.
        // DEPENDS_ON out: a package manifest depends on whole libraries.
        // SHARES_DEPENDENCY out: two manifests naming one library.
        (ec::IMPLEMENTS, 2),
        // SHARES_DATA_SOURCE out: a shared database, as for blast radius.
        (ec::RPC_CALLS, 3),
        (ec::NAVIGATES_TO, 2),
        // CO_CHANGES out: git history is a heuristic, never structure.
        // SAME_AS out: identity reaches a walk only through the quotient (DD.1).
    ],
    // DD.3: one row per `same_as::RULES` name, in that order (the guard test
    // below pins it). `walk` is a ranking-visible dial: a class contracted in
    // a walk joins its members' neighbourhoods into one blast radius.
    identity_rules: &[
        // A class of one external package spans every repo depending on it:
        // it collapses in rows, but walking it would join every dependant of
        // `react` in every repo into one blast radius.
        IdentityRule {
            name: same_as::RULE_PACKAGE_NAME,
            from: &[nk::PACKAGE_DEP],
            to: &[nk::PACKAGE_DEP],
            key: "package:<eco>:<name>, ecosystem-normalised",
            tier: IdentityTier::Fact,
            walk: false,
        },
        // A PROJECT has no edges, so contracting it with the dependencies
        // naming it fans out only to the library's real consumers.
        IdentityRule {
            name: same_as::RULE_MANIFEST_NAME,
            from: &[nk::PACKAGE_DEP],
            to: &[nk::PROJECT],
            key: "dependency name == manifest label (per ecosystem)",
            tier: IdentityTier::Fact,
            walk: true,
        },
        IdentityRule {
            name: same_as::RULE_MESSAGE_FQNAME,
            from: &[nk::MESSAGE_TYPE],
            to: &[nk::MESSAGE_TYPE],
            key: "message:<flavor>:<fq name>",
            tier: IdentityTier::Fact,
            walk: true,
        },
        // A contract op is paired from its ORIGIN by `same_as::contract_op_key`.
        IdentityRule {
            name: same_as::RULE_CONTRACT_OP,
            from: &[nk::DOC_SECTION],
            to: &[nk::DOC_SECTION],
            key: "METHOD + normalised path, or action + channel, under one project",
            tier: IdentityTier::Derived,
            walk: true,
        },
        IdentityRule {
            name: same_as::RULE_CONTRACT_ROUTE,
            from: &[nk::DOC_SECTION],
            to: &[nk::DOC_SECTION],
            key: "the one ROUTE the op's path matches",
            tier: IdentityTier::Derived,
            walk: true,
        },
        IdentityRule {
            name: same_as::RULE_SCIP_SYMBOL,
            from: SCIP_DEF_KINDS,
            to: SCIP_DEF_KINDS,
            key: "SCIP scheme manager package descriptor",
            tier: IdentityTier::Fact,
            walk: true,
        },
        // Vendored copies are guesses: they never collapse and never walk.
        IdentityRule {
            name: same_as::RULE_BODY_HASH,
            from: VENDORED_KINDS,
            to: VENDORED_KINDS,
            key: "path-suffix files, name chain, byte-identical body",
            tier: IdentityTier::Heuristic,
            walk: false,
        },
        IdentityRule {
            name: same_as::RULE_PATH_SUFFIX,
            from: VENDORED_KINDS,
            to: VENDORED_KINDS,
            key: "path-suffix files under a confirmed vendoring offset, body differs",
            tier: IdentityTier::Heuristic,
            walk: false,
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
        for preset in [None, Some("repair"), Some("review"), Some("onboard"), Some("centrality")] {
            let w = CODE_TABLES.activation_config(preset).edge_weights.get(&ec::CO_CHANGES).copied();
            assert_eq!(w, Some(0.0), "preset {preset:?}");
        }
    }

    #[test]
    fn same_as_never_conducts() {
        // REGRESSION GUARD (DD.3). SAME_AS joins an identity class's members
        // to its star center: one npm package in every repo, hundreds of
        // vendored candidates. Identity reaches a walk or a ranking only
        // through the quotient, so the edge carries no blast radius or
        // liveness, is no effect, groups no community and weighs 0 in every
        // activation preset (an unlisted category would weigh 1.0).
        assert!(!CODE_TABLES.carries(ec::SAME_AS), "SAME_AS must stay OUT of carry_edges");
        assert!(!CODE_TABLES.effect_sinks.iter().any(|s| s.via.contains(&ec::SAME_AS)));
        assert_eq!(CODE_TABLES.community_weight(ec::SAME_AS), 0);
        for preset in [None, Some("repair"), Some("review"), Some("onboard"), Some("centrality")] {
            let w = CODE_TABLES.activation_config(preset).edge_weights.get(&ec::SAME_AS).copied();
            assert_eq!(w, Some(0.0), "preset {preset:?}");
        }
    }

    /// DD.3: one identity rule per `same_as` rule name, in its order, with
    /// the tiers and walk dials 0.5.2 ruled (Q17 / Q18): packages and
    /// vendored copies never walk, vendored copies are HEURISTIC.
    #[test]
    fn identity_rules_cover_every_rule() {
        use IdentityTier::{Derived, Fact, Heuristic};
        assert_eq!(CODE_TABLES.validate(), Ok(()));
        let rules = CODE_TABLES.identity_rules;
        let names: Vec<&str> = rules.iter().map(|r| r.name).collect();
        assert_eq!(names, same_as::RULES);
        let table: Vec<(&str, IdentityTier, bool)> =
            rules.iter().map(|r| (r.name, r.tier, r.walk)).collect();
        assert_eq!(
            table,
            [
                ("package_name", Fact, false),
                ("manifest_name", Fact, true),
                ("message_fqname", Fact, true),
                ("contract_op", Derived, true),
                ("contract_route", Derived, true),
                ("scip_symbol", Fact, true),
                ("body_hash", Heuristic, false),
                ("path_suffix", Heuristic, false),
            ]
        );

        let rule = |name: &str| CODE_TABLES.identity_rule(name);
        let pairs = |name: &str| rule(name).map(|r| (r.from, r.to));
        const DEP: &[NodeKindId] = &[nk::PACKAGE_DEP];
        const PROJECT: &[NodeKindId] = &[nk::PROJECT];
        const MESSAGE: &[NodeKindId] = &[nk::MESSAGE_TYPE];
        const DOC: &[NodeKindId] = &[nk::DOC_SECTION];
        assert_eq!(pairs("package_name"), Some((DEP, DEP)));
        assert_eq!(pairs("manifest_name"), Some((DEP, PROJECT)));
        assert_eq!(pairs("message_fqname"), Some((MESSAGE, MESSAGE)));
        for name in ["contract_op", "contract_route"] {
            assert_eq!(pairs(name), Some((DOC, DOC)), "{name}");
        }
        assert_eq!(pairs("scip_symbol"), Some((SCIP_DEF_KINDS, SCIP_DEF_KINDS)));
        assert_eq!(SCIP_DEF_KINDS.len(), 8);
        for name in ["body_hash", "path_suffix"] {
            assert_eq!(pairs(name), Some((VENDORED_KINDS, VENDORED_KINDS)), "{name}");
            assert!(rule(name).is_some_and(|r| !r.tier.collapses()), "{name} never collapses");
        }
        assert_eq!(VENDORED_KINDS[0], nk::MODULE);
        assert_eq!(&VENDORED_KINDS[1..], &SCIP_DEF_KINDS[..6], "a file plus its declared code");

        // Every key is a line, the rule names are the bare EVIDENCE rules.
        for r in rules {
            assert!(!r.key.is_empty() && !r.key.contains('\n'), "{}", r.name);
            let diverged = same_as::evidence_rule(r.name, true);
            assert_eq!(same_as::split_evidence_rule(&diverged), (r.name, true), "{diverged}");
        }
        assert!(rule("package_name/diverged").is_none());
    }

    /// CD.1c: communities group by what code does to code. Structure
    /// (CONTAINS, DEPENDS_ON), docs, tests, git history and every SHARES_*
    /// pairing weigh 0 - each would glue unrelated code through one hub - and
    /// exactly those: every other registered category is listed, so a new
    /// category must be weighed or added here.
    #[test]
    fn community_weights_exclude_structure_history_and_docs() {
        assert_eq!(CODE_TABLES.validate(), Ok(()));
        let w = |c| CODE_TABLES.community_weight(c);
        for c in [ec::CONTAINS, ec::DOCUMENTS, ec::TESTS, ec::DEPENDS_ON, ec::CO_CHANGES] {
            assert_eq!(w(c), 0, "{} is left out", ec::name(c));
        }
        let shares: Vec<_> =
            ec::ALL.iter().filter(|(_, n)| n.starts_with("SHARES_")).map(|(c, _)| *c).collect();
        assert_eq!(shares.len(), 7, "{shares:?}");
        for c in &shares {
            assert_eq!(w(*c), 0, "{} is left out", ec::name(*c));
        }
        for c in [ec::CALLS, ec::HANDLED_BY, ec::DEFINES, ec::HTTP_CALLS, ec::QUEUE_FLOWS] {
            assert!(w(c) > 0, "{} groups", ec::name(c));
        }
        assert!(w(ec::CALLS) > w(ec::DEFINES), "a call binds harder than a definition");

        let left_out: Vec<&str> =
            ec::ALL.iter().filter(|(c, _)| w(*c) == 0).map(|(_, n)| *n).collect();
        assert_eq!(
            left_out,
            [
                "CONTAINS",
                "DOCUMENTS",
                "TESTS",
                "SHARES_SCHEMA",
                "SHARES_DATA_ENTITY",
                "SHARES_CRON_SCHEDULE",
                "SHARES_CONFIG",
                "SHARES_INFRA_REF",
                "DEPENDS_ON",
                "SHARES_DEPENDENCY",
                "SHARES_DATA_SOURCE",
                "CO_CHANGES",
                "SAME_AS",
            ]
        );
        assert_eq!(CODE_TABLES.community_weights.len() + left_out.len(), ec::ALL.len());
        // The table is laid out in `ec::ALL` order.
        let listed: Vec<_> = CODE_TABLES.community_weights.iter().map(|(c, _)| *c).collect();
        let in_all: Vec<_> =
            ec::ALL.iter().map(|(c, _)| *c).filter(|c| listed.contains(c)).collect();
        assert_eq!(listed, in_all);
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
        // CC.10a: `centrality` zeroes structure, tests and docs, and keeps
        // every other base weight.
        let centrality = CODE_TABLES.activation_config(Some("centrality"));
        for c in [ec::DEFINES, ec::CONTAINS, ec::DOCUMENTS, ec::TESTS] {
            assert_eq!(centrality.edge_weights[&c], 0.0, "category {}", c.0);
        }
        for (c, w) in &base.edge_weights {
            if ![ec::DEFINES, ec::CONTAINS, ec::DOCUMENTS, ec::TESTS].contains(c) {
                assert_eq!(centrality.edge_weights[c], *w, "category {}", c.0);
            }
        }
        for name in ["default", "nonsense"] {
            assert_eq!(
                CODE_TABLES.activation_config(Some(name)).edge_weights,
                base.edge_weights,
                "preset {name:?} is the base"
            );
        }
    }
}
