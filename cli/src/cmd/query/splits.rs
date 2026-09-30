//! `glia splits` (CD.2d) — the human surface over the engine's
//! `splits::splits` (CD.2b / CD.2c): where a scope splits into services at the
//! least coupling. Without `--from` / `--to` it is the global mode: the module
//! (or community) quotient bisected by Stoer-Wagner above a balance floor
//! (`--min-share`), recursive to `--parts` parts. With both it is the anchored
//! `st` mode: the minimum cut that separates the `--from` side (part 0) from
//! the `--to` side, each a path, a project label or a node.
//!
//! The report: a heading `Suggested cut (heuristic): <mode>, <quotient>
//! quotient, cut weight W (global min G, balanced yes|no)`; the parts table
//! (id, label, nodes, units, `glia arch` services, entrypoints) and each
//! part's modules; the cut edges table (from -> to, the parts, category,
//! weight, `file:line` at the edge's evidence site); the blockers, `Shared
//! writes` (a data entity two or more parts write) and `Cycles between parts`
//! (parts that depend on each other both ways), each `_(none)_` when empty;
//! and one line per part against `glia arch` (`part 1: splits_service
//! backend`). Every cut is tier heuristic: a suggestion, never a verdict.
//!
//! Exit 0 with a cut; 1 with none, the engine's absence saying why (fewer than
//! two units, too many units, a side naming nothing or both sides sharing a
//! unit); 2 on a build error or a refused argument: `--from` without `--to`
//! (or the reverse), `--parts` outside 2..=8, `--min-share` outside [0, 0.5],
//! or a `--quotient` other than module / community. `--json` is the whole
//! `SplitAnswer`. The engine's `[splits] mode=... surface=cli` stderr line is
//! the fired_on marker.

use glia_engine::Located;
use glia_engine::splits::{
    CutEdge, DEFAULT_MIN_SHARE, DEFAULT_PARTS, DEFAULT_SEED, MAX_PARTS, PartCycle, SharedWrite,
    SplitAnswer, SplitArgs, SplitPart, splits,
};

use crate::cmd::resolve::print_absence;
use crate::common::generate_for;

/// [`SplitArgs::surface`] for the marker.
const SURFACE: &str = "cli";
/// `--min-share` at most: the smaller side of a bisection holds at most half.
const MAX_MIN_SHARE: f64 = 0.5;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Split only the nodes under this repo-relative path or project label
    /// (see `glia projects`); edges leaving it are not in the view.
    #[arg(long)]
    scope: Option<String>,
    /// Parts wanted, 2 to 8 (the global mode; `--from` / `--to` always cut in
    /// two).
    #[arg(
        long,
        default_value_t = DEFAULT_PARTS,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(2..=MAX_PARTS as u64),
    )]
    parts: usize,
    /// What a part is made of: `module` (each node's enclosing module) or
    /// `community` (its seeded Leiden community).
    #[arg(long, default_value = "module", value_parser = ["module", "community"])]
    quotient: String,
    /// The smaller side's least share of a part's nodes, 0 to 0.5: a cut
    /// below it is not balanced.
    #[arg(long, default_value_t = DEFAULT_MIN_SHARE)]
    min_share: f64,
    /// The source side of an anchored cut (part 0): a path, a project label,
    /// or a node's qname or name. Needs `--to`.
    #[arg(long)]
    from: Option<String>,
    /// The sink side of an anchored cut, read as `--from` is. Needs `--from`.
    #[arg(long)]
    to: Option<String>,
    /// Seeds the community quotient.
    #[arg(long, default_value_t = DEFAULT_SEED)]
    seed: u64,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

/// Why the arguments are refused before any build, or `None`.
fn args_error(a: &Args) -> Option<String> {
    match (&a.from, &a.to) {
        (Some(_), None) => {
            return Some("--from needs --to: the sink is not set".to_string());
        }
        (None, Some(_)) => {
            return Some("--to needs --from: the source is not set".to_string());
        }
        _ => {}
    }
    min_share_error(a.min_share)
}

