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
//!
//! CE.4f adds nested children (`GET /v1/blocks/{block}/children` for every
//! block with `has_children`, to depth 8), page trees (`GET /v1/pages/{id}`,
//! `child_page` blocks as pages) and the 429 / 5xx retry (`Retry-After`).
//! Pre-fix (HEAD d85f32f) the converter counted toggle / table as unsupported
//! and never read their children: `toggles_keep_their_code` saw no children
//! request, `nested_lists_indent` got `- a` alone, `tables_render` an empty
//! body, and `rate_limit_is_retried` failed with `429: rate_limited`.

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
            dropped: 0,
            requests: 5,
            retries: 0,
            depth_capped: 0,
            unreadable: 0,
            truncated: false,
        }
    );
    assert_eq!(
        stats.sync_marker("db1", 2),
        "[docs] sync source=notion database=db1 data_sources=1 fetched=2 kept=2 blocks=6 unsupported=0 requests=5 retries=0 depth_capped=0"
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
                block("unsupported", json!({})),
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
    // CE.4f: the image renders as a link; table_of_contents carries no
    // content and is dropped; `unsupported` (Notion's own type for a block
    // the API cannot serve) is still counted.
    assert_eq!(
        stats.unsupported.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(),
        [("unsupported", 1)]
    );
    assert_eq!((stats.unsupported_total(), stats.dropped), (1, 1));
    assert_eq!((stats.fetched, stats.pages, stats.skipped_archived, stats.blocks), (2, 1, 1, 4));
    assert_eq!(
        stats.unsupported_marker().as_deref(),
        Some("[docs] notion unsupported unsupported=1")
    );
    let (record, _) = record_from_page(&pages[0]);
    assert_eq!(record.text, "# Guide\n\nRead `Guide`\n\n[a.png](https://img.example/a.png)\n");
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

// ---- CE.4f: nested blocks, page trees, retries ----

const ROOT: &str = "2b000000-0000-4000-8000-000000000001";
const SUB_A: &str = "2b000000-0000-4000-8000-000000000002";
const SUB_B: &str = "2b000000-0000-4000-8000-000000000003";

/// A block with `has_children`, its children read by another request.
fn parent(id: &str, kind: &str, body: Value) -> Value {
    json!({ "object": "block", "id": id, "type": kind,
            "has_children": true, "archived": false, "in_trash": false, kind: body })
}

fn text_body(content: &str) -> Value {
    json!({ "rich_text": [span(content, false)], "color": "default" })
}

/// The database, its one query answer listing `PAGE_A` titled `title`.
fn one_page_db(title: &str) -> Vec<Canned> {
    vec![database(&["ds1"]), list(vec![page(PAGE_A, title, "2026-09-01T00:00:00.000Z")], None)]
}

fn rate_limited(retry_after: &str) -> Canned {
    Canned::status(
        429,
        json!({ "object": "error", "status": 429, "code": "rate_limited",
                "message": "You have been rate limited." })
        .to_string(),
    )
    .with_header("Retry-After", retry_after)
}

/// `GET /v1/pages/{id}`: a page under a page, its title in the `title`
/// property.
fn page_object(id: &str, title: &str) -> Canned {
    Canned::ok(
        json!({
            "object": "page",
            "id": id,
            "created_time": "2026-09-01T00:00:00.000Z",
            "last_edited_time": "2026-09-21T08:00:00.000Z",
            "archived": false,
            "in_trash": false,
            "parent": { "type": "page_id", "page_id": ROOT },
            "url": format!("https://www.notion.so/{}", id.replace('-', "")),
            "properties": { "title": { "id": "title", "type": "title", "title": [span(title, false)] } },
        })
        .to_string(),
    )
}

fn child_page(id: &str, title: &str) -> Value {
    // A sub-page with content has has_children, but its blocks are a page of
    // their own, never read as this page's content.
    parent(id, "child_page", json!({ "title": title }))
}

fn pull_one(canned: Vec<Canned>) -> (glia_doc_sources::Page, NotionStats, Vec<Recorded>) {
    let stub = StubServer::start(canned).expect("stub binds");
    let pulled = notion::pull_database(&cfg(&stub.origin()), "db1", 10);
    let seen = stub.finish();
    let (mut pages, stats) = pulled.expect("pull succeeds");
    assert_eq!(pages.len(), 1);
    (pages.remove(0), stats, seen)
}

#[test]
fn toggles_keep_their_code() {
    let toggle = "3c000000-0000-4000-8000-0000000000aa";
    let mut canned = one_page_db("Guide");
    canned.push(list(vec![parent(toggle, "toggle", text_body("Placing an order"))], None));
    canned.push(list(
        vec![block("code", json!({ "rich_text": [span("fn place() {}\n", false)], "language": "rust", "caption": [] }))],
        None,
    ));
    let (page, stats, seen) = pull_one(canned);

    assert_eq!(seen.len(), 4);
    seen.iter().for_each(assert_headers);
    assert_eq!(seen[3].target, format!("/v1/blocks/{toggle}/children?page_size=100"));
    let (record, _) = record_from_page(&page);
    assert_eq!(
        record.text,
        "# Guide\n\n- Placing an order\n  ```rust\n  fn place() {}\n  ```\n",
        "the toggle's summary is a list line and its code is indented under it"
    );
    assert_eq!((stats.blocks, stats.unsupported_total(), stats.requests), (2, 0, 4));
}

#[test]
fn nested_lists_indent() {
    let mut canned = one_page_db("Guide");
    canned.push(list(vec![parent("li-a", "bulleted_list_item", text_body("a"))], None));
    canned.push(list(vec![block("numbered_list_item", text_body("b"))], None));
    let (page, stats, seen) = pull_one(canned);
    assert_eq!(seen[3].target, "/v1/blocks/li-a/children?page_size=100");
    let (record, _) = record_from_page(&page);
    assert_eq!(record.text, "# Guide\n\n- a\n  1. b\n");
    assert_eq!(stats.blocks, 2);
}

#[test]
fn tables_render() {
    let row = |a: &str, b: &str| block("table_row", json!({ "cells": [[span(a, false)], [span(b, false)]] }));
    let mut canned = one_page_db("Guide");
    canned.push(list(
        vec![parent("tbl-1", "table", json!({ "table_width": 2, "has_column_header": true, "has_row_header": false }))],
        None,
    ));
    canned.push(list(vec![row("h1", "h2"), row("a", "b")], None));
    let (page, stats, seen) = pull_one(canned);
    assert_eq!(seen[3].target, "/v1/blocks/tbl-1/children?page_size=100");
    let (record, _) = record_from_page(&page);
    assert_eq!(record.text, "# Guide\n\n| h1 | h2 |\n| --- | --- |\n| a | b |\n");
    assert_eq!((stats.blocks, stats.unsupported_total()), (3, 0));
}

#[test]
fn page_tree_mode() {
    let toggle = "3c000000-0000-4000-8000-0000000000bb";
    let stub = StubServer::start(vec![
        page_object(ROOT, "Engineering"),
        list(
            vec![
                block("paragraph", json!({ "rich_text": [span("Owned by ", false), span("OrderService", true)] })),
                child_page(SUB_A, "Runbooks"),
            ],
            None,
        ),
        // The first sub-page read is throttled once.
        rate_limited("0"),
        page_object(SUB_A, "Runbooks"),
        // SUB_B sits one level deeper, inside a toggle of SUB_A.
        list(vec![parent(toggle, "toggle", text_body("More"))], None),
        list(vec![child_page(SUB_B, "Deploy")], None),
        page_object(SUB_B, "Deploy"),
        list(
            vec![block("code", json!({ "rich_text": [span("glia build .\n", false)], "language": "shell", "caption": [] }))],
            None,
        ),
    ])
    .expect("stub binds");
    let pulled = notion::pull_page_tree(&cfg(&stub.origin()), ROOT, notion::DEFAULT_MAX_PAGES);
    let seen = stub.finish();
    let (pages, stats) = pulled.expect("pull succeeds");

    let targets: Vec<&str> = seen.iter().map(|r| r.target.as_str()).collect();
    assert_eq!(
        targets,
        [
            format!("/v1/pages/{ROOT}"),
            format!("/v1/blocks/{ROOT}/children?page_size=100"),
            format!("/v1/pages/{SUB_A}"),
            format!("/v1/pages/{SUB_A}"),
            format!("/v1/blocks/{SUB_A}/children?page_size=100"),
            format!("/v1/blocks/{toggle}/children?page_size=100"),
            format!("/v1/pages/{SUB_B}"),
            format!("/v1/blocks/{SUB_B}/children?page_size=100"),
        ]
    );
    seen.iter().for_each(assert_headers);
    assert!(seen.iter().all(|r| r.method == "GET"));

    let root = "2b000000000040008000000000000001";
    assert_eq!(pages.len(), 3);
    assert!(pages.iter().all(|p| p.container == root && p.kind == DocSourceKind::Notion));
    let titles: Vec<&str> = pages.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, ["Engineering", "Runbooks", "Deploy"], "titles come from GET /v1/pages");
    assert_eq!(pages[1].url, format!("https://www.notion.so/{}", SUB_A.replace('-', "")));
    assert_eq!(pages[1].version, "2026-09-21T08:00:00.000Z");
    let paths: Vec<String> = pages.iter().map(|p| record_from_page(p).0.rel_path).collect();
    assert_eq!(
        paths,
        [
            format!("notion/{root}/engineering.md"),
            format!("notion/{root}/runbooks.md"),
            format!("notion/{root}/deploy.md"),
        ]
    );
    let (record, _) = record_from_page(&pages[0]);
    assert_eq!(
        record.text,
        "# Engineering\n\nOwned by `OrderService`\n\n\
         - [Runbooks](https://www.notion.so/2b000000000040008000000000000002)\n"
    );
    assert!(record_from_page(&pages[2]).0.text.contains("```shell\nglia build .\n```"));

    assert_eq!(
        (stats.data_sources, stats.fetched, stats.pages, stats.retries, stats.truncated),
        (0, 3, 3, 1, false)
    );
    assert_eq!(
        stats.tree_marker(root, 3),
        "[docs] sync source=notion root=2b000000000040008000000000000001 data_sources=0 fetched=3 kept=3 blocks=5 unsupported=0 requests=8 retries=1 depth_capped=0"
    );
}

