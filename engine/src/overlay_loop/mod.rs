//! The overlay loop (CE.3b, + CE.3c, CE.3d): propose / try / accept for
//! candidate `.glia/overlay.toml` stanzas - a format-preserving TOML writer,
//! a trial that builds base, base + candidate and each stanza's marginal
//! effect on one in-memory parse cache, the gap work list for the model step,
//! and the only writer of `.glia/overlay.toml`. Directory-module slot, reached
//! by module path (`glia_engine::overlay_loop::<item>`): its owners add their
//! files (writer.rs, trial.rs, propose.rs, accept.rs) and declare them here.
//! Filled by CE.3b.
