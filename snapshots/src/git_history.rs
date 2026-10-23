//! Capture a repo's git history for the history snapshot (LF.5a).
//!
//! Runs the local `git` binary against the repo root and parses its output:
//! - `git log -z --numstat -M --no-merges --relative --format=%x1e%H%x1f%ct`
//!   gives, per commit, `\x1e<sha>\x1f<unix>` then `\0\n`, then one
//!   `<a>\t<d>\t<path>\0` per file; a rename is `<a>\t<d>\t\0<old>\0<new>\0`
//!   and a binary file's counts are `-\t-`;
//! - `git blame --porcelain -w <head> -- <path>` (only when blame is asked for)
//!   gives, per final line, a `<sha> <orig> <final> [<n>]` header, the commit's
//!   key lines the first time a sha appears, and the line itself after a TAB;
//! - when messages are kept (EB.1, the default; `--no-messages` turns all of
//!   this off), `git log -z --no-merges --format=%x1e%H%x1f%at%x1f%B` over the
//!   same window gives each commit's author time and raw message, and
//!   `git log --merges --format=%H%x1f%P%x1f%s` plus one
//!   `git rev-list --no-merges <second parent> ^<first parent>` per labelled
//!   merge gives the branch each commit came in on (rank_eval's
//!   `branch_labels` / `merge_label`).
//!
//! Shas, committer and author times, paths, line counts, and commit messages
//! and merge subjects are read. Messages go only into `messages.jsonl`, never
//! into the build's inputs: [`clean_message_counted`] drops the trailer lines
//! (`Co-Authored-By:`, `Signed-off-by:` .. and a final git trailer block, the
//! identity-bearing lines), and every subject, body and branch label passes
//! `redact_untrusted`. Author and committer names and emails and file
//! contents are never read: the formats name no identity placeholder, and
//! every porcelain key line but `committer-time` is skipped unread.
//! Parsing splits on delimiters only (never slices by a computed index) and
//! converts paths with `from_utf8_lossy`, so odd bytes degrade, never panic.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Stdio};

use glia_code_domain::snapshots::{
    BlameFile, HistoryCommit, HistoryFile, HistoryMessage, HistoryMeta, clean_message_counted, redact_untrusted,
};
use serde::Serialize;

/// Options for [`crate::history_sync`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryOptions {
    /// `git log -n`: the newest this many non-merge commits (default 2000).
    pub max_commits: usize,
    /// `git log --since=<since>`, passed through verbatim (default none).
    pub since: Option<String>,
    /// Also blame the most-changed files (default false: blame is the slow part).
    pub blame: bool,
    /// How many files to blame when `blame` is on (default 300).
    pub blame_max_files: usize,
    /// Keep each commit's message, author time and merged-branch label in
    /// `messages.jsonl` (default true, EB.1); false writes no sidecar.
    pub messages: bool,
    /// Which surface called the sync (`lib`, `cli`, `pyo3`); the marker names it.
    pub surface: &'static str,
}

impl Default for HistoryOptions {
    fn default() -> Self {
        Self { max_commits: 2000, since: None, blame: false, blame_max_files: 300, messages: true, surface: "lib" }
    }
}

/// What a history sync captured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistorySummary {
    /// Full sha of the commit the history was read from.
    pub head: String,
    /// Commits captured.
    pub commits: usize,
    /// Distinct files touched, a renamed file counted once (under its newest path).
    pub files: usize,
    /// File entries git detected as renames.
    pub renames: usize,
    /// File entries git reported as binary (no line counts).
    pub binary: usize,
    /// Files blamed.
    pub blame_files: usize,
    /// Blame runs across those files.
    pub runs: usize,
    /// Rows of `messages.jsonl` (one per commit); 0 with messages off.
    pub messages: usize,
    /// Secret spans `redact_untrusted` replaced in subjects, bodies and labels.
    pub redacted: usize,
    /// Trailer lines [`clean_message_counted`] dropped.
    pub trailers_dropped: usize,
    /// Commits that carry a merged-branch label.
    pub branch_labels: usize,
}

/// Everything one sync read, before it is written.
pub(crate) struct Capture {
    pub(crate) meta: HistoryMeta,
    pub(crate) commits: Vec<HistoryCommit>,
    pub(crate) blame: Vec<BlameFile>,
    /// One row per commit, in commit order; `None` with messages off.
    pub(crate) messages: Option<Vec<HistoryMessage>>,
    pub(crate) redacted: usize,
    pub(crate) trailers_dropped: usize,
}

