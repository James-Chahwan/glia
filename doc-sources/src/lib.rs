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

pub mod confluence;
pub mod confluence_rest;
pub mod filter;
pub mod markdown;
pub mod snapshot;

pub use filter::TitleFilter;
pub use snapshot::{Page, record_from_page, slug, write_snapshot};
