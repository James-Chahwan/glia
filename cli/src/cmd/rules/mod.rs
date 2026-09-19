//! `rules` area: rule and structure checks — effects (LE.4d), cycles (LE.6b),
//! check (LE.8), spec-status (LE.9b).
//!
//! Each command is a variant of `RulesCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/rules/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

mod cycles;
mod spec_status;

#[derive(Subcommand, Debug)]
pub(crate) enum RulesCmd {
    /// Cycles (LE.6b): cross-service event loops first (a node-level loop
    /// through queue / event hops, with a located witness), then call loops,
    /// then service-level possible loops (each service publishes to the other
    /// but no handler-to-producer path is in the graph), then module import
    /// cycles. A report: exits 0 whatever it finds.
    Cycles(cycles::Args),
    /// Spec status (LE.9b): per feature, the declared API ops (OpenAPI,
    /// quokka feature.yaml) that a route implements and the ones still
    /// missing, then the routes of governed services no op declares. Pact and
    /// handler-annotation ops are not declarations. A report: exits 0
    /// whatever it finds.
    SpecStatus(spec_status::Args),
}

pub(crate) fn run(c: RulesCmd) -> i32 {
    match c {
        RulesCmd::Cycles(a) => cycles::run(a),
        RulesCmd::SpecStatus(a) => spec_status::run(a),
    }
}
