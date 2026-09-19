//! LD.8a — absence answers: an empty answer says WHY it is empty.
//!
//! `resolve_signal_located`, `governing_docs` and `find::find_nodes` return
//! `absence::Answer { results, absence }`. `absence` is `Some` exactly when
//! `results` is empty, and it is a FACT about the graph as built — reason,
//! one-sentence note, the edge categories the answer depended on, their
//! coverage caveat rows, and nearest-qname suggestions for an unknown symbol.
//!
//! Before LD.8a, over this same 9-line `app.py`: `governing_docs("nope")` was
//! `Err("no node with qname/name `nope`")`, `governing_docs("app::helper")`
//! and `resolve("zzz.py", diff)` were a bare `[]`, and a scope that removed
//! every row was indistinguishable from "nothing there".

use glia_engine::absence::{Absence, Answer};
use glia_engine::find::{FindOptions, find_nodes};
use glia_engine::{GenerateResult, generate_one, governing_docs, resolve_signal_located};

/// LD.1's fixture: `def helper` on line 4, `def main` (calls it) on line 8.
const APP_PY: &str =
    "import os\n\n\ndef helper(x):\n    return x + 1\n\n\ndef main():\n    return helper(2)\n";

fn build() -> (tempfile::TempDir, GenerateResult) {
    let td = tempfile::tempdir().expect("tempdir");
    std::fs::write(td.path().join("app.py"), APP_PY).expect("write app.py");
    let result = generate_one(td.path().to_str().expect("utf-8 path")).expect("generate_one");
    (td, result)
}

/// The invariant every answer keeps: an absence iff no results.
fn absence_of<T>(a: &Answer<T>) -> &Absence {
    assert_eq!(
        a.absence.is_some(),
        a.results.is_empty(),
        "absence iff empty"
    );
    a.absence
        .as_ref()
        .expect("an empty answer carries an absence")
}

#[test]
fn unknown_symbol_is_an_absence_with_suggestions_not_an_error() {
    let (_td, r) = build();
    let docs = governing_docs(&r.merged, "nope", None);
    let a = absence_of(&docs);
    assert_eq!((a.tier, a.reason), ("FACT", "unknown_symbol"), "{a:?}");
    assert_eq!(a.mechanisms, ["DOCUMENTS"]);
    assert_eq!(a.query, "nope");
    assert!(a.note.contains("`nope`"), "{}", a.note);
    assert!(a.suggestions.len() <= 3, "{:?}", a.suggestions);
    assert!(a.nodes_searched >= 3, "app, helper, main at least: {a:?}");
    assert_eq!(a.unparsed_files, 0, "only a surface fills it");

    // A near miss is suggested by qname, nearest first.
    let near = governing_docs(&r.merged, "helpr", None);
    let a = absence_of(&near);
    assert_eq!(a.reason, "unknown_symbol");
    assert_eq!(
        a.suggestions.first().map(String::as_str),
        Some("app::helper"),
        "{a:?}"
    );
}

#[test]
fn a_known_symbol_with_no_doc_edge_is_no_edges_with_its_caveat() {
    let (_td, r) = build();
    let docs = governing_docs(&r.merged, "app::helper", None);
    let a = absence_of(&docs);
    assert_eq!((a.tier, a.reason), ("FACT", "no_edges"), "{a:?}");
    assert_eq!(a.mechanisms, ["DOCUMENTS"]);
    assert_eq!(
        a.note,
        "no DOCUMENTS edge reaches `app::helper` in this graph"
    );
    assert!(a.suggestions.is_empty());
    // The universal contract-JSON row, and no row of another mechanism.
    assert!(
        a.caveats
            .iter()
            .any(|c| c.language == "*" && c.edge_category == "DOCUMENTS" && c.edges_found == 0),
        "{:?}",
        a.caveats
    );
    assert!(
        a.caveats
            .iter()
            .all(|c| c.edge_category == "DOCUMENTS" || c.edge_category == "*")
    );

    // Resolution goes through find: the dotted path is the same symbol, so the
    // answer is the same `no_edges`, not an unknown symbol.
    let dotted = governing_docs(&r.merged, "app.helper", None);
    assert_eq!(absence_of(&dotted).reason, "no_edges");
    assert_eq!(absence_of(&dotted).note, a.note);
}

