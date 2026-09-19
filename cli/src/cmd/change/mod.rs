//! `change` area: change-driven views — delta (LE.1c), diff-impact (LE.2),
//! tests-for (LE.3b), patterns (LE.7b).
//!
//! Each command is a variant of `ChangeCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/change/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

mod delta;

#[derive(Subcommand, Debug)]
pub(crate) enum ChangeCmd {
    /// Delta (LE.1c): what the working tree's change did to the graph against
    /// a git rev (`--base`, default HEAD) — nodes added, removed, modified or
    /// moved and edges added, removed or reconfidenced, each located (a
    /// removed row in the base rev). Saves the parse-cache sidecar as an
    /// incremental build does, never a layout. Exits 0 on an answer, 2 on a
    /// git or build error.
    Delta(delta::Args),
}

pub(crate) fn run(c: ChangeCmd) -> i32 {
    match c {
        ChangeCmd::Delta(a) => delta::run(a),
    }
}
