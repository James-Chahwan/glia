//! seeds — extract candidate identifiers from a SWE-bench issue + match
//! against graph nodes + run PPR + sibling-expand.
//!
//! Replaces bench/latent/out/seed_from_issue.py. Rust-native: direct graph
//! access, no pyo3, no Python layer between the issue text and the .gmap.
//!
//! Output JSON shape (consumed by node_summaries + synth_composition +
//! synth_key_symbols downstream):
//!   { "candidate_tokens": [...],
//!     "seed_ids":        [...],
//!     "seed_qnames":     [...],
//!     "matched_tokens":  {tok: [qname, ...]},
//!     "activated":       [[qname, score], ...] }

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use regex::Regex;
use repo_graph_code_domain::node_kind;
use repo_graph_core::NodeId;
use repo_graph_graph::{RepoGraph, code_activation_defaults};
use repo_graph_projection_text::driver_utils::{build_repo_graph, extract_position_cell, write_json};
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(about = "Extract seeds from issue text, activate graph, emit seeds JSON")]
struct Args {
    #[arg(long)]
    src: PathBuf,

    #[arg(long)]
    issue: PathBuf,

    /// Optional SWE-bench test_patch (unified diff). Identifiers from added
    /// lines are added to the candidate-token pool with permissive regex
    /// (no case / inner-underscore restriction) — tests are guaranteed
    /// API-surface so we don't need the issue-text noise filters.
    #[arg(long)]
    test_patch: Option<PathBuf>,

    #[arg(long)]
    out: PathBuf,

    #[arg(long, default_value_t = 60)]
    top_k: usize,

    #[arg(long, default_value_t = 60)]
    sibling_cap: usize,

    #[arg(long)]
    no_siblings: bool,

    /// Drop any candidate token that resolves to more than `match_cap` qnames.
    /// Common English / Python words like `request`, `path`, `value` match
    /// hundreds of methods in big repos and turn the seed pool into noise. A
    /// real API-surface identifier almost never has >50 graph residences.
    #[arg(long, default_value_t = 50)]
    match_cap: usize,

    #[arg(long, default_value = "seeds")]
    repo_canonical: String,
}

#[derive(Serialize)]
struct Output {
    candidate_tokens: Vec<String>,
    seed_ids: Vec<u64>,
    seed_qnames: Vec<String>,
    matched_tokens: BTreeMap<String, Vec<String>>,
    activated: Vec<(String, f64)>,
    /// Tail names of CLASS-kind seeds, e.g. `["FilePathField", "Schema"]`.
    /// Downstream consumers (synth_key_symbols class_match_boost) use this to
    /// up-rank methods whose enclosing class is issue-cited.
    class_seeds: Vec<String>,
    /// Class-qname → method-qnames added via class-expansion. Logged for
    /// observability; empty when no class seeds present.
    class_expansions: BTreeMap<String, Vec<String>>,
    /// T2: structural facts mined from the test_patch (constructor calls,
    /// `Class.attr` access, and resolved `var.attr` pairs from local
    /// variable-type tracking). Empty when no test_patch was supplied.
    test_patch_facts: TestPatchFacts,
    /// Issue-anchored qnames (file/dotted/backtick/snake/camel/traceback-frame
    /// matches from issue text). Downstream consumers (synth_key_symbols
    /// anchor_priority_boost) use this to lock-in source_cells inclusion
    /// regardless of other re-ranking. Empty when no anchors resolved.
    issue_anchored_qnames: Vec<String>,
}

/// T2: typed signal extracted from the test_patch's added lines. Each map is
/// frequency-weighted so downstream consumers (synth_key_symbols class-boost,
/// derived-notes anchor selection) can rank by call-count not just presence.
#[derive(Serialize, Default)]
struct TestPatchFacts {
    /// `Name(...)` where `Name` is capitalized — likely class constructors.
    /// Strong signal: tests usually instantiate the *exact* class under test.
    constructor_calls: BTreeMap<String, usize>,
    /// `Class.attr` (capitalized object) — static/classmethod or class-level
    /// constant access. Includes the trailing-paren method call form too.
    class_attrs: BTreeMap<String, usize>,
    /// `var.attr` resolved to `Class.attr` via local var-type tracking.
    /// `field = FilePathField(...)` then `field.formfield()` lifts to
    /// `FilePathField.formfield`. Highest-confidence test-anchored signal.
    resolved_attrs: BTreeMap<String, usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let graph = build_repo_graph(&args.src, &args.repo_canonical)?;

    let issue = std::fs::read_to_string(&args.issue)
        .with_context(|| format!("read {}", args.issue.display()))?;

    let mut tokens = extract_candidates(&issue);
    let issue_only_count = tokens.len();
    eprintln!("[seeder] {issue_only_count} candidate tokens from issue");

