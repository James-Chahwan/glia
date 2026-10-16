//! `glia check` (LE.8) — the CI gate over the engine's `check::check`: every
//! declared `[[constraint]]` rule evaluated, one section per violated rule
//! with its located evidence. Exit 0 when nothing blocks, 1 on a BLOCKING
//! violation, 2 on a build failure or a rule that could not be evaluated
//! (DB.3d). `--json` is the report. Each evidence row carries the tier `glia
//! why` gives that edge (CC.3), its note in parentheses.
//!
//! The exit code reads only [`Violation::blocking`], which the engine sets
//! from the violation's tier and its rule's `min_tier` (DB.3a; a heuristic
//! violation never blocks, 0.5.2 Q2). A non-blocking violation is printed in
//! full with [`ADVISORY`] on its tier line, and the summary splits the count
//! (`- violations: <V> (blocking <B>, advisory <A>)`).
//!
//! Per kind: a forbid_edge rule's and a reflexion divergence's edges in one
//! table; each cycle of a no_cycle rule with a shortest witness; each
//! `violation` row of a rule invariant (DB.3a) with its subject, location,
//! witness edges and Q5 coverage caveats; each hit of a transitive
//! forbid_edge (DB.3b) with its witness hops in path order. At most
//! [`MAX_PRINTED_PER_RULE`] violations print per rule.
//!
//! A declared reflexion model (CC.5b) prints as a `## reflexion model` section
//! before the violations (CC.5c): its components, the component dependency
//! matrix, the absences with their coverage caveats and the unmapped files.
//! Its divergences are violations, one per-edge section each like a
//! forbid_edge rule's, gated at the default `min_tier`.

use glia_engine::CoverageNote;
use glia_engine::check::{
    CONVERGENCE, CheckReport, DIVERGENCE, FORBID_EDGE, FORBID_PATH, MAX_EVIDENCE,
    MAX_UNMAPPED_SAMPLE, NO_CYCLE, RULE, Reflexion, Violation, ViolationEdge, check,
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

/// Exit code: 2 when a rule could not be evaluated, else 1 when any
/// violation is blocking, else 0 (an advisory violation is reported, never a
/// CI failure).
fn exit_code(r: &CheckReport) -> i32 {
    if !r.errors.is_empty() {
        2
    } else if r.violations.iter().any(|v| v.blocking) {
        1
    } else {
        0
    }
}

/// Appended to a non-blocking violation's tier: its tier is below its rule's
/// `min_tier`, or heuristic, which never blocks (0.5.2 Q2).
const ADVISORY: &str = ", advisory (below this rule's min_tier; does not fail CI)";

/// [`ADVISORY`] when `v` does not block, else nothing.
fn advisory(v: &Violation) -> &'static str {
    if v.blocking { "" } else { ADVISORY }
}

/// Violations printed per rule; `--json` lists all.
const MAX_PRINTED_PER_RULE: usize = 50;

/// `file:line`, `file`, or `—`.
fn located(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

/// An evidence row's site, as [`located`].
fn at(e: &ViolationEdge) -> String {
    located(e.file.as_deref(), e.line)
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

/// Caveat lines printed per absence (and per rule row); `--json` carries
/// them all.
const MAX_ABSENCE_CAVEATS: usize = 5;

/// At most [`MAX_ABSENCE_CAVEATS`] `  ⚠ <category> (<language>): <note>`
/// lines, then how many more `--json` lists.
fn print_caveats(caveats: &[CoverageNote]) {
    for c in caveats.iter().take(MAX_ABSENCE_CAVEATS) {
        println!("  ⚠ {} ({}): {}", c.edge_category, c.language, c.note);
    }
    if caveats.len() > MAX_ABSENCE_CAVEATS {
        println!(
            "  _(+{} more caveat(s); --json lists all)_",
            caveats.len() - MAX_ABSENCE_CAVEATS
        );
    }
}

/// A forbid_edge rule and a reflexion divergence report edges (`count` is the
/// edges, the evidence at most [`MAX_EVIDENCE`] of them); a no_cycle rule
/// reports cycles.
fn per_edge(rule_kind: &str) -> bool {
    rule_kind == FORBID_EDGE || rule_kind == DIVERGENCE
}

/// A rule row's subject: each head value in backticks, `, `-joined; `—` for
/// a row with no head value.
fn subject(v: &Violation) -> String {
    if v.subject.is_empty() {
        "—".to_string()
    } else {
        format!("`{}`", v.subject.join("`, `"))
    }
}

/// One violation `k` (1-based within its rule): its heading line, its
/// evidence table, and what the table leaves out.
fn print_violation(k: usize, v: &Violation) {
    let mark = advisory(v);
    match v.rule_kind {
        FORBID_EDGE => println!("_{} forbidden edge(s), tier {}{mark}:_", v.count, v.tier),
        DIVERGENCE => println!(
            "_{} edge(s) the model does not allow, tier {}{mark}:_",
            v.count, v.tier
        ),
        NO_CYCLE => println!(
            "_cycle {k}: {} node(s) in the component, tier {}{mark}; a shortest cycle through it:_",
            v.count, v.tier
        ),
        RULE => println!(
            "_violation {k}: {} - tier {}{mark} at {}_",
            subject(v),
            v.tier,
            located(v.file.as_deref(), v.line)
        ),
        FORBID_PATH => println!(
            "_path {k} into {} - {} hop(s), tier {}{mark}:_",
            subject(v),
            v.count,
            v.tier
        ),
        // `rule_kind` is a string, not an enum: a kind with no arm above
        // still prints its subject, count and tier, then its rows.
        other => println!(
            "_{other} {k}: {} - count {}, tier {}{mark}:_",
            subject(v),
            v.count,
            v.tier
        ),
    }
    println!();
    // A rule row no edge derives has no witness: its caveats say why "no
    // edge" may be a blind extraction (Q5).
    if v.rule_kind != RULE || !v.evidence.is_empty() {
        print_edges(&v.evidence);
    }
    let capped = per_edge(v.rule_kind) || v.rule_kind == RULE;
    if capped && v.count > v.evidence.len() {
        println!(
            "_(showing the first {} of {}; at most {MAX_EVIDENCE} are listed)_",
            v.evidence.len(),
            v.count
        );
        println!();
    }
    if !v.caveats.is_empty() {
        print_caveats(&v.caveats);
        println!();
    }
}

/// One section per violated rule: a forbid_edge rule's edges, a reflexion
/// divergence's edges, each cycle of a no_cycle rule with its witness, each
/// row of a rule invariant, or each hit of a transitive forbid_edge; at most
/// [`MAX_PRINTED_PER_RULE`] of them.
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
    for (i, v) in group.iter().take(MAX_PRINTED_PER_RULE).enumerate() {
        print_violation(i + 1, v);
    }
    if group.len() > MAX_PRINTED_PER_RULE {
        println!(
            "_(showing {MAX_PRINTED_PER_RULE} of {}; --json lists all)_",
            group.len()
        );
        println!();
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
        print_caveats(&a.caveats);
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
    let blocking = report.violations.iter().filter(|v| v.blocking).count();
    println!(
        "- violations: {} (blocking {blocking}, advisory {})",
        report.violations.len(),
        report.violations.len() - blocking
    );
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
        // A rule's parse or analysis error is one diagnostic per line, in
        // source order (DA.16): the first on the bullet, the rest under it.
        for (id, msg) in &report.errors {
            let mut lines = msg.lines();
            println!("- `{id}`: {}", lines.next().unwrap_or_default());
            for l in lines {
                println!("  {l}");
            }
        }
        println!();
    }
    code
}
