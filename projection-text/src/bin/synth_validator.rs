//! synth_validator — C1, cycle 0.6 sage-loop spitball.
//!
//! Parses a model's first-pass diff + the graph-derived directive that
//! produced it, emits a structured critique if the diff misaligns with the
//! directive's named targets / access paths. The critique feeds pass-2
//! prompting (run_instance.py --two-pass) for the model to revise.
//!
//! Motivation: cycle 0.7 evidence — django-10914 regressed from
//! RIGHT-LINE-WRONG-CONTENT to APPLY-FAIL because B1's test_expectation
//! channel surfaced `FileSystemStorage::file_permissions_mode` (a class
//! method) but the gold fix is a default value in `conf/global_settings.py`.
//! The composer's PRIMARY designation routed the model to the wrong file.
//! A diff-validator would catch "edited storage.py but directive named
//! storage.py — matches" → "but the second-pass test still fails" →
//! suggest the OTHER named targets next pass.
//!
//! Critique categories (emit any that apply):
//!   - TARGET-MISMATCH: diff edits a function/file not named in directive
//!   - ACCESS-PATH-MISS: directive listed `self.root.X` but diff uses `self.X`
//!   - EMPTY-DIFF: no parseable diff content
//!   - HUNK-LINE-DRIFT: diff's hunk start lines are far from directive's
//!     named line ranges (suggests fuzz tolerance limit will be hit)
//!
//! Emits inert sentinel `(no critique: diff aligns with directive)` when
//! all checks pass so the orchestrator can skip pass-2.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;

#[derive(Parser, Debug)]
#[command(about = "Critique a model's first-pass diff against its graph-derived directive (C1)")]
struct Args {
    /// Model's first-pass diff output (typically workdir/out.txt).
    #[arg(long)]
    diff: PathBuf,

    /// The composed directive that produced the diff (workdir/directive.txt).
    #[arg(long)]
    directive: PathBuf,

    /// Output path for the critique markdown.
    #[arg(long)]
    text_out: PathBuf,
}

