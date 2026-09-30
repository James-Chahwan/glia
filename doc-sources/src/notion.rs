//! A Notion database or page tree as a doc source, through the Notion API
//! (CE.4e, CE.4f).
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
//! [`pull_page_tree`] reads a root page and its sub-pages instead:
//! `GET /v1/pages/{id}` (title, url, last edit) and its blocks per page, every
//! `child_page` block queued as a page of its own, breadth-first.
//!
//! A block with `has_children` (other than `child_page` / `child_database`,
//! which are pages, not content) has its children read with another
//! `GET /v1/blocks/{block_id}/children`, recursively to [`MAX_BLOCK_DEPTH`]
//! levels, and attached as its `children` array for [`crate::notion_md`]; a
//! synced copy reads its original's children instead.
//!
//! Every request carries `Authorization: Bearer <integration token>`,
//! `Notion-Version: 2025-09-03` ([`NOTION_VERSION`]) and
//! `Content-Type: application/json`. The version is pinned by header, so a
//! workspace on a newer API version still answers in the 2025-09-03 shape.
//! Requests are sequential; HTTP redirects are not followed, no page URL is
//! ever fetched and only pages the integration was given are read
//! (SECURITY.md: no crawling). An HTTP 429 waits its `Retry-After` seconds (1
//! without one, at most 30) and a 502 / 503 / 504 one second, then retries, at
//! most [`MAX_RETRIES`] times per request.
//!
//! The token resolves flag -> `NOTION_TOKEN` env -> `./.env`; the origin is
//! `https://api.notion.com` unless `--api` names another (a proxy, or the
//! loopback test server), and goes through [`checked_origin`]: https, or plain
//! http to loopback only. An error names Notion's status, code and message,
//! never the token.
//!
//! Page bodies are converted to Markdown by [`crate::notion_md`].

use std::collections::{BTreeMap, HashSet, VecDeque};
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
/// Levels of nested block children read under a page's top-level blocks; a
/// block with children deeper than this is counted in
/// [`NotionStats::depth_capped`] and its children are not read.
pub const MAX_BLOCK_DEPTH: usize = 8;
/// Levels of sub-pages [`pull_page_tree`] descends below the root page.
pub const MAX_PAGE_DEPTH: usize = 5;
/// Retries per request after an HTTP 429 or 502 / 503 / 504.
pub const MAX_RETRIES: usize = 3;
/// Seconds a 429 waits when it names no `Retry-After`.
const DEFAULT_WAIT_SECS: u64 = 1;
/// Longest wait honoured, whatever `Retry-After` says.
const MAX_WAIT_SECS: u64 = 30;
/// Seconds a 502 / 503 / 504 waits before its retry.
const SERVER_ERROR_WAIT_SECS: u64 = 1;

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
    container_of(database_id, "--database", "database")
}

/// The container a page tree is filed under: the root page's id, as
/// [`database_container`] forms it.
pub fn page_container(page_id: &str) -> Result<String, String> {
    container_of(page_id, "--page", "page")
}

fn container_of(id: &str, flag: &str, what: &str) -> Result<String, String> {
    let trimmed = id.trim();
    check_id(trimmed).map_err(|_| {
        format!(
            "{flag} takes a Notion {what} id (32 hex digits, hyphens optional, from the {what}'s URL), got {id:?}"
        )
    })?;
    Ok(bare_id(trimmed))
}

