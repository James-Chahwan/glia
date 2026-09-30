//! `glia patterns` (LE.7b, promoted by CC.12b) — the human surface over the engine's
//! `patterns::{pattern_conformance, pattern_conformance_delta}` (LE.7a): route
//! handlers grouped per service, each handler's role chain to its first effect
//! sink as a signature, a population's most frequent signature as its
//! convention, and every handler off it a located DIVERGENCE (tier heuristic:
//! an observed convention, never a rule).
//!
//! The command runs without a flag. 0.5.0 refused without `--experimental`
//! until the engine met its promotion criterion (the engine module docs,
//! "Promotion criterion"); CC.12b promoted it, so `--experimental` is hidden,
//! still accepted until 0.5.2 and prints
//! `[patterns] note: --experimental is no longer needed (accepted until 0.5.2)`
//! on stderr before anything else, the answer unchanged.
//!
//! Two modes. Whole graph (the default): a fresh build of the repo (`--with`
//! merges more repos in) and every exception listed. Delta (`--base <rev>`):
//! the working tree against the rev through the graph delta (LE.1b), the
//! conventions taken from the whole working-tree graph, and only the
//! divergences the change touched listed; it builds the one repo, so `--with`
//! with `--base` is a usage error, and so is `--no-overlay` (both sides are
//! built with the repo's overlay).
//!
//! `--group-by service` (the default) keys a population by the service
//! `glia arch` shows; `--group-by package` by (service, the handler file's
//! directory), which splits a one-directory Go service into its packages
//! (CA.5b). A BLIND handler, one the graph follows no chain from
//! (`handler>(no effect)`), counts toward a population's size but not its
//! share: the verdict is over the sighted handlers, and a blind handler is
//! listed, never a divergence.
//!
//! Table mode prints the counts (with the handlers left out of every
//! population, by reason), then one section per population, headed
//! `## <service>[ / <package>]`, carrying its status: `judged`
//! (`- <matching>/<sighted> sighted handlers follow <convention>` in the
//! heading, a `| handler | route | signature | location |` row per
//! divergence, then `- blind (the graph follows no chain): <n>` and a
//! `| handler | route | location |` row per blind handler), `no_convention`
//! (the top signature and its share over the sighted handlers, then the
//! blind rows), `blind` (`- blind: <sighted> of <size> handlers reach a sink,
//! fewer than <min_support>` in the heading, then the blind rows),
//! `too_small` (one `_(population below min support: ...)_` line). A repo
//! with no placed handler says so instead of printing an empty report.
//! `--json` prints the engine's `PatternReport` (`{delta_mode, handlers,
//! judged, skipped_small, excluded, role_sources, populations, divergences,
//! blind}`). Every `file:line` is 1-based (LD.1).
//!
//! Exit 0 on an answer, 2 on a usage error or a git / build failure with the
//! engine's message. Delta mode saves the working tree's parse-cache sidecar
//! (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored) as an incremental
//! build does, under `GLIA_NO_PERSIST=1` too; never a `.gmap` layout.
//!
//! Fired-on marker: this surface's `[patterns] surface=cli mode=<graph|delta>`
//! once the answer is in, beside the engine's `[patterns] populations=..` line
//! (and `[patterns] delta touched_nodes=..` in delta mode). 0.5.0 spelled both
//! `[patterns] experimental ..`.

use glia_engine::delta::graph_delta_vs_rev;
use glia_engine::patterns::{
    DEFAULT_MIN_SHARE_PCT, DEFAULT_MIN_SUPPORT, Divergence, GroupBy, PatternArgs,
    PatternReport, Population, pattern_conformance, pattern_conformance_delta,
};

use crate::common::{build_options, generate_for};

/// What `--experimental` prints since the promotion (CC.12b), verbatim.
const EXPERIMENTAL_NOTE: &str =
    "[patterns] note: --experimental is no longer needed (accepted until 0.5.2)";

