//! synth_derived_notes — assemble the final outcome-style `## Derived notes`
//! block that the candle runner reads.
//!
//! Consumes prior synth stage outputs (source_cells.json from synth_key_symbols
//! and summaries-aplus.json from synth_composition + synth_callsite_argflow)
//! plus the raw issue text, and emits the three-bullet outcome-style block
//! proven at 7B Q4 by the G6 SOLVE arm on marshmallow-1359:
//!
//!   1. Chain-walker note  — Build 4 logic, regex on walker methods.
//!   2. Polymorphism note  — parses Build 3 callsite arg-flow cells.
//!   3. Attr-presence note — parses Build 2 attr-presence cells in source_cells.
//!
//! Block-level phrasing: every bullet uses the same outcome-style register.
//! G5 proved a single-bullet swap is byte-identical to not swapping; G6 proved
//! whole-block commit flips MISS→SOLVE. The assembler here enforces the
//! invariant by owning all three bullet renderers.
//!
//! This bin does **not** touch the graph. It's a pure formatter over the
//! JSON artifacts the upstream synth bins have already written. Rust owns
//! the final prose; `run_instance.py` is a thin pipeline driver that invokes
//! this bin and splices the output into the prefix.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;
use serde_json::Value;

#[derive(Parser, Debug)]
#[command(about = "Assemble the outcome-style `## Derived notes` block from prior synth outputs")]
struct Args {
    /// source_cells.json from synth_key_symbols. Each cell is
    /// `{qname, file, source, rank, reason}`. Source can be real code or a
    /// `# Attribute `.X` presence` comment block (Build 2 output).
    #[arg(long)]
    source_cells: PathBuf,

    /// summaries JSON (typically summaries-aplus.json) — the callsite-argflow
    /// cells Build 3 appended live here, with `summary` starting
    /// `# Callsite arg-flow for polymorphic method ...`.
    #[arg(long)]
    summaries: PathBuf,

    /// Issue / problem-statement text file. Drives CamelCase augmentation of
    /// the attr-presence absent-list and in-issue disambiguation for the
    /// polymorphism bullet's leaf pick.
    #[arg(long)]
    issue: PathBuf,

    /// Output path for the rendered block text. If omitted, write to stdout.
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Debug)]
struct SourceCell {
    qname: String,
    source: String,
}

#[derive(Debug)]
struct SummaryCell {
    summary: String,
}

fn load_source_cells(path: &PathBuf) -> Result<Vec<SourceCell>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let v: Value = serde_json::from_slice(&bytes).context("parse source_cells JSON")?;
    let arr = v.as_array().context("source_cells root must be an array")?;
    let mut out = Vec::with_capacity(arr.len());
    for cell in arr {
        let qname = cell
            .get("qname")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        let source = cell
            .get("source")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        out.push(SourceCell { qname, source });
    }
    Ok(out)
}

fn load_summaries(path: &PathBuf) -> Result<Vec<SummaryCell>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let v: Value = serde_json::from_slice(&bytes).context("parse summaries JSON")?;
    let arr = v.as_array().context("summaries root must be an array")?;
    let mut out = Vec::with_capacity(arr.len());
    for cell in arr {
        let summary = cell
            .get("summary")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        out.push(SummaryCell { summary });
    }
    Ok(out)
}

