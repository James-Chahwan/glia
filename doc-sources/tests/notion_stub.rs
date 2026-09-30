//! CE.4e — the Notion adapter, driven against the LA.11 loopback stub
//! (`glia_doc_sources::stub`).
//!
//! Every request goes over real TCP, so these pin the wire shape a live
//! workspace would see: `GET /v1/databases/{id}`, `POST
//! /v1/data_sources/{id}/query` with `page_size` / `start_cursor`, `GET
//! /v1/blocks/{page}/children?page_size=100`, the `Notion-Version: 2025-09-03`
//! and `Authorization: Bearer` headers, and Notion's error JSON. The bodies
//! follow the documented 2025-09-03 shapes (developers.notion.com: Retrieve a
//! database, Query a data source, Retrieve block children). The token is
//! always passed explicitly (or the Config is built by hand), so neither the
//! process env nor a `./.env` can steer a test.

use glia_code_domain::DocSourceKind;
use glia_doc_sources::notion::{self, Config, NOTION_VERSION, NotionStats};
use glia_doc_sources::record_from_page;
use glia_doc_sources::stub::{Canned, Recorded, StubServer};
use serde_json::{Value, json};

const TOKEN: &str = "secret_t";
const PAGE_A: &str = "1a2b3c4d-0000-4000-8000-00000000000a";
const PAGE_B: &str = "1a2b3c4d-0000-4000-8000-00000000000b";

fn cfg(origin: &str) -> Config {
    Config::resolve(Some(TOKEN.to_string()), Some(origin.to_string()))
        .expect("a loopback origin resolves")
}

fn database(sources: &[&str]) -> Canned {
    let ds: Vec<Value> = sources
        .iter()
        .map(|id| json!({ "id": id, "name": "Docs" }))
        .collect();
    Canned::ok(
        json!({
            "object": "database",
            "id": "db1",
            "title": [{ "type": "text", "plain_text": "Engineering docs" }],
            "data_sources": ds,
        })
        .to_string(),
    )
}

/// A data source query result page (2025-09-03 page object).
fn page(id: &str, title: &str, created: &str) -> Value {
    json!({
        "object": "page",
        "id": id,
        "created_time": created,
        "last_edited_time": "2026-09-20T10:00:00.000Z",
        "archived": false,
        "in_trash": false,
        "parent": { "type": "data_source_id", "data_source_id": "ds1", "database_id": "db1" },
        "url": format!("https://www.notion.so/{}", id.replace('-', "")),
        "properties": {
            "Status": { "id": "s", "type": "status", "status": null },
            "Name": { "id": "title", "type": "title", "title": [
                { "type": "text", "text": { "content": title, "link": null },
                  "annotations": { "bold": false, "code": false }, "plain_text": title, "href": null },
            ]},
        },
    })
}

fn list(results: Vec<Value>, next: Option<&str>) -> Canned {
    Canned::ok(
        json!({
            "object": "list",
            "results": results,
            "has_more": next.is_some(),
            "next_cursor": next,
            "type": "page_or_data_source",
        })
        .to_string(),
    )
}

fn span(content: &str, code: bool) -> Value {
    json!({
        "type": "text",
        "text": { "content": content, "link": null },
        "annotations": { "bold": false, "italic": false, "strikethrough": false,
                         "underline": false, "code": code, "color": "default" },
        "plain_text": content,
        "href": null,
    })
}

fn block(kind: &str, body: Value) -> Value {
    json!({ "object": "block", "id": format!("blk-{kind}"), "type": kind,
            "has_children": false, "archived": false, "in_trash": false, kind: body })
}

/// The acceptance page body: `## Orders`, a paragraph naming
/// `OrderService.place` in code, and a python code block.
fn order_blocks(line: &str) -> Canned {
    list(
        vec![
            block("heading_2", json!({ "rich_text": [span("Orders", false)], "is_toggleable": false })),
            block(
                "paragraph",
                json!({ "rich_text": [span(line, false), span("OrderService.place", true)] }),
            ),
            block(
                "code",
                json!({ "rich_text": [span("svc.place(order)\n", false)], "language": "python", "caption": [] }),
            ),
        ],
        None,
    )
}

fn assert_headers(req: &Recorded) {
    assert_eq!(req.header("notion-version"), Some(NOTION_VERSION), "{}", req.target);
    assert_eq!(req.header("notion-version"), Some("2025-09-03"));
    assert_eq!(req.header("authorization"), Some("Bearer secret_t"), "{}", req.target);
    assert_eq!(req.header("content-type"), Some("application/json"), "{}", req.target);
}

fn body(req: &Recorded) -> Value {
    serde_json::from_str(&req.body).expect("a JSON request body")
}

