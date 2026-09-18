//! Multi-path cross-stack trace and entry flows. Filled by LD.4a, which moves
//! `cross_stack_trace` / `TraceHop` here from `answers`; extended by LD.4b.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::trace::<item>`, never flattened into the
//! crate root.
