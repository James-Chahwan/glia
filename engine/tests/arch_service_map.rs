//! A9.2 fired_on proof: the `[arch]` marker + a real monorepo splitting into
//! ≥2 services on a REAL build, not a hand-built graph.
//!
//! `bench/substrate-gap/fixtures/xstack-ts-go-http` is one directory holding a
//! TS client and a Go server. Under `generate_one` it is a single repo, which
//! is exactly the case that used to render as ONE service with ZERO links while
//! the `HTTP_CALLS` cross-edge sat in the graph. Run with `-- --nocapture` to
//! see the marker.

use repo_graph_engine::{generate_one, service_map};

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/xstack-ts-go-http"
    )
    .to_string()
}

#[test]
fn single_repo_monorepo_splits_into_services_with_links() {
    let r = generate_one(&fixture()).expect("fixture builds");
    assert_eq!(
        r.repo_labels.len(),
        1,
        "generate_one keeps exactly one human repo label"
    );
    assert_eq!(
        r.repo_labels.values().next().map(String::as_str),
        Some("xstack-ts-go-http"),
        "the label is the directory name, not the xxhash of the path"
    );

    let map = service_map(&r.merged, &r.repo_labels);
    assert_eq!(map.keying, "top_level_dir", "one repo → key on the directory");
    let ids: Vec<&str> = map.services.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"client") && ids.contains(&"server"),
        "expected client + server services, got {ids:?}"
    );
    assert!(
        !map.links.is_empty(),
        "the HTTP_CALLS cross-edge must surface as a service link; got 0 links \
         over services {ids:?}"
    );
    let http: Vec<&str> = map
        .links
        .iter()
        .filter(|l| l.mechanism == "HTTP_CALLS")
        .map(|l| l.channel.as_str())
        .collect();
    assert!(
        !http.is_empty() && http.iter().all(|c| c.contains("/users")),
        "expected HTTP_CALLS links over the /users channels, got {http:?}"
    );

    // The ENDPOINT / ROUTE nodes are the ones with no POSITION cell — if
    // `node_file` tier 2 regressed they would be unlocated and both services
    // would lose their route/endpoint counts.
    let routes: usize = map.services.iter().map(|s| s.routes).sum();
    let endpoints: usize = map.services.iter().map(|s| s.endpoints).sum();
    assert!(routes >= 1 && endpoints >= 1, "routes={routes} endpoints={endpoints}");
}
