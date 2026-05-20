//! synth_traceback_target — graph-side L2 candidate.
//!
//! Parses Python tracebacks from the SWE-bench issue text, finds graph nodes
//! whose POSITION cell range contains the traceback's line numbers, and
//! emits a high-priority synthetic "target" entry into the summaries pool.
//!
//! The TARGETING comes from the GRAPH (POSITION cells matched against
//! traceback line numbers), not from any hand-written instance-specific
//! text. Works on any SWE-bench instance whose issue contains a Python
//! traceback.
//!
//! Output JSON shape (compatible with downstream run_llama_pathB.py):
//!   appends `{id, qname, score, summary}` entries to the existing pool.
//!
//! Why this is graph-side, not prompt-side:
//! - Existing prefix already contains diagnosis + access paths from
//!   derived_notes / access_paths sections.
//! - The model has the answer but defaults to easier targets under generic
//!   suffix (validated 2026-05-21, cycle 0 marshmallow loop).
//! - This bin tells the model WHICH node the bug lives in, derived directly
//!   from the issue's traceback line numbers using POSITION cells.
//!
//! Test: marshmallow-1359 issue contains:
//!   File ".../marshmallow/fields.py", line 1117, in _bind_to_schema
//! → match the node whose POSITION spans line 1117 in fields.py
//! → emit a high-score TARGET cell pointing at it
//! → re-run inference with GENERIC suffix
//! → if model now produces correct fix, latent injection controls target.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;
use repo_graph_core::Node;
use repo_graph_projection_text::driver_utils::{
    build_repo_graph, extract_position_cell, read_json, write_json,
};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(about = "Append graph-derived target cells to an existing summaries pool")]
struct Args {
    /// Repo source dir to parse → .gmap (in-memory).
    #[arg(long)]
    src: PathBuf,

    /// SWE-bench issue text file (contains the Python traceback we parse).
    #[arg(long)]
    issue: PathBuf,

    /// Existing summaries-aplus.json (or summaries.json) — input pool. When
    /// `--out` is given, target cells are appended here (L2 latent-pool path,
    /// 2026-05-21 confirmed INEFFECTIVE: runner mean-pools all cells into
    /// 1-vec each so score/label distinctions don't reach the model).
    #[arg(long)]
    summaries: Option<PathBuf>,

    /// Output path for augmented pool JSON (legacy L2 path). Pass either
    /// this or `--text-out`. Prefer `--text-out` for actual steering.
    #[arg(long)]
    out: Option<PathBuf>,

    /// Output path for a TEXT directive block (iter3 path). The block
    /// is meant to be concatenated into the suffix or appended to the prefix
    /// so the model sees it as prefix/suffix tokens, not as a mean-pooled
    /// anonymous vector. This is the channel that actually steers the model
    /// (validated 2026-05-21 cycle 0 iter1).
    #[arg(long)]
    text_out: Option<PathBuf>,

    /// Score assigned to synthetic target cells in the legacy pool path.
    /// IGNORED by run_llama_pathB.py (which doesn't read the score field).
    /// Kept for compat.
    #[arg(long, default_value_t = 999.0)]
    target_score: f64,

    /// Optional: also write the matched (file, line, qname) tuples to a
    /// debug JSON for cycle-log auditing.
    #[arg(long)]
    matches_debug: Option<PathBuf>,

