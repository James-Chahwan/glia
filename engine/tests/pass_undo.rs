//! LC.10a: the one piece of post-pass state a merge of pre-built layouts
//! cannot recompute is the confidence `demote_unmatched_http_nodes` overwrote.
//! A ROUTE / ENDPOINT with no HTTP_CALLS partner in ITS build is lowered from
//! Strong to Medium, so the same endpoint is Medium in a layout built from
//! `web/` alone and Strong when `web/` and `api/` are built together. The pass
//! records `(node, value before the pass)` in `MergedGraph::pass_undo`, the
//! layout persists it in `manifest.json`, and `undo_pass_mutations` puts the
//! value back before a re-merge.
//!
//! Measured before LC.10a: web-only `endpoint:GET:/users` medium, together
//! strong, and nothing recorded the pre-demotion value.

use std::path::{Path, PathBuf};

use glia_code_domain::node_kind;
use glia_core::{Confidence, NodeId};
use glia_engine::persist::{layout_meta, load_layout, persist_layout};
use glia_engine::{GenerateResult, generate_many};
use glia_graph::MergedGraph;

const ENDPOINT: &str = "endpoint:GET:/users";

/// A Flask route in `api/` and a TypeScript client fetching it in `web/`.
fn fixture(tmp: &Path) -> (PathBuf, PathBuf) {
    let api = tmp.join("api");
    let web = tmp.join("web");
    std::fs::create_dir_all(&api).unwrap();
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(
        api.join("app.py"),
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\")\n\
         def list_users():\n    return []\n",
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

fn build(repos: &[&Path]) -> GenerateResult {
    let paths: Vec<String> = repos.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    generate_many(&paths).unwrap()
}

/// The id of the one ENDPOINT whose qname is [`ENDPOINT`].
fn endpoint_id(m: &MergedGraph) -> NodeId {
    let ids: Vec<NodeId> = m
        .nodes_of_kind(node_kind::ENDPOINT)
        .into_iter()
        .filter(|id| {
            m.graphs.iter().any(|g| g.nav.qname_by_id.get(id).is_some_and(|q| q == ENDPOINT))
        })
        .collect();
    assert_eq!(ids.len(), 1, "exactly one {ENDPOINT} node: {ids:?}");
    ids[0]
}

/// The confidence of every instance of `id`, across every graph.
fn confidences(m: &MergedGraph, id: NodeId) -> Vec<Confidence> {
    m.graphs.iter().flat_map(|g| g.nodes.iter()).filter(|n| n.id == id).map(|n| n.confidence).collect()
}

#[test]
fn demotion_is_recorded_persisted_and_undoable() {
    let tmp = tempfile::tempdir().unwrap();
    let (_api, web) = fixture(tmp.path());

    let r = build(&[&web]);
    let ep = endpoint_id(&r.merged);
    assert_eq!(confidences(&r.merged, ep), vec![Confidence::Medium], "web-only: demoted");
    assert_eq!(r.merged.pass_undo, vec![(ep, Confidence::Strong)], "the demotion is recorded");

    let dir = tmp.path().join("layout");
    let meta = layout_meta(&r.repo_labels, &r.repo_roots, &r.parse_errors, &dir);
    persist_layout(&r.merged, &meta, &dir, "test").unwrap();
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
    assert!(
        manifest.contains("\"pass_undo\"") && manifest.contains("\"strong\""),
        "manifest records the undo: {manifest}"
    );

    let mut loaded = load_layout(&dir).unwrap().merged;
    assert_eq!(loaded.pass_undo, r.merged.pass_undo, "pass_undo survives the layout");
    assert_eq!(confidences(&loaded, ep), vec![Confidence::Medium], "loaded as written");

    loaded.undo_pass_mutations();
    assert_eq!(confidences(&loaded, ep), vec![Confidence::Strong], "undo restores Strong");
    assert!(loaded.pass_undo.is_empty(), "undo clears the record");

    // A second undo has nothing left to restore.
    loaded.undo_pass_mutations();
    assert_eq!(confidences(&loaded, ep), vec![Confidence::Strong]);
}

#[test]
fn no_demotion_no_undo() {
    let tmp = tempfile::tempdir().unwrap();
    let (api, web) = fixture(tmp.path());

    let r = build(&[&api, &web]);
    let ep = endpoint_id(&r.merged);
    assert_eq!(confidences(&r.merged, ep), vec![Confidence::Strong], "together: matched");
    assert!(r.merged.pass_undo.is_empty(), "nothing demoted, nothing recorded: {:?}", r.merged.pass_undo);

    // A layout without demotions writes no `pass_undo` key at all, so its
    // manifest bytes are what they were before LC.10a.
    let dir = tmp.path().join("layout");
    let meta = layout_meta(&r.repo_labels, &r.repo_roots, &r.parse_errors, &dir);
    persist_layout(&r.merged, &meta, &dir, "test").unwrap();
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
    assert!(!manifest.contains("pass_undo"), "no key when empty: {manifest}");
    assert!(load_layout(&dir).unwrap().merged.pass_undo.is_empty());
}

/// The undone web-only layout answers like the together build for the node
/// the pass touched: its confidence is the one `generate_many` never changed.
#[test]
fn undone_layout_matches_the_together_build() {
    let tmp = tempfile::tempdir().unwrap();
    let (api, web) = fixture(tmp.path());

    let together = build(&[&api, &web]);
    let web_only = build(&[&web]);
    let dir = tmp.path().join("web-layout");
    let meta = layout_meta(&web_only.repo_labels, &web_only.repo_roots, &web_only.parse_errors, &dir);
    persist_layout(&web_only.merged, &meta, &dir, "test").unwrap();
    let mut loaded = load_layout(&dir).unwrap().merged;
    loaded.undo_pass_mutations();

    let ep = endpoint_id(&together.merged);
    assert_eq!(endpoint_id(&loaded), ep, "same repo, same id");
    assert_eq!(confidences(&loaded, ep), confidences(&together.merged, ep));
}
