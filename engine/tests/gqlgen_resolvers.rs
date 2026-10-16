//! DT.5f — gqlgen resolver sets. The `gqlgen-resolvers` fixture is a gqlgen
//! project without its generated executable schema: SDL root fields
//! (`schema.graphqls`) and gqlgen's resolver template
//! (`type queryResolver struct{ *Resolver }`, one method per field, the root
//! getters `Resolver.Query()` / `Mutation()`). The `gqlgen` Resolve pass
//! joins each SDL GRAPHQL_RESOLVER to its Go method with a HANDLED_BY
//! (DERIVED, `resolver:gqlgen`), so the methods are live and no dead_symbol
//! rows; the look-alike `cacheResolver::Products`, with no `*Resolver`
//! embed, pairs nothing and stays a row.

use std::path::Path;

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, gaps_report};
use glia_engine::why::why_edge;
use glia_engine::{entrypoint_reachable, generate_one};
use glia_graph::MergedGraph;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/gqlgen-resolvers";
const FILES: [&str; 7] = [
    "api/go.mod",
    "api/cmd/server/main.go",
    "api/graph/resolver.go",
    "api/graph/schema.graphqls",
    "api/graph/schema.resolvers.go",
    "api/graph/model/models_gen.go",
    "api/internal/cache/cache.go",
];
const SCHEMA: &str = "api::graph::schema.resolvers::";

/// The fixture copied into a tempdir and built; `edit` may rewrite a file.
fn build(edit: impl Fn(&str, String) -> String) -> MergedGraph {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let dir = tempfile::tempdir().expect("tempdir");
    for rel in FILES {
        let body =
            std::fs::read_to_string(src.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, edit(rel, body)).expect("write");
    }
    let root = dir.path().to_str().expect("utf-8 temp path").to_string();
    generate_one(&root)
        .unwrap_or_else(|e| panic!("generate_one: {e}"))
        .merged
}

fn qname(m: &MergedGraph, id: glia_core::NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// `(from, to, rule)` of every HANDLED_BY cross edge stamped `resolver:gqlgen`, sorted.
fn gqlgen_edges(m: &MergedGraph) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = m
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::HANDLED_BY)
        .filter_map(|e| {
            let ev = Evidence::of(e)?;
            (ev.emitter == "resolver:gqlgen").then(|| {
                (
                    qname(m, e.from),
                    qname(m, e.to),
                    ev.rule.unwrap_or_default(),
                )
            })
        })
        .collect();
    out.sort();
    out
}

fn dead(m: &MergedGraph) -> Vec<String> {
    gaps_report(m, &[], &GapsOptions::default())
        .expect("known categories")
        .rows
        .iter()
        .filter(|r| r.category == DEAD_SYMBOL)
        .map(|r| r.qname.clone())
        .collect()
}

fn row(from: &str, to: &str, rule: &str) -> (String, String, String) {
    (from.to_string(), format!("{SCHEMA}{to}"), rule.to_string())
}

#[test]
fn sdl_fields_and_root_types_pair_the_gqlgen_template() {
    let m = build(|_, body| body);
    assert_eq!(
        gqlgen_edges(&m),
        [
            row(
                "graphql_resolver:Mutation @api",
                "Resolver::Mutation",
                "gqlgen_root"
            ),
            row(
                "graphql_resolver:Query @api",
                "Resolver::Query",
                "gqlgen_root"
            ),
            row(
                "graphql_resolver:productById @api",
                "queryResolver::ProductByID",
                "gqlgen_field"
            ),
            row(
                "graphql_resolver:products @api",
                "queryResolver::Products",
                "gqlgen_field"
            ),
            row(
                "graphql_resolver:restock @api",
                "mutationResolver::Restock",
                "gqlgen_field"
            ),
        ]
    );
    // Every HANDLED_BY of the build is one of those five: the look-alike
    // `cacheResolver::Products` and the other root's methods pair nothing.
    let handled = m
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::HANDLED_BY)
        .count();
    assert_eq!(handled, 5);
}

#[test]
fn only_the_look_alike_stays_dead() {
    let m = build(|_, body| body);
    assert_eq!(
        dead(&m),
        ["api::internal::cache::cache::cacheResolver::Products"]
    );
    let live = entrypoint_reachable(&m);
    let restock = m
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.iter())
        .find(|(_, q)| q.as_str() == format!("{SCHEMA}mutationResolver::Restock"))
        .map(|(id, _)| *id)
        .expect("mutationResolver::Restock");
    assert!(live.contains(&restock), "the SDL field reaches its method");
}

#[test]
fn why_names_the_gqlgen_rule_at_tier_derived() {
    let m = build(|_, body| body);
    let a = why_edge(
        &m,
        "graphql_resolver:products @api",
        &format!("{SCHEMA}queryResolver::Products"),
        Some("HANDLED_BY"),
    )
    .expect("a why answer");
    assert!(a.found, "{a:?}");
    let e = &a.edges[0];
    assert_eq!(
        (e.tier, e.emitter.as_deref(), e.rule.as_deref()),
        ("derived", Some("resolver:gqlgen"), Some("gqlgen_field"))
    );
}

#[test]
fn without_the_resolver_embed_nothing_pairs() {
    // `type queryResolver struct{ *Resolver }` -> `struct{ Stock map[string]int }`:
    // a struct named like a root resolver is no gqlgen template without the embed.
    let m = build(|rel, body| {
        if rel.ends_with("schema.resolvers.go") {
            body.replace("struct{ *Resolver }", "struct{ Stock map[string]int }")
        } else {
            body
        }
    });
    assert_eq!(gqlgen_edges(&m), []);
    let rows = dead(&m);
    assert!(
        rows.contains(&format!("{SCHEMA}queryResolver::Products")),
        "{rows:?}"
    );
}
