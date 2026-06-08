//! pyo3 bindings for `repo-graph-engine`. The orchestration logic
//! (file walking, per-language parsing, cross-cutting extraction, resolver
//! execution, post-passes) lives in `engine/src/lib.rs` and is shared with
//! the `glia` CLI. This file is intentionally thin — only the Python-facing
//! surface lives here.

use std::path::Path;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, Confidence, NodeId, RepoId};
use repo_graph_engine::{generate_many as engine_generate_many, generate_one, parse_one};
use repo_graph_graph::MergedGraph;
use repo_graph_store::{
    default_gmap_dir as store_default_gmap_dir, is_gmap_stale, read_merged_sharded,
    write_merged_sharded,
};

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

    /// Scoped dense sigil text for just `node_ids` (+ structural glue), not the
    /// whole graph (WP-C / GR-3). `full` keeps untruncated cell bodies. Pass the
    /// top-K from `activate` for a scoped view / `mode=prose` precursor.
    #[pyo3(signature = (node_ids, full=false))]
    fn dense_text_subset(&self, node_ids: Vec<u64>, full: bool) -> String {
        let ids: Vec<NodeId> = node_ids.into_iter().map(NodeId).collect();
        let sub = self.merged.subset(&ids);
        if full {
            repo_graph_projection_text::render_merged_full(&sub)
        } else {
            repo_graph_projection_text::render_merged(&sub)
        }
    }

    /// Prose projection (WP-C / GR-3) of just `node_ids`: one readable block per
    /// node (kind, qname, location, doc/code preview). Backs `mode=prose`.
    fn prose(&self, node_ids: Vec<u64>) -> String {
        let ids: Vec<NodeId> = node_ids.into_iter().map(NodeId).collect();
        let sub = self.merged.subset(&ids);
        repo_graph_projection_text::render_prose(&sub)
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

    /// All cells on a node as `(cell_type_id, payload)` pairs (WP-J / #8).
    /// Pair the id with `cell_type_names()` to label. Text/Json payloads return
    /// their string (imports/state-var/doc cells included); Bytes payloads
    /// (cached embeddings) return "". Structured access instead of scraping
    /// `dense_text`. Empty if the node id is unknown.
    fn node_cells(&self, node_id: u64) -> Vec<(u32, String)> {
        let id = NodeId(node_id);
        for g in &self.merged.graphs {
            for n in &g.nodes {
                if n.id == id {
                    return n
                        .cells
                        .iter()
                        .map(|c| {
                            let payload = match &c.payload {
                                CellPayload::Text(s) | CellPayload::Json(s) => s.clone(),
                                CellPayload::Bytes(_) => String::new(),
                            };
                            (c.kind.0, payload)
                        })
                        .collect();
                }
            }
        }
        Vec::new()
    }

    /// Spreading activation (PPR) from `seed_ids`. `profile` (WP-F / GR-5)
    /// selects an edge-weight preset — "default", "repair", "review", or
    /// "onboard" — so the same engine serves different agent tasks. Returns
    /// `(id, score)` pairs, score-sorted, capped at `top_k`.
    #[pyo3(signature = (seed_ids, top_k=None, profile=None))]
    fn activate(
        &self,
        seed_ids: Vec<u64>,
        top_k: Option<usize>,
        profile: Option<String>,
    ) -> Vec<(u64, f64)> {
        let seeds: Vec<NodeId> = seed_ids.into_iter().map(NodeId).collect();
        let mut config = match profile.as_deref() {
            Some(p) => repo_graph_graph::code_activation_profile(p),
            None => repo_graph_graph::code_activation_defaults(),
        };
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

    /// Resolve a simple name to a node id. Deterministic across processes: when
    /// several nodes share the name (e.g. an Angular component's `CLASS` and its
    /// framework `COMPONENT` marker), the highest-degree node wins rather than
    /// whichever the per-process `HashMap` seed happened to order first — the
    /// root cause of `impact`/`trace` intermittently returning empty.
    fn find_node(&self, name: &str) -> Option<u64> {
        self.merged.resolve_name(name).map(|id| id.0)
    }

    /// Substring search over qnames, returned sorted by node id so repeated
    /// calls (and any caller that takes `[0]`) are reproducible across processes.
    /// Resolve a failure/change signal to seed node ids (WP-B / GR-2 `locate`).
    /// `kind` ∈ {"stacktrace", "test", "diff", "auto"}; "auto" sniffs the shape.
    /// Frame/symbol/path → node-id resolution (and the sniffer) run in Rust;
    /// unresolvable tokens are simply absent. Feed the result to `activate`.
    #[pyo3(signature = (text, kind="auto"))]
    fn resolve_signal(&self, text: &str, kind: &str) -> Vec<u64> {
        self.merged.resolve_signal(text, kind).into_iter().map(|id| id.0).collect()
    }

    fn find_nodes_by_qname(&self, pattern: &str) -> Vec<u64> {
        self.merged
            .qnames_containing(pattern)
            .into_iter()
            .map(|id| id.0)
            .collect()
    }

    /// Persist this graph to a sharded `.gmap` layout at `dir`. Creates `dir`
    /// if missing. Idempotent: re-writing the same graph is content-hash
    /// skipped (see `write_sharded`'s skip-when-unchanged logic).
    fn save_to(&self, dir: &str) -> PyResult<()> {
        write_merged_sharded(&self.merged, Path::new(dir))
            .map(|_| ())
            .map_err(|e| PyValueError::new_err(format!("save_to({dir}): {e}")))
    }

    /// Convenience: save under the conventional `<repo>/.ai/repo-graph/`. The
    /// wrapper's cache-load path will find it there.
    fn save_to_default(&self, repo_path: &str) -> PyResult<()> {
        let dir = store_default_gmap_dir(Path::new(repo_path));
        write_merged_sharded(&self.merged, &dir)
            .map(|_| ())
            .map_err(|e| PyValueError::new_err(format!("save_to_default({}): {e}", dir.display())))
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

/// Build the graph for a repo. `incremental` (default True, WP-D) reuses a
/// per-file parse cache at `<repo>/.ai/repo-graph/parse_cache.bin` so unchanged
/// files skip tree-sitter; the result is identical to a clean build. Pass
/// `incremental=False` to force a full reparse.
#[pyfunction]
#[pyo3(signature = (repo_path, incremental=true))]
fn generate(repo_path: &str, incremental: bool) -> PyResult<PyGraph> {
    let result = if incremental {
        repo_graph_engine::generate_one_incremental(repo_path)
    } else {
        generate_one(repo_path)
    }
    .map_err(PyValueError::new_err)?;
    if !result.parse_errors.is_empty()
        && result.merged.graphs.iter().all(|g| g.nodes.is_empty())
    {
        return Err(PyValueError::new_err(format!(
            "no nodes produced; {} parse errors: {}",
            result.parse_errors.len(),
            result.parse_errors.first().unwrap_or(&String::new())
        )));
    }
    // Auto-persist to the conventional gmap dir so the next session can
    // `load_from_gmap` instead of regenerating. Failure to write is logged but
    // not fatal — a fresh in-memory graph is still usable, the cache layer is
    // an optimization. Opt out with `GLIA_NO_PERSIST=1` (tests / experiments
    // that don't want side effects on the target repo).
    if std::env::var("GLIA_NO_PERSIST").as_deref() != Ok("1") {
        let dir = store_default_gmap_dir(Path::new(repo_path));
        if let Err(e) = write_merged_sharded(&result.merged, &dir) {
            eprintln!(
                "[repo-graph-py] warning: failed to persist gmap to {}: {e}",
                dir.display()
            );
        }
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

/// Load a previously-generated graph from a sharded `.gmap` directory.
/// `dir` must contain `manifest.json` + the per-shard `.gmap` files written by
/// `PyGraph.save_to` / `save_to_default`. Returns a `PyGraph` whose downstream
/// methods (node_count, dense_text, activate, …) behave identically to a fresh
/// `generate()` result, except `RepoGraph.properties` is empty (parse-time-only
/// state, not persisted at FORMAT_VERSION=1).
#[pyfunction]
fn load_from_gmap(dir: &str) -> PyResult<PyGraph> {
    let merged = read_merged_sharded(Path::new(dir))
        .map_err(|e| PyValueError::new_err(format!("load_from_gmap({dir}): {e}")))?;
    Ok(PyGraph { merged })
}

/// Conventional gmap directory path for a repo: `<repo>/.ai/repo-graph`.
/// The Python wrapper uses this to know where to look for a cached graph.
#[pyfunction]
fn default_gmap_dir(repo_path: &str) -> String {
    store_default_gmap_dir(Path::new(repo_path))
        .to_string_lossy()
        .into_owned()
}

/// Is the cached gmap at `gmap_dir` older than any source file under
/// `repo_path`? Used by the wrapper to decide between load and regenerate.
/// Returns `true` if the gmap is missing entirely. Skips `.git`, `target`,
/// `node_modules`, `.venv`, `__pycache__`, and `.ai/`.
#[pyfunction]
fn is_stale(gmap_dir: &str, repo_path: &str) -> bool {
    is_gmap_stale(Path::new(gmap_dir), Path::new(repo_path))
}

/// Canonical node-kind `id → name` table (WP-I / #3). Lets the wrapper decode
/// `nodes_json` kinds without a hardcoded Python table that goes stale when a
/// kind is added. Returns `[(id, name)]`.
#[pyfunction]
fn kind_names() -> Vec<(u32, String)> {
    node_kind::ALL.iter().map(|(id, n)| (id.0, (*n).to_string())).collect()
}

/// Canonical edge-category `id → name` table (WP-I / #3). Pairs with
/// `edges_json` category ids.
#[pyfunction]
fn category_names() -> Vec<(u32, String)> {
    edge_category::ALL.iter().map(|(id, n)| (id.0, (*n).to_string())).collect()
}

/// Canonical cell-type `id → name` table — labels the structured cells exposed
/// by `node_cells` (WP-J).
#[pyfunction]
fn cell_type_names() -> Vec<(u32, String)> {
    cell_type::ALL.iter().map(|(id, n)| (id.0, (*n).to_string())).collect()
}

#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ============================================================================
// Module definition
// ============================================================================

#[pymodule]
fn repo_graph_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(generate, m)?)?;
    m.add_function(wrap_pyfunction!(generate_many, m)?)?;
    m.add_function(wrap_pyfunction!(parse_file_to_json, m)?)?;
    m.add_function(wrap_pyfunction!(load_from_gmap, m)?)?;
    m.add_function(wrap_pyfunction!(default_gmap_dir, m)?)?;
    m.add_function(wrap_pyfunction!(is_stale, m)?)?;
    m.add_function(wrap_pyfunction!(kind_names, m)?)?;
    m.add_function(wrap_pyfunction!(category_names, m)?)?;
    m.add_function(wrap_pyfunction!(cell_type_names, m)?)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_class::<PyGraph>()?;
    Ok(())
}
