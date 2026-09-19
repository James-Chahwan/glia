//! `glia docs-for` (tier-4 P3 payoff) — the doc sections that DOCUMENTS
//! <qname>, located. An unknown qname is an absence answer (exit 0, with
//! suggestions), not an error.

use crate::cmd::resolve::{live_glyph, print_absence};
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
    let mut docs = glia_engine::governing_docs(&result.merged, qname, scope);
    if let Some(a) = docs.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    if json {
        println!("{}", serde_json::to_string(&docs).unwrap_or_default());
        return 0;
    }
    println!("# glia docs-for `{qname}`");
    println!();
    if let Some(a) = &docs.absence {
        println!("_(no governing docs)_");
        print_absence(a);
        return 0;
    }
    println!("| live | kind | doc section | location |");
    println!("|:-:|---|---|---|");
    for d in &docs.results {
        let loc = match (&d.file, d.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        println!("| {} | {} | `{}` | {} |", live_glyph(d.live), d.kind, d.qname, loc);
    }
    0
}
