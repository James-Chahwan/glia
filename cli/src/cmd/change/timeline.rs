//! `glia timeline` (CD.5d) — the time-travel graph: the human surface over
//! `glia_engine::timeline` (CD.5c).
//!
//! - `build <repo> [--revs N] [--head REV]` builds the last `N` commits of
//!   `REV`'s first-parent chain (default 20 of `HEAD`, `1..=200`) and writes
//!   the sidecar `<repo>/.glia/graph/timeline.gmap` (self-gitignored). Prints
//!   one row per built rev (index, short sha, UTC date, subject), the counts,
//!   every skipped rev with its reason, and where the sidecar went: `not
//!   written` under `GLIA_NO_PERSIST=1`.
//! - `history <repo> <qname> [--category C]` reads the sidecar and prints one
//!   line per edge span that ever touched the node (every id it had, moves
//!   followed): `CALLS -> app::b::g   since 1a2b3c4 2026-09-01 'subject'
//!   until 9f8e7d6 2026-09-03 'subject'` (`still present` while the edge is
//!   there at the window's last rev; `(before the window)` when it was already
//!   present at the first rev, so its true start is earlier), `<-` for an edge
//!   into the node, then the other end's kind and last-seen `file:line`. The
//!   CURRENT graph is loaded for the absences only (coverage caveats and
//!   suggestions): the default layout through `persist::load_or_rebuild`, which
//!   serves a fresh layout as is and rebuilds a stale or missing one (written
//!   back unless `GLIA_NO_PERSIST=1`). An empty answer prints its absence.
//! - `as-of <repo> <rev>` reads the sidecar and prints the graph at one rev
//!   (an index of the window, or a commit id prefix of at least 7 hex chars):
//!   the rev, its node and edge counts, and edges by category.
//!
//! `--json` prints the engine's struct: `TimelineBuilt`, the `Answer` of
//! `EdgeHistoryRow`s (`{results, absence}`) or `AsOfSummary`. `history` before
//! any `build` (or over a sidecar an older glia wrote) is not an error: it
//! prints the engine's note naming `glia timeline build` (with `--json`,
//! `{"results": [], "absence": null, "note": "<note>"}`) and exits 0.
//!
//! Exit codes (the `delta` convention): 0 on an answer, an absence or that
//! note; 2 on a git or build error (`build`: not a git work tree, an unknown
//! `--head`, no rev of the window built), a current graph that cannot be
//! loaded (`history`), and a missing sidecar or a rev naming nothing
//! (`as-of`).
//!
//! Fired-on markers: the engine's `[timeline] repo=<label> revs=<N> ...` /
//! `[timeline] history <qname> rows=<r>` / `[timeline] as_of <sha> ...`, then
//! this surface's `[timeline] surface=cli <build|history|as_of> rows=<n>`
//! (`n`: revs, history rows, or the view's edges).

use std::path::Path;

use clap::Subcommand;
use glia_engine::absence::Answer;
use glia_engine::persist::{default_layout_dir, load_or_rebuild};
use glia_engine::timeline::{
    AsOfSummary, DEFAULT_REVS, EdgeHistoryRow, MAX_REVS, RevRef, TimelineArgs, TimelineBuilt, as_of,
    build_timeline, edge_history, load_timeline,
};

use crate::cmd::resolve::print_absence;

/// An answer, an absence, or the no-timeline note.
const EXIT_OK: i32 = 0;
/// A git or build error, an unloadable graph, or a rev naming nothing.
const EXIT_ERROR: i32 = 2;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: TimelineCmd,
}

