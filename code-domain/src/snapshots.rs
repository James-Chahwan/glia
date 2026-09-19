//! Git-history and test-report snapshot records, and their `data_hash`.
//!
//! External, HEAD-dependent inputs enter glia the way docs do: a separate
//! snapshot step (the `glia-snapshots` crate — the only code that shells out)
//! writes files under `<repo>/.glia/`, and the deterministic build reads them
//! back through this module. Nothing here spawns a process or touches git.
//!
//! The history snapshot (LF.5a) is `<repo>/.glia/history-snapshot/`:
//! - `commits.jsonl` — one [`HistoryCommit`] per line, newest first (git's order);
//! - `blame.jsonl` — one [`BlameFile`] per line, sorted by path; empty when
//!   blame was not requested;
//! - `meta.json` — the [`HistoryMeta`], written LAST. Its `data_hash` covers
//!   `commits.jsonl` then `blame.jsonl`, so a reader that finds a meta whose
//!   hash matches has complete data ([`read_history`] checks it).
//!
//! Neither a wall-clock timestamp nor an author identity is ever stored: a
//! re-sync at the same HEAD writes identical bytes, and per-person attribution
//! (team ownership) is out of scope. Every file is written `<file>.tmp` then
//! renamed; the store's input fingerprint (LF.1d) skips `*.tmp`.
//!
//! The test-report snapshot (LF.6a) is `<repo>/.glia/test-snapshot/`, and it
//! describes ONE run: a re-ingest replaces it whole.
//! - `cases.jsonl` — one [`TestCaseRecord`] per failed or errored test case,
//!   sorted by (report, suite, classname, name);
//! - `lcov.jsonl` — one [`LcovFileRecord`] per covered source file, sorted by
//!   its repo-relative path (else its `SF:` path);
//! - `meta.json` — the [`TestsMeta`], written LAST, `data_hash` over
//!   `cases.jsonl` then `lcov.jsonl` ([`read_tests`] checks it).
//!
//! Test output is untrusted text: a failing assertion can print a token. Every
//! free-text field of a case passes through [`redact_untrusted`] (A13.7's key
//! denylist plus secret-shaped values) when it is written AND when it is read
//! back, so a hand-edited snapshot cannot carry a secret into the graph either.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;

use crate::walk_gating::CONTROL_DIR;

/// The history snapshot's directory name under the `.glia` control dir.
pub const HISTORY_DIR: &str = "history-snapshot";
/// `commits.jsonl`: one [`HistoryCommit`] per line, newest first.
pub const HISTORY_COMMITS_FILE: &str = "commits.jsonl";
/// `blame.jsonl`: one [`BlameFile`] per line, sorted by path.
pub const HISTORY_BLAME_FILE: &str = "blame.jsonl";
/// A snapshot's metadata file, written after its data files.
pub const META_FILE: &str = "meta.json";
/// [`HistoryMeta::version`] this module writes and accepts.
pub const HISTORY_VERSION: u32 = 1;
/// [`HistoryMeta::generator`] of `glia history sync`.
pub const HISTORY_GENERATOR: &str = "glia history sync";

/// `.glia/history-snapshot/meta.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryMeta {
    /// [`HISTORY_VERSION`]; any other value reads as an incomplete snapshot.
    pub version: u32,
    /// Who wrote it ([`HISTORY_GENERATOR`]).
    pub generator: String,
    /// Full sha of the commit the history was read from.
    pub head: String,
    /// Number of rows in `commits.jsonl`.
    pub commits: usize,
    /// The `git log -n` bound the sync used.
    pub max_commits: usize,
    /// The `git log --since` bound the sync used, verbatim.
    #[serde(default)]
    pub since: Option<String>,
    /// The repo root relative to the git top level, `/`-terminated
    /// (`git rev-parse --show-prefix`); `""` when the root is the top level.
    /// Every path in the snapshot is relative to the repo root, not the top level.
    pub relative_to: String,
    /// Number of rows in `blame.jsonl`.
    pub blame_files: usize,
    /// [`data_hash`] over `commits.jsonl` then `blame.jsonl`.
    pub data_hash: String,
}

impl HistoryMeta {
    /// A meta for a history read at `head`. The derived fields (`commits`,
    /// `blame_files`, `data_hash`) are left empty: [`write_history`] fills them
    /// from the rows it writes.
    pub fn new(head: String, max_commits: usize, since: Option<String>, relative_to: String) -> Self {
        Self {
            version: HISTORY_VERSION,
            generator: HISTORY_GENERATOR.to_string(),
            head,
            commits: 0,
            max_commits,
            since,
            relative_to,
            blame_files: 0,
            data_hash: String::new(),
        }
    }
}

/// One non-merge commit: its sha, committer time and the files it touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryCommit {
    /// Full commit sha.
    pub c: String,
    /// Committer time, unix seconds (`%ct`).
    pub t: i64,
    /// The files it touched, in git's `--numstat` order.
    pub files: Vec<HistoryFile>,
}

/// One file of a commit's `--numstat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryFile {
    /// Path relative to the repo root (for a rename, the new path).
    pub p: String,
    /// Lines added; `None` for a binary file.
    pub a: Option<u32>,
    /// Lines deleted; `None` for a binary file.
    pub d: Option<u32>,
    /// The rename source, when git detected this file as a rename (`-M`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// `git blame` of one file at the snapshot's head, as runs of consecutive
/// lines last changed at the same committer time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlameFile {
    /// Path relative to the repo root.
    pub p: String,
    /// `[start_line, end_line, committer_time]`: 1-based, end inclusive,
    /// sorted by start line, non-overlapping.
    pub runs: Vec<[i64; 3]>,
}

/// A complete history snapshot, as [`read_history`] returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySnapshot {
    pub meta: HistoryMeta,
    pub commits: Vec<HistoryCommit>,
    pub blame: Vec<BlameFile>,
}

/// xxhash64 (seed 0) over the concatenation of `parts`, as 16 lowercase hex
/// digits. The snapshot writers, their readers and fixture authors all compute
/// a snapshot's `data_hash` with it.
pub fn data_hash(parts: &[&[u8]]) -> String {
    use core::hash::Hasher;
    let mut h = twox_hash::XxHash64::with_seed(0);
    for part in parts {
        h.write(part);
    }
    format!("{:016x}", h.finish())
}

