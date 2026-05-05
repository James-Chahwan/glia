//! synth_constant_decl — module-level UPPERCASE constants whose name appears
//! in the issue text.
//!
//! Step 3 receptor for #77 / task #78 (bucket-1 shape: module-level constant
//! changes — django-10914 `FILE_UPLOAD_PERMISSIONS = None → 0o644` shape).
//!
//! Glia's Python parser doesn't model module-level assignments as nodes
//! (only CLASS attributes via `node_kind::ATTRIBUTE`). Rather than extend
//! the parser, this bin re-walks `*.py` files with a regex over top-level
//! UPPERCASE assignments — same pattern as `synth_callsite_argflow`'s AST
//! re-walk. Emits one pool cell per (issue-mentioned constant name) match,
//! appended to summaries-aplus.json so the latent harness picks it up.
//!
//! Cell shape:
//!   qname:   `synth::ConstantDecl::<module-qname>::<NAME>`
//!   summary: `Module-level constant in <module-qname> (file:L):
//!             NAME = VALUE
//!             Co-located UPPERCASE constants in this module: A, B, C, ...`

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;
use repo_graph_projection_text::driver_utils::{read_json, write_json};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(about = "Synthesise pool cells for module-level UPPERCASE constants mentioned in issue")]
struct Args {
    /// Repo root to walk for `*.py` files.
    #[arg(long)]
    src: PathBuf,

    /// Issue text path. UPPERCASE_SNAKE_CASE tokens here are matched against
    /// constant LHS names in module bodies.
    #[arg(long)]
    issue: PathBuf,

    /// Existing summaries-hybrid JSON to append to.
    #[arg(long)]
    summaries: PathBuf,

    /// Output path for the augmented summaries JSON.
    #[arg(long)]
    out: PathBuf,

    /// Co-located neighbour count emitted per cell (for context).
    #[arg(long, default_value_t = 8)]
    colocated_cap: usize,

    /// First synthetic cell id. Picks a high water mark to avoid collisions
    /// with existing summary ids and other synth bins.
    #[arg(long, default_value_t = 30_000_000)]
    id_start: u64,

    /// Cap on emitted cells (defensive — prevents pathological repos from
    /// blowing up the pool).
    #[arg(long, default_value_t = 20)]
    max_cells: usize,
}

#[derive(Serialize, Deserialize, Clone)]
struct SummaryEntry {
    id: u64,
    qname: String,
    score: f64,
    summary: String,
}

struct ConstantDecl {
    module_qname: String,
    file_rel: String,
    name: String,
    value: String,
    line: usize,
}

fn extract_constants_from_file(content: &str, module_qname: &str, file_rel: &str) -> Vec<ConstantDecl> {
    let assign_re = Regex::new(r"(?m)^([A-Z][A-Z0-9_]{2,})\s*=\s*(.+?)$").unwrap();
    let mut out = Vec::new();
    for cap in assign_re.captures_iter(content) {
        let full_match = cap.get(0).unwrap();
        let name = cap.get(1).unwrap().as_str().to_string();
        let mut value = cap.get(2).unwrap().as_str().trim().to_string();
        if value.len() > 200 {
            value.truncate(200);
            value.push_str("...");
        }
        let line = content[..full_match.start()].matches('\n').count() + 1;
        out.push(ConstantDecl {
            module_qname: module_qname.to_string(),
            file_rel: file_rel.to_string(),
            name,
            value,
            line,
        });
    }
    out
}

fn extract_issue_uppercase_tokens(issue: &str) -> (Vec<String>, HashMap<String, usize>) {
    let token_re = Regex::new(r"\b([A-Z][A-Z0-9_]{2,})\b").unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut order = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for cap in token_re.captures_iter(issue) {
        let t = cap.get(1).unwrap().as_str().to_string();
        *counts.entry(t.clone()).or_insert(0) += 1;
        if seen.insert(t.clone()) {
            order.push(t);
        }
    }
    (order, counts)
}

