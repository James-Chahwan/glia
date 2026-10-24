//! The vocabulary side file (FB.13d, engram-34): every identifier-like token
//! of the repo's tracked text at the exported commit, written as
//! `<out>.vocab.jsonl` ([`crate::vocab_side_path`]) beside the gmap when
//! `glia-export-engram --vocab` is given.
//!
//! **Why.** Engram's "(not in memory: X)" note flags a name its store does not
//! hold; some of those names are real (locals, fields, keys, files the export
//! filters) and only glia sees the whole tracked text. This file is the one
//! index of it, bound to the gmap it was exported beside.
//!
//! **The capture.** [`glia_snapshots::capture_vocab`] (FB.13a) over the
//! commit's tree: tracked = the git index as that commit recorded it, so an
//! untracked or uncommitted file is never read and every clone of one commit
//! reads the same blobs. Everything the export filters (tests, generated,
//! vendored, noise, non-code, `--exclude-path`) is in it.
//!
//! **One file-id space.** A token's `files` name ids in the export's own file
//! table (`Gmap.files`, the `files.json` sidecar): a path the table holds keeps
//! its table id, and every other path the vocabulary read gets a path row
//! `{"f":<id>,"path":"<repo-relative>"}`, numbered on from the one after the
//! table's largest id, in path (byte) order. A reader resolves an id through the table, then
//! the path rows.
//!
//! **The format.** JSONL, every object's keys sorted (`BTreeMap`), serde_json
//! compact, non-ASCII literal, each line `\n`-terminated:
//! 1. the header `{"files":<total>,"gmap_digest":"<16 hex>","tokens":<n>,
//!    "vocab_side":1}`: `files` is the size of the file-id space (the table's
//!    entries plus the path rows), `gmap_digest` the gmap's
//!    [`content_digest`](engram_core::content_digest) as `meta.json` names it
//!    (a reader drops a file whose digest is not its gmap's), `tokens` the
//!    token rows;
//! 2. the path rows, sorted by path;
//! 3. one row per distinct token, sorted by token bytes:
//!    `{"files":[<ids ascending>],"n":<occurrences>,"t":"<token>"}`, or for a
//!    token in more than [`VOCAB_FILES_CAP`] files
//!    `{"common":true,"n":<n>,"t":"<token>"}` - exactly the rows FB.13a's
//!    `vocab.jsonl` holds, with the file ids remapped.
//!
//! The bytes are a pure function of the commit's tree and the gmap's file
//! table: no map is iterated into them but `BTreeMap`s. A `--since` step
//! writes a full file, never a diff.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use engram_core::Gmap;
use glia_code_domain::snapshots::VOCAB_FILES_CAP;
use glia_snapshots::Vocab;

/// The side file's schema version (the header's `vocab_side`): bumped when a
/// field is renamed or removed or changes meaning.
pub const VOCAB_SIDE_VERSION: u32 = 1;

/// What [`write_vocab_side`] wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VocabSideStats {
    /// Token rows written.
    pub tokens: usize,
    /// Path rows written: files the vocabulary read that the export's file
    /// table does not hold.
    pub extra_files: usize,
    /// Token rows marked `common` (more than [`VOCAB_FILES_CAP`] files).
    pub common: usize,
    /// Bytes of the whole file.
    pub bytes: usize,
}

/// The export's file table as `path -> id` (`gmap.files` inverted).
pub fn file_ids_of(gmap: &Gmap) -> BTreeMap<String, u32> {
    gmap.files.iter().map(|(id, p)| (p.clone(), *id)).collect()
}

