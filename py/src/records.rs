//! The whole graph as flat JSON records: every node, every edge.

use pyo3::prelude::*;

use std::collections::HashSet;

use repo_graph_code_domain::node_kind;
use repo_graph_core::{Confidence, Node, NodeId};
use repo_graph_engine::profile::CODE_PROFILE;
use repo_graph_graph::roles::roles_in;
use repo_graph_graph::{MergedGraph, RepoGraph};

use crate::convert::escape_json;
use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    /// Every node as `{id, kind, name, qname, confidence, path, start_line,
    /// end_line, roles, entry, live}`. `start_line` / `end_line` are 1-based
    /// and inclusive — the same base as the `line` of every answer record
    /// (`blast_radius`, `resolve`, `cross_stack_trace`, ...), so the two never
    /// disagree about where a node starts. `roles` names the framework roles
    /// the node plays (`["COMPONENT"]`, `["SERVICE"]`, ...; `[]` when none):
    /// since LB.3a a component or service is a CLASS / FUNCTION carrying a
    /// ROLE cell, so a consumer that tiers or iconifies by role reads `roles`,
    /// not `kind`. `entry` (LD.6) is the code domain's entrypoint rule over the
    /// node's kind, name and roles (the kinds `entry_kinds()` lists, `main` /
    /// `test*` functions, a COMPONENT role); `live` is whether an entrypoint
    /// reaches it (`false` = likely dead), the flag every answer record
    /// carries. Returns a JSON array.
    fn nodes_json(&self) -> PyResult<String> {
        Ok(nodes_json_string(&self.merged, self.live()))
    }

    fn edges_json(&self) -> PyResult<String> {
        let mut out = String::from("[");
        let mut first = true;
        let all_edges = self
            .merged
            .graphs
            .iter()
            .flat_map(|g| g.edges.iter())
            .chain(self.merged.cross_edges.iter());
        for e in all_edges {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                r#"{{"from":{},"to":{},"category":{}}}"#,
                e.from.0, e.to.0, e.category.0,
            ));
        }
        out.push(']');
        Ok(out)
    }
}

/// The body of `PyGraph::nodes_json`, pyo3-free so `cargo test -p
/// repo-graph-py` can exercise it (see the crate doc's link note). `live` is
/// the graph's `entrypoint_reachable` set. Prints LD.6's
/// `[live] annotate surface=nodes_json rows=<n> live=<l> entry_kinds=<k>`,
/// the engine's `answers::live_marker` format.
fn nodes_json_string(merged: &MergedGraph, live: &HashSet<NodeId>) -> String {
    let mut out = String::from("[");
    let (mut rows, mut live_rows) = (0usize, 0usize);
    for g in &merged.graphs {
        for n in &g.nodes {
            if rows > 0 {
                out.push(',');
            }
            rows += 1;
            let is_live = live.contains(&n.id);
            live_rows += usize::from(is_live);
            out.push_str(&node_record_json(g, n, is_live));
        }
    }
    out.push(']');
    eprintln!(
        "[live] annotate surface=nodes_json rows={rows} live={live_rows} entry_kinds={}",
        CODE_PROFILE.tables.entry.kinds.len()
    );
    out
}

/// One `nodes_json` record. Stored POSITION rows are 0-based; this emits
/// them 1-based, the answer-record base. `roles` comes from the one reader,
/// `roles_in` (the node's own role kind plus its ROLE cell), always present;
/// `entry` applies `CODE_PROFILE.tables.entry` to the kind, name and those
/// roles (LD.6), and `live` is the caller's membership test.
fn node_record_json(g: &RepoGraph, n: &Node, live: bool) -> String {
    let kind = g.nav.kind_by_id.get(&n.id).copied();
    let name = g.nav.name_by_id.get(&n.id).map(|s| s.as_str()).unwrap_or("");
    let qname = g.nav.qname_by_id.get(&n.id).map(|s| s.as_str()).unwrap_or("");
    let conf = match n.confidence {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    };
    // GR-1: surface the node's source span from its POSITION cell.
    // Stored rows are 0-based (tree-sitter); emit 1-based inclusive.
    // Nodes without a span (synthetic / cross-stack) carry null.
    let span = match repo_graph_projection_text::node_position(n) {
        Some(p) => format!(
            r#","path":"{}","start_line":{},"end_line":{}"#,
            escape_json(&p.file),
            p.start_line + 1,
            p.end_line + 1,
        ),
        None => r#","path":null,"start_line":null,"end_line":null"#.to_string(),
    };
    let role_kinds = roles_in(kind, &n.cells);
    let entry = CODE_PROFILE.tables.entry.is_entry(kind, name, &role_kinds);
    let roles: Vec<String> = role_kinds
        .into_iter()
        .map(|r| format!("\"{}\"", escape_json(node_kind::name(r))))
        .collect();
    format!(
        r#"{{"id":{},"kind":{},"name":"{}","qname":"{}","confidence":"{}"{},"roles":[{}],"entry":{},"live":{}}}"#,
        n.id.0,
        kind.map(|k| k.0).unwrap_or(0),
        escape_json(name),
        escape_json(qname),
        conf,
        span,
        roles.join(","),
        entry,
        live,
    )
}

