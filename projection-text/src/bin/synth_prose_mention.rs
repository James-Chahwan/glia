//! synth_prose_mention — graph-side directive from issue PROSE mentions.
//!
//! Bundle B2 (cycle 0.6, 2026-05-21). Third channel of graph-derived NAMED
//! content for the directive composer (B4):
//!
//!   - synth_traceback_target: traceback frames (1/7 of cycle_loop_set hits)
//!   - synth_test_expectation: test_patch identifiers (7/7 hits via tests)
//!   - synth_prose_mention:    backtick-quoted + CamelCase symbols in prose
//!
//! Targeted at no-traceback instances whose test_patch is small or
//! uninformative (e.g. a one-line assertion flip) but whose issue text DOES
//! name the relevant classes / settings / functions. Django issues often
//! reference `FileSystemStorage`, `FILE_UPLOAD_PERMISSIONS`, etc — these
//! resolve cleanly via the tail-index against the repo graph.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;
use repo_graph_code_domain::node_kind;
use repo_graph_core::NodeId;
use repo_graph_projection_text::driver_utils::build_repo_graph;

#[derive(Parser, Debug)]
#[command(about = "Derive a directive from issue prose mentions (B2, cycle 0.6)")]
struct Args {
    #[arg(long)]
    src: PathBuf,

    /// SWE-bench issue text file. Scanned for backtick-quoted and CamelCase
    /// symbols that match graph qnames.
    #[arg(long)]
    issue: PathBuf,

    #[arg(long)]
    text_out: PathBuf,

    #[arg(long, default_value_t = 5)]
    max_targets: usize,

    #[arg(long, default_value = "seeds")]
    repo_canonical: String,
}

#[derive(Debug, Clone)]
struct Mention {
    name: String,
    qname: String,
    kind_label: &'static str,
}

const STOP: &[&str] = &[
    "I", "We", "You", "The", "When", "Then", "If", "But", "And", "Or", "It", "This",
    "That", "There", "Here", "What", "Why", "How", "Bug", "Issue", "Fix", "TODO",
    "Note", "Example", "Description", "Error", "Exception", "Traceback", "File",
    "Hello", "Hi",
];

fn main() -> Result<()> {
    let args = Args::parse();
    let issue_text = std::fs::read_to_string(&args.issue)
        .with_context(|| format!("read issue {}", args.issue.display()))?;

    let mentions = extract_mentions(&issue_text);
    eprintln!(
        "[synth_prose_mention] {} unique candidate mentions in prose",
        mentions.len()
    );

    if mentions.is_empty() {
        write_empty(&args.text_out, "no prose mentions matched extraction patterns")?;
        return Ok(());
    }

    let graph = build_repo_graph(&args.src, &args.repo_canonical)
        .context("build_repo_graph failed")?;
    eprintln!("[synth_prose_mention] graph: {} nodes", graph.nodes.len());

    let tail_idx = build_tail_index(&graph);
    let mut resolved: Vec<Mention> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for cand in &mentions {
        // Try exact tail match first; fall back to dotted-suffix match
        // (e.g. `Storage.file_permissions_mode` → match `file_permissions_mode`).
        let lookup = cand.rsplit('.').next().unwrap_or(cand);
        let Some(hits) = tail_idx.get(lookup) else {
            continue;
        };
        let mut sorted = hits.clone();
        sorted.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| a.1.cmp(b.1)));
        let (nid, qn) = sorted[0];
        if !seen.insert(qn.to_string()) {
            continue;
        }
        let kind_label = match graph.nav.kind_by_id.get(&nid).copied() {
            Some(k) if k == node_kind::METHOD => "method",
            Some(k) if k == node_kind::FUNCTION => "function",
            Some(k) if k == node_kind::CLASS => "class",
            _ => "symbol",
        };
        resolved.push(Mention {
            name: cand.clone(),
            qname: qn.to_string(),
            kind_label,
        });
    }
    eprintln!(
        "[synth_prose_mention] {} of {} mentions resolved to graph qnames",
        resolved.len(),
        mentions.len()
    );

    if args.max_targets > 0 && resolved.len() > args.max_targets {
        // Prefer CLASS over METHOD over FUNCTION in prose context (class
        // names are the most common useful prose reference).
        resolved.sort_by(|a, b| {
            kind_priority(a.kind_label)
                .cmp(&kind_priority(b.kind_label))
                .then_with(|| a.qname.len().cmp(&b.qname.len()))
                .then_with(|| a.qname.cmp(&b.qname))
        });
        resolved.truncate(args.max_targets);
    }

    if resolved.is_empty() {
        write_empty(&args.text_out, "no prose mentions resolved to graph qnames")?;
        return Ok(());
    }

    let directive = render_directive(&resolved);
    if let Some(parent) = args.text_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.text_out, directive.as_bytes())?;
    eprintln!(
        "[synth_prose_mention] wrote directive: {} ({} mentions)",
        args.text_out.display(),
        resolved.len()
    );
    Ok(())
}

