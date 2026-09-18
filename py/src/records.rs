//! The whole graph as flat JSON records: every node, every edge.

use pyo3::prelude::*;

use repo_graph_core::Confidence;

use crate::convert::escape_json;
use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    fn nodes_json(&self) -> PyResult<String> {
        let mut out = String::from("[");
        let mut first = true;
        for g in &self.merged.graphs {
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
        Ok(out)
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
