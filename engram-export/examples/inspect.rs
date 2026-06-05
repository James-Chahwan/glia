//! Inspect an emitted `.engram-gmap`: version, field coverage, edge-kind +
//! provenance histograms. Verification tool for the v3 contract.
//!
//!   cargo run -p repo-graph-engram-export --example inspect -- <path.engram-gmap>

use std::collections::BTreeMap;

use engram_core::{Content, Gmap};

fn main() {
    let path = std::env::args().nth(1).expect("usage: inspect <path.engram-gmap>");
    let bytes = std::fs::read(&path).expect("read gmap");
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
        .filter(|x| matches!(&x.content, Content::Proposition(_)))
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

fn pct(a: usize, b: usize) -> f64 {
    if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 }
}
