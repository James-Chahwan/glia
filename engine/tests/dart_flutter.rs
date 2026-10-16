//! DT.5e — Dart / Flutter widget trees become edges, and widget / State
//! classes are framework components.
//!
//! Built from `bench/substrate-gap/fixtures/dart-flutter-widgets` (copied
//! into a tempdir): a Riverpod ConsumerStatefulWidget / ConsumerState pair,
//! StatelessWidgets built inside a `builder:` closure and with `const` /
//! `new`, callbacks passed by name, and a ChangeNotifier. The Dart parser
//! credits a closure's calls to its member, reads `const X(..)` / `new X(..)`
//! as a call of X, makes a method or function passed by name a USES (rule
//! `method_ref`), and tags each widget / State subclass in a Flutter file
//! with a COMPONENT overlay that graph::roles folds into ROLE COMPONENT on
//! its CLASS. The code domain's `flutter` INVOKED_MEMBERS row then makes a
//! live component's lifecycle members live.
//!
//! At HEAD 5dfe4c72 the fixture had CALLS 1/4 (only `main -> App`), USES
//! 0/2, ROLE 0/4 and 20 dead_symbol rows (19 once DT.5d withheld
//! `App::App`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::{cell_type, edge_category, evidence::Evidence, node_kind};
use glia_core::{EdgeCategoryId, NodeId};
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, gaps_report};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const CARD: &str = "lib::features::card";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures/dart-flutter-widgets")
}

/// Copy a fixture's sources (never its key.json or a stray `.ai/` / `.glia/`).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for e in std::fs::read_dir(from).expect("read fixture dir").flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            if e.file_name() != ".ai" && e.file_name() != ".glia" {
                copy_tree(&src, &dst);
            }
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).expect("copy");
        }
    }
}

fn build_in(dir: &Path) -> MergedGraph {
    copy_tree(&fixture_dir(), dir);
    generate_one(&dir.display().to_string()).expect("fixture builds").merged
}

fn build() -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build_in(tmp.path());
    (tmp, m)
}

/// The one node whose qname is exactly `qname`.
fn node(m: &MergedGraph, qname: &str) -> NodeId {
    let ids = m.qnames_exact(qname);
    assert_eq!(ids.len(), 1, "one node `{qname}`: {ids:?}");
    ids[0]
}

/// Every edge of `category` as `(from qname, to qname)`, with its evidence.
fn edges(m: &MergedGraph, category: EdgeCategoryId) -> Vec<(String, String, Option<Evidence>)> {
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    for g in &m.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
    }
    let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
    let mut out: Vec<(String, String, Option<Evidence>)> = m
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| (name(&e.from), name(&e.to), Evidence::read(&e.cells)))
        .collect();
    out.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    out
}

fn has(edges: &[(String, String, Option<Evidence>)], from: &str, to: &str) -> bool {
    edges.iter().any(|(f, t, _)| f == from && t == to)
}

/// The roles of the node `qname`, through the one reader.
fn roles(m: &MergedGraph, qname: &str) -> Vec<glia_core::NodeKindId> {
    let id = node(m, qname);
    for g in &m.graphs {
        if let Some(n) = g.nodes.iter().find(|n| n.id == id) {
            return glia_graph::roles::roles_in(g.nav.kind_by_id.get(&id).copied(), &n.cells);
        }
    }
    Vec::new()
}

#[test]
fn closures_creations_and_names_passed_by_value_become_edges() {
    let (_tmp, m) = build();
    let calls = edges(&m, edge_category::CALLS);
    for (from, to, why) in [
        ("lib::main::App::build", format!("{CARD}::CardPage"), "inside the Builder closure"),
        (&format!("{CARD}::_CardPageState::build"), format!("{CARD}::_Dot"), "const _Dot()"),
        (&format!("{CARD}::_CardPageState::build"), format!("{CARD}::_Label"), "new _Label(..)"),
        ("lib::main::main", "lib::main::App".to_string(), "the HEAD control"),
    ] {
        assert!(has(&calls, from, &to), "{why}: CALLS {from} -> {to}; CALLS = {calls:?}");
    }
    let uses = edges(&m, edge_category::USES);
    for (from, to) in [
        ("lib::main::App::build".to_string(), "lib::main::_noop".to_string()),
        (format!("{CARD}::_CardPageState::build"), format!("{CARD}::_CardPageState::_save")),
    ] {
        let ev = uses
            .iter()
            .find(|(f, t, _)| *f == from && *t == to)
            .and_then(|(_, _, ev)| ev.clone())
            .unwrap_or_else(|| panic!("USES {from} -> {to} with evidence; USES = {uses:?}"));
        assert_eq!(ev.emitter, "parser:dart");
        assert_eq!(ev.rule.as_deref(), Some("method_ref"));
    }
    // The fixture's forbid rows: `onTap: onTap` is the local `final onTap`,
    // and no lifecycle hop is stored as an edge.
    assert!(!has(&uses, &format!("{CARD}::_CardPageState::build"), &format!("{CARD}::_CardPageState::onTap")));
    assert!(!has(&calls, &format!("{CARD}::CartModel"), &format!("{CARD}::CartModel::dispose")));
    assert_eq!(uses.len(), 2, "the two names passed by value: {uses:?}");
}

