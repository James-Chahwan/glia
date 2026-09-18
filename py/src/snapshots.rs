//! Slot for LF.5d (`history_sync`) and LF.6d (`tests_ingest`): the snapshot
//! steps. Its `PyGraph` methods go in an attributed `impl PyGraph` block here,
//! as in `graph.rs`; module functions submit their own `registry::ModuleFns`
//! (see `lib.rs`).
