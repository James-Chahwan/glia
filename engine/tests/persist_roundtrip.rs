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

use glia_engine::persist::{default_layout_dir, layout_meta, load_layout, persist_layout, persist_result};
use glia_engine::{GenerateResult, generate_many, generate_one, service_map};
use glia_store::{
    TIMELINE_FILE, TimelineRev, TimelineStore, external_inputs_fingerprint, is_gmap_stale,
    read_timeline, timeline_subject, write_timeline,
};

mod git_fixture;

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
    let as_set = |g: &glia_graph::RepoGraph| -> BTreeSet<u64> {
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
    let dir = glia_store::default_gmap_dir(&api);
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

/// `manifest.json` of the layout at `dir`, as JSON.
fn manifest_json(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap()
}

/// CD.5b: the manifest records the commit a rooted repo had checked out
/// (`repos[].rev`, read from `.git` by the persist, equal to `git rev-parse
/// HEAD`) and moves with HEAD; two builds at one HEAD write the same manifest
/// bytes; the timeline sidecar in the layout dir survives a rebuild (persist's
/// orphan sweep leaves it, the `.glia` input fingerprint skips it, the layout
/// stays fresh); a non-git root writes no `rev` key at all.
#[test]
fn manifest_records_head_rev() {
    let repo = git_fixture::GitRepo::init();
    repo.write("app/a.py", "def f():\n    return g()\n\n\ndef g():\n    return 1\n");
    repo.write("app/old.py", "def gone():\n    return 0\n");
    let first = repo.commit("first");
    let head = String::from_utf8_lossy(&repo.git(&["rev-parse", "HEAD"]).stdout).trim().to_string();
    assert_eq!(head, first);

    let dir = default_layout_dir(repo.root());
    let r = generate_one(repo.path()).unwrap();
    persist_result(&r, &dir, "test").unwrap();
    let repos = manifest_json(&dir)["repos"].clone();
    assert_eq!(repos.as_array().map(Vec::len), Some(1), "{repos}");
    assert_eq!(repos[0]["rev"], head.as_str(), "{repos}");
    assert_eq!(repos[0]["root"], "../..", "{repos}");

    // A timeline sidecar written beside the layout.
    let repo_id = *r.repo_roots.keys().next().unwrap();
    let timeline = TimelineStore {
        repo: repo_id,
        revs: vec![TimelineRev { sha: first.clone(), time: 1_760_000_000, subject: timeline_subject("first") }],
        ..Default::default()
    };
    let inputs = external_inputs_fingerprint(repo.root());
    write_timeline(&dir, &timeline).unwrap();
    let sidecar = std::fs::read(dir.join(TIMELINE_FILE)).unwrap();
    assert_eq!(external_inputs_fingerprint(repo.root()), inputs, "the sidecar is not a .glia input");
    assert!(!is_gmap_stale(&dir, repo.root()), "the sidecar does not make the layout stale");

    // A new commit (a rename, an edit, a delete) and a rebuild: the rev
    // follows HEAD, the sidecar stays.
    repo.git_mv("app/a.py", "app/b.py");
    repo.write("app/b.py", "def f():\n    return h()\n\n\ndef h():\n    return 2\n");
    repo.remove("app/old.py");
    let second = repo.commit("second");
    assert_ne!(second, first);
    persist_result(&generate_one(repo.path()).unwrap(), &dir, "test").unwrap();
    assert_eq!(manifest_json(&dir)["repos"][0]["rev"], second.as_str());
    assert_eq!(std::fs::read(dir.join(TIMELINE_FILE)).unwrap(), sidecar, "persist left the sidecar alone");
    assert_eq!(read_timeline(&dir).unwrap(), Some(timeline));
    let manifest = std::fs::read(dir.join("manifest.json")).unwrap();
    persist_result(&generate_one(repo.path()).unwrap(), &dir, "test").unwrap();
    assert_eq!(std::fs::read(dir.join("manifest.json")).unwrap(), manifest, "one HEAD, one manifest");

    // Real git's other layouts: refs packed by `git pack-refs`, a root below
    // the work tree's top, and a linked worktree on its own branch.
    let rev_of = |root: &Path| {
        let roots = std::collections::BTreeMap::from([(1u64, root.to_string_lossy().into_owned())]);
        layout_meta(&Default::default(), &roots, &[], &root.join(".glia/graph")).repos[0].rev.clone()
    };
    repo.git(&["pack-refs", "--all"]);
    assert!(!repo.root().join(".git/refs/heads/main").exists(), "main is packed");
    assert_eq!(rev_of(repo.root()).as_deref(), Some(second.as_str()), "packed-refs");
    assert_eq!(rev_of(&repo.root().join("app")).as_deref(), Some(second.as_str()), "a subdirectory");
    let wt = repo.root().parent().unwrap().join("wt");
    let wt_arg = wt.to_str().unwrap();
    repo.git(&["worktree", "add", "-q", "-b", "side", wt_arg]);
    std::fs::write(wt.join("app/side.py"), "def s():\n    return 3\n").unwrap();
    repo.git(&["-C", wt_arg, "add", "-A"]);
    repo.git(&["-C", wt_arg, "commit", "-q", "-m", "side"]);
    let side = String::from_utf8_lossy(&repo.git(&["-C", wt_arg, "rev-parse", "HEAD"]).stdout).trim().to_string();
    assert_ne!(side, second);
    assert_eq!(rev_of(&wt).as_deref(), Some(side.as_str()), "a linked worktree");
    assert_eq!(rev_of(repo.root()).as_deref(), Some(second.as_str()), "the main tree keeps its HEAD");

    // Not a git work tree: no `rev` key, the manifest bytes of before CD.5b.
    let tmp = tempfile::tempdir().unwrap();
    assert!(
        tmp.path().ancestors().all(|a| !a.join(".git").exists()),
        "precondition: the temp dir {} must not sit inside a git work tree",
        tmp.path().display()
    );
    let (api, web) = two_repo_fixture(tmp.path());
    let out = tmp.path().join("out");
    persist(&build(&api, &web), &out);
    let text = String::from_utf8(std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest_json(&out)["repos"].as_array().map(Vec::len), Some(2));
    assert!(!text.contains("\"rev\""), "{text}");
}
