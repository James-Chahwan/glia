//! glia-snapshots — external-input snapshot writers.
//!
//! History and test reports are external, run-dependent inputs, so they enter
//! glia the way Confluence docs do: a separate snapshot step writes files
//! under `<repo>/.glia/`, and the deterministic build reads them back through
//! `glia_code_domain::snapshots`. The build never runs git; this crate
//! is the only glia code that spawns a process, and only when a sync is
//! called.
//!
//! [`history_sync`] reads the local git (no fetch, no remote, no other repo)
//! and writes `<repo>/.glia/history-snapshot/`. Author and committer names and
//! emails are never captured.
//!
//! [`tests_ingest`] reads test reports a CI run produced — JUnit XML, CI logs,
//! lcov — and writes `<repo>/.glia/test-snapshot/`. It spawns nothing and
//! reads only the files it is given.

mod ci_log;
mod git_history;
mod junit;
mod lcov;

use std::io::Write as _;
use std::path::{Path, PathBuf};

use glia_code_domain::snapshots::{TestCaseRecord, TestsMeta, write_history, write_tests};
use glia_code_domain::walk_gating::CONTROL_DIR;
use serde::Serialize;

pub use ci_log::parse_ci_log;
pub use git_history::{HistoryOptions, HistorySummary};
pub use junit::{JunitCounts, parse_junit};
pub use lcov::parse_lcov;

/// Largest report file read: CI XML and logs can be huge, and a report past
/// this is skipped with an error rather than read into memory.
pub(crate) const MAX_REPORT_BYTES: usize = 50 << 20;

/// `.glia/.gitignore`, created when absent: the control dir's local,
/// regenerable inputs. The graph layout dir ignores itself (its own `*`), and
/// the checked-in inputs (`overlay.toml`, `cells.jsonl`) are not listed.
const GLIA_GITIGNORE: &str = "\
# Local, regenerable glia inputs (written by glia; safe to delete).
vectors.jsonl
docs-snapshot/
history-snapshot/
test-snapshot/
";

/// Read the git history of the repo at `repo_root` and write it to
/// `<repo_root>/.glia/history-snapshot/` (commits.jsonl, blame.jsonl, then
/// meta.json). Creates `.glia/.gitignore` when absent, never overwriting one.
///
/// Capture errors (not a directory, not in a git work tree, no commits, git
/// missing or failing) come back as a plain message before anything is
/// written. On success prints the `[history] sync` marker on stderr.
pub fn history_sync(repo_root: &Path, opts: &HistoryOptions) -> Result<HistorySummary, String> {
    let capture = git_history::capture(repo_root, opts)?;
    let summary = git_history::summarize(&capture);
    write_history(repo_root, capture.meta, &capture.commits, &capture.blame)?;
    ensure_glia_gitignore(repo_root)?;
    eprintln!("{}", sync_marker(&repo_label(repo_root), &summary, opts));
    Ok(summary)
}

/// The repo's project name (manifest label, else directory name).
fn repo_label(root: &Path) -> String {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    glia_code_domain::project_roots::project_name(&canonical)
        .unwrap_or_else(|| canonical.display().to_string())
}

/// `[history] sync repo=<label> head=<12> commits=N files=N renames=N binary=N
/// blame_files=N runs=N window=max:N[,since:<since>] surface=<surface>`.
fn sync_marker(label: &str, s: &HistorySummary, opts: &HistoryOptions) -> String {
    let head: String = s.head.chars().take(12).collect();
    let mut window = format!("max:{}", opts.max_commits);
    if let Some(since) = &opts.since {
        let since: String = since.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect();
        window.push_str(&format!(",since:{since}"));
    }
    format!(
        "[history] sync repo={label} head={head} commits={} files={} renames={} binary={} blame_files={} runs={} window={window} surface={}",
        s.commits, s.files, s.renames, s.binary, s.blame_files, s.runs, opts.surface
    )
}

/// Options for [`tests_ingest`]: the reports one CI run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestsIngestOptions {
    /// JUnit XML reports.
    pub junit: Vec<PathBuf>,
    /// CI logs, read for their failure summary lines.
    pub logs: Vec<PathBuf>,
    /// lcov tracefiles.
    pub lcov: Vec<PathBuf>,
    /// A label for the run (a CI run id), stored verbatim in the meta.
    pub run: Option<String>,
    /// Which surface called the ingest (`lib`, `cli`, `pyo3`); the marker names it.
    pub surface: &'static str,
}

