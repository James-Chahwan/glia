//! repo-graph-store — `.gmap` container format: write a `RepoGraph` to disk
//! as a single rkyv-archived `Container`, mmap it back zero-copy, and traverse
//! the archived form via `NodeLike` / `EdgeLike` (see `repo-graph-core`).
//!
//! Design notes (locked 2026-04-17):
//! - **Single rkyv-archived top-level `Container` per file**, not the
//!   sectioned-byte-ranges layout from the spec. Sectioned layout becomes
//!   relevant at v0.4.5c (sharding) or v0.4.10 (multi-domain). For one
//!   single-domain repo, one container is simpler and gives us everything
//!   the zero-copy invariant promises.
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
//! `repo_graph_store::<Item>` path through the glob re-exports below, so a
//! store packet edits the module that holds its symbol, never this facade):
//! - `container` — `Container` / `Header` / `RegistryEntry`, `MAGIC`,
//!   `FORMAT_VERSION`, `MmapContainer`, single-file cell mutation, and the
//!   `ArchivedContainer` accessors.
//! - `code_section` — `CodeNavStore` / `SymbolTableStore`, the code-domain
//!   constructors (`Header::for_code`, `Container::from_repo_graph` / ...),
//!   `write_repo_graph`.
//! - `layout` — `DEFAULT_GMAP_SUBDIR`, the sharded manifest layout, the
//!   `MergedGraph` round-trip, `upsert_cell_sharded`, `is_gmap_stale`.
//! - `error` — `StoreError`.

mod error;
mod container;
mod code_section;
mod layout;

pub use error::*;
pub use container::*;
pub use code_section::*;
pub use layout::*;
