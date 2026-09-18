//! One-hop graph traversal from a node.

use pyo3::prelude::*;

use repo_graph_core::NodeId;

use crate::graph::PyGraph;

#[pymethods]
impl PyGraph {
    fn neighbours(&self, node_id: u64) -> Vec<(u64, u32)> {
        let id = NodeId(node_id);
        let mut result = Vec::new();
        for g in &self.merged.graphs {
            for e in &g.edges {
                if e.from == id {
                    result.push((e.to.0, e.category.0));
                }
            }
        }
        for e in &self.merged.cross_edges {
            if e.from == id {
                result.push((e.to.0, e.category.0));
            }
        }
        result
    }
}
