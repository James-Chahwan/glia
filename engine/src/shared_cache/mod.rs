//! The shared cache, engine half (CE.2a, + CE.2b, CE.2d): content-addressed
//! keys per cached file, ParseCache export and a verified import into the
//! local parse cache, and the whole-layout key / export / verified install for
//! a clean checkout at a known git tree. The transport that moves the keys and
//! payloads is CLI-only; nothing here does network I/O. Directory-module slot,
//! reached by module path (`glia_engine::shared_cache::<item>`): its owners add
//! their files (key.rs, export.rs, import.rs, layout.rs) and declare them here.
//! Filled by CE.2a.

mod export;
mod key;

pub use export::{CacheRow, CacheRows, Export, ExportedEntry, cache_rows, export_entries};
pub use key::{CacheKey, file_key};
