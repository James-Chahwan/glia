//! Confluence Cloud REST — blocking client (ureq). This is the **network** half
//! of Tier-4 doc ingestion, deliberately kept in `doc-sources` (never in the
//! engine or the published wheel) so the engine stays deterministic and the
//! byte-identical build gate holds: `sync` fetches into a local snapshot; the
//! build only ever reads that snapshot.
//!
//! Auth is HTTP Basic `email:token` (a **classic** Atlassian API token — scoped
//! tokens without Confluence scopes 403). Credentials resolve, in order:
//! explicit args → process env → a `.env` in the current directory.

use crate::snapshot::{Page, PageBody};
use crate::transport::{b64, load_dotenv, pick};
use glia_code_domain::DocSourceKind;

/// Resolved Confluence credentials + site.
pub struct Config {
    pub site: String,
    pub email: String,
    pub token: String,
}

impl Config {
    /// Resolve from (flag → env → `./.env`) for each of site/email/token.
    pub fn resolve(
        site: Option<String>,
        email: Option<String>,
        token: Option<String>,
    ) -> Result<Config, String> {
        let dot = load_dotenv();
        let miss = |k: &str| format!("missing {k} (pass --{}, or set it in env / ./.env)", k.to_ascii_lowercase().replace("confluence_", ""));
        let cfg = Config {
            site: pick(site, "CONFLUENCE_SITE", &dot).ok_or_else(|| miss("CONFLUENCE_SITE"))?,
            email: pick(email, "CONFLUENCE_EMAIL", &dot).ok_or_else(|| miss("CONFLUENCE_EMAIL"))?,
            token: pick(token, "CONFLUENCE_TOKEN", &dot).ok_or_else(|| miss("CONFLUENCE_TOKEN"))?,
        };
        let origin = cfg.checked_origin()?;
        let transport = if origin.starts_with("http://") { "plain http, loopback" } else { "https" };
        eprintln!("[docs] origin={origin} ({transport})");
        Ok(cfg)
    }

    /// The scheme + authority every request and page URL is built on.
    ///
    /// `site` (the `--site` flag, or `CONFLUENCE_SITE` from env / `./.env`) is
    /// read as:
    /// - a bare host, e.g. `acme.atlassian.net` -> `https://acme.atlassian.net`
    ///   (unchanged from before origins were accepted);
    /// - `https://host[:port]` -> itself, minus a trailing `/`;
    /// - `http://host[:port]` -> itself, minus a trailing `/` — but a request is
    ///   only ever sent over plain http when the host is loopback (`127.0.0.1`,
    ///   `localhost` or `[::1]`, optionally with a numeric port). Any other
    ///   `http://` site is refused by [`Config::resolve`] and by every request,
    ///   because Basic credentials would travel in cleartext. The loopback
    ///   form exists for the test stub (`crate::stub`).
    pub fn origin(&self) -> String {
        let lower = self.site.to_ascii_lowercase();
        for scheme in ["https://", "http://"] {
            if lower.starts_with(scheme) {
                let rest = self.site[scheme.len()..].trim_end_matches('/');
                return format!("{scheme}{rest}");
            }
        }
        format!("https://{}", self.site)
    }

    /// [`Config::origin`], refused when it is plain http to a non-loopback
    /// host ([`crate::transport::checked_origin`], the rule every adapter shares).
    fn checked_origin(&self) -> Result<String, String> {
        crate::transport::checked_origin(&self.origin())
    }

    fn base(&self) -> Result<String, String> {
        Ok(format!("{}/wiki/rest/api", self.checked_origin()?))
    }
    fn auth(&self) -> String {
        format!("Basic {}", b64(format!("{}:{}", self.email, self.token).as_bytes()))
    }
}

/// A created/updated page, echoed back to the caller.
pub struct PageRef {
    pub id: String,
    pub title: String,
    pub version: i64,
    pub url: String,
}

