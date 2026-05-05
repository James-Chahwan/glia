//! synth_source_map — emit a compact source-map text block for the prefix.
//!
//! Goal: kill 7B Q4's round-100 line-number guessing in `@@ -N,M @@` headers.
//! Bench 2 N=15 (2026-04-26): 5 of 9 APPLY-FAILs picked line 100/123/757 —
//! pure guesses because the prefix exposes code WITHOUT line numbers. The
//! diff-context healer rescues some via content-anchored re-fit, but it's a
//! band-aid: the principled fix is to give the model real line numbers up
//! front so it doesn't need to guess.
//!
//! For the top-N activated files (ranked by sum of activation scores), emit
//! one block per file listing CLASS / METHOD / FUNCTION nodes with their
//! real start_line from POSITION cells. Sorted by line so the map reads
//! top-to-bottom of source.
//!
//! Output is a text file (not pool cells) — read directly by run_instance.py
//! and spliced into the prefix, same pattern as `synth_derived_notes`.
//!
//! Block shape:
//!   ## Source map (real line numbers — use these in @@ -N,M @@ headers):
//!
//!   ### `src/_pytest/assertion/rewrite.py`
//!   - L42  class AssertionRewriter
//!   - L660 def AssertionRewriter.run
//!   - L720 def AssertionRewriter.visit_Call
//!   ...

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use repo_graph_code_domain::node_kind;
use repo_graph_core::NodeKindId;
use repo_graph_projection_text::driver_utils::{
    build_repo_graph, extract_position_cell, read_json,
};
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Parser, Debug)]
#[command(about = "Emit per-file source-map block with real line numbers from POSITION cells")]
struct Args {
    #[arg(long)]
    src: PathBuf,

    #[arg(long)]
    seeds: PathBuf,

    /// Output text file (block ready to splice into prefix).
    #[arg(long)]
    out: PathBuf,

    /// source_cells.json from synth_key_symbols. V3 narrows the source map to
    /// ONLY symbols that are already in source_cells (the Key symbols top-5)
    /// plus their parent class. Pure line-number reference; no new method
    /// names introduced beyond what Key symbols already shows. Without this,
    /// V1/V2's per-file enumeration distracts the model (smoke regression
    /// pytest-11143 2026-04-26: model picked unactivated `def variable` over
    /// activated `def run` because both equally visible). Optional — when
    /// absent, falls back to listing all activated methods + parent classes.
    #[arg(long)]
    source_cells: Option<PathBuf>,

    /// How many top-activated files to emit blocks for.
    #[arg(long, default_value_t = 6)]
    top_files: usize,

    /// Absolute per-file safety cap. Classes are always included unconditionally
    /// (cheap structural anchors); activated methods fill remaining budget.
    #[arg(long, default_value_t = 120)]
    per_file_cap: usize,

    /// How many source lines to show above + below each anchor line. 0 disables
    /// excerpts entirely (V3 behaviour). N=63 audit (2026-04-28): line-drift
    /// was 71% of APPLY-FAILs with target file already in cells. 5-line excerpts
    /// not enough — model still invents `@@` line numbers. Bump cap to 30 so
    /// model sees full method bodies and can copy real lines.
    #[arg(long, default_value_t = 2)]
    excerpt_context: usize,

    /// Per-file byte cap on excerpts. Prevents one huge file's excerpts from
    /// monopolising the prefix when 40+ activated methods get listed.
    /// Bumped 4096 → 12000 to match wider excerpt_context (30-line bodies).
    #[arg(long, default_value_t = 12000)]
    per_file_excerpt_bytes: usize,

    #[arg(long, default_value = "sourcemap")]
    repo_canonical: String,
}

#[derive(Deserialize)]
struct SeedsFile {
    activated: Vec<(String, f64)>,
}

#[derive(Deserialize)]
struct SourceCell {
    qname: String,
    #[allow(dead_code)]
    file: String,
}

#[derive(Deserialize)]
struct PositionPayload {
    file: String,
    start_line: usize,
    #[allow(dead_code)]
    end_line: usize,
}

#[derive(Clone)]
struct SymRow {
    line: usize,
    kind_label: &'static str,
    label: String,
    activated: bool,
}

fn kind_label(kind: NodeKindId) -> Option<&'static str> {
    if kind == node_kind::CLASS {
        Some("class")
    } else if kind == node_kind::METHOD {
        Some("def")
    } else if kind == node_kind::FUNCTION {
        Some("def")
    } else {
        None
    }
}

