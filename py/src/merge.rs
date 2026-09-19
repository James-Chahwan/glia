//! `merge_gmaps` (LC.10c): the Python surface of the gmap merge
//! (`repo_graph_engine::merge`, LC.10b) — combine pre-built `.gmap` layouts
//! into one `PyGraph` without their sources checked out. `glia merge --gmap`
//! is the CLI surface of the same merge.

use std::path::{Path, PathBuf};

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_engine::GenerateResult;
use repo_graph_engine::merge::{MergeMember, merge_layouts, persist_merge};

use crate::graph::PyGraph;
use crate::registry::ModuleFns;

/// Merge the pre-built `.gmap` layouts at `dirs` (in order) into the graph one
/// build of all of them gives: each layout is loaded as it is, the
/// cross-service resolvers and post-passes re-run over the union, and repo
/// labels are recomputed over it (`repo_graph_engine::merge::merge_layouts`).
/// Returns a `PyGraph` carrying the union's labels, roots and parse errors.
///
/// Each member is named by its directory's basename, a trailing `.glia/graph`
/// stripped (`api/.glia/graph` -> `api`), characters outside `[A-Za-z0-9._-]`
/// made `_` and leading dots dropped — the rule `glia merge --gmap` uses. With
/// `out`, the merge is also written there as a layout whose manifest records
/// the members (`persist_merge`, writer `py`); `load_from_gmap(out)` reads it
/// back.
///
/// Prints `[merge] members=<n> (gmap=<n> repo=0) ...` (the fired_on marker),
/// a `[merge] caveat: ...` line per thing the merge cannot reproduce, and with
/// `out` `[merge] wrote <out> writer=py ...` to stderr.
///
/// Raises ValueError, naming the member, when `dirs` is empty, a layout is
/// missing or unreadable, two members get one name, two layouts hold one repo
/// (built separately from one checkout identity), or `out` cannot be written.
#[pyfunction]
#[pyo3(signature = (dirs, out=None))]
fn merge_gmaps(dirs: Vec<String>, out: Option<String>) -> PyResult<PyGraph> {
    merge_gmap_dirs(&dirs, out.as_deref())
        .map(PyGraph::from_result)
        .map_err(|e| PyValueError::new_err(format!("merge_gmaps: {e}")))
}

/// The whole of `merge_gmaps` minus pyo3, so `cargo test -p repo-graph-py`
/// covers it (see the crate doc).
fn merge_gmap_dirs(dirs: &[String], out: Option<&str>) -> Result<GenerateResult, String> {
    let members: Vec<MergeMember> = dirs
        .iter()
        .map(|dir| MergeMember::Gmap { name: member_name(Path::new(dir)), dir: PathBuf::from(dir) })
        .collect();
    let merged = merge_layouts(&members)?;
    if let Some(dir) = out {
        persist_merge(&merged, Path::new(dir), "py")?;
    }
    Ok(merged.result)
}

/// A layout dir's member name: the basename, a trailing `.glia/graph`
/// stripped; a path with no basename as given (`.`, `..`) is named by its
/// canonical path; every character outside `[A-Za-z0-9._-]` becomes `_` and
/// leading dots are dropped, the plain name `merge_layouts` requires (an
/// empty or repeated one is refused there). The same rule as `glia merge
/// --gmap` (cli/src/cmd/merge.rs `member_name`).
fn member_name(path: &Path) -> String {
    let base = |p: &Path| {
        let p = if p.ends_with(".glia/graph") {
            p.parent().and_then(Path::parent).unwrap_or(p)
        } else {
            p
        };
        p.file_name().map(|s| s.to_string_lossy().into_owned())
    };
    let raw = base(path)
        .or_else(|| path.canonicalize().ok().and_then(|c| base(&c)))
        .unwrap_or_default();
    let plain: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    plain.trim_start_matches('.').to_string()
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(merge_gmaps, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "merge", add: register } }

#[cfg(test)]
mod tests {
    use super::*;

    use repo_graph_engine::generate_many;
    use repo_graph_engine::persist::{
        LoadOutcome, default_layout_dir, load_or_rebuild, persist_result,
    };

