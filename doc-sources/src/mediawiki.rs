//! A MediaWiki wiki as a doc source, through its Action API (CE.4d).
//!
//! [`pull`] reads exactly the namespace or category the user names, as a
//! generator on one `action=query` request that also returns each page's
//! current wikitext and revision id:
//!
//! ```text
//! <api>?action=query&format=json&formatversion=2&maxlag=5
//!      &prop=revisions|info&rvprop=content|ids|timestamp&rvslots=main&inprop=url
//!      &generator=allpages&gapnamespace=<N>&gaplimit=50
//!   or &generator=categorymembers&gcmtitle=Category:<C>&gcmtype=page&gcmlimit=50
//! ```
//!
//! 50 is the most pages a request may carry content for (mediawiki.org
//! API:Revisions); a pull capped below 50 pages lists only that many. Every
//! key of a response's `continue` object is sent back on the next request
//! until a response has none. The adapter never fetches a
//! page URL and never follows a link or an HTTP redirect (SECURITY.md: no
//! crawling); the only URL it requests is `--api`, the same shape as the
//! Confluence space pull.
//!
//! Etiquette: requests are sequential, carry `maxlag=5` and a
//! `glia/<release> (docs sync)` User-Agent (MediaWiki's API etiquette requires
//! one). A `maxlag` error or an HTTP 429 / 503 waits `Retry-After` seconds (5
//! without one, at most 30) and retries, at most [`MAX_RETRIES`] times per
//! request.
//!
//! Auth is a bearer token (an OAuth 2 owner-only consumer's access token),
//! resolved flag -> `MEDIAWIKI_TOKEN` env -> `./.env`; a public wiki needs
//! none. Cookie login is not supported. The origin goes through
//! [`checked_origin`]: https, or plain http to loopback only.

use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::io::Read;
use std::time::Duration;

use glia_code_domain::DocSourceKind;
use serde_json::Value;

use crate::snapshot::{Page, PageBody, slug};
use crate::transport::{checked_origin, load_dotenv, pick};
use crate::wikidir::check_container;
use crate::wikitext::{WikitextStats, wikitext_to_markdown};

/// Pages one sync pulls at most unless told otherwise (`--max-pages`).
pub const DEFAULT_MAX_PAGES: usize = 5000;
/// Pages per request: the API's cap when revision content is requested.
pub const BATCH: usize = 50;
/// Retries per request after a `maxlag` error or an HTTP 429 / 503.
pub const MAX_RETRIES: usize = 3;
/// Seconds waited when a throttle names no `Retry-After`.
const DEFAULT_WAIT_SECS: u64 = 5;
/// Longest wait honoured, whatever `Retry-After` says.
const MAX_WAIT_SECS: u64 = 30;
/// Responses in a row that list no new page and resolve no pending one before
/// the pull gives up: a server that keeps continuing without progress would
/// otherwise be followed forever.
const MAX_IDLE: usize = 10;
/// Largest response body read (a 50-page batch is a few MB at most).
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// The request parameters the adapter sets; a `continue` key of the same name
/// is not echoed, so a response cannot rewrite the query.
const FIXED: [(&str, &str); 8] = [
    ("action", "query"),
    ("format", "json"),
    ("formatversion", "2"),
    ("maxlag", "5"),
    ("prop", "revisions|info"),
    ("rvprop", "content|ids|timestamp"),
    ("rvslots", "main"),
    ("inprop", "url"),
];

/// Where and how to pull.
pub struct Config {
    /// The wiki's `api.php` URL (`https://wiki.example/w/api.php`).
    pub api: String,
    /// Bearer token sent as `Authorization: Bearer <token>`; `None` for a
    /// public wiki.
    pub token: Option<String>,
    /// The container the pages are filed under (`docspace::wiki::<container>`).
    pub container: String,
    /// `glia/<release> (docs sync)`.
    pub user_agent: String,
}

