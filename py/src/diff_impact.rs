//! **diff_impact** (LE.2): what a change affects, in one call — the changed
//! nodes of a change and ONE multi-seed, located, ranked blast radius around
//! them, over the engine's `diff_impact::{diff_impact_from_diff,
//! diff_impact_vs_rev}`.
//!
//! `PyGraph.diff_impact(diff_text, ..)` answers a pasted unified diff (or a
//! changed-file list) over a built graph; the module function
//! `diff_impact_vs_rev(repo_path, base="HEAD", ..)` builds the repo's working
//! tree and the rev itself (LE.1b's graph delta), so it takes a path. Each
//! returns the engine's answer as a native dict (LD.2); a bad `direction` or
//! an engine `Err` raises `ValueError`.
//!
//! Transport only: the seeds, the radius, the location of every row and the
//! `[diff-impact] mode=..` marker live in the engine. The helpers the pyo3
//! entry points delegate to are pyo3-free, so `cargo test -p repo-graph-py`
//! covers them (see the crate doc).

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_engine::BlastOptions;
use repo_graph_engine::diff_impact::{DiffImpact, diff_impact_from_diff, diff_impact_vs_rev};
use repo_graph_graph::MergedGraph;

use crate::blast::reach_named;
use crate::convert::to_py;
use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// The engine's options from the keyword arguments both entry points take;
/// `Err` names a bad `direction`.
fn options(
    direction: &str,
    depth: usize,
    top_k: Option<usize>,
    live_only: bool,
    scope: Option<&str>,
) -> Result<BlastOptions, String> {
    let mut opts = BlastOptions::default();
    opts.direction = reach_named(direction)?;
    opts.depth = depth;
    opts.top_k = top_k;
    opts.live_only = live_only;
    opts.scope = scope.map(str::to_string);
    Ok(opts)
}

/// The body of [`PyGraph::diff_impact`], minus pyo3.
fn diff_answer(
    merged: &MergedGraph,
    diff_text: &str,
    direction: &str,
    depth: usize,
    top_k: Option<usize>,
    live_only: bool,
    scope: Option<&str>,
) -> Result<DiffImpact, String> {
    let opts = options(direction, depth, top_k, live_only, scope)?;
    Ok(diff_impact_from_diff(merged, diff_text, &opts))
}

/// The body of [`diff_impact_vs_rev_py`], minus pyo3.
fn rev_answer(
    repo_path: &str,
    base: &str,
    direction: &str,
    depth: usize,
    top_k: Option<usize>,
    live_only: bool,
    scope: Option<&str>,
) -> Result<DiffImpact, String> {
    let opts = options(direction, depth, top_k, live_only, scope)?;
    diff_impact_vs_rev(repo_path, base, &opts)
}

#[pymethods]
impl PyGraph {
    /// **diff_impact** (LE.2): what the change a unified diff describes
    /// affects, over this graph (the diff's new side). The narrowest node
    /// spanning each added line (a changed-file list: every node of each
    /// file) seeds ONE blast radius, `blast_radius`'s walk and ranking over
    /// all the seeds at once. `direction` ∈ {`forward`, `backward`, `both`},
    /// `depth` hops, `top_k` rows, `live_only` and `scope` as in
    /// `blast_radius`.
    ///
    /// Returns a dict `{base, changed, edges_added, edges_removed, impact,
    /// unresolved_diff_files}`: `base` is `None` here; `changed` rows `{id,
    /// qname, kind, file, line, change, seed}` with `change` `diff_hit` and
    /// `seed` false for a module beside a finer changed node of its file;
    /// `impact` is `blast_radius`'s dict, each result carrying the `seed`
    /// whose wave reached it first; `unresolved_diff_files` names the diff's
    /// files that place no node (a hunk that only deletes lines, a deleted
    /// file, an unparsed file). The edge lists are empty in this mode. Lines
    /// are 1-based. Raises ValueError on a bad `direction`.
    #[pyo3(signature = (diff_text, direction="both", depth=4, top_k=None, live_only=false, scope=None))]
    fn diff_impact(
        &self,
        py: Python<'_>,
        diff_text: &str,
        direction: &str,
        depth: usize,
        top_k: Option<usize>,
        live_only: bool,
        scope: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        let answer = diff_answer(&self.merged, diff_text, direction, depth, top_k, live_only, scope)
            .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }
}

