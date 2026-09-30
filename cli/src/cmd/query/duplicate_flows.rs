//! `glia duplicate-flows` (CD.4f) — the human surface over the engine's
//! `duplicate_flows::duplicate_flows` (CD.4e): entry flows whose reached sets
//! are identical (exact, tier derived) or overlap at a Jaccard threshold
//! (near, tier heuristic), with utility hubs and test entries left out.
//!
//! A header line counts the entries compared, the flows, the utility hubs
//! left out and the LSH candidate pairs; then one block per group:
//! `## exact (derived): GET /orders = GET /v2/orders  (3 shared nodes)` or
//! `## near (heuristic, jaccard 0.78): POST /orders ~ PUT /orders/<id>  (14
//! shared nodes of 18)`, its entries (`qname  KIND  file:line`), its `glia
//! arch` services and the nodes it `differs by` (the engine keeps the first
//! ten by qname; a line counts the rest).
//!
//! A report: it exits 0 whatever it finds, and an empty answer prints the
//! engine's absence (why none). Exit 2 is a build failure or a `--threshold`
//! outside (0, 1] (the engine takes any value: above 1 finds no near group,
//! 0 pairs every candidate). `--json` is the whole `DuplicateFlows` answer.
//! The engine's `[dupflows] ... surface=cli` stderr line is the fired_on
//! marker.

use glia_engine::Located;
use glia_engine::duplicate_flows::{
    DEFAULT_DEPTH, DEFAULT_MIN_SIZE, DEFAULT_THRESHOLD, DupFlowArgs, DupFlowGroup, DuplicateFlows,
    EXACT, duplicate_flows,
};

use crate::cmd::resolve::print_absence;
use crate::common::generate_for;

/// [`DupFlowArgs::surface`] for the marker.
const SURFACE: &str = "cli";
/// Entries a group's heading names before `+N more`.
const TITLE_ENTRIES: usize = 4;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Compare only the entries under this repo-relative path or project
    /// label (see `glia projects`); their flows may leave it.
    #[arg(long)]
    scope: Option<String>,
    /// Hops each entry's flow follows over the carry edges.
    #[arg(long, default_value_t = DEFAULT_DEPTH)]
    depth: usize,
    /// The least Jaccard overlap a near pair keeps, in (0, 1]; 1 keeps only
    /// the exact groups.
    #[arg(long, default_value_t = DEFAULT_THRESHOLD)]
    threshold: f64,
    /// Flows reaching fewer nodes are skipped (0 acts as 1).
    #[arg(long, default_value_t = DEFAULT_MIN_SIZE)]
    min_size: usize,
    /// Compare test entries too (left out by default: table tests would fill
    /// the exact groups).
    #[arg(long)]
    include_tests: bool,
    /// Keep the utility hubs (fan-in hubs such as a logger) in every flow
    /// instead of leaving them out.
    #[arg(long)]
    keep_hubs: bool,
    /// Emit JSON instead of text.
    #[arg(long)]
    json: bool,
}

/// Why `--threshold t` is refused, or `None` for a number in (0, 1].
fn threshold_error(t: f64) -> Option<String> {
    if t > 0.0 && t <= 1.0 {
        None
    } else {
        Some(format!("--threshold must be in (0, 1], got {t}"))
    }
}

pub(crate) fn run(args: Args) -> i32 {
    if let Some(e) = threshold_error(args.threshold) {
        eprintln!("error: {e}");
        return 2;
    }
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut query = DupFlowArgs::default();
    query.scope = args.scope;
    query.depth = args.depth;
    query.threshold = args.threshold;
    query.min_size = args.min_size;
    query.include_tests = args.include_tests;
    query.keep_hubs = args.keep_hubs;
    query.surface = SURFACE;
    let mut answer = duplicate_flows(&result.merged, &result.repo_labels, &query);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }

    println!("# glia duplicate-flows `{}`", args.repo);
    println!();
    for line in header(&answer, &query) {
        println!("{line}");
    }
    println!();
    if let Some(a) = &answer.absence {
        println!("_(no duplicate flows)_");
        println!();
        print_absence(a);
        return 0;
    }
    for g in &answer.groups {
        for line in group_block(g) {
            println!("{line}");
        }
        println!();
    }
    0
}