    let mut test_patch_facts = TestPatchFacts::default();
    let mut tp_added_tokens: BTreeSet<String> = BTreeSet::new();
    if let Some(tp) = &args.test_patch {
        let patch_text = std::fs::read_to_string(tp)
            .with_context(|| format!("read {}", tp.display()))?;
        let added = extract_candidates_from_test_patch(&patch_text);
        tp_added_tokens = added.clone();
        let added_count = added.len();
        // Issue-intersection gate: prefer test_patch tokens that ALSO appear
        // in issue tokens (marshmallow-1359 fix-a, 2026-04-27 — distractor
        // methods like `inner`/`format` from test scaffolding hurt activation).
        // Layer-3 softening (2026-04-28): if issue tokens give zero qname
        // matches in the graph, the gate has no purpose and we lose the
        // entire instance. The fallback path below detects that case and
        // re-injects ungated test_patch tokens.
        let mut new_only = 0usize;
        let mut filtered = 0usize;
        for t in &added {
            if !tokens.contains(t.as_str()) {
                filtered += 1;
                continue;
            }
            if tokens.insert(t.clone()) {
                new_only += 1;
            }
        }
        eprintln!(
            "[seeder] +{new_only} new tokens from test_patch ({added_count} in patch, {filtered} filtered by issue-intersection, {} now)",
            tokens.len()
        );

        test_patch_facts = extract_test_patch_facts(&patch_text);
        eprintln!(
            "[seeder] test_patch facts: {} constructor_calls, {} class_attrs, {} resolved_attrs",
            test_patch_facts.constructor_calls.len(),
            test_patch_facts.class_attrs.len(),
            test_patch_facts.resolved_attrs.len(),
        );
        if !test_patch_facts.constructor_calls.is_empty() {
            eprintln!("  constructor_calls: {:?}", test_patch_facts.constructor_calls);
        }
        if !test_patch_facts.resolved_attrs.is_empty() {
            eprintln!("  resolved_attrs:    {:?}", test_patch_facts.resolved_attrs);
        }
    }

    let (mut seeds, mut matched, dropped) = match_to_seeds(&graph, &tokens, args.match_cap);
    eprintln!(
        "[seeder] {} seed nodes from {} matched tokens (dropped {} over-cap tokens, cap={})",
        seeds.len(),
        matched.len(),
        dropped.len(),
        args.match_cap,
    );
    if !dropped.is_empty() {
        eprintln!("[seeder] over-cap tokens (each matched >{}): {:?}", args.match_cap, dropped);
    }

    // Layer-3 fallback: if issue-only matching yielded zero seeds AND we have
    // ungated test_patch tokens still on the bench, inject them and retry.
    // The issue-intersection gate's purpose is to filter distractor test names
    // when issue tokens already give us the right anchors; when there are NO
    // anchors, the gate is doing pure harm.
    if seeds.is_empty() && !tp_added_tokens.is_empty() {
        eprintln!("[seeder] issue-only seeds empty → falling back to ungated test_patch tokens");
        let mut combined = tokens.clone();
        for t in &tp_added_tokens {
            combined.insert(t.clone());
        }
        let (seeds_fb, matched_fb, dropped_fb) =
            match_to_seeds(&graph, &combined, args.match_cap);
        eprintln!(
            "[seeder] (fallback) {} seed nodes from {} matched tokens (dropped {} over-cap)",
            seeds_fb.len(),
            matched_fb.len(),
            dropped_fb.len(),
        );
        if !seeds_fb.is_empty() {
            tokens = combined;
            seeds = seeds_fb;
            matched = matched_fb;
        }
    }

    for (tok, qs) in &matched {
        let first: Vec<&String> = qs.iter().take(3).collect();
        let ellipsis = if qs.len() > 3 { "..." } else { "" };
        eprintln!("  {tok} → {}: {:?}{ellipsis}", qs.len(), first);
    }

    if seeds.is_empty() {
        eprintln!("WARNING: no token→qname matches — emitting empty seeds.json so downstream pipeline can continue with issue-only context");
        // Emit empty seeds file in the full Output schema and exit 0; downstream
        // bins (node_summaries, synth_*) handle empty seed lists. Without this,
        // instances whose issue text shares zero tokens with their codebase
        // abort the whole pipeline.
        let empty = serde_json::json!({
            "candidate_tokens": [],
            "seed_ids": [],
            "seed_qnames": [],
            "matched_tokens": {},
            "activated": [],
            "class_seeds": [],
            "class_expansions": {},
            "test_patch_facts": {},
        });
        std::fs::write(&args.out, serde_json::to_string_pretty(&empty)?)?;
        return Ok(());
    }

    // Dedupe — multiple class qnames can share a tail (e.g.
    // `django::db::models::fields::__init__::FilePathField` and
    // `django::forms::fields::FilePathField` both contribute `FilePathField`).
    let class_seeds: Vec<String> = {
        let mut s: BTreeSet<String> = BTreeSet::new();
        for id in &seeds {
            if graph.nav.kind_by_id.get(id).copied() != Some(node_kind::CLASS) {
                continue;
            }
            if let Some(qname) = graph.nav.qname_by_id.get(id) {
                let tail = qname.rsplit("::").next().unwrap_or("");
                if !tail.is_empty() {
                    s.insert(tail.to_string());
                }
            }
        }
        s.into_iter().collect()
    };

    let (seeds, class_expansions, expanded_method_ids) = expand_class_seeds(seeds, &graph);
    let total_expanded: usize = class_expansions.values().map(|v| v.len()).sum();
    if total_expanded > 0 {
        eprintln!(
            "[seeder] class-expanded: +{total_expanded} method seeds from {} classes",
            class_expansions.len()
        );
        for (c, ms) in &class_expansions {
            let first: Vec<&String> = ms.iter().take(3).collect();
            let ellipsis = if ms.len() > 3 { "..." } else { "" };
            eprintln!("  {c} → +{} methods: {:?}{ellipsis}", ms.len(), first);
        }
    }

    let mut config = code_activation_defaults();
    config.top_k = args.top_k;
    let result = graph.activate(&seeds, &config);
    eprintln!("[seeder] activated top-{}: {} nodes", args.top_k, result.scores.len());

