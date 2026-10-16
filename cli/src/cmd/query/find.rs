//! `glia find` (LD.3b) — ranked fuzzy symbol lookup over name / qname, each
//! row naming the tier that matched it and located. The engine's
//! `find::find_nodes` does the work; this parses `--kind` names and prints.
//! `--json` is the LD.8a envelope `{results, absence}`; an empty table prints
//! the absence block under `_(no match)_`.
//!
//! DD.9: a FACT / DERIVED identity class (one package, message or contract op
//! in several places) is one row; its location cell appends `(also in: <repo>
//! <file>:<line>, ...)` and `; diverged` when the members' contents differ,
//! and `--json` carries the row's `identity`. `--no-collapse` is one row per
//! member.

use glia_code_domain::node_kind;
use glia_core::NodeKindId;
use glia_engine::find::{DEFAULT_TOP_K, FindOptions, FoundNode, find_nodes};
use glia_engine::identity::Collapse;

use crate::cmd::resolve::{live_glyph, print_absence};
use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// A symbol, a qname (`UserService::get_user`), a dotted or slashed path,
    /// or a fragment of one.
    query: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Keep the first N rows; 0 keeps every match.
    #[arg(long, default_value_t = DEFAULT_TOP_K)]
    top_k: usize,
    /// Keep only nodes of this kind (a node-kind name such as FUNCTION or
    /// CLASS). Repeatable.
    #[arg(long)]
    kind: Vec<String>,
    /// Restrict the answer to a repo-relative path or a project label (see
    /// `glia projects`). Nodes with no file (ENDPOINT / ROUTE / doc spaces)
    /// are kept, not dropped.
    #[arg(long)]
    scope: Option<String>,
    /// One row per member of an identity class. By default a package,
    /// message or contract op found in several places (a FACT / DERIVED
    /// SAME_AS class) is one row naming the other places (`also in`).
    #[arg(long)]
    no_collapse: bool,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

/// Node-kind names → ids, case-insensitively. An unknown name is an error
/// naming every valid one.
fn parse_kinds(names: &[String]) -> Result<Vec<NodeKindId>, String> {
    names
        .iter()
        .map(|n| {
            node_kind::ALL
                .iter()
                .find(|(_, name)| name.eq_ignore_ascii_case(n))
                .map(|(id, _)| *id)
                .ok_or_else(|| {
                    let valid: Vec<&str> = node_kind::ALL.iter().map(|(_, name)| *name).collect();
                    format!("unknown --kind '{n}'; valid kinds: {}", valid.join(", "))
                })
        })
        .collect()
}

pub(crate) fn run(args: Args) -> i32 {
    let kinds = match parse_kinds(&args.kind) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut opts = FindOptions::default();
    opts.top_k = args.top_k;
    opts.kinds = (!kinds.is_empty()).then_some(kinds);
    opts.scope = args.scope;
    opts.collapse = if args.no_collapse {
        Collapse::off()
    } else {
        Collapse::with_labels(&result.repo_labels)
    };
    let mut answer = find_nodes(&result.merged, &args.query, &opts);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    println!("# glia find `{}`", args.query);
    println!();
    if let Some(a) = &answer.absence {
        println!("_(no match)_");
        print_absence(a);
        return 0;
    }
    println!("| match | live | kind | qname | location |");
    println!("|---|:-:|---|---|---|");
    for r in &answer.results {
        println!(
            "| {} | {} | {} | `{}` | {} |",
            r.r#match,
            live_glyph(r.live),
            r.kind,
            r.qname,
            location_cell(r)
        );
    }
    0
}

/// `file:line`, `file` or `—`.
fn place(file: Option<&str>, line: Option<i64>) -> Option<String> {
    match (file, line) {
        (Some(f), Some(l)) => Some(format!("{f}:{l}")),
        (Some(f), None) => Some(f.to_string()),
        _ => None,
    }
}

/// The table's location cell: the row's place, then, for a row that stands
/// for an identity class, `(also in: <repo> <file>:<line>, ...; diverged)`.
/// An unlocated member is its repo (and its qname when that differs from the
/// row's: a PACKAGE_DEP class shares one qname, so its repo is what differs).
fn location_cell(r: &FoundNode) -> String {
    let at = place(r.file.as_deref(), r.line).unwrap_or_else(|| "—".to_string());
    let Some(id) = &r.identity else {
        return at;
    };
    let others: Vec<String> = id
        .also_in
        .iter()
        .map(|a| match place(a.file.as_deref(), a.line) {
            Some(p) => format!("{} {p}", a.repo),
            None if a.qname != r.qname => format!("{} `{}`", a.repo, a.qname),
            None => a.repo.clone(),
        })
        .collect();
    let diverged = if id.diverged { "; diverged" } else { "" };
    format!("{at} (also in: {}{diverged})", others.join(", "))
}
