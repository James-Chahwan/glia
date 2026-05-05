//! node_summaries — per-activated-node summary strings for Path B pooled
//! vectors.
//!
//! Replaces scratch/latent/out/node_summaries.py. Rust-native: reads CODE
//! cells directly off nodes (every Python METHOD/FUNCTION/CLASS/MODULE carries
//! one as of the current parser), so pooled vectors embed real source instead
//! of reconstructed stubs.
//!
//! Output: JSON list of {id, qname, score, summary}, sorted by PPR order from
//! the input seeds file. Consumed by `run_pathB` as the pool that gets mean-
//! pooled into 4096-dim vectors.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use repo_graph_code_domain::node_kind;
use repo_graph_core::Node;
use repo_graph_projection_text::driver_utils::{
    build_repo_graph, extract_code_cell, node_by_qname_index, read_json, write_json,
};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(about = "Build per-node summaries (real source via CODE cell) for pooling")]
struct Args {
    #[arg(long)]
    src: PathBuf,

    #[arg(long)]
    seeds: PathBuf,

    #[arg(long)]
    out: PathBuf,

    /// Cap each summary at this many chars so huge functions don't dominate
    /// a single pooled vector. 0 disables the cap.
    #[arg(long, default_value_t = 2000)]
    max_chars: usize,

    #[arg(long, default_value = "summaries")]
    repo_canonical: String,
}

#[derive(Deserialize)]
struct SeedsFile {
    activated: Vec<(String, f64)>,
}

#[derive(Serialize)]
struct SummaryEntry {
    id: u64,
    qname: String,
    score: f64,
    summary: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let graph = build_repo_graph(&args.src, &args.repo_canonical)?;
    let seeds: SeedsFile = read_json(&args.seeds)?;

    if seeds.activated.is_empty() {
        eprintln!("[summaries] seeds file has no activated entries");
        std::process::exit(3);
    }

    let qname_to_node = node_by_qname_index(&graph);

    let mut out: Vec<SummaryEntry> = Vec::new();
    let mut with_source = 0usize;
    let mut without_source = 0usize;
    let mut skipped = 0usize;

    for (qname, score) in &seeds.activated {
        let node = match qname_to_node.get(qname.as_str()) {
            Some(n) => *n,
            None => {
                eprintln!("[summaries] skip (no node match): {qname}");
                skipped += 1;
                continue;
            }
        };
        let summary = build_summary(node, graph.nav.kind_by_id.get(&node.id).copied(), qname, args.max_chars);
        if summary.has_source {
            with_source += 1;
        } else {
            without_source += 1;
        }
        out.push(SummaryEntry {
            id: node.id.0,
            qname: qname.clone(),
            score: *score,
            summary: summary.text,
        });
    }

    eprintln!(
        "[summaries] wrote {} entries ({} with CODE source, {} without, {} skipped)",
        out.len(),
        with_source,
        without_source,
        skipped,
    );
    write_json(&args.out, &out)?;
    eprintln!("[write] {}", args.out.display());
    Ok(())
}

struct Summary {
    text: String,
    has_source: bool,
}

fn build_summary(
    node: &Node,
    kind: Option<repo_graph_core::NodeKindId>,
    qname: &str,
    max_chars: usize,
) -> Summary {
    if let Some(source) = extract_code_cell(node) {
        let capped = if max_chars > 0 && source.len() > max_chars {
            format!("{}\n# ... (truncated)", &source[..max_chars])
        } else {
            source.to_string()
        };
        return Summary {
            text: format!("# {qname}\n{capped}"),
            has_source: true,
        };
    }

    // Fallback for nodes without CODE cells (synthetic endpoints, ATTRIBUTE
    // derived nodes). Keep the old stub shape so pooling still sees something
    // in real-code embedding space.
    let name = qname.rsplit("::").next().unwrap_or("?");
    let fallback = match kind {
        Some(k) if k == node_kind::METHOD => format!("def {name}(self, *args, **kwargs): ...  # {qname}"),
        Some(k) if k == node_kind::FUNCTION => format!("def {name}(*args, **kwargs): ...  # {qname}"),
        Some(k) if k == node_kind::CLASS => format!("class {name}: ...  # {qname}"),
        Some(k) if k == node_kind::MODULE => format!("import {}", qname.replace("::", ".")),
        _ => format!("# {qname}"),
    };
    Summary {
        text: fallback,
        has_source: false,
    }
}
