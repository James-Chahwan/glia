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
//! whose wave reached the row first. A row standing for an identity class
//! names its other members' repos by the build's labels (DD.10). `--json`
//! prints the engine's `DiffImpact` (`{base, changed, edges_added,
//! edges_removed, impact, unresolved_diff_files}`). Every `file:line` is
//! 1-based (LD.1).
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
//! 0.5.3 (EA.8): `--ranking ppr|ppr-wide|bfs` and `--no-cochange` pick the
//! file ranking that orders the impact rows (co-change from the working
//! tree's `.glia/history-snapshot` first by default), over one `RankCache`
//! (holder `cli`) holding the build's repo roots: the working tree's in rev
//! mode, the build's (the `--with` repos' included) for `--diff`. A `##
//! files` table of the `--top-files` best files follows the impact table
//! (`--json`: `impact.files`); `[rank] surface=diff_impact ranking=<r> ..`.
//!
//! EC.8: `--ranking` has no default. Absent, the impact rows follow the
//! repo's calibrated pick (`glia history calibrate`, read through the same
//! `RankCache`; SELh0, or SELs0 under `--no-cochange`), else `ppr`; given, it
//! is explicit. A `ranking: <pick> (<source>[: <reason>], shape <k>)` line
//! follows the impact table, `--json` carries `impact.selection`, and stderr
//! the engine's `[select] surface=diff_impact ..` line.
//!
//! FH.8: the impact rows and seeds are cut to `--top-k` after the ranking,
//! in the table and the JSON; `--full` keeps every one. A cut answer's JSON
//! carries `impact.truncated: {results, seeds}` and the table ends with a
//! `_(<shown> of <total> rows ..)_` line. The JSON leaves out null, empty and
//! default fields (the list is on the engine's `DiffImpact` and
//! `BlastRadius`).
//!
//! FH.8c, the overview answer (as `glia blast-radius`): `--top-k` defaults
//! to `DEFAULT_OVERVIEW_ROWS` (25; it was 200), the seeds and the `changed`
//! rows to 25 (`changed_truncated` counts the rest, the `## Changed` table
//! says so); `impact.areas` maps every row by area with the `--scope` that
//! selects it (`## areas` after `## files`), `impact.truncated.next` names
//! the largest area's `--scope` and the next `--offset`, `--offset <N>`
//! skips the first N impact rows, and the rows are slim unless `--full`
//! (`seed_index`, no `name` repeating the qname's tail, a 4-digit `score`).
//! stderr, per `--json`: `[answer-size] surface=diff_impact rows=<r>
//! shown=<k> offset=<o> areas=<a> bytes=<b> ..`.
//!
//! Fired-on marker: the engine's
//! `[diff-impact] mode=<rev|diff> base=<rev|-> changed=<C> seeds=<S> impact=<I> edges +<a> -<r> unresolved_files=<U>`.

use std::io::Read;

use glia_engine::delta::{DeltaEdge, graph_delta_vs_rev};
use glia_engine::diff_impact::{DiffImpact, diff_impact_from_delta, diff_impact_from_diff};
use glia_engine::identity::Collapse;
use glia_engine::{BlastOptions, DEFAULT_OVERVIEW_ROWS, answer_json};
use glia_graph::Reach;

use crate::cmd::blast_radius::{render_areas, truncated_note};
use crate::cmd::resolve::{live_glyph, print_absence};
use crate::common::{
    CalibratedRankingArgs, ImpactDirection, applied_ranking, build_options, generate_for,
    rank_cache, render_files, render_selection,
};

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
    /// Keep the first K impact rows and the first K (at most 25) seeds and
    /// changed rows of the ranked answer; a cut answer says how many it left
    /// out and which `--scope` / `--offset` reads more (`impact.truncated`).
    #[arg(long, value_name = "K", default_value_t = DEFAULT_OVERVIEW_ROWS)]
    top_k: usize,
    /// Skip the first N impact rows of the ranked, scoped answer (the next
    /// page).
    #[arg(long, value_name = "N", default_value_t = 0)]
    offset: usize,
    /// Keep every impact row, seed and changed row, every field whole: no
    /// `--top-k` cut, no areas, no slim rows.
    #[arg(long, conflicts_with_all = ["top_k", "offset"])]
    full: bool,
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
    #[command(flatten)]
    ranking: CalibratedRankingArgs,
    /// How many ranked files the `## files` table (`--json`: `impact.files`)
    /// lists.
    #[arg(long, value_name = "N", default_value_t = BlastOptions::default().top_files)]
    top_files: usize,
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

