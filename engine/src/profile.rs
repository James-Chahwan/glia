//! The code domain's engine profile: `CODE_PROFILE` and `CODE_PASSES`. Filled
//! by LD.13 and LD.14a; extended by LD.14b, LD.6, LE.3a and LE.4d.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `glia_engine::profile::<item>`, never flattened into the
//! crate root.
//!
//! LD.13: [`CODE_PASSES`] is every build pass the code domain runs over an
//! assembled [`MergedGraph`], in run order: the cross-graph resolvers
//! (`Resolve`), the post-passes of [`crate::passes`] (`Post`), then the
//! evidence fill and the determinism sort (`Finalize`). Every build tail —
//! `generate_one*` and `generate_many*` ([`run_code_passes_with`], given the
//! build's [`CodeBuildCtx`]) and a layout merge ([`crate::merge`],
//! [`run_code_passes`]) — is one call. A new resolver or build pass is one
//! [`PassSpec`] here, never a new call in `build/`.
//!
//! The build context is [`CodeBuildCtx`] (LF.2b): every repo's external
//! inputs (`.glia/overlay.toml`, loaded once per repo right after its walk)
//! and the build's `overlay` switch. Its first reader is the `external_edges`
//! pass, the first `Post` spec: the overlay `[[edge]]` stanzas land after the
//! resolvers and before every post-pass. A later stage that needs another
//! argument (LF.2d's mounts) adds a field.
//!
//! LD.14a: [`CODE_PROFILE`] is the code domain's whole
//! [`DomainProfile`]: the data tables (`code_domain::profile::CODE_TABLES` —
//! registries, entry rule, carry edges, effect sinks, activation weights and
//! presets) plus [`CODE_PASSES`]. [`run_code_passes`] runs the passes through
//! it.
//!
//! LD.14b: its tables are the only source of the code domain's query dials.
//! Liveness (`answers::entrypoint_reachable`) seeds from `tables.entry` and
//! walks `tables.carry_edges`; `cross_stack_trace` walks the same carry set;
//! blast radius passes `&CODE_PROFILE.tables` to `MergedGraph::blast_radius`;
//! `resolve_signal_located` ranks with `tables.activation_config(None)`. The
//! hardcoded lists they replaced (the engine's entrypoint predicate, the graph
//! crate's blast carry list and its activation default / preset functions) are
//! deleted; `code_profile_matches_head_tables` pins the tables to their values
//! as literals.
//!
//! LD.6: `tables.entry` is the one entrypoint set. It gained the inbound
//! handler kinds liveness lacked (GRPC_SERVER, RPC_PROCEDURE, QUEUE_CONSUMER,
//! GRAPHQL_RESOLVER, CRON_JOB), and [`entry_kinds`] lists its kinds for the
//! consumers that kept their own copy.

use glia_activation::passes::{PassRegistry, PassReport, PassSpec, Stage};
use glia_activation::profile::DomainProfile;
use glia_code_domain::{cell_type, evidence, node_kind};
use glia_core::NodeKindId;
use glia_graph::{
    CliInvocationResolver, ConfigResolver, CronResolver, CrossGraphResolver, DbResolver,
    EventBusResolver, GraphQLStackResolver, GrpcStackResolver, HttpStackResolver, IacResolver,
    MergedGraph, MessageSchemaResolver, PackageResolver, QueueStackResolver, RpcStackResolver,
    SharedSchemaResolver, WebSocketStackResolver,
};

use crate::build::BuildOptions;
use crate::external::{RepoInputs, apply_external_edges};
use crate::{adr, passes};

/// What every code pass receives besides the graph (LF.2b): each built repo's
/// external inputs, in argument order, and whether the overlay applies
/// ([`BuildOptions::overlay`]). Made by the build only: its fields are
/// crate-private. A layout merge ([`crate::merge`]) runs with no inputs: the
/// overlay edges its members were built with are carried as member edges.
pub struct CodeBuildCtx {
    pub(crate) inputs: Vec<RepoInputs>,
    pub(crate) overlay: bool,
}

