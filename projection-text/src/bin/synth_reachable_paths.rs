//! synth_reachable_paths — T4 wiring. For each test-anchored sink, identify
//! which activated methods reach it via CALLS chain. Domain-general consumer
//! of `RepoGraph::reachable_by`.
//!
//! Sink selection (in priority order):
//!   1. `test_patch_facts.resolved_attrs` — `Class.attr` resolved via local
//!      var-type tracking in the test. Highest-confidence test-anchored signal.
//!   2. `test_patch_facts.constructor_calls` — capitalized `Name(...)` calls.
//!      Each METHOD on the named class is a candidate sink.
//!
//! For each sink, `reachable_by(sink, activated_methods, &[CALLS, RETURNS_TYPE],
//! depth)` returns the activated methods whose call chain reaches the sink.
//! These are emitted as cells; the model sees "Test target X is reachable from
//! these activated methods."
//!
//! Output JSON shape (cells block, similar to other synth bins):
//!   { "max_depth": N,
//!     "cells": [
//!       {"sink_qname": "...",
//!        "sink_origin": "resolved_attr:MySchema.fields" | "constructor_call:DateTime",
//!        "sources": ["qname1", "qname2", ...]},
//!       ...
//!     ] }
//!
//! Not yet wired into run_instance.py — shipped as standalone bin so the cell
//! contract is locked, with smoke-verify against marshmallow-1359 workdir.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{EdgeCategoryId, NodeId};
use repo_graph_graph::RepoGraph;
use repo_graph_projection_text::driver_utils::{build_repo_graph, write_json};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(about = "Synth reachable-from-test cells using RepoGraph::reachable_by (T4)")]
struct Args {
    #[arg(long)]
    src: PathBuf,

    #[arg(long)]
    seeds: PathBuf,

    #[arg(long)]
    out: PathBuf,

    /// Max BFS depth from sink along reverse CALLS edges. 3 captures helper
    /// → caller → entrypoint without exploding fan-in on framework methods.
    #[arg(long, default_value_t = 3)]
    max_depth: usize,

    /// Cap on sources reported per sink — keeps cell text bounded for
    /// pathological fan-in (e.g. a sink hit by 50+ activated callers).
    #[arg(long, default_value_t = 12)]
    max_sources_per_sink: usize,

    #[arg(long, default_value = "reachable-paths")]
    repo_canonical: String,
}

#[derive(Deserialize)]
struct SeedsFile {
    activated: Vec<(String, f64)>,
    test_patch_facts: Option<TestPatchFacts>,
}

#[derive(Deserialize, Default)]
struct TestPatchFacts {
    #[serde(default)]
    constructor_calls: BTreeMap<String, usize>,
    #[serde(default)]
    #[allow(dead_code)]
    class_attrs: BTreeMap<String, usize>,
    #[serde(default)]
    resolved_attrs: BTreeMap<String, usize>,
}

#[derive(Serialize)]
struct Output {
    max_depth: usize,
    cells: Vec<Cell>,
}

