//! Build identity for glia.
//!
//! [`RELEASE`] is the human-facing version. [`PARSER_STAMP`] is a content hash
//! of every source file that can change graph content (see `build.rs` for the
//! exact input set). [`BUILD_STAMP`] combines them as semver build metadata —
//! `0.4.18+p3fa9c1d0b7e45a12` — and is what caches and manifests key on: the
//! release alone was the wrong granularity, because a parser fix merged without
//! a version bump left every incremental consumer serving pre-fix parses.

/// The workspace release version (`[workspace.package] version`).
pub const RELEASE: &str = env!("CARGO_PKG_VERSION");

/// 16 lowercase hex chars: FNV-1a over every graph-shaping source file.
pub const PARSER_STAMP: &str = env!("GLIA_PARSER_STAMP");

/// `<release>+p<parser stamp>` — the cache / manifest key.
pub const BUILD_STAMP: &str = concat!(env!("CARGO_PKG_VERSION"), "+p", env!("GLIA_PARSER_STAMP"));

/// Human-readable one-liner for `--version` style output.
pub const VERSION_LINE: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (build ",
    env!("CARGO_PKG_VERSION"),
    "+p",
    env!("GLIA_PARSER_STAMP"),
    ")"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_stamp_is_16_lowercase_hex() {
        assert_eq!(PARSER_STAMP.len(), 16, "PARSER_STAMP = {PARSER_STAMP:?}");
        assert!(
            PARSER_STAMP.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "PARSER_STAMP = {PARSER_STAMP:?} is not lowercase hex"
        );
        assert_ne!(PARSER_STAMP, "0000000000000000", "stamp hashed nothing");
    }

    #[test]
    fn build_stamp_composes_release_and_parser_stamp() {
        assert_eq!(BUILD_STAMP, format!("{RELEASE}+p{PARSER_STAMP}"));
        assert_ne!(BUILD_STAMP, RELEASE);
        assert_eq!(VERSION_LINE, format!("{RELEASE} (build {BUILD_STAMP})"));
    }
}
