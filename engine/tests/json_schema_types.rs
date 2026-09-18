//! LA.16 (programme A10.12) acceptance: JSON Schema files become located
//! MESSAGE_TYPE nodes, `message:jsonschema:<root>[.<def>]`, and two services
//! that vendor the same schema are joined by MessageSchemaResolver.
//!
//! The unit tests in `parsers/code/extractors/src/schemas.rs` prove the sniff
//! and the scanner, and `walk.rs` proves admission. This builds the committed
//! substrate fixture `xschema-jsonschema-shared` through `generate_many` (the
//! entry `bench/substrate-gap` grades through, which writes nothing into the
//! repos): the schema files survive the walk, route to the "json" group, and
//! the shared root pairs across the two repos, while billing's data file whose
//! nested object only looks schema-shaped mints nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, NodeId, RepoId};
use repo_graph_engine::generate_many;

fn fixture(dir: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures/xschema-jsonschema-shared")
        .join(dir)
        .to_string_lossy()
        .to_string()
}

fn json_cell(cells: &[repo_graph_core::Cell], kind: repo_graph_core::CellTypeId) -> Option<String> {
    cells
        .iter()
        .find(|c| c.kind == kind)
        .and_then(|c| match &c.payload {
            CellPayload::Json(j) => Some(j.clone()),
            _ => None,
        })
}

#[test]
fn json_schemas_are_shared_message_types() {
    let r = generate_many(&[fixture("orders"), fixture("billing")]).unwrap();
    assert!(r.parse_errors.is_empty(), "{:?}", r.parse_errors);
    // Labels are the input directories' names (A9.2).
    let repo_of = |label: &str| {
        r.repo_labels
            .iter()
            .find(|(_, l)| l.as_str() == label)
            .map(|(id, _)| RepoId(*id))
            .unwrap_or_else(|| panic!("no repo labelled {label}: {:?}", r.repo_labels))
    };
    let (orders, billing) = (repo_of("orders"), repo_of("billing"));

    // Every MESSAGE_TYPE, per repo, with its cells; and every node's qname.
    let mut types: HashMap<RepoId, BTreeSet<String>> = HashMap::new();
    let mut by_qname: BTreeMap<String, Vec<(RepoId, NodeId, Vec<repo_graph_core::Cell>)>> =
        BTreeMap::new();
    let mut qname_of: HashMap<NodeId, String> = HashMap::new();
    let mut edges = Vec::new();
    for g in &r.merged.graphs {
        edges.extend(g.edges.iter().cloned());
        for n in &g.nodes {
            let q = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            assert!(
                !q.contains("settings"),
                "the look-alike data file minted {q}"
            );
            qname_of.insert(n.id, q.clone());
            if g.nav.kind_by_id.get(&n.id) == Some(&node_kind::MESSAGE_TYPE) {
                types.entry(g.repo).or_default().insert(q.clone());
                by_qname
                    .entry(q)
                    .or_default()
                    .push((g.repo, n.id, n.cells.clone()));
            }
        }
    }
    let set = |qs: &[&str]| {
        qs.iter()
            .map(|q| q.to_string())
            .collect::<BTreeSet<String>>()
    };
    assert_eq!(
        types.get(&billing),
        Some(&set(&[
            "message:jsonschema:OrderCreated",
            "message:jsonschema:OrderCreated.Address"
        ])),
    );
    assert_eq!(
        types.get(&orders),
        Some(&set(&[
            "message:jsonschema:OrderCreated",
            "message:jsonschema:OrderCreated.Address",
            "message:jsonschema:RefundIssued",
        ])),
    );
    assert_eq!(
        types.values().map(BTreeSet::len).sum::<usize>(),
        5,
        "exactly five, no extras"
    );

    // One SHARES_SCHEMA cross-edge joins the two OrderCreated roots.
    let roots = &by_qname["message:jsonschema:OrderCreated"];
    assert_eq!(roots.len(), 2);
    let root_ids: HashSet<NodeId> = roots.iter().map(|(_, id, _)| *id).collect();
    let shares: Vec<_> = r
        .merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::SHARES_SCHEMA)
        .filter(|e| root_ids.contains(&e.from) && root_ids.contains(&e.to))
        .collect();
    assert_eq!(shares.len(), 1, "one pair for one shared root");
    assert_ne!(shares[0].from, shares[0].to);

    // OrderCreated DEFINES OrderCreated.Address in each repo.
    for (repo, root, _) in roots {
        let address = by_qname["message:jsonschema:OrderCreated.Address"]
            .iter()
            .find(|(r2, _, _)| r2 == repo)
            .map(|(_, id, _)| *id)
            .unwrap();
        assert!(
            edges.iter().any(|e| e.category == edge_category::DEFINES
                && e.from == *root
                && e.to == address),
            "{repo:?}: OrderCreated DEFINES OrderCreated.Address"
        );
    }

    // ORIGIN and POSITION on the root, identical in both vendored copies.
    for (_, id, cells) in roots {
        let origin = json_cell(cells, cell_type::ORIGIN).unwrap_or_default();
        assert!(
            origin.contains(r#""provenance":"contract","source":"jsonschema""#),
            "{}: {origin}",
            qname_of[id]
        );
        assert_eq!(
            json_cell(cells, cell_type::POSITION).as_deref(),
            Some(r#"{"file":"schemas/order-created.schema.json","start_line":0,"end_line":17}"#)
        );
    }
    assert!(
        !qname_of
            .values()
            .any(|q| q == "message:jsonschema:settings")
    );
}
