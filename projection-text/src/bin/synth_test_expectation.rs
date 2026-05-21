//! synth_test_expectation — graph-side directive derived from SWE-bench
//! `test_patch.patch`.
//!
//! Bundle B1 (cycle 0.6, 2026-05-21). Cycle 0.3 produced 1/7 PASS on the
//! 7-instance loop set. 3/7 instances have NO Python traceback in the issue
//! text — synth_traceback_target falls back to an inert directive, so the
//! model sees nothing graph-derived steering it toward the right function.
//!
//! The F2P test BODIES (in `test_patch.patch`) name the symbols the fix is
//! supposed to support. This bin parses the test_patch, extracts the added
//! test-function code, identifies referenced symbols, resolves them against
//! the repo graph (qname match), and emits a markdown directive in the same
//! shape as synth_traceback_target.
//!
//! Why graph-derived NAMES not raw tokens: cycle 0.5 D falsified
//! "structural form alone is enough" — placeholder slots failed marshmallow
//! identically to baseline. The lever is structure × graph-derived NAMED
//! content, multiplied (see project_cycle_04_directive_mechanism_reframe).
//! This bin widens the source funnel for the NAMED-content factor.
//!
//! Pipeline: test_patch.patch → unified-diff parse → per-hunk identifier
//! extraction → tail-index match against graph qnames → directive markdown.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;
use repo_graph_code_domain::node_kind;
use repo_graph_core::NodeId;
use repo_graph_projection_text::driver_utils::build_repo_graph;

#[derive(Parser, Debug)]
#[command(about = "Derive a directive block from SWE-bench test_patch.patch (B1, cycle 0.6)")]
struct Args {
    /// Repo source dir to parse → in-memory RepoGraph (same entry as synth_traceback_target).
    #[arg(long)]
    src: PathBuf,

    /// SWE-bench `test_patch.patch` file (unified diff of the F2P test additions).
    #[arg(long)]
    test_patch: PathBuf,

    /// Output path for the markdown directive block. The block is meant to be
    /// concatenated into the suffix (real-pipeline layout — same channel as
    /// synth_traceback_target's --text-out output).
    #[arg(long)]
    text_out: PathBuf,

    /// Maximum number of named targets to emit in the directive. Default 6 —
    /// enough to cover a typical test_patch's surface, narrow enough that the
    /// directive stays focused (the cycle 0.5 D result showed long unfocused
    /// directives don't add information). Pass 0 for no cap.
    #[arg(long, default_value_t = 6)]
    max_targets: usize,

    #[arg(long, default_value = "seeds")]
    repo_canonical: String,
}

/// One hunk of the test_patch: the new line span and the added (`+`-prefixed)
/// source body. We also pull out the test-function names mentioned in `def
/// test_xxx(` lines inside the hunk body so the directive can name them.
#[derive(Debug, Clone)]
struct TestPatchHunk {
    new_file: String,
    new_start_line: u32,
    added_body: String,
    test_fns: Vec<String>,
}

/// A symbol referenced by an added test body that resolved to a graph qname.
#[derive(Debug, Clone)]
struct ResolvedSymbol {
    /// Bare identifier as it appears in the test body (e.g. `_bind_to_schema`).
    name: String,
    /// Best-scoring graph qname for it (shortest qname wins).
    qname: String,
    /// Kind of the matched node (METHOD / FUNCTION / CLASS).
    kind_label: &'static str,
}

const STOP_IDENTS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
    "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with",
    "yield", "True", "False", "None", "self", "cls", "test", "tests", "py", "pytest",
    "fixture", "parametrize", "approx", "raises", "warns", "match", "case",
];

