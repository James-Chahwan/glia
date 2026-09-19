//! `glia why` (LE.5) — every edge from one node to another with the extractor
//! or resolver that emitted it, its call site and confidence, tiered fact /
//! derived / heuristic: the human surface over the engine's `why::why_edge`.
//! With no direct edge it prints the witness path (a shortest carry path,
//! each hop explained the same way) and the LD.8a absence block. `--json` is
//! the whole `WhyAnswer`. Exit 0 when an edge was found, 1 when none was
//! (path or not), 2 on a build failure, an unknown node or an unknown
//! category.

use repo_graph_engine::why::{EdgeWhy, why_edge};

use crate::cmd::resolve::print_absence;
use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// The source node: a qname (`shop::a::place`, `shop.a.place`) or an
    /// exact simple name (every node with that name, up to 8).
    from: String,
    /// The target node, the same way.
    to: String,
    /// Only edges of this category (an edge-category name, any case, e.g.
    /// CALLS or HTTP_CALLS).
    #[arg(long)]
    category: Option<String>,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Emit JSON instead of a table.
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
    let mut answer = match why_edge(
        &result.merged,
        &args.from,
        &args.to,
        args.category.as_deref(),
    ) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    let code = if answer.found { 0 } else { 1 };
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return code;
    }
    println!("# glia why `{}` -> `{}`", args.from.trim(), args.to.trim());
    println!();
    // Several nodes on a side: name each row's ends.
    let ends = answer.from_nodes.len() > 1 || answer.to_nodes.len() > 1;
    if answer.found {
        print_rows(&answer.edges, ends, false);
    } else {
        println!("_(no direct edge)_");
        if !answer.path.is_empty() {
            println!();
            println!(
                "witness path ({} hops, a connection, not the reason):",
                answer.path.len()
            );
            println!();
            print_rows(&answer.path, true, true);
        }
        if let Some(a) = &answer.absence {
            println!();
            print_absence(a);
        }
    }
    if let Some(note) = &answer.note {
        println!();
        println!("> note: {note}");
    }
    code
}

/// `| category | confidence | tier | emitter | rule | site | note |`, led by
/// `| hop |` for a path and `| from | to |` when `ends`.
fn print_rows(rows: &[EdgeWhy], ends: bool, hops: bool) {
    let mut head = String::from("|");
    let mut rule = String::from("|");
    if hops {
        head.push_str(" hop |");
        rule.push_str("--:|");
    }
    if ends {
        head.push_str(" from | to |");
        rule.push_str("---|---|");
    }
    head.push_str(" category | confidence | tier | emitter | rule | site | note |");
    rule.push_str("---|---|---|---|---|---|---|");
    println!("{head}");
    println!("{rule}");
    for (i, r) in rows.iter().enumerate() {
        let mut line = String::from("|");
        if hops {
            line.push_str(&format!(" {} |", i + 1));
        }
        if ends {
            line.push_str(&format!(" `{}` | `{}` |", r.from_qname, r.to_qname));
        }
        let site = match &r.site {
            Some(s) => match s.line {
                Some(l) => format!("{}:{l}", s.file),
                None => s.file.clone(),
            },
            None => "—".to_string(),
        };
        line.push_str(&format!(
            " {} | {} | {} | {} | {} | {site} | {} |",
            r.category,
            r.confidence,
            r.tier,
            r.emitter.as_deref().unwrap_or("—"),
            r.rule.as_deref().unwrap_or("—"),
            r.note.as_deref().unwrap_or("")
        ));
        println!("{line}");
    }
}
