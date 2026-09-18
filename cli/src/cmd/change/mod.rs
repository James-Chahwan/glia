//! `change` area: change-driven views — delta (LE.1c), diff-impact (LE.2),
//! tests-for (LE.3b), patterns (LE.7b).
//!
//! Each command is a variant of `ChangeCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/change/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub(crate) enum ChangeCmd {}

pub(crate) fn run(c: ChangeCmd) -> i32 {
    match c {}
}