/// The answer for whichever change source `args` names, and how many repos
/// its build holds.
fn answer(args: &Args, opts: &BlastOptions) -> Result<(DiffImpact, usize), String> {
    if let Some(src) = &args.diff {
        if args.base.is_some() {
            return Err(
                "give one change source: --base <rev> or --diff <file|->, not both".to_string(),
            );
        }
        let text = read_diff(src)?;
        let built = generate_for(&args.repo, &args.with)?;
        // Collapsing with the build's repo labels when the collapse is on and
        // has none (DD.10, as `glia review` does), so a collapsed row's
        // `also_in` names each repo by its label, not `repo#<id % 1000>`.
        let mut opts = opts.clone();
        if opts.collapse.enabled && opts.collapse.repo_labels.is_empty() {
            opts.collapse = Collapse::with_labels(&built.repo_labels);
        }
        opts.rank_cache = Some(rank_cache(&built));
        let mut a = diff_impact_from_diff(&built.merged, &text, &opts);
        if let Some(x) = a.impact.absence.as_mut() {
            x.unparsed_files = built.parse_errors.len();
        }
        return Ok((a, built.repo_labels.len()));
    }
    if !args.with.is_empty() {
        return Err(
            "--base builds the one repo against its git rev; --with needs --diff".to_string(),
        );
    }
    if !build_options().overlay {
        return Err(
            "--no-overlay does not apply to --base: both sides are built with the repo's overlay"
                .to_string(),
        );
    }
    // `diff_impact_vs_rev`'s two steps, so the radius gets a CLI-held rank
    // cache over the working tree's roots between them (the engine labels
    // the collapse with its repo labels, FD.28).
    let rev = graph_delta_vs_rev(&args.repo, args.base.as_deref().unwrap_or("HEAD"))?;
    let mut opts = opts.clone();
    opts.rank_cache = Some(rank_cache(&rev.after));
    Ok((
        diff_impact_from_delta(&rev, &opts),
        rev.after.repo_labels.len(),
    ))
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
    opts.top_k = (!args.full).then_some(args.top_k);
    opts.offset = args.offset;
    opts.live_only = args.live_only;
    opts.scope = args.scope.clone();
    args.ranking.apply(&mut opts);
    opts.top_files = args.top_files;
    let (a, repos) = match answer(&args, &opts) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if args.json {
        let text = answer_json(
            "diff_impact",
            &a,
            a.impact.rows_total(),
            a.impact.results.len(),
        );
        println!("{}", text.unwrap_or_default());
    } else {
        let source = match (&a.base, &args.diff) {
            (Some(b), _) => format!("vs `{b}`"),
            (None, Some(d)) => format!("diff `{d}`"),
            (None, None) => "diff".to_string(),
        };
        render(&args.repo, &source, &a, dir, args.depth);
        render_selection(a.impact.selection.as_ref());
        if a.impact.absence.is_none() {
            let ranking = applied_ranking(a.impact.selection.as_ref(), opts.ranking);
            render_files(&a.impact.files, ranking, repos > 1);
            render_areas(&a.impact);
        }
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
    // FH.8c: a cut `changed` list no longer holds every seed row.
    let seeds = if a.changed_truncated > 0 {
        a.impact.size().seeds
    } else {
        a.changed.iter().filter(|c| c.seed).count()
    };
    println!("# glia diff-impact `{repo}` ({source}, {dir}, depth ≤ {depth})");
    println!();
    println!(
        "- changed: {} nodes, {seeds} seeds; edges +{} -{}; in radius: {}",
        a.changed.len() + a.changed_truncated,
        a.edges_added.len(),
        a.edges_removed.len(),
        a.impact.rows_total()
    );
    if !a.unresolved_diff_files.is_empty() {
        println!(
            "- diff files no node placed: {}",
            a.unresolved_diff_files.join(", ")
        );
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
        if a.changed_truncated > 0 {
            println!();
            println!(
                "_({} of {} changed; --full lists every row)_",
                a.changed.len(),
                a.changed.len() + a.changed_truncated
            );
        }
        println!();
    }
    edge_rows("Edges added", &a.edges_added);
    edge_rows("Edges removed", &a.edges_removed);
    println!("## Impact");
    println!();
    for s in a.impact.seeds.iter().filter(|s| !s.linked_seeds.is_empty()) {
        println!(
            "> seed `{}` is one carry edge from: {}",
            s.qname,
            s.linked_seeds.join(", ")
        );
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
    if let Some(note) = truncated_note(&a.impact) {
        println!();
        println!("{note}");
    }
}