#[test]
fn page_tree_caps_depth_and_pages() {
    // A chain root -> p1 -> ... -> p6: p6 lies 6 levels down, past the cap of
    // 5, so it is counted and never read.
    let ids: Vec<String> = (0..=6).map(|i| format!("4d000000-0000-4000-8000-00000000000{i}")).collect();
    let mut canned = Vec::new();
    for (i, id) in ids.iter().take(6).enumerate() {
        canned.push(page_object(id, &format!("Level {i}")));
        canned.push(list(vec![child_page(&ids[i + 1], &format!("Level {}", i + 1))], None));
    }
    let stub = StubServer::start(canned).expect("stub binds");
    let pulled = notion::pull_page_tree(&cfg(&stub.origin()), &ids[0], 100);
    let seen = stub.finish();
    let (pages, stats) = pulled.expect("pull succeeds");
    assert_eq!(pages.len(), 6);
    assert_eq!(stats.depth_capped, 1);
    assert!(!seen.iter().any(|r| r.target.contains(&ids[6])), "the page past the cap is not read");
    assert_eq!(notion::MAX_PAGE_DEPTH, 5);

    // --max-pages 2 over a root with two sub-pages: the third stays queued.
    let stub = StubServer::start(vec![
        page_object(ROOT, "Engineering"),
        list(vec![child_page(SUB_A, "Runbooks"), child_page(SUB_B, "Deploy")], None),
        page_object(SUB_A, "Runbooks"),
        list(vec![], None),
    ])
    .expect("stub binds");
    let pulled = notion::pull_page_tree(&cfg(&stub.origin()), ROOT, 2);
    let seen = stub.finish();
    let (pages, stats) = pulled.expect("pull succeeds");
    assert_eq!(seen.len(), 4);
    assert_eq!(pages.len(), 2);
    assert_eq!((stats.fetched, stats.truncated), (2, true));

    let err = notion::pull_page_tree(&cfg("http://127.0.0.1:9"), "../v1/users", 10)
        .err()
        .unwrap_or_default();
    assert!(err.starts_with("--page takes a Notion page id"), "{err}");
}

