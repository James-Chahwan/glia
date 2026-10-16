//! DT.3b (tweaks T3) acceptance: a WebSocket client whose URL base sits in the
//! repo configuration folds through the const table after the parse cache, as
//! the A11.2 endpoint fold does for HTTP clients, on REAL builds.
//!
//! `grade.py` reads the installed wheel, so it cannot see this change until the
//! end-of-wave rebuild; these tests build the committed fixture from the
//! working tree. The cache test is the reason the fold runs post-cache: a
//! cache-served client must fold against the CURRENT table, never the one its
//! parse was cached under.

use std::collections::BTreeSet;
use std::path::Path;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::{
    GenerateResult, ParseCache, generate_many, generate_one, generate_one_with_cache,
};
use glia_graph::MergedGraph;

const FIXTURE: &str = "xcut-websocket-env-base";

fn fixture(dirs: &[&str]) -> GenerateResult {
    let root = format!(
        "{}/../bench/substrate-gap/fixtures/{FIXTURE}",
        env!("CARGO_MANIFEST_DIR")
    );
    let dirs: Vec<String> = dirs.iter().map(|d| format!("{root}/{d}")).collect();
    generate_many(&dirs).expect("fixture builds")
}

/// Every qname of `kind`, across graphs.
fn qnames(m: &MergedGraph, kind: NodeKindId) -> BTreeSet<String> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .kind_by_id
                .iter()
                .filter(move |(_, k)| **k == kind)
                .filter_map(move |(id, _)| g.nav.qname_by_id.get(id).cloned())
        })
        .collect()
}

fn node_id(m: &MergedGraph, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .find_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .find_map(|(id, q)| (q == qname).then_some(*id))
        })
        .unwrap_or_else(|| panic!("no node {qname}"))
}

/// Every `category` edge, as `(from qname, to qname)`, sorted.
fn edges(m: &MergedGraph, category: EdgeCategoryId) -> BTreeSet<(String, String)> {
    let qname = |id: NodeId| {
        m.graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_default()
    };
    m.all_edges()
        .filter(|e| e.category == category)
        .map(|e| (qname(e.from), qname(e.to)))
        .collect()
}

