//! Transport helpers every doc-source adapter shares (CE.4a): the
//! plain-http-only-to-loopback origin check, `./.env` loading, the
//! flag -> env -> `./.env` credential pick and the Basic-auth base64.
//!
//! Moved verbatim out of `confluence_rest` so the Notion / MediaWiki adapters
//! refuse cleartext credentials with the same rule and the same message; the
//! LA.11 tests in `tests/confluence_stub.rs` pin both through Confluence.

use std::collections::HashMap;

/// `host[:port]` where host is `127.0.0.1`, `localhost` or `[::1]` (any case)
/// and the port, when present, is all digits. Anything else — a path, userinfo
/// (`127.0.0.1:80@evil.example`), a look-alike (`127.0.0.1.evil.example`) — is
/// not loopback.
pub fn is_loopback_authority(authority: &str) -> bool {
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => match v6.split_once(']') {
            Some((inner, after)) => (format!("[{inner}]"), after),
            None => return false,
        },
        None => match authority.find(':') {
            Some(i) => (authority[..i].to_string(), &authority[i..]),
            None => (authority.to_string(), ""),
        },
    };
    let port_ok = match port.strip_prefix(':') {
        Some(digits) => !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
        None => port.is_empty(),
    };
    port_ok && ["127.0.0.1", "localhost", "[::1]"].contains(&host.to_ascii_lowercase().as_str())
}

/// `origin` (scheme + authority, no path: `https://host[:port]` or
/// `http://host[:port]`), refused when it is plain http to a non-loopback
/// host: credentials would travel in cleartext. The loopback form exists for
/// the test stub (`crate::stub`).
pub fn checked_origin(origin: &str) -> Result<String, String> {
    match origin.strip_prefix("http://") {
        Some(authority) if !is_loopback_authority(authority) => Err(format!(
            "refusing plain http:// to non-loopback host {authority}: Basic credentials would travel in cleartext"
        )),
        _ => Ok(origin.to_string()),
    }
}

/// Load `./.env` into a map (best-effort; missing file → empty). `KEY=value`,
/// `#` comments and blank lines skipped. Does not touch the process env.
pub fn load_dotenv() -> HashMap<String, String> {
    let mut m = HashMap::new();
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

/// One credential, resolved flag → process env `key` → `dotenv[key]`.
pub fn pick(flag: Option<String>, key: &str, dotenv: &HashMap<String, String>) -> Option<String> {
    flag.or_else(|| std::env::var(key).ok())
        .or_else(|| dotenv.get(key).cloned())
}

/// Minimal standard base64 (no deps) for the Basic-auth header.
pub fn b64(input: &[u8]) -> String {
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
    use super::*;

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

    #[test]
    fn checked_origin_refuses_cleartext_to_remote_hosts_only() {
        assert_eq!(checked_origin("https://example.com"), Ok("https://example.com".to_string()));
        assert_eq!(checked_origin("http://127.0.0.1:9"), Ok("http://127.0.0.1:9".to_string()));
        assert_eq!(checked_origin("http://[::1]:9"), Ok("http://[::1]:9".to_string()));
        let err = checked_origin("http://example.com").err().unwrap_or_default();
        assert_eq!(
            err,
            "refusing plain http:// to non-loopback host example.com: Basic credentials would travel in cleartext"
        );
        assert!(checked_origin("http://127.0.0.1:80@example.com").is_err());
        assert!(checked_origin("http://127.0.0.1:9/wiki").is_err(), "an origin carries no path");
    }

    #[test]
    fn pick_prefers_flag_then_env_then_dotenv() {
        let dot: HashMap<String, String> =
            [("GLIA_CE4A_PICK_ONLY_IN_DOTENV".to_string(), "dot".to_string())].into();
        assert_eq!(
            pick(Some("flag".into()), "GLIA_CE4A_PICK_ONLY_IN_DOTENV", &dot).as_deref(),
            Some("flag")
        );
        assert_eq!(pick(None, "GLIA_CE4A_PICK_ONLY_IN_DOTENV", &dot).as_deref(), Some("dot"));
        assert_eq!(pick(None, "GLIA_CE4A_PICK_NOWHERE", &dot), None);
    }
}
