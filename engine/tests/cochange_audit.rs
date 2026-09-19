//! LF.5c: the co-change-without-edge audit. A CO_CHANGES edge (LF.5b, from a
//! `.glia/history-snapshot/`) says two files change together; the audit
//! reports the pairs the static graph cannot explain: no edge of any category
//! but CO_CHANGES / DEFINES / CONTAINS joins a node of one file to a node of
//! the other, directly or through at most two file-less nodes (three hops).
//!
//! Snapshots are written with `code_domain::snapshots::write_history` (no git
//! needed) into temp trees; the sources of probe g1 are the committed
//! substrate-gap fixture `history-cochange` (svc/a.py and svc/b.py co-changed
//! 4 times, no edge of any kind between them).

use std::collections::HashSet;
use std::path::Path;

use repo_graph_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};
use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId};
use repo_graph_engine::gaps::{
    COCHANGE_NO_EDGE, CochangeGap, GapsOptions, HEURISTIC, cochange_gaps, gaps_report,
};
use repo_graph_engine::generate_one;
use repo_graph_graph::MergedGraph;

const A_PY: &str = include_str!("../../bench/substrate-gap/fixtures/history-cochange/svc/a.py");
const B_PY: &str = include_str!("../../bench/substrate-gap/fixtures/history-cochange/svc/b.py");
const C_PY: &str = include_str!("../../bench/substrate-gap/fixtures/history-cochange/svc/c.py");

/// b.py calls a.a(): a direct CALLS (and IMPORTS) link.
const B_CALLS_A: &str = "from svc.a import a\n\n\ndef b():\n    return a()\n";
/// a -> c -> b through a third file: c.py is not file-less, so no bridge.
const A_CALLS_C: &str = "from svc.c import c\n\n\ndef a():\n    return c()\n";
const C_CALLS_B: &str = "from svc.b import b\n\n\ndef c():\n    return b()\n";

const CLIENT_TS: &str = "export async function loadUsers() {\n  return fetch('/users');\n}\n";
const API_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\", methods=[\"GET\"])\ndef list_users():\n    return []\n";
/// The same API serving a path the client never calls: the HTTP pair breaks.
const API_ORDERS_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/orders\", methods=[\"GET\"])\ndef list_orders():\n    return []\n";

const T0: i64 = 1_767_225_600;

/// Commits, newest first: `n` commits per group touching every file of it.
fn commits(groups: &[(&[&str], usize)]) -> Vec<HistoryCommit> {
    let mut out = Vec::new();
    let mut i = 0usize;
    for (files, n) in groups {
        for _ in 0..*n {
            i += 1;
            out.push(HistoryCommit {
                c: format!("{i:02}{}", "0".repeat(38)),
                t: T0 + i64::try_from(i).expect("small") * 86_400,
                files: files
                    .iter()
                    .map(|p| HistoryFile {
                        p: (*p).to_string(),
                        a: Some(1),
                        d: Some(0),
                        from: None,
                    })
                    .collect(),
            });
        }
    }
    out.reverse();
    out
}

/// Probe g1's history: a+b three times, then all three at init (a+b: 4).
fn g1_commits() -> Vec<HistoryCommit> {
    let mut c = commits(&[(&["svc/a.py", "svc/b.py"], 3)]);
    c.extend(commits(&[(&["svc/a.py", "svc/b.py", "svc/c.py"], 1)]));
    c
}

/// A temp tree holding `files` and a history snapshot of `history`.
fn tree(files: &[(&str, &str)], history: &[HistoryCommit]) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    for (p, text) in files {
        let path = d.path().join(p);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    let head = history
        .first()
        .map_or_else(|| "0".repeat(40), |c| c.c.clone());
    write_history(
        d.path(),
        HistoryMeta::new(head, 2000, None, String::new()),
        history,
        &[],
    )
    .expect("write snapshot");
    d
}

fn build(root: &Path) -> MergedGraph {
    generate_one(&root.to_string_lossy()).expect("build").merged
}

fn g1_tree(b_py: &str, a_py: &str, c_py: &str) -> tempfile::TempDir {
    tree(
        &[("svc/a.py", a_py), ("svc/b.py", b_py), ("svc/c.py", c_py)],
        &g1_commits(),
    )
}

