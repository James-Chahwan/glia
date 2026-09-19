//! `inputs` area: non-code inputs — gaps (LF.2c), history (LF.5d), tests (LF.6d).
//!
//! Each command is a variant of `InputsCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/inputs/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

mod gaps;

#[derive(Subcommand, Debug)]
pub(crate) enum InputsCmd {
    /// Blind-spot report (LF.2c): unpaired / ambiguous / unresolved
    /// endpoints, uncalled routes, tag-only queue nodes, dead symbols, and the
    /// overlay's own rot (orphaned / redundant stanzas, orphaned cell rows),
    /// each with the overlay section that could repair it. `--overlay-delta`
    /// measures an overlay edit instead (two builds). A report: exits 0.
    Gaps(gaps::Args),
}

pub(crate) fn run(c: InputsCmd) -> i32 {
    match c {
        InputsCmd::Gaps(a) => gaps::run(a),
    }
}
