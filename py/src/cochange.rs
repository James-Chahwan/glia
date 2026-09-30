//! **cochange** (CC.11c): pyo3 surface for `glia_engine::cochange` (CC.11a /
//! CC.11b) — what else usually changes with a change, from git history.
//! `PyGraph.cochange(files, ..)` answers for given repo-relative files over a
//! built graph and its repo roots (`cochange_multi`: the pairwise rules of the
//! CO_CHANGES edges plus the multi-file rules counted from each root's
//! history snapshot); the module function `cochange_vs_rev(repo_path,
//! base="HEAD", ..)` builds the repo's working tree itself and queries its
//! change against the rev, so it takes a path. Each returns the engine's
//! `Cochange` as a native dict (LD.2); `min_confidence` outside [0, 1] and an
//! engine `Err` raise `ValueError`.
//!
//! Transport only: the rules, the floors, the static-link test and the
//! `[cochange-suggest] ... source=multi` marker live in the engine. The
//! helpers the pyo3 entry points delegate to are pyo3-free, so `cargo test -p
//! glia-py` covers them (see the crate doc).

use std::collections::BTreeMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::cochange::{Cochange, CochangeArgs, cochange_multi, cochange_vs_rev};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;
use crate::registry::ModuleFns;

// The signatures below spell `min_support=3, top=20` as literals so
// `__text_signature__` shows them (and `min_confidence=0.3`, the engine's 300
// per mille); `options_default_is_the_engine_default` pins them.

/// The engine's floors from the keyword arguments; `Err` for a
/// `min_confidence` outside [0, 1] (NaN included).
fn options(
    min_confidence: f64,
    min_support: u32,
    top: usize,
    unlinked_only: bool,
) -> Result<CochangeArgs, String> {
    if !(0.0..=1.0).contains(&min_confidence) {
        return Err(format!(
            "min_confidence must be a share in [0, 1], got {min_confidence}"
        ));
    }
    let mut args = CochangeArgs::default();
    // In [0, 1000] after the check, so the cast neither truncates nor wraps.
    args.min_confidence_permille = (min_confidence * 1000.0).round() as u32;
    args.min_support = min_support;
    args.top = top;
    args.unlinked_only = unlinked_only;
    Ok(args)
}

/// The body of [`PyGraph::cochange`], minus pyo3.
fn files_answer(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    files: &[String],
    args: &CochangeArgs,
) -> Cochange {
    cochange_multi(merged, repo_roots, files, args)
}

/// The body of [`cochange_vs_rev_py`], minus pyo3.
fn rev_answer(repo_path: &str, base: &str, args: &CochangeArgs) -> Result<Cochange, String> {
    cochange_vs_rev(repo_path, base, args)
}

#[pymethods]
impl PyGraph {
    /// **cochange** (CC.11c): the files that usually change with `files`
    /// (repo-relative paths, e.g. `["svc/report.py"]`), from the git history
    /// the build ingested (`history_sync` before `generate`). A rule
    /// "antecedent -> file" holds when, of the commits that changed the
    /// antecedent (one query file, or several together), at least
    /// `min_confidence` (a share in [0, 1]) and `min_support` commits also
    /// changed the file; one row per suggested file (its best rule), at most
    /// `top` rows; `unlinked_only=True` keeps only the files no static link
    /// joins to the query.
    ///
    /// Returns a dict `{query_files, unmapped, rows, absence}`: `rows`
    /// `{file, module_qname, antecedent, support, antecedent_commits,
    /// confidence_permille, link, source, tier, note}` ordered by confidence,
    /// then support. `confidence_permille` is `1000 * support /
    /// antecedent_commits` (directional); `link` is `direct`, `bridged` or
    /// `none` (then `note` says: a blind spot, or coupling outside code);
    /// `source` is `pairwise` (one antecedent file) or `multi` (several);
    /// `module_qname` is empty for a file no MODULE names (a yaml, a
    /// migration); `tier` is always `heuristic`. `unmapped` lists the query
    /// files no MODULE names; `absence` is set exactly when `rows` is empty
    /// (`no_history`: no snapshot was ingested; `no_match`: nothing past the
    /// floors). Raises ValueError for `min_confidence` outside [0, 1].
    #[pyo3(signature = (files, min_confidence=0.3, min_support=3, top=20, unlinked_only=false))]
    fn cochange(
        &self,
        py: Python<'_>,
        files: Vec<String>,
        min_confidence: f64,
        min_support: u32,
        top: usize,
        unlinked_only: bool,
    ) -> PyResult<Py<PyAny>> {
        let args = options(min_confidence, min_support, top, unlinked_only)
            .map_err(PyValueError::new_err)?;
        let mut answer = files_answer(&self.merged, &self.repo_roots, &files, &args);
        if let Some(a) = answer.absence.as_mut() {
            a.unparsed_files = self.parse_errors.len();
        }
        to_py(py, serde_json::to_string(&answer))
    }
}