/// Build 4 — chain-walker bullet.
///
/// Detects the `while hasattr(x, "attr"): x = x.attr` idiom in any source cell
/// and renders it as an outcome-style claim: *every X has .Y and .Z (walks .Y
/// until it hits the outermost T). Both are set during M.*
///
/// Cross-bullet coupling: when `poly_delegating` is non-empty (the polymorphism
/// bullet identified container wrappers), the walked-attr gets an inline
/// elaboration *"(the immediate container — either a containing `C1`/`C2` field
/// or the outer `T`)"*. Matches G6 phrasing where bullet 1 references bullet 2's
/// container list.
fn chain_bullet(cells: &[SourceCell], poly_delegating: &[String]) -> Option<String> {
    let walker = Regex::new(
        r#"while\s+hasattr\(\s*(\w+)\s*,\s*["'](\w+)["']\s*\)\s*:\s*\n\s+(?:\w+)\s*=\s*(?:\w+)\.\w+"#,
    )
    .ok()?;
    let terminal = Regex::new(r"isinstance\(\s*\w+\s*,\s*(\w+)\s*\)").ok()?;

    for cell in cells {
        let Some(m) = walker.captures(&cell.source) else {
            continue;
        };
        let walked_attr = m.get(2)?.as_str().to_string();
        let parts: Vec<&str> = cell.qname.split("::").collect();
        if parts.len() < 2 {
            continue;
        }
        let method_name = parts[parts.len() - 1].to_string();
        let class_name = parts[parts.len() - 2].to_string();

        let terminal_name = terminal
            .captures(&cell.source)
            .and_then(|tm| tm.get(1).map(|g| g.as_str().to_string()))
            .map(|t| t.replace("ABC", ""))
            .unwrap_or_else(|| "target container".to_string());

        // Find a setter method elsewhere that assigns `self.<walked_attr> = …`.
        let setter_pat = format!(r"self\.{}\s*=", regex::escape(&walked_attr));
        let setter_re = Regex::new(&setter_pat).ok()?;
        let mut setter_method: Option<String> = None;
        for other in cells {
            if other.qname == cell.qname {
                continue;
            }
            if other.source.is_empty() || other.source.starts_with("# ") {
                continue;
            }
            if setter_re.is_match(&other.source) {
                if let Some(simple) = other.qname.rsplit("::").next() {
                    setter_method = Some(simple.to_string());
                    break;
                }
            }
        }

        let parent_elab = if !poly_delegating.is_empty() {
            let containers = poly_delegating
                .iter()
                .take(2)
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join("/");
            format!(
                " (the immediate container — either a containing {containers} field or the outer `{terminal_name}`)"
            )
        } else {
            String::new()
        };

        let mut out = format!(
            "Every `{class_name}` has `.{walked_attr}`{parent_elab} and `.{method_name}` \
             (walks `.{walked_attr}` until it hits the outermost `{terminal_name}`)."
        );
        if let Some(setter) = setter_method {
            out.push_str(&format!(" Both are set during `{setter}`."));
        }
        return Some(out);
    }
    None
}

/// Structurally detect top-level binder classes from a callsite-argflow
/// summary. A class C is a binder for `poly_method` when it has a method
/// `M ≠ poly_method` that calls `poly_method` (i.e. the call originates
/// outside the polymorphic chain itself). Wrapper Fields call `poly_method`
/// from inside their own `poly_method` impl — those are the real wrappers,
/// not binders.
///
/// Why structural detection vs. a name suffix heuristic: the polymorphism
/// bullet must distinguish container Fields (wrappers like `List`/`Tuple`)
/// from the top-level binder (e.g. `Schema`/`BaseSchema`). A name-suffix rule
/// like `ends_with("Schema")` is marshmallow-specific and produces false
/// positives in unrelated repos. The "housing method ≠ poly method" property
/// is a structural fact of the call graph, so it generalizes across
/// frameworks (Django `Model._meta` mapper loops, SQLAlchemy mapper
/// configure passes, etc.).
fn detect_binders_in_summary(summary: &str, poly_method: &str) -> HashSet<String> {
    let mut binders = HashSet::new();
    if poly_method.is_empty() {
        return binders;
    }
    let line_re = match Regex::new(r"#\s+In\s+`([^`]+)`:") {
        Ok(r) => r,
        Err(_) => return binders,
    };
    for line in summary.lines() {
        let Some(cap) = line_re.captures(line) else {
            continue;
        };
        let Some(qname_match) = cap.get(1) else {
            continue;
        };
        let parts: Vec<&str> = qname_match.as_str().split("::").collect();
        if parts.len() < 2 {
            continue;
        }
        let housing_method = parts[parts.len() - 1];
        let housing_class = parts[parts.len() - 2];
        if housing_method != poly_method {
            binders.insert(housing_class.to_string());
        }
    }
    binders
}

