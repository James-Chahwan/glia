//! synth_runtime_type — P3.1 (Option 4, cycle 1.1).
//!
//! When the issue's traceback names a failed ATTRIBUTE access
//! (`'X' object has no attribute 'Y'`), this bin walks the graph to find
//! every CLASS that DOES define attribute `Y`. The model gets:
//!   - the runtime type (X, from the exception)
//!   - the candidate classes that define `Y` (graph-resolved)
//!   - the conclusion: route from X to one of the candidate classes via
//!     an access-path traversal
//!
//! Complementary to synth_traceback_target's receiver-type inference:
//! TT already says "receiver is X"; this bin tells the model where `Y` is
//! actually defined so the access-path traversal lands somewhere real.
//!
//! Output: markdown directive component. Empty when traceback is missing
//! or exception isn't AttributeError.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;
use repo_graph_code_domain::node_kind;
use repo_graph_projection_text::driver_utils::{build_repo_graph, extract_position_cell};

#[derive(Parser, Debug)]
#[command(about = "Graph-resolve where a failed attribute name IS defined (P3.1, cycle 1.1)")]
struct Args {
    #[arg(long)]
    src: PathBuf,

    #[arg(long)]
    issue: PathBuf,

    #[arg(long)]
    text_out: PathBuf,

    #[arg(long, default_value = "seeds")]
    repo_canonical: String,

    /// Max candidate class entries to emit. 0 = no cap.
    #[arg(long, default_value_t = 12)]
    max_candidates: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let issue = std::fs::read_to_string(&args.issue).context("read issue")?;

    // Parse exception: "AttributeError: 'X' object has no attribute 'Y'"
    let attr_re = Regex::new(
        r#"^(\w*AttributeError):\s*['"]?(\w+)['"]?\s+object has no attribute\s+['"](\w+)['"]"#,
    )?;
    let mut receiver_type: Option<String> = None;
    let mut attr_name: Option<String> = None;
    for line in issue.lines() {
        let t = line.trim();
        if let Some(c) = attr_re.captures(t) {
            receiver_type = Some(c.get(2).unwrap().as_str().to_string());
            attr_name = Some(c.get(3).unwrap().as_str().to_string());
            break;
        }
    }
    let (recv, attr) = match (receiver_type, attr_name) {
        (Some(r), Some(a)) => (r, a),
        _ => {
            write_empty(&args.text_out, "no AttributeError parsed from issue")?;
            return Ok(());
        }
    };

    let graph = build_repo_graph(&args.src, &args.repo_canonical)
        .context("build_repo_graph failed")?;

    // Find every node whose simple name == attr and whose kind is
    // METHOD or FUNCTION (properties are tagged as methods in glia's
    // node_kind registry). Also catch class-level attribute assignments
    // by scanning POSITION cells' source line — but for v1 we focus on
    // METHOD/FUNCTION since those are graph-first-class.
    let mut candidates: Vec<(String, String, u32)> = Vec::new(); // (class_qname, file, start_line)
    for node in &graph.nodes {
        let Some(&kind) = graph.nav.kind_by_id.get(&node.id) else { continue };
        if kind != node_kind::METHOD && kind != node_kind::FUNCTION {
            continue;
        }
        let Some(name) = graph.nav.name_by_id.get(&node.id) else { continue };
        if name != &attr {
            continue;
        }
        // Find the enclosing class qname (parent of method) — fall back
        // to "(top-level)" for FUNCTIONs.
        let enclosing_qname = match graph.nav.parent_of.get(&node.id) {
            Some(parent_id) => graph.nav.qname_by_id.get(parent_id).cloned()
                .unwrap_or_else(|| "(unknown)".into()),
            None => "(top-level)".into(),
        };
        // File + line from POSITION.
        let (file, line) = match extract_position_cell(node) {
            Some(pj) => {
                let v: serde_json::Value = serde_json::from_str(pj).unwrap_or_default();
                let f = v.get("file").and_then(|x| x.as_str()).unwrap_or("?").to_string();
                let l = v.get("start_line").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                (f, l)
            }
            None => ("?".into(), 0),
        };
        candidates.push((enclosing_qname, file, line));
    }