/// The counts the groups were found among, and the LSH buckets skipped.
fn header(a: &DuplicateFlows, q: &DupFlowArgs) -> Vec<String> {
    let exact = a.groups.iter().filter(|g| g.kind == EXACT).count();
    let hubs = if q.keep_hubs {
        "utility hubs kept".to_string()
    } else {
        format!("{} utility hub(s) left out", a.hubs_ignored)
    };
    let tests = if q.include_tests {
        "test entries included"
    } else {
        "test entries left out"
    };
    let mut lines = vec![
        format!(
            "- {} entries compared ({tests}), {} flows of >= {} nodes within {} hops; {hubs}",
            a.entries,
            a.flows,
            q.min_size.max(1),
            q.depth
        ),
        format!(
            "- {} exact group(s), {} near group(s) at jaccard >= {} from {} LSH candidate pair(s)",
            exact,
            a.groups.len() - exact,
            q.threshold,
            a.candidates
        ),
    ];
    if a.oversized_buckets > 0 {
        lines.push(format!(
            "- {} LSH band bucket(s) too big to pair were skipped: a near pair only they would find is missed",
            a.oversized_buckets
        ));
    }
    lines
}

/// `## <kind> (<tier>[, jaccard j]): <entries>  (<shared>)`, then the
/// entries, the services and the `differs by` rows.
fn group_block(g: &DupFlowGroup) -> Vec<String> {
    let exact = g.kind == EXACT;
    let names: Vec<&str> = g.entries.iter().map(|e| e.qname.as_str()).collect();
    let sep = if exact { " = " } else { " ~ " };
    let mut title = names
        .iter()
        .take(TITLE_ENTRIES)
        .copied()
        .collect::<Vec<_>>()
        .join(sep);
    if names.len() > TITLE_ENTRIES {
        title.push_str(&format!(" (+{} more)", names.len() - TITLE_ENTRIES));
    }
    let head = if exact {
        format!(
            "## {} ({}): {title}  ({} shared nodes)",
            g.kind, g.tier, g.shared
        )
    } else {
        format!(
            "## {} ({}, jaccard {:.2}): {title}  ({} shared nodes of {})",
            g.kind, g.tier, g.jaccard, g.shared, g.union
        )
    };
    let mut lines = vec![head, String::new()];
    lines.extend(g.entries.iter().map(|e| format!("- {}", row(e))));
    if !g.services.is_empty() {
        lines.push(format!("- services: {}", g.services.join(", ")));
    }
    lines.extend(
        g.differing
            .iter()
            .map(|d| format!("- differs by {}", row(d))),
    );
    let more = (g.union - g.shared).saturating_sub(g.differing.len());
    if more > 0 {
        lines.push(format!("- differs by {more} more node(s)"));
    }
    lines
}

/// `` `qname`  KIND  file:line ``.
fn row(l: &Located) -> String {
    format!(
        "`{}`  {}  {}",
        l.qname,
        l.kind,
        at(l.file.as_deref(), l.line)
    )
}

/// `file:line`, the file alone, or `—` for an unlocated node.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{at, threshold_error};

    #[test]
    fn threshold_is_in_zero_one() {
        for ok in [1.0, 0.8, 0.01] {
            assert_eq!(threshold_error(ok), None, "{ok}");
        }
        for bad in [0.0, -0.5, 1.5, f64::NAN, f64::INFINITY] {
            let e = threshold_error(bad).expect("refused");
            assert!(e.starts_with("--threshold must be in (0, 1], got "), "{e}");
        }
    }

    #[test]
    fn at_cell() {
        assert_eq!(at(Some("a.py"), Some(3)), "a.py:3");
        assert_eq!(at(Some("a.py"), None), "a.py");
        assert_eq!(at(None, Some(3)), "—");
    }
}