/// Environment variables that would point git at another repository than the
/// `-C <root>` it is given (a sync run from inside a git hook inherits them).
const REPO_OVERRIDE_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// Read the history of the repo at `root`.
pub(crate) fn capture(root: &Path, opts: &HistoryOptions) -> Result<Capture, String> {
    if opts.max_commits == 0 {
        return Err("max_commits must be at least 1".to_string());
    }
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let prefix = run_git(root, &["rev-parse", "--show-prefix"])
        .map_err(|e| format!("{} is not inside a git work tree: {e}", root.display()))?;
    let relative_to = first_line(&prefix);
    let head = run_git(root, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .map_err(|_| format!("{} has no commits: HEAD does not name a commit", root.display()))?;
    let head = first_line(&head);
    if !is_sha(head.as_bytes()) {
        return Err(format!("git rev-parse HEAD printed {head:?}, not a commit sha"));
    }

    let window = Window::new(opts, &head, !relative_to.is_empty());
    let log = run_git(
        root,
        &window.args(&[
            "log",
            "-z",
            "--numstat",
            "-M",
            "--no-merges",
            "--relative",
            "--no-color",
            "--no-ext-diff",
            "--format=%x1e%H%x1f%ct",
        ]),
    )?;
    let commits = parse_log(&log)?;
    let blame = if opts.blame { blame_top_files(root, &head, &commits, opts.blame_max_files)? } else { Vec::new() };
    let (messages, redacted, trailers_dropped) = if opts.messages {
        let (rows, redacted, trailers) = capture_messages(root, &window, &commits)?;
        (Some(rows), redacted, trailers)
    } else {
        (None, 0, 0)
    };
    let meta = HistoryMeta::new(head, opts.max_commits, opts.since.clone(), relative_to);
    Ok(Capture { meta, commits, blame, messages, redacted, trailers_dropped })
}

/// The window arguments every log of one sync shares: `--max-count`, the
/// optional `--since`, the resolved head sha and the pathspec rule.
struct Window {
    max_count: String,
    since: Option<String>,
    head: String,
    below_top: bool,
}

impl Window {
    fn new(opts: &HistoryOptions, head: &str, below_top: bool) -> Self {
        Self {
            max_count: format!("--max-count={}", opts.max_commits),
            since: opts.since.as_ref().map(|s| format!("--since={s}")),
            head: head.to_string(),
            below_top,
        }
    }

    /// `lead` then the window: `--max-count`, `--since`, the head sha, and
    /// `-- .` below the top level.
    fn args<'a>(&'a self, lead: &[&'a str]) -> Vec<&'a str> {
        let mut args: Vec<&str> = lead.to_vec();
        args.push(&self.max_count);
        if let Some(since) = &self.since {
            args.push(since);
        }
        // The resolved sha, not `HEAD`: the log, the blame and meta.head name
        // one commit even if HEAD moves mid-sync.
        args.push(&self.head);
        if self.below_top {
            // A root below the git top level: only commits that touch it.
            // Without the pathspec, `--relative` still lists every commit
            // (those touching only other directories with an empty file list),
            // and they would eat the `-n` window. At the top level the
            // pathspec would only turn on history simplification, so it is
            // left off there.
            args.extend(["--", "."]);
        }
        args
    }
}

/// Each commit's message row (EB.1), in commit order, plus the redaction and
/// trailer counts. A commit the message log did not list gets an empty row
/// and is counted on a `[history] messages unmatched=<n>` line.
fn capture_messages(
    root: &Path,
    window: &Window,
    commits: &[HistoryCommit],
) -> Result<(Vec<HistoryMessage>, usize, usize), String> {
    let log = run_git(root, &window.args(&["log", "-z", "--no-merges", "--no-color", "--format=%x1e%H%x1f%at%x1f%B"]))?;
    let raw = parse_message_log(&log)?;
    let (labels, label_redactions) = branch_labels(root, window);
    let (mut redacted, mut trailers, mut unmatched) = (label_redactions, 0, 0);
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let br = labels.get(&commit.c).map(|(_, label)| label.clone());
        let Some((at, message)) = raw.get(&commit.c) else {
            unmatched += 1;
            rows.push(HistoryMessage { c: commit.c.clone(), at: 0, s: String::new(), b: String::new(), br });
            continue;
        };
        let (s, b, dropped, n) = clean_message_counted(message);
        trailers += dropped;
        redacted += n;
        rows.push(HistoryMessage { c: commit.c.clone(), at: *at, s, b, br });
    }
    if unmatched > 0 {
        eprintln!("[history] messages unmatched={unmatched} of {}", commits.len());
    }
    Ok((rows, redacted, trailers))
}

/// Parse `git log -z --format=%x1e%H%x1f%at%x1f%B`: sha -> (author time, raw
/// message). Split on the record and field delimiters only.
pub(crate) fn parse_message_log(bytes: &[u8]) -> Result<BTreeMap<String, (i64, String)>, String> {
    let mut records = bytes.split(|b| *b == 0x1e);
    let lead = records.next().unwrap_or_default();
    if !lead.iter().all(u8::is_ascii_whitespace) {
        return Err(format!(
            "git log (messages): unexpected output before the first commit record: {:?}",
            lossy(&lead[..lead.len().min(80)])
        ));
    }
    let mut out = BTreeMap::new();
    for record in records {
        let mut fields = record.splitn(3, |b| *b == 0x1f);
        let sha = fields.next().unwrap_or_default();
        let time = fields.next().unwrap_or_default();
        let Some(message) = fields.next() else {
            return Err(format!("git log (messages): malformed record {:?}", lossy(&record[..record.len().min(80)])));
        };
        if !is_sha(sha) {
            return Err(format!("git log (messages): malformed commit sha {:?}", lossy(sha)));
        }
        let c = lossy(sha);
        let at: i64 = lossy(time)
            .trim()
            .parse()
            .map_err(|_| format!("git log (messages): commit {c} has author time {:?}", lossy(time)))?;
        // `-z` ends each record with a NUL.
        let mut message = message;
        while let Some(rest) = message.strip_suffix(b"\0") {
            message = rest;
        }
        out.insert(c, (at, lossy(message)));
    }
    Ok(out)
}

/// sha -> (side size, branch label): rank_eval's `branch_labels`. For each
/// merge of the window (log order) whose subject [`merge_label`] names, every
/// commit `git rev-list --no-merges <second parent> ^<first parent>` lists
/// takes the label when that side is strictly smaller than the one it holds,
/// so a lane merged into a wave branch keeps the lane, and the first merge
/// wins a tie. A merge whose side git cannot list (a parent object missing in
/// a shallow clone) is skipped and counted on a `[history] branch labels
/// skipped merges=<n>` line. Labels are redacted; the second value is how
/// many spans were.
fn branch_labels(root: &Path, window: &Window) -> (BTreeMap<String, (usize, String)>, usize) {
    let mut best: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let (mut redacted, mut skipped) = (0, 0);
    let log = match run_git(root, &window.args(&["log", "--merges", "--no-color", "--format=%H%x1f%P%x1f%s"])) {
        Ok(log) => log,
        Err(e) => {
            eprintln!("[history] branch labels skipped reason={e}");
            return (best, 0);
        }
    };
    for line in lossy(&log).lines() {
        let mut p = line.split('\x1f');
        let (Some(_), Some(ps), Some(subj)) = (p.next(), p.next(), p.next()) else { continue };
        let ps: Vec<&str> = ps.split_whitespace().collect();
        if ps.len() < 2 {
            continue;
        }
        let Some(label) = merge_label(subj) else { continue };
        let side = match run_git(root, &["rev-list", "--no-merges", ps[1], &format!("^{}", ps[0])]) {
            Ok(side) => lossy(&side),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let (label, n) = redact_untrusted(&label);
        redacted += n;
        let shas: Vec<&str> = side.lines().collect();
        let n = shas.len();
        for s in shas {
            let e = best.entry(s.to_string()).or_insert((usize::MAX, String::new()));
            if n < e.0 {
                *e = (n, label.clone());
            }
        }
    }
    if skipped > 0 {
        eprintln!("[history] branch labels skipped merges={skipped} reason=a parent object is missing");
    }
    (best, redacted)
}

/// The branch a merge commit's subject names (None for a subtree import of
/// another repository): rank_eval's `merge_label`, verbatim.
pub(crate) fn merge_label(subj: &str) -> Option<String> {
    if subj.starts_with("Merge commit '") && subj.contains("' as '") {
        return None;
    }
    if let Some(r) = subj.strip_prefix("Merge pull request #") {
        let num: String = r.chars().take_while(char::is_ascii_digit).collect();
        return Some(format!("pr#{num}"));
    }
    for pre in ["Merge branch '", "Merge remote-tracking branch '"] {
        if let Some(r) = subj.strip_prefix(pre) {
            return r.split('\'').next().map(str::to_string);
        }
    }
    let r = subj.strip_prefix("Merge ")?;
    let (head, into) = match r.rsplit_once(" into ") {
        Some((h, i)) => (h, Some(i.trim().trim_matches('\''))),
        None => (r, None),
    };
    let head = head.split(" (").next().unwrap_or(head).trim().trim_matches('\'');
    if head.is_empty() {
        return None;
    }
    Some(match into {
        Some(i) if !matches!(i, "main" | "master") => format!("{head}@{i}"),
        _ => head.to_string(),
    })
}

/// The counts a sync reports. `files` follows renames newest to oldest, the
/// way the build attributes history: a file's older paths fold into its newest.
pub(crate) fn summarize(cap: &Capture) -> HistorySummary {
    let mut alias: BTreeMap<&str, &str> = BTreeMap::new();
    let mut files: BTreeSet<&str> = BTreeSet::new();
    let (mut renames, mut binary) = (0, 0);
    for commit in &cap.commits {
        for file in &commit.files {
            let current = alias.get(file.p.as_str()).copied().unwrap_or(file.p.as_str());
            files.insert(current);
            if let Some(old) = file.from.as_deref() {
                renames += 1;
                alias.insert(old, current);
            }
            if file.a.is_none() || file.d.is_none() {
                binary += 1;
            }
        }
    }
    HistorySummary {
        head: cap.meta.head.clone(),
        commits: cap.commits.len(),
        files: files.len(),
        renames,
        binary,
        blame_files: cap.blame.len(),
        runs: cap.blame.iter().map(|b| b.runs.len()).sum(),
        messages: cap.messages.as_ref().map_or(0, Vec::len),
        redacted: cap.redacted,
        trailers_dropped: cap.trailers_dropped,
        branch_labels: cap.messages.as_ref().map_or(0, |m| m.iter().filter(|r| r.br.is_some()).count()),
    }
}

/// Run `git -C <root> <args>`; its stdout, or an error carrying git's stderr.
fn run_git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off", "-c", "log.showSignature=false"])
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null());
    for var in REPO_OVERRIDE_ENV {
        cmd.env_remove(var);
    }
    let out = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("the git binary was not found on PATH ({e}); history sync reads the repo with the local git")
        } else {
            format!("could not run git: {e}")
        }
    })?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let sub = args.first().copied().unwrap_or("");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        Err(format!("`git {sub}` failed ({})", out.status))
    } else {
        Err(format!("`git {sub}` failed ({}): {stderr}", out.status))
    }
}

