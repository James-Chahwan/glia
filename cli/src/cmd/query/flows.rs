//! `glia flows` (LD.4b) — every entry point's forward flow: the human surface
//! over the engine's `trace::entry_flows`. The table has one row per flow,
//! `| key | kind | entry | reach | xsvc | mechanisms | location |`; `--json`
//! is the engine's `Vec<EntryFlow>`, each row carrying its located hops and
//! services. The key is the feature word `glia trace` resolves to that entry
//! when the word names a dead end. A report: exits 0 whatever it finds; exit
//! 2 is a build failure.

use repo_graph_engine::trace::{DEFAULT_DEPTH, EntryFlow, entry_flows};

use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Maximum hops each flow follows from its entry point.
    #[arg(long, default_value_t = DEFAULT_DEPTH)]
    depth: usize,
    /// Emit JSON (the list of flows) instead of a table.
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
    // The `[flows] entries=` fired_on marker is printed inside the engine.
    let flows = entry_flows(&result.merged, &result.repo_labels, args.depth);
    if args.json {
        println!("{}", serde_json::to_string(&flows).unwrap_or_default());
        return 0;
    }
    render(&args, &flows);
    0
}

/// `file:line`, `file`, or `—`.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

fn render(args: &Args, flows: &[EntryFlow]) {
    println!("# glia flows `{}` (depth ≤ {})", args.repo, args.depth);
    println!();
    if flows.is_empty() {
        println!(
            "_(no entry point reaches anything within {} hops)_",
            args.depth
        );
        return;
    }
    println!("| key | kind | entry | reach | xsvc | mechanisms | location |");
    println!("|---|---|---|--:|:-:|---|---|");
    for f in flows {
        println!(
            "| `{}` | {} | `{}` | {} | {} | {} | {} |",
            f.key,
            f.entry.kind,
            f.entry.qname,
            f.reach,
            if f.cross_service { "✔" } else { "" },
            f.mechanisms.join(", "),
            at(f.entry.file.as_deref(), f.entry.line)
        );
    }
}