/// Parse one Build 3 callsite-argflow summary cell.
///
/// Returns (method_name, defined_classes, delegating_callers).
/// `delegating_callers` = the set of caller-classes that invoke the method via
/// a **non-self** receiver while passing `self` (=caller) as an argument. That
/// shape signals usage-typed polymorphism (container wraps leaf + delegates).
fn parse_callsite_summary(summary: &str) -> (String, Vec<String>, Vec<String>) {
    let name_re = Regex::new(r"polymorphic method `(\w+)`").unwrap();
    let defined_re = Regex::new(r"# Defined on classes: ([^\n]+)").unwrap();
    // `# In `qname`: `call(...)`` — extract the call text.
    let line_re = Regex::new(r"#\s+In\s+`([^`]+)`:\s+`([^`]+)`").unwrap();
    let recv_re = Regex::new(r"^([^.]+(?:\.[^(]+)?)\.\w+\(").unwrap();
    let arg_re = Regex::new(r"self \(=(\w+)\)").unwrap();

    let name = name_re
        .captures(summary)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        .unwrap_or_default();

    let defined: Vec<String> = defined_re
        .captures(summary)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut delegating: Vec<String> = Vec::new();
    for line in summary.lines() {
        let Some(lm) = line_re.captures(line) else {
            continue;
        };
        let callsite = match lm.get(2) {
            Some(c) => c.as_str(),
            None => continue,
        };
        let Some(rm) = recv_re.captures(callsite) else {
            continue;
        };
        let receiver = rm.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        let Some(am) = arg_re.captures(callsite) else {
            continue;
        };
        let container = am.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
        if receiver != "self" && !delegating.contains(&container) {
            delegating.push(container);
        }
    }

    (name, defined, delegating)
}

/// Pre-computed polymorphism facts. Shared across all three bullet renderers
/// so the rendered block reads as one cross-coupled observation, not three
/// independent ones (per G6 SOLVE block-level phrasing — see
/// `feedback_derived_notes_bullets_need_cross_coupling.md`).
#[derive(Default, Clone)]
struct PolyFacts {
    method: String,                    // e.g. "_bind_to_schema"
    leaf: String,                      // e.g. "DateTime"
    delegating: Vec<String>,           // e.g. ["List", "Tuple"]  (top-level binders filtered, sorted by issue-mention count)
    defined: Vec<String>,              // raw defined list from the Build 3 cell
    top_level_binder: Option<String>,  // the class the filter removed (e.g. "Schema"); used as the "outer X" anchor in the polymorphism bullet
}

/// Derive the top-level binder class name from chain-walker source cells.
/// The walker idiom `while hasattr(x, "parent") and not isinstance(x, T): x = x.parent`
/// terminates at class `T` — that is the "outer X" the polymorphism bullet
/// needs to name. For marshmallow, `Field.root` source yields `T = Schema`.
/// Used as fallback when Build 3's callsite cell doesn't include the binder
/// (e.g. the caller-side binding lives in a sibling polymorphic method whose
/// cell wasn't picked as `best`).
fn derive_top_level_binder(cells: &[SourceCell]) -> Option<String> {
    let walker = Regex::new(
        r#"while\s+hasattr\(\s*(\w+)\s*,\s*["'](\w+)["']\s*\)"#,
    )
    .ok()?;
    let terminal = Regex::new(r"isinstance\(\s*\w+\s*,\s*(\w+)\s*\)").ok()?;
    for cell in cells {
        if walker.is_match(&cell.source) {
            if let Some(tm) = terminal.captures(&cell.source) {
                if let Some(m) = tm.get(1) {
                    return Some(m.as_str().replace("ABC", ""));
                }
            }
        }
    }
    None
}

