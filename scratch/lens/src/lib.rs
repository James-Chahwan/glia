//! glia-lens — public library surface for slice 1.
//!
//! Exposes the JSONL types, lens math, and renderers so slice 2 (neuropil
//! `Lens` tab) can depend on the same canonical shapes without the binary.
//! The binary `lens` lives in `src/main.rs` and uses these modules; external
//! consumers depend on this lib via `glia-lens = { path = "scratch/lens" }`.

pub mod jsonl;
pub mod lens;
pub mod render_ascii;
pub mod render_png;
pub mod runtime;
