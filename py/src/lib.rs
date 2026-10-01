//! pyo3 bindings for `glia-engine`, published to PyPI as `glia-py` and imported
//! as `glia_py` (the 0.4.x releases used the old repo-graph names; LD.11b).
//! The `#[pymodule]` fn name below, `[lib] name` in Cargo.toml and
//! `[tool.maturin] module-name` in pyproject.toml must agree: pyo3 exports
//! `PyInit_<fn name>` and Python looks up `PyInit_<module name>`.
//!
//! The orchestration logic (file walking, per-language parsing, cross-cutting
//! extraction, resolver execution, post-passes) lives in the `engine` crate
//! and is shared with the `glia` CLI. This crate is intentionally thin — only
//! the Python-facing surface lives here.
//!
//! **Layout: one module per primitive.** The `PyGraph` class is declared once,
//! in `graph.rs`; every other module adds its methods in its own
//! `#[pymethods] impl PyGraph` block (pyo3 `multiple-pymethods`). A module
//! that owns `#[pyfunction]`s ends with a `register` fn and
//! `inventory::submit! { registry::ModuleFns { .. } }`; the `#[pymodule]` below
//! runs every submission sorted by module name. Adding a primitive therefore
//! never edits this file: answers go in their primitive's module; helpers
//! that tests exercise stay pyo3-free.
//!
//! **Why tests only reach pyo3-free helpers.** The unit-test harness never
//! initialises a Python interpreter, so the whole body of a binding lives in
//! a helper that touches no pyo3 type (`arch::service_map_json`,
//! `contracts::contracts_json`) — the way `cargo test -p glia-py`
//! covers this binding at all.
//!
//! **Link note.** `extension-module` is deliberately NOT a Cargo feature of
//! this crate. `multiple-pymethods` and `ModuleFns` register through
//! linker-section constructors, which keep every pyo3 trampoline alive in the
//! unit-test executable; with `extension-module` libpython is not linked and
//! that executable fails to link (`undefined symbol: PyLong_FromLong` …).
//! Wheel builds still get it: `pyproject.toml` sets
//! `[tool.maturin] features = ["pyo3/extension-module"]`.

use pyo3::prelude::*;

mod arch;
mod blast;
mod build;
mod cells;
mod check;
mod cochange;
mod communities;
mod contract_breaks;
mod contracts;
mod convert;
mod cycles;
mod delta;
mod diff_impact;
mod docs;
mod duplicate_flows;
mod effects;
mod feature_flows;
mod find;
mod flags;
mod gaps;
mod graph;
mod hotspots;
mod hubs;
mod implementors;
mod layout;
mod merge;
mod overlay_loop;
mod pack;
mod pages;
mod patterns;
mod records;
mod registry;
mod review;
mod serves;
mod snapshots;
mod spec_status;
mod splits;
mod tests_for;
mod text;
mod timeline;
mod trace;
mod traversal;
mod why;

#[pymodule]
fn glia_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let mut fns: Vec<&registry::ModuleFns> =
        inventory::iter::<registry::ModuleFns>.into_iter().collect();
    fns.sort_by_key(|f| f.name);
    for f in fns {
        (f.add)(m)?;
    }
    m.add_class::<graph::PyGraph>()?;
    Ok(())
}
