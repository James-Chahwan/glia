//! `store` area: the .gmap store — inspect (LC.4), cell (LF.1c).
//!
//! Each command is a variant of `StoreCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/store/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub(crate) enum StoreCmd {}

pub(crate) fn run(c: StoreCmd) -> i32 {
    match c {}
}
