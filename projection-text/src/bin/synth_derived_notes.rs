//! synth_derived_notes — assemble the final outcome-style `## Derived notes`
//! block that the candle runner reads.
//!
//! Consumes prior synth stage outputs (source_cells.json from synth_key_symbols
//! and summaries-aplus.json from synth_composition + synth_callsite_argflow)
//! plus the raw issue text, and emits the three-bullet outcome-style block
//! proven at 7B Q4 by the G6 SOLVE arm on marshmallow-1359 (chain-walker,
//! polymorphism and attr-presence notes).
//!
//! The pass is `research::derived_notes::DerivedNotesSynth` (LD.12e): this
//! bin loads every source_cells.json row onto the view as a `key_symbols`
//! cell and every summaries entry as a `summary` cell, runs the one hook
//! through `ActivationPlan::synthesize`, and writes its block (the empty
//! string when no bullet fires). The hook reads no graph; `synthesize` takes
//! one, so the bin hands it an empty one (`build_python` over no files) and
//! parses no source. Prints
//! `[synth] plan hooks=[derived_notes] cells=N` after the plan runs.
//! `synth_plan` runs the same hook after the three hooks whose output this bin
//! reads from disk, in one plan over one graph.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use glia_activation::ActivationConfig;
use glia_activation::plan::{ActivatedView, ActivationPlan, SynthCell};
use glia_core::RepoId;
use glia_graph::{RepoGraph, build_python};
use glia_projection_text::research::derived_notes::{DERIVED_NOTES, DerivedNotesSynth};
use glia_projection_text::research::key_symbols::{KEY_SYMBOLS, SUMMARY};
use serde_json::Value;

#[derive(Parser, Debug)]
#[command(about = "Assemble the outcome-style `## Derived notes` block from prior synth outputs")]
struct Args {
    /// source_cells.json from synth_key_symbols. Each cell is
    /// `{qname, file, source, rank, reason}`. Source can be real code or a
    /// `# Attribute `.X` presence` comment block (Build 2 output).
    #[arg(long)]
    source_cells: PathBuf,

    /// summaries JSON (typically summaries-aplus.json) — the callsite-argflow
    /// cells Build 3 appended live here, with `summary` starting
    /// `# Callsite arg-flow for polymorphic method ...`.
    #[arg(long)]
    summaries: PathBuf,

    /// Issue / problem-statement text file. Drives CamelCase augmentation of
    /// the attr-presence absent-list and in-issue disambiguation for the
    /// polymorphism bullet's leaf pick.
    #[arg(long)]
    issue: PathBuf,

    /// Output path for the rendered block text. If omitted, write to stdout.
    #[arg(long)]
    out: Option<PathBuf>,
}

/// `path`'s JSON array, each element as a JSON value.
fn load_rows(path: &PathBuf, what: &str) -> Result<Vec<Value>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let v: Value = serde_json::from_slice(&bytes).with_context(|| format!("parse {what} JSON"))?;
    match v {
        Value::Array(rows) => Ok(rows),
        _ => Err(anyhow!("{what} root must be an array")),
    }
}

/// `row[field]` as a string; `""` when it is missing or not a string.
fn str_field(row: &Value, field: &str) -> String {
    row.get(field).and_then(|s| s.as_str()).unwrap_or("").to_string()
}

/// The source_cells.json rows as `key_symbols` cells, in file order: `key`
/// the row's `qname`, `text` its `source`, `id` its 1-based position.
fn load_source_cells(path: &PathBuf) -> Result<Vec<SynthCell>> {
    let rows = load_rows(path, "source_cells")?;
    Ok(rows
        .iter()
        .enumerate()
        .map(|(i, row)| SynthCell {
            hook: KEY_SYMBOLS,
            id: i as u64 + 1,
            key: str_field(row, "qname"),
            anchor: None,
            text: str_field(row, "source"),
            score: 0.0,
            attrs: Vec::new(),
        })
        .collect())
}

/// Every summaries entry as a `summary` cell, in file order: `key` the
/// entry's `qname`, `text` its `summary`, `id` and `score` its own (0 when
/// missing). No entry is filtered: the hook reads them all.
fn load_summaries(path: &PathBuf) -> Result<Vec<SynthCell>> {
    let rows = load_rows(path, "summaries")?;
    Ok(rows
        .iter()
        .map(|row| SynthCell {
            hook: SUMMARY,
            id: row.get("id").and_then(Value::as_u64).unwrap_or(0),
            key: str_field(row, "qname"),
            anchor: None,
            text: str_field(row, "summary"),
            score: row.get("score").and_then(Value::as_f64).unwrap_or(0.0),
            attrs: Vec::new(),
        })
        .collect())
}

fn main() -> Result<()> {
    let args = Args::parse();

    let cells = load_source_cells(&args.source_cells)?;
    let summaries = load_summaries(&args.summaries)?;
    let issue =
        fs::read_to_string(&args.issue).with_context(|| format!("read {}", args.issue.display()))?;

    let graph = build_python(RepoId::from_canonical("synth-derived-notes"), Vec::new())
        .map_err(|e| anyhow!("build_python: {e:?}"))?;
    let hook = DerivedNotesSynth { issue };
    let plan = ActivationPlan::<RepoGraph>::new(ActivationConfig::default()).synth(&hook);
    let mut view = ActivatedView::from_ranked(Vec::new());
    view.synth.extend(cells);
    view.synth.extend(summaries);
    plan.synthesize(&graph, &mut view);

    let notes: Vec<&SynthCell> = view.cells_of(DERIVED_NOTES).collect();
    eprintln!("[synth] plan hooks=[{}] cells={}", view.applied.join(","), notes.len());
    let block = notes.first().map_or("", |c| c.text.as_str());

    match args.out {
        Some(path) => fs::write(&path, block)
            .with_context(|| format!("write {}", path.display()))?,
        None => print!("{}", block),
    }
    Ok(())
}