/// `<root>/.glia/history-snapshot`.
pub fn history_dir(root: &Path) -> PathBuf {
    root.join(CONTROL_DIR).join(HISTORY_DIR)
}

/// Write a history snapshot under `<root>/.glia/history-snapshot/`:
/// `commits.jsonl` (rows in the order given), `blame.jsonl` (sorted by path),
/// then `meta.json` LAST. An existing `meta.json` is removed first, so a write
/// that stops part-way leaves a snapshot [`read_history`] rejects rather than
/// one that pairs an old meta with new data.
///
/// `meta.commits`, `meta.blame_files` and `meta.data_hash` are derived here
/// from the rows, whatever the caller set. Returns the meta as written.
pub fn write_history(
    root: &Path,
    mut meta: HistoryMeta,
    commits: &[HistoryCommit],
    blame: &[BlameFile],
) -> Result<HistoryMeta, String> {
    let dir = history_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let meta_path = dir.join(META_FILE);
    remove_if_present(&meta_path)?;

    let mut blame_sorted: Vec<&BlameFile> = blame.iter().collect();
    blame_sorted.sort_by(|x, y| x.p.cmp(&y.p));
    let commits_bytes = jsonl(commits.iter())?;
    let blame_bytes = jsonl(blame_sorted.iter().copied())?;
    write_atomic(&dir.join(HISTORY_COMMITS_FILE), &commits_bytes)?;
    write_atomic(&dir.join(HISTORY_BLAME_FILE), &blame_bytes)?;

    meta.commits = commits.len();
    meta.blame_files = blame.len();
    meta.data_hash = data_hash(&[&commits_bytes, &blame_bytes]);
    write_atomic(&meta_path, &meta_bytes(&meta)?)?;
    Ok(meta)
}

/// The history snapshot under `<root>/.glia/history-snapshot/`, or `None`.
///
/// No snapshot directory is the normal "never synced" case: `None`, silently.
/// A directory whose `meta.json` is absent, unreadable or of another version,
/// whose data does not hash to `meta.data_hash`, whose rows do not parse or
/// whose row counts disagree with the meta is an incomplete snapshot: `None`
/// and one `[history] snapshot incomplete` line on stderr naming the reason.
/// A missing `blame.jsonl` reads as empty (blame is optional).
pub fn read_history(root: &Path) -> Option<HistorySnapshot> {
    let dir = history_dir(root);
    if !dir.is_dir() {
        return None;
    }
    match load_history(&dir) {
        Ok(snapshot) => Some(snapshot),
        Err(reason) => {
            eprintln!("[history] snapshot incomplete dir={} reason={reason}", dir.display());
            None
        }
    }
}

fn load_history(dir: &Path) -> Result<HistorySnapshot, String> {
    let meta_bytes = read_required(&dir.join(META_FILE))?;
    let meta: HistoryMeta =
        serde_json::from_slice(&meta_bytes).map_err(|e| format!("{META_FILE}: {e}"))?;
    if meta.version != HISTORY_VERSION {
        return Err(format!("{META_FILE}: version {} (reader wants {HISTORY_VERSION})", meta.version));
    }
    let commits_bytes = read_required(&dir.join(HISTORY_COMMITS_FILE))?;
    let blame_bytes = match std::fs::read(dir.join(HISTORY_BLAME_FILE)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("{HISTORY_BLAME_FILE}: {e}")),
    };
    let got = data_hash(&[&commits_bytes, &blame_bytes]);
    if got != meta.data_hash {
        return Err(format!("data_hash {got} != meta {}", meta.data_hash));
    }
    let commits: Vec<HistoryCommit> = parse_jsonl(&commits_bytes, HISTORY_COMMITS_FILE)?;
    let blame: Vec<BlameFile> = parse_jsonl(&blame_bytes, HISTORY_BLAME_FILE)?;
    if commits.len() != meta.commits || blame.len() != meta.blame_files {
        return Err(format!(
            "row counts commits={} blame_files={} != meta commits={} blame_files={}",
            commits.len(),
            blame.len(),
            meta.commits,
            meta.blame_files
        ));
    }
    Ok(HistorySnapshot { meta, commits, blame })
}

fn read_required(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{name} absent")
        } else {
            format!("{name}: {e}")
        }
    })
}

/// One compact JSON object per row, each `\n`-terminated.
fn jsonl<'a, T: Serialize + 'a>(rows: impl Iterator<Item = &'a T>) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut out, row).map_err(|e| format!("serialize row: {e}"))?;
        out.push(b'\n');
    }
    Ok(out)
}

/// Rows of a JSONL file; blank lines are skipped.
fn parse_jsonl<T: DeserializeOwned>(bytes: &[u8], name: &str) -> Result<Vec<T>, String> {
    let mut rows = Vec::new();
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let row = serde_json::from_slice(line).map_err(|e| format!("{name}:{}: {e}", i + 1))?;
        rows.push(row);
    }
    Ok(rows)
}

/// Write `bytes` to `<path>.tmp`, then rename it over `path`.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} -> {}: {e}", tmp.display(), path.display()))
}

/// Remove `path` if it exists.
fn remove_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {}: {e}", path.display())),
    }
}