impl Config {
    /// Check `api` (https, or plain http to loopback; no userinfo, query or
    /// fragment; a path), resolve the token flag -> `MEDIAWIKI_TOKEN` env ->
    /// `./.env`, and take `container` or else the api host (lowercased, no
    /// port). `release` names the User-Agent.
    pub fn resolve(
        api: &str,
        token: Option<String>,
        container: Option<String>,
        release: &str,
    ) -> Result<Config, String> {
        let (origin, _) = split_api(api)?;
        checked_origin(&origin)?;
        let container = match container {
            Some(c) => {
                check_container(&c)?;
                c
            }
            None => {
                let host = host_of(&origin).to_ascii_lowercase();
                check_container(&host)
                    .map_err(|e| format!("the api host as container: {e}; pass --container"))?;
                host
            }
        };
        let dot = load_dotenv();
        let token = pick(token, "MEDIAWIKI_TOKEN", &dot).filter(|t| !t.trim().is_empty());
        Ok(Config {
            api: api.to_string(),
            token,
            container,
            user_agent: format!("glia/{release} (docs sync)"),
        })
    }

    /// `scheme://host[:port]` of the api, refused when it is plain http to a
    /// non-loopback host. The sync marker prints it (never the path or token).
    pub fn origin(&self) -> Result<String, String> {
        checked_origin(&split_api(&self.api)?.0)
    }
}

/// What one sync pulls: a namespace by id, or a category's pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// `generator=allpages&gapnamespace=<n>` (0 is the main namespace).
    Namespace(u32),
    /// `generator=categorymembers&gcmtitle=Category:<name>`; a leading
    /// `Category:` in the name is not doubled. Pages only: no subcategories
    /// and no files, and no recursion into subcategories.
    Category(String),
}

impl Selection {
    /// `ns:<n>` or `category:<name>`, as the sync marker prints it.
    pub fn label(&self) -> String {
        match self {
            Selection::Namespace(n) => format!("ns:{n}"),
            Selection::Category(c) => format!("category:{}", category_name(c)),
        }
    }

    /// The generator parameters, listing `limit` pages per request.
    fn params(&self, limit: usize) -> Vec<(&'static str, String)> {
        let limit = limit.to_string();
        match self {
            Selection::Namespace(n) => vec![
                ("generator", "allpages".into()),
                ("gapnamespace", n.to_string()),
                ("gaplimit", limit),
            ],
            Selection::Category(c) => vec![
                ("generator", "categorymembers".into()),
                ("gcmtitle", format!("Category:{}", category_name(c))),
                ("gcmtype", "page".into()),
                ("gcmlimit", limit),
            ],
        }
    }
}

/// `name` without a leading `Category:` (any case), trimmed.
fn category_name(name: &str) -> &str {
    let name = name.trim();
    match name.get(..9) {
        Some(head) if head.eq_ignore_ascii_case("category:") => name[9..].trim_start(),
        _ => name,
    }
}

/// What one [`pull`] saw. Every page the API listed is counted once in
/// `fetched` and in exactly one of `pages` or a `skipped_*` count, so
/// `fetched == pages + skipped()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullStats {
    /// Distinct pages the API listed (at most `max_pages`).
    pub fetched: usize,
    /// Wikitext pages returned.
    pub pages: usize,
    /// Listed as `missing` or `invalid`.
    pub skipped_missing: usize,
    /// Redirect pages (`#REDIRECT [[..]]` holds no prose). They are listed
    /// and dropped here rather than filtered by the server: under
    /// `$wgMiserMode` (Wikimedia wikis) `gapfilterredir` filters after the
    /// limit, so a run of redirects comes back as empty batches (measured on
    /// www.mediawiki.org's Help namespace).
    pub skipped_redirect: usize,
    /// Pages whose content model is not `wikitext` (CSS, JS, JSON, Lua ...).
    pub skipped_model: usize,
    /// Pages whose revision content never arrived (hidden or suppressed, or
    /// still waiting on a revision continuation when the cap stopped the pull).
    pub skipped_no_content: usize,
    /// HTTP requests sent, retries included.
    pub requests: usize,
    /// Requests resent after a `maxlag` error or an HTTP 429 / 503.
    pub retries: usize,
    /// The pull stopped at `max_pages` with more left to list (a further
    /// title in the last response, or a `continue` still outstanding).
    pub truncated: bool,
    /// What converting the returned pages kept and dropped, summed.
    pub wikitext: WikitextStats,
}

