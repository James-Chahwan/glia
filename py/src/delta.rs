//! **graph_delta** (LE.1c): what the working tree's change did to the graph
//! against a git rev, as a module function over the engine's
//! `delta::graph_delta_vs_rev` (LE.1b). The engine builds both sides itself
//! (the working tree incrementally, the rev materialised read-only into a
//! temp dir), so this takes a repo path, not a `PyGraph`.
//!
//! Transport only: the build, the diff, the location of every row and the
//! engine's `[delta] base=...` marker live in the engine. The helper the
//! pyfunction delegates to is pyo3-free, so `cargo test -p repo-graph-py`
//! covers it (see the crate doc); it prints this surface's marker,
//! `[delta] surface=pyo3 rows=<n>`.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_engine::delta::{GraphDeltaAnswer, graph_delta_vs_rev};

use crate::convert::to_py;
use crate::registry::ModuleFns;

/// The whole body of [`graph_delta`], minus pyo3: the engine's answer, with
/// the surface marker (`n` = node rows + edge rows). `Err` is the engine's
/// message: not a directory, git missing, not a git work tree, an unknown
/// rev, or a failed build.
fn graph_delta_answer(repo_path: &str, base: &str) -> Result<GraphDeltaAnswer, String> {
    let answer = graph_delta_vs_rev(repo_path, base)?.answer;
    eprintln!("[delta] surface=pyo3 rows={}", answer.nodes.len() + answer.edges.len());
    Ok(answer)
}

/// **graph_delta** (LE.1c): the graph delta of git rev `base` (a branch, tag,
/// sha or `"HEAD~N"`; default `"HEAD"`) against the working tree of the repo
/// at `repo_path`, in one call.
///
/// Returns a dict `{base, files, counts, nodes, edges}`: `files` is the rev
/// build's parse-cache work `{reused, reparsed, evicted}`; `counts` the row
/// counts by change plus `regions_excluded` / `moves_ignored`; `nodes` the
/// rows `{change, id, before_id, qname, before_qname, kind, file, line,
/// side}` (`change` is `added` / `removed` / `modified` / `moved`, `side`
/// `before` for a removed node, located in the rev); `edges` the rows
/// `{change, from_qname, to_qname, category, confidence, was_confidence,
/// site_file, site_line, basis, emitter}` (`added` / `removed` /
/// `reconfidenced`). Lines are 1-based; ids are exact ints.
///
/// Saves the working tree's parse-cache sidecar
/// (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored) as
/// `generate(incremental=True)` does, never a `.gmap` layout. Raises
/// `ValueError` with the engine's message on a git or build failure.
#[pyfunction]
#[pyo3(signature = (repo_path, base="HEAD"))]
fn graph_delta(py: Python<'_>, repo_path: &str, base: &str) -> PyResult<Py<PyAny>> {
    let answer = graph_delta_answer(repo_path, base).map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&answer))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(graph_delta, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "delta", add: register } }

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
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

    /// A committed two-file python repo (`place` calls `price`), made with a
    /// fixed identity under an empty global git config. Panics without a
    /// `git` binary.
    fn shop(name: &str) -> (Scratch, PathBuf) {
        let root = std::env::temp_dir().join(format!("glia-le1c-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        std::fs::create_dir_all(top.join("shop")).expect("scratch dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(["-c", "user.name=glia", "-c", "user.email=glia@example.invalid"])
                .args(["-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
                .arg("-C")
                .arg(&top)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", &gitconfig)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .output()
                .unwrap_or_else(|e| panic!("LE.1c graph_delta test needs a `git` binary: {e}"));
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        std::fs::write(
            top.join("shop/a.py"),
            "def place(o):\n    return price(o)\n\n\ndef price(o):\n    return o\n",
        )
        .expect("write a.py");
        std::fs::write(
            top.join("shop/b.py"),
            "from shop.a import place\n\n\ndef checkout(o):\n    return place(o)\n",
        )
        .expect("write b.py");
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "shop"]);
        (Scratch(root), top)
    }

    /// LE.1c: the helper behind pyo3 `graph_delta` returns the documented
    /// object — `{base, files, counts, nodes, edges}` in that order, the
    /// order `to_py` hands Python — with an added call located on its 1-based
    /// line, and an unknown rev is the engine's error. The delta itself is
    /// covered by `engine/tests/graph_delta.rs`.
    #[test]
    fn graph_delta_value_is_the_documented_object() {
        let (_scratch, top) = shop("object");
        let repo = top.to_str().expect("utf-8 temp path");
        std::fs::write(
            top.join("shop/a.py"),
            "def place(o):\n    audit(o)\n    return price(o)\n\n\ndef price(o):\n    return o\n\n\n\
             def audit(o):\n    return o\n",
        )
        .expect("edit a.py");

        let answer = graph_delta_answer(repo, "HEAD").expect("delta vs HEAD");
        let text = serde_json::to_string(&answer).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        let keys: Vec<&str> = v.as_object().expect("an object").keys().map(String::as_str).collect();
        let mut want = vec!["base", "files", "counts", "nodes", "edges"];
        want.sort_unstable();
        assert_eq!(keys, want, "{text}");
        let order: Vec<usize> = ["\"base\":", "\"files\":", "\"counts\":", "\"nodes\":", "\"edges\":"]
            .iter()
            .map(|k| text.find(k).unwrap_or(usize::MAX))
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "field order: {text}");
        assert_eq!(v["base"], "HEAD");
        let call = v["edges"]
            .as_array()
            .and_then(|es| {
                es.iter().find(|e| {
                    e["change"] == "added"
                        && e["category"] == "CALLS"
                        && e["to_qname"].as_str().is_some_and(|q| q.ends_with("::audit"))
                })
            })
            .unwrap_or_else(|| panic!("place -> audit CALLS added: {text}"));
        assert_eq!((call["site_file"].as_str(), call["site_line"].as_i64()), (Some("shop/a.py"), Some(2)));
        assert!(v["counts"]["nodes_added"].as_u64().is_some_and(|n| n >= 1), "{text}");

        let err = graph_delta_answer(repo, "no-such-rev").expect_err("unknown rev");
        assert!(err.contains("unknown rev no-such-rev"), "{err}");
    }
}
