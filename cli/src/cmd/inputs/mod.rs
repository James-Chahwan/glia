//! `inputs` area: non-code inputs — gaps (LF.2c), history (LF.5d), tests (LF.6d).
//!
//! Each command is a variant of `InputsCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/inputs/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub(crate) enum InputsCmd {}

pub(crate) fn run(c: InputsCmd) -> i32 {
    match c {}
}
