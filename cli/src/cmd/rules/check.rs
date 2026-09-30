//! `glia check` (LE.8) — the CI gate over the engine's `check::check`: every
//! declared `[[constraint]]` rule evaluated, one section per violated rule
//! with its located evidence. Exit 0 clean, 1 on a violation, 2 on a build
//! failure or a rule that could not be evaluated. `--json` is the report.
//! Each evidence row carries the tier `glia why` gives that edge (CC.3), its
//! note in parentheses; the exit code never reads the tier.
//!
//! A declared reflexion model (CC.5b) prints as a `## reflexion model` section
//! before the violations (CC.5c): its components, the component dependency
//! matrix, the absences with their coverage caveats and the unmapped files.
//! Its divergences are violations, one per-edge section each like a
//! forbid_edge rule's, so they exit 1.

use glia_engine::check::{
    CONVERGENCE, CheckReport, DIVERGENCE, FORBID_EDGE, MAX_EVIDENCE, MAX_UNMAPPED_SAMPLE,
    Reflexion, Violation, ViolationEdge, check,
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

/// The row's tier, with its note in parentheses when it has one.
fn tier(e: &ViolationEdge) -> String {
    match e.note.as_deref() {
        Some(n) => format!("{} ({n})", e.tier),
        None => e.tier.to_string(),
    }
}

fn print_edges(rows: &[ViolationEdge]) {
    println!("| # | category | from | to | at | emitter | tier |");
    println!("|--:|---|---|---|---|---|---|");
    for (i, e) in rows.iter().enumerate() {
        println!(
            "| {} | {} | `{}` | `{}` | {} | {} | {} |",
            i + 1,
            e.category,
            e.from_qname,
            e.to_qname,
            at(e),
            e.emitter.as_deref().unwrap_or("—"),
            tier(e),
        );
    }
    println!();
}

/// Caveat lines printed per absence; `--json` carries them all.
const MAX_ABSENCE_CAVEATS: usize = 5;

/// A forbid_edge rule and a reflexion divergence report edges (`count` is the
/// edges, the evidence at most [`MAX_EVIDENCE`] of them); a no_cycle rule
/// reports cycles.
fn per_edge(rule_kind: &str) -> bool {
    rule_kind == FORBID_EDGE || rule_kind == DIVERGENCE
}

/// One section per violated rule: a forbid_edge rule's edges, a reflexion
/// divergence's edges, or each cycle of a no_cycle rule with its witness.
fn print_rule(group: &[&Violation]) {
    let Some(first) = group.first() else {
        return;
    };
    let decl = first.decl.as_deref().unwrap_or("cell API");
    let n: usize = if per_edge(first.rule_kind) {
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
        } else if v.rule_kind == DIVERGENCE {
            println!(
                "_{} edge(s) the model does not allow, tier {}:_",
                v.count, v.tier
            );
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
        if v.count > v.evidence.len() && per_edge(v.rule_kind) {
            println!(
                "_(showing the first {} of {}; at most {MAX_EVIDENCE} are listed)_",
                v.evidence.len(),
                v.count
            );
            println!();
        }
    }
}

/// The `## reflexion model` section: components, the dependency matrix in
/// `(from, to)` order, the absences with at most [`MAX_ABSENCE_CAVEATS`]
/// caveats each, and the unmapped files.
fn print_model(m: &Reflexion) {
    println!(
        "## reflexion model ({})",
        if m.closed { "closed" } else { "open" }
    );
    println!();
    if m.closed {
        println!(
            "_{} {CONVERGENCE}(s), {} {DIVERGENCE}(s), {} absence(s)_",
            m.convergences,
            m.divergences,
            m.absences.len()
        );
    } else {
        println!(
            "_open: dependencies are observed, not judged - add [[layer]] or kind = \"allow\" stanzas to close it_"
        );
    }
    println!();
    for c in &m.components {
        let paths = if c.paths.is_empty() {
            "—".to_string()
        } else {
            c.paths.join(", ")
        };
        match c.layer.as_deref() {
            Some(l) => println!("- {}: {paths} (layer {l}, {} nodes)", c.name, c.nodes),
            None => println!("- {}: {paths} ({} nodes)", c.name, c.nodes),
        }
    }
    println!();
    if m.matrix.is_empty() {
        println!("_(no dependency between components)_");
    } else {
        println!("| from | to | edges | status | tier | allowed by |");
        println!("|---|---|--:|---|---|---|");
        for c in &m.matrix {
            println!(
                "| {} | {} | {} | {} | {} | {} |",
                c.from,
                c.to,
                c.edges,
                c.status,
                c.tier,
                c.allowed_by.as_deref().unwrap_or("—"),
            );
        }
    }
    println!();
    println!("### absences");
    println!();
    if m.absences.is_empty() {
        println!("_(none)_");
    }
    for a in &m.absences {
        println!(
            "- {} -> {}: allowed by {} ({}), no edge found",
            a.from,
            a.to,
            a.rule_id,
            a.decl.as_deref().unwrap_or("cell API"),
        );
        for c in a.caveats.iter().take(MAX_ABSENCE_CAVEATS) {
            println!("  ⚠ {} ({}): {}", c.edge_category, c.language, c.note);
        }
        if a.caveats.len() > MAX_ABSENCE_CAVEATS {
            println!(
                "  _(+{} more caveat(s); --json lists all)_",
                a.caveats.len() - MAX_ABSENCE_CAVEATS
            );
        }
    }
    println!();
    println!("### unmapped");
    println!();
    let u = &m.unmapped;
    if u.files == 0 {
        println!("_(none: every located file maps to a component)_");
    } else {
        let more = if u.files > u.sample.len() {
            ", ..."
        } else {
            ""
        };
        println!(
            "{} files, {} nodes, {} edges into components; e.g. {}{more}",
            u.files,
            u.nodes,
            u.edges_to_mapped,
            u.sample.join(", "),
        );
        if u.files > u.sample.len() {
            println!();
            println!(
                "_(showing {} of {} files; at most {MAX_UNMAPPED_SAMPLE} are listed)_",
                u.sample.len(),
                u.files
            );
        }
    }
    println!();
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
    if report.rules == 0 && report.reflexion.is_none() {
        println!("_(no rules declared - add [[constraint]] stanzas to .glia/overlay.toml)_");
        println!(
            "_(or model the architecture: [[component]] stanzas map paths to components; [[layer]] and kind = \"allow\" judge their dependencies)_"
        );
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
    if let Some(m) = &report.reflexion {
        print_model(m);
        // The rule sections below are not the model's.
        println!("## violations");
        println!();
    }
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
