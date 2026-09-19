//! **tests_for** (LE.3b): the tests to run for a change — the test cases that
//! reach the changed nodes, located and tiered fact / derived / heuristic —
//! over the engine's `tests_for::{tests_for, tests_for_diff, tests_for_rev}`.
//!
//! `PyGraph.tests_for(qnames, ..)` and `PyGraph.tests_for_diff(diff_text, ..)`
//! answer over a built graph; the module function `tests_for_rev(repo_path,
//! base="HEAD", ..)` builds the repo's working tree and the rev itself (LE.1b's
//! graph delta), so it takes a path. Each returns the engine's answer as a
//! native dict (LD.2); an engine `Err` raises `ValueError`.
//!
//! Transport only: the walk, the tiers, the location of every row and the
//! `[tests-for] seeds=..` marker live in the engine. The helpers the pyo3
//! entry points delegate to are pyo3-free, so `cargo test -p glia-py`
//! covers them (see the crate doc).

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::tests_for::{
    TestsFor, TestsForArgs, tests_for, tests_for_diff, tests_for_rev,
};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// The engine's options from the keyword arguments every entry point takes.
/// The pyo3 signatures spell `depth`'s default as the literal `6`, so Python
/// introspection shows it (a constant renders as `...`); the unit test pins it
/// to the engine's `DEFAULT_MAX_DEPTH`.
fn options(depth: usize, scope: Option<&str>, module_level: bool) -> TestsForArgs {
    let mut args = TestsForArgs::default();
    args.max_depth = depth;
    args.scope = scope.map(str::to_string);
    args.module_level = module_level;
    args
}

/// The body of [`PyGraph::tests_for`], minus pyo3.
fn qname_answer(
    merged: &MergedGraph,
    qnames: &[String],
    depth: usize,
    scope: Option<&str>,
    module_level: bool,
) -> Result<TestsFor, String> {
    let names: Vec<&str> = qnames.iter().map(String::as_str).collect();
    tests_for(merged, &names, &options(depth, scope, module_level))
}

/// The body of [`PyGraph::tests_for_diff`], minus pyo3.
fn diff_answer(
    merged: &MergedGraph,
    diff_text: &str,
    depth: usize,
    scope: Option<&str>,
    module_level: bool,
) -> Result<TestsFor, String> {
    tests_for_diff(merged, diff_text, &options(depth, scope, module_level))
}

/// The body of [`tests_for_rev_py`], minus pyo3.
fn rev_answer(
    repo_path: &str,
    base: &str,
    depth: usize,
    scope: Option<&str>,
    module_level: bool,
) -> Result<TestsFor, String> {
    tests_for_rev(repo_path, base, &options(depth, scope, module_level))
}

#[pymethods]
impl PyGraph {
    /// **tests_for** (LE.3b): the tests to run when the nodes `qnames` name
    /// (qnames or bare names) change. The test cases reaching them backward
    /// within `depth` hops over calls, TESTS edges and the cross-service
    /// links (an integration test's HTTP call to a changed handler's route).
    /// `scope` (a path or project label) keeps the tests under it;
    /// `module_level=False` leaves out the heuristic tier.
    ///
    /// Returns a dict `{seeds, tests, test_files, untested, unresolved,
    /// absence}`: `tests` rows `{qname, name, kind, file, line, tier, reason,
    /// depth, covers, path}` ordered fact, derived, heuristic, then by depth;
    /// `tier` is `fact` (a TESTS edge straight to the seed, or the seed is a
    /// test), `derived` (reached through other edges) or `heuristic` (a test
    /// module paired by name with a seed's module); `path` is the witness as
    /// `[qname, category]` hops from the test to its nearest seed; lines are
    /// 1-based. `test_files` is the sorted files of the rows; `untested` the
    /// seeds no test case reaches; `unresolved` the names no node has;
    /// `absence` is set exactly when `tests` is empty. Raises ValueError on
    /// an empty `qnames` or more seeds than the engine walks from.
    #[pyo3(signature = (qnames, depth=6, scope=None, module_level=true))]
    fn tests_for(
        &self,
        py: Python<'_>,
        qnames: Vec<String>,
        depth: usize,
        scope: Option<&str>,
        module_level: bool,
    ) -> PyResult<Py<PyAny>> {
        let answer = qname_answer(&self.merged, &qnames, depth, scope, module_level)
            .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }

    /// **tests_for_diff** (LE.3b): `tests_for` seeded by a unified diff's
    /// added lines (each resolved to the narrowest node spanning it), or by a
    /// changed-file list (one path per line: every node of each file). Same
    /// keywords and dict as `tests_for`.
    #[pyo3(signature = (diff_text, depth=6, scope=None, module_level=true))]
    fn tests_for_diff(
        &self,
        py: Python<'_>,
        diff_text: &str,
        depth: usize,
        scope: Option<&str>,
        module_level: bool,
    ) -> PyResult<Py<PyAny>> {
        let answer = diff_answer(&self.merged, diff_text, depth, scope, module_level)
            .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }
}

/// **tests_for_rev** (LE.3b): `PyGraph.tests_for` seeded by what the working
/// tree's change against git rev `base` (a branch, tag, sha or `"HEAD~N"`;
/// default `"HEAD"`) did to the graph of the repo at `repo_path`: the added,
/// moved and edited nodes and the surviving ends of every call-like edge
/// gained or lost (a deleted test seeds what it called). Builds both sides
/// itself and saves the parse-cache sidecar
/// (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored) as
/// `generate(incremental=True)` does, never a `.gmap` layout. Same keywords
/// and dict as `tests_for`; raises ValueError on a git or build failure.
#[pyfunction]
#[pyo3(name = "tests_for_rev", signature = (repo_path, base="HEAD", depth=6, scope=None, module_level=true))]
fn tests_for_rev_py(
    py: Python<'_>,
    repo_path: &str,
    base: &str,
    depth: usize,
    scope: Option<&str>,
    module_level: bool,
) -> PyResult<Py<PyAny>> {
    let answer =
        rev_answer(repo_path, base, depth, scope, module_level).map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&answer))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(tests_for_rev_py, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "tests_for", add: register } }

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use glia_engine::tests_for::DEFAULT_MAX_DEPTH;

    use super::*;

    /// A scratch dir under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SERVICE_PY: &str = "def price(order):\n    return order\n\n\n\