/// Every stored `kind` cell payload on `id`, across graphs.
fn cells(m: &MergedGraph, id: NodeId, kind: glia_core::CellTypeId) -> Vec<String> {
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == kind)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) => Some(j.clone()),
            _ => None,
        })
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn pairs(items: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    items
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn configured_bases_fold_and_pair_their_own_routes() {
    let r = fixture(&["client", "server"]);
    let m = &r.merged;
    // The whole-member URL takes its path, the template its base's `/rt`:
    // neither the generic `ws` nor the tail-only `/prices` is minted.
    assert_eq!(
        qnames(m, node_kind::WS_CLIENT),
        set(&["ws_client:/rt/feed", "ws_client:/rt/prices"])
    );
    // Both pair the upgrading handler through their own routes (LA.18b's
    // inherited tier); nothing pairs through the decoy GET /prices alone.
    assert_eq!(
        edges(m, edge_category::WS_CONNECTS),
        pairs(&[
            ("ws_client:/rt/feed", "ws:ws"),
            ("ws_client:/rt/prices", "ws:ws"),
        ])
    );
    // The configured authority is the host each site dials, written in
    // environment.ts, so it is not marked external.
    for client in ["ws_client:/rt/feed", "ws_client:/rt/prices"] {
        assert_eq!(
            cells(m, node_id(m, client), cell_type::ENDPOINT_HIT),
            vec![r#"{"via":"ws","host":"feed-svc:8080"}"#.to_string()],
            "{client}"
        );
    }
    // Re-anchored in each function, post-cache: `extractor:anchor` rule
    // `const_fold`.
    let uses: BTreeSet<(String, String)> = edges(m, edge_category::USES)
        .into_iter()
        .filter(|(_, to)| to.starts_with("ws_client:"))
        .collect();
    assert_eq!(
        uses,
        pairs(&[
            ("src::live::openFeed", "ws_client:/rt/feed"),
            ("src::live::openPrices", "ws_client:/rt/prices"),
        ])
    );
    let feed = node_id(m, "ws_client:/rt/feed");
    let ev = m
        .all_edges()
        .find(|e| e.to == feed && e.category == edge_category::USES)
        .and_then(Evidence::of)
        .expect("the USES edge carries its evidence");
    assert_eq!(ev.emitter, "extractor:anchor");
    assert_eq!(ev.rule.as_deref(), Some("const_fold"));
    // Shaped like a literal client: ENDPOINT_HIT, then POSITION at the call.
    assert_eq!(
        cells(m, feed, cell_type::POSITION),
        vec![r#"{"file":"src/live.ts","start_line":3,"end_line":3}"#.to_string()]
    );
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn fixture_source(rel: &str) -> String {
    let p = format!(
        "{}/../bench/substrate-gap/fixtures/{FIXTURE}/client/{rel}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

/// The fixture's client in a tempdir, so the test can edit its environment.
fn client_repo(root: &Path) {
    write(root, "package.json", &fixture_source("package.json"));
    write(
        root,
        "src/environment.ts",
        &fixture_source("src/environment.ts"),
    );
    write(root, "src/live.ts", &fixture_source("src/live.ts"));
}

#[test]
fn cache_served_client_folds_against_the_current_table() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("client");
    client_repo(&repo);
    let path = repo.to_str().unwrap();
    let mut cache = ParseCache::new();
    let cold = generate_one_with_cache(path, &mut cache).unwrap();
    assert_eq!(
        qnames(&cold.merged, node_kind::WS_CLIENT),
        set(&["ws_client:/rt/feed", "ws_client:/rt/prices"])
    );

    // Only environment.ts changes: live.ts is served from the parse cache,
    // and must fold against the new binding, exactly as a clean build does.
    write(
        &repo,
        "src/environment.ts",
        "export const environment = { feedUrl: 'ws://feed-svc:8080/rt/live', wsBase: 'ws://feed-svc:8080/rt' };\n",
    );
    let edited = generate_one_with_cache(path, &mut cache).unwrap();
    let diff = cache.last_diff().expect("the build recorded its diff");
    assert!(
        diff.reused.iter().any(|p| p.ends_with("live.ts")),
        "live.ts is cache-served: {diff:?}"
    );
    assert!(
        diff.reparsed.iter().any(|p| p.ends_with("environment.ts")),
        "{diff:?}"
    );
    assert_eq!(
        qnames(&edited.merged, node_kind::WS_CLIENT),
        set(&["ws_client:/rt/live", "ws_client:/rt/prices"])
    );
    let clean = generate_one(path).unwrap();
    assert_eq!(
        qnames(&edited.merged, node_kind::WS_CLIENT),
        qnames(&clean.merged, node_kind::WS_CLIENT)
    );
    assert_eq!(
        edges(&edited.merged, edge_category::USES),
        edges(&clean.merged, edge_category::USES),
        "incremental == clean"
    );
    let live = node_id(&edited.merged, "ws_client:/rt/live");
    assert_eq!(
        cells(&edited.merged, live, cell_type::ENDPOINT_HIT),
        cells(
            &clean.merged,
            node_id(&clean.merged, "ws_client:/rt/live"),
            cell_type::ENDPOINT_HIT
        )
    );
}

/// A base bound once per deployment is ambiguous: the strict resolver binds
/// neither, so the client keeps its per-file reading and records no host.
#[test]
fn a_base_bound_per_deployment_is_left_unfolded() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("client");
    client_repo(&repo);
    write(
        &repo,
        "src/environment.prod.ts",
        "export const environment = { feedUrl: 'wss://feed.example.com/rt/feed', wsBase: 'wss://feed.example.com/rt' };\n",
    );
    let r = generate_one(repo.to_str().unwrap()).unwrap();
    let m = &r.merged;
    assert_eq!(
        qnames(m, node_kind::WS_CLIENT),
        set(&["ws_client:ws", "ws_client:/prices"])
    );
    assert_eq!(
        cells(m, node_id(m, "ws_client:/prices"), cell_type::ENDPOINT_HIT),
        vec![r#"{"via":"ws"}"#.to_string()]
    );
}
