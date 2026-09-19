//! synth_key_symbols — select top-K activated METHOD/FUNCTION nodes with
//! issue-token boost + backtick-qname intermediates from A+ cell summaries;
//! emit their CODE cells as a `source_cells.json` file for the prefix
//! "## Key symbols from the repository (full code):" block.
//!
//! Why this exists: marshmallow-1359 smoke runs showed auto-prefix without a
//! source anchor fabricates isinstance-guards on non-existent methods. The
//! hand-crafted GOLD prefix carried full bodies of 5 target methods. This bin
//! reproduces that anchor automatically from graph state + issue text.
//!
//! The pass is `research::key_symbols::KeySymbolsSynth` (LD.12d): this bin
//! fills the hook from `--issue` / `--test-patch` / seeds.json / `--chain`,
//! loads every `--summaries` entry onto the view as a `summary` cell, runs
//! the one hook through `ActivationPlan::synthesize`, and writes its cells
//! back as `source_cells.json`. Prints
//! `[synth] plan hooks=[key_symbols] cells=N` after the plan runs.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use glia_activation::ActivationConfig;
use glia_activation::plan::{ActivatedView, ActivationPlan, SynthCell};
use glia_graph::RepoGraph;
use glia_projection_text::driver_utils::{
    build_repo_graph, load_chain_depths, read_json, write_json,
};
use glia_projection_text::research::key_symbols::{
    KEY_SYMBOLS, KeySymbolsSynth, SUMMARY, TestPatchFacts, cell_attr,
};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(about = "Select top-K activated methods + backtick intermediates, emit CODE cells")]
struct Args {
    #[arg(long)]
    src: PathBuf,

    #[arg(long)]
    seeds: PathBuf,

    #[arg(long)]
    issue: PathBuf,

    /// Optional SWE-bench `test_patch`. When provided, class-seed mentions
    /// inside the patch text count toward `class_match_boost` weight — tests
    /// usually instantiate the *specific* class under test (e.g.
    /// `FilePathField(path=...)` x3 vs `CharField` x0), discriminating
    /// methods of the target class from collateral classes that also share
    /// the issue surface.
    #[arg(long)]
    test_patch: Option<PathBuf>,

    /// A+ summaries JSON (from synth_composition output). Backtick-qname
    /// mentions in the top-N A+ cell summaries are pulled as extra source
    /// cells (catches `Field.root` etc. that aren't in the activated top-K
    /// but are referenced by the composition paths).
    #[arg(long)]
    summaries: PathBuf,

    #[arg(long)]
    out: PathBuf,

    #[arg(long, default_value_t = 5)]
    top_k: usize,

    /// How many A+ cells to scan for backtick intermediates.
    #[arg(long, default_value_t = 5)]
    aplus_scan: usize,

    /// G4: Max extra "chain-grounding" cells appended beyond top_k, one per
    /// named intermediate (attribute, @property accessor, target method)
    /// referenced by an AccessPath cell that mentions any top-K selection.
    #[arg(long, default_value_t = 5)]
    chain_grounding_cap: usize,

    /// Cap each emitted source cell at this many chars.
    #[arg(long, default_value_t = 3000)]
    max_chars: usize,

    /// Optional CALLS-chain JSON (from `synth_call_chain`). Methods whose qname
    /// is in the chain receive a depth-weighted score boost on top of PPR +
    /// issue-anchor scoring: depth 0|1 = +20, depth 2 = +10, depth ≥3 = +5.
    /// Lifts test-reachable symbols above PPR-only neighbours.
    #[arg(long)]
    chain: Option<PathBuf>,

    #[arg(long, default_value = "keysym")]
    repo_canonical: String,
}

