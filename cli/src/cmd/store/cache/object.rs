//! The store object of one cached parse (CE.2c): the bytes a shared store
//! holds per content address, and their authentication.
//!
//! ```text
//! b"GLIAPC01" (8) | flags u8 (bit0 = signed) | key [u8; 32] | payload_len u64 LE | payload
//!                 | mac [u8; 32]   (signed only: blake3::keyed_hash(signing key, every byte before it))
//! ```
//!
//! The payload is the engine's export of one sidecar entry
//! (`glia_engine::shared_cache::ExportedEntry::payload`); the key is its
//! content address (`CacheKey`). A store serves objects written on other
//! machines, so [`decode`] trusts nothing it can check: it bounds the size
//! before looking inside, checks every length against the bytes present, and
//! refuses an object that names another key than the one requested (a store
//! cannot answer one key with another key's object). Under a keyed
//! [`Signing`] it also refuses an unsigned object and verifies the MAC with a
//! constant-time compare (`blake3::Hash`'s `Eq`). blake3's keyed hash is a
//! MAC, so no hmac crate is needed. The MAC lives here, in the CLI, and
//! nowhere in the engine or the glia-py wheel (James's ruling, 2026-09-30).
//!
//! The signing key is never printed: [`Signing`]'s `Debug` says only whether
//! it holds one, and no error quotes the key text.

use std::fmt;

use glia_engine::shared_cache::CacheKey;

/// The object format's magic and version.
const MAGIC: &[u8; 8] = b"GLIAPC01";
/// Flag bit 0: the object carries a MAC.
const FLAG_SIGNED: u8 = 1;
/// Magic, flags, key and payload length.
const HEADER_LEN: usize = 8 + 1 + 32 + 8;
/// The MAC's length (a blake3 hash).
const MAC_LEN: usize = 32;
/// The largest object [`decode`] reads. A store read (`ObjectStore::get`)
/// stops one byte past it, so an oversized object costs at most this much
/// memory before it is refused.
pub(crate) const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
/// The environment variable a signing key is read from ([`Signing::from_env`]).
pub(crate) const KEY_ENV: &str = "GLIA_CACHE_KEY";

/// Why [`decode`] refused an object. Its text never quotes the object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reject {
    /// Larger than [`MAX_OBJECT_BYTES`].
    TooLarge,
    /// Not a well-formed object: what is wrong with it.
    Format(&'static str),
    /// A well-formed object of another key than the one requested.
    KeyMismatch,
    /// An object without a MAC, read under a keyed [`Signing`].
    Unsigned,
    /// The MAC does not verify: another signing key, or altered bytes.
    BadMac,
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reject::TooLarge => write!(f, "object larger than {MAX_OBJECT_BYTES} bytes"),
            Reject::Format(what) => write!(f, "not a cache object ({what})"),
            Reject::KeyMismatch => f.write_str("object of another key than its path names"),
            Reject::Unsigned => f.write_str("unsigned object in a keyed store"),
            Reject::BadMac => f.write_str("MAC does not verify (another key, or altered bytes)"),
        }
    }
}

/// How objects are signed and checked: with a 32-byte blake3 key (signed on
/// [`encode`], MAC required on [`decode`]) or without one (`--unsigned`:
/// written without a MAC, and any object's MAC left unchecked).
#[derive(Clone)]
pub(crate) struct Signing {
    key: Option<[u8; 32]>,
}

impl fmt::Debug for Signing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signing")
            .field("signed", &self.is_signed())
            .finish()
    }
}

impl Signing {
    /// A key given as 64 hex characters (either case), surrounding whitespace
    /// trimmed. The error names the problem, never the text.
    pub(crate) fn from_hex(text: &str) -> Result<Self, String> {
        fn nibble(c: u8) -> Option<u8> {
            match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            }
        }
        let bytes = text.trim().as_bytes();
        if bytes.len() != 64 {
            return Err(format!(
                "a cache key is 64 hex characters (32 bytes), this one has {}",
                bytes.len()
            ));
        }
        let mut key = [0u8; 32];
        for (k, pair) in key.iter_mut().zip(bytes.chunks_exact(2)) {
            let (Some(hi), Some(lo)) = (nibble(pair[0]), nibble(pair[1])) else {
                return Err("a cache key is hex; this one has a non-hex character".to_string());
            };
            *k = (hi << 4) | lo;
        }
        Ok(Signing { key: Some(key) })
    }

    /// The key in `GLIA_CACHE_KEY`: `Ok(None)` when it is unset or blank.
    pub(crate) fn from_env() -> Result<Option<Self>, String> {
        match std::env::var(KEY_ENV) {
            Ok(v) if !v.trim().is_empty() => Self::from_hex(&v)
                .map(Some)
                .map_err(|e| format!("{KEY_ENV}: {e}")),
            Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(format!(
                "{KEY_ENV}: a cache key is hex; this one is not text"
            )),
        }
    }

    /// No key: objects are written without a MAC and read without a check.
    pub(crate) fn unsigned() -> Self {
        Signing { key: None }
    }

    pub(crate) fn is_signed(&self) -> bool {
        self.key.is_some()
    }
}