    #[arg(long, default_value = "seeds")]
    repo_canonical: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PoolEntry {
    id: u64,
    qname: String,
    score: f64,
    summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PositionPayload {
    file: String,
    start_line: u32,
    end_line: u32,
}

#[derive(Debug, Clone, Serialize)]
struct TracebackMatch {
    traceback_file: String,
    traceback_line: u32,
    traceback_function: String,
    matched_qname: String,
    matched_node_file: String,
    matched_start_line: u32,
    matched_end_line: u32,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // 1. Parse issue text for Python tracebacks: `File "...", line N, in <fn>`.
    //    Also capture the SOURCE LINE shown beneath each File line (the
    //    "buggy line" — typically the next non-blank indented line).
    let issue_text = std::fs::read_to_string(&args.issue)
        .with_context(|| format!("read issue {}", args.issue.display()))?;
    let traceback_re = Regex::new(
        r#"File\s+"([^"]+)",\s*line\s+(\d+),\s*in\s+(\w+)"#,
    )?;
    // Build linear list of (file, line, fn, source_line_opt) by walking the issue lines.
    let issue_lines: Vec<&str> = issue_text.lines().collect();
    let mut tracebacks: Vec<(String, u32, String, Option<String>)> = Vec::new();
    for (i, line) in issue_lines.iter().enumerate() {
        if let Some(c) = traceback_re.captures(line) {
            let file = c.get(1).unwrap().as_str().to_string();
            let lineno = c.get(2).unwrap().as_str().parse::<u32>().unwrap_or(0);
            let fn_name = c.get(3).unwrap().as_str().to_string();
            if lineno == 0 { continue; }
            // Look ahead 1-2 lines for the source-line excerpt.
            let mut src: Option<String> = None;
            for off in 1..=2 {
                if let Some(nl) = issue_lines.get(i + off) {
                    let t = nl.trim();
                    if !t.is_empty()
                        && !traceback_re.is_match(nl)
                        && !t.starts_with("AttributeError")
                        && !t.starts_with("TypeError")
                        && !t.starts_with("ValueError")
                        && !t.starts_with("KeyError")
                    {
                        src = Some(t.to_string());
                        break;
                    }
                }
            }
            tracebacks.push((file, lineno, fn_name, src));
        }
    }
    // Parse the final exception line (after the last traceback File line):
    // "AttributeError: '<TypeName>' object has no attribute '<attr>'"
    let exc_re = Regex::new(
        r#"^(\w+Error):\s*'([^']+)'\s+object\s+has\s+no\s+attribute\s+'([^']+)'"#,
    )?;
    let mut exc_info: Option<(String, String, String)> = None; // (error_class, object_type, missing_attr)
    for line in &issue_lines {
        if let Some(c) = exc_re.captures(line.trim()) {
            exc_info = Some((
                c.get(1).unwrap().as_str().to_string(),
                c.get(2).unwrap().as_str().to_string(),
                c.get(3).unwrap().as_str().to_string(),
            ));
        }
    }
    if tracebacks.is_empty() {
        eprintln!(
            "[synth_traceback_target] no Python tracebacks found in issue; \
             writing pool unchanged"
        );
    } else {
        eprintln!(
            "[synth_traceback_target] {} traceback line(s) parsed from issue",
            tracebacks.len()
        );
        for (f, l, fn_name, src) in &tracebacks {
            eprintln!(
                "  · {f}:{l} in {fn_name} | src: {}",
                src.as_deref().unwrap_or("(none)")
            );
        }
    }
    if let Some((cls, obj_type, missing_attr)) = &exc_info {
        eprintln!("[synth_traceback_target] exception: {cls}: '{obj_type}' has no attribute '{missing_attr}'");
    }

    // 2. Build graph for the repo.
    let graph = build_repo_graph(&args.src, &args.repo_canonical)
        .context("build_repo_graph failed")?;
    eprintln!(
        "[synth_traceback_target] graph: {} nodes",
        graph.nodes.len()
    );

    // 3. For each traceback (file, line), find nodes whose POSITION cell
    //    spans that line in a path that ends with the traceback's file.
    let mut matches: Vec<TracebackMatch> = Vec::new();
    let mut seen_qnames: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for (tb_file, tb_line, tb_fn, _tb_src) in &tracebacks {
        // The traceback path is usually absolute (e.g. /home/.../fields.py).
        // Strip leading components to compare against graph's relative paths.
        let tb_basename = std::path::Path::new(tb_file)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(tb_file.as_str());
        for node in &graph.nodes {
            let Some(pos_json) = extract_position_cell(node) else { continue };
            let Ok(pos) = serde_json::from_str::<PositionPayload>(pos_json) else {
                continue;
            };
            // File-path containment: graph stores relative paths.
            let graph_basename = std::path::Path::new(&pos.file)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(pos.file.as_str());
            if graph_basename != tb_basename {
                continue;
            }
            // Line containment.
            if *tb_line < pos.start_line || *tb_line > pos.end_line {
                continue;
            }
            let qname = qname_of_node(&graph, node)
                .unwrap_or_else(|| format!("node:{:x}", node.id.0));
            if !seen_qnames.insert(qname.clone()) {
                continue;
            }
            matches.push(TracebackMatch {
                traceback_file: tb_file.clone(),
                traceback_line: *tb_line,
                traceback_function: tb_fn.clone(),
                matched_qname: qname,
                matched_node_file: pos.file,
                matched_start_line: pos.start_line,
                matched_end_line: pos.end_line,
            });
        }
    }
    eprintln!(
        "[synth_traceback_target] matched {} graph node(s) from tracebacks",
        matches.len()
    );
    for m in &matches {
        eprintln!(
            "  · {}:{} in {} → {} (POSITION {}:{}-{})",
            m.traceback_file,
            m.traceback_line,
            m.traceback_function,
            m.matched_qname,
            m.matched_node_file,
            m.matched_start_line,
            m.matched_end_line,
        );
    }

    // 4 (optional). Legacy L2 path: append target cells to JSON pool.
    if let (Some(summaries_path), Some(out_path)) = (&args.summaries, &args.out) {
        let mut pool: Vec<PoolEntry> = read_json(summaries_path)?;
        eprintln!(
            "[synth_traceback_target] (legacy L2) pool input: {} entries; max score: {:.3}",
            pool.len(),
            pool.iter().map(|e| e.score).fold(f64::NEG_INFINITY, f64::max),
        );
        let mut next_synth_id: u64 = 0xFFFF_FFFF_0000_0000;
        for m in &matches {
            let summary = format!(
                "## Required fix target\n\nEdit the function `{}` in `{}` (lines {}-{}).",
                m.matched_qname, m.matched_node_file, m.matched_start_line, m.matched_end_line
            );
            pool.push(PoolEntry {
                id: next_synth_id,
                qname: format!("synth::Target::{}", m.matched_qname),
                score: args.target_score,
                summary,
            });
            next_synth_id += 1;
        }
        write_json(out_path, &pool)?;
        eprintln!(
            "[synth_traceback_target] (legacy L2) wrote pool with {} entries",
            pool.len()
        );
    }

    // 5 (primary). iter3 text-directive path: emit a directive block to be
    // concatenated into the suffix or appended to the prefix as ACTUAL TEXT
    // TOKENS (not mean-pooled). This is the only channel that has been
    // observed to steer target selection (cycle 0 iter1, 2026-05-21).
    if let Some(text_path) = &args.text_out {
        // Filter: prefer narrower POSITION ranges per (file, line) — collapse module/class spans down to method-granularity matches.
        let mut narrowest_by_loc: std::collections::BTreeMap<(String, u32), &TracebackMatch> =
            std::collections::BTreeMap::new();
        for m in &matches {
            let key = (m.traceback_file.clone(), m.traceback_line);
            let cur_span = narrowest_by_loc
                .get(&key)
                .map(|c| c.matched_end_line - c.matched_start_line)
                .unwrap_or(u32::MAX);
            let this_span = m.matched_end_line - m.matched_start_line;
            if this_span < cur_span {
                narrowest_by_loc.insert(key, m);
            }
        }

        // Pick the DEEPEST traceback frame (last one parsed = innermost call).
        // For Python tracebacks, this is the frame where the exception throws.
        let target_match = tracebacks.last().and_then(|(tb_file, tb_line, _, _)| {
            narrowest_by_loc.get(&(tb_file.clone(), *tb_line)).copied()
        });
        let target_src_line: Option<&String> = tracebacks
            .last()
            .and_then(|(_, _, _, src)| src.as_ref());

        // Build anti-target list: all OTHER narrowest matches besides the target.
        let antitarget_qnames: Vec<&String> = narrowest_by_loc
            .values()
            .filter(|m| target_match.map(|t| t.matched_qname != m.matched_qname).unwrap_or(true))
            .map(|m| &m.matched_qname)
            .collect();

        let mut s = String::new();
        s.push_str("\n## Required fix target (graph-derived from issue traceback)\n\n");
        if let Some(tm) = target_match {
            s.push_str(&format!(
                "Edit ONE function: `{}` in `{}` (function body spans lines {}-{}).\n",
                tm.matched_qname, tm.matched_node_file, tm.matched_start_line, tm.matched_end_line,
            ));
            s.push_str(&format!(
                "\nThe SWE-bench issue traceback names this function at line {} (the DEEPEST frame — where the exception throws).\n",
                tm.traceback_line,
            ));
            // Surface the buggy source line + the exception class together so
            // the model knows EXACTLY which line to edit AND what access is
            // failing. Both extracted from the issue text (graph-derived
            // target + issue-derived buggy line + issue-derived exception).
            if let Some(src) = target_src_line {
                s.push_str(&format!(
                    "\nThe BUGGY LINE (per traceback line {}) is:\n```\n{}\n```\n",
                    tm.traceback_line, src,
                ));
            }
            if let Some((cls, obj_type, missing_attr)) = &exc_info {
                s.push_str(&format!(
                    "\nThe exception is `{cls}: '{obj_type}' object has no attribute '{missing_attr}'`. The receiver of the failing `.{missing_attr}` access is a `{obj_type}` instance at runtime, NOT what the variable name suggests. Replace the failing `<X>.{missing_attr}` access with the access-path that actually resolves to `.{missing_attr}` (consult the '## Reachable access paths:' section in the prefix above; e.g. `self.root.{missing_attr}`).\n",
                ));
            } else {
                s.push_str(
                    "\nThe prefix lists access-paths under '## Reachable access paths:'. When the issue describes a `<X>.<Y>` attribute access that fails because `<X>` has no `.<Y>`, replace `<X>.<Y>` with the access-path that resolves to `<Y>` (e.g. `self.root.<Y>`).\n",
                );
            }
            if !antitarget_qnames.is_empty() {
                s.push_str("\n**Do NOT modify** these functions (they appear earlier in the call stack but the bug is NOT in them):\n");
                for q in &antitarget_qnames {
                    s.push_str(&format!("- `{q}`\n"));
                }
            }
            s.push_str(&format!(
                "\nEdit ONLY `{}` and ONLY the buggy line above. Emit a minimal unified diff.\n",
                tm.matched_node_file,
            ));
        } else {
            s.push_str(
                "(no graph node matched the deepest traceback frame — no targeting available)\n",
            );
        }
        if let Some(parent) = text_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(text_path, s.as_bytes())?;
        eprintln!(
            "[synth_traceback_target] wrote text directive: {} ({})",
            text_path.display(),
            target_match
                .map(|m| format!("target = {}", m.matched_qname))
                .unwrap_or_else(|| "no target".to_string()),
        );
    }

    if let Some(dbg) = args.matches_debug {
        write_json(&dbg, &matches)?;
    }
    Ok(())
}

fn qname_of_node(graph: &repo_graph_graph::RepoGraph, node: &Node) -> Option<String> {
    graph.nav.qname_by_id.get(&node.id).cloned()
}
