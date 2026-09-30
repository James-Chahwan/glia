//! `glia cache gc` (CE.2c): prune a directory store.
//!
//! A store is `v1/<stamp>/<aa>/<key>.gpc`, one directory per build stamp, and
//! a release never reads another stamp's objects (every key hashes the
//! stamp). So GC works stamp first: the stamps are ordered newest first by
//! the mtime of their `LAST` file (a push touches it on every upload; a stamp
//! dir without one uses its own mtime; ties by name), and every stamp beyond
//! the newest `keep_stamps` is removed whole. Then, under `max_bytes`, the
//! kept objects are deleted oldest first (by mtime, then by path) until the
//! total is at most `max_bytes`. A staging file (`*.tmp`) older than an hour
//! is a crashed pusher's and is removed; a fresh one belongs to a push in
//! flight and is left alone. Only `.gpc` objects count as bytes.
//!
//! fired_on marker, once per run:
//! `[cache] gc store=<dir> stamps_kept=<k> stamps_removed=<r> objects_removed=<o> bytes=<before>-><after>`

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::store::LAST_FILE;

/// A staging file older than this is a crashed pusher's.
const STALE_TMP: Duration = Duration::from_secs(60 * 60);

/// What `gc` keeps.
#[derive(Debug, Clone)]
pub(crate) struct GcOptions {
    /// The newest stamps kept (default 2: this release and the one before).
    pub(crate) keep_stamps: usize,
    /// The byte budget for the kept objects, if any.
    pub(crate) max_bytes: Option<u64>,
}

impl Default for GcOptions {
    fn default() -> Self {
        GcOptions {
            keep_stamps: 2,
            max_bytes: None,
        }
    }
}

/// What one `gc` did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GcSummary {
    /// Kept stamps, newest first.
    pub(crate) kept: Vec<String>,
    /// Removed stamps, newest first.
    pub(crate) removed: Vec<String>,
    /// Objects deleted: those in removed stamps plus those over the budget.
    pub(crate) objects_removed: usize,
    /// Stale staging files deleted from kept stamps.
    pub(crate) tmp_removed: usize,
    /// Object bytes before and after.
    pub(crate) bytes_before: u64,
    pub(crate) bytes_after: u64,
}

/// One object of a stamp.
struct Object {
    mtime: SystemTime,
    size: u64,
    /// `<stamp>/<aa>/<key>.gpc`, for the deterministic tie-break.
    rel: String,
    path: PathBuf,
}

/// One stamp directory's contents.
#[derive(Default)]
struct Listing {
    objects: Vec<Object>,
    /// `*.tmp` staging files with their mtimes.
    tmps: Vec<(PathBuf, SystemTime)>,
}

