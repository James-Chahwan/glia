//! PPR power-iteration profile harness.
//!
//! Manual `--bench` harness (no criterion dep) — measures per-iteration
//! wall time on synthetic graphs of growing N, plus a real-shaped graph
//! sample. Outputs a single markdown table per run for at-a-glance
//! profiling against the sub-10ms target (#4 from neuropil substrate
//! priorities).
//!
//! Run via: `cargo bench -p repo-graph-activation`

use std::collections::HashMap;
use std::time::Instant;

use repo_graph_activation::{activate, ActivationConfig};
use repo_graph_core::{Confidence, Edge, EdgeCategoryId, NodeId};

fn build_grid(n: usize, density: usize) -> (Vec<NodeId>, Vec<Edge>) {
    // Synthetic graph: n nodes arranged in a ring with random short edges.
    // `density` controls average degree (outgoing edges per node).
    let nodes: Vec<NodeId> = (0..n as u64).map(NodeId).collect();
    let mut edges = Vec::with_capacity(n * density);
    let cat = EdgeCategoryId(1);
    for i in 0..n {
        edges.push(Edge {
            from: NodeId(i as u64),
            to: NodeId(((i + 1) % n) as u64),
            category: cat,
            confidence: Confidence::Strong,
        });
        for k in 1..=density {
            let target = (i + k * 7) % n;
            if target != i {
                edges.push(Edge {
                    from: NodeId(i as u64),
                    to: NodeId(target as u64),
                    category: cat,
                    confidence: Confidence::Strong,
                });
            }
        }
    }
    (nodes, edges)
}

fn bench_size(label: &str, n: usize, density: usize, seeds_n: usize) {
    let (nodes, edges) = build_grid(n, density);
    let seeds: Vec<NodeId> = (0..seeds_n.min(n)).map(|i| NodeId(i as u64)).collect();
    let mut weights = HashMap::new();
    weights.insert(EdgeCategoryId(1), 1.0);
    let config = ActivationConfig {
        max_iterations: 32,
        damping: 0.5,
        epsilon: 1e-6,
        edge_weights: weights,
        ..Default::default()
    };

    // Warm-up
    let _ = activate(&nodes, &edges, &seeds, &config);

    // Measure 5 runs, take median
    let mut walls = vec![];
    for _ in 0..5 {
        let t0 = Instant::now();
        let res = activate(&nodes, &edges, &seeds, &config);
        let wall = t0.elapsed().as_micros();
        let _ = res.iterations;
        walls.push(wall);
    }
    walls.sort();
    let median = walls[walls.len() / 2];
    let total_edges = edges.len();
    println!(
        "| {:8} | n={:>6} edges={:>8} seeds={:>3} | {:>7}µs | {:>8.2}ms |",
        label,
        n,
        total_edges,
        seeds_n,
        median,
        (median as f64) / 1000.0,
    );
}

fn main() {
    println!("# PPR power-iteration profile");
    println!();
    println!("| Label    | Size                                | Median  | Wall     |");
    println!("|----------|-------------------------------------|---------|----------|");

    // Synthetic ladder — typical glia .gmap sizes
    bench_size("tiny",   100,    2, 3);
    bench_size("small",  1_000,  3, 5);
    bench_size("medium", 10_000, 4, 10);
    bench_size("large",  50_000, 5, 20);
    bench_size("huge",   100_000, 5, 50);

    // Real-shaped: heavy seeds (e.g. multiple-query activation)
    bench_size("multi-seed-small",  1_000,  3, 50);
    bench_size("multi-seed-medium", 10_000, 4, 100);

    println!();
    println!("Target: sub-10ms for 10K-node graphs (medium row).");
    println!("Bottlenecks to instrument if median > target:");
    println!("  - id_to_idx HashMap allocation per call");
    println!("  - incoming Vec<Vec<>> allocation per call");
    println!("  - edge-weight HashMap lookups in hot loop");
    println!("  - power iteration scalar loop (SIMD candidate)");
}
