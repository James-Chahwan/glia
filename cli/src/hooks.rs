//! Git hooks.
//!
//! `glia install-hooks <repo>` writes the REBUILD hooks (`post-commit`,
//! `post-merge`, `post-checkout`; each runs `glia build .` or `--command`) so
//! every change keeps `<repo>/.glia/graph`, the layout the MCP reads, fresh.
//! They go where git reads hooks from — `git rev-parse --git-path hooks` —
//! which honours `core.hooksPath`
//! and, for a linked worktree, is the common dir's `hooks/`, shared by every
//! worktree of that repo (the old `<gitdir>/hooks` of a worktree is never run).
//!
//! `--pair <sibling>` adds the G8 / u151 branch-pair lock (PAIR hooks):
//! - `pre-commit` → `glia hook pre-commit --pair <sibling>`: a commit passes
//!   iff the sibling's CHECKED-OUT branch equals ours. Checked out rather than
//!   merely existing, because the consumer builds against the sibling's
//!   working tree through path deps: pinning a sha from a branch that is not
//!   checked out would record code that was never compiled. Blocks with exit 1.
//! - `commit-msg` → `glia hook commit-msg --pair <sibling> <msg-file>`: writes
//!   `Glia-Pinned-At: <sibling HEAD>` into the message, replacing an existing
//!   one so `--amend` keeps exactly one, current pin. Never blocks. The pin
//!   cannot live in `pre-commit` (git passes it no arguments, so it never sees
//!   the message) nor in `prepare-commit-msg` (it runs before the editor, and a
//!   trailer there turns git's empty-message abort into a trailer-only commit).
//!
//! The sibling path is relative to the repo's top-level (u151's `../glia`).
//! Escape hatches: `git commit --no-verify` skips both hooks;
//! `GLIA_BRANCH_PAIR=skip` skips the check only (the pin is still written).
//! Both scripts fail open when `glia` is not on PATH: a missing tool is an
//! environment problem, not a branch mismatch. Needs git >= 2.13.
//!
//! `HooksCmd` is the flattened slot for the hidden `glia hook …` entry points
//! those scripts call; it lists after `install-hooks` in `Cmd`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Subcommand;

/// Hooks that keep `<repo>/.glia/graph` fresh; installed on every run.
const REBUILD_HOOKS: &[&str] = &["post-commit", "post-merge", "post-checkout"];
/// The branch-pair lock; installed only with `--pair`.
const PAIR_HOOKS: &[&str] = &["pre-commit", "commit-msg"];
/// First comment line of every script glia writes. Unchanged since the first
/// release, so existing installs stay recognised (and removable).
const HOOK_MARKER: &str = "# glia-install-hooks: managed";
/// The commit-message trailer `commit-msg` pins the sibling's HEAD under.
const TRAILER: &str = "Glia-Pinned-At";
/// `GLIA_BRANCH_PAIR=skip` skips the branch check (not the pin).
const SKIP_ENV: &str = "GLIA_BRANCH_PAIR";
/// What the rebuild hooks run when `--command` is not given.
const DEFAULT_REBUILD_COMMAND: &str = "glia build .";

/// What `git rev-parse --local-env-vars` lists (git 2.55): the variables that
/// pin a git command to one repository. Git exports `GIT_INDEX_FILE` to every
/// commit hook, and an absolute `GIT_DIR` too in a linked worktree, so a
/// `git -C <sibling>` run from a hook would otherwise read OUR repo — and a
/// mismatch would pass as a match. Every command aimed at a named directory
/// clears them.
const REPO_LOCAL_ENV: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

