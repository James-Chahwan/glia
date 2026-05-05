//! synth_callsite_argflow — usage-typed polymorphism cell synthesiser.
//!
//! For each polymorphic method simple-name (defined on ≥2 classes in the
//! graph), scan every activated METHOD/FUNCTION body via AST re-walk and
//! collect calls matching that simple-name. Record (caller_class, receiver,
//! positional-args) per call; when ≥2 distinct caller-classes (or receivers)
//! are observed, emit a `callsite-argflow` cell that enumerates the
//! call-site/receiver polymorphism. Appends the cells to an existing
//! summaries-hybrid JSON so the Path B latent pipeline picks them up.
//!
//! Rationale: marshmallow's `_bind_to_schema` is called from `List`, `Tuple`,
//! and `Schema._bind_field`; each passes `self` (different class every time)
//! as the `schema` argument to the inner field's bind. The graph's CALLS
//! edges don't carry arg info and many receivers fall into `ComplexReceiver`
//! at resolve time, so the polymorphism facts aren't graph-edge-derivable.
//! This bin re-walks the AST for the activated methods' bodies to recover the
//! facts as auto-synthesised prose — candidate for closing the B2.2 oracle
//! bullet automatically.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use repo_graph_code_domain::node_kind;
use repo_graph_core::NodeId;
use repo_graph_parser_python::extract_calls_with_args;
use repo_graph_projection_text::driver_utils::{
    build_repo_graph, extract_code_cell, node_by_qname_index, reverse_qname_index,
};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(about = "Synthesise callsite-argflow cells for usage-typed polymorphism")]
struct Args {
    /// Repo root to walk for `*.py` files.
    #[arg(long)]
    src: PathBuf,

    /// Seeds JSON: top-level `activated: [[qname, score], ...]`.
    #[arg(long)]
    seeds: PathBuf,

    /// Existing summaries-hybrid JSON to append to.
    #[arg(long)]
    summaries: PathBuf,

    /// Output path for the augmented summaries JSON.
    #[arg(long)]
    out: PathBuf,

    /// Canonical repo identifier.
    #[arg(long, default_value = "synth-callsite-argflow")]
    repo_canonical: String,

    /// First synthetic cell id. Picks a high water mark to avoid collision with
    /// existing summary ids.
    #[arg(long, default_value_t = 20_000_000)]
    id_start: u64,
}

#[derive(Deserialize)]
struct SeedsFile {
    activated: Vec<(String, f64)>,
}

#[derive(Serialize, Deserialize, Clone)]
struct SummaryEntry {
    id: u64,
    qname: String,
    score: f64,
    summary: String,
}

#[derive(Debug, Clone)]
struct CallerRecord {
    caller_qname: String,
    caller_class_simple: String,
    receiver_text: String,
    args_resolved: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let graph = build_repo_graph(&args.src, &args.repo_canonical)?;

    let seeds_bytes = fs::read(&args.seeds)
        .with_context(|| format!("read {}", args.seeds.display()))?;
    let seeds: SeedsFile = serde_json::from_slice(&seeds_bytes).context("parse seeds JSON")?;
    eprintln!("[seeds] {} activated qnames", seeds.activated.len());

    let qname_to_id = reverse_qname_index(&graph);
    let mut activated: Vec<NodeId> = Vec::new();
    for (qname, _) in &seeds.activated {
        if let Some(&id) = qname_to_id.get(qname.as_str()) {
            activated.push(id);
        }
    }
    let activated_set: HashSet<NodeId> = activated.iter().copied().collect();
    eprintln!("[resolve] {} activated qnames matched", activated_set.len());

    // All METHODs in the graph, grouped by simple name. We need the global view
    // (not just activated) to decide whether a simple-name is polymorphic.
    let mut simple_to_all_methods: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
    for (id, kind) in &graph.nav.kind_by_id {
        if *kind != node_kind::METHOD {
            continue;
        }
        let Some(qname) = graph.nav.qname_by_id.get(id) else {
            continue;
        };
        let simple = qname
            .rsplit("::")
            .next()
            .unwrap_or(qname.as_str())
            .to_string();
        simple_to_all_methods.entry(simple).or_default().push(*id);
    }
    // Determinism: per-name Vecs got NodeIds in graph.nav.kind_by_id (HashMap)
    // iter order. Sort each so polymorphism check + downstream caller iteration
    // stay deterministic.
    for v in simple_to_all_methods.values_mut() {
        v.sort_by_key(|id| id.0);
    }

    // Polymorphic simple-names: defined on ≥2 classes across the graph.
    let polymorphic: BTreeSet<String> = simple_to_all_methods
        .iter()
        .filter(|(_, ids)| ids.len() >= 2)
        .map(|(k, _)| k.clone())
        .collect();
    eprintln!(
        "[polymorphism] {} polymorphic simple-names (defined on ≥2 classes)",
        polymorphic.len()
    );

    // qname → &Node for CODE cell extraction.
    let node_by_qname = node_by_qname_index(&graph);

