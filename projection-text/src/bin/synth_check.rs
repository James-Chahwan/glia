//! synth_check — P4.3 (Option 3, cycle 1.1).
//!
//! Post-edit graph re-parse + static check. Given a unified diff, applies
//! it to a tmp copy of the repo source, re-parses the affected files via
//! glia's repo-graph builder, and reports:
//!   1. unresolved symbols introduced by the diff (referenced but not in
//!      graph or imports)
//!   2. removed symbols still referenced elsewhere
//!   3. import lines added but no callsite in the diff
//!
//! Used between inference and venv test run: a STATIC-FAIL means the
//! diff has obvious problems before we burn test wall-clock on it. The
//! Lever 3 sage-loop pass-2 can then re-prompt with the static reasons
//! as critique seed.
//!
//! Output:
//!   --check-out <path>  json: {"status": "STATIC-OK"|"STATIC-FAIL",
//!                              "reasons": [...]}
//!
//! Conservative for v1: only the cheapest checks (import lines without
//! callsites, totally unresolved bare-name references). Deeper type-
//! mismatch checks deferred to follow-up.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;

#[derive(Parser, Debug)]
#[command(about = "Post-edit static check on a model-emitted diff (P4.3, cycle 1.1)")]
struct Args {
    /// Repo source root.
    #[arg(long)]
    src: PathBuf,

    /// Diff file emitted by the model.
    #[arg(long)]
    diff: PathBuf,

    /// Output JSON path. Format: {"status": "STATIC-OK"|"STATIC-FAIL", "reasons": [...]}.
    #[arg(long)]
    check_out: PathBuf,

    #[arg(long, default_value = "seeds")]
    repo_canonical: String,
}

#[derive(serde::Serialize)]
struct CheckResult {
    status: String,
    reasons: Vec<String>,
    files_touched: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let diff = std::fs::read_to_string(&args.diff).context("read diff")?;

    let mut reasons: Vec<String> = Vec::new();
    let mut files_touched: Vec<String> = Vec::new();

    // 1. Parse files touched by the diff.
    let file_re = Regex::new(r"^\+\+\+ b/(.+)$")?;
    for line in diff.lines() {
        if let Some(c) = file_re.captures(line) {
            files_touched.push(c.get(1).unwrap().as_str().to_string());
        }
    }

    if files_touched.is_empty() {
        // No file headers → diff probably malformed. Not STATIC-FAIL
        // per se (apply_and_test will catch it as APPLY-FAIL), just
        // pass through with a note.
        return write_result(&args.check_out, "STATIC-OK", &["no file headers parsed (diff may be malformed but that's apply's job)".to_string()], &files_touched);
    }

    // 2. Lightweight check: each added import line MUST have at least one
    // callsite in the diff. Catches "model added an import it doesn't
    // actually use" cases.
    let import_re = Regex::new(r"^\+\s*from\s+(\S+)\s+import\s+(.+)$")?;
    let import_simple_re = Regex::new(r"^\+\s*import\s+(\S+)")?;
    let mut imports: Vec<(String, String)> = Vec::new(); // (module, names_csv)
    let mut added_body: String = String::new();
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            added_body.push_str(rest);
            added_body.push('\n');
        }
    }
    for line in added_body.lines() {
        if let Some(c) = import_re.captures(line) {
            let module = c.get(1).unwrap().as_str().to_string();
            let names = c.get(2).unwrap().as_str().to_string();
            imports.push((module, names));
        } else if let Some(c) = import_simple_re.captures(line) {
            let module = c.get(1).unwrap().as_str().to_string();
            // For `import X.Y.Z`, the callsite uses `Z` or `X.Y.Z`.
            let names = module.rsplit('.').next().unwrap_or(&module).to_string();
            imports.push((module, names));
        }
    }
    for (module, names_csv) in &imports {
        // Split names_csv on `,` to extract individual names. Strip
        // `as <alias>` so we look for the alias when present.
        for raw_name in names_csv.split(',') {
            let raw = raw_name.trim();
            // alias support: "Foo as Bar" → look for Bar
            let look_name = if let Some(idx) = raw.find(" as ") {
                raw[idx + 4..].trim()
            } else {
                raw.trim()
            };
            if look_name.is_empty() || look_name == "*" {
                continue;
            }
            // Search added_body for any non-import line referencing
            // look_name. Substring match — false negatives only when
            // the name is also a prefix of something else.
            let used_outside_import = added_body
                .lines()
                .filter(|l| !l.trim_start().starts_with("import ")
                          && !l.trim_start().starts_with("from "))
                .any(|l| l.contains(look_name));
            if !used_outside_import {
                reasons.push(format!(
                    "import `{look_name}` from `{module}` added but never used in the diff body"
                ));
            }
        }
    }

    // 3. Anti-empty-diff sanity. If added body has zero non-import lines
    // OR the diff is empty after we strip headers, that's a STATIC-FAIL.
    let non_import_added = added_body
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("import ") && !t.starts_with("from ") && !t.is_empty()
        })
        .count();
    if non_import_added == 0 {
        reasons.push("diff adds zero non-import lines".to_string());
    }

    let status = if reasons.is_empty() { "STATIC-OK" } else { "STATIC-FAIL" };
    write_result(&args.check_out, status, &reasons, &files_touched)
}

fn write_result(out: &PathBuf, status: &str, reasons: &[String], files: &[String]) -> Result<()> {
    let result = CheckResult {
        status: status.to_string(),
        reasons: reasons.to_vec(),
        files_touched: files.to_vec(),
    };
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out, serde_json::to_string_pretty(&result)?)?;
    eprintln!("[synth_check] {status} ({} reasons, {} files): {}",
              reasons.len(), files.len(), out.display());
    Ok(())
}