impl CodeBuildCtx {
    /// A build's context: its repos' inputs and options.
    pub(crate) fn new(inputs: Vec<RepoInputs>, opts: &BuildOptions) -> Self {
        Self { inputs, overlay: opts.overlay }
    }

    /// No external inputs: every external stage is a no-op.
    pub(crate) fn empty() -> Self {
        Self { inputs: Vec::new(), overlay: true }
    }
}

/// Run one cross-graph resolver. Resolvers only append to `cross_edges`, so
/// the range it appended is stamped with `emitter` (`resolver:<pass name>`,
/// LC.3a). A resolver that attached its own evidence (with a rule, LC.3c)
/// keeps it: a stamp never overrides.
fn resolve<R: CrossGraphResolver>(merged: &mut MergedGraph, resolver: &R, emitter: &str) {
    let n = merged.cross_edges.len();
    merged.run(resolver);
    if let Some(added) = merged.cross_edges.get_mut(n..) {
        evidence::stamp_missing(added, emitter);
    }
}

/// A `Resolve` spec whose evidence emitter is `resolver:<name>`: the pass
/// name and the emitter label are one literal, so they cannot drift.
macro_rules! resolver {
    ($name:literal, $resolver:expr) => {
        PassSpec {
            name: $name,
            stage: Stage::Resolve,
            after: &[],
            populates: &[],
            run: |m, _| resolve(m, &$resolver, concat!("resolver:", $name)),
        }
    };
}

