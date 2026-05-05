//! synth_call_chain — forward CALLS-edge reachability from F2P test methods
//! and (optionally) from production symbols referenced in the F2P test body.
//!
//! Step 1 of #77 (Phase 2.6). Walks the in-memory graph forward via
//! `edge_category::CALLS` from each FAIL_TO_PASS test method, depth-limited.
//! Emits an ordered set of (qname, depth, anchor_label) records that
//! downstream synth bins (synth_composition, synth_key_symbols) consume to
//! boost cells whose enclosing method is in-chain.
//!
//! Per `project_0426_step0_discovery.md`: 9/10 of our N=20 misses need
//! forward call-chain reachability from the F2P method, not Joern-style
//! REACHING_DEF. Glia already has CALLS edges; this bin is the missing
//! query primitive.
//!
//! Per `project_0426_step1_chain_sparsity.md`: F2P-method anchors are usually
//! 0-length because glia's CALLS edges don't cover constructor calls or
//! attribute calls on parameters — exactly the patterns test bodies use. The
//! `--test-patch` arg expands the anchor set: identifier candidates from `+`
//! lines of the test_patch are tail-name-matched against METHOD/FUNCTION/CLASS
//! qnames in the graph, and each match becomes an additional BFS anchor.
//! CLASS anchors expand to their methods (CLASS has no outgoing CALLS itself).
//!
//! Anchor-resolution: F2P labels look like
//! `path/to/test.py::Class::method[parametrize]`. We strip the parametrize
//! marker + `.py`, then suffix-match against METHOD/FUNCTION qnames in the
//! graph, preferring matches whose qname contains a `tests` segment.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;
use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::NodeId;
use repo_graph_projection_text::driver_utils::build_repo_graph;
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(about = "Forward CALLS-chain from F2P methods + test_patch-referenced production symbols")]
struct Args {
    /// Repo root to walk for `*.py` files.
    #[arg(long)]
    src: PathBuf,

    /// FAIL_TO_PASS labels. Either a path to a JSON file containing a
    /// `["label", ...]` list, or a comma-separated inline string.
    #[arg(long)]
    f2p: String,

    /// Optional unified-diff test_patch. Identifiers from `+` lines that
    /// match METHOD/FUNCTION/CLASS qname tails become extra BFS anchors.
    #[arg(long)]
    test_patch: Option<PathBuf>,

    /// Output JSON path.
    #[arg(long)]
    out: PathBuf,

    /// Max BFS depth from each anchor.
    #[arg(long, default_value_t = 4)]
    max_depth: usize,

    /// Drop test_patch tokens with more than this many graph matches (noise).
    #[arg(long, default_value_t = 5)]
    match_cap: usize,

    /// Canonical repo identifier passed to `build_repo_graph`.
    #[arg(long, default_value = "synth-call-chain")]
    repo_canonical: String,
}

#[derive(Serialize, Deserialize)]
struct ChainEntry {
    qname: String,
    depth: usize,
}

#[derive(Serialize, Deserialize)]
struct AnchorChain {
    anchor_label: String,
    anchor_qname: Option<String>,
    methods: Vec<ChainEntry>,
}

#[derive(Serialize, Deserialize)]
struct ChainOutput {
    max_depth: usize,
    chains: Vec<AnchorChain>,
}

fn parse_f2p(spec: &str) -> Result<Vec<String>> {
    let p = std::path::Path::new(spec);
    if p.is_file() {
        let bytes = fs::read(p).with_context(|| format!("read {}", p.display()))?;
        let labels: Vec<String> =
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", p.display()))?;
        Ok(labels)
    } else {
        Ok(spec
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }
}

/// `tests/test_x.py::Class::method[param]` → `test_x::Class::method`.
fn label_to_qname_suffix(label: &str) -> String {
    let no_param = label.split('[').next().unwrap_or(label);
    let parts: Vec<&str> = no_param.split("::").collect();
    if parts.is_empty() {
        return String::new();
    }
    let file_part = parts[0];
    let file_stem = std::path::Path::new(file_part)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_part);
    let mut chunks: Vec<&str> = vec![file_stem];
    for p in &parts[1..] {
        chunks.push(p);
    }
    chunks.join("::")
}

