//! Merge pre-built layouts (`merge_layouts` and the workspace manifest),
//! recomputing resolvers and passes over the union. Filled by LC.10b.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::merge::<item>`, never flattened into the
//! crate root.
