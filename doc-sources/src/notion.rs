//! A Notion database as a doc source, through the Notion API (CE.4e).
//!
//! [`pull_database`] reads exactly the database the user names. Since API
//! version 2025-09-03 a database holds one or more data sources, so a pull is:
//!
//! ```text
//! GET  /v1/databases/{database_id}                  -> data_sources [{id, name}]
//! POST /v1/data_sources/{data_source_id}/query      {"page_size": 100[, "start_cursor": c]}
//!                                                   -> results, has_more, next_cursor
//! GET  /v1/blocks/{page_id}/children?page_size=100[&start_cursor=c]   (per page)
//! ```
//!
//! Every request carries `Authorization: Bearer <integration token>`,
//! `Notion-Version: 2025-09-03` ([`NOTION_VERSION`]) and
//! `Content-Type: application/json`. The version is pinned by header, so a
//! workspace on a newer API version still answers in the 2025-09-03 shape.
//! Requests are sequential (one per page for its blocks, N+1 in all); HTTP
//! redirects are not followed and no page URL is ever fetched (SECURITY.md: no
//! crawling). A 429 is an error naming it: retries are CE.4f's.
//!
//! The token resolves flag -> `NOTION_TOKEN` env -> `./.env`; the origin is
//! `https://api.notion.com` unless `--api` names another (a proxy, or the
//! loopback test server), and goes through [`checked_origin`]: https, or plain
//! http to loopback only. An error names Notion's status, code and message,
//! never the token.
//!
//! Page bodies are converted to Markdown by [`crate::notion_md`]; this module
//! reads top-level blocks only (nested children, page trees and retries are
//! CE.4f's).

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::time::Duration;

use glia_code_domain::DocSourceKind;
use serde_json::{Value, json};

use crate::notion_md::blocks_to_markdown;
use crate::snapshot::{Page, PageBody, slug};
use crate::transport::{checked_origin, load_dotenv, pick};

/// The Notion API version every request pins (`Notion-Version` header): the
/// first with data sources.
pub const NOTION_VERSION: &str = "2025-09-03";
/// The Notion API origin used unless `--api` names another.
pub const DEFAULT_ORIGIN: &str = "https://api.notion.com";
/// Pages one sync lists at most unless told otherwise (`--max-pages`).
pub const DEFAULT_MAX_PAGES: usize = 5000;
/// Results per request: the API's maximum `page_size`.
pub const PAGE_SIZE: usize = 100;
/// Responses in a row that bring nothing new before a listing gives up: a
/// server that keeps answering `has_more` without progress would otherwise be
/// followed forever.
const MAX_IDLE: usize = 10;
/// Largest response body read (100 pages or blocks is well under 1 MB).
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
/// Longest id accepted into a request path (a UUID with hyphens is 36).
const MAX_ID_LEN: usize = 64;

/// Where and how to pull.
pub struct Config {
    /// The integration token, sent as `Authorization: Bearer <token>`.
    pub token: String,
    /// `scheme://host[:port]`, no path ([`DEFAULT_ORIGIN`] by default).
    pub origin: String,
}

impl Config {
    /// Take `api` as the origin (`scheme://host[:port]`, no path, query,
    /// fragment or userinfo; https, or plain http to loopback only) or else
    /// [`DEFAULT_ORIGIN`], then resolve the token flag -> `NOTION_TOKEN` env
    /// -> `./.env`. The origin is checked first, so a refused origin is
    /// reported whether or not a token is set.
    pub fn resolve(token: Option<String>, api: Option<String>) -> Result<Config, String> {
        let origin = match api {
            Some(api) => parse_origin(&api)?,
            None => DEFAULT_ORIGIN.to_string(),
        };
        let origin = checked_origin(&origin)?;
        let dot = load_dotenv();
        let token = pick(token, "NOTION_TOKEN", &dot)
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| {
                "missing NOTION_TOKEN (pass --token, or set it in env / ./.env): an internal integration's token, with the database shared to the integration".to_string()
            })?;
        Ok(Config { token, origin })
    }
}

