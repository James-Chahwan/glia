//! Shared helpers for Python-domain driver bins (seeds, node_summaries,
//! synth_key_symbols, synth_composition). Behind `driver` feature gate.
//!
//! All bins follow the same shape: walk a Python source tree, build an
//! in-memory `RepoGraph`, read a seeds JSON, do something, write JSON.
//! These helpers factor out the walk + build + qname-index + cell-reads.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use repo_graph_code_domain::cell_type;
use repo_graph_core::{CellPayload, Node, NodeId, RepoId};
use repo_graph_graph::{RepoGraph, build_python};
use repo_graph_parser_python::{FileParse, parse_file};
use walkdir::WalkDir;

/// Walk a directory tree, parse every `.py` file, collect `FileParse`s.
/// qnames derive from paths relative to `src` (e.g. `src/marshmallow/fields.py`
/// → `src::marshmallow::fields`).
pub fn parse_tree(src: &Path, repo: RepoId) -> Result<Vec<FileParse>> {
    let mut parses = Vec::new();
    for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().map_or(true, |ext| ext != "py") {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(src)
            .unwrap_or(entry.path())
            .to_path_buf();
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let module_qname = rel_str.trim_end_matches(".py").replace('/', "::");
        let source = match fs::read_to_string(entry.path()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[parse] skip {}: {e}", entry.path().display());
                continue;
            }
        };
        match parse_file(&source, &rel_str, &module_qname, repo) {
            Ok(fp) => parses.push(fp),
            Err(e) => eprintln!("[parse] skip {}: {e:?}", entry.path().display()),
        }
    }
    Ok(parses)
}

/// Full pipeline: parse a repo + build the Python graph.
pub fn build_repo_graph(src: &Path, repo_canonical: &str) -> Result<RepoGraph> {
    let repo = RepoId::from_canonical(repo_canonical);
    let parses = parse_tree(src, repo)?;
    eprintln!("[parse] {} Python files", parses.len());
    let graph = build_python(repo, parses).map_err(|e| anyhow!("build_python: {e:?}"))?;
    eprintln!("[graph] {} nodes, {} edges", graph.nodes.len(), graph.edges.len());
    Ok(graph)
}

/// qname → NodeId lookup for resolving seed/activated qnames back to graph nodes.
/// Determinism: iterate `graph.nodes` (Vec, deterministic) NOT `qname_by_id`
/// (HashMap with randomized hasher). On duplicate qnames "last insert wins"
/// then reflects node-Vec order, which is stable across runs. Same fix already
/// applied to `node_by_qname_index` below — this function had the same bug.
pub fn reverse_qname_index(graph: &RepoGraph) -> HashMap<&str, NodeId> {
    let mut out: HashMap<&str, NodeId> = HashMap::new();
    for n in &graph.nodes {
        if let Some(qn) = graph.nav.qname_by_id.get(&n.id) {
            out.insert(qn.as_str(), n.id);
        }
    }
    out
}

/// qname → &Node lookup. Useful when callers need cells on top of ids.
pub fn node_by_qname_index(graph: &RepoGraph) -> HashMap<&str, &Node> {
    // Iterate graph.nodes (Vec, deterministic) not qname_by_id (HashMap with
    // randomized hasher). On duplicate qnames, "last insert wins" then reflects
    // node-Vec order — stable across runs — instead of hasher-dependent order.
    let mut out: HashMap<&str, &Node> = HashMap::new();
    for n in &graph.nodes {
        if let Some(qn) = graph.nav.qname_by_id.get(&n.id) {
            out.insert(qn.as_str(), n);
        }
    }
    out
}

/// Pull the CODE cell text off a node, if present. Python METHOD/FUNCTION/CLASS
/// nodes carry a CODE cell with the full source slice as set by
/// `parsers/code/python/src/lib.rs:build_cells`.
pub fn extract_code_cell(node: &Node) -> Option<&str> {
    for cell in &node.cells {
        if cell.kind == cell_type::CODE {
            if let CellPayload::Text(t) = &cell.payload {
                return Some(t.as_str());
            }
        }
    }
    None
}

/// Pull the POSITION cell JSON off a node, if present. Shape:
/// `{"file":"...","start_line":N,"end_line":N}`.
pub fn extract_position_cell(node: &Node) -> Option<&str> {
    for cell in &node.cells {
        if cell.kind == cell_type::POSITION {
            if let CellPayload::Json(j) = &cell.payload {
                return Some(j.as_str());
            }
        }
    }
    None
}

/// Read a JSON file, bubble up a helpful error including the path.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse JSON {}", path.display()))
}

/// Write a struct as pretty JSON.
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

// ============================================================================
// Chain-depth helpers (consume synth_call_chain JSON)
// ============================================================================

#[derive(serde::Deserialize)]
struct ChainEntryDe {
    qname: String,
    depth: usize,
}

#[derive(serde::Deserialize)]
struct AnchorChainDe {
    #[allow(dead_code)]
    anchor_label: String,
    anchor_qname: Option<String>,
    methods: Vec<ChainEntryDe>,
}

#[derive(serde::Deserialize)]
struct ChainOutputDe {
    #[allow(dead_code)]
    max_depth: usize,
    chains: Vec<AnchorChainDe>,
}

/// Read a synth_call_chain JSON and reduce to `qname → min CALLS-chain depth`.
/// Anchors enter at depth 0; their BFS-reachable methods at their reported depth.
pub fn load_chain_depths(path: &Path) -> Result<HashMap<String, usize>> {
    let parsed: ChainOutputDe = read_json(path)?;
    let mut out: HashMap<String, usize> = HashMap::new();
    for chain in parsed.chains {
        if let Some(anchor) = chain.anchor_qname {
            insert_min(&mut out, anchor, 0);
        }
        for m in chain.methods {
            insert_min(&mut out, m.qname, m.depth);
        }
    }
    Ok(out)
}

fn insert_min(map: &mut HashMap<String, usize>, qname: String, depth: usize) {
    map.entry(qname)
        .and_modify(|d| {
            if depth < *d {
                *d = depth;
            }
        })
        .or_insert(depth);
}

/// Score boost for a chain depth lookup. Anchor and direct callees (depth 0|1)
/// get the strongest boost; weakens with reach. `None` means qname not in chain.
///
/// Weights cut 4× (20/10/5 → 5/3/1) after marshmallow-1359 chain-smoke regression:
/// +20 for depth 0|1 was strong enough to dominate body_boost (max +15) + tail_boost
/// (+10), shoving the v8 load-bearing `Mapping::_bind_to_schema` cell out of #5 in
/// favor of `DateTime::__init__`. Reduced magnitude makes chain a tiebreaker rather
/// than a dominator while still ranking test-reachable methods above PPR-only ties.
pub fn chain_depth_boost(depth: Option<usize>) -> f64 {
    match depth {
        None => 0.0,
        Some(0) | Some(1) => 5.0,
        Some(2) => 3.0,
        Some(_) => 1.0,
    }
}
