//! The one persist / load module shared by py and cli: `persist`, `load` and
//! `load_or_rebuild` over one on-disk layout. Filled by LC.7; extended by
//! LC.8, LC.9 and LC.10a.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::persist::<item>`, never flattened into the
//! crate root.
