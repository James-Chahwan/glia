//! `glia coverage` (P2) — for the languages present, the known extraction
//! caveats + edges-found per flagged category. When the build ingested a git
//! history snapshot (CO_CHANGES edges, LF.5b), the table gains the
//! co-change audit (LF.5c, `gaps::cochange_gaps`): the file pairs that change
//! together with no static link — where THIS repo's graph is likely blind.
//! `--json` stays the caveat array alone.

use glia_code_domain::edge_category;
use glia_engine::gaps::{CochangeGap, cochange_gaps};

use crate::common::generate_for;

/// Co-change rows the table shows; `glia gaps --category cochange_no_edge`
/// lists the rest.
const COCHANGE_TOP: usize = 20;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in. Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let with = args.with.as_slice();
    let json = args.json;
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let report = glia_engine::coverage_report(&result.merged);
    if json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
        return 0;
    }
    println!("# glia coverage `{repo}`");
    println!();
    println!("_Where glia is known-partial — verify these dimensions with grep._");
    println!();
    println!("| language | edge | found | caveat → verify |");
    println!("|---|---|--:|---|");
    for n in &report {
        println!(
            "| {} | {} | {} | {} — _{}_ |",
            n.language, n.edge_category, n.edges_found, n.note, n.verify
        );
    }
    if result
        .merged
        .all_edges()
        .any(|e| e.category == edge_category::CO_CHANGES)
    {
        print_cochange(repo, &cochange_gaps(&result.merged, None));
    }
    0
}

/// `python`, or `typescript / python` for a cross-language pair (`?`: a
/// language the caveat table does not name).
fn languages(g: &CochangeGap) -> String {
    let (a, b) = (g.language_a.unwrap_or("?"), g.language_b.unwrap_or("?"));
    if g.cross_language {
        format!("{a} / {b}")
    } else {
        a.to_string()
    }
}

/// The co-change section: at most [`COCHANGE_TOP`] rows of `gaps` (already
/// ranked by co-changes).
fn print_cochange(repo: &str, gaps: &[CochangeGap]) {
    println!();
    println!(
        "## co-change without a static edge (heuristic: co-change is history, not proof of coupling)"
    );
    println!();
    println!(
        "_File pairs git history says change together that no static edge joins, directly or through a file-less node (endpoint, route, queue): a coupling extraction may miss — read both files._"
    );
    println!();
    if gaps.is_empty() {
        println!("none: every co-changing pair shares a static link.");
        return;
    }
    println!("| file a | file b | co-changes | ratio ‰ | languages |");
    println!("|---|---|--:|--:|---|");
    for g in gaps.iter().take(COCHANGE_TOP) {
        println!(
            "| {} | {} | {} | {} | {} |",
            g.file_a.replace('|', "\\|"),
            g.file_b.replace('|', "\\|"),
            g.cochanges,
            g.ratio_permille,
            languages(g)
        );
    }
    if gaps.len() > COCHANGE_TOP {
        println!();
        println!(
            "_top {COCHANGE_TOP} of {n}: `glia gaps {repo} --category cochange_no_edge --top-k {n}` lists them all._",
            n = gaps.len()
        );
    }
}
