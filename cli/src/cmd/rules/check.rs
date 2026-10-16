//! `glia check` (LE.8) — the CI gate over the engine's
//! `check::check_with_roots`: every declared `[[constraint]]` rule evaluated,
//! one section per violated rule with its located evidence. Exit 0 when
//! nothing blocks, 1 on a BLOCKING violation, 2 on a build failure or a rule
//! that could not be evaluated (DB.3d), and with `--strict` also on a rule
//! that checks nothing (DT.7b). `--json` is the report. Each evidence row
//! carries the tier `glia why` gives that edge (CC.3), its note in
//! parentheses.
//!
//! The exit code reads only [`Violation::blocking`], which the engine sets
//! from the violation's tier and its rule's `min_tier` (DB.3a; a heuristic
//! violation never blocks, 0.5.2 Q2). A non-blocking violation is printed in
//! full with [`ADVISORY`] on its tier line, and the summary splits the count
//! (`- violations: <V> (blocking <B>, advisory <A>)`).
//!
//! Per kind: a forbid_edge rule's and a reflexion divergence's edges in one
//! table; each cycle of a no_cycle rule with a shortest witness; each
//! `violation` row of a rule invariant (DB.3a) with its subject, location
//! and witness edges; each hit of a transitive forbid_edge (DB.3b) with its
//! witness hops in path order. At most [`MAX_PRINTED_PER_RULE`] violations
//! print per rule.
//!
//! A rule's Q5 coverage caveats (the categories its negations read; a blind
//! extraction looks like a missing edge) print once, in one block under the
//! rule heading before its first row (DT.8a), deduplicated by (category,
//! language, note) in first-seen order: one rule's negations are static, so
//! every row carries the same list, and the per-row layout repeated it
//! verbatim (Kina, 8 rows: 24 `⚠` lines for 3 caveats). `--json` keeps each
//! violation's own list. fired_on, on stderr after the sections when a rule
//! printed a block: `[check] caveats rules=<r> lines=<l> folded=<f>`, `r` the
//! rules printing a caveat block, `l` the `⚠` lines those blocks printed, `f`
//! the lines the per-row layout would have printed beyond `l` over the
//! printed rows.
//!
//! A declared reflexion model (CC.5b) prints as a `## reflexion model` section
//! before the violations (CC.5c): its components, the component dependency
//! matrix, the absences with their coverage caveats and the unmapped files.
//! Its divergences are violations, one per-edge section each like a
//! forbid_edge rule's, gated at the default `min_tier`.
//!
//! Rules that check nothing (DT.7b): the report is `check_with_roots` over
//! the build's repo roots, so a declared `[[constraint]]` / `[[component]]` /
//! `[[layer]]` stanza the build anchored nowhere, rejected or does not hold,
//! and a rule whose scope no located node sits in, print under `## Rules
//! that check nothing`, each with its reason and a `fix:` line, after the
//! unchecked list. They never change the exit code, and an overlay of only
//! such stanzas is not "no rules declared". `--strict` makes each an errors
//! row (printed under `## rule errors (not evaluated)` with its `fix:` line,
//! not under the heading above), so it exits 2. The summary line gains `,
//! checking nothing <n>` (the non-blocking ones) only when there is one.

use std::collections::HashSet;
use std::path::PathBuf;

