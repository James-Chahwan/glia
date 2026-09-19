//! `glia diff-impact` (LE.2) — what a change affects, in one call: the human
//! surface over the engine's `diff_impact::{diff_impact_vs_rev,
//! diff_impact_from_diff}`.
//!
//! One change source: the working tree's change against a git rev
//! (`--base`, the default, `HEAD`), or a pasted unified diff / changed-file
//! list (`--diff <file>`, `-` reads stdin) over a fresh build of the repo
//! (`--with` merges more repos in; rev mode builds the one repo). Giving both
//! is a usage error.
//!
//! Table mode prints the changed nodes (each with its change and whether it
//! seeds the radius), the edges the change added or removed (rev mode), the
//! diff files no node placed (pasted mode), then the radius in
//! `blast-radius`'s column layout, always with its `seed` column: the seed
//! whose wave reached the row first. `--json` prints the engine's
//! `DiffImpact` (`{base, changed, edges_added, edges_removed, impact,
//! unresolved_diff_files}`). Every `file:line` is 1-based (LD.1).
//!
//! Exit 0 on an answer (an empty radius prints its absence), 2 on a usage
//! error (`--base` with `--diff`, `--with` or `--no-overlay` in rev mode), an
//! unreadable `--diff`, or a git / build failure with the engine's message.
//!
//! Rev mode builds through the engine's graph delta (LE.1b), which saves the
//! working tree's parse-cache sidecar (`<repo>/.glia/graph/parse_cache.bin`,
//! self-gitignored) as an incremental build does, under `GLIA_NO_PERSIST=1`
//! too; never a `.gmap` layout.
//!
//! Fired-on marker: the engine's
//! `[diff-impact] mode=<rev|diff> base=<rev|-> changed=<C> seeds=<S> impact=<I> edges +<a> -<r> unresolved_files=<U>`.

use std::io::Read;

use repo_graph_engine::BlastOptions;
use repo_graph_engine::delta::DeltaEdge;
use repo_graph_engine::diff_impact::{DiffImpact, diff_impact_from_diff, diff_impact_vs_rev};
use repo_graph_graph::Reach;

use crate::cmd::resolve::{live_glyph, print_absence};
use crate::common::{ImpactDirection, build_options, generate_for};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root (a git work tree in rev mode).
    repo: String,
    /// The git rev to compare the working tree against (a branch, tag, sha
    /// or `HEAD~N`); the default change source, as `HEAD`.
    #[arg(long, value_name = "REV")]
    base: Option<String>,
    /// A unified diff or a changed-file list in this file, instead of a git
    /// rev; `-` reads stdin.
    #[arg(long, value_name = "FILE")]
    diff: Option<String>,
    /// Which way the radius spreads.
    #[arg(long, value_enum, default_value_t = ImpactDirection::Both)]
    direction: ImpactDirection,
    /// Maximum hops along carry edges, from the nearest seed.
    #[arg(long, default_value_t = 4)]
    depth: usize,
    /// Keep only the top-K rows by PPR score.
    #[arg(long)]
    top_k: Option<usize>,
    /// Drop rows not reachable from an entrypoint (likely-dead code).
    #[arg(long)]
    live_only: bool,
    /// Keep only the rows under this repo-relative path or project label,
    /// before the `--top-k` cut.
    #[arg(long)]
    scope: Option<String>,
    /// Additional repos to merge in (cross-service). Repeatable; `--diff`
    /// only.
    #[arg(long)]
    with: Vec<String>,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

/// The diff text `--diff` names: a file, or stdin for `-`.
fn read_diff(src: &str) -> Result<String, String> {
    if src == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| format!("reading the diff from stdin: {e}"))?;
        return Ok(text);
    }
    std::fs::read_to_string(src).map_err(|e| format!("reading the diff {src}: {e}"))
}

