//! synth_directive — multi-channel directive composer (Bundle B4).
//!
//! Cycle 0.6, 2026-05-21. The cycle 0.5 A + 0.5 D results pinned the lever:
//! structural form × graph-derived NAMED content, multiplied. Three channels
//! produce graph-derived NAMED content:
//!
//!   1. synth_traceback_target — Python traceback frames (high precision; ~1/7
//!      cycle_loop_set has a parseable traceback)
//!   2. synth_test_expectation — test_patch identifiers (broad: ~7/7)
//!   3. synth_prose_mention    — backtick + CamelCase issue prose (varies)
//!
//! This bin runs the three as subprocesses, scores each emitted directive
//! block, composes them into a single ordered directive, and writes the
//! result to --text-out. Replaces the Python string-prepending logic at
//! run_instance.py:750-761 with a Rust orchestrator.
//!
//! Scoring rubric (uplift-per-line, heuristic):
//!   + 4 per resolved-target bullet line
//!   + 2 if the directive contains "Edit ONE" or "PRIMARY"
//!   + 1 if traceback exists (highest precision)
//!   - 1 per "(no ..." empty-directive line
//!
//! Channels with score <= 0 are dropped. Highest score becomes the PRIMARY
//! block (verbatim); others are appended as supporting "additional signals".

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(about = "Compose traceback + test_expectation + prose_mention into one directive (B4)")]
struct Args {
    /// Repo source dir — forwarded to each child synth bin's --src.
    #[arg(long)]
    src: PathBuf,

    /// SWE-bench issue.txt — feeds synth_traceback_target and synth_prose_mention.
    #[arg(long)]
    issue: PathBuf,

    /// SWE-bench test_patch.patch — feeds synth_test_expectation. Optional;
    /// when missing the test_expectation channel is skipped.
    #[arg(long)]
    test_patch: Option<PathBuf>,

    /// Path to write the composed directive markdown.
    #[arg(long)]
    text_out: PathBuf,

    /// Directory for per-channel intermediate directives. Default: same dir as
    /// --text-out with a `_channels` suffix. Useful for cycle log auditing.
    #[arg(long)]
    channels_dir: Option<PathBuf>,

    /// Path to the workspace target/release/ dir containing the child bins.
    /// Default: derived from the running binary's path (assumes synth_directive
    /// lives in target/release/ alongside the others).
    #[arg(long)]
    bin_dir: Option<PathBuf>,

    #[arg(long, default_value = "seeds")]
    repo_canonical: String,
}

#[derive(Debug, Clone)]
struct ChannelOutput {
    label: &'static str,
    path: PathBuf,
    text: String,
    score: i32,
    resolved_targets: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let channels_dir = args.channels_dir.clone().unwrap_or_else(|| {
        let stem = args
            .text_out
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("directive");
        let parent = args
            .text_out
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        parent.join(format!("{stem}_channels"))
    });
    std::fs::create_dir_all(&channels_dir)?;

