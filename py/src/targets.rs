//! **targets** (FA.19): pyo3 surface for `glia_engine::targets` (build-target
//! graphs read from the `targets sync` snapshot, FA.18). Private module slot,
//! declared by F0.4 and doc-only until FA.19 fills it with its
//! `#[pyfunction]`s (`register()` plus `inventory::submit!`) or
//! `#[pymethods] impl PyGraph` block, and creates
//! `py/api_surface/targets.txt` and `py/tests/surface/test_targets.py`; FA.19
//! never edits `py/src/lib.rs`.
