//! `glia delta` (LE.1c) — what a change did to the graph: the human surface
//! over the engine's `delta::graph_delta_vs_rev`, the git rev `--base`
//! (default `HEAD`) against the working tree.
//!
//! Table mode prints the counts of the whole delta, then one table of node
//! rows (`--edges-only` leaves it out) and one of edge rows (`--category`
//! keeps the named categories only). `--json` prints the engine's
//! `GraphDeltaAnswer` (`{base, files, counts, nodes, edges}`) with the same
//! two filters applied to its rows; `counts` is always the whole delta's.
//! Every `file:line` is the record's own 1-based line (LD.1), printed as is.
//!
//! Exit 0 on an answer (an empty delta prints `_(no graph change)_`), 2 on an
//! unknown `--category`, `--no-overlay`, or a git / build failure with the
//! engine's message.
//!
//! Writes: the working tree's parse-cache sidecar
//! (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored), as an incremental
//! build does, under `GLIA_NO_PERSIST=1` too — the engine saves it
//! unconditionally, and it is what makes the next delta reparse only the
//! changed files. Never a `.gmap` layout.
//!
//! Fired-on marker: the engine's `[delta] materialized ...` and
//! `[delta] base=<rev> ...` lines, then this surface's
//! `[delta] surface=cli rows=<n>`, `n` the node + edge rows printed.

use glia_code_domain::edge_category;
use glia_engine::delta::{DeltaEdge, DeltaNode, GraphDeltaAnswer, graph_delta_vs_rev};

use crate::common::build_options;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo (a git work tree).
    repo: String,
    /// The git rev to compare the working tree against: a branch, tag, sha
    /// or `HEAD~N`.
    #[arg(long, default_value = "HEAD")]
    base: String,
    /// Emit JSON (`{base, files, counts, nodes, edges}`) instead of tables.
    #[arg(long)]
    json: bool,
    /// Leave out the node rows.
    #[arg(long)]
    edges_only: bool,
    /// Keep only the edge rows of this category (`CALLS`, `HTTP_CALLS`, ...;
    /// case-insensitive). Repeatable.
    #[arg(long)]
    category: Vec<String>,
}

/// Each `--category` as its canonical `edge_category` name, or the error
/// naming every valid one.
fn canonical_categories(given: &[String]) -> Result<Vec<&'static str>, String> {
    let mut out: Vec<&'static str> = Vec::with_capacity(given.len());
    for g in given {
        let hit = edge_category::ALL.iter().find(|(_, n)| n.eq_ignore_ascii_case(g.trim()));
        match hit {
            Some((_, n)) => {
                if !out.contains(n) {
                    out.push(n);
                }
            }
            None => {
                let valid: Vec<&str> = edge_category::ALL.iter().map(|(_, n)| *n).collect();
                return Err(format!("unknown edge category `{g}`; valid: {}", valid.join(", ")));
            }
        }
    }
    Ok(out)
}

/// The rows `--edges-only` / `--category` keep; `counts` is left whole.
fn filter(answer: &mut GraphDeltaAnswer, edges_only: bool, categories: &[&str]) {
    if edges_only {
        answer.nodes.clear();
    }
    if !categories.is_empty() {
        answer.edges.retain(|e| categories.contains(&e.category));
    }
}

/// `file:line`, `file`, or `—`.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

/// A node row's qname cell: `` `old` → `new` `` when it moved.
fn node_name(n: &DeltaNode) -> String {
    match n.before_qname.as_deref() {
        Some(was) if was != n.qname => format!("`{was}` → `{}`", n.qname),
        _ => format!("`{}`", n.qname),
    }
}

/// A node row's location; a removed node is located in the base rev.
fn node_location(n: &DeltaNode, base: &str) -> String {
    let loc = at(n.file.as_deref(), n.line);
    if n.side == "before" { format!("{loc} (at {base})") } else { loc }
}

