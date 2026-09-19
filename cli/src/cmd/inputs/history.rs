//! `glia history sync` (LF.5d) — the CLI surface of the history snapshot step,
//! `glia_snapshots::history_sync`: read the repo's local git (no fetch, no
//! remote, no author or committer identity) and write
//! `<repo>/.glia/history-snapshot/` (commits.jsonl, blame.jsonl, meta.json).
//! The next `glia build` ingests it (LF.5b): churn and blame-recency ATTN
//! cells, and CO_CHANGES edges between modules that change together.
//!
//! The build never syncs on its own: it stays offline and deterministic and
//! reads whatever snapshot is on disk, the way `glia docs sync` feeds it
//! Confluence pages (docs/overlay.md, "History snapshot"). Transport only:
//! the capture, the snapshot format and the `[history] sync ... surface=cli`
//! marker live in the snapshots crate. Exits 0 on a written snapshot, 1 when
//! the sync fails (nothing written).

use std::path::Path;

use clap::Subcommand;
use glia_snapshots::{HistoryOptions, history_sync};
use repo_graph_code_domain::snapshots::history_dir;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: HistoryCmd,
}

#[derive(Subcommand, Debug)]
enum HistoryCmd {
    /// Read the repo's git history and write `<repo>/.glia/history-snapshot/`,
    /// replacing any earlier snapshot. Then `glia build <repo>` ingests it.
    Sync {
        /// Repo whose history to read.
        repo: String,
        /// Read the newest this many non-merge commits (`git log -n`).
        #[arg(long, default_value_t = HistoryOptions::default().max_commits)]
        max_commits: usize,
        /// Only commits newer than this date, passed to `git log --since`
        /// verbatim (`2026-01-01`, `6 months ago`).
        #[arg(long)]
        since: Option<String>,
        /// Also blame the most-changed files, for per-symbol recency. Slower.
        #[arg(long)]
        blame: bool,
        /// How many files `--blame` blames, most-changed first.
        #[arg(long, default_value_t = HistoryOptions::default().blame_max_files)]
        blame_max_files: usize,
    },
}

pub(crate) fn run(args: Args) -> i32 {
    match args.action {
        HistoryCmd::Sync {
            repo,
            max_commits,
            since,
            blame,
            blame_max_files,
        } => {
            let opts = HistoryOptions {
                max_commits,
                since,
                blame,
                blame_max_files,
                surface: "cli",
            };
            sync(&repo, &opts)
        }
    }
}

fn sync(repo: &str, opts: &HistoryOptions) -> i32 {
    let root = Path::new(repo);
    match history_sync(root, opts) {
        Ok(summary) => {
            let head: String = summary.head.chars().take(12).collect();
            println!(
                "synced {} commits (head {head}) -> {}",
                summary.commits,
                history_dir(root).display()
            );
            println!("run `glia build {repo}` to ingest.");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}