fn extract_mentions(text: &str) -> Vec<String> {
    // Two patterns:
    //   1. Backtick-quoted code: `foo`, `Foo.bar`, `foo()`
    //   2. CamelCase identifiers in prose: FileSystemStorage, MyClass
    // Skip common English words via STOP list.
    let backtick = Regex::new(r"`([A-Za-z_][A-Za-z0-9_.]*?)(?:\(\))?`").unwrap();
    let camel = Regex::new(r"\b([A-Z][a-z]+(?:[A-Z][a-z0-9]+){1,})\b").unwrap();
    let stop: HashSet<&str> = STOP.iter().copied().collect();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for c in backtick.captures_iter(text) {
        let s = c.get(1).unwrap().as_str().to_string();
        if s.len() >= 2 && !stop.contains(s.as_str()) {
            out.insert(s);
        }
    }
    for c in camel.captures_iter(text) {
        let s = c.get(1).unwrap().as_str().to_string();
        if !stop.contains(s.as_str()) {
            out.insert(s);
        }
    }
    out.into_iter().collect()
}

fn build_tail_index<'g>(
    graph: &'g repo_graph_graph::RepoGraph,
) -> BTreeMap<&'g str, Vec<(NodeId, &'g str)>> {
    let mut out: BTreeMap<&str, Vec<(NodeId, &str)>> = BTreeMap::new();
    for n in &graph.nodes {
        let Some(&kind) = graph.nav.kind_by_id.get(&n.id) else {
            continue;
        };
        if kind != node_kind::METHOD && kind != node_kind::FUNCTION && kind != node_kind::CLASS {
            continue;
        }
        let Some(qn) = graph.nav.qname_by_id.get(&n.id) else {
            continue;
        };
        let qn_str = qn.as_str();
        let tail = qn_str.rsplit("::").next().unwrap_or(qn_str);
        out.entry(tail).or_default().push((n.id, qn_str));
    }
    out
}

fn kind_priority(kind: &str) -> u8 {
    match kind {
        "class" => 0,
        "method" => 1,
        "function" => 2,
        _ => 3,
    }
}

fn render_directive(resolved: &[Mention]) -> String {
    let mut s = String::new();
    s.push_str("\n## Required fix target (graph-derived from issue prose mentions)\n\n");
    s.push_str(
        "The SWE-bench issue text references these symbols by name. The implementation \
         file containing them is likely the right edit site:\n\n",
    );
    for r in resolved {
        s.push_str(&format!(
            "- `{}` ({}, mentioned in issue as `{}`)\n",
            r.qname, r.kind_label, r.name,
        ));
    }
    s.push('\n');
    s.push_str(
        "Pick the SINGLE target whose behavior matches the bug described in the issue. \
         Emit a minimal unified diff against the implementation file containing it.\n",
    );
    s
}

fn write_empty(path: &PathBuf, reason: &str) -> Result<()> {
    let s = format!(
        "\n## Required fix target (graph-derived from issue prose mentions)\n\n\
         (no prose-mention directive: {reason})\n",
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, s.as_bytes())?;
    eprintln!(
        "[synth_prose_mention] wrote empty directive: {} ({reason})",
        path.display()
    );
    Ok(())
}