#[derive(Subcommand, Debug)]
enum TimelineCmd {
    /// Build the last `--revs` commits of `--head`'s first-parent chain (one
    /// incremental build each on the shared parse cache) and write the
    /// sidecar `<repo>/.glia/graph/timeline.gmap`, unless
    /// `GLIA_NO_PERSIST=1`. Prints the built revs and the counts.
    Build {
        /// Path to the repo (a git work tree).
        repo: String,
        /// Commits in the window, 1..=200.
        #[arg(
            long,
            default_value_t = DEFAULT_REVS,
            value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..=MAX_REVS as u64)
        )]
        revs: usize,
        /// The git rev whose first-parent chain the window ends at.
        #[arg(long, default_value = "HEAD")]
        head: String,
        /// Emit the engine's `TimelineBuilt` as JSON instead of the table.
        #[arg(long)]
        json: bool,
    },
    /// Every edge span that ever touched the node <qname> names (the exact
    /// qname, else the one qname ending `::<qname>`), from the sidecar:
    /// since which rev, until which rev or still present.
    History {
        /// Path to the repo (a git work tree with a built timeline).
        repo: String,
        /// The node: a qname, or its unique `::` suffix (`f`, `b::f`).
        qname: String,
        /// Keep only the edges of this category (`CALLS`, `IMPORTS`, ...;
        /// case-insensitive).
        #[arg(long)]
        category: Option<String>,
        /// Emit the engine's answer (`{results, absence}`) as JSON.
        #[arg(long)]
        json: bool,
    },
    /// The graph at one rev of the timeline: an index of the window (0 is
    /// the oldest) or a commit id prefix of at least 7 hex chars. Prints the
    /// rev and the counts by category.
    AsOf {
        /// Path to the repo (a git work tree with a built timeline).
        repo: String,
        /// A rev index or a commit id prefix of at least 7 hex chars.
        rev: String,
        /// Emit the engine's `AsOfSummary` as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(args: Args) -> i32 {
    match args.action {
        TimelineCmd::Build { repo, revs, head, json } => build(&repo, revs, head, json),
        TimelineCmd::History { repo, qname, category, json } => history(&repo, &qname, category.as_deref(), json),
        TimelineCmd::AsOf { repo, rev, json } => as_of_rev(&repo, &rev, json),
    }
}

fn build(repo: &str, revs: usize, head: String, json: bool) -> i32 {
    let mut a = TimelineArgs::default();
    a.revs = revs;
    a.head = head;
    let built = match build_timeline(repo, &a) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e}");
            return EXIT_ERROR;
        }
    };
    eprintln!("[timeline] surface=cli build rows={}", built.revs.len());
    if json {
        println!("{}", serde_json::to_string(&built).unwrap_or_default());
    } else {
        print_built(repo, &a.head, &built);
    }
    EXIT_OK
}

fn history(repo: &str, qname: &str, category: Option<&str>, json: bool) -> i32 {
    let store = match load_timeline(repo) {
        Ok(s) => s,
        Err(note) => {
            eprintln!("[timeline] surface=cli history rows=0");
            if json {
                // Built by hand to keep the answer's field order (serde_json
                // here sorts a `Value`'s keys).
                let note = serde_json::to_string(&note).unwrap_or_default();
                println!("{{\"results\":[],\"absence\":null,\"note\":{note}}}");
            } else {
                println!("> {note}");
            }
            return EXIT_OK;
        }
    };
    let root = Path::new(repo);
    let current = match load_or_rebuild(&default_layout_dir(root), Some(root), true) {
        Ok((result, _outcome)) => result,
        Err(e) => {
            eprintln!("error: the current graph of {repo}: {e}");
            return EXIT_ERROR;
        }
    };
    let mut answer = edge_history(&current.merged, &store, qname, category);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = current.parse_errors.len();
    }
    eprintln!("[timeline] surface=cli history rows={}", answer.results.len());
    if json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
    } else {
        print_history(qname, category, store.revs.len(), &answer);
    }
    EXIT_OK
}

fn as_of_rev(repo: &str, rev: &str, json: bool) -> i32 {
    let summary = match load_timeline(repo).and_then(|s| as_of(&s, rev)) {
        Ok(view) => view.summary(),
        Err(e) => {
            eprintln!("error: {e}");
            return EXIT_ERROR;
        }
    };
    eprintln!("[timeline] surface=cli as_of rows={}", summary.edges);
    if json {
        println!("{}", serde_json::to_string(&summary).unwrap_or_default());
    } else {
        print_as_of(repo, &summary);
    }
    EXIT_OK
}