fn short_label(qname: &str, kind_label: &'static str) -> String {
    // `pkg::mod::Class::method` → `Class.method` (2 trailing components)
    // `pkg::mod::standalone`     → `standalone`
    let parts: Vec<&str> = qname.split("::").collect();
    let n = parts.len();
    if kind_label == "class" {
        parts.last().copied().unwrap_or(qname).to_string()
    } else if n >= 2 {
        // Heuristic: if the second-to-last component looks like a class
        // (CamelCase), join `Class.method`; else just method/function name.
        let parent = parts[n - 2];
        let last = parts[n - 1];
        let parent_is_class = parent
            .chars()
            .next()
            .map_or(false, |c| c.is_ascii_uppercase());
        if parent_is_class {
            format!("{parent}.{last}")
        } else {
            last.to_string()
        }
    } else {
        qname.to_string()
    }
}

/// Tree-sitter POSITION cells often record the leading blank line (decorator
/// gap) as the def's start_line. If the anchor line is blank/whitespace, scan
/// forward up to 3 lines for the first non-blank line — that's where the
/// `def`/`class` actually lives, and what should appear in `@@ -N,M @@`.
fn refine_anchor(lines: &[String], anchor: usize) -> usize {
    if anchor == 0 || anchor > lines.len() {
        return anchor;
    }
    let line = &lines[anchor - 1];
    if !line.trim().is_empty() {
        return anchor;
    }
    for offset in 1..=3 {
        let probe = anchor + offset;
        if probe > lines.len() {
            break;
        }
        if !lines[probe - 1].trim().is_empty() {
            return probe;
        }
    }
    anchor
}

