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

pub mod confluence;
pub mod confluence_rest;
pub mod snapshot;

pub use snapshot::{Page, record_from_page, slug, write_snapshot};
