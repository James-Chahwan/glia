//! DT.1n acceptance: GraphQL selection drift, `glia_graph::graphql_pairs` and
//! the `graphql_selection` rows of `contract_fields`, over the committed
//! `models-graphql-stock` fixture (DT.1j's sources, read only: an Apollo
//! console and a gqlgen API whose `schema.graphqls` declares `type Item`).
//!
//! - `mutation AdjustStock { adjustStock { id onHand } }` selects only
//!   fields `Item` declares: identical.
//! - `query ListItems { items { id label priceCents onHand barcode } }`
//!   pairs `graphql_resolver:items` by the root field it selects (DT.11a;
//!   the op name ListItems keys no resolver) and asks for `barcode`, which
//!   `Item` lacks: breaking, `barcode` graphql_field_undefined.
//!
//! fired_on: `[models] graphql pairs=2 undefined_fields=1 open=0 refused=0`
//! and `[contract-fields] .. graphql_selection=2`, printed by
//! `contract_fields`: `cargo test -p glia-engine --test model_graphql --
//! --nocapture 2>&1 | grep -E '^\[(models\] graphql|contract-fields\])'`.

use std::path::{Path, PathBuf};

use glia_code_domain::wire::Shape;
use glia_engine::contract_fields::{FieldDiffRow, contract_fields};
use glia_engine::generate_many;
use glia_graph::{MergedGraph, graphql_pairs};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
}

/// The repo paths a fixture's key.json `dirs` names, in order.
fn fixture_dirs(name: &str) -> Vec<String> {
    let root = fixture(name);
    let key: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("key.json")).unwrap()).unwrap();
    key["dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| root.join(d.as_str().unwrap()).to_str().unwrap().to_string())
        .collect()
}

fn stock() -> MergedGraph {
    generate_many(&fixture_dirs("models-graphql-stock"))
        .expect("the fixture builds")
        .merged
}

#[test]
fn graphql_pairs_over_the_stock_fixture() {
    let gp = graphql_pairs(&stock());
    eprintln!("{}", gp.marker());
    assert_eq!(
        gp.marker(),
        "[models] graphql pairs=2 undefined_fields=1 open=0 refused=0"
    );
    // (operation, op qname, resolver qname, schema type, (path, type, rule) changes).
    type Row<'a> = (
        String,
        &'a str,
        &'a str,
        Option<&'a str>,
        Vec<(&'a str, &'a str, &'a str)>,
    );
    let rows: Vec<Row<'_>> = gp
        .pairs
        .iter()
        .map(|p| {
            (
                format!("{} {} {}", p.op_type, p.op_name, p.root_field),
                p.op_qname.as_str(),
                p.resolver_qname.as_str(),
                p.schema_type_qname.as_deref(),
                p.changes
                    .iter()
                    .map(|c| (c.path.as_str(), c.type_name.as_str(), c.rule))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                "mutation AdjustStock adjustStock".to_string(),
                "graphql_op:AdjustStock",
                "graphql_resolver:adjustStock",
                Some("message:graphql:Item"),
                vec![],
            ),
            (
                "query ListItems items".to_string(),
                "graphql_op:ListItems",
                "graphql_resolver:items",
                Some("message:graphql:Item"),
                vec![("barcode", "Item", "graphql_field_undefined")],
            ),
        ]
    );
    let list = &gp.pairs[1];
    assert_eq!(
        (
            list.returns.as_str(),
            list.type_name.as_str(),
            list.response_keys.as_slice()
        ),
        ("[Item!]!", "Item", ["items".to_string()].as_slice())
    );
    let Shape::Object { fields, open } = &list.selection else {
        panic!("an object selection: {:?}", list.selection);
    };
    assert!(!open);
    assert_eq!(
        fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["id", "label", "priceCents", "onHand", "barcode"]
    );
    assert!(
        gp.pairs
            .iter()
            .all(|p| p.notes.is_empty() && p.refused.is_none())
    );
}

type RowView = (
    String,
    String,
    String,
    &'static str,
    Option<&'static str>,
    Vec<(
        String,
        String,
        &'static str,
        Option<String>,
        Option<String>,
        &'static str,
        bool,
    )>,
);

fn view(rows: &[FieldDiffRow]) -> Vec<RowView> {
    rows.iter()
        .filter(|r| r.pairing == "graphql_selection")
        .map(|r| {
            (
                r.key.clone(),
                r.producer.qname.clone(),
                r.consumer.qname.clone(),
                r.status,
                r.note,
                r.changes
                    .iter()
                    .map(|c| {
                        (
                            c.section.clone(),
                            c.field.clone(),
                            c.change,
                            c.producer.clone(),
                            c.consumer.clone(),
                            c.rule,
                            c.breaking,
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn graphql_selection_rows_over_the_stock_fixture() {
    let rows = contract_fields(&stock());
    let item = "message:graphql:Item";
    assert_eq!(
        view(&rows),
        [
            (
                "mutation AdjustStock adjustStock".to_string(),
                item.to_string(),
                "graphql_op:AdjustStock".to_string(),
                "identical",
                None,
                vec![],
            ),
            (
                "query ListItems items".to_string(),
                item.to_string(),
                "graphql_op:ListItems".to_string(),
                "breaking",
                None,
                vec![(
                    "selection".to_string(),
                    "barcode".to_string(),
                    "consumer_only",
                    None,
                    Some("barcode".to_string()),
                    "graphql_field_undefined",
                    true,
                )],
            ),
        ],
        "{rows:#?}"
    );
    let list = rows
        .iter()
        .find(|r| r.key == "query ListItems items")
        .expect("the ListItems row");
    assert_eq!(list.tier, "derived");
    let at = |s: &glia_engine::contract_fields::FieldSide| {
        (
            s.format.clone(),
            s.file.clone().unwrap_or_default(),
            s.line.unwrap_or(0),
        )
    };
    assert_eq!(
        at(&list.producer),
        (
            "graphql".to_string(),
            "graph/schema.graphqls".to_string(),
            1
        ),
        "the type at its declaration"
    );
    assert_eq!(
        at(&list.consumer),
        ("graphql".to_string(), "src/documents.ts".to_string(), 3),
        "the operation at its document"
    );
    assert_ne!(list.producer.repo_id, 0);
    assert_ne!(
        list.producer.repo_id, list.consumer.repo_id,
        "the api's type, the console's op"
    );
}

#[test]
fn graphql_selection_rows_are_deterministic() {
    let a = serde_json::to_string(&contract_fields(&stock())).unwrap();
    let b = serde_json::to_string(&contract_fields(&stock())).unwrap();
    assert_eq!(a, b);
    assert_eq!(graphql_pairs(&stock()), graphql_pairs(&stock()));
}