#[derive(clap::Args, Debug)]
pub(crate) struct InstallArgs {
    /// Path to the repo (any path inside its work tree).
    #[arg(default_value = ".")]
    repo: String,
    /// Uninstall instead of install: removes every glia-managed hook, rebuild
    /// and branch-pair alike; hooks glia did not write are left alone.
    #[arg(long)]
    uninstall: bool,
    /// Command to run from each rebuild hook (defaults to `glia build .`).
    /// Use this to point at a non-default `glia` binary or pass extra
    /// flags like `--out path/to/out`. Does not affect the branch-pair hooks.
    #[arg(long)]
    command: Option<String>,
    /// Also install the cross-repo branch-pair lock (G8 / u151) against this
    /// sibling repo, relative to the repo's top-level (e.g. `../glia`):
    /// `pre-commit` blocks a commit unless the sibling has the same branch
    /// checked out, and `commit-msg` pins `Glia-Pinned-At: <sibling HEAD>`.
    #[arg(long)]
    pair: Option<String>,
}

// Hook-side subcommands, flattened into `Cmd` after `install-hooks`. A new
// one adds a variant here and an arm in `dispatch`; the variant's doc
// comment is its help text.
#[derive(Subcommand, Debug)]
pub(crate) enum HooksCmd {
    /// Entry points for the hook scripts `install-hooks --pair` writes; git
    /// runs these, people do not.
    #[command(hide = true)]
    Hook {
        #[command(subcommand)]
        action: HookCmd,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum HookCmd {
    /// pre-commit: block unless <PAIR> has this repo's branch checked out.
    PreCommit {
        /// The sibling repo, relative to this repo's top-level.
        #[arg(long)]
        pair: String,
    },
    /// commit-msg: pin `Glia-Pinned-At: <PAIR's HEAD>` into <MSG_FILE>.
    CommitMsg {
        /// The message file git passes the hook.
        msg_file: String,
        /// The sibling repo, relative to this repo's top-level.
        #[arg(long)]
        pair: String,
    },
}

pub(crate) fn dispatch(c: HooksCmd) -> i32 {
    match c {
        HooksCmd::Hook {
            action: HookCmd::PreCommit { pair },
        } => run_pre_commit(&pair),
        HooksCmd::Hook {
            action: HookCmd::CommitMsg { msg_file, pair },
        } => run_commit_msg(&msg_file, &pair),
    }
}

// ── git plumbing ────────────────────────────────────────────────────────────

/// Trimmed stdout of a successful `cmd`; `None` when git fails or cannot run.
fn stdout_of(mut cmd: Command) -> Option<String> {
    let out = cmd.stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git -C <dir> <args>`, with the repo-local environment cleared so `dir`,
/// not an inherited `GIT_DIR`, decides the repository.
fn git_in(dir: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    for var in REPO_LOCAL_ENV {
        cmd.env_remove(var);
    }
    // `status` must not take the sibling's index lock from inside our commit.
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    stdout_of(cmd)
}

/// `git <args>` against the repo the hook runs in, environment inherited:
/// inside a hook git has already pointed it at the right repo and index.
fn git_here(args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    stdout_of(cmd)
}

/// `git <args>` in the hook's repo with `input` on stdin.
fn git_here_stdin(args: &[&str], input: &[u8]) -> Option<String> {
    let mut child = Command::new("git")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Take stdin so it is closed (EOF) before we wait.
    let mut stdin = child.stdin.take()?;
    let wrote = stdin.write_all(input).is_ok();
    drop(stdin);
    let out = child.wait_with_output().ok()?;
    if !wrote || !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A `git rev-parse` path answer made usable from our cwd: git prints paths
/// relative to the `-C` directory.
fn from_dir(dir: &Path, printed: String) -> PathBuf {
    let p = PathBuf::from(printed);
    if p.is_absolute() { p } else { dir.join(p) }
}

/// The directory git reads `repo`'s hooks from: `core.hooksPath` when set,
/// else the COMMON dir's `hooks/` (one per repo, shared by its worktrees).
fn hooks_dir(repo: &Path) -> Result<PathBuf, String> {
    if let Some(out) = git_in(repo, &["rev-parse", "--git-path", "hooks"]) {
        return Ok(from_dir(repo, out));
    }
    // git not runnable: read `.git` ourselves (core.hooksPath unknowable).
    let dot_git = repo.join(".git");
    if dot_git.is_dir() {
        return Ok(dot_git.join("hooks"));
    }
    if dot_git.is_file() {
        let gitdir = resolve_gitdir_file(&dot_git)
            .ok_or_else(|| format!("cannot resolve gitdir from {}", dot_git.display()))?;
        return Ok(common_dir(&gitdir).join("hooks"));
    }
    Err(format!("no .git directory at {}", repo.display()))
}

/// Read a `.git` file (worktree / submodule case) and extract the `gitdir:` path.
fn resolve_gitdir_file(git_file: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(git_file).ok()?;
    let rest = content.lines().find_map(|l| l.strip_prefix("gitdir:"))?;
    let p = PathBuf::from(rest.trim());
    if p.is_absolute() {
        return Some(p);
    }
    git_file.parent().map(|parent| parent.join(p))
}

/// A linked worktree's gitdir names the shared repo dir in its `commondir`
/// file; hooks live there, not in the per-worktree gitdir.
fn common_dir(gitdir: &Path) -> PathBuf {
    match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(s) if !s.trim().is_empty() => {
            let p = PathBuf::from(s.trim());
            if p.is_absolute() { p } else { gitdir.join(p) }
        }
        _ => gitdir.to_path_buf(),
    }
}

/// The work tree's top-level, which the `--pair` path is relative to.
fn toplevel_of(repo: &Path) -> PathBuf {
    git_in(repo, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.to_path_buf())
}

/// Inside a hook: the top-level of the repo being committed to.
fn hook_toplevel() -> PathBuf {
    git_here(&["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn is_git_repo(dir: &Path) -> bool {
    git_in(dir, &["rev-parse", "--git-dir"]).is_some()
}

// ── the branch-pair verdict ─────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// The sibling has our branch checked out; `pair_sha` is its HEAD (`None`
    /// while that branch has no commits).
    Match {
        branch: String,
        pair_sha: Option<String>,
    },
    /// The sibling has no branch of our name.
    Missing { branch: String },
    /// The sibling has our branch, but another one checked out.
    OtherBranch { branch: String, pair_branch: String },
    /// The sibling has our branch, but a detached HEAD.
    PairDetached { branch: String },
    /// We are on a detached HEAD (rebase / bisect in progress): no branch to pair.
    OursDetached,
    /// The sibling path is not a git repository.
    NoPairRepo,
}

/// Pure: our branch (`None` = detached), the sibling's checked-out branch
/// (`None` = detached), whether the sibling has a branch of our name, whether
/// the sibling is a repo at all, and its HEAD sha.
fn verdict(
    ours: Option<&str>,
    pair_head_branch: Option<&str>,
    pair_has_branch: bool,
    pair_ok: bool,
    pair_sha: Option<&str>,
) -> Verdict {
    let Some(branch) = ours else {
        return Verdict::OursDetached;
    };
    let branch = branch.to_string();
    if !pair_ok {
        return Verdict::NoPairRepo;
    }
    match pair_head_branch {
        Some(p) if p == branch => Verdict::Match {
            branch,
            pair_sha: pair_sha.map(str::to_string),
        },
        _ if !pair_has_branch => Verdict::Missing { branch },
        Some(p) => Verdict::OtherBranch {
            branch,
            pair_branch: p.to_string(),
        },
        None => Verdict::PairDetached { branch },
    }
}

/// The stderr lines for a verdict and the hook's exit code. `pair` is the
/// sibling path as installed (e.g. `../glia`), so every command it prints can
/// be pasted at the repo's top-level.
fn render(v: &Verdict, pair: &str) -> (Vec<String>, i32) {
    const BLOCKED: &str = "[hooks] branch-pair BLOCKED:";
    let bypass = format!(
        "[hooks] bypass once with `git commit --no-verify`, or {SKIP_ENV}=skip to skip only the check"
    );
    match v {
        Verdict::Match { branch, pair_sha } => {
            let at = pair_sha
                .as_deref()
                .map(|s| s.chars().take(7).collect::<String>())
                .unwrap_or_else(|| "(no commits)".to_string());
            (
                vec![format!("[hooks] branch-pair ok: {branch} == {pair} @ {at}")],
                0,
            )
        }
        Verdict::Missing { branch } => (
            vec![
                format!(
                    "{BLOCKED} glia branch `{branch}` missing - `git -C {pair} checkout -b {branch}` first"
                ),
                bypass,
            ],
            1,
        ),
        Verdict::OtherBranch {
            branch,
            pair_branch,
        } => (
            vec![
                format!(
                    "{BLOCKED} {pair} is on `{pair_branch}`, not `{branch}` - `git -C {pair} checkout {branch}`"
                ),
                bypass,
            ],
            1,
        ),
        Verdict::PairDetached { branch } => (
            vec![
                format!(
                    "{BLOCKED} {pair} is on a detached HEAD, not `{branch}` - `git -C {pair} checkout {branch}`"
                ),
                bypass,
            ],
            1,
        ),
        Verdict::OursDetached => (
            vec!["[hooks] branch-pair skipped: detached HEAD".to_string()],
            0,
        ),
        Verdict::NoPairRepo => (
            vec![format!(
                "{BLOCKED} {pair} is not a git repository (install-hooks --pair)"
            )],
            1,
        ),
    }
}

/// `glia hook pre-commit --pair <pair>`.
fn run_pre_commit(pair: &str) -> i32 {
    if std::env::var(SKIP_ENV).as_deref() == Ok("skip") {
        eprintln!("[hooks] branch-pair skipped ({SKIP_ENV}=skip)");
        return 0;
    }
    let pair_dir = hook_toplevel().join(pair);
    let ours = git_here(&["symbolic-ref", "-q", "--short", "HEAD"]);
    let pair_ok = is_git_repo(&pair_dir);
    let (pair_head, pair_has, pair_sha) = match (&ours, pair_ok) {
        (Some(branch), true) => (
            git_in(&pair_dir, &["symbolic-ref", "-q", "--short", "HEAD"]),
            git_in(
                &pair_dir,
                &[
                    "rev-parse",
                    "-q",
                    "--verify",
                    &format!("refs/heads/{branch}"),
                ],
            )
            .is_some(),
            git_in(&pair_dir, &["rev-parse", "-q", "--verify", "HEAD"]),
        ),
        _ => (None, false, None),
    };
    let v = verdict(
        ours.as_deref(),
        pair_head.as_deref(),
        pair_has,
        pair_ok,
        pair_sha.as_deref(),
    );
    let (lines, code) = render(&v, pair);
    for line in lines {
        eprintln!("{line}");
    }
    if matches!(v, Verdict::Match { .. })
        && git_in(&pair_dir, &["status", "--porcelain"]).is_some_and(|s| !s.is_empty())
    {
        eprintln!(
            "[hooks] branch-pair: {pair} has uncommitted changes; the pin records HEAD, not the working tree"
        );
    }
    code
}

/// `glia hook commit-msg --pair <pair> <msg-file>`. Never blocks: a pin that
/// cannot be written is a warning, and the commit proceeds.
fn run_commit_msg(msg_file: &str, pair: &str) -> i32 {
    let msg = match std::fs::read(msg_file) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[hooks] warning: cannot read {msg_file}: {e} - {TRAILER} not written");
            return 0;
        }
    };
    // Comments aside, an empty message must reach git still empty, so git
    // aborts the commit exactly as it would without this hook.
    match git_here_stdin(&["stripspace", "--strip-comments"], &msg) {
        Some(s) if s.trim().is_empty() => return 0,
        Some(_) => {}
        None => {
            eprintln!("[hooks] warning: `git stripspace` failed - {TRAILER} not written");
            return 0;
        }
    }
    let pair_dir = hook_toplevel().join(pair);
    let Some(sha) = git_in(&pair_dir, &["rev-parse", "-q", "--verify", "HEAD"]) else {
        eprintln!("[hooks] warning: cannot resolve {pair} HEAD - {TRAILER} not written");
        return 0;
    };
    let trailer = format!("{TRAILER}: {sha}");
    let replaced = git_here(&[
        "interpret-trailers",
        "--in-place",
        "--if-exists",
        "replace",
        "--trailer",
        &trailer,
        msg_file,
    ]);
    if replaced.is_none() {
        eprintln!("[hooks] warning: `git interpret-trailers` failed - {TRAILER} not written");
        return 0;
    }
    eprintln!("[hooks] pinned {trailer}");
    0
}

// ── install / uninstall ─────────────────────────────────────────────────────

/// `glia install-hooks`.
pub(crate) fn run(args: InstallArgs) -> i32 {
    let repo = args.repo.as_str();
    let repo_path = Path::new(repo);
    let hooks = match hooks_dir(repo_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    if args.uninstall {
        let mut removed = 0;
        for hook in REBUILD_HOOKS.iter().chain(PAIR_HOOKS) {
            if remove_glia_hook(&hooks.join(hook)) {
                removed += 1;
            }
        }
        eprintln!(
            "removed {removed} glia-managed hook(s) from {}",
            hooks.display()
        );
        return 0;
    }

    let top = toplevel_of(repo_path);
    if let Some(p) = args.pair.as_deref() {
        let resolved = top.join(p);
        if !is_git_repo(&resolved) {
            eprintln!(
                "error: --pair {p}: {} is not a git repository (the path is relative to {})",
                resolved.display(),
                top.display()
            );
            return 2;
        }
    }

    if let Err(e) = std::fs::create_dir_all(&hooks) {
        eprintln!("error creating {}: {e}", hooks.display());
        return 4;
    }
    let hooks = hooks.canonicalize().unwrap_or(hooks);

    let cmd = args.command.as_deref().unwrap_or(DEFAULT_REBUILD_COMMAND);
    let mut plan: Vec<(&str, String)> = REBUILD_HOOKS
        .iter()
        .map(|h| (*h, rebuild_script(h, cmd)))
        .collect();
    if let Some(p) = args.pair.as_deref() {
        plan.extend(PAIR_HOOKS.iter().map(|h| (*h, pair_script(h, p))));
    }

    let mut written = 0;
    let mut skipped = 0;
    for (hook, body) in plan {
        let hook_path = hooks.join(hook);
        // If a non-glia hook already exists, refuse to clobber.
        if hook_path.exists() && !is_glia_managed(&hook_path) {
            eprintln!(
                "skipping {}: existing hook is not glia-managed (preserve user content)",
                hook_path.display()
            );
            skipped += 1;
            continue;
        }
        if let Err(e) = std::fs::write(&hook_path, body) {
            eprintln!("error writing {}: {e}", hook_path.display());
            return 5;
        }
        // chmod +x — ignore failure on platforms without unix perms.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755));
        }
        written += 1;
    }

    eprintln!(
        "[hooks] installed {written} hook(s) into {} (skipped {skipped} non-managed, branch-pair={})",
        hooks.display(),
        args.pair.as_deref().unwrap_or("off")
    );
    if args.pair.is_none() {
        let kept: Vec<&str> = PAIR_HOOKS
            .iter()
            .copied()
            .filter(|h| is_glia_managed(&hooks.join(h)))
            .collect();
        if !kept.is_empty() {
            eprintln!(
                "[hooks] note: branch-pair hook(s) from an earlier --pair install left in place ({}); `--uninstall` removes them",
                kept.join(", ")
            );
        }
    }
    if hooks_in_work_tree(repo_path, &top, &hooks) {
        eprintln!(
            "[hooks] note: {} is inside the work tree (core.hooksPath) - the hooks are visible to git status",
            hooks.display()
        );
    }
    eprintln!("hook command: {cmd}");
    eprintln!();
    eprintln!("note: rebuild latency scales with repo size. To uninstall:");
    eprintln!("  glia install-hooks {} --uninstall", repo);
    0
}

/// `core.hooksPath` can name a tracked directory (`.githooks/`, `.husky/`):
/// true when `hooks` sits under the work tree but outside the git dir.
fn hooks_in_work_tree(repo: &Path, top: &Path, hooks: &Path) -> bool {
    let Ok(top) = top.canonicalize() else {
        return false;
    };
    let common = git_in(repo, &["rev-parse", "--git-common-dir"])
        .map(|c| from_dir(repo, c))
        .and_then(|c| c.canonicalize().ok());
    hooks.starts_with(&top) && !common.is_some_and(|c| hooks.starts_with(c))
}

fn rebuild_script(hook_name: &str, cmd: &str) -> String {
    format!(
        r#"#!/bin/sh
{HOOK_MARKER}
# Managed by `glia install-hooks`. Re-run on changes to keep <repo>/.glia/graph fresh.
# Hook: {hook_name}
# Edit `--command` and re-run install-hooks to change. Remove with `--uninstall`.

{cmd}
"#
    )
}

/// The branch-pair scripts: fail open without `glia`, else hand over to the
/// hidden `glia hook` entry point.
fn pair_script(hook_name: &str, pair: &str) -> String {
    let quoted = sh_quote(pair);
    let (skipped, exec) = if hook_name == "commit-msg" {
        (
            format!("{TRAILER} not written"),
            format!("exec glia hook commit-msg --pair {quoted} \"$1\""),
        )
    } else {
        (
            "branch-pair check skipped".to_string(),
            format!("exec glia hook pre-commit --pair {quoted}"),
        )
    };
    format!(
        r#"#!/bin/sh
{HOOK_MARKER}
# Managed by `glia install-hooks --pair`: the cross-repo branch-pair lock (G8 / u151).
# Hook: {hook_name}
# Bypass once: `git commit --no-verify`, or {SKIP_ENV}=skip (check only). Remove with `--uninstall`.

command -v glia >/dev/null 2>&1 || {{ echo '[hooks] glia not on PATH - {skipped}' >&2; exit 0; }}
{exec}
"#
    )
}

/// POSIX single-quoting: `'` inside becomes `'\''`.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn is_glia_managed(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|s| s.contains(HOOK_MARKER))
        .unwrap_or(false)
}

fn remove_glia_hook(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    if !is_glia_managed(path) {
        eprintln!("skipping {}: not glia-managed", path.display());
        return false;
    }
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("error removing {}: {e}", path.display());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(branch: &str, sha: Option<&str>) -> Verdict {
        Verdict::Match {
            branch: branch.to_string(),
            pair_sha: sha.map(str::to_string),
        }
    }

    #[test]
    fn verdict_covers_all_six_outcomes() {
        let sha = "48bac86f00000000000000000000000000000000";
        assert_eq!(
            verdict(Some("feat-x"), Some("feat-x"), true, true, Some(sha)),
            m("feat-x", Some(sha))
        );
        assert_eq!(
            verdict(Some("feat-x"), Some("main"), false, true, Some(sha)),
            Verdict::Missing {
                branch: "feat-x".into()
            }
        );
        assert_eq!(
            verdict(Some("feat-x"), Some("main"), true, true, Some(sha)),
            Verdict::OtherBranch {
                branch: "feat-x".into(),
                pair_branch: "main".into()
            }
        );
        assert_eq!(
            verdict(Some("feat-x"), None, true, true, Some(sha)),
            Verdict::PairDetached {
                branch: "feat-x".into()
            }
        );
        assert_eq!(
            verdict(None, Some("feat-x"), true, true, Some(sha)),
            Verdict::OursDetached
        );
        assert_eq!(
            verdict(Some("feat-x"), None, false, false, None),
            Verdict::NoPairRepo
        );
    }

    #[test]
    fn verdict_edge_cases() {
        // A detached sibling without our branch needs `checkout -b`, not `checkout`.
        assert_eq!(
            verdict(Some("feat-x"), None, false, true, Some("abc")),
            Verdict::Missing {
                branch: "feat-x".into()
            }
        );
        // Our branch checked out but unborn in the sibling: a match with no sha.
        assert_eq!(
            verdict(Some("feat-x"), Some("feat-x"), false, true, None),
            m("feat-x", None)
        );
        // Detached wins over a missing sibling: rebase / bisect is never blocked.
        assert_eq!(
            verdict(None, None, false, false, None),
            Verdict::OursDetached
        );
    }

    #[test]
    fn render_prints_the_u151_messages() {
        let (ok, code) = render(&m("feat-x", Some("48bac86f0000")), "../glia");
        assert_eq!(
            (ok, code),
            (
                vec!["[hooks] branch-pair ok: feat-x == ../glia @ 48bac86".to_string()],
                0
            )
        );
        let (unborn, _) = render(&m("feat-x", None), "../glia");
        assert_eq!(
            unborn,
            ["[hooks] branch-pair ok: feat-x == ../glia @ (no commits)"]
        );
        let (missing, code) = render(
            &Verdict::Missing {
                branch: "feat-x".into(),
            },
            "../glia",
        );
        assert_eq!(code, 1);
        assert_eq!(
            missing[0],
            "[hooks] branch-pair BLOCKED: glia branch `feat-x` missing - `git -C ../glia checkout -b feat-x` first"
        );
        let (other, code) = render(
            &Verdict::OtherBranch {
                branch: "feat-x".into(),
                pair_branch: "main".into(),
            },
            "../glia",
        );
        assert_eq!(code, 1);
        assert_eq!(
            other[0],
            "[hooks] branch-pair BLOCKED: ../glia is on `main`, not `feat-x` - `git -C ../glia checkout feat-x`"
        );
        let (detached, code) = render(
            &Verdict::PairDetached {
                branch: "feat-x".into(),
            },
            "../glia",
        );
        assert_eq!(code, 1);
        assert!(
            detached[0].contains("../glia is on a detached HEAD"),
            "{detached:?}"
        );
        assert_eq!(
            render(&Verdict::OursDetached, "../glia"),
            (
                vec!["[hooks] branch-pair skipped: detached HEAD".to_string()],
                0
            )
        );
        assert_eq!(
            render(&Verdict::NoPairRepo, "../glia"),
            (
                vec!["[hooks] branch-pair BLOCKED: ../glia is not a git repository (install-hooks --pair)".to_string()],
                1
            )
        );
    }

    #[test]
    fn sh_quote_escapes_quotes_and_keeps_spaces() {
        assert_eq!(sh_quote("../glia"), "'../glia'");
        assert_eq!(sh_quote("../it's my glia"), r"'../it'\''s my glia'");
    }

    #[test]
    fn pair_scripts_are_managed_and_fail_open() {
        let pre = pair_script("pre-commit", "../it's glia");
        assert!(
            pre.starts_with(&format!("#!/bin/sh\n{HOOK_MARKER}\n")),
            "{pre}"
        );
        assert!(pre.contains("command -v glia >/dev/null 2>&1 || { echo '[hooks] glia not on PATH - branch-pair check skipped' >&2; exit 0; }"), "{pre}");
        assert!(
            pre.ends_with("exec glia hook pre-commit --pair '../it'\\''s glia'\n"),
            "{pre}"
        );
        let msg = pair_script("commit-msg", "../glia");
        assert!(
            msg.ends_with("exec glia hook commit-msg --pair '../glia' \"$1\"\n"),
            "{msg}"
        );
        assert!(
            msg.contains("glia not on PATH - Glia-Pinned-At not written"),
            "{msg}"
        );
    }

    #[test]
    fn common_dir_follows_a_worktree_to_the_shared_repo() {
        let dir = std::env::temp_dir().join(format!("glia-lg2-commondir-{}", std::process::id()));
        let wt_gitdir = dir.join(".git").join("worktrees").join("wt");
        std::fs::create_dir_all(&wt_gitdir).expect("gitdir");
        assert_eq!(common_dir(&wt_gitdir), wt_gitdir);
        std::fs::write(wt_gitdir.join("commondir"), "../..\n").expect("commondir");
        assert_eq!(common_dir(&wt_gitdir), wt_gitdir.join("../.."));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
