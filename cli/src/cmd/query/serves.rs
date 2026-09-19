//! `glia serves` (LD.8b) — who serves a channel: the human surface over the
//! engine's `serves::serves`. `--json` is the LD.8a envelope
//! `{results, absence}`; an empty table prints the absence block (the FACT,
//! the caveat rows, the near misses) under `_(nothing serves it)_`. An absence
//! is an answer, so it exits 0; exit 2 is a build failure.

use repo_graph_engine::serves::serves;

use crate::cmd::resolve::{live_glyph, print_absence};
use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// The channel: `METHOD /path` (`POST /orders`), a bare `/path` (every
    /// verb), or a queue topic (`orders.created`).
    channel: String,
    /// How to read the channel: `auto` takes a leading HTTP verb or `/` as
    /// HTTP and anything else as a queue topic.
    #[arg(long, default_value = "auto", value_parser = ["auto", "http", "queue"])]
    mechanism: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

/// `file:line`, `file`, or `—`.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

pub(crate) fn run(args: Args) -> i32 {
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut answer = match serves(&result.merged, &args.channel, &args.mechanism) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    println!("# glia serves `{}`", args.channel.trim());
    println!();
    if let Some(a) = &answer.absence {
        println!("_(nothing serves it)_");
        print_absence(a);
        return 0;
    }
    println!("| match | live | kind | server | location | handlers |");
    println!("|---|:-:|---|---|---|---|");
    for s in &answer.results {
        let handlers = if s.handlers.is_empty() {
            "—".to_string()
        } else {
            s.handlers
                .iter()
                .map(|h| format!("`{}` {}", h.qname, at(h.file.as_deref(), h.line)))
                .collect::<Vec<_>>()
                .join("<br>")
        };
        println!(
            "| {} ({}) | {} | {} | `{}` | {} | {handlers} |",
            s.r#match,
            s.confidence,
            live_glyph(s.live),
            s.kind,
            s.qname,
            at(s.file.as_deref(), s.line)
        );
    }
    0
}
