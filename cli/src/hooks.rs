//! Git hooks: `glia install-hooks` drops `.git/hooks/{post-commit,
//! post-merge,post-checkout}` so the target repo's `.gmap` rebuilds on each
//! change, and `HooksCmd` is the flattened slot for hook-side subcommands
//! (LG.2 adds its hidden `hook` command there, not on `Cmd`).

use std::path::Path;

use clap::Subcommand;

const HOOK_NAMES: &[&str] = &["post-commit", "post-merge", "post-checkout"];
const HOOK_MARKER: &str = "# glia-install-hooks: managed";

#[derive(clap::Args, Debug)]
pub(crate) struct InstallArgs {
    /// Path to the repo (must contain a `.git` dir).
    #[arg(default_value = ".")]
    repo: String,
    /// Uninstall instead of install.
    #[arg(long)]
    uninstall: bool,
    /// Command to run from each hook (defaults to `glia build .`).
    /// Use this to point at a non-default `glia` binary or pass extra
    /// flags like `--out path/to/out`.
    #[arg(long)]
    command: Option<String>,
}

// Hook-side subcommands, flattened into `Cmd` after `install-hooks`. A new
// one adds a variant here and an arm in `dispatch`; the variant's doc
// comment is its help text.
#[derive(Subcommand, Debug)]
pub(crate) enum HooksCmd {}

pub(crate) fn dispatch(c: HooksCmd) -> i32 {
    match c {}
}

/// `glia install-hooks`.
pub(crate) fn run(args: InstallArgs) -> i32 {
    let repo = args.repo.as_str();
    let uninstall = args.uninstall;
    let command = args.command.as_deref();
    let repo_path = Path::new(repo);
    let git_dir = repo_path.join(".git");
    if !git_dir.exists() {
        eprintln!("error: no .git directory at {}", repo_path.display());
        return 1;
    }
    let hooks_dir = if git_dir.is_dir() {
        git_dir.join("hooks")
    } else {
        // Worktree case: `.git` is a file pointing at the real gitdir.
        match resolve_gitdir_file(&git_dir) {
            Some(p) => p.join("hooks"),
            None => {
                eprintln!("error: cannot resolve gitdir from {}", git_dir.display());
                return 1;
            }
        }
    };
    if let Err(e) = std::fs::create_dir_all(&hooks_dir) {
        eprintln!("error creating {}: {e}", hooks_dir.display());
        return 4;
    }

    let cmd = command.unwrap_or("glia build .").to_string();
    let mut written = 0;
    let mut removed = 0;
    let mut skipped = 0;

    for hook in HOOK_NAMES {
        let hook_path = hooks_dir.join(hook);
        if uninstall {
            if remove_glia_hook(&hook_path) {
                removed += 1;
            }
            continue;
        }
        // If a non-glia hook already exists, refuse to clobber.
        if hook_path.exists() && !is_glia_managed(&hook_path) {
            eprintln!(
                "skipping {}: existing hook is not glia-managed (preserve user content)",
                hook_path.display()
            );
            skipped += 1;
            continue;
        }
        let body = render_hook_script(hook, &cmd);
        if let Err(e) = std::fs::write(&hook_path, body) {
            eprintln!("error writing {}: {e}", hook_path.display());
            return 5;
        }
        // chmod +x — ignore failure on platforms without unix perms.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &hook_path,
                std::fs::Permissions::from_mode(0o755),
            );
        }
        written += 1;
    }

    if uninstall {
        eprintln!("removed {removed} glia-managed hook(s) from {}", hooks_dir.display());
    } else {
        eprintln!(
            "installed {written} hook(s) into {} (skipped {skipped} non-managed)",
            hooks_dir.display()
        );
        eprintln!("hook command: {cmd}");
        eprintln!();
        eprintln!("note: rebuild latency scales with repo size. To uninstall:");
        eprintln!("  glia install-hooks {} --uninstall", repo);
    }
    0
}

fn render_hook_script(hook_name: &str, cmd: &str) -> String {
    format!(
        r#"#!/bin/sh
{HOOK_MARKER}
# Managed by `glia install-hooks`. Re-run on changes to keep .gmap fresh.
# Hook: {hook_name}
# Edit `--command` and re-run install-hooks to change. Remove with `--uninstall`.

{cmd}
"#
    )
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
        eprintln!(
            "skipping {}: not glia-managed",
            path.display()
        );
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

/// Read a `.git` file (worktree case) and extract the `gitdir:` path.
fn resolve_gitdir_file(git_file: &Path) -> Option<std::path::PathBuf> {
    let content = std::fs::read_to_string(git_file).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("gitdir:") {
            let p = std::path::PathBuf::from(rest.trim());
            if p.is_absolute() {
                return Some(p);
            }
            return git_file.parent().map(|parent| parent.join(p));
        }
    }
    None
}