/// The first line of a git answer, without its line ending.
fn first_line(bytes: &[u8]) -> String {
    let line = bytes.split(|b| *b == b'\n').next().unwrap_or_default();
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8_lossy(line).into_owned()
}

/// A full SHA-1 or SHA-256 object name.
fn is_sha(s: &[u8]) -> bool {
    (s.len() == 40 || s.len() == 64) && s.iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Parse `git log -z --numstat -M --format=%x1e%H%x1f%ct` output.
pub(crate) fn parse_log(bytes: &[u8]) -> Result<Vec<HistoryCommit>, String> {
    let mut records = bytes.split(|b| *b == 0x1e);
    let lead = records.next().unwrap_or_default();
    if !lead.iter().all(u8::is_ascii_whitespace) {
        return Err(format!(
            "git log: unexpected output before the first commit record: {:?}",
            lossy(&lead[..lead.len().min(80)])
        ));
    }
    let mut commits = Vec::new();
    for record in records {
        let header_len = record.iter().position(|b| *b == 0 || *b == b'\n').unwrap_or(record.len());
        let (header, rest) = record.split_at(header_len);
        let mut fields = header.split(|b| *b == 0x1f);
        let sha = fields.next().unwrap_or_default();
        let time = fields.next().unwrap_or_default();
        if !is_sha(sha) || fields.next().is_some() {
            return Err(format!("git log: malformed commit header {:?}", lossy(header)));
        }
        let c = lossy(sha);
        let t: i64 = lossy(time)
            .trim()
            .parse()
            .map_err(|_| format!("git log: commit {c} has committer time {:?}", lossy(time)))?;

        let mut files = Vec::new();
        let mut tokens = rest.split(|b| *b == 0);
        while let Some(token) = tokens.next() {
            let stat = trim_leading_newlines(token);
            if stat.is_empty() {
                continue;
            }
            let mut parts = stat.splitn(3, |b| *b == b'\t');
            let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
                return Err(format!("git log: commit {c}: unparseable numstat entry {:?}", lossy(stat)));
            };
            let a = parse_count(added).ok_or_else(|| format!("git log: commit {c}: bad added count {:?}", lossy(added)))?;
            let d = parse_count(deleted).ok_or_else(|| format!("git log: commit {c}: bad deleted count {:?}", lossy(deleted)))?;
            if path.is_empty() {
                // A rename: the source and destination follow as their own tokens.
                let (Some(old), Some(new)) = (tokens.next(), tokens.next()) else {
                    return Err(format!("git log: commit {c}: rename entry without both paths"));
                };
                if old.is_empty() || new.is_empty() {
                    return Err(format!("git log: commit {c}: rename entry with an empty path"));
                }
                files.push(HistoryFile { p: lossy(new), a, d, from: Some(lossy(old)) });
            } else {
                files.push(HistoryFile { p: lossy(path), a, d, from: None });
            }
        }
        commits.push(HistoryCommit { c, t, files });
    }
    Ok(commits)
}