#[cfg(test)]
mod tests {
    use super::{node_record_json, nodes_json_string};
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
    use repo_graph_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};
    use repo_graph_graph::{RepoGraph, SymbolTable};

    /// LB.3b: every record carries `roles`, read through `roles_in` — a CLASS
    /// folded from an `@Injectable` names SERVICE, a plain FUNCTION an empty
    /// list, a standalone role node its own kind.
    #[test]
    fn node_record_carries_roles() {
        let repo = RepoId::from_canonical("test://records-roles");
        let class = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, "svc::Api");
        let func = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, "svc::helper");
        let comp = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::COMPONENT, "ui::Card");
        let mut nav = CodeNav::default();
        nav.record(class, "Api", "svc::Api", node_kind::CLASS, None);
        nav.record(func, "helper", "svc::helper", node_kind::FUNCTION, None);
        nav.record(comp, "Card", "ui::Card", node_kind::COMPONENT, None);
        let node = |id, cells| Node { id, repo, confidence: Confidence::Strong, cells };
        let role = Cell {
            kind: cell_type::ROLE,
            payload: CellPayload::Json(r#"{"roles":["SERVICE"]}"#.into()),
        };
        let g = RepoGraph {
            repo,
            nodes: vec![node(class, vec![role]), node(func, vec![]), node(comp, vec![])],
            edges: vec![],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let rec = |i: usize| node_record_json(&g, &g.nodes[i], false);
        assert!(rec(0).ends_with(r#","roles":["SERVICE"],"entry":false,"live":false}"#), "{}", rec(0));
        assert!(rec(1).ends_with(r#","roles":[],"entry":false,"live":false}"#), "{}", rec(1));
        assert!(rec(2).ends_with(r#","roles":["COMPONENT"],"entry":true,"live":false}"#), "{}", rec(2));
        let v: serde_json::Value = serde_json::from_str(&rec(0)).expect("valid JSON");
        assert_eq!(v["kind"], node_kind::CLASS.0);
        assert_eq!(v["qname"], "svc::Api");
        assert_eq!(v["roles"], serde_json::json!(["SERVICE"]));
    }

    /// LD.1: `nodes_json` and every answer record share ONE line base. Before
    /// LD.1 `nodes_json` said `helper` starts on line 4 while `blast_radius` /
    /// `resolve` / `cross_stack_trace` (all through `locate_node`) said 3 —
    /// two conventions in one wheel, visible to MCP clients as `find` and
    /// `impact` disagreeing about the same function.
    #[test]
    fn nodes_json_and_records_share_one_line_base() {
        let root = std::env::temp_dir().join(format!("glia-ld1-py-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        let app = "import os\n\n\ndef helper(x):\n    return x + 1\n\n\ndef main():\n    return helper(2)\n";
        std::fs::write(root.join("app.py"), app).expect("write fixture");
        let built = repo_graph_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let merged = built.expect("build").merged;

        let live = repo_graph_engine::entrypoint_reachable(&merged);
        let v: serde_json::Value =
            serde_json::from_str(&nodes_json_string(&merged, &live)).expect("valid JSON");
        let mut spans: Vec<(String, i64, Option<i64>)> = v
            .as_array()
            .expect("a JSON array")
            .iter()
            .filter_map(|n| {
                let start = n["start_line"].as_i64()?;
                let id = NodeId(n["id"].as_u64()?);
                let qname = n["qname"].as_str()?.to_string();
                Some((qname, start, repo_graph_engine::locate_node(&merged, id).line))
            })
            .collect();
        spans.sort();
        let qnames: Vec<&str> = spans.iter().map(|(q, _, _)| q.as_str()).collect();
        assert_eq!(qnames, ["app", "app::helper", "app::main"], "{spans:?}");
        for (qname, start, located) in &spans {
            assert_eq!(
                Some(*start),
                *located,
                "{qname}: nodes_json start_line and the answer-record line must agree"
            );
        }
        // The values themselves: 1-based, as an editor shows them.
        let starts: Vec<i64> = spans.iter().map(|(_, s, _)| *s).collect();
        assert_eq!(starts, [1, 4, 8]);

        // LD.6: every record carries bool `entry` / `live`. `main` is an
        // entry by name and live; `helper` is live through main's call but no
        // entry; the MODULE is neither.
        let flags: Vec<(String, bool, bool)> = v
            .as_array()
            .expect("a JSON array")
            .iter()
            .map(|n| {
                let entry = n["entry"].as_bool().expect("bool entry");
                let live = n["live"].as_bool().expect("bool live");
                (n["qname"].as_str().unwrap_or("").to_string(), entry, live)
            })
            .collect();
        for (q, entry, live) in [("app", false, false), ("app::helper", false, true), ("app::main", true, true)] {
            assert!(flags.contains(&(q.to_string(), entry, live)), "{q}: {flags:?}");
        }
    }
}
