//! glia CLI — `glia <subcommand> [args]`.
//!
//! Layout: this file holds the global `Cli`, the `Cmd` enum and the dispatch.
//! Each command's arguments (`Args`) and body (`run`) live in
//! `cmd/<command>.rs`; `install-hooks` lives in `hooks.rs`; helpers shared by
//! several commands live in `common.rs`. A `Cmd` variant keeps its doc comment
//! — clap uses it as the subcommand's `about` — so help text is set here.
//!
//! Commands the 0.5.0 leap adds go in an area enum (`cmd::query`,
//! `cmd::change`, `cmd::rules`, `cmd::store`, `cmd::inputs`) or
//! `hooks::HooksCmd`, each flattened into `Cmd`: they list after
//! `install-hooks` in `glia --help` and never touch this file.

mod cmd;
mod common;
mod hooks;
#[cfg(test)]
mod surface;

use clap::{Parser, Subcommand};

/// `glia --version` body: `<release> (build <release>+p<parser stamp>)`. The
/// build half is a content hash of every graph-shaping source, so two binaries
/// that print the same release but different builds contain different parsers.
const VERSION: &str = repo_graph_engine::VERSION_LINE;

#[derive(Parser, Debug)]
#[command(
    name = "glia",
    version = VERSION,
    about = "glia — cross-service code-graph engine"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Walk a repo and print a summary of node-kinds + cross-graph edges.
    Analyze(cmd::analyze::Args),
    /// Architecture summary (A9.2): the services in this repo (or across the
    /// merged repos) and the cross-service links between them, each labelled
    /// with its mechanism (http/queue/grpc/ws/event/cli/graphql) and the
    /// channel it travels over.
    Arch(cmd::arch::Args),
    /// Reachability walk: which entities does <qname> depend on / get hit by.
    Impact(cmd::impact::Args),
    /// Blast radius (P3): the complete, edge-category-aware, PPR-ranked, located
    /// closure around <qname> — what it affects / what affects it, across service
    /// boundaries, in one call. Excludes structural import/contain edges so the
    /// radius doesn't fan out through shared containers.
    BlastRadius(cmd::blast_radius::Args),
    /// Docs-for (tier-4 P3): the doc sections that DOCUMENTS <qname> — "what are
    /// the rules for X?" — located.
    DocsFor(cmd::docs_for::Args),
    /// Coverage (P2): for the languages present in the repo, the known
    /// extraction caveats + edges-found per flagged category, so you fall back
    /// to grep deliberately where glia is known-partial.
    Coverage(cmd::coverage::Args),
    /// Projects (A8.6): the manifest-rooted sub-projects in the repo — label,
    /// ecosystem, path, manifest. The vocabulary for `--scope`: pass a label
    /// (`@shop/web`) or a path (`apps/web`) and get the same answer.
    Projects(cmd::projects::Args),
    /// Message contracts (A12): for every queue topic, the producer's and
    /// consumer's declared message type and whether they agree. Report-only —
    /// no SHARES_SCHEMA edge is emitted.
    Contracts(cmd::contracts::Args),
    /// Cross-stack trace (P3): follow <feature> forward across service
    /// boundaries and print the ordered path, each hop labeled with its
    /// mechanism (http/queue/grpc/call) and whether it crossed a service.
    Trace(cmd::trace::Args),
    /// Resolve (P3): a failure/change signal (stacktrace, diff, test id) → the
    /// ranked, located nodes it points at, in one call.
    Resolve(cmd::resolve::Args),
    /// Merge N repos into one MergedGraph; cross-resolvers fire across repo
    /// boundaries. Emit summary + cross-edge counts + (optionally) JSON.
    Merge(cmd::merge::Args),
    /// Walk a repo and write one `.gmap` per per-language sub-graph to
    /// `<repo>/.glia/` (or a custom dir). Idempotent + atomic.
    Build(cmd::build::Args),
    /// Sync external docs (Confluence) to/from a repo's doc snapshot. This is
    /// the **network** step, deliberately separate from `build` so the
    /// byte-identical build stays deterministic: `sync` fetches into
    /// `<repo>/.glia/docs-snapshot/`, then `build` ingests that snapshot.
    Docs(cmd::docs::Args),
    /// Install git hooks (`post-commit`, `post-merge`, `post-checkout`) into
    /// the directory git reads hooks from (core.hooksPath / the repo's common
    /// dir) so the `.gmap` rebuilds automatically on each change. Opt-in only —
    /// rebuild latency on big repos can be noticeable. `--pair <sibling>` adds
    /// the cross-repo branch-pair lock (`pre-commit` + `commit-msg`, G8 / u151).
    InstallHooks(hooks::InstallArgs),
    #[command(flatten)]
    Query(cmd::query::QueryCmd),
    #[command(flatten)]
    Change(cmd::change::ChangeCmd),
    #[command(flatten)]
    Rules(cmd::rules::RulesCmd),
    #[command(flatten)]
    Store(cmd::store::StoreCmd),
    #[command(flatten)]
    Inputs(cmd::inputs::InputsCmd),
    #[command(flatten)]
    Hooks(hooks::HooksCmd),
}

