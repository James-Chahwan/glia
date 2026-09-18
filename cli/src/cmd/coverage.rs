//! `glia coverage` (P2) — for the languages present, the known extraction
//! caveats + edges-found per flagged category.

use crate::common::generate_for;

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
    let report = repo_graph_engine::coverage_report(&result.merged);
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
    0
}