/// The first 7 chars of a commit id.
fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// `<short sha> <date> '<subject>'`.
fn rev_label(r: &RevRef) -> String {
    format!("{} {} '{}'", short(&r.sha), utc_date(r.time), r.subject)
}

/// A subject as one markdown table cell.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

fn print_built(repo: &str, head: &str, b: &TimelineBuilt) {
    println!("# glia timeline build `{repo}` ({} revs of {head})", b.revs.len());
    println!();
    println!("| rev | sha | date | subject |");
    println!("|---|---|---|---|");
    for r in &b.revs {
        println!("| {} | {} | {} | {} |", r.index, short(&r.sha), utc_date(r.time), cell(&r.subject));
    }
    println!();
    println!(
        "- nodes: {} | edges: {} ({} spans, {} closed in the window) | moves chained: {}",
        b.nodes, b.edges, b.edge_spans, b.closed, b.moves
    );
    for (sha, why) in &b.skipped {
        println!("- skipped {}: {why}", short(sha));
    }
    match &b.written {
        Some(p) => println!("- written: {p}"),
        None => println!("- not written (GLIA_NO_PERSIST=1)"),
    }
}

/// One history line (module docs).
fn history_line(r: &EdgeHistoryRow) -> String {
    let arrow = if r.direction == "in" { "<-" } else { "->" };
    let before = if r.since_window_start { " (before the window)" } else { "" };
    let until = r.until.as_ref().map_or_else(|| "still present".to_string(), |u| format!("until {}", rev_label(u)));
    let at = match (r.file.as_deref(), r.line) {
        (Some(f), Some(l)) => format!(" at {f}:{l}"),
        (Some(f), None) => format!(" at {f}"),
        _ => String::new(),
    };
    format!(
        "{} {arrow} {}   since {}{before}   {until}   [{}{at}]",
        r.category,
        r.other_qname,
        rev_label(&r.since),
        r.other_kind
    )
}

fn print_history(qname: &str, category: Option<&str>, revs: usize, a: &Answer<EdgeHistoryRow>) {
    let of = category.map_or_else(String::new, |c| format!(" ({c} only)"));
    println!("# glia timeline history `{qname}`{of} over {revs} revs");
    println!();
    if let Some(abs) = &a.absence {
        println!("_(no edge history)_");
        println!();
        print_absence(abs);
        return;
    }
    for r in &a.results {
        println!("{}", history_line(r));
    }
}

fn print_as_of(repo: &str, s: &AsOfSummary) {
    println!("# glia timeline as-of `{repo}` rev {} ({})", s.rev.index, rev_label(&s.rev));
    println!();
    println!("- nodes: {} | edges: {}", s.nodes, s.edges);
    println!();
    if s.by_category.is_empty() {
        println!("_(no edges at this rev)_");
        return;
    }
    println!("| category | edges |");
    println!("|---|---|");
    for (c, n) in &s.by_category {
        println!("| {c} | {n} |");
    }
}

/// Unix seconds as their UTC day, `YYYY-MM-DD`: Howard Hinnant's
/// civil-from-days over the proleptic Gregorian calendar (eras of 400 years,
/// March-based years so the leap day ends one). No locale, no wall clock. A
/// copy of `cmd/query/hotspots.rs`'s private helper (that file is CC.10b's):
/// hoisting both into `common.rs` removes it.
fn utc_date(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_date_known_days() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert_eq!(utc_date(1_767_225_599), "2025-12-31");
        assert_eq!(utc_date(1_767_225_600), "2026-01-01");
    }

    #[test]
    fn subjects_escape_table_pipes() {
        assert_eq!(cell("a | b"), "a \\| b");
        assert_eq!(short("0123456789abcdef"), "0123456");
        assert_eq!(short("abc"), "abc");
    }
}