fn main() {
    let cli = Cli::parse();
    let exit = match cli.cmd {
        Cmd::Analyze(a) => cmd::analyze::run(a),
        Cmd::Arch(a) => cmd::arch::run(a),
        Cmd::Impact(a) => cmd::impact::run(a),
        Cmd::BlastRadius(a) => cmd::blast_radius::run(a),
        Cmd::DocsFor(a) => cmd::docs_for::run(a),
        Cmd::Coverage(a) => cmd::coverage::run(a),
        Cmd::Projects(a) => cmd::projects::run(a),
        Cmd::Contracts(a) => cmd::contracts::run(a),
        Cmd::Trace(a) => cmd::trace::run(a),
        Cmd::Resolve(a) => cmd::resolve::run(a),
        Cmd::Merge(a) => cmd::merge::run(a),
        Cmd::Build(a) => cmd::build::run(a),
        Cmd::Docs(a) => cmd::docs::run(a),
        Cmd::InstallHooks(a) => hooks::run(a),
        Cmd::Query(c) => cmd::query::run(c),
        Cmd::Change(c) => cmd::change::run(c),
        Cmd::Rules(c) => cmd::rules::run(c),
        Cmd::Store(c) => cmd::store::run(c),
        Cmd::Inputs(c) => cmd::inputs::run(c),
        Cmd::Hooks(c) => hooks::dispatch(c),
    };
    std::process::exit(exit);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A1.6: `glia --version` must name the engine it was built from. Before
    /// the fix cli/Cargo.toml pinned 0.4.13 against a 0.4.18 workspace, so the
    /// release half lied (audit-2026-06-10 #4's bug class); the build half did
    /// not exist, so a stale binary was indistinguishable from a fresh one.
    #[test]
    fn version_names_the_workspace_release_and_the_build_stamp() {
        use clap::CommandFactory;
        assert_eq!(
            env!("CARGO_PKG_VERSION"),
            repo_graph_engine::RELEASE,
            "glia-cli's version drifted from the workspace release"
        );
        let rendered = Cli::command().render_version();
        let line = rendered.trim_end();
        let want_prefix = format!(
            "glia {rel} (build {rel}+p",
            rel = repo_graph_engine::RELEASE
        );
        assert!(line.starts_with(&want_prefix), "--version printed {line:?}");
        let hex = line
            .strip_prefix(&want_prefix)
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap_or_default();
        assert_eq!(hex, repo_graph_engine::PARSER_STAMP, "--version printed {line:?}");
        assert_eq!(hex.len(), 16, "--version printed {line:?}");
        assert!(
            hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "--version printed {line:?}"
        );
    }
}