#[test]
fn rate_limit_is_retried() {
    let mut canned = one_page_db("Order Flow");
    canned.push(rate_limited("0"));
    canned.push(order_blocks("Calls "));
    let (page, stats, seen) = pull_one(canned);
    assert_eq!(seen.len(), 4);
    assert_eq!(seen[2].target, format!("/v1/blocks/{PAGE_A}/children?page_size=100"));
    assert_eq!(seen[2].target, seen[3].target, "the throttled request is sent again");
    assert_eq!((stats.requests, stats.retries), (4, 1));
    assert!(record_from_page(&page).0.text.contains("Calls `OrderService.place`"));
    assert!(stats.sync_marker("db1", 1).ends_with(" requests=4 retries=1 depth_capped=0"));

    // Four 429s in a row: three retries, then an error naming the 429.
    let stub = StubServer::start(vec![
        rate_limited("0"),
        rate_limited("0"),
        rate_limited("0"),
        rate_limited("0"),
    ])
    .expect("stub binds");
    let err = notion::pull_database(&cfg(&stub.origin()), "db1", 10)
        .err()
        .expect("a lasting 429 fails");
    let seen = stub.finish();
    assert_eq!(seen.len(), 4, "one request and three retries");
    assert!(err.starts_with("429: rate_limited - "), "{err}");
    assert!(err.ends_with("(still failing after 3 retries)"), "{err}");
    assert!(!err.contains(TOKEN));
}

