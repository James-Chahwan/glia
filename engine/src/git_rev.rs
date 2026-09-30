//! Read-only materialisation of a git rev for the graph delta (LE.1b).
//!
//! [`materialize_rev`] writes the tree of one commit into a private temp dir so
//! the engine can build it as an ordinary directory. Local git objects only:
//! `rev-parse`, `ls-tree`, one `cat-file --batch`, `ls-files` and a
//! name-only `diff`, every one under `GIT_OPTIONAL_LOCKS=0`, so nothing is
//! fetched, nothing authenticates and the index is never rewritten.
//!
//! What lands in the temp dir, and why both sides of a delta then agree:
//! - regular blobs, at their paths relative to the directory glia was pointed
//!   at (`ls-tree` run there lists that subtree only, so a monorepo
//!   subdirectory materialises just its own files);
//! - symlinks whose target is relative and stays inside the tree (unix only);
//!   any other symlink is skipped and counted, so the walker can never read
//!   outside the checkout;
//! - no submodules: a gitlink is skipped and counted. The working-tree walk
//!   collapses a checked-out submodule into a nested-repo REGION with no nodes
//!   in it, and the delta drops REGIONs from both sides;
//! - the untracked external inputs under `<repo>/.glia`: every file the store
//!   fingerprints as an input (`glia_store::external_inputs_fingerprint`:
//!   the layout dir and the parse cache excluded) that neither the rev nor the
//!   index tracks, so a docs / history / test snapshot the rev never held is
//!   read by both builds and is no delta. A control file the rev tracks
//!   (`.glia/overlay.toml`) comes from the rev; one tracked since, from nobody:
//!   it is part of the change.
//!
//! [`changed_files`] (CC.11b) lists the working tree's change against a rev
//! (a name-only `diff` plus the untracked files), with the same helpers.
//!
//! [`head_commit`] (CD.5b) is the one reader here that runs no git process:
//! a build records the commit a layout was built at (`RepoMeta.rev`) from the
//! `.git` files alone, so a build never spawns git.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use glia_code_domain::walk_gating::CONTROL_DIR;

/// A rev as the caller gave it and the commit it resolved to.
pub(crate) struct Rev {
    pub(crate) given: String,
    pub(crate) sha: String,
}

/// A private directory under the system temp dir, removed with everything in
/// it when dropped: on success, on every `Err` path and on unwind alike.
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Result<Self, String> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let mut last = String::new();
        for _ in 0..16 {
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("glia-delta-{pid}-{seq}-{nanos:08x}"));
            match create_private_dir(&path) {
                Ok(()) => return Ok(TempDir { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = e.to_string(),
                Err(e) => return Err(format!("cannot create a temp dir under {}: {e}", base.display())),
            }
        }
        Err(format!("cannot create a temp dir under {}: {last}", base.display()))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best effort: a leftover dir under the temp root is harmless.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

/// One materialised rev and what it held.
pub(crate) struct Materialized {
    pub(crate) dir: TempDir,
    /// Regular files written.
    pub(crate) files: usize,
    /// Submodules (gitlinks) not materialised.
    pub(crate) gitlinks_skipped: usize,
    /// Symlinks recreated.
    pub(crate) symlinks: usize,
    /// Symlinks not recreated: absolute or escaping targets, or no symlink
    /// support on this platform.
    pub(crate) skipped_symlinks: usize,
    /// Untracked `.glia` input files copied in from the working tree.
    pub(crate) snapshots: usize,
}

/// A `git -C <repo>` command that takes no lock and reads no stdin.
fn git(repo: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(repo).env("GIT_OPTIONAL_LOCKS", "0").stdin(Stdio::null());
    c
}

/// Stdout of a git command, `None` when it exits non-zero.
fn git_output(repo: &Path, args: &[&str]) -> Result<Option<Vec<u8>>, String> {
    let out = git(repo)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("git not found on PATH: {e}"))?;
    Ok(out.status.success().then_some(out.stdout))
}

/// Resolve `rev` to a commit in the repository `repo` sits in.
pub(crate) fn resolve_rev(repo: &Path, rev: &str) -> Result<Rev, String> {
    let fail = || format!("not a git work tree or unknown rev {rev}: {}", repo.display());
    // A leading '-' would be read as an option, never as a rev.
    if rev.is_empty() || rev.starts_with('-') {
        return Err(fail());
    }
    let spec = format!("{rev}^{{commit}}");
    let out = git_output(repo, &["rev-parse", "--verify", "--quiet", &spec])?.ok_or_else(fail)?;
    let sha = String::from_utf8_lossy(&out).trim().to_string();
    if sha.is_empty() {
        return Err(fail());
    }
    Ok(Rev { given: rev.to_string(), sha })
}

/// What an `ls-tree` entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Symlink,
    Gitlink,
}

/// One `ls-tree -r -z` entry: `<mode> SP <type> SP <oid> TAB <path>`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    kind: EntryKind,
    oid: String,
    path: Vec<u8>,
}

