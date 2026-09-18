//! `effects(A)`: the effect sinks downstream of a node (DB read / write,
//! queue produce, HTTP call, event emit and the other outbound markers), with
//! located witness paths. Filled by LE.4d.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::effects::<item>`, never flattened into the
//! crate root.
