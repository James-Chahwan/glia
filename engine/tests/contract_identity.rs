//! LB.12 acceptance: a contract op, and a markdown DOC_SECTION, is scoped by
//! the DIRECTORY + STEM of the file that declares it, on a REAL build of the
//! `contract-per-file-identity` fixture.
//!
//! `grade.py` reads the installed wheel, so it cannot see this change until
//! the end-of-wave rebuild; this grades the working tree directly. The build
//! runs on a temp copy so no persist cache lands in the committed fixture.
//! HEAD before LB.12: services/orders/openapi.yaml, services/billing/openapi.yaml
//! and services/billing/openapi.json all minted `contract::openapi::GET:/orders`
//! (one NodeId carrying both services' POSITION cells), and docs/orders/guide.md
//! + docs/billing/guide.md both minted `docs::guide::setup`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use repo_graph_code_domain::{cell_type, dir_stem_qname, node_kind};
use repo_graph_code_extractors::contracts::contract_op_qname;
use repo_graph_core::{CellPayload, NodeId};
use repo_graph_engine::{generate_one, governing_docs};
use repo_graph_graph::MergedGraph;

const FIXTURE: &str = "contract-per-file-identity";

/// The fixture's contract files, repo-relative.
const CONTRACT_FILES: [&str; 4] = [
    "openapi.yaml",
    "services/orders/openapi.yaml",
    "services/billing/openapi.yaml",
    "services/billing/openapi.json",
];

fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).expect("fixture dir reads") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name();
        // A committed persist cache is not part of the fixture's source.
        if name == ".ai" || name == ".glia" || name == "key.json" {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(&name));
        if src.is_dir() {
            std::fs::create_dir_all(&dst).expect("mkdir");
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copy");
        }
    }
}

fn build() -> (tempfile::TempDir, MergedGraph) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(FIXTURE);
    let tmp = tempfile::tempdir().expect("tempdir");
    copy_tree(&fixture, tmp.path());
    let merged = generate_one(tmp.path().to_str().expect("utf-8 tempdir"))
        .expect("fixture builds")
        .merged;
    (tmp, merged)
}

/// Every DOC_SECTION record: (qname, id), one per graph that holds it.
fn doc_sections(m: &MergedGraph) -> Vec<(String, NodeId)> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::DOC_SECTION))
                .map(move |n| (g.nav.qname_by_id[&n.id].clone(), n.id))
        })
        .collect()
}

/// The distinct ids behind the DOC_SECTION qnames starting with `prefix`.
fn ids_by_qname(m: &MergedGraph, prefix: &str) -> BTreeMap<String, BTreeSet<u64>> {
    let mut out: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    for (q, id) in doc_sections(m) {
        if q.starts_with(prefix) {
            out.entry(q).or_default().insert(id.0);
        }
    }
    out
}

/// The `file` of every POSITION cell on every record of `id`.
fn position_files(m: &MergedGraph, id: NodeId) -> BTreeSet<String> {
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .flat_map(|n| n.cells.iter())
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) if c.kind == cell_type::POSITION => {
                let v: serde_json::Value = serde_json::from_str(j).ok()?;
                v.get("file")?.as_str().map(str::to_string)
            }
            _ => None,
        })
        .collect()
}

fn op_id(m: &MergedGraph, qname: &str) -> NodeId {
    doc_sections(m)
        .into_iter()
        .find(|(q, _)| q == qname)
        .map(|(_, id)| id)
        .unwrap_or_else(|| panic!("no DOC_SECTION {qname}"))
}

