//! **targets** (HT.7): pyo3 surface for the build-target domain — the
//! snapshot step `targets_sync` (`glia_snapshots::targets_sync`, HT.3) and
//! `PyGraph.targets_owners` / `targets_deps` / `targets_rdeps` /
//! `targets_affected` over `glia_engine::targets` (HT.6), the answers the CLI's
//! `glia targets` gives, as native dicts / lists (LD.2).
//!
//! The snapshot step writes `<repo>/.glia/targets-snapshot/` and never runs
//! inside a build; the answers read it through `targets::load_or_build` (the
//! `.glia/graph/targets.gmap` sidecar, rebuilt when the snapshot or glia
//! moved) for the PyGraph's first repo root (its first graph's repo) and
//! never run bazel. Every row is tier `fact`. The GIL is released around the
//! sync and the sidecar build; an error (a failed sync, no snapshot, a
//! sidecar that cannot be written) is a `ValueError`.
//!
//! The bodies are pyo3-free (`sync_json`, `owners_json`, `walk_json`,
//! `affected_json`), so `cargo test -p glia-py` covers them. fired_on, on
//! stderr: the snapshot crate's `[targets] sync ..` and the engine's
//! `[targets] sidecar ..` / `[targets] answer kind=..`, then this surface's
//! `[targets] surface=py cmd=<sync|owners|deps|rdeps|affected> ..` once per
//! call.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use glia_engine::targets::{TargetsView, load_or_build, targets_dir};
use glia_graph::MergedGraph;
use glia_snapshots::{TargetsOptions, targets_sync};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::convert::to_py;
use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// The sync behind [`targets_sync_py`], minus pyo3: the library's sync, then
/// `[targets] surface=py cmd=sync workspace=<w> rows=<n>`; the summary as
/// JSON. `Err` is the library's plain message, nothing written then.
fn sync_json(
    repo: &str,
    bazel: &str,
    workspace: &str,
    expr: &str,
    from_file: Option<PathBuf>,
) -> Result<String, String> {
    let mut opts = TargetsOptions::in_workspace(workspace);
    opts.bazel = bazel.to_string();
    opts.expr = expr.to_string();
    opts.from = from_file;
    let summary = targets_sync(Path::new(repo), &opts)?;
    let ws = if summary.workspace.is_empty() {
        "."
    } else {
        summary.workspace.as_str()
    };
    eprintln!(
        "[targets] surface=py cmd=sync workspace={ws} rows={}",
        summary.targets
    );
    serde_json::to_string(&summary).map_err(|e| e.to_string())
}

/// **targets_sync** (HT.3 / HT.7): ask bazel once for the targets of one
/// workspace of the repo at `repo` (`bazel query '<expr>'
/// --output=streamed_jsonproto` run in `<repo>/<workspace>`), or read the
/// saved query output `from_file` instead, and write
/// `<repo>/.glia/targets-snapshot/`, replacing only that workspace's rows.
/// Never part of a build; the `PyGraph.targets_*` answers read it.
///
/// `bazel`: the bazel (or bazelisk) binary; `workspace`: repo-relative, `""`
/// the repo root; `expr`: the query expression (default every target and
/// source file, `//...:*`). Returns `{workspace, targets, rules,
/// source_files, external_labels, skipped, bazel_version}` (`bazel_version`
/// `"from-file"` for a saved output). Raises `ValueError` when the sync
/// fails (no workspace, bazel missing or failing, an unreadable file), and
/// writes nothing then. The GIL is released while it runs.
#[pyfunction]
#[pyo3(name = "targets_sync", signature = (repo, bazel="bazel", workspace="", expr="//...:*", from_file=None))]
fn targets_sync_py(
    py: Python<'_>,
    repo: &str,
    bazel: &str,
    workspace: &str,
    expr: &str,
    from_file: Option<PathBuf>,
) -> PyResult<Py<PyAny>> {
    let text = py
        .detach(|| sync_json(repo, bazel, workspace, expr, from_file))
        .map_err(PyValueError::new_err)?;
    to_py(py, Ok(text))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(targets_sync_py, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "targets", add: register } }