/// `api` as `scheme://authority` with the scheme lowercased and any trailing
/// `/` dropped; anything with a path, query, fragment or userinfo is refused.
fn parse_origin(api: &str) -> Result<String, String> {
    let lower = api.to_ascii_lowercase();
    let refuse = || {
        format!(
            "--api for --source notion is the API origin, scheme://host[:port] with no path (default {DEFAULT_ORIGIN}), got {api:?}"
        )
    };
    let scheme = ["https://", "http://"]
        .into_iter()
        .find(|s| lower.starts_with(s))
        .ok_or_else(refuse)?;
    let authority = api[scheme.len()..].trim_end_matches('/');
    if authority.contains('@') {
        return Err("--api carries no credentials: pass the integration token with --token".into());
    }
    if authority.is_empty() || authority.contains(['/', '?', '#']) {
        return Err(refuse());
    }
    Ok(format!("{scheme}{authority}"))
}

/// The container a database's pages are filed under
/// (`docspace::notion::<container>`): its id with hyphens removed, lowercased,
/// so the dashed and the bare form of one id name one container. Refused when
/// the id holds anything but ASCII letters, digits and `-` (it goes into a
/// request path).
pub fn database_container(database_id: &str) -> Result<String, String> {
    let id = database_id.trim();
    check_id(id).map_err(|_| {
        format!(
            "--database takes a Notion database id (32 hex digits, hyphens optional, from the database's URL), got {database_id:?}"
        )
    })?;
    Ok(id.replace('-', "").to_ascii_lowercase())
}

/// An id as a request path segment: 1..=[`MAX_ID_LEN`] ASCII letters, digits
/// and `-`.
fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > MAX_ID_LEN
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(format!("{id:?} is not a Notion id"));
    }
    Ok(())
}

/// What one [`pull_database`] saw. Every page the queries listed is counted
/// once in `fetched` and in exactly one of `pages` or `skipped_archived`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotionStats {
    /// Data sources the database lists (each one queried).
    pub data_sources: usize,
    /// Distinct pages the queries listed (at most `max_pages`).
    pub fetched: usize,
    /// Pages returned.
    pub pages: usize,
    /// Listed pages that are archived or in the trash: skipped, no blocks read.
    pub skipped_archived: usize,
    /// Blocks read from the returned pages' children listings.
    pub blocks: usize,
    /// Block types the converter does not render, and how many of each it
    /// skipped (sorted by type, so the marker is deterministic).
    pub unsupported: BTreeMap<String, usize>,
    /// HTTP requests sent.
    pub requests: usize,
    /// The pull stopped at `max_pages` with more left to list (a further page
    /// in the last response, a cursor outstanding, or a data source not
    /// queried).
    pub truncated: bool,
}

impl NotionStats {
    /// Every unsupported block skipped, over all types.
    pub fn unsupported_total(&self) -> usize {
        self.unsupported.values().sum()
    }

    /// The fired_on line `[docs] sync source=notion database=<container>
    /// data_sources=<d> fetched=<f> kept=<k> blocks=<b> unsupported=<u>
    /// requests=<r>`, where `kept` is what survived the title filter.
    pub fn sync_marker(&self, container: &str, kept: usize) -> String {
        format!(
            "[docs] sync source=notion database={container} data_sources={} fetched={} kept={kept} blocks={} unsupported={} requests={}",
            self.data_sources,
            self.fetched,
            self.blocks,
            self.unsupported_total(),
            self.requests
        )
    }

    /// `[docs] notion unsupported <type>=<n> ...`, or `None` when every block
    /// was rendered.
    pub fn unsupported_marker(&self) -> Option<String> {
        (!self.unsupported.is_empty()).then(|| {
            let parts: Vec<String> = self
                .unsupported
                .iter()
                .map(|(kind, n)| format!("{kind}={n}"))
                .collect();
            format!("[docs] notion unsupported {}", parts.join(" "))
        })
    }
}

