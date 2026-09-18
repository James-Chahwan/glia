//! Slot for LE.6b: `cycles` (cross-service event loops, service-level loops,
//! import cycles). Its `PyGraph` methods go in an attributed `impl PyGraph`
//! block here, as in `graph.rs`; module functions submit their own
//! `registry::ModuleFns` (see `lib.rs`).
