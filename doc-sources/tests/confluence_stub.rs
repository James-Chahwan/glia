//! LA.11 — the Confluence REST transport, driven against the loopback stub.
//!
//! Every request goes over real TCP to `repo_graph_doc_sources::stub`, so these
//! pin the wire shape (targets, Basic auth, JSON bodies, pagination) that only a
//! live Atlassian site could exercise before `--site` accepted an origin.
//! Credentials are always passed explicitly, so neither the process env nor a
//! `./.env` can steer a test at a real site.

use repo_graph_doc_sources::confluence_rest::{self, Config};
use repo_graph_doc_sources::stub::{Canned, StubServer};

/// base64("e@x:t") — the Basic credential every stubbed call must carry.
const AUTH: &str = "Basic ZUB4OnQ=";

fn cfg(site: &str) -> Result<Config, String> {
    Config::resolve(
        Some(site.to_string()),
        Some("e@x".to_string()),
        Some("t".to_string()),
    )
}

fn page_json(n: usize, version: i64) -> serde_json::Value {
    serde_json::json!({
        "id": n.to_string(),
        "title": format!("Page {n}"),
        "space": { "key": "K" },
        "version": { "number": version },
        "body": { "storage": { "value": format!("<p>body {n}</p>") } },
        "_links": { "webui": format!("/spaces/K/pages/{n}") },
    })
}

fn results(range: std::ops::Range<usize>) -> Canned {
    let rows: Vec<_> = range.map(|n| page_json(n, 1)).collect();
    Canned::ok(serde_json::json!({ "results": rows }).to_string())
}

#[test]
fn pull_space_paginates_and_sends_basic_auth() {
    let stub = StubServer::start(vec![results(0..100), results(100..101)]).expect("stub binds");
    let origin = stub.origin();
    let pages = confluence_rest::pull_space(&cfg(&origin).expect("loopback origin resolves"), "K");
    let seen = stub.finish();
    let pages = pages.expect("pull_space succeeds against the stub");

    assert_eq!(
        seen.len(),
        2,
        "one GET per 100-row page, stopping on the short one"
    );
    for (req, start) in seen.iter().zip([0, 100]) {
        assert_eq!(req.method, "GET");
        assert_eq!(
            req.target,
            format!(
                "/wiki/rest/api/space/K/content/page?limit=100&start={start}&expand=body.storage,version"
            )
        );
        assert_eq!(req.header("authorization"), Some(AUTH));
        assert_eq!(req.header("accept"), Some("application/json"));
    }
    assert_eq!(pages.len(), 101);
    assert_eq!(pages[100].title, "Page 100");
    assert_eq!(pages[100].space, "K");
    assert_eq!(pages[100].storage, "<p>body 100</p>");
    assert_eq!(pages[100].url, format!("{origin}/wiki/spaces/K/pages/100"));
}

#[test]
fn fetch_page_decodes_version_and_space() {
    let stub =
        StubServer::start(vec![Canned::ok(page_json(42, 7).to_string())]).expect("stub binds");
    let origin = stub.origin();
    let page = confluence_rest::fetch_page(&cfg(&origin).expect("resolves"), "42");
    let seen = stub.finish();
    let page = page.expect("fetch_page succeeds against the stub");

    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(
        seen[0].target,
        "/wiki/rest/api/content/42?expand=body.storage,version,space"
    );
    assert_eq!(seen[0].header("authorization"), Some(AUTH));
    assert_eq!(page.space, "K");
    assert_eq!(page.version, "7");
    assert_eq!(page.title, "Page 42");
    assert_eq!(page.storage, "<p>body 42</p>");
    assert_eq!(page.url, format!("{origin}/wiki/spaces/K/pages/42"));
}

#[test]
fn create_page_posts_storage_body() {
    let stub =
        StubServer::start(vec![Canned::ok(page_json(9, 1).to_string())]).expect("stub binds");
    let origin = stub.origin();
    let made =
        confluence_rest::create_page(&cfg(&origin).expect("resolves"), "K", "Page 9", "<p>hi</p>");
    let seen = stub.finish();
    let made = made.expect("create_page succeeds against the stub");

    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].target, "/wiki/rest/api/content");
    assert_eq!(seen[0].header("authorization"), Some(AUTH));
    assert!(
        seen[0]
            .header("content-type")
            .is_some_and(|v| v.starts_with("application/json")),
        "POST body must be declared JSON: {:?}",
        seen[0].header("content-type")
    );
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).expect("POST body is JSON");
    assert_eq!(body["type"], "page");
    assert_eq!(body["title"], "Page 9");
    assert_eq!(body["space"]["key"], "K");
    assert_eq!(body["body"]["storage"]["representation"], "storage");
    assert_eq!(body["body"]["storage"]["value"], "<p>hi</p>");
    assert!(body.get("version").is_none(), "a create carries no version");

    assert_eq!(made.id, "9");
    assert_eq!(made.version, 1);
    assert_eq!(made.url, format!("{origin}/wiki/spaces/K/pages/9"));
}

