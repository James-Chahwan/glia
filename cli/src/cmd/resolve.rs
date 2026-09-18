//! `glia resolve` (P3) — a failure/change signal (stacktrace, diff, test id)
//! → the ranked, located nodes it points at.

use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// The signal text (stacktrace, diff hunk, test id, or free text).
    signal: String,
    /// Additional repos to merge in. Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Signal kind: `auto` (sniff), `stacktrace`, `test`, or `diff`.
    #[arg(long, default_value = "auto")]
    kind: String,
    /// Keep only the top-K by relevance.
    #[arg(long)]
    top_k: Option<usize>,
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
    let signal = args.signal.as_str();
    let with = args.with.as_slice();
    let kind = args.kind.as_str();
    let top_k = args.top_k;
    let scope = args.scope.as_deref();
    let json = args.json;
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let answer =
        repo_graph_engine::resolve_signal_located(&result.merged, signal, kind, top_k, scope);
    if json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    println!("# glia resolve ({kind})");
    println!();
    if answer.is_empty() {
        println!("_(nothing resolved)_");
        return 0;
    }
    println!("| score | kind | qname | location |");
    println!("|--:|---|---|---|");
    for a in &answer {
        let loc = match (&a.file, a.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        println!("| {:.4} | {} | `{}` | {} |", a.score, a.kind, a.qname, loc);
    }
    0
}
