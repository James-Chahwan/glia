//! `query` area: graph lookups — pages (LA.6e), find (LD.3b), flows (LD.4b),
//! implementors (LD.7c), serves (LD.8b), why (LE.5).
//!
//! Each command is a variant of `QueryCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/query/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

mod find;

#[derive(Subcommand, Debug)]
pub(crate) enum QueryCmd {
    /// Find (LD.3b): the ranked, located nodes a symbol, qname or fragment
    /// names. Each row says which tier matched it — exact_qname, exact_name,
    /// exact_ci, qname_suffix, name_prefix, name_word, name_substring,
    /// qname_substring, subsequence — ranked by tier, then by degree.
    Find(find::Args),
}

pub(crate) fn run(c: QueryCmd) -> i32 {
    match c {
        QueryCmd::Find(a) => find::run(a),
    }
}
