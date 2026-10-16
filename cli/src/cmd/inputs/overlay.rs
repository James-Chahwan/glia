//! `glia overlay propose|try|accept` (CE.3e) — the CLI of the overlay loop,
//! `glia_engine::overlay_loop` (CE.3b-d). The loop that writes
//! `.glia/overlay.toml` from the gaps report is one command here, not each
//! consumer's own script:
//!
//! - `propose` lists the gaps an overlay stanza could close (the `glia gaps`
//!   rows, ids included), each with the source lines around it and, for a
//!   `suspected_edge` row, its paste-ready `[[edge]]` draft (CD.3b), plus the
//!   docs/overlay.md guide per `suggest` value. Writes nothing.
//! - `try` builds the tree with the overlay as it is, with a candidate file's
//!   stanzas merged in and (unless `--no-leave-one-out`) once more per stanza
//!   without it, and reports every category that moved and each stanza's
//!   marginal effect with a keep / review / drop verdict. Writes only the
//!   parse cache.
//! - `accept` is the only writer of `.glia/overlay.toml`: a candidate's
//!   chosen stanzas in (`--only <ref>`, default all), orphaned / redundant /
//!   rotted rules out by gap id (`--remove`), validated by the loader,
//!   written atomically; `--dry-run` prints the diff and writes nothing.
//!
//! A candidate may carry `[[constraint]]` rules (DB.9a; handles
//! `constraint#<n>`): a `forbid_edge`, a `no_cycle`, or an `invariant` with
//! `rule`. `allow` and a prose invariant are refused (a person writes them
//! by hand). `accept` refuses a rule `glia check` could not evaluate and a
//! rule whose id the file already declares with another body; `try` judges
//! every rule it is given.
//!
//! `try` renders the rule part of the report (DB.9c) when it has one: a
//! `rules` column in the stanza table and a `rules (...)` block under it
//! (each `[[constraint]]` stanza's status with its located violations, each
//! graph stanza's moved violations), a `rules:` totals line, and the verdict
//! line split into its graph and rule parts. A rule-free try prints exactly
//! the pre-0.5.2 text.
//!
//! Transport + rendering only: the builds, verdicts, validation and the
//! `[overlay] propose|try|accept ... surface=cli` markers live in the engine.
//! This module prints the `[overlay] candidate file=<path> ...` marker
//! itself (`report_candidate`), since only it knows the file.
//!
//! Exit codes: `propose` / `try` are reports, 0 whatever they find and 2 on a
//! build or candidate error. `accept` exits 0 when written (or on a dry run,
//! or when nothing changed), 1 when validation refuses the result (a
//! candidate the loader would drop, a rule `glia check` could not evaluate,
//! a new file that would not load as written, a constant already pinned to
//! another value, a rule id declared with another body: nothing written),
//! and 2 on a usage or build error (neither `--candidate` nor `--remove`, an
//! unknown stanza ref or gap id, an unreadable file). The global
//! `--no-overlay` is refused (exit 2): every step works on the overlay.

use std::collections::BTreeMap;
use std::path::Path;

use clap::Subcommand;
use glia_engine::check::{DERIVED, FACT, HEURISTIC};
use glia_engine::gaps::{CATEGORIES, DROP, GapRow, KEEP, REVIEW, verdict};
use glia_engine::overlay_loop::{
    AcceptOptions, AcceptSummary, DEFAULT_SNIPPET_LINES, DEFAULT_TOP_K, GraphDelta, MAX_RULE_HITS,
    Proposal, ProposeOptions, ProposedGap, RULE_DUPLICATE, RULE_ERROR, RULE_HOLDS, RULE_ORPHANED,
    RULE_VIOLATED, RuleHit, RuleTrial, RulesDelta, Snippet, StanzaTrial, TryOptions, TryReport,
    accept, propose, report_candidate, try_candidate,
};

use crate::common::build_options;

/// The engine's accept refusals that mean "the result would not validate"
/// (exit 1), after its `overlay accept: ` prefix: the candidate refused by
/// the loader or holding a rule `glia check` could not evaluate
/// (`candidate: ...`, `candidate:<line>: ...`), the merged or new file not
/// loading as written (a version raise that would load hidden stanzas
/// included), a constant the file pins to another value, a `[[constraint]]`
/// id the file declares with another body (DB.9a). Every other refusal is a
/// usage, read, build or write error (exit 2).
const VALIDATION_REFUSALS: [&str; 5] = [
    "candidate",
    "the merged ",
    "the new ",
    "constant ",
    "constraint ",
];

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: OverlayCmd,
}

