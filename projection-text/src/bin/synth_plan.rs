//! synth_plan — the four-bin synth chain as ONE `ActivationPlan` over ONE
//! graph load (LD.12e).
//!
//! The chain was four processes exchanging JSON, three of them rebuilding
//! the graph: `synth_composition` (A+ access paths, appended to the
//! summaries) → `synth_callsite_argflow` (call-site arg-flow, appended) →
//! `synth_key_symbols` (source_cells.json) → `synth_derived_notes` (the
//! `## Derived notes` block). This bin parses `--src` once, resolves the
//! seeds' `activated` qnames into the ranked view (seeds order, seed
//! scores), loads every `--summaries` entry onto it as a `summary` cell, and
//! runs `AccessPathSynth` → `CallsiteArgflowSynth` → `KeySymbolsSynth` →
//! `DerivedNotesSynth` through one `ActivationPlan::synthesize`. Each hook
//! sees every earlier hook's cells, in emission order, so the output equals
//! the chain's when every bin gets the same `--repo-canonical`.
//!
//! Writes into `--out-dir`:
//! * `summaries-aplus.json`: the summaries, then the access-path and
//!   call-site cells, as `[{id, qname, score, summary}]` (the chain's
//!   `synth_callsite_argflow --out`);
//! * `source_cells.json`: the key-symbols cells (`synth_key_symbols --out`);
//! * `derived_notes.md`: the block, empty when no bullet fires
//!   (`synth_derived_notes --out`).
//!
//! Flags are the union of the four bins', with their defaults; the chain's
//! `--repo-canonical` defaults differ bin to bin, so this bin's is its own.
//! Prints `[synth] plan hooks=[access_path,callsite_argflow,key_symbols,
//! derived_notes] cells=N` (N = the cells the plan added) after the plan runs.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use glia_activation::ActivationConfig;
use glia_activation::plan::{ActivatedView, ActivationPlan, SynthCell};
use glia_core::NodeId;
use glia_graph::RepoGraph;
use glia_projection_text::driver_utils::{
    build_repo_graph, load_chain_depths, read_json, reverse_qname_index, write_json,
};
use glia_projection_text::hooks::{
    ACCESS_PATH, AccessPathSynth, CALLSITE_ARGFLOW, CallsiteArgflowSynth,
};
use glia_projection_text::research::derived_notes::{DERIVED_NOTES, DerivedNotesSynth};
use glia_projection_text::research::key_symbols::{
    KEY_SYMBOLS, KeySymbolsSynth, SUMMARY, TestPatchFacts, cell_attr,
};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(about = "Run the access-path, call-site, key-symbols and derived-notes synth hooks in one plan")]
struct Args {
    /// Repo root to walk for `*.py` files, parsed once.
    #[arg(long)]
    src: PathBuf,

    /// Seeds JSON: `activated: [[qname, score], ...]`, plus the optional
    /// `class_seeds`, `test_patch_facts` and `issue_anchored_qnames` the
    /// key-symbols ranker reads.
    #[arg(long)]
    seeds: PathBuf,

    /// Existing summaries-hybrid JSON (`[{id, qname, score, summary}, ...]`):
    /// loaded onto the view before the plan runs, first in
    /// summaries-aplus.json.
    #[arg(long)]
    summaries: PathBuf,

    /// Issue / problem-statement text file (key symbols and derived notes).
    #[arg(long)]
    issue: PathBuf,

    /// Optional SWE-bench `test_patch` (key symbols' class-seed mentions).
    #[arg(long)]
    test_patch: Option<PathBuf>,

    /// Optional CALLS-chain JSON from `synth_call_chain` (key symbols'
    /// chain-depth boost).
    #[arg(long)]
    chain: Option<PathBuf>,

    /// Access paths: BFS hop cap, including the terminal data-attribute hop.
    #[arg(long, default_value_t = 3)]
    max_hops: usize,

    /// Key symbols: selections before chain grounding.
    #[arg(long, default_value_t = 5)]
    top_k: usize,

    /// Key symbols: how many A+ cells to scan for backtick intermediates.
    #[arg(long, default_value_t = 5)]
    aplus_scan: usize,

    /// Key symbols: max chain-grounding cells appended beyond `top_k`.
    #[arg(long, default_value_t = 5)]
    chain_grounding_cap: usize,

    /// Key symbols: cap each emitted source cell at this many chars.
    #[arg(long, default_value_t = 3000)]
    max_chars: usize,

    /// Call-site arg-flow: the first cell id, a high-water mark above the
    /// summary ids.
    #[arg(long, default_value_t = 20_000_000)]
    id_start: u64,

    /// Canonical repo identifier for the RepoId hash. Pass the same value the
    /// chain's bins got to reproduce their output.
    #[arg(long, default_value = "synth-plan")]
    repo_canonical: String,

    /// Directory for summaries-aplus.json, source_cells.json and
    /// derived_notes.md; created when missing.
    #[arg(long)]
    out_dir: PathBuf,
}

#[derive(Deserialize)]
struct SeedsFile {
    activated: Vec<(String, f64)>,
    #[serde(default)]
    class_seeds: Vec<String>,
    #[serde(default)]
    test_patch_facts: TestPatchFacts,
    #[serde(default)]
    issue_anchored_qnames: Vec<String>,
}

