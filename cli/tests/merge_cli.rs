//! LC.10c — `glia merge` over pre-built `.gmap` layouts: `--gmap DIR`
//! (repeatable), `--workspace FILE` and `--layout OUT_DIR`, merged by the
//! engine's `merge::merge_layouts` (LC.10b). Positional REPOS alone keep the
//! source merge byte-for-byte.
//!
//! Measured before LC.10c (HEAD 2711871 debug CLI): `glia merge --gmap x` was
//! rejected by clap (`error: unexpected argument '--gmap' found`, exit 2), and
//! `glia merge <api>/.glia/graph <web>/.glia/graph` walked the layout dirs as
//! source trees: `error: no graphs produced from 2 paths; first error: `.
//!
//! Every test drives the real binary over copies of
//! `tests/fixtures/http_stack_smoke` (a Go backend and an Angular frontend
//! joined by three HTTP_CALLS cross edges) in scratch dirs. `[merge] ` stderr
//! lines are relayed, so the fired_on marker is grep-able:
//! `cargo test -p glia-cli --test merge_cli -- --nocapture 2>&1 | grep -o '\[merge\] members=.*'`

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use repo_graph_engine::generate_many;
use repo_graph_engine::persist::{default_layout_dir, load_layout, persist_result};
use repo_graph_store::{MANIFEST_NAME, MANIFEST_VERSION};

/// `glia merge <backend> <frontend>` stdout, captured from the binary built at
/// HEAD 2711871 before LC.10c touched `glia merge`. The positional source
/// merge must keep printing exactly this; a merge of the two repos' separately
/// built layouts prints it too (LC.10b reproduces the joint build).
const JOINT_SUMMARY: &str = "# glia analyze

- nodes: 26
- edges (intra-repo): 26
- cross-edges: 3

## Node kinds

| Kind | Count |
|---|---|
| METHOD | 8 |
| FUNCTION | 4 |
| ENDPOINT | 3 |
| MODULE | 3 |
| ROUTE | 3 |
| CLASS | 2 |
| INTERFACE | 1 |
| PROJECT | 1 |
| STRUCT | 1 |

## Edge categories

| Category | Count |
|---|---|
| DEFINES | 16 |
| CALLS | 6 |
| HANDLED_BY | 3 |
| HTTP_CALLS | 3 |
| IMPORTS | 1 |
";

/// A scratch dir, created fresh and removed on drop (`cli` has no
/// dev-dependencies, so no `tempfile`). Canonical, so paths compare equal to
/// what the binary prints.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glia-lc10c-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir.canonicalize().expect("canonical scratch dir"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli/ has a parent")
        .join("tests/fixtures/http_stack_smoke")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let dest = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).expect("copy fixture file");
        }
    }
}

/// Every path under `root`, relative and sorted, directories included.
fn tree(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir").flatten() {
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

/// Copies of the fixture's backend and frontend at `<s>/api` and `<s>/web`.
fn api_web(s: &Scratch) -> (PathBuf, PathBuf) {
    let (api, web) = (s.0.join("api"), s.0.join("web"));
    copy_tree(&fixture().join("backend"), &api);
    copy_tree(&fixture().join("frontend"), &web);
    (api, web)
}

/// `repo` built alone and written to the layout `dir` by the one writer.
fn build_layout(repo: &Path, dir: &Path) {
    let r = generate_many(&[repo.to_string_lossy().into_owned()]).expect("build");
    persist_result(&r, dir, "test").expect("persist layout");
}

/// `glia merge <args>` (persisting allowed), `[merge] ` lines relayed.
fn merge(args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .arg("merge")
        .args(args)
        .env_remove("GLIA_NO_PERSIST")
        .output()
        .expect("glia runs");
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[merge] ") {
            eprintln!("{line}");
        }
    }
    out
}

fn ok(out: &Output) -> (String, String) {
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "glia merge exited {:?}\nstderr:\n{stderr}", out.status);
    (String::from_utf8_lossy(&out.stdout).into_owned(), stderr)
}

fn manifest(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join(MANIFEST_NAME)).expect("manifest written"))
        .expect("manifest json")
}