/// Pull every page in a space, bodies included, as `Page`s ready for
/// `record_from_page`. Paginates the content endpoint (100/page) and expands
/// `body.storage` inline so there is no per-page round trip.
pub fn pull_space(cfg: &Config, space: &str) -> Result<Vec<Page>, String> {
    let base = cfg.base()?;
    let mut out = Vec::new();
    let limit = 100;
    let mut start = 0;
    loop {
        let url = format!(
            "{}/space/{}/content/page?limit={}&start={}&expand=body.storage,version",
            base,
            space,
            limit,
            start
        );
        let v = get_json(cfg, &url)?;
        let results = v["results"].as_array().cloned().unwrap_or_default();
        let n = results.len();
        for p in &results {
            let webui = p["_links"]["webui"].as_str().unwrap_or("");
            out.push(Page {
                kind: DocSourceKind::Confluence,
                container: space.to_string(),
                title: p["title"].as_str().unwrap_or("").to_string(),
                url: format!("{}/wiki{}", cfg.origin(), webui),
                version: p["version"]["number"].as_i64().unwrap_or(1).to_string(),
                body: PageBody::ConfluenceStorage(
                    p["body"]["storage"]["value"].as_str().unwrap_or("").to_string(),
                ),
                slug_hint: None,
            });
        }
        if n < limit {
            break;
        }
        start += limit;
    }
    Ok(out)
}

/// Fetch one page (id, title, current version, storage body).
pub fn fetch_page(cfg: &Config, id: &str) -> Result<Page, String> {
    let url = format!("{}/content/{}?expand=body.storage,version,space", cfg.base()?, id);
    let v = get_json(cfg, &url)?;
    let webui = v["_links"]["webui"].as_str().unwrap_or("");
    Ok(Page {
        kind: DocSourceKind::Confluence,
        container: v["space"]["key"].as_str().unwrap_or("").to_string(),
        title: v["title"].as_str().unwrap_or("").to_string(),
        url: format!("{}/wiki{}", cfg.origin(), webui),
        version: v["version"]["number"].as_i64().unwrap_or(1).to_string(),
        body: PageBody::ConfluenceStorage(
            v["body"]["storage"]["value"].as_str().unwrap_or("").to_string(),
        ),
        slug_hint: None,
    })
}

/// Create a page from a storage-format body.
pub fn create_page(cfg: &Config, space: &str, title: &str, storage: &str) -> Result<PageRef, String> {
    let body = serde_json::json!({
        "type": "page",
        "title": title,
        "space": { "key": space },
        "body": { "storage": { "value": storage, "representation": "storage" } },
    });
    let v = send_json(cfg, "POST", &format!("{}/content", cfg.base()?), body)?;
    page_ref(cfg, &v)
}

/// Update an existing page (bumps `version.number`).
pub fn update_page(
    cfg: &Config,
    id: &str,
    space: &str,
    title: &str,
    storage: &str,
) -> Result<PageRef, String> {
    let current = fetch_page(cfg, id)?;
    let next = current.version.parse::<i64>().unwrap_or(1) + 1;
    let body = serde_json::json!({
        "id": id,
        "type": "page",
        "title": title,
        "space": { "key": space },
        "body": { "storage": { "value": storage, "representation": "storage" } },
        "version": { "number": next },
    });
    let v = send_json(cfg, "PUT", &format!("{}/content/{}", cfg.base()?, id), body)?;
    page_ref(cfg, &v)
}

fn page_ref(cfg: &Config, v: &serde_json::Value) -> Result<PageRef, String> {
    let webui = v["_links"]["webui"].as_str().unwrap_or("");
    Ok(PageRef {
        id: v["id"].as_str().unwrap_or("").to_string(),
        title: v["title"].as_str().unwrap_or("").to_string(),
        version: v["version"]["number"].as_i64().unwrap_or(0),
        url: format!("{}/wiki{}", cfg.origin(), webui),
    })
}

// ---- HTTP helpers ---------------------------------------------------------

fn get_json(cfg: &Config, url: &str) -> Result<serde_json::Value, String> {
    let resp = ureq::get(url)
        .set("Authorization", &cfg.auth())
        .set("Accept", "application/json")
        .call();
    into_json(resp)
}

fn send_json(
    cfg: &Config,
    method: &str,
    url: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let resp = ureq::request(method, url)
        .set("Authorization", &cfg.auth())
        .set("Accept", "application/json")
        .send_json(body);
    into_json(resp)
}

fn into_json(resp: Result<ureq::Response, ureq::Error>) -> Result<serde_json::Value, String> {
    match resp {
        Ok(r) => r.into_json().map_err(|e| format!("decode json: {e}")),
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            Err(format!("HTTP {code}: {}", body.chars().take(300).collect::<String>()))
        }
        Err(e) => Err(format!("request failed: {e}")),
    }
}