/// Select the best callsite-argflow cell by the same scoring the old
/// `polymorphism_bullet` used, and return its parsed facts with
/// top-level-binder classes filtered out of the delegating set.
///
/// The binder set is derived structurally per-candidate: the walker terminal
/// from `derive_top_level_binder` (passed in as `walker_binder`) plus any
/// class whose housing method (in the callsite-argflow summary) differs from
/// the polymorphic method name. No suffix heuristics — works across
/// frameworks regardless of naming convention.
fn select_best_callsite(
    summaries: &[SummaryCell],
    issue: &str,
    walker_binder: Option<&str>,
) -> Option<PolyFacts> {
    let candidates: Vec<&SummaryCell> = summaries
        .iter()
        .filter(|c| c.summary.trim_start().starts_with("# Callsite arg-flow"))
        .collect();
    if candidates.is_empty() {
        return None;
    }

    type Score = (i32, i32, i32, i32);
    let scored: Vec<(Score, &SummaryCell, HashSet<String>)> = candidates
        .iter()
        .map(|c| {
            let (name, defined, delegating) = parse_callsite_summary(&c.summary);
            if name.is_empty() {
                return ((-1, 0, 0, 0), *c, HashSet::new());
            }
            let mut binders = detect_binders_in_summary(&c.summary, &name);
            if let Some(b) = walker_binder {
                binders.insert(b.to_string());
            }
            let is_dunder = name.starts_with("__") && name.ends_with("__");
            let delegating_filt: Vec<&String> =
                delegating.iter().filter(|c| !binders.contains(*c)).collect();
            let has_delegation = if delegating_filt.is_empty() { 0 } else { 1 };
            let in_issue = if issue.contains(&name) { 1 } else { 0 };
            let def_count = defined.len() as i32;
            let def_score = if def_count <= 10 { -def_count } else { -999 };
            (
                (
                    if is_dunder { 0 } else { 1 },
                    has_delegation,
                    in_issue,
                    def_score,
                ),
                *c,
                binders,
            )
        })
        .collect();

    let best = scored.iter().max_by_key(|(s, _, _)| *s)?;
    let binders = &best.2;
    let (method, defined, delegating_raw) = parse_callsite_summary(&best.1.summary);
    if method.is_empty() {
        return None;
    }
    // Split the delegating list at the binder-set boundary. The first
    // delegating class that is also a binder becomes the "outer X" anchor in
    // the polymorphism bullet; the remainder are the container wrappers.
    let top_level_binder: Option<String> = delegating_raw
        .iter()
        .find(|c| binders.contains(*c))
        .cloned();
    let mut delegating: Vec<String> = delegating_raw
        .into_iter()
        .filter(|c| !binders.contains(c))
        .collect();

    // Rank container wrappers by word-boundary issue-mention count (descending,
    // stable). When the polymorphic method defines on {List, Tuple, Mapping}
    // but the issue only names List/Tuple, promote those so `.take(2)` in
    // `polymorphism_bullet` and `chain_bullet` renders the gold sibling pair.
    // Ties preserve the upstream callsite-argflow order.
    // `feedback_issue_mention_count_not_presence.md` — count mentions, not presence.
    let mut with_counts: Vec<(String, usize)> = delegating
        .iter()
        .map(|c| {
            let pat = format!(r"\b{}\b", regex::escape(c));
            let count = Regex::new(&pat)
                .ok()
                .map(|re| re.find_iter(issue).count())
                .unwrap_or(0);
            (c.clone(), count)
        })
        .collect();
    with_counts.sort_by(|a, b| b.1.cmp(&a.1));
    delegating = with_counts.into_iter().map(|(c, _)| c).collect();

    let non_delegating: Vec<&String> = defined
        .iter()
        .filter(|c| !delegating.contains(c) && !binders.contains(*c))
        .collect();
    let issue_leafs: Vec<&&String> =
        non_delegating.iter().filter(|c| issue.contains(c.as_str())).collect();
    let leaf = if let Some(l) = issue_leafs.first() {
        (**l).clone()
    } else if let Some(l) = non_delegating.first() {
        (*l).clone()
    } else if let Some(l) = defined.first() {
        l.clone()
    } else {
        String::new()
    };
    Some(PolyFacts { method, leaf, delegating, defined, top_level_binder })
}

