//! A9.6 — `service_map` over REAL builds of on-disk monorepo fixtures.
//!
//! Every A9 surface is an aggregate over data a unit test cannot synthesise
//! credibly, so these parse real sources from the workspace-level
//! `tests/fixtures/` (never `bench/`, which is outside the cargo workspace):
//!
//! - `arch_monorepo/{web,api}` — TS client + Go chi server as TOP-LEVEL dirs.
//! - `arch_nested/services/{web,api}` — the same bytes one level deeper. This is
//!   the A8 hand-off lock: top-level-dir keying collapses it to ONE service
//!   today, `ServiceKeying::ProjectRoots` already splits it. The manifests
//!   (`package.json`, `go.mod`) are what A8.4's walk-time detection will find.
//! - `arch_adjacency/{web,api}` — a Flask server, whose ROUTE nodes carry a
//!   bare-text ROUTE_METHOD cell and can only be placed via HANDLED_BY.

use std::path::PathBuf;

use repo_graph_engine::{
    GenerateResult, ServiceKeying, ServiceMap, generate_many, generate_one, service_map,
    service_map_with,
};

fn fixture(name: &str) -> String {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent()
        .expect("engine/ has a parent")
        .join("tests/fixtures")
        .join(name)
        .to_str()
        .expect("fixture path is UTF-8")
        .to_string()
}

fn build(name: &str) -> GenerateResult {
    generate_one(&fixture(name)).expect("fixture builds")
}

fn ids(map: &ServiceMap) -> Vec<&str> {
    map.services.iter().map(|s| s.id.as_str()).collect()
}

fn service<'a>(map: &'a ServiceMap, id: &str) -> &'a repo_graph_engine::ServiceSummary {
    map.services
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("no service {id:?} in {:?}", ids(map)))
}

/// Every link is `from → to` over HTTP_CALLS; returns the sorted channels.
fn http_channels(map: &ServiceMap, from: &str, to: &str) -> Vec<String> {
    for l in &map.links {
        assert_eq!(
            (l.from.as_str(), l.to.as_str(), l.mechanism),
            (from, to, "HTTP_CALLS"),
            "unexpected link {l:?}"
        );
    }
    let mut ch: Vec<String> = map.links.iter().map(|l| l.channel.clone()).collect();
    ch.sort();
    ch
}

const USERS: [&str; 2] = ["GET /users", "POST /users"];

#[test]
fn monorepo_keys_on_top_level_dir() {
    let r = build("arch_monorepo");
    let map = service_map(&r.merged, &r.repo_labels);
    assert_eq!(map.keying, "top_level_dir", "one repo → key on the directory");
    assert_eq!(ids(&map), ["api", "web"], "services are sorted top-level dirs");
    assert_eq!(http_channels(&map, "web", "api"), USERS);
    assert_eq!(map.self_links, 0);
    assert_eq!(service(&map, "api").languages, ["go"]);
    assert_eq!(service(&map, "web").languages, ["typescript"]);
    assert_eq!(
        (service(&map, "web").outbound, service(&map, "api").inbound),
        (2, 2),
        "in/out count surviving link rows"
    );
}

/// ENDPOINT and ROUTE nodes carry no POSITION cell. Here they are placed by
/// tier 2 — the ENDPOINT_HIT `file` (every language) and the Go ROUTE_METHOD
/// JSON `file`. Drop tier 2 and both counts go to zero.
#[test]
fn route_and_endpoint_are_located_via_cell_tier() {
    let r = build("arch_monorepo");
    let map = service_map(&r.merged, &r.repo_labels);
    assert_eq!(map.unlocated_nodes, 0, "every node placed");
    let (api, web) = (service(&map, "api"), service(&map, "web"));
    // chi's GET and POST on /users are ONE route node with two method cells.
    assert_eq!((api.routes, api.endpoints), (1, 0));
    assert_eq!((web.routes, web.endpoints), (0, 2));
}

