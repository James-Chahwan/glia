//! LF.5b: a `.glia/history-snapshot/` becomes churn ATTN on MODULEs, blame
//! ATTN on symbols and Weak CO_CHANGES edges between modules that change
//! together, and none of it leaks into blast radius, liveness or activation.
//!
//! Snapshots are written with `code_domain::snapshots::write_history` (no git
//! needed), into temp trees built from the substrate-gap fixture
//! `history-cochange`; `fixture_graph_matches_the_key` builds the committed
//! fixture itself, whose snapshot is hand-written and hash-checked.

use std::path::Path;

use repo_graph_code_domain::evidence::{Basis, Evidence};
use repo_graph_code_domain::profile::CODE_TABLES;
use repo_graph_code_domain::snapshots::{BlameFile, HistoryCommit, HistoryFile, HistoryMeta, write_history};
use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, Confidence, Edge, NodeId};
use repo_graph_engine::{
    BlastOptions, BuildOptions, blast_radius, entrypoint_reachable, generate_one, generate_one_opts,
};
use repo_graph_graph::MergedGraph;
use repo_graph_store::write_merged_sharded;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/history-cochange";
const A_PY: &str = include_str!("../../bench/substrate-gap/fixtures/history-cochange/svc/a.py");
const B_PY: &str = include_str!("../../bench/substrate-gap/fixtures/history-cochange/svc/b.py");
const C_PY: &str = include_str!("../../bench/substrate-gap/fixtures/history-cochange/svc/c.py");

const T1: i64 = 1_767_225_600;
const T2: i64 = 1_767_312_000;

fn file(p: &str, a: u32) -> HistoryFile {
    HistoryFile { p: p.to_string(), a: Some(a), d: Some(0), from: None }
}

fn renamed(p: &str, from: &str, a: u32) -> HistoryFile {
    HistoryFile { from: Some(from.to_string()), ..file(p, a) }
}

fn commit(n: usize, t: i64, files: Vec<HistoryFile>) -> HistoryCommit {
    HistoryCommit { c: format!("{n:02}{}", "0".repeat(38)), t, files }
}

/// The fixture's history, newest first: c alone; a+b three times; all three
/// at init.
fn fixture_commits() -> Vec<HistoryCommit> {
    let mut out = vec![commit(5, T1 + 4 * 86_400, vec![file("svc/c.py", 1)])];
    for i in (2..=4).rev() {
        out.push(commit(i, T1 + (i as i64 - 1) * 86_400, vec![file("svc/a.py", 1), file("svc/b.py", 1)]));
    }
    out.push(commit(1, T1, vec![file("svc/a.py", 2), file("svc/b.py", 2), file("svc/c.py", 2)]));
    out
}

/// `<tmp>/svc/{a,b,c}.py` with the fixture sources, plus `extra` files.
fn tree(extra: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(d.path().join("svc")).unwrap();
    for (p, text) in [("svc/a.py", A_PY), ("svc/b.py", B_PY), ("svc/c.py", C_PY)].iter().chain(extra) {
        std::fs::write(d.path().join(p), text).unwrap();
    }
    d
}

fn snapshot(root: &Path, commits: &[HistoryCommit], blame: &[BlameFile]) {
    let head = commits.first().map_or_else(|| "0".repeat(40), |c| c.c.clone());
    write_history(root, HistoryMeta::new(head, 2000, None, String::new()), commits, blame).expect("write snapshot");
}

fn drop_snapshot(root: &Path) {
    std::fs::remove_dir_all(root.join(".glia/history-snapshot")).expect("remove snapshot");
}

fn build(root: &Path) -> MergedGraph {
    generate_one(&root.to_string_lossy()).expect("build").merged
}

fn id_of(m: &MergedGraph, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.iter())
        .filter(|(_, q)| q.as_str() == qname)
        .map(|(id, _)| *id)
        .min_by_key(|id| id.0)
        .unwrap_or_else(|| panic!("no node {qname}"))
}

fn qname(m: &MergedGraph, id: NodeId) -> String {
    m.graphs.iter().find_map(|g| g.nav.qname_by_id.get(&id).cloned()).unwrap_or_default()
}

/// The ATTN cell payloads of `qname`, parsed.
fn attn(m: &MergedGraph, qname: &str) -> Vec<serde_json::Value> {
    let id = id_of(m, qname);
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == cell_type::ATTN)
        .map(|c| match &c.payload {
            CellPayload::Json(s) => serde_json::from_str(s).expect("ATTN is JSON"),
            other => panic!("ATTN payload {other:?}"),
        })
        .collect()
}

