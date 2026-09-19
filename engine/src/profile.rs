//! The code domain's engine profile: `CODE_PROFILE` and `CODE_PASSES`. Filled
//! by LD.13 and LD.14a; extended by LD.14b, LD.6, LE.3a and LE.4d.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::profile::<item>`, never flattened into the
//! crate root.
//!
//! LD.13: [`CODE_PASSES`] is every build pass the code domain runs over an
//! assembled [`MergedGraph`], in run order: the cross-graph resolvers
//! (`Resolve`), the post-passes of [`crate::passes`] (`Post`), then the
//! evidence fill and the determinism sort (`Finalize`). Every build tail —
//! `generate_one*`, `generate_many*` and a layout merge ([`crate::merge`]) —
//! is one [`run_code_passes`] call. A new resolver or build pass is one
//! [`PassSpec`] here, never a new call in `build/`.
//!
//! The build context is `()`: no pass takes an argument beyond the graph yet.
//! The first stage that does (LF.1a's external cells, LF.2b's external edges,
//! LF.2d's mounts) turns it into a `CodeBuildCtx` struct carrying exactly its
//! arguments.
//!
//! LD.14a: [`CODE_PROFILE`] is the code domain's whole
//! [`DomainProfile`]: the data tables (`code_domain::profile::CODE_TABLES` —
//! registries, entry rule, carry edges, effect sinks, activation weights and
//! presets) plus [`CODE_PASSES`]. [`run_code_passes`] runs the passes through
//! it. The query consumers (liveness, blast radius, PPR) switch to its tables
//! in LD.14b; until then `code_profile_matches_head_tables` pins the tables
//! equal to the hardcoded functions they replace.

use repo_graph_activation::passes::{PassRegistry, PassSpec, Stage};
use repo_graph_activation::profile::DomainProfile;
use repo_graph_code_domain::{cell_type, evidence};
use repo_graph_graph::{
    CliInvocationResolver, ConfigResolver, CronResolver, CrossGraphResolver, DbResolver,
    EventBusResolver, GraphQLStackResolver, GrpcStackResolver, HttpStackResolver, IacResolver,
    MergedGraph, MessageSchemaResolver, PackageResolver, QueueStackResolver, RpcStackResolver,
    SharedSchemaResolver, WebSocketStackResolver,
};

