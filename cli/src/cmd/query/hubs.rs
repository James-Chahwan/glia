//! `glia hubs` (CD.4c) — the human surface over the engine's `hubs::hubs`
//! (CD.4b): the nodes that carry the most structural load. Three tables —
//! Fan-in (many callers: the utilities that make every blast radius large),
//! Fan-out (many callees: orchestrators) and Cross-service (callers or callees
//! in two or more `glia arch` services). Each row shows its rank, qname, kind,
//! label (utility / orchestrator / bottleneck / connector), counted edges in
//! and out, the services on each side, where it is, and its HITS authority
//! and hub scores to three decimals. A footer states the thresholds the lists
//! were cut at: `max(--min-degree, p99)` of the non-zero degrees.
//!
//! `--category` takes a registered edge-category name exactly as
//! `glia_code_domain::edge_category::ALL` spells it (clap rejects any other,
//! listing them). `--json` is the whole answer `{fan_in, fan_out,
//! cross_service, nodes, edges, p99_in, p99_out, absence}`. Exit 0 with rows;
//! 1 with none, the absence saying why; 2 on a build or usage error. The
//! engine's `[hubs] ... surface=cli` stderr line is the fired_on marker.

use glia_code_domain::edge_category;
use glia_engine::hubs::{DEFAULT_MIN_DEGREE, DEFAULT_TOP, HubArgs, HubRow, HubsAnswer, hubs};

use crate::cmd::resolve::print_absence;
use crate::common::generate_for;

/// [`HubArgs::surface`] for the marker.
const SURFACE: &str = "cli";

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Rows only for nodes under this repo-relative path or project label
    /// (see `glia projects`); their edges from outside it still count.
    #[arg(long)]
    scope: Option<String>,
    /// Rows per table; 0 keeps every qualifying row.
    #[arg(long, default_value_t = DEFAULT_TOP)]
    top: usize,
    /// Count only this edge category (default: every carry edge but test
    /// coverage, docs and manifest dependencies).
    #[arg(
        long,
        value_parser = clap::builder::PossibleValuesParser::new(
            edge_category::ALL.iter().map(|(_, name)| *name)
        ),
    )]
    category: Option<String>,
    /// The floor under the p99 thresholds: a fan-in or fan-out hub has at
    /// least this many counted edges that way.
    #[arg(long, default_value_t = DEFAULT_MIN_DEGREE)]
    min_degree: u32,
    /// Keep test nodes and their edges (left out by default: test fan-out
    /// would inflate every production node's fan-in).
    #[arg(long)]
    include_tests: bool,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut query = HubArgs::default();
    query.scope = args.scope;
    query.top = args.top;
    query.category = args.category;
    query.min_degree = args.min_degree;
    query.include_tests = args.include_tests;
    query.surface = SURFACE;
    let mut answer = hubs(&result.merged, &result.repo_labels, &query);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    let code = if answer.absence.is_some() { 1 } else { 0 };
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return code;
    }

    println!("# glia hubs");
    println!();
    let counted = query.category.as_deref().map_or_else(
        || "carry edges less tests, docs, manifests".to_string(),
        |c| format!("{c} only"),
    );
    let tests = if query.include_tests {
        "test nodes included"
    } else {
        "test nodes left out"
    };
    println!(
        "- {} nodes, {} counted edges ({counted}; {tests})",
        answer.nodes, answer.edges
    );
    println!();
    if let Some(a) = &answer.absence {
        println!("_(no hubs)_");
        println!();
        print_absence(a);
        println!();
    } else {
        print_table("Fan-in", &answer.fan_in);
        print_table("Fan-out", &answer.fan_out);
        print_table("Cross-service", &answer.cross_service);
    }
    println!("{}", footer(&answer, query.min_degree));
    code
}

/// `## <title>` and its table, or `_(none)_` when the list is empty.
fn print_table(title: &str, rows: &[HubRow]) {
    println!("## {title}");
    println!();
    if rows.is_empty() {
        println!("_(none)_");
        println!();
        return;
    }
    println!("| # | node | kind | label | in | out | services | at | authority | hub |");
    println!("|--:|---|---|---|--:|--:|---|---|--:|--:|");
    for (i, r) in rows.iter().enumerate() {
        println!(
            "| {} | `{}` | {} | {} | {} | {} | {} | {} | {:.3} | {:.3} |",
            i + 1,
            r.qname,
            r.kind,
            r.label,
            r.fan_in,
            r.fan_out,
            services(&r.caller_services, &r.callee_services),
            at(r.file.as_deref(), r.line),
            r.authority,
            r.hub,
        );
    }
    println!();
}

/// `in a, b; out c`: the services of the callers, then of the callees, a side
/// with none left out; `—` when neither side has a located service.
fn services(callers: &[String], callees: &[String]) -> String {
    let sides: Vec<String> = [("in", callers), ("out", callees)]
        .into_iter()
        .filter(|(_, s)| !s.is_empty())
        .map(|(dir, s)| format!("{dir} {}", s.join(", ")))
        .collect();
    if sides.is_empty() {
        "—".to_string()
    } else {
        sides.join("; ")
    }
}

/// `file:line`, the file alone, or `—` for an unlocated node.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

/// The thresholds the fan-in / fan-out lists were cut at, as the engine
/// computes them (`max(min_degree, p99)`), and the cross-service rule.
fn footer(a: &HubsAnswer, min_degree: u32) -> String {
    format!(
        "- thresholds: fan-in >= {} (p99 {}, --min-degree {min_degree}); fan-out >= {} (p99 {}, --min-degree {min_degree}); cross-service: callers or callees in 2+ services",
        min_degree.max(a.p99_in),
        a.p99_in,
        min_degree.max(a.p99_out),
        a.p99_out,
    )
}

#[cfg(test)]
mod tests {
    use super::{at, services};

    #[test]
    fn services_cell_names_each_side() {
        let s = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(services(&s(&["a", "b"]), &[]), "in a, b");
        assert_eq!(services(&[], &s(&["c"])), "out c");
        assert_eq!(services(&s(&["a"]), &s(&["c"])), "in a; out c");
        assert_eq!(services(&[], &[]), "—");
    }

    #[test]
    fn at_cell() {
        assert_eq!(at(Some("a.py"), Some(3)), "a.py:3");
        assert_eq!(at(Some("a.py"), None), "a.py");
        assert_eq!(at(None, Some(3)), "—");
    }
}