fn read_excerpt(
    cache: &mut HashMap<String, Option<Vec<String>>>,
    src_root: &std::path::Path,
    rel_file: &str,
    anchor: usize,
    context: usize,
) -> Option<(usize, String)> {
    if context == 0 {
        return None;
    }
    let lines = cache
        .entry(rel_file.to_string())
        .or_insert_with(|| {
            let abs = src_root.join(rel_file);
            fs::read_to_string(&abs)
                .ok()
                .map(|s| s.lines().map(|l| l.to_string()).collect())
        })
        .as_ref()?;
    if anchor == 0 || anchor > lines.len() {
        return None;
    }
    let refined = refine_anchor(lines, anchor);
    let lo = refined.saturating_sub(context).max(1);
    let hi = (refined + context).min(lines.len());
    let mut out = String::new();
    for ln in lo..=hi {
        let marker = if ln == refined { ">" } else { " " };
        out.push_str(&format!("    | {marker} {ln:>5}: {}\n", lines[ln - 1]));
    }
    Some((refined, out))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let graph = build_repo_graph(&args.src, &args.repo_canonical)?;
    let seeds: SeedsFile = read_json(&args.seeds)?;
    let (source_cell_qnames, source_cell_rank): (HashSet<String>, HashMap<String, usize>) =
        match &args.source_cells {
            Some(p) => {
                let cells: Vec<SourceCell> = read_json(p)?;
                eprintln!("[srcmap] source_cells.json: {} qnames", cells.len());
                let qnames: HashSet<String> =
                    cells.iter().map(|c| c.qname.clone()).collect();
                let rank: HashMap<String, usize> = cells
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (c.qname.clone(), i))
                    .collect();
                (qnames, rank)
            }
            None => (HashSet::new(), HashMap::new()),
        };

    let activated_set: std::collections::HashSet<&str> =
        seeds.activated.iter().map(|(q, _)| q.as_str()).collect();
    let activated_score: HashMap<&str, f64> =
        seeds.activated.iter().map(|(q, s)| (q.as_str(), *s)).collect();

    // Walk every node in the graph; if it has a POSITION cell + a kind we map,
    // bucket by file. We DON'T limit to activated only — class/method labels
    // are useful even for sibling symbols in the same file (model often needs
    // to know "the class above the one I'm editing starts at L500").
    let mut by_file: HashMap<String, Vec<SymRow>> = HashMap::new();
    let mut file_score: HashMap<String, f64> = HashMap::new();

    let mut qname_by_row: HashMap<(String, usize, String), String> = HashMap::new();
    for node in &graph.nodes {
        let Some(kind) = graph.nav.kind_by_id.get(&node.id).copied() else {
            continue;
        };
        let Some(klabel) = kind_label(kind) else {
            continue;
        };
        let Some(pos_json) = extract_position_cell(node) else {
            continue;
        };
        let pos: PositionPayload = match serde_json::from_str(pos_json) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let qname = match graph.nav.qname_by_id.get(&node.id) {
            Some(q) => q.as_str(),
            None => continue,
        };
        let activated = activated_set.contains(qname);
        let label = short_label(qname, klabel);
        by_file
            .entry(pos.file.clone())
            .or_default()
            .push(SymRow {
                line: pos.start_line,
                kind_label: klabel,
                label: label.clone(),
                activated,
            });
        qname_by_row.insert((pos.file.clone(), pos.start_line, label), qname.to_string());
        if let Some(score) = activated_score.get(qname) {
            *file_score.entry(pos.file.clone()).or_insert(0.0) += *score;
        }
    }

    if file_score.is_empty() {
        eprintln!("[srcmap] no activated nodes with POSITION cells; emitting empty block");
        fs::write(&args.out, "")?;
        return Ok(());
    }

    // Pick top-N files by sum of activation scores (selection step) but then
    // SORT ALPHABETICALLY for output so there's no rank-implying ordering.
    // 2026-05-02: pytest-11143 investigate — model anchored on whichever cell
    // appeared first in source map regardless of correctness. The activation
    // score picks WHICH files to include; alphabetic ordering removes the
    // priority signal once they're in.
    let mut files: Vec<(String, f64)> = file_score.into_iter().collect();
    files.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    files.truncate(args.top_files);
    // Re-sort by file path for output presentation (alphabetic, no priority).
    files.sort_by(|a, b| a.0.cmp(&b.0));

    eprintln!(
        "[srcmap] selected {} activated files (presented alphabetically): {}",
        files.len(),
        files
            .iter()
            .map(|(f, s)| format!("{f}({s:.2})"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut block = String::new();
    block.push_str(
        "## Source map (real line numbers — use these in `@@ -N,M @@` headers):\n\n",
    );

    let v3_mode = !source_cell_qnames.is_empty();
    let mut file_cache: HashMap<String, Option<Vec<String>>> = HashMap::new();
    for (file, _score) in &files {
        let Some(rows) = by_file.get(file) else { continue };
        let methods: Vec<&SymRow> = if v3_mode {
            // V3: only methods/functions that are in source_cells (Key symbols'
            // top-5). Pure line-number annotation — no new method names beyond
            // what Key symbols already shows.
            rows.iter()
                .filter(|r| r.kind_label != "class")
                .filter(|r| {
                    qname_by_row
                        .get(&(file.clone(), r.line, r.label.clone()))
                        .map_or(false, |qn| source_cell_qnames.contains(qn))
                })
                .collect()
        } else {
            rows.iter()
                .filter(|r| r.kind_label != "class" && r.activated)
                .collect()
        };
        let method_labels: std::collections::HashSet<&str> =
            methods.iter().map(|r| r.label.as_str()).collect();
        // V3-strict (2026-04-26): in v3_mode, drop parent class-def lines.
        // marshmallow-1359 regression diagnosis: surplus `class List`/`class Tuple`/
        // `class DateTime` lines surfaced unactivated siblings to the model and
        // anchored it on the wrong override (List._bind_to_schema L633 instead of
        // DateTime L1113). source_cells already names the parent in each qname.
        let parent_classes: Vec<&SymRow> = if v3_mode {
            Vec::new()
        } else {
            rows.iter()
                .filter(|r| r.kind_label == "class")
                .filter(|c| {
                    method_labels
                        .iter()
                        .any(|m| m.split('.').next() == Some(c.label.as_str()))
                })
                .collect()
        };
        let mut chosen: Vec<&SymRow> = parent_classes;
        for r in &methods {
            if !chosen.iter().any(|c| c.line == r.line && c.label == r.label) {
                chosen.push(*r);
            }
        }
        // 2026-05-02 RESHAPED: always sort by source line. Previous v3_mode used
        // source_cells rank, but rank in source_cells is not gold-aligned and
        // sneaks attention bias back in (pytest-11143: is_rewrite_disabled rank
        // #3 was placed above run rank #4 within rewrite.py block, despite
        // run starting at L660 and is_rewrite_disabled at L744). Source-line
        // ordering = neutral context, model picks based on traceback frame
        // references in issue text.
        chosen.sort_by_key(|r| r.line);
        if chosen.len() > args.per_file_cap {
            chosen.truncate(args.per_file_cap);
        }
        if chosen.is_empty() {
            continue;
        }

        block.push_str(&format!("### `{file}`\n"));
        let mut excerpt_used: usize = 0;
        let ctx = args.excerpt_context.min(30);
        for r in chosen {
            // Refine the displayed line so it points at `def`/`class`, not the
            // blank line above. Falls back to recorded line when refinement
            // fails (no excerpt available).
            let display_line = match read_excerpt(&mut file_cache, &args.src, file, r.line, ctx) {
                Some((refined, _)) => refined,
                None => r.line,
            };
            block.push_str(&format!("- L{:<5} {} {}\n", display_line, r.kind_label, r.label));
            if ctx == 0 || excerpt_used >= args.per_file_excerpt_bytes {
                continue;
            }
            if let Some((_, ex)) = read_excerpt(&mut file_cache, &args.src, file, r.line, ctx) {
                if excerpt_used + ex.len() <= args.per_file_excerpt_bytes {
                    block.push_str(&ex);
                    excerpt_used += ex.len();
                }
            }
        }
        block.push('\n');
    }

    fs::write(&args.out, &block)?;
    eprintln!(
        "[srcmap] wrote {} ({} bytes)",
        args.out.display(),
        block.len()
    );
    Ok(())
}
