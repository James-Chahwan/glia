//! `inputs` area: non-code inputs — gaps (LF.2c), history (LF.5d), tests (LF.6d),
//! overlay (CE.3e), scip (CE.1c).
//!
//! Each command is a variant of `InputsCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/inputs/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

mod gaps;
mod history;
mod overlay;
mod scip;
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
    /// Overlay loop (CE-3): `propose` lists the gaps an overlay could close,
    /// with ids and source snippets; `try` builds a candidate stanza file
    /// against the repo and reports what each stanza changes (leave-one-out)
    /// with a keep / review / drop verdict; `accept` merges chosen stanzas
    /// into .glia/overlay.toml, or removes rules by gap id, after validating
    /// the result. Only accept writes, and only that file.
    Overlay(overlay::Args),
    /// SCIP index snapshot (CE-1): decode a compiler-grade SCIP index
    /// (scip-python, scip-typescript, scip-java, scip-go, rust-analyzer scip)
    /// and write `<repo>/.glia/scip-snapshot/`, which the next build ingests
    /// as FACT-tier CALLS / USES / IMPLEMENTS / INHERITS_FROM evidence; the
    /// build itself never reads the index. Exits 1 when the index cannot be
    /// decoded (nothing written).
    Scip(scip::Args),
}

pub(crate) fn run(c: InputsCmd) -> i32 {
    match c {
        InputsCmd::Gaps(a) => gaps::run(a),
        InputsCmd::History(a) => history::run(a),
        InputsCmd::Tests(a) => tests::run(a),
        InputsCmd::Overlay(a) => overlay::run(a),
        InputsCmd::Scip(a) => scip::run(a),
    }
}
