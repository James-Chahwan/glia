//! synth_test_assertion — Lever 4 (Tier 1 #1, cycle 1.1).
//!
//! Parses SWE-bench `test_patch.patch` for ASSERTION FORMS (the lines that
//! actually fail / would-fail). Surfaces the expected output the test wants
//! the implementation to produce, as a directive channel.
//!
//! Where synth_test_expectation gives the model TARGETING (which functions
//! the test names), this bin gives the model EDIT INTENT (what the test
//! expects to read after the fix). Together they pin both halves of the
//! gold-line skeleton problem.
//!
//! Assertion forms recognised:
//!   - `assert <expr> == <val>`
//!   - `assert <expr> != <val>`
//!   - `assert <expr> is <val>` / `is not <val>`
//!   - `assert <expr> in <val>` / `not in <val>`
//!   - `assert <expr>` (truthy)
//!   - `with pytest.raises(<E>[, match=...]):`
//!   - `pytest.raises(<E>)` inside contexts
//!   - `self.assertEqual(<a>, <b>)`
//!   - `self.assertRaises(<E>)`
//!   - `self.assertTrue(<expr>)` / `self.assertFalse(<expr>)`
//!   - `self.assertIsInstance(<x>, <C>)`
//!
//! Output: markdown directive block written to --text-out. Structure mirrors
//! synth_test_expectation so the composer can ingest it as one of its
//! channels. The block names each test function + its assertions in order,
//! grouped per added-test-function.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;

#[derive(Parser, Debug)]
#[command(about = "Derive an assertion-shape directive from test_patch.patch (Lever 4, cycle 1.1)")]
struct Args {
    /// SWE-bench test_patch.patch (unified diff of F2P test additions).
    #[arg(long)]
    test_patch: PathBuf,

    /// Output path for the markdown directive block.
    #[arg(long)]
    text_out: PathBuf,

    /// Maximum assertions to emit (per test function). 0 = no cap.
    #[arg(long, default_value_t = 8)]
    max_per_fn: usize,

    /// Reserved (channel composer compatibility). Unused by this bin but
    /// kept so the orchestrator's CLI shape stays uniform across synth bins.
    #[arg(long, default_value = "seeds")]
    repo_canonical: String,
}

#[derive(Debug, Clone)]
struct AssertedItem {
    /// The kind of assertion (eq, raises, truthy, in, ...).
    kind: String,
    /// The expression-under-test as the test wrote it (LHS of `==`, arg of
    /// raises, etc).
    expr: String,
    /// The expected value or expected exception class. None for truthy.
    expected: Option<String>,
    /// Optional second positional (for `assertEqual(a, b)` we record `b`
    /// here; for `assertIsInstance(x, C)` we record `C`).
    expected2: Option<String>,
    /// Raw line text for debugging.
    raw: String,
}

#[derive(Debug, Clone, Default)]
struct PerTestFn {
    name: String,
    asserts: Vec<AssertedItem>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let patch = std::fs::read_to_string(&args.test_patch)
        .context("reading test_patch")?;
    let mut per_fn = parse_patch(&patch)?;

    if per_fn.is_empty() {
        write_empty_directive(&args.text_out, "no test functions parsed from test_patch")?;
        return Ok(());
    }

    if args.max_per_fn > 0 {
        for tf in per_fn.values_mut() {
            if tf.asserts.len() > args.max_per_fn {
                tf.asserts.truncate(args.max_per_fn);
            }
        }
    }

    let directive = render_directive(&per_fn);
    if let Some(parent) = args.text_out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.text_out, directive.as_bytes())?;
    let n_asserts: usize = per_fn.values().map(|tf| tf.asserts.len()).sum();
    eprintln!(
        "[synth_test_assertion] wrote directive: {} ({} test fn(s), {} assertions)",
        args.text_out.display(),
        per_fn.len(),
        n_asserts,
    );
    Ok(())
}