/// `meta` as pretty JSON plus a trailing newline.
fn meta_bytes<T: Serialize>(meta: &T) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(meta).map_err(|e| format!("serialize {META_FILE}: {e}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

// ===========================================================================
// Test-report snapshot (LF.6a)
// ===========================================================================

/// The test-report snapshot's directory name under the `.glia` control dir.
pub const TESTS_DIR: &str = "test-snapshot";
/// `cases.jsonl`: one [`TestCaseRecord`] per failed or errored case.
pub const TESTS_CASES_FILE: &str = "cases.jsonl";
/// `lcov.jsonl`: one [`LcovFileRecord`] per covered source file.
pub const TESTS_LCOV_FILE: &str = "lcov.jsonl";
/// [`TestsMeta::version`] this module writes and accepts.
pub const TESTS_VERSION: u32 = 1;
/// [`TestCaseRecord::source`] of a case read from a JUnit XML report.
pub const SOURCE_JUNIT: &str = "junit";
/// [`TestCaseRecord::source`] of a case read from a CI log's summary lines.
pub const SOURCE_LOG: &str = "log";
/// [`TestCaseRecord::status`] of a failed assertion (`<failure>`).
pub const STATUS_FAILED: &str = "failed";
/// [`TestCaseRecord::status`] of an unexpected error (`<error>`).
pub const STATUS_ERROR: &str = "error";
/// Longest [`TestCaseRecord::message`] kept, in chars.
pub const MESSAGE_CAP: usize = 300;
/// Longest [`TestCaseRecord::trace`] kept, in chars.
pub const TRACE_CAP: usize = 4096;

/// `.glia/test-snapshot/meta.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestsMeta {
    /// [`TESTS_VERSION`]; any other value reads as an incomplete snapshot.
    pub version: u32,
    /// The caller's label for the run (a CI run id), verbatim.
    #[serde(default)]
    pub run: Option<String>,
    /// Every report read, sorted: repo-relative when under the repo, else as given.
    pub reports: Vec<String>,
    /// `failed + errors + skipped + passed`.
    pub cases_total: usize,
    /// Cases with status [`STATUS_FAILED`] in `cases.jsonl`.
    pub failed: usize,
    /// Cases with status [`STATUS_ERROR`] in `cases.jsonl`.
    pub errors: usize,
    /// Skipped cases: counted, never stored.
    pub skipped: usize,
    /// Passed cases: counted, never stored.
    pub passed: usize,
    /// Rows of `lcov.jsonl` (source files with line coverage).
    pub lcov_files: usize,
    /// [`data_hash`] over `cases.jsonl` then `lcov.jsonl`.
    pub data_hash: String,
}

impl TestsMeta {
    /// A meta for one run. The derived fields (`cases_total`, `failed`,
    /// `errors`, `lcov_files`, `data_hash`) are left empty: [`write_tests`]
    /// fills them from the rows it writes.
    pub fn new(run: Option<String>, reports: Vec<String>, skipped: usize, passed: usize) -> Self {
        Self {
            version: TESTS_VERSION,
            run,
            reports,
            cases_total: 0,
            failed: 0,
            errors: 0,
            skipped,
            passed,
            lcov_files: 0,
            data_hash: String::new(),
        }
    }
}

/// One failed or errored test case, from a JUnit report or a CI log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestCaseRecord {
    /// [`SOURCE_JUNIT`] or [`SOURCE_LOG`].
    pub source: String,
    /// The report it came from: repo-relative when under the repo, else as given.
    pub report: String,
    /// The innermost `<testsuite name>` (JUnit), or the describe path (jest log).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<String>,
    /// `<testcase classname>`, or what the log line implies (a dotted pytest
    /// module, a Go package, a Rust module path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classname: Option<String>,
    /// The test's own name.
    pub name: String,
    /// The test's file, as the report names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based line of the test in `file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// [`STATUS_FAILED`] or [`STATUS_ERROR`].
    pub status: String,
    /// The failure message, at most [`MESSAGE_CAP`] chars, redacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The failure body (traceback, stack, test log), at most [`TRACE_CAP`]
    /// chars, redacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<String>,
    /// Whether [`redact_untrusted`] removed anything from this case.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub redacted: bool,
}

impl TestCaseRecord {
    /// Redact every free-text field with [`redact_untrusted`], then cap
    /// `message` at [`MESSAGE_CAP`] and `trace` at [`TRACE_CAP`] chars, cutting
    /// on char boundaries. Redaction runs on the whole text first, so a token
    /// the cap would split is still recognised. Sets `redacted` when anything
    /// was redacted and returns how many spans were. Idempotent: a second call
    /// finds nothing and returns 0.
    pub fn sanitize(&mut self) -> usize {
        let mut spans = 0;
        let mut clean = |s: &mut String| {
            let (redacted, n) = redact_untrusted(s);
            if n > 0 {
                *s = redacted;
                spans += n;
            }
        };
        clean(&mut self.name);
        for field in [&mut self.suite, &mut self.classname, &mut self.file, &mut self.message, &mut self.trace]
            .into_iter()
            .flatten()
        {
            clean(field);
        }
        if let Some(message) = &mut self.message {
            truncate_chars(message, MESSAGE_CAP);
        }
        if let Some(trace) = &mut self.trace {
            truncate_chars(trace, TRACE_CAP);
        }
        if spans > 0 {
            self.redacted = true;
        }
        spans
    }
}

/// Line coverage of one source file, from lcov `SF:` / `DA:` records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LcovFileRecord {
    /// The `SF:` path as the report gives it.
    pub sf: String,
    /// `sf` relative to the repo root (`/`-separated); `None` when it is
    /// outside the repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
    /// `[line, hits]`: 1-based lines, sorted, one entry per line.
    pub lines: Vec<[u32; 2]>,
}

impl LcovFileRecord {
    /// The sort key: `rel`, else `sf`.
    pub fn key(&self) -> &str {
        self.rel.as_deref().unwrap_or(&self.sf)
    }
}

/// A complete test-report snapshot, as [`read_tests`] returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestsSnapshot {
    pub meta: TestsMeta,
    pub cases: Vec<TestCaseRecord>,
    pub lcov: Vec<LcovFileRecord>,
}

/// `<root>/.glia/test-snapshot`.
pub fn tests_dir(root: &Path) -> PathBuf {
    root.join(CONTROL_DIR).join(TESTS_DIR)
}

