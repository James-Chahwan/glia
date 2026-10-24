//! FB.13a: the identifier vocabulary of one commit's tree.
//!
//! "Tracked" is the commit's tree (the index as that commit recorded it), so
//! an untracked or staged-but-uncommitted file is never read and two clones of
//! one commit read the same blobs: `git ls-tree -r -z -l <rev>` lists them
//! (relative to the root when the root is a subdirectory of the work tree),
//! and one `git cat-file --batch` process streams their contents. A blob with
//! a NUL byte in its first [`BINARY_PROBE`] bytes (git's own binary
//! heuristic) or larger than [`MAX_BLOB_BYTES`] is skipped and counted.
//! Nothing the build's walk filters (generated, vendored, gitignored-but-
//! tracked) is filtered here: the reader wants every token at the commit.

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};

use glia_code_domain::snapshots::{VocabFile, VocabMeta, VocabToken, vocab_tokens};

/// The pure tokeniser, re-exported so a caller of this crate needs no
/// code-domain import; it lives in code-domain so the engine can tokenise
/// without depending on the process-spawning crate.
pub use glia_code_domain::snapshots::vocab_tokens as tokens_of;

/// git's binary heuristic window: a NUL in the first 8000 bytes.
pub const BINARY_PROBE: usize = 8000;
/// A blob larger than this (4 MiB) is skipped as `skipped_large`.
pub const MAX_BLOB_BYTES: u64 = 4 << 20;

/// What [`capture_vocab`] read: the rows `write_vocab` takes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Vocab {
    /// The meta to write: the full commit sha and the skip counts.
    pub meta: VocabMeta,
    /// The text files read, ids `0..n` in path order.
    pub files: Vec<VocabFile>,
    /// One row per distinct token, sorted by token bytes.
    pub tokens: Vec<VocabToken>,
}

/// Environment variables that would point git at another repository than the
/// `-C <root>` it is given (a sync run from a git hook inherits them). The
/// same list as the history reader's.
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

fn git(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off"])
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    for var in REPO_OVERRIDE_ENV {
        cmd.env_remove(var);
    }
    cmd
}

fn spawn_error(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        format!(
            "the git binary was not found on PATH ({e}); vocab sync reads the repo with the local git"
        )
    } else {
        format!("could not run git: {e}")
    }
}

/// Run `git -C <root> <args>`; its stdout, or an error carrying git's stderr.
fn run_git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = git(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(spawn_error)?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let sub = args.first().copied().unwrap_or("");
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(format!(
        "`git {sub}` failed ({}): {}",
        out.status,
        stderr.trim()
    ))
}

/// One blob of the tree: its object name, size and root-relative path.
struct Entry {
    oid: String,
    size: u64,
    path: String,
}

/// Read the vocabulary of commit `rev` (a revision git resolves to a commit)
/// of the repo at `repo_root`. Errors (not a directory, not a git work tree,
/// a revision that names no commit, git failing) come back before anything
/// is written; this function writes nothing.
pub fn capture_vocab(repo_root: &Path, rev: &str) -> Result<Vocab, String> {
    if !repo_root.is_dir() {
        return Err(format!("{} is not a directory", repo_root.display()));
    }
    if rev.is_empty() || rev.starts_with('-') {
        return Err(format!("{rev:?} is not a revision"));
    }
    run_git(repo_root, &["rev-parse", "--show-prefix"])
        .map_err(|e| format!("{} is not inside a git work tree: {e}", repo_root.display()))?;
    let spec = format!("{rev}^{{commit}}");
    let sha = run_git(repo_root, &["rev-parse", "--verify", "--quiet", &spec])
        .map_err(|_| format!("{rev:?} does not name a commit"))?;
    let sha = String::from_utf8_lossy(&sha).trim().to_string();
    if !(sha.len() == 40 || sha.len() == 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("git rev-parse printed {sha:?}, not a commit sha"));
    }

    let listing = run_git(repo_root, &["ls-tree", "-r", "-z", "-l", &sha])?;
    let mut entries = parse_ls_tree(&listing)?;
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let mut skipped_large = 0;
    entries.retain(|e| {
        let keep = e.size <= MAX_BLOB_BYTES;
        if !keep {
            skipped_large += 1;
        }
        keep
    });

    let mut files = Vec::new();
    let mut skipped_binary = 0;
    let mut acc: BTreeMap<String, (u64, Vec<u32>)> = BTreeMap::new();
    stream_blobs(repo_root, &entries, |entry, bytes| {
        let probe = &bytes[..bytes.len().min(BINARY_PROBE)];
        if probe.contains(&0) {
            skipped_binary += 1;
            return;
        }
        let id = files.len() as u32;
        files.push(VocabFile::new(id, entry.path.clone()));
        for (token, n) in vocab_tokens(bytes) {
            let slot = acc.entry(token).or_default();
            slot.0 += u64::from(n);
            slot.1.push(id);
        }
    })?;
    // BTreeMap<String> iterates in byte order (str's Ord is byte order), and
    // each id list is ascending because files are visited in id order.
    let tokens = acc
        .into_iter()
        .map(|(t, (n, ids))| VocabToken::new(t, n, ids))
        .collect();
    Ok(Vocab {
        meta: VocabMeta::new(sha, skipped_binary, skipped_large),
        files,
        tokens,
    })
}

