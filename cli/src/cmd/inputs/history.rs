//! `glia history sync` (LF.5d) and `glia history calibrate` (EC.6) — the CLI
//! surfaces of the history snapshot steps. `sync` is the history snapshot step,
//! `glia_snapshots::history_sync`: read the repo's local git (no fetch, no
//! remote, no author or committer identity) and write
//! `<repo>/.glia/history-snapshot/` (commits.jsonl, blame.jsonl, meta.json,
//! and messages.jsonl: each commit's redacted, trailer-stripped message, its
//! author time and merged-branch label, EB.1; `--no-messages` keeps none).
//! The next `glia build` ingests it (LF.5b): churn and blame-recency ATTN
//! cells, and CO_CHANGES edges between modules that change together.
//!
//! The build never syncs on its own: it stays offline and deterministic and
//! reads whatever snapshot is on disk, the way `glia docs sync` feeds it
//! Confluence pages (docs/overlay.md, "History snapshot"). Transport only:
//! the capture, the snapshot format and the `[history] sync ... surface=cli`
//! marker live in the snapshots crate. Exits 0 on a written snapshot, 1 when
//! the sync fails (nothing written).
//!
//! `calibrate` (EC.6) builds the repo's graph and runs
//! `glia_engine::calibrate::calibrate` over the snapshot `sync` wrote: a
//! leak-free online replay of the repo's own history over the fixed candidate
//! rankings, written as `<repo>/.glia/calibration-snapshot/`, the per-repo
//! ranking each ranked answer then picks (docs/overlay.md, "Calibration
//! snapshot"). A build never calibrates and calibrate never syncs. Transport
//! only: the replay, the record and the `[calibrate] done ... surface=cli`
//! marker live in the engine. Exits 0 on a written record, 1 when the step
//! fails (no snapshot: it names `glia history sync`; an unknown `--sets`
//! group: it lists the known ones), 2 when the build fails.

use std::path::Path;

use clap::Subcommand;
use glia_code_domain::snapshots::history_dir;
use glia_engine::calibrate::{
    CalibrateOptions, CalibrationSummary, DEFAULT_GROUPS, ReplayParams, calibrate, specs,
};
use glia_snapshots::{HistoryOptions, history_sync};

use crate::common::generate_for;

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
        /// Do not keep commit messages (the intent ranking of review / find --files then has no text)
        #[arg(long)]
        no_messages: bool,
    },
    /// Replay the history snapshot over the graph and write
    /// <repo>/.glia/calibration-snapshot/: the per-repo ranking each answer picks.
    Calibrate {
        /// Repo to calibrate (run `glia history sync <repo>` first).
        repo: String,
        /// The newest this many test commits the replay scores.
        #[arg(long, default_value_t = ReplayParams::default().max_commits)]
        max_commits: usize,
        /// Seed queries per test commit.
        #[arg(long, default_value_t = ReplayParams::default().max_seeds)]
        max_seeds: usize,
        /// Candidate groups to replay, comma-separated (default: every
        /// registered group).
        #[arg(long, value_delimiter = ',')]
        sets: Vec<String>,
        /// Print the summary as JSON.
        #[arg(long)]
        json: bool,
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
            no_messages,
        } => {
            let opts = HistoryOptions {
                max_commits,
                since,
                blame,
                blame_max_files,
                messages: !no_messages,
                surface: "cli",
            };
            sync(&repo, &opts)
        }
        HistoryCmd::Calibrate {
            repo,
            max_commits,
            max_seeds,
            sets,
            json,
        } => run_calibrate(&repo, max_commits, max_seeds, &sets, json),
    }
}

/// The registered group each `--sets` name names; an unknown name is the
/// engine's error listing the known ones. Empty is every registered group.
fn groups_of(sets: &[String]) -> Result<Vec<&'static str>, String> {
    if sets.is_empty() {
        return Ok(DEFAULT_GROUPS.to_vec());
    }
    sets.iter()
        .map(
            |name| match DEFAULT_GROUPS.iter().find(|g| **g == name.as_str()) {
                Some(g) => Ok(*g),
                // DEFAULT_GROUPS is every group `specs` registers, so `specs`
                // refuses this name with its list of the known ones.
                None => match specs(&[name.as_str()]) {
                    Err(e) => Err(e),
                    Ok(_) => Err(format!(
                        "unknown candidate group `{name}`: expected one of {}",
                        DEFAULT_GROUPS.join(", ")
                    )),
                },
            },
        )
        .collect()
}

fn run_calibrate(
    repo: &str,
    max_commits: usize,
    max_seeds: usize,
    sets: &[String],
    json: bool,
) -> i32 {
    let groups = match groups_of(sets) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let mut params = ReplayParams::default();
    params.max_commits = max_commits;
    params.max_seeds = max_seeds;
    let mut opts = CalibrateOptions::default();
    opts.params = params;
    opts.groups = groups;
    opts.surface = "cli";
    let built = match generate_for(repo, &[]) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    match calibrate(Path::new(repo), &built.merged, &opts) {
        Ok(summary) => {
            if json {
                match serde_json::to_string_pretty(&summary) {
                    Ok(text) => println!("{text}"),
                    Err(e) => {
                        eprintln!("error: {e}");
                        return 1;
                    }
                }
            } else {
                print_calibration(&summary);
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// One line per (set, shape) row, then the record's directory.
fn print_calibration(s: &CalibrationSummary) {
    for r in &s.sets {
        println!(
            "{} {}: pick {} over {} queries (online recall@10 {:.4} vs fallback {:.4}, delta {:+.4} [{:+.4}, {:+.4}])",
            r.set,
            r.shape,
            r.pick,
            r.queries,
            r.selection_recall10,
            r.fallback_recall10,
            r.delta_recall10,
            r.ci_low,
            r.ci_high
        );
    }
    println!("wrote {}", s.dir);
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
