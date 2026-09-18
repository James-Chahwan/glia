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
    /// Files this build could not parse. Empty for a `.gmap`-loaded graph.
    parse_errors: Vec<String>,
    /// `RepoId.0` → human repo label, carried over from `GenerateResult`.
    /// A `RepoId` is an xxhash of the repo path, so the label cannot be
    /// recovered from the graph itself and does NOT live in the `.gmap`;
    /// empty for a `.gmap`-loaded graph. Only `service_map` reads it.
    repo_labels: std::collections::BTreeMap<u64, String>,
}

/// The whole body of [`PyGraph::service_map`], minus pyo3. Split out because
/// this crate builds with `pyo3/extension-module` (no libpython at link time),
/// so ANY unit test that names a `#[pyclass]` fails to LINK — a helper that
/// touches no pyo3 type is the only way `cargo test -p repo-graph-py` can
/// cover this binding at all.
fn service_map_json(
    merged: &MergedGraph,
    repo_labels: &std::collections::BTreeMap<u64, String>,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&repo_graph_engine::service_map(merged, repo_labels))
}

/// The whole body of [`PyGraph::contracts`], minus pyo3 — split out for the
/// same link reason as [`service_map_json`].
fn contracts_json(merged: &MergedGraph) -> Result<String, serde_json::Error> {
    // `graphs` is one entry per (repo, language), so count distinct repos.
    let repos: std::collections::BTreeSet<u64> = merged.graphs.iter().map(|g| g.repo.0).collect();
    eprintln!("[contracts] surface=pyo3 repos={}", repos.len());
    serde_json::to_string(&repo_graph_engine::message_contracts(merged))
}

#[pymethods]
impl PyGraph {
    /// Files this build could not parse, as `"<path>: <reason>"`, in walk
    /// order. Empty for a graph loaded from a `.gmap` — parse state is not
    /// persisted. Lets a caller tell "this repo has no gRPC" apart from "the
    /// 40 files that would have shown gRPC all failed to parse".
    #[getter]
    fn parse_errors(&self) -> Vec<String> {
        self.parse_errors.clone()
    }

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

