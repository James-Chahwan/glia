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
//! emails are never captured; commit messages (trailers dropped, redacted) go
//! only into its `messages.jsonl` sidecar, which the build never reads (EB.1).
//!
//! [`tests_ingest`] reads test reports a CI run produced — JUnit XML, CI logs,
//! lcov — and adds them as one run to `<repo>/.glia/test-snapshot/`, which
//! keeps the last [`TestsIngestOptions::window`] runs (CC.9b). It spawns
//! nothing and reads only the files it is given.
//!
//! [`ScipImporter`] (CE.1b) takes a SCIP index's documents, decoded by its
//! caller, reads each one's source inside the repo and writes
//! `<repo>/.glia/scip-snapshot/`: definition names, reference lines with a
//! call flag, and symbol ids in string order. It spawns nothing and has no
//! protobuf dependency.
//!
//! [`vocab_sync`] (FB.13a) reads every tracked text file of one commit's tree
//! (`git ls-tree` + one `git cat-file --batch`) and writes the identifier
//! vocabulary to `<repo>/.glia/vocab-snapshot/`, a query-time input no build
//! reads (`glia_code_domain::snapshots::read_vocab` reads it back).

mod ci_log;
mod git_history;
mod junit;
mod lcov;
mod scip;
mod vocab;

use std::io::Write as _;
use std::path::{Path, PathBuf};

use glia_code_domain::snapshots::{
    TESTS_DEFAULT_WINDOW, TestCaseRecord, TestsMeta, append_tests_run, write_history_with_messages,
};
use glia_code_domain::walk_gating::CONTROL_DIR;
use serde::Serialize;

pub use ci_log::parse_ci_log;
pub use git_history::{HistoryOptions, HistorySummary};
pub use junit::{JunitCounts, parse_junit};
pub use lcov::parse_lcov;
pub use scip::{
    PositionEncoding, ScipDocumentIn, ScipImportOptions, ScipImportSummary, ScipImporter,
    ScipIndexInfo, ScipOccurrenceIn, ScipSymbolIn, next_is_call, unit_offset_to_byte,
};
pub use vocab::{BINARY_PROBE, MAX_BLOB_BYTES, Vocab, capture_vocab, tokens_of};

/// Largest report file read: CI XML and logs can be huge, and a report past
/// this is skipped with an error rather than read into memory.
pub(crate) const MAX_REPORT_BYTES: usize = 50 << 20;

/// `.glia/.gitignore`, created when absent: the control dir's local,
/// regenerable inputs. The graph layout dir ignores itself (its own `*`), and
/// the checked-in inputs (`overlay.toml`, `cells.jsonl`) are not listed. The
/// SCIP snapshot dir also ignores itself, since a repo whose `.gitignore`
/// predates it never gets this line. An existing `.glia/.gitignore` is never
/// rewritten, so a repo whose file predates FB.13a should add
/// `vocab-snapshot/` to it by hand.
const GLIA_GITIGNORE: &str = "\
# Local, regenerable glia inputs (written by glia; safe to delete).
vectors.jsonl
docs-snapshot/
history-snapshot/
test-snapshot/
scip-snapshot/
vocab-snapshot/
";

/// Read the git history of the repo at `repo_root` and write it to
/// `<repo_root>/.glia/history-snapshot/` (commits.jsonl, blame.jsonl,
/// messages.jsonl when [`HistoryOptions::messages`], then meta.json). Creates `.glia/.gitignore` when absent, never overwriting one.
///
/// Capture errors (not a directory, not in a git work tree, no commits, git
/// missing or failing) come back as a plain message before anything is
/// written. On success prints the `[history] sync` marker on stderr.
pub fn history_sync(repo_root: &Path, opts: &HistoryOptions) -> Result<HistorySummary, String> {
    let capture = git_history::capture(repo_root, opts)?;
    let summary = git_history::summarize(&capture);
    write_history_with_messages(
        repo_root,
        capture.meta,
        &capture.commits,
        &capture.blame,
        capture.messages.as_deref(),
    )?;
    ensure_glia_gitignore(repo_root)?;
    eprintln!("{}", sync_marker(&repo_label(repo_root), &summary, opts));
    Ok(summary)
}

/// Options for [`vocab_sync`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct VocabOptions {
    /// The commit whose tree is read; `None` is `HEAD`.
    pub rev: Option<String>,
}

