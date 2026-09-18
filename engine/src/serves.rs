//! Who serves a channel (HTTP `METHOD /path` through the route matcher, a
//! queue topic): located servers and handlers, or a FACT-tier 'nothing serves
//! it' with near misses. Filled by LD.8b.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::serves::<item>`, never flattened into the
//! crate root.