fn one_attn(m: &MergedGraph, qname: &str) -> serde_json::Value {
    let mut cells = attn(m, qname);
    assert_eq!(cells.len(), 1, "{qname}: {cells:?}");
    cells.remove(0)
}

fn cochanges(m: &MergedGraph) -> Vec<&Edge> {
    m.all_edges().filter(|e| e.category == edge_category::CO_CHANGES).collect()
}

fn cochange_pairs(m: &MergedGraph) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = cochanges(m).iter().map(|e| (qname(m, e.from), qname(m, e.to))).collect();
    out.sort();
    out
}

/// The committed fixture: its hand-written meta hashes to its data, and the
/// build carries exactly what `key.json` expects (and none of what it forbids).
#[test]
fn fixture_graph_matches_the_key() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let m = build(&root);
    let a = one_attn(&m, "svc::a");
    assert_eq!(a["source"], "git");
    assert_eq!((a["commits"].as_u64(), a["lines_added"].as_u64()), (Some(4), Some(5)));
    assert_eq!(a["head"], "c50000000000");
    assert_eq!(one_attn(&m, "svc::c")["commits"], 2);
    assert_eq!(one_attn(&m, "svc::a::a")["source"], "git-blame");
    assert_eq!(cochange_pairs(&m), [("svc::a".to_string(), "svc::b".to_string())]);
}

/// a.py is in 4 commits (1+1+1+2 lines), c.py in 2; every payload is compact
/// JSON in its documented field order, integers only.
#[test]
fn module_attn_counts() {
    let d = tree(&[]);
    snapshot(d.path(), &fixture_commits(), &[]);
    let m = build(d.path());
    let raw = |q: &str| {
        let id = id_of(&m, q);
        m.graphs
            .iter()
            .flat_map(|g| g.nodes.iter())
            .find(|n| n.id == id)
            .and_then(|n| n.cells.iter().find(|c| c.kind == cell_type::ATTN))
            .map(|c| match &c.payload {
                CellPayload::Json(s) => s.clone(),
                other => panic!("{other:?}"),
            })
            .expect("ATTN")
    };
    assert_eq!(
        raw("svc::a"),
        format!(
            r#"{{"source":"git","commits":4,"lines_added":5,"lines_deleted":0,"first":{T1},"last":{},"window_commits":5,"head":"050000000000"}}"#,
            T1 + 3 * 86_400
        )
    );
    let c = one_attn(&m, "svc::c");
    assert_eq!((c["commits"].as_u64(), c["lines_added"].as_u64()), (Some(2), Some(3)));
    assert_eq!((c["first"].as_i64(), c["last"].as_i64()), (Some(T1), Some(T1 + 4 * 86_400)));
    // No blame rows: no symbol ATTN.
    assert!(attn(&m, "svc::a::a").is_empty());
}

/// c.py -> c2.py at the newest commit: c2's churn counts every commit c.py
/// had before the rename, and the old path is not unmapped noise.
#[test]
fn rename_accrues_to_current_path() {
    let d = tree(&[("svc/c2.py", C_PY)]);
    std::fs::remove_file(d.path().join("svc/c.py")).unwrap();
    let commits = vec![
        commit(3, T1 + 2 * 86_400, vec![renamed("svc/c2.py", "svc/c.py", 0)]),
        commit(2, T1 + 86_400, vec![file("svc/c.py", 1)]),
        commit(1, T1, vec![file("svc/c.py", 2), file("svc/a.py", 2)]),
    ];
    snapshot(d.path(), &commits, &[]);
    let m = build(d.path());
    let c2 = one_attn(&m, "svc::c2");
    assert_eq!((c2["commits"].as_u64(), c2["lines_added"].as_u64()), (Some(3), Some(3)));
    assert_eq!((c2["first"].as_i64(), c2["last"].as_i64()), (Some(T1), Some(T1 + 2 * 86_400)));
    assert_eq!(one_attn(&m, "svc::a")["commits"], 1);
}