/// Write a test-report snapshot under `<root>/.glia/test-snapshot/`, replacing
/// any earlier one: `cases.jsonl` (each case sanitized, then sorted by
/// report, suite, classname, name), `lcov.jsonl` (sorted by [`LcovFileRecord::key`]),
/// then `meta.json` LAST. The old `meta.json` is removed first, so a write
/// that stops part-way leaves a snapshot [`read_tests`] rejects.
///
/// A case whose status is neither [`STATUS_FAILED`] nor [`STATUS_ERROR`] is
/// an error: passed and skipped cases are counted in the meta, never stored.
/// `meta.cases_total`, `failed`, `errors`, `lcov_files` and `data_hash` are
/// derived here from the rows, and `meta.reports` is sorted and deduplicated.
/// Returns the meta as written.
pub fn write_tests(
    root: &Path,
    mut meta: TestsMeta,
    cases: &[TestCaseRecord],
    lcov: &[LcovFileRecord],
) -> Result<TestsMeta, String> {
    let mut rows = cases.to_vec();
    for row in &mut rows {
        if row.status != STATUS_FAILED && row.status != STATUS_ERROR {
            return Err(format!(
                "case {:?}: status {:?} is not stored (only {STATUS_FAILED} and {STATUS_ERROR} are)",
                row.name, row.status
            ));
        }
        row.sanitize();
    }
    rows.sort_by(case_order);
    let mut coverage: Vec<&LcovFileRecord> = lcov.iter().collect();
    coverage.sort_by(|a, b| a.key().cmp(b.key()).then_with(|| a.sf.cmp(&b.sf)));

    let dir = tests_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let meta_path = dir.join(META_FILE);
    remove_if_present(&meta_path)?;
    let cases_bytes = jsonl(rows.iter())?;
    let lcov_bytes = jsonl(coverage.iter().copied())?;
    write_atomic(&dir.join(TESTS_CASES_FILE), &cases_bytes)?;
    write_atomic(&dir.join(TESTS_LCOV_FILE), &lcov_bytes)?;

    let (failed, errors) = status_counts(&rows)?;
    meta.failed = failed;
    meta.errors = errors;
    meta.cases_total = failed + errors + meta.skipped + meta.passed;
    meta.lcov_files = coverage.len();
    meta.reports.sort();
    meta.reports.dedup();
    meta.data_hash = data_hash(&[&cases_bytes, &lcov_bytes]);
    write_atomic(&meta_path, &meta_bytes(&meta)?)?;
    Ok(meta)
}

/// The test-report snapshot under `<root>/.glia/test-snapshot/`, or `None`.
///
/// No snapshot directory is the normal "never ingested" case: `None`,
/// silently. A directory whose `meta.json` is absent, unreadable or of another
/// version, whose `cases.jsonl` or `lcov.jsonl` is missing, whose data does not
/// hash to `meta.data_hash`, whose rows do not parse or whose counts disagree
/// with the meta is an incomplete snapshot: `None` and one
/// `[tests] snapshot incomplete` line on stderr naming the reason.
///
/// Every case comes back sanitized ([`TestCaseRecord::sanitize`]), whatever
/// the file holds.
pub fn read_tests(root: &Path) -> Option<TestsSnapshot> {
    let dir = tests_dir(root);
    if !dir.is_dir() {
        return None;
    }
    match load_tests(&dir) {
        Ok(snapshot) => Some(snapshot),
        Err(reason) => {
            eprintln!("[tests] snapshot incomplete dir={} reason={reason}", dir.display());
            None
        }
    }
}

fn load_tests(dir: &Path) -> Result<TestsSnapshot, String> {
    let meta: TestsMeta = serde_json::from_slice(&read_required(&dir.join(META_FILE))?)
        .map_err(|e| format!("{META_FILE}: {e}"))?;
    if meta.version != TESTS_VERSION {
        return Err(format!("{META_FILE}: version {} (reader wants {TESTS_VERSION})", meta.version));
    }
    let cases_bytes = read_required(&dir.join(TESTS_CASES_FILE))?;
    let lcov_bytes = read_required(&dir.join(TESTS_LCOV_FILE))?;
    let got = data_hash(&[&cases_bytes, &lcov_bytes]);
    if got != meta.data_hash {
        return Err(format!("data_hash {got} != meta {}", meta.data_hash));
    }
    let mut cases: Vec<TestCaseRecord> = parse_jsonl(&cases_bytes, TESTS_CASES_FILE)?;
    let lcov: Vec<LcovFileRecord> = parse_jsonl(&lcov_bytes, TESTS_LCOV_FILE)?;
    let (failed, errors) = status_counts(&cases)?;
    let total = failed + errors + meta.skipped + meta.passed;
    if failed != meta.failed || errors != meta.errors || total != meta.cases_total || lcov.len() != meta.lcov_files {
        return Err(format!(
            "row counts failed={failed} errors={errors} cases_total={total} lcov_files={} != meta \
             failed={} errors={} cases_total={} lcov_files={}",
            lcov.len(),
            meta.failed,
            meta.errors,
            meta.cases_total,
            meta.lcov_files
        ));
    }
    for case in &mut cases {
        case.sanitize();
    }
    Ok(TestsSnapshot { meta, cases, lcov })
}

/// `(failed, errors)` among `cases`; any other status is an error.
fn status_counts(cases: &[TestCaseRecord]) -> Result<(usize, usize), String> {
    let (mut failed, mut errors) = (0, 0);
    for case in cases {
        match case.status.as_str() {
            STATUS_FAILED => failed += 1,
            STATUS_ERROR => errors += 1,
            other => return Err(format!("{TESTS_CASES_FILE}: case {:?} has status {other:?}", case.name)),
        }
    }
    Ok((failed, errors))
}

/// The `cases.jsonl` order: (report, suite, classname, name), then every other
/// field, so equal keys still sort one way.
fn case_order(a: &TestCaseRecord, b: &TestCaseRecord) -> std::cmp::Ordering {
    fn key(r: &TestCaseRecord) -> impl Ord + '_ {
        (
            (&r.report, &r.suite, &r.classname, &r.name),
            (&r.file, r.line, &r.status, &r.source),
            (&r.message, &r.trace, r.redacted),
        )
    }
    key(a).cmp(&key(b))
}

/// Cut `s` to at most `max` chars, on a char boundary.
fn truncate_chars(s: &mut String, max: usize) {
    if let Some((cut, _)) = s.char_indices().nth(max) {
        s.truncate(cut);
    }
}

// ===========================================================================
// Untrusted-text redaction
// ===========================================================================

/// What replaces a redacted span (A13.7's userinfo mask uses the same `***`).
pub const REDACTED: &str = "***";

