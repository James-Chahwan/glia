//! LF.4b: ADRs under `doc/adr` (adr-tools' default) and the other ADR
//! directories are ingested, and every node an ADR section DOCUMENTS carries
//! one DECISION entry per ADR.

use std::path::{Path, PathBuf};

use glia_code_domain::{cell_type, node_kind};
use glia_core::{Cell, CellPayload};
use glia_engine::{GenerateResult, generate_one};

/// The committed substrate fixture this packet ships.
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/substrate-gap/fixtures/docs-adr-decision")
}

fn build(dir: &Path) -> GenerateResult {
    generate_one(dir.to_str().expect("utf-8 path")).expect("the repo builds")
}

/// A repo holding `files`.
fn repo(tmp: &Path, files: &[(&str, &str)]) -> GenerateResult {
    for (rel, text) in files {
        let path = tmp.join(rel);
        std::fs::create_dir_all(path.parent().expect("a file has a parent")).unwrap();
        std::fs::write(path, text).unwrap();
    }
    build(tmp)
}

/// Qnames of every node of `kind`.
fn qnames(r: &GenerateResult, kind: glia_core::NodeKindId) -> Vec<String> {
    let mut out: Vec<String> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(|n| g.nav.kind_by_id.get(&n.id) == Some(&kind))
                .filter_map(|n| g.nav.qname_by_id.get(&n.id).cloned())
        })
        .collect();
    out.sort();
    out
}

/// The cells of every instance of the node whose qname is `qname`.
fn cells_of(r: &GenerateResult, qname: &str) -> Vec<Vec<Cell>> {
    r.merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter(|n| g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname)))
        .map(|n| n.cells.clone())
        .collect()
}

/// The DECISION payload of `qname`'s one instance.
fn decision(r: &GenerateResult, qname: &str) -> Option<String> {
    let all = cells_of(r, qname);
    assert_eq!(all.len(), 1, "exactly one node is {qname}");
    let decisions: Vec<String> = all[0]
        .iter()
        .filter(|c| c.kind == cell_type::DECISION)
        .map(|c| match &c.payload {
            CellPayload::Json(s) => s.clone(),
            other => panic!("DECISION is JSON, got {other:?}"),
        })
        .collect();
    assert!(decisions.len() <= 1, "one DECISION cell per node: {decisions:?}");
    decisions.into_iter().next()
}

#[test]
fn adr_in_doc_adr_is_ingested() {
    let r = build(&fixture());
    let sections = qnames(&r, node_kind::DOC_SECTION);
    for slug in ["1-use-flask-for-orders", "status", "context", "decision", "consequences"] {
        let q = format!("docs::doc::adr::0001-use-flask-for-orders::{slug}");
        assert!(sections.contains(&q), "{q} in {sections:?}");
    }
}

#[test]
fn decision_lands_on_documented_node() {
    let r = build(&fixture());
    assert_eq!(
        decision(&r, "api::app::list_orders").as_deref(),
        Some(
            r#"[{"adr":"doc/adr/0001-use-flask-for-orders.md","id":"adr:doc/adr/0001-use-flask-for-orders.md","section":"docs::doc::adr::0001-use-flask-for-orders::decision","source":"adr","status":"accepted","title":"Use Flask for orders"}]"#
        )
    );
}

#[test]
fn non_adr_doc_dir_stays_out() {
    let r = build(&fixture());
    let sections = qnames(&r, node_kind::DOC_SECTION);
    assert!(
        sections.iter().all(|q| !q.starts_with("docs::doc::notes::")),
        "doc/notes.md is not an ADR and not ingested: {sections:?}"
    );
    assert_eq!(sections.len(), 5, "only the ADR's five sections: {sections:?}");
}

const APP: &str = "def save_order(order):\n    return order\n\n\ndef load_order(order_id):\n    return order_id\n";

#[test]
fn madr_status_line_is_read() {
    let tmp = tempfile::tempdir().unwrap();
    let r = repo(
        tmp.path(),
        &[
            ("app.py", APP),
            (
                "docs/decisions/0002-use-postgres.md",
                "# Use Postgres for orders\n\n* Status: proposed\n* Deciders: team\n\n\
                 ## Context and Problem Statement\n\nOrders need a store.\n\n\
                 ## Decision Drivers\n\n* durability\n\n\
                 ## Decision Outcome\n\nChosen option: write through `save_order`.\n",
            ),
        ],
    );
    assert_eq!(
        decision(&r, "app::save_order").as_deref(),
        Some(
            r#"[{"adr":"docs/decisions/0002-use-postgres.md","id":"adr:docs/decisions/0002-use-postgres.md","section":"docs::docs::decisions::0002-use-postgres::decision-outcome","source":"adr","status":"proposed","title":"Use Postgres for orders"}]"#
        )
    );
    assert_eq!(decision(&r, "app::load_order"), None, "an undocumented node gets nothing");
}

#[test]
fn two_sections_one_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let r = repo(
        tmp.path(),
        &[
            ("app.py", APP),
            (
                "adr/0003-idempotent-saves.md",
                "# 3. Idempotent saves\n\n## Status\n\nAccepted\n\n\
                 ## Context\n\n`save_order` is retried.\n\n\
                 ## Decision\n\nMake `save_order` idempotent.\n",
            ),
            (
                "adr/0004-archive.md",
                "# 4. Archive old orders\n\n## Status\n\nSuperseded by [5. Purge](0005-purge.md)\n\n\
                 ## Decision\n\nArchive via `save_order`.\n",
            ),
        ],
    );
    let Some(payload) = decision(&r, "app::save_order") else {
        panic!("save_order has a DECISION");
    };
    let entries: serde_json::Value = serde_json::from_str(&payload).unwrap();
    let ids: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    // Context AND Decision of 0003 mention it: one entry for that ADR, one
    // for 0004, in id order.
    assert_eq!(ids, ["adr:adr/0003-idempotent-saves.md", "adr:adr/0004-archive.md"]);
    assert_eq!(entries[1]["status"], "superseded");
    assert_eq!(entries[0]["section"], "docs::adr::0003-idempotent-saves::decision");
}

#[test]
fn numbered_doc_needs_the_adr_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let r = repo(
        tmp.path(),
        &[
            ("app.py", APP),
            // Outside an ADR directory: numbered AND shaped -> an ADR.
            (
                "docs/architecture/0005-load-path.md",
                "# 5. Load path\n\n## Status\n\nAccepted\n\n## Decision\n\nRead through `load_order`.\n",
            ),
            // Numbered but not shaped: a doc, not an ADR.
            ("docs/0006-setup.md", "# Setup\n\nCall `save_order` once.\n"),
        ],
    );
    assert!(decision(&r, "app::load_order").is_some_and(|d| d.contains(r#""title":"Load path""#)));
    assert_eq!(decision(&r, "app::save_order"), None);
}

#[test]
fn adr_decisions_are_deterministic() {
    let a = build(&fixture());
    let b = build(&fixture());
    assert_eq!(cells_of(&a, "api::app::list_orders"), cells_of(&b, "api::app::list_orders"));
}