/// The repo root the answers read: the first graph's repo's recorded root,
/// else the least recorded root. `Err` when the graph records none.
fn first_root(merged: &MergedGraph, repo_roots: &BTreeMap<u64, String>) -> Result<PathBuf, String> {
    merged
        .graphs
        .iter()
        .find_map(|g| repo_roots.get(&g.repo.0))
        .or_else(|| repo_roots.values().min())
        .map(PathBuf::from)
        .ok_or_else(|| "this graph records no repo root".to_string())
}

/// The targets view of `root`; `Err` naming the sync when there is no
/// snapshot.
fn view_of(root: &Path) -> Result<TargetsView, String> {
    load_or_build(root)?.ok_or_else(|| {
        format!(
            "no targets snapshot at {}/; run targets_sync({:?}) first",
            targets_dir(root).display(),
            root.display().to_string()
        )
    })
}

/// [`PyGraph::targets_owners`] minus pyo3: `[{file, owners}]` per distinct
/// file in input order.
fn owners_json(root: &Path, files: &[String]) -> Result<String, String> {
    let v = view_of(root)?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut rows = 0usize;
    for f in files.iter().filter(|f| seen.insert(f.as_str())) {
        let owners = v.owners(std::slice::from_ref(f));
        rows += owners.len();
        out.push(serde_json::json!({ "file": f, "owners": owners }));
    }
    eprintln!(
        "[targets] surface=py cmd=owners files={} rows={rows}",
        out.len()
    );
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

/// [`PyGraph::targets_deps`] / [`PyGraph::targets_rdeps`] minus pyo3: the
/// `{results, absence}` envelope.
fn walk_json(
    root: &Path,
    label: &str,
    depth: usize,
    workspace: Option<&str>,
    reverse: bool,
) -> Result<String, String> {
    let v = view_of(root)?;
    let query = match workspace {
        Some(w) => format!("{w}|{}", label.trim()),
        None => label.trim().to_string(),
    };
    let answer = if reverse {
        v.rdeps(&query, depth)
    } else {
        v.deps(&query, depth)
    };
    eprintln!(
        "[targets] surface=py cmd={} rows={} absence={}",
        if reverse { "rdeps" } else { "deps" },
        answer.results.len(),
        answer.absence.as_ref().map_or("-", |a| a.reason)
    );
    serde_json::to_string(&answer).map_err(|e| e.to_string())
}

/// [`PyGraph::targets_affected`] minus pyo3: the `Affected` record for
/// `files`, or (none given) the work tree's changes against `base`.
fn affected_json(root: &Path, files: Option<&[String]>, base: &str) -> Result<String, String> {
    let v = view_of(root)?;
    let a = match files {
        Some(f) => v.affected(f),
        None => v.affected_since(root, base)?,
    };
    eprintln!(
        "[targets] surface=py cmd=affected changed={} rows={} tests={} unowned={} caveats={}",
        a.changed.len(),
        a.targets.len(),
        a.tests.len(),
        a.unowned.len(),
        a.caveats.len()
    );
    serde_json::to_string(&a).map_err(|e| e.to_string())
}

#[pymethods]
impl PyGraph {
    /// **targets_owners** (HT.6 / HT.7): the build targets naming each of
    /// `files` (repo-relative) in `srcs` / `hdrs` / `textual_hdrs`, read from
    /// the first repo root's targets snapshot (`targets_sync`). Returns a list
    /// of `{file, owners}`, one per distinct file in input order, each owner
    /// `{workspace, label, kind, rule_class, role, via, depth, file, line,
    /// tier}` (`line` 1-based, `tier` `"fact"`). Raises `ValueError` with no
    /// snapshot.
    fn targets_owners(&self, py: Python<'_>, files: Vec<String>) -> PyResult<Py<PyAny>> {
        let root = first_root(&self.merged, &self.repo_roots).map_err(PyValueError::new_err)?;
        let text = py
            .detach(|| owners_json(&root, &files))
            .map_err(PyValueError::new_err)?;
        to_py(py, Ok(text))
    }

    /// **targets_deps** (HT.6 / HT.7): what target `label` (`//main:app`,
    /// `//main`) reaches over the carry edges (DEPENDS_ON, RUNTIME_DEP,
    /// HAS_SRC), at most `depth` hops: targets, source files and external
    /// labels by (depth, workspace, label). `workspace` picks one workspace's
    /// label. Returns `{results, absence}`; an unknown label or one reaching
    /// nothing is a fact absence (`results` empty). Raises `ValueError` with
    /// no snapshot.
    #[pyo3(signature = (label, depth=3, workspace=None))]
    fn targets_deps(
        &self,
        py: Python<'_>,
        label: &str,
        depth: usize,
        workspace: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let root = first_root(&self.merged, &self.repo_roots).map_err(PyValueError::new_err)?;
        let text = py
            .detach(|| walk_json(&root, label, depth, workspace.as_deref(), false))
            .map_err(PyValueError::new_err)?;
        to_py(py, Ok(text))
    }

    /// **targets_rdeps** (HT.6 / HT.7): what reaches target `label` over the
    /// carry edges, at most `depth` hops; otherwise as `targets_deps`.
    #[pyo3(signature = (label, depth=3, workspace=None))]
    fn targets_rdeps(
        &self,
        py: Python<'_>,
        label: &str,
        depth: usize,
        workspace: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let root = first_root(&self.merged, &self.repo_roots).map_err(PyValueError::new_err)?;
        let text = py
            .detach(|| walk_json(&root, label, depth, workspace.as_deref(), true))
            .map_err(PyValueError::new_err)?;
        to_py(py, Ok(text))
    }

    /// **targets_affected** (HT.6 / HT.7): what the changed `files`
    /// (repo-relative) affect, or with `files=None` the work tree's changes
    /// (untracked included) against `base`. Returns `{changed, owners,
    /// targets, tests, unowned, caveats}`: every target reaching a changed
    /// file backwards at any depth, the test targets among them, the changed
    /// files no target names, and a caveat when a BUILD / `.bzl` / MODULE /
    /// WORKSPACE file changed. Raises `ValueError` with no snapshot or an
    /// unknown `base`.
    #[pyo3(signature = (files=None, base="HEAD"))]
    fn targets_affected(
        &self,
        py: Python<'_>,
        files: Option<Vec<String>>,
        base: &str,
    ) -> PyResult<Py<PyAny>> {
        let root = first_root(&self.merged, &self.repo_roots).map_err(PyValueError::new_err)?;
        let text = py
            .detach(|| affected_json(&root, files.as_deref(), base))
            .map_err(PyValueError::new_err)?;
        to_py(py, Ok(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use glia_snapshots::DEFAULT_EXPR;

    /// The pyo3 default of `expr` is written out (so the surface shows the
    /// value); it must stay the snapshot step's default.
    #[test]
    fn the_expr_default_is_the_sync_default() {
        let src = include_str!("targets.rs");
        assert!(
            src.contains(&format!("expr=\"{DEFAULT_EXPR}\"")),
            "targets_sync's signature names expr={DEFAULT_EXPR:?}"
        );
    }

    /// A scratch dir under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).expect("mkdir");
        let mut entries: Vec<_> = std::fs::read_dir(from)
            .expect("read_dir")
            .flatten()
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let dst = to.join(e.file_name());
            if e.file_type().expect("file type").is_dir() {
                copy_tree(&e.path(), &dst);
            } else {
                std::fs::copy(e.path(), &dst).expect("copy");
            }
        }
    }

    /// engine/tests/data/targets' two workspaces under a scratch `repo/`,
    /// synced from snapshots/tests/data/bazel/'s query outputs.
    fn synced(name: &str) -> (Scratch, PathBuf) {
        let top = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .to_path_buf();
        let root = std::env::temp_dir().join(format!("glia-ht7-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        for ws in ["cpp", "go"] {
            copy_tree(
                &top.join("engine/tests/data/targets").join(ws),
                &repo.join(ws),
            );
            let raw = std::fs::read_to_string(
                top.join(format!("snapshots/tests/data/bazel/{ws}-stage3.jsonl")),
            )
            .expect("fixture");
            let abs = std::fs::canonicalize(repo.join(ws)).expect("canonical");
            let from = root.join(format!("{ws}.jsonl"));
            std::fs::write(&from, raw.replace("/WORKSPACE", &abs.display().to_string()))
                .expect("write");
            let s = sync_json(
                repo.to_str().expect("utf-8"),
                "bazel",
                ws,
                DEFAULT_EXPR,
                Some(from),
            )
            .expect("sync");
            let v: serde_json::Value = serde_json::from_str(&s).expect("json");
            assert_eq!(v["workspace"], ws);
            assert_eq!(v["bazel_version"], "from-file");
        }
        (Scratch(root), repo)
    }

    #[test]
    fn the_answers_over_a_synced_snapshot() {
        let (_s, repo) = synced("answers");
        let owners: serde_json::Value = serde_json::from_str(
            &owners_json(
                &repo,
                &[
                    "cpp/lib/hello-time.h".to_string(),
                    "nope.cc".to_string(),
                    "cpp/lib/hello-time.h".to_string(),
                ],
            )
            .expect("owners"),
        )
        .expect("json");
        assert_eq!(owners.as_array().expect("list").len(), 2, "{owners}");
        assert_eq!(owners[0]["owners"][0]["label"], "//lib:hello-time");
        assert_eq!(owners[0]["owners"][0]["tier"], "fact");
        assert_eq!(owners[1]["owners"], serde_json::json!([]));

        let deps: serde_json::Value = serde_json::from_str(
            &walk_json(&repo, "//main:hello-world", 1, None, false).expect("deps"),
        )
        .expect("json");
        assert!(deps["absence"].is_null());
        assert_eq!(deps["results"].as_array().expect("rows").len(), 4);
        let rdeps: serde_json::Value = serde_json::from_str(
            &walk_json(&repo, "//fortune:fortune", 1, Some("cpp"), true).expect("rdeps"),
        )
        .expect("json");
        assert_eq!(rdeps["absence"]["reason"], "unknown_symbol");

        let a: serde_json::Value = serde_json::from_str(
            &affected_json(&repo, Some(&["go/fortune/fortune.go".to_string()]), "HEAD")
                .expect("affected"),
        )
        .expect("json");
        assert_eq!(a["tests"][0]["label"], "//fortune:fortune_test");
        assert_eq!(a["targets"].as_array().expect("rows").len(), 3);
    }

    #[test]
    fn no_snapshot_and_a_failed_sync_are_errors() {
        let root = std::env::temp_dir().join(format!("glia-ht7-py-{}-none", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let _s = Scratch(root.clone());
        let err = owners_json(&root, &["a.cc".to_string()]).expect_err("no snapshot");
        assert!(
            err.contains("no targets snapshot") && err.contains("targets_sync"),
            "{err}"
        );
        let err = sync_json(
            root.to_str().expect("utf-8"),
            "bazel",
            "",
            DEFAULT_EXPR,
            None,
        )
        .expect_err("no workspace");
        assert!(err.contains("no bazel workspace"), "{err}");
        assert!(!targets_dir(&root).exists());
    }

    #[test]
    fn the_first_root_is_the_first_graphs_repo() {
        let merged = MergedGraph::default();
        let mut roots = BTreeMap::new();
        assert!(first_root(&merged, &roots).is_err());
        roots.insert(7, "/b".to_string());
        roots.insert(3, "/a".to_string());
        assert_eq!(
            first_root(&merged, &roots).expect("root"),
            PathBuf::from("/a")
        );
    }
}