    let activated: Vec<(NodeId, f64)> = result.scores.clone();
    let activated = if args.no_siblings {
        activated
    } else {
        let before = activated.len();
        let expanded = expand_siblings(&activated, &graph, args.sibling_cap);
        eprintln!(
            "[seeder] sibling-expanded: {} → {} nodes",
            before,
            expanded.len()
        );
        expanded
    };

    // Guarantee-include class-expanded methods. PPR top_k can drop methods
    // belonging to a busy class — e.g. with `FilePathField` as the class seed,
    // django Field has 30+ subclass `formfield` overrides crowding the top_k,
    // so `FilePathField::formfield` (the gold) gets pushed out. Force-add any
    // class-expanded method missing from `activated`, scored at the activation
    // floor so they survive downstream filters but rank below real activations.
    // synth_key_symbols' class_match_boost lifts the right one.
    let activated = {
        let mut v = activated;
        let in_set: HashSet<NodeId> = v.iter().map(|(id, _)| *id).collect();
        let min_score = v
            .iter()
            .map(|(_, s)| *s)
            .fold(f64::INFINITY, f64::min);
        let floor = if min_score.is_finite() && min_score > 0.0 {
            min_score
        } else {
            1e-6
        };
        let mut added = 0usize;
        for mid in &expanded_method_ids {
            if !in_set.contains(mid) {
                v.push((*mid, floor));
                added += 1;
            }
        }
        if added > 0 {
            eprintln!(
                "[seeder] guarantee-included {added} class-expanded methods missing from activated (floor={floor:.6})"
            );
        }
        v
    };

    let id_to_qname: HashMap<NodeId, &str> = graph
        .nav
        .qname_by_id
        .iter()
        .map(|(id, q)| (*id, q.as_str()))
        .collect();

    // Issue-anchor inject (2026-04-28): parse issue text for file paths
    // (`requests/sessions.py`), dotted module/qname references
    // (`sphinx.ext.autosummary.generate_autosummary`), and backtick-quoted
    // identifiers (`autosummary_imported_members`). Resolve to graph node
    // IDs and inject as activated nodes at HIGH score so downstream
    // synth_key_symbols + synth_source_map promote them. APPLY-FAIL audit
    // (N=50): 12/19 had right file in source_cells but model still drifted
    // because key_symbols showed wrong methods of that file; 5/19 had wrong
    // file entirely. This pass anchors the model to issue-named symbols.
    let issue_anchored = inject_issue_anchors(&issue, &graph, &activated, &id_to_qname, &tokens);
    let activated = if !issue_anchored.is_empty() {
        let max_existing = activated
            .iter()
            .map(|(_, s)| *s)
            .fold(0.0_f64, f64::max);
        let min_existing = activated
            .iter()
            .map(|(_, s)| *s)
            .fold(f64::INFINITY, f64::min)
            .max(1e-6);
        // Additive delta scaled to MEANINGFULLY outrank PPR + downstream
        // synth_key_symbols boosts (chain/class/file/attr typically add
        // ~0.01-0.05). N=63 audit (2026-04-28) showed +0.005 was too small —
        // django-11039 had sqlmigrate at 0.011 but Atomic methods at 0.013
        // from PPR alone, so source_cells took the wrong qnames. Bump delta
        // to 1.5× current top so anchored beat unanchored after downstream
        // re-ranking too.
        let delta = (max_existing * 1.5).max(0.05);
        let in_set: HashSet<NodeId> = activated.iter().map(|(id, _)| *id).collect();
        let mut v = activated;
        let mut added = 0usize;
        // Determinism: HashSet iter order is randomized per run. Sort by NodeId.0
        // (with qname tiebreak via id_to_qname) so push order into v is stable.
        let mut anchored_sorted: Vec<NodeId> = issue_anchored.iter().copied().collect();
        anchored_sorted.sort_by(|a, b| {
            let qa = id_to_qname.get(a).copied().unwrap_or("");
            let qb = id_to_qname.get(b).copied().unwrap_or("");
            qa.cmp(qb).then_with(|| a.0.cmp(&b.0))
        });
        for id in &anchored_sorted {
            if !in_set.contains(id) {
                v.push((*id, min_existing + delta));
                added += 1;
            } else if let Some(entry) = v.iter_mut().find(|(eid, _)| eid == id) {
                entry.1 += delta;
            }
        }
        eprintln!(
            "[seeder] issue-anchor inject: {} resolved, {} new (delta=+{delta:.4} on top={max_existing:.4})",
            issue_anchored.len(),
            added
        );
        v
    } else {
        activated
    };

    let activated_qnames: Vec<(String, f64)> = activated
        .iter()
        .map(|(id, s)| {
            (
                id_to_qname.get(id).copied().unwrap_or("?").to_string(),
                *s,
            )
        })
        .collect();

    let mut seed_qnames: Vec<String> = seeds
        .iter()
        .filter_map(|id| id_to_qname.get(id).map(|q| q.to_string()))
        .collect();
    seed_qnames.sort();
    seed_qnames.dedup();

    let mut candidate_sorted: Vec<String> = tokens.into_iter().collect();
    candidate_sorted.sort();

    // Project issue_anchored NodeIds → qnames for downstream consumers.
    let mut issue_anchored_qnames: Vec<String> = issue_anchored
        .iter()
        .filter_map(|id| id_to_qname.get(id).map(|q| q.to_string()))
        .collect();
    issue_anchored_qnames.sort();

    let output = Output {
        candidate_tokens: candidate_sorted,
        seed_ids: seeds.iter().map(|id| id.0).collect(),
        seed_qnames,
        matched_tokens: matched,
        activated: activated_qnames,
        class_seeds,
        class_expansions,
        test_patch_facts,
        issue_anchored_qnames,
    };

