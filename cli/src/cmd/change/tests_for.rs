//! `glia tests-for` (LE.3b) — the tests to run for a change: the human
//! surface over the engine's `tests_for::{tests_for, tests_for_diff,
//! tests_for_rev}`.
//!
//! Exactly one seed source: qnames (or bare names) after the repo, `--diff
//! <file>` (`-` reads stdin; a unified diff or a changed-file list), or
//! `--base <rev>` (the working tree's change against a git rev). Table mode
//! prints the seeds, then one row per test (tier, test, kind, `file:line`,
//! depth, its signals, the seeds it covers and the witness chain), the test
//! files, and the untested / unresolved seeds; an empty answer prints its
//! absence note. `--files-only` prints the test files one per line and
//! nothing else, to pipe into a runner (`pytest $(glia tests-for . --base main
//! --files-only)`). `--json` prints the engine's `TestsFor`.
//!
//! Rows rank by the signals the build ingested (CC.9a): a test that failed in
//! the latest ingested run (`glia tests ingest`) first, then a test covering a
//! seed on a failing trace, then by tier and by how often the test's file
//! changed with the seed's (`glia history sync`); a test module that only
//! co-changes adds a heuristic `cochange` row. `--no-signals` reads none of
//! them (the structural order). `--limit N` keeps the first N rows after
//! ranking, so a CI budget cuts the least likely tests, and `--files-only`
//! prints the kept rows' files.
//!
//! Exit 0 on an answer (an empty one included: this is a report, not a gate),
//! 2 on a usage error (no seed source or two, `--base` with `--with` or
//! `--no-overlay`, `--limit 0`), an unreadable `--diff`, a git / build failure
//! or more seeds than the engine walks from.
//!
//! `--base` builds through the engine's graph delta (LE.1b), which saves the
//! working tree's parse-cache sidecar (`<repo>/.glia/graph/parse_cache.bin`,
//! self-gitignored) as an incremental build does, under `GLIA_NO_PERSIST=1`
//! too; never a `.gmap` layout.
//!
//! Fired-on markers: the engine's
//! `[tests-for] seeds=<S> tests=<T> fact=<F> derived=<D> heuristic=<H> untested=<U> files=<N>`,
//! then, unless `--no-signals`,
//! `[tests-for] signals failed_last_run=<A> on_failing_trace=<B> cochange=<C> cochange_only=<D> omitted=<E>`.

use std::io::Read;

use glia_engine::tests_for::{
    COCHANGE, DEFAULT_MAX_DEPTH, TestHit, TestsFor, TestsForArgs, tests_for, tests_for_diff,
    tests_for_rev,
};

use crate::common::{build_options, generate_for};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Seed qnames or bare names (one seed source of three).
    qnames: Vec<String>,
    /// Seed from a unified diff or a changed-file list in this file; `-`
    /// reads stdin.
    #[arg(long, value_name = "FILE")]
    diff: Option<String>,
    /// Seed from the working tree's change against this git rev (a branch,
    /// tag, sha or `HEAD~N`).
    #[arg(long, value_name = "REV")]
    base: Option<String>,
    /// Hops of the backward walk from each seed.
    #[arg(long, default_value_t = DEFAULT_MAX_DEPTH)]
    depth: usize,
    /// Leave out the heuristic tier (test modules paired with a seed's module
    /// by name).
    #[arg(long)]
    no_module_level: bool,
    /// Keep only tests whose file is under this path or project label.
    #[arg(long)]
    scope: Option<String>,
    /// Keep the first N tests after ranking (a CI budget); `--files-only`
    /// prints their files.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
    /// Rank structurally only: read no test failure or git co-change and add
    /// no co-change row.
    #[arg(long)]
    no_signals: bool,
    /// Additional repos to merge in (cross-service). Repeatable. Not with
    /// `--base`.
    #[arg(long)]
    with: Vec<String>,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
    /// Print only the test files, one per line.
    #[arg(long, conflicts_with = "json")]
    files_only: bool,
}

