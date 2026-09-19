//! LC.3a: every edge carries one EVIDENCE cell naming the stage that emitted
//! it, and the fill pass locates it (file, 0-based line, basis) from its
//! endpoints when the emitter recorded no site.

use std::path::Path;

use repo_graph_code_domain::evidence::{self, Basis, Evidence, STAGES};
use repo_graph_code_domain::{cell_type, edge_category};
use repo_graph_core::{Edge, EdgeCategoryId, NodeId};
use repo_graph_engine::{generate_many, generate_one};
use repo_graph_graph::MergedGraph;
use repo_graph_store::write_merged_sharded;

/// The five-file shape: an intra-repo Python call through an import, a flask
/// route, a TS fetch on line 2 of its file, and a README mentioning `bar`.
fn write_fixture(dir: &Path) {
    std::fs::write(dir.join("a.py"), "def bar():\n    return 1\n").unwrap();
    std::fs::write(
        dir.join("b.py"),
        "from a import bar\n\n\ndef baz():\n    x = 1\n    return bar()\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("server.py"),
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
         @app.route(\"/users\")\ndef list_users():\n    return []\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("client.ts"),
        "export function load() {\n  return fetch(\"/users\");\n}\n",
    )
    .unwrap();
    std::fs::write(dir.join("README.md"), "# Guide\n\nCall `bar` to get one.\n").unwrap();
}

fn build() -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    write_fixture(&repo);
    let merged = generate_one(repo.to_str().unwrap()).unwrap().merged;
    (tmp, merged)
}

fn by_qname(m: &MergedGraph, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .find_map(|g| {
            g.nodes
                .iter()
                .find(|n| g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname))
                .map(|n| n.id)
        })
        .unwrap_or_else(|| panic!("no node {qname}"))
}

fn edge(m: &MergedGraph, from: NodeId, to: NodeId, cat: EdgeCategoryId) -> &Edge {
    m.all_edges()
        .find(|e| e.from == from && e.to == to && e.category == cat)
        .unwrap_or_else(|| panic!("no edge {from:?} -> {to:?} ({cat:?})"))
}

fn evidence_of(e: &Edge) -> Evidence {
    Evidence::of(e).unwrap_or_else(|| panic!("edge without evidence: {e:?}"))
}

#[test]
fn every_edge_has_evidence() {
    let (_tmp, m) = build();
    let mut n = 0usize;
    for e in m.all_edges() {
        n += 1;
        let cells = e
            .cells
            .iter()
            .filter(|c| c.kind == cell_type::EVIDENCE)
            .count();
        assert_eq!(cells, 1, "exactly one EVIDENCE cell: {e:?}");
        let ev = evidence_of(e);
        let stage = ev.emitter.split(':').next().unwrap_or("");
        assert!(
            STAGES.contains(&stage),
            "emitter outside the stage vocabulary: {}",
            ev.emitter
        );
    }
    assert!(n >= 10, "the fixture builds its edges: {n}");
}

#[test]
fn structural_edges_point_at_the_child() {
    let (_tmp, m) = build();
    let (a, bar) = (by_qname(&m, "a"), by_qname(&m, "a::bar"));
    let ev = evidence_of(edge(&m, a, bar, edge_category::DEFINES));
    assert_eq!(ev.emitter, "parser:python");
    assert_eq!(ev.basis, Basis::ToNode);
    assert_eq!(ev.file.as_deref(), Some("a.py"));
    let bar_cells = m
        .graphs
        .iter()
        .find_map(|g| g.nodes.iter().find(|n| n.id == bar))
        .map(|n| n.cells.clone())
        .unwrap_or_default();
    let (file, line) = evidence::locate(&bar_cells).expect("a::bar has a POSITION");
    assert_eq!(file, "a.py");
    assert_eq!(ev.line, line, "the child's POSITION start_line");
}

