//! Where cache objects live (CE.2c): the [`ObjectStore`] a push writes to and
//! a pull reads from, and [`DirStore`], a directory (local, or a shared
//! filesystem mount). CE.2e adds the HTTPS store behind the same trait.
//!
//! A relative object path (`object::object_rel`: `v1/<stamp>/<aa>/<key>.gpc`)
//! is validated before it touches the filesystem: only ASCII lower-case
//! letters, digits and `.` `/` `+` `-` `_`, no leading `/`, no empty, `.` or
//! `..` segment. A write stages `<file>.<pid>.tmp` and renames it into place,
//! so a reader never sees a torn object and two pushers of one key (whose
//! payloads are canonical, so byte-equal) race harmlessly; it then touches
//! `v1/<stamp>/LAST`, the recency `gc` orders stamps by.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::object::MAX_OBJECT_BYTES;

/// The file in `v1/<stamp>/` whose mtime is the stamp's last upload.
pub(crate) const LAST_FILE: &str = "LAST";

/// An object store: named, relative paths to whole objects.
pub(crate) trait ObjectStore: Sync {
    /// What the store is, for markers and errors.
    fn label(&self) -> String;
    /// The object at `rel` (at most `MAX_OBJECT_BYTES + 1` bytes of it), or
    /// `None` when there is none.
    fn get(&self, rel: &str) -> Result<Option<Vec<u8>>, String>;
    /// Whether an object is at `rel`.
    fn has(&self, rel: &str) -> Result<bool, String>;
    /// Publish `bytes` at `rel`, atomically.
    fn put(&self, rel: &str, bytes: &[u8]) -> Result<(), String>;
}

/// A directory store rooted at `root`.
#[derive(Debug, Clone)]
pub(crate) struct DirStore {
    root: PathBuf,
}

impl DirStore {
    pub(crate) fn new(root: &Path) -> Self {
        DirStore {
            root: root.to_path_buf(),
        }
    }

    /// `root/rel`, once `rel` is a valid object path.
    fn path(&self, rel: &str) -> Result<PathBuf, String> {
        validate_rel(rel)?;
        Ok(self.root.join(rel))
    }

    /// Touch `v1/<stamp>/LAST` for an object path `v1/<stamp>/...`.
    fn touch_last(&self, rel: &str) -> Result<(), String> {
        let mut parts = rel.split('/');
        let (Some("v1"), Some(stamp)) = (parts.next(), parts.next()) else {
            return Ok(());
        };
        let last = self.root.join("v1").join(stamp).join(LAST_FILE);
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&last)
            .and_then(|f| f.set_modified(SystemTime::now()))
            .map_err(|e| format!("{}: {e}", last.display()))
    }
}

/// Is `rel` a path this store may join onto its root?
pub(crate) fn validate_rel(rel: &str) -> Result<(), String> {
    let allowed = |c: char| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '/' | '+' | '-' | '_')
    };
    if rel.is_empty() || rel.starts_with('/') {
        return Err(format!("invalid object path {rel:?}: empty or absolute"));
    }
    if !rel.chars().all(allowed) {
        return Err(format!(
            "invalid object path {rel:?}: a character outside [a-z0-9./+_-]"
        ));
    }
    if rel
        .split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err(format!(
            "invalid object path {rel:?}: an empty, `.` or `..` segment"
        ));
    }
    Ok(())
}

impl ObjectStore for DirStore {
    fn label(&self) -> String {
        self.root.display().to_string()
    }

    fn get(&self, rel: &str) -> Result<Option<Vec<u8>>, String> {
        let path = self.path(rel)?;
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut out = Vec::new();
        file.take(MAX_OBJECT_BYTES as u64 + 1)
            .read_to_end(&mut out)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Some(out))
    }

    fn has(&self, rel: &str) -> Result<bool, String> {
        let path = self.path(rel)?;
        match std::fs::metadata(&path) {
            Ok(m) => Ok(m.is_file()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    fn put(&self, rel: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.path(rel)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut tmp = path.clone().into_os_string();
        tmp.push(format!(".{}.tmp", std::process::id()));
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{}: {e}", path.display()));
        }
        self.touch_last(rel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_store_rejects_traversal() {
        let dir = std::env::temp_dir().join(format!("glia-ce2c-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = DirStore::new(&dir);
        for bad in [
            "../x",
            "/abs",
            "v1/../../x",
            "v1/./x",
            "v1//x",
            "",
            "V1/x",
            "v1/x\\..\\y",
            "v1/x y",
        ] {
            assert!(store.put(bad, b"x").is_err(), "put({bad:?}) accepted");
            assert!(store.get(bad).is_err(), "get({bad:?}) accepted");
            assert!(store.has(bad).is_err(), "has({bad:?}) accepted");
        }
        assert!(!dir.join("..").join("x").exists());

        // A valid path round-trips, leaves no staging file and touches LAST.
        let rel = "v1/0.5.1+p0123/ab/abcd.gpc";
        assert_eq!(store.get(rel), Ok(None));
        assert_eq!(store.has(rel), Ok(false));
        store.put(rel, b"object").expect("put");
        assert_eq!(store.get(rel), Ok(Some(b"object".to_vec())));
        assert_eq!(store.has(rel), Ok(true));
        assert!(dir.join("v1/0.5.1+p0123").join(LAST_FILE).is_file());
        let left: Vec<_> = std::fs::read_dir(dir.join("v1/0.5.1+p0123/ab"))
            .expect("fan-out dir")
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["abcd.gpc"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