#[test]
fn a_signal_that_resolves_to_nothing_is_no_signal_match() {
    let (_td, r) = build();
    let res = resolve_signal_located(&r.merged, "zzz.py", "diff", None, None);
    let a = absence_of(&res);
    assert_eq!((a.tier, a.reason), ("FACT", "no_signal_match"), "{a:?}");
    assert!(a.mechanisms.is_empty(), "resolution follows no edge");
    assert!(a.caveats.is_empty());
    assert_eq!(
        a.note,
        "the diff signal held 1 path; none resolved to a node in this graph"
    );

    let frame = "Traceback:\n  File \"zzz.py\", line 3, in f\n  File \"yyy.py\", line 9, in g\n";
    let res = resolve_signal_located(&r.merged, frame, "auto", None, None);
    assert_eq!(
        absence_of(&res).note,
        "the stacktrace signal (auto-detected) held 2 stack frames; none resolved to a node in this graph"
    );
    let hunk = "--- a/zzz.py\n+++ b/zzz.py\n@@ -1,1 +1,2 @@\n x = 1\n+y = 2\n";
    let res = resolve_signal_located(&r.merged, hunk, "diff", None, None);
    assert_eq!(
        absence_of(&res).note,
        "the diff signal held 1 added line; none resolved to a node in this graph"
    );
}

#[test]
fn find_with_no_candidate_is_no_match() {
    let (_td, r) = build();
    let found = find_nodes(&r.merged, "qqqq", &FindOptions::default());
    let a = absence_of(&found);
    assert_eq!((a.tier, a.reason), ("FACT", "no_match"), "{a:?}");
    assert!(a.suggestions.is_empty(), "the subsequence tier already ran");
    assert!(a.mechanisms.is_empty() && a.caveats.is_empty());
    assert_eq!(
        a.note,
        "no node matches `qqqq` by name or qname in any find tier"
    );
}

#[test]
fn a_found_answer_carries_no_absence() {
    let (_td, r) = build();
    let res = resolve_signal_located(&r.merged, "app.py", "diff", None, None);
    assert_eq!(res.results.len(), 3, "app, app::helper, app::main");
    assert!(res.absence.is_none());
    let found = find_nodes(&r.merged, "helper", &FindOptions::default());
    assert!(!found.results.is_empty() && found.absence.is_none());
    // Serialised, the envelope is `{results, absence: null}`.
    let v = serde_json::to_value(&res).expect("serialises");
    assert!(v["absence"].is_null() && v["results"].as_array().is_some_and(|r| r.len() == 3));
}

#[test]
fn a_scope_that_empties_the_answer_is_no_match_naming_the_scope() {
    let (_td, r) = build();
    let res = resolve_signal_located(&r.merged, "app.py", "diff", None, Some("nowhere"));
    let a = absence_of(&res);
    assert_eq!((a.tier, a.reason), ("FACT", "no_match"), "{a:?}");
    assert_eq!(a.note, "3 results outside scope `nowhere`");

    let mut opts = FindOptions::default();
    opts.scope = Some("nowhere".to_string());
    let found = find_nodes(&r.merged, "helper", &opts);
    let a = absence_of(&found);
    assert_eq!(a.reason, "no_match");
    assert!(a.note.ends_with("outside scope `nowhere`"), "{}", a.note);

    // top_k 0 empties a resolved answer too, and says so.
    let res = resolve_signal_located(&r.merged, "app.py", "diff", Some(0), None);
    assert_eq!(
        absence_of(&res).note,
        "top_k 0 kept none of the 3 resolved nodes"
    );
}

#[test]
fn the_absence_serialises_every_field() {
    let (_td, r) = build();
    let docs = governing_docs(&r.merged, "app::helper", None);
    let v = serde_json::to_value(&docs).expect("serialises");
    assert_eq!(v["results"], serde_json::json!([]));
    let a = &v["absence"];
    for key in [
        "tier",
        "reason",
        "query",
        "note",
        "mechanisms",
        "caveats",
        "suggestions",
        "nodes_searched",
        "unparsed_files",
    ] {
        assert!(a.get(key).is_some(), "absence lacks `{key}`: {a}");
    }
    assert_eq!(a["tier"], "FACT");
    let row = &a["caveats"][0];
    for key in ["language", "edge_category", "note", "verify", "edges_found"] {
        assert!(row.get(key).is_some(), "caveat lacks `{key}`: {row}");
    }
}
