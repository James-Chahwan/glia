//! CE.4d — the MediaWiki Action API adapter, driven against the LA.11 loopback
//! stub (`glia_doc_sources::stub`).
//!
//! Every request goes over real TCP, so these pin the wire shape a live wiki
//! would see: the query parameters (`formatversion=2`, `rvslots=main`,
//! `maxlag=5`, the generator), the `continue` echo, the User-Agent and bearer
//! token, and the maxlag / 429 / 503 retries. The response bodies follow the
//! documented `formatversion=2` shape (mediawiki.org API:Query, API:Revisions,
//! API:Info). The token is always passed explicitly (or the Config is built by
//! hand), so neither the process env nor a `./.env` can steer a test.

use glia_code_domain::DocSourceKind;
use glia_doc_sources::PageBody;
use glia_doc_sources::mediawiki::{self, Config, PullStats, Selection};
use glia_doc_sources::stub::{Canned, Recorded, StubServer};

fn cfg(origin: &str) -> Config {
    Config::resolve(
        &format!("{origin}/w/api.php"),
        Some("tok".to_string()),
        Some("ops-wiki".to_string()),
        "9.9.9",
    )
    .expect("a loopback api resolves")
}

/// The wiki's public origin in the canned `fullurl`s: a page url is data the
/// adapter stores, never a URL it fetches.
const WIKI: &str = "https://ops.example";

/// One `query.pages` entry of a `formatversion=2` response, wikitext.
fn page(pageid: u64, title: &str, revid: u64, content: &str) -> serde_json::Value {
    page_model(pageid, title, revid, content, "wikitext")
}

fn page_model(
    pageid: u64,
    title: &str,
    revid: u64,
    content: &str,
    model: &str,
) -> serde_json::Value {
    serde_json::json!({
        "pageid": pageid,
        "ns": 0,
        "title": title,
        "contentmodel": model,
        "pagelanguage": "en",
        "pagelanguagehtmlcode": "en",
        "pagelanguagedir": "ltr",
        "touched": "2026-09-01T00:00:00Z",
        "lastrevid": revid,
        "length": content.len(),
        "fullurl": format!("{WIKI}/wiki/{}", title.replace(' ', "_")),
        "editurl": format!("{WIKI}/w/index.php?title={}&action=edit", title.replace(' ', "_")),
        "canonicalurl": format!("{WIKI}/wiki/{}", title.replace(' ', "_")),
        "revisions": [{
            "revid": revid,
            "parentid": 0,
            "timestamp": "2026-09-01T00:00:00Z",
            "slots": { "main": {
                "contentmodel": model,
                "contentformat": "text/x-wiki",
                "content": content,
            }},
        }],
    })
}

fn batch(pages: Vec<serde_json::Value>, cont: Option<serde_json::Value>) -> Canned {
    let mut v = serde_json::json!({ "query": { "pages": pages } });
    match cont {
        Some(c) => v["continue"] = c,
        None => v["batchcomplete"] = serde_json::Value::Bool(true),
    }
    Canned::ok(v.to_string())
}

fn assert_etiquette(req: &Recorded) {
    assert_eq!(req.method, "GET");
    assert!(
        req.target.starts_with("/w/api.php?action=query&"),
        "{}",
        req.target
    );
    for needle in ["formatversion=2", "rvslots=main", "maxlag=5", "format=json"] {
        assert!(
            req.target.contains(needle),
            "{needle} missing from {}",
            req.target
        );
    }
    assert_eq!(req.header("user-agent"), Some("glia/9.9.9 (docs sync)"));
    assert_eq!(req.header("authorization"), Some("Bearer tok"));
}

fn titles(pages: &[glia_doc_sources::Page]) -> Vec<&str> {
    pages.iter().map(|p| p.title.as_str()).collect()
}