impl PullStats {
    /// Every skipped page, over all reasons.
    pub fn skipped(&self) -> usize {
        self.skipped_missing + self.skipped_redirect + self.skipped_model + self.skipped_no_content
    }

    /// CE.4c's `[docs] wikitext pages=<n> headings=<h> code_blocks=<c>
    /// inline_code=<i> links=<l> templates_dropped=<t> unbalanced=<u>` line,
    /// over every returned page (before any title filter); `None` when the
    /// pull returned none.
    pub fn wikitext_marker(&self) -> Option<String> {
        let w = &self.wikitext;
        (self.pages > 0).then(|| {
            format!(
                "[docs] wikitext pages={} headings={} code_blocks={} inline_code={} links={} templates_dropped={} unbalanced={}",
                self.pages,
                w.headings,
                w.code_blocks,
                w.inline_code,
                w.links,
                w.templates_dropped,
                w.unbalanced
            )
        })
    }
}

/// Pull every wikitext page of `sel` from `cfg.api`, at most `max_pages`
/// listed pages, as [`Page`]s of [`DocSourceKind::Wiki`] in `cfg.container`.
///
/// Pages come in response order, each batch sorted by title. Per page: the
/// title is the API's (namespace prefix included), the version its current
/// revision id, the url its `fullurl` (stored, never fetched) and the body
/// its wikitext ([`PageBody::Wikitext`]); a title with no ASCII letter or
/// digit is filed as `page-<pageid>` so it keeps a manifest stem of its own.
/// Missing, redirect and non-wikitext pages are skipped and counted.
///
/// Errors: `max_pages` 0; an api [`Config::resolve`] would refuse; an API
/// `error` other than `maxlag` (`"<code>: <info>"`); an HTTP error or
/// redirect; a throttle still in place after [`MAX_RETRIES`] retries; a
/// `continue` that does not move, or [`MAX_IDLE`] responses in a row that
/// bring no new page.
pub fn pull(
    cfg: &Config,
    sel: &Selection,
    max_pages: usize,
) -> Result<(Vec<Page>, PullStats), String> {
    if max_pages == 0 {
        return Err("max_pages must be at least 1".into());
    }
    let (origin, path) = split_api(&cfg.api)?;
    let origin = checked_origin(&origin)?;
    let endpoint = format!("{origin}{path}");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(120))
        .redirects(0)
        .build();

    let mut base: Vec<(String, String)> = FIXED
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    // A pull capped below one batch asks for no more than it keeps. The limit
    // is fixed for the whole pull: a continuation replays the same generator.
    let limit = BATCH.min(max_pages);
    base.extend(
        sel.params(limit)
            .into_iter()
            .map(|(k, v)| (k.to_string(), v)),
    );
    let mut stats = PullStats::default();
    let mut out = Vec::new();
    let mut listed: HashSet<String> = HashSet::new();
    let mut done: HashSet<String> = HashSet::new();
    let mut pending: BTreeSet<String> = BTreeSet::new();
    let mut cont: Vec<(String, String)> = Vec::new();
    let mut idle = 0usize;
    loop {
        let query: Vec<&(String, String)> = base.iter().chain(&cont).collect();
        let v = get_json(&agent, cfg, &request_url(&endpoint, &query), &mut stats)?;
        let mut batch: Vec<&Value> = v["query"]["pages"]
            .as_array()
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        batch.sort_by(|a, b| title_of(a).cmp(title_of(b)));
        let mut capped = false;
        let before = (stats.fetched, done.len());
        for p in batch {
            let title = title_of(p);
            if done.contains(title) {
                continue;
            }
            if !listed.contains(title) {
                if stats.fetched >= max_pages {
                    capped = true;
                    continue;
                }
                listed.insert(title.to_string());
                stats.fetched += 1;
            }
            match read_page(p, cfg, &endpoint) {
                Listed::Pending => {
                    pending.insert(title.to_string());
                    continue;
                }
                Listed::Skip(reason) => *reason.count(&mut stats) += 1,
                Listed::Page(page, text) => {
                    stats.pages += 1;
                    stats.wikitext.add(&wikitext_to_markdown(text).1);
                    out.push(page);
                }
            }
            pending.remove(title);
            done.insert(title.to_string());
        }
        idle = if (stats.fetched, done.len()) == before {
            idle + 1
        } else {
            0
        };
        if capped {
            stats.truncated = true;
            break;
        }
        let Some(next) = continuation(&v, &base) else {
            break;
        };
        // At the cap, keep going only while listed pages still wait for
        // their content (a revision continuation); a new title is refused.
        if stats.fetched >= max_pages && pending.is_empty() {
            stats.truncated = true;
            break;
        }
        if next == cont {
            return Err(format!(
                "the API sent the same continuation twice ({}); stopping",
                next.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&")
            ));
        }
        if idle >= MAX_IDLE {
            return Err(format!(
                "the API sent {MAX_IDLE} continuations in a row without a new page; stopping"
            ));
        }
        cont = next;
    }
    stats.skipped_no_content += pending.len();
    Ok((out, stats))
}

