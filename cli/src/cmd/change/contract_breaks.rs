//! `glia contract-breaks` (CC.8b) — "did my change break my clients?", as a
//! CI gate: the human surface over the engine's
//! `contract_breaks::contract_breaks_vs_rev` (CC.8a), the git rev `--base`
//! (default `HEAD`) against the working tree. Every contract (an OpenAPI /
//! AsyncAPI op, a proto / Avro / JSON Schema message type) is paired old ->
//! new and judged by its format's evolution rules, and every client the change
//! left without a provider is listed.
//!
//! Table mode prints `# glia contract-breaks <repo> vs <rev>`, a summary line,
//! then `## breaking` and, unless `--breaking-only` left them out, `## unknown`
//! and `## compatible` (a section with no row is omitted, bar `## breaking`),
//! each `| kind | key | change | tier | at | rule | field | before | after |`
//! with one row per field change (`before` is the rev's declaration, `after`
//! the working tree's; a breaking one's rule in bold) and one row for a
//! contract removed, added or without a comparable field (its `change` carries
//! the engine's note). Then `## orphaned clients`, `| client | at | category |
//! was calling | reason | tier |`, when a client lost its provider. `at` is
//! the working tree's 1-based `file:line` (LD.1), a removed contract's the
//! rev's, marked `(at <rev>)`. An answer with no row prints
//! `_(no contract change vs <rev>)_` and the engine's absence. `--json`
//! prints the engine's `ContractBreaks` (`{base, schemas, orphaned_clients,
//! breaking, absence}`).
//!
//! Exit 1 when `breaking` (the breaking schema rows plus the orphaned clients)
//! is above 0, else 0, in both modes: a compatible or unknown change never
//! fails the gate. Exit 2 on a usage error (an `--avro` mode outside
//! `contract_breaks::AVRO_MODES`, `--no-overlay`) or a git / build failure,
//! with the engine's message.
//!
//! Writes: the working tree's parse-cache sidecar
//! (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored), as `glia delta`
//! does, under `GLIA_NO_PERSIST=1` too; never a `.gmap` layout.
//!
//! Fired-on marker: the engine's
//! `[contract-breaks] base=<rev> pairs=<P> breaking=<B> compatible=<C> unknown=<U> removed=<R> added=<A> orphaned_clients=<O>`,
//! once per run.

use glia_engine::contract_breaks::{
    AVRO_MODES, ContractBreakArgs, ContractBreaks, OrphanedClient, SchemaChange,
    contract_breaks_vs_rev,
};
use glia_engine::contract_fields::{FieldChange, FieldSide};

use crate::cmd::resolve::print_absence;
use crate::common::build_options;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo (a git work tree).
    repo: String,
    /// The git rev to compare the working tree against: a branch, tag, sha
    /// or `HEAD~N`.
    #[arg(long, value_name = "REV", default_value = "HEAD")]
    base: String,
    /// Avro's compatibility direction: `backward` (the new schema reads what
    /// the old one wrote), `forward` (the old schema reads what the new one
    /// writes) or `full` (both).
    #[arg(
        long,
        value_name = "MODE",
        default_value = AVRO_MODES[0],
        value_parser = clap::builder::PossibleValuesParser::new(AVRO_MODES),
    )]
    avro: String,
    /// List only the breaking contract changes (the exit code and the
    /// orphaned clients are the same either way).
    #[arg(long)]
    breaking_only: bool,
    /// Emit JSON (`{base, schemas, orphaned_clients, breaking, absence}`)
    /// instead of tables.
    #[arg(long)]
    json: bool,
}

/// The engine's arguments; `Err` names a mode outside [`AVRO_MODES`] (clap
/// has already refused one, so this only guards the `&'static str` lookup).
fn engine_args(avro: &str, breaking_only: bool) -> Result<ContractBreakArgs, String> {
    let Some(mode) = AVRO_MODES.iter().copied().find(|m| *m == avro) else {
        return Err(format!(
            "unknown avro mode `{avro}`: expected one of {}",
            AVRO_MODES.join(", ")
        ));
    };
    let mut args = ContractBreakArgs::default();
    args.avro_mode = mode;
    args.breaking_only = breaking_only;
    Ok(args)
}

/// A markdown table cell: a `|` in a type, key or path must not split the row.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// `file:line`, `file`, or `None`.
fn at(file: Option<&str>, line: Option<i64>) -> Option<String> {
    match (file, line) {
        (Some(f), Some(l)) => Some(format!("{f}:{l}")),
        (Some(f), None) => Some(f.to_string()),
        _ => None,
    }
}

/// A side's location, falling back to its qname.
fn side_at(s: &FieldSide) -> String {
    at(s.file.as_deref(), s.line).unwrap_or_else(|| format!("`{}`", s.qname))
}

/// Where a contract row points: the working tree's declaration, else (a
/// removed contract) the rev's, marked with the rev.
fn row_at(r: &SchemaChange, base: &str) -> String {
    match (&r.after, &r.before) {
        (Some(a), _) => side_at(a),
        (None, Some(b)) => format!("{} (at {base})", side_at(b)),
        (None, None) => "—".to_string(),
    }
}

/// The `change` cell: the contract's change, and the engine's note on it.
fn row_change(r: &SchemaChange) -> String {
    match r.note {
        Some(n) => format!("{} — _{n}_", r.change),
        None => r.change.to_string(),
    }
}