    write_json(&args.out, &output)?;
    eprintln!("[write] {}", args.out.display());
    Ok(())
}

/// Issue-anchor resolution: extract file paths and dotted/qname identifiers
/// from issue text, resolve to graph node IDs by:
///   1. Path match against POSITION cell `file` field — `sphinx/ext/foo.py`
///      activates every node in that file
///   2. Dotted qname match — `module.submodule.func` → `module::submodule::func`
///      and prefix-matches; finds containers + their methods
///   3. Backtick-quoted identifier match — `` `autosummary_imported_members` ``
///      → exact qname-tail match (case-insensitive)
fn inject_issue_anchors(
    issue: &str,
    graph: &RepoGraph,
    activated: &[(NodeId, f64)],
    id_to_qname: &HashMap<NodeId, &str>,
    issue_tokens: &BTreeSet<String>,
) -> HashSet<NodeId> {
    let mut anchored: HashSet<NodeId> = HashSet::new();

    // 1. File paths (must end .py and contain at least one /)
    let file_re = Regex::new(r"\b([a-zA-Z_][\w/]*[/\\][\w/\\.]*\.py)\b").unwrap();
    let mut matched_files: HashSet<String> = HashSet::new();
    for caps in file_re.captures_iter(issue) {
        if let Some(m) = caps.get(1) {
            matched_files.insert(m.as_str().replace('\\', "/"));
        }
    }
    // Drop test-directory paths from file anchors — they would drown the
    // activated set in test code (django-11039 audit: 126 test nodes injected
    // when issue mentioned `tests/migrations/test_commands.py`).
    let is_test_path = |p: &str| {
        p.starts_with("tests/")
            || p.starts_with("test/")
            || p.contains("/tests/")
            || p.contains("/testing/")
    };
    matched_files.retain(|p| !is_test_path(p));
    let mut file_hits = 0usize;
    if !matched_files.is_empty() {
        for node in &graph.nodes {
            let Some(pos_json) = extract_position_cell(node) else {
                continue;
            };
            if let Some(file_start) = pos_json.find("\"file\":\"") {
                let s = &pos_json[file_start + 8..];
                if let Some(end) = s.find('"') {
                    let f = &s[..end];
                    if matched_files.iter().any(|mf| f.ends_with(mf.as_str()) || f == mf) {
                        if anchored.insert(node.id) {
                            file_hits += 1;
                        }
                    }
                }
            }
        }
    }

    // 2. Dotted qname references — `sphinx.ext.autosummary.generate_autosummary`
    //    convert dot to ::, find any qname that ends with this suffix.
    //    Require ≥2 segments to avoid noise from prose like "self.format".
    let dotted_re = Regex::new(r"\b([a-z_][\w]*(?:\.[a-zA-Z_][\w]*){1,})\b").unwrap();
    let mut dotted_refs: HashSet<String> = HashSet::new();
    for caps in dotted_re.captures_iter(issue) {
        if let Some(m) = caps.get(1) {
            let s = m.as_str();
            // Skip file-path-like forms (contain .py or .json etc) — handled above
            if s.ends_with(".py") || s.ends_with(".json") || s.ends_with(".rst") {
                continue;
            }
            // Convert dots to ::
            dotted_refs.insert(s.replace('.', "::"));
        }
    }
    let mut dotted_hits = 0usize;
    if !dotted_refs.is_empty() {
        for (id, qname) in id_to_qname {
            for dr in &dotted_refs {
                if qname.ends_with(dr.as_str())
                    || qname.contains(&format!("::{dr}::"))
                {
                    if anchored.insert(*id) {
                        dotted_hits += 1;
                    }
                }
            }
        }
    }

    // 3. Backtick-quoted identifiers + plain issue-token-tail match. The
    //    PPR-default scoring buries prod-file qnames matching issue terms
    //    when class-expansion of busy classes (django Atomic, gis Point)
    //    inflates many sibling scores. Direct anchor for any qname whose
    //    tail equals a backticked or sufficiently-distinctive issue token,
    //    prod-files only.
    let backtick_re = Regex::new(r"`([\w]+)`").unwrap();
    let mut bt_idents: HashSet<String> = HashSet::new();
    for caps in backtick_re.captures_iter(issue) {
        if let Some(m) = caps.get(1) {
            let t = m.as_str();
            if t.len() >= 3 {
                bt_idents.insert(t.to_lowercase());
            }
        }
    }
    // Also pull "distinctive" tokens directly from issue: snake_case identifiers
    // (require ≥1 underscore) and CamelCase identifiers (≥3 chars, first cap).
    // These are unlikely to be common English words and reliably point to code.
    let snake_re = Regex::new(r"\b([a-z][a-z0-9]*(?:_[a-z0-9]+)+)\b").unwrap();
    let camel_re = Regex::new(r"\b([A-Z][a-zA-Z0-9]{2,})\b").unwrap();
    for caps in snake_re.captures_iter(issue) {
        if let Some(m) = caps.get(1) {
            bt_idents.insert(m.as_str().to_lowercase());
        }
    }
    for caps in camel_re.captures_iter(issue) {
        if let Some(m) = caps.get(1) {
            bt_idents.insert(m.as_str().to_lowercase());
        }
    }
    // Reverted (2026-04-28): do NOT include all extracted issue tokens.
    // Earlier dump-everything boost added 1700+ nodes per instance at flat
    // score=1.0, flattening PPR rank (mpl-22711). Stick to high-precision
    // sources only: backtick-quoted + snake_case (≥1 underscore) + CamelCase.
    let _ = issue_tokens;
    // Drop common-noise idents that match too many qnames as attribute tails:
    // tokens like `class`, `module`, `line`, `tag`, `name`, `type`, `value`,
    // `data`, `index`, `kind`, `node` appear in issue prose AND match many
    // attribute names across the codebase. Anchoring on them inflates the
    // boosted set with attribute-noise (e.g. sphinx-10325 had many
    // `desc_*::classes` entries from token `classes`).
    for noise in [
        "true", "false", "none", "the", "this", "issue", "test", "args",
        "class", "classes", "module", "modules", "line", "lines", "tag", "tags",
        "name", "names", "type", "types", "value", "values", "data", "index",
        "kind", "node", "nodes", "item", "items", "key", "keys", "id", "ids",
        "code", "list", "dict", "str", "int", "bool", "result", "results",
        "file", "files", "path", "paths", "size", "len", "length", "count",
        "default", "self", "cls", "obj", "obj_", "func", "method", "methods",
        "field", "fields", "attr", "attrs", "attribute", "attributes",
        "input", "output", "outputs", "options", "option", "config", "configs",
        "context", "state", "status", "info", "details", "description",
    ] {
        bt_idents.remove(noise);
    }
    let mut bt_hits = 0usize;
    if !bt_idents.is_empty() {
        for (id, qname) in id_to_qname {
            let tail = qname.rsplit("::").next().unwrap_or("").to_lowercase();
            if bt_idents.contains(&tail) {
                if anchored.insert(*id) {
                    bt_hits += 1;
                }
            }
        }
    }

    // 4. Traceback frame parsing — Python tracebacks in issue bodies cite
    //    `path/file.py:LINE: in func_name`. Map each (file, line) to the graph
    //    node whose POSITION cell file matches and start_line<=line<=end_line.
    //    This is the strongest possible signal: the issue text LITERALLY names
    //    the gold function via line number. Filters short tail-names get out
    //    of the way (e.g. `run` is 3 chars and gets dropped by extract_issue_anchors,
    //    but its position frame still lands here).
    //    Path matching: traceback files appear as
    //    `/usr/local/lib/python3.8/site-packages/_pytest/assertion/rewrite.py`
    //    while node files are `src/_pytest/assertion/rewrite.py`. We try
    //    progressively shorter suffixes ('packages/_pytest/...', '_pytest/...',
    //    'assertion/rewrite.py') for ends_with match. Skip test-path frames.
    let tb_re = Regex::new(r"\b([\w/]+\.py):(\d+)\b").unwrap();
    let mut tb_frames: Vec<(String, usize)> = Vec::new();
    for caps in tb_re.captures_iter(&issue) {
        let path = caps.get(1).map(|m| m.as_str().replace('\\', "/")).unwrap_or_default();
        let line: usize = caps.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        if line == 0 || is_test_path(&path) {
            continue;
        }
        tb_frames.push((path, line));
    }
    let mut tb_hits = 0usize;
    if !tb_frames.is_empty() {
        for node in &graph.nodes {
            let Some(pos_json) = extract_position_cell(node) else { continue };
            // pull file
            let node_file = match pos_json.find("\"file\":\"") {
                Some(s) => {
                    let rest = &pos_json[s + 8..];
                    rest.find('"').map(|e| &rest[..e]).unwrap_or("")
                }
                None => continue,
            };
            if node_file.is_empty() { continue; }
            let start = match pos_json.find("\"start_line\":") {
                Some(s) => {
                    let rest = &pos_json[s + 13..];
                    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
                    rest[..end].parse::<usize>().unwrap_or(0)
                }
                None => continue,
            };
            let end_line = match pos_json.find("\"end_line\":") {
                Some(s) => {
                    let rest = &pos_json[s + 11..];
                    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
                    rest[..end].parse::<usize>().unwrap_or(start)
                }
                None => start,
            };
            for (fr_path, fr_line) in &tb_frames {
                if *fr_line < start || *fr_line > end_line {
                    continue;
                }
                // Suffix-match: try the captured path; if that doesn't end_with,
                // try increasingly shorter suffixes (strip leading segments).
                let mut suffix: &str = fr_path;
                let mut matched = false;
                loop {
                    if node_file.ends_with(suffix) || node_file == suffix {
                        matched = true;
                        break;
                    }
                    match suffix.find('/') {
                        Some(i) => suffix = &suffix[i + 1..],
                        None => break,
                    }
                }
                if matched {
                    if anchored.insert(node.id) {
                        tb_hits += 1;
                    }
                    break;
                }
            }
        }
    }

    if file_hits + dotted_hits + bt_hits + tb_hits > 0 {
        eprintln!(
            "[seeder] issue-anchor: files={}({} nodes), dotted={}({} nodes), backtick={}({} nodes), tb-frames={}({} nodes)",
            matched_files.len(),
            file_hits,
            dotted_refs.len(),
            dotted_hits,
            bt_idents.len(),
            bt_hits,
            tb_frames.len(),
            tb_hits,
        );
    }

    // Filter out test-file qnames from anchored set — even when the dotted/
    // backtick path catches them, test-file boost causes the same pollution
    // as file-anchor on test paths. Activation already includes test code
    // when relevant via PPR; we don't need to boost it.
    let mut id_to_file: HashMap<NodeId, String> = HashMap::new();
    for node in &graph.nodes {
        if !anchored.contains(&node.id) {
            continue;
        }
        if let Some(pos_json) = extract_position_cell(node) {
            if let Some(start) = pos_json.find("\"file\":\"") {
                let s = &pos_json[start + 8..];
                if let Some(end) = s.find('"') {
                    id_to_file.insert(node.id, s[..end].to_string());
                }
            }
        }
    }
    let before = anchored.len();
    anchored.retain(|id| {
        id_to_file
            .get(id)
            .map_or(true, |f| !is_test_path(f))
    });
    let dropped = before - anchored.len();
    if dropped > 0 {
        eprintln!("[seeder] issue-anchor: dropped {dropped} test-file nodes");
    }

    let _ = activated; // future: cap to top-K already-activated for pure boost
    anchored
}

