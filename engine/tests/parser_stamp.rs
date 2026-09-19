//! A1.1 gate: the on-disk parse-cache stamp must bind to the PARSER SOURCES,
//! not just to the release version.
//!
//! Why this test exists: `ParseCache::load` accepts any sidecar whose stored
//! stamp equals the build's stamp. While that stamp was `env!("CARGO_PKG_VERSION")`
//! ("0.4.18"), a parser or extractor fix merged without a workspace version bump
//! was invisible to every incremental consumer — every file whose bytes did not
//! change kept serving the PRE-FIX `FileParse`, while `bench/substrate-gap`
//! (which builds cold) happily reported the cell fixed. This test reads the raw
//! sidecar bytes so it asserts on what is actually written to disk, not on an
//! in-process constant.

use glia_engine::{ParseCache, generate_one_incremental};

const RELEASE: &str = env!("CARGO_PKG_VERSION");

/// Decode the leading bincode `String` of `parse_cache.bin`: bincode 1.x writes
/// a `u64` little-endian byte length followed by the UTF-8 bytes. `stamp` is the
/// first serialized field of `ParseCache`, so it sits at offset 0.
fn read_sidecar_stamp(bytes: &[u8]) -> String {
    assert!(
        bytes.len() > 8,
        "parse_cache.bin is {} bytes — too short to hold a bincode length prefix",
        bytes.len()
    );
    let len = u64::from_le_bytes(bytes[0..8].try_into().expect("8 bytes")) as usize;
    assert!(
        len > 0 && len < 64,
        "leading bincode string length is {len} — not a plausible build stamp; \
         the first serialized field of ParseCache is no longer the stamp"
    );
    assert!(
        bytes.len() >= 8 + len,
        "parse_cache.bin is truncated: need {} bytes, have {}",
        8 + len,
        bytes.len()
    );
    let s = std::str::from_utf8(&bytes[8..8 + len])
        .expect("leading bincode string is not valid UTF-8");
    assert!(
        s.chars().all(|c| c.is_ascii_graphic() || c == ' '),
        "leading bincode string {s:?} is not ASCII-printable — this is not the stamp field"
    );
    s.to_string()
}

#[test]
fn cache_stamp_binds_to_parser_sources_not_just_release() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path();
    std::fs::write(repo.join("alpha.py"), "def alpha():\n    return 1\n").expect("write alpha");
    std::fs::write(
        repo.join("beta.py"),
        "from alpha import alpha\n\n\ndef beta():\n    return alpha() + 1\n",
    )
    .expect("write beta");
    let repo_str = repo.to_str().expect("utf-8 tempdir path");

    generate_one_incremental(repo_str).expect("incremental build");

    let sidecar = glia_store::default_gmap_dir(repo).join("parse_cache.bin");
    let bytes = std::fs::read(&sidecar).expect("parse_cache.bin was written");
    let stamp = read_sidecar_stamp(&bytes);

    // (1) The core claim: the stamp is not merely the release version.
    assert_ne!(
        stamp, RELEASE,
        "on-disk cache stamp is the bare release version — a parser change cannot invalidate it"
    );

    // (2) Shape: `<release>+p<16 lowercase hex>`.
    let prefix = format!("{RELEASE}+p");
    assert!(
        stamp.starts_with(&prefix),
        "cache stamp {stamp:?} does not start with {prefix:?}"
    );
    let tail = &stamp[prefix.len()..];
    assert_eq!(tail.len(), 16, "parser-stamp tail {tail:?} is not 16 chars");
    assert!(
        tail.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "parser-stamp tail {tail:?} is not lowercase hex"
    );

    // (3) Positive control: an untouched cache is still reused.
    assert!(
        !ParseCache::load(repo_str).is_empty(),
        "a freshly written cache did not load back — the stamp check rejects its own output"
    );

    // (4) The parser component is load-bearing: flip ONE hex digit in place
    // (same byte length, so the bincode framing stays valid) and the cache must
    // be rejected.
    let flip_at = 8 + stamp.len() - 1;
    let mut tampered = bytes.clone();
    tampered[flip_at] = if tampered[flip_at] == b'0' { b'1' } else { b'0' };
    std::fs::write(&sidecar, &tampered).expect("rewrite tampered sidecar");
    assert!(
        ParseCache::load(repo_str).is_empty(),
        "parser component of the stamp is not part of the accept check"
    );
}
