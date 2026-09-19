//! A10.5 acceptance: protobuf `message` / `enum` declarations reach the merged
//! graph as located MESSAGE_TYPE nodes.
//!
//! The unit tests in `parsers/code/extractors/src/schemas.rs` prove the
//! scanner. This proves the engine half against a real on-disk repo: the
//! `.proto` route stashes a MESSAGE-ONLY file (no `service`, which the route
//! used to skip), the post-pass provenance tagger leaves the contract ORIGIN
//! alone, and `locate_node` answers with the declaring file and line.

use glia_code_domain::{cell_type, node_kind};
use glia_core::{CellPayload, NodeId};
use glia_engine::{generate_one, locate_node};

/// Every MESSAGE_TYPE in the build as `(qname, id, ORIGIN payload)`, sorted.
fn message_types(merged: &glia_graph::MergedGraph) -> Vec<(String, NodeId, String)> {
    let mut out = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MESSAGE_TYPE) {
                continue;
            }
            let origin = n
                .cells
                .iter()
                .find_map(|c| match &c.payload {
                    CellPayload::Json(j) if c.kind == cell_type::ORIGIN => Some(j.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            out.push((g.nav.qname_by_id[&n.id].clone(), n.id, origin));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn messages_only_proto_yields_located_message_types() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("shared")).unwrap();
    // A shared `common.proto`: types only, no service.
    std::fs::write(
        repo.join("shared/common.proto"),
        "syntax = \"proto3\";\n\npackage common.v1;\n\nmessage Money {\n  string currency = 1;\n  int64 units = 2;\n}\n\nenum Status {\n  STATUS_UNSPECIFIED = 0;\n}\n",
    )
    .unwrap();

    let merged = generate_one(repo.to_str().unwrap()).unwrap().merged;
    let found = message_types(&merged);
    let qnames: Vec<&str> = found.iter().map(|(q, _, _)| q.as_str()).collect();
    assert_eq!(
        qnames,
        vec!["message:proto:common.v1.Money", "message:proto:common.v1.Status"],
        "a .proto with no service must still be parsed"
    );
    for (q, _, origin) in &found {
        assert!(
            origin.starts_with(r#"{"provenance":"contract","source":"proto""#),
            "{q}: the contract ORIGIN survives the provenance post-pass, got {origin}"
        );
    }

    let at = locate_node(&merged, found[0].1);
    assert_eq!(at.name, "Money", "nav name is the bare declared name");
    assert_eq!(at.qname, "message:proto:common.v1.Money");
    assert_eq!(at.kind, "MESSAGE_TYPE");
    assert_eq!(at.file.as_deref(), Some("shared/common.proto"));
    assert_eq!(at.line, Some(5), "1-based `message Money {{` line");
}