/// The `field` cell: `total`, a non-`fields` section (`request`,
/// `response:<code>`, `payload[:<name>]`) in front as `[response:200] total`.
fn field_name(c: &FieldChange) -> String {
    if c.section == "fields" {
        format!("`{}`", c.field)
    } else {
        format!("[{}] `{}`", c.section, c.field)
    }
}

/// The table rows of one contract: one per field change, else one for the
/// whole contract (removed, added, or not comparable).
fn contract_rows(r: &SchemaChange, base: &str) -> Vec<String> {
    let lead = format!(
        "| {} | {} | {} | {} | {} |",
        r.kind,
        cell(&format!("`{}`", r.key)),
        cell(&row_change(r)),
        r.tier,
        cell(&row_at(r, base)),
    );
    if r.changes.is_empty() {
        return vec![format!("{lead} — | — | — | — |")];
    }
    let side = |s: &Option<String>| s.as_deref().map(cell).unwrap_or_else(|| "—".to_string());
    r.changes
        .iter()
        .map(|c| {
            let rule = if c.breaking {
                format!("**{}**", c.rule)
            } else {
                c.rule.to_string()
            };
            format!(
                "{lead} {rule} | {} | {} | {} |",
                cell(&field_name(c)),
                side(&c.producer),
                side(&c.consumer),
            )
        })
        .collect()
}

fn print_contracts(title: &str, rows: &[&SchemaChange], base: &str) {
    println!("## {title}");
    println!();
    if rows.is_empty() {
        println!("_(none)_");
        println!();
        return;
    }
    println!("| kind | key | change | tier | at | rule | field | before | after |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for r in rows {
        for line in contract_rows(r, base) {
            println!("{line}");
        }
    }
    println!();
}

fn print_orphans(orphans: &[OrphanedClient]) {
    println!("## orphaned clients");
    println!();
    println!("| client | at | category | was calling | reason | tier |");
    println!("|---|---|---|---|---|---|");
    for o in orphans {
        println!(
            "| {} | {} | {} | {} | {} | {} |",
            cell(&format!("`{}`", o.client_qname)),
            cell(&at(o.file.as_deref(), o.line).unwrap_or_else(|| "—".to_string())),
            o.category,
            cell(&format!("`{}`", o.target_qname)),
            o.reason,
            o.tier,
        );
    }
    println!();
}

fn print_table(repo: &str, b: &ContractBreaks, breaking_only: bool) {
    let with = |status: &str| -> Vec<&SchemaChange> {
        b.schemas.iter().filter(|r| r.status == status).collect()
    };
    let (breaking, unknown, compatible) = (with("breaking"), with("unknown"), with("compatible"));
    println!("# glia contract-breaks `{repo}` vs {}", b.base);
    println!();
    let rest = if breaking_only {
        "unknown / compatible: left out (--breaking-only)".to_string()
    } else {
        format!("unknown: {} | compatible: {}", unknown.len(), compatible.len())
    };
    println!(
        "- breaking: {} ({} contract {}, {} orphaned {}) | {rest}",
        b.breaking,
        breaking.len(),
        if breaking.len() == 1 { "change" } else { "changes" },
        b.orphaned_clients.len(),
        if b.orphaned_clients.len() == 1 { "client" } else { "clients" },
    );
    println!();
    if b.schemas.is_empty() && b.orphaned_clients.is_empty() {
        let what = if breaking_only { "breaking contract change" } else { "contract change" };
        println!("_(no {what} vs {})_", b.base);
        if let Some(a) = &b.absence {
            println!();
            print_absence(a);
        }
        return;
    }
    print_contracts("breaking", &breaking, &b.base);
    if !unknown.is_empty() {
        print_contracts("unknown", &unknown, &b.base);
    }
    if !compatible.is_empty() {
        print_contracts("compatible", &compatible, &b.base);
    }
    if !b.orphaned_clients.is_empty() {
        print_orphans(&b.orphaned_clients);
    }
}

pub(crate) fn run(args: Args) -> i32 {
    if !build_options().overlay {
        eprintln!(
            "error: --no-overlay does not apply to contract-breaks: both sides are built with \
             the repo's overlay"
        );
        return 2;
    }
    let engine = match engine_args(&args.avro, args.breaking_only) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let answer = match contract_breaks_vs_rev(&args.repo, &args.base, &engine) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
    } else {
        print_table(&args.repo, &answer, args.breaking_only);
    }
    if answer.breaking > 0 { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_mode_maps_and_others_are_refused() {
        for mode in AVRO_MODES {
            let a = engine_args(mode, true).expect("an engine mode");
            assert_eq!((a.avro_mode, a.breaking_only), (*mode, true));
        }
        let err = engine_args("sideways", false).expect_err("no such mode");
        assert_eq!(err, "unknown avro mode `sideways`: expected one of backward, forward, full");
    }

    #[test]
    fn a_pipe_never_splits_a_row() {
        assert_eq!(cell("string|null"), "string\\|null");
        assert_eq!(at(Some("a.yaml"), Some(7)).as_deref(), Some("a.yaml:7"));
        assert_eq!(at(Some("a.yaml"), None).as_deref(), Some("a.yaml"));
        assert_eq!(at(None, Some(7)), None);
    }
}