/// Why a listed page is not returned.
#[derive(Debug, Clone, Copy)]
enum Skip {
    Missing,
    Redirect,
    Model,
    NoContent,
}

impl Skip {
    fn count(self, stats: &mut PullStats) -> &mut usize {
        match self {
            Skip::Missing => &mut stats.skipped_missing,
            Skip::Redirect => &mut stats.skipped_redirect,
            Skip::Model => &mut stats.skipped_model,
            Skip::NoContent => &mut stats.skipped_no_content,
        }
    }
}

/// One `query.pages` entry, read.
enum Listed<'a> {
    /// Listed without `revisions`: a later batch of the same continuation
    /// carries its content.
    Pending,
    Skip(Skip),
    Page(Page, &'a str),
}

fn title_of(p: &Value) -> &str {
    p["title"].as_str().unwrap_or("")
}

fn truthy(v: &Value) -> bool {
    // formatversion=2 sends booleans; formatversion=1 sends "" for true.
    matches!(v, Value::Bool(true) | Value::String(_))
}

fn read_page<'a>(p: &'a Value, cfg: &Config, endpoint: &str) -> Listed<'a> {
    if truthy(&p["missing"]) || truthy(&p["invalid"]) {
        return Listed::Skip(Skip::Missing);
    }
    if truthy(&p["redirect"]) {
        return Listed::Skip(Skip::Redirect);
    }
    let Some(rev) = p["revisions"].as_array().and_then(|r| r.first()) else {
        return Listed::Pending;
    };
    let main = &rev["slots"]["main"];
    let model = p["contentmodel"]
        .as_str()
        .or_else(|| main["contentmodel"].as_str())
        .or_else(|| rev["contentmodel"].as_str())
        .unwrap_or("wikitext");
    if model != "wikitext" {
        return Listed::Skip(Skip::Model);
    }
    let Some(text) = main["content"].as_str().or_else(|| rev["content"].as_str()) else {
        return Listed::Skip(Skip::NoContent);
    };
    let title = title_of(p).to_string();
    let version = rev["revid"]
        .as_u64()
        .map(|r| r.to_string())
        .or_else(|| rev["timestamp"].as_str().map(str::to_string))
        .unwrap_or_default();
    let url = match p["fullurl"].as_str() {
        Some(u) if u.starts_with("https://") || u.starts_with("http://") => u.to_string(),
        _ => format!(
            "{}?title={}",
            index_php(endpoint),
            encode(&title.replace(' ', "_"))
        ),
    };
    let slug_hint = slug(&title).is_empty().then(|| match p["pageid"].as_u64() {
        Some(id) => format!("page-{id}"),
        None => format!("page-rev-{version}"),
    });
    Listed::Page(
        Page {
            kind: DocSourceKind::Wiki,
            container: cfg.container.clone(),
            title,
            url,
            version,
            body: PageBody::Wikitext(text.to_string()),
            slug_hint,
        },
        text,
    )
}

