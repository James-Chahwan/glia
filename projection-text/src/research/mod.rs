//! Research synth passes as `activation::plan::SynthHook`s whose inputs are
//! task-shaped: the issue text, test patch and seeds file a research driver
//! reads and fills into the hook struct (the "issue-token grounding stays
//! driver-side" rule). The graph-shaped hooks live in [`crate::hooks`].
//!
//! * [`key_symbols`]: [`key_symbols::KeySymbolsSynth`], the body of the
//!   `synth_key_symbols` bin (LD.12d).
//! * [`derived_notes`]: [`derived_notes::DerivedNotesSynth`], the body of the
//!   `synth_derived_notes` bin (LD.12e): the `## Derived notes` block from
//!   the key-symbols cells and the summaries on the view.
//!
//! The `synth_plan` bin runs the whole chain as one plan over one graph:
//! access path, call-site arg-flow, key symbols, derived notes.

pub mod derived_notes;
pub mod key_symbols;
