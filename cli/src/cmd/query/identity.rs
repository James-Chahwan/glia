//! `glia identity` (DD.15): the human surface over the engine's
//! `identity::identity_classes` (DD.8). Every identity class the build
//! found: the FACT / DERIVED classes every answer collapses to one row (a
//! package two repos depend on, one message, one contract op, one SCIP
//! symbol), then the HEURISTIC same-as candidates no answer collapses
//! (vendored copies, overlay guesses; Q18). Each class names its rules and
//! tier, says whether its copies diverged and how many distinct contents
//! they hold, and lists every member located and labelled by repo, the
//! display member (the row answers show) first.
//!
//! `--tier` and `--rule` take the engine's names exactly (clap rejects any
//! other, listing them, before the build runs); `--diverged-only` keeps the
//! classes whose copies differ; `--limit` (default 50, `0` = every class)
//! cuts the listed classes, never the counts. `--json` is the whole
//! `IdentityReport` `{counts, classes, omitted, absence}`. Exit 0 with
//! classes; 1 with none, the absence saying why; 2 on a build or usage
//! error. The engine's `[identity] report classes=` stderr line is the
//! fired_on marker.

use glia_code_domain::same_as;
use glia_engine::identity::{IdentityClass, IdentityQuery, IdentityReport, identity_classes};

use crate::cmd::resolve::print_absence;
use crate::common::generate_for;

/// Classes listed when `--limit` is not given.
const DEFAULT_LIMIT: usize = 50;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (identity classes span repos). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Only classes of this tier: fact / derived collapse in answers,
    /// heuristic are same-as candidates.
    #[arg(long, value_parser = ["fact", "derived", "heuristic"])]
    tier: Option<String>,
    /// Only classes this identity rule joined.
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(same_as::RULES))]
    rule: Option<String>,
    /// Only classes whose members' contents differ.
    #[arg(long)]
    diverged_only: bool,
    /// Classes to list; 0 lists every class. The counts are never cut.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_LIMIT)]
    limit: usize,
    /// Emit the report as JSON.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut q = IdentityQuery::default();
    q.tier = args.tier.clone();
    q.rule = args.rule.clone();
    q.diverged_only = args.diverged_only;
    q.limit = Some(args.limit);
    q.repo_labels = result.repo_labels.clone();
    let mut report = match identity_classes(&result.merged, &q) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if let Some(a) = report.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    let code = if report.absence.is_some() { 1 } else { 0 };
    if args.json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
        return code;
    }
    print!("{}", heading(&args.repo, args.with.len(), &report));
    if let Some(a) = &report.absence {
        println!("_(no identity classes)_");
        println!();
        print_absence(a);
        return code;
    }
    print!("{}", body(&report));
    code
}

/// `# glia identity <repo>[ (+<n> repos)]`, the counts line and the
/// `rules:` line (each rule that joined a counted class, with its count).
fn heading(repo: &str, others: usize, r: &IdentityReport) -> String {
    let mut out = String::new();
    let more = match others {
        0 => String::new(),
        1 => " (+1 repo)".to_string(),
        n => format!(" (+{n} repos)"),
    };
    out.push_str(&format!("# glia identity {repo}{more}\n\n"));
    let c = &r.counts;
    out.push_str(&format!(
        "classes={} members={} fact={} derived={} heuristic={} diverged={}\n",
        c.classes, c.members, c.fact, c.derived, c.heuristic, c.diverged
    ));
    let by_rule: Vec<String> = c
        .by_rule
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(rule, n)| format!("{rule}={n}"))
        .collect();
    if !by_rule.is_empty() {
        out.push_str(&format!("rules: {}\n", by_rule.join(" ")));
    }
    out.push('\n');
    out
}

/// The listed classes in two sections (collapsed, then candidates; a
/// section with no listed class is left out), then the omitted line.
fn body(r: &IdentityReport) -> String {
    let mut out = String::new();
    let sections = [
        ("collapsed in answers (fact / derived)", true),
        ("same-as candidates (heuristic, never collapsed)", false),
    ];
    for (title, collapses) in sections {
        let classes: Vec<&IdentityClass> = r
            .classes
            .iter()
            .filter(|k| k.collapses == collapses)
            .collect();
        if classes.is_empty() {
            continue;
        }
        out.push_str(&format!("## {title}\n\n"));
        for k in classes {
            out.push_str(&class_lines(k));
        }
        out.push('\n');
    }
    if r.omitted > 0 {
        out.push_str(&format!("… {} more (--limit 0 for all)\n", r.omitted));
    }
    out
}

/// `- <key> [<rules>] <tier>[ diverged, <v> variants]`, then one indented
/// line per member: `<repo> <at> <kind> <qname>[ (variant <i>)]`, the variant
/// shown only when the members hold two or more contents.
fn class_lines(k: &IdentityClass) -> String {
    let rules = if k.rules.is_empty() {
        "no rule".to_string()
    } else {
        k.rules.join(", ")
    };
    let mut s = format!("- {} [{rules}] {}", k.key, k.tier);
    if k.diverged {
        s.push_str(&format!(
            " diverged, {} {}",
            k.variants,
            if k.variants == 1 {
                "variant"
            } else {
                "variants"
            }
        ));
    }
    s.push('\n');
    for m in &k.members {
        s.push_str(&format!(
            "  {} {} {} {}",
            m.repo,
            at(m.file.as_deref(), m.line),
            m.kind,
            m.qname
        ));
        if k.variants > 1
            && let Some(v) = m.variant
        {
            s.push_str(&format!(" (variant {v})"));
        }
        s.push('\n');
    }
    s
}

/// `file:line`, the file alone, or `—` for an unlocated member.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::at;

    #[test]
    fn at_cell() {
        assert_eq!(at(Some("a.ts"), Some(3)), "a.ts:3");
        assert_eq!(at(Some("package.json"), None), "package.json");
        assert_eq!(at(None, None), "—");
    }
}
