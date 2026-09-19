//! `glia blast-radius` (P3) — the complete, edge-category-aware, PPR-ranked,
//! located closure around one or more seeds, across service boundaries, in
//! one call. Many seeds are one walk and one ranking (LD.5): each row names
//! the seed whose wave reached it, and a seed another seed reaches in one
//! hop is listed as linked, never dropped.

use glia_engine::{BlastOptions, BlastRadius};
use glia_graph::Reach;

use crate::cmd::resolve::{live_glyph, print_absence};
use crate::common::{ImpactDirection, generate_for};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Qnames or simple names of the seed entities: one or more, answered
    /// as one radius. A name that matches no node is reported, not fatal.
    #[arg(required = true, num_args = 1.., value_name = "QNAME")]
    qnames: Vec<String>,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Which way the radius spreads.
    #[arg(long, value_enum, default_value_t = ImpactDirection::Both)]
    direction: ImpactDirection,
    /// Maximum hops along carry edges.
    #[arg(long, default_value_t = 4)]
    depth: usize,
    /// Keep only the top-K by PPR score.
    #[arg(long)]
    top_k: Option<usize>,
    /// Drop nodes not reachable from an entrypoint (likely-dead code).
    #[arg(long)]
    live_only: bool,
    /// Restrict the answer to a repo-relative path or a project label (see
    /// `glia projects`), e.g. `services/api` or `@shop/web`. Narrows
    /// WITHIN a repo: each `--with` repo's paths are relative to its OWN
    /// root, so a path passed as a separate repo will not match here.
    /// Nodes with no file (ENDPOINT / ROUTE / doc spaces) are kept, not
    /// dropped.
    #[arg(long)]
    scope: Option<String>,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

/// Exit 0 with the answer (empty or not), 2 when the build fails, 3 when
/// every seed query is unresolved — the absence is printed either way.
pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let result = match generate_for(repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let (direction, dir) = match args.direction {
        ImpactDirection::Forward => (Reach::Forward, "forward"),
        ImpactDirection::Backward => (Reach::Backward, "backward"),
        ImpactDirection::Both => (Reach::Both, "both"),
    };
    let mut opts = BlastOptions::default();
    opts.direction = direction;
    opts.depth = args.depth;
    opts.top_k = args.top_k;
    opts.live_only = args.live_only;
    opts.scope = args.scope.clone();
    let queries: Vec<&str> = args.qnames.iter().map(String::as_str).collect();
    let mut answer = glia_engine::blast_radius(&result.merged, &queries, &opts);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    let code = if answer.seeds.is_empty() && !answer.unresolved.is_empty() {
        eprintln!("hint: use `glia analyze {repo} --format json` to list qnames.");
        3
    } else {
        0
    };
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return code;
    }
    render(&answer, &queries, dir, args.depth);
    code
}

/// The markdown table: a `seed` column when there is more than one seed, the
/// seeds' links and the unresolved queries as `>` lines, the absence block
/// under an empty answer.
fn render(answer: &BlastRadius, queries: &[&str], dir: &str, depth: usize) {
    let asked = queries.iter().map(|q| format!("`{q}`")).collect::<Vec<_>>().join(", ");
    println!("# glia blast-radius {asked} ({dir}, depth ≤ {depth})");
    println!();
    if !answer.unresolved.is_empty() {
        println!("> unresolved: {}", answer.unresolved.join(", "));
    }
    for s in answer.seeds.iter().filter(|s| !s.linked_seeds.is_empty()) {
        println!("> seed `{}` is one carry edge from: {}", s.qname, s.linked_seeds.join(", "));
    }
    if !answer.unresolved.is_empty() || answer.seeds.iter().any(|s| !s.linked_seeds.is_empty()) {
        println!();
    }
    if let Some(a) = &answer.absence {
        println!("_(nothing in radius)_");
        print_absence(a);
        return;
    }
    let many = answer.seeds.len() > 1;
    if many {
        println!("| score | depth | live | via | kind | qname | seed | location |");
        println!("|--:|--:|:-:|---|---|---|---|---|");
    } else {
        println!("| score | depth | live | via | kind | qname | location |");
        println!("|--:|--:|:-:|---|---|---|---|");
    }
    for a in &answer.results {
        let loc = match (&a.file, a.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        let seed = if many { format!(" `{}` |", a.seed) } else { String::new() };
        println!(
            "| {:.4} | {} | {} | {} | {} | `{}` |{seed} {} |",
            a.score,
            a.depth,
            live_glyph(a.live),
            a.reason,
            a.kind,
            a.qname,
            loc
        );
    }
}
