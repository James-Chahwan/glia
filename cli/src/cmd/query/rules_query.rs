//! `glia query` (DB.5) — transport over the engine's `rules::query` (DB.2b, the
//! prelude merged in by DB.2c) and `rules::query_gql` (DB.7c): read the program,
//! build the graph (`--with` merges first), ask, render, choose the exit code.
//! The file is named `rules_query.rs`, not `query.rs`, so the module is not
//! `cmd::query::query` (clippy::module_inception, DC.8's handoff).
//!
//! **The program.** `-e <TEXT>`, else the positional `<FILE>` (`-` reads
//! stdin), else stdin when it is not a terminal. A terminal stdin, or a program
//! that is empty after trimming, is a usage error. Diagnostics name the source:
//! the file path, `<-e>` or `<stdin>`. `--gql` reads the text as a GQL read
//! query; `--output` names Datalog outputs (`name` or `name/arity`, repeatable;
//! default the program's sinks) and is a usage error with `--gql`, whose one
//! output is `result`. `--max-rows` is the evaluation's row budget (the engine's
//! budget stop names it); `--limit` cuts each output's rows after the sort, the
//! totals kept.
//!
//! **Text.** `# glia query `<repo>` (datalog|gql)`, then per output
//! `## <pred>/<arity> - <total> row(s)` (GQL: `## result - ...`) and a table
//! `| # | <column>... | tier | at |`: a node value is `` `qname` ``, a string
//! as is, an integer in decimal; `at` is the row's first node value's
//! `file:line` (a GQL path row's start), or `—`. With `--explain` each row is
//! followed by its witness table, check's evidence layout (category, from, to,
//! at, emitter, tier). A cut output says so; the coverage caveats of rows read
//! through a negation print once per output under its table, deduplicated. No
//! row in any output: the LD.8a absence, its FACT note and caveats.
//!
//! **`--json`** is the engine's `QueryAnswer` (keys `language`, `outputs`,
//! `results`, `absence`), same exit codes.
//!
//! **Exit**: 0 with rows, 1 with none (the absence says why; a `--limit 0` of a
//! non-empty answer is 0), 2 on a usage, read, build, parse, analysis, budget or
//! evaluation error (the engine's rendered text on stderr, one diagnostic per
//! line, `<source>:<line>:<col>: error: <message>`).
//!
//! **fired_on**, on stderr: the engine's `[query] lang=<datalog|gql> outputs=<o>
//! rows=<r> ... surface=cli` once per answered query, then
//! `[query] surface=cli exit=<0|1|2>` on every exit of `run`.

use std::collections::BTreeSet;
use std::io::{IsTerminal, Read};

use glia_engine::check::{MAX_EVIDENCE, ViolationEdge};
use glia_engine::rules::{
    QueryAnswer, QueryOptions, QueryOutput, QueryRow, QueryValue, query, query_gql,
};

use crate::cmd::resolve::print_absence;
use crate::common::generate_for;

/// `QueryOptions::surface`: the engine's marker and budget hint.
const SURFACE: &str = "cli";

/// `--limit`'s default: rows shown per output.
const DEFAULT_LIMIT: usize = 200;

/// The usage error for a missing or empty program.
const GIVE_A_PROGRAM: &str = "give a program: <file>, -e <text> or stdin";

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// The program file; `-` reads stdin. With neither it nor `-e`, a piped
    /// stdin is read.
    #[arg(conflicts_with = "expr")]
    file: Option<String>,
    /// The program text.
    #[arg(short = 'e', long = "expr", value_name = "TEXT")]
    expr: Option<String>,
    /// The text is a GQL read query (MATCH ... RETURN ...), not Datalog.
    #[arg(long)]
    gql: bool,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// An output predicate to answer, `name` or `name/arity`. Repeatable;
    /// default: the program's sinks. Datalog only.
    #[arg(long, value_name = "PRED")]
    output: Vec<String>,
    /// Rows shown per output, cut after the sort; the totals stay.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_LIMIT)]
    limit: usize,
    /// The evaluation's row budget (default: the rules engine's).
    #[arg(long, value_name = "N")]
    max_rows: Option<usize>,
    /// Follow each row with its witness: the derivation's edges, located.
    #[arg(long)]
    explain: bool,
    /// Emit the answer as JSON instead of tables (same exit codes).
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let code = answer(&args);
    eprintln!("[query] surface={SURFACE} exit={code}");
    code
}

