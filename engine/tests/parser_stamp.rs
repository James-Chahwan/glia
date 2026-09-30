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

/// The parse-cache frame's magic (CD.7a) and the offset of the stamp after it
/// and its `u64` length.
const FRAME_MAGIC: &[u8] = b"GLIAPCZ1";
const STAMP_AT: usize = FRAME_MAGIC.len() + 8;

/// Read the build stamp from the `parse_cache.bin` frame header (CD.7a):
/// `b"GLIAPCZ1"`, then a `u64` little-endian byte length and the UTF-8 stamp,
/// uncompressed, so `ParseCache::load` can compare it without decompressing.
fn read_sidecar_stamp(bytes: &[u8]) -> String {
    assert!(
        bytes.len() > STAMP_AT,
        "parse_cache.bin is {} bytes — too short to hold the frame magic and a stamp length",
        bytes.len()
    );
    assert_eq!(
        &bytes[..FRAME_MAGIC.len()],
        FRAME_MAGIC,
        "parse_cache.bin does not open with the GLIAPCZ1 frame magic"
    );
    let len = u64::from_le_bytes(bytes[FRAME_MAGIC.len()..STAMP_AT].try_into().expect("8 bytes")) as usize;
    assert!(
        len > 0 && len < 64,
        "frame header stamp length is {len} — not a plausible build stamp"
    );
    assert!(
        bytes.len() >= STAMP_AT + len,
        "parse_cache.bin is truncated: need {} bytes, have {}",
        STAMP_AT + len,
        bytes.len()
    );
    let s = std::str::from_utf8(&bytes[STAMP_AT..STAMP_AT + len])
        .expect("frame header stamp is not valid UTF-8");
    assert!(
        s.chars().all(|c| c.is_ascii_graphic() || c == ' '),
        "frame header stamp {s:?} is not ASCII-printable — this is not the stamp field"
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

    // (4) The parser component is load-bearing: flip ONE hex digit of the
    // header stamp in place (same byte length, so the frame stays valid) and
    // the cache must be rejected.
    let flip_at = STAMP_AT + stamp.len() - 1;
    let mut tampered = bytes.clone();
    tampered[flip_at] = if tampered[flip_at] == b'0' { b'1' } else { b'0' };
    std::fs::write(&sidecar, &tampered).expect("rewrite tampered sidecar");
    assert!(
        ParseCache::load(repo_str).is_empty(),
        "parser component of the stamp is not part of the accept check"
    );
}
