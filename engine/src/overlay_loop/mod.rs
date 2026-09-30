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
//!
//! `trial` (CE.3c): [`try_candidate`] builds the tree with the overlay as it
//! is, with the candidate merged in and, per stanza, with every stanza but
//! that one, all on one in-memory parse cache, and reports every category's
//! delta ([`GraphDelta`]), each stanza's marginal effect and the target gaps
//! it closes ([`StanzaTrial`]), and a keep / review / drop verdict
//! ([`TryReport`]). It writes nothing but the parse cache.
//!
//! `propose` (CE.3d): [`propose`] is the model step's work list - every gap
//! an overlay could close (`glia gaps` rows, ids included), each with the
//! source lines around it ([`Snippet`]) read from the one root that holds
//! its file, and a [`Proposal::guide`] to the docs/overlay.md section per
//! `suggest` value. It writes nothing.
//!
//! `accept` (CE.3d): [`accept`] is the only writer of `.glia/overlay.toml`:
//! it merges a candidate's chosen stanzas, removes orphaned / redundant rules
//! by gap id (by their identity, never a line), refuses any text the loader
//! would not load as validated, writes atomically and returns the diff
//! ([`AcceptSummary`]).

mod accept;
mod propose;
mod trial;
mod writer;

pub use accept::{AcceptOptions, AcceptSummary, accept};
pub use propose::{
    DEFAULT_SNIPPET_LINES, DEFAULT_TOP_K, Proposal, ProposeOptions, ProposedGap, Snippet, propose,
};
pub use trial::{GraphDelta, StanzaTrial, TryOptions, TryReport, try_candidate};
pub use writer::{
    Candidate, Location, Merged, StanzaRef, merge, parse_candidate, report_candidate, without,
};