/// `run` without its marker: the exit code.
fn answer(args: &Args) -> i32 {
    if args.gql && !args.output.is_empty() {
        eprintln!("error: --output names Datalog outputs; a GQL query has one, `result`");
        return 2;
    }
    let (source, text) = match program(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut opts = QueryOptions::default();
    opts.outputs = args.output.clone();
    opts.limit = Some(args.limit);
    opts.explain = args.explain;
    opts.row_budget = args.max_rows;
    opts.source_name = source;
    opts.surface = SURFACE;
    let asked = if args.gql {
        query_gql(&result.merged, &text, &opts)
    } else {
        query(&result.merged, &text, &opts)
    };
    let mut answer = match asked {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", e.rendered);
            return 2;
        }
    };
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    let code = if answer.absence.is_some() { 1 } else { 0 };
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
    } else {
        print_answer(&args.repo, &answer, args.explain);
    }
    code
}

/// The program's source name and text (module doc, **The program**).
fn program(args: &Args) -> Result<(String, String), String> {
    let (source, text) = match (&args.expr, args.file.as_deref()) {
        (Some(e), _) => ("<-e>".to_string(), e.clone()),
        (None, Some("-")) => ("<stdin>".to_string(), read_stdin()?),
        (None, Some(path)) => (
            path.to_string(),
            std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?,
        ),
        (None, None) if std::io::stdin().is_terminal() => return Err(GIVE_A_PROGRAM.to_string()),
        (None, None) => ("<stdin>".to_string(), read_stdin()?),
    };
    if text.trim().is_empty() {
        return Err(format!("{GIVE_A_PROGRAM} ({source} is empty)"));
    }
    Ok((source, text))
}

fn read_stdin() -> Result<String, String> {
    let mut text = String::new();
    std::io::stdin()
        .lock()
        .read_to_string(&mut text)
        .map_err(|e| format!("cannot read stdin: {e}"))?;
    Ok(text)
}

/// The text rendering (module doc, **Text**).
fn print_answer(repo: &str, a: &QueryAnswer, explain: bool) {
    println!("# glia query `{repo}` ({})", a.language);
    println!();
    let mut rows = a.results.iter();
    for out in &a.outputs {
        let shown: Vec<&QueryRow> = rows.by_ref().take(out.shown).collect();
        print_output(a.language, out, &shown, explain);
    }
    if let Some(absence) = &a.absence {
        print_absence(absence);
    }
}

/// `## <pred>/<arity>` for Datalog, `## result` for GQL.
fn heading(language: &str, out: &QueryOutput) -> String {
    if language == "gql" {
        out.predicate.clone()
    } else {
        format!("{}/{}", out.predicate, out.arity)
    }
}

/// One output: its heading, its rows (each followed by its witness with
/// `explain`), the cut note and the rows' caveats.
fn print_output(language: &str, out: &QueryOutput, rows: &[&QueryRow], explain: bool) {
    println!("## {} - {} row(s)", heading(language, out), out.total);
    println!();
    if rows.is_empty() {
        if out.total > 0 {
            println!("_(showing 0 of {}; --limit <n> shows more)_", out.total);
        } else {
            println!("_(no rows)_");
        }
        println!();
        return;
    }
    if explain {
        for (i, r) in rows.iter().enumerate() {
            print_table_head(&out.columns);
            print_row(i + 1, r);
            println!();
            print_witness(r);
        }
    } else {
        print_table_head(&out.columns);
        for (i, r) in rows.iter().enumerate() {
            print_row(i + 1, r);
        }
        println!();
    }
    if out.shown < out.total {
        println!(
            "_(showing {} of {}; --limit <n> shows more)_",
            out.shown, out.total
        );
        println!();
    }
    print_caveats(rows);
}

