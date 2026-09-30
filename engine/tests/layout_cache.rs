//! CE.2d gate: the whole-layout cache, engine half. A clean checkout at a git
//! tree another checkout already built installs that checkout's finished
//! `.glia/graph/` layout instead of building, once the key it re-derives
//! locally (build stamp, repo identity, HEAD tree, `.glia` inputs, target,
//! walk digest) matches and the result loads fresh.
//!
//! The fixture is a git origin holding a.py + b.ts in one commit, cloned twice
//! (`<tmp>/a`, `<tmp>/b`): one remote, so one repo identity. Every test skips
//! with a printed note when no `git` binary is on PATH (the engine's key needs
//! git; LC.9's gitignore test does the same).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_engine::persist::{default_layout_dir, load_layout, persist_result};
use glia_engine::shared_cache::{
    CacheKey, LayoutExport, LayoutInstall, LayoutKey, export_layout, install_layout, layout_key,
};
use glia_engine::{GenerateResult, generate_one};
use glia_store::is_gmap_stale;

const FILES: &[(&str, &str)] = &[
    ("a.py", "def a():\n    return 1\n"),
    ("b.ts", "export function b(): number {\n  return 2;\n}\n"),
];

type Entries = Vec<(String, Vec<u8>)>;

struct Fixture {
    tmp: tempfile::TempDir,
    a: PathBuf,
    b: PathBuf,
}

/// Run one hermetic git command in `dir` (no system or global config, a
/// fixed identity); panics on failure.
fn git(home: &Path, dir: &Path, args: &[&str]) {
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
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", home)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The origin and its two clones, or `None` (with a note) without git.
fn fixture() -> Option<Fixture> {
    if !Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("note: no git binary on PATH; skipping the layout cache test");
        return None;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let origin = tmp.path().join("origin");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&origin).expect("origin");
    git(&home, &origin, &["init", "-q"]);
    for (name, text) in FILES {
        std::fs::write(origin.join(name), text).expect("write fixture file");
    }
    git(&home, &origin, &["add", "-A"]);
    git(&home, &origin, &["commit", "-q", "-m", "fixture"]);
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    for side in [&a, &b] {
        git(&home, tmp.path(), &["clone", "-q", s(&origin), s(side)]);
    }
    Some(Fixture { tmp, a, b })
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 temp path")
}

/// `glia build <repo>`'s write: a cold build persisted to the default layout.
fn build_and_persist(repo: &Path) -> GenerateResult {
    let r = generate_one(s(repo)).expect("build");
    persist_result(&r, &default_layout_dir(repo), "test").expect("persist");
    r
}

/// The key and files `repo`'s fresh layout exports.
fn ready(repo: &Path) -> (CacheKey, String, Entries) {
    match export_layout(s(repo)).expect("export") {
        LayoutExport::Ready { key, tree, entries } => (key, tree, entries),
        other => panic!("export of {} not ready: {other:?}", repo.display()),
    }
}

/// Every regular file directly in `dir`, name -> bytes.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).expect("read"),
            )
        })
        .collect()
}

