//! glia-snapshots — external-input snapshot writers.
//!
//! History (and, later, test reports) is an external, HEAD-dependent input, so
//! it enters glia the way Confluence docs do: a separate snapshot step writes
//! files under `<repo>/.glia/`, and the deterministic build reads them back
//! through `repo_graph_code_domain::snapshots`. The build never runs git; this
//! crate is the only glia code that spawns a process, and only when a sync is
//! called.
//!
//! [`history_sync`] reads the local git (no fetch, no remote, no other repo)
//! and writes `<repo>/.glia/history-snapshot/`. Author and committer names and
//! emails are never captured.

mod git_history;

use std::io::Write as _;
use std::path::Path;

use repo_graph_code_domain::snapshots::write_history;
use repo_graph_code_domain::walk_gating::CONTROL_DIR;

pub use git_history::{HistoryOptions, HistorySummary};

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
    repo_graph_code_domain::project_roots::project_name(&canonical)
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
    fn gitignore_lists_only_regenerable_inputs() {
        let lines: Vec<&str> = GLIA_GITIGNORE.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(lines, ["vectors.jsonl", "docs-snapshot/", "history-snapshot/", "test-snapshot/"]);
    }
}
