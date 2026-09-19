//! glia-store — `.gmap` container format: write a `RepoGraph` to disk
//! as a domain-free rkyv core `Container` plus a code section, mmap it back
//! zero-copy, and traverse the archived form via `NodeLike` / `EdgeLike`
//! (see `glia-core`).
//!
//! Design notes (locked 2026-04-17):
//! - **Preamble + domain-free core + named sections** (LC.5b, format 2): a
//!   32-byte preamble (`GLIAGMAP` magic, format version, the core's offset
//!   and length), then each domain's archived bytes as a named section, then
//!   the rkyv-archived core `Container`, whose section table locates them;
//!   every offset is 16-aligned so `rkyv::access` validates over a
//!   page-aligned mmap (layout: `FORMAT_VERSION`'s doc). A reader that links
//!   no domain crate still decodes the core; a cell edit re-encodes the core
//!   and copies the sections back verbatim.
//! - **HashMaps converted to sorted `Vec<(K, V)>` at the serialise boundary.**
//!   Reasons: rkyv 0.8 supports `HashMap` but the cost is real; sorted Vecs
//!   align with the spec's "domain-owned indices as named byte ranges" model;
//!   binary-search lookup on the archived form is fast enough for the access
//!   patterns we have today (resolver lookups happen during build, not on
//!   the read path; on-disk read path is BFS which iterates rather than
//!   point-queries).
//! - **`.gmap` extension** — confirmed name (also matches the LLM-interface
//!   framing: `gmap binary / gmap text / gmap vectors`).
//! - **Atomic write via `.gmap.tmp` + rename** — no torn-write window.
//! - **Self-referential mmap holder** — the `MmapContainer` ties the `Mmap`
//!   and the borrow into one struct via a private constructor that bounds
//!   the lifetime. No `ouroboros`; the surface is small enough to hand-roll.
//!
//! Module layout (LC.5a — a pure move; every item keeps its
//! `glia_store::<Item>` path through the glob re-exports below, so a
//! store packet edits the module that holds its symbol, never this facade):
//! - `container` — `Container` / `Header` / `RegistryEntry`, `MAGIC`,
//!   `FORMAT_VERSION`, the section layout (`SectionEntry`, `EncodedSection`,
//!   `OwnedFile`, `encode_section`, `write_container`), `MmapContainer`,
//!   single-file cell mutation, and the `ArchivedContainer` accessors.
//! - `code_section` — the code domain's section (`CodeSection`,
//!   `encode_repo_graph` / `decode_repo_graph`, `qname_of`),
//!   `CodeNavStore` / `SymbolTableStore`, the code-domain constructors
//!   (`Header::for_code`, `Container::from_repo_graph` / ...),
//!   `write_repo_graph`.
//! - `layout` — `DEFAULT_GMAP_SUBDIR`, the sharded manifest layout, the
//!   `MergedGraph` round-trip, `upsert_cell_sharded`, `is_gmap_stale`.
//! - `error` — `StoreError`.
//! - `inspect` — `inspect_path` / `Inspection` (LC.4): a file or layout
//!   decoded from its core and header registries alone, no domain crate.
//! - `cells` — the cell write API (LF.1b): `write_cell` / `remove_cell_entry`
//!   upsert the `.glia` cell sidecars under a lock and write through into a
//!   layout only while it is fresh.

mod error;
mod container;
mod code_section;
mod layout;
mod inspect;
mod cells;

pub use error::*;
pub use container::*;
pub use code_section::*;
pub use layout::*;
pub use inspect::*;
pub use cells::*;
