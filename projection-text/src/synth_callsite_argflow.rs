//! Refactored entry point for the `synth_callsite_argflow` driver bin.
//!
//! Hosts the graph-walking logic that used to live inside that bin's
//! `fn main()` so it can be called in-process from glia-3d's Inject scene
//! (and other consumers) without going through disk JSON.
//!
//! See `bin/synth_callsite_argflow.rs` for the bin wrapper that handles
//! CLI args + summaries-hybrid append.

#![cfg(feature = "driver")]

use std::collections::{BTreeMap, BTreeSet, HashSet};

use repo_graph_code_domain::node_kind;
use repo_graph_core::NodeId;
use repo_graph_graph::RepoGraph;
use repo_graph_parser_python::extract_calls_with_args;

use crate::driver_utils::{extract_code_cell, node_by_qname_index, reverse_qname_index};

#[derive(Debug, Clone)]
pub struct CallsiteArgflowCell {
    pub id: u64,
    pub qname: String,
    pub score: f64,
    pub summary: String,
}

/// Public entry point. Takes the in-memory `RepoGraph`, an `activated`
/// NodeId set (typically PPR top-K resolved to method/function nodes),
/// and emits one `CallsiteArgflowCell` per polymorphic simple-name with
/// observed call-site polymorphism.
pub fn run(
    graph: &RepoGraph,
    activated: &[NodeId],
    id_start: u64,
) -> Vec<CallsiteArgflowCell> {
    let activated_set: HashSet<NodeId> = activated.iter().copied().collect();

    // All METHODs grouped by simple name (whole-graph view — needed to
    // decide if a name is polymorphic).
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
    for v in simple_to_all_methods.values_mut() {
        v.sort_by_key(|id| id.0);
    }

    let polymorphic: BTreeSet<String> = simple_to_all_methods
        .iter()
        .filter(|(_, ids)| ids.len() >= 2)
        .map(|(k, _)| k.clone())
        .collect();

    let qname_to_id = reverse_qname_index(graph);
    let node_by_qname = node_by_qname_index(graph);

    #[derive(Clone)]
    struct CallerRecord {
        caller_qname: String,
        caller_class_simple: String,
        receiver_text: String,
        args_resolved: Vec<String>,
    }

    let mut per_name: BTreeMap<String, Vec<CallerRecord>> = BTreeMap::new();
    // Iterate the activated Vec (deterministic) not the HashSet (random).
    for caller_id in activated {
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

    let mut emitted: Vec<CallsiteArgflowCell> = Vec::new();
    let mut next_id = id_start;
    for (simple_name, mut callers) in per_name {
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

        emitted.push(CallsiteArgflowCell {
            id: next_id,
            qname: primary_qname,
            score: 0.0,
            summary: text,
        });
        next_id += 1;
    }

    emitted
}