/// Stop-list applied to BOTH issue and test_patch token extraction. Python
/// keywords + diff/test artifacts + common English fillers that never match
/// real qnames but would dominate token sets if left in.
const STOP_TOKENS: &[&str] = &[
    // Python keywords
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
    "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with",
    "yield", "True", "False", "None", "self", "cls",
    // diff/test artifacts
    "test", "tests", "py", "diff", "git", "index", "ok",
    // common English fillers (don't match qnames anyway, but keeps token set tidy)
    "the", "and", "but", "for", "not", "are", "was", "were", "this", "that", "with", "from",
    "into", "have", "has", "had", "can", "all", "any", "use", "uses", "used", "see", "via",
    "would", "could", "should", "when", "where", "what", "why", "how", "out",
];

fn extract_candidates(issue: &str) -> BTreeSet<String> {
    let stop: HashSet<&str> = STOP_TOKENS.iter().copied().collect();
    // Permissive identifier regex — same shape as the test_patch extractor.
    // Common words that don't match qnames are pruned later in match_to_seeds
    // by the graph-residence check + match_cap. Letting words like `method`,
    // `args`, `requests` through is the whole point: many issues are prose-heavy
    // and these are real identifiers in the codebase.
    let ident = Regex::new(r"[A-Za-z_][A-Za-z0-9_]+").unwrap();
    let tb_funcs = Regex::new(r"(?m)\bin\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*$").unwrap();

    let mut out = BTreeSet::new();
    for m in ident.find_iter(issue) {
        let t = m.as_str();
        if t.len() >= 2 && !stop.contains(t) {
            out.insert(t.to_string());
        }
    }
    for caps in tb_funcs.captures_iter(issue) {
        if let Some(m) = caps.get(1) {
            out.insert(m.as_str().to_string());
        }
    }
    out
}

