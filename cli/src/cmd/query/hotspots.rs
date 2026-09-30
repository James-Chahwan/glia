//! `glia hotspots` (CC.10b) — the human surface over the engine's
//! `hotspots::hotspots` (CC.10a): the modules and symbols that change often
//! AND sit where much depends on them. One table per level asked for; each row
//! shows its churn, its churn rank and its centrality rank over the same
//! ranked population (`of`), the UTC day of its last change and where it is.
//! There is no composite score: rows order by the two ranks together, and
//! every row is tier heuristic (git history and a PageRank model).
//!
//! The header dates the history (`- history to <YYYY-MM-DD> (...)`, from the
//! answer's `history_head`). `--json` is the whole answer `{modules, symbols,
//! history_head, absence}`. Exit 0 with rows; 1 with none, the absence saying
//! why (`no_history`: run `glia history sync`; `no_match`: nothing past the
//! filters); 2 on a build or usage error.

use glia_engine::hotspots::{
    DEFAULT_MIN_CHURN, DEFAULT_TOP, Hotspot, HotspotArgs, LEVEL_BOTH, LEVEL_MODULE, LEVEL_SYMBOL,
    LEVELS, hotspots, parse_level,
};

use crate::cmd::resolve::print_absence;
use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// `module`: MODULE nodes by the commits that touched their file;
    /// `symbol`: functions, methods and classes by the distinct blame times
    /// in their span (needs `glia history sync --blame`); `both`.
    #[arg(
        long,
        default_value = LEVEL_BOTH,
        value_parser = clap::builder::PossibleValuesParser::new(LEVELS),
    )]
    level: String,
    /// Rows per table; 0 keeps every ranked row. The ranks always range over
    /// the whole population.
    #[arg(long, default_value_t = DEFAULT_TOP)]
    top: usize,
    /// Smallest churn that ranks: commits for a module, blame span changes
    /// for a symbol.
    #[arg(long, default_value_t = DEFAULT_MIN_CHURN)]
    min_churn: u32,
    /// Keep test and generated code (left out by default: it churns with the
    /// code it tests).
    #[arg(long)]
    include_tests: bool,
    /// Keep only nodes under this repo-relative path or project label (see
    /// `glia projects`); the ranks range over what is kept.
    #[arg(long)]
    scope: Option<String>,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let Some(level) = parse_level(&args.level) else {
        eprintln!("error: unknown --level '{}'; valid: {}", args.level, LEVELS.join(", "));
        return 2;
    };
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut query = HotspotArgs::default();
    query.level = level;
    query.top = args.top;
    query.min_churn = args.min_churn;
    query.include_tests = args.include_tests;
    query.scope = args.scope;
    let mut answer = hotspots(&result.merged, &query);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    let code = if answer.absence.is_some() { 1 } else { 0 };
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return code;
    }

    println!("# glia hotspots");
    println!();
    let modules = (level != LEVEL_SYMBOL).then(|| ranked_line(&answer.modules, "modules"));
    let symbols = (level != LEVEL_MODULE).then(|| ranked_line(&answer.symbols, "symbols"));
    let counts: Vec<String> = [modules, symbols].into_iter().flatten().collect();
    match answer.history_head {
        Some(t) => println!("- history to {} ({} ranked)", utc_date(t), counts.join(", ")),
        None => println!("- no history"),
    }
    println!();
    if let Some(a) = &answer.absence {
        println!("_(no hotspots)_");
        println!();
        print_absence(a);
        return code;
    }
    if level != LEVEL_SYMBOL {
        print_table("modules", &answer.modules, query.min_churn);
    }
    if level != LEVEL_MODULE {
        print_table("symbols", &answer.symbols, query.min_churn);
    }
    code
}

/// `3 modules`: the population the level's ranks range over (every row
/// carries it; 0 when none ranked).
fn ranked_line(rows: &[Hotspot], noun: &str) -> String {
    format!("{} {noun}", rows.first().map_or(0, |r| r.ranked))
}

/// `## <title>` and its table, or `_(none ...)_` when the level ranked nothing.
fn print_table(title: &str, rows: &[Hotspot], min_churn: u32) {
    println!("## {title}");
    println!();
    if rows.is_empty() {
        println!("_(none at --min-churn {min_churn})_");
        println!();
        return;
    }
    println!("| # | node | churn | churn rank | centrality rank | of | last change | at |");
    println!("|--:|---|--:|--:|--:|--:|---|---|");
    for (i, r) in rows.iter().enumerate() {
        let at = match (r.file.as_deref(), r.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.to_string(),
            _ => "—".to_string(),
        };
        println!(
            "| {} | `{}` | {} | {} | {} | {} | {} | {at} |",
            i + 1,
            r.qname,
            r.churn,
            r.churn_rank,
            r.centrality_rank,
            r.ranked,
            utc_date(r.last_change),
        );
    }
    println!();
}

/// Unix seconds as their UTC day, `YYYY-MM-DD`: Howard Hinnant's
/// civil-from-days over the proleptic Gregorian calendar (eras of 400 years,
/// March-based years so the leap day ends one). No locale, no wall clock.
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
    use super::utc_date;

    #[test]
    fn utc_date_known_days() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert_eq!(utc_date(1_767_225_600), "2026-01-01");
    }

    #[test]
    fn utc_date_day_edges() {
        assert_eq!(utc_date(-1), "1969-12-31");
        assert_eq!(utc_date(1_767_225_599), "2025-12-31");
        assert_eq!(utc_date(951_868_799), "2000-02-29");
        assert_eq!(utc_date(951_868_800), "2000-03-01");
        assert_eq!(utc_date(4_107_542_400), "2100-03-01");
    }
}