/// 14 of 16 ROUTE_METHOD emitters write a bare `Text("GET")`. A Flask route is
/// placed ONLY by the single HANDLED_BY hop to its handler — drop the
/// adjacency pass and both links become `unplaced` and vanish.
#[test]
fn adjacency_tier_places_a_route_with_a_bare_method_cell() {
    let r = build("arch_adjacency");
    let map = service_map(&r.merged, &r.repo_labels);
    assert_eq!(map.unlocated_nodes, 0, "every node placed");
    assert_eq!(ids(&map), ["api", "web"]);
    assert_eq!(service(&map, "api").routes, 2, "Flask: one ROUTE per method");
    assert_eq!(service(&map, "api").languages, ["python"]);
    assert_eq!(http_channels(&map, "web", "api"), USERS);
}

/// KNOWN v1 LIMITATION, pinned on purpose: `TopLevelDir` keys on the FIRST
/// path segment, so `services/web` + `services/api` collapse into one service
/// `services` and the HTTP link is dropped as a self-link. (The same rule lists
/// `.ai`, `docs`, `scripts` and `(root)` as services on a real corpus.)
///
/// A8.4 (project_roots pre-pass) is the fix: once `default_keying` returns
/// `ProjectRoots` from the detected manifests, THIS test must be retargeted —
/// the next test already asserts the destination.
#[test]
fn nested_monorepo_collapses_under_top_level_dir_until_a8_project_roots() {
    let r = build("arch_nested");
    let map = service_map(&r.merged, &r.repo_labels);
    assert_eq!(map.keying, "top_level_dir");
    assert_eq!(ids(&map), ["services"]);
    assert!(map.links.is_empty(), "both ends in one service: {:?}", map.links);
    assert_eq!(map.self_links, 2, "GET + POST /users both dropped as self-links");
}

/// The A8 hand-off lock: same graph, explicit project roots → two services and
/// a live link. A8 only has to make `default_keying` produce these roots.
#[test]
fn project_roots_keying_splits_the_nested_monorepo() {
    let r = build("arch_nested");
    let keying = ServiceKeying::ProjectRoots(vec!["services/web".into(), "services/api".into()]);
    let map = service_map_with(&r.merged, &r.repo_labels, &keying);
    assert_eq!(map.keying, "project_roots");
    assert_eq!(ids(&map), ["services/api", "services/web"]);
    assert_eq!(http_channels(&map, "services/web", "services/api"), USERS);
    assert_eq!(map.self_links, 0);
    assert_eq!(map.unlocated_nodes, 0);
}

#[test]
fn merged_repos_key_per_repo() {
    let r = generate_many(&[fixture("arch_monorepo/web"), fixture("arch_monorepo/api")])
        .expect("fixtures build");
    let map = service_map(&r.merged, &r.repo_labels);
    assert_eq!(map.keying, "per_repo", "≥2 repos → one service per repo");
    // Directory basenames, not `repo<u64>` hashes.
    assert_eq!(ids(&map), ["api", "web"]);
    assert_eq!(service(&map, "api").repo, "api");
    assert_eq!(http_channels(&map, "web", "api"), USERS);
}

/// Guards the HashMap-seed class of bug (`file_index`, `svc_of` and
/// `cross_links` all hold HashMaps): two independent builds and two renders of
/// the same graph must serialise byte-identically.
#[test]
fn service_map_is_deterministic() {
    let render = |r: &GenerateResult| {
        serde_json::to_string(&service_map(&r.merged, &r.repo_labels)).expect("serialises")
    };
    let a = build("arch_monorepo");
    let b = build("arch_monorepo");
    let first = render(&a);
    assert_eq!(first, render(&a), "same graph, two renders");
    assert_eq!(first, render(&b), "two builds of the same tree");
    assert!(first.contains("\"keying\":\"top_level_dir\""), "{first}");
}
