//! `store` area: the .gmap store — inspect (LC.4), cell (LF.1c), cache (CE.2c,
//! +CE.2d, CE.2e).
//!
//! Each command is a variant of `StoreCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/store/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

mod cache;
mod cell;
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
    /// Cell (LF.1c): write, remove and audit the cells the repo's `.glia`
    /// sidecars carry (`cells.jsonl`: CONSTRAINT / DECISION / CONV entries;
    /// `vectors.jsonl`: VECTORs). `set` / `rm` go through the store's write
    /// API (sidecar under its lock, plus the default layout while it is
    /// fresh); `ls --check` binds every row against a fresh build through the
    /// build's resolver and exits 1 when a row is ambiguous, orphaned or
    /// rejected; `--rekey` rewrites rows a moved node re-bound.
    Cell(cell::Args),
    /// Shared parse cache (CE-2): push the parse cache of a built checkout to
    /// an object store, pull what another checkout is missing into
    /// <repo>/.glia/graph/parse_cache.bin (the next build reuses it; the build
    /// itself never touches the network), gc a directory store. Objects are
    /// signed with GLIA_CACHE_KEY / --key-file; an unsigned store needs
    /// --unsigned and is re-parse-sampled on pull.
    Cache(cache::Args),
}

pub(crate) fn run(c: StoreCmd) -> i32 {
    match c {
        StoreCmd::Inspect(a) => inspect::run(a),
        StoreCmd::Cell(a) => cell::run(a),
        StoreCmd::Cache(a) => cache::run(a),
    }
}