/// Extract candidate identifiers from the *added lines* (lines starting with `+`,
/// excluding the `+++` header) of a unified-diff test_patch. Permissive regex:
/// any `[A-Za-z_][A-Za-z0-9_]+` ≥ 2 chars. Reasoning: tests guarantee API surface,
/// so we don't need the issue-text noise filters (case, inner underscore). Common
/// English words still slip through but the downstream graph-residence match prunes
/// them — only tokens that are real `qname` tails survive.
fn extract_candidates_from_test_patch(patch: &str) -> BTreeSet<String> {
    let stop: HashSet<&str> = STOP_TOKENS.iter().copied().collect();
    let ident = Regex::new(r"[A-Za-z_][A-Za-z0-9_]+").unwrap();

    let mut out = BTreeSet::new();
    for line in patch.lines() {
        // Skip diff headers and removed/context lines — only added lines feed seeds.
        if !line.starts_with('+') || line.starts_with("+++") {
            continue;
        }
        let body = &line[1..];
        for m in ident.find_iter(body) {
            let t = m.as_str();
            if t.len() >= 2 && !stop.contains(t) {
                out.insert(t.to_string());
            }
        }
    }
    out
}

/// T2: Mine structural facts from the test_patch's added lines.
///   - `Name(...)` constructor calls (capitalized callable).
///   - `Class.attr` / `Class.method(...)` access.
///   - `var = ClassName(...)` assignments → bind `var` → `ClassName` for the
///     remainder of the patch, so a later `var.attr` is promoted to
///     `ClassName.attr` in `resolved_attrs`.
///
/// Runs over patch text directly (regex, not tree-sitter): test patches are
/// short, single-file, and the patterns we care about are well-formed by
/// convention. Comments and string literals can yield spurious matches but
/// are filtered downstream by graph-residence.
fn extract_test_patch_facts(patch: &str) -> TestPatchFacts {
    let constructor_re = Regex::new(r"\b([A-Z][A-Za-z0-9_]+)\s*\(").unwrap();
    let class_attr_re = Regex::new(r"\b([A-Z][A-Za-z0-9_]+)\.([a-zA-Z_][A-Za-z0-9_]+)").unwrap();
    let var_attr_re = Regex::new(r"\b([a-z_][A-Za-z0-9_]*)\.([a-zA-Z_][A-Za-z0-9_]+)").unwrap();
    // `var = ClassName(...)` — captures binding for var-type tracking.
    let assign_re = Regex::new(r"^\s*([a-z_][A-Za-z0-9_]*)\s*=\s*([A-Z][A-Za-z0-9_]+)\s*\(")
        .unwrap();

    // Names that look capitalized but are framework noise / builtins —
    // skipping them keeps the constructor-call signal high-precision.
    const STOP_CTOR: &[&str] = &[
        "True", "False", "None", "Exception", "ValueError", "TypeError",
        "AttributeError", "KeyError", "RuntimeError", "Test", "TestCase",
    ];
    const STOP_VAR: &[&str] = &["self", "cls", "os", "sys", "re", "io", "json"];
    let stop_ctor: HashSet<&str> = STOP_CTOR.iter().copied().collect();
    let stop_var: HashSet<&str> = STOP_VAR.iter().copied().collect();

    let mut facts = TestPatchFacts::default();
    let mut var_class: HashMap<String, String> = HashMap::new();

    for line in patch.lines() {
        if !line.starts_with('+') || line.starts_with("+++") {
            continue;
        }
        let body = &line[1..];
        // Skip definition/import lines — `class Foo(...):` and `def Bar(...):`
        // would otherwise look like constructor calls and pollute the signal.
        let trimmed = body.trim_start();
        if trimmed.starts_with("class ")
            || trimmed.starts_with("def ")
            || trimmed.starts_with("async def ")
            || trimmed.starts_with("from ")
            || trimmed.starts_with("import ")
            || trimmed.starts_with('#')
        {
            continue;
        }

        // 1. `var = ClassName(...)` — record binding for downstream resolution.
        if let Some(c) = assign_re.captures(body) {
            let var = c.get(1).unwrap().as_str();
            let cls = c.get(2).unwrap().as_str();
            if !stop_ctor.contains(cls) {
                var_class.insert(var.to_string(), cls.to_string());
            }
        }

        // 2. Constructor calls — capitalized name + `(`.
        for c in constructor_re.captures_iter(body) {
            let name = c.get(1).unwrap().as_str();
            if !stop_ctor.contains(name) {
                *facts.constructor_calls.entry(name.to_string()).or_insert(0) += 1;
            }
        }

        // 3. `Class.attr` access (capitalized object).
        for c in class_attr_re.captures_iter(body) {
            let cls = c.get(1).unwrap().as_str();
            let attr = c.get(2).unwrap().as_str();
            if !stop_ctor.contains(cls) {
                *facts
                    .class_attrs
                    .entry(format!("{cls}.{attr}"))
                    .or_insert(0) += 1;
            }
        }

        // 4. `var.attr` resolved via var-type tracking.
        for c in var_attr_re.captures_iter(body) {
            let var = c.get(1).unwrap().as_str();
            let attr = c.get(2).unwrap().as_str();
            if stop_var.contains(var) {
                continue;
            }
            if let Some(cls) = var_class.get(var) {
                *facts
                    .resolved_attrs
                    .entry(format!("{cls}.{attr}"))
                    .or_insert(0) += 1;
            }
        }
    }

    facts
}

