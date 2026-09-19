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
    match std::fs::remove_file(&meta_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove {}: {e}", meta_path.display())),
    }

    let mut blame_sorted: Vec<&BlameFile> = blame.iter().collect();
    blame_sorted.sort_by(|x, y| x.p.cmp(&y.p));
    let commits_bytes = jsonl(commits.iter())?;
    let blame_bytes = jsonl(blame_sorted.iter().copied())?;
    write_atomic(&dir.join(HISTORY_COMMITS_FILE), &commits_bytes)?;
    write_atomic(&dir.join(HISTORY_BLAME_FILE), &blame_bytes)?;

    meta.commits = commits.len();
    meta.blame_files = blame.len();
    meta.data_hash = data_hash(&[&commits_bytes, &blame_bytes]);
    let mut meta_bytes =
        serde_json::to_vec_pretty(&meta).map_err(|e| format!("serialize {META_FILE}: {e}"))?;
    meta_bytes.push(b'\n');
    write_atomic(&meta_path, &meta_bytes)?;
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
}
