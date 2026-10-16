//! `glia trace` (P3 cross_stack_trace, LD.4a) — follow <feature> forward
//! across service boundaries and print the ranked distinct paths it takes,
//! each hop with its mechanism; `--to` asks for the paths between two nodes.
//!
//! DD.11: the walk crosses identity classes. A walk class (one message,
//! contract op or SCIP symbol, or a package and its project, in several
//! places) is one node of every path, shown as its display member; a hop into
//! a FACT / DERIVED class appends `(also in: <repo> <file>:<line>, ...)` to its
//! location cell (`; diverged` when the members' contents differ) and
//! `--json` carries the hop's `to_identity`. `--no-collapse` is the
//! per-member walk.

use glia_engine::identity::{Collapse, RowIdentity};
use glia_engine::trace::{TraceAnswer, TraceHop, TraceOptions, cross_stack_trace};

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
    /// Walk every member of an identity class on its own. By default a
    /// message, contract op or SCIP symbol found in several places (a walk
    /// class) is one node of every path, so a path reaching one copy goes on
    /// through every copy's successors.
    #[arg(long)]
    no_collapse: bool,
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
    opts.collapse = if args.no_collapse {
        Collapse::off()
    } else {
        Collapse::with_labels(&result.repo_labels)
    };
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
            let loc = location_cell(h);
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

/// `file:line`, `file` or `None`.
fn place(file: Option<&str>, line: Option<i64>) -> Option<String> {
    match (file, line) {
        (Some(f), Some(l)) => Some(format!("{f}:{l}")),
        (Some(f), None) => Some(f.to_string()),
        _ => None,
    }
}

/// A hop's location cell: its destination's place (`—` without one), then,
/// for a hop into an identity class, `(also in: ...; diverged)` - the
/// `glia blast-radius` cell (DD.10).
fn location_cell(h: &TraceHop) -> String {
    let at = place(h.to_file.as_deref(), h.to_line).unwrap_or_else(|| "—".to_string());
    match &h.to_identity {
        Some(id) => format!("{at} {}", also_in(id, &h.to_qname)),
        None => at,
    }
}

/// `(also in: ...; diverged)`. An unlocated member is its repo, and its
/// qname when that differs from the hop's (a PACKAGE_DEP class shares one
/// qname, so its repo is what tells the members apart).
fn also_in(id: &RowIdentity, qname: &str) -> String {
    let others: Vec<String> = id
        .also_in
        .iter()
        .map(|m| match place(m.file.as_deref(), m.line) {
            Some(p) => format!("{} {p}", m.repo),
            None if m.qname != qname => format!("{} `{}`", m.repo, m.qname),
            None => m.repo.clone(),
        })
        .collect();
    let diverged = if id.diverged { "; diverged" } else { "" };
    format!("(also in: {}{diverged})", others.join(", "))
}
