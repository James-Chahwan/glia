//! `glia trace` (P3 cross_stack_trace, LD.4a) — follow <feature> forward
//! across service boundaries and print the ranked distinct paths it takes,
//! each hop with its mechanism; `--to` asks for the paths between two nodes.

use glia_engine::trace::{TraceAnswer, TraceOptions, cross_stack_trace};

use crate::cmd::resolve::{live_glyph, print_absence};
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
    /// Maximum hops per path.
    #[arg(long, default_value_t = 6)]
    depth: usize,
    /// Trace to this node (qname or simple name): the ranked paths from
    /// <FEATURE> to it, or the shortest undirected one when no directed
    /// path exists.
    #[arg(long)]
    to: Option<String>,
    /// Keep the first N ranked paths (0 keeps every one).
    #[arg(long, default_value_t = 10)]
    max_paths: usize,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut opts = TraceOptions::default();
    opts.depth = args.depth;
    opts.to = args.to.clone();
    opts.max_paths = args.max_paths;
    let answer = cross_stack_trace(&result.merged, &args.feature, &opts);
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    render(&args, &answer);
    0
}

fn render(args: &Args, answer: &TraceAnswer) {
    let (feature, depth) = (args.feature.as_str(), args.depth);
    match &args.to {
        Some(to) => println!("# glia trace `{feature}` → `{to}` (depth ≤ {depth})"),
        None => println!("# glia trace `{feature}` (depth ≤ {depth})"),
    }
    println!();
    if let Some(a) = &answer.absence {
        let empty = match (&answer.seed, &answer.target, &args.to) {
            (None, _, _) => "_(nothing resolved)_",
            (Some(_), None, Some(_)) => "_(target not resolved)_",
            (Some(_), _, Some(_)) => "_(no path)_",
            (Some(_), _, None) => "_(no outward flow)_",
        };
        println!("{empty}");
        print_absence(a);
        return;
    }
    for (i, p) in answer.paths.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!(
            "### path {} - {} hops, {} cross-service ({})",
            p.rank,
            p.length,
            p.cross_service_hops,
            p.mechanisms.join(", ")
        );
        if !p.directed {
            println!("_undirected: no carry path runs this way; the shortest path over any edge, walked either way_");
        }
        println!();
        println!("| depth | mechanism | xsvc | from | → to | live | location |");
        println!("|--:|---|:-:|---|---|:-:|---|");
        for h in &p.hops {
            let loc = match (&h.to_file, h.to_line) {
                (Some(f), Some(l)) => format!("{f}:{l}"),
                (Some(f), None) => f.clone(),
                _ => "—".to_string(),
            };
            let xsvc = if h.cross_service { "✔" } else { "" };
            println!(
                "| {} | {} | {} | `{}` | `{}` ({}) | {} | {} |",
                h.depth,
                h.mechanism,
                xsvc,
                h.from_qname,
                h.to_qname,
                h.to_kind,
                live_glyph(h.to_live),
                loc
            );
        }
    }
    if answer.truncated {
        println!();
        println!(
            "> truncated: the path search stopped at its step budget ({}); the ranking covers the paths found before it. Lower --depth to see them all.",
            glia_engine::trace::EXPANSION_BUDGET
        );
    }
}
