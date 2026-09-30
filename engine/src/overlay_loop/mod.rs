//! The overlay loop (CE.3b, + CE.3c, CE.3d): propose / try / accept for
//! candidate `.glia/overlay.toml` stanzas - a format-preserving TOML writer,
//! a trial that builds base, base + candidate and each stanza's marginal
//! effect on one in-memory parse cache, the gap work list for the model step,
//! and the only writer of `.glia/overlay.toml`. Directory-module slot, reached
//! by module path (`glia_engine::overlay_loop::<item>`): its owners add their
//! files (writer.rs, trial.rs, propose.rs, accept.rs) and declare them here.
//!
//! `writer` (CE.3b): a candidate is overlay text holding only overlay
//! sections and entrypoints, each stanza linked to the gaps it targets by
//! `# gap: <id>` comments ([`parse_candidate`] / [`report_candidate`], which
//! prints the `[overlay] candidate` marker); [`merge`] writes chosen stanzas
//! into the user's file keeping its comments and layout, and [`without`]
//! takes one back out. A build reads a candidate through
//! `BuildOptions::with_overlay_text`, never through the file, and such a
//! build is never persisted.

mod writer;

pub use writer::{
    Candidate, Location, Merged, StanzaRef, merge, parse_candidate, report_candidate, without,
};