/// The object for `payload` under `key`, with a MAC when `signing` has a key.
pub(crate) fn encode(key: &CacheKey, payload: &[u8], signing: &Signing) -> Vec<u8> {
    let mac_len = if signing.is_signed() { MAC_LEN } else { 0 };
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + mac_len);
    out.extend_from_slice(MAGIC);
    out.push(if signing.is_signed() { FLAG_SIGNED } else { 0 });
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(payload);
    if let Some(k) = &signing.key {
        let mac = blake3::keyed_hash(k, &out);
        out.extend_from_slice(mac.as_bytes());
    }
    out
}

/// The payload of the object `bytes`, fetched for `expected`, or why it is
/// refused. Checks, in order: the size bound, the header, the key, the
/// length, then (when `signing` has a key) that the object is signed and its
/// MAC verifies. Nothing is allocated from a length the object states.
pub(crate) fn decode(
    bytes: &[u8],
    expected: &CacheKey,
    signing: &Signing,
) -> Result<Vec<u8>, Reject> {
    if bytes.len() > MAX_OBJECT_BYTES {
        return Err(Reject::TooLarge);
    }
    if bytes.len() < HEADER_LEN {
        return Err(Reject::Format("shorter than the header"));
    }
    let (magic, rest) = bytes.split_at(MAGIC.len());
    if magic != MAGIC {
        return Err(Reject::Format("bad magic"));
    }
    let (flags, rest) = rest.split_at(1);
    let flags = flags[0];
    if flags & !FLAG_SIGNED != 0 {
        return Err(Reject::Format("unknown flag bits"));
    }
    let signed = flags & FLAG_SIGNED != 0;
    let (key, rest) = rest.split_at(32);
    if key != expected.as_bytes() {
        return Err(Reject::KeyMismatch);
    }
    let (len, rest) = rest.split_at(8);
    let mut len_le = [0u8; 8];
    len_le.copy_from_slice(len);
    let payload_len = u64::from_le_bytes(len_le);
    let mac_len = if signed { MAC_LEN } else { 0 };
    // Checked against the bytes present before anything is sliced by it.
    let present = rest.len() as u64;
    if present < mac_len as u64 || payload_len != present - mac_len as u64 {
        return Err(Reject::Format(
            "payload length does not match the object size",
        ));
    }
    let (payload, mac) = rest.split_at(rest.len() - mac_len);
    if let Some(k) = &signing.key {
        if !signed {
            return Err(Reject::Unsigned);
        }
        let mut mac_bytes = [0u8; 32];
        mac_bytes.copy_from_slice(mac);
        let want = blake3::keyed_hash(k, &bytes[..bytes.len() - MAC_LEN]);
        // blake3::Hash's PartialEq is constant-time.
        if want != blake3::Hash::from_bytes(mac_bytes) {
            return Err(Reject::BadMac);
        }
    }
    Ok(payload.to_vec())
}

/// The store path of `key`'s object under build stamp `stamp`:
/// `v1/<stamp>/<key[0..2]>/<key>.gpc`. The stamp is a directory so GC can
/// drop a whole release at once; `+` is a valid path and URL segment
/// character.
pub(crate) fn object_rel(stamp: &str, key: &CacheKey) -> String {
    let hex = key.to_hex();
    format!("v1/{stamp}/{}/{hex}.gpc", &hex[..2])
}

#[cfg(test)]
mod tests {
    use super::super::store::{DirStore, ObjectStore};
    use super::*;

    fn key(b: u8) -> CacheKey {
        CacheKey::from_bytes([b; 32])
    }

    fn signing(b: u8) -> Signing {
        Signing { key: Some([b; 32]) }
    }

