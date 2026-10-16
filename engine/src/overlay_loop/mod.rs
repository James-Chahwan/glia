//! The overlay loop (CE.3b, + CE.3c, CE.3d): propose / try / accept for
//! candidate `.glia/overlay.toml` stanzas - a format-preserving TOML writer,
//! a trial that builds base, base + candidate and each stanza's marginal
//! effect on one in-memory parse cache, the gap work list for the model step,
//! and the only writer of `.glia/overlay.toml`. Directory-module slot, reached
//! by module path (`glia_engine::overlay_loop::<item>`): its owners add their
//! files (writer.rs, trial.rs, propose.rs, accept.rs) and declare them here.
//!
//! `writer` (CE.3b, + DB.9a): a candidate is overlay text holding only
//! overlay sections, entrypoints and `[[constraint]]` rules (a `forbid_edge`,
//! a `no_cycle`, or an `invariant` with `rule`; never `allow` or a prose
//! invariant), each stanza linked to the gaps it targets by `# gap: <id>`
//! comments ([`parse_candidate`] / [`report_candidate`], which prints the
//! `[overlay] candidate` marker); [`merge`] writes chosen stanzas into the
//! user's file keeping its comments and layout (moving it to `version = 2`
//! in place only when a written rule needs it), and [`without`] takes one
//! back out. A build reads a candidate through
//! `BuildOptions::with_overlay_text`, never through the file, and such a
//! build is never persisted.
//!
//! `trial` (CE.3c, + DB.9b): [`try_candidate`] builds the tree with the
//! overlay as it is, with the candidate merged in and, per graph stanza, with
//! every stanza but that one, all on one in-memory parse cache, and reports
//! every category's delta ([`GraphDelta`]), each stanza's marginal effect and
//! the target gaps it closes ([`StanzaTrial`]), a keep / review / drop
//! verdict ([`TryReport`]) and, for the declared rules, the violations each
//! build adds or resolves ([`RulesDelta`], [`RuleHit`]), with every
//! `[[constraint]]` stanza judged on WITH ([`RuleTrial`], [`RulesReport`]).
//! It writes nothing but the parse cache.
//!
//! `propose` (CE.3d): [`propose`] is the model step's work list - every gap
//! an overlay could close (`glia gaps` rows, ids included), each with the
//! source lines around it ([`Snippet`]) read from the one root that holds
//! its file, and a [`Proposal::guide`] to the docs/overlay.md section per
//! `suggest` value. It writes nothing, and drafts no rule: a rule states
//! intent, which the graph does not hold (DB.9a); the guide says how to write
//! one.
//!
//! `accept` (CE.3d): [`accept`] is the only writer of `.glia/overlay.toml`:
//! it merges a candidate's chosen stanzas, removes orphaned / redundant /
//! rotted rules by gap id (by their identity, never a line), refuses a
//! chosen rule `glia check` could not evaluate and any text the loader would
//! not load as validated, writes atomically and returns the diff
//! ([`AcceptSummary`]).

mod accept;
mod propose;
mod trial;
mod writer;

pub use accept::{AcceptOptions, AcceptSummary, accept};
pub use propose::{
    DEFAULT_SNIPPET_LINES, DEFAULT_TOP_K, Proposal, ProposeOptions, ProposedGap, Snippet, propose,
};
pub use trial::{
    GraphDelta, MAX_RULE_HITS, RULE_DUPLICATE, RULE_ERROR, RULE_HOLDS, RULE_ORPHANED,
    RULE_VIOLATED, RuleHit, RuleTrial, RulesDelta, RulesReport, StanzaTrial, TryOptions, TryReport,
    try_candidate,
};
pub use writer::{
    Candidate, Location, Merged, StanzaRef, merge, parse_candidate, report_candidate, without,
};