/// The answer for whichever change source `args` names.
fn answer(args: &Args, opts: &BlastOptions) -> Result<DiffImpact, String> {
    if let Some(src) = &args.diff {
        if args.base.is_some() {
            return Err("give one change source: --base <rev> or --diff <file|->, not both".to_string());
        }
        let text = read_diff(src)?;
        let built = generate_for(&args.repo, &args.with)?;
        let mut a = diff_impact_from_diff(&built.merged, &text, opts);
        if let Some(x) = a.impact.absence.as_mut() {
            x.unparsed_files = built.parse_errors.len();
        }
        return Ok(a);
    }
    if !args.with.is_empty() {
        return Err("--base builds the one repo against its git rev; --with needs --diff".to_string());
    }
    if !build_options().overlay {
        return Err(
            "--no-overlay does not apply to --base: both sides are built with the repo's overlay".to_string(),
        );
    }
    diff_impact_vs_rev(&args.repo, args.base.as_deref().unwrap_or("HEAD"), opts)
}

pub(crate) fn run(args: Args) -> i32 {
    let (direction, dir) = match args.direction {
        ImpactDirection::Forward => (Reach::Forward, "forward"),
        ImpactDirection::Backward => (Reach::Backward, "backward"),
        ImpactDirection::Both => (Reach::Both, "both"),
    };
    let mut opts = BlastOptions::default();
    opts.direction = direction;
    opts.depth = args.depth;
    opts.top_k = args.top_k;
    opts.live_only = args.live_only;
    opts.scope = args.scope.clone();
    let a = match answer(&args, &opts) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if args.json {
        println!("{}", serde_json::to_string(&a).unwrap_or_default());
    } else {
        let source = match (&a.base, &args.diff) {
            (Some(b), _) => format!("vs `{b}`"),
            (None, Some(d)) => format!("diff `{d}`"),
            (None, None) => "diff".to_string(),
        };
        render(&args.repo, &source, &a, dir, args.depth);
    }
    0
}

/// `file:line`, `file`, or `—`.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

fn edge_rows(title: &str, edges: &[DeltaEdge]) {
    if edges.is_empty() {
        return;
    }
    println!("## {title}");
    println!();
    println!("| category | from | to | site |");
    println!("|---|---|---|---|");
    for e in edges {
        println!(
            "| {} | `{}` | `{}` | {} |",
            e.category,
            e.from_qname,
            e.to_qname,
            at(e.site_file.as_deref(), e.site_line)
        );
    }
    println!();
}

fn render(repo: &str, source: &str, a: &DiffImpact, dir: &str, depth: usize) {
    let seeds = a.changed.iter().filter(|c| c.seed).count();
    println!("# glia diff-impact `{repo}` ({source}, {dir}, depth ≤ {depth})");
    println!();
    println!(
        "- changed: {} nodes, {seeds} seeds; edges +{} -{}; in radius: {}",
        a.changed.len(),
        a.edges_added.len(),
        a.edges_removed.len(),
        a.impact.results.len()
    );
    if !a.unresolved_diff_files.is_empty() {
        println!("- diff files no node placed: {}", a.unresolved_diff_files.join(", "));
    }
    println!();
    if !a.changed.is_empty() {
        println!("## Changed");
        println!();
        println!("| change | seed | kind | qname | location |");
        println!("|---|:-:|---|---|---|");
        for c in &a.changed {
            println!(
                "| {} | {} | {} | `{}` | {} |",
                c.change,
                if c.seed { "●" } else { "" },
                c.kind,
                c.qname,
                at(c.file.as_deref(), c.line)
            );
        }
        println!();
    }
    edge_rows("Edges added", &a.edges_added);
    edge_rows("Edges removed", &a.edges_removed);
    println!("## Impact");
    println!();
    for s in a.impact.seeds.iter().filter(|s| !s.linked_seeds.is_empty()) {
        println!("> seed `{}` is one carry edge from: {}", s.qname, s.linked_seeds.join(", "));
    }
    if a.impact.seeds.iter().any(|s| !s.linked_seeds.is_empty()) {
        println!();
    }
    if let Some(x) = &a.impact.absence {
        println!("_(nothing in radius)_");
        print_absence(x);
        return;
    }
    println!("| score | depth | live | via | kind | qname | seed | location |");
    println!("|--:|--:|:-:|---|---|---|---|---|");
    for r in &a.impact.results {
        println!(
            "| {:.4} | {} | {} | {} | {} | `{}` | `{}` | {} |",
            r.score,
            r.depth,
            live_glyph(r.live),
            r.reason,
            r.kind,
            r.qname,
            r.seed,
            at(r.file.as_deref(), r.line)
        );
    }
}
