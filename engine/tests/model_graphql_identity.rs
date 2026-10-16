//! DT.1k acceptance: `graphql_model` identity over the committed
//! `models-graphql-stock` fixture (an Apollo console and a gqlgen API, built
//! with `generate_many` over the dirs its key.json names).
//!
//! The console types each call's result at the call
//! (`client.query<{ items: InventoryItem[] }>({ query: LIST_ITEMS })`,
//! `client.mutate<{ adjustStock: InventoryItem }>({ mutation: ADJUST_STOCK })`):
//! the TS parser writes a `graphql_result` MESSAGE_TYPE cell on each calling
//! FUNCTION. The models pass binds each document to its `gql` const, takes the
//! op minted in that const's span, follows each result key to the root field
//! the op selects and the type that field returns, and joins the console's
//! `InventoryItem` (it lists `barcode`, which `type Item` lacks: diverged) and
//! gqlgen's generated `model.Item` with `message:graphql:Item` in one DERIVED
//! class, the type the center. The hand-written `internal/stock.Item` has the
//! same name and json names but is never generated: it joins nothing.
//!
//! Baseline before DT.1k (HEAD 51632fcd): the fixture graded SAME_AS 0/2 and
//! its two new FUNCTION MESSAGE_TYPE cells 0/2 (forbid 0).
//!
//! fired_on, on stderr of every build of the fixture: `[ts-wire]
//! graphql_results=2 file=src/inventory.ts`, `[models] graphql results=2
//! documents_resolved=2 generated_models=1 classes=1` and `[evidence-rules]
//! resolver=models http_model=0 graphql_model=2`.

use std::path::{Path, PathBuf};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::wire::{ServerWire, WireForm};
use glia_code_domain::{edge_category, node_kind, same_as};
use glia_core::Confidence;
use glia_engine::identity::{IdentityQuery, identity_classes};
use glia_engine::merge::{MergeMember, merge_layouts, persist_merge, read_workspace};
use glia_engine::persist::persist_result;
use glia_engine::why::why_edge;
use glia_engine::{GenerateResult, generate_many};
use glia_graph::MergedGraph;

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

fn stock() -> GenerateResult {
    generate_many(&fixture_dirs("models-graphql-stock")).expect("models-graphql-stock builds")
}

fn qname(m: &MergedGraph, id: glia_core::NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// `(from, to, emitter, rule, file, 0-based line, confidence)` of every
/// SAME_AS edge, sorted.
type Row = (String, String, String, String, String, u32, Confidence);

fn same_as_rows(m: &MergedGraph) -> Vec<Row> {
    let mut rows: Vec<Row> = m
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::SAME_AS)
        .map(|e| {
            assert_eq!(e.cells.len(), 1, "one EVIDENCE cell: {e:?}");
            let ev = Evidence::of(e).expect("every SAME_AS edge carries EVIDENCE");
            (
                qname(m, e.from),
                qname(m, e.to),
                ev.emitter,
                ev.rule.unwrap_or_default(),
                ev.file.unwrap_or_default(),
                ev.line.unwrap_or(u32::MAX),
                e.confidence,
            )
        })
        .collect();
    rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    rows
}

fn row(from: &str, rule: &str, file: &str, line: u32) -> Row {
    (
        from.to_string(),
        "message:graphql:Item".to_string(),
        "resolver:models".to_string(),
        rule.to_string(),
        file.to_string(),
        line,
        Confidence::Medium,
    )
}

#[test]
fn result_and_generated_models_join_the_schema_type() {
    let r = stock();
    assert_eq!(
        same_as_rows(&r.merged),
        [
            // gqlgen's generated model, at its declaration.
            row(
                "graph::model::models_gen::Item",
                "graphql_model",
                "graph/model/models_gen.go",
                4
            ),
            // barcode is beyond the type; the first join by op qname is
            // AdjustStock's call (src/inventory.ts:11).
            row(
                "src::types::InventoryItem",
                "graphql_model/diverged",
                "src/inventory.ts",
                10
            ),
        ],
        "the hand-written internal::stock::item::Item joins nothing"
    );
}

/// The TS parser's cells: each calling FUNCTION carries its result type and
/// its document.
#[test]
fn each_typed_call_carries_its_graphql_result() {
    let r = stock();
    let mut sites: Vec<(String, String, u32, String)> = Vec::new();
    for g in &r.merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::FUNCTION) {
                continue;
            }
            for w in n.cells.iter().filter_map(ServerWire::from_cell) {
                if w.form == WireForm::GraphqlResult {
                    sites.push((
                        qname(&r.merged, n.id),
                        w.document.clone().unwrap_or_default(),
                        w.line,
                        w.raw.clone(),
                    ));
                }
            }
        }
    }
    sites.sort();
    assert_eq!(
        sites,
        [
            (
                "src::inventory::adjust".to_string(),
                "ADJUST_STOCK".to_string(),
                11,
                "{ adjustStock: InventoryItem }".to_string()
            ),
            (
                "src::inventory::emptyShelves".to_string(),
                "LIST_ITEMS".to_string(),
                6,
                "{ items: InventoryItem[] }".to_string()
            ),
        ]
    );
}

