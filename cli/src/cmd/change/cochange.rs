//! `glia cochange` (CC.11c) — co-change suggestions, the human surface over
//! the engine's `cochange::{cochange_multi, cochange_vs_rev}` (CC.11a /
//! CC.11b): for the files of a change, the files that usually change with
//! them in git history ("you changed X; Y usually changes with it"), each with
//! a directional confidence, its support, the antecedent file(s) the rule
//! starts from and whether any static link joins them. The natural use is a
//! pre-commit or PR step: `glia cochange . --base main`.
//!
//! Exactly one query source: files after the repo, or `--base <rev>` (the
//! working tree's change against a git rev: tracked changes, deletions, both
//! paths of a rename and untracked files). Files are repo-relative; `./` and
//! `..` parts are resolved and an absolute path under the repo (or a `--with`
//! repo) is made relative to it. A path outside every repo is a usage error,
//! never a silently unmapped file. `--min-confidence` is a decimal share in
//! [0, 1] (the engine's per mille floor: 0.3 is 300).
//!
//! Table mode prints the query, then `| file | confidence | support | because
//! | static link | source |` — confidence as `0.80 (4/5)`: of the 5 commits
//! that changed the antecedent (`because`), 4 also changed the file — and
//! then the query files no MODULE names. Every row is heuristic: co-change is
//! history, not proof of coupling; a row whose static link is `none` is the
//! case the answer is for (a blind spot, or coupling outside code). An empty
//! answer prints its absence: `no_history` (no `glia history sync` snapshot)
//! or `no_match` (nothing past the floors). `--json` prints the engine's
//! `Cochange` `{query_files, unmapped, rows, absence}`.
//!
//! Exit 0 with rows, 1 with an absence, 2 on a usage error (files and
//! `--base`, neither, `--base` with `--with` or `--no-overlay`, a path outside
//! the repo, `--min-confidence` outside [0, 1]) or a git / build failure.
//! `--base` builds the working tree once, persisting nothing.
//!
//! Fired-on marker, the engine's, once per run:
//! `[cochange-suggest] query_files=<q> unmapped=<u> candidates=<c> rows=<r> unlinked=<n> source=multi antecedents=<a> commits_scanned=<s>`.

use std::path::{Component, Path, PathBuf};

use glia_engine::cochange::{Cochange, CochangeArgs, CochangeRow, cochange_multi, cochange_vs_rev};

use crate::cmd::resolve::print_absence;
use crate::common::{build_options, generate_for};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// The changed files, repo-relative (an absolute path under the repo is
    /// made relative). One query source of two.
    files: Vec<String>,
    /// Query the working tree's change against this git rev (a branch, tag,
    /// sha or `HEAD~N`) instead of given files.
    #[arg(long, value_name = "REV")]
    base: Option<String>,
    /// Additional repos to merge in (cross-service). Repeatable. Not with
    /// `--base`.
    #[arg(long)]
    with: Vec<String>,
    /// A rule's confidence floor: the share of the antecedent's commits that
    /// also changed the file, a decimal in [0, 1].
    #[arg(long, value_name = "SHARE", default_value = "0.3", value_parser = parse_confidence)]
    min_confidence: u32,
    /// A rule's support floor: commits that changed the antecedent and the
    /// file together.
    #[arg(long, value_name = "N", default_value_t = CochangeArgs::default().min_support)]
    min_support: u32,
    /// At most N rows.
    #[arg(long, value_name = "N", default_value_t = CochangeArgs::default().top)]
    top: usize,
    /// Keep only the rows no static link joins to the query.
    #[arg(long)]
    unlinked_only: bool,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

/// `--min-confidence`: a decimal share in [0, 1] as per mille, rounded to
/// the nearest.
fn parse_confidence(s: &str) -> Result<u32, String> {
    let share: f64 = s
        .trim()
        .parse()
        .map_err(|_| format!("`{s}` is not a decimal share"))?;
    if !(0.0..=1.0).contains(&share) {
        return Err(format!("`{s}` is outside [0, 1]"));
    }
    // In [0, 1000] after the check, so the cast neither truncates nor wraps.
    Ok((share * 1000.0).round() as u32)
}