fn parse_ls_tree(out: &[u8]) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    for rec in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let bad = || format!("unexpected ls-tree record: {}", String::from_utf8_lossy(rec));
        let tab = rec.iter().position(|b| *b == b'\t').ok_or_else(bad)?;
        let head = std::str::from_utf8(&rec[..tab]).map_err(|_| bad())?;
        let path = rec[tab + 1..].to_vec();
        let mut parts = head.split(' ');
        let (Some(mode), Some(kind), Some(oid), None) = (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        let kind = match (kind, mode) {
            ("blob", "120000") => EntryKind::Symlink,
            ("blob", _) => EntryKind::File,
            ("commit", _) => EntryKind::Gitlink,
            // `-r` never lists a tree; anything else is not content.
            _ => continue,
        };
        if !is_safe_rel(&path) {
            return Err(format!("refusing unsafe path in the rev tree: {}", String::from_utf8_lossy(&path)));
        }
        entries.push(Entry { kind, oid: oid.to_string(), path });
    }
    Ok(entries)
}

/// A relative path with only normal components: no leading `/`, no empty,
/// `.`, `..` or `.git` component.
fn is_safe_rel(path: &[u8]) -> bool {
    !path.is_empty()
        && path[0] != b'/'
        && path.split(|b| *b == b'/').all(|c| !matches!(c, b"" | b"." | b".." | b".git"))
}

/// Whether a symlink at `link` (a safe repo-relative path) pointing at
/// `target` stays inside the tree whatever the other links in it are: the
/// target is relative, its `..` components all lead (so a link to a directory
/// cannot be climbed out of) and there are no more of them than `link` has
/// parent directories, which are real directories (a git path is never both
/// a tree and a link).
fn symlink_stays_inside(link: &[u8], target: &[u8]) -> bool {
    if target.is_empty() || target[0] == b'/' {
        return false;
    }
    let depth = link.split(|b| *b == b'/').count() - 1;
    let mut ups = 0usize;
    let mut descended = false;
    for c in target.split(|b| *b == b'/') {
        match c {
            b"" | b"." => {}
            b".." if descended => return false,
            b".." => ups += 1,
            _ => descended = true,
        }
    }
    ups <= depth
}

#[cfg(unix)]
fn bytes_path(b: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(b))
}

#[cfg(not(unix))]
fn bytes_path(b: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(b).into_owned())
}

#[cfg(unix)]
fn make_symlink(target: &[u8], at: &Path) -> std::io::Result<bool> {
    std::os::unix::fs::symlink(bytes_path(target), at).map(|()| true)
}

#[cfg(not(unix))]
fn make_symlink(_target: &[u8], _at: &Path) -> std::io::Result<bool> {
    Ok(false)
}

fn write_at(root: &Path, rel: &[u8], bytes: &[u8]) -> Result<PathBuf, String> {
    let dest = root.join(bytes_path(rel));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("materialise {}: {e}", parent.display()))?;
    }
    std::fs::write(&dest, bytes).map_err(|e| format!("materialise {}: {e}", dest.display()))?;
    Ok(dest)
}

/// Write the tree of `rev` (the subtree of `repo` when `repo` is a
/// subdirectory of its work tree) into a new temp dir, plus the untracked
/// `.glia` inputs (module docs). Prints the `[delta] materialized` marker.
pub(crate) fn materialize_rev(repo: &Path, rev: &Rev) -> Result<Materialized, String> {
    let listing = git_output(repo, &["ls-tree", "-r", "-z", &rev.sha])?
        .ok_or_else(|| format!("git ls-tree {} failed in {}", rev.given, repo.display()))?;
    let entries = parse_ls_tree(&listing)?;
    let dir = TempDir::new()?;
    let mut m = Materialized {
        dir,
        files: 0,
        gitlinks_skipped: 0,
        symlinks: 0,
        skipped_symlinks: 0,
        snapshots: 0,
    };
    let wanted: Vec<&Entry> = entries.iter().filter(|e| e.kind != EntryKind::Gitlink).collect();
    m.gitlinks_skipped = entries.len() - wanted.len();
    let root = m.dir.path().to_path_buf();
    stream_blobs(repo, &wanted, |entry, bytes| {
        match entry.kind {
            EntryKind::File => {
                write_at(&root, &entry.path, bytes)?;
                m.files += 1;
            }
            EntryKind::Symlink => {
                let at = root.join(bytes_path(&entry.path));
                let made = symlink_stays_inside(&entry.path, bytes)
                    && at.parent().is_none_or(|p| std::fs::create_dir_all(p).is_ok())
                    && make_symlink(bytes, &at).map_err(|e| format!("materialise {}: {e}", at.display()))?;
                if made {
                    m.symlinks += 1;
                } else {
                    m.skipped_symlinks += 1;
                }
            }
            EntryKind::Gitlink => {}
        }
        Ok(())
    })?;
    let tracked: BTreeSet<String> =
        entries.iter().map(|e| String::from_utf8_lossy(&e.path).into_owned()).collect();
    m.snapshots = copy_untracked_inputs(repo, &root, &tracked)?;
    eprintln!(
        "[delta] materialized {} files from {} (gitlinks_skipped={} symlinks={} skipped_symlinks={} snapshots={})",
        m.files, rev.given, m.gitlinks_skipped, m.symlinks, m.skipped_symlinks, m.snapshots
    );
    Ok(m)
}

