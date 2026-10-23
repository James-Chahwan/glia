//! `glia blast-radius` (P3) — the complete, edge-category-aware, PPR-ranked,
//! located closure around one or more seeds, across service boundaries, in
//! one call. Many seeds are one walk and one ranking (LD.5): each row names
//! the seed whose wave reached it, and a seed another seed reaches in one
//! hop is listed as linked, never dropped.
//!
//! DD.10: identity classes collapse by default. A walk class (one message,
//! contract op, SCIP symbol, or a package and its project, in several places)
//! is one node in the walk, and a FACT / DERIVED class is one row: its
//! location cell appends `(also in: <repo> <file>:<line>, ...)` and
//! `; diverged` when the members' contents differ, and `--json` carries the
//! row's `identity`. `--no-collapse` is the per-member walk and rows.
//!
//! 0.5.3 (EA.8): the rows are ordered by the measured file ranking
//! (`--ranking ppr|ppr-wide|bfs`, co-change from `.glia/history-snapshot`
//! first unless `--no-cochange`) over one `RankCache` holding the build's
//! repo roots (the `--with` repos' included), and the table is followed by a
//! `## files` table of the `--top-files` best files (`--json`: `files`).
//! Fired_on, stderr: `[rank-cache] holder=cli ..` and the engine's `[rank]
//! surface=blast_radius ranking=<P1|PT*|B0|H10|H10[PT*]|H10[B0]> ..`.
//!
//! EC.8: `--ranking` has no default. Absent, the rows follow the repo's
//! calibrated pick (`glia history calibrate`; SELh0, or SELs0 under
//! `--no-cochange`), else `ppr`; given, it is explicit. A `ranking: <pick>
//! (<source>[: <reason>], shape <k>)` line follows the table, `--json`
//! carries `selection`, and stderr the engine's `[select]
//! surface=blast_radius ..` line.

use glia_engine::identity::{Collapse, RowIdentity};
use glia_engine::{BlastAnswer, BlastOptions, BlastRadius};
use glia_graph::Reach;

use crate::cmd::resolve::{live_glyph, print_absence};
use crate::common::{
    CalibratedRankingArgs, ImpactDirection, applied_ranking, generate_for, rank_cache, render_files, render_selection,
};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Qnames or simple names of the seed entities: one or more, answered
    /// as one radius. A name that matches no node is reported, not fatal.
    #[arg(required = true, num_args = 1.., value_name = "QNAME")]
    qnames: Vec<String>,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Which way the radius spreads.
    #[arg(long, value_enum, default_value_t = ImpactDirection::Both)]
    direction: ImpactDirection,
    /// Maximum hops along carry edges.
    #[arg(long, default_value_t = 4)]
    depth: usize,
    /// Keep only the top-K by PPR score.
    #[arg(long)]
    top_k: Option<usize>,
    /// Drop nodes not reachable from an entrypoint (likely-dead code).
    #[arg(long)]
    live_only: bool,
    /// Restrict the answer to a repo-relative path or a project label (see
    /// `glia projects`), e.g. `services/api` or `@shop/web`. Narrows
    /// WITHIN a repo: each `--with` repo's paths are relative to its OWN
    /// root, so a path passed as a separate repo will not match here.
    /// Nodes with no file (ENDPOINT / ROUTE / doc spaces) are kept, not
    /// dropped.
    #[arg(long)]
    scope: Option<String>,
    /// Walk and list every member of an identity class on its own. By
    /// default a package, message, contract op or SCIP symbol found in
    /// several places (a FACT / DERIVED SAME_AS class) is one row naming the
    /// other places (`also in`), and a walk class is one node in the walk.
    #[arg(long)]
    no_collapse: bool,
    #[command(flatten)]
    ranking: CalibratedRankingArgs,
    /// How many ranked files the `## files` table (`--json`: `files`) lists.
    #[arg(long, value_name = "N", default_value_t = BlastOptions::default().top_files)]
    top_files: usize,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

/// Exit 0 with the answer (empty or not), 2 when the build fails, 3 when
/// every seed query is unresolved — the absence is printed either way.
pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let result = match generate_for(repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
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
    opts.collapse = if args.no_collapse {
        Collapse::off()
    } else {
        Collapse::with_labels(&result.repo_labels)
    };
    args.ranking.apply(&mut opts);
    opts.top_files = args.top_files;
    opts.rank_cache = Some(rank_cache(&result));
    let queries: Vec<&str> = args.qnames.iter().map(String::as_str).collect();
    let mut answer = glia_engine::blast_radius(&result.merged, &queries, &opts);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    let code = if answer.seeds.is_empty() && !answer.unresolved.is_empty() {
        eprintln!("hint: use `glia analyze {repo} --format json` to list qnames.");
        3
    } else {
        0
    };
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return code;
    }
    render(&answer, &queries, dir, args.depth);
    render_selection(answer.selection.as_ref());
    if answer.absence.is_none() {
        let ranking = applied_ranking(answer.selection.as_ref(), opts.ranking);
        render_files(&answer.files, ranking, result.repo_labels.len() > 1);
    }
    code
}

/// The markdown table: a `seed` column when there is more than one seed, the
/// seeds' links and the unresolved queries as `>` lines, the absence block
/// under an empty answer.
fn render(answer: &BlastRadius, queries: &[&str], dir: &str, depth: usize) {
    let asked = queries.iter().map(|q| format!("`{q}`")).collect::<Vec<_>>().join(", ");
    println!("# glia blast-radius {asked} ({dir}, depth ≤ {depth})");
    println!();
    if !answer.unresolved.is_empty() {
        println!("> unresolved: {}", answer.unresolved.join(", "));
    }
    for s in answer.seeds.iter().filter(|s| !s.linked_seeds.is_empty()) {
        println!("> seed `{}` is one carry edge from: {}", s.qname, s.linked_seeds.join(", "));
    }
    if !answer.unresolved.is_empty() || answer.seeds.iter().any(|s| !s.linked_seeds.is_empty()) {
        println!();
    }
    if let Some(a) = &answer.absence {
        println!("_(nothing in radius)_");
        print_absence(a);
        return;
    }
    let many = answer.seeds.len() > 1;
    if many {
        println!("| score | depth | live | via | kind | qname | seed | location |");
        println!("|--:|--:|:-:|---|---|---|---|---|");
    } else {
        println!("| score | depth | live | via | kind | qname | location |");
        println!("|--:|--:|:-:|---|---|---|---|");
    }
    for a in &answer.results {
        let seed = if many { format!(" `{}` |", a.seed) } else { String::new() };
        println!(
            "| {:.4} | {} | {} | {} | {} | `{}` |{seed} {} |",
            a.score,
            a.depth,
            live_glyph(a.live),
            a.reason,
            a.kind,
            a.qname,
            location_cell(a)
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

/// The table's location cell: the row's place (`—` without one), then, for a
/// row that stands for an identity class, `(also in: <repo> <file>:<line>,
/// ...; diverged)`.
fn location_cell(a: &BlastAnswer) -> String {
    let at = place(a.file.as_deref(), a.line).unwrap_or_else(|| "—".to_string());
    match &a.identity {
        Some(id) => format!("{at} {}", also_in(id, &a.qname)),
        None => at,
    }
}

/// `(also in: ...; diverged)`. An unlocated member is its repo, and its
/// qname when that differs from the row's (a PACKAGE_DEP class shares one
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
