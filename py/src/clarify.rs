//! **clarify** (FC.14): pyo3 surface for `glia_engine::clarify` (the
//! clarification queue: `clarify::questions` / `clarify::answer`). Private
//! module slot, declared by F0.4 and doc-only until FC.14 fills it with its
//! `#[pymethods] impl PyGraph` block or `#[pyfunction]`s (`register()` plus
//! `inventory::submit!`), and creates `py/api_surface/clarify.txt` and
//! `py/tests/surface/test_clarify.py`; FC.14 never edits `py/src/lib.rs`.
//!
//! Reserved for 0.5.6 (track T6); empty in 0.5.4.