/// Pull every page of the database `database_id` from `cfg.origin`, at most
/// `max_pages` listed pages, as [`Page`]s of [`DocSourceKind::Notion`] in the
/// container [`database_container`] names.
///
/// The database's data sources are queried in id order; the listed pages are
/// sorted by (`created_time`, `id`), so a pull's page order does not depend
/// on the API's. Per page: the title is the plain text of the property whose
/// type is `title`, the url the page's `url` (stored, never fetched), the
/// version its `last_edited_time` and the body its top-level blocks as
/// Markdown ([`blocks_to_markdown`]). A page whose title slugs to nothing is
/// filed as `page-<id>`, and one whose title slug an earlier page already took
/// as `<slug>-<id>`, so two pages never share a manifest path. Archived and
/// trashed pages are skipped and counted.
///
/// Errors: `max_pages` 0; a database or listed id that is not an id; an origin
/// [`Config::resolve`] would refuse; a non-2xx answer (`"<status>: <code> -
/// <message>"`, from Notion's error JSON); a redirect; a database with no data
/// source; `has_more` without a new cursor, or [`MAX_IDLE`] responses in a
/// row that bring nothing new.
pub fn pull_database(
    cfg: &Config,
    database_id: &str,
    max_pages: usize,
) -> Result<(Vec<Page>, NotionStats), String> {
    if max_pages == 0 {
        return Err("max_pages must be at least 1".into());
    }
    let container = database_container(database_id)?;
    let client = Client::new(cfg)?;
    let mut stats = NotionStats::default();

    let db = client.send("GET", &format!("/v1/databases/{}", database_id.trim()), None, &mut stats)?;
    let mut sources: Vec<String> = Vec::new();
    for ds in db["data_sources"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let id = ds["id"].as_str().unwrap_or("");
        check_id(id).map_err(|e| format!("database {container} lists a data source whose id {e}"))?;
        sources.push(id.to_string());
    }
    sources.sort();
    sources.dedup();
    if sources.is_empty() {
        return Err(format!(
            "database {container} lists no data sources (is --database a database id, not a page or data source id?)"
        ));
    }
    stats.data_sources = sources.len();

    let mut listed: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    'sources: for ds in &sources {
        if stats.fetched >= max_pages {
            // A data source left unqueried may hold more pages.
            stats.truncated = true;
            break;
        }
        let path = format!("/v1/data_sources/{ds}/query");
        let mut cursor: Option<String> = None;
        let mut idle = 0usize;
        loop {
            let mut body = json!({ "page_size": PAGE_SIZE.min(max_pages) });
            if let Some(c) = &cursor {
                body["start_cursor"] = Value::String(c.clone());
            }
            let v = client.send("POST", &path, Some(&body), &mut stats)?;
            let before = stats.fetched;
            let mut capped = false;
            for page in v["results"].as_array().map(Vec::as_slice).unwrap_or_default() {
                if page["object"].as_str().is_some_and(|o| o != "page") {
                    continue;
                }
                let id = page["id"].as_str().unwrap_or("");
                check_id(id).map_err(|e| format!("data source {ds} listed a page whose id {e}"))?;
                if seen.contains(id) {
                    continue;
                }
                if stats.fetched >= max_pages {
                    capped = true;
                    continue;
                }
                seen.insert(id.to_string());
                stats.fetched += 1;
                listed.push(page.clone());
            }
            let has_more = v["has_more"].as_bool().unwrap_or(false);
            if capped || (has_more && stats.fetched >= max_pages) {
                stats.truncated = true;
                break 'sources;
            }
            if !has_more {
                break;
            }
            idle = if stats.fetched == before { idle + 1 } else { 0 };
            cursor = Some(next_cursor(&v, cursor.as_deref(), idle, &format!("data source {ds}"))?);
        }
    }

    listed.sort_by(|a, b| {
        let key = |p: &Value| {
            (
                p["created_time"].as_str().unwrap_or("").to_string(),
                p["id"].as_str().unwrap_or("").to_string(),
            )
        };
        key(a).cmp(&key(b))
    });

    let mut out = Vec::new();
    let mut stems: HashSet<String> = HashSet::new();
    for page in &listed {
        if page["archived"].as_bool() == Some(true) || page["in_trash"].as_bool() == Some(true) {
            stats.skipped_archived += 1;
            continue;
        }
        let id = page["id"].as_str().unwrap_or("");
        let blocks = client.children(id, &mut stats)?;
        let markdown = blocks_to_markdown(&blocks, &mut stats);
        let title = page_title(page);
        let bare_id = id.replace('-', "").to_ascii_lowercase();
        let stem = slug(&title);
        let slug_hint = if stem.is_empty() {
            Some(format!("page-{bare_id}"))
        } else if stems.contains(&stem) {
            Some(format!("{stem}-{bare_id}"))
        } else {
            None
        };
        stems.insert(slug_hint.as_deref().map_or(stem, slug));
        stats.pages += 1;
        out.push(Page {
            kind: DocSourceKind::Notion,
            container: container.clone(),
            title,
            url: page["url"].as_str().unwrap_or("").to_string(),
            version: page["last_edited_time"].as_str().unwrap_or("").to_string(),
            body: PageBody::Markdown(markdown),
            slug_hint,
        });
    }
    Ok((out, stats))
}

