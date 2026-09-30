//! External doc-source adapters (Tier-4) — fetch + normalize Confluence /
//! Notion / wiki pages into `DocRecord`s the engine ingests. This crate does
//! the network I/O + format conversion; it stays OUT of the deterministic
//! engine and the published wheel (`py/`).
//!
//! Shipped today: the Confluence **storage-format XHTML → markdown** converter
//! (pure, offline-testable) — the piece that makes the doc→code linker work,
//! because it faithfully preserves code spans (`` `X` `` / fenced blocks) so the
//! existing backtick-identifier linker fires. The REST fetch + snapshot store +
//! Notion/wiki adapters build on top of it.
//!
//! The write direction is [`markdown`] — **markdown → storage-format XHTML**,
//! also pure and offline-testable, round-trip-pinned to `storage_to_markdown`.
//! It is what lets `glia docs push --markdown` take a plain `.md` file instead
//! of hand-written `ac:`/`ri:` macro XHTML.
//!
//! [`snapshot`] is the source-neutral seam every adapter writes through (a
//! [`Page`] of any [`glia_code_domain::DocSourceKind`], redacted, merged into
//! the manifest by (source, container)); [`transport`] holds the origin /
//! credential / base64 helpers the adapters share. [`wikidir`] reads a local
//! GitHub / GitLab wiki checkout's Markdown pages (CE.4b, `--source dir`), and
//! its `.mediawiki` / `.wiki` pages through [`wikitext`], a dependency-free
//! MediaWiki markup → markdown converter that keeps headings and code spans
//! (CE.4c).

pub mod confluence;
pub mod confluence_rest;
pub mod filter;
pub mod markdown;
pub mod snapshot;
#[doc(hidden)]
pub mod stub;
pub mod transport;
pub mod wikidir;
pub mod wikitext;

pub use filter::TitleFilter;
pub use snapshot::{
    Page, PageBody, SnapshotSource, SnapshotWrite, record_from_page, slug, write_snapshot,
};
