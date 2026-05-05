//! pyo3 bindings for `repo-graph-engine`. The orchestration logic
//! (file walking, per-language parsing, cross-cutting extraction, resolver
//! execution, post-passes) lives in `engine/src/lib.rs` and is shared with
//! the `glia` CLI. This file is intentionally thin — only the Python-facing
//! surface lives here.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_code_domain::node_kind;
use repo_graph_core::{Confidence, NodeId, RepoId};
use repo_graph_engine::{generate_many as engine_generate_many, generate_one, parse_one};
use repo_graph_graph::MergedGraph;

#[pyclass]
struct PyGraph {
    merged: MergedGraph,
}

#[pymethods]
impl PyGraph {
    fn node_count(&self) -> usize {
        self.merged.graphs.iter().map(|g| g.nodes.len()).sum()
    }

    fn edge_count(&self) -> usize {
        self.merged.graphs.iter().map(|g| g.edges.len()).sum::<usize>()
            + self.merged.cross_edges.len()
    }

    fn cross_edge_count(&self) -> usize {
        self.merged.cross_edges.len()
    }

    fn dense_text(&self) -> String {
        repo_graph_projection_text::render_merged(&self.merged)
    }

    /// Same as `dense_text()` but preserves full cell bodies (source code is
    /// not truncated to a one-line preview). Use for LLM context construction
    /// where the model needs the actual function body, not a signature stub.
    fn dense_text_full(&self) -> String {
        repo_graph_projection_text::render_merged_full(&self.merged)
    }

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
                out.push_str(&format!(
                    r#"{{"id":{},"kind":{},"name":"{}","qname":"{}","confidence":"{}"}}"#,
                    n.id.0,
                    kind,
                    escape_json(name),
                    escape_json(qname),
                    conf,
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

    fn activate(&self, seed_ids: Vec<u64>, top_k: Option<usize>) -> Vec<(u64, f64)> {
        let seeds: Vec<NodeId> = seed_ids.into_iter().map(NodeId).collect();
        let mut config = repo_graph_graph::code_activation_defaults();
        if let Some(k) = top_k {
            config.top_k = k;
        }
        let result = self.merged.activate(&seeds, &config);
        result
            .scores
            .iter()
            .map(|(id, score)| (id.0, *score))
            .collect()
    }

    fn find_node(&self, name: &str) -> Option<u64> {
        for g in &self.merged.graphs {
            for (id, n) in &g.nav.name_by_id {
                if n == name {
                    return Some(id.0);
                }
            }
        }
        None
    }

    fn find_nodes_by_qname(&self, pattern: &str) -> Vec<u64> {
        let mut result = Vec::new();
        for g in &self.merged.graphs {
            for (id, q) in &g.nav.qname_by_id {
                if q.contains(pattern) {
                    result.push(id.0);
                }
            }
        }
        result
    }
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

// ============================================================================
// Module functions
// ============================================================================

#[pyfunction]
fn generate(repo_path: &str) -> PyResult<PyGraph> {
    let result = generate_one(repo_path).map_err(PyValueError::new_err)?;
    if !result.parse_errors.is_empty()
        && result.merged.graphs.iter().all(|g| g.nodes.is_empty())
    {
        return Err(PyValueError::new_err(format!(
            "no nodes produced; {} parse errors: {}",
            result.parse_errors.len(),
            result.parse_errors.first().unwrap_or(&String::new())
        )));
    }
    Ok(PyGraph {
        merged: result.merged,
    })
}

/// Generate a single MergedGraph from multiple repo paths. Each path becomes
/// its own RepoId, so cross-graph resolvers (HttpStack, DbResolver, etc.) fire
/// across the boundary. Used for substrate eval where one wants to validate
/// that two unrelated services pair correctly under the resolver layer.
#[pyfunction]
fn generate_many(repo_paths: Vec<String>) -> PyResult<PyGraph> {
    let result = engine_generate_many(&repo_paths).map_err(PyValueError::new_err)?;
    Ok(PyGraph {
        merged: result.merged,
    })
}

#[pyfunction]
fn parse_file_to_json(source: &str, path: &str, lang: &str) -> PyResult<String> {
    let repo = RepoId(1);
    let fp = parse_one(source, path, lang, repo).map_err(PyValueError::new_err)?;
    let _ = node_kind::MODULE; // ensure the import is preserved if module impl evolves

    let mut out = String::from("[");
    let mut first = true;
    for n in &fp.nodes {
        let kind = fp.nav.kind_by_id.get(&n.id).map(|k| k.0).unwrap_or(0);
        let name = fp.nav.name_by_id.get(&n.id).map(|s| s.as_str()).unwrap_or("");
        let qname = fp.nav.qname_by_id.get(&n.id).map(|s| s.as_str()).unwrap_or("");
        let conf = match n.confidence {
            Confidence::Strong => "strong",
            Confidence::Medium => "medium",
            Confidence::Weak => "weak",
        };
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!(
            r#"{{"id":{},"kind":{},"name":"{}","qname":"{}","confidence":"{}"}}"#,
            n.id.0,
            kind,
            escape_json(name),
            escape_json(qname),
            conf,
        ));
    }
    out.push(']');
    Ok(out)
}

#[pyfunction]
fn version() -> &'static str {
    "0.4.10"
}

// ============================================================================
// Module definition
// ============================================================================

#[pymodule]
fn repo_graph_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(generate, m)?)?;
    m.add_function(wrap_pyfunction!(generate_many, m)?)?;
    m.add_function(wrap_pyfunction!(parse_file_to_json, m)?)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_class::<PyGraph>()?;
    Ok(())
}