/// `…/api.php` -> `…/index.php`, the page view a `fullurl`-less response
/// falls back to.
fn index_php(endpoint: &str) -> String {
    match endpoint.strip_suffix("api.php") {
        Some(dir) => format!("{dir}index.php"),
        None => format!(
            "{}/index.php",
            endpoint.rsplit_once('/').map_or(endpoint, |(d, _)| d)
        ),
    }
}

/// Every key of the response's `continue` object, as the next request's
/// extra parameters (sorted by key), or `None` when the listing is done.
fn continuation(v: &Value, base: &[(String, String)]) -> Option<Vec<(String, String)>> {
    let obj = v["continue"].as_object()?;
    let mut next: Vec<(String, String)> = obj
        .iter()
        .filter(|(k, _)| !base.iter().any(|(b, _)| b == *k))
        .filter_map(|(k, v)| {
            let v = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some((k.clone(), v))
        })
        .collect();
    next.sort();
    Some(next)
}

// ---- api URL --------------------------------------------------------------

/// `(scheme://authority, /path)` of an api URL, scheme lowercased.
fn split_api(api: &str) -> Result<(String, String), String> {
    let lower = api.to_ascii_lowercase();
    let Some(scheme) = ["https://", "http://"]
        .into_iter()
        .find(|s| lower.starts_with(s))
    else {
        return Err(format!(
            "--api must be an https:// URL to the wiki's api.php (or plain http:// to loopback), got {api:?}"
        ));
    };
    let rest = &api[scheme.len()..];
    let (authority, path) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
    if authority.contains('@') {
        return Err("--api carries no credentials: pass a bearer token with --token".into());
    }
    if api.contains(['?', '#']) {
        return Err(format!(
            "--api is the api.php URL without a query or fragment, got {api:?}"
        ));
    }
    if authority.is_empty() || path.trim_matches('/').is_empty() {
        return Err(format!(
            "--api names the wiki's api.php endpoint (e.g. https://wiki.example/w/api.php), got {api:?}"
        ));
    }
    Ok((format!("{scheme}{authority}"), path.to_string()))
}

/// The host of `scheme://host[:port]`: `[v6]` kept whole, any port dropped.
fn host_of(origin: &str) -> &str {
    let authority = origin.split_once("://").map_or(origin, |(_, a)| a);
    match authority.strip_prefix('[') {
        Some(_) => authority.find(']').map_or(authority, |i| &authority[..=i]),
        None => authority.split(':').next().unwrap_or(authority),
    }
}

/// Percent-encode a query component: RFC 3986 unreserved bytes pass, every
/// other byte of its UTF-8 is `%XX`.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn request_url(endpoint: &str, query: &[&(String, String)]) -> String {
    let pairs: Vec<String> = query
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect();
    format!("{endpoint}?{}", pairs.join("&"))
}

// ---- HTTP -----------------------------------------------------------------

