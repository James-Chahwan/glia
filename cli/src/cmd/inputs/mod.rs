//! `inputs` area: non-code inputs — gaps (LF.2c), history (LF.5d), tests (LF.6d).
//!
//! Each command is a variant of `InputsCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/inputs/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

mod gaps;
mod history;
mod tests;

#[derive(Subcommand, Debug)]
pub(crate) enum InputsCmd {
    /// Blind-spot report (LF.2c): unpaired / ambiguous / unresolved
    /// endpoints, uncalled routes, tag-only queue nodes, dead symbols, and the
    /// overlay's own rot (orphaned / redundant stanzas, orphaned cell rows),
    /// each with the overlay section that could repair it. `--overlay-delta`
    /// measures an overlay edit instead (two builds). A report: exits 0.
    Gaps(gaps::Args),
    /// Git history snapshot (LF.5d): `history sync` reads the repo's local git
    /// and writes `<repo>/.glia/history-snapshot/`, which the next build
    /// ingests (churn ATTN, CO_CHANGES); the build itself never syncs. Exits
    /// 1 when the sync fails.
    History(history::Args),
    /// Test-report snapshot (LF.6d): `tests ingest` reads one CI run's JUnit
    /// XML, CI logs and lcov tracefiles and writes
    /// `<repo>/.glia/test-snapshot/`, which the next build ingests (FAIL and
    /// COVERAGE cells); the build itself never ingests. A malformed report is
    /// a `warning:` and is skipped; exits 1 only when no report could be read.
    Tests(tests::Args),
}

pub(crate) fn run(c: InputsCmd) -> i32 {
    match c {
        InputsCmd::Gaps(a) => gaps::run(a),
        InputsCmd::History(a) => history::run(a),
        InputsCmd::Tests(a) => tests::run(a),
    }
}