#[derive(Deserialize)]
struct SeedsFile {
    activated: Vec<(String, f64)>,
    /// Tail names of CLASS-kind seeds, e.g. `["FilePathField", "Schema"]`.
    /// Emitted by `seeds` bin's class-expansion pass. Used here to up-rank
    /// methods whose enclosing class matches an issue-cited class identifier.
    /// Defaults to empty when seeds.json predates the field.
    #[serde(default)]
    class_seeds: Vec<String>,
    /// T2: structural facts mined from the test_patch by the `seeds` bin.
    /// Defaults to empty when seeds.json predates the field or no test_patch
    /// was provided.
    #[serde(default)]
    test_patch_facts: TestPatchFacts,
    /// 2026-04-28 N=67 audit: even with seeds.rs anchor delta 1.5×, source_cells
    /// still picked transaction.py for django-11039 over sqlmigrate.py because
    /// other boosts (class_match, chain, body) added ~10-20 each, dwarfing the
    /// anchor signal of ~0.05. This list is the issue-anchored qnames; we add
    /// an `anchor_priority_boost` of +25.0 to them in the ranker so they
    /// reliably win source_cells inclusion.
    #[serde(default)]
    issue_anchored_qnames: Vec<String>,
}

/// One `--summaries` entry: loaded onto the view as a [`SUMMARY`] cell.
#[derive(Deserialize)]
struct SummaryEntry {
    id: u64,
    qname: String,
    score: f64,
    summary: String,
}

#[derive(Serialize)]
struct SourceCell {
    qname: String,
    file: String,
    source: String,
    rank: usize,
    reason: String,
    /// POSITION-cell start_line of the FUNCTION/CLASS this source represents.
    /// Used by run_instance.py target_file_block picker to window the file
    /// source. Populated from extract_position_cell on the (possibly parent-
    /// promoted) effective node.
    #[serde(default)]
    start_line: usize,
    #[serde(default)]
    end_line: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let graph = build_repo_graph(&args.src, &args.repo_canonical)?;
    let seeds: SeedsFile = read_json(&args.seeds)?;
    let summaries: Vec<SummaryEntry> = read_json(&args.summaries)?;

    let issue = std::fs::read_to_string(&args.issue)?;
    let test_patch_text = match &args.test_patch {
        Some(p) => std::fs::read_to_string(p).unwrap_or_default(),
        None => String::new(),
    };

    let chain_depths: HashMap<String, usize> = match &args.chain {
        Some(p) => {
            let m = load_chain_depths(p)?;
            eprintln!("[keysym] chain JSON: {} qnames, depth histogram {:?}",
                m.len(), depth_histogram(&m));
            m
        }
        None => HashMap::new(),
    };

    let hook = KeySymbolsSynth {
        activated: seeds.activated,
        class_seeds: seeds.class_seeds,
        test_patch_facts: seeds.test_patch_facts,
        issue_anchored_qnames: seeds.issue_anchored_qnames,
        issue,
        test_patch: test_patch_text,
        chain_depths,
        top_k: args.top_k,
        aplus_scan: args.aplus_scan,
        chain_grounding_cap: args.chain_grounding_cap,
        max_chars: args.max_chars,
    };
    let plan = ActivationPlan::<RepoGraph>::new(ActivationConfig::default()).synth(&hook);
    let mut view = ActivatedView::from_ranked(Vec::new());
    view.synth.extend(summaries.into_iter().map(|s| SynthCell {
        hook: SUMMARY,
        id: s.id,
        key: s.qname,
        anchor: None,
        text: s.summary,
        score: s.score,
        attrs: Vec::new(),
    }));
    plan.synthesize(&graph, &mut view);

    let out: Vec<SourceCell> = view
        .cells_of(KEY_SYMBOLS)
        .map(source_cell)
        .collect::<Result<_>>()?;
    eprintln!("[synth] plan hooks=[{}] cells={}", view.applied.join(","), out.len());
    write_json(&args.out, &out)?;
    eprintln!("[write] {}", args.out.display());
    Ok(())
}

/// A key_symbols cell as the `source_cells.json` row it was before the hook.
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

fn depth_histogram(map: &HashMap<String, usize>) -> Vec<(usize, usize)> {
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for &d in map.values() {
        *counts.entry(d).or_insert(0) += 1;
    }
    let mut out: Vec<(usize, usize)> = counts.into_iter().collect();
    out.sort_by_key(|(d, _)| *d);
    out
}
