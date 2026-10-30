//! FD.78 — the walk and directory symlinks, end to end through a build.
//!
//! An in-repo directory symlink (flutter/samples' `web/<x> -> ../<x>`) is not
//! descended: its target is walked once, where it lies, so no node is minted
//! under the link's path. A directory symlink whose target lies outside the
//! repo is walked once per canonical target (the first link in name order),
//! and a link back into a directory already entered (a cycle) is cut.
//!
//! Before FD.78 (probe at c4152966): `web/app -> ../app` minted every node
//! twice, under `app::..` and `web::app::..`, and two links to one outside
//! tree plus a loop inside it read one file 81 times. Run with
//! `-- --nocapture` and grep `^\[walk\] symlinks root=` for the fired_on
//! marker.
#![cfg(unix)]

use std::os::unix::fs::symlink;
use std::path::Path;

use glia_engine::{GenerateResult, generate_one};

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// Every qname of the build, one entry per (graph, node), sorted, so a node
/// minted twice under one qname counts twice.
fn qnames(r: &GenerateResult) -> Vec<String> {
    let mut out: Vec<String> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.values().cloned())
        .collect();
    out.sort();
    out
}

fn build(root: &Path) -> Vec<String> {
    qnames(&generate_one(root.to_str().unwrap()).unwrap())
}

#[test]
fn an_in_repo_directory_link_is_walked_at_its_target_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    write(
        &root.join("app/a.dart"),
        "String helper() => 'x';\nvoid main() { helper(); }\n",
    );
    std::fs::create_dir_all(root.join("web")).unwrap();
    symlink("../app", root.join("web/app")).unwrap();

    let q = build(&root);
    assert_eq!(
        q.iter().filter(|n| *n == "app::a::helper").count(),
        1,
        "{q:#?}"
    );
    assert!(
        !q.iter().any(|n| n.starts_with("web::app::")),
        "a node under the link's path: {q:#?}"
    );
}

#[test]
fn an_out_of_repo_target_is_walked_once_and_cycles_are_cut() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    let out = tmp.path().join("out");
    write(&root.join("main.dart"), "void main() {}\n");
    write(&out.join("lib/b.dart"), "String g() => 'y';\n");
    symlink(&out, root.join("ext")).unwrap();
    symlink(&out, root.join("ext2")).unwrap();
    symlink(out.join("lib"), out.join("lib/again")).unwrap();

    let q = build(&root);
    assert_eq!(
        q.iter().filter(|n| *n == "ext::lib::b::g").count(),
        1,
        "{q:#?}"
    );
    assert!(
        !q.iter().any(|n| n.starts_with("ext2::")),
        "a second link to one outside tree: {q:#?}"
    );
    assert!(
        !q.iter().any(|n| n.contains("::again::")),
        "a symlink cycle was descended: {q:#?}"
    );
}