/// Why `--min-share s` is refused, or `None` for a number in [0, 0.5].
fn min_share_error(s: f64) -> Option<String> {
    if (0.0..=MAX_MIN_SHARE).contains(&s) {
        None
    } else {
        Some(format!(
            "--min-share must be a number in [0, {MAX_MIN_SHARE}], got {s}"
        ))
    }
}

pub(crate) fn run(args: Args) -> i32 {
    if let Some(e) = args_error(&args) {
        eprintln!("error: {e}");
        return 2;
    }
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut query = SplitArgs::default();
    query.scope = args.scope;
    query.parts = args.parts;
    query.quotient = args.quotient;
    query.min_share = args.min_share;
    query.seed = args.seed;
    query.source = args.from;
    query.sink = args.to;
    query.surface = SURFACE;
    let mut answer = splits(&result.merged, &result.repo_labels, &query);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    let code = if answer.absence.is_some() { 1 } else { 0 };
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return code;
    }

    println!("# glia splits `{}`", args.repo);
    println!();
    if let Some(a) = &answer.absence {
        println!(
            "No cut: {}, {} quotient, {} unit(s)",
            answer.mode, answer.quotient, answer.units
        );
        println!();
        print_absence(a);
        return code;
    }
    println!("{}", heading(&answer));
    println!();
    println!(
        "- {} units, {} parts, {} cut edges",
        answer.units,
        answer.parts.len(),
        answer.cut_edges_total
    );
    println!();
    print_parts(&answer.parts);
    print_cut_edges(&answer);
    print_shared_writes(&answer.shared_writes);
    print_cycles(&answer.cycles);
    println!("## Against glia arch");
    println!();
    for d in &answer.arch {
        let services = if d.services.is_empty() {
            String::new()
        } else {
            format!(" {}", d.services.join(", "))
        };
        println!("- part {}: {}{services}", d.part, d.verdict);
    }
    code
}

/// `Suggested cut (heuristic): global, module quotient, cut weight 10
/// (global min 5, balanced yes)`.
fn heading(a: &SplitAnswer) -> String {
    format!(
        "Suggested cut ({}): {}, {} quotient, cut weight {} (global min {}, balanced {})",
        a.tier,
        a.mode,
        a.quotient,
        a.cut_weight,
        a.global_min_weight,
        if a.balanced { "yes" } else { "no" },
    )
}

