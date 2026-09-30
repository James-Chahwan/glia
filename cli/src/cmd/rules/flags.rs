//! `glia flags` (CC.7c) — the human surface over the engine's
//! `flags::flags` (CC.7b): the stale feature-flag report a developer scans
//! before a cleanup sprint. A report, not a gate (like `glia cycles`): it
//! exits 0 whatever it finds; exit 2 is a build or usage error. A team that
//! gates on dead flags reads `--json`.
//!
//! The header counts the flags and the flag definition files and says whether
//! the quiet rule was evaluated (`yes (history to <YYYY-MM-DD>)`, from the
//! report's `history_now`, or `no (run glia history sync --blame)`). Then one
//! table per finding, in the engine's `STATUSES` order (dead, undefined,
//! single_site, quiet): `| flag | tier | readers | read at | defined at | note
//! |`, each site the first read / definition with `+N more`, a Flipt
//! definition (no line) as its bare file. Then `## all flags`, the inventory
//! (`| flag | providers | readers | definitions |`).
//!
//! `--status` is presentation: the engine returns every flag, and the filter
//! keeps the listed findings' tables and, in `--json`, the rows holding one of
//! them. The header and the JSON `counts` stay report-wide, and the inventory
//! table is left out.

use glia_engine::flags::{
    DEFAULT_QUIET_DAYS, FlagArgs, FlagRow, FlagSite, FlagsReport, STATUS_QUIET, STATUS_UNDEFINED,
    STATUSES, flags, parse_status,
};

use crate::cmd::resolve::print_absence;
use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (a flag read in one repo and defined in
    /// another is one row). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Days every reader of a flag must be unchanged, before the history
    /// snapshot's newest change, for the flag to be `quiet`.
    #[arg(long, default_value_t = DEFAULT_QUIET_DAYS)]
    quiet_days: u32,
    /// Keep only these findings' tables (and, with --json, the flags holding
    /// one of them). Repeatable.
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(STATUSES))]
    status: Vec<String>,
    /// Keep only flags with a definition or read under this repo-relative
    /// path or project label (see `glia projects`); a kept flag stays whole.
    #[arg(long)]
    scope: Option<String>,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let mut wanted: Vec<&'static str> = Vec::new();
    for s in &args.status {
        match parse_status(s) {
            Some(st) => wanted.push(st),
            None => {
                eprintln!(
                    "error: unknown --status '{s}'; valid: {}",
                    STATUSES.join(", ")
                );
                return 2;
            }
        }
    }
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut query = FlagArgs::default();
    query.quiet_days = args.quiet_days;
    query.scope = args.scope;
    let mut report = flags(&result.merged, &query);
    if let Some(a) = report.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    if args.json {
        if !wanted.is_empty() {
            report.flags.retain(|r| holds_any(r, &wanted));
        }
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
        return 0;
    }

    println!("# glia flags `{}`", args.repo);
    println!();
    println!("{}", header(&report));
    println!();
    if let Some(a) = &report.absence {
        println!("_(no flags)_");
        println!();
        print_absence(a);
        return 0;
    }
    let statuses: Vec<&str> = STATUSES
        .into_iter()
        .filter(|s| wanted.is_empty() || wanted.contains(s))
        .collect();
    for status in statuses {
        print_finding(&report, status);
    }
    if wanted.is_empty() {
        print_inventory(&report.flags);
    }
    0
}

/// `- flags: K; definition files: D; quiet evaluated: yes (history to <day>)`.
fn header(r: &FlagsReport) -> String {
    let quiet = match (r.quiet_evaluated, r.history_now) {
        (true, Some(t)) => format!("yes (history to {})", utc_date(t)),
        _ => "no (run glia history sync --blame)".to_string(),
    };
    format!(
        "- flags: {}; definition files: {}; quiet evaluated: {quiet}",
        r.flags.len(),
        r.definitions_in_graph
    )
}

fn holds_any(r: &FlagRow, wanted: &[&str]) -> bool {
    r.findings.iter().any(|f| wanted.contains(&f.status))
}

/// `## <status>` and one row per flag holding that finding, or why the table
/// is empty.
fn print_finding(r: &FlagsReport, status: &str) {
    println!("## {status}");
    println!();
    let rows: Vec<(&FlagRow, usize)> = r
        .flags
        .iter()
        .filter_map(|row| {
            row.findings
                .iter()
                .position(|f| f.status == status)
                .map(|i| (row, i))
        })
        .collect();
    if rows.is_empty() {
        let why = if status == STATUS_QUIET && !r.quiet_evaluated {
            "not evaluated: no reader carries history; run glia history sync --blame"
        } else if status == STATUS_UNDEFINED && r.definitions_in_graph == 0 {
            "not evaluated: no flag definition file in the graph"
        } else {
            "none"
        };
        println!("_({why})_");
        println!();
        return;
    }
    println!("| flag | tier | readers | read at | defined at | note |");
    println!("|---|---|--:|---|---|---|");
    for (row, i) in rows {
        let f = &row.findings[i];
        println!(
            "| `{}` | {} | {} | {} | {} | {} |",
            cell(&row.key),
            f.tier,
            row.readers,
            first_site(&row.reads),
            first_site(&row.definitions),
            cell(&f.note),
        );
    }
    println!();
}

/// `## all flags`: every flag with its providers, readers and definitions.
fn print_inventory(rows: &[FlagRow]) {
    println!("## all flags");
    println!();
    println!("| flag | providers | readers | definitions |");
    println!("|---|---|--:|--:|");
    for r in rows {
        let providers = if r.providers.is_empty() {
            "—".to_string()
        } else {
            cell(&r.providers.join(", "))
        };
        println!(
            "| `{}` | {providers} | {} | {} |",
            cell(&r.key),
            r.readers,
            r.definitions.len()
        );
    }
    println!();
}

/// The first site as `file:line`, its bare file when it has no line (a Flipt
/// definition), else its qname; `+N more` when there are others; `—` for none.
fn first_site(sites: &[FlagSite]) -> String {
    let Some(s) = sites.first() else {
        return "—".to_string();
    };
    let at = match (s.file.as_deref(), s.line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        (None, _) => s.qname.clone(),
    };
    match sites.len() - 1 {
        0 => cell(&at),
        more => format!("{} +{more} more", cell(&at)),
    }
}

/// A table cell's text with its pipes escaped, so a key or note cannot split
/// the row.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// Unix seconds as their UTC day, `YYYY-MM-DD`: Howard Hinnant's
/// civil-from-days over the proleptic Gregorian calendar (eras of 400 years,
/// March-based years so the leap day ends one). No locale, no wall clock. The
/// same day `glia hotspots` and `glia timeline` print for a unix time.
fn utc_date(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::{cell, utc_date};

    #[test]
    fn utc_date_known_days() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_700_000_000), "2023-11-14");
        assert_eq!(utc_date(1_717_280_000), "2024-06-01");
        assert_eq!(utc_date(-1), "1969-12-31");
    }

    #[test]
    fn cells_escape_pipes() {
        assert_eq!(cell("a|b"), "a\\|b");
        assert_eq!(cell("plain"), "plain");
    }
}