/// `p` with `.` parts dropped and `..` parts applied; `None` when a `..`
/// climbs above the start of `p` (or above `/`).
fn lexical(p: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    return None;
                }
                out.pop();
            }
            other => out.push(other),
        }
    }
    Some(out)
}

/// A relative path's parts joined by `/`; `None` for an empty one.
fn slash_joined(p: &Path) -> Option<String> {
    let parts: Vec<String> = p
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// `given` as a repo-relative path. A relative path is taken as relative to
/// the repo (its `.` / `..` parts resolved); an absolute one is made
/// relative to the first of `roots` that holds it, lexically or through
/// symlinks. `Err` for a path outside every root, or a root itself.
fn repo_relative(given: &str, roots: &[&str]) -> Result<String, String> {
    let outside = || {
        format!(
            "`{given}` is no file under the repo ({}): give paths relative to the repo root",
            roots.join(", ")
        )
    };
    let path = Path::new(given);
    if !path.is_absolute() {
        return lexical(path)
            .as_deref()
            .and_then(slash_joined)
            .ok_or_else(outside);
    }
    let mut files: Vec<PathBuf> = lexical(path).into_iter().collect();
    files.extend(std::fs::canonicalize(path));
    for root in roots {
        let mut bases: Vec<PathBuf> = std::path::absolute(root)
            .ok()
            .and_then(|r| lexical(&r))
            .into_iter()
            .collect();
        bases.extend(std::fs::canonicalize(root));
        for base in &bases {
            for file in &files {
                if let Some(rel) = file.strip_prefix(base).ok().and_then(slash_joined) {
                    return Ok(rel);
                }
            }
        }
    }
    Err(outside())
}

/// The answer for whichever query source `args` names.
fn answer(args: &Args, opts: &CochangeArgs) -> Result<Cochange, String> {
    match (&args.base, args.files.is_empty()) {
        (Some(base), true) => {
            if !args.with.is_empty() {
                return Err(
                    "--base queries the one repo's change against its git rev; it does not take --with"
                        .to_string(),
                );
            }
            if !build_options().overlay {
                return Err(
                    "--no-overlay does not apply to --base: the working tree is built with the repo's overlay"
                        .to_string(),
                );
            }
            cochange_vs_rev(&args.repo, base, opts)
        }
        (None, false) => {
            let mut roots = vec![args.repo.as_str()];
            roots.extend(args.with.iter().map(String::as_str));
            let files = args
                .files
                .iter()
                .map(|f| repo_relative(f, &roots))
                .collect::<Result<Vec<_>, _>>()?;
            let built = generate_for(&args.repo, &args.with)?;
            let mut a = cochange_multi(&built.merged, &built.repo_roots, &files, opts);
            if let Some(x) = a.absence.as_mut() {
                x.unparsed_files = built.parse_errors.len();
            }
            Ok(a)
        }
        _ => Err("give exactly one of: changed files after the repo, or --base <rev>".to_string()),
    }
}

/// A table cell: `|` escaped.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// `0.80 (4/5)`: the row's per mille confidence in hundredths, truncated as
/// the per mille is, then its support over the antecedent's commits.
fn confidence(r: &CochangeRow) -> String {
    confidence_cell(r.confidence_permille, r.support, r.antecedent_commits)
}

fn confidence_cell(permille: u32, support: u32, commits: u32) -> String {
    let hundredths = permille / 10;
    format!(
        "{}.{:02} ({support}/{commits})",
        hundredths / 100,
        hundredths % 100
    )
}

/// `` `a`, `b` `` or `none`.
fn names(v: &[String]) -> String {
    if v.is_empty() {
        return "none".to_string();
    }
    v.iter()
        .map(|s| format!("`{s}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_table(args: &Args, a: &Cochange) {
    let n = a.query_files.len();
    let files = if n == 1 { "file" } else { "files" };
    println!("# glia cochange `{}`", args.repo);
    println!();
    match &args.base {
        Some(base) => println!(
            "- query: the working tree's change against `{base}` ({n} {files}): {}",
            names(&a.query_files)
        ),
        None => println!("- query: {} ({n} {files})", names(&a.query_files)),
    }
    let unlinked = a.rows.iter().filter(|r| r.note.is_some()).count();
    println!(
        "- suggestions: {} ({unlinked} with no static link); tier heuristic: co-change is history, not proof of coupling",
        a.rows.len()
    );
    println!();
    if let Some(x) = &a.absence {
        println!("_(no co-change suggestions)_");
        println!();
        print_absence(x);
    } else {
        println!("| file | confidence | support | because | static link | source |");
        println!("|---|--:|--:|---|---|---|");
        for r in &a.rows {
            println!(
                "| {} | {} | {} | {} | {} | {} |",
                cell(&r.file),
                confidence(r),
                r.support,
                cell(&r.antecedent.join(", ")),
                r.link,
                r.source
            );
        }
        if let Some(note) = a.rows.iter().find_map(|r| r.note.as_deref()) {
            println!();
            println!("> static link `none`: {note}");
        }
    }
    println!();
    println!("- unmapped: {}", names(&a.unmapped));
}

pub(crate) fn run(args: Args) -> i32 {
    let mut opts = CochangeArgs::default();
    opts.min_confidence_permille = args.min_confidence;
    opts.min_support = args.min_support;
    opts.top = args.top;
    opts.unlinked_only = args.unlinked_only;
    let a = match answer(&args, &opts) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let code = i32::from(a.absence.is_some());
    if args.json {
        println!("{}", serde_json::to_string(&a).unwrap_or_default());
    } else {
        print_table(&args, &a);
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_parses_to_per_mille() {
        assert_eq!(
            parse_confidence("0.3"),
            Ok(CochangeArgs::default().min_confidence_permille),
            "the flag's default is the engine's"
        );
        let ok: Vec<u32> = ["0", "1", "1.0", "0.25", "0.3333", " 0.5 "]
            .iter()
            .map(|s| parse_confidence(s).expect(s))
            .collect();
        assert_eq!(ok, [0, 1000, 1000, 250, 333, 500]);
        for bad in ["1.5", "-0.1", "high", "NaN", "inf", ""] {
            assert!(parse_confidence(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn paths_are_made_repo_relative() {
        let roots = ["/work/repo", "/work/other"];
        let rel = |g: &str| repo_relative(g, &roots);
        assert_eq!(rel("svc/a.py").as_deref(), Ok("svc/a.py"));
        assert_eq!(rel("./svc/./a.py").as_deref(), Ok("svc/a.py"));
        assert_eq!(rel("svc/x/../a.py").as_deref(), Ok("svc/a.py"));
        assert_eq!(rel("/work/repo/svc/a.py").as_deref(), Ok("svc/a.py"));
        assert_eq!(rel("/work/repo/./svc/a.py").as_deref(), Ok("svc/a.py"));
        assert_eq!(rel("/work/other/b.go").as_deref(), Ok("b.go"));
        for bad in [
            "../a.py",
            "svc/../../a.py",
            "/work/elsewhere/a.py",
            "/work/repo",
            ".",
        ] {
            let e = rel(bad).expect_err(bad);
            assert!(e.contains("is no file under the repo"), "{bad}: {e}");
        }
    }

    #[test]
    fn confidence_cell_truncates_to_hundredths() {
        let cells: Vec<String> = [800, 1000, 444, 66, 999]
            .iter()
            .map(|&p| confidence_cell(p, 4, 5))
            .collect();
        assert_eq!(
            cells,
            [
                "0.80 (4/5)",
                "1.00 (4/5)",
                "0.44 (4/5)",
                "0.06 (4/5)",
                "0.99 (4/5)"
            ]
        );
    }
}