/// GET `url` as JSON, retrying a `maxlag` error or an HTTP 429 / 503 after
/// `Retry-After` seconds ([`DEFAULT_WAIT_SECS`] without one, at most
/// [`MAX_WAIT_SECS`]), at most [`MAX_RETRIES`] times.
fn get_json(
    agent: &ureq::Agent,
    cfg: &Config,
    url: &str,
    stats: &mut PullStats,
) -> Result<Value, String> {
    let mut retries = 0;
    loop {
        stats.requests += 1;
        let mut req = agent
            .get(url)
            .set("User-Agent", &cfg.user_agent)
            .set("Accept", "application/json");
        if let Some(token) = &cfg.token {
            req = req.set("Authorization", &format!("Bearer {token}"));
        }
        let (reason, wait) = match req.call() {
            Ok(resp) if (300..400).contains(&resp.status()) => {
                return Err(format!(
                    "HTTP {} redirect to {}: redirects are not followed, pass the final api.php URL as --api",
                    resp.status(),
                    resp.header("Location").unwrap_or("(no Location)")
                ));
            }
            Ok(resp) => {
                let retry_after = wait_secs(resp.header("Retry-After"));
                let v = read_json(resp)?;
                let Some(error) = v.get("error") else {
                    return Ok(v);
                };
                let code = error["code"].as_str().unwrap_or("error");
                if code != "maxlag" {
                    let info = error["info"].as_str().unwrap_or("");
                    return Err(format!(
                        "{code}: {}",
                        info.chars().take(300).collect::<String>()
                    ));
                }
                ("maxlag".to_string(), retry_after)
            }
            Err(ureq::Error::Status(code @ (429 | 503), resp)) => (
                format!("HTTP {code}"),
                wait_secs(resp.header("Retry-After")),
            ),
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                return Err(format!(
                    "HTTP {code}: {}",
                    body.chars().take(300).collect::<String>()
                ));
            }
            Err(e) => return Err(format!("request failed: {e}")),
        };
        if retries >= MAX_RETRIES {
            return Err(format!(
                "{reason}: still throttled after {MAX_RETRIES} retries"
            ));
        }
        retries += 1;
        stats.retries += 1;
        std::thread::sleep(Duration::from_secs(wait));
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

fn read_json(resp: ureq::Response) -> Result<Value, String> {
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read response: {e}"))?;
    if buf.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(format!("response larger than {MAX_RESPONSE_BYTES} bytes"));
    }
    serde_json::from_slice(&buf)
        .map_err(|e| format!("decode json (is --api the wiki's api.php?): {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_query_components() {
        assert_eq!(encode("revisions|info"), "revisions%7Cinfo");
        assert_eq!(
            encode("Category:Release Runbooks"),
            "Category%3ARelease%20Runbooks"
        );
        assert_eq!(encode("gapcontinue||"), "gapcontinue%7C%7C");
        assert_eq!(encode("Été~a-b.c_d"), "%C3%89t%C3%A9~a-b.c_d");
    }

    #[test]
    fn splits_the_api_url() {
        assert_eq!(
            split_api("HTTPS://Wiki.Example:8443/w/api.php"),
            Ok(("https://Wiki.Example:8443".into(), "/w/api.php".into()))
        );
        assert_eq!(host_of("https://wiki.example:8443"), "wiki.example");
        assert_eq!(host_of("http://[::1]:9"), "[::1]");
        assert_eq!(host_of("http://127.0.0.1"), "127.0.0.1");
        assert!(split_api("ftp://wiki.example/api.php").is_err());
        assert!(split_api("https:///api.php").is_err());
        assert!(split_api("https://wiki.example/").is_err());
    }

    #[test]
    fn category_prefix_and_waits() {
        assert_eq!(category_name("Category:Runbooks"), "Runbooks");
        assert_eq!(category_name(" category: Runbooks "), "Runbooks");
        assert_eq!(category_name("Runbooks"), "Runbooks");
        assert_eq!(category_name("Cat"), "Cat");
        assert_eq!(wait_secs(None), 5);
        assert_eq!(wait_secs(Some("0")), 0);
        assert_eq!(wait_secs(Some("120")), 30);
        assert_eq!(wait_secs(Some("Wed, 21 Oct 2026 07:28:00 GMT")), 5);
    }

    #[test]
    fn index_php_fallback() {
        assert_eq!(
            index_php("https://w.example/w/api.php"),
            "https://w.example/w/index.php"
        );
        assert_eq!(
            index_php("https://w.example/w/api"),
            "https://w.example/w/index.php"
        );
    }
}