/// Every build pass of the code domain. Resolvers run in the order their
/// cross edges land in (before the Finalize sort fixes the stored order).
pub(crate) const CODE_PASSES: PassRegistry<MergedGraph, CodeBuildCtx> = PassRegistry::new(&[
    // LF.2d: the HTTP resolver with the build's `[[route_prefix]]` mounts. No
    // mounts (no overlay, or none declared) is the plain resolver.
    PassSpec {
        name: "http",
        stage: Stage::Resolve,
        after: &[],
        populates: &[],
        run: |m, ctx| {
            let mounts = RepoInputs::route_mounts(m, &ctx.inputs, ctx.overlay);
            resolve(m, &HttpStackResolver::with_mounts(&mounts), "resolver:http")
        },
    },
    resolver!("grpc", GrpcStackResolver),
    resolver!("rpc", RpcStackResolver),
    resolver!("queue", QueueStackResolver),
    resolver!("graphql", GraphQLStackResolver),
    resolver!("websocket", WebSocketStackResolver),
    resolver!("eventbus", EventBusResolver),
    resolver!("shared_schema", SharedSchemaResolver),
    // A10.7 — MESSAGE_TYPE nodes joined across repos on the exact qname.
    resolver!("message_schema", MessageSchemaResolver),
    resolver!("cli", CliInvocationResolver),
    resolver!("db", DbResolver),
    resolver!("cron", CronResolver),
    resolver!("config", ConfigResolver),
    resolver!("iac", IacResolver),
    resolver!("package", PackageResolver),
    // LF.2b: the external edges (`.glia/overlay.toml` `[[edge]]` stanzas),
    // first of the post-passes so every one of them sees them. They carry
    // their own EVIDENCE (emitter `overlay:edge`), which the Finalize fill
    // leaves alone and the sort orders.
    PassSpec {
        name: "external_edges",
        stage: Stage::Post,
        after: &[],
        populates: &[],
        run: |m, ctx| apply_external_edges(m, &ctx.inputs, ctx.overlay),
    },
    PassSpec {
        name: "downgrade_test_paths",
        stage: Stage::Post,
        after: &[],
        populates: &[],
        run: |m, _| passes::downgrade_test_paths(m),
    },
    // Reads the HTTP_CALLS edges, and the confidence the test-path downgrade
    // left (a Weak node stays Weak).
    PassSpec {
        name: "demote_unmatched_http_nodes",
        stage: Stage::Post,
        after: &["http", "external_edges", "downgrade_test_paths"],
        populates: &[],
        run: |m, _| passes::demote_unmatched_http_nodes(m),
    },
    PassSpec {
        name: "emit_tests_edges",
        stage: Stage::Post,
        after: &[],
        populates: &[],
        run: |m, _| passes::emit_tests_edges(m),
    },
    // LE.3a: a TEST cell on every node a TESTS edge points at, listing its
    // direct tests - reads the parser's function-level edges, the module
    // pairing just emitted and the overlay's declared ones.
    PassSpec {
        name: "fill_test_cells",
        stage: Stage::Post,
        after: &["external_edges", "emit_tests_edges"],
        populates: &[cell_type::TEST],
        run: |m, _| passes::fill_test_cells(m).report(),
    },
    PassSpec {
        name: "link_doc_sections",
        stage: Stage::Post,
        after: &[],
        populates: &[],
        run: |m, _| passes::link_doc_sections(m),
    },
    // LF.4b: a DECISION entry on every node an ADR section DOCUMENTS - reads
    // the doc linker's edges; the external cell stage (after the passes)
    // merges sidecar / declared decisions into the same entry array.
    PassSpec {
        name: "fill_adr_decisions",
        stage: Stage::Post,
        after: &["link_doc_sections"],
        populates: &[cell_type::DECISION],
        run: |m, _| adr::fill_adr_decisions(m).report(),
    },
    // A contract edge is never above its ROUTE's confidence, so it reads the
    // confidence the HTTP demotion left.
    PassSpec {
        name: "link_contract_routes",
        stage: Stage::Post,
        after: &["demote_unmatched_http_nodes"],
        populates: &[],
        run: |m, _| passes::link_contract_routes(m),
    },
    // CG.4b: reads the HTTP_CALLS edges, resolver and overlay `[[edge]]`
    // alike - an ENDPOINT nothing pairs is stamped `external` when every call
    // site names a third-party host.
    PassSpec {
        name: "tag_synthetic_provenance",
        stage: Stage::Post,
        after: &["http", "external_edges"],
        populates: &[cell_type::ORIGIN],
        run: |m, _| passes::tag_synthetic_provenance(m),
    },
    // LC.3a: locate every edge's evidence, after every pass that adds an
    // edge and before the sort, whose canonical order compares cells.
    PassSpec {
        name: "fill_evidence_sites",
        stage: Stage::Finalize,
        after: &[],
        populates: &[],
        run: |m, _| passes::fill_evidence_sites(m).report(),
    },
    // Deterministic cross-edge order: several resolvers emit pairs by
    // iterating HashMap indexes (per-process seed), so the edge SET was stable
    // but its Vec order — and therefore cross_stack.gmap's bytes — flapped
    // across processes and even clean-vs-incremental in one process (audit
    // 2026-06-10 #6). One sort here covers all resolvers and post-passes. The
    // order is total (LC.2): same-key edges that differ only in their cells
    // still land in one order, so the bytes cannot flap once edges carry cells.
    PassSpec {
        name: "sort_cross_edges",
        stage: Stage::Finalize,
        after: &["fill_evidence_sites"],
        populates: &[],
        run: |m, _| m.sort_cross_edges(),
    },
]);

/// The code domain's profile: [`CODE_TABLES`](glia_code_domain::profile::CODE_TABLES)
/// plus [`CODE_PASSES`]. A `static` (a `const` would copy it at every use);
/// its initializer reads only consts.
pub static CODE_PROFILE: DomainProfile<MergedGraph, CodeBuildCtx> = DomainProfile {
    tables: glia_code_domain::profile::CODE_TABLES,
    passes: CODE_PASSES,
};