fn mtime(path: &Path) -> Result<SystemTime, String> {
    std::fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn read_dir(dir: &Path) -> Result<Vec<std::fs::DirEntry>, String> {
    let mut entries = std::fs::read_dir(dir)
        .and_then(|it| it.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());
    Ok(entries)
}

/// The objects and staging files of one stamp dir (`<aa>/<file>`; nothing
/// is followed through a symlink).
fn list_stamp(dir: &Path, stamp: &str) -> Result<Listing, String> {
    let mut out = Listing::default();
    for fan in read_dir(dir)? {
        let is_dir = fan
            .file_type()
            .map_err(|e| format!("{}: {e}", fan.path().display()))?;
        if !is_dir.is_dir() {
            continue;
        }
        let fan_name = fan.file_name().to_string_lossy().into_owned();
        for f in read_dir(&fan.path())? {
            let ft = f
                .file_type()
                .map_err(|e| format!("{}: {e}", f.path().display()))?;
            if !ft.is_file() {
                continue;
            }
            let name = f.file_name().to_string_lossy().into_owned();
            let meta = f
                .metadata()
                .map_err(|e| format!("{}: {e}", f.path().display()))?;
            let modified = meta
                .modified()
                .map_err(|e| format!("{}: {e}", f.path().display()))?;
            if name.ends_with(".tmp") {
                out.tmps.push((f.path(), modified));
            } else if name.ends_with(".gpc") {
                out.objects.push(Object {
                    mtime: modified,
                    size: meta.len(),
                    rel: format!("{stamp}/{fan_name}/{name}"),
                    path: f.path(),
                });
            }
        }
    }
    Ok(out)
}

/// Prune the directory store at `root` (see the module doc).
pub(crate) fn gc(root: &Path, opts: &GcOptions) -> Result<GcSummary, String> {
    let v1 = root.join("v1");
    let mut stamps: Vec<(SystemTime, String, PathBuf)> = Vec::new();
    if v1.is_dir() {
        for e in read_dir(&v1)? {
            let ft = e
                .file_type()
                .map_err(|err| format!("{}: {err}", e.path().display()))?;
            if !ft.is_dir() {
                continue;
            }
            let dir = e.path();
            let last = dir.join(LAST_FILE);
            let recency = match std::fs::symlink_metadata(&last) {
                Ok(m) if m.is_file() => m
                    .modified()
                    .map_err(|err| format!("{}: {err}", last.display()))?,
                _ => mtime(&dir)?,
            };
            stamps.push((recency, e.file_name().to_string_lossy().into_owned(), dir));
        }
    }
    // Newest first; equal recencies by name.
    stamps.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut summary = GcSummary::default();
    let now = SystemTime::now();
    let mut kept_objects: Vec<Object> = Vec::new();
    for (i, (_, name, dir)) in stamps.into_iter().enumerate() {
        let listing = list_stamp(&dir, &name)?;
        let bytes: u64 = listing.objects.iter().map(|o| o.size).sum();
        summary.bytes_before += bytes;
        if i < opts.keep_stamps {
            for (tmp, modified) in listing.tmps {
                let age = now.duration_since(modified).unwrap_or_default();
                if age > STALE_TMP {
                    std::fs::remove_file(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
                    summary.tmp_removed += 1;
                }
            }
            kept_objects.extend(listing.objects);
            summary.kept.push(name);
        } else {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            summary.objects_removed += listing.objects.len();
            summary.removed.push(name);
        }
    }

    let mut total: u64 = kept_objects.iter().map(|o| o.size).sum();
    if let Some(max) = opts.max_bytes {
        kept_objects.sort_by(|a, b| a.mtime.cmp(&b.mtime).then_with(|| a.rel.cmp(&b.rel)));
        for o in &kept_objects {
            if total <= max {
                break;
            }
            match std::fs::remove_file(&o.path) {
                Ok(()) => {}
                // A concurrent GC removed it first: its bytes are gone either way.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", o.path.display())),
            }
            total -= o.size;
            summary.objects_removed += 1;
        }
    }
    summary.bytes_after = total;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_mtime(path: &Path, t: SystemTime) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(t))
            .expect("set mtime");
    }

    fn write(path: &Path, len: usize, t: SystemTime) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, vec![0u8; len]).expect("write");
        set_mtime(path, t);
    }

    #[test]
    fn gc_keeps_the_newest_stamps() {
        let root = std::env::temp_dir().join(format!("glia-ce2c-gc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let now = SystemTime::now();
        let ago = |s: u64| now - Duration::from_secs(s);
        let v1 = root.join("v1");
        // Three stamps; LAST mtimes t1 < t2 < t3, names in the other order so
        // the order is the recency's, not the name's.
        for (stamp, t) in [("c", ago(3000)), ("b", ago(2000)), ("a", ago(1000))] {
            write(&v1.join(stamp).join("aa/aa01.gpc"), 100, ago(900));
            write(&v1.join(stamp).join(LAST_FILE), 0, t);
        }
        // Kept stamps' objects of different ages: a is 100 + 300, b is 100 + 200.
        write(&v1.join("a/bb/bb02.gpc"), 300, ago(100));
        write(&v1.join("b/cc/cc03.gpc"), 200, ago(950));
        // Staging files: 2 hours old (a crashed push) and fresh (in flight).
        write(&v1.join("a/aa/aa09.gpc.123.tmp"), 7, ago(7200));
        write(&v1.join("a/aa/aa08.gpc.124.tmp"), 7, ago(10));

        let s = gc(
            &root,
            &GcOptions {
                keep_stamps: 2,
                max_bytes: None,
            },
        )
        .expect("gc");
        assert_eq!(s.kept, ["a", "b"]);
        assert_eq!(s.removed, ["c"]);
        assert_eq!((s.objects_removed, s.tmp_removed), (1, 1));
        assert_eq!((s.bytes_before, s.bytes_after), (800, 700));
        assert!(!v1.join("c").exists());
        assert!(!v1.join("a/aa/aa09.gpc.123.tmp").exists());
        assert!(v1.join("a/aa/aa08.gpc.124.tmp").exists());

        // Half the kept bytes: the oldest objects go first (b's 950s-old one,
        // then the two 900s-old ones by path), stopping at the budget.
        let s = gc(
            &root,
            &GcOptions {
                keep_stamps: 2,
                max_bytes: Some(350),
            },
        )
        .expect("gc budget");
        assert_eq!(s.removed, Vec::<String>::new());
        assert_eq!(s.objects_removed, 3);
        assert_eq!((s.bytes_before, s.bytes_after), (700, 300));
        assert!(!v1.join("b/cc/cc03.gpc").exists());
        assert!(!v1.join("a/aa/aa01.gpc").exists());
        assert!(!v1.join("b/aa/aa01.gpc").exists());
        assert!(v1.join("a/bb/bb02.gpc").exists());

        // A stamp dir without LAST orders by its own mtime (created now, so
        // newer than a's and b's LAST).
        write(&v1.join("z/zz/zz01.gpc"), 10, ago(5));
        let s = gc(
            &root,
            &GcOptions {
                keep_stamps: 1,
                max_bytes: None,
            },
        )
        .expect("gc no LAST");
        assert_eq!(
            (s.kept, s.removed),
            (
                vec!["z".to_string()],
                vec!["a".to_string(), "b".to_string()]
            )
        );
        assert_eq!(
            (s.objects_removed, s.bytes_before, s.bytes_after),
            (1, 310, 10)
        );

        // keep 0 empties the store; an empty store is a no-op.
        let s = gc(
            &root,
            &GcOptions {
                keep_stamps: 0,
                max_bytes: None,
            },
        )
        .expect("gc all");
        assert_eq!((s.kept.len(), s.removed.len(), s.bytes_after), (0, 1, 0));
        let s = gc(&root, &GcOptions::default()).expect("gc empty");
        assert_eq!(s, GcSummary::default());
        let _ = std::fs::remove_dir_all(&root);
    }
}
