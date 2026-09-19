//! `glia resolve` (P3) — a failure/change signal (stacktrace, diff, test id)
//! → the ranked, located nodes it points at.

use glia_engine::absence::Absence;

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
    let mut answer =
        glia_engine::resolve_signal_located(&result.merged, signal, kind, top_k, scope);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    if json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    println!("# glia resolve ({kind})");
    println!();
    if let Some(a) = &answer.absence {
        println!("_(nothing resolved)_");
        print_absence(a);
        return 0;
    }
    println!("| score | live | kind | qname | location |");
    println!("|--:|:-:|---|---|---|");
    for a in &answer.results {
        let loc = match (&a.file, a.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        println!("| {:.4} | {} | {} | `{}` | {} |", a.score, live_glyph(a.live), a.kind, a.qname, loc);
    }
    0
}

/// The `live` column of the answer tables (LD.6), as `blast-radius` renders
/// it: `●` reachable from an entrypoint, `⊘` likely dead. Used by `resolve`,
/// `trace`, `docs-for` and `find`.
pub(crate) fn live_glyph(live: bool) -> &'static str {
    if live { "●" } else { "⊘" }
}

/// The LD.8a absence block, printed under an empty answer's `_(...)_` line
/// by `resolve`, `docs-for` and `find`: the FACT, one line per coverage
/// caveat, the unparsed-file count when there is one, and the suggestions.
pub(crate) fn print_absence(a: &Absence) {
    println!("> FACT: {}", a.note);
    for c in &a.caveats {
        println!(
            "> caveat ({}, {}): {} - verify: {}",
            c.language, c.edge_category, c.note, c.verify
        );
    }
    if a.unparsed_files > 0 {
        println!(
            "> {} file(s) failed to parse, so the graph may be missing what they hold",
            a.unparsed_files
        );
    }
    if !a.suggestions.is_empty() {
        println!("> did you mean: {}", a.suggestions.join(", "));
    }
}
