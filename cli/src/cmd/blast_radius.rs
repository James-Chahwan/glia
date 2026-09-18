//! `glia blast-radius` (P3) — the complete, edge-category-aware, PPR-ranked,
//! located closure around <qname>, across service boundaries, in one call.

use crate::common::{ImpactDirection, generate_for};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Qname or simple name of the seed entity.
    qname: String,
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

pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let qname = args.qname.as_str();
    let with = args.with.as_slice();
    let (direction, depth, top_k, live_only) = (args.direction, args.depth, args.top_k, args.live_only);
    let scope = args.scope.as_deref();
    let json = args.json;
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let dir = match direction {
        ImpactDirection::Forward => "forward",
        ImpactDirection::Backward => "backward",
        ImpactDirection::Both => "both",
    };
    let answer = match repo_graph_engine::blast_radius_by_qname(
        &result.merged,
        qname,
        dir,
        depth,
        top_k,
        live_only,
        scope,
    ) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("hint: use `glia analyze {repo} --format json` to list qnames.");
            return 3;
        }
    };
    if json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    println!("# glia blast-radius `{qname}` ({dir}, depth ≤ {depth})");
    println!();
    if answer.is_empty() {
        println!("_(nothing in radius)_");
        return 0;
    }
    println!("| score | depth | live | via | kind | qname | location |");
    println!("|--:|--:|:-:|---|---|---|---|");
    for a in &answer {
        let loc = match (&a.file, a.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        let live = if a.live { "●" } else { "⊘" };
        println!(
            "| {:.4} | {} | {} | {} | {} | `{}` | {} |",
            a.score, a.depth, live, a.reason, a.kind, a.qname, loc
        );
    }
    0
}