def place(order):\n    return price(order)\n";

    /// `shop/service.py` (`place` calls `price`) and `shop/tests/test_service.py`
    /// (`test_price` calls `price`), under a fresh scratch dir.
    fn shop(name: &str) -> (Scratch, PathBuf) {
        let root = std::env::temp_dir().join(format!("glia-le3b-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        for (rel, text) in [
            ("shop/service.py", SERVICE_PY),
            (
                "shop/tests/test_service.py",
                "from shop.service import price\n\n\ndef test_price():\n    assert price(1) == 1\n",
            ),
        ] {
            let p = top.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("scratch dir");
            std::fs::write(&p, text).expect("write fixture");
        }
        (Scratch(root), top)
    }

    /// The object every entry point returns, as `to_py` hands it to Python.
    fn value(a: &TestsFor) -> serde_json::Value {
        serde_json::from_str(&serde_json::to_string(a).expect("serialises")).expect("valid JSON")
    }

    fn git(top: &Path, gitconfig: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=glia",
                "-c",
                "user.email=glia@example.invalid",
            ])
            .args([
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
            .unwrap_or_else(|e| panic!("LE.3b tests_for_rev test needs a `git` binary: {e}"));
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// LE.3b: the helpers behind pyo3 `tests_for` / `tests_for_diff` return
    /// the documented object in field order, the keywords reach the engine,
    /// and an empty seed list is the engine's error. The walk itself is
    /// covered by `engine/tests/tests_for.rs`.
    #[test]
    fn graph_helpers_return_the_documented_object() {
        let (_scratch, top) = shop("graph");
        let built = glia_engine::generate_one(top.to_str().expect("utf-8")).expect("build");
        let a = qname_answer(
            &built.merged,
            &["price".to_string()],
            DEFAULT_MAX_DEPTH,
            None,
            true,
        )
        .expect("answer");
        let v = value(&a);
        let text = serde_json::to_string(&a).expect("serialises");
        let order: Vec<usize> = [
            "\"seeds\":",
            "\"tests\":",
            "\"test_files\":",
            "\"untested\":",
            "\"unresolved\":",
            "\"absence\":",
        ]
        .iter()
        .map(|k| text.find(k).unwrap_or(usize::MAX))
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "field order: {text}");
        assert_eq!(v["seeds"], serde_json::json!(["shop::service::price"]));
        let first = &v["tests"][0];
        assert_eq!(first["qname"], "shop::tests::test_service::test_price");
        assert_eq!(
            (first["tier"].as_str(), first["line"].as_i64()),
            (Some("fact"), Some(4))
        );
        assert_eq!(
            v["test_files"],
            serde_json::json!(["shop/tests/test_service.py"])
        );

        // Nothing calls place: its module's name pairing is its only row,
        // and module_level=false leaves the answer empty, with its absence.
        let paired = qname_answer(
            &built.merged,
            &["place".to_string()],
            DEFAULT_MAX_DEPTH,
            None,
            true,
        )
        .expect("answer");
        assert_eq!(value(&paired)["tests"][0]["tier"], "heuristic");
        assert_eq!(paired.untested, ["shop::service::place"]);
        let cases = qname_answer(
            &built.merged,
            &["place".to_string()],
            DEFAULT_MAX_DEPTH,
            None,
            false,
        )
        .expect("answer");
        assert!(cases.tests.is_empty(), "{:?}", cases.tests);
        assert_eq!(value(&cases)["absence"]["reason"], "no_edges");
        assert_eq!(
            DEFAULT_MAX_DEPTH, 6,
            "the pyo3 signatures' literal depth default"
        );
        let o = options(2, Some("shop"), false);
        assert_eq!(
            (o.max_depth, o.scope.as_deref(), o.module_level),
            (2, Some("shop"), false)
        );

        let diff = "--- a/shop/service.py\n+++ b/shop/service.py\n@@ -1,2 +1,2 @@\n def price(order):\n\
-    return order\n+    return order + 0\n";
        let d = diff_answer(
            &built.merged,
            diff,
            DEFAULT_MAX_DEPTH,
            Some("shop/tests"),
            true,
        )
        .expect("answer");
        assert_eq!(d.seeds, ["shop::service::price"]);
        assert_eq!(d.test_files, ["shop/tests/test_service.py"]);

        assert!(qname_answer(&built.merged, &[], DEFAULT_MAX_DEPTH, None, true).is_err());
    }

    /// LE.3b: the helper behind the module function `tests_for_rev` seeds
    /// from the working tree's change, and an unknown rev is the engine's
    /// error.
    #[test]
    fn rev_helper_seeds_from_the_working_tree() {
        let (scratch, top) = shop("rev");
        let gitconfig = scratch.0.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        git(&top, &gitconfig, &["init", "-q"]);
        git(&top, &gitconfig, &["add", "-A"]);
        git(&top, &gitconfig, &["commit", "-q", "-m", "shop"]);
        std::fs::write(
            top.join("shop/service.py"),
            SERVICE_PY.replace("return order\n", "return order + 0\n"),
        )
        .expect("edit");
        let repo = top.to_str().expect("utf-8");
        let a = rev_answer(repo, "HEAD", DEFAULT_MAX_DEPTH, None, true).expect("answer");
        assert_eq!(a.seeds, ["shop::service::price"]);
        assert_eq!(a.test_files, ["shop/tests/test_service.py"]);
        let err = rev_answer(repo, "no-such-rev", DEFAULT_MAX_DEPTH, None, true)
            .expect_err("unknown rev");
        assert!(err.contains("no-such-rev"), "{err}");
    }
}
