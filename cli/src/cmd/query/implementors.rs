//! `glia implementors` (LD.7c) — who implements or extends a type (or
//! overrides a method), transitively, or with `--up` what it implements or
//! extends: the human surface over the engine's `implementors::implementors`.
//! `--json` is the LD.8a envelope `{results, absence}`; an empty table prints
//! the absence block (the FACT, the heritage caveat rows of the target's
//! language, the suggestions). An absence is an answer, so it exits 0; exit 2
//! is a build failure.

use glia_engine::implementors::{HierarchyDirection, implementors};

use crate::cmd::resolve::{live_glyph, print_absence};
use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// The type or method: a qname (`Shop::IRepo`, `Shop::IRepo::Get`) or an
    /// exact simple name.
    qname: String,
    /// Walk up: the supertypes (or the methods it implements / overrides).
    #[arg(long)]
    up: bool,
    /// Direct implementors (or supertypes) only, not the whole hierarchy.
    #[arg(long)]
    direct: bool,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
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
    let direction = if args.up {
        HierarchyDirection::Up
    } else {
        HierarchyDirection::Down
    };
    let mut answer = implementors(&result.merged, &args.qname, direction, !args.direct);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    let (what, none) = if args.up {
        ("supertypes", "_(no supertypes)_")
    } else {
        ("implementors", "_(no implementors)_")
    };
    println!("# glia {what} of `{}`", args.qname.trim());
    println!();
    if let Some(a) = &answer.absence {
        println!("{none}");
        print_absence(a);
        return 0;
    }
    println!("| depth | relation | tier | live | kind | qname | via | location |");
    println!("|--:|---|---|:-:|---|---|---|---|");
    for r in &answer.results {
        let via = r
            .via
            .as_deref()
            .map_or("—".to_string(), |v| format!("`{v}`"));
        let at = match (r.file.as_deref(), r.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.to_string(),
            _ => "—".to_string(),
        };
        println!(
            "| {} | {} | {} | {} | {} | `{}` | {via} | {at} |",
            r.depth,
            r.relation,
            r.tier,
            live_glyph(r.live),
            r.kind,
            r.qname
        );
    }
    0
}