/// `id` with hyphens removed, lowercased.
fn bare_id(id: &str) -> String {
    id.replace('-', "").to_ascii_lowercase()
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

/// What one [`pull_database`] or [`pull_page_tree`] saw. Every page listed
/// (or, in a page tree, read) is counted once in `fetched` and in exactly one
/// of `pages` or `skipped_archived`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotionStats {
    /// Data sources the database lists (each one queried); 0 for a page tree.
    pub data_sources: usize,
    /// Distinct pages the queries listed, or the page tree read (at most
    /// `max_pages`).
    pub fetched: usize,
    /// Pages returned.
    pub pages: usize,
    /// Listed pages that are archived or in the trash: skipped, no blocks read.
    pub skipped_archived: usize,
    /// Blocks read from the returned pages, nested children included.
    pub blocks: usize,
    /// Block types the converter does not render, and how many of each it
    /// skipped (sorted by type, so the marker is deterministic).
    pub unsupported: BTreeMap<String, usize>,
    /// Blocks with no content to render (`breadcrumb`, `table_of_contents`),
    /// dropped.
    pub dropped: usize,
    /// HTTP requests sent, retries included.
    pub requests: usize,
    /// Requests resent after an HTTP 429 or 502 / 503 / 504.
    pub retries: usize,
    /// Blocks whose children lie deeper than [`MAX_BLOCK_DEPTH`], and child
    /// pages deeper than [`MAX_PAGE_DEPTH`]: not read.
    pub depth_capped: usize,
    /// Content the integration cannot read (a 403 / 404): a synced copy's
    /// original, or a sub-page of a page tree. Skipped, never fatal.
    pub unreadable: usize,
    /// The pull stopped at `max_pages` with more left to list (a further page
    /// in the last response, a cursor outstanding, a data source not queried,
    /// or sub-pages still queued).
    pub truncated: bool,
}

impl NotionStats {
    /// Every unsupported block skipped, over all types.
    pub fn unsupported_total(&self) -> usize {
        self.unsupported.values().sum()
    }

    /// The fired_on line of a database pull, `[docs] sync source=notion
    /// database=<container> data_sources=<d> fetched=<f> kept=<k> blocks=<b>
    /// unsupported=<u> requests=<r> retries=<t> depth_capped=<c>`, where
    /// `kept` is what survived the title filter.
    pub fn sync_marker(&self, container: &str, kept: usize) -> String {
        self.marker("database", container, kept)
    }

    /// The fired_on line of a page-tree pull: [`NotionStats::sync_marker`]'s
    /// with `root=<root container>` in place of `database=`.
    pub fn tree_marker(&self, root: &str, kept: usize) -> String {
        self.marker("root", root, kept)
    }

