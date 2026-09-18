//! `glia find` (LD.3b) — ranked fuzzy symbol lookup over name / qname, each
//! row naming the tier that matched it and located. The engine's
//! `find::find_nodes` does the work; this parses `--kind` names and prints.
//! `--json` is the LD.8a envelope `{results, absence}`; an empty table prints
//! the absence block under `_(no match)_`.

use repo_graph_code_domain::node_kind;
use repo_graph_core::NodeKindId;
use repo_graph_engine::find::{DEFAULT_TOP_K, FindOptions, find_nodes};

use crate::cmd::resolve::print_absence;
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
    println!("| match | kind | qname | location |");
    println!("|---|---|---|---|");
    for r in &answer.results {
        let loc = match (&r.file, r.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        println!("| {} | {} | `{}` | {} |", r.r#match, r.kind, r.qname, loc);
    }
    0
}