fn main() -> Result<()> {
    let args = Args::parse();

    let patch_text = std::fs::read_to_string(&args.test_patch)
        .with_context(|| format!("read test_patch {}", args.test_patch.display()))?;
    let hunks = parse_test_patch(&patch_text)?;
    eprintln!(
        "[synth_test_expectation] parsed {} hunks across {} test_patch files",
        hunks.len(),
        hunks
            .iter()
            .map(|h| h.new_file.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
    );
    for h in &hunks {
        eprintln!(
            "  · {}:{}+ tests=[{}] body={}b",
            h.new_file,
            h.new_start_line,
            h.test_fns.join(","),
            h.added_body.len(),
        );
    }

    if hunks.is_empty() {
        write_empty_directive(&args.text_out, "no parseable hunks in test_patch")?;
        return Ok(());
    }

    let graph = build_repo_graph(&args.src, &args.repo_canonical)
        .context("build_repo_graph failed")?;
    eprintln!(
        "[synth_test_expectation] graph: {} nodes",
        graph.nodes.len()
    );

    let tail_idx = build_tail_index(&graph);
    let mut idents: BTreeSet<String> = BTreeSet::new();
    let mut all_test_fns: BTreeSet<String> = BTreeSet::new();
    for h in &hunks {
        for ident in extract_identifiers(&h.added_body) {
            idents.insert(ident);
        }
        for f in &h.test_fns {
            all_test_fns.insert(f.clone());
        }
    }
    eprintln!(
        "[synth_test_expectation] {} unique identifiers in test bodies, {} test fns",
        idents.len(),
        all_test_fns.len(),
    );

    let mut resolved: Vec<ResolvedSymbol> = Vec::new();
    let mut seen_qnames: HashSet<String> = HashSet::new();
    for ident in &idents {
        // Skip the test functions themselves — they don't live in the graph
        // (test_patch ADDS them) and pointing the model at them is wrong.
        if all_test_fns.contains(ident) {
            continue;
        }
        let Some(matches) = tail_idx.get(ident.as_str()) else {
            continue;
        };
        // Prefer the shortest qname (least nested) as the canonical resolution.
        // Tie-break by lexical order for determinism.
        let mut sorted = matches.clone();
        sorted.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| a.1.cmp(b.1)));
        let (nid, qn) = sorted[0];
        if !seen_qnames.insert(qn.to_string()) {
            continue;
        }
        let kind_label = match graph.nav.kind_by_id.get(&nid).copied() {
            Some(k) if k == node_kind::METHOD => "method",
            Some(k) if k == node_kind::FUNCTION => "function",
            Some(k) if k == node_kind::CLASS => "class",
            _ => "symbol",
        };
        resolved.push(ResolvedSymbol {
            name: ident.clone(),
            qname: qn.to_string(),
            kind_label,
        });
    }
    eprintln!(
        "[synth_test_expectation] resolved {} of {} identifiers to graph qnames",
        resolved.len(),
        idents.len(),
    );

    if args.max_targets > 0 && resolved.len() > args.max_targets {
        // Heuristic ranking: prefer METHOD over CLASS over FUNCTION (the fix
        // usually lands in a method), then prefer shorter qnames (closer to
        // the public surface). Stable sort.
        resolved.sort_by(|a, b| {
            kind_priority(a.kind_label)
                .cmp(&kind_priority(b.kind_label))
                .then_with(|| a.qname.len().cmp(&b.qname.len()))
                .then_with(|| a.qname.cmp(&b.qname))
        });
        resolved.truncate(args.max_targets);
    }

    if resolved.is_empty() {
        write_empty_directive(&args.text_out, "no test_patch identifiers matched any graph qname")?;
        return Ok(());
    }

    let directive = render_directive(&hunks, &all_test_fns, &resolved);
    if let Some(parent) = args.text_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.text_out, directive.as_bytes())?;
    eprintln!(
        "[synth_test_expectation] wrote directive: {} ({} resolved targets, {} test fns)",
        args.text_out.display(),
        resolved.len(),
        all_test_fns.len(),
    );
    Ok(())
}