#[test]
fn widget_and_state_classes_carry_role_component() {
    let (_tmp, m) = build();
    for q in [
        "lib::main::App".to_string(),
        format!("{CARD}::CardPage"),
        format!("{CARD}::_CardPageState"),
        format!("{CARD}::_Dot"),
        format!("{CARD}::_Label"),
    ] {
        assert_eq!(roles(&m, &q), [node_kind::COMPONENT], "{q}");
        let id = node(&m, &q);
        let kind = m.graphs.iter().find_map(|g| g.nav.kind_by_id.get(&id).copied());
        assert_eq!(kind, Some(node_kind::CLASS), "{q} keeps its kind");
    }
    assert!(roles(&m, &format!("{CARD}::CartModel")).is_empty(), "a ChangeNotifier is no component");
    // Every overlay folded into its CLASS: no COMPONENT node is left.
    for g in &m.graphs {
        assert!(!g.nav.kind_by_id.values().any(|k| *k == node_kind::COMPONENT));
        for n in &g.nodes {
            let role_cells = n.cells.iter().filter(|c| c.kind == cell_type::ROLE).count();
            assert!(role_cells <= 1, "one ROLE cell per node");
        }
    }
}

#[test]
fn dead_symbol_is_exactly_the_change_notifier_and_the_shadowed_method() {
    let (_tmp, m) = build();
    let rep = gaps_report(&m, &[], &GapsOptions::default()).expect("known categories");
    let mut rows: Vec<String> =
        rep.rows.iter().filter(|r| r.category == DEAD_SYMBOL).map(|r| r.qname.clone()).collect();
    rows.sort();
    let mut want = vec![
        format!("{CARD}::CartModel"),
        format!("{CARD}::CartModel::clear"),
        format!("{CARD}::CartModel::dispose"),
        format!("{CARD}::_CardPageState::onTap"),
    ];
    want.sort();
    assert_eq!(rows, want, "4 of HEAD's 20 rows stay");
}

/// Set by [`markers_name_the_components_and_the_new_sites`] on its child run.
const CHILD_ENV: &str = "GLIA_DT5E_CHILD_DIR";

/// A no-op unless run as the child of
/// [`markers_name_the_components_and_the_new_sites`], whose stderr carries
/// the parser's markers.
#[test]
fn child_build_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        build_in(Path::new(&dir));
    }
}

#[test]
fn markers_name_the_components_and_the_new_sites() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_build_for_stderr", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, tmp.path())
        .env("GLIA_THREADS", "1")
        .output()
        .expect("re-run the test binary");
    assert!(out.status.success(), "child failed: {}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines = |prefix: &str| -> Vec<String> {
        let mut v: Vec<String> =
            stderr.lines().filter(|l| l.starts_with(prefix)).map(String::from).collect();
        v.sort();
        v
    };
    assert_eq!(
        lines("[dart-flutter]"),
        [
            "[dart-flutter] components=1 (widgets=1 states=0) file=lib/main.dart",
            "[dart-flutter] components=4 (widgets=3 states=1) file=lib/features/card.dart",
        ]
    );
    let calls = lines("[dart-calls]");
    assert_eq!(calls.len(), 2, "{calls:?}");
    let card = calls.iter().find(|l| l.ends_with("file=lib/features/card.dart")).expect("card");
    assert!(card.contains(" closures=1 creations=4 method_refs=1 "), "{card}");
    let main = calls.iter().find(|l| l.ends_with("file=lib/main.dart")).expect("main");
    assert!(main.contains(" closures=1 creations=1 method_refs=1 "), "{main}");
}