/// Key names whose value must never be stored: A13.7's `.env` denylist.
/// Substring, not exact: `STRIPE_SECRET_KEY`, `JWT_TOKEN` and `DB_PASSWD` all
/// have to hit.
///
/// STOPGAP COPY. A13.7's list and helpers are private to
/// `repo-graph-code-extractors` (`parsers/code/extractors/src/config.rs`, and
/// a copy in `constants.rs`), a crate above this one that LF.6a may not edit.
/// `snapshots/tests/test_reports.rs::secret_denylist_is_a13_7s` fails on any
/// drift between the three lists. REMOVAL: make `config.rs` and `constants.rs`
/// call [`is_secret_name`] and [`mask_userinfo`] from here, delete their
/// copies and that drift test; this module is then the one implementation.
pub const SECRET_NEEDLES: [&str; 12] = [
    "SECRET", "PASSWORD", "PASSWD", "TOKEN", "APIKEY", "API_KEY", "PRIVATE_KEY",
    "CREDENTIAL", "ACCESS_KEY", "SESSION_KEY", "SALT", "SIGNING",
];

/// Whether a key's value must never be stored (A13.7).
pub fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_NEEDLES.iter().any(|needle| upper.contains(needle))
}

/// `scheme://user:pass@host/path` -> `scheme://***@host/path` (A13.7). The
/// host is kept; only userinfo carrying a `:` password is masked, and only
/// inside the authority (an `@` after the first `/`, `?` or `#` is the path's).
pub fn mask_userinfo(value: &str) -> Option<String> {
    let scheme_end = value.find("://")? + 3;
    let rest = value.get(scheme_end..)?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let at = rest.get(..authority_end)?.rfind('@')?;
    if !rest.get(..at)?.contains(':') {
        return None;
    }
    Some(format!("{}{REDACTED}{}", value.get(..scheme_end)?, rest.get(at..)?))
}

/// Token prefixes that make a value a credential by construction, each with
/// the least body length after it (a shorter run is prose, not a key).
const SECRET_PREFIXES: [(&str, usize); 24] = [
    ("sk_live_", 8), ("sk_test_", 8), ("rk_live_", 8), ("rk_test_", 8), ("whsec_", 16),
    ("ghp_", 20), ("gho_", 20), ("ghu_", 20), ("ghs_", 20), ("ghr_", 20), ("github_pat_", 20),
    ("glpat-", 16),
    ("xoxb-", 10), ("xoxp-", 10), ("xoxa-", 10), ("xoxr-", 10), ("xoxs-", 10), ("xapp-", 10),
    ("sk-", 20), ("AIza", 30), ("npm_", 30), ("pypi-", 30),
    ("AKIA", 16), ("ASIA", 16),
];

/// Redact secrets from untrusted free text (CI output, test failure
/// messages): returns the text with every secret span replaced by
/// [`REDACTED`], and how many spans were.
///
/// - PEM private-key blocks (`-----BEGIN … PRIVATE KEY-----` to its `END`
///   line) go whole;
/// - URL userinfo with a password is masked by [`mask_userinfo`], host kept;
/// - secret-shaped tokens go whole: a JWT (`eyJ….….…`) or a known credential
///   prefix ([`SECRET_PREFIXES`]: Stripe, GitHub, GitLab, Slack, AWS, Google,
///   npm, PyPI, `sk-` API keys);
/// - the credential after `Bearer ` / `Basic ` goes;
/// - the value of a key the A13.7 denylist names ([`is_secret_name`], `-` read
///   as `_`) goes, for `KEY=v`, `KEY: v`, `"key": "v"`, `key => v`, `key := v`.
///
/// Over-redaction is the safe direction: a code line in a traceback that
/// assigns to `token` loses its right-hand side. Idempotent: redacting the
/// output again finds nothing.
pub fn redact_untrusted(text: &str) -> (String, usize) {
    let (pem, n1) = redact_pem_blocks(text);
    let (urls, n2) = redact_url_userinfo(&pem);
    let (tokens, n3) = redact_tokens(&urls);
    (tokens, n1 + n2 + n3)
}

fn redact_pem_blocks(text: &str) -> (String, usize) {
    const BEGIN: &str = "-----BEGIN ";
    const END: &str = "-----END ";
    let (mut out, mut rest, mut n) = (String::with_capacity(text.len()), text, 0);
    while let Some(at) = rest.find(BEGIN) {
        let label_start = at + BEGIN.len();
        let label_end = rest[label_start..].find("-----").map(|k| label_start + k);
        let Some(label_end) = label_end.filter(|&e| rest[label_start..e].ends_with("PRIVATE KEY")) else {
            out.push_str(&rest[..label_start]);
            rest = &rest[label_start..];
            continue;
        };
        out.push_str(&rest[..at]);
        out.push_str(REDACTED);
        n += 1;
        let body = label_end + 5;
        rest = match rest[body..].find(END) {
            Some(k) => {
                let end_label = body + k + END.len();
                rest[end_label..].find("-----").map_or("", |k2| &rest[end_label + k2 + 5..])
            }
            None => "",
        };
    }
    out.push_str(rest);
    (out, n)
}

fn redact_url_userinfo(text: &str) -> (String, usize) {
    let b = text.as_bytes();
    let (mut out, mut copied, mut from, mut n) = (String::with_capacity(text.len()), 0, 0, 0);
    while let Some(k) = text[from..].find("://") {
        let at = from + k;
        let mut start = at;
        while start > copied && (b[start - 1].is_ascii_alphanumeric() || matches!(b[start - 1], b'+' | b'.' | b'-')) {
            start -= 1;
        }
        while start < at && !b[start].is_ascii_alphabetic() {
            start += 1;
        }
        let end = run(b, at + 3, |c| !c.is_ascii_whitespace() && !b"\"'<>()[]{}`,".contains(&c));
        if start < at
            && let Some(masked) = mask_userinfo(&text[start..end])
        {
            out.push_str(&text[copied..start]);
            out.push_str(&masked);
            copied = end;
            n += 1;
        }
        from = end.max(at + 3);
    }
    out.push_str(&text[copied..]);
    (out, n)
}