#[test]
fn paginates_with_continue() {
    let stub = StubServer::start(vec![
        batch(
            vec![
                page(
                    1,
                    "Order Flow",
                    11,
                    "== Flow ==\n<code>OrderService</code> places orders.\n",
                ),
                page(2, "Deploy", 12, "Run the deploy.\n"),
            ],
            Some(serde_json::json!({ "gapcontinue": "Zeta", "continue": "gapcontinue||" })),
        ),
        batch(vec![page(3, "Zeta", 13, "Last page.\n")], None),
    ])
    .expect("stub binds");
    let pulled = mediawiki::pull(&cfg(&stub.origin()), &Selection::Namespace(0), 5000);
    let seen = stub.finish();
    let (pages, stats) = pulled.expect("pull succeeds against the stub");

    assert_eq!(
        seen.len(),
        2,
        "one request per batch, stopping without a continue"
    );
    for req in &seen {
        assert_etiquette(req);
        assert!(
            req.target
                .contains("&prop=revisions%7Cinfo&rvprop=content%7Cids%7Ctimestamp&"),
            "{}",
            req.target
        );
        assert!(
            req.target.contains("&generator=allpages&gapnamespace=0&"),
            "{}",
            req.target
        );
        assert!(req.target.contains("&gaplimit=50"), "{}", req.target);
        assert!(
            !req.target.contains("filterredir"),
            "redirects are dropped client-side, never filtered after the limit: {}",
            req.target
        );
    }
    assert!(
        !seen[0].target.contains("gapcontinue"),
        "{}",
        seen[0].target
    );
    assert!(
        seen[1].target.contains("&gapcontinue=Zeta"),
        "{}",
        seen[1].target
    );
    assert!(
        seen[1].target.contains("&continue=gapcontinue%7C%7C"),
        "{}",
        seen[1].target
    );

    assert_eq!(
        titles(&pages),
        ["Deploy", "Order Flow", "Zeta"],
        "sorted by title within each batch"
    );
    let versions: Vec<&str> = pages.iter().map(|p| p.version.as_str()).collect();
    assert_eq!(versions, ["12", "11", "13"]);
    let flow = &pages[1];
    assert_eq!(flow.kind, DocSourceKind::Wiki);
    assert_eq!(flow.container, "ops-wiki");
    assert_eq!(
        flow.url,
        format!("{WIKI}/wiki/Order_Flow"),
        "the url is the API's fullurl"
    );
    assert!(flow.slug_hint.is_none());
    assert!(
        matches!(&flow.body, PageBody::Wikitext(s) if s.starts_with("== Flow ==\n<code>OrderService</code>")),
        "the wikitext is carried as is"
    );
    assert_eq!(
        stats,
        PullStats {
            fetched: 3,
            pages: 3,
            requests: 2,
            wikitext: stats.wikitext,
            ..PullStats::default()
        }
    );
    assert_eq!(
        (stats.wikitext.headings, stats.wikitext.inline_code),
        (1, 1)
    );
    assert_eq!(
        stats.wikitext_marker().as_deref(),
        Some(
            "[docs] wikitext pages=3 headings=1 code_blocks=0 inline_code=1 links=0 templates_dropped=0 unbalanced=0"
        )
    );
}

#[test]
fn category_selection() {
    let stub = StubServer::start(vec![batch(Vec::new(), None), batch(Vec::new(), None)])
        .expect("stub binds");
    let origin = stub.origin();
    let config = cfg(&origin);
    let plain = mediawiki::pull(&config, &Selection::Category("Runbooks".into()), 5000);
    let prefixed = mediawiki::pull(
        &config,
        &Selection::Category("Category:Release Runbooks".into()),
        5000,
    );
    let seen = stub.finish();

    let (pages, stats) = plain.expect("an empty category is not an error");
    assert!(pages.is_empty());
    assert_eq!((stats.fetched, stats.requests), (0, 1));
    assert!(prefixed.is_ok());
    assert_eq!(seen.len(), 2);
    for req in &seen {
        assert_etiquette(req);
        assert!(
            req.target.contains("&gcmtype=page&gcmlimit=50"),
            "{}",
            req.target
        );
        assert!(!req.target.contains("allpages"), "{}", req.target);
    }
    assert!(
        seen[0]
            .target
            .contains("&generator=categorymembers&gcmtitle=Category%3ARunbooks&"),
        "{}",
        seen[0].target
    );
    assert!(
        seen[1]
            .target
            .contains("&gcmtitle=Category%3ARelease%20Runbooks&"),
        "a `Category:` prefix is not doubled: {}",
        seen[1].target
    );
    assert_eq!(
        Selection::Category("Runbooks".into()).label(),
        "category:Runbooks"
    );
    assert_eq!(Selection::Namespace(12).label(), "ns:12");
}