#[test]
fn contract_ops_are_scoped_by_directory_and_stem() {
    let (_tmp, m) = build();
    let ops = ids_by_qname(&m, "contract::");
    let qnames: Vec<&str> = ops.keys().map(String::as_str).collect();
    assert_eq!(
        qnames,
        [
            "contract::openapi::GET:/health",
            "contract::services::billing::openapi::GET:/orders",
            "contract::services::billing::openapi::POST:/invoices",
            "contract::services::orders::openapi::GET:/orders",
        ],
        "two directories: two GET /orders ops; the billing yaml / json twin: one"
    );
    // One NodeId per qname, and no id shared between qnames.
    assert!(ops.values().all(|ids| ids.len() == 1), "{ops:?}");
    let distinct: BTreeSet<u64> = ops.values().flatten().copied().collect();
    assert_eq!(distinct.len(), 4);

    // Each op is recorded by the files that declare it, and only those: the
    // orders op is no longer stacked with billing's POSITION cells.
    let orders = op_id(&m, "contract::services::orders::openapi::GET:/orders");
    assert_eq!(
        position_files(&m, orders),
        BTreeSet::from(["services/orders/openapi.yaml".to_string()])
    );
    let billing = op_id(&m, "contract::services::billing::openapi::GET:/orders");
    assert_eq!(
        position_files(&m, billing),
        BTreeSet::from([
            "services/billing/openapi.json".to_string(),
            "services/billing/openapi.yaml".to_string(),
        ]),
        "the twin is one op declared by both of its files"
    );
    let health = op_id(&m, "contract::openapi::GET:/health");
    assert_eq!(
        position_files(&m, health),
        BTreeSet::from(["openapi.yaml".to_string()])
    );
}

/// The op scope is the shared directory + stem rule, not the MODULE rule:
/// the twin's two files stay two MODULEs (LB.9a, full file name) while their
/// ops merge.
#[test]
fn op_scope_is_the_directory_stem_rule() {
    let (_tmp, m) = build();
    let ops = ids_by_qname(&m, "contract::");
    for file in CONTRACT_FILES {
        let scope = format!("contract::{}::", dir_stem_qname(file));
        assert!(
            ops.keys().any(|q| q.starts_with(&scope)),
            "{file}: no op under {scope} in {:?}",
            ops.keys()
        );
        assert!(contract_op_qname(file, "GET:/x").starts_with(&scope));
    }
    let modules: BTreeSet<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .kind_by_id
                .iter()
                .filter(|(_, k)| **k == node_kind::MODULE)
                .filter_map(move |(id, _)| g.nav.qname_by_id.get(id).cloned())
        })
        .filter(|q| q.starts_with("services::billing::openapi"))
        .collect();
    assert_eq!(
        modules,
        BTreeSet::from([
            "services::billing::openapi.json".to_string(),
            "services::billing::openapi.yaml".to_string(),
        ])
    );
}

/// `glia docs-for` on the one GET /orders route: one op per declaring
/// directory, each still paired through ORIGIN.
#[test]
fn governing_docs_lists_one_op_per_directory() {
    let (_tmp, m) = build();
    let docs = governing_docs(&m, "GET /orders", None);
    let mut got: Vec<(String, Option<String>)> = docs
        .results
        .iter()
        .filter(|d| d.qname.starts_with("contract::"))
        .map(|d| (d.qname.clone(), d.file.clone()))
        .collect();
    got.sort();
    let qnames: Vec<&str> = got.iter().map(|(q, _)| q.as_str()).collect();
    assert_eq!(
        qnames,
        [
            "contract::services::billing::openapi::GET:/orders",
            "contract::services::orders::openapi::GET:/orders",
        ],
        "{got:?}"
    );
    assert_eq!(got[1].1.as_deref(), Some("services/orders/openapi.yaml"));
}

/// The markdown half: two directories' `guide.md` are two sections.
#[test]
fn doc_sections_are_scoped_by_directory_and_stem() {
    let (_tmp, m) = build();
    let docs = ids_by_qname(&m, "docs::");
    let setup: Vec<&str> = docs
        .keys()
        .map(String::as_str)
        .filter(|q| q.ends_with("::setup"))
        .collect();
    assert_eq!(
        setup,
        [
            "docs::docs::billing::guide::setup",
            "docs::docs::orders::guide::setup"
        ]
    );
    let ids: BTreeSet<u64> = docs.values().flatten().copied().collect();
    assert_eq!(
        ids.len(),
        docs.len(),
        "one NodeId per section qname: {docs:?}"
    );
    let billing = op_id(&m, "docs::docs::billing::guide::setup");
    assert_eq!(
        position_files(&m, billing),
        BTreeSet::from(["docs/billing/guide.md".to_string()])
    );
}
