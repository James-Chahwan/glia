//! synth_composition — A+ access-path cell synthesizer driver.
//!
//! Walks a Python source tree, builds an in-memory `RepoGraph`, resolves an
//! activated qname set from a seeds JSON to NodeIds, runs `synth_paths` +
//! `render_cells` from the `composition` module, and appends the resulting
//! cells to an existing summaries-hybrid JSON (shape: `[{id, qname, score,
//! summary}, ...]`) used downstream by the Path B latent-injection harness.
//!
//! Domain-general: no marshmallow-specific logic. Works on any Python repo
//! whose seeds JSON stores `activated: [[qname, score], ...]`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use repo_graph_core::{NodeId, RepoId};
use repo_graph_graph::{RepoGraph, build_python};
use repo_graph_parser_python::{FileParse, parse_file};
use repo_graph_projection_text::composition::{render_cells, synth_paths};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(about = "Synthesise A+ access-path composition cells and append to summaries JSON")]
struct Args {
    /// Repo root to walk for `*.py` files. qnames derive from paths relative
    /// to this directory (e.g. `src/marshmallow/fields.py` →
    /// `src::marshmallow::fields`).
    #[arg(long)]
    src: PathBuf,

    /// Seeds JSON path. Must contain top-level `activated: [[qname, score],
    /// ...]`.
    #[arg(long)]
    seeds: PathBuf,

    /// Existing summaries-hybrid JSON to read and append to. Shape:
    /// `[{id, qname, score, summary}, ...]`.
    #[arg(long)]
    summaries: PathBuf,

    /// Output path for the augmented summaries JSON.
    #[arg(long)]
    out: PathBuf,

    /// BFS hop cap (including the terminal data-attribute hop).
    #[arg(long, default_value_t = 3)]
    max_hops: usize,

    /// Canonical repo identifier for the synthetic RepoId hash.
    #[arg(long, default_value = "synth-composition")]
    repo_canonical: String,
}

#[derive(Deserialize)]
struct SeedsFile {
    activated: Vec<(String, f64)>,
}

#[derive(Serialize, Deserialize, Clone)]
struct SummaryEntry {
    id: u64,
    qname: String,
    score: f64,
    summary: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let repo = RepoId::from_canonical(&args.repo_canonical);

    let parses = parse_tree(&args.src, repo)?;
    eprintln!("[parse] {} Python files", parses.len());

    let graph = build_python(repo, parses).map_err(|e| anyhow!("build_python: {e:?}"))?;
    eprintln!("[graph] {} nodes, {} edges", graph.nodes.len(), graph.edges.len());

    let seeds: SeedsFile = serde_json::from_slice(
        &fs::read(&args.seeds).with_context(|| format!("read {}", args.seeds.display()))?,
    )
    .context("parse seeds JSON")?;
    eprintln!("[seeds] {} activated qnames", seeds.activated.len());

    let qname_to_id = reverse_qname_index(&graph);
    let mut activated: Vec<NodeId> = Vec::new();
    let mut scores: Vec<(NodeId, f64)> = Vec::new();
    let mut missed: Vec<&str> = Vec::new();
    for (qname, score) in &seeds.activated {
        if let Some(&id) = qname_to_id.get(qname.as_str()) {
            activated.push(id);
            scores.push((id, *score));
        } else {
            missed.push(qname.as_str());
        }
    }
    eprintln!(
        "[resolve] {}/{} qnames matched ({} missed)",
        activated.len(),
        seeds.activated.len(),
        missed.len()
    );
    if !missed.is_empty() {
        let show = missed.iter().take(5).copied().collect::<Vec<_>>().join(", ");
        eprintln!("[resolve] first unmatched: {show}");
    }

    {
        use repo_graph_code_domain::node_kind;
        let total_attrs = graph
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ATTRIBUTE)
            .count();
        let activated_set: std::collections::HashSet<NodeId> = activated.iter().copied().collect();
        let activated_attrs = graph
            .nav
            .kind_by_id
            .iter()
            .filter(|(id, k)| **k == node_kind::ATTRIBUTE && activated_set.contains(id))
            .count();
        eprintln!("[diag] ATTRIBUTE nodes: {} total, {} activated", total_attrs, activated_attrs);
        let opts_matches: Vec<&String> = graph
            .nav
            .qname_by_id
            .iter()
            .filter(|(id, q)| {
                graph.nav.kind_by_id.get(id).copied() == Some(node_kind::ATTRIBUTE)
                    && q.ends_with("::opts")
            })
            .map(|(_, q)| q)
            .collect();
        eprintln!("[diag] ATTRIBUTE qnames ending in ::opts: {opts_matches:?}");
    }

    let paths = synth_paths(&activated, &scores, &graph, args.max_hops);
    eprintln!("[synth] {} AccessPaths", paths.len());

    let cells = render_cells(&paths);
    eprintln!("[render] {} CompositionCells", cells.len());

    let existing: Vec<SummaryEntry> = serde_json::from_slice(
        &fs::read(&args.summaries)
            .with_context(|| format!("read {}", args.summaries.display()))?,
    )
    .context("parse summaries JSON")?;
    eprintln!("[append] {} existing + {} new = {}", existing.len(), cells.len(), existing.len() + cells.len());

    let mut out: Vec<SummaryEntry> = existing;
    out.extend(cells.into_iter().map(|c| SummaryEntry {
        id: c.id,
        qname: c.qname,
        score: c.score,
        summary: c.summary,
    }));

    fs::write(&args.out, serde_json::to_vec_pretty(&out)?)
        .with_context(|| format!("write {}", args.out.display()))?;
    eprintln!("[write] {}", args.out.display());

    Ok(())
}

fn parse_tree(src: &Path, repo: RepoId) -> Result<Vec<FileParse>> {
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
        let module_qname = rel_str
            .trim_end_matches(".py")
            .replace('/', "::");
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

fn reverse_qname_index(graph: &RepoGraph) -> HashMap<&str, NodeId> {
    // Determinism: iterate graph.nodes (Vec) not qname_by_id (HashMap) so
    // duplicate qnames resolve consistently across runs. Same pattern as
    // driver_utils::node_by_qname_index. (Synth_composition has a private
    // duplicate of this fn — keep in sync if either is updated.)
    let mut out: HashMap<&str, NodeId> = HashMap::new();
    for n in &graph.nodes {
        if let Some(qn) = graph.nav.qname_by_id.get(&n.id) {
            out.insert(qn.as_str(), n.id);
        }
    }
    out
}

