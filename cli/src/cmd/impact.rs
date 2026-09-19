//! `glia impact` — reachability walk: which entities does <qname> depend on /
//! get hit by, over every edge category, as depth-tagged tables.

use repo_graph_activation::algo::{Adjacency, CategorySet, Walk, reach};
use repo_graph_core::NodeId;
use repo_graph_engine::generate_one_opts;
use repo_graph_graph::MergedGraph;

use crate::common::{ImpactDirection, build_options, edge_category_name, lookup_node_info};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Qname of the entity to analyze (e.g. `app::services::api::Handler`).
    qname: String,
    /// Direction of the walk.
    #[arg(long, value_enum, default_value_t = ImpactDirection::Both)]
    direction: ImpactDirection,
    /// Maximum walk depth.
    #[arg(long, default_value_t = 4)]
    depth: usize,
}

pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let qname = args.qname.as_str();
    let (direction, depth) = (args.direction, args.depth);
    let result = match generate_one_opts(repo, false, &build_options()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let merged = &result.merged;

    // Resolve qname → NodeId across all repo graphs (one match expected; if
    // multiple, walk all). Sorted by node id so the section order is stable
    // across processes (the resolver no longer leaks HashMap iteration order).
    let targets: Vec<NodeId> = merged.qnames_exact(qname);
    if targets.is_empty() {
        eprintln!("error: no node with qname `{qname}` in {repo}");
        eprintln!();
        eprintln!("hint: use `glia analyze {repo} --format json` to list available qnames.");
        return 3;
    }

    println!("# glia impact `{qname}`");
    println!();
    for target in &targets {
        let info = lookup_node_info(merged, *target);
        println!(
            "## {} `{}` (kind={}, repo={})",
            info.name, info.qname, info.kind_name, info.repo
        );
        println!();
        if matches!(direction, ImpactDirection::Forward | ImpactDirection::Both) {
            println!("### Forward — what this reaches (depth ≤ {depth})");
            println!();
            walk_and_print(merged, *target, depth, /*forward=*/ true);
            println!();
        }
        if matches!(direction, ImpactDirection::Backward | ImpactDirection::Both) {
            println!("### Backward — what reaches this (depth ≤ {depth})");
            println!();
            walk_and_print(merged, *target, depth, /*forward=*/ false);
            println!();
        }
    }
    0
}

/// One breadth-first walk from `start` over every edge category (`impact`
/// never filtered by category), on a CSR index built once per walk (LD.15b):
/// each reached node with its depth and the category that first reached it,
/// in discovery order.
fn walk_and_print(merged: &MergedGraph, start: NodeId, max_depth: usize, forward: bool) {
    let adj = Adjacency::build(merged, &CategorySet::all());
    let walk = if forward { Walk::Forward } else { Walk::Backward };
    let hits: Vec<(NodeId, usize, &'static str)> = reach::bfs(&adj, &[start], walk, max_depth)
        .reached
        .iter()
        .map(|r| (r.id, r.depth, edge_category_name(r.via)))
        .collect();
    if hits.is_empty() {
        println!("_(none)_");
        return;
    }
    println!("| Depth | Kind | Qname | Edge |");
    println!("|---|---|---|---|");
    for (id, d, cat) in hits {
        let info = lookup_node_info(merged, id);
        println!("| {d} | {} | `{}` | {} |", info.kind_name, info.qname, cat);
    }
}