#[test]
fn server_errors_are_retried() {
    let mut canned = vec![Canned::status(503, "<html>unavailable</html>")];
    canned.extend(one_page_db("Order Flow"));
    canned.push(order_blocks("Calls "));
    let (_, stats, seen) = pull_one(canned);
    assert_eq!(seen[0].target, seen[1].target);
    assert_eq!((stats.requests, stats.retries), (4, 1));

    // Anything else is not retried (a resend would find the stub closed and
    // fail as a transport error instead).
    let stub = StubServer::start(vec![Canned::status(
        500,
        json!({ "object": "error", "status": 500, "code": "internal_server_error", "message": "boom" })
            .to_string(),
    )])
    .expect("stub binds");
    let err = notion::pull_database(&cfg(&stub.origin()), "db1", 10).err().unwrap_or_default();
    let seen = stub.finish();
    assert_eq!(err, "500: internal_server_error - boom");
    assert_eq!(seen.len(), 1);
}

#[test]
fn depth_is_capped() {
    // Ten toggles, each inside the one before.
    let id = |i: usize| format!("tog-{i:02}");
    let mut canned = one_page_db("Deep");
    for i in 1..=9 {
        canned.push(list(vec![parent(&id(i), "toggle", text_body(&format!("t{i}")))], None));
    }
    let stub = StubServer::start(canned).expect("stub binds");
    let pulled = notion::pull_database(&cfg(&stub.origin()), "db1", 10);
    let seen = stub.finish();
    let (pages, stats) = pulled.expect("pull succeeds");

    let chain: Vec<&str> = seen
        .iter()
        .map(|r| r.target.as_str())
        .filter(|t| t.starts_with("/v1/blocks/tog-"))
        .collect();
    assert_eq!(chain.len(), notion::MAX_BLOCK_DEPTH, "{chain:?}");
    assert_eq!(chain.len(), 8);
    assert_eq!(chain.last().copied(), Some("/v1/blocks/tog-08/children?page_size=100"));
    assert_eq!(stats.depth_capped, 1, "tog-09's children are past the cap");
    assert_eq!(stats.blocks, 9);
    let text = record_from_page(&pages[0]).0.text;
    assert!(text.contains("- t1\n  - t2\n    - t3\n"), "{text}");
    assert!(text.contains(&format!("\n{}- t9\n", " ".repeat(16))), "{text}");
}

#[test]
fn synced_copies_read_their_original() {
    let copy = json!({ "object": "block", "id": "sync-copy", "type": "synced_block", "has_children": true,
        "synced_block": { "synced_from": { "type": "block_id", "block_id": "sync-orig" } } });
    let hidden = json!({ "object": "block", "id": "sync-copy2", "type": "synced_block", "has_children": true,
        "synced_block": { "synced_from": { "type": "block_id", "block_id": "sync-hidden" } } });
    let mut canned = one_page_db("Shared");
    canned.push(list(vec![copy, hidden, block("paragraph", text_body("after"))], None));
    canned.push(list(
        vec![block("paragraph", json!({ "rich_text": [span("Uses ", false), span("Ledger", true)] }))],
        None,
    ));
    canned.push(Canned::status(
        404,
        json!({ "object": "error", "status": 404, "code": "object_not_found",
                "message": "Could not find block with ID: sync-hidden." })
        .to_string(),
    ));
    let (page, stats, seen) = pull_one(canned);

    assert_eq!(seen[3].target, "/v1/blocks/sync-orig/children?page_size=100");
    assert_eq!(seen[4].target, "/v1/blocks/sync-hidden/children?page_size=100");
    assert!(!seen.iter().any(|r| r.target.contains("sync-copy")), "a copy reads its original");
    assert_eq!(stats.unreadable, 1, "an original the integration cannot read is skipped");
    assert_eq!(record_from_page(&page).0.text, "# Shared\n\nUses `Ledger`\n\nafter\n");
}
