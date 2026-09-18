//! `why(edge)`: every edge between two nodes with its emitting extractor or
//! resolver, call sites and confidence, or a located witness path when there
//! is no direct edge. Filled by LE.5.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::why::<item>`, never flattened into the
//! crate root.
