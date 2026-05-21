//! synth_pr_hint — P4.2 (Option 2, cycle 1.1).
//!
//! Surfaces SWE-bench parquet's `hints_text` field as a low-score directive
//! channel. The hints field contains discussion comments / PR commentary
//! from the issue thread — sometimes a developer's plain-English
//! explanation of what the fix should do, sometimes a "closing" remark.
//!
//! Score is intentionally low (this is BACKGROUND context, not
//! authoritative). When the hint is substantive, it gives the model a
//! human-language plan; when it's dismissive ("closing", "thanks"), it's
//! filtered out.
//!
//! The CLI signature mirrors the other synth bins so synth_directive can
//! spawn it uniformly. Reads the hint text directly from --hints-text
//! (the orchestrator extracts the parquet field once and passes the
//! string in via file). Cannot read parquet from Rust without deps.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(about = "PR/issue hint channel from SWE-bench hints_text (P4.2, cycle 1.1)")]
struct Args {
    /// File containing the raw hints_text. Optional — if absent OR empty,
    /// the bin writes an empty directive.
    #[arg(long)]
    hints_text: Option<PathBuf>,

    #[arg(long)]
    text_out: PathBuf,

    /// Minimum hint character count to consider substantive. Hints
    /// shorter than this are dropped (filters one-liner "closing" /
    /// "thanks" remarks).
    #[arg(long, default_value_t = 50)]
    min_chars: usize,

    /// Reserved for CLI uniformity with other synth bins.
    #[arg(long, default_value = "seeds")]
    repo_canonical: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let hints = match &args.hints_text {
        Some(p) if p.exists() => std::fs::read_to_string(p).unwrap_or_default(),
        _ => String::new(),
    };
    let trimmed = hints.trim();
    let body = if trimmed.is_empty() {
        write_empty(&args.text_out, "no hints_text supplied")?;
        return Ok(());
    } else if trimmed.len() < args.min_chars {
        write_empty(&args.text_out, &format!(
            "hints_text below min_chars threshold ({}/{})", trimmed.len(), args.min_chars,
        ))?;
        return Ok(());
    } else {
        trimmed
    };

    // Filter common dismissive-only patterns. Pattern: a hint that
    // contains ONLY words like {closing, close, thanks, duplicate, see,
    // works for me, won't fix} is signal-poor.
    let lower = body.to_lowercase();
    let dismissive_keywords = ["closing", "won't fix", "wontfix", "duplicate", "thanks", "ok thanks"];
    let body_word_count = body.split_whitespace().count();
    let only_dismissive = body_word_count <= 10 &&
        dismissive_keywords.iter().any(|k| lower.contains(k));
    if only_dismissive {
        write_empty(&args.text_out, "hints_text is short + dismissive")?;
        return Ok(());
    }

    let mut s = String::new();
    s.push_str("\n## Developer hints (issue thread / PR discussion)\n\n");
    s.push_str(
        "Comments from the issue thread. These are human-written context — \
         sometimes a fix plan, sometimes background discussion. Use only \
         if the content describes the fix shape; ignore if it's just \
         conversation.\n\n",
    );
    // Cap to ~2000 chars to keep prefix bounded.
    let body_capped = if body.len() > 2000 { &body[..2000] } else { body };
    s.push_str("```\n");
    s.push_str(body_capped);
    s.push_str("\n```\n");

    if let Some(parent) = args.text_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.text_out, s.as_bytes())?;
    eprintln!(
        "[synth_pr_hint] wrote directive: {} ({} chars in, {} chars out)",
        args.text_out.display(),
        trimmed.len(),
        s.len(),
    );
    Ok(())
}

fn write_empty(path: &PathBuf, reason: &str) -> Result<()> {
    let s = format!(
        "\n## Developer hints (issue thread / PR discussion)\n\n\
         (no hints directive: {reason})\n",
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, s.as_bytes())?;
    eprintln!("[synth_pr_hint] wrote empty directive: {} ({reason})", path.display());
    Ok(())
}