/// The names under `<repo>/.glia`: the layout dir and nothing staged beside it.
fn glia_dir_names(repo: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(repo.join(".glia"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Every node and edge (cross edges included) as sorted JSON lines.
fn graph_lines(r: &GenerateResult) -> (Vec<String>, Vec<String>) {
    let mut nodes: Vec<String> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .map(|n| serde_json::to_string(n).expect("node json"))
        .collect();
    let mut edges: Vec<String> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| &g.edges)
        .chain(&r.merged.cross_edges)
        .map(|e| serde_json::to_string(e).expect("edge json"))
        .collect();
    nodes.sort();
    edges.sort();
    (nodes, edges)
}

fn manifest_repos(repo: &Path) -> serde_json::Value {
    let bytes = std::fs::read(default_layout_dir(repo).join("manifest.json")).expect("manifest");
    serde_json::from_slice::<serde_json::Value>(&bytes).expect("manifest json")["repos"].clone()
}

#[test]
fn clean_clone_installs_the_layout() {
    let Some(f) = fixture() else { return };
    build_and_persist(&f.a);
    let (key, tree, entries) = ready(&f.a);
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.contains(&"manifest.json") && names.iter().any(|n| n.ends_with(".gmap")),
        "{names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| *n == "parse_cache.bin" || *n == ".gitignore"),
        "{names:?}"
    );
    assert_eq!(
        layout_key(s(&f.b)).expect("key b"),
        LayoutKey::Clean {
            key,
            tree: tree.clone()
        }
    );

    let installed = install_layout(s(&f.b), &key, entries.clone()).expect("install");
    assert!(
        matches!(&installed, LayoutInstall::Installed { tree: t, entries: n, .. } if *t == tree && *n == entries.len()),
        "{installed:?}"
    );
    let b_dir = default_layout_dir(&f.b);
    let on_disk = snapshot(&b_dir);
    for (name, bytes) in entries.iter().filter(|(n, _)| n != "manifest.json") {
        assert_eq!(on_disk.get(name), Some(bytes), "{name} differs from a's");
    }
    assert!(
        on_disk.contains_key(".gitignore"),
        "no self-ignore: {:?}",
        on_disk.keys()
    );
    assert_eq!(
        glia_dir_names(&f.b),
        ["graph"],
        "a staging dir was left behind"
    );
    // The manifest names b, rooted at b, at b's HEAD commit.
    let (repos_a, repos_b) = (manifest_repos(&f.a), manifest_repos(&f.b));
    assert_eq!(repos_a[0]["label"], "a");
    assert_eq!(repos_b[0]["label"], "b");
    assert_eq!(repos_b[0]["root"], "../..");
    assert_eq!(repos_b[0]["id"], repos_a[0]["id"]);
    assert_eq!(repos_b[0]["rev"], repos_a[0]["rev"]);

    assert!(
        !is_gmap_stale(&b_dir, &f.b),
        "the installed layout is stale"
    );
    let loaded = load_layout(&b_dir).expect("load b's layout");
    let cold = generate_one(s(&f.b)).expect("cold build of b");
    assert_eq!(
        graph_lines(&loaded),
        graph_lines(&cold),
        "installed layout != cold build of b"
    );
    assert!(!graph_lines(&cold).0.is_empty());
    assert_eq!(loaded.repo_labels, cold.repo_labels);
    assert!(git_status_clean(&f.b), "the install dirtied b's git status");
}

fn git_status_clean(repo: &Path) -> bool {
    matches!(layout_key(s(repo)), Ok(LayoutKey::Clean { .. }))
}

#[test]
fn dirty_worktree_skips() {
    let Some(f) = fixture() else { return };
    build_and_persist(&f.a);
    let (key, _, entries) = ready(&f.a);

    std::fs::write(f.b.join("notes.py"), "x = 1\n").expect("untracked file");
    assert!(matches!(
        layout_key(s(&f.b)).expect("key"),
        LayoutKey::Dirty(_)
    ));
    let got = install_layout(s(&f.b), &key, entries.clone()).expect("install");
    assert!(matches!(got, LayoutInstall::Dirty(_)), "{got:?}");
    assert!(!f.b.join(".glia").exists(), "a dirty install wrote into b");

    // A modified tracked file is dirty too, and so is a's export of it.
    std::fs::remove_file(f.b.join("notes.py")).expect("rm");
    std::fs::write(f.b.join("a.py"), "def a():\n    return 3\n").expect("edit");
    assert!(matches!(
        install_layout(s(&f.b), &key, entries).expect("install"),
        LayoutInstall::Dirty(_)
    ));
    std::fs::write(f.a.join("b.ts"), "export const b = 1;\n").expect("edit a");
    assert!(matches!(
        export_layout(s(&f.a)).expect("export"),
        LayoutExport::Dirty(_)
    ));
}

#[test]
fn export_needs_a_fresh_single_repo_layout() {
    let Some(f) = fixture() else { return };
    // No layout yet: clean, keyed, stale.
    let got = export_layout(s(&f.a)).expect("export");
    assert!(
        matches!(&got, LayoutExport::Stale { reason, .. } if reason.contains("no layout")),
        "{got:?}"
    );
    build_and_persist(&f.a);
    ready(&f.a);
    // Same bytes, newer mtime: git stays clean, the layout goes stale.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(f.a.join("a.py"))
        .and_then(|file| file.set_modified(later))
        .expect("touch a.py");
    let got = export_layout(s(&f.a)).expect("export");
    assert!(
        matches!(&got, LayoutExport::Stale { reason, .. } if reason.contains("stale")),
        "{got:?}"
    );
}

#[test]
fn glia_inputs_are_in_the_key() {
    let Some(f) = fixture() else { return };
    build_and_persist(&f.a);
    let (key, _, entries) = ready(&f.a);
    std::fs::create_dir_all(f.b.join(".glia")).expect(".glia");
    std::fs::write(f.b.join(".glia/overlay.toml"), "# no stanzas\n").expect("overlay");
    let LayoutKey::Clean { key: b_key, .. } = layout_key(s(&f.b)).expect("key b") else {
        panic!("an untracked .glia input made b dirty");
    };
    assert_ne!(b_key, key, "the .glia inputs are not in the key");
    let got = install_layout(s(&f.b), &key, entries).expect("install");
    assert!(
        matches!(&got, LayoutInstall::Rejected(r) if r.starts_with("key mismatch")),
        "{got:?}"
    );
    assert!(
        !default_layout_dir(&f.b).exists(),
        "a rejected install wrote b's layout"
    );
}

#[test]
fn walk_only_state_is_in_the_key() {
    let Some(f) = fixture() else { return };
    build_and_persist(&f.a);
    let (key, _, _) = ready(&f.a);
    // An empty `dist/` is invisible to git and a REGION to the walk.
    std::fs::create_dir(f.b.join("dist")).expect("dist");
    let LayoutKey::Clean { key: b_key, .. } = layout_key(s(&f.b)).expect("key b") else {
        panic!("an empty dir made b dirty");
    };
    assert_ne!(b_key, key, "a REGION the walk sees is not in the key");
    std::fs::remove_dir(f.b.join("dist")).expect("rm dist");
    assert_eq!(
        layout_key(s(&f.b)).expect("key b"),
        layout_key(s(&f.a)).expect("key a")
    );
    // A source file git's own excludes hide: status is clean, the walk reads it.
    std::fs::write(f.b.join("hidden.py"), "def hidden():\n    return 0\n").expect("hidden.py");
    let exclude = f.b.join(".git/info/exclude");
    std::fs::create_dir_all(exclude.parent().expect("info dir")).expect("info dir");
    std::fs::write(&exclude, "hidden.py\n").expect("exclude");
    let got = layout_key(s(&f.b)).expect("key b");
    assert!(
        matches!(&got, LayoutKey::Dirty(r) if r.contains("hidden.py")),
        "{got:?}"
    );
}

#[test]
fn hostile_entry_names_are_refused() {
    let Some(f) = fixture() else { return };
    build_and_persist(&f.a);
    let (key, _, entries) = ready(&f.a);
    let first = install_layout(s(&f.b), &key, entries.clone()).expect("install");
    assert!(
        matches!(first, LayoutInstall::Installed { .. }),
        "{first:?}"
    );
    let b_dir = default_layout_dir(&f.b);
    let before = snapshot(&b_dir);

    for bad in [
        "../evil",
        "../../evil.gmap",
        "sub/x.gmap",
        "parse_cache.bin",
        "timeline.gmap",
    ] {
        let mut hostile = entries.clone();
        hostile.push((bad.to_string(), b"payload".to_vec()));
        let got = install_layout(s(&f.b), &key, hostile).expect("install");
        assert!(
            matches!(&got, LayoutInstall::Rejected(r) if r.contains(bad)),
            "{bad}: {got:?}"
        );
    }
    // An entry the manifest does not name.
    let mut stray = entries.clone();
    stray.push(("repo-1-77.gmap".to_string(), b"payload".to_vec()));
    let got = install_layout(s(&f.b), &key, stray).expect("install");
    assert!(
        matches!(&got, LayoutInstall::Rejected(r) if r.contains("not named")),
        "{got:?}"
    );

    for outside in [
        f.b.join(".glia/evil"),
        f.b.join("evil"),
        f.b.join("evil.gmap"),
        f.tmp.path().join("evil"),
    ] {
        assert!(!outside.exists(), "{} was written", outside.display());
    }
    assert_eq!(snapshot(&b_dir), before, "the previous layout changed");
    assert_eq!(glia_dir_names(&f.b), ["graph"]);
}

#[test]
fn rejected_layout_restores_the_old_one() {
    let Some(f) = fixture() else { return };
    build_and_persist(&f.a);
    let (key, _, entries) = ready(&f.a);
    // b's layout before the pull: another repo's graph, other shard names.
    let other = f.tmp.path().join("other");
    std::fs::create_dir_all(&other).expect("other");
    std::fs::write(other.join("c.py"), "def c():\n    return 3\n").expect("c.py");
    let r = generate_one(s(&other)).expect("build other");
    let b_dir = default_layout_dir(&f.b);
    persist_result(&r, &b_dir, "test").expect("persist other into b");
    let before = snapshot(&b_dir);
    assert!(before.len() >= 3, "{:?}", before.keys());

    // The offered manifest names a shard the offer does not carry.
    let mut broken: Entries = entries.clone();
    let manifest = broken
        .iter_mut()
        .find(|(n, _)| n == "manifest.json")
        .expect("manifest entry");
    let mut m: serde_json::Value = serde_json::from_slice(&manifest.1).expect("manifest json");
    m["shards"]
        .as_array_mut()
        .expect("shards")
        .push(serde_json::json!({
            "name": "ghost", "path": "repo-1-99.gmap", "content_hash": "0000000000000000"
        }));
    manifest.1 = serde_json::to_vec_pretty(&m).expect("manifest bytes");
    let got = install_layout(s(&f.b), &key, broken).expect("install");
    assert!(
        matches!(&got, LayoutInstall::Rejected(r) if r.contains("does not load")),
        "{got:?}"
    );
    assert_eq!(
        snapshot(&b_dir),
        before,
        "the pre-install layout was not restored byte for byte"
    );
    assert_eq!(
        glia_dir_names(&f.b),
        ["graph"],
        "a staging dir was left behind"
    );

    // The intact offer then replaces it, and the other repo's shards go.
    let got = install_layout(s(&f.b), &key, entries.clone()).expect("install");
    assert!(matches!(got, LayoutInstall::Installed { .. }), "{got:?}");
    let after = snapshot(&b_dir);
    let mut want: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    want.push(".gitignore");
    want.sort();
    assert_eq!(after.keys().map(String::as_str).collect::<Vec<_>>(), want);
}
