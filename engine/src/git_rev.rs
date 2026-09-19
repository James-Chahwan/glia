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
    fn frames_are_read_by_byte_count() {
        // A blob holding a newline and a fake header must not split the stream.
        let stream = b"aaaa blob 13\nx\nbbbb blob 1\nbbbb blob 0\n\n";
        let mut r = BufReader::new(&stream[..]);
        assert_eq!(read_frame(&mut r, "aaaa").expect("first"), b"x\nbbbb blob 1");
        assert_eq!(read_frame(&mut r, "bbbb").expect("second"), b"");
        assert!(read_frame(&mut BufReader::new(&b"cccc missing\n"[..]), "cccc").is_err());
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