/// `(name, source)` of each member the manifest at `dir` records.
fn members(dir: &Path) -> Vec<(String, String)> {
    manifest(dir)["members"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|m| {
                    let s = |k: &str| m[k].as_str().unwrap_or_default().to_string();
                    (s("name"), s("source"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 scratch path")
}

#[test]
fn merge_accepts_gmap_dirs() {
    let sc = Scratch::new("gmap");
    let (api, web) = api_web(&sc);
    // One layout in its repo's default dir (named `api` once `.glia/graph` is
    // stripped), one in a plain dir named `web`.
    let a = default_layout_dir(&api);
    let b = sc.0.join("layouts").join("web");
    build_layout(&api, &a);
    build_layout(&web, &b);
    let m = sc.0.join("merged");

    let (stdout, stderr) = ok(&merge(&["--gmap", s(&a), "--gmap", s(&b), "--layout", s(&m)]));
    assert!(
        stderr.contains("[merge] members=2 (gmap=2 repo=0) repos=2 "),
        "no merge marker:\n{stderr}"
    );
    assert!(stderr.contains("[merge] wrote "), "no layout write:\n{stderr}");
    assert_eq!(stdout, JOINT_SUMMARY, "the layout merge reproduces the joint build's summary");

    let man = manifest(&m);
    assert_eq!(man["schema_version"].as_u64(), Some(u64::from(MANIFEST_VERSION)));
    assert_eq!(man["schema_version"].as_u64(), Some(2));
    assert_eq!(man["repos"].as_array().map(Vec::len), Some(2), "{man}");
    let gmap = |n: &str| (n.to_string(), "gmap".to_string());
    assert_eq!(members(&m), [gmap("api"), gmap("web")]);
    let loaded = load_layout(&m).expect("the merged layout loads");
    assert_eq!(loaded.merged.cross_edges.len(), 3);
    assert_eq!(loaded.repo_labels.len(), 2);
}

#[test]
fn workspace_and_repo_members_merge() {
    let sc = Scratch::new("workspace");
    let (api, web) = api_web(&sc);
    build_layout(&api, &default_layout_dir(&api));
    let ws_dir = sc.0.join("ws");
    std::fs::create_dir_all(&ws_dir).expect("mkdir ws");
    let ws = ws_dir.join("glia.workspace.json");
    std::fs::write(
        &ws,
        r#"{"version": 1, "members": [{"name": "backend", "gmap": "../api/.glia/graph"}]}"#,
    )
    .expect("write workspace");
    let m = sc.0.join("merged");

    // The workspace's members first, then REPOS as repo members (built into
    // their own default layout first: `web` has none yet).
    let (stdout, stderr) = ok(&merge(&["--workspace", s(&ws), s(&web), "--layout", s(&m)]));
    assert!(
        stderr.contains("[merge] members=2 (gmap=1 repo=1) repos=2 "),
        "no merge marker:\n{stderr}"
    );
    assert_eq!(stdout, JOINT_SUMMARY);
    let member = |n: &str, src: &str| (n.to_string(), src.to_string());
    assert_eq!(members(&m), [member("backend", "gmap"), member("web", "repo")]);
    assert!(default_layout_dir(&web).join(MANIFEST_NAME).is_file(), "the repo member's layout");
}

#[test]
fn positional_merge_is_unchanged() {
    let (backend, frontend) = (fixture().join("backend"), fixture().join("frontend"));
    let before = (tree(&backend), tree(&frontend));
    let (stdout, _) = ok(&merge(&[s(&backend), s(&frontend)]));
    assert_eq!(stdout, JOINT_SUMMARY);
    assert_eq!((tree(&backend), tree(&frontend)), before, "a source merge writes nothing");
}

#[test]
fn positional_merge_with_layout_writes_the_joint_build() {
    let sc = Scratch::new("positional-layout");
    let (api, web) = api_web(&sc);
    let before = (tree(&api), tree(&web));
    let m = sc.0.join("merged");
    let (stdout, _) = ok(&merge(&[s(&api), s(&web), "--layout", s(&m)]));
    assert_eq!(stdout, JOINT_SUMMARY);
    assert_eq!((tree(&api), tree(&web)), before, "a source merge writes nothing into its repos");
    assert_eq!(manifest(&m)["repos"].as_array().map(Vec::len), Some(2));
    assert!(members(&m).is_empty(), "a source merge's layout is the joint build's, no members");
}

#[test]
fn incremental_with_gmap_is_rejected() {
    let sc = Scratch::new("incremental");
    let (api, _web) = api_web(&sc);
    let a = default_layout_dir(&api);
    build_layout(&api, &a);
    let m = sc.0.join("merged");
    let ws = sc.0.join("glia.workspace.json");
    for flags in [["--gmap", s(&a)], ["--workspace", s(&ws)]] {
        let out = merge(&[flags[0], flags[1], "--incremental", "--layout", s(&m)]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{flags:?}: {stderr}");
        assert_eq!(stderr.lines().count(), 1, "{flags:?}: one line:\n{stderr}");
        assert!(stderr.starts_with("error: --incremental"), "{stderr}");
        assert!(out.stdout.is_empty());
        assert!(!m.exists(), "{flags:?}: no output dir");
    }
}

#[test]
fn merge_errors_exit_2() {
    let sc = Scratch::new("errors");
    let (api, _web) = api_web(&sc);
    let a = default_layout_dir(&api);
    build_layout(&api, &a);
    let m = sc.0.join("merged");
    let gone = sc.0.join("gone");
    for (args, needle) in [
        (vec!["--gmap", s(&gone)], "member 'gone'"),
        (vec!["--gmap", s(&a), "--gmap", s(&a)], "two members are named 'api'"),
        (vec!["--workspace", s(&sc.0.join("missing.json"))], "workspace "),
    ] {
        let mut argv = args.clone();
        argv.extend(["--layout", s(&m)]);
        let out = merge(&argv);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains(needle), "{args:?}: {needle:?} missing from:\n{stderr}");
        assert!(!m.exists(), "{args:?}: nothing written");
    }
}