/// The side file's bytes for `vocab` over the file table `file_ids`, bound
/// to a gmap whose `content_digest` is `gmap_digest`.
pub fn vocab_side_bytes(
    vocab: &Vocab,
    file_ids: &BTreeMap<String, u32>,
    gmap_digest: u64,
) -> io::Result<(Vec<u8>, VocabSideStats)> {
    let invalid = |e: serde_json::Error| io::Error::new(io::ErrorKind::InvalidData, e);
    // The vocabulary's own ids are `0..n`, its index in path order; map each
    // to the one id space. A path outside the table takes the next free id.
    let extra: BTreeSet<&str> = vocab
        .files
        .iter()
        .map(|f| f.path.as_str())
        .filter(|p| !file_ids.contains_key(*p))
        .collect();
    let first_extra = file_ids.values().max().map_or(1, |m| m + 1);
    let extra_ids: BTreeMap<&str, u32> = extra
        .iter()
        .zip(first_extra..)
        .map(|(p, id)| (*p, id))
        .collect();
    let mut remap: BTreeMap<u32, u32> = BTreeMap::new();
    for f in &vocab.files {
        let id = match file_ids.get(&f.path) {
            Some(id) => *id,
            None => extra_ids.get(f.path.as_str()).copied().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "vocab path lost its id")
            })?,
        };
        remap.insert(f.f, id);
    }

    let mut stats = VocabSideStats {
        tokens: vocab.tokens.len(),
        extra_files: extra_ids.len(),
        common: vocab.tokens.iter().filter(|t| t.common).count(),
        bytes: 0,
    };
    let header: BTreeMap<&str, serde_json::Value> = BTreeMap::from([
        ("files", (file_ids.len() + extra_ids.len()).into()),
        ("gmap_digest", format!("{gmap_digest:016x}").into()),
        ("tokens", stats.tokens.into()),
        ("vocab_side", VOCAB_SIDE_VERSION.into()),
    ]);
    let mut out = serde_json::to_vec(&header).map_err(invalid)?;
    out.push(b'\n');
    for (path, id) in &extra_ids {
        let row: BTreeMap<&str, serde_json::Value> =
            BTreeMap::from([("f", (*id).into()), ("path", (*path).into())]);
        out.extend(serde_json::to_vec(&row).map_err(invalid)?);
        out.push(b'\n');
    }
    for token in &vocab.tokens {
        let mut row: BTreeMap<&str, serde_json::Value> =
            BTreeMap::from([("n", token.n.into()), ("t", token.t.as_str().into())]);
        match (&token.files, token.common) {
            (Some(files), false) if files.len() <= VOCAB_FILES_CAP => {
                let mut ids = files
                    .iter()
                    .map(|f| {
                        remap.get(f).copied().ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!(
                                    "token {:?} names file id {f}, which the vocab lacks",
                                    token.t
                                ),
                            )
                        })
                    })
                    .collect::<io::Result<Vec<u32>>>()?;
                ids.sort_unstable();
                row.insert("files", ids.into());
            }
            _ => {
                row.insert("common", true.into());
            }
        }
        out.extend(serde_json::to_vec(&row).map_err(invalid)?);
        out.push(b'\n');
    }
    stats.bytes = out.len();
    Ok((out, stats))
}

/// Write the side file for `vocab` (over the export's file table `file_ids`,
/// beside a gmap whose `content_digest` is `gmap_digest`) to `out`,
/// tmp-then-rename: the header, the path rows, then the token rows.
pub fn write_vocab_side(
    vocab: &Vocab,
    file_ids: &BTreeMap<String, u32>,
    gmap_digest: u64,
    out: &Path,
) -> io::Result<VocabSideStats> {
    let (bytes, stats) = vocab_side_bytes(vocab, file_ids, gmap_digest)?;
    crate::write_atomic(out, &bytes)?;
    Ok(stats)
}

/// The commit an export reads its vocabulary at: the work tree's `HEAD`
/// when the build ran, and how many tracked paths under the root differ from
/// it (`git status --porcelain=v1 -uno`; the build read the work tree, the
/// vocabulary reads the commit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRev {
    /// The full commit sha.
    pub sha: String,
    /// Modified tracked paths under the root (0 for a clean tree).
    pub dirty: usize,
}

/// Environment variables that would point git at another repository than the
/// `-C <root>` it is given (the same list as glia-snapshots' readers).
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

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null());
    for var in REPO_OVERRIDE_ENV {
        cmd.env_remove(var);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// [`ExportRev`] of the work tree holding `root`; an error when `root` is in
/// no git work tree or its `HEAD` names no commit.
pub fn export_rev(root: &Path) -> Result<ExportRev, String> {
    let sha = git(root, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
    let sha = String::from_utf8_lossy(&sha).trim().to_string();
    if !(sha.len() == 40 || sha.len() == 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("git rev-parse printed {sha:?}, not a commit sha"));
    }
    let status = git(root, &["status", "--porcelain=v1", "-z", "-uno", "--", "."])?;
    // `-z` records are NUL-terminated; a rename (`R`/`C`) carries its source
    // path as one more record, which is not a second modified path.
    let mut dirty = 0;
    let mut records = status.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        dirty += 1;
        if matches!(rec.first(), Some(b'R' | b'C')) {
            records.next();
        }
    }
    Ok(ExportRev { sha, dirty })
}