/// An edge row's change cell: a reconfidenced edge shows `was → now`.
fn edge_change(e: &DeltaEdge) -> String {
    match e.was_confidence {
        Some(was) => format!("{} ({was} → {})", e.change, e.confidence),
        None => e.change.to_string(),
    }
}

/// An edge row's site: the evidence line, its basis when that line is an
/// endpoint's declaration rather than the asserting construct, and the base
/// rev for a removed edge (located in the before graph).
fn edge_site(e: &DeltaEdge, base: &str) -> String {
    let s = at(e.site_file.as_deref(), e.site_line);
    if e.site_file.is_none() {
        return s;
    }
    let mut notes: Vec<String> = Vec::new();
    if let Some(b) = e.basis.filter(|b| *b != "site") {
        notes.push(b.to_string());
    }
    if e.change == "removed" {
        notes.push(format!("at {base}"));
    }
    if notes.is_empty() { s } else { format!("{s} ({})", notes.join(", ")) }
}

fn print_table(repo: &str, a: &GraphDeltaAnswer, empty: bool) {
    let c = &a.counts;
    println!("# glia delta `{repo}` vs {}", a.base);
    println!();
    println!(
        "- nodes: +{} added, -{} removed, ~{} modified, >{} moved | edges: +{} added, -{} removed, ~{} reconfidenced | files: {} reused, {} reparsed, {} evicted",
        c.nodes_added,
        c.nodes_removed,
        c.nodes_modified,
        c.nodes_moved,
        c.edges_added,
        c.edges_removed,
        c.edges_reconfidenced,
        a.files.reused,
        a.files.reparsed,
        a.files.evicted,
    );
    if c.regions_excluded > 0 || c.moves_ignored > 0 {
        println!(
            "- left out: {} REGION nodes (ignored / vendored dirs), {} move pairs not applied",
            c.regions_excluded, c.moves_ignored
        );
    }
    println!();
    if empty {
        println!("_(no graph change)_");
        return;
    }
    if a.nodes.is_empty() && a.edges.is_empty() {
        println!("_(no row matches the filters)_");
        return;
    }
    if !a.nodes.is_empty() {
        println!("## Nodes");
        println!();
        println!("| change | kind | qname | location |");
        println!("|---|---|---|---|");
        for n in &a.nodes {
            println!("| {} | {} | {} | {} |", n.change, n.kind, node_name(n), node_location(n, &a.base));
        }
        println!();
    }
    if !a.edges.is_empty() {
        println!("## Edges");
        println!();
        println!("| change | category | from | to | site |");
        println!("|---|---|---|---|---|");
        for e in &a.edges {
            println!(
                "| {} | {} | `{}` | `{}` | {} |",
                edge_change(e),
                e.category,
                e.from_qname,
                e.to_qname,
                edge_site(e, &a.base)
            );
        }
    }
}

pub(crate) fn run(args: Args) -> i32 {
    if !build_options().overlay {
        eprintln!(
            "error: --no-overlay does not apply to delta: both sides are built with the repo's \
             overlay, so an overlay edit shows as graph change"
        );
        return 2;
    }
    let categories = match canonical_categories(&args.category) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut answer = match graph_delta_vs_rev(&args.repo, &args.base) {
        Ok(d) => d.answer,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let empty = answer.nodes.is_empty() && answer.edges.is_empty();
    filter(&mut answer, args.edges_only, &categories);
    eprintln!("[delta] surface=cli rows={}", answer.nodes.len() + answer.edges.len());
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
    } else {
        print_table(&args.repo, &answer, empty);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_canonicalise_and_reject_unknown() {
        let got = canonical_categories(&["calls".into(), "CALLS".into(), " Http_Calls ".into()]);
        assert_eq!(got, Ok(vec!["CALLS", "HTTP_CALLS"]));
        let err = canonical_categories(&["CALL".into()]).expect_err("CALL is no category");
        assert!(err.starts_with("unknown edge category `CALL`; valid: "), "{err}");
        for (_, name) in edge_category::ALL {
            assert!(err.contains(name), "{name} missing from: {err}");
        }
    }
}
