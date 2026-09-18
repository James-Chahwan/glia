//! The external-input stage: everything that enters the graph from outside
//! the source (cell sidecars, overlay edges and wrappers, declared knowledge,
//! entrypoints, git history, test reports). Filled by LF.1a; LF.2b, LF.2e,
//! LF.3b, LF.4a, LF.5b and LF.6b add their stage files (cells.rs, overlay.rs,
//! declared.rs, entrypoints.rs, history.rs, test_reports.rs, wrappers.rs) and
//! declare them here as they need them.
//!
//! Directory-module slot declared by L0.2 so its owners edit only this
//! directory. Crate-private: cross-module items are `pub(crate)`.