impl Default for TestsIngestOptions {
    fn default() -> Self {
        Self { junit: Vec::new(), logs: Vec::new(), lcov: Vec::new(), run: None, surface: "lib" }
    }
}

/// A report [`tests_ingest`] could not read; the others were still ingested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportError {
    /// The report, labelled as the snapshot would store it.
    pub report: String,
    pub reason: String,
}

/// What a test-report ingest stored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TestsSummary {
    /// Every report read, sorted, as the meta records them.
    pub reports: Vec<String>,
    /// JUnit reports read.
    pub junit_files: usize,
    /// CI logs read.
    pub log_files: usize,
    /// lcov tracefiles read.
    pub lcov_files: usize,
    /// Test cases seen: `failed + errors + skipped + passed`.
    pub cases: usize,
    pub failed: usize,
    pub errors: usize,
    pub skipped: usize,
    pub passed: usize,
    /// Rows of `cases.jsonl` (every failed and errored case).
    pub stored: usize,
    /// Of those, cases a secret was redacted from.
    pub redacted: usize,
    /// Rows of `lcov.jsonl` (source files with line coverage).
    pub covered_files: usize,
    /// Reports skipped as unreadable or malformed.
    pub report_errors: Vec<ReportError>,
}

/// Read the test reports of one CI run and write them to
/// `<repo_root>/.glia/test-snapshot/`, replacing any earlier snapshot:
/// `cases.jsonl` (failed and errored cases, secrets redacted, messages and
/// traces capped), `lcov.jsonl` (line hits per source file) and `meta.json`.
/// Creates `.glia/.gitignore` when absent, never overwriting one.
///
/// A report that cannot be read, is over 50 MiB or is malformed is skipped
/// with a `[tests] skip report=… reason=…` line on stderr and listed in
/// [`TestsSummary::report_errors`]; the rest are still ingested. No reports
/// given, or none readable, is an error and writes nothing. On success prints
/// the `[tests] ingest` marker on stderr.
pub fn tests_ingest(repo_root: &Path, opts: &TestsIngestOptions) -> Result<TestsSummary, String> {
    if !repo_root.is_dir() {
        return Err(format!("not a directory: {}", repo_root.display()));
    }
    if opts.junit.is_empty() && opts.logs.is_empty() && opts.lcov.is_empty() {
        return Err("no test reports given (JUnit XML, CI log or lcov)".to_string());
    }
    let mut summary = TestsSummary::default();
    let mut cases: Vec<TestCaseRecord> = Vec::new();
    let mut coverage = Vec::new();
    let mut skipped_passed = (0, 0);
    let read = |path: &Path, summary: &mut TestsSummary| -> Option<(String, Vec<u8>)> {
        let label = report_label(repo_root, path);
        if summary.reports.contains(&label) {
            return None; // Given twice: read once.
        }
        match read_report(path) {
            Ok(bytes) => Some((label, bytes)),
            Err(reason) => {
                skip_report(summary, label, reason);
                None
            }
        }
    };
    for path in &opts.junit {
        let Some((label, bytes)) = read(path, &mut summary) else { continue };
        match parse_junit(&bytes, &label) {
            Ok((rows, counts)) => {
                cases.extend(rows);
                skipped_passed.0 += counts.skipped;
                skipped_passed.1 += counts.passed;
                summary.junit_files += 1;
                summary.reports.push(label);
            }
            Err(reason) => skip_report(&mut summary, label, reason),
        }
    }
    for path in &opts.logs {
        let Some((label, bytes)) = read(path, &mut summary) else { continue };
        cases.extend(parse_ci_log(&String::from_utf8_lossy(&bytes), &label));
        summary.log_files += 1;
        summary.reports.push(label);
    }
    for path in &opts.lcov {
        let Some((label, bytes)) = read(path, &mut summary) else { continue };
        coverage.extend(parse_lcov(&String::from_utf8_lossy(&bytes), repo_root));
        summary.lcov_files += 1;
        summary.reports.push(label);
    }
    if summary.reports.is_empty() {
        let reasons: Vec<String> =
            summary.report_errors.iter().map(|e| format!("{}: {}", e.report, e.reason)).collect();
        return Err(format!("no report could be read ({})", reasons.join("; ")));
    }

    for case in &mut cases {
        case.sanitize();
    }
    let meta = TestsMeta::new(opts.run.clone(), summary.reports.clone(), skipped_passed.0, skipped_passed.1);
    let written = write_tests(repo_root, meta, &cases, &lcov::merge(coverage))?;
    ensure_glia_gitignore(repo_root)?;

    summary.reports = written.reports;
    summary.cases = written.cases_total;
    summary.failed = written.failed;
    summary.errors = written.errors;
    summary.skipped = written.skipped;
    summary.passed = written.passed;
    summary.stored = cases.len();
    summary.redacted = cases.iter().filter(|c| c.redacted).count();
    summary.covered_files = written.lcov_files;
    eprintln!("{}", ingest_marker(&repo_label(repo_root), &summary, opts.surface));
    Ok(summary)
}

