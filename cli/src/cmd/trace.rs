//! `glia trace` (P3 cross_stack_trace) — follow <feature> forward across
//! service boundaries; the ordered path with a mechanism per hop.

use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Qname or simple name of the feature/entry entity.
    feature: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Maximum hops.
    #[arg(long, default_value_t = 6)]
    depth: usize,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let feature = args.feature.as_str();
    let with = args.with.as_slice();
    let (depth, json) = (args.depth, args.json);
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let hops = match repo_graph_engine::cross_stack_trace(&result.merged, feature, depth) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("hint: use `glia analyze {repo} --format json` to list qnames.");
            return 3;
        }
    };
    if json {
        println!("{}", serde_json::to_string(&hops).unwrap_or_default());
        return 0;
    }
    println!("# glia trace `{feature}` (depth ≤ {depth})");
    println!();
    if hops.is_empty() {
        println!("_(no outward flow)_");
        return 0;
    }
    println!("| depth | mechanism | xsvc | from | → to | location |");
    println!("|--:|---|:-:|---|---|---|");
    for h in &hops {
        let loc = match (&h.to_file, h.to_line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        let xsvc = if h.cross_service { "✔" } else { "" };
        println!(
            "| {} | {} | {} | `{}` | `{}` ({}) | {} |",
            h.depth, h.mechanism, xsvc, h.from_qname, h.to_qname, h.to_kind, loc
        );
    }
    0
}
