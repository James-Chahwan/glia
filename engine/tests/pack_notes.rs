//! FB.11 — corrections reach answers: a node's DECISION / CONV entries,
//! written through `.glia/cells.jsonl` (the cell sidecar the build applies),
//! render as `note[<source>/<id>, <tier>]: ..` lines in its pack text at
//! Outline and above, and the manifest counts them (`PackedNode::notes`).
//!
//! The fixture: `shop/pay.py` holds `charge_card(o)` and `refund(o)`, which
//! calls it; the sidecar carries the notes on `shop::pay::charge_card`.

use std::path::Path;

use glia_engine::generate_one;
use glia_engine::pack::{Pack, PackArgs, PackedNode, pack};
use glia_graph::MergedGraph;

const PAY_PY: &str = "def charge_card(o):
    \"\"\"Charge the order's card.\"\"\"
    total = o.total
    return total


def refund(o):
    return charge_card(o)
";

const QNAME: &str = "shop::pay::charge_card";

/// A tree with `shop/pay.py` and, when `rows` is not empty, one
/// `.glia/cells.jsonl` row per `(cell, entry JSON)` on [`QNAME`].
fn build(root: &Path, rows: &[(&str, &str)]) -> MergedGraph {
    std::fs::create_dir_all(root.join("shop")).expect("mkdir shop");
    std::fs::write(root.join("shop/pay.py"), PAY_PY).expect("write pay.py");
    if !rows.is_empty() {
        std::fs::create_dir_all(root.join(".glia")).expect("mkdir .glia");
        let lines: Vec<String> = rows
            .iter()
            .map(|(cell, entry)| {
                format!(r#"{{"qname":"{QNAME}","cell":"{cell}","entry":{entry}}}"#)
            })
            .collect();
        std::fs::write(root.join(".glia/cells.jsonl"), lines.join("\n") + "\n")
            .expect("write cells.jsonl");
    }
    generate_one(&root.to_string_lossy())
        .expect("generate_one")
        .merged
}

fn generous() -> PackArgs {
    let mut a = PackArgs::default();
    a.budget_tokens = 100_000;
    a
}

fn node<'p>(p: &'p Pack, qname: &str) -> &'p PackedNode {
    p.nodes
        .iter()
        .find(|n| n.qname == qname)
        .unwrap_or_else(|| panic!("{qname} not packed: {:?}", p.nodes))
}

#[test]
fn an_engram_correction_renders_as_a_heuristic_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    let m = build(
        dir.path(),
        &[(
            "DECISION",
            r#"{"source":"api","id":"engram:3f2a","text":"keep charge_card idempotent"}"#,
        )],
    );
    let p = pack(&m, "charge_card", &generous());
    assert!(
        p.text
            .contains("note[api/engram:3f2a, heuristic]: keep charge_card idempotent\n"),
        "{}",
        p.text
    );
    assert_eq!(node(&p, QNAME).notes, 1);
    assert_eq!(node(&p, "shop::pay::refund").notes, 0);
    let json = serde_json::to_value(&p).expect("serialize");
    let nodes = json["nodes"].as_array().cloned().unwrap_or_default();
    let with: Vec<&serde_json::Value> = nodes.iter().filter(|n| n.get("notes").is_some()).collect();
    assert_eq!(with.len(), 1, "notes is skipped when 0: {nodes:?}");
    assert_eq!(with[0]["notes"], 1);
}

#[test]
fn four_notes_show_three_and_count_the_rest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let m = build(
        dir.path(),
        &[
            (
                "CONV",
                r#"{"source":"api","id":"engram:a1","text":"first"}"#,
            ),
            (
                "CONV",
                r#"{"source":"api","id":"engram:a2","text":"second"}"#,
            ),
            (
                "CONV",
                r#"{"source":"api","id":"engram:a3","text":"third"}"#,
            ),
            (
                "DECISION",
                r#"{"source":"api","id":"engram:a4","title":"fourth"}"#,
            ),
        ],
    );
    let p = pack(&m, "charge_card", &generous());
    for want in [
        "note[api/engram:a1, heuristic]: first\n",
        "note[api/engram:a2, heuristic]: second\n",
        "note[api/engram:a3, heuristic]: third\n",
        "(+1 more notes)\n",
    ] {
        assert!(p.text.contains(want), "{want:?} in {}", p.text);
    }
    assert!(!p.text.contains("fourth"), "{}", p.text);
    assert_eq!(node(&p, QNAME).notes, 4);
}

#[test]
fn notes_add_text_never_order() {
    let plain_dir = tempfile::tempdir().expect("tempdir");
    let plain = pack(&build(plain_dir.path(), &[]), "charge_card", &generous());
    let noted_dir = tempfile::tempdir().expect("tempdir");
    let noted = pack(
        &build(
            noted_dir.path(),
            &[(
                "CONV",
                r#"{"source":"api","id":"000001","text":"called from the refund path"}"#,
            )],
        ),
        "charge_card",
        &generous(),
    );
    let order = |p: &Pack| -> Vec<(String, &'static str)> {
        p.nodes
            .iter()
            .map(|n| (n.qname.clone(), n.fidelity))
            .collect()
    };
    assert_eq!(order(&plain), order(&noted));
    assert!(plain.nodes.iter().all(|n| n.notes == 0));
    assert!(!plain.text.contains("note["), "{}", plain.text);
    assert_eq!(
        noted.text.replace(
            "note[api/000001, heuristic]: called from the refund path\n",
            ""
        ),
        plain.text
    );
}