/// Every blob of `entries`, in order, through ONE `git cat-file --batch`:
/// the oids are fed from a writer thread (so neither pipe can fill up and
/// stall the other) and each `<oid> <type> <size>\n<bytes>\n` frame is read
/// by its byte count, never by line.
fn stream_blobs(
    repo: &Path,
    entries: &[&Entry],
    mut each: impl FnMut(&Entry, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut child = git(repo)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("git not found on PATH: {e}"))?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("git cat-file: no pipes".to_string());
    };
    let oids: Vec<String> = entries.iter().map(|e| e.oid.clone()).collect();
    let writer = std::thread::spawn(move || -> std::io::Result<()> {
        let mut w = BufWriter::new(stdin);
        for oid in oids {
            w.write_all(oid.as_bytes())?;
            w.write_all(b"\n")?;
        }
        w.flush()
    });
    let mut reader = BufReader::new(stdout);
    let read = entries.iter().try_for_each(|entry| {
        let bytes = read_frame(&mut reader, &entry.oid)?;
        each(entry, &bytes)
    });
    if read.is_err() {
        let _ = child.kill();
    }
    drop(reader);
    let wrote = writer.join();
    let status = child.wait().map_err(|e| format!("git cat-file: {e}"))?;
    read?;
    match wrote {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(format!("git cat-file: writing oids: {e}")),
        Err(_) => return Err("git cat-file: oid writer panicked".to_string()),
    }
    if !status.success() {
        return Err(format!("git cat-file --batch exited with {status}"));
    }
    Ok(())
}

/// One `--batch` frame for `oid`.
fn read_frame(reader: &mut impl BufRead, oid: &str) -> Result<Vec<u8>, String> {
    let mut header = Vec::new();
    reader
        .read_until(b'\n', &mut header)
        .map_err(|e| format!("git cat-file: {e}"))?;
    let text = String::from_utf8_lossy(&header);
    let fields: Vec<&str> = text.trim_end().split(' ').collect();
    let size: usize = match fields.as_slice() {
        [got, "blob", size] if *got == oid => size
            .parse()
            .map_err(|_| format!("git cat-file: bad frame header {text:?}"))?,
        _ => return Err(format!("git cat-file: expected blob {oid}, got {:?}", text.trim_end())),
    };
    let mut bytes = vec![0u8; size];
    reader
        .read_exact(&mut bytes)
        .map_err(|e| format!("git cat-file: blob {oid}: {e}"))?;
    let mut lf = [0u8; 1];
    reader
        .read_exact(&mut lf)
        .map_err(|e| format!("git cat-file: blob {oid}: {e}"))?;
    Ok(bytes)
}

/// Copy every `.glia` input of the working tree that neither the rev
/// (`tracked`) nor the index tracks into `root`; returns how many.
fn copy_untracked_inputs(repo: &Path, root: &Path, tracked: &BTreeSet<String>) -> Result<usize, String> {
    let inputs = glia_store::external_inputs_fingerprint(repo);
    if inputs.is_empty() {
        return Ok(0);
    }
    let indexed: BTreeSet<String> = git_output(repo, &["ls-files", "-z", "--", CONTROL_DIR])?
        .map(|out| {
            out.split(|b| *b == 0)
                .filter(|p| !p.is_empty())
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .collect()
        })
        .unwrap_or_default();
    let mut copied = 0usize;
    for rel in inputs.keys() {
        if tracked.contains(rel) || indexed.contains(rel) || !is_safe_rel(rel.as_bytes()) {
            continue;
        }
        let dest = root.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("materialise {}: {e}", parent.display()))?;
        }
        std::fs::copy(repo.join(rel), &dest).map_err(|e| format!("copy {rel}: {e}"))?;
        copied += 1;
    }
    Ok(copied)
}

/// The renames git sees between `rev` and the working tree, as
/// `(old_path, new_path)` relative to `repo` (`diff --relative -M`): the
/// declared tier of `glia_graph::identity::detect_moves_with`. A file
/// moved without `git mv` is untracked at its new path and is not listed;
/// move detection still pairs it by body or name. Empty when the diff fails.
pub(crate) fn declared_renames(repo: &Path, rev: &Rev) -> Vec<(String, String)> {
    let args = ["diff", "--relative", "-M", "--name-status", "-z", "--no-color", &rev.sha, "--"];
    match git_output(repo, &args) {
        Ok(Some(out)) => parse_name_status(&out),
        _ => Vec::new(),
    }
}

