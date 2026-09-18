//! Absence answers: the `Answer<T>` envelope whose empty result carries a
//! FACT-tier not-found (`Absence`) plus the coverage caveats of the
//! mechanisms it depended on. Filled by LD.8a.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `repo_graph_engine::absence::<item>`, never flattened into the
//! crate root.
