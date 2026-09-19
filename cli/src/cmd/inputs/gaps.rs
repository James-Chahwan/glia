//! `glia gaps` (LF.2c) — the human surface over the engine's
//! `gaps::gaps_report`: the ranked blind-spot report an overlay agent works
//! from, one table per category with the overlay section that could repair
//! each row. `--overlay-delta` prints `gaps::overlay_delta` instead: the tree
//! built without and then with its overlay, and what the overlay changed.
//!
//! Every category, the ordering and the marker live in the engine; this is
//! transport + rendering only. A report, not a gate: it exits 0 whatever it
//! finds, and 2 on a build error or an unknown `--category`.

use std::path::PathBuf;

use glia_engine::gaps::{
    CATEGORIES, GapRow, GapsOptions, OverlayDelta, gaps_report, overlay_delta,
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
    /// (rules, edges per category, orphans before -> after). Two builds.
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

fn print_delta(d: &OverlayDelta) {
    println!("| | without overlay | with overlay |");
    println!("|---|---|---|");
    println!("| edges | {} | {} |", d.edges_without, d.edges_with);
    println!("| orphans | {} | {} |", d.orphans_without, d.orphans_with);
    println!();
    println!("rules: {}", d.rules);
    if d.added_by_category.is_empty() {
        println!("edges by category: unchanged");
    } else {
        let per: Vec<String> = d
            .added_by_category
            .iter()
            .map(|(c, n)| format!("{c} {n:+}"))
            .collect();
        println!("edges by category: {}", per.join(", "));
    }
    let verdict = if d.orphans_with < d.orphans_without {
        "orphans fell: keep the overlay"
    } else {
        "orphans did not fall"
    };
    println!("{verdict}");
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
        println!("| qname | kind | at | tier | suggest | detail |");
        println!("|---|---|---|---|---|---|");
        for r in rows {
            println!(
                "| `{}` | {} | {} | {} | {} | {} |",
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