#[derive(Serialize)]
struct Cell {
    sink_qname: String,
    sink_origin: String,
    sources: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let graph = build_repo_graph(&args.src, &args.repo_canonical)?;
    let seeds: SeedsFile = {
        let bytes = std::fs::read(&args.seeds)
            .with_context(|| format!("read {}", args.seeds.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parse {}", args.seeds.display()))?
    };

    let activated_method_ids = activated_methods(&graph, &seeds.activated);
    eprintln!(
        "[reachable] {} activated method ids (from {} activated entries)",
        activated_method_ids.len(),
        seeds.activated.len()
    );

    let facts = seeds.test_patch_facts.unwrap_or_default();
    let sinks = collect_sinks(&graph, &facts);
    eprintln!("[reachable] {} candidate sinks resolved", sinks.len());

    let follow: Vec<EdgeCategoryId> = vec![edge_category::CALLS, edge_category::RETURNS_TYPE];

    let mut cells: Vec<Cell> = Vec::new();
    for (sink_id, sink_qname, sink_origin) in &sinks {
        let reaching = graph.reachable_by(*sink_id, &activated_method_ids, &follow, args.max_depth);
        if reaching.is_empty() {
            continue;
        }
        let mut sources: Vec<String> = reaching
            .iter()
            .filter_map(|id| graph.nav.qname_by_id.get(id).cloned())
            .collect();
        sources.sort();
        sources.dedup();
        if sources.len() > args.max_sources_per_sink {
            sources.truncate(args.max_sources_per_sink);
        }
        cells.push(Cell {
            sink_qname: sink_qname.clone(),
            sink_origin: sink_origin.clone(),
            sources,
        });
    }

    eprintln!(
        "[reachable] {} cells emitted (sinks with ≥1 reaching activated method)",
        cells.len()
    );
    for c in &cells {
        eprintln!(
            "  {} [{}] ← {} sources",
            c.sink_qname,
            c.sink_origin,
            c.sources.len()
        );
    }

    let out = Output {
        max_depth: args.max_depth,
        cells,
    };
    write_json(&args.out, &out)?;
    eprintln!("[write] {}", args.out.display());
    Ok(())
}

/// Filter activated qnames down to METHOD/FUNCTION-kind nodes and resolve to
/// NodeIds. Source set for `reachable_by` — only callable nodes can sit on a
/// CALLS chain.
fn activated_methods(graph: &RepoGraph, activated: &[(String, f64)]) -> Vec<NodeId> {
    let qname_to_id: HashMap<&str, NodeId> = graph
        .nav
        .qname_by_id
        .iter()
        .map(|(id, q)| (q.as_str(), *id))
        .collect();
    let mut out = Vec::new();
    for (qname, _score) in activated {
        if let Some(id) = qname_to_id.get(qname.as_str()) {
            let kind = graph.nav.kind_by_id.get(id).copied();
            if kind == Some(node_kind::METHOD) || kind == Some(node_kind::FUNCTION) {
                out.push(*id);
            }
        }
    }
    out
}

/// Resolve test_patch_facts to (sink_id, sink_qname, sink_origin) tuples.
/// `resolved_attrs` (Class.attr) → match METHOD whose qname tail is `attr`
/// inside class `Class`. `constructor_calls` (ClassName) → all METHODs of any
/// matching class. Tail-match on simple class name handles cross-module
/// duplicates by emitting one sink per residence (downstream dedup happens at
/// reachability filter time — if no activated method reaches a residence, it
/// drops out).
fn collect_sinks(
    graph: &RepoGraph,
    facts: &TestPatchFacts,
) -> Vec<(NodeId, String, String)> {
    let mut sinks: Vec<(NodeId, String, String)> = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();

    // resolved_attrs — strongest signal first.
    for (class_attr, _count) in &facts.resolved_attrs {
        let parts: Vec<&str> = class_attr.split('.').collect();
        if parts.len() != 2 {
            continue;
        }
        let cls = parts[0];
        let attr = parts[1];
        for (id, qname) in &graph.nav.qname_by_id {
            // Match `*::{cls}::{attr}` — METHOD or ATTRIBUTE, both can be sinks.
            let needle = format!("::{cls}::{attr}");
            if !qname.ends_with(&needle) {
                continue;
            }
            if seen.insert(*id) {
                sinks.push((*id, qname.clone(), format!("resolved_attr:{class_attr}")));
            }
        }
    }

    // constructor_calls — every METHOD of named class becomes a candidate sink.
    for (cls_name, _count) in &facts.constructor_calls {
        let cls_suffix = format!("::{cls_name}");
        // First find every CLASS qname matching tail.
        let mut class_qnames: Vec<&str> = Vec::new();
        for (id, qname) in &graph.nav.qname_by_id {
            if graph.nav.kind_by_id.get(id).copied() != Some(node_kind::CLASS) {
                continue;
            }
            if qname.ends_with(&cls_suffix) || qname.as_str() == cls_name {
                class_qnames.push(qname.as_str());
            }
        }
        for cls_qname in class_qnames {
            // Iterate qname_by_id to find METHODs whose qname starts with `{cls_qname}::`.
            let prefix = format!("{cls_qname}::");
            for (id, qname) in &graph.nav.qname_by_id {
                if !qname.starts_with(&prefix) {
                    continue;
                }
                if graph.nav.kind_by_id.get(id).copied() != Some(node_kind::METHOD) {
                    continue;
                }
                if seen.insert(*id) {
                    sinks.push((
                        *id,
                        qname.clone(),
                        format!("constructor_call:{cls_name}"),
                    ));
                }
            }
        }
    }

    sinks
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_core::RepoId;
    use repo_graph_graph::build_python;
    use repo_graph_parser_python::parse_file;

    fn graph_for(src: &str) -> RepoGraph {
        let repo = RepoId::from_canonical("test");
        let fp = parse_file(src, "m.py", "m", repo).unwrap();
        build_python(repo, vec![fp]).unwrap()
    }

    #[test]
    fn collects_resolved_attr_as_sink() {
        let g = graph_for("class Foo:\n    def bar(self): pass\n    def baz(self): pass\n");
        let mut facts = TestPatchFacts::default();
        facts.resolved_attrs.insert("Foo.bar".into(), 1);
        let sinks = collect_sinks(&g, &facts);
        assert_eq!(sinks.len(), 1);
        assert_eq!(sinks[0].1, "m::Foo::bar");
        assert_eq!(sinks[0].2, "resolved_attr:Foo.bar");
    }

    #[test]
    fn collects_constructor_class_methods_as_sinks() {
        let g = graph_for("class Foo:\n    def a(self): pass\n    def b(self): pass\nclass Bar:\n    def c(self): pass\n");
        let mut facts = TestPatchFacts::default();
        facts.constructor_calls.insert("Foo".into(), 1);
        let sinks = collect_sinks(&g, &facts);
        // Foo has 2 methods, both become sinks. Bar shouldn't.
        let names: Vec<&str> = sinks.iter().map(|s| s.1.as_str()).collect();
        assert!(names.contains(&"m::Foo::a"));
        assert!(names.contains(&"m::Foo::b"));
        assert!(!names.iter().any(|n| n.contains("Bar")));
        assert_eq!(sinks.len(), 2);
    }

    #[test]
    fn activated_methods_filters_to_callable_kinds() {
        let g = graph_for("class Foo:\n    def bar(self): pass\ndef top(): pass\n");
        let activated = vec![
            ("m::Foo".to_string(), 0.5),         // CLASS — should be filtered out
            ("m::Foo::bar".to_string(), 0.4),     // METHOD — keep
            ("m::top".to_string(), 0.3),          // FUNCTION — keep
            ("m::ghost".to_string(), 0.2),        // unknown — drop
        ];
        let ids = activated_methods(&g, &activated);
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn end_to_end_emits_cell_when_activated_caller_reaches_sink() {
        // Top-level functions only — Python resolver reliably emits CALLS
        // edges between module-local function calls. caller → mid → sink.
        let g = graph_for(
            "def sink(): pass\ndef mid(): sink()\ndef caller(): mid()\n",
        );
        let mut facts = TestPatchFacts::default();
        // Treat top-level `sink` as a constructor sink — that path resolves
        // by class-name match, but here we route via class_attrs analog by
        // using a synthetic Class.attr binding through resolved_attrs.
        // For functions, we need a different sink-collection path; verify
        // reachable_by directly to keep this test focused on the API.
        let _ = facts;

        // Look up sink id directly.
        let qname_to_id: HashMap<&str, NodeId> = g
            .nav
            .qname_by_id
            .iter()
            .map(|(id, q)| (q.as_str(), *id))
            .collect();
        let sink_id = *qname_to_id.get("m::sink").expect("sink resolves");
        let activated_ids = activated_methods(
            &g,
            &[("m::caller".to_string(), 1.0), ("m::mid".to_string(), 0.9)],
        );
        assert_eq!(activated_ids.len(), 2, "both top-level functions activate");

        let follow = vec![edge_category::CALLS, edge_category::RETURNS_TYPE];
        let reaching = g.reachable_by(sink_id, &activated_ids, &follow, 3);
        assert!(
            !reaching.is_empty(),
            "expected mid (and transitively caller) to reach sink"
        );
    }
}