/// The plain text of the page property whose `type` is `title`.
fn page_title(page: &Value) -> String {
    let Some(props) = page["properties"].as_object() else {
        return String::new();
    };
    props
        .values()
        .find(|p| p["type"] == "title")
        .and_then(|p| p["title"].as_array())
        .map(|spans| {
            spans
                .iter()
                .filter_map(|s| s["plain_text"].as_str())
                .collect::<String>()
        })
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The `next_cursor` of a `has_more` response, refused when it is missing,
/// repeats the cursor just sent, or `idle` responses in a row brought nothing.
fn next_cursor(v: &Value, sent: Option<&str>, idle: usize, what: &str) -> Result<String, String> {
    let Some(next) = v["next_cursor"].as_str().filter(|c| !c.is_empty()) else {
        return Err(format!("{what}: has_more without a next_cursor; stopping"));
    };
    if sent == Some(next) {
        return Err(format!("{what}: the API sent the same cursor twice; stopping"));
    }
    if idle >= MAX_IDLE {
        return Err(format!(
            "{what}: {MAX_IDLE} responses in a row brought nothing new; stopping"
        ));
    }
    Ok(next.to_string())
}

/// One pull's HTTP client: the checked origin, the token and a no-redirect agent.
struct Client<'a> {
    cfg: &'a Config,
    origin: String,
    agent: ureq::Agent,
}

impl<'a> Client<'a> {
    fn new(cfg: &'a Config) -> Result<Client<'a>, String> {
        let origin = checked_origin(&parse_origin(&cfg.origin)?)?;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(30))
            .timeout_read(Duration::from_secs(120))
            .redirects(0)
            .build();
        Ok(Client { cfg, origin, agent })
    }

    /// Every top-level block of `page_id`, in document order, following
    /// `next_cursor` 100 at a time.
    fn children(&self, page_id: &str, stats: &mut NotionStats) -> Result<Vec<Value>, String> {
        let mut blocks = Vec::new();
        let mut cursor: Option<String> = None;
        let mut idle = 0usize;
        loop {
            let mut path = format!("/v1/blocks/{page_id}/children?page_size={PAGE_SIZE}");
            if let Some(c) = &cursor {
                path.push_str("&start_cursor=");
                path.push_str(&encode(c));
            }
            let v = self.send("GET", &path, None, stats)?;
            let results = v["results"].as_array().map(Vec::as_slice).unwrap_or_default();
            blocks.extend(results.iter().cloned());
            if !v["has_more"].as_bool().unwrap_or(false) {
                return Ok(blocks);
            }
            idle = if results.is_empty() { idle + 1 } else { 0 };
            cursor = Some(next_cursor(&v, cursor.as_deref(), idle, &format!("page {page_id}"))?);
        }
    }

    /// Send one request and read its JSON answer; a non-2xx answer is
    /// `"<status>: <code> - <message>"` with the token masked.
    fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        stats: &mut NotionStats,
    ) -> Result<Value, String> {
        stats.requests += 1;
        let req = self
            .agent
            .request(method, &format!("{}{path}", self.origin))
            .set("Authorization", &format!("Bearer {}", self.cfg.token))
            .set("Notion-Version", NOTION_VERSION)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json");
        let resp = match body {
            Some(b) => req.send_string(&b.to_string()),
            None => req.call(),
        };
        let result = match resp {
            Ok(r) if (300..400).contains(&r.status()) => Err(format!(
                "{}: redirect - redirects are not followed; pass the API origin itself as --api",
                r.status()
            )),
            Ok(r) => read_json(r),
            Err(ureq::Error::Status(code, r)) => {
                Err(api_error(code, &r.into_string().unwrap_or_default()))
            }
            Err(e) => Err(format!("request failed: {e}")),
        };
        result.map_err(|e| mask(&e, &self.cfg.token))
    }
}

