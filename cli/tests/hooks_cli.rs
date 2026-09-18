//! LG.2 — `glia install-hooks` writes where git actually reads hooks (linked
//! worktrees, `core.hooksPath`), and `--pair <sibling>` adds the G8 / u151
//! branch-pair lock: a `pre-commit` that blocks unless the sibling repo has
//! our branch CHECKED OUT, and a `commit-msg` that pins
//! `Glia-Pinned-At: <sibling HEAD>` into the message.
//!
//! Every test builds throwaway repos under the system temp dir and drives real
//! git with the developer's config shut out (`GIT_CONFIG_GLOBAL=/dev/null`,
//! `GIT_CONFIG_NOSYSTEM=1`, fixed author / committer) and `PATH` led by the
//! directory of the glia under test, so the hooks' `glia` is this binary.
//! `[hooks] ` stderr lines are relayed, so the fired_on marker is grep-able:
//! `cargo test -p glia-cli --test hooks_cli -- --nocapture 2>&1 | grep -o '\[hooks\] branch-pair ok: .*'`
//! (`-o`, not `^`: libtest's progress text can precede a relayed line).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch dir, created fresh and removed on drop (`cli` has no
/// dev-dependencies, so no `tempfile`). Canonical, so it compares equal to the
/// absolute paths git prints.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glia-lg2-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir.canonicalize().expect("canonical scratch dir"))
    }

    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn glia_bin_dir() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_glia"))
        .parent()
        .expect("glia binary has a parent dir")
        .to_path_buf()
}

fn inherited_path() -> Vec<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect()
}

/// The glia under test first, then the inherited PATH.
fn path_with_glia() -> OsString {
    let mut dirs = vec![glia_bin_dir()];
    dirs.extend(inherited_path());
    std::env::join_paths(dirs).expect("PATH joins")
}

/// The inherited PATH minus every dir holding a `glia` — a developer's own
/// install included — so `command -v glia` really fails inside the hook.
fn path_without_glia() -> OsString {
    let dirs: Vec<PathBuf> = inherited_path()
        .into_iter()
        .filter(|d| !d.join("glia").exists() && d != &glia_bin_dir())
        .collect();
    std::env::join_paths(dirs).expect("PATH joins")
}

fn ceiling() -> PathBuf {
    let tmp = std::env::temp_dir();
    tmp.canonicalize().unwrap_or(tmp)
}

/// `program` in `cwd` with the isolated environment every test uses.
fn isolated(program: &str, cwd: &Path) -> Command {
    let mut c = Command::new(program);
    c.current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "glia test")
        .env("GIT_AUTHOR_EMAIL", "test@glia.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_NAME", "glia test")
        .env("GIT_COMMITTER_EMAIL", "test@glia.invalid")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_EDITOR", "true")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("PATH", path_with_glia())
        // Scratch repos sit under the temp dir: never discover a repo above it.
        .env("GIT_CEILING_DIRECTORIES", ceiling())
        .env_remove("GLIA_BRANCH_PAIR");
    // A cargo test launched from inside a git hook must not aim these at the
    // outer repo.
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
    ] {
        c.env_remove(var);
    }
    c
}

fn relay(out: &Output) {
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[hooks] ") {
            eprintln!("{line}");
        }
    }
}

