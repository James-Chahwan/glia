//! `glia check` (LE.8) — the CI gate over the engine's `check::check`: every
//! declared `[[constraint]]` rule evaluated, one section per violated rule
//! with its located evidence. Exit 0 clean, 1 on a violation, 2 on a build
//! failure or a rule that could not be evaluated. `--json` is the report.

use repo_graph_engine::check::{
    CheckReport, FORBID_EDGE, MAX_EVIDENCE, Violation, ViolationEdge, check,
};

use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Emit the report as JSON instead of tables (same exit codes).
    #[arg(long)]
    json: bool,
}

/// Exit code: 2 when a rule could not be evaluated, else 1 on a violation,
/// else 0.
fn exit_code(r: &CheckReport) -> i32 {
    if !r.errors.is_empty() {
        2
    } else if !r.violations.is_empty() {
        1
    } else {
        0
    }
}

/// `file:line`, `file`, or `—`.
fn at(e: &ViolationEdge) -> String {
    match (e.file.as_deref(), e.line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

fn print_edges(rows: &[ViolationEdge]) {
    println!("| # | category | from | to | at | emitter |");
    println!("|--:|---|---|---|---|---|");
    for (i, e) in rows.iter().enumerate() {
        println!(
            "| {} | {} | `{}` | `{}` | {} | {} |",
            i + 1,
            e.category,
            e.from_qname,
            e.to_qname,
            at(e),
            e.emitter.as_deref().unwrap_or("—"),
        );
    }
    println!();
}

/// One section per violated rule: a forbid_edge rule's edges, or each cycle
/// of a no_cycle rule with its witness.
fn print_rule(group: &[&Violation]) {
    let Some(first) = group.first() else {
        return;
    };
    let decl = first.decl.as_deref().unwrap_or("cell API");
    let n: usize = if first.rule_kind == FORBID_EDGE {
        group.iter().map(|v| v.count).sum()
    } else {
        group.len()
    };
    println!(
        "### {} ({}, {decl}) - {n} violation(s)",
        first.rule_id, first.rule_kind
    );
    println!();
    for (i, v) in group.iter().enumerate() {
        if v.rule_kind == FORBID_EDGE {
            println!("_{} forbidden edge(s), tier {}:_", v.count, v.tier);
        } else {
            println!(
                "_cycle {}: {} node(s) in the component, tier {}; a shortest cycle through it:_",
                i + 1,
                v.count,
                v.tier
            );
        }
        println!();
        print_edges(&v.evidence);
        if v.count > v.evidence.len() && v.rule_kind == FORBID_EDGE {
            println!(
                "_(showing the first {} of {}; at most {MAX_EVIDENCE} are listed)_",
                v.evidence.len(),
                v.count
            );
            println!();
        }
    }
}

pub(crate) fn run(args: Args) -> i32 {
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let report = check(&result.merged);
    let code = exit_code(&report);
    if args.json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
        return code;
    }
    println!("# glia check `{}`", args.repo);
    println!();
    if report.rules == 0 {
        println!("_(no rules declared - add [[constraint]] stanzas to .glia/overlay.toml)_");
        return code;
    }
    println!(
        "- rules: {} (checked {}, unchecked {}, errors {})",
        report.rules,
        report.checked,
        report.unchecked.len(),
        report.errors.len()
    );
    println!("- violations: {}", report.violations.len());
    println!();
    if report.violations.is_empty() {
        println!("_(no violations)_");
        println!();
    }
    // Violations arrive sorted by rule id: group the runs.
    let mut i = 0;
    while i < report.violations.len() {
        let id = &report.violations[i].rule_id;
        let group: Vec<&Violation> = report.violations[i..]
            .iter()
            .take_while(|v| &v.rule_id == id)
            .collect();
        i += group.len();
        print_rule(&group);
    }
    if !report.unchecked.is_empty() {
        println!("## unchecked (not machine-checkable)");
        println!();
        for id in &report.unchecked {
            println!("- `{id}`");
        }
        println!();
    }
    if !report.errors.is_empty() {
        println!("## rule errors (not evaluated)");
        println!();
        for (id, msg) in &report.errors {
            println!("- `{id}`: {msg}");
        }
        println!();
    }
    code
}
