//! **embed** (FB.5c): pyo3 surface for `glia embed` (glia-embed, the
//! VectorRow snapshot under `.glia/`), reached through the py crate's `embed`
//! feature only. Private module slot, declared by F0.4 and doc-only until
//! FB.5c fills it with its `#[pyfunction]`s (`register()` plus
//! `inventory::submit!`) or `#[pymethods] impl PyGraph` block, and creates
//! `py/api_surface/embed.txt` and `py/tests/surface/test_embed.py`; FB.5c
//! never edits `py/src/lib.rs`.