/// Parse `git ls-tree -r -z -l` output: `<mode> SP <type> SP <oid> SP+ <size>
/// TAB <path> NUL`. Only `blob` entries are kept (a submodule is a `commit`).
fn parse_ls_tree(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for record in bytes.split(|b| *b == 0) {
        if record.is_empty() {
            continue;
        }
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .ok_or_else(|| "git ls-tree: a record without a path".to_string())?;
        let head = String::from_utf8_lossy(&record[..tab]);
        let mut fields = head.split_ascii_whitespace();
        let (_mode, kind, oid, size) = (fields.next(), fields.next(), fields.next(), fields.next());
        let (Some(kind), Some(oid), Some(size)) = (kind, oid, size) else {
            return Err(format!("git ls-tree: unexpected record {head:?}"));
        };
        if kind != "blob" {
            continue;
        }
        let size = size
            .parse::<u64>()
            .map_err(|_| format!("git ls-tree: size {size:?} of {oid}"))?;
        // A path that is not UTF-8 is read lossily (U+FFFD), deterministically.
        let path = String::from_utf8_lossy(&record[tab + 1..]).into_owned();
        out.push(Entry {
            oid: oid.to_string(),
            size,
            path,
        });
    }
    Ok(out)
}

/// Feed every entry's object name to one `git cat-file --batch` and call
/// `visit` with each blob's bytes, in entry order. The names are written from
/// a second thread so a large tree cannot deadlock the two pipes.
fn stream_blobs(
    root: &Path,
    entries: &[Entry],
    mut visit: impl FnMut(&Entry, &[u8]),
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut child = git(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_error)?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return Err("git cat-file: no pipes".to_string());
    };
    let names: Vec<u8> = entries
        .iter()
        .flat_map(|e| e.oid.bytes().chain([b'\n']))
        .collect();
    let writer = std::thread::spawn(move || stdin.write_all(&names));

    let mut reader = BufReader::with_capacity(1 << 16, stdout);
    let mut result = Ok(());
    let mut header = Vec::new();
    let mut body = Vec::new();
    for entry in entries {
        header.clear();
        if let Err(e) = reader.read_until(b'\n', &mut header) {
            result = Err(format!("git cat-file: read: {e}"));
            break;
        }
        let line = String::from_utf8_lossy(&header);
        let mut fields = line.split_ascii_whitespace();
        let (oid, kind, size) = (fields.next(), fields.next(), fields.next());
        let size = match (oid, kind, size.and_then(|s| s.parse::<usize>().ok())) {
            (Some(oid), Some("blob"), Some(size)) if oid == entry.oid => size,
            _ => {
                result = Err(format!(
                    "git cat-file: unexpected header {:?} for {}",
                    line.trim(),
                    entry.path
                ));
                break;
            }
        };
        body.resize(size + 1, 0);
        if let Err(e) = reader.read_exact(&mut body) {
            result = Err(format!("git cat-file: read {}: {e}", entry.path));
            break;
        }
        visit(entry, &body[..size]);
    }
    if result.is_err() {
        // Closes git's end of the pipes, so a writer blocked on a full pipe
        // returns and the join below cannot hang.
        let _ = child.kill();
    }
    drop(reader);
    let written = writer.join();
    let status = child.wait_with_output();
    result?;
    match written {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(format!("git cat-file: write: {e}")),
        Err(_) => return Err("git cat-file: the writer thread panicked".to_string()),
    }
    match status {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "`git cat-file` failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Err(format!("git cat-file: wait: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_tree_keeps_blobs_with_their_sizes() {
        let raw = b"100644 blob aaaa       12\ta b.rs\x00160000 commit bbbb       -\tsub\x00100755 blob cccc 4194305\tbig.bin\0";
        let got = parse_ls_tree(raw).unwrap();
        let rows: Vec<(&str, u64, &str)> = got
            .iter()
            .map(|e| (e.oid.as_str(), e.size, e.path.as_str()))
            .collect();
        assert_eq!(
            rows,
            [("aaaa", 12, "a b.rs"), ("cccc", 4_194_305, "big.bin")]
        );
        assert!(parse_ls_tree(b"100644 blob aaaa 12 nopath\0").is_err());
    }
}
