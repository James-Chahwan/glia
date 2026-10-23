//! pyo3 surface for `glia_engine::watch` (DC.9): the `WatchSession` class, one
//! repo's incremental build held open in the process, registered through this
//! module's own `ModuleFns`. The engine does the work (DC.7a-c); this module
//! is transport.
//!
//! - `WatchSession(repo, overlay=True)` opens the session: it loads the repo's
//!   parse cache sidecar once, walks and builds (`WatchSession::open`).
//!   `ValueError` when it cannot (the path is not a directory), as `generate`
//!   raises for a build error.
//! - `apply(paths)` -> dict: the update for the paths the caller saw change
//!   (absolute under the repo, or repo-relative; a `str` or `os.PathLike`
//!   each), the engine's `WatchUpdate` as a native dict (LD.2): `{seq, files:
//!   {added, modified, removed, reparsed, reused}, groups_rebuilt,
//!   full_rebuild, total_nodes, total_edges, counts, nodes, edges,
//!   phase_ms}`. An update that changed nothing leaves `seq` where it was
//!   with empty rows. `RuntimeError` when the repo is no longer a directory.
//! - `poll()` -> dict | None: the session's own stat scan, then `apply` of
//!   what it found (`WatchSession::poll`); `None` for a quiet scan and for a
//!   touch that changed no content.
//! - `snapshot()` -> `PyGraph`: an independent copy of the session's current
//!   graph (a clone of its `GenerateResult`, the memory a fresh `generate`
//!   costs), marked with the session's `overlay`. A later `apply` never
//!   changes a snapshot the caller still serves.
//! - `persist()`: writes the current graph as the repo's layout
//!   `<repo>/.glia/graph/` (writer `watch`), then the held parse cache beside
//!   it. A session opened with `overlay=False` is refused (`ValueError`) and
//!   writes nothing, as `PyGraph.save_to_default` refuses an
//!   `overlay=False` graph: the default dir holds the overlay-applied graph.
//!   A write error is an `OSError`.
//!
//! Nothing is written under the repo but by `persist()`.
//!
//! **Threads.** `__init__`, `apply`, `poll`, `snapshot` and `persist` release
//! the GIL (`Python::detach`) while the engine works, so a watcher thread can
//! update the session while other threads keep serving queries on a
//! snapshot. The session itself takes one caller at a time: `apply` and
//! `poll` borrow it mutably, so a second Python thread that calls into the
//! same session meanwhile gets pyo3's `RuntimeError` ("Already borrowed")
//! instead of waiting. Drive one session from one thread.
//!
//! fired_on, on stderr: `[watch] session repo=<repo> overlay=<true|false>
//! surface=py` once per constructed session (CK.1's `surface=py`
//! convention), after the engine's `[watch] open ...` line. The engine's
//! `[watch] update` / `noop` / `poll` lines follow per call.
//!
//! The pyfunction-free helpers ([`open_session`], [`update_json`],
//! [`persist_default`]) hold the logic, so `cargo test -p glia-py` covers it
//! without an interpreter (see the crate doc).

use std::path::{Path, PathBuf};