#[derive(Subcommand, Debug)]
enum OverlayCmd {
    /// List the gaps an overlay stanza could close: per category the rows
    /// (id, qname, location, tier, suggested section), the source lines
    /// around each and a suspected edge's paste-ready `[[edge]]`. Builds the
    /// tree with the overlay as it is; writes nothing. Exits 0, 2 on a build
    /// error.
    Propose {
        /// Path to the repo root (its `.glia/overlay.toml` is the one the
        /// loop edits).
        repo: String,
        /// Additional repos to merge in (cross-service). Repeatable.
        #[arg(long)]
        with: Vec<String>,
        /// Only this category's rows. Repeatable; default: every category an
        /// overlay can repair (all but `wrapped_sink`).
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(CATEGORIES))]
        category: Vec<String>,
        /// Rows kept per category, after ranking (counts stay the totals).
        #[arg(long, default_value_t = DEFAULT_TOP_K)]
        top_k: usize,
        /// Lines of source shown either side of a row's line.
        #[arg(long, default_value_t = DEFAULT_SNIPPET_LINES)]
        snippet_lines: usize,
        /// Emit JSON (`{rows, counts, snippets, ambiguous_root, guide}`; a row
        /// is the `glia gaps` row plus `snippet`) instead of tables.
        #[arg(long)]
        json: bool,
    },
    /// Try a candidate stanza file against the repo: build with the overlay
    /// as it is (base), with every stanza merged in (with) and, per graph
    /// stanza, with all but that one, then report what moved and a keep /
    /// review / drop verdict per stanza. Every declared rule is checked on
    /// each build: a [[constraint]] stanza is judged on with (holds /
    /// violated / error / orphaned / duplicate, its violations located), a
    /// graph stanza that moves a rule's violations is review, and the verdict
    /// combines the graph and rule parts. Writes only the parse cache. Exits
    /// 0, 2 on a build or candidate error.
    Try {
        /// Path to the repo root whose `.glia/overlay.toml` the candidate
        /// would join.
        repo: String,
        /// The candidate: overlay sections, entrypoints and [[constraint]]
        /// rules only, each stanza under `# gap: <id>` comments naming the
        /// gaps it targets.
        #[arg(long)]
        candidate: String,
        /// Additional repos to merge in (cross-service). Repeatable.
        #[arg(long)]
        with: Vec<String>,
        /// Build only base and with (two builds): the totals and the verdict,
        /// no per-stanza attribution.
        #[arg(long)]
        no_leave_one_out: bool,
        /// Emit JSON (`{stanzas, base, with, delta, verdict, closed, builds}`,
        /// + `rules` when a rule was checked) instead of tables.
        #[arg(long)]
        json: bool,
    },
    /// Write into `<repo>/.glia/overlay.toml`: a candidate's chosen stanzas
    /// added and orphaned / redundant rules removed by gap id, validated by
    /// the loader, written atomically. The only writer of that file. Exits 0
    /// written (or dry run), 1 when validation refuses (nothing written), 2
    /// on a usage or build error.
    Accept {
        /// Path to the repo root.
        repo: String,
        /// The candidate stanza file to take stanzas from.
        #[arg(long)]
        candidate: Option<String>,
        /// Add only this candidate stanza (`wrapper#1`, `edge#2`,
        /// `constants.GATEWAY`, `entrypoints#1`, `constraint#1`). Repeatable;
        /// default: every stanza of the candidate.
        #[arg(long, requires = "candidate")]
        only: Vec<String>,
        /// Remove the stanza of this `orphaned_rule` / `redundant_rule` /
        /// `rotted_rule` gap (`gap:<16 hex>`, from `propose` or `glia gaps`).
        /// Repeatable.
        #[arg(long)]
        remove: Vec<String>,
        /// Validate and print the diff; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Emit JSON (`{added, removed, duplicates, file, dry_run, written,
        /// diff}`) instead of the diff and summary.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(args: Args) -> i32 {
    if !build_options().overlay {
        eprintln!(
            "error: `glia overlay` works on the overlay; drop --no-overlay (propose and try build with `.glia/overlay.toml` as it is)"
        );
        return 2;
    }
    match args.action {
        OverlayCmd::Propose {
            repo,
            with,
            category,
            top_k,
            snippet_lines,
            json,
        } => {
            let mut opts = ProposeOptions::default();
            opts.categories = category;
            opts.top_k = top_k;
            opts.snippet_lines = snippet_lines;
            opts.surface = "cli";
            run_propose(&repo_paths(&repo, with), &opts, json)
        }
        OverlayCmd::Try {
            repo,
            candidate,
            with,
            no_leave_one_out,
            json,
        } => {
            let mut opts = TryOptions::default();
            opts.leave_one_out = !no_leave_one_out;
            opts.surface = "cli";
            run_try(&repo_paths(&repo, with), &candidate, &opts, json)
        }
        OverlayCmd::Accept {
            repo,
            candidate,
            only,
            remove,
            dry_run,
            json,
        } => {
            if candidate.is_none() && remove.is_empty() {
                eprintln!(
                    "error: nothing to accept: give --candidate <FILE>, --remove <GAP_ID>, or both"
                );
                return 2;
            }
            let mut opts = AcceptOptions::default();
            opts.only = only;
            opts.remove = remove;
            opts.dry_run = dry_run;
            opts.surface = "cli";
            run_accept(&repo, candidate.as_deref(), &opts, json)
        }
    }
}