/// `file:line`, `file`, or `—`.
fn at(t: &TestHit) -> String {
    match (t.file.as_deref(), t.line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

/// `test -[CALLS]-> a -[TESTS]-> seed`, or `(changed)` for a changed test.
fn chain(t: &TestHit) -> String {
    if t.path.is_empty() {
        return "(changed)".to_string();
    }
    let mut out = format!("`{}`", t.name);
    for (qname, category) in &t.path {
        let tail = qname.rsplit("::").next().unwrap_or(qname);
        out.push_str(&format!(" -[{category}]-> `{tail}`"));
    }
    out
}

/// The row's signals, comma-joined (`cochange` with its per-mille
/// confidence), or `—`.
fn signals(t: &TestHit) -> String {
    if t.signals.is_empty() {
        return "—".to_string();
    }
    t.signals
        .iter()
        .map(|s| match (*s, t.cochange_permille) {
            (COCHANGE, Some(p)) => format!("{s} {p}‰"),
            _ => s.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn names(v: &[String]) -> String {
    if v.is_empty() {
        "none".to_string()
    } else {
        v.iter()
            .map(|s| format!("`{s}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn print_table(repo: &str, a: &TestsFor) {
    let count = |tier: &str| a.tests.iter().filter(|t| t.tier == tier).count();
    println!("# glia tests-for `{repo}`");
    println!();
    println!("- seeds: {}", names(&a.seeds));
    println!(
        "- tests: {} (fact {}, derived {}, heuristic {}) in {} files",
        a.tests.len(),
        count("fact"),
        count("derived"),
        count("heuristic"),
        a.test_files.len()
    );
    if a.omitted > 0 {
        println!("- omitted: {} (past --limit)", a.omitted);
    }
    println!("- untested: {}", names(&a.untested));
    if !a.unresolved.is_empty() {
        println!("- unresolved: {}", names(&a.unresolved));
    }
    println!();
    if a.tests.is_empty() {
        match &a.absence {
            Some(x) => println!("_(no tests: {})_", x.note),
            None => println!("_(no tests)_"),
        }
        return;
    }
    println!("| # | tier | test | kind | at | depth | signals | covers | via |");
    println!("|--:|---|---|---|---|--:|---|---|---|");
    for (i, t) in a.tests.iter().enumerate() {
        println!(
            "| {} | {} | `{}` | {} | {} | {} | {} | {} | {} |",
            i + 1,
            t.tier,
            t.qname,
            t.kind,
            at(t),
            t.depth,
            signals(t),
            names(&t.covers),
            chain(t)
        );
    }
    println!();
    println!("## Test files");
    println!();
    for f in &a.test_files {
        println!("- {f}");
    }
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

/// The answer for whichever seed source `args` names.
fn answer(args: &Args, opts: &TestsForArgs) -> Result<TestsFor, String> {
    let sources = usize::from(!args.qnames.is_empty())
        + usize::from(args.diff.is_some())
        + usize::from(args.base.is_some());
    if sources != 1 {
        return Err(
            "give exactly one seed source: qnames after the repo, --diff <file|->, or --base <rev>"
                .to_string(),
        );
    }
    if let Some(base) = &args.base {
        if !args.with.is_empty() {
            return Err(
                "--base builds the one repo against its git rev; it does not take --with"
                    .to_string(),
            );
        }
        if !build_options().overlay {
            return Err(
                "--no-overlay does not apply to --base: both sides are built with the repo's overlay"
                    .to_string(),
            );
        }
        return tests_for_rev(&args.repo, base, opts);
    }
    let diff = args.diff.as_deref().map(read_diff).transpose()?;
    let built = generate_for(&args.repo, &args.with)?;
    match diff {
        Some(text) => tests_for_diff(&built.merged, &text, opts),
        None => {
            let qnames: Vec<&str> = args.qnames.iter().map(String::as_str).collect();
            tests_for(&built.merged, &qnames, opts)
        }
    }
}

pub(crate) fn run(args: Args) -> i32 {
    let mut opts = TestsForArgs::default();
    opts.max_depth = args.depth;
    opts.scope = args.scope.clone();
    opts.module_level = !args.no_module_level;
    opts.limit = args.limit;
    opts.signals = !args.no_signals;
    let a = match answer(&args, &opts) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if args.files_only {
        for f in &a.test_files {
            println!("{f}");
        }
    } else if args.json {
        println!("{}", serde_json::to_string(&a).unwrap_or_default());
    } else {
        print_table(&args.repo, &a);
    }
    0
}
