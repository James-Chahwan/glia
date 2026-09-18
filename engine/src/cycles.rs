//! Cycles: cross-service event loops (node-level, located witness),
//! service-level possible loops, then import cycles. Filled by LE.6b.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::cycles::<item>`, never flattened into the
//! crate root.