/// The primary repo, then the `--with` repos.
fn repo_paths(repo: &str, with: Vec<String>) -> Vec<String> {
    let mut paths = vec![repo.to_string()];
    paths.extend(with);
    paths
}

/// The candidate file's text; `Err` names the file.
fn read_candidate(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read candidate {path}: {e}"))
}

/// A table cell: pipes escaped so a qname cannot split the row.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// `file:line`, `file`, or `—`.
fn at(r: &GapRow) -> String {
    match (r.file.as_deref(), r.line) {
        (Some(f), Some(n)) => format!("{f}:{n}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

// ---------------------------------------------------------------------------
// propose
// ---------------------------------------------------------------------------

fn run_propose(paths: &[String], opts: &ProposeOptions, json: bool) -> i32 {
    let proposal = match propose(paths, opts) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if json {
        println!("{}", serde_json::to_string(&proposal).unwrap_or_default());
        return 0;
    }
    let repo = paths.first().map_or("", String::as_str);
    let with: String = paths
        .iter()
        .skip(1)
        .map(|w| format!(" --with {w}"))
        .collect();
    println!("# glia overlay propose `{repo}`");
    print_proposal(&proposal);
    println!();
    println!("## next");
    println!();
    println!("{}", proposal.guide);
    println!();
    println!("glia overlay try {repo} --candidate <FILE>{with}");
    println!("glia overlay accept {repo} --candidate <FILE> [--only <STANZA>]...");
    0
}

fn print_proposal(p: &Proposal) {
    for category in CATEGORIES {
        let rows: Vec<&ProposedGap> = p
            .rows
            .iter()
            .filter(|r| r.gap.category == category)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let total = p.counts.get(category).copied().unwrap_or(rows.len());
        println!();
        if rows.len() < total {
            println!("## {category} — {total} (top {})", rows.len());
        } else {
            println!("## {category} — {total}");
        }
        println!();
        println!("| id | qname | at | tier | suggest |");
        println!("|---|---|---|---|---|");
        for r in &rows {
            let g = &r.gap;
            println!(
                "| {} | `{}` | {} | {} | {} |",
                g.id,
                cell(&g.qname),
                cell(&at(g)),
                g.tier,
                cell(g.suggest)
            );
        }
        for r in &rows {
            print_row_source(r);
        }
    }
    println!();
    if p.rows.is_empty() {
        println!("no gap an overlay could close");
    }
    let totals: Vec<String> = p.counts.iter().map(|(c, n)| format!("{c}={n}")).collect();
    println!(
        "totals: {} | snippets={} ambiguous_root={}",
        totals.join(" "),
        p.snippets,
        p.ambiguous_root
    );
}

/// One row's snippet (a fenced block, the row's line marked `>`) and its
/// paste-ready stanza (a ```toml block), when it has them.
fn print_row_source(r: &ProposedGap) {
    if r.snippet.is_none() && r.gap.draft.is_none() {
        return;
    }
    println!();
    println!("{} `{}` at {}", r.gap.id, cell(&r.gap.qname), at(&r.gap));
    if let Some(s) = &r.snippet {
        print_snippet(s, r.gap.line);
    }
    if let Some(draft) = &r.gap.draft {
        println!();
        println!("```toml");
        println!("{}", draft.trim_end_matches('\n'));
        println!("```");
    }
}

fn print_snippet(s: &Snippet, line: Option<i64>) {
    let lang = Path::new(&s.file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let last = s.start_line + s.lines.len().saturating_sub(1);
    let width = last.to_string().len();
    println!();
    println!("```{lang}");
    for (i, text) in s.lines.iter().enumerate() {
        let n = s.start_line + i;
        let mark = if line.and_then(|l| usize::try_from(l).ok()) == Some(n) {
            '>'
        } else {
            ' '
        };
        println!("{mark}{n:>width$} | {text}");
    }
    println!("```");
}

// ---------------------------------------------------------------------------
// try
// ---------------------------------------------------------------------------

fn run_try(paths: &[String], candidate: &str, opts: &TryOptions, json: bool) -> i32 {
    let text = match read_candidate(candidate) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if let Err(e) = report_candidate(Some(candidate), &text) {
        eprintln!("error: candidate {candidate} refused: {e}");
        return 2;
    }
    let report = match try_candidate(paths, &text, opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
        return 0;
    }
    let repo = paths.first().map_or("", String::as_str);
    println!("# glia overlay try `{repo}` --candidate {candidate}");
    println!();
    print_totals(&report);
    println!();
    print_stanzas(&report, opts.leave_one_out);
    print_rule_block(&report);
    print_try_next(repo, candidate, &report);
    0
}

/// Hit lines a stanza's rule block shows; `--json` lists up to
/// [`MAX_RULE_HITS`].
const SHOWN_HITS: usize = 5;

/// Why a verdict is what it is (`gaps::verdict`).
fn verdict_reason(v: &str) -> &'static str {
    match v {
        KEEP => "a gap category fell or the graph grew, none rose",
        REVIEW => "the graph improved, but a gap category rose too",
        DROP => "no gap category fell and the graph did not grow",
        _ => "unknown verdict",
    }
}

/// `before | after | delta` per changed node kind, edge category and gap
/// category, under the total gaps line.
fn print_totals(r: &TryReport) {
    let row = |label: &str, before: usize, after: usize| {
        let d = i128::try_from(after).unwrap_or(0) - i128::try_from(before).unwrap_or(0);
        println!("| {label} | {before} | {after} | {d:+} |");
    };
    println!("| | base | with | delta |");
    println!("|---|---|---|---|");
    row("gaps (total)", r.base.total_gaps(), r.with.total_gaps());
    let n = |m: &BTreeMap<&'static str, usize>, k: &str| m.get(k).copied().unwrap_or(0);
    for k in r.delta.nodes.keys() {
        let (b, w) = (&r.base.nodes_by_kind, &r.with.nodes_by_kind);
        row(&format!("node {k}"), n(b, k), n(w, k));
    }
    for k in r.delta.edges.keys() {
        let (b, w) = (&r.base.edges_by_category, &r.with.edges_by_category);
        row(&format!("edge {k}"), n(b, k), n(w, k));
    }
    for k in r.delta.gaps.keys() {
        let (b, w) = (&r.base.gaps_by_category, &r.with.gaps_by_category);
        row(&format!("gap {k}"), n(b, k), n(w, k));
    }
    if r.delta.is_empty() {
        println!();
        println!("changed: nothing (no node kind, edge category or gap category moved)");
    }
    println!();
    match &r.rules {
        // The rule part (DB.9b): the graph part's reason is a rule move when
        // the counts alone keep.
        Some(rules) => {
            let g = rules.graph_verdict;
            let why = if g == REVIEW && verdict(&r.base, &r.with) == KEEP {
                "a declared rule's violations moved"
            } else {
                verdict_reason(g)
            };
            let rv = rules
                .rule_verdict
                .unwrap_or("none (no [[constraint]] stanza)");
            println!(
                "verdict: {} - graph part {g} ({why}), rule part {rv}",
                r.verdict
            );
        }
        None => println!("verdict: {} - {}", r.verdict, verdict_reason(r.verdict)),
    }
    if r.closed.is_empty() {
        println!("closed: none of the linked gaps");
    } else {
        println!("closed: {}", r.closed.join(", "));
    }
    println!("builds: {}", r.builds);
    if let Some(rules) = &r.rules {
        // The only line naming the rule statuses under --no-leave-one-out.
        let statuses: Vec<String> = [
            RULE_HOLDS,
            RULE_VIOLATED,
            RULE_ERROR,
            RULE_ORPHANED,
            RULE_DUPLICATE,
        ]
        .iter()
        .filter_map(|s| {
            rules
                .statuses
                .get(s)
                .filter(|n| **n > 0)
                .map(|n| format!("{s} {n}"))
        })
        .collect();
        let d = &rules.delta;
        let mut line = String::from("rules: ");
        if !statuses.is_empty() {
            line.push_str(&statuses.join(", "));
            line.push_str("; ");
        }
        line.push_str(&format!(
            "violations +{} -{} ({} blocking)",
            d.added_total, d.resolved_total, d.blocking
        ));
        if !d.errors.is_empty() {
            let ids: Vec<&str> = d.errors.iter().map(|(id, _)| id.as_str()).collect();
            line.push_str(&format!("; errors: {}", ids.join(", ")));
        }
        println!("{line}");
    }
}

/// `KIND +n, KIND -m` of one delta map, `—` when empty.
fn moves(m: &BTreeMap<&'static str, i64>) -> String {
    if m.is_empty() {
        return "—".to_string();
    }
    m.iter()
        .map(|(k, d)| format!("{k} {d:+}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_stanzas(r: &TryReport, leave_one_out: bool) {
    if !leave_one_out {
        println!("per-stanza attribution skipped (--no-leave-one-out)");
        return;
    }
    if r.stanzas.is_empty() {
        println!("no stanza in the candidate");
        return;
    }
    let ruled = has_rule_part(r);
    if ruled {
        println!("| stanza | verdict | closes | nodes | edges | gaps | rules |");
        println!("|---|---|---|---|---|---|---|");
    } else {
        println!("| stanza | verdict | closes | nodes | edges | gaps |");
        println!("|---|---|---|---|---|---|");
    }
    for s in &r.stanzas {
        let closes = if s.closes.is_empty() {
            "—".to_string()
        } else {
            s.closes.join(", ")
        };
        let GraphDelta {
            nodes, edges, gaps, ..
        } = &s.marginal;
        let rules = if ruled {
            format!(" {} |", cell(&rule_cell(s)))
        } else {
            String::new()
        };
        println!(
            "| {} | {} | {} | {} | {} | {} |{rules}",
            s.stanza,
            s.verdict,
            closes,
            moves(nodes),
            moves(edges),
            moves(gaps)
        );
    }
}

/// Some stanza has a rule part: a `[[constraint]]` stanza's trial, or a
/// graph stanza's moved violations.
fn has_rule_part(r: &TryReport) -> bool {
    r.stanzas
        .iter()
        .any(|s| s.rule.is_some() || !s.rules.is_empty())
}

/// `fact <n>, derived <n>, heuristic <n>`, zero tiers left out.
fn tiers(t: &BTreeMap<&'static str, usize>) -> String {
    let known = [FACT, DERIVED, HEURISTIC];
    let rest = t.keys().copied().filter(|k| !known.contains(k));
    known
        .into_iter()
        .chain(rest)
        .filter_map(|k| t.get(k).filter(|n| **n > 0).map(|n| format!("{k} {n}")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A stanza's `rules` cell: its rule's status (a `[[constraint]]` stanza),
/// the violations it moves (a graph stanza), or `—`.
fn rule_cell(s: &StanzaTrial) -> String {
    if let Some(t) = &s.rule {
        return match t.status {
            RULE_HOLDS if t.empty_scopes.is_empty() => RULE_HOLDS.to_string(),
            RULE_HOLDS => format!("holds, empty scope {}", t.empty_scopes.join(", ")),
            RULE_VIOLATED => format!(
                "{} violation(s) ({}; {} blocking)",
                t.violations,
                tiers(&t.tiers),
                t.blocking
            ),
            other => other.to_string(),
        };
    }
    if s.rules.is_empty() {
        return "—".to_string();
    }
    let d = &s.rules;
    let mut out = format!(
        "+{} -{} ({} blocking)",
        d.added_total, d.resolved_total, d.blocking
    );
    if !d.errors.is_empty() {
        out.push_str(&format!(", {} error(s)", d.errors.len()));
    }
    out
}

/// `scope `a` holds no node` / `scopes `a`, `b` hold no node`.
fn empty_scope_phrase(scopes: &[String]) -> String {
    let named: Vec<String> = scopes.iter().map(|s| format!("`{s}`")).collect();
    if named.len() == 1 {
        format!("scope {} holds no node", named.join(", "))
    } else {
        format!("scopes {} hold no node", named.join(", "))
    }
}

/// Why a `[[constraint]]` stanza's rule trial reads as it does.
fn rule_reason(t: &RuleTrial) -> String {
    match t.status {
        RULE_HOLDS if t.empty_scopes.is_empty() => "holds on the code as it is".to_string(),
        RULE_HOLDS => format!(
            "holds, but {}: it may pass vacuously",
            empty_scope_phrase(&t.empty_scopes)
        ),
        RULE_VIOLATED => {
            let blocking = if t.blocking == 0 {
                "none blocking".to_string()
            } else {
                format!("{} blocking: `glia check` exits 1 on them", t.blocking)
            };
            let mut out = format!(
                "flags {} violation(s) in the code as it is ({blocking})",
                t.violations
            );
            if !t.empty_scopes.is_empty() {
                out.push_str(&format!(", and {}", empty_scope_phrase(&t.empty_scopes)));
            }
            out
        }
        RULE_ERROR => format!(
            "does not evaluate, `glia check` would exit 2: {}",
            flat(t.error.as_deref().unwrap_or("no message"))
        ),
        RULE_ORPHANED => "binds no node (no `anchor` and no PROJECT at its scope, or an `anchor` that names no node): `glia check` never reads it".to_string(),
        RULE_DUPLICATE => "already declared: the overlay as it is holds a rule of this id".to_string(),
        other => other.to_string(),
    }
}

/// A message on one line: its line breaks as `; `.
fn flat(message: &str) -> String {
    message
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// One violation: `<rule_kind> [`<rule_id>`] `<key>` - <tier>[, advisory]
/// at <file:line | file | —>`; the id is left out under its own rule.
fn hit_line(h: &RuleHit, with_id: bool) -> String {
    let at = match (h.file.as_deref(), h.line) {
        (Some(f), Some(n)) => format!("{f}:{n}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    };
    let id = if with_id {
        format!(" `{}`", h.rule_id)
    } else {
        String::new()
    };
    let advisory = if h.blocking { "" } else { ", advisory" };
    format!(
        "{}{id} `{}` - {}{advisory} at {at}",
        h.rule_kind, h.key, h.tier
    )
}

/// Up to [`SHOWN_HITS`] signed hit lines, then how many more there are.
fn print_hits(hits: &[(char, &RuleHit)], total: usize, with_id: bool) {
    for (sign, h) in hits.iter().take(SHOWN_HITS) {
        println!("    {sign} {}", hit_line(h, with_id));
    }
    let shown = hits.len().min(SHOWN_HITS);
    if total > shown {
        println!(
            "    ... {} more (--json lists up to {MAX_RULE_HITS})",
            total - shown
        );
    }
}

/// What a graph stanza does to the declared rules (`WITHOUT(it) -> WITH`).
fn moved_reason(d: &RulesDelta) -> String {
    let moved = d.added_total + d.resolved_total > 0;
    match (moved, d.errors.len()) {
        (true, 0) => "moves declared-rule violations".to_string(),
        (true, n) => format!("moves declared-rule violations, and {n} rule(s) stop evaluating"),
        (false, n) => format!("{n} rule(s) stop evaluating"),
    }
}

/// The `rules (...)` block under the stanza table: per stanza with a rule
/// part, in stanza order, its verdict and why, then its located hits (`+`
/// added on WITH, `-` resolved).
fn print_rule_block(r: &TryReport) {
    if !has_rule_part(r) {
        return;
    }
    println!();
    println!("rules (glia check on the build with every stanza):");
    for s in &r.stanzas {
        if let Some(t) = &s.rule {
            println!(
                "- {} `{}`: {} - {}",
                s.stanza,
                t.rule_id,
                s.verdict,
                rule_reason(t)
            );
            let hits: Vec<(char, &RuleHit)> = t.hits.iter().map(|h| ('+', h)).collect();
            print_hits(&hits, t.violations, false);
        } else if !s.rules.is_empty() {
            let d = &s.rules;
            println!("- {}: {} - {}", s.stanza, s.verdict, moved_reason(d));
            let hits: Vec<(char, &RuleHit)> = d
                .added
                .iter()
                .map(|h| ('+', h))
                .chain(d.resolved.iter().map(|h| ('-', h)))
                .collect();
            print_hits(&hits, d.added_total + d.resolved_total, true);
            for (id, message) in &d.errors {
                println!("    ! `{id}` does not evaluate: {}", flat(message));
            }
        }
    }
}

/// The accept line for the kept stanzas, and the ones to look at first.
fn print_try_next(repo: &str, candidate: &str, r: &TryReport) {
    let named = |v: &str| -> Vec<&str> {
        r.stanzas
            .iter()
            .filter(|s| s.verdict == v)
            .map(|s| s.stanza.as_str())
            .collect()
    };
    let keep = named(KEEP);
    let review = named(REVIEW);
    println!();
    if !keep.is_empty() {
        let only: Vec<String> = keep.iter().map(|s| format!("--only {s}")).collect();
        println!(
            "accept the kept stanzas: glia overlay accept {repo} --candidate {candidate} {}",
            only.join(" ")
        );
    }
    if !review.is_empty() {
        println!("review before accepting: {}", review.join(", "));
    }
    if keep.is_empty() && review.is_empty() && !r.stanzas.is_empty() {
        println!("nothing to accept: every stanza is a drop");
    }
}

// ---------------------------------------------------------------------------
// accept
// ---------------------------------------------------------------------------

/// Exit 1 for a refusal [`VALIDATION_REFUSALS`] names, 2 otherwise.
fn refusal_exit(message: &str) -> i32 {
    let rest = message.strip_prefix("overlay accept: ").unwrap_or(message);
    if VALIDATION_REFUSALS.iter().any(|p| rest.starts_with(p)) {
        1
    } else {
        2
    }
}

fn run_accept(repo: &str, candidate: Option<&str>, opts: &AcceptOptions, json: bool) -> i32 {
    let text = match candidate.map(read_candidate).transpose() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if let (Some(file), Some(t)) = (candidate, text.as_deref())
        && let Err(e) = report_candidate(Some(file), t)
    {
        eprintln!("error: candidate {file} refused, nothing written: {e}");
        return 1;
    }
    let summary = match accept(repo, text.as_deref(), opts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return refusal_exit(&e);
        }
    };
    if json {
        println!("{}", serde_json::to_string(&summary).unwrap_or_default());
        return 0;
    }
    println!("# glia overlay accept `{repo}`");
    print_accept(&summary);
    0
}

fn print_accept(s: &AcceptSummary) {
    println!();
    if s.diff.is_empty() {
        println!("no change: the file already holds every chosen stanza");
    } else {
        println!("```diff");
        println!("{}", s.diff.trim_end_matches('\n'));
        println!("```");
    }
    println!();
    let added: usize = s.added.values().sum();
    if s.added.is_empty() {
        println!("added: 0");
    } else {
        let per: Vec<String> = s.added.iter().map(|(k, n)| format!("{k}={n}")).collect();
        println!("added: {added} ({})", per.join(" "));
    }
    println!("removed: {}", s.removed);
    println!("duplicates: {}", s.duplicates);
    if s.written {
        println!("wrote {}", s.file);
    } else if s.dry_run {
        println!("dry run: {} not written", s.file);
    } else {
        println!("unchanged: {} not written", s.file);
    }
}