fn parse_patch(patch: &str) -> Result<BTreeMap<String, PerTestFn>> {
    let def_re = Regex::new(r"def\s+(test_[A-Za-z_][A-Za-z0-9_]*)\s*\(")?;
    // assert <lhs> <op> <rhs> ; or assert <expr> (truthy)
    let assert_eq_re = Regex::new(r"^\s*assert\s+(.+?)\s+(==|!=|is\s+not|is|not\s+in|in)\s+(.+?)\s*(?:#.*)?$")?;
    let assert_truthy_re = Regex::new(r"^\s*assert\s+(.+?)\s*(?:#.*)?$")?;
    let raises_re = Regex::new(r"^\s*(?:with\s+)?pytest\.raises\(\s*([A-Za-z_][\w\.]*)\s*(?:,\s*match\s*=\s*([^)]+))?\)\s*:?")?;
    let assert_equal_re = Regex::new(r"^\s*self\.assertEqual\s*\(\s*(.+?)\s*,\s*(.+?)\s*\)\s*(?:#.*)?$")?;
    let assert_raises_re = Regex::new(r"^\s*self\.assertRaises\s*\(\s*([A-Za-z_][\w\.]*)\s*\)")?;
    let assert_truefalse_re = Regex::new(r"^\s*self\.(assertTrue|assertFalse)\s*\(\s*(.+?)\s*\)\s*(?:#.*)?$")?;
    let assert_isinstance_re = Regex::new(r"^\s*self\.assertIsInstance\s*\(\s*(.+?)\s*,\s*(.+?)\s*\)")?;

    let mut per_fn: BTreeMap<String, PerTestFn> = BTreeMap::new();
    let mut cur_fn: Option<String> = None;
    let mut in_added_block = false;

    for line in patch.lines() {
        // Only consider added lines (start with `+`, but NOT `+++` file
        // header). Track def boundaries within the added stream.
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with("@@") {
            in_added_block = true; // entered a hunk
            continue;
        }
        if !in_added_block {
            continue;
        }
        let body = if let Some(rest) = line.strip_prefix('+') {
            rest
        } else {
            continue;
        };
        // Track def boundary.
        if let Some(c) = def_re.captures(body) {
            let name = c.get(1).unwrap().as_str().to_string();
            cur_fn = Some(name.clone());
            per_fn.entry(name.clone()).or_insert_with(|| PerTestFn {
                name,
                asserts: Vec::new(),
            });
            continue;
        }
        let Some(fname) = &cur_fn else { continue };
        let entry = per_fn.entry(fname.clone()).or_insert_with(|| PerTestFn {
            name: fname.clone(),
            asserts: Vec::new(),
        });

        // Order of attempts: more-specific patterns first so generic
        // truthy doesn't swallow `assert X == Y`.
        if let Some(c) = raises_re.captures(body) {
            let exc = c.get(1).unwrap().as_str().to_string();
            let match_str = c.get(2).map(|m| m.as_str().trim().to_string());
            entry.asserts.push(AssertedItem {
                kind: "raises".into(),
                expr: "(context block)".into(),
                expected: Some(exc),
                expected2: match_str,
                raw: body.trim().to_string(),
            });
            continue;
        }
        if let Some(c) = assert_eq_re.captures(body) {
            let lhs = c.get(1).unwrap().as_str().trim().to_string();
            let op = c.get(2).unwrap().as_str().trim().to_string();
            let rhs = c.get(3).unwrap().as_str().trim().to_string();
            entry.asserts.push(AssertedItem {
                kind: format!("assert_{}", op_to_kind(&op)),
                expr: lhs,
                expected: Some(rhs),
                expected2: None,
                raw: body.trim().to_string(),
            });
            continue;
        }
        if let Some(c) = assert_equal_re.captures(body) {
            let a = c.get(1).unwrap().as_str().trim().to_string();
            let b = c.get(2).unwrap().as_str().trim().to_string();
            entry.asserts.push(AssertedItem {
                kind: "assertEqual".into(),
                expr: a,
                expected: Some(b),
                expected2: None,
                raw: body.trim().to_string(),
            });
            continue;
        }
        if let Some(c) = assert_raises_re.captures(body) {
            let exc = c.get(1).unwrap().as_str().to_string();
            entry.asserts.push(AssertedItem {
                kind: "assertRaises".into(),
                expr: "(callable)".into(),
                expected: Some(exc),
                expected2: None,
                raw: body.trim().to_string(),
            });
            continue;
        }
        if let Some(c) = assert_truefalse_re.captures(body) {
            let kind_str = c.get(1).unwrap().as_str().to_string();
            let expr = c.get(2).unwrap().as_str().trim().to_string();
            entry.asserts.push(AssertedItem {
                kind: kind_str,
                expr,
                expected: None,
                expected2: None,
                raw: body.trim().to_string(),
            });
            continue;
        }
        if let Some(c) = assert_isinstance_re.captures(body) {
            let x = c.get(1).unwrap().as_str().trim().to_string();
            let cls = c.get(2).unwrap().as_str().trim().to_string();
            entry.asserts.push(AssertedItem {
                kind: "assertIsInstance".into(),
                expr: x,
                expected: Some(cls),
                expected2: None,
                raw: body.trim().to_string(),
            });
            continue;
        }
        if let Some(c) = assert_truthy_re.captures(body) {
            let body_trim = c.get(1).unwrap().as_str().trim();
            // Skip if it actually looked like a == / != we missed (the
            // truthy regex is non-anchored at the operator so it can over-
            // match). Suppress when body contains == or !=.
            if body_trim.contains("==") || body_trim.contains("!=") {
                continue;
            }
            entry.asserts.push(AssertedItem {
                kind: "truthy".into(),
                expr: body_trim.to_string(),
                expected: None,
                expected2: None,
                raw: body.trim().to_string(),
            });
        }
    }

    // Drop test functions with zero asserts (probably setup/scaffolding).
    per_fn.retain(|_, v| !v.asserts.is_empty());
    Ok(per_fn)
}