use pyo3::exceptions::{PyOSError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;

use glia_engine::BuildOptions;
use glia_engine::persist::default_layout_dir;
use glia_engine::watch::{WatchSession, WatchUpdate};

use crate::convert::to_py;
use crate::graph::{PyGraph, default_dir_refusal};
use crate::registry::ModuleFns;

/// The fired_on line a constructed session prints.
fn session_marker(repo: &str, overlay: bool) -> String {
    format!("[watch] session repo={repo} overlay={overlay} surface=py")
}

/// Open the session behind `WatchSession(repo, overlay)`, minus pyo3: the
/// engine's open with the overlay switched as asked, then the marker. `Err`
/// is the engine's message.
fn open_session(repo: &str, overlay: bool) -> Result<WatchSession, String> {
    let opts = BuildOptions::default().with_overlay(overlay);
    let session = WatchSession::open(repo, &opts)?;
    eprintln!("{}", session_marker(repo, overlay));
    Ok(session)
}

/// One update as the JSON text [`to_py`] turns into the native dict: the
/// `WatchUpdate`'s serde fields in declaration order.
pub(crate) fn update_json(u: &WatchUpdate) -> Result<String, serde_json::Error> {
    serde_json::to_string(u)
}

/// Why [`persist_default`] wrote nothing, or stopped part way.
#[derive(Debug)]
enum PersistError {
    /// The default-dir rule: an `overlay=False` session (`ValueError`).
    Refused(String),
    /// The layout or the parse cache could not be written (`OSError`).
    Write(String),
}

/// `persist()` minus pyo3: the layout at the repo's default dir, then the
/// parse cache beside it; refused before any write for an overlay-free
/// session. Returns the layout dir.
fn persist_default(
    session: &WatchSession,
    repo: &str,
    overlay: bool,
) -> Result<PathBuf, PersistError> {
    let dir = default_layout_dir(Path::new(repo));
    if let Some(why) = default_dir_refusal(overlay) {
        return Err(PersistError::Refused(format!(
            "persist({}): {why}",
            dir.display()
        )));
    }
    session
        .persist(&dir)
        .map_err(|e| PersistError::Write(format!("persist({}): {e}", dir.display())))?;
    session.save_cache().map_err(|e| {
        PersistError::Write(format!(
            "persist({}): parse cache: {e}",
            dir.display()
        ))
    })?;
    Ok(dir)
}

/// One repo's incremental build held open in this process. Opening it loads
/// the repo's parse cache sidecar once and builds; `apply(paths)` updates it
/// from the paths the caller saw change and `poll()` from its own stat scan,
/// each returning the update as a dict (`poll()`: None when nothing
/// changed). `snapshot()` is an independent `PyGraph` of the current build,
/// unchanged by later updates; `persist()` writes the repo's default layout
/// and the parse cache, and refuses an `overlay=False` session. Nothing else
/// writes under the repo. The GIL is released while the engine works; drive
/// one session from one thread (a concurrent second call raises
/// RuntimeError "Already borrowed"). ValueError when `repo` is not a
/// directory.
#[pyclass(name = "WatchSession")]
pub(crate) struct PyWatchSession {
    session: WatchSession,
    /// The repo path as given: the default layout dir is under it.
    repo: String,
    /// Whether the session's builds apply `.glia/overlay.toml`.
    overlay: bool,
}

#[pymethods]
impl PyWatchSession {
    #[new]
    #[pyo3(signature = (repo, overlay=true))]
    fn new(py: Python<'_>, repo: &str, overlay: bool) -> PyResult<Self> {
        let session = py
            .detach(|| open_session(repo, overlay))
            .map_err(PyValueError::new_err)?;
        Ok(Self {
            session,
            repo: repo.to_string(),
            overlay,
        })
    }

    /// Bring the session up to date with `paths`, the files and directories
    /// the caller saw change (absolute under the repo, or repo-relative). A
    /// path not named is not re-read. Returns the update as a dict: `seq`,
    /// `files` (`added`, `modified`, `removed`, `reparsed`, `reused`),
    /// `groups_rebuilt`, `full_rebuild`, `total_nodes`, `total_edges`,
    /// `counts`, `nodes`, `edges`, `phase_ms`. An update that changed
    /// nothing keeps `seq` and has no rows. RuntimeError when the repo is no
    /// longer a directory (the session keeps its previous state).
    fn apply(&mut self, py: Python<'_>, paths: Vec<PathBuf>) -> PyResult<Py<PyAny>> {
        let session = &mut self.session;
        let update = py
            .detach(|| session.apply(&paths))
            .map_err(PyRuntimeError::new_err)?;
        to_py(py, update_json(&update))
    }

    /// Stat the session's files, directories and repo-level inputs and
    /// apply what changed: the update as `apply` returns it, or None when
    /// nothing changed (a quiet scan, or a touch with no content change).
    /// RuntimeError as `apply`.
    fn poll(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let session = &mut self.session;
        match py.detach(|| session.poll()).map_err(PyRuntimeError::new_err)? {
            Some(update) => Ok(Some(to_py(py, update_json(&update))?)),
            None => Ok(None),
        }
    }

    /// The session's current graph as an independent `PyGraph` (a copy:
    /// later updates do not change it), `overlay_applied` as the session's
    /// `overlay`. It shares the session's ranking cache (holder `watch`), so
    /// a ranked answer on two snapshots of one unchanged session builds its
    /// indexes once.
    fn snapshot(&self, py: Python<'_>) -> PyGraph {
        let session = &self.session;
        let result = py.detach(|| session.result().clone());
        // The clone's graph is the session's, so the session's ranking
        // indexes serve it (EA.7): a second snapshot's first ranked answer
        // reads them warm.
        PyGraph::from_result_with_rank(result, session.rank_cache()).with_overlay_applied(self.overlay)
    }

    /// Write the current graph as the repo's layout `<repo>/.glia/graph/`
    /// (what `load_from_gmap` reads), then the session's parse cache beside
    /// it. ValueError, writing nothing, for a session opened with
    /// `overlay=False`; OSError when a write fails.
    fn persist(&self, py: Python<'_>) -> PyResult<()> {
        let (session, repo, overlay) = (&self.session, self.repo.as_str(), self.overlay);
        match py.detach(|| persist_default(session, repo, overlay)) {
            Ok(_) => Ok(()),
            Err(PersistError::Refused(why)) => Err(PyValueError::new_err(why)),
            Err(PersistError::Write(why)) => Err(PyOSError::new_err(why)),
        }
    }
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyWatchSession>()?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "watch", add: register } }

#[cfg(test)]
mod tests {
    use super::{PersistError, open_session, persist_default, session_marker, update_json};
    use std::path::{Path, PathBuf};

