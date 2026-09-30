//! The HTTPS object store (CE.2e): a cache store served over HTTP by any
//! static server that answers GET / HEAD and accepts PUT (nginx with WebDAV
//! PUT, an S3 bucket behind a proxy). The object at `rel` is `<base>/<rel>`:
//!
//! - `get` is `GET`: 200 is the object, read at most `MAX_OBJECT_BYTES + 1`
//!   bytes (the directory store's bound, so an oversized object is refused by
//!   `object::decode` alike); 404 is no object; any other status is a store
//!   failure `GET <rel>: HTTP <code>`.
//! - `has` is `HEAD`: 200 present, 404 absent, anything else a failure.
//! - `put` is `PUT` of the object bytes (`application/octet-stream`); 200,
//!   201 and 204 are success.
//!
//! Redirects are not followed (a 3xx is a failure), so the bearer token only
//! ever reaches the origin named on the command line. Every request carries
//! `User-Agent: glia/<release>` and, when `GLIA_CACHE_TOKEN` is set,
//! `Authorization: Bearer <token>`. Plain `http://` is allowed only to a
//! loopback host (`127.0.0.1`, `localhost`, `[::1]`; the rule
//! `glia_doc_sources::transport::is_loopback_authority` applies to every
//! glia transport), so neither the token nor the objects travel in cleartext
//! over a network. The object format and its checks are the directory
//! store's: an HTML error page served with 200 is refused as malformed.
//!
//! Only the glia binary speaks HTTP: neither the engine nor the glia-py wheel
//! links an HTTP client. The store is `Sync` (a `ureq::Agent` is), so a
//! pull's fetch workers share one agent and its connection pool.
//!
//! fired_on marker, once per push or pull through this store (host only,
//! never the path or the token):
//! `[cache] store=<scheme>://<host> transport=<https|http-loopback> requests=<n> (get=<g> head=<h> put=<p>) status_4xx=<a> status_5xx=<b>`

use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use glia_doc_sources::transport::is_loopback_authority;

use super::object::MAX_OBJECT_BYTES;
use super::store::{ObjectStore, validate_rel};

/// The environment variable holding the store's bearer token.
pub(crate) const TOKEN_ENV: &str = "GLIA_CACHE_TOKEN";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(60);
/// Most bytes of an error answer's body read (to let the connection be reused).
const DRAIN_BYTES: u64 = 64 * 1024;

/// How a store URL is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transport {
    Https,
    /// Plain `http://` to a loopback host (a local server, the tests).
    HttpLoopback,
}

/// A checked store URL: `https://<host>[/path]`, or `http://` to a loopback
/// host; no credentials, query or fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoreUrl {
    transport: Transport,
    /// `<scheme>://<authority>`, for the marker.
    origin: String,
    /// `<origin><path>`, without a trailing `/`.
    base: String,
}

impl StoreUrl {
    /// Parse the part of a STORE argument after `<scheme>://`, where `scheme`
    /// is `http` or `https` (any case). A usage error names what is wrong
    /// and never echoes credentials.
    pub(crate) fn parse(scheme: &str, rest: &str) -> Result<StoreUrl, String> {
        let transport = if scheme.eq_ignore_ascii_case("https") {
            Transport::Https
        } else if scheme.eq_ignore_ascii_case("http") {
            Transport::HttpLoopback
        } else {
            return Err(format!("unsupported store scheme {scheme}://"));
        };
        let scheme = if transport == Transport::Https {
            "https"
        } else {
            "http"
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(format!(
                "store URL {scheme}://…: credentials in the URL are not supported; set {TOKEN_ENV}"
            ));
        }
        if authority.is_empty() {
            return Err(format!("store URL {scheme}://{rest}: no host"));
        }
        if rest.contains(['?', '#']) || rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(format!(
                "store URL {scheme}://{authority}: a query, fragment or space is not supported"
            ));
        }
        if transport == Transport::HttpLoopback && !is_loopback_authority(authority) {
            return Err(format!(
                "refusing plain http:// to non-loopback host {authority}: the token and objects would travel in cleartext; use https://"
            ));
        }
        let origin = format!("{scheme}://{authority}");
        let base = format!("{origin}{}", path.trim_end_matches('/'));
        Ok(StoreUrl {
            transport,
            origin,
            base,
        })
    }
}