#[test]
fn graph_edges_are_attributed() {
    let (_tmp, m) = build();
    let (baz, bar) = (by_qname(&m, "b::baz"), by_qname(&m, "a::bar"));
    let ev = evidence_of(edge(&m, baz, bar, edge_category::CALLS));
    assert!(ev.emitter.starts_with("graph:"), "{ev:?}");
    assert_eq!(ev.file.as_deref(), Some("b.py"));
    // from_node until a stage records the call site (then site, same file).
    assert!(matches!(ev.basis, Basis::FromNode | Basis::Site), "{ev:?}");
}

/// LC.3d: a graph-resolved edge names the mechanism and the branch that
/// bound it, not only the graph stage: `from a import bar` + `bar()` is an
/// import binding.
#[test]
fn cross_file_call_names_the_resolver_branch() {
    let (_tmp, m) = build();
    let (baz, bar) = (by_qname(&m, "b::baz"), by_qname(&m, "a::bar"));
    let ev = evidence_of(edge(&m, baz, bar, edge_category::CALLS));
    assert_eq!(ev.emitter, "graph:calls", "{ev:?}");
    assert_eq!(ev.rule.as_deref(), Some("import_binding"), "{ev:?}");
    assert_eq!(ev.file.as_deref(), Some("b.py"));
}

#[test]
fn resolver_and_pass_edges_are_attributed() {
    let (_tmp, m) = build();
    let http = m
        .all_edges()
        .find(|e| e.category == edge_category::HTTP_CALLS)
        .expect("the flask route pairs with the fetch");
    let ev = evidence_of(http);
    assert_eq!(ev.emitter, "resolver:http");
    assert_eq!(ev.file.as_deref(), Some("client.ts"));
    // ENDPOINT_HIT line 2 (1-based) through the http_node_span fallback.
    assert_eq!(ev.line, Some(1));
    assert_eq!(ev.basis, Basis::FromNode);

    let doc = m
        .all_edges()
        .find(|e| e.category == edge_category::DOCUMENTS)
        .expect("the README documents bar");
    let ev = evidence_of(doc);
    assert_eq!(ev.emitter, "pass:doclink");
    assert_eq!(ev.rule.as_deref(), Some("unique"));
    assert_eq!(ev.file.as_deref(), Some("README.md"));
}

/// Map of file name -> bytes for every file in a sharded output dir.
fn dir_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn evidence_is_deterministic() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    write_fixture(&repo);
    let repo_s = repo.to_str().unwrap();
    let (out1, out2) = (tmp.path().join("out1"), tmp.path().join("out2"));
    write_merged_sharded(&generate_one(repo_s).unwrap().merged, &out1).unwrap();
    write_merged_sharded(&generate_one(repo_s).unwrap().merged, &out2).unwrap();
    let (a, b) = (dir_bytes(&out1), dir_bytes(&out2));
    assert_eq!(
        a.iter().map(|(n, _)| n).collect::<Vec<_>>(),
        b.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    for ((name, ba), (_, bb)) in a.iter().zip(b.iter()) {
        assert_eq!(ba, bb, "{name} bytes differ");
    }
}

fn layout_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "gmap"))
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum()
}

/// The cost of evidence on the glia self-build: the layout with every edge's
/// cells against the same layout with them stripped. Run by hand:
/// `cargo test -p repo-graph-engine --test edge_evidence -- --ignored --nocapture`.
#[test]
#[ignore]
fn evidence_size_budget_on_self() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let root = root.canonicalize().unwrap();
    let mut merged = generate_many(&[root.to_string_lossy().to_string()])
        .unwrap()
        .merged;
    let tmp = tempfile::tempdir().unwrap();
    let (with, without) = (tmp.path().join("with"), tmp.path().join("without"));
    write_merged_sharded(&merged, &with).unwrap();
    for g in &mut merged.graphs {
        for e in &mut g.edges {
            e.cells.clear();
        }
    }
    for e in &mut merged.cross_edges {
        e.cells.clear();
    }
    write_merged_sharded(&merged, &without).unwrap();
    let (a, b) = (layout_bytes(&with), layout_bytes(&without));
    let ratio = a as f64 / b as f64;
    println!("[evidence-size] with={a} without={b} ratio={ratio:.4}");
    assert!(
        ratio <= 1.25,
        "evidence grows the self-build layout by more than 25%: {ratio:.4}"
    );
}