/// The code domain's entrypoint kinds (LD.6): `CODE_PROFILE.tables.entry.kinds`
/// as `(id, name)`, in table order — the ONE entrypoint set liveness seeds
/// from (`entrypoint_reachable`) and the dense-text `*` sigil reads, so a
/// consumer (the repo-graph wrapper's entry tiering, pyo3 `entry_kinds()`)
/// derives its set from here instead of keeping a copy. Kinds only: the
/// table's entry ROLES (a CLASS carrying ROLE COMPONENT) and its `main` /
/// `test*` / `Test*` name rule also make entries; `nodes_json`'s per-node
/// `entry` flag applies the whole rule.
pub fn entry_kinds() -> Vec<(NodeKindId, &'static str)> {
    CODE_PROFILE
        .tables
        .entry
        .kinds
        .iter()
        .map(|k| (*k, node_kind::name(*k)))
        .collect()
}

/// Run [`CODE_PASSES`] over an assembled graph, through [`CODE_PROFILE`]: the
/// whole build tail, given the build's context (its repos' external inputs
/// and options).
///
/// Markers, once per call: LD.13's fired_on line, printed by
/// `DomainProfile::run_passes` with `domain=` read from the tables'
/// `graph_type`,
///   `[passes] domain=code resolve=<r> post=<p> finalize=<f>`
/// (grep token `[passes] domain=code`), then LC.2's
/// `[edge-cells] intra=<n> cross=<c> with_cells=<k>`.
///
/// Returns the registry's [`PassReport`]: what ran and what each pass and
/// stage took, which the caller's `[timing] build` line reads (CA.9).
pub(crate) fn run_code_passes_with(merged: &mut MergedGraph, ctx: &CodeBuildCtx) -> PassReport {
    let report = CODE_PROFILE.run_passes(merged, ctx);
    passes::edge_cells_marker(merged);
    report
}