/// Build 3 → outcome-style polymorphism bullet. When `attr_name` is provided
/// (from the attr-presence bullet), appends the G6 bridge sentence
/// *"That container does not have a `.X` attribute."* — the explicit link
/// between the polymorphism observation and the attr-presence observation.
fn polymorphism_bullet(facts: &PolyFacts, attr_name: Option<&str>) -> Option<String> {
    if facts.method.is_empty() {
        return None;
    }
    if !facts.delegating.is_empty() {
        let wrap_str = facts
            .delegating
            .iter()
            .take(2)
            .map(|c| format!("`{c}`"))
            .collect::<Vec<_>>()
            .join("/");
        let leaf = if facts.leaf.is_empty() { "the leaf class".to_string() } else { facts.leaf.clone() };
        // Anchor phrase: name the top-level binder (e.g. `Schema`) explicitly.
        // The word that names what `self.root` returns is load-bearing — it
        // steers the model toward the `self.root.X` composition fix. Abstract
        // phrasing ("enclosing top-level context") collapsed to defensive
        // getattr in v4. See `project_0421_rust_only_v4_result.md`.
        let binder_anchor = match &facts.top_level_binder {
            Some(b) => format!("the outer `{b}`"),
            None => "the outer schema".to_string(),
        };
        let mut out = format!(
            "When `{leaf}` is used inside {wrap_str}, \
             `{name}` is called with the containing {wrap_str} instance as its argument, \
             not {binder_anchor}.",
            name = facts.method,
        );
        if let Some(a) = attr_name {
            out.push_str(&format!(" That container does not have a `.{a}` attribute."));
        }
        return Some(out);
    }

    Some(format!(
        "`{name}` is defined on {}. Its receiver at call-time determines which implementation runs.",
        facts
            .defined
            .iter()
            .map(|c| format!("`{c}`"))
            .collect::<Vec<_>>()
            .join(", "),
        name = facts.method,
    ))
}