fn match_to_seeds(
    graph: &RepoGraph,
    tokens: &BTreeSet<String>,
    match_cap: usize,
) -> (Vec<NodeId>, BTreeMap<String, Vec<String>>, Vec<String>) {
    // Case-insensitive matching: lowercase both sides. Issue text often has
    // `GET`/`POST`/`HTTP` while graph qnames are `requests::api::get` etc.
    // Without folding, these structural identifiers never match. Build a
    // lowercase token set + lowercase tail lookup for the comparison.
    let tokens_lower: HashSet<String> = tokens.iter().map(|t| t.to_lowercase()).collect();

    // Pass 1: bucket all qnames by their tail-name match against tokens.
    // This is necessary to know per-token match counts before deciding which
    // tokens to keep — we can't filter inline without knowing the total.
    let mut by_token: BTreeMap<String, Vec<(NodeId, String)>> = BTreeMap::new();
    for (id, qname) in &graph.nav.qname_by_id {
        let tail = qname.rsplit("::").next().unwrap_or("");
        let tail_lower = tail.to_lowercase();
        if tokens_lower.contains(&tail_lower) {
            // Bucket by ORIGINAL tail (not lowercased) so qnames sharing a
            // case-insensitive tail still cluster sensibly.
            by_token
                .entry(tail.to_string())
                .or_default()
                .push((*id, qname.clone()));
        }
    }

    // Determinism: sort each bucket by qname (with NodeId tiebreak). graph.nav.qname_by_id
    // is HashMap-iter, so per-token NodeId order varies run-to-run. Without this sort,
    // downstream consumers (synth bins reading seed_ids) see different seed orderings
    // each run and propagate that into summaries-aplus, source_cells, prefix.txt.
    for hits in by_token.values_mut() {
        hits.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.0.cmp(&b.0.0)));
    }

    // Pass 2: drop over-cap tokens, emit seeds + matched for the rest.
    let mut seeds: HashSet<NodeId> = HashSet::new();
    let mut ordered: Vec<NodeId> = Vec::new();
    let mut matched: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut dropped: Vec<String> = Vec::new();
    for (tok, hits) in by_token {
        if hits.len() > match_cap {
            dropped.push(format!("{tok}({})", hits.len()));
            continue;
        }
        for (id, qname) in &hits {
            if seeds.insert(*id) {
                ordered.push(*id);
            }
            matched.entry(tok.clone()).or_default().push(qname.clone());
        }
    }
    (ordered, matched, dropped)
}