/// `(file_a, file_b, cochanges)` per gap, in answer order.
fn pairs(gaps: &[CochangeGap]) -> Vec<(String, String, u32)> {
    gaps.iter()
        .map(|g| (g.file_a.clone(), g.file_b.clone(), g.cochanges))
        .collect()
}

fn cochange_edges(m: &MergedGraph) -> usize {
    m.all_edges()
        .filter(|e| e.category == edge_category::CO_CHANGES)
        .count()
}

/// The first POSITION file of every node, by id.
fn file_of(m: &MergedGraph, id: NodeId) -> Option<String> {
    let n = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .find(|n| n.id == id)?;
    n.cells
        .iter()
        .filter(|c| c.kind == cell_type::POSITION)
        .find_map(|c| match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) => {
                serde_json::from_str::<serde_json::Value>(s)
                    .ok()?
                    .get("file")?
                    .as_str()
                    .map(str::to_string)
            }
            _ => None,
        })
}

/// Is there one edge (not CO_CHANGES / DEFINES / CONTAINS) whose two ends sit
/// in `a` and `b` (either way round)? Computed here, independently of the
/// engine, to tell a direct link from a bridged one.
fn direct_edge(m: &MergedGraph, a: &str, b: &str) -> bool {
    let skip = [
        edge_category::CO_CHANGES,
        edge_category::DEFINES,
        edge_category::CONTAINS,
    ];
    m.all_edges().filter(|e| !skip.contains(&e.category)).any(|e| {
        let (f, t) = (file_of(m, e.from), file_of(m, e.to));
        matches!((f.as_deref(), t.as_deref()), (Some(x), Some(y)) if (x == a && y == b) || (x == b && y == a))
    })
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

/// Probe g1: svc/a.py and svc/b.py co-changed 4 times with no edge of any
/// category between them — the audit's one row.
#[test]
fn unlinked_pair_is_a_gap() {
    let d = g1_tree(B_PY, A_PY, C_PY);
    let m = build(d.path());
    assert_eq!(cochange_edges(&m), 1, "LF.5b emits the a+b pair");
    let gaps = cochange_gaps(&m, None);
    assert_eq!(
        pairs(&gaps),
        [("svc/a.py".to_string(), "svc/b.py".to_string(), 4)]
    );
    let g = &gaps[0];
    assert_eq!(g.ratio_permille, 1000);
    assert_eq!(
        (g.language_a, g.language_b, g.cross_language),
        (Some("python"), Some("python"), false)
    );
    assert_eq!(g.tier, HEURISTIC);
    assert!(cochange_gaps(&m, Some(0)).is_empty(), "top_k cuts");
}

/// b.py calls a.a(): the pair shares a direct static link, so no gap.
#[test]
fn direct_call_is_linked() {
    let d = g1_tree(B_CALLS_A, A_PY, C_PY);
    let m = build(d.path());
    assert_eq!(cochange_edges(&m), 1);
    assert!(
        direct_edge(&m, "svc/a.py", "svc/b.py"),
        "b.py -> a.py edge extracted"
    );
    assert!(cochange_gaps(&m, None).is_empty());
}

/// a -> c -> b: the path runs through a third FILE, which is no bridge (only
/// file-less nodes are), so the a+b pair is still a gap.
#[test]
fn third_file_is_no_bridge() {
    let d = g1_tree(B_PY, A_CALLS_C, C_CALLS_B);
    let m = build(d.path());
    let a = id_of(&m, "svc::a::a");
    let c = id_of(&m, "svc::c::c");
    assert!(
        m.all_edges()
            .any(|e| e.from == a && e.to == c && e.category == edge_category::CALLS)
    );
    assert_eq!(
        pairs(&cochange_gaps(&m, None)),
        [("svc/a.py".to_string(), "svc/b.py".to_string(), 4)]
    );
}

/// web/client.ts fetches `/users`, api/app.py serves it: the pair is linked
/// through two file-less nodes (loadUsers -> ENDPOINT -> ROUTE -> list_users),
/// with no direct edge between the files. The control (the API serves
/// `/orders` instead) breaks the HTTP pair and the same history is a gap.
#[test]
fn http_bridge_is_linked() {
    let history = commits(&[(&["api/app.py", "web/client.ts"], 3)]);
    let d = tree(
        &[("web/client.ts", CLIENT_TS), ("api/app.py", API_PY)],
        &history,
    );
    let m = build(d.path());
    assert_eq!(cochange_edges(&m), 1);
    assert!(
        m.all_edges()
            .any(|e| e.category == edge_category::HTTP_CALLS),
        "the ENDPOINT pairs with the ROUTE"
    );
    assert!(
        !direct_edge(&m, "api/app.py", "web/client.ts"),
        "no direct edge: the link is a bridge"
    );
    assert!(cochange_gaps(&m, None).is_empty());

    let control = tree(
        &[("web/client.ts", CLIENT_TS), ("api/app.py", API_ORDERS_PY)],
        &history,
    );
    let m = build(control.path());
    assert!(
        !m.all_edges()
            .any(|e| e.category == edge_category::HTTP_CALLS)
    );
    let gaps = cochange_gaps(&m, None);
    assert_eq!(
        pairs(&gaps),
        [("api/app.py".to_string(), "web/client.ts".to_string(), 3)]
    );
    assert_eq!(
        (gaps[0].language_a, gaps[0].language_b),
        (Some("python"), Some("typescript"))
    );
    assert!(gaps[0].cross_language);
}

/// Splice a chain of `n` file-less nodes between svc::a::a and svc::b::b
/// (`n + 1` hops).
fn chain(m: &mut MergedGraph, n: usize) {
    let (a, b) = (id_of(m, "svc::a::a"), id_of(m, "svc::b::b"));
    let g = &mut m.graphs[0];
    let repo = g.repo;
    let mut prev = a;
    for i in 0..n {
        let id = NodeId(0xC0C0_0000 + u64::try_from(i).expect("small"));
        g.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        g.nav.kind_by_id.insert(id, node_kind::QUEUE_PRODUCER);
        g.nav.qname_by_id.insert(id, format!("hop{i}"));
        g.nav.name_by_id.insert(id, format!("hop{i}"));
        m.cross_edges.push(Edge::new(
            prev,
            id,
            edge_category::EVENT_FLOWS,
            Confidence::Weak,
        ));
        prev = id;
    }
    m.cross_edges.push(Edge::new(
        prev,
        b,
        edge_category::EVENT_FLOWS,
        Confidence::Weak,
    ));
}

/// Two file-less nodes (three hops) still link; three (four hops) do not.
#[test]
fn three_hop_limit() {
    let d = g1_tree(B_PY, A_PY, C_PY);
    let base = build(d.path());
    let gap = [("svc/a.py".to_string(), "svc/b.py".to_string(), 4)];
    assert_eq!(pairs(&cochange_gaps(&base, None)), gap);
    for (n, linked) in [(0, true), (1, true), (2, true), (3, false), (4, false)] {
        let mut m = build(d.path());
        chain(&mut m, n);
        let got = pairs(&cochange_gaps(&m, None));
        if linked {
            assert!(
                got.is_empty(),
                "{n} file-less node(s), {} hops: linked",
                n + 1
            );
        } else {
            assert_eq!(got, gap, "{n} file-less node(s), {} hops: a gap", n + 1);
        }
    }
    // A node WITH a file in the middle is no bridge, however short the path.
    let mut m = build(d.path());
    chain(&mut m, 1);
    let mid = NodeId(0xC0C0_0000);
    let pos = Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(r#"{"file":"svc/c.py","start_line":0,"end_line":1}"#.into()),
    };
    for n in m.graphs[0].nodes.iter_mut().filter(|n| n.id == mid) {
        n.cells.push(pos.clone());
    }
    assert_eq!(pairs(&cochange_gaps(&m, None)), gap);
}

/// A `cochange_no_edge` row's (file, qname, detail, kind, tier, suggest, line).
type RowKey = (
    Option<String>,
    String,
    String,
    &'static str,
    &'static str,
    &'static str,
    Option<i64>,
);

/// Four files: a+b co-change 5 times, c+d 3 times, e+f 3 times with e calling
/// f. The `cochange_no_edge` rows of gaps_report are exactly cochange_gaps:
/// one row per gap, on the first file's MODULE, naming the other file.
#[test]
fn gaps_category_matches() {
    let e_py = "from svc.f import f\n\n\ndef e():\n    return f()\n";
    let f_py = "def f():\n    return 6\n";
    let history = {
        let mut h = commits(&[(&["svc/a.py", "svc/b.py"], 5)]);
        h.extend(commits(&[(&["svc/c.py", "svc/d.py"], 3)]));
        h.extend(commits(&[(&["svc/e.py", "svc/f.py"], 3)]));
        h
    };
    let d = tree(
        &[
            ("svc/a.py", A_PY),
            ("svc/b.py", B_PY),
            ("svc/c.py", C_PY),
            ("svc/d.py", "def d():\n    return 4\n"),
            ("svc/e.py", e_py),
            ("svc/f.py", f_py),
        ],
        &history,
    );
    let m = build(d.path());
    assert_eq!(cochange_edges(&m), 3);
    let gaps = cochange_gaps(&m, None);
    assert_eq!(
        pairs(&gaps),
        [
            ("svc/a.py".to_string(), "svc/b.py".to_string(), 5),
            ("svc/c.py".to_string(), "svc/d.py".to_string(), 3),
        ],
        "sorted by cochanges desc; e+f linked"
    );
    assert_eq!(
        pairs(&cochange_gaps(&m, Some(1))),
        pairs(&gaps[..1]),
        "top_k keeps the strongest"
    );

    let mut opts = GapsOptions::default();
    opts.category = Some(COCHANGE_NO_EDGE.to_string());
    let rep = gaps_report(&m, &[], &opts).expect("known category");
    assert_eq!(rep.count(COCHANGE_NO_EDGE), gaps.len());
    let rows: HashSet<RowKey> = rep
        .rows
        .iter()
        .map(|r| {
            (
                r.file.clone(),
                r.qname.clone(),
                r.detail.clone(),
                r.kind,
                r.tier,
                r.suggest,
                r.line,
            )
        })
        .collect();
    let want: HashSet<RowKey> = gaps
        .iter()
        .map(|g| {
            let module = |f: &str| f.trim_end_matches(".py").replace('/', "::");
            (
                Some(g.file_a.clone()),
                module(&g.file_a),
                format!(
                    "with={} ({}); cochanges={}; ratio_permille={}; languages=python,python",
                    g.file_b,
                    module(&g.file_b),
                    g.cochanges,
                    g.ratio_permille
                ),
                "MODULE",
                HEURISTIC,
                "edge",
                Some(1),
            )
        })
        .collect();
    assert_eq!(rows, want);
    assert!(rep.rows.iter().all(|r| r.category == COCHANGE_NO_EDGE));

    // Every category at once: the co-change rows are the same rows.
    let all = gaps_report(&m, &[], &GapsOptions::default()).expect("report");
    let again: Vec<_> = all
        .rows
        .iter()
        .filter(|r| r.category == COCHANGE_NO_EDGE)
        .cloned()
        .collect();
    assert_eq!(again, rep.rows);
}

/// Two builds of one tree answer the same bytes.
#[test]
fn deterministic() {
    let history = {
        let mut h = commits(&[(&["svc/a.py", "svc/b.py"], 4)]);
        h.extend(commits(&[(&["svc/b.py", "svc/c.py"], 3)]));
        h.extend(commits(&[(&["svc/a.py", "svc/c.py"], 3)]));
        h
    };
    let d = tree(
        &[("svc/a.py", A_PY), ("svc/b.py", B_PY), ("svc/c.py", C_PY)],
        &history,
    );
    let run = || {
        let m = build(d.path());
        let rep = gaps_report(&m, &[], &GapsOptions::default()).expect("report");
        (
            serde_json::to_string(&cochange_gaps(&m, None)).expect("json"),
            serde_json::to_string(&rep).expect("json"),
        )
    };
    let first = run();
    assert_eq!(first, run());
    assert!(
        first
            .0
            .contains("\"file_a\":\"svc/a.py\",\"file_b\":\"svc/b.py\",\"cochanges\":4"),
        "{}",
        first.0
    );
}