#[test]
fn maxlag_is_retried() {
    let lagged = serde_json::json!({ "error": {
        "code": "maxlag",
        "info": "Waiting for db1: 7 seconds lagged.",
        "host": "db1",
        "lag": 7,
    }});
    let stub = StubServer::start(vec![
        Canned::ok(lagged.to_string()).with_header("Retry-After", "0"),
        Canned::status(429, "slow down").with_header("Retry-After", "0"),
        Canned::status(503, "busy").with_header("Retry-After", "0"),
        batch(vec![page(1, "Home", 5, "Welcome.\n")], None),
    ])
    .expect("stub binds");
    let pulled = mediawiki::pull(&cfg(&stub.origin()), &Selection::Namespace(0), 5000);
    let seen = stub.finish();
    let (pages, stats) = pulled.expect("maxlag, 429 and 503 are retried");
    assert_eq!(seen.len(), 4);
    assert!(
        seen.iter().all(|r| r.target == seen[0].target),
        "a retry resends the same request"
    );
    assert_eq!(titles(&pages), ["Home"]);
    assert_eq!((stats.requests, stats.retries), (4, 3));
}

#[test]
fn retries_are_capped_at_three() {
    let lagged = serde_json::json!({ "error": { "code": "maxlag", "info": "lagged" } }).to_string();
    let stub = StubServer::start(
        (0..4)
            .map(|_| Canned::ok(lagged.clone()).with_header("Retry-After", "0"))
            .collect(),
    )
    .expect("stub binds");
    let pulled = mediawiki::pull(&cfg(&stub.origin()), &Selection::Namespace(0), 5000);
    let seen = stub.finish();
    assert_eq!(seen.len(), 4, "the first try plus three retries");
    let err = pulled.err().expect("still lagged after three retries");
    assert!(err.contains("maxlag") && err.contains("3 retries"), "{err}");
}

#[test]
fn api_errors_are_reported() {
    let denied = serde_json::json!({ "error": {
        "code": "readapidenied",
        "info": "You need read permission to use this module.",
    }});
    let stub = StubServer::start(vec![
        Canned::ok(denied.to_string()),
        Canned::status(404, "no such wiki"),
        Canned::status(301, "").with_header("Location", "https://elsewhere.example/w/api.php"),
    ])
    .expect("stub binds");
    let config = cfg(&stub.origin());
    let api_error = mediawiki::pull(&config, &Selection::Namespace(0), 10);
    let http_error = mediawiki::pull(&config, &Selection::Namespace(0), 10);
    let redirect = mediawiki::pull(&config, &Selection::Namespace(0), 10);
    let seen = stub.finish();
    assert_eq!(seen.len(), 3, "no error is retried, no redirect followed");
    assert_eq!(
        api_error.err().as_deref(),
        Some("readapidenied: You need read permission to use this module.")
    );
    let err = http_error.err().unwrap_or_default();
    assert!(
        err.starts_with("HTTP 404:") && err.contains("no such wiki"),
        "{err}"
    );
    let err = redirect.err().unwrap_or_default();
    assert!(
        err.contains("HTTP 301") && err.contains("https://elsewhere.example/w/api.php"),
        "{err}"
    );
}

