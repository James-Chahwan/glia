//! `rules` area: rule and structure checks — effects (LE.4d), cycles (LE.6b),
//! check (LE.8), spec-status (LE.9b).
//!
//! Each command is a variant of `RulesCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/rules/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

mod check;
mod cycles;
mod effects;
mod spec_status;

#[derive(Subcommand, Debug)]
pub(crate) enum RulesCmd {
    /// Check (LE.8): evaluate the declared `[[constraint]]` rules of
    /// `.glia/overlay.toml` (and cell-API rules) against the graph.
    /// `forbid_edge` lists every direct edge from scope `from` to scope `to`
    /// (tier fact); `no_cycle` lists each cycle among the modules (or, with
    /// `categories`, the nodes) in its scope (tier derived); `invariant`
    /// rules are listed as unchecked. A node is in a scope only when its
    /// file sits under that path (or it is a PROJECT there): a node with no
    /// file never matches. Scopes are repo-relative, so with --with a rule
    /// is evaluated against every merged repo's nodes at that path.
    /// Exit codes for CI: 0 no violations, 1 violations, 2 a build error or
    /// a rule that could not be evaluated (unknown edge category, a scope no
    /// node sits in).
    Check(check::Args),
    /// Cycles (LE.6b): cross-service event loops first (a node-level loop
    /// through queue / event hops, with a located witness), then call loops,
    /// then service-level possible loops (each service publishes to the other
    /// but no handler-to-producer path is in the graph), then module import
    /// cycles. A report: exits 0 whatever it finds.
    Cycles(cycles::Args),
    /// Effects (LE.4d): what the named nodes do to the outside world - the
    /// effect sinks downstream of them (DB read / write with the SQL verb,
    /// queue produce, outbound HTTP / RPC / WS / GraphQL call, event emit),
    /// each with its witness path from the seed and the receivers one flow
    /// hop past it. A config key seeds from the functions reading it. With
    /// --cross-service the walk continues past each send into the receiving
    /// handler. A report: exits 0 whatever it finds.
    Effects(effects::Args),
    /// Spec status (LE.9b): per feature, the declared API ops (OpenAPI,
    /// quokka feature.yaml) that a route implements and the ones still
    /// missing, then the routes of governed services no op declares. Pact and
    /// handler-annotation ops are not declarations. A report: exits 0
    /// whatever it finds.
    SpecStatus(spec_status::Args),
}

pub(crate) fn run(c: RulesCmd) -> i32 {
    match c {
        RulesCmd::Check(a) => check::run(a),
        RulesCmd::Cycles(a) => cycles::run(a),
        RulesCmd::Effects(a) => effects::run(a),
        RulesCmd::SpecStatus(a) => spec_status::run(a),
    }
}
