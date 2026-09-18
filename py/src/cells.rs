//! Slot for LF.1b: the cell write API (`set_cell` / `write_cell` /
//! `remove_cell` / `node_cell_bytes`). Its `PyGraph` methods go in an
//! attributed `impl PyGraph` block here, as in `graph.rs`; module functions
//! submit their own `registry::ModuleFns` (see `lib.rs`).