#[test]
fn pulls_a_database() {
    // B is listed first but created later: pages come back in created order.
    let stub = StubServer::start(vec![
        database(&["ds1"]),
        list(vec![page(PAGE_B, "Shipping", "2026-09-02T00:00:00.000Z")], Some("c2")),
        list(vec![page(PAGE_A, "Order Flow", "2026-09-01T00:00:00.000Z")], None),
        order_blocks("Calls "),
        order_blocks("Ships after "),
    ])
    .expect("stub binds");
    let pulled = notion::pull_database(&cfg(&stub.origin()), "db1", notion::DEFAULT_MAX_PAGES);
    let seen = stub.finish();
    let (pages, stats) = pulled.expect("pull succeeds");

    assert_eq!(seen.len(), 5);
    seen.iter().for_each(assert_headers);
    assert_eq!((seen[0].method.as_str(), seen[0].target.as_str()), ("GET", "/v1/databases/db1"));
    for req in &seen[1..3] {
        assert_eq!((req.method.as_str(), req.target.as_str()), ("POST", "/v1/data_sources/ds1/query"));
    }
    assert_eq!(body(&seen[1]), json!({ "page_size": 100 }));
    assert_eq!(body(&seen[2]), json!({ "page_size": 100, "start_cursor": "c2" }));
    assert_eq!(seen[3].method, "GET");
    assert_eq!(seen[3].target, format!("/v1/blocks/{PAGE_A}/children?page_size=100"));
    assert_eq!(seen[4].target, format!("/v1/blocks/{PAGE_B}/children?page_size=100"));

    assert_eq!(
        stats,
        NotionStats {
            data_sources: 1,
            fetched: 2,
            pages: 2,
            skipped_archived: 0,
            blocks: 6,
            unsupported: Default::default(),
            requests: 5,
            truncated: false,
        }
    );
    assert_eq!(
        stats.sync_marker("db1", 2),
        "[docs] sync source=notion database=db1 data_sources=1 fetched=2 kept=2 blocks=6 unsupported=0 requests=5"
    );
    assert_eq!(pages.len(), 2);
    let first = &pages[0];
    assert_eq!(first.kind, DocSourceKind::Notion);
    assert_eq!(first.container, "db1");
    assert_eq!(first.title, "Order Flow");
    assert_eq!(first.url, format!("https://www.notion.so/{}", PAGE_A.replace('-', "")));
    assert_eq!(first.version, "2026-09-20T10:00:00.000Z");
    assert_eq!(pages[1].title, "Shipping");

    let (record, redacted) = record_from_page(first);
    assert_eq!(redacted, 0);
    assert_eq!(record.rel_path, "notion/db1/order-flow.md");
    for needle in ["# Order Flow\n", "## Orders", "Calls `OrderService.place`", "```python\nsvc.place(order)\n```"] {
        assert!(record.text.contains(needle), "{needle:?} missing from\n{}", record.text);
    }
}

#[test]
fn unsupported_blocks_are_counted() {
    let mut archived = page(PAGE_B, "Old", "2026-09-02T00:00:00.000Z");
    archived["in_trash"] = json!(true);
    let stub = StubServer::start(vec![
        database(&["ds1"]),
        list(vec![page(PAGE_A, "Guide", "2026-09-01T00:00:00.000Z"), archived], None),
        list(
            vec![
                block("paragraph", json!({ "rich_text": [span("Read ", false), span("Guide", true)] })),
                block("image", json!({ "type": "external", "external": { "url": "https://img.example/a.png" }, "caption": [] })),
                block("table_of_contents", json!({ "color": "default" })),
            ],
            None,
        ),
    ])
    .expect("stub binds");
    let pulled = notion::pull_database(&cfg(&stub.origin()), "db1", 10);
    let seen = stub.finish();
    let (pages, stats) = pulled.expect("pull succeeds");

    assert_eq!(seen.len(), 3, "no blocks are read for the trashed page");
    assert_eq!(body(&seen[1]), json!({ "page_size": 10 }), "a pull capped below 100 asks for no more");
    assert_eq!(
        stats.unsupported.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(),
        [("image", 1), ("table_of_contents", 1)]
    );
    assert_eq!(stats.unsupported_total(), 2);
    assert_eq!((stats.fetched, stats.pages, stats.skipped_archived, stats.blocks), (2, 1, 1, 3));
    assert_eq!(
        stats.unsupported_marker().as_deref(),
        Some("[docs] notion unsupported image=1 table_of_contents=1")
    );
    let (record, _) = record_from_page(&pages[0]);
    assert_eq!(record.text, "# Guide\n\nRead `Guide`\n");
}

