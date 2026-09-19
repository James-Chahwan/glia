//! Research synth passes as `activation::plan::SynthHook`s whose inputs are
//! task-shaped: the issue text, test patch and seeds file a research driver
//! reads and fills into the hook struct (the "issue-token grounding stays
//! driver-side" rule). The graph-shaped hooks live in [`crate::hooks`].
//!
//! * [`key_symbols`]: [`key_symbols::KeySymbolsSynth`], the body of the
//!   `synth_key_symbols` bin (LD.12d).

pub mod key_symbols;
