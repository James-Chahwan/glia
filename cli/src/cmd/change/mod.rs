//! `change` area: change-driven views — delta (LE.1c), diff-impact (LE.2),
//! tests-for (LE.3b), patterns (LE.7b).
//!
//! Each command is a variant of `ChangeCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/change/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

mod delta;
mod diff_impact;
mod patterns;
mod tests_for;

#[derive(Subcommand, Debug)]
pub(crate) enum ChangeCmd {
    /// Delta (LE.1c): what the working tree's change did to the graph against
    /// a git rev (`--base`, default HEAD) — nodes added, removed, modified or
    /// moved and edges added, removed or reconfidenced, each located (a
    /// removed row in the base rev). Saves the parse-cache sidecar as an
    /// incremental build does, never a layout. Exits 0 on an answer, 2 on a
    /// git or build error.
    Delta(delta::Args),
    /// Diff-impact (LE.2): what a change affects, in one call — the changed
    /// nodes of the working tree's change against a git rev (`--base`,
    /// default HEAD) or of a pasted diff (`--diff`), and ONE ranked, located
    /// blast radius around them, each row naming the changed node whose wave
    /// reached it. Rev mode also seeds the callers that lost a call and a
    /// hunk that only deletes lines; pasted mode names the diff files it
    /// could not place. Exits 0 on an answer, 2 on a usage, git or build
    /// error.
    DiffImpact(diff_impact::Args),
    /// Tests-for (LE.3b): the tests to run for a change — the test cases that
    /// reach the changed nodes backward over calls, TESTS edges and the
    /// cross-service links (an integration test's HTTP call to the changed
    /// handler's route), each located and tiered fact (a TESTS edge straight
    /// to the seed), derived (reached through other edges) or heuristic (a
    /// test module paired by name with a seed's module). Seeds: qnames, a
    /// diff (`--diff`), or the working tree's change against a git rev
    /// (`--base`). `--files-only` prints the test files for a runner. Exits
    /// 0 on an answer, 2 on a usage, git or build error.
    TestsFor(tests_for::Args),
    /// Patterns (LE.7b, EXPERIMENTAL; refuses to run without
    /// `--experimental`): pattern conformance — the route handlers of each
    /// service grouped as a population, each handler's role chain to its
    /// first effect sink as a signature (`handler>service>repository>db`),
    /// the most frequent one declared the population's convention at
    /// `--min-share` percent of at least `--min-support` handlers, and every
    /// handler off it a located DIVERGENCE (an observation, never a rule).
    /// `--base <rev>` lists only the divergences the working tree's change
    /// touched. Exits 0 on an answer, 2 without `--experimental`, on a usage
    /// error, or a git or build error.
    Patterns(patterns::Args),
}

pub(crate) fn run(c: ChangeCmd) -> i32 {
    match c {
        ChangeCmd::Delta(a) => delta::run(a),
        ChangeCmd::DiffImpact(a) => diff_impact::run(a),
        ChangeCmd::TestsFor(a) => tests_for::run(a),
        ChangeCmd::Patterns(a) => patterns::run(a),
    }
}