#[test]
fn errors_hide_the_token() {
    let stub = StubServer::start(vec![Canned::status(
        401,
        json!({ "object": "error", "status": 401, "code": "unauthorized",
                "message": "API token is invalid.", "request_id": "r-1" })
        .to_string(),
    )])
    .expect("stub binds");
    let err = notion::pull_database(&cfg(&stub.origin()), "db1", 10)
        .err()
        .expect("a 401 fails");
    let seen = stub.finish();
    assert_eq!(seen.len(), 1);
    assert_eq!(err, "401: unauthorized - API token is invalid.");
    assert!(!err.contains(TOKEN), "{err}");

    // A server that echoes the token back still does not get it printed.
    let stub = StubServer::start(vec![Canned::status(
        400,
        json!({ "object": "error", "status": 400, "code": "validation_error",
                "message": format!("bad header Bearer {TOKEN}") })
        .to_string(),
    )])
    .expect("stub binds");
    let err = notion::pull_database(&cfg(&stub.origin()), "db1", 10)
        .err()
        .expect("a 400 fails");
    stub.finish();
    assert!(err.starts_with("400: validation_error - ") && !err.contains(TOKEN), "{err}");

    // A 429 is an error that names it (retries are CE.4f's).
    let stub = StubServer::start(vec![Canned::status(
        429,
        json!({ "object": "error", "status": 429, "code": "rate_limited",
                "message": "You have been rate limited." })
        .to_string(),
    )
    .with_header("Retry-After", "1")])
    .expect("stub binds");
    let err = notion::pull_database(&cfg(&stub.origin()), "db1", 10)
        .err()
        .expect("a 429 fails");
    stub.finish();
    assert!(err.starts_with("429: rate_limited"), "{err}");
}

#[test]
fn remote_plain_http_refused() {
    let err = Config::resolve(Some(TOKEN.into()), Some("http://notion.example".into()))
        .err()
        .unwrap_or_default();
    assert!(err.starts_with("refusing plain http:// to non-loopback host notion.example"), "{err}");
    assert!(!err.contains(TOKEN));

    // A hand-built Config is refused by the pull itself, before any request.
    let cfg = Config { token: TOKEN.into(), origin: "http://notion.example".into() };
    let err = notion::pull_database(&cfg, "db1", 10).err().unwrap_or_default();
    assert!(err.starts_with("refusing plain http://"), "{err}");

    // An origin carries no path; the database id no path segment.
    assert!(Config::resolve(Some(TOKEN.into()), Some("https://api.notion.com/v1".into())).is_err());
    let cfg = Config { token: TOKEN.into(), origin: "https://api.notion.com".into() };
    let err = notion::pull_database(&cfg, "../v1/users", 10).err().unwrap_or_default();
    assert!(err.starts_with("--database takes a Notion database id"), "{err}");
}

#[test]
fn max_pages_stops_the_listing() {
    let stub = StubServer::start(vec![
        database(&["ds2", "ds1"]),
        list(vec![page(PAGE_A, "Order Flow", "2026-09-01T00:00:00.000Z")], Some("c2")),
        order_blocks("Calls "),
    ])
    .expect("stub binds");
    let pulled = notion::pull_database(&cfg(&stub.origin()), "db1", 1);
    let seen = stub.finish();
    let (pages, stats) = pulled.expect("pull succeeds");
    assert_eq!(seen.len(), 3, "no second query after the cap");
    assert_eq!(seen[1].target, "/v1/data_sources/ds1/query", "data sources are queried in id order");
    assert_eq!(body(&seen[1]), json!({ "page_size": 1 }));
    assert_eq!(pages.len(), 1);
    assert_eq!((stats.data_sources, stats.fetched, stats.requests, stats.truncated), (2, 1, 3, true));
}

#[test]
fn repeated_titles_keep_their_own_paths() {
    let stub = StubServer::start(vec![
        database(&["ds1"]),
        list(
            vec![
                page(PAGE_B, "Notes", "2026-09-02T00:00:00.000Z"),
                page(PAGE_A, "Notes", "2026-09-01T00:00:00.000Z"),
                page("1a2b3c4d-0000-4000-8000-00000000000c", "", "2026-09-03T00:00:00.000Z"),
            ],
            None,
        ),
        list(vec![], None),
        list(vec![], None),
        list(vec![], None),
    ])
    .expect("stub binds");
    let pulled = notion::pull_database(&cfg(&stub.origin()), "DB1", 10);
    stub.finish();
    let (pages, _) = pulled.expect("pull succeeds");
    let paths: Vec<String> = pages.iter().map(|p| record_from_page(p).0.rel_path).collect();
    assert_eq!(
        paths,
        [
            "notion/db1/notes.md",
            "notion/db1/notes-1a2b3c4d00004000800000000000000b.md",
            "notion/db1/page-1a2b3c4d00004000800000000000000c.md",
        ]
    );
}