fn parse_test_patch(patch: &str) -> Result<Vec<TestPatchHunk>> {
    let file_re = Regex::new(r"^\+\+\+ b/(.+)$")?;
    let hunk_re = Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$")?;
    let def_re = Regex::new(r"def\s+(test_[A-Za-z_][A-Za-z0-9_]*)\s*\(")?;

    let mut out: Vec<TestPatchHunk> = Vec::new();
    let mut cur_file: Option<String> = None;
    let mut cur_hunk: Option<TestPatchHunk> = None;
    for line in patch.lines() {
        if let Some(c) = file_re.captures(line) {
            // Flush any in-progress hunk before switching file.
            if let Some(h) = cur_hunk.take() {
                if !h.added_body.is_empty() {
                    out.push(h);
                }
            }
            cur_file = Some(c.get(1).unwrap().as_str().to_string());
            continue;
        }
        if let Some(c) = hunk_re.captures(line) {
            if let Some(h) = cur_hunk.take() {
                if !h.added_body.is_empty() {
                    out.push(h);
                }
            }
            let Some(file) = cur_file.clone() else { continue };
            let new_start: u32 = c.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
            cur_hunk = Some(TestPatchHunk {
                new_file: file,
                new_start_line: new_start,
                added_body: String::new(),
                test_fns: Vec::new(),
            });
            continue;
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(h) = cur_hunk.as_mut() {
            if let Some(stripped) = line.strip_prefix('+') {
                h.added_body.push_str(stripped);
                h.added_body.push('\n');
                if let Some(c) = def_re.captures(stripped) {
                    h.test_fns.push(c.get(1).unwrap().as_str().to_string());
                }
            }
        }
    }
    if let Some(h) = cur_hunk.take() {
        if !h.added_body.is_empty() {
            out.push(h);
        }
    }
    Ok(out)
}

fn extract_identifiers(body: &str) -> Vec<String> {
    let stop: HashSet<&str> = STOP_IDENTS.iter().copied().collect();
    let ident_re = Regex::new(r"[A-Za-z_][A-Za-z0-9_]+").unwrap();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for m in ident_re.find_iter(body) {
        let t = m.as_str();
        if t.len() >= 2 && !stop.contains(t) {
            out.insert(t.to_string());
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
        "method" => 0,
        "class" => 1,
        "function" => 2,
        _ => 3,
    }
}

fn render_directive(
    hunks: &[TestPatchHunk],
    test_fns: &BTreeSet<String>,
    resolved: &[ResolvedSymbol],
) -> String {
    let mut s = String::new();
    s.push_str("\n## Required fix target (graph-derived from test_patch expectations)\n\n");
    s.push_str(
        "The SWE-bench failing-to-passing test bodies (added in `test_patch.patch`) \
         exercise the symbols listed below. The implementation must change so these \
         tests pass — the directive below names the implementation-side functions, \
         classes, and methods that the tests assert on. Edit ONE of these (whichever \
         contains the bug), not the test_patch itself.\n\n",
    );

    // Files touched.
    let files: BTreeSet<&str> = hunks.iter().map(|h| h.new_file.as_str()).collect();
    if !files.is_empty() {
        s.push_str("Tests added to:\n");
        for f in &files {
            s.push_str(&format!("- `{f}`\n"));
        }
        s.push('\n');
    }

    // Test function names.
    if !test_fns.is_empty() {
        s.push_str("New test function(s) that must pass:\n");
        for f in test_fns {
            s.push_str(&format!("- `{f}`\n"));
        }
        s.push('\n');
    }

    // Resolved targets — the implementation surface the tests reach.
    s.push_str("Implementation-side targets (graph-resolved from test body identifiers):\n");
    for r in resolved {
        s.push_str(&format!(
            "- `{}` ({}, referenced as `{}` in the new tests)\n",
            r.qname, r.kind_label, r.name,
        ));
    }
    s.push('\n');

    s.push_str(
        "Pick the SINGLE target above whose behavior most directly determines the \
         test's assertion outcome — that is where the fix lands. Do NOT modify the \
         test files themselves. Emit a minimal unified diff against the \
         implementation file(s).\n",
    );
    s
}

fn write_empty_directive(path: &PathBuf, reason: &str) -> Result<()> {
    let s = format!(
        "\n## Required fix target (graph-derived from test_patch expectations)\n\n\
         (no test_patch directive: {reason})\n",
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, s.as_bytes())?;
    eprintln!(
        "[synth_test_expectation] wrote empty directive: {} (reason: {reason})",
        path.display()
    );
    Ok(())
}