#[test]
fn update_page_bumps_version() {
    let stub = StubServer::start(vec![
        Canned::ok(page_json(5, 2).to_string()),
        Canned::ok(page_json(5, 3).to_string()),
    ])
    .expect("stub binds");
    let origin = stub.origin();
    let updated = confluence_rest::update_page(
        &cfg(&origin).expect("resolves"),
        "5",
        "K",
        "Page 5",
        "<p>v3</p>",
    );
    let seen = stub.finish();
    let updated = updated.expect("update_page succeeds against the stub");

    assert_eq!(
        seen.len(),
        2,
        "GET the current version, then PUT the next one"
    );
    assert_eq!(seen[0].method, "GET");
    assert_eq!(
        seen[0].target,
        "/wiki/rest/api/content/5?expand=body.storage,version,space"
    );
    assert_eq!(seen[1].method, "PUT");
    assert_eq!(seen[1].target, "/wiki/rest/api/content/5");
    assert_eq!(seen[1].header("authorization"), Some(AUTH));
    let body: serde_json::Value = serde_json::from_str(&seen[1].body).expect("PUT body is JSON");
    assert_eq!(body["id"], "5");
    assert_eq!(body["version"]["number"], 3);
    assert_eq!(body["body"]["storage"]["value"], "<p>v3</p>");

    assert_eq!(updated.version, 3);
    assert_eq!(updated.url, format!("{origin}/wiki/spaces/K/pages/5"));
}

#[test]
fn http_error_is_reported_with_status() {
    let stub = StubServer::start(vec![Canned {
        status: 404,
        body: "Site temporarily unavailable".into(),
    }])
    .expect("stub binds");
    let origin = stub.origin();
    let pulled = confluence_rest::pull_space(&cfg(&origin).expect("resolves"), "K");
    let seen = stub.finish();

    assert_eq!(
        seen.len(),
        1,
        "the error comes from the stub, not from the transport"
    );
    let err = pulled.err().expect("a 404 is an error");
    assert!(
        err.starts_with("HTTP 404:"),
        "status must lead the error: {err}"
    );
    assert!(
        err.contains("Site temporarily unavailable"),
        "body must be kept: {err}"
    );
}

#[test]
fn plain_http_to_remote_host_is_refused() {
    for site in [
        "http://example.com",
        "http://example.com:8080/",
        "http://127.0.0.1.example.com",
        // userinfo that names a loopback host while the real host is remote
        "http://127.0.0.1:80@example.com",
        "http://localhost@example.com",
        "http://127.0.0.1:8080/wiki",
    ] {
        let err = cfg(site)
            .err()
            .unwrap_or_else(|| panic!("{site} must be refused"));
        assert!(err.contains("plain http://"), "{site}: {err}");
    }
    for site in [
        "http://127.0.0.1:9",
        "http://localhost:9/",
        "http://[::1]:9",
        "http://LOCALHOST",
    ] {
        assert!(cfg(site).is_ok(), "{site} is loopback and must be accepted");
    }
    // Config's fields are public, so the check also guards the request path: a
    // hand-built Config never sends Basic credentials over plain http to a
    // remote host (this call fails before any connection is attempted).
    let hand_built = Config {
        site: "http://example.com".into(),
        email: "e@x".into(),
        token: "t".into(),
    };
    let err = confluence_rest::pull_space(&hand_built, "K")
        .err()
        .expect("refused");
    assert!(err.contains("refusing plain http://"), "{err}");
}

#[test]
fn bare_host_still_means_https() {
    let with = |site: &str| Config {
        site: site.into(),
        email: "e@x".into(),
        token: "t".into(),
    };
    // Every CONFLUENCE_SITE written before origins were accepted keeps working.
    assert_eq!(
        with("acme.atlassian.net").origin(),
        "https://acme.atlassian.net"
    );
    assert_eq!(with("127.0.0.1:8443").origin(), "https://127.0.0.1:8443");
    // An explicit https origin is taken as given, minus a trailing slash.
    assert_eq!(
        with("https://acme.atlassian.net/").origin(),
        "https://acme.atlassian.net"
    );
    assert_eq!(
        with("HTTPS://acme.atlassian.net").origin(),
        "https://acme.atlassian.net"
    );
    assert_eq!(with("http://127.0.0.1:9/").origin(), "http://127.0.0.1:9");
    assert!(cfg("acme.atlassian.net").is_ok());
    assert!(cfg("https://acme.atlassian.net").is_ok());
}