impl VocabOptions {
    /// Read commit `rev` instead of `HEAD`.
    pub fn at(rev: impl Into<String>) -> Self {
        Self {
            rev: Some(rev.into()),
        }
    }
}

/// What a vocabulary sync wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct VocabSummary {
    /// Full sha of the commit read.
    pub rev: String,
    /// Text files read.
    pub files: usize,
    /// Distinct tokens.
    pub tokens: usize,
    /// Tokens in more than [`VOCAB_FILES_CAP`](glia_code_domain::snapshots::VOCAB_FILES_CAP) files.
    pub common: usize,
    /// Blobs skipped as binary.
    pub skipped_binary: usize,
    /// Blobs skipped as larger than [`MAX_BLOB_BYTES`].
    pub skipped_large: usize,
    /// Wall time of the capture and write, milliseconds.
    pub ms: u64,
}

/// Read the identifier vocabulary of commit `opts.rev` (default `HEAD`) of the
/// repo at `repo_root` and write it to `<repo_root>/.glia/vocab-snapshot/`
/// (files.jsonl, vocab.jsonl, then meta.json). Creates `.glia/.gitignore`
/// when absent, never overwriting one. Capture errors come back before
/// anything is written. On success prints, on stderr,
/// `[vocab] sync rev=<12> files=<f> tokens=<t> common=<c> skipped_binary=<b>
/// skipped_large=<l> ms=<ms>`.
pub fn vocab_sync(repo_root: &Path, opts: &VocabOptions) -> Result<VocabSummary, String> {
    let start = std::time::Instant::now();
    let rev = opts.rev.as_deref().unwrap_or("HEAD");
    let vocab = capture_vocab(repo_root, rev)?;
    let meta = glia_code_domain::snapshots::write_vocab(
        repo_root,
        vocab.meta,
        &vocab.files,
        &vocab.tokens,
    )?;
    ensure_glia_gitignore(repo_root)?;
    let summary = VocabSummary {
        rev: meta.rev,
        files: meta.files,
        tokens: meta.tokens,
        common: meta.common,
        skipped_binary: meta.skipped_binary,
        skipped_large: meta.skipped_large,
        ms: start.elapsed().as_millis() as u64,
    };
    eprintln!("{}", vocab_marker(&summary));
    Ok(summary)
}

/// The `[vocab] sync` marker line.
fn vocab_marker(s: &VocabSummary) -> String {
    let rev: String = s.rev.chars().take(12).collect();
    format!(
        "[vocab] sync rev={rev} files={} tokens={} common={} skipped_binary={} skipped_large={} ms={}",
        s.files, s.tokens, s.common, s.skipped_binary, s.skipped_large, s.ms
    )
}

/// The repo's project name (manifest label, else directory name).
fn repo_label(root: &Path) -> String {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    glia_code_domain::project_roots::project_name(&canonical)
        .unwrap_or_else(|| canonical.display().to_string())
}

/// `[history] sync repo=<label> head=<12> commits=N files=N renames=N binary=N
/// blame_files=N runs=N window=max:N[,since:<since>] messages=N redacted=N
/// branch_labels=N trailers_dropped=N surface=<surface>` (`messages=off` with
/// messages off).
fn sync_marker(label: &str, s: &HistorySummary, opts: &HistoryOptions) -> String {
    let head: String = s.head.chars().take(12).collect();
    let mut window = format!("max:{}", opts.max_commits);
    if let Some(since) = &opts.since {
        let since: String = since
            .chars()
            .map(|c| if c.is_whitespace() { '_' } else { c })
            .collect();
        window.push_str(&format!(",since:{since}"));
    }
    let messages = if opts.messages {
        format!(
            "messages={} redacted={} branch_labels={} trailers_dropped={}",
            s.messages, s.redacted, s.branch_labels, s.trailers_dropped
        )
    } else {
        "messages=off".to_string()
    };
    format!(
        "[history] sync repo={label} head={head} commits={} files={} renames={} binary={} blame_files={} runs={} window={window} {messages} surface={}",
        s.commits, s.files, s.renames, s.binary, s.blame_files, s.runs, opts.surface
    )
}

/// Options for [`tests_ingest`]: the reports one CI run produced, and how
/// many runs the snapshot keeps.
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
    /// The most runs the snapshot keeps, this one included
    /// ([`TESTS_DEFAULT_WINDOW`]); 1 replaces the snapshot, as 0.5.0 did. 0
    /// is an error.
    pub window: usize,
    /// Drop every earlier run first: this run is seq 0.
    pub reset: bool,
    /// Which surface called the ingest (`lib`, `cli`, `pyo3`); the marker names it.
    pub surface: &'static str,
}