/// Blame is 1-based, POSITION 0-based: f()'s lines are all T1, g()'s are T1
/// (its def line) and T2 (its body).
#[test]
fn blame_symbol_attn() {
    let fg = "def f():\n    return 1\n\n\ndef g():\n    return 2\n";
    let d = tree(&[("svc/fg.py", fg)]);
    let commits = vec![commit(2, T2, vec![file("svc/fg.py", 1)]), commit(1, T1, vec![file("svc/fg.py", 6)])];
    let blame = vec![BlameFile { p: "svc/fg.py".into(), runs: vec![[1, 5, T1], [6, 6, T2]] }];
    snapshot(d.path(), &commits, &blame);
    let m = build(d.path());
    let f = one_attn(&m, "svc::fg::f");
    assert_eq!(f["source"], "git-blame");
    assert_eq!((f["span_changes"].as_u64(), f["last"].as_i64()), (Some(1), Some(T1)));
    let g = one_attn(&m, "svc::fg::g");
    assert_eq!((g["span_changes"].as_u64(), g["last"].as_i64()), (Some(2), Some(T2)));
    assert_eq!(g["head"], "020000000000");
    // A file blame does not cover gives its symbols nothing.
    assert!(attn(&m, "svc::a::a").is_empty());

    // The same history without blame rows: modules keep churn, symbols get none.
    snapshot(d.path(), &commits, &[]);
    let m = build(d.path());
    assert!(attn(&m, "svc::fg::f").is_empty() && attn(&m, "svc::fg::g").is_empty());
    assert_eq!(one_attn(&m, "svc::fg")["commits"], 2);
}

/// a and b co-change 4 times (ratio 1000): exactly one Weak CO_CHANGES a -> b
/// with its ATTN and evidence; the one-off a+c / b+c pairs get none. The edge
/// is a fact input, so `--no-overlay` keeps it.
#[test]
fn cochange_edge_above_support() {
    let d = tree(&[]);
    snapshot(d.path(), &fixture_commits(), &[]);
    let m = build(d.path());
    let edges = cochanges(&m);
    assert_eq!(edges.len(), 1, "{:?}", cochange_pairs(&m));
    let e = edges[0];
    assert_eq!((qname(&m, e.from).as_str(), qname(&m, e.to).as_str()), ("svc::a", "svc::b"));
    assert_eq!(e.confidence, Confidence::Weak);
    let cell = e.cells.iter().find(|c| c.kind == cell_type::ATTN).expect("edge ATTN");
    assert_eq!(cell.payload, CellPayload::Json(r#"{"cochanges":4,"ratio_permille":1000,"window_commits":5}"#.into()));
    let ev = Evidence::of(e).expect("evidence");
    assert_eq!(ev.emitter, "history:cochange");
    assert_eq!(ev.file.as_deref(), Some(".glia/history-snapshot/meta.json"));
    assert_eq!(ev.basis, Basis::File);

    let no_overlay = generate_one_opts(&d.path().to_string_lossy(), false, &BuildOptions::default().with_overlay(false))
        .expect("build")
        .merged;
    assert_eq!(cochange_pairs(&no_overlay), [("svc::a".to_string(), "svc::b".to_string())]);
}

/// a + c once, a + b twice: below support (3), no edge at all.
#[test]
fn below_support_no_edge() {
    let d = tree(&[]);
    let commits = vec![
        commit(3, T1 + 2, vec![file("svc/a.py", 1), file("svc/b.py", 1)]),
        commit(2, T1 + 1, vec![file("svc/a.py", 1), file("svc/b.py", 1)]),
        commit(1, T1, vec![file("svc/a.py", 1), file("svc/c.py", 1)]),
    ];
    snapshot(d.path(), &commits, &[]);
    let m = build(d.path());
    assert!(cochanges(&m).is_empty(), "{:?}", cochange_pairs(&m));
    assert_eq!(one_attn(&m, "svc::a")["commits"], 3);
}

/// Three commits over 31 modules add no pair (a mass commit is noise); the
/// same three commits over 30 modules pair all 435 of them.
#[test]
fn mass_commit_ignored() {
    let names: Vec<String> = (0..31).map(|i| format!("svc/m{i:02}.py")).collect();
    let body = "def f():\n    return 0\n";
    let extra: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), body)).collect();
    let d = tree(&extra);
    let commits_over = |files: &[String]| -> Vec<HistoryCommit> {
        (1..=3).rev().map(|i| commit(i, T1 + i as i64, files.iter().map(|p| file(p, 1)).collect())).collect()
    };
    snapshot(d.path(), &commits_over(&names), &[]);
    let m = build(d.path());
    assert!(cochanges(&m).is_empty(), "a 31-file commit paired {} modules", cochanges(&m).len());
    assert_eq!(one_attn(&m, "svc::m30")["commits"], 3, "a mass commit still counts as churn");

    snapshot(d.path(), &commits_over(&names[..30]), &[]);
    let m = build(d.path());
    assert_eq!(cochanges(&m).len(), 30 * 29 / 2);
}

