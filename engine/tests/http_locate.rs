//! A3.6 acceptance: `locate_node` places ROUTE and ENDPOINT nodes.
//!
//! No ROUTE or ENDPOINT node carries a POSITION cell, so before A3.6 every
//! P3 answer (`blast_radius`, `cross_stack_trace`, `resolve`,
//! `governing_docs`) reported `file: null, line: null` for exactly the nodes a
//! cross-stack question is about. The span was on the graph all along:
//!
//! - tier 2 — the ENDPOINT_HIT cell (every client language) and the JSON
//!   ROUTE_METHOD cell (parser-go, ts_routes) carry `file` + a 1-indexed
//!   `line`, which the locator converts to POSITION's 0-indexed row. Go
//!   (LA.32a) and ts_routes (LB.11b) ROUTEs now also carry a POSITION per
//!   registration, so tier 1 answers them first;
//! - tier 3 — the eleven parsers that write ROUTE_METHOD as a bare verb are
//!   placed by the handler the route is HANDLED_BY.
//!
//! LD.1: every tier stays row-based internally and `Locator::locate` adds 1
//! at its single exit, so a reported `line` is the 1-based line an editor
//! shows — the same number the cell (tier 2) or `nodes_json` (tiers 1, 3)
//! carries. Every assertion pins an EXACT line: the base conversion is the
//! easy thing to get wrong (twice, or not at all), and `is_some()` would not
//! catch an off-by-one.

use repo_graph_engine::{Locator, cross_stack_trace, generate_many, locate_node};
use repo_graph_graph::MergedGraph;