fn run(mut c: Command) -> Output {
    let out = c.output().expect("process spawns");
    relay(&out);
    out
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn git(cwd: &Path, args: &[&str]) -> Output {
    let mut c = isolated("git", cwd);
    c.args(args);
    run(c)
}

/// git that must succeed; returns trimmed stdout.
fn git_ok(cwd: &Path, args: &[&str]) -> String {
    let out = git(cwd, args);
    assert!(
        out.status.success(),
        "git {args:?} in {} exited {:?}\nstderr:\n{}",
        cwd.display(),
        out.status,
        stderr(&out)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn glia(cwd: &Path, args: &[&str]) -> Output {
    let mut c = isolated(env!("CARGO_BIN_EXE_glia"), cwd);
    c.args(args);
    run(c)
}

fn glia_ok(cwd: &Path, args: &[&str]) -> String {
    let out = glia(cwd, args);
    assert!(
        out.status.success(),
        "glia {args:?} exited {:?}\nstderr:\n{}",
        out.status,
        stderr(&out)
    );
    stderr(&out)
}

/// `git init -b main <name>` under `root` with one commit; returns its path.
fn init_repo(root: &Path, name: &str) -> PathBuf {
    git_ok(root, &["init", "-q", "-b", "main", name]);
    let dir = root.join(name);
    std::fs::write(dir.join("README"), name).expect("write README");
    git_ok(&dir, &["add", "README"]);
    git_ok(&dir, &["commit", "-q", "-m", "init"]);
    dir
}

/// Stage one new change in `dir` so the next `git commit` has something to do.
fn stage(dir: &Path, file: &str) {
    std::fs::write(dir.join(file), file).expect("write staged file");
    git_ok(dir, &["add", file]);
}

/// `np` (the consumer, on `feat-x`) and its sibling `gl`, on `gl_branch`.
/// `gl_has_feat_x` creates `feat-x` in gl without checking it out.
fn pair_repos(s: &Scratch, gl_branch: &str, gl_has_feat_x: bool) -> (PathBuf, PathBuf) {
    let np = init_repo(s.root(), "np");
    git_ok(&np, &["checkout", "-q", "-b", "feat-x"]);
    let gl = init_repo(s.root(), "gl");
    if gl_has_feat_x {
        git_ok(&gl, &["branch", "feat-x"]);
    }
    if gl_branch != "main" {
        git_ok(&gl, &["checkout", "-q", gl_branch]);
    }
    glia_ok(
        s.root(),
        &[
            "install-hooks",
            "np",
            "--pair",
            "../gl",
            "--command",
            "true",
        ],
    );
    (np, gl)
}

fn pins(np: &Path) -> Vec<String> {
    git_ok(
        np,
        &[
            "log",
            "-1",
            "--format=%(trailers:key=Glia-Pinned-At,valueonly)",
        ],
    )
    .lines()
    .map(str::to_string)
    .filter(|l| !l.is_empty())
    .collect()
}

fn marker_fired(marker: &Path) -> bool {
    std::fs::read_to_string(marker)
        .map(|s| s.contains("fired"))
        .unwrap_or(false)
}

#[test]
fn install_fires_in_a_linked_worktree() {
    let s = Scratch::new("worktree");
    let main = init_repo(s.root(), "m");
    git_ok(&main, &["worktree", "add", "-q", "../wt"]);
    let marker = s.root().join("marker");
    let command = format!("echo fired >> {}", marker.display());
    let err = glia_ok(s.root(), &["install-hooks", "wt", "--command", &command]);
    let wt = s.root().join("wt");
    stage(&wt, "a");
    git_ok(&wt, &["commit", "-q", "-m", "in the worktree"]);
    assert!(
        marker_fired(&marker),
        "post-commit never ran in the worktree\n{err}"
    );
    // Shared by every worktree of the repo, as git defines.
    let hooks = main.join(".git").join("hooks");
    assert!(
        err.contains(&format!(
            "[hooks] installed 3 hook(s) into {} (skipped 0 non-managed, branch-pair=off)",
            hooks.display()
        )),
        "{err}"
    );
}

#[test]
fn install_honours_core_hooks_path() {
    let s = Scratch::new("hookspath");
    let repo = init_repo(s.root(), "r");
    git_ok(&repo, &["config", "core.hooksPath", ".githooks"]);
    let marker = s.root().join("marker");
    let command = format!("echo fired >> {}", marker.display());
    let err = glia_ok(s.root(), &["install-hooks", "r", "--command", &command]);
    stage(&repo, "a");
    git_ok(&repo, &["commit", "-q", "-m", "x"]);
    assert!(
        marker_fired(&marker),
        "post-commit never ran from core.hooksPath\n{err}"
    );
    let hooks = repo.join(".githooks");
    assert!(hooks.join("post-commit").is_file(), "{err}");
    assert!(
        err.contains(&format!(
            "[hooks] note: {} is inside the work tree (core.hooksPath) - the hooks are visible to git status",
            hooks.display()
        )),
        "{err}"
    );
}

#[test]
fn pair_blocks_a_missing_branch() {
    let s = Scratch::new("missing");
    let (np, _gl) = pair_repos(&s, "main", false);
    stage(&np, "a");
    let out = git(&np, &["commit", "-q", "-m", "x"]);
    assert!(
        !out.status.success(),
        "commit went through: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains(
            "[hooks] branch-pair BLOCKED: glia branch `feat-x` missing - `git -C ../gl checkout -b feat-x` first"
        ),
        "{}",
        stderr(&out)
    );
}

#[test]
fn pair_blocks_a_branch_that_is_not_checked_out() {
    let s = Scratch::new("not-checked-out");
    let (np, _gl) = pair_repos(&s, "main", true);
    stage(&np, "a");
    let out = git(&np, &["commit", "-q", "-m", "x"]);
    assert!(
        !out.status.success(),
        "commit went through: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains(
            "[hooks] branch-pair BLOCKED: ../gl is on `main`, not `feat-x` - `git -C ../gl checkout feat-x`"
        ),
        "{}",
        stderr(&out)
    );
}

#[test]
fn pair_match_pins_the_pair_head() {
    let s = Scratch::new("match");
    let (np, gl) = pair_repos(&s, "feat-x", true);
    stage(&np, "a");
    let out = git(&np, &["commit", "-q", "-m", "x"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let gl_head = git_ok(&gl, &["rev-parse", "HEAD"]);
    assert!(
        stderr(&out).contains(&format!(
            "[hooks] branch-pair ok: feat-x == ../gl @ {}",
            &gl_head[..7]
        )),
        "{}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains(&format!("[hooks] pinned Glia-Pinned-At: {gl_head}")),
        "{}",
        stderr(&out)
    );
    assert_eq!(pins(&np), std::slice::from_ref(&gl_head));

    // The pair moves; `--amend -m` pins its new HEAD.
    stage(&gl, "b");
    git_ok(&gl, &["commit", "-q", "-m", "gl moves"]);
    git_ok(&np, &["commit", "-q", "--amend", "-m", "y"]);
    let gl_head = git_ok(&gl, &["rev-parse", "HEAD"]);
    assert_eq!(pins(&np), [gl_head]);

    // `--amend --no-edit` keeps the old message, trailer included: the pin is
    // REPLACED, never stacked. A dirty pair is reported, not blocked.
    stage(&gl, "c");
    git_ok(&gl, &["commit", "-q", "-m", "gl moves again"]);
    std::fs::write(gl.join("scratch.txt"), "wip").expect("dirty the pair");
    let out = git(&np, &["commit", "-q", "--amend", "--no-edit"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "[hooks] branch-pair: ../gl has uncommitted changes; the pin records HEAD, not the working tree"
        ),
        "{}",
        stderr(&out)
    );
    let gl_head = git_ok(&gl, &["rev-parse", "HEAD"]);
    assert_eq!(pins(&np), [gl_head]);
}

#[test]
fn empty_message_still_aborts() {
    let s = Scratch::new("empty");
    let (np, gl) = pair_repos(&s, "feat-x", true);
    stage(&np, "a");
    // GIT_EDITOR=true leaves only the comment template: git must still abort,
    // not commit a trailer-only message.
    let out = git(&np, &["commit"]);
    assert!(
        !out.status.success(),
        "empty commit went through: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("Aborting commit due to empty commit message"),
        "{}",
        stderr(&out)
    );

    // An editor that writes a message above the template: exactly one pin,
    // and the comment lines are still stripped.
    let editor = s.root().join("editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\n{ printf 'edited subject\\n'; cat \"$1\"; } > \"$1.new\" && mv \"$1.new\" \"$1\"\n",
    )
    .expect("write editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755))
            .expect("chmod editor");
    }
    let mut c = isolated("git", &np);
    c.env("GIT_EDITOR", &editor).args(["commit", "-q"]);
    let out = run(c);
    assert!(out.status.success(), "{}", stderr(&out));
    let gl_head = git_ok(&gl, &["rev-parse", "HEAD"]);
    assert_eq!(pins(&np), std::slice::from_ref(&gl_head));
    let body = git_ok(&np, &["log", "-1", "--format=%B"]);
    assert_eq!(body, format!("edited subject\n\nGlia-Pinned-At: {gl_head}"));
}

#[test]
fn no_verify_skips_both() {
    let s = Scratch::new("no-verify");
    let (np, _gl) = pair_repos(&s, "main", false);
    stage(&np, "a");
    git_ok(&np, &["commit", "-q", "--no-verify", "-m", "x"]);
    assert!(
        pins(&np).is_empty(),
        "--no-verify still pinned: {:?}",
        pins(&np)
    );
}

#[test]
fn skip_env_skips_the_check_only() {
    let s = Scratch::new("skip-env");
    let (np, gl) = pair_repos(&s, "main", false);
    stage(&np, "a");
    let mut c = isolated("git", &np);
    c.env("GLIA_BRANCH_PAIR", "skip")
        .args(["commit", "-q", "-m", "x"]);
    let out = run(c);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("[hooks] branch-pair skipped (GLIA_BRANCH_PAIR=skip)"),
        "{}",
        stderr(&out)
    );
    assert_eq!(pins(&np), [git_ok(&gl, &["rev-parse", "HEAD"])]);
}

#[test]
fn missing_glia_fails_open() {
    let s = Scratch::new("no-glia");
    let (np, _gl) = pair_repos(&s, "main", false);
    stage(&np, "a");
    let mut c = isolated("git", &np);
    c.env("PATH", path_without_glia())
        .args(["commit", "-q", "-m", "x"]);
    let out = run(c);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("glia not on PATH"),
        "{}",
        stderr(&out)
    );
    assert!(pins(&np).is_empty());
}

#[test]
fn uninstall_removes_managed_only() {
    let s = Scratch::new("uninstall");
    let np = init_repo(s.root(), "np");
    let _gl = init_repo(s.root(), "gl");
    let hooks = np.join(".git").join("hooks");
    let user_hook = "#!/bin/sh\n# the user's own pre-commit\nexit 0\n";
    std::fs::write(hooks.join("pre-commit"), user_hook).expect("write user hook");
    let err = glia_ok(
        s.root(),
        &[
            "install-hooks",
            "np",
            "--pair",
            "../gl",
            "--command",
            "true",
        ],
    );
    assert!(
        err.contains(&format!(
            "[hooks] installed 4 hook(s) into {} (skipped 1 non-managed, branch-pair=../gl)",
            hooks.display()
        )),
        "{err}"
    );
    for hook in ["post-commit", "post-merge", "post-checkout", "commit-msg"] {
        assert!(hooks.join(hook).is_file(), "{hook} not installed");
    }
    assert_eq!(
        std::fs::read_to_string(hooks.join("pre-commit"))
            .ok()
            .as_deref(),
        Some(user_hook)
    );

    glia_ok(s.root(), &["install-hooks", "np", "--uninstall"]);
    for hook in ["post-commit", "post-merge", "post-checkout", "commit-msg"] {
        assert!(!hooks.join(hook).exists(), "{hook} survived --uninstall");
    }
    assert_eq!(
        std::fs::read_to_string(hooks.join("pre-commit"))
            .ok()
            .as_deref(),
        Some(user_hook)
    );
}

/// Git exports an absolute GIT_DIR / GIT_INDEX_FILE to hooks run in a linked
/// worktree. Unless the hook scrubs them, `git -C <pair> ...` reads OUR repo
/// and a mismatch passes as a match.
#[test]
fn pair_check_in_a_worktree_reads_the_pair() {
    let s = Scratch::new("worktree-pair");
    let np = init_repo(s.root(), "np");
    git_ok(&np, &["worktree", "add", "-q", "-b", "feat-x", "../npwt"]);
    let _gl = init_repo(s.root(), "gl");
    glia_ok(
        s.root(),
        &[
            "install-hooks",
            "npwt",
            "--pair",
            "../gl",
            "--command",
            "true",
        ],
    );
    let wt = s.root().join("npwt");
    stage(&wt, "a");
    let out = git(&wt, &["commit", "-q", "-m", "x"]);
    assert!(
        !out.status.success(),
        "commit went through: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out)
            .contains("glia branch `feat-x` missing - `git -C ../gl checkout -b feat-x` first"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn install_rejects_a_pair_that_is_not_a_repo() {
    let s = Scratch::new("bad-pair");
    let _np = init_repo(s.root(), "np");
    std::fs::create_dir_all(s.root().join("plain")).expect("plain dir");
    let out = glia(s.root(), &["install-hooks", "np", "--pair", "../plain"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("not a git repository"),
        "{}",
        stderr(&out)
    );
}