impl Default for TestsIngestOptions {
    fn default() -> Self {
        Self {
            junit: Vec::new(),
            logs: Vec::new(),
            lcov: Vec::new(),
            run: None,
            window: TESTS_DEFAULT_WINDOW,
            reset: false,
            surface: "lib",
        }
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
    /// Runs the snapshot holds now, this one included.
    pub runs: usize,
    /// This run's seq: 0 for the first run (or after a reset), then one more
    /// per ingest.
    pub seq: u32,
    /// Reports skipped as unreadable or malformed.
    pub report_errors: Vec<ReportError>,
}

/// Read the test reports of one CI run and add them as the newest run of
/// `<repo_root>/.glia/test-snapshot/` (`append_tests_run`): `cases.jsonl`
/// (every kept run's failed and errored cases, secrets redacted, messages and
/// traces capped), `lcov.jsonl` (this run's line hits per source file) and
/// `meta.json`. The snapshot keeps the last `opts.window` runs, this one
/// included; `opts.reset` drops the earlier ones first. Creates
/// `.glia/.gitignore` when absent, never overwriting one.
///
/// A report that cannot be read, is over 50 MiB or is malformed is skipped
/// with a `[tests] skip report=… reason=…` line on stderr and listed in
/// [`TestsSummary::report_errors`]; the rest are still ingested. No reports
/// given, none readable, or a window of 0 is an error and writes nothing. On
/// success prints the `[tests] ingest` marker on stderr.
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
        let Some((label, bytes)) = read(path, &mut summary) else {
            continue;
        };
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
        let Some((label, bytes)) = read(path, &mut summary) else {
            continue;
        };
        cases.extend(parse_ci_log(&String::from_utf8_lossy(&bytes), &label));
        summary.log_files += 1;
        summary.reports.push(label);
    }
    for path in &opts.lcov {
        let Some((label, bytes)) = read(path, &mut summary) else {
            continue;
        };
        coverage.extend(parse_lcov(&String::from_utf8_lossy(&bytes), repo_root));
        summary.lcov_files += 1;
        summary.reports.push(label);
    }
    if summary.reports.is_empty() {
        let reasons: Vec<String> = summary
            .report_errors
            .iter()
            .map(|e| format!("{}: {}", e.report, e.reason))
            .collect();
        return Err(format!("no report could be read ({})", reasons.join("; ")));
    }

    for case in &mut cases {
        case.sanitize();
    }
    let meta = TestsMeta::new(
        opts.run.clone(),
        summary.reports.clone(),
        skipped_passed.0,
        skipped_passed.1,
    );
    let written = append_tests_run(
        repo_root,
        meta,
        &cases,
        &lcov::merge(coverage),
        opts.window,
        opts.reset,
    )?;
    ensure_glia_gitignore(repo_root)?;

    summary.runs = written.runs.len();
    summary.seq = written.latest_seq().unwrap_or(0);
    summary.reports = written.reports;
    summary.cases = written.cases_total;
    summary.failed = written.failed;
    summary.errors = written.errors;
    summary.skipped = written.skipped;
    summary.passed = written.passed;
    summary.stored = cases.len();
    summary.redacted = cases.iter().filter(|c| c.redacted).count();
    summary.covered_files = written.lcov_files;
    eprintln!(
        "{}",
        ingest_marker(&repo_label(repo_root), &summary, opts.window, opts.surface)
    );
    Ok(summary)
}

/// A report's label: repo-relative (`/`-separated) when it is under the repo,
/// else the path as given.
fn report_label(root: &Path, path: &Path) -> String {
    let under = std::fs::canonicalize(root)
        .ok()
        .zip(std::fs::canonicalize(path).ok())
        .and_then(|(root, path)| {
            let rel = path.strip_prefix(&root).ok()?;
            let parts: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            Some(parts.join("/"))
        });
    under.unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// A report's bytes, unless it is over [`MAX_REPORT_BYTES`] or unreadable.
fn read_report(path: &Path) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path)
        .map_err(|e| format!("read: {e}"))?
        .len();
    if len > MAX_REPORT_BYTES as u64 {
        return Err(format!(
            "{len} bytes is over the {} MiB report cap",
            MAX_REPORT_BYTES >> 20
        ));
    }
    std::fs::read(path).map_err(|e| format!("read: {e}"))
}