/// `"<status>: <code> - <message>"` from Notion's error object
/// (`{"object":"error","status":401,"code":"unauthorized","message":".."}`),
/// or the start of the body when it is not one.
fn api_error(status: u16, body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let code = v["code"].as_str().unwrap_or("error");
    let message = v["message"]
        .as_str()
        .map_or_else(|| body.trim().to_string(), str::to_string);
    format!(
        "{status}: {code} - {}",
        message.chars().take(300).collect::<String>()
    )
}

/// `text` with every occurrence of `token` replaced by `<token>`.
fn mask(text: &str, token: &str) -> String {
    if token.is_empty() {
        text.to_string()
    } else {
        text.replace(token, "<token>")
    }
}

fn read_json(resp: ureq::Response) -> Result<Value, String> {
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read response: {e}"))?;
    if buf.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(format!("response larger than {MAX_RESPONSE_BYTES} bytes"));
    }
    serde_json::from_slice(&buf).map_err(|e| format!("decode json (is --api the Notion API?): {e}"))
}

/// Percent-encode a query component: RFC 3986 unreserved bytes pass, every
/// other byte is `%XX`.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_scheme_and_authority_only() {
        assert_eq!(parse_origin("HTTPS://api.notion.com/"), Ok("https://api.notion.com".into()));
        assert_eq!(parse_origin("http://127.0.0.1:9"), Ok("http://127.0.0.1:9".into()));
        assert!(parse_origin("https://api.notion.com/v1").is_err());
        assert!(parse_origin("https://api.notion.com?x=1").is_err());
        assert!(parse_origin("https://tok@api.notion.com").is_err());
        assert!(parse_origin("api.notion.com").is_err());
        assert!(parse_origin("https://").is_err());
    }

    #[test]
    fn database_ids_become_containers() {
        assert_eq!(
            database_container("1F0E2A3B-4C5D-6E7F-8091-A2B3C4D5E6F7"),
            Ok("1f0e2a3b4c5d6e7f8091a2b3c4d5e6f7".into())
        );
        assert_eq!(database_container("db1"), Ok("db1".into()));
        assert!(database_container("../pages").is_err());
        assert!(database_container("").is_err());
        assert!(database_container("a?b").is_err());
    }

    #[test]
    fn errors_name_status_code_and_message_and_mask_the_token() {
        let body = r#"{"object":"error","status":401,"code":"unauthorized","message":"API token is invalid."}"#;
        assert_eq!(api_error(401, body), "401: unauthorized - API token is invalid.");
        assert_eq!(api_error(502, "<html>bad gateway</html>"), "502: error - <html>bad gateway</html>");
        assert_eq!(
            mask("401: unauthorized - token secret_t rejected", "secret_t"),
            "401: unauthorized - token <token> rejected"
        );
    }

    #[test]
    fn titles_come_from_the_title_property() {
        let page = json!({ "properties": {
            "Tags": { "id": "a", "type": "multi_select", "multi_select": [] },
            "Name": { "id": "title", "type": "title", "title": [
                { "type": "text", "plain_text": "Order " },
                { "type": "text", "plain_text": "Flow" },
            ]},
        }});
        assert_eq!(page_title(&page), "Order Flow");
        assert_eq!(page_title(&json!({})), "");
    }

    #[test]
    fn cursors_must_move() {
        let v = json!({ "has_more": true, "next_cursor": "c2" });
        assert_eq!(next_cursor(&v, None, 0, "x"), Ok("c2".into()));
        assert!(next_cursor(&v, Some("c2"), 0, "x").is_err());
        assert!(next_cursor(&v, None, MAX_IDLE, "x").is_err());
        assert!(next_cursor(&json!({ "has_more": true, "next_cursor": null }), None, 0, "x").is_err());
        assert_eq!(encode("a b/c"), "a%20b%2Fc");
    }
}