/// `--group-by`: what keys a population (the engine's `GroupBy`).
#[derive(Copy, Clone, Debug, clap::ValueEnum)]
pub(crate) enum GroupByArg {
    /// The service `glia arch` shows.
    Service,
    /// The service and the handler file's repo-relative directory (a Go
    /// package).
    Package,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root (a git work tree with `--base`).
    repo: String,
    /// Accepted until 0.5.2 and ignored but for a note on stderr: the
    /// command left experimental in 0.5.1 (CC.12b).
    #[arg(long, hide = true)]
    experimental: bool,
    /// Delta mode: judge the working tree's graph, listing only the
    /// divergences the change against this git rev (a branch, tag, sha or
    /// `HEAD~N`) touched.
    #[arg(long, value_name = "REV")]
    base: Option<String>,
    /// Smallest population (handlers of one service) that gets a verdict.
    #[arg(long, default_value_t = DEFAULT_MIN_SUPPORT)]
    min_support: usize,
    /// Share of a population, in percent (0-100), the most frequent signature
    /// needs to be declared its convention.
    #[arg(long, default_value_t = DEFAULT_MIN_SHARE_PCT)]
    min_share: usize,
    /// Keep only handlers located under this repo-relative path or project
    /// label.
    #[arg(long)]
    scope: Option<String>,
    /// What keys a population: its service, or its service and the handler
    /// file's directory (a Go package).
    #[arg(long, value_enum, default_value_t = GroupByArg::Service)]
    group_by: GroupByArg,
    /// Additional repos to merge in (cross-service). Repeatable; whole-graph
    /// mode only.
    #[arg(long)]
    with: Vec<String>,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

/// The engine's options from the flags, or the usage error.
fn pattern_args(args: &Args) -> Result<PatternArgs, String> {
    if args.min_share > 100 {
        return Err(format!("--min-share is a percentage (0-100), got {}", args.min_share));
    }
    let mut p = PatternArgs::default();
    p.min_support = args.min_support;
    p.min_share_pct = args.min_share;
    p.scope = args.scope.clone();
    p.group_by = match args.group_by {
        GroupByArg::Service => GroupBy::Service,
        GroupByArg::Package => GroupBy::Package,
    };
    Ok(p)
}

/// The report for whichever mode `args` names, and the mode's marker word.
fn answer(args: &Args, p: &PatternArgs) -> Result<(PatternReport, &'static str), String> {
    let Some(base) = args.base.as_deref() else {
        let built = generate_for(&args.repo, &args.with)?;
        return Ok((pattern_conformance(&built.merged, &built.repo_labels, p), "graph"));
    };
    if !args.with.is_empty() {
        return Err("--base builds the one repo against its git rev; --with needs whole-graph mode (no --base)".to_string());
    }
    if !build_options().overlay {
        return Err(
            "--no-overlay does not apply to --base: both sides are built with the repo's overlay".to_string(),
        );
    }
    let d = graph_delta_vs_rev(&args.repo, base)?;
    let report = pattern_conformance_delta(&d.after.merged, &d.after.repo_labels, &d.delta, p);
    Ok((report, "delta"))
}

pub(crate) fn run(args: Args) -> i32 {
    if args.experimental {
        eprintln!("{EXPERIMENTAL_NOTE}");
    }
    let p = match pattern_args(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let (report, mode) = match answer(&args, &p) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    eprintln!("[patterns] surface=cli mode={mode}");
    if args.json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
    } else {
        print!("{}", render(&args.repo, args.base.as_deref(), &p, &report));
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

/// `METHOD path`, whichever half is known, or `—`.
fn route(method: Option<&str>, path: Option<&str>) -> String {
    match (method, path) {
        (Some(m), Some(p)) => format!("{m} {p}"),
        (None, Some(p)) => p.to_string(),
        (Some(m), None) => m.to_string(),
        (None, None) => "—".to_string(),
    }
}

/// `k=v, ...` for a count map, or `none`.
fn counts<'a>(m: impl IntoIterator<Item = (&'a &'static str, &'a usize)>) -> String {
    let parts: Vec<String> = m.into_iter().map(|(k, v)| format!("{k}={v}")).collect();
    if parts.is_empty() { "none".to_string() } else { parts.join(", ") }
}

/// The table report (module docs).
fn render(repo: &str, base: Option<&str>, p: &PatternArgs, r: &PatternReport) -> String {
    let mut out = String::new();
    let mode = match base {
        Some(b) => format!("delta vs `{b}`"),
        None => "whole graph".to_string(),
    };
    out.push_str(&format!("# glia patterns `{repo}` ({mode})\n\n"));
    out.push_str(&format!(
        "- handlers: {} in {} populations; judged: {}; below min support ({}): {}; divergences{}: {}; blind{}: {}\n",
        r.handlers,
        r.populations.len(),
        r.judged,
        p.min_support,
        r.skipped_small,
        if r.delta_mode { " touched by the change" } else { "" },
        r.divergences.len(),
        if r.delta_mode { " touched by the change" } else { "" },
        r.blind,
    ));
    out.push_str(&format!("- excluded handlers: {}\n", counts(&r.excluded)));
    out.push_str(&format!("- role sources: {}\n\n", counts(&r.role_sources)));
    if r.populations.is_empty() {
        out.push_str("_(no route handler placed in any population: nothing to judge)_\n");
        return out;
    }
    for pop in &r.populations {
        population(&mut out, pop, r, p);
    }
    out
}

fn population(out: &mut String, pop: &Population, r: &PatternReport, p: &PatternArgs) {
    let sigs: Vec<String> = pop.signatures.iter().map(|(s, n)| format!("`{s}` ×{n}")).collect();
    let name = match &pop.package {
        Some(pkg) => format!("{} / {pkg}", pop.service),
        None => pop.service.clone(),
    };
    match pop.status {
        "too_small" => {
            out.push_str(&format!(
                "_(population below min support: `{name}` has {} handlers, fewer than {})_\n\n",
                pop.size, p.min_support
            ));
            return;
        }
        "judged" => out.push_str(&format!(
            "## {name} - {}/{} sighted handlers follow `{}`\n\n",
            pop.matching,
            pop.sighted,
            pop.convention.as_deref().unwrap_or_default()
        )),
        "blind" => out.push_str(&format!(
            "## {name} - blind: {} of {} handlers reach a sink, fewer than {}\n\n",
            pop.sighted, pop.size, p.min_support
        )),
        _ => out.push_str(&format!(
            "## {name} - no convention: no signature holds {}% of {} sighted handlers\n\n",
            p.min_share_pct, pop.sighted
        )),
    }
    out.push_str(&format!("- signatures: {}\n\n", sigs.join(", ")));
    if pop.status == "judged" {
        divergence_rows(out, pop, r);
    }
    blind_rows(out, pop, r);
}

/// The population's divergences the report lists (delta mode: the touched
/// ones): its exceptions that are among the report's divergences, so the rows
/// match the population's (service, package), not the service alone.
fn divergence_rows(out: &mut String, pop: &Population, r: &PatternReport) {
    let listed = |e: &Divergence| {
        r.divergences.iter().any(|d| {
            d.service == e.service && d.handler == e.handler && d.file == e.file && d.line == e.line
        })
    };
    let rows: Vec<&Divergence> = pop.exceptions.iter().filter(|e| listed(e)).collect();
    if rows.is_empty() {
        if r.delta_mode && !pop.exceptions.is_empty() {
            out.push_str(&format!(
                "_(no divergence touched by the change; {} in the whole graph)_\n\n",
                pop.exceptions.len()
            ));
        } else {
            out.push_str("_(no divergence)_\n\n");
        }
        return;
    }
    out.push_str("| handler | route | signature | location |\n|---|---|---|---|\n");
    for d in rows {
        out.push_str(&format!(
            "| `{}` | {} | `{}` | {} |\n",
            d.handler,
            route(d.route_method.as_deref(), d.route_path.as_deref()),
            d.signature,
            at(d.file.as_deref(), d.line)
        ));
    }
    out.push('\n');
}

/// The blind handlers: their count, then a row each the report lists (delta
/// mode: the touched ones).
fn blind_rows(out: &mut String, pop: &Population, r: &PatternReport) {
    let blind = pop.size.saturating_sub(pop.sighted);
    if blind == 0 {
        return;
    }
    if r.delta_mode {
        out.push_str(&format!(
            "- blind (the graph follows no chain): {blind}; touched by the change: {}\n\n",
            pop.blind.len()
        ));
    } else {
        out.push_str(&format!("- blind (the graph follows no chain): {blind}\n\n"));
    }
    if pop.blind.is_empty() {
        return;
    }
    out.push_str("| handler | route | location |\n|---|---|---|\n");
    for b in &pop.blind {
        let route = route(b.route_method.as_deref(), b.route_path.as_deref());
        out.push_str(&format!("| `{}` | {route} | {} |\n", b.handler, at(b.file.as_deref(), b.line)));
    }
    out.push('\n');
}
