//! `store` area: the .gmap store — inspect (LC.4), cell (LF.1c).
//!
//! Each command is a variant of `StoreCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/store/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

mod inspect;

#[derive(Subcommand, Debug)]
pub(crate) enum StoreCmd {
    /// Inspect (LC.4): what a .gmap file or a layout directory holds — per
    /// shard its graph type, format, node / edge counts and sections, then the
    /// counts by node kind, edge category and node / edge cell type, every id
    /// named from the file's own header (no domain table is consulted). Exits
    /// 1 with the reason when a file cannot be read (an old format says
    /// "rebuild the graph").
    Inspect(inspect::Args),
}

pub(crate) fn run(c: StoreCmd) -> i32 {
    match c {
        StoreCmd::Inspect(a) => inspect::run(a),
    }
}
