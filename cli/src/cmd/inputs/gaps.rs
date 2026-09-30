//! `glia gaps` (LF.2c) — the human surface over the engine's
//! `gaps::gaps_report`: the ranked blind-spot report an overlay agent works
//! from, one table per category with the overlay section that could repair
//! each row. `--overlay-delta` prints `gaps::overlay_delta` instead: the tree
//! built without and then with its overlay, and what the overlay changed.
//!
//! Every category, the ordering and the marker live in the engine; this is
//! transport + rendering only. A report, not a gate: it exits 0 whatever it
//! finds, and 2 on a build error or an unknown `--category`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use glia_engine::gaps::{
    CATEGORIES, DROP, GapRow, GapsOptions, KEEP, OverlayDelta, REVIEW, gaps_report, overlay_delta,
};

use crate::common::generate_for;

/// Rows shown per category unless `--top-k` says otherwise: `dead_symbol`
/// alone can list thousands on a big repo.
const DEFAULT_TOP_K: usize = 50;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Emit JSON (`{counts, skipped, rows}`, or the overlay delta) instead of tables.
    #[arg(long)]
    json: bool,
    /// Rows kept per category, after ranking (counts stay the totals).
    #[arg(long, default_value_t = DEFAULT_TOP_K)]
    top_k: usize,
    /// Only this category's rows.
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(CATEGORIES))]
    category: Option<String>,
    /// Build without and then with the overlay and print what it changed
    /// (rules; nodes per kind, edges per category and gaps per category
    /// before -> after; orphans) and the verdict: keep, review or drop. Two builds.
    #[arg(long)]
    overlay_delta: bool,
}

/// `file:line`, `file`, or `—`.
fn at(r: &GapRow) -> String {
    match (r.file.as_deref(), r.line) {
        (Some(f), Some(n)) => format!("{f}:{n}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

/// A table cell: pipes escaped so a qname or detail cannot split the row.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// `before→after (+d)` of one key in two count maps.
fn moved(
    before: &BTreeMap<&'static str, usize>,
    after: &BTreeMap<&'static str, usize>,
    key: &str,
    d: i64,
) -> String {
    let n = |m: &BTreeMap<&'static str, usize>| m.get(key).copied().unwrap_or(0);
    format!("{}→{} ({d:+})", n(before), n(after))
}

/// Why the engine's verdict is what it is (`gaps::verdict`).
fn verdict_reason(v: &str) -> &'static str {
    match v {
        KEEP => "a gap category fell or the graph grew, none rose",
        REVIEW => "the graph improved, but a gap category rose too",
        DROP => "no gap category fell and the graph did not grow",
        _ => "unknown verdict",
    }
}

fn print_delta(d: &OverlayDelta) {
    println!("| | without overlay | with overlay |");
    println!("|---|---|---|");
    println!("| edges | {} | {} |", d.edges_without, d.edges_with);
    println!("| orphans | {} | {} |", d.orphans_without, d.orphans_with);
    println!(
        "| gaps | {} | {} |",
        d.without.total_gaps(),
        d.with.total_gaps()
    );
    println!();
    println!("rules: {}", d.rules);
    let mut changed: Vec<String> = Vec::new();
    for (k, n) in &d.nodes_added_by_kind {
        let (b, a) = (&d.without.nodes_by_kind, &d.with.nodes_by_kind);
        changed.push(format!("node kind {k}: {}", moved(b, a, k, *n)));
    }
    for (c, n) in &d.added_by_category {
        let (b, a) = (&d.without.edges_by_category, &d.with.edges_by_category);
        changed.push(format!("edge category {c}: {}", moved(b, a, c, *n)));
    }
    let (b, a) = (&d.without.gaps_by_category, &d.with.gaps_by_category);
    let gap_keys: BTreeSet<&'static str> = b.keys().chain(a.keys()).copied().collect();
    for c in gap_keys {
        let n = |m: &BTreeMap<&'static str, usize>| {
            i64::try_from(m.get(c).copied().unwrap_or(0)).unwrap_or(i64::MAX)
        };
        let delta = n(a) - n(b);
        if delta != 0 {
            changed.push(format!("gap category {c}: {}", moved(b, a, c, delta)));
        }
    }
    if changed.is_empty() {
        println!("changed: nothing (no node kind, edge category or gap category moved)");
    } else {
        println!("changed:");
        for line in changed {
            println!("- {line}");
        }
    }
    println!();
    println!("verdict: {} - {}", d.verdict, verdict_reason(d.verdict));
}

pub(crate) fn run(args: Args) -> i32 {
    if args.overlay_delta {
        let mut repos = vec![args.repo.clone()];
        repos.extend(args.with.iter().cloned());
        return match overlay_delta(&repos, false) {
            Ok(d) if args.json => {
                println!("{}", serde_json::to_string(&d).unwrap_or_default());
                0
            }
            Ok(d) => {
                println!("# glia gaps `{}` --overlay-delta", args.repo);
                println!();
                print_delta(&d);
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                2
            }
        };
    }

    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let roots: Vec<(u64, PathBuf)> = result
        .repo_roots
        .iter()
        .map(|(r, p)| (*r, PathBuf::from(p)))
        .collect();
    let mut opts = GapsOptions::default();
    opts.top_k_per_category = Some(args.top_k);
    opts.category = args.category.clone();
    opts.surface = "cli";
    let report = match gaps_report(&result.merged, &roots, &opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if args.json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
        return 0;
    }

    println!("# glia gaps `{}`", args.repo);
    for category in CATEGORIES {
        if args.category.as_deref().is_some_and(|c| c != category) {
            continue;
        }
        let rows: Vec<&GapRow> = report
            .rows
            .iter()
            .filter(|r| r.category == category)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let total = report.count(category);
        println!();
        if rows.len() < total {
            println!("## {category} — {total} (top {})", rows.len());
        } else {
            println!("## {category} — {total}");
        }
        println!();
        println!("| id | qname | kind | at | tier | suggest | detail |");
        println!("|---|---|---|---|---|---|---|");
        for r in rows {
            println!(
                "| {} | `{}` | {} | {} | {} | {} | {} |",
                r.id,
                cell(&r.qname),
                r.kind,
                cell(&at(r)),
                r.tier,
                cell(r.suggest),
                cell(&r.detail)
            );
        }
    }
    println!();
    let totals: Vec<String> = CATEGORIES
        .iter()
        .map(|c| match report.counts.get(c) {
            Some(n) => format!("{c}={n}"),
            None => format!("{c}=skipped"),
        })
        .collect();
    println!("totals: {}", totals.join(" "));
    if !report.skipped.is_empty() {
        println!("_skipped (no repo root): {}_", report.skipped.join(", "));
    }
    0
}