#[test]
fn plain_http_to_remote_is_refused() {
    for api in [
        "http://wiki.example/w/api.php",
        "http://127.0.0.1.wiki.example/w/api.php",
        "http://127.0.0.1:80@wiki.example/w/api.php",
    ] {
        let err = Config::resolve(api, Some("tok".into()), None, "t")
            .err()
            .unwrap_or_else(|| panic!("{api} must be refused"));
        assert!(
            err.contains("plain http://") || err.contains("--token"),
            "{api}: {err}"
        );
    }
    // Config's fields are public: a hand-built one is refused by pull too,
    // before any connection is attempted.
    let hand_built = Config {
        api: "http://wiki.example/w/api.php".into(),
        token: Some("tok".into()),
        container: "w".into(),
        user_agent: "glia/t (docs sync)".into(),
    };
    let err = mediawiki::pull(&hand_built, &Selection::Namespace(0), 10)
        .err()
        .expect("refused");
    assert!(err.contains("refusing plain http://"), "{err}");

    for (api, why) in [
        ("wiki.example/w/api.php", "https://"),
        ("https://wiki.example", "api.php"),
        ("https://wiki.example/w/api.php?action=query", "query"),
        ("https://user:pw@wiki.example/w/api.php", "--token"),
    ] {
        let err = Config::resolve(api, None, None, "t")
            .err()
            .unwrap_or_default();
        assert!(err.contains(why), "{api}: {err}");
    }
}

#[test]
fn container_defaults_to_the_api_host() {
    let c = Config::resolve("https://Wiki.Example:8443/w/api.php", None, None, "t")
        .expect("https resolves");
    assert_eq!(c.container, "wiki.example");
    assert_eq!(c.user_agent, "glia/t (docs sync)");
    let err = Config::resolve("http://[::1]:9/w/api.php", None, None, "t")
        .err()
        .unwrap_or_default();
    assert!(err.contains("--container"), "{err}");
    let c = Config::resolve("http://[::1]:9/w/api.php", None, Some("local".into()), "t")
        .expect("an explicit container");
    assert_eq!(c.container, "local");
    assert!(Config::resolve("https://w.example/api.php", None, Some("a/b".into()), "t").is_err());
}

#[test]
fn non_wikitext_is_skipped() {
    let mut redirect = page(3, "Old Flow", 23, "#REDIRECT [[Order Flow]]\n");
    redirect["redirect"] = serde_json::Value::Bool(true);
    let missing = serde_json::json!({ "ns": 0, "title": "Gone", "missing": true });
    let stub = StubServer::start(vec![batch(
        vec![
            page_model(1, "Common.css", 21, "body { color: red }", "css"),
            page(2, "Runbook", 22, "== Steps ==\n"),
            redirect,
            missing,
        ],
        None,
    )])
    .expect("stub binds");
    let pulled = mediawiki::pull(&cfg(&stub.origin()), &Selection::Namespace(0), 5000);
    stub.finish();
    let (pages, stats) = pulled.expect("pull succeeds");
    assert_eq!(titles(&pages), ["Runbook"]);
    assert_eq!(stats.skipped_model, 1);
    assert_eq!((stats.skipped_redirect, stats.skipped_missing), (1, 1));
    assert_eq!((stats.fetched, stats.pages, stats.skipped()), (4, 1, 3));
}

#[test]
fn max_pages_truncates() {
    let stub = StubServer::start(vec![batch(
        vec![page(1, "B", 2, "b\n"), page(2, "A", 1, "a\n")],
        Some(serde_json::json!({ "gapcontinue": "C", "continue": "gapcontinue||" })),
    )])
    .expect("stub binds");
    let pulled = mediawiki::pull(&cfg(&stub.origin()), &Selection::Namespace(0), 1);
    let seen = stub.finish();
    let (pages, stats) = pulled.expect("pull succeeds");
    assert_eq!(seen.len(), 1, "no request after the cap");
    assert_eq!(titles(&pages), ["A"]);
    assert!(stats.truncated);
    assert_eq!((stats.fetched, stats.requests), (1, 1));
    assert!(
        seen[0].target.ends_with("&gaplimit=1"),
        "a cap below one batch lists no more: {}",
        seen[0].target
    );
    let zero = mediawiki::pull(&cfg("http://127.0.0.1:9"), &Selection::Namespace(0), 0);
    assert!(zero.is_err(), "max_pages 0 is refused before any request");
}

