//! `glia tests ingest` (LF.6d) — the CLI surface of the test-report snapshot
//! step, `glia_snapshots::tests_ingest`: read the reports one CI run produced
//! (JUnit XML, CI logs, lcov tracefiles) and write
//! `<repo>/.glia/test-snapshot/` (cases.jsonl, lcov.jsonl, meta.json). The
//! next `glia build` ingests it (LF.6b / LF.6c: FAIL and COVERAGE cells).
//!
//! The build never ingests on its own: it stays offline and deterministic and
//! reads whatever snapshot is on disk, the way `glia history sync` feeds it
//! git history. Transport only: the parsing, the redaction, the snapshot
//! format and the `[tests] ingest ... surface=cli` marker live in the
//! snapshots crate.
//!
//! Every report read is listed on stdout and every report skipped is a
//! `warning:` line on stderr, so nothing is ingested silently. Exits 0 when at
//! least one report was ingested, 1 when none could be (nothing written), 2 on
//! a usage error (no report flag).

use std::path::{Path, PathBuf};

use clap::Subcommand;
use glia_snapshots::{TestsIngestOptions, TestsSummary, tests_ingest};
use repo_graph_code_domain::snapshots::tests_dir;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: TestsCmd,
}

#[derive(Subcommand, Debug)]
enum TestsCmd {
    /// Read one CI run's test reports and write `<repo>/.glia/test-snapshot/`,
    /// replacing any earlier snapshot. Then `glia build <repo>` ingests it.
    /// Give the repo first: each report flag takes every path after it, so a
    /// shell glob (`--junit reports/*.xml`) passes all its matches.
    #[command(
        group = clap::ArgGroup::new("reports").required(true).multiple(true),
        override_usage = "glia tests ingest <REPO> <--junit <PATH>...|--log <PATH>...|--lcov <PATH>...> [--run <LABEL>]"
    )]
    Ingest {
        /// Repo the reports belong to.
        repo: String,
        /// JUnit XML reports (pytest, surefire, jest-junit, go-junit-report, ...).
        #[arg(long, value_name = "PATH", num_args = 1.., group = "reports")]
        junit: Vec<PathBuf>,
        /// CI logs, read for their failure summary lines (pytest, go test,
        /// cargo test, jest).
        #[arg(long, value_name = "PATH", num_args = 1.., group = "reports")]
        log: Vec<PathBuf>,
        /// lcov tracefiles, read for per-line hit counts.
        #[arg(long, value_name = "PATH", num_args = 1.., group = "reports")]
        lcov: Vec<PathBuf>,
        /// A label for the run (a CI run id), stored verbatim in the meta.
        #[arg(long, value_name = "LABEL")]
        run: Option<String>,
    },
}

pub(crate) fn run(args: Args) -> i32 {
    match args.action {
        TestsCmd::Ingest {
            repo,
            junit,
            log,
            lcov,
            run,
        } => {
            let opts = TestsIngestOptions {
                junit,
                logs: log,
                lcov,
                run,
                surface: "cli",
            };
            ingest(&repo, &opts)
        }
    }
}

fn ingest(repo: &str, opts: &TestsIngestOptions) -> i32 {
    let root = Path::new(repo);
    match tests_ingest(root, opts) {
        Ok(summary) => {
            for line in warning_lines(&summary) {
                eprintln!("{line}");
            }
            for line in report_lines(&summary, root, repo) {
                println!("{line}");
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// One `warning:` line per report the ingest skipped (unreadable, too big,
/// malformed); the lib has already printed its `[tests] skip` marker for each.
fn warning_lines(s: &TestsSummary) -> Vec<String> {
    s.report_errors
        .iter()
        .map(|e| format!("warning: skipped {}: {}", e.report, e.reason))
        .collect()
}

/// The stdout of a successful ingest: `read <report>` per report read, then
/// what was stored and where, then how to use it.
fn report_lines(s: &TestsSummary, root: &Path, repo: &str) -> Vec<String> {
    let mut lines: Vec<String> = s.reports.iter().map(|r| format!("read {r}")).collect();
    lines.push(format!(
        "ingested {} failing case(s) and {} lcov file(s) -> {}",
        s.stored,
        s.lcov_files,
        tests_dir(root).display()
    ));
    lines.push(format!("run `glia build {repo}` to ingest."));
    lines
}

#[cfg(test)]
mod unit {
    use clap::FromArgMatches;
    use glia_snapshots::ReportError;

    use super::*;

    fn parse(argv: &[&str]) -> Result<Args, clap::Error> {
        let cmd = <Args as clap::Args>::augment_args(clap::Command::new("tests").no_binary_name(true));
        let matches = cmd.try_get_matches_from(argv)?;
        Args::from_arg_matches(&matches)
    }

    #[test]
    fn a_report_flag_is_required_and_each_takes_several_paths() {
        let err = parse(&["ingest", "."]).expect_err("no report flag");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
        assert!(parse(&["ingest", ".", "--run", "ci-1"]).is_err(), "--run alone is not a report");

        let args = parse(&["ingest", "repo", "--junit", "a.xml", "b.xml", "--lcov", "c.lcov", "--junit", "d.xml"])
            .expect("parses");
        let TestsCmd::Ingest { repo, junit, log, lcov, run } = args.action;
        assert_eq!(repo, "repo");
        assert_eq!(junit, [PathBuf::from("a.xml"), PathBuf::from("b.xml"), PathBuf::from("d.xml")]);
        assert!(log.is_empty());
        assert_eq!(lcov, [PathBuf::from("c.lcov")]);
        assert_eq!(run, None);

        let args = parse(&["ingest", "repo", "--log", "ci.log", "--run", "ci-42"]).expect("parses");
        let TestsCmd::Ingest { log, run, .. } = args.action;
        assert_eq!((log, run.as_deref()), (vec![PathBuf::from("ci.log")], Some("ci-42")));
    }

    #[test]
    fn output_lists_every_report_read_and_skipped() {
        let summary = TestsSummary {
            reports: vec!["a.xml".into(), "cov.lcov".into()],
            junit_files: 1,
            lcov_files: 1,
            stored: 2,
            report_errors: vec![ReportError { report: "bad.xml".into(), reason: "malformed XML".into() }],
            ..TestsSummary::default()
        };
        assert_eq!(warning_lines(&summary), ["warning: skipped bad.xml: malformed XML"]);
        let root = Path::new("/r");
        assert_eq!(
            report_lines(&summary, root, "/r"),
            [
                "read a.xml".to_string(),
                "read cov.lcov".to_string(),
                format!("ingested 2 failing case(s) and 1 lcov file(s) -> {}", tests_dir(root).display()),
                "run `glia build /r` to ingest.".to_string(),
            ]
        );
    }
}