    /// The keys of `apply()`'s dict, in order (`WatchUpdate`'s fields).
    const UPDATE_KEYS: [&str; 10] = [
        "seq",
        "files",
        "groups_rebuilt",
        "full_rebuild",
        "total_nodes",
        "total_edges",
        "counts",
        "nodes",
        "edges",
        "phase_ms",
    ];
    const FILES_KEYS: [&str; 5] = ["added", "modified", "removed", "reparsed", "reused"];

    fn python_repo(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("glia-dc9-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        std::fs::write(root.join("a.py"), "def foo():\n    return bar()\n\ndef bar():\n    return 1\n")
            .expect("write a.py");
        std::fs::write(root.join("b.py"), "def b():\n    return 2\n").expect("write b.py");
        root
    }

    /// Every path under `root`, relative and sorted.
    fn tree(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("readable dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                out.push(path.strip_prefix(root).expect("under root").to_path_buf());
            }
        }
        out.sort();
        out
    }

    /// The keys of a JSON object's text, in the order they appear: the order
    /// `json.loads` keeps in the dict (`serde_json::Value`'s map sorts them).
    fn key_order(text: &str, keys: &[&str]) -> Vec<usize> {
        keys.iter()
            .map(|k| text.find(&format!("\"{k}\":")).expect("key present"))
            .collect()
    }

    /// DC.9: an update from a temp-repo session serialises with exactly the
    /// keys `apply()` documents, top level and `files`, in that order, and
    /// carries the edit: `files.modified == ["a.py"]`, `seq` 1. A second
    /// apply of the unchanged file is a noop: `seq` stays, no rows.
    #[test]
    fn update_json_keys() {
        let root = python_repo("keys");
        let path = root.to_str().expect("utf-8 temp path").to_string();
        let mut s = open_session(&path, true).expect("open");
        std::fs::write(
            root.join("a.py"),
            "def foo():\n    return bar() + 1\n\ndef bar():\n    return 1\n\ndef baz():\n    return 3\n",
        )
        .expect("edit a.py");
        let u = s.apply(&[PathBuf::from("a.py")]).expect("apply");
        let text = update_json(&u).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&text).expect("json");
        let top = v.as_object().expect("an object");
        let mut keys: Vec<&str> = top.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut want = UPDATE_KEYS.to_vec();
        want.sort_unstable();
        assert_eq!(keys, want, "{text}");
        let order = key_order(&text, &UPDATE_KEYS);
        assert!(order.windows(2).all(|w| w[0] < w[1]), "field order: {text}");
        let files = top["files"].as_object().expect("files is an object");
        let mut fkeys: Vec<&str> = files.keys().map(String::as_str).collect();
        fkeys.sort_unstable();
        let mut fwant = FILES_KEYS.to_vec();
        fwant.sort_unstable();
        assert_eq!(fkeys, fwant, "{text}");
        assert_eq!(top["seq"], 1, "{text}");
        assert_eq!(files["modified"], serde_json::json!(["a.py"]), "{text}");
        assert_eq!(files["added"], serde_json::json!([]), "{text}");
        assert!(top["full_rebuild"].is_null(), "{text}");
        assert_eq!(top["total_nodes"], s.result().total_nodes, "{text}");
        assert!(
            top["nodes"].as_array().is_some_and(|rows| !rows.is_empty()),
            "the edit adds baz: {text}"
        );

        let again = s.apply(&[PathBuf::from("a.py")]).expect("apply");
        assert_eq!(again.seq, 1, "an unchanged file is a noop");
        assert!(again.nodes.is_empty() && again.edges.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// DC.9: `persist()` refuses an overlay-free session before any write,
    /// and an overlay-applied one writes the default layout plus the parse
    /// cache; opening and applying wrote nothing under the repo.
    #[test]
    fn persist_default_dir_rule() {
        let root = python_repo("persist");
        let path = root.to_str().expect("utf-8 temp path").to_string();
        let before = tree(&root);

        let bare = open_session(&path, false).expect("open overlay=false");
        match persist_default(&bare, &path, false) {
            Err(PersistError::Refused(why)) => assert!(why.contains("overlay=False"), "{why}"),
            other => panic!("an overlay=false session must be refused: {other:?}"),
        }
        assert_eq!(tree(&root), before, "the refusal wrote nothing");

        let mut s = open_session(&path, true).expect("open");
        s.apply(&[root.join("b.py")]).expect("apply");
        assert_eq!(tree(&root), before, "open and apply write nothing under the repo");
        let dir = persist_default(&s, &path, true).expect("persist");
        assert!(dir.join("manifest.json").is_file(), "{:?}", tree(&root));
        assert!(dir.join("parse_cache.bin").is_file(), "{:?}", tree(&root));
        let _ = std::fs::remove_dir_all(&root);

        let missing = std::env::temp_dir().join(format!("glia-dc9-missing-{}", std::process::id()));
        assert!(open_session(missing.to_str().expect("utf-8"), true).is_err());
    }

    #[test]
    fn marker_text() {
        assert_eq!(
            session_marker("/r", false),
            "[watch] session repo=/r overlay=false surface=py"
        );
    }
}