#[test]
fn the_identity_report_lists_one_diverged_graphql_class() {
    let r = stock();
    let mut q = IdentityQuery::default();
    q.repo_labels = r.repo_labels.clone();
    q.rule = Some(same_as::RULE_GRAPHQL_MODEL.to_string());
    let report = identity_classes(&r.merged, &q).expect("a known rule");
    assert_eq!(
        (
            report.counts.classes,
            report.counts.derived,
            report.counts.diverged
        ),
        (1, 1, 1)
    );
    assert!(
        report.counts.by_rule.contains(&("graphql_model", 1)),
        "{:?}",
        report.counts.by_rule
    );
    let class = &report.classes[0];
    assert_eq!(
        (
            class.tier,
            class.rules.as_slice(),
            class.collapses,
            class.walks
        ),
        ("derived", ["graphql_model"].as_slice(), true, true)
    );
    let mut members: Vec<(&str, &str)> = class
        .members
        .iter()
        .map(|m| (m.repo.as_str(), m.qname.as_str()))
        .collect();
    members.sort();
    assert_eq!(
        members,
        [
            ("api", "graph::model::models_gen::Item"),
            ("api", "message:graphql:Item"),
            ("console", "src::types::InventoryItem"),
        ]
    );
}

#[test]
fn why_explains_the_graphql_identity_edge() {
    let r = stock();
    let a = why_edge(
        &r.merged,
        "src::types::InventoryItem",
        "message:graphql:Item",
        Some("SAME_AS"),
    )
    .expect("both sides resolve");
    assert!(a.found, "{a:?}");
    let e = &a.edges[0];
    assert_eq!(
        (
            e.category,
            e.tier,
            e.emitter.as_deref(),
            e.rule.as_deref(),
            e.cross_repo
        ),
        (
            "SAME_AS",
            "derived",
            Some("resolver:models"),
            Some("graphql_model/diverged"),
            true
        )
    );
    let site = e.site.as_ref().expect("located at the call site");
    assert_eq!(
        (site.file.as_str(), site.line),
        ("src/inventory.ts", Some(11))
    );
}

#[test]
fn graphql_identity_is_deterministic() {
    assert_eq!(same_as_rows(&stock().merged), same_as_rows(&stock().merged));
}

/// A layout member through the workspace reader, as a caller outside the
/// crate builds one.
fn gmap_member(name: &str, dir: &Path) -> MergeMember {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("glia.workspace.json");
    let member = format!(r#"{{"name":"{name}","gmap":"{}"}}"#, dir.display());
    std::fs::write(&path, format!(r#"{{"version":1,"members":[{member}]}}"#)).unwrap();
    read_workspace(&path).unwrap().remove(0)
}

/// Every `.gmap` file directly in `dir`, by name.
fn gmaps(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".gmap"))
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

/// merge == joint build: neither member alone pairs an operation, the merge
/// re-runs `graphql` and then `models` over the union from persisted cells
/// and symbol tables (the generated model's ORIGIN and its file's header
/// alike), and its shards are the joint build's bytes.
#[test]
fn merge_equals_the_joint_build() {
    let dirs = fixture_dirs("models-graphql-stock");
    let tmp = tempfile::tempdir().unwrap();
    let (a, b, t) = (
        tmp.path().join("console"),
        tmp.path().join("api"),
        tmp.path().join("joint"),
    );
    let console = generate_many(&dirs[..1]).unwrap();
    assert!(
        same_as_rows(&console.merged).is_empty(),
        "one repo pairs no operation"
    );
    persist_result(&console, &a, "test").unwrap();
    persist_result(&generate_many(&dirs[1..]).unwrap(), &b, "test").unwrap();
    let joint = generate_many(&dirs).unwrap();
    persist_result(&joint, &t, "test").unwrap();

    let m = merge_layouts(&[gmap_member("console", &a), gmap_member("api", &b)]).unwrap();
    assert_eq!(same_as_rows(&m.result.merged), same_as_rows(&joint.merged));
    assert_eq!(same_as_rows(&m.result.merged).len(), 2);
    let out = tmp.path().join("merged");
    persist_merge(&m, &out, "test").unwrap();
    let (gm, gt) = (gmaps(&out), gmaps(&t));
    assert_eq!(
        gm.iter().map(|g| &g.0).collect::<Vec<_>>(),
        gt.iter().map(|g| &g.0).collect::<Vec<_>>()
    );
    for ((name, bm), (_, bt)) in gm.iter().zip(&gt) {
        assert!(
            bm == bt,
            "{name}: merged bytes differ from the joint build's"
        );
    }
}