    let bin_dir = args
        .bin_dir
        .clone()
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        })
        .ok_or_else(|| anyhow::anyhow!("could not determine bin dir; pass --bin-dir"))?;
    eprintln!("[synth_directive] bin dir: {}", bin_dir.display());

    let mut channels: Vec<ChannelOutput> = Vec::new();

    // Channel 1: traceback.
    let tb_path = channels_dir.join("traceback.md");
    let tb_status = Command::new(bin_dir.join("synth_traceback_target"))
        .arg("--src").arg(&args.src)
        .arg("--issue").arg(&args.issue)
        .arg("--text-out").arg(&tb_path)
        .arg("--repo-canonical").arg(&args.repo_canonical)
        .status()
        .context("spawn synth_traceback_target")?;
    if tb_status.success() && tb_path.exists() {
        let text = std::fs::read_to_string(&tb_path)?;
        let (score, targets) = score_directive(&text, "traceback");
        channels.push(ChannelOutput {
            label: "traceback",
            path: tb_path,
            text,
            score,
            resolved_targets: targets,
        });
    } else {
        eprintln!("[synth_directive] traceback channel: skipped (exit {:?})", tb_status.code());
    }

    // Channel 2: test_expectation (skipped if --test-patch missing).
    if let Some(tp) = &args.test_patch {
        let te_path = channels_dir.join("test_expectation.md");
        let te_status = Command::new(bin_dir.join("synth_test_expectation"))
            .arg("--src").arg(&args.src)
            .arg("--test-patch").arg(tp)
            .arg("--text-out").arg(&te_path)
            .arg("--repo-canonical").arg(&args.repo_canonical)
            .status()
            .context("spawn synth_test_expectation")?;
        if te_status.success() && te_path.exists() {
            let text = std::fs::read_to_string(&te_path)?;
            let (score, targets) = score_directive(&text, "test_expectation");
            channels.push(ChannelOutput {
                label: "test_expectation",
                path: te_path,
                text,
                score,
                resolved_targets: targets,
            });
        } else {
            eprintln!("[synth_directive] test_expectation: skipped (exit {:?})", te_status.code());
        }
    } else {
        eprintln!("[synth_directive] test_expectation: skipped (no --test-patch)");
    }

    // Channel 3: prose mention.
    let pm_path = channels_dir.join("prose_mention.md");
    let pm_status = Command::new(bin_dir.join("synth_prose_mention"))
        .arg("--src").arg(&args.src)
        .arg("--issue").arg(&args.issue)
        .arg("--text-out").arg(&pm_path)
        .arg("--repo-canonical").arg(&args.repo_canonical)
        .status()
        .context("spawn synth_prose_mention")?;
    if pm_status.success() && pm_path.exists() {
        let text = std::fs::read_to_string(&pm_path)?;
        let (score, targets) = score_directive(&text, "prose_mention");
        channels.push(ChannelOutput {
            label: "prose_mention",
            path: pm_path,
            text,
            score,
            resolved_targets: targets,
        });
    } else {
        eprintln!("[synth_directive] prose_mention: skipped (exit {:?})", pm_status.code());
    }

    for c in &channels {
        eprintln!(
            "[synth_directive] channel {} score={} targets={} bytes={}",
            c.label,
            c.score,
            c.resolved_targets,
            c.text.len()
        );
    }

    // Compose. Sort channels by score descending; drop non-positive scores.
    let mut keep: Vec<&ChannelOutput> = channels.iter().filter(|c| c.score > 0).collect();
    keep.sort_by(|a, b| b.score.cmp(&a.score));

    if keep.is_empty() {
        let stub = format!(
            "\n## Required fix target (graph-derived, composed)\n\n\
             (no channel produced a non-empty directive — channels tried: {})\n",
            channels
                .iter()
                .map(|c| c.label)
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Some(parent) = args.text_out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&args.text_out, stub.as_bytes())?;
        eprintln!(
            "[synth_directive] wrote stub directive (all channels inert): {}",
            args.text_out.display()
        );
        return Ok(());
    }

    let mut composed = String::new();
    composed.push_str(&keep[0].text);
    if keep.len() > 1 {
        composed.push_str("\n## Additional graph-derived signals\n\n");
        composed.push_str(
            "The channels below independently surfaced overlapping targets. \
             Cross-channel agreement raises confidence; use them to disambiguate \
             the primary target above.\n\n",
        );
        for c in &keep[1..] {
            composed.push_str(&format!("### From {} (score {})\n", c.label, c.score));
            composed.push_str(&strip_directive_header(&c.text));
            composed.push('\n');
        }
    }

    if let Some(parent) = args.text_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.text_out, composed.as_bytes())?;
    eprintln!(
        "[synth_directive] wrote composed directive: {} (primary={}, {} additional channel(s))",
        args.text_out.display(),
        keep[0].label,
        keep.len() - 1,
    );
    Ok(())
}

fn score_directive(text: &str, _label: &str) -> (i32, usize) {
    let mut score = 0i32;
    let mut targets = 0usize;
    let mut had_traceback = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("- `") {
            score += 4;
            targets += 1;
        }
        if t.contains("Edit ONE") || t.contains("PRIMARY") {
            score += 2;
        }
        if t.contains("from issue traceback") {
            had_traceback = true;
        }
        if t.starts_with("(no ") {
            score -= 1;
        }
    }
    if had_traceback && targets > 0 {
        score += 1;
    }
    (score, targets)
}

fn strip_directive_header(text: &str) -> String {
    // Drop the first markdown "## " heading line so additional channels read
    // as inline sections rather than competing top-level headings.
    let mut out = String::new();
    let mut saw_first_h2 = false;
    for line in text.lines() {
        if !saw_first_h2 && line.trim_start().starts_with("## ") {
            saw_first_h2 = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}
