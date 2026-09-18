//! Ranked fuzzy find: tiered, explainable matching over name / qname with a
//! degree tie-break, located. Filled by LD.3b; extended by LD.6 and LD.8a.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::find::<item>`, never flattened into the
//! crate root.