use glia_engine::CoverageNote;
use glia_engine::check::{
    CONVERGENCE, CheckOptions, CheckReport, DIVERGENCE, FORBID_EDGE, FORBID_PATH, MAX_EVIDENCE,
    MAX_UNMAPPED_SAMPLE, NO_CYCLE, RULE, Reflexion, Unanchored, Violation, ViolationEdge,
    check_with_roots,
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
    /// Fail (exit 2) on a declared rule that checks nothing: a stanza the
    /// build anchored nowhere, rejected or does not hold, or a rule whose
    /// scope no located node sits in. Without it they are listed, never a CI
    /// failure.
    #[arg(long)]
    strict: bool,
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

/// Caveat lines printed per absence (and per rule); `--json` carries them
/// all.
const MAX_ABSENCE_CAVEATS: usize = 5;

/// At most [`MAX_ABSENCE_CAVEATS`] `  ⚠ <category> (<language>): <note>`
/// lines, then how many more `--json` lists. Returns the `⚠` lines printed.
fn print_caveats(caveats: &[CoverageNote]) -> usize {
    for c in caveats.iter().take(MAX_ABSENCE_CAVEATS) {
        println!("  ⚠ {} ({}): {}", c.edge_category, c.language, c.note);
    }
    if caveats.len() > MAX_ABSENCE_CAVEATS {
        println!(
            "  _(+{} more caveat(s); --json lists all)_",
            caveats.len() - MAX_ABSENCE_CAVEATS
        );
    }
    caveats.len().min(MAX_ABSENCE_CAVEATS)
}

/// The line a rule's caveat block opens with (DT.8a).
const CAVEAT_BLOCK: &str = "_coverage caveats (the categories this rule's negations read; a blind extraction looks like a missing edge):_";

/// Every violation's caveats in `group`, deduplicated by (category,
/// language, note) in first-seen order (DT.8a).
fn rule_caveats(group: &[&Violation]) -> Vec<CoverageNote> {
    let mut seen: HashSet<(&str, &str, &str)> = HashSet::new();
    let mut out = Vec::new();
    for c in group.iter().flat_map(|v| &v.caveats) {
        if seen.insert((c.edge_category, c.language, c.note)) {
            out.push(c.clone());
        }
    }
    out
}

/// One rule section's `⚠` lines (DT.8a): `lines` its caveat block printed,
/// `per_row` the lines the per-violation layout printed over the same
/// printed rows.
#[derive(Default)]
struct CaveatFold {
    lines: usize,
    per_row: usize,
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
    // A rule row no edge derives has no witness: its rule's caveat block,
    // under the heading, says why "no edge" may be a blind extraction (Q5).
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
}

/// One section per violated rule: its caveat block (DT.8a), then a
/// forbid_edge rule's edges, a reflexion divergence's edges, each cycle of a
/// no_cycle rule with its witness, each row of a rule invariant, or each hit
/// of a transitive forbid_edge; at most [`MAX_PRINTED_PER_RULE`] of them.
fn print_rule(group: &[&Violation]) -> CaveatFold {
    let Some(first) = group.first() else {
        return CaveatFold::default();
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
    let caveats = rule_caveats(group);
    let mut fold = CaveatFold::default();
    if !caveats.is_empty() {
        println!("{CAVEAT_BLOCK}");
        println!();
        fold.lines = print_caveats(&caveats);
        println!();
    }
    for (i, v) in group.iter().take(MAX_PRINTED_PER_RULE).enumerate() {
        print_violation(i + 1, v);
        fold.per_row += v.caveats.len().min(MAX_ABSENCE_CAVEATS);
    }
    if group.len() > MAX_PRINTED_PER_RULE {
        println!(
            "_(showing {MAX_PRINTED_PER_RULE} of {}; --json lists all)_",
            group.len()
        );
        println!();
    }
    fold
}

/// The `## Rules that check nothing` section (DT.7b): every non-blocking
/// [`Unanchored`] row, its reason on the bullet and its fix under it; the
/// repo root too when the build merged several (`with`).
fn print_unanchored(rows: &[&Unanchored], with: bool) {
    println!("## Rules that check nothing");
    println!();
    println!(
        "_declared, but nothing is evaluated for them; not a CI failure (--strict makes each a rule error, exit 2)_"
    );
    println!();
    for u in rows {
        let decl = u.decl.as_deref().unwrap_or("cell API");
        let repo = match (&u.repo, with) {
            (Some(r), true) => format!(", {r}"),
            _ => String::new(),
        };
        println!(
            "- `{}` ({}, {decl}{repo}): {}",
            u.rule_id, u.section, u.reason
        );
        println!("  fix: {}", u.fix);
    }
    println!();
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
    let roots: Vec<(u64, PathBuf)> = result
        .repo_roots
        .iter()
        .map(|(r, p)| (*r, PathBuf::from(p)))
        .collect();
    let mut opts = CheckOptions::default();
    opts.strict = args.strict;
    let report = check_with_roots(&result.merged, &roots, &opts);
    let code = exit_code(&report);
    if args.json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
        return code;
    }
    println!("# glia check `{}`", args.repo);
    println!();
    // Under --strict every row is an errors row, printed there.
    let nothing: Vec<&Unanchored> = report.unanchored.iter().filter(|u| !u.blocking).collect();
    if report.rules == 0
        && report.reflexion.is_none()
        && report.errors.is_empty()
        && report.unanchored.is_empty()
    {
        println!("_(no rules declared - add [[constraint]] stanzas to .glia/overlay.toml)_");
        println!(
            "_(or model the architecture: [[component]] stanzas map paths to components; [[layer]] and kind = \"allow\" judge their dependencies)_"
        );
        return code;
    }
    let checking_nothing = if nothing.is_empty() {
        String::new()
    } else {
        format!(", checking nothing {}", nothing.len())
    };
    println!(
        "- rules: {} (checked {}, unchecked {}, errors {}{checking_nothing})",
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
    let (mut caveat_rules, mut caveat_lines, mut folded) = (0usize, 0usize, 0usize);
    while i < report.violations.len() {
        let id = &report.violations[i].rule_id;
        let group: Vec<&Violation> = report.violations[i..]
            .iter()
            .take_while(|v| &v.rule_id == id)
            .collect();
        i += group.len();
        let fold = print_rule(&group);
        if fold.lines > 0 {
            caveat_rules += 1;
            caveat_lines += fold.lines;
            folded += fold.per_row.saturating_sub(fold.lines);
        }
    }
    if !report.unchecked.is_empty() {
        println!("## unchecked (not machine-checkable)");
        println!();
        for id in &report.unchecked {
            println!("- `{id}`");
        }
        println!();
    }
    if !nothing.is_empty() {
        print_unanchored(&nothing, !args.with.is_empty());
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
    if caveat_rules > 0 {
        eprintln!("[check] caveats rules={caveat_rules} lines={caveat_lines} folded={folded}");
    }
    code
}