#[derive(Debug, Clone)]
struct DirectiveTargets {
    /// Qnames named in any directive bullet (` - `qname` ...`).
    qnames: BTreeSet<String>,
    /// Tail-names of qnames (after last `::`).
    qname_tails: BTreeSet<String>,
    /// File paths mentioned in the directive (e.g. `src/marshmallow/fields.py`).
    files: BTreeSet<String>,
    /// Line ranges named like "function body spans lines N-M".
    line_ranges: Vec<(String, u32, u32)>,
    /// Access paths the directive instructed (e.g. `self.root.opts`).
    access_paths: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct DiffShape {
    files: BTreeSet<String>,
    /// (file, hunk_start_line, hunk_end_line, enclosing_context).
    hunks: Vec<(String, u32, u32, String)>,
    /// All `+ ` line bodies, joined for substring search.
    added_text: String,
    /// `-` line bodies.
    removed_text: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let diff_text = std::fs::read_to_string(&args.diff)
        .with_context(|| format!("read diff {}", args.diff.display()))?;
    let directive_text = std::fs::read_to_string(&args.directive)
        .with_context(|| format!("read directive {}", args.directive.display()))?;

    let targets = parse_directive(&directive_text);
    let shape = parse_diff(&diff_text);

    eprintln!(
        "[synth_validator] directive: {} qnames, {} files, {} line_ranges, {} access_paths",
        targets.qnames.len(),
        targets.files.len(),
        targets.line_ranges.len(),
        targets.access_paths.len(),
    );
    eprintln!(
        "[synth_validator] diff: {} files, {} hunks, +{}b -{}b",
        shape.files.len(),
        shape.hunks.len(),
        shape.added_text.len(),
        shape.removed_text.len(),
    );

    let mut critiques: Vec<String> = Vec::new();

    if shape.files.is_empty() && shape.added_text.trim().is_empty() {
        critiques.push(format!(
            "**EMPTY-DIFF**: The first-pass output contained no parseable diff. \
             The directive named these targets — pick ONE and emit a minimal \
             unified diff targeting it:\n{}",
            targets
                .qnames
                .iter()
                .map(|q| format!("- `{q}`"))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    } else {
        // TARGET-MISMATCH: did the diff touch any directive-named file/qname?
        let mut file_match = false;
        let mut qname_match = false;
        for df in &shape.files {
            // Treat directive files as substring-of-diff-file OR vice versa
            // (handles `src/marshmallow/fields.py` vs `marshmallow/fields.py`).
            for tf in &targets.files {
                if df.ends_with(tf.as_str()) || tf.ends_with(df.as_str()) {
                    file_match = true;
                    break;
                }
            }
            if file_match {
                break;
            }
        }
        for (_, _, _, ctx) in &shape.hunks {
            for tail in &targets.qname_tails {
                if ctx.contains(tail) {
                    qname_match = true;
                    break;
                }
            }
            if qname_match {
                break;
            }
        }
        if !file_match && !targets.files.is_empty() {
            critiques.push(format!(
                "**TARGET-MISMATCH (file)**: The diff edits files {:?} but the \
                 directive named these files: {:?}. Edit one of the directive's \
                 files instead — the gold fix is in one of those.",
                shape.files.iter().collect::<Vec<_>>(),
                targets.files.iter().collect::<Vec<_>>(),
            ));
        }
        if !qname_match
            && !targets.qname_tails.is_empty()
            && !shape.hunks.is_empty()
        {
            critiques.push(format!(
                "**TARGET-MISMATCH (function)**: Hunk contexts {:?} don't include \
                 any directive-named qname tail {:?}. The directive's targets are \
                 functions to edit; route the diff to one of those.",
                shape
                    .hunks
                    .iter()
                    .map(|(_, _, _, c)| c.trim())
                    .collect::<Vec<_>>(),
                targets.qname_tails.iter().collect::<Vec<_>>(),
            ));
        }

        // ACCESS-PATH-MISS: if the directive listed `self.root.X` style paths,
        // verify the diff's added lines USE that prefix.
        for ap in &targets.access_paths {
            // Pull the "tail" past the last `.` — that's the missing attr.
            if let Some(last_dot) = ap.rfind('.') {
                let prefix = &ap[..last_dot];
                let attr = &ap[last_dot + 1..];
                // If the diff's added text mentions the attr but NOT with the
                // directive's prefix, flag it.
                if shape.added_text.contains(attr) && !shape.added_text.contains(prefix) {
                    critiques.push(format!(
                        "**ACCESS-PATH-MISS**: Directive listed access path `{}` \
                         but the added lines reference `.{}` without the `{}.` \
                         prefix. Replace bare `.{}` access with `{}` to match \
                         the directive's resolution.",
                        ap, attr, prefix, attr, ap,
                    ));
                    break;
                }
            }
        }

        // HUNK-LINE-DRIFT: if the directive named line ranges and the diff's
        // hunks are far away (≥10 lines outside any named range), flag it.
        if !targets.line_ranges.is_empty() && !shape.hunks.is_empty() {
            let mut any_close = false;
            for (df, hs, he, _) in &shape.hunks {
                for (tf, ts, te) in &targets.line_ranges {
                    if df.ends_with(tf.as_str()) || tf.ends_with(df.as_str()) {
                        let max_lo = (*hs).max(*ts);
                        let min_hi = (*he).min(*te);
                        let dist = if max_lo <= min_hi {
                            0
                        } else {
                            max_lo - min_hi
                        };
                        if dist <= 10 {
                            any_close = true;
                            break;
                        }
                    }
                }
                if any_close {
                    break;
                }
            }
            if !any_close {
                critiques.push(format!(
                    "**HUNK-LINE-DRIFT**: Diff's hunk lines {:?} are >10 lines \
                     from any directive-named line range {:?}. Apply-fuzz \
                     tolerance will likely reject. Retarget hunks to the \
                     directive's named line ranges.",
                    shape
                        .hunks
                        .iter()
                        .map(|(f, s, e, _)| format!("{f}:{s}-{e}"))
                        .collect::<Vec<_>>(),
                    targets
                        .line_ranges
                        .iter()
                        .map(|(f, s, e)| format!("{f}:{s}-{e}"))
                        .collect::<Vec<_>>(),
                ));
            }
        }
    }

    let critique_md = if critiques.is_empty() {
        "(no critique: diff aligns with directive)\n".to_string()
    } else {
        let mut s = String::new();
        s.push_str("## Critique of first-pass diff (graph-derived)\n\n");
        s.push_str(
            "Your first-pass attempt below has issues against the directive that \
             guided it. Address each critique and produce a CORRECTED diff:\n\n",
        );
        for c in &critiques {
            s.push_str("- ");
            s.push_str(c);
            s.push_str("\n\n");
        }
        s.push_str("## Revised guidance\n\n");
        if !targets.qnames.is_empty() {
            s.push_str("The directive's targets are:\n");
            for q in &targets.qnames {
                s.push_str(&format!("- `{q}`\n"));
            }
            s.push('\n');
        }
        s.push_str(
            "Edit ONE target above (whichever fixes the bug), emit a minimal \
             unified diff. Do NOT repeat the previous attempt's edits.\n",
        );
        s
    };

    if let Some(parent) = args.text_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.text_out, critique_md.as_bytes())?;
    eprintln!(
        "[synth_validator] wrote critique: {} ({} critique{})",
        args.text_out.display(),
        critiques.len(),
        if critiques.len() == 1 { "" } else { "s" },
    );
    Ok(())
}

fn parse_directive(text: &str) -> DirectiveTargets {
    // Extract qnames from ANY backtick-quoted string that looks like a Python
    // qname (contains `::` OR is a single identifier). Both bullet-line
    // bullets AND paragraph-statement targets (e.g. synth_traceback_target's
    // "Edit ONE function: `artist::Artist::format_cursor_data`") flow through
    // here. Filters out access-path patterns (start with `self.`) which are
    // handled separately.
    let qname_quoted_re = Regex::new(r"`([A-Za-z_][A-Za-z0-9_:]*)`").unwrap();
    let file_re = Regex::new(r"in\s+`([^`]+\.\w+)`").unwrap();
    let line_range_re =
        Regex::new(r"in\s+`([^`]+\.\w+)`\s*\(function body spans lines (\d+)-(\d+)\)").unwrap();
    let access_path_re = Regex::new(r"`(self\.[A-Za-z_][A-Za-z0-9_.]*)`").unwrap();
    let mut qnames = BTreeSet::new();
    let mut qname_tails = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut line_ranges = Vec::new();
    let mut access_paths = BTreeSet::new();
    for line in text.lines() {
        for c in qname_quoted_re.captures_iter(line) {
            let qn = c.get(1).unwrap().as_str();
            // Skip access paths (self.X) — caught by access_path_re below.
            if qn.starts_with("self.") {
                continue;
            }
            // Require qname-shape: contains `::` OR is plain identifier ≥ 2 chars.
            if !qn.contains("::") && qn.len() < 2 {
                continue;
            }
            let tail = qn.rsplit("::").next().unwrap_or(qn).to_string();
            qname_tails.insert(tail);
            qnames.insert(qn.to_string());
        }
        for c in file_re.captures_iter(line) {
            let f = c.get(1).unwrap().as_str().to_string();
            // Skip meta-references that aren't actual target files.
            if f == "test_patch.patch" || f.ends_with(".patch") {
                continue;
            }
            files.insert(f);
        }
        for c in line_range_re.captures_iter(line) {
            let f = c.get(1).unwrap().as_str().to_string();
            let s: u32 = c.get(2).unwrap().as_str().parse().unwrap_or(0);
            let e: u32 = c.get(3).unwrap().as_str().parse().unwrap_or(0);
            line_ranges.push((f, s, e));
        }
        for c in access_path_re.captures_iter(line) {
            access_paths.insert(c.get(1).unwrap().as_str().to_string());
        }
    }
    DirectiveTargets { qnames, qname_tails, files, line_ranges, access_paths }
}

fn parse_diff(text: &str) -> DiffShape {
    let file_re = Regex::new(r"^\+\+\+ b/(.+)$").unwrap();
    let diff_git_re = Regex::new(r"^diff --git a/\S+ b/(\S+)$").unwrap();
    let hunk_re = Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@(.*)$").unwrap();
    let mut files = BTreeSet::new();
    let mut hunks = Vec::new();
    let mut cur_file: Option<String> = None;
    let mut added_text = String::new();
    let mut removed_text = String::new();
    for line in text.lines() {
        if let Some(c) = file_re.captures(line) {
            cur_file = Some(c.get(1).unwrap().as_str().to_string());
            files.insert(cur_file.clone().unwrap());
            continue;
        }
        if let Some(c) = diff_git_re.captures(line) {
            cur_file = Some(c.get(1).unwrap().as_str().to_string());
            files.insert(cur_file.clone().unwrap());
            continue;
        }
        if let Some(c) = hunk_re.captures(line) {
            if let Some(f) = &cur_file {
                let start: u32 = c.get(1).unwrap().as_str().parse().unwrap_or(0);
                let count: u32 = c.get(2).map(|m| m.as_str().parse().unwrap_or(1)).unwrap_or(1);
                let ctx = c.get(3).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
                hunks.push((f.clone(), start, start + count - 1, ctx));
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            if !line.starts_with("+++") {
                added_text.push_str(rest);
                added_text.push('\n');
            }
        } else if let Some(rest) = line.strip_prefix('-') {
            if !line.starts_with("---") {
                removed_text.push_str(rest);
                removed_text.push('\n');
            }
        }
    }
    DiffShape { files, hunks, added_text, removed_text }
}
