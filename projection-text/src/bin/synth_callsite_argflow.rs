//! synth_callsite_argflow — usage-typed polymorphism cell synthesiser.
//!
//! For each polymorphic method simple-name (defined on ≥2 classes in the
//! graph), scan every activated METHOD/FUNCTION body via AST re-walk and
//! collect calls matching that simple-name. Record (caller_class, receiver,
//! positional-args) per call; when ≥2 distinct caller-classes (or receivers)
//! are observed, emit a `callsite-argflow` cell that enumerates the
//! call-site/receiver polymorphism. Appends the cells to an existing
//! summaries-hybrid JSON so the Path B latent pipeline picks them up.
//!
//! Rationale: marshmallow's `_bind_to_schema` is called from `List`, `Tuple`,
//! and `Schema._bind_field`; each passes `self` (different class every time)
//! as the `schema` argument to the inner field's bind. The graph's CALLS
//! edges don't carry arg info and many receivers fall into `ComplexReceiver`
//! at resolve time, so the polymorphism facts aren't graph-edge-derivable.
//! This bin re-walks the AST for the activated methods' bodies to recover the
//! facts as auto-synthesised prose — candidate for closing the B2.2 oracle
//! bullet automatically.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use repo_graph_core::NodeId;
use repo_graph_projection_text::driver_utils::{build_repo_graph, reverse_qname_index};
use repo_graph_projection_text::synth_callsite_argflow::run as run_callsite_argflow;
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(about = "Synthesise callsite-argflow cells for usage-typed polymorphism")]
struct Args {
    /// Repo root to walk for `*.py` files.
    #[arg(long)]
    src: PathBuf,

    /// Seeds JSON: top-level `activated: [[qname, score], ...]`.
    #[arg(long)]
    seeds: PathBuf,

    /// Existing summaries-hybrid JSON to append to.
    #[arg(long)]
    summaries: PathBuf,

    /// Output path for the augmented summaries JSON.
    #[arg(long)]
    out: PathBuf,

    /// Canonical repo identifier.
    #[arg(long, default_value = "synth-callsite-argflow")]
    repo_canonical: String,

    /// First synthetic cell id. Picks a high water mark to avoid collision with
    /// existing summary ids.
    #[arg(long, default_value_t = 20_000_000)]
    id_start: u64,
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

    let graph = build_repo_graph(&args.src, &args.repo_canonical)?;

    let seeds_bytes = fs::read(&args.seeds)
        .with_context(|| format!("read {}", args.seeds.display()))?;
    let seeds: SeedsFile = serde_json::from_slice(&seeds_bytes).context("parse seeds JSON")?;
    eprintln!("[seeds] {} activated qnames", seeds.activated.len());

    let qname_to_id = reverse_qname_index(&graph);
    let mut activated: Vec<NodeId> = Vec::new();
    for (qname, _) in &seeds.activated {
        if let Some(&id) = qname_to_id.get(qname.as_str()) {
            activated.push(id);
        }
    }
    eprintln!("[resolve] {} activated qnames matched", activated.len());

    // All graph-walking + cell synthesis logic now lives in
    // `synth_callsite_argflow::run` so glia-3d's Inject scene calls the
    // same code in-process. Bin handles only the disk I/O.
    let cells = run_callsite_argflow(&graph, &activated, args.id_start);
    let emitted: Vec<SummaryEntry> = cells
        .into_iter()
        .map(|c| SummaryEntry {
            id: c.id,
            qname: c.qname,
            score: c.score,
            summary: c.summary,
        })
        .collect();
    eprintln!("[synth] {} callsite-argflow cells emitted", emitted.len());

    let existing: Vec<SummaryEntry> = serde_json::from_slice(
        &fs::read(&args.summaries)
            .with_context(|| format!("read {}", args.summaries.display()))?,
    )
    .context("parse summaries JSON")?;
    let mut out: Vec<SummaryEntry> = existing;
    out.extend(emitted);
    fs::write(&args.out, serde_json::to_vec_pretty(&out)?)
        .with_context(|| format!("write {}", args.out.display()))?;
    eprintln!("[write] {}", args.out.display());

    Ok(())
}