fn main() -> Result<()> {
    let args = Args::parse();

    let issue = fs::read_to_string(&args.issue)
        .with_context(|| format!("read {}", args.issue.display()))?;
    let (issue_tokens, issue_token_counts) = extract_issue_uppercase_tokens(&issue);
    eprintln!("[constdecl] {} UPPERCASE issue tokens: {}",
        issue_tokens.len(),
        issue_tokens.iter().take(8)
            .map(|t| format!("{}×{}", t, issue_token_counts.get(t).copied().unwrap_or(0)))
            .collect::<Vec<_>>().join(", "));

    if issue_tokens.is_empty() {
        eprintln!("[constdecl] no issue tokens; emitting 0 cells");
        let existing: Vec<SummaryEntry> = read_json(&args.summaries)?;
        write_json(&args.out, &existing)?;
        return Ok(());
    }
    let issue_token_set: std::collections::HashSet<&str> =
        issue_tokens.iter().map(String::as_str).collect();

    // Per-file constant scan.
    let mut by_module: HashMap<String, Vec<ConstantDecl>> = HashMap::new();
    let mut files_scanned = 0usize;
    for entry in WalkDir::new(&args.src).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() { continue; }
        if entry.path().extension().map_or(true, |x| x != "py") { continue; }
        let rel = entry.path().strip_prefix(&args.src).unwrap_or(entry.path());
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let module_qname = rel_str.trim_end_matches(".py").replace('/', "::");
        let content = match fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        files_scanned += 1;
        let consts = extract_constants_from_file(&content, &module_qname, &rel_str);
        if !consts.is_empty() {
            by_module.insert(module_qname, consts);
        }
    }
    eprintln!("[constdecl] scanned {} files, {} have UPPERCASE constants",
        files_scanned, by_module.len());

    // Match issue tokens against per-module constants. Emit one cell per
    // (module, matched-name) pair. Stable order for reproducibility.
    let mut module_keys: Vec<&String> = by_module.keys().collect();
    module_keys.sort();
    let mut new_cells: Vec<SummaryEntry> = Vec::new();
    let mut next_id = args.id_start;
    for module_qname in module_keys {
        let consts = &by_module[module_qname];
        let names_in_module: Vec<&str> = consts.iter().map(|c| c.name.as_str()).collect();
        for c in consts {
            if !issue_token_set.contains(c.name.as_str()) {
                continue;
            }
            let co_located: Vec<&str> = names_in_module.iter()
                .filter(|n| **n != c.name.as_str())
                .copied()
                .take(args.colocated_cap)
                .collect();
            let summary = format!(
                "Module-level constant in `{}` ({}:{}):\n{} = {}\nCo-located UPPERCASE constants in this module: {}",
                c.module_qname,
                c.file_rel,
                c.line,
                c.name,
                c.value,
                if co_located.is_empty() { "(none)".to_string() } else { co_located.join(", ") },
            );
            // Score by issue mention count × 5.0 — N=2 lesson from django-10914:
            // a constant mentioned 3× ("FILE_UPLOAD_PERMISSIONS") in the issue is
            // far more load-bearing than one mentioned once. The Python rendering
            // loop sorts by this score descending and takes top-3 for the prefix
            // constants block, so multi-mentioned constants outrank tangential
            // ones (e.g. FILE_UPLOAD_HANDLERS mentioned only as co-located noise).
            let mention_count = issue_token_counts.get(c.name.as_str()).copied().unwrap_or(1);
            let score = (mention_count as f64) * 5.0;
            new_cells.push(SummaryEntry {
                id: next_id,
                qname: format!("synth::ConstantDecl::{}::{}", c.module_qname, c.name),
                score,
                summary,
            });
            next_id += 1;
            if new_cells.len() >= args.max_cells {
                eprintln!("[constdecl] cap hit at {} cells", args.max_cells);
                break;
            }
        }
        if new_cells.len() >= args.max_cells { break; }
    }

    eprintln!("[constdecl] {} ConstantDecl cells emitted", new_cells.len());
    for c in new_cells.iter().take(5) {
        eprintln!("  [constdecl] {}", c.qname);
    }

    let mut existing: Vec<SummaryEntry> = read_json(&args.summaries)?;
    existing.extend(new_cells);
    write_json(&args.out, &existing)?;
    eprintln!("[constdecl] wrote {} ({} total cells)", args.out.display(), existing.len());

    Ok(())
}