    fn marker(&self, key: &str, value: &str, kept: usize) -> String {
        format!(
            "[docs] sync source=notion {key}={value} data_sources={} fetched={} kept={kept} blocks={} unsupported={} requests={} retries={} depth_capped={}",
            self.data_sources,
            self.fetched,
            self.blocks,
            self.unsupported_total(),
            self.requests,
            self.retries,
            self.depth_capped
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
/// version its `last_edited_time` and the body its blocks, nested children
/// included, as Markdown ([`blocks_to_markdown`]). A page whose title slugs to
/// nothing is filed as `page-<id>`, and one whose title slug an earlier page
/// already took as `<slug>-<id>`, so two pages never share a manifest path.
/// Archived and trashed pages are skipped and counted.
///
/// Errors: `max_pages` 0; a database or listed id that is not an id; an origin
/// [`Config::resolve`] would refuse; a non-2xx answer (`"<status>: <code> -
/// <message>"`, from Notion's error JSON); a redirect; a database with no data
/// source; `has_more` without a new cursor, or [`MAX_IDLE`] responses in a
/// row that bring nothing new; a 429 or 502 / 503 / 504 still answered after
/// [`MAX_RETRIES`] retries.
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
        if is_archived(page) {
            stats.skipped_archived += 1;
            continue;
        }
        let id = page["id"].as_str().unwrap_or("");
        let blocks = client.children_tree(id, 0, &mut stats)?;
        out.push(to_page(page, id, &blocks, &container, &mut stems, &mut stats));
    }
    Ok((out, stats))
}

/// Pull the page `root_page_id` and its sub-pages from `cfg.origin`, at most
/// `max_pages` pages read, as [`Page`]s of [`DocSourceKind::Notion`] in the
/// container [`page_container`] names (the root's id).
///
/// Breadth-first from the root: per page, `GET /v1/pages/{id}` for its title
/// (the `title` property), `url` (stored, never fetched) and
/// `last_edited_time`, then its blocks as in [`pull_database`]; every
/// `child_page` block among them (in document order, a toggle's or column's
/// included) is queued as a page of its own, at most [`MAX_PAGE_DEPTH`]
/// levels below the root (deeper ones are counted in `depth_capped`). Pages
/// come back in that order, each read once. An archived or trashed page is
/// skipped with its sub-pages; a sub-page the integration cannot read (403 /
/// 404) is counted in `unreadable` and skipped. Slugs as in
/// [`pull_database`].
///
/// Errors: `max_pages` 0; a root or child page id that is not an id; an
/// origin [`Config::resolve`] would refuse; a non-2xx answer for the root or
/// for blocks (`"<status>: <code> - <message>"`); a redirect; a 429 or 502 /
/// 503 / 504 still answered after [`MAX_RETRIES`] retries.
pub fn pull_page_tree(
    cfg: &Config,
    root_page_id: &str,
    max_pages: usize,
) -> Result<(Vec<Page>, NotionStats), String> {
    if max_pages == 0 {
        return Err("max_pages must be at least 1".into());
    }
    let container = page_container(root_page_id)?;
    let client = Client::new(cfg)?;
    let mut stats = NotionStats::default();
    let root = root_page_id.trim().to_string();
    let mut seen: HashSet<String> = HashSet::from([bare_id(&root)]);
    let mut queue: VecDeque<(String, usize)> = VecDeque::from([(root, 0)]);
    let mut out = Vec::new();
    let mut stems: HashSet<String> = HashSet::new();
    while let Some((id, depth)) = queue.pop_front() {
        if stats.fetched >= max_pages {
            stats.truncated = true;
            break;
        }
        let page = match client.send("GET", &format!("/v1/pages/{id}"), None, &mut stats) {
            Ok(page) => page,
            Err(e) if depth > 0 && e.unreadable() => {
                stats.unreadable += 1;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        stats.fetched += 1;
        if is_archived(&page) {
            stats.skipped_archived += 1;
            continue;
        }
        let blocks = client.children_tree(&id, 0, &mut stats)?;
        let mut children = Vec::new();
        child_pages(&blocks, &mut children);
        for child in children {
            check_id(child).map_err(|e| format!("page {id} holds a child page whose id {e}"))?;
            if depth >= MAX_PAGE_DEPTH {
                stats.depth_capped += 1;
            } else if seen.insert(bare_id(child)) {
                queue.push_back((child.to_string(), depth + 1));
            }
        }
        out.push(to_page(&page, &id, &blocks, &container, &mut stems, &mut stats));
    }
    Ok((out, stats))
}

fn is_archived(page: &Value) -> bool {
    page["archived"].as_bool() == Some(true) || page["in_trash"].as_bool() == Some(true)
}

/// The ids of every `child_page` block in `blocks` and their fetched
/// children, in document order.
fn child_pages<'a>(blocks: &'a [Value], out: &mut Vec<&'a str>) {
    for block in blocks {
        if block["type"] == "child_page"
            && let Some(id) = block["id"].as_str()
        {
            out.push(id);
        }
        if let Some(kids) = block["children"].as_array() {
            child_pages(kids, out);
        }
    }
}

/// One page object and its block tree as a [`Page`] of `container`: the
/// title property, `url`, `last_edited_time`, the blocks as Markdown, and a
/// slug hint when the title slugs to nothing (`page-<id>`) or to a stem an
/// earlier page took (`<slug>-<id>`).
fn to_page(
    page: &Value,
    id: &str,
    blocks: &[Value],
    container: &str,
    stems: &mut HashSet<String>,
    stats: &mut NotionStats,
) -> Page {
    let markdown = blocks_to_markdown(blocks, stats);
    let title = page_title(page);
    let bare = bare_id(id);
    let stem = slug(&title);
    let slug_hint = if stem.is_empty() {
        Some(format!("page-{bare}"))
    } else if stems.contains(&stem) {
        Some(format!("{stem}-{bare}"))
    } else {
        None
    };
    stems.insert(slug_hint.as_deref().map_or(stem, slug));
    stats.pages += 1;
    Page {
        kind: DocSourceKind::Notion,
        container: container.to_string(),
        title,
        url: page["url"].as_str().unwrap_or("").to_string(),
        version: page["last_edited_time"].as_str().unwrap_or("").to_string(),
        body: PageBody::Markdown(markdown),
        slug_hint,
    }
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

    /// The blocks under `block_id` (a page at `depth` 0), in document order,
    /// each block with `has_children` carrying its own read recursively as a
    /// `children` array. `child_page` / `child_database` blocks are pages, not
    /// content: their children are not read. A synced copy (`synced_from`
    /// names another block) carries its original's children; an original the
    /// integration cannot read is counted in `unreadable` and skipped. A block
    /// whose children would sit deeper than [`MAX_BLOCK_DEPTH`] is counted in
    /// `depth_capped` and left without them.
    fn children_tree(
        &self,
        block_id: &str,
        depth: usize,
        stats: &mut NotionStats,
    ) -> Result<Vec<Value>, SendError> {
        let mut blocks = self.children(block_id, stats)?;
        for block in &mut blocks {
            let kind = block["type"].as_str().unwrap_or("");
            if matches!(kind, "child_page" | "child_database") {
                continue;
            }
            let original = (kind == "synced_block")
                .then(|| block["synced_block"]["synced_from"]["block_id"].as_str())
                .flatten()
                .map(str::to_string);
            if original.is_none() && block["has_children"].as_bool() != Some(true) {
                continue;
            }
            let Some(read_from) = original.clone().or_else(|| block["id"].as_str().map(str::to_string))
            else {
                continue;
            };
            check_id(&read_from)
                .map_err(|e| format!("block {block_id} holds a block whose id {e}"))?;
            if depth >= MAX_BLOCK_DEPTH {
                stats.depth_capped += 1;
                continue;
            }
            let kids = match self.children_tree(&read_from, depth + 1, stats) {
                Ok(kids) => kids,
                Err(e) if original.is_some() && e.unreadable() => {
                    stats.unreadable += 1;
                    continue;
                }
                Err(e) => return Err(e),
            };
            if let Some(obj) = block.as_object_mut() {
                obj.insert("children".to_string(), Value::Array(kids));
            }
        }
        Ok(blocks)
    }

    /// Every block directly under `block_id`, in document order, following
    /// `next_cursor` 100 at a time.
    fn children(&self, block_id: &str, stats: &mut NotionStats) -> Result<Vec<Value>, SendError> {
        let mut blocks = Vec::new();
        let mut cursor: Option<String> = None;
        let mut idle = 0usize;
        loop {
            let mut path = format!("/v1/blocks/{block_id}/children?page_size={PAGE_SIZE}");
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
            cursor = Some(next_cursor(&v, cursor.as_deref(), idle, &format!("block {block_id}"))?);
        }
    }

    /// Send one request and read its JSON answer; a non-2xx answer is
    /// `"<status>: <code> - <message>"` with the token masked. An HTTP 429
    /// waits its `Retry-After` seconds ([`DEFAULT_WAIT_SECS`] without one, at
    /// most [`MAX_WAIT_SECS`]) and a 502 / 503 / 504
    /// [`SERVER_ERROR_WAIT_SECS`], then the request is sent again, at most
    /// [`MAX_RETRIES`] times; each send counts in `requests`, each resend in
    /// `retries`.
    fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        stats: &mut NotionStats,
    ) -> Result<Value, SendError> {
        let url = format!("{}{path}", self.origin);
        let fail = |status: Option<u16>, message: &str| SendError {
            status,
            message: mask(message, &self.cfg.token),
        };
        let mut retries = 0;
        loop {
            stats.requests += 1;
            let req = self
                .agent
                .request(method, &url)
                .set("Authorization", &format!("Bearer {}", self.cfg.token))
                .set("Notion-Version", NOTION_VERSION)
                .set("Content-Type", "application/json")
                .set("Accept", "application/json");
            let resp = match body {
                Some(b) => req.send_string(&b.to_string()),
                None => req.call(),
            };
            let (code, message, wait) = match resp {
                Ok(r) if (300..400).contains(&r.status()) => {
                    let message = format!(
                        "{}: redirect - redirects are not followed; pass the API origin itself as --api",
                        r.status()
                    );
                    return Err(fail(Some(r.status()), &message));
                }
                Ok(r) => return read_json(r).map_err(|e| fail(None, &e)),
                Err(ureq::Error::Status(code, r)) => {
                    let wait = match code {
                        429 => Some(wait_secs(r.header("Retry-After"))),
                        502..=504 => Some(SERVER_ERROR_WAIT_SECS),
                        _ => None,
                    };
                    let message = api_error(code, &r.into_string().unwrap_or_default());
                    match wait {
                        Some(wait) => (code, message, wait),
                        None => return Err(fail(Some(code), &message)),
                    }
                }
                Err(e) => return Err(fail(None, &format!("request failed: {e}"))),
            };
            if retries >= MAX_RETRIES {
                let message = format!("{message} (still failing after {MAX_RETRIES} retries)");
                return Err(fail(Some(code), &message));
            }
            retries += 1;
            stats.retries += 1;
            std::thread::sleep(Duration::from_secs(wait));
        }
    }
}

/// A failed request: the HTTP status when there was one, and the message
/// (token masked) a pull reports.
struct SendError {
    status: Option<u16>,
    message: String,
}

impl SendError {
    /// The integration was refused the resource (403) or cannot see it (404,
    /// Notion's answer for anything not shared with it).
    fn unreadable(&self) -> bool {
        matches!(self.status, Some(403 | 404))
    }
}

impl From<SendError> for String {
    fn from(e: SendError) -> String {
        e.message
    }
}

impl From<String> for SendError {
    fn from(message: String) -> SendError {
        SendError { status: None, message }
    }
}

/// `Retry-After` as whole seconds (an HTTP-date is not read), else
/// [`DEFAULT_WAIT_SECS`]; never more than [`MAX_WAIT_SECS`].
fn wait_secs(header: Option<&str>) -> u64 {
    header
        .and_then(|h| h.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_WAIT_SECS)
        .min(MAX_WAIT_SECS)
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
    fn page_ids_become_containers() {
        assert_eq!(
            page_container("1A2B3C4D-0000-4000-8000-00000000000A"),
            Ok("1a2b3c4d00004000800000000000000a".into())
        );
        let err = page_container("../v1/users").err().unwrap_or_default();
        assert!(err.starts_with("--page takes a Notion page id"), "{err}");
    }

    #[test]
    fn retry_after_is_whole_seconds_capped() {
        assert_eq!(wait_secs(Some("0")), 0);
        assert_eq!(wait_secs(Some(" 7 ")), 7);
        assert_eq!(wait_secs(Some("600")), MAX_WAIT_SECS);
        assert_eq!(wait_secs(Some("Wed, 21 Oct 2026 07:28:00 GMT")), DEFAULT_WAIT_SECS);
        assert_eq!(wait_secs(None), DEFAULT_WAIT_SECS);
    }

    #[test]
    fn child_pages_are_found_in_document_order_at_any_depth() {
        let blocks = vec![
            json!({ "type": "child_page", "id": "p1", "child_page": { "title": "One" } }),
            json!({ "type": "toggle", "id": "t", "toggle": {}, "has_children": true, "children": [
                json!({ "type": "paragraph", "id": "x", "paragraph": {} }),
                json!({ "type": "child_page", "id": "p2", "child_page": { "title": "Two" } }),
            ]}),
            json!({ "type": "child_database", "id": "d1", "child_database": { "title": "Db" } }),
            json!({ "type": "child_page", "id": "p3", "child_page": { "title": "Three" } }),
        ];
        let mut out = Vec::new();
        child_pages(&blocks, &mut out);
        assert_eq!(out, ["p1", "p2", "p3"]);
    }

    #[test]
    fn markers_name_the_database_or_the_root() {
        let stats = NotionStats {
            fetched: 3,
            pages: 3,
            blocks: 9,
            requests: 7,
            retries: 1,
            ..NotionStats::default()
        };
        assert_eq!(
            stats.tree_marker("r1", 3),
            "[docs] sync source=notion root=r1 data_sources=0 fetched=3 kept=3 blocks=9 unsupported=0 requests=7 retries=1 depth_capped=0"
        );
        assert!(stats.sync_marker("db1", 2).starts_with("[docs] sync source=notion database=db1 data_sources=0 "));
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
