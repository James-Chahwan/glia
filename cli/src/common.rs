//! Helpers shared by more than one command: graph generation for the
//! `--with` commands, the summary table and JSON dumps (`analyze`, `merge`),
//! node lookups and pretty names, and `ImpactDirection` (`impact`,
//! `blast-radius`).

use std::collections::BTreeMap;

use clap::ValueEnum;
use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{NodeId, NodeKindId};
use repo_graph_engine::{GenerateResult, generate_many, generate_one};
use repo_graph_graph::MergedGraph;

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum ImpactDirection {
    /// Forward — what this entity reaches (calls, http_calls, etc.).
    Forward,
    /// Backward — what reaches this entity (predecessors).
    Backward,
    /// Both directions in one pass.
    Both,
}

/// Build a graph from one repo, or merge several (`--with`) so cross-service
/// resolvers fire across the boundary — shared by the P2/P3 commands.
pub(crate) fn generate_for(repo: &str, with: &[String]) -> Result<GenerateResult, String> {
    if with.is_empty() {
        generate_one(repo)
    } else {
        let mut repos = vec![repo.to_string()];
        repos.extend(with.iter().cloned());
        generate_many(&repos)
    }
}

pub(crate) fn print_summary_table(r: &GenerateResult) {
    let merged = &r.merged;
    let total_intra: usize = merged.graphs.iter().map(|g| g.edges.len()).sum();
    println!("# glia analyze");
    println!();
    println!("- nodes: {}", r.total_nodes);
    println!("- edges (intra-repo): {total_intra}");
    println!("- cross-edges: {}", merged.cross_edges.len());
    if !r.parse_errors.is_empty() {
        println!("- parse errors: {}", r.parse_errors.len());
    }
    println!();

    // Node kind histogram.
    let mut kind_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if let Some(kind) = g.nav.kind_by_id.get(&n.id) {
                *kind_counts.entry(node_kind_name(*kind)).or_insert(0) += 1;
            }
        }
    }
    println!("## Node kinds");
    println!();
    println!("| Kind | Count |");
    println!("|---|---|");
    let mut rows: Vec<_> = kind_counts.into_iter().collect();
    rows.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (k, c) in rows {
        println!("| {k} | {c} |");
    }
    println!();

    // Edge category histogram (intra + cross combined).
    let mut cat_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for g in &merged.graphs {
        for e in &g.edges {
            *cat_counts.entry(edge_category_name(e.category)).or_insert(0) += 1;
        }
    }
    for e in &merged.cross_edges {
        *cat_counts.entry(edge_category_name(e.category)).or_insert(0) += 1;
    }
    println!("## Edge categories");
    println!();
    println!("| Category | Count |");
    println!("|---|---|");
    let mut rows: Vec<_> = cat_counts.into_iter().collect();
    rows.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (k, c) in rows {
        println!("| {k} | {c} |");
    }
}

pub(crate) fn print_json(merged: &MergedGraph) {
    let mut nodes = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).map(|k| k.0).unwrap_or(0);
            let name = g.nav.name_by_id.get(&n.id).cloned().unwrap_or_default();
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            nodes.push(serde_json::json!({
                "id": n.id.0,
                "repo": g.repo.0,
                "kind": kind,
                "kind_name": node_kind_name(NodeKindId(kind)),
                "name": name,
                "qname": qname,
            }));
        }
    }
    let mut edges = Vec::new();
    for g in &merged.graphs {
        for e in &g.edges {
            edges.push(serde_json::json!({
                "from": e.from.0,
                "to": e.to.0,
                "category": edge_category_name(e.category),
                "intra": true,
            }));
        }
    }
    for e in &merged.cross_edges {
        edges.push(serde_json::json!({
            "from": e.from.0,
            "to": e.to.0,
            "category": edge_category_name(e.category),
            "intra": false,
        }));
    }
    let out = serde_json::json!({ "nodes": nodes, "edges": edges });
    println!("{}", serde_json::to_string(&out).unwrap_or_default());
}

pub(crate) fn write_json_to(merged: &MergedGraph, out: &mut Vec<u8>) {
    use std::io::Write;
    let mut nodes = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).map(|k| k.0).unwrap_or(0);
            let name = g.nav.name_by_id.get(&n.id).cloned().unwrap_or_default();
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            nodes.push(serde_json::json!({
                "id": n.id.0,
                "repo": g.repo.0,
                "kind": kind,
                "kind_name": node_kind_name(NodeKindId(kind)),
                "name": name,
                "qname": qname,
            }));
        }
    }
    let mut edges = Vec::new();
    for g in &merged.graphs {
        for e in &g.edges {
            edges.push(serde_json::json!({
                "from": e.from.0,
                "to": e.to.0,
                "category": edge_category_name(e.category),
                "intra": true,
            }));
        }
    }
    for e in &merged.cross_edges {
        edges.push(serde_json::json!({
            "from": e.from.0,
            "to": e.to.0,
            "category": edge_category_name(e.category),
            "intra": false,
        }));
    }
    let json = serde_json::json!({ "nodes": nodes, "edges": edges });
    let _ = writeln!(out, "{}", serde_json::to_string(&json).unwrap_or_default());
}

pub(crate) fn all_edges(merged: &MergedGraph) -> impl Iterator<Item = &repo_graph_core::Edge> {
    merged
        .graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(merged.cross_edges.iter())
}

pub(crate) struct NodeInfo {
    pub(crate) name: String,
    pub(crate) qname: String,
    pub(crate) kind_name: &'static str,
    pub(crate) repo: u64,
}

pub(crate) fn lookup_node_info(merged: &MergedGraph, id: NodeId) -> NodeInfo {
    for g in &merged.graphs {
        if g.nav.qname_by_id.contains_key(&id) {
            return NodeInfo {
                name: g.nav.name_by_id.get(&id).cloned().unwrap_or_default(),
                qname: g.nav.qname_by_id.get(&id).cloned().unwrap_or_default(),
                kind_name: g
                    .nav
                    .kind_by_id
                    .get(&id)
                    .map(|k| node_kind_name(*k))
                    .unwrap_or("unknown"),
                repo: g.repo.0,
            };
        }
    }
    NodeInfo {
        name: String::new(),
        qname: format!("(unknown:{})", id.0),
        kind_name: "unknown",
        repo: 0,
    }
}

// ----------------------------------------------------------------------------
// Pretty-name lookups for code-domain ID constants.
// ----------------------------------------------------------------------------

pub(crate) fn node_kind_name(k: NodeKindId) -> &'static str {
    // Delegate to the canonical code-domain table (WP-I) — no local copy to go
    // stale when a kind is added.
    node_kind::name(k)
}

pub(crate) fn edge_category_name(c: repo_graph_core::EdgeCategoryId) -> &'static str {
    // Delegate to the canonical code-domain table (WP-I).
    edge_category::name(c)
}