    #[test]
    fn objects_are_authenticated() {
        let k = signing(7);
        let payload = b"a cached parse".to_vec();
        let obj = encode(&key(1), &payload, &k);
        assert_eq!(decode(&obj, &key(1), &k), Ok(payload.clone()));

        // One payload byte flipped.
        let mut flipped = obj.clone();
        flipped[HEADER_LEN] ^= 1;
        assert_eq!(decode(&flipped, &key(1), &k), Err(Reject::BadMac));
        // Another signing key.
        assert_eq!(decode(&obj, &key(1), &signing(8)), Err(Reject::BadMac));
        // The object served at another key's path.
        assert_eq!(decode(&obj, &key(2), &k), Err(Reject::KeyMismatch));
        // Rewritten to name the other key: its MAC no longer verifies.
        let mut renamed = obj.clone();
        renamed[9..41].copy_from_slice(key(2).as_bytes());
        assert_eq!(decode(&renamed, &key(2), &k), Err(Reject::BadMac));
        // An unsigned object under a keyed Signing.
        let unsigned = encode(&key(1), &payload, &Signing::unsigned());
        assert_eq!(decode(&unsigned, &key(1), &k), Err(Reject::Unsigned));
        // The unsigned mode reads both (the re-parse sample is its check).
        assert_eq!(
            decode(&unsigned, &key(1), &Signing::unsigned()),
            Ok(payload.clone())
        );
        assert_eq!(
            decode(&obj, &key(1), &Signing::unsigned()),
            Ok(payload.clone())
        );

        // Malformed: truncated, bad magic, unknown flags, lying lengths.
        assert!(matches!(
            decode(&obj[..HEADER_LEN - 1], &key(1), &k),
            Err(Reject::Format(_))
        ));
        let mut magic = obj.clone();
        magic[0] = b'X';
        assert_eq!(
            decode(&magic, &key(1), &k),
            Err(Reject::Format("bad magic"))
        );
        let mut flags = obj.clone();
        flags[8] |= 0x80;
        assert_eq!(
            decode(&flags, &key(1), &k),
            Err(Reject::Format("unknown flag bits"))
        );
        for len in [0u64, payload.len() as u64 + 1, u64::MAX] {
            let mut lying = unsigned.clone();
            lying[41..49].copy_from_slice(&len.to_le_bytes());
            assert!(
                matches!(
                    decode(&lying, &key(1), &Signing::unsigned()),
                    Err(Reject::Format(_))
                ),
                "length {len} accepted"
            );
        }
        // A signed object too short to hold its MAC.
        let mut short = encode(&key(1), b"", &k);
        short.truncate(HEADER_LEN + 3);
        assert!(matches!(
            decode(&short, &key(1), &k),
            Err(Reject::Format(_))
        ));

        // 65 MiB on disk: the store read stops one byte past the bound and
        // decode refuses it before looking inside.
        let dir = std::env::temp_dir().join(format!("glia-ce2c-obj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = DirStore::new(&dir);
        let rel = object_rel("0.0.0+ptest", &key(3));
        store.put(&rel, b"x").expect("put");
        let big = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.join(&rel))
            .expect("open object");
        big.set_len(65 * 1024 * 1024)
            .expect("grow to 65 MiB (sparse)");
        let read = store.get(&rel).expect("get").expect("present");
        assert_eq!(read.len(), MAX_OBJECT_BYTES + 1);
        assert_eq!(decode(&read, &key(3), &k), Err(Reject::TooLarge));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_parse_and_never_print() {
        let hex = "ab".repeat(32);
        let s = Signing::from_hex(&format!("  {hex}\n")).expect("trimmed hex");
        assert_eq!(s.key, Some([0xab; 32]));
        assert_eq!(
            Signing::from_hex(&hex.to_uppercase()).expect("upper").key,
            Some([0xab; 32])
        );
        assert!(format!("{s:?}").contains("signed: true"));
        assert!(!format!("{s:?}").contains("ab"), "Debug printed the key");
        let err = Signing::from_hex(&"zz".repeat(32)).expect_err("non-hex");
        assert!(!err.contains("zz"), "{err}");
        let err = Signing::from_hex("secretsecret").expect_err("short");
        assert!(!err.contains("secret"), "{err}");
    }

    #[test]
    fn object_paths_fan_out_by_key() {
        let k = CacheKey::from_hex(&format!("0f{}", "1".repeat(62))).expect("hex");
        assert_eq!(
            object_rel("0.5.1+p0123456789abcdef", &k),
            format!("v1/0.5.1+p0123456789abcdef/0f/0f{}.gpc", "1".repeat(62))
        );
    }
}
