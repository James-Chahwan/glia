//! LB.1 gate: a repo's `RepoId` — and so every `NodeId` built under it — comes
//! from what the checkout IS (its git remote, its git dir, or its directory
//! name), not from the literal path string the build was handed.
//!
//! `NodeId::from_parts` hashes `RepoId.0` into every id, and the RepoId used to
//! be `xxhash("file://<path as typed>")`: `repo`, `$PWD/repo`, a second clone
//! and a `git worktree` of one repo shared 0 of 3 NodeIds, and a moved checkout
//! threw away a valid parse cache. Graph delta (LE.1), merging pre-built
//! `.gmap`s (LC.10) and `export --since` (LG.8) all compare builds by id.
//!
//! The control (`same_basename_repos_stay_distinct_in_a_merge`) holds the other
//! side of the design rule: distinct repos in one merge must stay distinct even
//! when their identity keys collide.

use std::path::{Path, PathBuf};

use repo_graph_code_domain::walk_gating::{IdentitySource, repo_identity};
use repo_graph_engine::{
    GenerateResult, ParseCache, generate_many, generate_one, generate_one_with_cache,
};

const SVC_PY: &str = "def checkout():\n    return total()\n\ndef total():\n    return 1\n";

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

fn s(p: &Path) -> String {
    p.to_str().unwrap().to_string()
}

/// Sorted NodeIds of every node in the build.
fn node_ids(r: &GenerateResult) -> Vec<u64> {
    let mut ids: Vec<u64> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(|n| n.id.0))
        .collect();
    ids.sort_unstable();
    ids
}

/// Recursive copy — the second "clone" of a non-git checkout.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

/// One directory under four spellings and a byte-identical copy elsewhere:
/// five builds, one NodeId set.
#[test]
fn node_ids_ignore_path_spelling_and_clone_location() {
    let t = tempfile::tempdir().unwrap();
    let one = t.path().join("one").join("shop");
    write(&one, "app/svc.py", SVC_PY);
    std::fs::create_dir_all(one.join("x")).unwrap();
    let two = t.path().join("two").join("shop");
    copy_tree(&one, &two);

    let base = node_ids(&generate_one(&s(&one)).unwrap());
    assert!(base.len() >= 3, "fixture must produce the module and its two functions");
    for spelling in [
        format!("{}/.", s(&one)),
        format!("{}/x/..", s(&one)),
        s(&two),
    ] {
        let ids = node_ids(&generate_one(&spelling).unwrap());
        assert_eq!(ids, base, "NodeIds moved with the path spelling {spelling}");
    }
}

/// A linked worktree and its main checkout are one repository: same remote,
/// same common git dir, so the same ids. The layout is what `git worktree add`
/// writes, by hand, so the test needs no git binary.
#[test]
fn worktree_and_main_checkout_share_ids() {
    let t = tempfile::tempdir().unwrap();
    let main = t.path().join("main");
    let wt2 = t.path().join("wt2");
    write(
        &main,
        ".git/config",
        "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:Example/Shop.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
    );
    write(&main, ".git/worktrees/wt2/commondir", "../..\n");
    write(&main, "app/svc.py", SVC_PY);
    let wt_gitdir = main.join(".git").join("worktrees").join("wt2");
    write(&wt2, ".git", &format!("gitdir: {}\n", wt_gitdir.display()));
    write(&wt2, "app/svc.py", SVC_PY);

    let a = node_ids(&generate_one(&s(&main)).unwrap());
    let b = node_ids(&generate_one(&s(&wt2)).unwrap());
    assert!(!a.is_empty());
    assert_eq!(a, b, "a worktree minted different NodeIds from its main checkout");
    let id = repo_identity(&wt2);
    assert_eq!(id.key, "git:github.com/example/shop");
    assert_eq!(id.source, IdentitySource::GitRemote);
    assert_eq!(repo_identity(&main), id);
}

/// CONTROL: two different repos that happen to share a directory name keep
/// distinct RepoIds when merged, and both repos' nodes survive.
#[test]
fn same_basename_repos_stay_distinct_in_a_merge() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("a").join("app");
    let b = t.path().join("b").join("app");
    write(&a, "alpha.py", "def alpha_only():\n    return 1\n");
    write(&b, "beta.py", "def beta_only():\n    return 2\n");

    // Both inputs have the key `dir:app`; the build disambiguates them.
    assert_eq!(repo_identity(&a).key, "dir:app");
    assert_eq!(repo_identity(&a), repo_identity(&b));
    let r = generate_many(&[s(&a), s(&b)]).unwrap();
    let mut repos: Vec<u64> = r.merged.graphs.iter().map(|g| g.repo.0).collect();
    repos.sort_unstable();
    repos.dedup();
    assert_eq!(repos.len(), 2, "same-basename repos collapsed onto one RepoId");
    assert!(!r.merged.qnames_containing("alpha_only").is_empty(), "repo a's nodes lost");
    assert!(!r.merged.qnames_containing("beta_only").is_empty(), "repo b's nodes lost");
    assert_eq!(r.repo_labels.len(), 2, "one human label per input");
}

/// Moving a checkout keeps its identity, so the parse cache built before the
/// move is still valid after it.
#[test]
fn moved_repo_reuses_its_parse_cache() {
    let t = tempfile::tempdir().unwrap();
    let before: PathBuf = t.path().join("one").join("shop");
    write(&before, "app/svc.py", SVC_PY);
    let after: PathBuf = t.path().join("moved").join("shop");

    let mut cache = ParseCache::new();
    let cold = generate_one_with_cache(&s(&before), &mut cache).unwrap();
    assert_eq!(cache.stats.reparsed, 1);

    std::fs::create_dir_all(after.parent().unwrap()).unwrap();
    std::fs::rename(&before, &after).unwrap();
    let warm = generate_one_with_cache(&s(&after), &mut cache).unwrap();
    assert_eq!(cache.stats.reused, 1, "a moved repo discarded its parse cache");
    assert_eq!(node_ids(&warm), node_ids(&cold));
}
