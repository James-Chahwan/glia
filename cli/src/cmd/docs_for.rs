//! `glia docs-for` (tier-4 P3 payoff) — the doc sections that DOCUMENTS
//! <qname>, located.

use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Qname or simple name of the code entity.
    qname: String,
    /// Additional repos to merge in. Repeatable.
    #[arg(long)]
    with: Vec<String>,
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
    let qname = args.qname.as_str();
    let with = args.with.as_slice();
    let scope = args.scope.as_deref();
    let json = args.json;
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let docs = match repo_graph_engine::governing_docs(&result.merged, qname, scope) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("hint: use `glia analyze {repo} --format json` to list qnames.");
            return 3;
        }
    };
    if json {
        println!("{}", serde_json::to_string(&docs).unwrap_or_default());
        return 0;
    }
    println!("# glia docs-for `{qname}`");
    println!();
    if docs.is_empty() {
        println!("_(no governing docs)_");
        return 0;
    }
    println!("| kind | doc section | location |");
    println!("|---|---|---|");
    for d in &docs {
        let loc = match (&d.file, d.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        println!("| {} | `{}` | {} |", d.kind, d.qname, loc);
    }
    0
}