/// One summaries-hybrid entry, read from `--summaries` and written to
/// summaries-aplus.json.
#[derive(Serialize, Deserialize)]
struct SummaryEntry {
    id: u64,
    qname: String,
    score: f64,
    summary: String,
}

/// One source_cells.json row, the shape `synth_key_symbols` writes.
#[derive(Serialize)]
struct SourceCell {
    qname: String,
    file: String,
    source: String,
    rank: usize,
    reason: String,
    start_line: usize,
    end_line: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let graph = build_repo_graph(&args.src, &args.repo_canonical)?;
    let seeds: SeedsFile = read_json(&args.seeds)?;
    let summaries: Vec<SummaryEntry> = read_json(&args.summaries)?;
    let issue = fs::read_to_string(&args.issue)
        .with_context(|| format!("read {}", args.issue.display()))?;
    let test_patch = match &args.test_patch {
        Some(p) => fs::read_to_string(p).unwrap_or_default(),
        None => String::new(),
    };
    let chain_depths: HashMap<String, usize> = match &args.chain {
        Some(p) => load_chain_depths(p)?,
        None => HashMap::new(),
    };

    // (id, seed score) in seeds order: the ranked view every hook reads.
    let qname_to_id = reverse_qname_index(&graph);
    let ranked: Vec<(NodeId, f64)> = seeds
        .activated
        .iter()
        .filter_map(|(qname, score)| qname_to_id.get(qname.as_str()).map(|&id| (id, *score)))
        .collect();
    eprintln!("[resolve] {}/{} qnames matched", ranked.len(), seeds.activated.len());

    let access = AccessPathSynth { max_hops: args.max_hops };
    let callsite = CallsiteArgflowSynth { id_start: args.id_start };
    let key_symbols = KeySymbolsSynth {
        activated: seeds.activated,
        class_seeds: seeds.class_seeds,
        test_patch_facts: seeds.test_patch_facts,
        issue_anchored_qnames: seeds.issue_anchored_qnames,
        issue: issue.clone(),
        test_patch,
        chain_depths,
        top_k: args.top_k,
        aplus_scan: args.aplus_scan,
        chain_grounding_cap: args.chain_grounding_cap,
        max_chars: args.max_chars,
    };
    let notes = DerivedNotesSynth { issue };
    let plan = ActivationPlan::<RepoGraph>::new(ActivationConfig::default())
        .synth(&access)
        .synth(&callsite)
        .synth(&key_symbols)
        .synth(&notes);

    let mut view = ActivatedView::from_ranked(ranked);
    view.synth.extend(summaries.into_iter().map(|s| SynthCell {
        hook: SUMMARY,
        id: s.id,
        key: s.qname,
        anchor: None,
        text: s.summary,
        score: s.score,
        attrs: Vec::new(),
    }));
    let preloaded = view.synth.len();
    plan.synthesize(&graph, &mut view);
    eprintln!(
        "[synth] plan hooks=[{}] cells={}",
        view.applied.join(","),
        view.synth.len() - preloaded
    );

    fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create {}", args.out_dir.display()))?;

    let aplus: Vec<SummaryEntry> = view
        .synth
        .iter()
        .filter(|c| [SUMMARY, ACCESS_PATH, CALLSITE_ARGFLOW].contains(&c.hook))
        .map(|c| SummaryEntry {
            id: c.id,
            qname: c.key.clone(),
            score: c.score,
            summary: c.text.clone(),
        })
        .collect();
    write_out_json(&args.out_dir.join("summaries-aplus.json"), &aplus)?;

    let source_cells: Vec<SourceCell> =
        view.cells_of(KEY_SYMBOLS).map(source_cell).collect::<Result<_>>()?;
    write_out_json(&args.out_dir.join("source_cells.json"), &source_cells)?;

    let block = view.cells_of(DERIVED_NOTES).next().map_or("", |c| c.text.as_str());
    let notes_path = args.out_dir.join("derived_notes.md");
    fs::write(&notes_path, block).with_context(|| format!("write {}", notes_path.display()))?;
    eprintln!("[write] {}", notes_path.display());
    Ok(())
}

fn write_out_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_json(path, value)?;
    eprintln!("[write] {}", path.display());
    Ok(())
}

/// A key_symbols cell as the source_cells.json row `synth_key_symbols` writes.
fn source_cell(cell: &SynthCell) -> Result<SourceCell> {
    Ok(SourceCell {
        qname: cell.key.clone(),
        file: attr(cell, "file")?.to_string(),
        source: cell.text.clone(),
        rank: num_attr(cell, "rank")?,
        reason: attr(cell, "reason")?.to_string(),
        start_line: num_attr(cell, "start_line")?,
        end_line: num_attr(cell, "end_line")?,
    })
}

fn attr<'a>(cell: &'a SynthCell, name: &str) -> Result<&'a str> {
    cell_attr(cell, name).ok_or_else(|| anyhow!("key_symbols cell {} has no `{name}`", cell.key))
}

fn num_attr(cell: &SynthCell, name: &str) -> Result<usize> {
    attr(cell, name)?
        .parse()
        .with_context(|| format!("key_symbols cell {}: `{name}` is not a number", cell.key))
}