/// Three services under one tempdir:
/// - `web/`    — a TS client (`fetch('/users')`, call on line 2) and
///   an Express server, `app.get('/health', ...)` on line 5, whose inline
///   handler gives ts_routes nothing to bind: the route is placed by its own
///   registration POSITION (LB.11b), not by a handler;
/// - `goapi/`  — a chi server, `r.Get("/users", listUsers)` on line 15;
/// - `pyapi/`  — a Flask server, `@app.route('/users/<int:uid>')` + `get_user`.
fn fixture() -> (tempfile::TempDir, MergedGraph) {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path();
    for d in ["web", "goapi", "pyapi"] {
        std::fs::create_dir_all(root.join(d)).expect("mkdir");
    }
    std::fs::write(
        root.join("web/api.ts"),
        "export async function loadUsers() {\n  const res = await fetch('/users');\n  return res.json();\n}\n",
    )
    .expect("write api.ts");
    std::fs::write(
        root.join("web/server.ts"),
        "import express from 'express';\n\nconst app = express();\n\napp.get('/health', (req, res) => res.send('ok'));\n\napp.listen(3000);\n",
    )
    .expect("write server.ts");
    std::fs::write(
        root.join("goapi/main.go"),
        "package main\n\
         \n\
         import (\n\
         \t\"net/http\"\n\
         \n\
         \t\"github.com/go-chi/chi/v5\"\n\
         )\n\
         \n\
         func listUsers(w http.ResponseWriter, r *http.Request) {\n\
         \tw.Write([]byte(\"[]\"))\n\
         }\n\
         \n\
         func main() {\n\
         \tr := chi.NewRouter()\n\
         \tr.Get(\"/users\", listUsers)\n\
         \thttp.ListenAndServe(\":8080\", r)\n\
         }\n",
    )
    .expect("write main.go");
    std::fs::write(
        root.join("pyapi/app.py"),
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route('/users/<int:uid>')\ndef get_user(uid):\n    return {\"id\": uid}\n",
    )
    .expect("write app.py");

    let paths: Vec<String> = ["web", "goapi", "pyapi"]
        .iter()
        .map(|d| root.join(d).to_string_lossy().into_owned())
        .collect();
    let merged = generate_many(&paths).expect("generate_many").merged;
    (td, merged)
}

/// `(kind, file, line)` for the one node with `qname`.
fn locate(m: &MergedGraph, qname: &str) -> (&'static str, Option<String>, Option<i64>) {
    let id = m
        .node_id_by_qname(qname)
        .unwrap_or_else(|| panic!("no node `{qname}` in the fixture graph"));
    let at = locate_node(m, id);
    assert_eq!(at.qname, qname);
    assert_eq!(at.id, id.0);
    (at.kind, at.file, at.line)
}

#[test]
fn http_nodes_are_located_by_cell_or_handler() {
    let (_td, m) = fixture();

    // (1) ENDPOINT via its ENDPOINT_HIT cell: `fetch` sits on 1-indexed line
    // 2 of api.ts — the cell's own line, reported as-is (1 → row 1 → +1).
    assert_eq!(
        locate(&m, "endpoint:GET:/users"),
        ("ENDPOINT", Some("api.ts".to_string()), Some(2)),
    );

    // (2) Go ROUTE via its JSON ROUTE_METHOD cell: `r.Get(...)` is 1-indexed
    // line 15. One node per (method, path), `GET /users` (LB.11a).
    assert_eq!(
        locate(&m, "GET /users"),
        ("ROUTE", Some("main.go".to_string()), Some(15)),
    );

    // (3) Flask ROUTE: ROUTE_METHOD is the bare text "GET", so the span is
    // borrowed from the handler it is HANDLED_BY — exactly `get_user`'s own
    // POSITION, whatever row the parser starts that span on.
    let handler = locate(&m, "app::get_user");
    assert_eq!(handler.0, "FUNCTION");
    assert_eq!(handler.1.as_deref(), Some("app.py"));
    assert_eq!(handler.2, Some(7), "get_user's span starts on its 1-based `def` line");
    assert_eq!(
        locate(&m, "GET /users/<int:uid>"),
        ("ROUTE", handler.1.clone(), handler.2),
    );

    // (4) ts_routes ROUTE (LB.11b): one node per (method, path), `GET
    // /health`, located by tier 1 — the POSITION at its registration, 0-based
    // row 4, reported as the 1-based line 5. Before LB.11b it was
    // `route:/health` with a `"line":0` placeholder and no line at all.
    assert_eq!(
        locate(&m, "GET /health"),
        ("ROUTE", Some("server.ts".to_string()), Some(5)),
    );
    assert!(m.node_id_by_qname("route:/health").is_none());
}

#[test]
fn trace_hops_into_http_nodes_carry_a_location() {
    // The four P3 call sites inherit the fix with no change of their own;
    // `cross_stack_trace` is the one whose hop target is typically an HTTP
    // node, so prove the fallback reaches it end to end.
    let (_td, m) = fixture();
    let hops = cross_stack_trace(&m, "loadUsers", 4).expect("seed resolves");
    let located: Vec<(&str, Option<&str>, Option<i64>)> = hops
        .iter()
        .map(|h| (h.to_qname.as_str(), h.to_file.as_deref(), h.to_line))
        .collect();
    assert!(
        located.contains(&("endpoint:GET:/users", Some("api.ts"), Some(2))),
        "{located:?}"
    );
    assert!(
        located.contains(&("GET /users", Some("main.go"), Some(15))),
        "{located:?}"
    );
}

/// LD.1 scale: a `Locator` is built once (O(V)) and then answers each row from
/// its index, so an answer with R rows costs O(V + R). Before LD.1 every
/// located row re-scanned `g.nodes`: 20k rows over a 20k-node graph was 4e8
/// node visits. 20k lookups against a 20k-node graph must finish well inside
/// 2 s even in a debug build, and each must be the right node.
#[test]
fn locator_answers_do_not_rescan_nodes() {
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
    use repo_graph_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};
    use repo_graph_graph::{RepoGraph, SymbolTable};

    const N: usize = 20_000;
    let repo = RepoId::from_canonical("test://locator-scale");
    let mut nav = CodeNav::default();
    let mut nodes = Vec::with_capacity(N);
    let mut expect: Vec<(NodeId, String)> = Vec::with_capacity(N);
    for i in 0..N {
        let qname = format!("m::f{i}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);
        nav.record(id, &format!("f{i}"), &qname, node_kind::FUNCTION, None);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(format!(
                    r#"{{"file":"m.py","start_line":{i},"end_line":{i}}}"#
                )),
            }],
        });
        expect.push((id, qname));
    }
    let m = MergedGraph::new(vec![RepoGraph {
        repo,
        nodes,
        edges: vec![],
        nav,
        symbols: SymbolTable::default(),
        unresolved_calls: vec![],
        unresolved_refs: vec![],
        properties: Default::default(),
    }]);

    let started = std::time::Instant::now();
    let loc = Locator::new(&m);
    for (i, (id, qname)) in expect.iter().enumerate() {
        let at = loc.locate(*id);
        assert_eq!(&at.qname, qname);
        assert_eq!(at.line, Some(i as i64 + 1), "row {i} is line {}", i + 1);
    }
    let took = started.elapsed();
    assert!(
        took < std::time::Duration::from_secs(2),
        "{N} locates over {N} nodes took {took:?}: the index is being bypassed"
    );
}