/// The token in `GLIA_CACHE_TOKEN`, if set and non-empty. It must be
/// printable ASCII without spaces (a header value); the error never shows it.
pub(crate) fn token_from_env() -> Result<Option<String>, String> {
    let Some(raw) = std::env::var_os(TOKEN_ENV) else {
        return Ok(None);
    };
    let Some(token) = raw.to_str().map(str::trim) else {
        return Err(format!("{TOKEN_ENV} is not valid UTF-8"));
    };
    if token.is_empty() {
        return Ok(None);
    }
    if !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(format!(
            "{TOKEN_ENV} holds a character an HTTP header cannot carry (printable ASCII, no spaces)"
        ));
    }
    Ok(Some(token.to_string()))
}

/// Read and drop at most [`DRAIN_BYTES`] of an answer's body, so its
/// connection can go back to the agent's pool.
fn drain(resp: ureq::Response) {
    let _ = std::io::copy(
        &mut resp.into_reader().take(DRAIN_BYTES),
        &mut std::io::sink(),
    );
}

/// Per-method request counts and answer classes, for the marker.
#[derive(Debug, Default)]
struct Counts {
    get: AtomicUsize,
    head: AtomicUsize,
    put: AtomicUsize,
    status_4xx: AtomicUsize,
    status_5xx: AtomicUsize,
}

/// A store at an HTTPS (or loopback HTTP) URL.
pub(crate) struct HttpStore {
    agent: ureq::Agent,
    url: StoreUrl,
    token: Option<String>,
    counts: Counts,
}

impl HttpStore {
    pub(crate) fn new(url: StoreUrl, token: Option<String>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(IO_TIMEOUT)
            .timeout_write(IO_TIMEOUT)
            .redirects(0)
            .user_agent(&format!("glia/{}", glia_engine::RELEASE))
            .build();
        HttpStore {
            agent,
            url,
            token,
            counts: Counts::default(),
        }
    }