    /// A scratch dir removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("glia-lc10c-py-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("scratch dir");
            Scratch(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("has a parent")).expect("mkdir");
        std::fs::write(p, body).expect("write fixture");
    }

    /// A Flask route in `<s>/api` and a TypeScript client fetching it in
    /// `<s>/web`, each built alone into its own `<repo>/.glia/graph`.
    fn api_web_layouts(s: &Scratch) -> (String, String) {
        let (api, web) = (s.0.join("api"), s.0.join("web"));
        write(
            &api,
            "app.py",
            "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\")\n\
             def list_users():\n    return []\n",
        );
        write(
            &web,
            "client.ts",
            "export async function loadUsers() {\n  const res = await fetch(\"/users\");\n  \
             return res.json();\n}\n",
        );
        let mut dirs = Vec::new();
        for repo in [&api, &web] {
            let r = generate_many(&[repo.to_string_lossy().into_owned()]).expect("build");
            let dir = default_layout_dir(repo);
            persist_result(&r, &dir, "test").expect("persist");
            dirs.push(dir.to_string_lossy().into_owned());
        }
        (dirs.remove(0), dirs.remove(0))
    }

    #[test]
    fn member_names_follow_the_basename_rule() {
        let name = |p: &str| member_name(Path::new(p));
        assert_eq!(name("ci/api"), "api");
        assert_eq!(name("svc/api/.glia/graph"), "api");
        assert_eq!(name("svc/api/.glia/graph/"), "api");
        assert_eq!(name("layouts/my api"), "my_api");
        assert_eq!(name("layouts/.hidden"), "hidden");
        assert_eq!(name("/"), "");
    }

    /// LC.10c: two layouts built separately merge into one graph holding both
    /// repos, labelled by their roots, with the HTTP link only the union has;
    /// `out` writes a layout whose manifest records both members.
    #[test]
    fn merges_two_layouts_and_writes_the_merged_layout() {
        let s = Scratch::new("merge");
        let (api, web) = api_web_layouts(&s);
        let out = s.0.join("merged");
        let r = merge_gmap_dirs(&[api.clone(), web.clone()], out.to_str()).expect("merge");

        let mut labels: Vec<&str> = r.repo_labels.values().map(String::as_str).collect();
        labels.sort_unstable();
        assert_eq!(labels, ["api", "web"]);
        assert!(!r.merged.cross_edges.is_empty(), "the union resolves the HTTP link");

        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(out.join(repo_graph_store::MANIFEST_NAME)).expect("manifest written"),
        )
        .expect("manifest json");
        let names: Vec<(&str, &str)> = manifest["members"]
            .as_array()
            .expect("members recorded")
            .iter()
            .map(|m| (m["name"].as_str().unwrap_or(""), m["source"].as_str().unwrap_or("")))
            .collect();
        assert_eq!(names, [("api", "gmap"), ("web", "gmap")]);
        assert_eq!(manifest["repos"].as_array().map(Vec::len), Some(2));
        // `load_from_gmap(out)` serves the merged layout as it is, no rebuild.
        let (back, outcome) =
            load_or_rebuild(&out, None, false).expect("the merged layout loads fresh");
        assert_eq!(outcome, LoadOutcome::Fresh);
        assert_eq!(back.total_nodes, r.total_nodes);
        assert_eq!(back.merged.cross_edges.len(), r.merged.cross_edges.len());

        let alone = merge_gmap_dirs(std::slice::from_ref(&web), None).expect("merge one");
        assert_eq!(alone.repo_labels.len(), 1);
        assert!(r.total_nodes > alone.total_nodes);
    }

    #[test]
    fn merge_errors_name_the_problem() {
        let s = Scratch::new("errors");
        let (api, _web) = api_web_layouts(&s);
        let err = |dirs: &[String]| merge_gmap_dirs(dirs, None).err().unwrap_or_default();
        let e = err(&[]);
        assert!(e.contains("no members"), "{e}");
        let e = err(&[s.0.join("gone").to_string_lossy().into_owned()]);
        assert!(e.contains("member 'gone'"), "{e}");
        let e = err(&[api.clone(), api]);
        assert!(e.contains("two members are named 'api'"), "{e}");
    }
}
