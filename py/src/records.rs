//! The whole graph as flat JSON records: every node, every edge.

use pyo3::prelude::*;

use repo_graph_core::Confidence;
use repo_graph_graph::MergedGraph;

use crate::convert::escape_json;
use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    /// Every node as `{id, kind, name, qname, confidence, path, start_line,
    /// end_line}`. `start_line` / `end_line` are 1-based and inclusive — the
    /// same base as the `line` of every answer record (`blast_radius`,
    /// `resolve`, `cross_stack_trace`, ...), so the two never disagree about
    /// where a node starts. Returns a JSON array.
    fn nodes_json(&self) -> PyResult<String> {
        Ok(nodes_json_string(&self.merged))
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
/// repo-graph-py` can exercise it (see the crate doc's link note). Stored
/// POSITION rows are 0-based; this emits them 1-based, the answer-record base.
fn nodes_json_string(merged: &MergedGraph) -> String {
    let mut out = String::from("[");
    let mut first = true;
    for g in &merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).map(|k| k.0).unwrap_or(0);
            let name = g.nav.name_by_id.get(&n.id).map(|s| s.as_str()).unwrap_or("");
            let qname = g.nav.qname_by_id.get(&n.id).map(|s| s.as_str()).unwrap_or("");
            let conf = match n.confidence {
                Confidence::Strong => "strong",
                Confidence::Medium => "medium",
                Confidence::Weak => "weak",
            };
            if !first {
                out.push(',');
            }
            first = false;
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
            out.push_str(&format!(
                r#"{{"id":{},"kind":{},"name":"{}","qname":"{}","confidence":"{}"{}}}"#,
                n.id.0,
                kind,
                escape_json(name),
                escape_json(qname),
                conf,
                span,
            ));
        }
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    use super::nodes_json_string;
    use repo_graph_core::NodeId;

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

        let v: serde_json::Value =
            serde_json::from_str(&nodes_json_string(&merged)).expect("valid JSON");
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
    }
}