fn find_f2p_anchor(
    graph: &repo_graph_graph::RepoGraph,
    suffix: &str,
) -> Option<(NodeId, String)> {
    if suffix.is_empty() {
        return None;
    }
    let suffix_with_sep = format!("::{suffix}");

    let mut method_matches: Vec<(NodeId, &str)> = Vec::new();
    let mut function_matches: Vec<(NodeId, &str)> = Vec::new();
    for n in &graph.nodes {
        let Some(&kind) = graph.nav.kind_by_id.get(&n.id) else {
            continue;
        };
        let is_method = kind == node_kind::METHOD;
        let is_function = kind == node_kind::FUNCTION;
        if !is_method && !is_function {
            continue;
        }
        let Some(qn) = graph.nav.qname_by_id.get(&n.id) else {
            continue;
        };
        let qn_str = qn.as_str();
        if qn_str == suffix || qn_str.ends_with(&suffix_with_sep) {
            if is_method {
                method_matches.push((n.id, qn_str));
            } else {
                function_matches.push((n.id, qn_str));
            }
        }
    }

    let mut all_matches: Vec<(NodeId, &str)> = Vec::new();
    all_matches.extend(method_matches);
    all_matches.extend(function_matches);
    if all_matches.is_empty() {
        return None;
    }

    let mut test_located: Vec<(NodeId, &str)> = all_matches
        .iter()
        .copied()
        .filter(|(_, q)| {
            q.split("::")
                .any(|seg| seg == "tests" || seg == "test" || seg.starts_with("test_"))
        })
        .collect();
    if !test_located.is_empty() {
        test_located.sort_by_key(|(_, q)| q.len());
        let pick = test_located[0];
        return Some((pick.0, pick.1.to_string()));
    }
    all_matches.sort_by_key(|(_, q)| q.len());
    let pick = all_matches[0];
    Some((pick.0, pick.1.to_string()))
}

/// Identifier candidates from added (`+`-prefixed) lines of a unified diff.
/// Stop-list mirrors `seeds.rs::extract_candidates_from_test_patch` — Python
/// keywords plus a few diff/test artifacts.
fn extract_candidates_from_test_patch(patch: &str) -> BTreeSet<String> {
    const STOP: &[&str] = &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is",
        "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with",
        "yield", "True", "False", "None", "self", "cls", "test", "tests", "py",
    ];
    let stop: HashSet<&str> = STOP.iter().copied().collect();
    let ident = Regex::new(r"[A-Za-z_][A-Za-z0-9_]+").unwrap();

    let mut out = BTreeSet::new();
    for line in patch.lines() {
        if !line.starts_with('+') || line.starts_with("+++") {
            continue;
        }
        let body = &line[1..];
        for m in ident.find_iter(body) {
            let t = m.as_str();
            if t.len() >= 2 && !stop.contains(t) {
                out.insert(t.to_string());
            }
        }
    }
    out
}

/// Map: qname-tail (simple name) → Vec<(NodeId, qname)>, restricted to
/// METHOD / FUNCTION / CLASS. Iterate `graph.nodes` (deterministic Vec) not
/// `qname_by_id` (HashMap with randomized hasher).
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

/// Class anchors expand to their methods (a CLASS node has no outgoing CALLS).
/// Returns each method's NodeId + qname.
fn expand_class_to_methods<'g>(
    graph: &'g repo_graph_graph::RepoGraph,
    class_id: NodeId,
) -> Vec<(NodeId, &'g str)> {
    let Some(methods_map) = graph.symbols.class_methods.get(&class_id) else {
        return Vec::new();
    };
    // Iterate `methods_map` then sort by qname for deterministic order — the
    // map itself is HashMap-backed.
    let mut out: Vec<(NodeId, &str)> = methods_map
        .values()
        .filter_map(|mid| {
            let q = graph.nav.qname_by_id.get(mid)?.as_str();
            Some((*mid, q))
        })
        .collect();
    out.sort_by_key(|(_, q)| *q);
    out
}

/// BFS forward via CALLS, recording depth. Start node excluded.
fn call_chain(
    graph: &repo_graph_graph::RepoGraph,
    start: NodeId,
    max_depth: usize,
) -> Vec<(NodeId, usize)> {
    let mut visited: HashSet<NodeId> = HashSet::from([start]);
    let mut out: Vec<(NodeId, usize)> = Vec::new();
    let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(start, 0)]);
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        for e in &graph.edges {
            if e.from != node {
                continue;
            }
            if e.category != edge_category::CALLS {
                continue;
            }
            if visited.insert(e.to) {
                out.push((e.to, depth + 1));
                queue.push_back((e.to, depth + 1));
            }
        }
    }
    out
}

fn methods_from_reachable(
    graph: &repo_graph_graph::RepoGraph,
    reachable: Vec<(NodeId, usize)>,
) -> Vec<ChainEntry> {
    reachable
        .into_iter()
        .filter_map(|(nid, depth)| {
            let kind = graph.nav.kind_by_id.get(&nid).copied()?;
            if kind != node_kind::METHOD && kind != node_kind::FUNCTION {
                return None;
            }
            let q = graph.nav.qname_by_id.get(&nid)?.clone();
            Some(ChainEntry { qname: q, depth })
        })
        .collect()
}