    /// Send `method` for `rel` (with `body` for a PUT) and return the answer
    /// whatever its status; `Err` for an invalid path or a transport failure.
    fn send(&self, method: &str, rel: &str, body: Option<&[u8]>) -> Result<ureq::Response, String> {
        validate_rel(rel)?;
        let counter = match method {
            "GET" => &self.counts.get,
            "HEAD" => &self.counts.head,
            _ => &self.counts.put,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        let mut req = self
            .agent
            .request(method, &format!("{}/{rel}", self.url.base));
        if let Some(t) = &self.token {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        let sent = match body {
            Some(b) => req
                .set("Content-Type", "application/octet-stream")
                .send_bytes(b),
            None => req.call(),
        };
        let resp = match sent {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => return Err(format!("{method} {rel}: {t}")),
        };
        match resp.status() {
            400..=499 => self.counts.status_4xx.fetch_add(1, Ordering::Relaxed),
            500..=599 => self.counts.status_5xx.fetch_add(1, Ordering::Relaxed),
            _ => 0,
        };
        Ok(resp)
    }

    /// The store failure for an answer of an unexpected status.
    fn unexpected(method: &str, rel: &str, resp: ureq::Response) -> String {
        let code = resp.status();
        drain(resp);
        let note = if (300..400).contains(&code) {
            " (redirects are not followed)"
        } else {
            ""
        };
        format!("{method} {rel}: HTTP {code}{note}")
    }

    /// The fired_on marker line: the origin, never the path or the token.
    pub(crate) fn marker(&self) -> String {
        let c = &self.counts;
        let (get, head, put) = (
            c.get.load(Ordering::Relaxed),
            c.head.load(Ordering::Relaxed),
            c.put.load(Ordering::Relaxed),
        );
        let transport = match self.url.transport {
            Transport::Https => "https",
            Transport::HttpLoopback => "http-loopback",
        };
        format!(
            "[cache] store={} transport={transport} requests={} (get={get} head={head} put={put}) status_4xx={} status_5xx={}",
            self.url.origin,
            get + head + put,
            c.status_4xx.load(Ordering::Relaxed),
            c.status_5xx.load(Ordering::Relaxed)
        )
    }
}

impl ObjectStore for HttpStore {
    fn label(&self) -> String {
        self.url.base.clone()
    }

    fn get(&self, rel: &str) -> Result<Option<Vec<u8>>, String> {
        let resp = self.send("GET", rel, None)?;
        match resp.status() {
            200 => {
                let mut out = Vec::new();
                resp.into_reader()
                    .take(MAX_OBJECT_BYTES as u64 + 1)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("GET {rel}: {e}"))?;
                Ok(Some(out))
            }
            404 => {
                drain(resp);
                Ok(None)
            }
            _ => Err(Self::unexpected("GET", rel, resp)),
        }
    }

    fn has(&self, rel: &str) -> Result<bool, String> {
        let resp = self.send("HEAD", rel, None)?;
        match resp.status() {
            200 => Ok(true),
            404 => Ok(false),
            _ => Err(Self::unexpected("HEAD", rel, resp)),
        }
    }

    fn put(&self, rel: &str, bytes: &[u8]) -> Result<(), String> {
        let resp = self.send("PUT", rel, Some(bytes))?;
        match resp.status() {
            200 | 201 | 204 => {
                drain(resp);
                Ok(())
            }
            _ => Err(Self::unexpected("PUT", rel, resp)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Result<StoreUrl, String> {
        let (scheme, rest) = s.split_once("://").expect("a URL");
        StoreUrl::parse(scheme, rest)
    }

    #[test]
    fn store_urls_parse_and_refuse() {
        let u = url("https://cache.example/glia/").expect("https");
        assert_eq!(u.transport, Transport::Https);
        assert_eq!(u.origin, "https://cache.example");
        assert_eq!(u.base, "https://cache.example/glia");
        assert_eq!(
            url("HTTPS://h:8443").expect("bare host").base,
            "https://h:8443"
        );
        let local = url("http://127.0.0.1:8080/x").expect("loopback");
        assert_eq!(local.transport, Transport::HttpLoopback);
        assert_eq!(local.origin, "http://127.0.0.1:8080");
        assert!(url("http://[::1]/x").is_ok());
        assert!(url("http://localhost/x").is_ok());

        let remote = url("http://example.com/x").expect_err("remote http");
        assert!(
            remote.starts_with("refusing plain http:// to non-loopback host example.com"),
            "{remote}"
        );
        for bad in [
            "http://127.0.0.1.evil.example/x",
            "http://127.0.0.1:80@evil.example/x",
        ] {
            assert!(url(bad).is_err(), "{bad}");
        }
        let creds = url("https://user:s3cret@h/x").expect_err("userinfo");
        assert!(!creds.contains("s3cret"), "{creds}");
        for bad in [
            "https:///x",
            "https://h/x?sig=1",
            "https://h/x#f",
            "https://h/a b",
        ] {
            assert!(url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_marker_names_the_origin_only() {
        let store = HttpStore::new(
            url("https://cache.example/team/glia").expect("url"),
            Some("t0k".to_string()),
        );
        assert_eq!(store.label(), "https://cache.example/team/glia");
        assert_eq!(
            store.marker(),
            "[cache] store=https://cache.example transport=https requests=0 (get=0 head=0 put=0) status_4xx=0 status_5xx=0"
        );
        // An invalid path is refused before any request.
        assert!(store.get("../x").is_err());
        assert!(store.marker().contains("requests=0"));
    }
}