/// The working tree's changed files against `rev`, relative to `repo`, sorted
/// and deduplicated (CC.11b, the co-change suggestions' `--base` query):
/// every tracked path whose working-tree content differs from the rev
/// (`diff --relative --no-renames --name-only`, so a rename lists both of its
/// paths whatever `diff.renames` says, a deletion included) plus every
/// untracked, not-ignored file (`ls-files --others --exclude-standard`).
/// Paths under the `.glia` control dir are dropped: the walk hard-skips it
/// (LF.1d) and a snapshot is an input, not a change. `Err` when either git
/// command fails.
pub(crate) fn changed_files(repo: &Path, rev: &Rev) -> Result<Vec<String>, String> {
    let diff = ["diff", "--relative", "--no-renames", "--name-only", "-z", "--no-color", &rev.sha, "--"];
    let tracked = git_output(repo, &diff)?
        .ok_or_else(|| format!("git diff {} failed in {}", rev.given, repo.display()))?;
    let others = ["ls-files", "--others", "--exclude-standard", "-z"];
    let untracked = git_output(repo, &others)?
        .ok_or_else(|| format!("git ls-files failed in {}", repo.display()))?;
    Ok(changed_paths(&[&tracked, &untracked]))
}

/// The NUL-separated paths of `listings`, deduplicated and sorted, with every
/// path under the `.glia` control dir dropped.
fn changed_paths(listings: &[&[u8]]) -> Vec<String> {
    let control = format!("{CONTROL_DIR}/");
    let set: BTreeSet<String> = listings
        .iter()
        .flat_map(|out| out.split(|b| *b == 0))
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .filter(|p| p != CONTROL_DIR && !p.starts_with(&control))
        .collect();
    set.into_iter().collect()
}

/// Most `ref:` hops [`head_commit`] follows from `HEAD` (git's own limit).
const MAX_SYMREF_HOPS: usize = 5;
/// Most bytes read from `HEAD`, a loose ref, `commondir` or a `.git` file:
/// each is one short line.
const SMALL_FILE_CAP: u64 = 4096;

/// The commit the work tree at `root` has checked out (CD.5b, the manifest's
/// `RepoMeta.rev`), read from the `.git` files alone: no git process runs.
///
/// Discovery is git's: from `root` (canonicalised) up through its ancestors,
/// the first `.git` that is a directory holding a `HEAD` file, or a file
/// reading `gitdir: <path>` (a worktree or a submodule; `path` relative to the
/// file's directory), names the gitdir. `<gitdir>/commondir`, when present,
/// names the shared dir (relative to the gitdir), else it is the gitdir.
/// `HEAD` then resolves: a 40- or 64-hex id (detached) as is, lowercased;
/// `ref: <name>` through `<gitdir>/<name>`, else `<commondir>/<name>`, else the
/// `<commondir>/packed-refs` line naming it (comment and peeled `^` lines
/// skipped), at most [`MAX_SYMREF_HOPS`] symbolic hops.
///
/// `None` for a root in no git work tree, an unborn branch (no commit yet), a
/// ref name that is not a plain `refs/...` path (no `..`, `.` or empty
/// component, so a ref never reads outside the gitdir chain), a `HEAD`, ref,
/// `commondir`, `packed-refs` or `.git` file that is a symlink or anything
/// else unreadable, and the reftable ref backend (its `HEAD` names no real
/// ref).
pub(crate) fn head_commit(root: &Path) -> Option<String> {
    let start = std::fs::canonicalize(root).ok()?;
    for dir in start.ancestors() {
        let dot_git = dir.join(".git");
        let Ok(meta) = std::fs::symlink_metadata(&dot_git) else {
            continue;
        };
        let gitdir = if meta.is_file() {
            // A gitfile: git stops here whether it is valid or not.
            return gitdir_of_file(&dot_git, dir).and_then(|g| commit_of_gitdir(&g));
        } else if meta.is_dir() || (meta.is_symlink() && dot_git.is_dir()) {
            dot_git
        } else {
            return None;
        };
        // A `.git` directory without `HEAD` is not a repository: git keeps
        // looking further up, and so does this.
        if regular_file(&gitdir.join("HEAD")) {
            return commit_of_gitdir(&gitdir);
        }
    }
    None
}

/// Is `path` a regular file (a symlink is not followed, so never one)?
fn regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

