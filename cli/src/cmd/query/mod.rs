//! `query` area: graph lookups — pages (LA.6e), find (LD.3b), flows (LD.4b),
//! implementors (LD.7c), serves (LD.8b), why (LE.5), pack (CC.4c), hotspots
//! (CC.10b), communities (CD.1e), splits (CD.2d), hubs (CD.4c),
//! duplicate-flows (CD.4f).
//!
//! Each command is a variant of `QueryCmd` (flattened into `Cmd`, so it lists
//! at top level in `glia --help`) plus one arm in `run`, with its `Args`
//! and `run` in its own `cmd/query/<name>.rs`. The variant carries the doc
//! comment (clap's `about`); the `Args` struct carries none.

use clap::Subcommand;

mod communities;
mod duplicate_flows;
mod find;
mod flows;
mod hotspots;
mod hubs;
mod implementors;
mod pack;
mod pages;
mod serves;
mod splits;
mod why;

#[derive(Subcommand, Debug)]
pub(crate) enum QueryCmd {
    /// Find (LD.3b): the ranked, located nodes a symbol, qname or fragment
    /// names. Each row says which tier matched it — exact_qname, exact_name,
    /// exact_ci, qname_suffix, name_prefix, name_word, name_substring,
    /// qname_substring, subsequence — ranked by tier, then by degree.
    Find(find::Args),
    /// Flows (LD.4b): every entry point's forward flow over the carry edges
    /// (calls, HTTP, queues, ...; never DEFINES / CONTAINS), one row each:
    /// its key (the feature word `glia trace` resolves to it), how many nodes
    /// it reaches, whether it crosses a service, the mechanisms it uses and
    /// where the entry is. Rows sharing a key are all kept. Exits 0.
    Flows(flows::Args),
    /// Hotspots (CC.10b): modules and symbols that change often AND sit where
    /// much depends on them — each row's git churn, its churn rank and its
    /// PageRank centrality rank over the same ranked population, its last
    /// change and where it is; no composite score. Module churn is commits,
    /// symbol churn is blame span changes, both read from the history
    /// snapshot (`glia history sync [--blame]`). Tier heuristic. Exits 0 with
    /// rows, 1 with none (the absence says why), 2 on an error.
    Hotspots(hotspots::Args),
    /// Implementors (LD.7c): who implements or extends a type, or overrides a
    /// method, over IMPLEMENTS / INHERITS_FROM — the whole hierarchy unless
    /// `--direct`, the supertypes with `--up`. Each row is located, names the
    /// edge that entered it and its tier: FACT (declared), DERIVED (inferred,
    /// e.g. Go method sets) or HEURISTIC, the weakest edge on its path.
    /// Nothing found is a FACT with the language's heritage caveats: exits 0.
    Implementors(implementors::Args),
    /// Pages (LA.6e): the frontend's client-router pages with their handlers,
    /// the navigation links between them, dead deep links (a router link no
    /// route serves, with the catch-all that absorbs it) and pages no in-repo
    /// link reaches. A report: exits 0 whatever it finds.
    Pages(pages::Args),
    /// Serves (LD.8b): who serves a channel — an HTTP `METHOD /path` (a bare
    /// `/path` means every verb) through the HTTP resolver's route matcher, or
    /// a queue topic. Each server is located, names the matcher tier that
    /// reached it and its handlers; nothing serving it is a FACT with the
    /// mechanism's caveats and near misses. An answer either way: exits 0.
    Serves(serves::Args),
    /// Why (LE.5): every edge from one node to another, each with the
    /// extractor or resolver that emitted it, its rule, call site and
    /// confidence, tiered fact (read at a site), derived (paired by a resolver
    /// or pass) or heuristic (a name-only guess, an overlay or git history).
    /// With no direct edge: a shortest carry path as a witness, and the
    /// absence. Exits 0 found, 1 not found, 2 on an error.
    Why(why::Args),
}

pub(crate) fn run(c: QueryCmd) -> i32 {
    match c {
        QueryCmd::Find(a) => find::run(a),
        QueryCmd::Flows(a) => flows::run(a),
        QueryCmd::Hotspots(a) => hotspots::run(a),
        QueryCmd::Implementors(a) => implementors::run(a),
        QueryCmd::Pages(a) => pages::run(a),
        QueryCmd::Serves(a) => serves::run(a),
        QueryCmd::Why(a) => why::run(a),
    }
}