/// Build 2 → outcome-style attr-presence bullet.
///
/// Parses a source cell whose `source` begins `# Attribute `.X` presence`.
/// Augments the absent-list with CamelCase class names mentioned in the issue
/// that appear in source_cells qnames (so e.g. `List` gets reported as absent
/// even if it didn't rank into the top-K source cells).
///
/// When multiple attr-presence cells exist (synth_key_symbols can emit one per
/// ranked attribute), prefer the one whose attribute name appears as a
/// dotted-access token in the issue text — that's almost always the attribute
/// the bug report is actually about. Falls back to source-cell order.
fn attr_presence_bullet(
    cells: &[SourceCell],
    issue: &str,
    extra_candidates: &[String],
) -> Option<(String, String)> {
    let name_re = Regex::new(r"# Attribute `\.(\w+)` presence").unwrap();
    let defined_re = Regex::new(r"# Defined on class: `(\w+)`").unwrap();
    let absent_re = Regex::new(r"# NOT defined on in-scope classes: ([^\n]+)").unwrap();
    let camel_re = Regex::new(r"\b([A-Z][A-Za-z0-9_]+)\b").unwrap();
    let issue_dot_re = Regex::new(r"\.([a-zA-Z_][a-zA-Z0-9_]*)").unwrap();

    // Count dotted-access occurrences per attribute name in the issue. More
    // mentions = more likely to be the attribute the bug report is about.
    let mut issue_dot_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for cap in issue_dot_re.captures_iter(issue) {
        if let Some(m) = cap.get(1) {
            *issue_dot_counts.entry(m.as_str().to_string()).or_insert(0) += 1;
        }
    }

    // Collect candidate cells with their parsed attribute name, then rank by
    // issue-mention count (higher wins). Stable sort preserves source order
    // on ties (so zero-mention attrs still respect synth_key_symbols rank).
    let mut candidates: Vec<(&SourceCell, String, usize)> = cells
        .iter()
        .filter(|c| c.source.starts_with("# Attribute `."))
        .filter_map(|c| {
            let attr = name_re
                .captures(&c.source)
                .and_then(|m| m.get(1).map(|g| g.as_str().to_string()))?;
            let count = issue_dot_counts.get(&attr).copied().unwrap_or(0);
            Some((c, attr, count))
        })
        .collect();
    candidates.sort_by(|a, b| b.2.cmp(&a.2));

    for (cell, _attr, _) in candidates {
        let Some(n) = name_re.captures(&cell.source).and_then(|c| c.get(1)) else {
            continue;
        };
        let Some(d) = defined_re.captures(&cell.source).and_then(|c| c.get(1)) else {
            continue;
        };
        let name = n.as_str().to_string();
        let defined = d.as_str().to_string();

        let absent_raw = absent_re
            .captures(&cell.source)
            .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
            .unwrap_or_default();
        let mut absent_list: Vec<String> =
            if absent_raw.is_empty() || absent_raw == "(none)" {
                Vec::new()
            } else {
                absent_raw
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            };

        // Augment: any CamelCase class in the issue that also appears as a
        // qname parent in source_cells, but isn't the defining class and isn't
        // already listed as absent.
        let issue_classes: Vec<String> = camel_re
            .captures_iter(issue)
            .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
            .collect();
        let mut augment_sources: std::collections::HashSet<String> = cells
            .iter()
            .filter_map(|c| {
                let parts: Vec<&str> = c.qname.split("::").collect();
                if parts.len() >= 2 {
                    Some(parts[parts.len() - 2].to_string())
                } else {
                    None
                }
            })
            .collect();
        // Cross-bullet coupling: accept polymorphism-bullet's `defined` classes
        // (container leaves like `List`/`Tuple`) as absent-list candidates even
        // if they don't have a source_cell. Gates on issue-mention to keep
        // fallout narrow. Matches G6 absent-list `Field, List, Tuple, DateTime`.
        for c in extra_candidates {
            if issue.contains(c.as_str()) {
                augment_sources.insert(c.clone());
            }
        }
        for cname in issue_classes {
            if augment_sources.contains(&cname)
                && cname != defined
                && !absent_list.contains(&cname)
            {
                absent_list.push(cname);
            }
        }

        let mut parts = vec![format!("`.{name}` is defined on class `{defined}`.")];
        if !absent_list.is_empty() {
            parts.push(format!(
                "It is NOT defined on {}.",
                absent_list
                    .iter()
                    .map(|c| format!("`{c}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            let primary_absent = &absent_list[0];
            parts.push(format!(
                "Only `{defined}` instances carry `.{name}`; plain `{primary_absent}` instances do not."
            ));
        }
        return Some((parts.join(" "), name));
    }
    None
}

fn main() -> Result<()> {
    let args = Args::parse();

    let cells = load_source_cells(&args.source_cells)?;
    let summaries = load_summaries(&args.summaries)?;
    let issue =
        fs::read_to_string(&args.issue).with_context(|| format!("read {}", args.issue.display()))?;

    // Pre-compute shared facts so bullets can cross-reference. Order:
    // poly facts → attr bullet (uses poly.defined) → poly bullet (uses attr
    // name) → chain bullet (uses poly.delegating). Rendered in the original
    // chain/poly/attr order.
    //
    // Walker terminal class (e.g. marshmallow `Schema` from the
    // `Field.root` walker) seeds the binder set used to filter the
    // delegating list inside `select_best_callsite`. The structural binder
    // detector inside that function adds any additional iterate-and-dispatch
    // class for the candidate's polymorphic method.
    let walker_binder = derive_top_level_binder(&cells);
    let mut poly_facts =
        select_best_callsite(&summaries, &issue, walker_binder.as_deref())
            .unwrap_or_default();
    if poly_facts.top_level_binder.is_none() {
        poly_facts.top_level_binder = walker_binder;
    }
    let attr_bullet = attr_presence_bullet(&cells, &issue, &poly_facts.defined);
    let attr_name: Option<String> = attr_bullet.as_ref().map(|(_, n)| n.clone());

    let mut bullets: Vec<String> = Vec::new();
    if let Some(b) = chain_bullet(&cells, &poly_facts.delegating) {
        bullets.push(b);
    }
    if let Some(b) = polymorphism_bullet(&poly_facts, attr_name.as_deref()) {
        bullets.push(b);
    }
    if let Some((b, _)) = attr_bullet {
        bullets.push(b);
    }

    let block = if bullets.is_empty() {
        String::new()
    } else {
        let mut s = String::from("## Derived notes (from symbols above)\n\n");
        for b in &bullets {
            s.push_str(&format!("- {b}\n"));
        }
        s.push('\n');
        s
    };

    match args.out {
        Some(path) => fs::write(&path, &block)
            .with_context(|| format!("write {}", path.display()))?,
        None => print!("{}", block),
    }
    Ok(())
}