fn op_to_kind(op: &str) -> &'static str {
    match op {
        "==" => "eq",
        "!=" => "ne",
        "is" => "is",
        "is not" | "is\u{a0}not" => "is_not",
        "in" => "in",
        "not in" | "not\u{a0}in" => "not_in",
        _ => "op",
    }
}

fn render_directive(per_fn: &BTreeMap<String, PerTestFn>) -> String {
    let mut s = String::new();
    s.push_str("\n## Test-assertion expectations (graph-derived from test_patch)\n\n");
    s.push_str(
        "The F2P tests assert the expressions below. The fix must make each \
         expression evaluate to its expected value (or raise the expected \
         exception) — these are the BEHAVIOR requirements, complementary to \
         the targeting-side directives. Read these to know what the implementation \
         must produce post-fix, not just which function to edit.\n\n",
    );
    for (name, tf) in per_fn.iter() {
        s.push_str(&format!("### {name}\n\n"));
        for a in &tf.asserts {
            match a.kind.as_str() {
                "assert_eq" => s.push_str(&format!(
                    "- Expression `{}` must equal `{}`.\n", a.expr,
                    a.expected.as_deref().unwrap_or("?"),
                )),
                "assert_ne" => s.push_str(&format!(
                    "- Expression `{}` must NOT equal `{}`.\n", a.expr,
                    a.expected.as_deref().unwrap_or("?"),
                )),
                "assert_is" => s.push_str(&format!(
                    "- `{}` must be (identity) `{}`.\n", a.expr,
                    a.expected.as_deref().unwrap_or("?"),
                )),
                "assert_is_not" => s.push_str(&format!(
                    "- `{}` must NOT be (identity) `{}`.\n", a.expr,
                    a.expected.as_deref().unwrap_or("?"),
                )),
                "assert_in" => s.push_str(&format!(
                    "- `{}` must appear in `{}`.\n", a.expr,
                    a.expected.as_deref().unwrap_or("?"),
                )),
                "assert_not_in" => s.push_str(&format!(
                    "- `{}` must NOT appear in `{}`.\n", a.expr,
                    a.expected.as_deref().unwrap_or("?"),
                )),
                "assertEqual" => s.push_str(&format!(
                    "- `{}` must equal `{}` (self.assertEqual).\n", a.expr,
                    a.expected.as_deref().unwrap_or("?"),
                )),
                "raises" | "assertRaises" => {
                    let exc = a.expected.as_deref().unwrap_or("?");
                    if let Some(m) = &a.expected2 {
                        s.push_str(&format!(
                            "- Code under test must raise `{exc}` (message matching `{m}`).\n"
                        ));
                    } else {
                        s.push_str(&format!(
                            "- Code under test must raise `{exc}`.\n"
                        ));
                    }
                }
                "assertTrue" => s.push_str(&format!(
                    "- `{}` must evaluate truthy.\n", a.expr,
                )),
                "assertFalse" => s.push_str(&format!(
                    "- `{}` must evaluate falsy.\n", a.expr,
                )),
                "assertIsInstance" => s.push_str(&format!(
                    "- `{}` must be an instance of `{}`.\n",
                    a.expr, a.expected.as_deref().unwrap_or("?"),
                )),
                "truthy" => s.push_str(&format!(
                    "- `{}` must evaluate truthy.\n", a.expr,
                )),
                other => s.push_str(&format!(
                    "- ({other}) {}\n", a.raw,
                )),
            }
        }
        s.push('\n');
    }
    s.push_str(
        "Each assertion above describes one BEHAVIOR the fix must guarantee. \
         Pick the implementation site (named in the targeting blocks) and the \
         smallest change that makes the asserted expressions evaluate as \
         specified. Emit a minimal unified diff.\n",
    );
    s
}

fn write_empty_directive(path: &PathBuf, reason: &str) -> Result<()> {
    let s = format!(
        "\n## Test-assertion expectations (graph-derived from test_patch)\n\n\
         (no assertion directive: {reason})\n",
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, s.as_bytes())?;
    eprintln!(
        "[synth_test_assertion] wrote empty directive: {} (reason: {reason})",
        path.display()
    );
    Ok(())
}
