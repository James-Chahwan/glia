//! A3.6 acceptance: `locate_node` places ROUTE and ENDPOINT nodes.
//!
//! No ROUTE or ENDPOINT node carries a POSITION cell, so before A3.6 every
//! P3 answer (`blast_radius`, `cross_stack_trace`, `resolve`,
//! `governing_docs`) reported `file: null, line: null` for exactly the nodes a
//! cross-stack question is about. The span was on the graph all along:
//!
//! - tier 2 — the ENDPOINT_HIT cell (every client language) and the JSON
//!   ROUTE_METHOD cell (parser-go, ts_routes) carry `file` + a 1-indexed
//!   `line`, which `locate_node` must convert to POSITION's 0-indexed row;
//! - tier 3 — the eleven parsers that write ROUTE_METHOD as a bare verb are
//!   placed by the handler the route is HANDLED_BY.
//!
//! Every assertion pins an EXACT line: the 1 → 0 conversion is the easy thing
//! to get wrong, and `is_some()` would not catch an off-by-one.

use repo_graph_engine::{cross_stack_trace, generate_many, locate_node};
use repo_graph_graph::MergedGraph;

/// Three services under one tempdir:
/// - `web/`    — a TS client (`fetch('/users')`, call on 0-indexed row 1) and
///   an Express server whose inline handler gives ts_routes nothing to bind,
///   so its ROUTE carries only the `"line":0` placeholder;
/// - `goapi/`  — a chi server, `r.Get("/users", listUsers)` on row 14;
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
    let (_, q, kind, file, line) = locate_node(m, id);
    assert_eq!(q, qname);
    (kind, file, line)
}

#[test]
fn http_nodes_are_located_by_cell_or_handler() {
    let (_td, m) = fixture();

    // (1) ENDPOINT via its ENDPOINT_HIT cell: `fetch` sits on 1-indexed line
    // 2 of api.ts, which is 0-indexed row 1 — the POSITION convention.
    assert_eq!(
        locate(&m, "endpoint:GET:/users"),
        ("ENDPOINT", Some("api.ts".to_string()), Some(1)),
    );

    // (2) Go ROUTE via its JSON ROUTE_METHOD cell: `r.Get(...)` is 1-indexed
    // line 15, 0-indexed row 14.
    assert_eq!(
        locate(&m, "route:/users"),
        ("ROUTE", Some("main.go".to_string()), Some(14)),
    );

    // (3) Flask ROUTE: ROUTE_METHOD is the bare text "GET", so the span is
    // borrowed from the handler it is HANDLED_BY — exactly `get_user`'s own
    // POSITION, whatever row the parser starts that span on.
    let handler = locate(&m, "app::get_user");
    assert_eq!(handler.0, "FUNCTION");
    assert_eq!(handler.1.as_deref(), Some("app.py"));
    assert_eq!(handler.2, Some(6), "get_user's POSITION starts on its `def` row");
    assert_eq!(
        locate(&m, "GET /users/<int:uid>"),
        ("ROUTE", handler.1.clone(), handler.2),
    );

    // (4) ts_routes writes `"line":0` — "unknown", not line 0. The file is
    // real; the line must be None rather than a bogus -1 or 0.
    assert_eq!(
        locate(&m, "route:/health"),
        ("ROUTE", Some("server.ts".to_string()), None),
    );
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
        located.contains(&("endpoint:GET:/users", Some("api.ts"), Some(1))),
        "{located:?}"
    );
    assert!(
        located.contains(&("route:/users", Some("main.go"), Some(14))),
        "{located:?}"
    );
}