/// A response too large for one batch leaves some pages without `revisions`
/// and continues them with `rvcontinue` while the generator stays put; the
/// next batch repeats the finished pages (without revisions) beside the
/// rest. Each page is counted and kept once.
#[test]
fn revision_continuation_fills_pending_pages() {
    let bare = |id: u64, title: &str| serde_json::json!({ "pageid": id, "ns": 0, "title": title, "contentmodel": "wikitext" });
    let stub = StubServer::start(vec![
        batch(
            vec![page(1, "A", 11, "a\n"), bare(2, "B")],
            Some(serde_json::json!({ "rvcontinue": "2|22", "continue": "||" })),
        ),
        batch(vec![bare(1, "A"), page(2, "B", 22, "b\n")], None),
    ])
    .expect("stub binds");
    // At the cap (2 listed) the revision continuation is still followed:
    // B is listed, only its content is outstanding.
    let pulled = mediawiki::pull(&cfg(&stub.origin()), &Selection::Namespace(0), 2);
    let seen = stub.finish();
    let (pages, stats) = pulled.expect("pull succeeds");
    assert!(!stats.truncated);
    assert!(
        seen[1].target.contains("&rvcontinue=2%7C22"),
        "{}",
        seen[1].target
    );
    assert_eq!(titles(&pages), ["A", "B"]);
    assert_eq!((stats.fetched, stats.pages, stats.skipped()), (2, 2, 0));
}

#[test]
fn no_token_sends_no_authorization() {
    let stub = StubServer::start(vec![batch(Vec::new(), None)]).expect("stub binds");
    let config = Config {
        api: format!("{}/w/api.php", stub.origin()),
        token: None,
        container: "w".into(),
        user_agent: "glia/t (docs sync)".into(),
    };
    let pulled = mediawiki::pull(&config, &Selection::Namespace(0), 10);
    let seen = stub.finish();
    assert!(pulled.is_ok());
    assert_eq!(seen[0].header("authorization"), None);
    assert_eq!(seen[0].header("user-agent"), Some("glia/t (docs sync)"));
}

/// A continuation that does not move would loop forever: refused.
#[test]
fn a_stuck_continuation_is_an_error() {
    let cont = serde_json::json!({ "gapcontinue": "B", "continue": "gapcontinue||" });
    let stub = StubServer::start(vec![
        batch(Vec::new(), Some(cont.clone())),
        batch(Vec::new(), Some(cont)),
    ])
    .expect("stub binds");
    let pulled = mediawiki::pull(&cfg(&stub.origin()), &Selection::Namespace(0), 10);
    let seen = stub.finish();
    assert_eq!(seen.len(), 2);
    let err = pulled.err().unwrap_or_default();
    assert!(err.contains("continuation"), "{err}");
}

/// Continuations that keep moving but never bring a page are followed at most
/// ten times in a row.
#[test]
fn an_idle_continuation_is_bounded() {
    let stub = StubServer::start(
        (0..10)
            .map(|n| {
                batch(
                    Vec::new(),
                    Some(serde_json::json!({ "gapcontinue": format!("P{n}"), "continue": "gapcontinue||" })),
                )
            })
            .collect(),
    )
    .expect("stub binds");
    let pulled = mediawiki::pull(&cfg(&stub.origin()), &Selection::Namespace(0), 10);
    let seen = stub.finish();
    assert_eq!(seen.len(), 10);
    let err = pulled.err().unwrap_or_default();
    assert!(err.contains("10 continuations in a row"), "{err}");
}