/// Every span below starts and ends at an ASCII byte (or the text's end), so
/// slicing at them is always on a char boundary.
fn redact_tokens(text: &str) -> (String, usize) {
    let b = text.as_bytes();
    let (mut out, mut copied, mut i, mut n) = (String::with_capacity(text.len()), 0, 0, 0);
    while i < b.len() {
        if b[i].is_ascii() && (i == 0 || !is_word_byte(b[i - 1])) {
            let span = secret_token_end(b, i)
                .map(|end| (i, end))
                .or_else(|| auth_scheme_value(b, i))
                .or_else(|| secret_assignment_value(b, i));
            if let Some((start, end)) = span {
                out.push_str(&text[copied..start]);
                out.push_str(REDACTED);
                copied = end;
                i = end;
                n += 1;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    (out, n)
}

/// The end of a secret-shaped token starting at `i`: a known prefix with a
/// long enough body, or a JWT.
fn secret_token_end(b: &[u8], i: usize) -> Option<usize> {
    for (prefix, min_body) in SECRET_PREFIXES {
        if b[i..].starts_with(prefix.as_bytes()) {
            let body = i + prefix.len();
            let end = run(b, body, is_key_byte);
            if end - body >= min_body {
                return Some(end);
            }
        }
    }
    if !b[i..].starts_with(b"eyJ") {
        return None;
    }
    let header = run(b, i, is_key_byte);
    if header - i < 10 || b.get(header) != Some(&b'.') {
        return None;
    }
    let payload = run(b, header + 1, is_key_byte);
    if payload - (header + 1) < 4 || b.get(payload) != Some(&b'.') {
        return None;
    }
    Some(run(b, payload + 1, is_key_byte))
}

/// The credential after `Bearer ` / `Basic ` at `i`.
fn auth_scheme_value(b: &[u8], i: usize) -> Option<(usize, usize)> {
    for (scheme, base64) in [("bearer", false), ("basic", true)] {
        let word_end = i + scheme.len();
        if b.len() <= word_end || !b[i..word_end].eq_ignore_ascii_case(scheme.as_bytes()) || b[word_end] != b' ' {
            continue;
        }
        let start = run(b, word_end, |c| c == b' ');
        let pred: fn(u8) -> bool = if base64 { is_base64_byte } else { is_credential_byte };
        let end = run(b, start, pred);
        let token = &b[start..end];
        // `Basic authentication` is prose: a base64 credential has a capital, digit or symbol.
        if token.len() >= 8 && (!base64 || token.iter().any(|c| !c.is_ascii_lowercase())) {
            return Some((start, end));
        }
    }
    None
}

/// The value assigned to a denylisted key starting at `i`.
fn secret_assignment_value(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if !(b[i].is_ascii_alphabetic() || b[i] == b'_') {
        return None;
    }
    let key_end = run(b, i, is_key_byte);
    let key = std::str::from_utf8(&b[i..key_end]).ok()?;
    if !is_secret_name(&key.replace('-', "_")) {
        return None;
    }
    let mut j = key_end;
    if matches!(b.get(j), Some(b'"' | b'\'')) {
        j += 1;
    }
    j = run(b, j, |c| c == b' ' || c == b'\t');
    j = match (b.get(j), b.get(j + 1)) {
        (Some(b'='), Some(b'>')) | (Some(b':'), Some(b'=')) => j + 2,
        (Some(b'='), Some(b'=' | b'~')) | (Some(b':'), Some(b':')) => return None,
        (Some(b'=' | b':'), _) => j + 1,
        _ => return None,
    };
    j = run(b, j, |c| c == b' ' || c == b'\t');
    let (start, end) = match b.get(j) {
        Some(&quote @ (b'"' | b'\'' | b'`')) => (j + 1, run(b, j + 1, |c| c != quote && c != b'\n')),
        _ => (j, run(b, j, |c| !c.is_ascii_whitespace() && !b",;&)}]<>\"'`".contains(&c))),
    };
    let value = &b[start..end];
    (!value.is_empty() && value != REDACTED.as_bytes()).then_some((start, end))
}

/// The first index at or after `from` whose byte fails `pred`.
fn run(b: &[u8], from: usize, pred: impl Fn(u8) -> bool) -> usize {
    let mut j = from.min(b.len());
    while j < b.len() && pred(b[j]) {
        j += 1;
    }
    j
}

fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn is_key_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

fn is_base64_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=')
}

fn is_credential_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'+' | b'/' | b'=' | b'~')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "glia-snapshots-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> (HistoryMeta, Vec<HistoryCommit>, Vec<BlameFile>) {
        let meta = HistoryMeta::new("ab".repeat(20), 2000, None, String::new());
        let commits = vec![
            HistoryCommit {
                c: "cd".repeat(20),
                t: 200,
                files: vec![HistoryFile {
                    p: "svc/c2.py".into(),
                    a: Some(0),
                    d: Some(0),
                    from: Some("svc/c.py".into()),
                }],
            },
            HistoryCommit {
                c: "ef".repeat(20),
                t: 100,
                files: vec![
                    HistoryFile { p: "svc/a.py".into(), a: Some(1), d: Some(0), from: None },
                    HistoryFile { p: "img.png".into(), a: None, d: None, from: None },
                ],
            },
        ];
        let blame = vec![
            BlameFile { p: "svc/b.py".into(), runs: vec![[1, 2, 100]] },
            BlameFile { p: "svc/a.py".into(), runs: vec![[1, 1, 100], [2, 3, 200]] },
        ];
        (meta, commits, blame)
    }

    #[test]
    fn data_hash_is_xxhash64_over_the_concatenation() {
        assert_eq!(data_hash(&[b"ab", b"c"]), data_hash(&[b"abc"]));
        assert_eq!(data_hash(&[b"abc", b""]), data_hash(&[b"", b"abc"]));
        assert_ne!(data_hash(&[b"abc"]), data_hash(&[b"abd"]));
        let empty = data_hash(&[]);
        assert_eq!(empty.len(), 16);
        assert!(empty.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        // xxhash64 seed 0 of the empty input.
        assert_eq!(empty, "ef46db3751d8e999");
    }

    #[test]
    fn write_then_read_round_trips_with_derived_meta_fields() {
        let root = tmp_root("roundtrip");
        let (meta, commits, blame) = sample();
        let written = write_history(&root, meta, &commits, &blame).unwrap();
        assert_eq!(written.commits, 2);
        assert_eq!(written.blame_files, 2);
        let snap = read_history(&root).expect("complete snapshot");
        assert_eq!(snap.meta, written);
        assert_eq!(snap.commits, commits);
        // blame.jsonl is sorted by path whatever order the rows came in.
        let paths: Vec<&str> = snap.blame.iter().map(|b| b.p.as_str()).collect();
        assert_eq!(paths, ["svc/a.py", "svc/b.py"]);
        // Compact rows: a rename carries `from`, other files omit it; binaries are null.
        let text = std::fs::read_to_string(history_dir(&root).join(HISTORY_COMMITS_FILE)).unwrap();
        assert!(text.contains(r#"{"p":"svc/c2.py","a":0,"d":0,"from":"svc/c.py"}"#), "{text}");
        assert!(text.contains(r#"{"p":"svc/a.py","a":1,"d":0}"#), "{text}");
        assert!(text.contains(r#"{"p":"img.png","a":null,"d":null}"#), "{text}");
        // No temp file is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(history_dir(&root))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn same_rows_write_identical_bytes() {
        let root = tmp_root("identical");
        let (meta, commits, blame) = sample();
        write_history(&root, meta.clone(), &commits, &blame).unwrap();
        let read_all = |root: &Path| {
            [HISTORY_COMMITS_FILE, HISTORY_BLAME_FILE, META_FILE]
                .map(|f| std::fs::read(history_dir(root).join(f)).unwrap())
        };
        let first = read_all(&root);
        write_history(&root, meta, &commits, &blame).unwrap();
        assert_eq!(first, read_all(&root));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn never_synced_is_none() {
        let root = tmp_root("absent");
        assert_eq!(read_history(&root), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn incomplete_snapshots_read_as_none() {
        let root = tmp_root("incomplete");
        let (meta, commits, blame) = sample();
        let dir = history_dir(&root);

        // Data edited after the meta was written: hash mismatch.
        write_history(&root, meta.clone(), &commits, &blame).unwrap();
        let path = dir.join(HISTORY_COMMITS_FILE);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert_eq!(read_history(&root), None);

        // No meta: a write that never finished.
        write_history(&root, meta.clone(), &commits, &blame).unwrap();
        std::fs::remove_file(dir.join(META_FILE)).unwrap();
        assert_eq!(read_history(&root), None);

        // Another version.
        let written = write_history(&root, meta.clone(), &commits, &blame).unwrap();
        let other = HistoryMeta { version: HISTORY_VERSION + 1, ..written };
        std::fs::write(dir.join(META_FILE), serde_json::to_vec(&other).unwrap()).unwrap();
        assert_eq!(read_history(&root), None);

        // An unknown meta field.
        write_history(&root, meta.clone(), &commits, &blame).unwrap();
        let text = std::fs::read_to_string(dir.join(META_FILE)).unwrap();
        std::fs::write(dir.join(META_FILE), text.replacen('{', r#"{"when":1,"#, 1)).unwrap();
        assert_eq!(read_history(&root), None);

        // Missing blame.jsonl reads as empty, so a blame-less snapshot survives it.
        write_history(&root, meta, &commits, &[]).unwrap();
        std::fs::remove_file(dir.join(HISTORY_BLAME_FILE)).unwrap();
        let snap = read_history(&root).expect("blame-less snapshot");
        assert!(snap.blame.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- test-report snapshot (LF.6a) ---

    fn case(report: &str, name: &str, status: &str) -> TestCaseRecord {
        TestCaseRecord {
            source: SOURCE_JUNIT.into(),
            report: report.into(),
            suite: None,
            classname: Some("tests.test_app".into()),
            name: name.into(),
            file: Some("tests/test_app.py".into()),
            line: Some(4),
            status: status.into(),
            message: Some("ValueError: boom".into()),
            trace: None,
            redacted: false,
        }
    }

    fn tests_sample() -> (TestsMeta, Vec<TestCaseRecord>, Vec<LcovFileRecord>) {
        let meta = TestsMeta::new(Some("ci-42".into()), vec!["b.xml".into(), "a.xml".into(), "a.xml".into()], 1, 5);
        let cases = vec![
            case("b.xml", "test_z", STATUS_ERROR),
            case("a.xml", "test_y", STATUS_FAILED),
            case("a.xml", "test_x", STATUS_FAILED),
        ];
        let lcov = vec![
            LcovFileRecord { sf: "/r/web/x.ts".into(), rel: Some("web/x.ts".into()), lines: vec![[1, 0]] },
            LcovFileRecord { sf: "/r/api/app.py".into(), rel: Some("api/app.py".into()), lines: vec![[1, 2], [3, 0]] },
        ];
        (meta, cases, lcov)
    }

    #[test]
    fn tests_write_then_read_round_trips_with_derived_meta() {
        let root = tmp_root("tests-roundtrip");
        let (meta, cases, lcov) = tests_sample();
        let written = write_tests(&root, meta, &cases, &lcov).unwrap();
        assert_eq!((written.failed, written.errors, written.skipped, written.passed), (2, 1, 1, 5));
        assert_eq!(written.cases_total, 9);
        assert_eq!(written.lcov_files, 2);
        assert_eq!(written.reports, ["a.xml", "b.xml"]);
        let snap = read_tests(&root).expect("complete snapshot");
        assert_eq!(snap.meta, written);
        let names: Vec<&str> = snap.cases.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["test_x", "test_y", "test_z"], "sorted by (report, suite, classname, name)");
        let rels: Vec<&str> = snap.lcov.iter().map(LcovFileRecord::key).collect();
        assert_eq!(rels, ["api/app.py", "web/x.ts"]);
        // Compact rows: absent optionals and an unredacted flag are omitted.
        let text = std::fs::read_to_string(tests_dir(&root).join(TESTS_CASES_FILE)).unwrap();
        assert!(!text.contains("null") && !text.contains("redacted") && !text.contains("trace"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tests_same_rows_write_identical_bytes_and_replace() {
        let root = tmp_root("tests-identical");
        let (meta, cases, lcov) = tests_sample();
        let read_all = |root: &Path| {
            [TESTS_CASES_FILE, TESTS_LCOV_FILE, META_FILE].map(|f| std::fs::read(tests_dir(root).join(f)).unwrap())
        };
        write_tests(&root, meta.clone(), &cases, &lcov).unwrap();
        let first = read_all(&root);
        let mut reversed = cases.clone();
        reversed.reverse();
        write_tests(&root, meta.clone(), &reversed, &lcov).unwrap();
        assert_eq!(first, read_all(&root), "input order never reaches the bytes");
        // A re-ingest replaces the snapshot: it describes one run.
        write_tests(&root, meta, &cases[..1], &[]).unwrap();
        let snap = read_tests(&root).unwrap();
        assert_eq!((snap.cases.len(), snap.lcov.len()), (1, 0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tests_write_rejects_a_passed_case() {
        let root = tmp_root("tests-status");
        let (meta, _, _) = tests_sample();
        let err = write_tests(&root, meta, &[case("a.xml", "t", "passed")], &[]).unwrap_err();
        assert!(err.contains("\"passed\""), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn incomplete_test_snapshots_read_as_none() {
        let root = tmp_root("tests-incomplete");
        let (meta, cases, lcov) = tests_sample();
        let dir = tests_dir(&root);
        assert_eq!(read_tests(&root), None, "never ingested");

        write_tests(&root, meta.clone(), &cases, &lcov).unwrap();
        let path = dir.join(TESTS_CASES_FILE);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert_eq!(read_tests(&root), None, "hash mismatch");

        write_tests(&root, meta.clone(), &cases, &lcov).unwrap();
        std::fs::remove_file(dir.join(TESTS_LCOV_FILE)).unwrap();
        assert_eq!(read_tests(&root), None, "lcov.jsonl absent");

        write_tests(&root, meta.clone(), &cases, &lcov).unwrap();
        std::fs::remove_file(dir.join(META_FILE)).unwrap();
        assert_eq!(read_tests(&root), None, "no meta");

        let written = write_tests(&root, meta, &cases, &lcov).unwrap();
        let lying = TestsMeta { passed: written.passed + 1, ..written };
        std::fs::write(dir.join(META_FILE), serde_json::to_vec(&lying).unwrap()).unwrap();
        assert_eq!(read_tests(&root), None, "cases_total disagrees with the counts");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_tests_redacts_a_hand_edited_snapshot() {
        let root = tmp_root("tests-hand-edited");
        let dir = tests_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        let row = r#"{"source":"log","report":"ci.log","name":"test_pay","status":"failed","message":"got sk_live_4eC39HqLyjWDarjtT1zdp7dc"}"#;
        let cases_bytes = format!("{row}\n").into_bytes();
        std::fs::write(dir.join(TESTS_CASES_FILE), &cases_bytes).unwrap();
        std::fs::write(dir.join(TESTS_LCOV_FILE), b"").unwrap();
        let mut meta = TestsMeta::new(None, vec!["ci.log".into()], 0, 0);
        meta.failed = 1;
        meta.cases_total = 1;
        meta.data_hash = data_hash(&[&cases_bytes, b""]);
        std::fs::write(dir.join(META_FILE), serde_json::to_vec(&meta).unwrap()).unwrap();
        let snap = read_tests(&root).expect("hand-written snapshot");
        assert_eq!(snap.cases[0].message.as_deref(), Some("got ***"));
        assert!(snap.cases[0].redacted);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sanitize_caps_on_char_boundaries_after_redacting() {
        let mut c = case("a.xml", "t", STATUS_FAILED);
        c.message = Some("é".repeat(MESSAGE_CAP + 50));
        c.trace = Some(format!("{}\nAuthorization: Bearer abcdef0123456789", "漢".repeat(TRACE_CAP)));
        assert_eq!(c.sanitize(), 1, "the bearer credential past the cap is still one span");
        assert_eq!(c.message.as_deref().map(|m| m.chars().count()), Some(MESSAGE_CAP));
        assert_eq!(c.trace.as_deref().map(|t| t.chars().count()), Some(TRACE_CAP));
        assert!(c.redacted);
        let again = c.clone();
        assert_eq!(c.sanitize(), 0, "idempotent");
        assert_eq!(c, again);
    }

    #[test]
    fn redaction_removes_secret_shapes() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        // Assembled at run time so no scanner mistakes this source for a leak.
        let github = format!("gh {}_0123456789abcdefghijABCDEFGHIJ", "ghp");
        let pem = format!("a\n-----BEGIN {0} PRIVATE KEY-----\nMIIE\n-----END {0} PRIVATE KEY-----\nb", "RSA");
        let bare_jwt = format!("token was {jwt}!");
        let cases = [
            ("charge failed for sk_live_4eC39HqLyjWDarjtT1zdp7dc", "charge failed for ***"),
            (&bare_jwt as &str, "token was ***!"),
            ("Authorization: Bearer 9f8e7d6c5b4a3210", "Authorization: Bearer ***"),
            ("Authorization: Basic dXNlcjpwYXNz", "Authorization: Basic ***"),
            ("STRIPE_SECRET_KEY=abc123 next", "STRIPE_SECRET_KEY=*** next"),
            (r#"{"password": "hunter2", "user": "bob"}"#, r#"{"password": "***", "user": "bob"}"#),
            ("x-api-key: k-123, other", "x-api-key: ***, other"),
            ("db_passwd => 'pw'", "db_passwd => '***'"),
            ("url postgres://app:s3cr3t@db.internal:5432/x ok", "url postgres://***@db.internal:5432/x ok"),
            ("key AKIAIOSFODNN7EXAMPLE used", "key *** used"),
            (&github as &str, "gh ***"),
            (&pem as &str, "a\n***\nb"),
        ];
        for (input, want) in cases {
            let (got, n) = redact_untrusted(input);
            assert_eq!(got, want, "input {input:?}");
            assert!(n >= 1, "input {input:?}");
            assert_eq!(redact_untrusted(&got), (got.clone(), 0), "idempotent on {input:?}");
        }
    }

    #[test]
    fn redaction_keeps_locations_and_prose() {
        for keep in [
            "token_test.go:12: expected 200, got 500",
            "at com.x.TokenService.refresh(TokenService.java:42)",
            "File \"app/secrets.py\", line 3, in get_secret",
            "Basic authentication failed",
            "assert token == expected",
            "task-queue sk-short",
            "https://github.com/acme/api/blob/main/a.py",
            "naïve café — 日本語 password",
        ] {
            assert_eq!(redact_untrusted(keep), (keep.to_string(), 0), "{keep:?}");
        }
    }
}