fn trim_leading_newlines(mut token: &[u8]) -> &[u8] {
    while let Some(rest) = token.strip_prefix(b"\n") {
        token = rest;
    }
    token
}

/// A `--numstat` count: `Some(n)` (saturating at `u32::MAX`), `Some(None)` for
/// a binary file's `-`, `None` when it is neither.
fn parse_count(field: &[u8]) -> Option<Option<u32>> {
    if field == b"-" {
        return Some(None);
    }
    if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n = field.iter().fold(0u64, |n, b| n.saturating_mul(10).saturating_add(u64::from(b - b'0')));
    Some(Some(u32::try_from(n).unwrap_or(u32::MAX)))
}

/// Blame the `max_files` most-changed text files that still exist at `head`,
/// ranked by commit count descending then path. A file git cannot blame is
/// skipped with a `[history] blame skipped` warning. Rows sorted by path.
fn blame_top_files(
    root: &Path,
    head: &str,
    commits: &[HistoryCommit],
    max_files: usize,
) -> Result<Vec<BlameFile>, String> {
    let mut touched: BTreeMap<&str, usize> = BTreeMap::new();
    let mut binary_now: BTreeMap<&str, bool> = BTreeMap::new();
    for commit in commits {
        for file in &commit.files {
            *touched.entry(file.p.as_str()).or_default() += 1;
            // Commits run newest first, so the first entry is the file's latest state.
            binary_now.entry(file.p.as_str()).or_insert(file.a.is_none() || file.d.is_none());
        }
    }
    let at_head = blobs_at(root, head)?;
    let mut ranked: Vec<(&str, usize)> = touched
        .into_iter()
        .filter(|(p, _)| at_head.contains(*p) && !binary_now.get(p).copied().unwrap_or(false))
        .collect();
    ranked.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(y.0)));
    ranked.truncate(max_files);

    let mut out = Vec::with_capacity(ranked.len());
    for (path, _) in ranked {
        let blamed = run_git(root, &["blame", "--porcelain", "-w", head, "--", path]).and_then(|b| parse_blame(&b));
        match blamed {
            Ok(runs) => out.push(BlameFile { p: path.to_string(), runs }),
            Err(e) => eprintln!("[history] blame skipped path={path} reason={e}"),
        }
    }
    out.sort_by(|x, y| x.p.cmp(&y.p));
    Ok(out)
}