/// The first line of the small regular file at `path`, trimmed.
fn first_line(path: &Path) -> Option<String> {
    use std::io::Read;
    if !regular_file(path) {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(path).ok()?.take(SMALL_FILE_CAP).read_to_string(&mut text).ok()?;
    Some(text.lines().next().unwrap_or("").trim().to_string())
}

/// The gitdir a `.git` file at `dot_git` (in the work tree `dir`) names.
fn gitdir_of_file(dot_git: &Path, dir: &Path) -> Option<PathBuf> {
    let line = first_line(dot_git)?;
    let named = line.strip_prefix("gitdir:")?.trim();
    if named.is_empty() {
        return None;
    }
    let gitdir = dir.join(named);
    gitdir.is_dir().then_some(gitdir)
}

/// `HEAD` of `gitdir` resolved to a commit id (see [`head_commit`]).
fn commit_of_gitdir(gitdir: &Path) -> Option<String> {
    let common = match first_line(&gitdir.join("commondir")) {
        Some(c) if !c.is_empty() => gitdir.join(c),
        _ => gitdir.to_path_buf(),
    };
    let mut target = first_line(&gitdir.join("HEAD"))?;
    for _ in 0..=MAX_SYMREF_HOPS {
        let Some(name) = target.strip_prefix("ref:").map(str::trim) else {
            return object_id(&target);
        };
        if !is_ref_name(name) {
            return None;
        }
        match first_line(&gitdir.join(name)).or_else(|| first_line(&common.join(name))) {
            Some(next) => target = next,
            None => return packed_ref(&common, name),
        }
    }
    None
}

/// A plain ref path: `refs/` then only normal, non-empty components, so it
/// joins onto a gitdir without leaving it.
fn is_ref_name(name: &str) -> bool {
    name.starts_with("refs/")
        && !name.contains(['\\', '\0'])
        && name.split('/').all(|c| !matches!(c, "" | "." | ".."))
}

/// A full object id (40 hex, sha-1; 64 hex, sha-256), lowercased.
fn object_id(s: &str) -> Option<String> {
    (matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| s.to_ascii_lowercase())
}

/// The id `<common>/packed-refs` records for `name`.
fn packed_ref(common: &Path, name: &str) -> Option<String> {
    let path = common.join("packed-refs");
    if !regular_file(&path) {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
        .find_map(|l| {
            let (id, refname) = l.trim_end().split_once(' ')?;
            (refname == name).then(|| object_id(id))?
        })
}

/// One commit of a first-parent window (CD.5c): its full id, committer time
/// (unix seconds) and subject as git prints it (`%s`: the title paragraph on
/// one line). The subject is user text, returned verbatim: the timeline
/// stores it only through `glia_store::timeline_subject`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevInfo {
    pub(crate) sha: String,
    pub(crate) time: i64,
    pub(crate) subject: String,
}

/// The last `n` commits of `head`'s first-parent chain, OLDEST first (CD.5c,
/// the timeline's window): `rev-list --first-parent --max-count=<n>` for the
/// ids in order, then one `log --no-walk=unsorted` over them for each id's
/// committer time and subject (NUL-separated, so no subject can split a
/// record). Both plumbing-safe: no signature check, no colour, no pager.
/// Fewer than `n` when the history is shorter. `Err` when git is missing or
/// either command fails.
pub(crate) fn first_parent_log(repo: &Path, head: &Rev, n: usize) -> Result<Vec<RevInfo>, String> {
    let max = format!("--max-count={n}");
    let listed = git_output(repo, &["rev-list", "--first-parent", &max, &head.sha, "--"])?
        .ok_or_else(|| format!("git rev-list {} failed in {}", head.given, repo.display()))?;
    let shas: Vec<String> = String::from_utf8_lossy(&listed)
        .lines()
        .filter_map(|l| object_id(l.trim()))
        .collect();
    if shas.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["log", "--no-walk=unsorted", "--no-show-signature", "--no-color", "--format=%H%x00%ct%x00%s", "-z"];
    args.extend(shas.iter().map(String::as_str));
    args.push("--");
    let out = git_output(repo, &args)?.ok_or_else(|| format!("git log failed in {}", repo.display()))?;
    let records = parse_log_records(&out);
    let mut revs = Vec::with_capacity(shas.len());
    for sha in shas.iter().rev() {
        let rec = records
            .iter()
            .find(|r| &r.sha == sha)
            .ok_or_else(|| format!("git log did not list commit {sha} in {}", repo.display()))?;
        revs.push(rec.clone());
    }
    Ok(revs)
}

/// The `%H NUL %ct NUL %s` records of a `log -z` listing (records NUL
/// terminated too), in listing order. A record whose id is not an object id
/// or whose time is not an integer is dropped, so its commit reads as
/// unlisted.
fn parse_log_records(out: &[u8]) -> Vec<RevInfo> {
    let mut fields: Vec<&[u8]> = out.split(|b| *b == 0).collect();
    // The last record's terminator leaves one empty field behind.
    if fields.len() % 3 == 1 && fields.last().is_some_and(|f| f.is_empty()) {
        fields.pop();
    }
    fields
        .chunks_exact(3)
        .filter_map(|r| {
            let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
            let sha = object_id(text(r[0]).trim())?;
            let time = text(r[1]).trim().parse::<i64>().ok()?;
            Some(RevInfo { sha, time, subject: text(r[2]) })
        })
        .collect()
}

/// The renames git sees from commit `a` to commit `b`, as `(old_path,
/// new_path)` relative to `repo` (`diff --relative -M`, the
/// [`declared_renames`] listing between two commits): the declared tier of
/// `glia_graph::identity::detect_moves_with` for one timeline step (CD.5c).
/// Empty when the diff fails.
pub(crate) fn renames_between(repo: &Path, a: &str, b: &str) -> Vec<(String, String)> {
    if a.starts_with('-') || b.starts_with('-') {
        return Vec::new();
    }
    let args = ["diff", "--relative", "-M", "--name-status", "-z", "--no-color", a, b, "--"];
    match git_output(repo, &args) {
        Ok(Some(out)) => parse_name_status(&out),
        _ => Vec::new(),
    }
}

/// The `R<score>` pairs of a `--name-status -z` listing.
fn parse_name_status(out: &[u8]) -> Vec<(String, String)> {
    let mut fields = out.split(|b| *b == 0).map(|f| String::from_utf8_lossy(f).into_owned());
    let mut pairs = Vec::new();
    while let Some(status) = fields.next() {
        if status.is_empty() {
            continue;
        }
        let two = status.starts_with('R') || status.starts_with('C');
        let (Some(first), second) = (fields.next(), if two { fields.next() } else { None }) else {
            break;
        };
        if let (true, Some(second)) = (status.starts_with('R'), second) {
            pairs.push((first, second));
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_tree_records_parse_by_kind() {
        let out = b"100644 blob aaaa\tsrc/a.py\x00100755 blob bbbb\tbin/run\x00120000 blob cccc\tlink\x00160000 commit dddd\tvendor/sub\x00";
        let e = parse_ls_tree(out).expect("parses");
        let kinds: Vec<EntryKind> = e.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [EntryKind::File, EntryKind::File, EntryKind::Symlink, EntryKind::Gitlink]);
        assert_eq!(e[0].path, b"src/a.py");
        assert_eq!(e[3].oid, "dddd");
        // A path with a tab or space after the first tab is kept whole.
        let odd = parse_ls_tree(b"100644 blob eeee\tdir/with space\tand tab.py\x00").expect("parses");
        assert_eq!(odd[0].path, b"dir/with space\tand tab.py");
    }

    #[test]
    fn unsafe_tree_paths_are_refused() {
        for bad in ["../x", "a/../../x", "/etc/passwd", "a//b", "./a", ".git/config", "a/.git/hooks/x"] {
            let rec = format!("100644 blob ffff\t{bad}\x00");
            assert!(parse_ls_tree(rec.as_bytes()).is_err(), "{bad}");
        }
        assert!(is_safe_rel(b".github/workflows/ci.yml"));
        assert!(is_safe_rel(b".glia/overlay.toml"));
    }

    #[test]
    fn symlinks_must_stay_inside_the_tree() {
        assert!(symlink_stays_inside(b"a/link", b"b.py"));
        assert!(symlink_stays_inside(b"a/link", b"../c/d.py"));
        assert!(symlink_stays_inside(b"a/b/link", b"../../top.py"));
        assert!(symlink_stays_inside(b"a/link", b"./x"));
        assert!(!symlink_stays_inside(b"link", b"../outside"));
        assert!(!symlink_stays_inside(b"a/link", b"../../outside"));
        assert!(!symlink_stays_inside(b"a/link", b"/etc/passwd"));
        assert!(!symlink_stays_inside(b"a/link", b""));
        // A `..` after a normal component could climb out through a link to
        // a directory (`a/up -> ..` then `a/l2 -> up/../..`): refused.
        assert!(!symlink_stays_inside(b"a/l2", b"up/../.."));
        assert!(!symlink_stays_inside(b"a/l2", b"up/.."));
    }

    #[test]
    fn name_status_keeps_renames_only() {
        let out = b"M\x00a.py\x00R100\x00shop/a.py\x00shop/c.py\x00C075\x00x.py\x00y.py\x00D\x00gone.py\x00R087\x00p/q.go\x00r/q.go\x00";
        assert_eq!(
            parse_name_status(out),
            vec![
                ("shop/a.py".to_string(), "shop/c.py".to_string()),
                ("p/q.go".to_string(), "r/q.go".to_string()),
            ]
        );
        assert!(parse_name_status(b"").is_empty());
    }

    #[test]
    fn changed_paths_merge_sort_and_skip_the_control_dir() {
        let tracked = b"src/b.py\x00.glia/overlay.toml\x00a.py\x00";
        let untracked = b"a.py\x00.glia/history-snapshot/commits.jsonl\x00new.py\x00.gliax/keep.txt\x00";
        assert_eq!(changed_paths(&[tracked, untracked]), [".gliax/keep.txt", "a.py", "new.py", "src/b.py"]);
        assert!(changed_paths(&[b"", b""]).is_empty());
    }

    #[test]
    fn frames_are_read_by_byte_count() {
        // A blob holding a newline and a fake header must not split the stream.
        let stream = b"aaaa blob 13\nx\nbbbb blob 1\nbbbb blob 0\n\n";
        let mut r = BufReader::new(&stream[..]);
        assert_eq!(read_frame(&mut r, "aaaa").expect("first"), b"x\nbbbb blob 1");
        assert_eq!(read_frame(&mut r, "bbbb").expect("second"), b"");
        assert!(read_frame(&mut BufReader::new(&b"cccc missing\n"[..]), "cccc").is_err());
    }

    const SHA_A: &str = "1111111111111111111111111111111111111111";
    const SHA_B: &str = "2222222222222222222222222222222222222222";
    const SHA_C: &str = "3333333333333333333333333333333333333333";

    /// Write `text` at `root/rel`, creating parent dirs.
    fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, text).expect("write");
    }

    /// A work tree at `<tmp>/<name>` whose `.git` holds `HEAD` = `head`.
    fn repo_with_head(tmp: &Path, name: &str, head: &str) -> PathBuf {
        let root = tmp.join(name);
        put(&root, ".git/HEAD", head);
        std::fs::create_dir_all(root.join(".git/refs/heads")).expect("refs");
        root
    }

    #[test]
    fn head_commit_reads_loose_ref() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = repo_with_head(tmp.path(), "r", "ref: refs/heads/main\n");
        put(&root, ".git/refs/heads/main", &format!("{SHA_A}\n"));
        assert_eq!(head_commit(&root).as_deref(), Some(SHA_A));
        // A subdirectory of the work tree finds the same `.git` above it.
        std::fs::create_dir_all(root.join("src/pkg")).expect("subdir");
        assert_eq!(head_commit(&root.join("src/pkg")).as_deref(), Some(SHA_A));
        // A branch nested in a directory, and a symbolic ref to it.
        put(&root, ".git/refs/heads/feat/x", &format!("{SHA_B}\n"));
        put(&root, ".git/refs/heads/alias", "ref: refs/heads/feat/x\n");
        put(&root, ".git/HEAD", "ref: refs/heads/alias\n");
        assert_eq!(head_commit(&root).as_deref(), Some(SHA_B));
        // An unborn branch (a fresh `git init`) has no commit.
        put(&root, ".git/HEAD", "ref: refs/heads/none\n");
        assert_eq!(head_commit(&root), None);
    }

    #[test]
    fn head_commit_reads_packed_ref() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = repo_with_head(tmp.path(), "r", "ref: refs/heads/dev\n");
        put(
            &root,
            ".git/packed-refs",
            &format!(
                "# pack-refs with: peeled fully-peeled sorted \n{SHA_A} refs/heads/main\n\
                 {SHA_C} refs/tags/v1\n^{SHA_A}\n{SHA_B} refs/heads/dev\n"
            ),
        );
        assert_eq!(head_commit(&root).as_deref(), Some(SHA_B));
        // A loose ref overrides its packed line.
        put(&root, ".git/refs/heads/dev", &format!("{SHA_C}\n"));
        assert_eq!(head_commit(&root).as_deref(), Some(SHA_C));
        // A name only a peeled line or a prefix matches is not found.
        put(&root, ".git/HEAD", "ref: refs/heads/ma\n");
        assert_eq!(head_commit(&root), None);
    }

    #[test]
    fn head_commit_detached() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = repo_with_head(tmp.path(), "r", &format!("{SHA_A}\n"));
        assert_eq!(head_commit(&root).as_deref(), Some(SHA_A));
        // sha-256 repositories; an upper-case id is lowercased.
        let sha256 = "ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        put(&root, ".git/HEAD", &format!("{sha256}\n"));
        assert_eq!(head_commit(&root), Some(sha256.to_ascii_lowercase()));
        for bad in ["", "1111", "not a commit id at all, forty chars long.", "ref: HEAD"] {
            put(&root, ".git/HEAD", bad);
            assert_eq!(head_commit(&root), None, "{bad:?}");
        }
    }

    #[test]
    fn head_commit_worktree_gitdir_file() {
        let tmp = tempfile::tempdir().expect("tmp");
        let main = repo_with_head(tmp.path(), "main", "ref: refs/heads/main\n");
        put(&main, ".git/refs/heads/main", &format!("{SHA_A}\n"));
        put(&main, ".git/packed-refs", &format!("{SHA_B} refs/heads/feature\n"));
        // `git worktree add ../wt feature`: a gitfile to a per-worktree gitdir
        // whose `commondir` leads back to the main `.git`.
        let wt = tmp.path().join("wt");
        let wt_gitdir = main.join(".git/worktrees/wt");
        put(&wt, ".git", &format!("gitdir: {}\n", wt_gitdir.display()));
        put(&wt_gitdir, "HEAD", "ref: refs/heads/feature\n");
        put(&wt_gitdir, "commondir", "../..\n");
        assert_eq!(head_commit(&wt).as_deref(), Some(SHA_B), "the branch, packed in the common dir");
        assert_eq!(head_commit(&main).as_deref(), Some(SHA_A), "the main tree keeps its own HEAD");
        // A submodule: a relative gitfile into the superproject's modules dir.
        let sub = main.join("vendor/sub");
        put(&sub, ".git", "gitdir: ../../.git/modules/vendor/sub\n");
        put(&main, ".git/modules/vendor/sub/HEAD", &format!("{SHA_C}\n"));
        assert_eq!(head_commit(&sub).as_deref(), Some(SHA_C));
        // A gitfile that names nothing stops the search (git does too).
        put(&sub, ".git", "gitdir: ../../nowhere\n");
        assert_eq!(head_commit(&sub), None);
        put(&sub, ".git", "not a gitfile\n");
        assert_eq!(head_commit(&sub), None);
    }

    #[test]
    fn head_commit_non_git_is_none() {
        let tmp = tempfile::tempdir().expect("tmp");
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(plain.join("src")).expect("mkdir");
        assert!(
            tmp.path().ancestors().all(|a| !a.join(".git").exists()),
            "precondition: the temp dir {} must not sit inside a git work tree",
            tmp.path().display()
        );
        assert_eq!(head_commit(&plain), None);
        assert_eq!(head_commit(&tmp.path().join("missing")), None);
        // A `.git` directory with no HEAD is not a repository: none here, and
        // one below a real work tree defers to the work tree above it.
        std::fs::create_dir_all(plain.join(".git/refs")).expect("empty .git");
        assert_eq!(head_commit(&plain), None);
        let outer = repo_with_head(tmp.path(), "outer", &format!("{SHA_A}\n"));
        std::fs::create_dir_all(outer.join("inner/.git")).expect("empty inner .git");
        assert_eq!(head_commit(&outer.join("inner")).as_deref(), Some(SHA_A));
    }

    #[test]
    fn head_commit_never_reads_outside_the_gitdir() {
        let tmp = tempfile::tempdir().expect("tmp");
        // A ref name that climbs out of the gitdir is refused before any read.
        put(tmp.path(), "outside", &format!("{SHA_A}\n"));
        let root = repo_with_head(tmp.path(), "r", "ref: refs/../../../outside\n");
        assert_eq!(head_commit(&root), None);
        for bad in ["ref: refs//heads/x", "ref: refs/./heads", "ref: heads/main", "ref: /etc/passwd"] {
            put(&root, ".git/HEAD", bad);
            assert_eq!(head_commit(&root), None, "{bad:?}");
        }
        // A symlinked ref or HEAD is not followed.
        #[cfg(unix)]
        {
            put(&root, ".git/HEAD", "ref: refs/heads/main\n");
            std::os::unix::fs::symlink(tmp.path().join("outside"), root.join(".git/refs/heads/main"))
                .expect("symlink");
            assert_eq!(head_commit(&root), None, "a symlinked loose ref");
            let other = repo_with_head(tmp.path(), "o", "");
            std::fs::remove_file(other.join(".git/HEAD")).expect("rm");
            std::os::unix::fs::symlink(tmp.path().join("outside"), other.join(".git/HEAD")).expect("symlink");
            assert_eq!(head_commit(&other), None, "a symlinked HEAD");
        }
    }

    #[test]
    fn log_records_parse_with_empty_and_odd_subjects() {
        let sha_b = "b".repeat(40);
        let out = format!("{SHA_A}\x001700000000\x00first line\x00{sha_b}\x001700000100\x00\x00");
        let r = parse_log_records(out.as_bytes());
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!((r[0].sha.as_str(), r[0].time, r[0].subject.as_str()), (SHA_A, 1_700_000_000, "first line"));
        assert_eq!((r[1].sha.as_str(), r[1].time, r[1].subject.as_str()), (sha_b.as_str(), 1_700_000_100, ""));
        // No trailing terminator parses the same.
        let bare = &out[..out.len() - 1];
        assert_eq!(parse_log_records(bare.as_bytes()), r);
        // A subject holding tabs and a would-be separator stays one field.
        let odd = format!("{SHA_A}\x001\x00fix: a\tb \x1e c\x00");
        assert_eq!(parse_log_records(odd.as_bytes())[0].subject, "fix: a\tb \x1e c");
        // A malformed record is dropped, never guessed.
        assert!(parse_log_records(b"nothex\x00x\x00s\x00").is_empty());
        assert!(parse_log_records(format!("{SHA_A}\x00soon\x00s\x00").as_bytes()).is_empty());
        assert!(parse_log_records(b"").is_empty());
    }

    #[test]
    fn renames_between_refuses_an_option() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(renames_between(tmp.path(), "--output=x", "HEAD").is_empty());
        assert!(renames_between(tmp.path(), "HEAD", "-p").is_empty());
    }

    #[test]
    fn temp_dir_is_removed_on_drop() {
        let d = TempDir::new().expect("temp dir");
        let p = d.path().to_path_buf();
        std::fs::write(p.join("f"), "x").expect("write");
        assert!(p.is_dir());
        drop(d);
        assert!(!p.exists());
    }
}
