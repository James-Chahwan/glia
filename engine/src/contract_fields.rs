//! Field-level contract diff: producer vs consumer fields per schema copy,
//! topic, AsyncAPI channel and HTTP route, with verdicts by each format's own
//! compatibility rules. Filled by LE.10c.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::contract_fields::<item>`, never flattened
//! into the crate root.