/// CO_CHANGES is heuristic: blast radius never follows it, liveness is the
/// graph's without the snapshot, and activation scores are identical with and
/// without it under every preset (its weight is 0).
#[test]
fn cochange_is_not_carried() {
    // main() calls a(), which calls b(): a real carry path next to the
    // co-change a <-> c that must not become one.
    let a = "from svc.b import b\n\n\ndef a():\n    return b()\n";
    let main = "from svc.a import a\n\n\ndef main():\n    return a()\n";
    let d = tree(&[("svc/main.py", main)]);
    std::fs::write(d.path().join("svc/a.py"), a).unwrap();
    let commits: Vec<HistoryCommit> =
        (1..=4).rev().map(|i| commit(i, T1 + i as i64, vec![file("svc/a.py", 1), file("svc/c.py", 1)])).collect();
    snapshot(d.path(), &commits, &[]);
    let with = build(d.path());
    assert_eq!(cochange_pairs(&with), [("svc::a".to_string(), "svc::c".to_string())]);
    drop_snapshot(d.path());
    let without = build(d.path());
    assert!(cochanges(&without).is_empty());

    for q in ["svc::a", "svc::a::a", "svc::c"] {
        let radius = |m: &MergedGraph| -> Vec<(String, &'static str)> {
            let answer = blast_radius(m, &[q], &BlastOptions::default());
            assert!(answer.unresolved.is_empty(), "{q} resolves");
            let mut rows: Vec<(String, &'static str)> =
                answer.results.into_iter().map(|r| (r.qname, r.reason)).collect();
            rows.sort();
            rows
        };
        let got = radius(&with);
        assert!(got.iter().all(|(_, why)| *why != "CO_CHANGES"), "{q}: {got:?}");
        assert_eq!(got, radius(&without), "{q}: blast radius moved");
    }

    let mut live_with: Vec<u64> = entrypoint_reachable(&with).into_iter().map(|id| id.0).collect();
    let mut live_without: Vec<u64> = entrypoint_reachable(&without).into_iter().map(|id| id.0).collect();
    live_with.sort_unstable();
    live_without.sort_unstable();
    assert_eq!(live_with, live_without, "liveness moved");

    let seeds = [id_of(&with, "svc::a"), id_of(&with, "svc::c")];
    for preset in [None, Some("repair"), Some("review"), Some("onboard")] {
        let config = CODE_TABLES.activation_config(preset);
        let scores = |m: &MergedGraph| -> Vec<(u64, u64)> {
            m.activate(&seeds, &config).scores.iter().map(|(id, s)| (id.0, s.to_bits())).collect()
        };
        assert_eq!(scores(&with), scores(&without), "preset {preset:?}: activation moved");
    }
}

/// Two builds of one tree with a snapshot write the same bytes.
#[test]
fn history_builds_are_byte_identical() {
    let d = tree(&[]);
    let blame = vec![BlameFile { p: "svc/a.py".into(), runs: vec![[1, 2, T1]] }];
    snapshot(d.path(), &fixture_commits(), &blame);
    let out = tempfile::tempdir().expect("tempdir");
    let bytes = |tag: &str| -> Vec<(String, Vec<u8>)> {
        let m = build(d.path());
        assert_eq!(cochanges(&m).len(), 1);
        let dir = out.path().join(tag);
        write_merged_sharded(&m, &dir).expect("write layout");
        let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
            .collect();
        files.sort();
        files
    };
    let (first, second) = (bytes("a"), bytes("b"));
    assert_eq!(first.len(), second.len());
    for ((name, x), (_, y)) in first.iter().zip(&second) {
        assert_eq!(x, y, "{name} bytes differ");
    }
    // The snapshot is in the graph at all: the module kind of a is intact.
    let m = build(d.path());
    let a = id_of(&m, "svc::a");
    assert!(m.graphs.iter().any(|g| g.nav.kind_by_id.get(&a) == Some(&node_kind::MODULE)));
}