/// **cochange_vs_rev** (CC.11c): `PyGraph.cochange` for the working tree's
/// change against git rev `base` (a branch, tag, sha or `"HEAD~N"`; default
/// `"HEAD"`) of the repo at `repo_path`: its tracked changes, deletions, both
/// paths of a rename and untracked files, nothing under `.glia/`. Builds the
/// working tree itself (reading its history snapshot) and writes nothing.
/// Same keywords and dict as `PyGraph.cochange`; a clean tree answers with a
/// `no_match` absence. Raises ValueError for `min_confidence` outside [0, 1]
/// or on a git or build failure (an unknown rev, a path that is no
/// directory).
#[pyfunction]
#[pyo3(name = "cochange_vs_rev", signature = (repo_path, base="HEAD", min_confidence=0.3, min_support=3, top=20, unlinked_only=false))]
fn cochange_vs_rev_py(
    py: Python<'_>,
    repo_path: &str,
    base: &str,
    min_confidence: f64,
    min_support: u32,
    top: usize,
    unlinked_only: bool,
) -> PyResult<Py<PyAny>> {
    let args =
        options(min_confidence, min_support, top, unlinked_only).map_err(PyValueError::new_err)?;
    let answer = rev_answer(repo_path, base, &args).map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&answer))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(cochange_vs_rev_py, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "cochange", add: register } }

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};

    use super::*;

    /// A scratch dir under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const ADMIN: &str = "svc/admin.py";
    const REPORT: &str = "svc/report.py";
    const PAGE: &str = "web/page.ts";
    const REPORT_PY: &str = "def format_report(rows):\n    return \", \".join(rows)\n";

    /// CC.11a's acceptance tree under a fresh scratch dir (`repo/`), with its
    /// synthetic history snapshot: admin 45 commits, 3 with report (report's
    /// only 3), 20 with page (page's only 20), 22 alone; admin imports report.
    fn tree(name: &str) -> (Scratch, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("glia-cc11c-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        for (rel, text) in [
            (
                ADMIN,
                "from svc.report import format_report\n\n\ndef admin_summary(rows):\n    return format_report(rows)\n",
            ),
            (REPORT, REPORT_PY),
            (
                PAGE,
                "export function renderPage(title: string): string {\n  return title;\n}\n",
            ),
        ] {
            let p = top.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("scratch dir");
            std::fs::write(&p, text).expect("write fixture");
        }
        let mut commits = Vec::new();
        for (files, n) in [
            (&[ADMIN, REPORT][..], 3),
            (&[ADMIN, PAGE][..], 20),
            (&[ADMIN][..], 22),
        ] {
            for _ in 0..n {
                let i = commits.len() + 1;
                commits.push(HistoryCommit {
                    c: format!("{i:03}{}", "0".repeat(37)),
                    t: 1_767_225_600 + i64::try_from(i).expect("small") * 3_600,
                    files: files
                        .iter()
                        .map(|p| HistoryFile {
                            p: (*p).to_string(),
                            a: Some(1),
                            d: Some(0),
                            from: None,
                        })
                        .collect(),
                });
            }
        }
        commits.reverse();
        let head = commits[0].c.clone();
        write_history(
            &top,
            HistoryMeta::new(head, 2000, None, String::new()),
            &commits,
            &[],
        )
        .expect("write snapshot");
        (Scratch(root), top)
    }

    /// The object every entry point returns, as `to_py` hands it to Python.
    fn value(a: &Cochange) -> serde_json::Value {
        serde_json::from_str(&serde_json::to_string(a).expect("serialises")).expect("valid JSON")
    }

    fn files(v: &serde_json::Value) -> Vec<&str> {
        v["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|r| r["file"].as_str().expect("file"))
            .collect()
    }

    fn git(top: &Path, gitconfig: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=glia",
                "-c",
                "user.email=glia@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .arg("-C")
            .arg(top)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap_or_else(|e| panic!("CC.11c cochange_vs_rev test needs a `git` binary: {e}"));
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The pyo3 signatures' literal defaults are the engine's, and
    /// `min_confidence` maps to per mille, refusing a share outside [0, 1].
    #[test]
    fn options_default_is_the_engine_default() {
        let o = options(0.3, 3, 20, false).expect("defaults");
        let d = CochangeArgs::default();
        assert_eq!(
            (
                o.min_confidence_permille,
                o.min_support,
                o.top,
                o.unlinked_only
            ),
            (
                d.min_confidence_permille,
                d.min_support,
                d.top,
                d.unlinked_only
            )
        );
        let o = options(0.25, 4, 1, true).expect("keywords");
        assert_eq!(
            (
                o.min_confidence_permille,
                o.min_support,
                o.top,
                o.unlinked_only
            ),
            (250, 4, 1, true)
        );
        assert_eq!(
            options(1.0, 3, 20, false).map(|o| o.min_confidence_permille),
            Ok(1000)
        );
        assert_eq!(
            options(0.0, 3, 20, false).map(|o| o.min_confidence_permille),
            Ok(0)
        );
        for bad in [1.5, -0.1, f64::NAN, f64::INFINITY] {
            let e = options(bad, 3, 20, false).expect_err("outside [0, 1]");
            assert!(e.contains("[0, 1]"), "{e}");
        }
    }

    /// CC.11c: the helper behind `PyGraph.cochange` returns the documented
    /// object in field order; report -> admin is the first row (3/3, linked),
    /// and unlinked_only on admin keeps web/page.ts only.
    #[test]
    fn files_helper_returns_the_documented_object() {
        let (_scratch, top) = tree("files");
        let built = glia_engine::generate_one(top.to_str().expect("utf-8")).expect("build");
        let args = CochangeArgs::default();
        let a = files_answer(
            &built.merged,
            &built.repo_roots,
            &[REPORT.to_string()],
            &args,
        );
        let text = serde_json::to_string(&a).expect("serialises");
        let order: Vec<usize> = [
            "\"query_files\":",
            "\"unmapped\":",
            "\"rows\":",
            "\"absence\":",
        ]
        .iter()
        .map(|k| text.find(k).unwrap_or(usize::MAX))
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "field order: {text}");
        let v = value(&a);
        let row = &v["rows"][0];
        assert_eq!(row["file"], ADMIN);
        assert_eq!(
            (
                row["support"].as_u64(),
                row["antecedent_commits"].as_u64(),
                row["confidence_permille"].as_u64()
            ),
            (Some(3), Some(3), Some(1000))
        );
        assert_eq!(
            (
                row["link"].as_str(),
                row["source"].as_str(),
                row["tier"].as_str()
            ),
            (Some("direct"), Some("pairwise"), Some("heuristic"))
        );
        assert!(v["absence"].is_null());

        let unlinked = options(0.3, 3, 20, true).expect("keywords");
        let a = files_answer(
            &built.merged,
            &built.repo_roots,
            &[ADMIN.to_string()],
            &unlinked,
        );
        assert_eq!(files(&value(&a)), [PAGE]);

        let strict = options(0.3, 4, 20, false).expect("keywords");
        let a = files_answer(
            &built.merged,
            &built.repo_roots,
            &[REPORT.to_string()],
            &strict,
        );
        assert_eq!(value(&a)["absence"]["reason"], "no_match");
    }

    /// CC.11c: the helper behind `cochange_vs_rev` queries the working tree's
    /// change (report.py edited -> admin), a clean tree is a `no_match`
    /// absence, and an unknown rev is the engine's error.
    #[test]
    fn rev_helper_queries_the_working_tree_change() {
        let (scratch, top) = tree("rev");
        let gitconfig = scratch.0.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        git(&top, &gitconfig, &["init", "-q"]);
        git(&top, &gitconfig, &["add", "svc", "web"]);
        git(&top, &gitconfig, &["commit", "-q", "-m", "tree"]);
        let repo = top.to_str().expect("utf-8");
        let args = CochangeArgs::default();

        let clean = rev_answer(repo, "HEAD", &args).expect("clean");
        let v = value(&clean);
        assert_eq!(v["query_files"], serde_json::json!([]));
        assert_eq!(v["absence"]["reason"], "no_match");

        std::fs::write(top.join(REPORT), format!("{REPORT_PY}# edited\n")).expect("edit");
        let a = rev_answer(repo, "HEAD", &args).expect("answer");
        let v = value(&a);
        assert_eq!(v["query_files"], serde_json::json!([REPORT]));
        assert_eq!(files(&v), [ADMIN]);

        let err = rev_answer(repo, "no-such-rev", &args).expect_err("unknown rev");
        assert!(err.contains("no-such-rev"), "{err}");
    }
}