use crate::passes;

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
pub(crate) const CODE_PASSES: PassRegistry<MergedGraph> = PassRegistry::new(&[
    resolver!("http", HttpStackResolver),
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
        after: &["http", "downgrade_test_paths"],
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
    PassSpec {
        name: "link_doc_sections",
        stage: Stage::Post,
        after: &[],
        populates: &[],
        run: |m, _| passes::link_doc_sections(m),
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
    PassSpec {
        name: "tag_synthetic_provenance",
        stage: Stage::Post,
        after: &[],
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

/// The code domain's profile: [`CODE_TABLES`](repo_graph_code_domain::profile::CODE_TABLES)
/// plus [`CODE_PASSES`]. A `static` (a `const` would copy it at every use);
/// its initializer reads only consts.
pub static CODE_PROFILE: DomainProfile<MergedGraph> = DomainProfile {
    tables: repo_graph_code_domain::profile::CODE_TABLES,
    passes: CODE_PASSES,
};

/// Run [`CODE_PASSES`] over an assembled graph, through [`CODE_PROFILE`]: the
/// whole build tail.
///
/// Markers, once per call: LD.13's fired_on line, printed by
/// `DomainProfile::run_passes` with `domain=` read from the tables'
/// `graph_type`,
///   `[passes] domain=code resolve=<r> post=<p> finalize=<f>`
/// (grep token `[passes] domain=code`), then LC.2's
/// `[edge-cells] intra=<n> cross=<c> with_cells=<k>`.
pub(crate) fn run_code_passes(merged: &mut MergedGraph) {
    CODE_PROFILE.run_passes(merged, &());
    passes::edge_cells_marker(merged);
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use repo_graph_core::{Cell, CellTypeId};

    use super::*;

    const HEAD_ORDER: [&str; 23] = [
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
        "downgrade_test_paths",
        "demote_unmatched_http_nodes",
        "emit_tests_edges",
        "link_doc_sections",
        "link_contract_routes",
        "tag_synthetic_provenance",
        "fill_evidence_sites",
        "sort_cross_edges",
    ];

    /// The registry reproduces the pre-LD.13 hardcoded tail exactly:
    /// `run_all_resolvers` (15), `post_passes` (6), then fill-then-sort as
    /// the last two steps (LC.3a).
    #[test]
    fn code_passes_order_is_head_order() {
        assert_eq!(CODE_PASSES.validate(), Ok(()));
        let names: Vec<&str> = CODE_PASSES.order().iter().map(|s| s.name).collect();
        assert_eq!(names, HEAD_ORDER);
        let stages: Vec<Stage> = CODE_PASSES.order().iter().map(|s| s.stage).collect();
        let count = |st: Stage| stages.iter().filter(|s| **s == st).count();
        assert_eq!(
            (count(Stage::Resolve), count(Stage::Post), count(Stage::Finalize)),
            (15, 6, 2)
        );
    }

    /// LD.14a: [`CODE_PROFILE`]'s tables equal the HEAD hardcoding they
    /// replace — the carry edges in order, the activation config of every
    /// preset field by field (exact f64), and the entrypoint truth table —
    /// until LD.14b switches the consumers and deletes the originals.
    #[test]
    fn code_profile_matches_head_tables() {
        use repo_graph_activation::ActivationConfig;
        use repo_graph_code_domain::{edge_category as ec, node_kind as nk};
        use repo_graph_core::NodeKindId;
        use repo_graph_graph::{blast_carry_edges, code_activation_defaults, code_activation_profile};

        let t = &CODE_PROFILE.tables;
        assert_eq!(t.carry_edges, blast_carry_edges().as_slice());
        assert_eq!(t.carry_edges.len(), 26);

        for p in [None, Some("default"), Some("repair"), Some("review"), Some("onboard"), Some("nonsense")] {
            let head: ActivationConfig = match p {
                None => code_activation_defaults(),
                Some(name) => code_activation_profile(name),
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
        assert_eq!(t.activation_weights.len(), 19);
        let presets: Vec<(&str, usize)> =
            t.activation_presets.iter().map(|p| (p.name, p.overrides.len())).collect();
        assert_eq!(presets, [("repair", 6), ("review", 5), ("onboard", 5)]);

        // HEAD `engine::answers::is_entrypoint(kind, name, roles)`, literally.
        const ENTRY_KINDS: [NodeKindId; 6] = [
            nk::ROUTE,
            nk::GRPC_SERVICE,
            nk::WS_HANDLER,
            nk::EVENT_HANDLER,
            nk::CLI_COMMAND,
            nk::COMPONENT,
        ];
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
        // 6 entry kinds x 8 names, plus FUNCTION / METHOD x the 5 names
        // `main` / `test*` / `Test*` match.
        assert_eq!(entries_without_roles, 6 * 8 + 2 * 5);

        assert_eq!(CODE_PROFILE.validate(), Ok(()));
        assert_eq!(
            CODE_PROFILE.cell_populators(),
            [("tag_synthetic_provenance", &[cell_type::ORIGIN][..])]
        );
        assert_eq!(t.effect_sinks, [ec::ACCESSES_DATA, ec::QUEUE_FLOWS, ec::HTTP_CALLS, ec::EVENT_FLOWS]);
        assert_eq!(t.graph_type, repo_graph_code_domain::GRAPH_TYPE);
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
        const FIXTURES: [&str; 5] = [
            "arch-monorepo-flows",
            "py-tests",
            "xstack-go-http",
            "xcut-queue-queue_flows",
            "contract-openapi-yaml",
        ];
        let mut observed: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
        for fixture in FIXTURES {
            let mut merged = crate::build::assemble_many(&fixture_dirs(fixture), false)
                .unwrap_or_else(|e| panic!("{fixture}: {e}"))
                .merged;
            for spec in CODE_PASSES.order() {
                let before = node_cells(&merged);
                (spec.run)(&mut merged, &());
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