/// A report's label: repo-relative (`/`-separated) when it is under the repo,
/// else the path as given.
fn report_label(root: &Path, path: &Path) -> String {
    let under = std::fs::canonicalize(root).ok().zip(std::fs::canonicalize(path).ok()).and_then(|(root, path)| {
        let rel = path.strip_prefix(&root).ok()?;
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        Some(parts.join("/"))
    });
    under.unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// A report's bytes, unless it is over [`MAX_REPORT_BYTES`] or unreadable.
fn read_report(path: &Path) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("read: {e}"))?.len();
    if len > MAX_REPORT_BYTES as u64 {
        return Err(format!("{len} bytes is over the {} MiB report cap", MAX_REPORT_BYTES >> 20));
    }
    std::fs::read(path).map_err(|e| format!("read: {e}"))
}

fn skip_report(summary: &mut TestsSummary, report: String, reason: String) {
    eprintln!("[tests] skip report={report} reason={reason}");
    summary.report_errors.push(ReportError { report, reason });
}

/// `[tests] ingest repo=<label> junit_files=N log_files=N lcov_files=N cases=N
/// failed=N errors=N skipped=N passed=N stored=N surface=<surface>`.
fn ingest_marker(label: &str, s: &TestsSummary, surface: &str) -> String {
    format!(
        "[tests] ingest repo={label} junit_files={} log_files={} lcov_files={} cases={} failed={} errors={} \
         skipped={} passed={} stored={} surface={surface}",
        s.junit_files, s.log_files, s.lcov_files, s.cases, s.failed, s.errors, s.skipped, s.passed, s.stored
    )
}

/// Create `<root>/.glia/.gitignore` with [`GLIA_GITIGNORE`] unless one exists.
fn ensure_glia_gitignore(root: &Path) -> Result<(), String> {
    let dir = root.join(CONTROL_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join(".gitignore");
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => file
            .write_all(GLIA_GITIGNORE.as_bytes())
            .map_err(|e| format!("write {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(format!("create {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_names_the_window_and_surface() {
        let summary = HistorySummary {
            head: "3bd47ffe34bc0123456789abcdef0123456789ab".into(),
            commits: 5,
            files: 3,
            renames: 1,
            binary: 0,
            blame_files: 0,
            runs: 0,
        };
        assert_eq!(
            sync_marker("g1", &summary, &HistoryOptions::default()),
            "[history] sync repo=g1 head=3bd47ffe34bc commits=5 files=3 renames=1 binary=0 \
             blame_files=0 runs=0 window=max:2000 surface=lib"
        );
        let opts = HistoryOptions { since: Some("2 weeks ago".into()), surface: "cli", ..HistoryOptions::default() };
        assert!(sync_marker("g1", &summary, &opts).ends_with("window=max:2000,since:2_weeks_ago surface=cli"));
    }

    #[test]
    fn ingest_marker_is_the_specified_line() {
        let summary = TestsSummary {
            junit_files: 1,
            lcov_files: 1,
            cases: 3,
            failed: 1,
            passed: 2,
            stored: 1,
            ..TestsSummary::default()
        };
        assert_eq!(
            ingest_marker("g1", &summary, "lib"),
            "[tests] ingest repo=g1 junit_files=1 log_files=0 lcov_files=1 cases=3 failed=1 errors=0 \
             skipped=0 passed=2 stored=1 surface=lib"
        );
    }

    #[test]
    fn gitignore_lists_only_regenerable_inputs() {
        let lines: Vec<&str> = GLIA_GITIGNORE.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(lines, ["vectors.jsonl", "docs-snapshot/", "history-snapshot/", "test-snapshot/"]);
    }
}