fn print_parts(parts: &[SplitPart]) {
    println!("## Parts");
    println!();
    println!("| part | label | nodes | units | services | entries |");
    println!("|--:|---|--:|--:|---|--:|");
    for p in parts {
        println!(
            "| {} | `{}` | {} | {} | {} | {} |",
            p.id,
            p.label,
            p.nodes,
            p.units,
            histogram(&p.services),
            p.entries,
        );
    }
    println!();
    for p in parts {
        let listed = p
            .modules
            .iter()
            .map(|m| format!("`{m}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let listed = if listed.is_empty() {
            "—".to_string()
        } else {
            listed
        };
        println!("- part {} modules: {listed}", p.id);
    }
    println!();
}

fn print_cut_edges(a: &SplitAnswer) {
    println!("## Cut edges");
    println!();
    if a.cut_edges.is_empty() {
        println!("_(none)_");
        println!();
        return;
    }
    println!("| from -> to | parts | category | weight | at |");
    println!("|---|---|---|--:|---|");
    for c in &a.cut_edges {
        println!(
            "| `{}` -> `{}` | {} -> {} | {} | {} | {} |",
            c.from_qname,
            c.to_qname,
            c.from_part,
            c.to_part,
            c.category,
            c.weight,
            at(c.file.as_deref(), c.line),
        );
    }
    println!();
    if a.cut_edges.len() < a.cut_edges_total {
        println!(
            "_({} of {} cut edges listed, heaviest first)_",
            a.cut_edges.len(),
            a.cut_edges_total
        );
        println!();
    }
}

fn print_shared_writes(rows: &[SharedWrite]) {
    println!("## Shared writes");
    println!();
    if rows.is_empty() {
        println!("_(none)_");
        println!();
        return;
    }
    println!("| entity | kind | parts | modes | writers | tier |");
    println!("|---|---|---|---|---|---|");
    for w in rows {
        println!(
            "| {} | {} | {} | {} | {} | {} |",
            located(&w.entity),
            w.kind,
            ids(&w.parts),
            modes(&w.modes),
            writers(&w.writers, w.writers_total),
            w.tier,
        );
    }
    println!();
}

fn print_cycles(rows: &[PartCycle]) {
    println!("## Cycles between parts");
    println!();
    if rows.is_empty() {
        println!("_(none)_");
        println!();
        return;
    }
    println!("| parts | witness | tier |");
    println!("|---|---|---|");
    for c in rows {
        let witness = c
            .witness
            .iter()
            .map(witness_edge)
            .collect::<Vec<_>>()
            .join("; ");
        println!("| {} | {witness} | {} |", ids(&c.parts), c.tier);
    }
    println!();
}

/// `0 -> 1 `a` -> `b` CALLS orders/api.py:12`: one direction of a cycle.
fn witness_edge(c: &CutEdge) -> String {
    format!(
        "{} -> {} `{}` -> `{}` {} {}",
        c.from_part,
        c.to_part,
        c.from_qname,
        c.to_qname,
        c.category,
        at(c.file.as_deref(), c.line)
    )
}

/// `` `qname` file:line ``, or the qname alone for an unlocated node (a
/// data entity has no site of its own).
fn located(l: &Located) -> String {
    match l.file {
        Some(_) => format!("`{}` {}", l.qname, at(l.file.as_deref(), l.line)),
        None => format!("`{}`", l.qname),
    }
}

/// The listed writers, and `+N more` when `total` exceeds them.
fn writers(listed: &[Located], total: usize) -> String {
    let mut cell = listed.iter().map(located).collect::<Vec<_>>().join("; ");
    if total > listed.len() {
        if !cell.is_empty() {
            cell.push_str("; ");
        }
        cell.push_str(&format!("+{} more", total - listed.len()));
    }
    if cell.is_empty() {
        "—".to_string()
    } else {
        cell
    }
}

/// `0 write, 1 read_write`.
fn modes(m: &[(u32, &str)]) -> String {
    m.iter()
        .map(|(p, mode)| format!("{p} {mode}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `0, 1`.
fn ids(parts: &[u32]) -> String {
    parts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `orders 14, util 2`, or `—` for none.
fn histogram(counts: &[(String, usize)]) -> String {
    if counts.is_empty() {
        return "—".to_string();
    }
    counts
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `file:line`, the file alone, or `—` for an unlocated edge or node.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{at, histogram, ids, min_share_error, modes};

    #[test]
    fn min_share_is_a_number_in_zero_to_a_half() {
        for good in [0.0, 0.1, 0.5] {
            assert_eq!(min_share_error(good), None, "{good}");
        }
        for bad in [-0.1, 0.51, f64::NAN, f64::INFINITY] {
            assert!(min_share_error(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn cells() {
        assert_eq!(at(Some("a.py"), Some(3)), "a.py:3");
        assert_eq!(at(Some("a.py"), None), "a.py");
        assert_eq!(at(None, Some(3)), "—");
        assert_eq!(
            histogram(&[("orders".to_string(), 14), ("util".to_string(), 2)]),
            "orders 14, util 2"
        );
        assert_eq!(histogram(&[]), "—");
        assert_eq!(ids(&[0, 1]), "0, 1");
        assert_eq!(
            modes(&[(0, "write"), (1, "read_write")]),
            "0 write, 1 read_write"
        );
    }
}