fn print_table_head(columns: &[String]) {
    let names: Vec<String> = columns.iter().map(|c| cell(c)).collect();
    let mut head = String::from("| # |");
    let mut rule = String::from("|--:|");
    for n in &names {
        head.push_str(&format!(" {n} |"));
        rule.push_str("---|");
    }
    println!("{head} tier | at |");
    println!("{rule}---|---|");
}

fn print_row(n: usize, r: &QueryRow) {
    let mut line = format!("| {n} |");
    for v in &r.values {
        line.push_str(&format!(" {} |", value(v)));
    }
    println!("{line} {} | {} |", r.tier, at(r.file.as_deref(), r.line));
}

/// A value's cell: a node `` `qname` ``, a string as is, an integer in decimal.
fn value(v: &QueryValue) -> String {
    match v {
        QueryValue::Node { qname, .. } => format!("`{}`", cell(qname)),
        QueryValue::Str(s) => cell(s),
        QueryValue::Int(i) => i.to_string(),
        other => cell(&format!("{other:?}")),
    }
}

/// `text` safe inside a markdown table cell: `|` escaped, line breaks as spaces.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// `file:line`, the file alone, or `—`.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

/// A row's witness edges in check's evidence layout, or why there are none.
fn print_witness(r: &QueryRow) {
    if r.witness.is_empty() {
        println!("_(witness: no edge; the derivation reads node facts only)_");
        println!();
        return;
    }
    println!("_witness: {} edge(s)_", r.witness.len());
    println!();
    println!("| # | category | from | to | at | emitter | tier |");
    println!("|--:|---|---|---|---|---|---|");
    for (i, e) in r.witness.iter().enumerate() {
        println!(
            "| {} | {} | `{}` | `{}` | {} | {} | {} |",
            i + 1,
            e.category,
            cell(&e.from_qname),
            cell(&e.to_qname),
            at(e.file.as_deref(), e.line),
            e.emitter.as_deref().unwrap_or("—"),
            edge_tier(e),
        );
    }
    println!();
    if r.witness.len() >= MAX_EVIDENCE {
        println!("_(at most {MAX_EVIDENCE} witness edges are listed)_");
        println!();
    }
}

/// An edge's tier, with its note in parentheses when it has one.
fn edge_tier(e: &ViolationEdge) -> String {
    match e.note.as_deref() {
        Some(n) => format!("{} ({})", e.tier, cell(n)),
        None => e.tier.to_string(),
    }
}

/// The coverage caveats of the output's rows that read through a negation,
/// once each, in first-seen order.
fn print_caveats(rows: &[&QueryRow]) {
    let caveated = rows.iter().filter(|r| !r.caveats.is_empty()).count();
    if caveated == 0 {
        return;
    }
    println!("_{caveated} row(s) rest on an absence (a negation); its coverage caveats:_");
    println!();
    let mut seen = BTreeSet::new();
    for c in rows.iter().flat_map(|r| &r.caveats) {
        if seen.insert((c.language, c.edge_category, c.note)) {
            println!(
                "> caveat ({}, {}): {} - verify: {}",
                c.language, c.edge_category, c.note, c.verify
            );
        }
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::{at, cell};

    #[test]
    fn cells_escape_pipes_and_breaks() {
        assert_eq!(cell("a|b\nc"), "a\\|b c");
        assert_eq!(cell("web::app::page"), "web::app::page");
    }

    #[test]
    fn at_cell() {
        assert_eq!(at(Some("a.py"), Some(3)), "a.py:3");
        assert_eq!(at(Some("a.py"), None), "a.py");
        assert_eq!(at(None, Some(3)), "—");
    }
}