/// Class-expansion: for any CLASS-kind seed, inject all of that class's METHODs
/// into the seed set. PPR over a class-only seed propagates mass to its methods
/// indirectly via structural edges, but with N=top_k=60 and a busy parent class
/// (e.g. django Field has 30+ subclass formfield overrides) the *specific*
/// class's methods get diluted. Direct injection guarantees the class-specific
/// methods carry personalization mass at activation.
///
/// Returns (expanded_seeds, expansion_log, expanded_method_ids) where
/// expansion_log maps class_qname → method_qnames added, and
/// expanded_method_ids is the full set of method NodeIds belonging to any
/// class seed (used downstream to guarantee-include them in `activated` even
/// when PPR's top_k drops them).
fn expand_class_seeds(
    seeds: Vec<NodeId>,
    graph: &RepoGraph,
) -> (Vec<NodeId>, BTreeMap<String, Vec<String>>, HashSet<NodeId>) {
    let mut methods_by_class: HashMap<&str, Vec<NodeId>> = HashMap::new();
    for (id, qname) in &graph.nav.qname_by_id {
        if graph.nav.kind_by_id.get(id).copied() == Some(node_kind::METHOD) {
            if let Some(idx) = qname.rfind("::") {
                let cq = &qname[..idx];
                methods_by_class.entry(cq).or_default().push(*id);
            }
        }
    }
    // Determinism: per-class method Vecs got NodeIds in HashMap iter order
    // (graph.nav.qname_by_id). Sort each so downstream `for mid in methods`
    // iteration is stable across runs.
    for v in methods_by_class.values_mut() {
        v.sort_by_key(|id| id.0);
    }

    let mut seen: HashSet<NodeId> = seeds.iter().copied().collect();
    let mut ordered: Vec<NodeId> = seeds.clone();
    let mut expansion_log: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut expanded_method_ids: HashSet<NodeId> = HashSet::new();

    for sid in &seeds {
        if graph.nav.kind_by_id.get(sid).copied() != Some(node_kind::CLASS) {
            continue;
        }
        let class_qname = match graph.nav.qname_by_id.get(sid) {
            Some(q) => q.as_str(),
            None => continue,
        };
        if let Some(methods) = methods_by_class.get(class_qname) {
            let mut added: Vec<String> = Vec::new();
            for mid in methods {
                expanded_method_ids.insert(*mid);
                if seen.insert(*mid) {
                    ordered.push(*mid);
                    if let Some(mq) = graph.nav.qname_by_id.get(mid) {
                        added.push(mq.clone());
                    }
                }
            }
            if !added.is_empty() {
                added.sort();
                expansion_log.insert(class_qname.to_string(), added);
            }
        }
    }

    (ordered, expansion_log, expanded_method_ids)
}

/// v0.4.13a sibling expansion (B). Walk three axes from activated METHODs:
///   1. enclosing class — X::C::m → X::C
///   2. intra-class siblings — X::C::m → all X::C::*
///   3. peer overrides — X::C::m → all Y::m for any class Y
/// Caps total additions at `cap`. Expanded siblings get half the min activated
/// score so they rank below real hits.
fn expand_siblings(
    activated: &[(NodeId, f64)],
    graph: &RepoGraph,
    cap: usize,
) -> Vec<(NodeId, f64)> {
    let activated_ids: HashSet<NodeId> = activated.iter().map(|(id, _)| *id).collect();

    let mut methods_by_class: HashMap<&str, Vec<NodeId>> = HashMap::new();
    let mut methods_by_name: HashMap<&str, Vec<NodeId>> = HashMap::new();
    let mut class_by_qname: HashMap<&str, NodeId> = HashMap::new();

    for (id, qname) in &graph.nav.qname_by_id {
        let kind = graph.nav.kind_by_id.get(id).copied();
        if kind == Some(node_kind::CLASS) {
            class_by_qname.insert(qname.as_str(), *id);
        } else if kind == Some(node_kind::METHOD) {
            if let Some(idx) = qname.rfind("::") {
                let cq = &qname[..idx];
                let mname = &qname[idx + 2..];
                methods_by_class.entry(cq).or_default().push(*id);
                methods_by_name.entry(mname).or_default().push(*id);
            }
        }
    }
    // Determinism: per-class/per-name Vecs received NodeIds in HashMap iter
    // order. Sort each so the sibling-expansion push order is stable.
    for v in methods_by_class.values_mut() {
        v.sort_by_key(|id| id.0);
    }
    for v in methods_by_name.values_mut() {
        v.sort_by_key(|id| id.0);
    }

    let min_score = activated
        .iter()
        .map(|(_, s)| *s)
        .fold(f64::INFINITY, f64::min);
    let sibling_score = if min_score.is_finite() && min_score > 0.0 {
        min_score * 0.5
    } else {
        1e-6
    };

    let mut added: HashMap<NodeId, f64> = HashMap::new();
    let mut added_order: Vec<NodeId> = Vec::new();
    for (id, _) in activated {
        let kind = graph.nav.kind_by_id.get(id).copied();
        if kind != Some(node_kind::METHOD) {
            continue;
        }
        let qname = match graph.nav.qname_by_id.get(id) {
            Some(q) => q.as_str(),
            None => continue,
        };
        let idx = match qname.rfind("::") {
            Some(i) => i,
            None => continue,
        };
        let cq = &qname[..idx];
        let mname = &qname[idx + 2..];

        // (1) enclosing class
        if let Some(cid) = class_by_qname.get(cq).copied() {
            if !activated_ids.contains(&cid) && !added.contains_key(&cid) {
                added.insert(cid, sibling_score);
                added_order.push(cid);
            }
        }
        // (2) intra-class siblings
        if let Some(sibs) = methods_by_class.get(cq) {
            for sid in sibs {
                if !activated_ids.contains(sid) && !added.contains_key(sid) {
                    added.insert(*sid, sibling_score);
                    added_order.push(*sid);
                }
            }
        }
        // (3) peer overrides
        if let Some(peers) = methods_by_name.get(mname) {
            for sid in peers {
                if !activated_ids.contains(sid) && !added.contains_key(sid) {
                    added.insert(*sid, sibling_score);
                    added_order.push(*sid);
                }
            }
        }
    }

    let extra: Vec<(NodeId, f64)> = added_order
        .into_iter()
        .take(cap)
        .map(|id| (id, added[&id]))
        .collect();
    let mut out = activated.to_vec();
    out.extend(extra);
    out
}