    // Fallback: if no METHOD/FUNCTION nodes match, scan POSITION-cell
    // file paths for `self.<attr> = ` assignment lines and find the
    // enclosing class by walking up class nodes in the graph that span
    // that line. This catches instance attributes (e.g. marshmallow's
    // `self.opts = ...` set in `BaseSchema.__init__`).
    if candidates.is_empty() {
        // Match any of: `self.opts = ...`, `cls.opts = ...`, `klass.opts = ...`,
        // or class-body-level `opts = ...`. The marshmallow metaclass uses
        // `klass.opts = klass.OPTIONS_CLASS(...)` which the strict self.opts
        // regex missed.
        let attr_esc = regex::escape(&attr);
        let assign_re = Regex::new(&format!(
            r"^\s*(?:(?:self|cls|klass|[A-Za-z_][\w]*)\s*\.)?{attr_esc}\s*[:=]",
        ))?;
        // Index class nodes by file for fast line-spanning lookup.
        let mut class_spans: Vec<(String, String, u32, u32)> = Vec::new();
        // (qname, file, start_line, end_line)
        for node in &graph.nodes {
            let Some(&kind) = graph.nav.kind_by_id.get(&node.id) else { continue };
            if kind != node_kind::CLASS {
                continue;
            }
            let Some(qn) = graph.nav.qname_by_id.get(&node.id) else { continue };
            let Some(pj) = extract_position_cell(node) else { continue };
            let v: serde_json::Value = serde_json::from_str(pj).unwrap_or_default();
            let f = v.get("file").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let sl = v.get("start_line").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
            let el = v.get("end_line").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
            if !f.is_empty() && el > 0 {
                class_spans.push((qn.clone(), f, sl, el));
            }
        }
        // For each class, read its source file and look for self.<attr>= lines.
        let mut visited_files: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        for (qn, file, sl, el) in &class_spans {
            let abs = args.src.join(file);
            let lines = visited_files.entry(file.clone()).or_insert_with(|| {
                std::fs::read_to_string(&abs).map(|t| t.lines().map(|s| s.to_string()).collect()).unwrap_or_default()
            });
            for ln in *sl..=(*el).min(lines.len() as u32) {
                if let Some(line_text) = lines.get((ln as usize).saturating_sub(1)) {
                    if assign_re.is_match(line_text) {
                        candidates.push((qn.clone(), file.clone(), ln));
                        break; // one match per class is enough
                    }
                }
            }
        }
    }

    // Dedup + sort.
    let unique: BTreeSet<(String, String, u32)> = candidates.into_iter().collect();
    let mut cands: Vec<(String, String, u32)> = unique.into_iter().collect();
    cands.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
    if args.max_candidates > 0 && cands.len() > args.max_candidates {
        cands.truncate(args.max_candidates);
    }

    if cands.is_empty() {
        write_empty(&args.text_out, &format!(
            "no graph candidates define `.{attr}` (receiver type `{recv}`)",
        ))?;
        return Ok(());
    }

    let mut s = String::new();
    s.push_str("\n## Runtime-type routing (graph-derived from attribute-access exception)\n\n");
    s.push_str(&format!(
        "The runtime exception names the failing receiver type as `{recv}`. The failing attribute is `.{attr}`. The graph resolves the following classes / contexts that DO define `.{attr}`:\n\n",
    ));
    for (cls, file, line) in &cands {
        s.push_str(&format!(
            "- `{cls}` defines `.{attr}` ({file} line {line})\n",
        ));
    }
    s.push_str(&format!(
        "\nA `{recv}` instance does NOT define `.{attr}`. The fix likely routes from `{recv}` to ONE of the classes above via an access-path traversal. Common routing patterns when the receiver is a wrapper/child class: `<recv>.root`, `<recv>.parent`, `<recv>.inner`, `<recv>.parent.opts` — pick the access path that lands on one of the candidate classes above.\n",
    ));

    if let Some(parent) = args.text_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.text_out, s.as_bytes())?;
    eprintln!(
        "[synth_runtime_type] wrote directive: {} (recv=`{recv}`, attr=`.{attr}`, {} candidate classes)",
        args.text_out.display(),
        cands.len(),
    );
    Ok(())
}

fn write_empty(path: &PathBuf, reason: &str) -> Result<()> {
    let s = format!(
        "\n## Runtime-type routing (graph-derived from attribute-access exception)\n\n\
         (no runtime-type directive: {reason})\n",
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, s.as_bytes())?;
    eprintln!("[synth_runtime_type] wrote empty directive: {} ({reason})", path.display());
    Ok(())
}
