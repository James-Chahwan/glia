//! `glia contracts` (A12.3) — the human surface over A12.2's
//! message_contracts: per queue topic, producer vs consumer message type.
//!
//! `--fields` (LE.10d) extends it past the type-name match with LE.10c's
//! field-level diff (`contract_fields::contract_fields`): after the topic
//! table, one row per producer → consumer pairing (schema copy, topic, AsyncAPI
//! channel, OpenAPI route vs Pact) with its verdict and declared changes.
//! Without `--fields` the output is exactly the topic table it always was.

use repo_graph_engine::contract_fields::{FieldChange, FieldDiffRow, FieldSide};

use crate::common::generate_for;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Keep only rows whose status is `mismatch`.
    #[arg(long)]
    mismatch_only: bool,
    /// Also diff the declared fields of each contract pairing (schema copies,
    /// queue topics, AsyncAPI channels, OpenAPI routes vs Pact): a second
    /// table after the topic table; with --json, an object {topics, fields}.
    #[arg(long)]
    fields: bool,
    /// With --fields: keep only field rows whose status is `breaking`.
    #[arg(long, requires = "fields")]
    breaking_only: bool,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let with = args.with.as_slice();
    let (mismatch_only, json) = (args.mismatch_only, args.json);
    eprintln!("[contracts] surface=cli repos={}", 1 + with.len());
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    // Pairing, verdicts, sorting and the `[contracts] topics=…` line all live
    // in the engine; this is transport + rendering only.
    let mut rows = repo_graph_engine::message_contracts(&result.merged);
    if mismatch_only {
        rows.retain(|r| r.status == "mismatch");
    }
    // The field diff prints its own `[contract-fields] pairs=…` line (and,
    // through message_contracts, a second `[contracts] topics=…`) before the
    // surface marker below.
    let fields = args.fields.then(|| {
        let mut f = repo_graph_engine::contract_fields::contract_fields(&result.merged);
        if args.breaking_only {
            f.retain(|r| r.status == "breaking");
        }
        eprintln!("[contract-fields] surface=cli rows={}", f.len());
        f
    });
    if json {
        let Some(fields) = fields else {
            println!("{}", serde_json::to_string(&rows).unwrap_or_default());
            return 0;
        };
        // Written by hand so `topics` comes first: serde_json's map (no
        // `preserve_order` in this workspace) would sort `fields` ahead of it.
        return match (serde_json::to_string(&rows), serde_json::to_string(&fields)) {
            (Ok(t), Ok(f)) => {
                println!("{{\"topics\":{t},\"fields\":{f}}}");
                0
            }
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("error: {e}");
                2
            }
        };
    }
    // Paths are relative to each repo's own root, so a merge names the repo.
    let located = |repo_id: u64, file: Option<&str>, line: Option<i64>| {
        let f = file?;
        let loc = line.map_or(f.to_string(), |l| format!("{f}:{l}"));
        Some(match result.repo_labels.get(&repo_id) {
            Some(label) if !with.is_empty() => format!("{label}/{loc}"),
            _ => loc,
        })
    };
    print_topics(repo, &rows, mismatch_only, &located);
    if let Some(fields) = fields {
        print_fields(&fields, args.breaking_only, &located);
    }
    0
}

type Locate<'a> = dyn Fn(u64, Option<&str>, Option<i64>) -> Option<String> + 'a;

/// The topic table — the whole of `glia contracts` before `--fields`.
fn print_topics(
    repo: &str,
    rows: &[repo_graph_engine::MessageContractRow],
    mismatch_only: bool,
    located: &Locate<'_>,
) {
    println!("# glia contracts `{repo}`");
    println!();
    if rows.is_empty() {
        let what = if mismatch_only {
            "mismatches"
        } else {
            "queue topics"
        };
        println!("_(no {what})_");
        return;
    }
    type Side = Option<repo_graph_engine::MessageContractSide>;
    let side_type = |s: &Side| match s {
        None => "—".to_string(),
        Some(s) => s
            .message_type
            .as_ref()
            .map_or("_untyped_".to_string(), |t| format!("`{t}`")),
    };
    let side_loc = |s: &Side| {
        let s = s.as_ref()?;
        located(s.repo_id, s.file.as_deref(), s.line)
    };
    println!("| topic | producer type | consumer type | status | where |");
    println!("|---|---|---|---|---|");
    for r in rows {
        let topic = if r.topic_is_tag {
            format!("`{}` (tag)", r.topic)
        } else {
            format!("`{}`", r.topic)
        };
        let mut status = match r.status {
            "unknown" => r.status.to_string(),
            s => format!("{s} ({})", r.confidence),
        };
        if let Some(n) = r.note {
            status.push_str(&format!(" — _{n}_"));
        }
        let loc = match (side_loc(&r.producer), side_loc(&r.consumer)) {
            (Some(p), Some(c)) => format!("{p} → {c}"),
            (Some(x), None) | (None, Some(x)) => x,
            _ => "—".to_string(),
        };
        println!(
            "| {topic} | {} | {} | {status} | {loc} |",
            side_type(&r.producer),
            side_type(&r.consumer)
        );
    }
}

/// The `--fields` table: one row per pairing, its changes in one cell.
fn print_fields(rows: &[FieldDiffRow], breaking_only: bool, located: &Locate<'_>) {
    println!();
    println!("## field contracts");
    println!();
    if rows.is_empty() {
        let what = if breaking_only {
            "breaking field contracts"
        } else {
            "field contracts"
        };
        println!("_(no {what})_");
        return;
    }
    // A side is where it is declared (the 1-based line the engine located) and
    // the format that declares it; a side with no file falls back to its qname.
    let side = |s: &FieldSide| {
        let at = located(s.repo_id, s.file.as_deref(), s.line)
            .unwrap_or_else(|| format!("`{}`", s.qname));
        if s.format.is_empty() {
            cell(&at)
        } else {
            cell(&format!("{at} ({})", s.format))
        }
    };
    println!("| pairing | key | producer | consumer | status | changes |");
    println!("|---|---|---|---|---|---|");
    for r in rows {
        let mut status = r.status.to_string();
        if let Some(n) = r.note {
            status.push_str(&format!(" — _{n}_"));
        }
        let changes = if r.changes.is_empty() {
            "—".to_string()
        } else {
            let all: Vec<String> = r.changes.iter().map(change).collect();
            cell(&all.join("; "))
        };
        println!(
            "| {} | {} | {} | {} | {status} | {changes} |",
            r.pairing,
            cell(&format!("`{}`", r.key)),
            side(&r.producer),
            side(&r.consumer),
        );
    }
}

/// `total_cents: int64 -> int32 (proto_wire_type)`, a non-`fields` section
/// (`request`, `response:<code>`, `payload[:<name>]`) in front, a side that
/// does not declare the field as `—`, a breaking change in bold.
fn change(c: &FieldChange) -> String {
    let side = |s: &Option<String>| s.clone().unwrap_or_else(|| "—".to_string());
    let section = if c.section == "fields" {
        String::new()
    } else {
        format!("[{}] ", c.section)
    };
    let text = format!(
        "{section}{}: {} -> {} ({})",
        c.field,
        side(&c.producer),
        side(&c.consumer),
        c.rule
    );
    if c.breaking {
        format!("**{text}**")
    } else {
        text
    }
}

/// A markdown table cell: a `|` in a type or a path must not split the row.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}