/// Regular files (mode 100644 / 100755) in `head`'s tree below the root, by
/// path relative to the root.
fn blobs_at(root: &Path, head: &str) -> Result<BTreeSet<String>, String> {
    let listing = run_git(root, &["ls-tree", "-r", "-z", head])?;
    let mut blobs = BTreeSet::new();
    for entry in listing.split(|b| *b == 0) {
        let mut halves = entry.splitn(2, |b| *b == b'\t');
        let (Some(info), Some(path)) = (halves.next(), halves.next()) else { continue };
        let mut info = info.split(|b| *b == b' ');
        let (mode, kind) = (info.next().unwrap_or_default(), info.next().unwrap_or_default());
        if kind == b"blob" && (mode == b"100644" || mode == b"100755") {
            blobs.insert(lossy(path));
        }
    }
    Ok(blobs)
}

/// Parse `git blame --porcelain` into `[start, end, committer_time]` runs.
pub(crate) fn parse_blame(bytes: &[u8]) -> Result<Vec<[i64; 3]>, String> {
    let mut time_of: BTreeMap<&[u8], i64> = BTreeMap::new();
    let mut lines: Vec<(i64, &[u8])> = Vec::new();
    let mut current: Option<&[u8]> = None;
    for line in bytes.split(|b| *b == b'\n') {
        // The blamed line's content, and the blank tail: never read.
        if line.is_empty() || line.first() == Some(&b'\t') {
            continue;
        }
        if let Some(value) = line.strip_prefix(b"committer-time ") {
            let t: i64 = lossy(value)
                .trim()
                .parse()
                .map_err(|_| format!("blame: bad committer-time {:?}", lossy(value)))?;
            if let Some(sha) = current {
                time_of.entry(sha).or_insert(t);
            }
            continue;
        }
        let mut fields = line.split(|b| *b == b' ');
        let sha = fields.next().unwrap_or_default();
        if !is_sha(sha) {
            // author, author-mail, committer, committer-mail, *-tz, summary,
            // previous, filename, boundary: skipped unread.
            continue;
        }
        let _orig = fields.next();
        let final_line: i64 = fields
            .next()
            .and_then(|f| lossy(f).parse().ok())
            .ok_or_else(|| format!("blame: malformed line header {:?}", lossy(line)))?;
        lines.push((final_line, sha));
        current = Some(sha);
    }
    lines.sort_by_key(|(n, _)| *n);
    let mut runs: Vec<[i64; 3]> = Vec::new();
    for (n, sha) in lines {
        let t = *time_of
            .get(sha)
            .ok_or_else(|| format!("blame: no committer-time for commit {}", lossy(sha)))?;
        match runs.last_mut() {
            Some(run) if run[1] + 1 == n && run[2] == t => run[1] = n,
            _ => runs.push([n, n, t]),
        }
    }
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA_A: &str = "4f08640b4e60adac1d8b13cb0bc6ac7959de1498";
    const SHA_B: &str = "580e6cfe53355ea8f1e11247f7ed5484f8d8c12e";
    const SHA_C: &str = "158f1bc7092ca345693cd686f51518c4f0e0907d";

    fn file(p: &str, a: Option<u32>, d: Option<u32>, from: Option<&str>) -> HistoryFile {
        HistoryFile { p: p.into(), a, d, from: from.map(str::to_string) }
    }

    #[test]
    fn parses_renames_binaries_and_empty_commits() {
        let mut raw = Vec::new();
        raw.extend_from_slice(format!("\x1e{SHA_A}\x1f1767571200\0\n0\t0\t\0svc/c.py\0svc/c2.py\0").as_bytes());
        raw.extend_from_slice(format!("\x1e{SHA_B}\x1f1767484800\0").as_bytes());
        raw.extend_from_slice(format!("\x1e{SHA_C}\x1f1767225600\0\n2\t0\tsvc/a.py\0-\t-\timg\tx.png\0").as_bytes());
        let commits = parse_log(&raw).unwrap();
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].c, SHA_A);
        assert_eq!(commits[0].t, 1767571200);
        assert_eq!(commits[0].files, [file("svc/c2.py", Some(0), Some(0), Some("svc/c.py"))]);
        assert!(commits[1].files.is_empty(), "an empty commit keeps its record");
        // A tab inside a path survives: only the first two tabs split.
        assert_eq!(
            commits[2].files,
            [file("svc/a.py", Some(2), Some(0), None), file("img\tx.png", None, None, None)]
        );
    }

    #[test]
    fn odd_bytes_degrade_and_malformed_output_errors_without_panicking() {
        let raw = [format!("\x1e{SHA_A}\x1f1\0\n1\t0\t").into_bytes(), b"caf\xe9.py\0".to_vec()].concat();
        let commits = parse_log(&raw).unwrap();
        assert_eq!(commits[0].files[0].p, "caf\u{fffd}.py");
        assert_eq!(parse_log(b"").unwrap(), Vec::<HistoryCommit>::new());
        for bad in [
            "gpg: Signature made\n\x1e".to_string(),
            "\x1enot-a-sha\x1f1\0".to_string(),
            format!("\x1e{SHA_A}\x1fsoon\0"),
            format!("\x1e{SHA_A}\x1f1\0\nx\ty\tp\0"),
            format!("\x1e{SHA_A}\x1f1\0\n1\t0\t\0only-old\0"),
            format!("\x1e{SHA_A}\x1f1\0\nnotabs\0"),
        ] {
            assert!(parse_log(bad.as_bytes()).is_err(), "{bad:?}");
        }
        assert_eq!(parse_count(b"99999999999"), Some(Some(u32::MAX)));
    }

    #[test]
    fn blame_collapses_runs_and_never_reads_identity_lines() {
        let porcelain = format!(
            "{SHA_C} 1 1 2\nauthor Some Body\nauthor-mail <some@body.invalid>\nauthor-time 5\n\
             committer Some Body\ncommitter-mail <some@body.invalid>\ncommitter-time 100\n\
             summary {SHA_B} 1 1 looks like a header\nboundary\nfilename a.py\n\tdef f():\n\
             {SHA_C} 2 2\n\tcommitter-time 7\n\
             {SHA_B} 3 3 2\nauthor X\ncommitter-time 200\nsummary edit\nprevious {SHA_C} a.py\nfilename a.py\n\tdef g(x):\n\
             {SHA_B} 4 4\n\t    return x\n\
             {SHA_C} 5 5 1\n\t# tail\n"
        );
        let runs = parse_blame(porcelain.as_bytes()).unwrap();
        assert_eq!(runs, [[1, 2, 100], [3, 4, 200], [5, 5, 100]]);
        assert!(parse_blame(format!("{SHA_A} 1 1 1\n\tx\n").as_bytes()).is_err());
    }

    #[test]
    fn merge_label_rules() {
        for (subject, want) in [
            ("Merge pull request #123 from org/feature-y", Some("pr#123")),
            ("Merge branch 'feat-x'", Some("feat-x")),
            ("Merge branch 'feat-x' into main", Some("feat-x")),
            ("Merge branch 'lane' into wave", Some("lane")),
            ("Merge remote-tracking branch 'origin/fix-z'", Some("origin/fix-z")),
            ("Merge lane-3 into wave-2", Some("lane-3@wave-2")),
            ("Merge lane-3 into main", Some("lane-3")),
            ("Merge lane-3 into 'master'", Some("lane-3")),
            ("Merge lane-3 (squashed) into dev", Some("lane-3@dev")),
            ("Merge feature-q", Some("feature-q")),
            ("Merge commit 'abc123' as 'vendor/lib'", None),
            ("Merge ", None),
            ("Update readme", None),
        ] {
            assert_eq!(merge_label(subject).as_deref(), want, "{subject:?}");
        }
    }

    #[test]
    fn message_log_parses_records_and_degrades_without_panicking() {
        let raw = format!("\x1e{SHA_A}\x1f1700\x1fsubj\n\nbody\n\0\x1e{SHA_B}\x1f1600\x1f\0");
        let got = parse_message_log(raw.as_bytes()).unwrap();
        assert_eq!(got.get(SHA_A), Some(&(1700, "subj\n\nbody\n".to_string())));
        assert_eq!(got.get(SHA_B), Some(&(1600, String::new())));
        let odd = [format!("\x1e{SHA_A}\x1f1\x1fcaf").into_bytes(), b"\xe9\0".to_vec()].concat();
        assert_eq!(parse_message_log(&odd).unwrap()[SHA_A].1, "caf\u{fffd}");
        for bad in ["junk\x1e".to_string(), format!("\x1e{SHA_A}\x1f1"), "\x1enope\x1f1\x1fm".to_string(), format!("\x1e{SHA_A}\x1fx\x1fm")] {
            assert!(parse_message_log(bad.as_bytes()).is_err(), "{bad:?}");
        }
    }
}
