//! `glia contracts` (A12.3) — the human surface over A12.2's
//! message_contracts: per queue topic, producer vs consumer message type.

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
    if json {
        println!("{}", serde_json::to_string(&rows).unwrap_or_default());
        return 0;
    }
    println!("# glia contracts `{repo}`");
    println!();
    if rows.is_empty() {
        let what = if mismatch_only {
            "mismatches"
        } else {
            "queue topics"
        };
        println!("_(no {what})_");
        return 0;
    }
    type Side = Option<repo_graph_engine::MessageContractSide>;
    let side_type = |s: &Side| match s {
        None => "—".to_string(),
        Some(s) => s
            .message_type
            .as_ref()
            .map_or("_untyped_".to_string(), |t| format!("`{t}`")),
    };
    // Paths are relative to each repo's own root, so a merge names the repo.
    let side_loc = |s: &Side| {
        let s = s.as_ref()?;
        let f = s.file.as_deref()?;
        let loc = s.line.map_or(f.to_string(), |l| format!("{f}:{l}"));
        Some(match result.repo_labels.get(&s.repo_id) {
            Some(label) if !with.is_empty() => format!("{label}/{loc}"),
            _ => loc,
        })
    };
    println!("| topic | producer type | consumer type | status | where |");
    println!("|---|---|---|---|---|");
    for r in &rows {
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
    0
}
