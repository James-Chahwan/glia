//! The content address of one cached parse (CE.2a).
//!
//! A shared store accepts objects written on other machines, so the address is
//! a cryptographic hash (blake3), never the local cache's xxhash64. It hashes
//! every input the build's language branch reads to produce the cached parse
//! (`route::route_branches`), and nothing else:
//!
//! - the build stamp (`BUILD_STAMP`, the parse cache's `CACHE_VERSION`): every
//!   parser or extractor change moves it;
//! - the repo identity key (`walk_gating::repo_identity`), baked into every
//!   NodeId of the parse;
//! - the language tag (`route::parser_route`);
//! - the repo-relative path as the walk spells it (`/`-separated on Unix; the
//!   parse bakes the same spelling into its nodes, so a checkout that spells it
//!   otherwise must not share the object);
//! - the MODULE qname the LB.9b plan gives the file (`ModuleQnames::plan`: a
//!   same-stem sibling names it by file name), which names every symbol under it;
//! - the go.mod module-set key (`GoModules::context_key`) for a Go file only:
//!   the Go parser is the one parser handed the module set (`parse_one_as`);
//! - the blake3 of the file's content.
//!
//! The TS path aliases, the C/C++ include roots and the `.glia` overlay are read
//! after the cache (the build's post-cache grafts and graph build), so no key
//! needs them. The key is an address, not a secret and not a signature: the
//! CLI's object MAC (CE.2c) authenticates a fetched payload.

use std::fmt;

/// blake3 `derive_key` context: the key's domain and version. A change to the
/// field list or their framing is a new context string, never a reuse.
const KEY_CONTEXT: &str = "glia parse cache object v1";

/// The 32-byte blake3 content address of one cached parse ([`file_key`]).
/// Its text form ([`CacheKey::to_hex`], `Display`, serde) is 64 lower-case hex
/// characters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheKey([u8; 32]);

impl CacheKey {
    /// The key of these 32 raw bytes (a store object carries the key raw).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        CacheKey(bytes)
    }

    /// The key's 32 raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 64 lower-case hex characters.
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(64);
        for b in self.0 {
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 0x0f)]));
        }
        out
    }

    /// The inverse of [`CacheKey::to_hex`]: exactly 64 lower-case hex
    /// characters, `Err` on anything else (upper case, whitespace, another
    /// length), so one key has one text form.
    pub fn from_hex(s: &str) -> Result<Self, String> {
        fn nibble(c: u8) -> Option<u8> {
            match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                _ => None,
            }
        }
        let bytes = s.as_bytes();
        if bytes.len() != 64 {
            return Err(format!(
                "cache key: want 64 hex characters, got {}",
                bytes.len()
            ));
        }
        let mut out = [0u8; 32];
        for (o, pair) in out.iter_mut().zip(bytes.chunks_exact(2)) {
            let (Some(hi), Some(lo)) = (nibble(pair[0]), nibble(pair[1])) else {
                return Err("cache key: not lower-case hex".to_string());
            };
            *o = (hi << 4) | lo;
        }
        Ok(CacheKey(out))
    }
}

impl fmt::Display for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CacheKey({})", self.to_hex())
    }
}

impl serde::Serialize for CacheKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

/// The content address of the parse of one file: blake3 `derive_key`
/// (context `"glia parse cache object v1"`) over each field framed as a `u64` LE length and its bytes,
/// in this order: `stamp`, `repo_key`, `lang`, `path`, `module_qname`, the Go
/// module-set key (`go_ctx` when `lang` is `"go"`, else `""`), then the 32-byte
/// blake3 of `content`. The framing makes the fields unambiguous (`"ab" + "c"`
/// never hashes as `"a" + "bc"`), and a non-Go file's key never moves with
/// the go.mod set it does not read.
pub fn file_key(
    stamp: &str,
    repo_key: &str,
    lang: &str,
    path: &str,
    module_qname: &str,
    go_ctx: &str,
    content: &[u8],
) -> CacheKey {
    let go = if lang == "go" { go_ctx } else { "" };
    let content_hash = blake3::hash(content);
    let mut h = blake3::Hasher::new_derive_key(KEY_CONTEXT);
    for field in [
        stamp.as_bytes(),
        repo_key.as_bytes(),
        lang.as_bytes(),
        path.as_bytes(),
        module_qname.as_bytes(),
        go.as_bytes(),
        content_hash.as_bytes().as_slice(),
    ] {
        h.update(&(field.len() as u64).to_le_bytes());
        h.update(field);
    }
    CacheKey(*h.finalize().as_bytes())
}
