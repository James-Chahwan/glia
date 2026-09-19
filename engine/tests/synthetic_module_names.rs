//! LB.9a acceptance: a non-code file's MODULE is named by its full file name
//! (`api::user.proto`), so it never shares a NodeId with a same-stem code
//! file (`api/user.go` -> `api::user`) or a same-stem file of another format
//! (`docs/swagger.yaml` + `docs/swagger.json`, `svc/Dockerfile` +
//! `svc/Dockerfile.prod`).
//!
//! The route unit test `synthetic_modules_are_named_by_file_name` proves each
//! branch's qname. This proves the merged graph on a tempdir copy of the
//! `module-synthetic-filename` fixture: before LB.9a the `.proto` and the
//! `.go` were two MODULE records under one id (the proto and go groups are
//! separate RepoGraphs), and the two Dockerfiles folded into one MODULE with
//! every edge doubled.

use std::collections::HashMap;
use std::path::Path;

use glia_code_domain::{edge_category, node_kind};
use glia_core::NodeId;
use glia_engine::{generate_one, locate_node};
use glia_graph::MergedGraph;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/module-synthetic-filename";

/// Copy the fixture's sources (not its key.json) into `dst`.
fn copy_fixture(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            std::fs::create_dir_all(&to).expect("mkdir");
            copy_fixture(&from, &to);
        } else if entry.file_name() != "key.json" {
            std::fs::copy(&from, &to).expect("copy fixture file");
        }
    }
}

fn build() -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    copy_fixture(&src, tmp.path());
    let merged = generate_one(tmp.path().to_str().expect("utf-8 tempdir"))
        .expect("generate_one")
        .merged;
    (tmp, merged)
}

/// Every MODULE node record in the build whose qname is `qname`, one entry
/// per record (a shared id in two graphs counts twice).
fn modules(merged: &MergedGraph, qname: &str) -> Vec<NodeId> {
    merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(move |n| (g, n.id)))
        .filter(|(g, id)| {
            g.nav.kind_by_id.get(id) == Some(&node_kind::MODULE)
                && g.nav.qname_by_id.get(id).map(String::as_str) == Some(qname)
        })
        .map(|(_, id)| id)
        .collect()
}

fn one_module(merged: &MergedGraph, qname: &str) -> NodeId {
    let found = modules(merged, qname);
    assert_eq!(found.len(), 1, "exactly one MODULE record `{qname}`");
    found[0]
}

#[test]
fn a_proto_and_a_same_stem_go_file_are_two_located_modules() {
    let (_tmp, merged) = build();
    let go = one_module(&merged, "api::user");
    let proto = one_module(&merged, "api::user.proto");
    assert_ne!(go, proto, "the .go and the .proto MODULE ids differ");

    let at = locate_node(&merged, go);
    assert_eq!(at.file.as_deref(), Some("api/user.go"));
    let at = locate_node(&merged, proto);
    assert_eq!(at.file.as_deref(), Some("api/user.proto"));
    assert_eq!(at.name, "user.proto");
    assert_eq!(at.qname, "api::user.proto");
}

#[test]
fn same_stem_non_code_files_are_separate_modules_with_their_own_edges() {
    let (_tmp, merged) = build();
    let yaml = one_module(&merged, "docs::swagger.yaml");
    let json = one_module(&merged, "docs::swagger.json");
    assert_ne!(yaml, json);
    assert!(
        modules(&merged, "docs::swagger").is_empty(),
        "no stem-form MODULE"
    );

    let dockerfile = one_module(&merged, "svc::Dockerfile");
    let prod = one_module(&merged, "svc::Dockerfile.prod");
    assert_ne!(dockerfile, prod);

    let qname_of = |id: NodeId| {
        merged
            .graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_default()
    };
    let defines_config = |from: NodeId| {
        let mut to: Vec<String> = merged
            .all_edges()
            .filter(|e| e.from == from && e.category == edge_category::DEFINES_CONFIG)
            .map(|e| qname_of(e.to))
            .collect();
        to.sort();
        to
    };
    assert_eq!(
        defines_config(dockerfile),
        ["config:env:PORT"],
        "one DEFINES_CONFIG edge, not the fold's doubled pair"
    );
    assert_eq!(
        defines_config(prod),
        ["config:env:LOG_LEVEL", "config:env:PORT"]
    );
}

#[test]
fn no_node_id_is_held_by_two_node_lists() {
    let (_tmp, merged) = build();
    let mut holders: HashMap<NodeId, Vec<usize>> = HashMap::new();
    for (gi, g) in merged.graphs.iter().enumerate() {
        for n in &g.nodes {
            holders.entry(n.id).or_default().push(gi);
        }
    }
    let mut shared: Vec<(String, Vec<usize>)> = holders
        .into_iter()
        .filter(|(_, gs)| gs.len() > 1)
        .map(|(id, gs)| {
            let q = merged
                .graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
                .unwrap_or_default();
            (q, gs)
        })
        .collect();
    shared.sort();
    assert!(
        shared.is_empty(),
        "ids held by more than one node list: {shared:?}"
    );
}
