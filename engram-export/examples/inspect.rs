//! Inspect an emitted `.engram-gmap`: version, field coverage, edge-kind +
//! provenance histograms. Verification tool for the v3 contract. A path
//! ending in `.diff` is a `--since` run's `GmapDiff` (LG.8a): its version,
//! both digests, the list lengths and the first keys of each node list.
//!
//!   cargo run -p glia-engram-export --example inspect -- <path.engram-gmap>
//!   cargo run -p glia-engram-export --example inspect -- <path.engram-gmap.diff>

use std::collections::BTreeMap;

use engram_core::{Content, Gmap, GmapDiff};

fn main() {
    let path = std::env::args().nth(1).expect("usage: inspect <path.engram-gmap[.diff]>");
    let bytes = std::fs::read(&path).expect("read gmap");
    if path.ends_with(".diff") {
        inspect_diff(&bincode::deserialize(&bytes).expect("decode diff"));
        return;
    }
    let g: Gmap = bincode::deserialize(&bytes).expect("decode gmap");

    let n = g.nodes.len();
    let with_concept = g.nodes.iter().filter(|x| x.concept_hint.is_some()).count();
    let with_identity = g.nodes.iter().filter(|x| x.identity_hint.is_some()).count();
    let with_qname = g
        .nodes
        .iter()
        .filter(|x| matches!(&x.content, Content::Symbol { qname: Some(_), .. }))
        .count();
    let with_doc = g
        .nodes
        .iter()
        .filter(|x| matches!(&x.content, Content::Symbol { doc: Some(_), .. }))
        .count();
    let with_imports = g
        .nodes
        .iter()
        .filter(|x| matches!(&x.content, Content::Symbol { imports: Some(v), .. } if !v.is_empty()))
        .count();
    let propositions = g
        .nodes
        .iter()
        .filter(|x| matches!(&x.content, Content::Proposition { .. }))
        .count();

    println!("format_version : {}", g.format_version);
    println!("nodes          : {n}");
    println!("  concept_hint : {with_concept} ({:.0}%)", pct(with_concept, n));
    println!("  identity_hint: {with_identity} ({:.0}%)", pct(with_identity, n));
    println!("  qname        : {with_qname} ({:.0}%)", pct(with_qname, n));
    println!("  doc          : {with_doc} ({:.0}%)", pct(with_doc, n));
    println!("  imports      : {with_imports} ({:.0}%)", pct(with_imports, n));
    println!("  propositions : {propositions} (docs)");
    println!("  files map    : {} entries", g.files.len());
    for x in &g.nodes {
        if let Content::Symbol { imports: Some(v), .. } = &x.content {
            if !v.is_empty() {
                println!("    e.g. imports {:?} on {}", v, x.key.rsplit("::").next().unwrap_or(""));
                break;
            }
        }
    }
    for x in g.nodes.iter().take(400) {
        if let Content::Symbol { doc: Some(d), .. } = &x.content {
            println!("    e.g. {} :: {}", x.key.rsplit("::").next().unwrap_or(""), d.chars().take(70).collect::<String>());
            break;
        }
    }

    let mut prov: BTreeMap<String, usize> = BTreeMap::new();
    for x in &g.nodes {
        *prov.entry(x.provenance.clone().unwrap_or_else(|| "authored".into())).or_default() += 1;
    }
    println!("provenance:");
    for (k, v) in &prov {
        println!("  {k:<16} {v}");
    }

    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut weighted = 0usize;
    for e in &g.edges {
        *kinds.entry(format!("{:?}", e.kind)).or_default() += 1;
        if e.weight.is_some() {
            weighted += 1;
        }
    }
    println!("edges          : {} ({weighted} weighted)", g.edges.len());
    for (k, v) in &kinds {
        println!("  {k:<12} {v}");
    }

    // Sample concept_hints that are NOT a prefix of their own key (proves the
    // heuristic did real feature-mapping, not echo the path).
    println!("sample concept_hints:");
    let mut seen = std::collections::BTreeSet::new();
    for x in &g.nodes {
        if let Some(c) = &x.concept_hint
            && !x.key.starts_with(c)
            && seen.insert(c.clone())
        {
            println!("  {c:<28} <- {}", x.key);
            if seen.len() >= 8 {
                break;
            }
        }
    }
}

/// The diff mode: counts, digests and the first 5 keys of each node list.
fn inspect_diff(d: &GmapDiff) {
    println!("format_version : {}", d.format_version);
    println!("base_digest    : {:016x}", d.base_digest);
    println!("target_digest  : {:016x}", d.target_digest);
    let added: Vec<&str> = d.added.iter().map(|n| n.key.as_str()).collect();
    let removed: Vec<&str> = d.removed.iter().map(String::as_str).collect();
    let modified: Vec<String> = d
        .modified
        .iter()
        .map(|c| {
            let moved = if c.prior_key == c.node.key {
                String::new()
            } else {
                format!(" (was {})", c.prior_key)
            };
            let loc = if c.location_only { " [location only]" } else { "" };
            format!("{}{moved}{loc}", c.node.key)
        })
        .collect();
    print_keys("added", &added);
    print_keys("removed", &removed);
    print_keys("modified", &modified);
    // Where the changes sit: modified nodes per file of their new span.
    let mut by_file: BTreeMap<&str, usize> = BTreeMap::new();
    for c in &d.modified {
        let file = match &c.node.content {
            Content::Symbol { span, .. } | Content::Proposition { span: Some(span), .. } => {
                d.files.get(&span.file).map_or("<no file>", String::as_str)
            }
            _ => "<no span>",
        };
        *by_file.entry(file).or_default() += 1;
    }
    for (file, n) in &by_file {
        println!("  in {file}: {n}");
    }
    println!("edges          : +{} / -{}", d.edges_added.len(), d.edges_removed.len());
    println!("files map      : {} entries", d.files.len());
}

fn print_keys<S: AsRef<str>>(label: &str, keys: &[S]) {
    println!("{label:<15}: {}", keys.len());
    for k in keys.iter().take(5) {
        println!("  {}", k.as_ref());
    }
}

fn pct(a: usize, b: usize) -> f64 {
    if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 }
}
