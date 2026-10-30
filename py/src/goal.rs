//! **goal** (FC.7, +FC.8): pyo3 surface for `glia_engine::goal` (the
//! goal-layer connector: units, key extraction, resolution, the tiered joiner
//! and its why / impact / drift / check answers). Private module slot,
//! declared by F0.4 and doc-only until FC.7 fills it with its
//! `#[pymethods] impl PyGraph` block or `#[pyfunction]`s (`register()` plus
//! `inventory::submit!`), and creates `py/api_surface/goal.txt` and
//! `py/tests/surface/test_goal.py`; neither FC.7 nor FC.8 edits
//! `py/src/lib.rs`.
//!
//! Reserved for 0.5.5 (track T6); empty in 0.5.4.
