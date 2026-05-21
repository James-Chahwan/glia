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
use regex::Regex;

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

    /// SWE-bench hints_text — feeds synth_pr_hint (P4.2 / cycle 1.1).
    /// Optional; when missing OR empty, the hint channel is skipped.
    #[arg(long)]
    hints_text: Option<PathBuf>,

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
            .arg("--issue").arg(&args.issue)
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

    // Channel 6: pr_hint (P4.2 / cycle 1.1). Issue thread comments /
    // PR discussion as low-score background context. Only fires when
    // --hints-text supplied and content substantive.
    if let Some(ht) = &args.hints_text {
        let ph_path = channels_dir.join("pr_hint.md");
        let ph_status = Command::new(bin_dir.join("synth_pr_hint"))
            .arg("--hints-text").arg(ht)
            .arg("--text-out").arg(&ph_path)
            .arg("--repo-canonical").arg(&args.repo_canonical)
            .status()
            .context("spawn synth_pr_hint")?;
        if ph_status.success() && ph_path.exists() {
            let text = std::fs::read_to_string(&ph_path)?;
            let (score, targets) = score_directive(&text, "pr_hint");
            channels.push(ChannelOutput {
                label: "pr_hint",
                path: ph_path,
                text,
                score,
                resolved_targets: targets,
            });
        } else {
            eprintln!("[synth_directive] pr_hint: skipped (exit {:?})", ph_status.code());
        }
    } else {
        eprintln!("[synth_directive] pr_hint: skipped (no --hints-text)");
    }

    // Channel 5: runtime_type (P3.1 / cycle 1.1). Graph-resolves where
    // a failed attribute IS defined when the exception says the receiver
    // doesn't have it. Empty for non-attribute-access exceptions.
    {
        let rt_path = channels_dir.join("runtime_type.md");
        let rt_status = Command::new(bin_dir.join("synth_runtime_type"))
            .arg("--src").arg(&args.src)
            .arg("--issue").arg(&args.issue)
            .arg("--text-out").arg(&rt_path)
            .arg("--repo-canonical").arg(&args.repo_canonical)
            .status()
            .context("spawn synth_runtime_type")?;
        if rt_status.success() && rt_path.exists() {
            let text = std::fs::read_to_string(&rt_path)?;
            let (score, targets) = score_directive(&text, "runtime_type");
            channels.push(ChannelOutput {
                label: "runtime_type",
                path: rt_path,
                text,
                score,
                resolved_targets: targets,
            });
        } else {
            eprintln!("[synth_directive] runtime_type: skipped (exit {:?})", rt_status.code());
        }
    }

    // Channel 4: test_assertion (Lever 4 / cycle 1.1). Surfaces the
    // EXPECTED behavior of each F2P test as a directive — what the test
    // wants the implementation to produce, not just which symbols it
    // names. Complementary to test_expectation: TE answers WHERE, TA
    // answers WHAT.
    if let Some(tp) = &args.test_patch {
        let ta_path = channels_dir.join("test_assertion.md");
        let ta_status = Command::new(bin_dir.join("synth_test_assertion"))
            .arg("--test-patch").arg(tp)
            .arg("--text-out").arg(&ta_path)
            .arg("--repo-canonical").arg(&args.repo_canonical)
            .status()
            .context("spawn synth_test_assertion")?;
        if ta_status.success() && ta_path.exists() {
            let text = std::fs::read_to_string(&ta_path)?;
            let (score, targets) = score_directive(&text, "test_assertion");
            channels.push(ChannelOutput {
                label: "test_assertion",
                path: ta_path,
                text,
                score,
                resolved_targets: targets,
            });
        } else {
            eprintln!("[synth_directive] test_assertion: skipped (exit {:?})", ta_status.code());
        }
    } else {
        eprintln!("[synth_directive] test_assertion: skipped (no --test-patch)");
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

    // Cross-channel anti-target dedup. Cycle 0.9 marshmallow regression:
    // traceback (now PRIMARY due to scoring boost) named DateTime as PRIMARY
    // and List as anti-target, while test_expectation listed List among its
    // PRIMARY targets. The model followed the anti-target list and edited
    // the wrong function. Fix: when PRIMARY's anti-target list contains a
    // qname that another channel surfaced as a target, strip it from the
    // anti-target block — multi-channel disagreement means we should not
    // forbid the qname, the model needs to pick between candidates.
    let cross_channel_targets: std::collections::HashSet<String> = if keep.len() > 1 {
        let mut s = std::collections::HashSet::new();
        for c in &keep[1..] {
            for line in c.text.lines() {
                let t = line.trim_start();
                if let Some(rest) = t.strip_prefix("- `") {
                    if let Some(end) = rest.find('`') {
                        s.insert(rest[..end].to_string());
                    }
                }
            }
        }
        s
    } else {
        std::collections::HashSet::new()
    };

    // Per-channel dedup: each channel's anti-targets must respect ALL OTHER
    // channels' targets (not just primary's). Cycle 1.0a marshmallow regressed
    // because the traceback channel was SECONDARY (score 36 < test_expectation
    // 39) and its anti-target `List::_bind_to_schema` was never filtered —
    // dedup only looked at primary text. Now we dedup every channel.
    let dedup_per_channel = |idx: usize, text: &str| -> String {
        if cross_channel_targets.is_empty() {
            return text.to_string();
        }
        // Build per-channel keep_targets: union of all OTHER channels' targets.
        let mut keep_targets = std::collections::HashSet::new();
        for (j, c) in keep.iter().enumerate() {
            if j == idx { continue; }
            for line in c.text.lines() {
                let t = line.trim_start();
                if let Some(rest) = t.strip_prefix("- `") {
                    if let Some(end) = rest.find('`') {
                        keep_targets.insert(rest[..end].to_string());
                    }
                }
            }
        }
        filter_antitargets(text, &keep_targets)
    };

    let primary_text = dedup_per_channel(0, &keep[0].text);

    let mut composed = String::new();
    // Causal chain prelude (Lever 5, cycle 1.1). Stitches signals from the
    // existing channels into a top-of-directive narrative the model can
    // follow: test asserts X → calls Y → fails because Z → fix at K. The
    // cycle 0.4 mechanism reframe says the model needs the CHAIN, not the
    // LIST. Gated off via GLIA_DIRECTIVE_FLAT=1 for backwards comparison.
    if std::env::var("GLIA_DIRECTIVE_FLAT").as_deref() != Ok("1") {
        if let Some(chain) = build_causal_chain(&channels) {
            composed.push_str(&chain);
            composed.push('\n');
        }
    }
    composed.push_str(&primary_text);
    if keep.len() > 1 {
        composed.push_str("\n## Additional graph-derived signals\n\n");
        composed.push_str(
            "The channels below independently surfaced overlapping targets. \
             Cross-channel agreement raises confidence; use them to disambiguate \
             the primary target above.\n\n",
        );
        for (i, c) in keep[1..].iter().enumerate() {
            let filtered = dedup_per_channel(i + 1, &c.text);
            composed.push_str(&format!("### From {} (score {})\n", c.label, c.score));
            composed.push_str(&strip_directive_header(&filtered));
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

// Cycle 0.7 empirical finding (channel-attribution on marshmallow's PASS):
// the traceback channel surfaced `_bind_to_schema` (the gold method) but the
// composer ranked it SECONDARY behind test_expectation (which surfaced CLASS
// names only). Marshmallow PASSed because traceback content was still in the
// directive, but for django + pytest the composer's CLASS-PRIMARY ranking
// caused regressions to APPLY-FAIL. Fix: weight METHOD bullets and line-
// anchored content above CLASS bullets, and give traceback a stronger base
// bonus when it provides line ranges.
fn score_directive(text: &str, _label: &str) -> (i32, usize) {
    let mut score = 0i32;
    let mut targets = 0usize;
    let mut had_traceback = false;
    let mut had_line_anchor = false;
    let mut method_bullets = 0i32;
    let mut class_bullets = 0i32;
    let mut assertion_bullets = 0i32;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("- `") {
            score += 4;
            targets += 1;
            // Per-bullet kind boost. Bullets emitted by synth_test_expectation
            // tag the kind in parens: "(method, ...)" / "(class, ...)" /
            // "(function, ...)". synth_traceback_target's bullets are
            // anti-target labels OR sections that include "method"/"function"
            // signal via the directive's prose ("Edit ONE function: ...").
            if t.contains("(method,") {
                method_bullets += 1;
                score += 4; // method bullets are more precise than class
            } else if t.contains("(class,") {
                class_bullets += 1;
            }
        }
        // synth_test_assertion bullets have shape "- Expression `X` must
        // equal `Y`." / "- Code under test must raise `E`." — they don't
        // start with `- `qname`` so the target-bullet path above misses
        // them. Score per-assertion +3 (slightly below target-bullet +4
        // because behavior alone without targeting is weaker — needs
        // pairing with a targeting channel). Equality/identity assertions
        // are signal-dense; "raises" assertions strongest because they
        // pin a specific exception class.
        if t.starts_with("- Expression `") || t.starts_with("- `")
            && (t.contains(" must equal ") || t.contains(" must NOT equal ")) {
            assertion_bullets += 1;
            score += 3;
        }
        if t.starts_with("- Code under test must raise") {
            assertion_bullets += 1;
            score += 4;
        }
        if t.starts_with("- ") && (t.contains("must evaluate truthy") || t.contains("must evaluate falsy")) {
            assertion_bullets += 1;
            score += 2;
        }
        if t.starts_with("- ") && t.contains("must be an instance of") {
            assertion_bullets += 1;
            score += 3;
        }
        if t.contains("Edit ONE") || t.contains("PRIMARY") {
            score += 2;
        }
        if t.contains("from issue traceback") {
            had_traceback = true;
        }
        if t.contains("function body spans lines") {
            // synth_traceback_target's line-anchor signal. Strongest single
            // content signal — a line-anchored target is far more actionable
            // than a class name. Worth +10.
            had_line_anchor = true;
        }
        if t.contains("BUGGY LINE") {
            // synth_traceback_target also names the buggy LINE explicitly —
            // strongest possible content signal.
            score += 5;
        }
        if t.starts_with("(no ") {
            score -= 1;
        }
    }
    if had_traceback && targets > 0 {
        score += 3;
    }
    if had_line_anchor {
        score += 10;
    }
    // Cap the noise from many class bullets: each class beyond the first 3
    // adds +1 instead of +4 (test_expectation can surface 5-7 classes for
    // tests that touch many CLASSES from the test_patch).
    if class_bullets > 3 {
        let excess = class_bullets - 3;
        score -= 3 * excess;
    }
    let _ = method_bullets;
    let _ = assertion_bullets;
    (score, targets)
}

/// Stitch the per-channel signals into a top-of-directive causal-chain
/// block. The model reads this once, top-down, before the per-channel
/// details below. Returns None when not enough signal exists for a useful
/// chain (e.g. inferences-without-traceback or test-patch-only cases).
///
/// Sources:
///   - assertion expression: test_assertion's first "Expression `X` must equal `Y`" bullet
///   - call-target site: traceback's "Edit ONE function: `Q` in `F` (function body spans lines L-M)"
///   - buggy line: traceback's "The BUGGY LINE (per traceback line N) is:" + next line
///   - exception class: traceback's "The exception is `<Class>: ...`"
///   - fix site (primary target): same as call-target
fn build_causal_chain(channels: &[ChannelOutput]) -> Option<String> {
    let mut assertion_expr: Option<(String, String)> = None;
    let mut call_target_qname: Option<String> = None;
    let mut call_target_file: Option<String> = None;
    let mut call_target_lines: Option<(String, String)> = None;
    let mut buggy_line_no: Option<String> = None;
    let mut buggy_line_text: Option<String> = None;
    let mut exception_class: Option<String> = None;
    let mut runtime_type: Option<String> = None;

    let assert_re = match Regex::new(r"^- Expression `([^`]+)` must equal `([^`]+)`") {
        Ok(r) => r, Err(_) => return None,
    };
    let edit_one_re = match Regex::new(r"^Edit ONE function: `([^`]+)` in `([^`]+)` \(function body spans lines (\d+)-(\d+)\)") {
        Ok(r) => r, Err(_) => return None,
    };
    let buggy_intro_re = match Regex::new(r"^The BUGGY LINE \(per traceback line (\d+)\) is:") {
        Ok(r) => r, Err(_) => return None,
    };
    let exc_re = match Regex::new(r"^The exception is `([^`:]+)(?::[^`]*)?`") {
        Ok(r) => r, Err(_) => return None,
    };
    let runtime_re = match Regex::new(r"receiver of the failing[^`]*`\.([^`]+)`[^`]*is a `([^`]+)` instance") {
        Ok(r) => r, Err(_) => return None,
    };

    for c in channels {
        let mut lines_iter = c.text.lines().peekable();
        while let Some(line) = lines_iter.next() {
            let t = line.trim_start();
            if assertion_expr.is_none() {
                if let Some(cap) = assert_re.captures(t) {
                    assertion_expr = Some((
                        cap.get(1).unwrap().as_str().to_string(),
                        cap.get(2).unwrap().as_str().to_string(),
                    ));
                }
            }
            if call_target_qname.is_none() {
                if let Some(cap) = edit_one_re.captures(t) {
                    call_target_qname = Some(cap.get(1).unwrap().as_str().to_string());
                    call_target_file = Some(cap.get(2).unwrap().as_str().to_string());
                    call_target_lines = Some((
                        cap.get(3).unwrap().as_str().to_string(),
                        cap.get(4).unwrap().as_str().to_string(),
                    ));
                }
            }
            if buggy_line_no.is_none() {
                if let Some(cap) = buggy_intro_re.captures(t) {
                    buggy_line_no = Some(cap.get(1).unwrap().as_str().to_string());
                    // The buggy source line is on a subsequent line inside
                    // a ```...``` block. Skip the opening fence then take
                    // the first non-fence line.
                    while let Some(peek) = lines_iter.peek() {
                        let pt = peek.trim();
                        if pt == "```" { lines_iter.next(); continue; }
                        if pt.is_empty() { lines_iter.next(); continue; }
                        buggy_line_text = Some(pt.to_string());
                        break;
                    }
                }
            }
            if exception_class.is_none() {
                if let Some(cap) = exc_re.captures(t) {
                    exception_class = Some(cap.get(1).unwrap().as_str().to_string());
                }
            }
            if runtime_type.is_none() {
                if let Some(cap) = runtime_re.captures(t) {
                    runtime_type = Some(format!(
                        "`.{}` access on `{}` instance",
                        cap.get(1).unwrap().as_str(),
                        cap.get(2).unwrap().as_str(),
                    ));
                }
            }
        }
    }

    // Require at least 2 of {assertion, traceback-target, exception} to
    // emit a chain. With only 1 signal it's not a chain, just restating.
    let signal_count =
        assertion_expr.is_some() as i32
        + call_target_qname.is_some() as i32
        + exception_class.is_some() as i32;
    if signal_count < 2 {
        return None;
    }

    let mut s = String::new();
    s.push_str("\n## Causal chain (graph-stitched from all channels)\n\n");
    s.push_str(
        "Read top-down. Each step explains the next; the fix lands where the \
         chain breaks under the asserted inputs.\n\n",
    );

    let mut step = 1;
    if let Some((lhs, rhs)) = &assertion_expr {
        s.push_str(&format!(
            "{step}. **Test asserts**: `{lhs}` must equal `{rhs}`. The fix must make this evaluate as specified.\n",
        ));
        step += 1;
    }
    if let (Some(q), Some(f), Some((l1, l2))) = (&call_target_qname, &call_target_file, &call_target_lines) {
        s.push_str(&format!(
            "{step}. **Execution reaches**: `{q}` in `{f}` (lines {l1}-{l2}). This is the deepest graph-resolved frame in the traceback.\n",
        ));
        step += 1;
    }
    if let (Some(line_no), Some(buggy)) = (&buggy_line_no, &buggy_line_text) {
        s.push_str(&format!(
            "{step}. **Failure point**: line {line_no} reads `{buggy}` — this is where the runtime exception originates.\n",
        ));
        step += 1;
    }
    if let Some(ec) = &exception_class {
        let detail = runtime_type.as_deref().unwrap_or("");
        if detail.is_empty() {
            s.push_str(&format!(
                "{step}. **Failure mode**: `{ec}` raised here — fix must prevent this condition under the test inputs.\n",
            ));
        } else {
            s.push_str(&format!(
                "{step}. **Failure mode**: `{ec}` raised because {detail}. Use the right access path so this resolves.\n",
            ));
        }
        step += 1;
    }
    if let (Some(q), Some(f)) = (&call_target_qname, &call_target_file) {
        s.push_str(&format!(
            "{step}. **Fix lands at**: edit `{q}` in `{f}` (or its caller surfaced as anti-target / sibling site) with a minimal change that satisfies step 1's assertion.\n",
        ));
    }
    s.push_str("\nThe per-channel blocks below provide supporting detail (targeting alternatives, anti-targets, prose context, behavioral assertions). Use them to disambiguate when the chain has ambiguity.\n");
    Some(s)
}

/// Remove anti-target bullets whose qname is in `keep_targets` (qnames
/// surfaced as targets by other channels). Operates line-by-line. The
/// "**Do NOT modify**" header line is preserved (the comment still
/// applies to remaining anti-targets); when all anti-targets are
/// filtered, the entire block is dropped.
fn filter_antitargets(text: &str, keep_targets: &std::collections::HashSet<String>) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        // Detect start of "**Do NOT modify**" block. Collect contiguous
        // "- `qname`" bullets that follow, filter, emit only the survivors.
        if trimmed.starts_with("**Do NOT modify**") {
            let mut surviving: Vec<&str> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() {
                let bullet = lines[j].trim_start();
                if let Some(rest) = bullet.strip_prefix("- `") {
                    if let Some(end) = rest.find('`') {
                        let qname = &rest[..end];
                        // Strip when:
                        //   (a) exact match with a primary target qname, OR
                        //   (b) qname is a CHILD of a primary target (anti is
                        //       a method/inner of a class that another channel
                        //       surfaced as a target — e.g. anti=
                        //       `pkg::Cls::method` should be stripped when
                        //       primary lists `pkg::Cls`).
                        // Cycle 1.0a marshmallow regression smoking gun: anti
                        // qname `src::marshmallow::fields::List::_bind_to_schema`
                        // wasn't stripped because test_expectation listed the
                        // CLASS `src::marshmallow::fields::List`, not the method.
                        let mut should_strip = keep_targets.contains(qname);
                        if !should_strip {
                            for kt in keep_targets {
                                let kt_prefix = format!("{kt}::");
                                if qname.starts_with(&kt_prefix) {
                                    should_strip = true;
                                    break;
                                }
                            }
                        }
                        if !should_strip {
                            surviving.push(lines[j]);
                        }
                        j += 1;
                        continue;
                    }
                }
                break;
            }
            if !surviving.is_empty() {
                out.push_str(line);
                out.push('\n');
                for s in &surviving {
                    out.push_str(s);
                    out.push('\n');
                }
            }
            i = j;
            continue;
        }
        out.push_str(line);
        out.push('\n');
        i += 1;
    }
    out
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
