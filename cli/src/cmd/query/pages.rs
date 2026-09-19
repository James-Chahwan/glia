//! `glia pages` (LA.6e) — the human surface over the engine's
//! `pages::page_flow`: the frontend's pages, the links between them, dead deep
//! links and pages no in-repo link reaches. A report, not a gate: it exits 0
//! whatever it finds. `--json` is the `PageFlow` object
//! `{pages, links, dead, unlinked}`; `--dead-only` empties the other three.

use glia_engine::pages::page_flow;

use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Print only the dead links.
    #[arg(long)]
    dead_only: bool,
    /// Emit JSON instead of tables.
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
    // Pages, links, dead links, the server-route exemption and the sort order
    // all live in the engine; this is transport + rendering only.
    let mut flow = page_flow(&result.merged);
    eprintln!("{}", flow.marker("cli", 1 + args.with.len()));
    if args.dead_only {
        flow.pages.clear();
        flow.links.clear();
        flow.unlinked.clear();
    }
    if args.json {
        println!("{}", serde_json::to_string(&flow).unwrap_or_default());
        return 0;
    }

    println!("# glia pages `{}`", args.repo);
    if !args.dead_only {
        println!();
        println!("## Pages");
        println!();
        if flow.pages.is_empty() {
            println!("_(no client-router pages)_");
        } else {
            println!("| path | handler | where | inbound |");
            println!("|---|---|---|---|");
            for p in &flow.pages {
                let handler = match (&p.handler, &p.redirect_to) {
                    (Some(h), _) => format!("`{h}`"),
                    (None, Some(to)) => format!("redirect → `{to}`"),
                    (None, None) => "—".to_string(),
                };
                let path = if p.catchall {
                    format!("`{}` (catch-all)", p.path)
                } else {
                    format!("`{}`", p.path)
                };
                let place = at(p.handler_file.as_deref(), p.handler_line);
                println!("| {path} | {handler} | {place} | {} |", p.inbound_links);
            }
        }
        println!();
        println!("## Links");
        println!();
        if flow.links.is_empty() {
            println!("_(no resolved navigation links)_");
        } else {
            println!("| from | to | confidence | where |");
            println!("|---|---|---|---|");
            for l in &flow.links {
                println!(
                    "| `{}` | `{}` | {} | {} |",
                    l.from_qname,
                    l.to_path,
                    l.confidence,
                    at(l.from_file.as_deref(), l.from_line)
                );
            }
        }
    }
    println!();
    println!("## Dead links");
    println!();
    if flow.dead.is_empty() {
        println!("_(no dead links)_");
    } else {
        for d in &flow.dead {
            let absorbed = d
                .absorbed_by
                .as_deref()
                .map_or(String::new(), |a| format!("  (absorbed by {a})"));
            println!(
                "- `{}` -> `{}`{absorbed} — {}",
                d.from_qname,
                d.link,
                at(d.from_file.as_deref(), d.from_line)
            );
        }
    }
    if !args.dead_only {
        println!();
        println!("## Unlinked pages");
        println!();
        if flow.unlinked.is_empty() {
            println!("_(every page is linked)_");
        } else {
            println!(
                "_no in-repo link or redirect reaches these; external deep links may, and \
                 navigations built at runtime are not read (see `glia coverage`)._"
            );
            println!();
            for u in &flow.unlinked {
                println!("- `{u}`");
            }
        }
    }
    0
}