    // Walk every activated METHOD/FUNCTION body; extract calls; group by callee simple-name.
    // Determinism: iterate `activated` Vec (deterministic from seeds) not
    // `activated_set` HashSet — the latter's iter order shuffles per_name
    // bucket Vec content order across runs.
    let mut per_name: BTreeMap<String, Vec<CallerRecord>> = BTreeMap::new();
    for caller_id in &activated {
        let Some(&kind) = graph.nav.kind_by_id.get(caller_id) else {
            continue;
        };
        if kind != node_kind::METHOD && kind != node_kind::FUNCTION {
            continue;
        }
        let Some(caller_qname) = graph.nav.qname_by_id.get(caller_id) else {
            continue;
        };
        let Some(caller_node) = node_by_qname.get(caller_qname.as_str()) else {
            continue;
        };
        let Some(src) = extract_code_cell(caller_node) else {
            continue;
        };

        let caller_class_simple = graph
            .nav
            .parent_of
            .get(caller_id)
            .and_then(|c| graph.nav.qname_by_id.get(c))
            .map(|q| q.rsplit("::").next().unwrap_or(q.as_str()).to_string())
            .unwrap_or_default();

        for call in extract_calls_with_args(src) {
            if !polymorphic.contains(&call.callee_simple_name) {
                continue;
            }
            let args_resolved: Vec<String> = call
                .args
                .iter()
                .map(|a| {
                    if a == "self" && !caller_class_simple.is_empty() {
                        format!("self (={})", caller_class_simple)
                    } else {
                        a.clone()
                    }
                })
                .collect();
            per_name
                .entry(call.callee_simple_name.clone())
                .or_default()
                .push(CallerRecord {
                    caller_qname: caller_qname.clone(),
                    caller_class_simple: caller_class_simple.clone(),
                    receiver_text: call.receiver_text,
                    args_resolved,
                });
        }
    }

    // Emit one cell per polymorphic simple-name whose observed callers show
    // ≥2 distinct caller_class OR ≥2 distinct receiver_text.
    let mut emitted: Vec<SummaryEntry> = Vec::new();
    let mut next_id: u64 = args.id_start;
    for (simple_name, mut callers) in per_name {
        // Dedupe identical records.
        let mut seen: BTreeSet<(String, String, Vec<String>)> = BTreeSet::new();
        callers.retain(|r| {
            seen.insert((
                r.caller_qname.clone(),
                r.receiver_text.clone(),
                r.args_resolved.clone(),
            ))
        });

        let distinct_classes: BTreeSet<&str> = callers
            .iter()
            .map(|r| r.caller_class_simple.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        let distinct_receivers: BTreeSet<&str> =
            callers.iter().map(|r| r.receiver_text.as_str()).collect();
        if distinct_classes.len() < 2 && distinct_receivers.len() < 2 {
            continue;
        }

        let peers = simple_to_all_methods.get(&simple_name).unwrap();
        let peer_qnames: Vec<String> = peers
            .iter()
            .filter_map(|id| graph.nav.qname_by_id.get(id).cloned())
            .collect();
        let peer_classes: BTreeSet<String> = peer_qnames
            .iter()
            .filter_map(|q| q.rsplit("::").nth(1).map(|s| s.to_string()))
            .collect();

        // Primary qname: first activated peer if any, else first peer.
        let primary_qname = peer_qnames
            .iter()
            .find(|q| {
                qname_to_id
                    .get(q.as_str())
                    .is_some_and(|id| activated_set.contains(id))
            })
            .cloned()
            .unwrap_or_else(|| peer_qnames.first().cloned().unwrap_or_default());

        let mut text = String::new();
        text.push_str(&format!(
            "# Callsite arg-flow for polymorphic method `{simple_name}` (derived from AST call re-walk)\n"
        ));
        text.push_str(&format!(
            "# Defined on classes: {}\n",
            peer_classes.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
        for r in &callers {
            let recv = if r.receiver_text.is_empty() {
                "<bare>".to_string()
            } else {
                r.receiver_text.clone()
            };
            let args_str = r.args_resolved.join(", ");
            text.push_str(&format!(
                "# In `{}`: `{}.{}({})`\n",
                r.caller_qname, recv, simple_name, args_str
            ));
        }

        emitted.push(SummaryEntry {
            id: next_id,
            qname: primary_qname,
            score: 0.0,
            summary: text,
        });
        next_id += 1;
    }

    eprintln!("[synth] {} callsite-argflow cells emitted", emitted.len());

    let existing: Vec<SummaryEntry> = serde_json::from_slice(
        &fs::read(&args.summaries)
            .with_context(|| format!("read {}", args.summaries.display()))?,
    )
    .context("parse summaries JSON")?;
    let mut out: Vec<SummaryEntry> = existing;
    out.extend(emitted);
    fs::write(&args.out, serde_json::to_vec_pretty(&out)?)
        .with_context(|| format!("write {}", args.out.display()))?;
    eprintln!("[write] {}", args.out.display());

    Ok(())
}