/// **diff_impact_vs_rev** (LE.2): `PyGraph.diff_impact` for the working
/// tree's change against git rev `base` (a branch, tag, sha or `"HEAD~N"`;
/// default `"HEAD"`) of the repo at `repo_path`. Seeds: the added, moved and
/// edited nodes and the surviving ends of every call-like edge gained or
/// lost (a caller whose call was deleted, a hunk that only deletes lines).
/// `changed` carries LE.1b's `added` / `removed` / `modified` / `moved` rows
/// (a removed row located in the base rev, never a seed) and the
/// `edge_endpoint` seeds; `edges_added` / `edges_removed` the delta's edge
/// rows. Builds both sides itself and saves the parse-cache sidecar
/// (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored) as
/// `generate(incremental=True)` does, never a `.gmap` layout. Same keywords
/// and dict as `PyGraph.diff_impact`; raises ValueError on a bad `direction`
/// or a git or build failure.
#[pyfunction]
#[pyo3(name = "diff_impact_vs_rev", signature = (repo_path, base="HEAD", direction="both", depth=4, top_k=None, live_only=false, scope=None))]
fn diff_impact_vs_rev_py(
    py: Python<'_>,
    repo_path: &str,
    base: &str,
    direction: &str,
    depth: usize,
    top_k: Option<usize>,
    live_only: bool,
    scope: Option<&str>,
) -> PyResult<Py<PyAny>> {
    let answer = rev_answer(repo_path, base, direction, depth, top_k, live_only, scope)
        .map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&answer))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(diff_impact_vs_rev_py, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "diff_impact", add: register } }

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;

    /// A scratch dir under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const A_PY: &str = "def price(o):\n    return o\n\n\ndef place(o):\n    return price(o)\n";
    const B_PY: &str = "from shop.a import place\n\n\ndef checkout(o):\n    return place(o)\n";
    const DIFF: &str = "--- a/shop/a.py\n+++ b/shop/a.py\n@@ -1,2 +1,2 @@\n def price(o):\n-    return o\n+    return o or 0\n";

    /// `shop/a.py` (`place` calls `price`) and `shop/b.py` (`checkout` calls
    /// `place`), under a fresh scratch dir.
    fn shop(name: &str) -> (Scratch, PathBuf) {
        let root = std::env::temp_dir().join(format!("glia-le2-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        for (rel, text) in [("shop/a.py", A_PY), ("shop/b.py", B_PY)] {
            let p = top.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("scratch dir");
            std::fs::write(&p, text).expect("write fixture");
        }
        (Scratch(root), top)
    }

    /// The object every entry point returns, as `to_py` hands it to Python.
    fn value(a: &DiffImpact) -> serde_json::Value {
        serde_json::from_str(&serde_json::to_string(a).expect("serialises")).expect("valid JSON")
    }

    /// `(qname, depth, seed)` per impact row.
    fn rows(v: &serde_json::Value) -> Vec<(String, u64, String)> {
        let mut out: Vec<(String, u64, String)> = v["impact"]["results"]
            .as_array()
            .expect("results")
            .iter()
            .map(|r| {
                (
                    r["qname"].as_str().unwrap_or("").to_string(),
                    r["depth"].as_u64().unwrap_or(99),
                    r["seed"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        out.sort();
        out
    }

    fn git(top: &Path, gitconfig: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=glia", "-c", "user.email=glia@example.invalid"])
            .args(["-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .arg("-C")
            .arg(top)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap_or_else(|e| panic!("LE.2 diff_impact_vs_rev test needs a `git` binary: {e}"));
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// LE.2: the helper behind pyo3 `diff_impact` returns the documented
    /// object in field order, the keywords reach the engine, and a bad
    /// direction is an error. The seeding itself is covered by
    /// `engine/tests/diff_impact.rs`.
    #[test]
    fn diff_helper_returns_the_documented_object() {
        let (_scratch, top) = shop("diff");
        let built = repo_graph_engine::generate_one(top.to_str().expect("utf-8")).expect("build");
        let a = diff_answer(&built.merged, DIFF, "backward", 4, None, false, None).expect("answer");
        let text = serde_json::to_string(&a).expect("serialises");
        let order: Vec<usize> = [
            "\"base\":",
            "\"changed\":",
            "\"edges_added\":",
            "\"edges_removed\":",
            "\"impact\":",
            "\"unresolved_diff_files\":",
        ]
        .iter()
        .map(|k| text.find(k).unwrap_or(usize::MAX))
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "field order: {text}");
        let v = value(&a);
        assert!(v["base"].is_null());
        assert_eq!(v["changed"][0]["qname"], "shop::a::price");
        assert_eq!((v["changed"][0]["change"].as_str(), v["changed"][0]["seed"].as_bool()), (Some("diff_hit"), Some(true)));
        assert_eq!(
            rows(&v),
            [
                ("shop::a::place".to_string(), 1, "shop::a::price".to_string()),
                ("shop::b::checkout".to_string(), 2, "shop::a::price".to_string()),
            ]
        );
        let cut = diff_answer(&built.merged, DIFF, "backward", 1, None, false, None).expect("depth 1");
        assert_eq!(rows(&value(&cut)), [("shop::a::place".to_string(), 1, "shop::a::price".to_string())]);
        let fwd = diff_answer(&built.merged, DIFF, "forward", 4, None, false, None).expect("forward");
        assert!(fwd.impact.results.is_empty() && fwd.impact.absence.is_some());
        let err = diff_answer(&built.merged, DIFF, "sideways", 4, None, false, None).expect_err("bad direction");
        assert!(err.contains("sideways"), "{err}");
        let o = options("both", 2, Some(3), true, Some("shop")).expect("options");
        assert_eq!((o.depth, o.top_k, o.live_only, o.scope.as_deref()), (2, Some(3), true, Some("shop")));
    }

    /// LE.2: the helper behind the module function `diff_impact_vs_rev`
    /// seeds from the working tree's change, and an unknown rev is the
    /// engine's error.
    #[test]
    fn rev_helper_seeds_from_the_working_tree() {
        let (scratch, top) = shop("rev");
        let gitconfig = scratch.0.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        git(&top, &gitconfig, &["init", "-q"]);
        git(&top, &gitconfig, &["add", "-A"]);
        git(&top, &gitconfig, &["commit", "-q", "-m", "shop"]);
        std::fs::write(top.join("shop/a.py"), A_PY.replace("return o\n", "return o or 0\n")).expect("edit");
        let repo = top.to_str().expect("utf-8");
        let a = rev_answer(repo, "HEAD", "backward", 4, None, false, None).expect("answer");
        let v = value(&a);
        assert_eq!(v["base"], "HEAD");
        assert_eq!(
            rows(&v),
            [
                ("shop::a::place".to_string(), 1, "shop::a::price".to_string()),
                ("shop::b::checkout".to_string(), 2, "shop::a::price".to_string()),
            ]
        );
        let err = rev_answer(repo, "no-such-rev", "both", 4, None, false, None).expect_err("unknown rev");
        assert!(err.contains("no-such-rev"), "{err}");
    }
}
