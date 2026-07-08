//! Confluence Cloud REST — blocking client (ureq). This is the **network** half
//! of Tier-4 doc ingestion, deliberately kept in `doc-sources` (never in the
//! engine or the published wheel) so the engine stays deterministic and the
//! byte-identical build gate holds: `sync` fetches into a local snapshot; the
//! build only ever reads that snapshot.
//!
//! Auth is HTTP Basic `email:token` (a **classic** Atlassian API token — scoped
//! tokens without Confluence scopes 403). Credentials resolve, in order:
//! explicit args → process env → a `.env` in the current directory.

use crate::snapshot::Page;

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
        let pick = |flag: Option<String>, key: &str| -> Option<String> {
            flag.or_else(|| std::env::var(key).ok())
                .or_else(|| dot.get(key).cloned())
        };
        let miss = |k: &str| format!("missing {k} (pass --{}, or set it in env / ./.env)", k.to_ascii_lowercase().replace("confluence_", ""));
        Ok(Config {
            site: pick(site, "CONFLUENCE_SITE").ok_or_else(|| miss("CONFLUENCE_SITE"))?,
            email: pick(email, "CONFLUENCE_EMAIL").ok_or_else(|| miss("CONFLUENCE_EMAIL"))?,
            token: pick(token, "CONFLUENCE_TOKEN").ok_or_else(|| miss("CONFLUENCE_TOKEN"))?,
        })
    }

    fn base(&self) -> String {
        format!("https://{}/wiki/rest/api", self.site)
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
    let mut out = Vec::new();
    let limit = 100;
    let mut start = 0;
    loop {
        let url = format!(
            "{}/space/{}/content/page?limit={}&start={}&expand=body.storage,version",
            cfg.base(),
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
                space: space.to_string(),
                title: p["title"].as_str().unwrap_or("").to_string(),
                url: format!("https://{}/wiki{}", cfg.site, webui),
                version: p["version"]["number"].as_i64().unwrap_or(1).to_string(),
                storage: p["body"]["storage"]["value"].as_str().unwrap_or("").to_string(),
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
    let url = format!("{}/content/{}?expand=body.storage,version,space", cfg.base(), id);
    let v = get_json(cfg, &url)?;
    let webui = v["_links"]["webui"].as_str().unwrap_or("");
    Ok(Page {
        space: v["space"]["key"].as_str().unwrap_or("").to_string(),
        title: v["title"].as_str().unwrap_or("").to_string(),
        url: format!("https://{}/wiki{}", cfg.site, webui),
        version: v["version"]["number"].as_i64().unwrap_or(1).to_string(),
        storage: v["body"]["storage"]["value"].as_str().unwrap_or("").to_string(),
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
    let v = send_json(cfg, "POST", &format!("{}/content", cfg.base()), body)?;
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
    let v = send_json(cfg, "PUT", &format!("{}/content/{}", cfg.base(), id), body)?;
    page_ref(cfg, &v)
}

fn page_ref(cfg: &Config, v: &serde_json::Value) -> Result<PageRef, String> {
    let webui = v["_links"]["webui"].as_str().unwrap_or("");
    Ok(PageRef {
        id: v["id"].as_str().unwrap_or("").to_string(),
        title: v["title"].as_str().unwrap_or("").to_string(),
        version: v["version"]["number"].as_i64().unwrap_or(0),
        url: format!("https://{}/wiki{}", cfg.site, webui),
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

/// Load `./.env` into a map (best-effort; missing file → empty). `KEY=value`,
/// `#` comments and blank lines skipped. Does not touch the process env.
fn load_dotenv() -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    if let Ok(text) = std::fs::read_to_string(".env") {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                m.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    m
}

/// Minimal standard base64 (no deps) for the Basic-auth header.
fn b64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::b64;
    #[test]
    fn base64_matches_rfc_vectors() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob"), "Zm9vYg==");
        assert_eq!(b64(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }
}