fn main() -> Result<()> {
    let args = Args::parse();

    let graph = build_repo_graph(&args.src, &args.repo_canonical)?;

    let labels = parse_f2p(&args.f2p)?;
    eprintln!("[f2p] {} labels", labels.len());

    // Track which anchor qnames we've already chained to avoid duplicate BFS.
    let mut anchored: HashSet<String> = HashSet::new();
    let mut chains: Vec<AnchorChain> = Vec::new();

    // (1) F2P-method anchors.
    for label in &labels {
        let suffix = label_to_qname_suffix(label);
        match find_f2p_anchor(&graph, &suffix) {
            None => {
                eprintln!("[f2p-anchor] MISS {label} (suffix={suffix})");
                chains.push(AnchorChain {
                    anchor_label: format!("f2p:{label}"),
                    anchor_qname: None,
                    methods: Vec::new(),
                });
            }
            Some((id, qn)) => {
                if anchored.insert(qn.clone()) {
                    let reachable = call_chain(&graph, id, args.max_depth);
                    eprintln!(
                        "[f2p-anchor] HIT {label} → {qn} (chain {} methods)",
                        reachable.len()
                    );
                    let methods = methods_from_reachable(&graph, reachable);
                    chains.push(AnchorChain {
                        anchor_label: format!("f2p:{label}"),
                        anchor_qname: Some(qn),
                        methods,
                    });
                } else {
                    eprintln!("[f2p-anchor] DUP {label} → {qn} (already anchored)");
                }
            }
        }
    }

    // (2) test_patch-derived production-symbol anchors.
    if let Some(tp_path) = &args.test_patch {
        let patch_text = fs::read_to_string(tp_path)
            .with_context(|| format!("read {}", tp_path.display()))?;
        let candidates = extract_candidates_from_test_patch(&patch_text);
        eprintln!("[test_patch] {} candidate identifiers", candidates.len());

        let tail_index = build_tail_index(&graph);

        let mut accepted_tokens: Vec<String> = Vec::new();
        let mut dropped_tokens: Vec<String> = Vec::new();
        let mut tp_anchor_pool: Vec<(String, NodeId, String)> = Vec::new();

        for tok in &candidates {
            let Some(matches) = tail_index.get(tok.as_str()) else {
                continue;
            };
            if matches.len() > args.match_cap {
                dropped_tokens.push(format!("{tok}({})", matches.len()));
                continue;
            }
            accepted_tokens.push(tok.clone());
            for (id, qn) in matches {
                tp_anchor_pool.push((tok.clone(), *id, (*qn).to_string()));
            }
        }

        eprintln!(
            "[test_patch] {} tokens accepted ({} dropped over cap={})",
            accepted_tokens.len(),
            dropped_tokens.len(),
            args.match_cap
        );
        if !dropped_tokens.is_empty() {
            eprintln!(
                "[test_patch] dropped: {}",
                dropped_tokens.join(", ")
            );
        }

        for (tok, anchor_id, anchor_qn) in tp_anchor_pool {
            let kind = graph.nav.kind_by_id.get(&anchor_id).copied();
            // CLASS anchors → expand to methods, BFS each method.
            let bfs_starts: Vec<(NodeId, String)> = if kind == Some(node_kind::CLASS) {
                let methods = expand_class_to_methods(&graph, anchor_id);
                if methods.is_empty() {
                    eprintln!(
                        "[tp-anchor] {tok} → {anchor_qn} (CLASS, 0 methods, skipped)"
                    );
                    continue;
                }
                methods
                    .into_iter()
                    .map(|(mid, mqn)| (mid, mqn.to_string()))
                    .collect()
            } else {
                vec![(anchor_id, anchor_qn.clone())]
            };

            for (start_id, start_qn) in bfs_starts {
                if !anchored.insert(start_qn.clone()) {
                    continue;
                }
                let reachable = call_chain(&graph, start_id, args.max_depth);
                eprintln!(
                    "[tp-anchor] {tok} → {start_qn} (chain {} methods)",
                    reachable.len()
                );
                let methods = methods_from_reachable(&graph, reachable);
                chains.push(AnchorChain {
                    anchor_label: format!("test_patch:{tok}"),
                    anchor_qname: Some(start_qn),
                    methods,
                });
            }
        }
    }

    let total: usize = chains.iter().map(|c| c.methods.len()).sum();
    eprintln!(
        "[chain] {} anchors, {} method records (depth ≤ {})",
        chains.len(),
        total,
        args.max_depth
    );

    let out = ChainOutput {
        max_depth: args.max_depth,
        chains,
    };
    fs::write(&args.out, serde_json::to_vec_pretty(&out)?)
        .with_context(|| format!("write {}", args.out.display()))?;
    eprintln!("[write] {}", args.out.display());

    Ok(())
}
