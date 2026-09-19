//! LC.7: a graph loaded from a `.gmap` layout answers like the one that was
//! saved. Repo labels and roots and the build's parse errors ride
//! `manifest.json`; `RepoGraph.properties` rides each shard's code section.
//!
//! The measured failure mode before LC.7: `service_map` on a
//! `save_to` + `load_from_gmap` graph named its services `repo<id>` (the
//! RepoId hash) instead of `api` / `web`, a loaded graph reported no parse
//! errors by construction, and its `properties` sets were empty.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use repo_graph_engine::persist::{layout_meta, load_layout, persist_layout};
use repo_graph_engine::{GenerateResult, generate_many, service_map};

/// Two repos under one scratch dir: a Flask route plus a class with an
/// `@property` accessor, and a TypeScript client fetching that route.
fn two_repo_fixture(tmp: &Path) -> (PathBuf, PathBuf) {
    let api = tmp.join("api");
    let web = tmp.join("web");
    std::fs::create_dir_all(&api).unwrap();
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(
        api.join("app.py"),
        "from flask import Flask\n\napp = Flask(__name__)\n\n\nclass User:\n    \
         def __init__(self, name):\n        self._name = name\n\n    @property\n    \
         def name(self):\n        return self._name\n\n\n@app.route(\"/users\")\n\
         def list_users():\n    return [User(\"a\").name]\n",
    )
    .unwrap();
    std::fs::write(
        web.join("client.ts"),
        "export async function loadUsers() {\n  const res = await fetch(\"/users\");\n  \
         return res.json();\n}\n",
    )
    .unwrap();
    (api, web)
}

fn build(api: &Path, web: &Path) -> GenerateResult {
    generate_many(&[
        api.to_string_lossy().into_owned(),
        web.to_string_lossy().into_owned(),
    ])
    .unwrap()
}

fn persist(r: &GenerateResult, dir: &Path) {
    let meta = layout_meta(&r.repo_labels, &r.repo_roots, &r.parse_errors, dir);
    persist_layout(&r.merged, &meta, dir, "test").unwrap();
}

fn service_ids(r: &GenerateResult) -> Vec<String> {
    service_map(&r.merged, &r.repo_labels).services.into_iter().map(|s| s.id).collect()
}

#[test]
fn labels_survive_the_gmap() {
    let tmp = tempfile::tempdir().unwrap();
    let (api, web) = two_repo_fixture(tmp.path());
    let r = build(&api, &web);
    assert_eq!(service_ids(&r), vec!["api", "web"], "fresh build");

    let dir = tmp.path().join("out");
    persist(&r, &dir);
    let l = load_layout(&dir).unwrap();
    assert_eq!(l.repo_labels, r.repo_labels);
    assert_eq!(service_ids(&l), vec!["api", "web"], "loaded graph must not say repo<id>");
    assert_eq!((l.total_nodes, l.total_edges), (r.total_nodes, r.total_edges));
}

#[test]
fn parse_errors_survive_the_gmap() {
    let tmp = tempfile::tempdir().unwrap();
    let (api, web) = two_repo_fixture(tmp.path());
    let mut r = build(&api, &web);
    r.parse_errors.push("x.py: synthetic".to_string());

    let dir = tmp.path().join("out");
    persist(&r, &dir);
    let l = load_layout(&dir).unwrap();
    assert_eq!(l.parse_errors, r.parse_errors);
    assert!(l.parse_errors.iter().any(|e| e == "x.py: synthetic"));
}

#[test]
fn properties_survive_the_gmap() {
    let tmp = tempfile::tempdir().unwrap();
    let (api, web) = two_repo_fixture(tmp.path());
    let r = build(&api, &web);

    let dir = tmp.path().join("out");
    persist(&r, &dir);
    let l = load_layout(&dir).unwrap();
    assert_eq!(l.merged.graphs.len(), r.merged.graphs.len());
    let as_set = |g: &repo_graph_graph::RepoGraph| -> BTreeSet<u64> {
        g.properties.iter().map(|id| id.0).collect()
    };
    for (i, (fresh, loaded)) in r.merged.graphs.iter().zip(&l.merged.graphs).enumerate() {
        assert_eq!(as_set(loaded), as_set(fresh), "graph {i}");
    }
    assert!(
        r.merged.graphs.iter().any(|g| !g.properties.is_empty()),
        "the python graph carries the @property accessor"
    );
}

#[test]
fn roots_are_relative_and_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let (api, web) = two_repo_fixture(tmp.path());
    let r = build(&api, &web);

    let dir = tmp.path().join("out");
    persist(&r, &dir);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let roots: BTreeSet<String> = manifest["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["root"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(roots, BTreeSet::from(["../api".to_string(), "../web".to_string()]));

    let l = load_layout(&dir).unwrap();
    let loaded: BTreeSet<PathBuf> = l
        .repo_roots
        .values()
        .map(|p| std::fs::canonicalize(p).unwrap())
        .collect();
    let expected: BTreeSet<PathBuf> =
        [api, web].iter().map(|p| std::fs::canonicalize(p).unwrap()).collect();
    assert_eq!(loaded, expected);
}

/// The conventional in-repo layout dir sits two levels under the repo, and a
/// layout dir that does not exist yet still yields a relative root.
#[test]
fn in_repo_layout_root_is_dot_dot() {
    let tmp = tempfile::tempdir().unwrap();
    let (api, web) = two_repo_fixture(tmp.path());
    let r = build(&api, &web);
    let dir = api.join(".ai/repo-graph");
    assert!(!dir.exists());
    let meta = layout_meta(&r.repo_labels, &r.repo_roots, &r.parse_errors, &dir);
    let roots: BTreeSet<Option<String>> = meta.repos.iter().map(|m| m.root.clone()).collect();
    assert_eq!(
        roots,
        BTreeSet::from([Some("../..".to_string()), Some("../../../web".to_string())])
    );
}

/// A missing layout is reported as "rebuild", not as a caller error.
#[test]
fn missing_layout_needs_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let err = load_layout(&tmp.path().join("nowhere")).err().unwrap();
    assert!(err.needs_rebuild, "{err}");
}