    /// **blast_radius** (P3, handoff v6): the complete, deduped,
    /// edge-category-aware, PPR-ranked, LOCATED closure around `qname` — the
    /// answer that `find`→`impact`→`activate`→`read×N` composed to, in ONE call.
    /// Structural `imports`/`contains` edges are excluded so the radius doesn't
    /// fan out through shared containers (handoff P1 bullet 4). Each record:
    /// `{id, qname, name, kind, reason, depth, score, file, line}` where `reason`
    /// is the edge category that first put the node in scope. `direction` ∈
    /// {`forward` (what it affects), `backward` (what affects it), `both`}.
    /// Returns a JSON array, ranked by PPR score (desc). Errors if `qname`
    /// resolves to no node.
    ///
    /// `scope` (optional, default `None` = no-op) restricts the answer to nodes
    /// whose file lives under that repo-relative path — or under the project
    /// with that label (see `project_roots`) — applied BEFORE the
    /// `top_k` cut so a scoped `top_k` spends its budget in scope. Nodes with no
    /// locatable file (ENDPOINT/ROUTE/doc spaces) are KEPT. `scope` narrows
    /// WITHIN a repo — under a multi-repo merge each repo's paths are relative
    /// to its OWN root, so a path passed as a separate repo will not match.
    #[pyo3(signature = (qname, direction="both", depth=4, top_k=None, live_only=false, scope=None))]
    fn blast_radius(
        &self,
        qname: &str,
        direction: &str,
        depth: usize,
        top_k: Option<usize>,
        live_only: bool,
        scope: Option<&str>,
    ) -> PyResult<String> {
        let answer = repo_graph_engine::blast_radius_by_qname(
            &self.merged,
            qname,
            direction,
            depth,
            top_k,
            live_only,
            scope,
        )
        .map_err(PyValueError::new_err)?;
        serde_json::to_string(&answer).map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// **governing_docs** (tier-4 P3 payoff): the doc sections that DOCUMENTS
    /// `qname` — "what are the rules for X?" — located, in one call. Each record
    /// `{id, qname, name, kind, score, file, line}`. Returns a JSON array.
    /// `scope` (optional) keeps only the sections whose own file lives under
    /// that repo-relative path or project label (see `project_roots`);
    /// sections with no file are KEPT.
    #[pyo3(signature = (qname, scope=None))]
    fn governing_docs(&self, qname: &str, scope: Option<&str>) -> PyResult<String> {
        let docs = repo_graph_engine::governing_docs(&self.merged, qname, scope)
            .map_err(PyValueError::new_err)?;
        serde_json::to_string(&docs).map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// **coverage** (P2, handoff v6): for the languages present in the repo, the
    /// known extraction caveats + how many edges of each flagged category exist
    /// — so a consumer falls back to grep DELIBERATELY where glia is
    /// known-partial (dynamic dispatch, string-built URLs, non-standard HTTP
    /// clients) instead of trusting a silent blind spot. Each note
    /// `{language, edge_category, note, verify, edges_found}`. Returns a JSON array.
    fn coverage(&self) -> PyResult<String> {
        let report = repo_graph_engine::coverage_report(&self.merged);
        serde_json::to_string(&report).map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// **project_roots** (A8.6): the manifest-rooted sub-projects in this graph
    /// — the vocabulary for every `scope=` argument. Each record `{qname,
    /// label, ecosystem, manifest, path}`, sorted by `path` (`.` is the repo
    /// root). Pass a `label` (e.g. `@shop/web`) or a `path` as `scope`; both
    /// give the same answer. Read back out of the graph's PROJECT anchors, so
    /// a graph from `load_from_gmap` answers exactly like a fresh one.
    /// Returns a JSON array.
    fn project_roots(&self) -> PyResult<String> {
        let roots = repo_graph_engine::project_roots(&self.merged);
        serde_json::to_string(&roots).map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// **contracts** (A12): for every queue topic, the producer's and
    /// consumer's declared message type and whether they agree. Each row
    /// `{topic, topic_is_tag, pattern, producer, consumer, status, confidence,
    /// note}` where each side is `null` or `{node_id, repo_id, qname, topic,
    /// module, file, line, message_type, message_type_raw, form, window,
    /// types_seen, conflicting}`. `status` ∈ {match, mismatch, unknown}.
    /// `repo_id` is the raw `RepoId` (an xxhash of the repo path); Python has
    /// no label map for it yet. Report-only: no edge is emitted. Returns a
    /// JSON array.
    fn contracts(&self) -> PyResult<String> {
        contracts_json(&self.merged).map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// **service_map** (v6 follow-on): the architecture summary — one record
    /// per service (a manifest project root in a monorepo, with files under
    /// no root in one `(outside projects)` bucket; a top-level directory when
    /// the repo has no nested roots; one per repo when several were merged)
    /// plus the aggregated cross-service links, each
    /// labelled with its mechanism (HTTP_CALLS / QUEUE_FLOWS / GRPC_CALLS …)
    /// and the channel it travels over (route, topic, service name). Returns
    /// `{keying, services:[…], links:[…], self_links, unlocated_nodes}` as a
    /// JSON object; `keying` says which rule produced the service ids.
    ///
    /// Transport only — the keying and the link aggregation live in
    /// `repo_graph_engine::arch`, shared with `glia analyze`, so the CLI and
    /// MCP answers cannot drift apart.
    ///
    /// A graph from `load_from_gmap` carries no repo labels (they are not
    /// persisted), so its per-repo service ids fall back to `repo<id>`.
    /// Build with `generate`/`generate_many` for human-readable ids.
    fn service_map(&self) -> PyResult<String> {
        service_map_json(&self.merged, &self.repo_labels)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// **cross_stack_trace** (P3, handoff v6): follow `feature` forward across
    /// service boundaries and return the ORDERED path — each hop labeled with its
    /// `mechanism` (http/queue/grpc/call/…) and `cross_service` — in one call.
    /// Where `blast_radius` gives a ranked set, this gives the sequence: how a
    /// request flows end to end. Each hop `{depth, mechanism, cross_service,
    /// from_qname, to_qname, to_kind, to_file, to_line}`. Returns a JSON array.
    #[pyo3(signature = (feature, depth=6))]
    fn cross_stack_trace(&self, feature: &str, depth: usize) -> PyResult<String> {
        let answer = repo_graph_engine::cross_stack_trace(&self.merged, feature, depth)
            .map_err(PyValueError::new_err)?;
        serde_json::to_string(&answer).map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// **resolve** (P3, handoff v6): a failure/change signal (stacktrace, diff,
    /// test id, or `auto`-sniffed) → the ranked, LOCATED nodes it points at, in
    /// one call — the answer that `resolve_signal`→`activate`→`read×N` collapses
    /// to. Resolution order preserved; each record
    /// `{id, qname, name, kind, score, file, line}`. Returns a JSON array.
    ///
    /// `scope` (optional, default `None` = no-op; a path or a project label —
    /// see `project_roots`) filters the SEEDS before the
    /// PPR run, not the rendered result: frames resolve by file BASENAME, so an
    /// unscoped `utils.py` frame seeds every `utils.py` in the monorepo and
    /// those bogus seeds shape the scores of the real one. Scoped scores
    /// therefore legitimately differ from unscoped ones.
    #[pyo3(signature = (text, kind="auto", top_k=None, scope=None))]
    fn resolve(
        &self,
        text: &str,
        kind: &str,
        top_k: Option<usize>,
        scope: Option<&str>,
    ) -> PyResult<String> {
        let answer =
            repo_graph_engine::resolve_signal_located(&self.merged, text, kind, top_k, scope);
        serde_json::to_string(&answer).map_err(|e| PyValueError::new_err(e.to_string()))
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

    /// `scope` (optional) narrows the hits to one part of a monorepo using the
    /// same `/`-boundary rule as `blast_radius`/`resolve`/`governing_docs`, so
    /// a consumer never has to re-derive a path guess in Python. Nodes with no
    /// locatable file are KEPT. A project label (see `project_roots`) works
    /// here too — it is resolved once, before the per-node filter.
    #[pyo3(signature = (pattern, scope=None))]
    fn find_nodes_by_qname(&self, pattern: &str, scope: Option<&str>) -> Vec<u64> {
        let scope = scope.map(|s| repo_graph_engine::resolve_scope(&self.merged, s));
        self.merged
            .qnames_containing(pattern)
            .into_iter()
            .filter(|id| repo_graph_engine::node_in_scope(&self.merged, *id, scope.as_deref()))
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
    // Delegates to the shared escaper: the four-`replace` version this
    // replaced let every other control character below 0x20 through raw, so a
    // single stray 0x01 in one symbol name or file path made `json.loads`
    // raise `Invalid control character` for the entire graph (audit #16).
    repo_graph_projection_text::escape_json_string(s)
}

// ============================================================================
// Module functions
// ============================================================================

/// Build the graph for a repo. `incremental` (default True, WP-D) reuses a
/// per-file parse cache at `<repo>/.ai/repo-graph/parse_cache.bin` so unchanged
/// files skip tree-sitter; the result is identical to a clean build. Pass
/// `incremental=False` to force a full reparse (this also deletes the sidecar,
/// so the next incremental build starts cold).
#[pyfunction]
#[pyo3(signature = (repo_path, incremental=true))]
fn generate(repo_path: &str, incremental: bool) -> PyResult<PyGraph> {
    let result = if incremental {
        repo_graph_engine::generate_one_incremental(repo_path)
    } else {
        // An explicit clean build also discards the sidecar — otherwise the
        // next default-on build would reuse the cache the user was escaping.
        if let Err(e) = repo_graph_engine::ParseCache::purge(repo_path) {
            eprintln!("warning: could not remove parse cache: {e}");
        }
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
    if !result.parse_errors.is_empty() {
        eprintln!(
            "[parse] {} file(s) failed to parse (see PyGraph.parse_errors)",
            result.parse_errors.len()
        );
    }
    Ok(PyGraph {
        merged: result.merged,
        parse_errors: result.parse_errors,
        repo_labels: result.repo_labels,
    })
}

/// Generate a single MergedGraph from multiple repo paths. Each path becomes
/// its own RepoId, so cross-graph resolvers (HttpStack, DbResolver, etc.) fire
/// across the boundary. Used for substrate eval where one wants to validate
/// that two unrelated services pair correctly under the resolver layer.
///
/// `incremental=True` (WP-D, A1.4) gives each path its own per-file parse cache
/// at `<repo>/.ai/repo-graph/parse_cache.bin`, so unchanged files skip
/// tree-sitter; the result is identical to a clean build. The default is
/// False here — unlike `generate` — BECAUSE the substrate-gap eval grades
/// every multi-dir fixture through this entry point and must stay hermetic
/// (`GLIA_NO_PERSIST=1` does not gate the parse-cache sidecar). False never
/// touches an existing sidecar: this function has never written one by
/// default, so there is nothing to escape from.
#[pyfunction]
#[pyo3(signature = (repo_paths, incremental=false))]
fn generate_many(repo_paths: Vec<String>, incremental: bool) -> PyResult<PyGraph> {
    let result = if incremental {
        repo_graph_engine::generate_many_incremental(&repo_paths)
    } else {
        engine_generate_many(&repo_paths)
    }
    .map_err(PyValueError::new_err)?;
    if !result.parse_errors.is_empty() {
        eprintln!(
            "[parse] {} file(s) failed to parse (see PyGraph.parse_errors)",
            result.parse_errors.len()
        );
    }
    Ok(PyGraph {
        merged: result.merged,
        parse_errors: result.parse_errors,
        repo_labels: result.repo_labels,
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
    Ok(PyGraph {
        merged,
        parse_errors: Vec::new(),
        // Not persisted at FORMAT_VERSION=1: `service_map` degrades to
        // `repo<id>` ids for this graph. Documented on the method.
        repo_labels: std::collections::BTreeMap::new(),
    })
}

/// Conventional gmap directory path for a repo: `<repo>/.ai/repo-graph`.
/// The Python wrapper uses this to know where to look for a cached graph.
#[pyfunction]
fn default_gmap_dir(repo_path: &str) -> String {
    store_default_gmap_dir(Path::new(repo_path))
        .to_string_lossy()
        .into_owned()
}

/// Is the cached gmap at `gmap_dir` older than anything under `repo_path` that
/// the builder would read? Used by the wrapper to decide between load and
/// regenerate. Returns `true` if the gmap is missing entirely, or if it was
/// written by a different engine build.
///
/// Directory gating is shared with the builder's walk, so the scan skips
/// exactly what the parse skips: VCS/editor metadata (`.git`, `.hg`, `.svn`,
/// `.idea`, `.vscode`), the gmap dir (`.ai/`), dependency and build-output
/// trees (`node_modules`, `vendor`, `bower_components`, `.venv`,
/// `site-packages`, `target`, `dist`, `build`, `out`, `__pycache__`, `.cache`,
/// `.next`, `.nuxt`, `.angular`, `coverage`), plain directory entries in the
/// repo's top-level `.gitignore`, and copied web bundles. Churn confined to
/// one of those no longer forces a regenerate. A collapsed directory is still
/// one REGION node, so a gated directory whose OWN mtime is newer than the
/// manifest (an entry created or removed directly inside it) does mark the
/// gmap stale.
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

/// Build identity of THIS wheel: `<release>+p<16 hex>`, where the hex half is a
/// content hash of every graph-shaping source file (repo_graph_stamp). Two
/// wheels with the same `version()` but different `build_stamp()` contain
/// different parsers — which is how you catch a stale `.so` that maturin
/// repackaged without rebuilding. `version()` above stays the bare release:
/// the repo-graph wrapper and bench/substrate-gap/run.py read it.
#[pyfunction]
fn build_stamp() -> &'static str {
    repo_graph_engine::BUILD_STAMP
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
    m.add_function(wrap_pyfunction!(build_stamp, m)?)?;
    m.add_class::<PyGraph>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A1.6: `build_stamp()` extends `version()` — same release, plus the
    /// parser hash — so a wrapper that reads `version()` keeps working and a
    /// stale `.so` shows up as a stamp that did not move across a rebuild.
    #[test]
    fn build_stamp_is_the_release_plus_the_parser_stamp() {
        let stamp = build_stamp();
        assert_eq!(stamp, repo_graph_engine::BUILD_STAMP);
        let hex = stamp
            .strip_prefix(version())
            .and_then(|rest| rest.strip_prefix("+p"))
            .unwrap_or_default();
        assert_eq!(hex, repo_graph_engine::PARSER_STAMP, "build_stamp() = {stamp:?}");
        assert_eq!(hex.len(), 16, "build_stamp() = {stamp:?}");
    }

    /// A9.3: the binding is transport only, so the thing worth pinning is the
    /// wiring — the engine's `ServiceMap` serialises and the shape Python
    /// receives is the object the docstring promises. (Content correctness
    /// lives in the engine's own `arch_service_map` test; duplicating it here
    /// would only drift.)
    #[test]
    fn service_map_returns_the_documented_json_object() {
        let merged = MergedGraph::new(Vec::new());
        let json = service_map_json(&merged, &std::collections::BTreeMap::new())
            .expect("service_map serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let obj = v.as_object().expect("a JSON object, not an array");
        for key in [
            "keying",
            "services",
            "links",
            "self_links",
            "unlocated_nodes",
        ] {
            assert!(obj.contains_key(key), "missing `{key}` in {json}");
        }
        assert!(obj["services"].is_array());
        assert!(obj["links"].is_array());
    }

    /// A12.3: `contracts()` is transport only — pin the wiring (a real build's
    /// rows reach Python as a JSON array of the documented row shape). The
    /// verdict logic is covered by the engine's `message_contracts` tests.
    #[test]
    fn contracts_returns_the_documented_json_array() {
        let empty = contracts_json(&MergedGraph::new(Vec::new())).expect("serialises");
        assert_eq!(empty, "[]");

        let root = std::env::temp_dir().join(format!("glia-a12-3-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let publisher = "package svc\n\nimport \"github.com/nats-io/nats.go\"\n\n\
            func Publish(nc *nats.Conn) error {\n\treturn nc.Publish(\"orders\", nil)\n}\n";
        std::fs::write(root.join("publisher.go"), publisher).expect("write fixture");
        let built = generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let merged = built.expect("build").merged;

        let json = contracts_json(&merged).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let rows = v.as_array().expect("a JSON array, not an object");
        let first = rows
            .first()
            .and_then(|r| r.as_object())
            .expect("one row per topic");
        for key in [
            "topic",
            "topic_is_tag",
            "producer",
            "consumer",
            "status",
            "note",
        ] {
            assert!(first.contains_key(key), "missing `{key}` in {json}");
        }
        assert_eq!(first["topic"], "orders", "{json}");
        assert!(
            first["consumer"].is_null(),
            "a producer-only topic is one-sided: {json}"
        );
    }
}