/// [`run_code_passes_with`] and no external inputs: the tail of a layout
/// merge, whose members' external edges are already in their layouts.
pub(crate) fn run_code_passes(merged: &mut MergedGraph) -> PassReport {
    run_code_passes_with(merged, &CodeBuildCtx::empty())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use glia_core::{Cell, CellTypeId};

    use super::*;

    const HEAD_ORDER: [&str; 26] = [
        "http",
        "grpc",
        "rpc",
        "queue",
        "graphql",
        "websocket",
        "eventbus",
        "shared_schema",
        "message_schema",
        "cli",
        "db",
        "cron",
        "config",
        "iac",
        "package",
        "external_edges",
        "downgrade_test_paths",
        "demote_unmatched_http_nodes",
        "emit_tests_edges",
        "fill_test_cells",
        "link_doc_sections",
        "fill_adr_decisions",
        "link_contract_routes",
        "tag_synthetic_provenance",
        "fill_evidence_sites",
        "sort_cross_edges",
    ];

    /// The registry reproduces the pre-LD.13 hardcoded tail exactly:
    /// `run_all_resolvers` (15), `post_passes` (6), then fill-then-sort as
    /// the last two steps (LC.3a); LF.2b's external edges are the first
    /// post-pass, LE.3a's TEST cells follow the TESTS emitter, and LF.4b's
    /// ADR decisions follow the doc linker.
    #[test]
    fn code_passes_order_is_head_order() {
        assert_eq!(CODE_PASSES.validate(), Ok(()));
        let names: Vec<&str> = CODE_PASSES.order().iter().map(|s| s.name).collect();
        assert_eq!(names, HEAD_ORDER);
        let stages: Vec<Stage> = CODE_PASSES.order().iter().map(|s| s.stage).collect();
        let count = |st: Stage| stages.iter().filter(|s| **s == st).count();
        assert_eq!(
            (count(Stage::Resolve), count(Stage::Post), count(Stage::Finalize)),
            (15, 9, 2)
        );
    }

    /// LD.14a / LD.14b: [`CODE_PROFILE`]'s tables equal the hardcoding they
    /// replaced, pinned as literals now the originals are deleted — the carry
    /// edges in order, the activation config of every preset field by field
    /// (exact f64), and the entrypoint truth table. A change here is a
    /// deliberate change to liveness, blast radius or PPR ranking.
    #[test]
    fn code_profile_matches_head_tables() {
        use glia_activation::{ActivationConfig, Direction, Specificity};
        use glia_code_domain::{edge_category as ec, node_kind as nk};
        use glia_core::{EdgeCategoryId, NodeKindId};
        use std::collections::HashMap;

        let t = &CODE_PROFILE.tables;
        // The carry list `graph::blast` hardcoded before LD.14b, in order.
        const HEAD_CARRY: [EdgeCategoryId; 26] = [
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
        ];
        assert_eq!(t.carry_edges, HEAD_CARRY);

        // The base weights `graph::activation` hardcoded before LD.14b.
        const HEAD_WEIGHTS: [(EdgeCategoryId, f64); 20] = [
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
            // LF.5b: the git-history heuristic weighs 0 (no ranking change).
            (ec::CO_CHANGES, 0.0),
        ];
        // The preset overrides `graph::activation` hardcoded before LD.14b;
        // any other name, `"default"` included, is the base.
        let head_overrides = |preset: Option<&str>| -> &'static [(EdgeCategoryId, f64)] {
            match preset {
                Some("repair") => &[
                    (ec::CALLS, 8.0),
                    (ec::USES, 6.0),
                    (ec::ACCESSES_DATA, 6.0),
                    (ec::READS_CONFIG, 5.0),
                    (ec::IMPORTS, 5.0),
                    (ec::TESTS, 4.0),
                ],
                Some("review") => &[
                    (ec::CALLS, 6.0),
                    (ec::TESTS, 6.0),
                    (ec::IMPLEMENTS, 5.0),
                    (ec::INHERITS_FROM, 5.0),
                    (ec::RETURNS_TYPE, 4.0),
                ],
                Some("onboard") => &[
                    (ec::CONTAINS, 5.0),
                    (ec::IMPORTS, 5.0),
                    (ec::HANDLED_BY, 6.0),
                    (ec::DEFINES, 3.0),
                    (ec::DOCUMENTS, 3.0),
                ],
                // CC.10a: hotspots' PageRank lens, added after LD.14b.
                Some("centrality") => &[
                    (ec::DEFINES, 0.0),
                    (ec::CONTAINS, 0.0),
                    (ec::DOCUMENTS, 0.0),
                    (ec::TESTS, 0.0),
                ],
                _ => &[],
            }
        };
        for p in [
            None,
            Some("default"),
            Some("repair"),
            Some("review"),
            Some("onboard"),
            Some("centrality"),
            Some("nonsense"),
        ] {
            let mut weights: HashMap<EdgeCategoryId, f64> = HEAD_WEIGHTS.into_iter().collect();
            weights.extend(head_overrides(p).iter().copied());
            let head = ActivationConfig {
                damping: 0.5,
                direction: Direction::Forward,
                edge_weights: weights,
                node_specificity: Specificity::None,
                top_k: 50,
                max_iterations: 100,
                epsilon: 1e-6,
            };
            let got = t.activation_config(p);
            assert_eq!(got.edge_weights, head.edge_weights, "edge_weights, preset {p:?}");
            assert_eq!(got.damping, head.damping, "damping, preset {p:?}");
            assert_eq!(got.direction, head.direction, "direction, preset {p:?}");
            assert_eq!(got.node_specificity, head.node_specificity, "specificity, preset {p:?}");
            assert_eq!(got.top_k, head.top_k, "top_k, preset {p:?}");
            assert_eq!(got.max_iterations, head.max_iterations, "max_iterations, preset {p:?}");
            assert_eq!(got.epsilon, head.epsilon, "epsilon, preset {p:?}");
        }
        assert_eq!(t.activation_weights, HEAD_WEIGHTS);
        let presets: Vec<(&str, usize)> =
            t.activation_presets.iter().map(|p| (p.name, p.overrides.len())).collect();
        assert_eq!(presets, [("repair", 6), ("review", 5), ("onboard", 5), ("centrality", 4)]);
        for p in t.activation_presets {
            assert_eq!(p.overrides, head_overrides(Some(p.name)), "preset {}", p.name);
        }

        // The entrypoint predicate `engine::answers` hardcoded before LD.14b,
        // literally, plus the five inbound handler kinds LD.6 reconciled in
        // (GRPC_SERVER, RPC_PROCEDURE, QUEUE_CONSUMER, GRAPHQL_RESOLVER,
        // CRON_JOB), in table order.
        const ENTRY_KINDS: [NodeKindId; 11] = [
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
        ];
        assert_eq!(t.entry.kinds, ENTRY_KINDS, "entry kinds, in table order");
        let head_entry = |kind: Option<NodeKindId>, name: &str, roles: &[NodeKindId]| {
            if roles.contains(&nk::COMPONENT) {
                return true;
            }
            match kind {
                Some(k) if ENTRY_KINDS.contains(&k) => true,
                Some(k) if k == nk::FUNCTION || k == nk::METHOD => {
                    name == "main" || name.starts_with("test") || name.starts_with("Test")
                }
                _ => false,
            }
        };
        const NAMES: [&str; 8] = ["main", "Main", "test_x", "testFoo", "TestFoo", "tester", "handler", ""];
        let role_sets: [&[NodeKindId]; 4] =
            [&[], &[nk::COMPONENT], &[nk::SERVICE, nk::HOOK], &[nk::HOOK, nk::COMPONENT]];
        let kinds: Vec<Option<NodeKindId>> =
            nk::ALL.iter().map(|(k, _)| Some(*k)).chain([None]).collect();
        let mut entries_without_roles = 0;
        for &kind in &kinds {
            for name in NAMES {
                for roles in role_sets {
                    let head = head_entry(kind, name, roles);
                    assert_eq!(t.entry.is_entry(kind, name, roles), head, "{kind:?} {name:?} {roles:?}");
                    entries_without_roles += usize::from(head && roles.is_empty());
                }
            }
        }
        // 11 entry kinds x 8 names, plus FUNCTION / METHOD x the 5 names
        // `main` / `test*` / `Test*` match.
        assert_eq!(entries_without_roles, 11 * 8 + 2 * 5);

        assert_eq!(CODE_PROFILE.validate(), Ok(()));
        assert_eq!(
            CODE_PROFILE.cell_populators(),
            [
                ("fill_test_cells", &[cell_type::TEST][..]),
                ("fill_adr_decisions", &[cell_type::DECISION][..]),
                ("tag_synthetic_provenance", &[cell_type::ORIGIN][..]),
            ]
        );
        // LE.4d's sink table: (class, target kinds, reaching categories).
        let sinks: Vec<(&str, &[NodeKindId], &[EdgeCategoryId])> =
            t.effect_sinks.iter().map(|s| (s.class, s.kinds, s.via)).collect();
        let expect: [(&str, &[NodeKindId], &[EdgeCategoryId]); 8] = [
            (
                "db",
                &[nk::DATA_ENTITY, nk::DATABASE, nk::CACHE, nk::BLOB_STORE, nk::SEARCH_INDEX],
                &[ec::ACCESSES_DATA],
            ),
            ("email", &[nk::EMAIL_SERVICE], &[ec::ACCESSES_DATA]),
            ("queue_produce", &[nk::QUEUE_PRODUCER], &[ec::USES]),
            ("http_call", &[nk::ENDPOINT], &[ec::CALLS, ec::USES]),
            ("event_emit", &[nk::EVENT_EMITTER], &[ec::USES]),
            ("rpc_call", &[nk::GRPC_CLIENT, nk::RPC_CALL], &[ec::USES, ec::CALLS]),
            ("ws_send", &[nk::WS_CLIENT], &[ec::USES]),
            ("graphql_op", &[nk::GRAPHQL_OPERATION], &[ec::USES]),
        ];
        assert_eq!(sinks, expect);
        assert_eq!(t.graph_type, glia_code_domain::GRAPH_TYPE);
    }

    fn workspace() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(PathBuf::from)
            .expect("engine sits inside the workspace")
    }

    /// A fixture's `dirs`, from its key.json, resolved to paths.
    fn fixture_dirs(name: &str) -> Vec<String> {
        let root = workspace().join("bench/substrate-gap/fixtures").join(name);
        let key = std::fs::read_to_string(root.join("key.json"))
            .unwrap_or_else(|e| panic!("{name}/key.json: {e}"));
        let key: serde_json::Value = serde_json::from_str(&key).expect("key.json parses");
        key["dirs"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}/key.json has no dirs"))
            .iter()
            .map(|d| root.join(d.as_str().expect("dir is a string")).to_string_lossy().into_owned())
            .collect()
    }

    /// Every node's cells, per graph, in node order.
    fn node_cells(m: &MergedGraph) -> Vec<Vec<Vec<Cell>>> {
        m.graphs.iter().map(|g| g.nodes.iter().map(|n| n.cells.clone()).collect()).collect()
    }

    /// The types of the cells in `after` that `before` does not hold (a
    /// multiset difference over whole cells, so a rewritten payload counts
    /// as written).
    fn written(before: &[Cell], after: &[Cell]) -> BTreeSet<u32> {
        let mut left: Vec<&Cell> = before.iter().collect();
        let mut out = BTreeSet::new();
        for c in after {
            match left.iter().position(|b| *b == c) {
                Some(i) => {
                    left.swap_remove(i);
                }
                None => {
                    out.insert(c.kind.0);
                }
            }
        }
        out
    }

    /// Each pass's `populates` is exact over the fixture set: run the passes
    /// one at a time, and every node cell type a pass writes is declared, and
    /// every declared type is written at least once.
    #[test]
    fn populates_is_exact() {
        const FIXTURES: [&str; 6] = [
            "arch-monorepo-flows",
            "py-tests",
            "xstack-go-http",
            "xcut-queue-queue_flows",
            "contract-openapi-yaml",
            "docs-adr-decision",
        ];
        let mut observed: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
        for fixture in FIXTURES {
            let mut merged = crate::build::assemble_many(&fixture_dirs(fixture), false)
                .unwrap_or_else(|e| panic!("{fixture}: {e}"))
                .merged;
            for spec in CODE_PASSES.order() {
                let before = node_cells(&merged);
                (spec.run)(&mut merged, &CodeBuildCtx::empty());
                let after = node_cells(&merged);
                let mut types = BTreeSet::new();
                for (gi, g) in after.iter().enumerate() {
                    for (ni, cells) in g.iter().enumerate() {
                        let prior = before.get(gi).and_then(|b| b.get(ni));
                        types.extend(written(prior.map_or(&[][..], Vec::as_slice), cells));
                    }
                }
                let declared: BTreeSet<u32> =
                    spec.populates.iter().map(|t: &CellTypeId| t.0).collect();
                let undeclared: Vec<&u32> = types.difference(&declared).collect();
                assert!(
                    undeclared.is_empty(),
                    "{fixture}: pass {} wrote undeclared node cell types {undeclared:?}",
                    spec.name
                );
                observed.entry(spec.name).or_default().extend(types);
            }
        }
        for spec in CODE_PASSES.specs() {
            for t in spec.populates {
                assert!(
                    observed.get(spec.name).is_some_and(|o| o.contains(&t.0)),
                    "pass {} declares cell type {} but wrote none over {FIXTURES:?}",
                    spec.name,
                    t.0
                );
            }
        }
    }
}