fn skip_report(summary: &mut TestsSummary, report: String, reason: String) {
    eprintln!("[tests] skip report={report} reason={reason}");
    summary.report_errors.push(ReportError { report, reason });
}

/// `[tests] ingest repo=<label> junit_files=N log_files=N lcov_files=N cases=N
/// failed=N errors=N skipped=N passed=N stored=N runs=<R> window=<W> seq=<S>
/// surface=<surface>`: the counts are this run's, `runs` the runs the
/// snapshot holds now, `window` the most it keeps and `seq` this run's.
fn ingest_marker(label: &str, s: &TestsSummary, window: usize, surface: &str) -> String {
    format!(
        "[tests] ingest repo={label} junit_files={} log_files={} lcov_files={} cases={} failed={} errors={} \
         skipped={} passed={} stored={} runs={} window={window} seq={} surface={surface}",
        s.junit_files,
        s.log_files,
        s.lcov_files,
        s.cases,
        s.failed,
        s.errors,
        s.skipped,
        s.passed,
        s.stored,
        s.runs,
        s.seq
    )
}

/// Create `<root>/.glia/.gitignore` with [`GLIA_GITIGNORE`] unless one exists.
fn ensure_glia_gitignore(root: &Path) -> Result<(), String> {
    let dir = root.join(CONTROL_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join(".gitignore");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
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
            messages: 5,
            redacted: 1,
            trailers_dropped: 4,
            branch_labels: 2,
        };
        assert_eq!(
            sync_marker("g1", &summary, &HistoryOptions::default()),
            "[history] sync repo=g1 head=3bd47ffe34bc commits=5 files=3 renames=1 binary=0 \
             blame_files=0 runs=0 window=max:2000 messages=5 redacted=1 branch_labels=2 trailers_dropped=4 surface=lib"
        );
        let opts = HistoryOptions {
            since: Some("2 weeks ago".into()),
            surface: "cli",
            ..HistoryOptions::default()
        };
        assert!(sync_marker("g1", &summary, &opts).ends_with(
            "window=max:2000,since:2_weeks_ago messages=5 redacted=1 branch_labels=2 trailers_dropped=4 surface=cli"
        ));
        let off = HistoryOptions {
            messages: false,
            ..HistoryOptions::default()
        };
        assert!(
            sync_marker("g1", &summary, &off).ends_with("window=max:2000 messages=off surface=lib")
        );
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
            runs: 1,
            ..TestsSummary::default()
        };
        assert_eq!(
            ingest_marker("g1", &summary, 10, "lib"),
            "[tests] ingest repo=g1 junit_files=1 log_files=0 lcov_files=1 cases=3 failed=1 errors=0 \
             skipped=0 passed=2 stored=1 runs=1 window=10 seq=0 surface=lib"
        );
        // The packet's fired_on line: the third run of a window of 10.
        let third = TestsSummary {
            lcov_files: 0,
            cases: 2,
            passed: 1,
            runs: 3,
            seq: 2,
            ..summary
        };
        assert!(
            ingest_marker("g1", &third, 10, "cli")
                .ends_with(" cases=2 failed=1 errors=0 skipped=0 passed=1 stored=1 runs=3 window=10 seq=2 surface=cli")
        );
    }

    #[test]
    fn vocab_marker_is_the_specified_line() {
        let summary = VocabSummary {
            rev: "3bd47ffe34bc0123456789abcdef0123456789ab".into(),
            files: 3,
            tokens: 40,
            common: 1,
            skipped_binary: 2,
            skipped_large: 0,
            ms: 7,
        };
        assert_eq!(
            vocab_marker(&summary),
            "[vocab] sync rev=3bd47ffe34bc files=3 tokens=40 common=1 skipped_binary=2 skipped_large=0 ms=7"
        );
    }

    #[test]
    fn gitignore_lists_only_regenerable_inputs() {
        let lines: Vec<&str> = GLIA_GITIGNORE
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect();
        assert_eq!(
            lines,
            [
                "vectors.jsonl",
                "docs-snapshot/",
                "history-snapshot/",
                "test-snapshot/",
                "scip-snapshot/",
                "vocab-snapshot/"
            ]
        );
    }
}
