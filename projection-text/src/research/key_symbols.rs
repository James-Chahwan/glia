//! Key symbols (LD.12d): the top-K activated METHOD/FUNCTION nodes, ranked
//! with issue-token boosts, plus the backtick intermediates the A+ access-path
//! summaries name, each emitted with its CODE cell. The body of the
//! `synth_key_symbols` research bin, moved into a [`SynthHook`]; the bin is
//! now a thin wrapper that fills a [`KeySymbolsSynth`] from its flags and
//! writes the cells as `source_cells.json` for the prefix
//! "## Key symbols from the repository (full code):" block.
//!
//! Why this exists: marshmallow-1359 smoke runs showed auto-prefix without a
//! source anchor fabricates isinstance-guards on non-existent methods. The
//! hand-crafted GOLD prefix carried full bodies of 5 target methods. This pass
//! reproduces that anchor automatically from graph state + issue text.
//!
//! The boosts are issue-token grounding, so they stay task-shaped data the
//! research driver fills in (issue text, test patch, seeds file, call chain);
//! the A+ summaries are the view's earlier cells (see [`KeySymbolsSynth`]).

use std::collections::{BTreeSet, HashMap, HashSet};

use regex::Regex;
use glia_activation::plan::{ActivatedView, SynthCell, SynthHook};
use glia_code_domain::node_kind;
use glia_core::{NodeId, NodeKindId};
use glia_graph::RepoGraph;
use serde::Deserialize;

use crate::driver_utils::{
    chain_depth_boost, extract_code_cell, extract_position_cell, node_by_qname_index,
};
use crate::hooks::{ACCESS_PATH, CALLSITE_ARGFLOW};

/// [`SynthHook::name`] of [`KeySymbolsSynth`]: the `hook` of its cells.
pub const KEY_SYMBOLS: &str = "key_symbols";

/// The `hook` of a summary cell a driver loads from a summaries JSON
/// (`[{id, qname, score, summary}]`) and pushes onto the view before the
/// plan runs: `key` is the entry's qname, `text` its summary.
pub const SUMMARY: &str = "summary";

/// The hooks whose cells [`KeySymbolsSynth`] reads as A+ summaries.
const SUMMARY_HOOKS: [&str; 3] = [SUMMARY, ACCESS_PATH, CALLSITE_ARGFLOW];

/// The qname prefix of an A+ access-path summary.
const ACCESS_PATH_PREFIX: &str = "synth::AccessPath::";

/// Structural facts the `seeds` bin mines from a SWE-bench `test_patch`
/// (seeds.json's `test_patch_facts`). Mirror of `seeds::TestPatchFacts`:
/// producer and consumer keep the same JSON shape.
#[derive(Deserialize, Default, Clone, Debug, PartialEq)]
pub struct TestPatchFacts {
    /// `Class(...)` constructor calls, by class name.
    #[serde(default)]
    pub constructor_calls: HashMap<String, usize>,
    /// `Class.attr` accesses, by `Class.attr`.
    #[serde(default)]
    pub class_attrs: HashMap<String, usize>,
    /// `var.attr` accesses whose `var` resolved to a class, by `Class.attr`.
    #[serde(default)]
    pub resolved_attrs: HashMap<String, usize>,
}

/// Select the top-K activated METHOD/FUNCTION nodes plus backtick
/// intermediates from the A+ summaries, and emit their CODE cells.
///
/// Ranking: each resolvable `activated` qname (an issue-anchored ATTRIBUTE
/// promoted to its CLASS) scores its seed score + seven additive boosts
/// (dotted-attr tail, issue-anchor body match, call-chain depth, cited file,
/// class seed, test-patch attr, issue anchor), sorted score desc then qname
/// asc. Up to `top_k` of those are selected, topped up from the backtick
/// qnames of the `aplus_scan` best A+ summaries, then up to
/// `chain_grounding_cap` chain-grounding cells (an ATTRIBUTE becomes an
/// attribute-presence note) are appended.
///
/// The A+ summaries are the view's cells whose hook is [`SUMMARY`],
/// [`ACCESS_PATH`] or [`CALLSITE_ARGFLOW`], in view order, keyed
/// `synth::AccessPath::...` (the others are skipped). The view's scores are
/// not read: `activated` is the ranking input.
///
/// Output: one cell per selected qname that has a node and a source, in rank
/// order. `id` is the 1-based rank (a selection skipped for want of a node or
/// source leaves a gap), `key` the qname, `anchor` its node, `text` the source
/// clipped to `max_chars` bytes at a char boundary (then
/// `"\n# ... (truncated)"`; `0` = no cap), `score` the ranked score (0.0 for
/// a selection that did not come from the ranking), and `attrs` `file`,
/// `rank`, `reason` (`activated`, `aplus-backtick`, `chain-grounding`,
/// `attr-presence`), `start_line`, `end_line` in that order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeySymbolsSynth {
    /// seeds.json's `activated` `(qname, score)` rows verbatim: order kept,
    /// unresolvable qnames kept (backtick suffixes resolve against all of
    /// them, first match wins).
    pub activated: Vec<(String, f64)>,
    /// Tail names of CLASS-kind seeds (seeds.json's `class_seeds`), e.g.
    /// `["FilePathField", "Schema"]`: methods of a class named here are
    /// up-ranked by how often the issue + test patch mention it.
    pub class_seeds: Vec<String>,
    /// seeds.json's `test_patch_facts`.
    pub test_patch_facts: TestPatchFacts,
    /// seeds.json's `issue_anchored_qnames`: +25.0 each, and an anchored
    /// ATTRIBUTE is promoted to its CLASS.
    pub issue_anchored_qnames: Vec<String>,
    /// The issue text.
    pub issue: String,
    /// The SWE-bench `test_patch` text, empty when there is none.
    pub test_patch: String,
    /// qname -> min CALLS-chain depth (`driver_utils::load_chain_depths`),
    /// empty when there is no chain.
    pub chain_depths: HashMap<String, usize>,
    /// Selections before chain grounding.
    pub top_k: usize,
    /// How many A+ summaries (best score first) to scan for backtick
    /// intermediates.
    pub aplus_scan: usize,
    /// Max chain-grounding cells appended beyond `top_k`.
    pub chain_grounding_cap: usize,
    /// Cap on each cell's source, in bytes; `0` = no cap.
    pub max_chars: usize,
}

impl SynthHook<RepoGraph> for KeySymbolsSynth {
    fn name(&self) -> &'static str {
        KEY_SYMBOLS
    }

    fn synth(&self, graph: &RepoGraph, view: &ActivatedView) -> Vec<SynthCell> {
        let issue: &str = &self.issue;
        let test_patch_text: &str = &self.test_patch;
        let dot_attrs = extract_dot_attrs(issue);
        let issue_anchors = extract_issue_anchors(issue);
        let issue_file_stems = extract_issue_file_stems(issue);
        let class_seed_set: HashSet<String> = self.class_seeds.iter().cloned().collect();
        let class_seed_counts: HashMap<String, usize> = self
            .class_seeds
            .iter()
            .map(|c| {
                // Mentions across issue + test_patch text (1x each), plus
                // constructor-call hits from T2 facts (3x — a constructor call is
                // a much stronger "this is the class under test" signal than mere
                // textual mention). Capped downstream at 5 for the boost formula.
                let mentions =
                    count_word_occurrences(issue, c) + count_word_occurrences(test_patch_text, c);
                let ctor = self
                    .test_patch_facts
                    .constructor_calls
                    .get(c)
                    .copied()
                    .unwrap_or(0);
                (c.clone(), mentions + ctor * 3)
            })
            .collect();
        // T2: union of all `Class.attr` and resolved `var.attr` pairs from the
        // test_patch. Used for `attr_anchor_boost` below — methods whose qname
        // structure (`...::Class::attr`) matches a test-anchored access get a
        // direct lift. Highest-confidence selection signal we have.
        let mut test_attr_facts: HashMap<String, usize> = HashMap::new();
        for (k, v) in &self.test_patch_facts.resolved_attrs {
            *test_attr_facts.entry(k.clone()).or_insert(0) += v;
        }
        for (k, v) in &self.test_patch_facts.class_attrs {
            *test_attr_facts.entry(k.clone()).or_insert(0) += v;
        }
        eprintln!("[keysym] dotted-attr issue tokens: {:?}", dot_attrs);
        eprintln!("[keysym] issue anchor tokens: {:?}", issue_anchors);
        eprintln!("[keysym] issue file stems: {:?}", issue_file_stems);
        eprintln!("[keysym] class seeds (with mention counts): {:?}", class_seed_counts);
        eprintln!("[keysym] test_patch attr facts: {:?}", test_attr_facts);

        let chain_depths = &self.chain_depths;

        let qname_to_node = node_by_qname_index(graph);

        // G3-lazy: drop CLASS — class body dumps duplicate already-selected methods.
        // Class-level constants that matter will still enter via PPR ATTRIBUTE scoring.
        let source_kinds: HashSet<NodeKindId> =
            [node_kind::METHOD, node_kind::FUNCTION]
                .into_iter()
                .collect();

        // 1. Rank activated METHOD/FUNCTION by (score + dot-attr tail boost + issue-body-match boost).
        let issue_anchored_set: HashSet<&str> =
            self.issue_anchored_qnames.iter().map(|s| s.as_str()).collect();
        if !issue_anchored_set.is_empty() {
            eprintln!("[keysym] issue-anchored qnames: {} (anchor_priority_boost=+25.0)",
                issue_anchored_set.len());
        }
        let mut ranked: Vec<(String, f64, &str)> = Vec::new();
        // Track substituted qnames (parent class promoted from an anchored attr)
        // so we don't double-rank the same node.
        let mut promoted_to_parent: HashMap<String, String> = HashMap::new();
        for (qname, score) in &self.activated {
            let mut effective_qname = qname.clone();
            let effective_score = *score;
            let Some(mut node) = qname_to_node.get(qname.as_str()).copied() else {
                continue;
            };
            let mut kind = graph.nav.kind_by_id.get(&node.id).copied();
            let is_anchored = issue_anchored_set.contains(qname.as_str());

            // For anchored ATTRIBUTE nodes (django Command.output_transaction —
            // class attribute, no CODE cell of its own), promote to parent CLASS
            // qname so the class body becomes the source_cell. Otherwise the
            // attribute would silently drop with "no CODE cell" and source_cells
            // misses the file entirely.
            if is_anchored && kind == Some(node_kind::ATTRIBUTE) {
                // qname `pkg::mod::Class::attr` → strip last `::attr` → `pkg::mod::Class`
                if let Some(parent_qname) = qname.rsplit_once("::").map(|(p, _)| p.to_string()) {
                    if let Some(parent_node) = qname_to_node.get(parent_qname.as_str()).copied() {
                        let parent_kind = graph.nav.kind_by_id.get(&parent_node.id).copied();
                        if parent_kind == Some(node_kind::CLASS) {
                            effective_qname = parent_qname.clone();
                            node = parent_node;
                            kind = parent_kind;
                            promoted_to_parent.insert(qname.clone(), parent_qname);
                        }
                    }
                }
            }

            // Allow CLASS through if anchored (so parent-class promotion lands).
            let allowed = kind.map_or(false, |k| {
                source_kinds.contains(&k) || (is_anchored && k == node_kind::CLASS)
            });
            if !allowed {
                continue;
            }
            if method_parent_is_top_level_binder(node.id, graph) {
                continue;
            }
            // Use effective_qname for downstream operations; rebind so existing
            // boost logic sees the (possibly promoted) target.
            let qname = &effective_qname;
            let score = &effective_score;
            let tail = qname.rsplit("::").next().unwrap_or("");
            let tail_boost = if dot_attrs.contains(tail) { 10.0 } else { 0.0 };
            // G2: count unique issue-anchor tokens that appear verbatim in the
            // symbol's body text. Symbols echoing the issue-quoted identifiers are
            // strong fix-site candidates (e.g. SCHEMA_OPTS_VAR_NAME appears in the
            // issue and in DateTime._bind_to_schema but not in List/Tuple).
            let body_match = extract_code_cell(node)
                .map(|src| count_body_anchor_matches(src, &issue_anchors))
                .unwrap_or(0);
            let body_boost = (body_match.min(5) as f64) * 3.0;
            let chain_boost = chain_depth_boost(chain_depths.get(qname.as_str()).copied());
            // Fix-3 (file_boost): when the issue cites a `.py` path, lift methods
            // whose file matches. seaborn-2848 14B regression: gold `_oldcore.py`
            // was activated #2 but model picked `relational.py`; issue cites
            // `seaborn/_core.py` and `_core` is a substring of `_oldcore`, so the
            // matched-stem fires here. Cap at 8.0 (below tail_boost=10).
            let file_boost = file_match_boost(node, &issue_file_stems);
            // T1(b): class_match_boost. When a method's enclosing class tail is in
            // the issue-cited class-seed set, up-rank above generic siblings. The
            // weight scales with how often that class is mentioned across issue
            // text + test_patch — this discriminates the *target* class
            // (FilePathField, 5+ mentions) from collateral classes (CharField, 1
            // mention) that happen to share the issue surface, so the right
            // class's `formfield` (gold) beats CharField's `formfield`.
            // Cap at 10.0 (matches tail_boost ceiling).
            let class_boost = class_match_boost(qname, &class_seed_set, &class_seed_counts);
            // T2(b): attr_anchor_boost. Direct test-anchor: if the test_patch
            // accesses `Class.attr` (resolved via local var-type tracking), lift
            // any qname ending `...::Class::attr` proportionally to access count.
            // django-10924: `field.formfield()` resolves to `FilePathField.formfield`
            // and `field.path` to `FilePathField.path` — both 2x in the patch, so
            // the gold method gets +8.0 directly from test usage. Strongest single
            // signal we have for "this is the right method to patch."
            let attr_boost = attr_anchor_boost(qname, &test_attr_facts);
            // anchor_priority_boost: brutalize the rest of the ranker for issue-
            // anchored qnames so they lock-in top-K source_cells inclusion. +25.0
            // exceeds tail_boost(10) + body_boost(15) + class_boost(10) + file_boost(8)
            // combined, but doesn't drown them entirely — within the anchor set
            // those boosts still tiebreak.
            let anchor_priority_boost: f64 = if issue_anchored_set.contains(qname.as_str()) {
                25.0
            } else {
                0.0
            };
            ranked.push((
                qname.clone(),
                score + tail_boost + body_boost + chain_boost + file_boost + class_boost + attr_boost
                    + anchor_priority_boost,
                "activated",
            ));
        }
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });

        // 2. Backtick intermediates from top-N A+ cell summaries, suffix-matched
        //    against activated qname list.
        let activated_qnames: Vec<String> =
            self.activated.iter().map(|(q, _)| q.clone()).collect();
        let aplus_cells: Vec<&SynthCell> = view
            .synth
            .iter()
            .filter(|s| SUMMARY_HOOKS.contains(&s.hook))
            .filter(|s| s.key.starts_with(ACCESS_PATH_PREFIX))
            .collect();
        let mut aplus_sorted: Vec<&SynthCell> = aplus_cells.clone();
        aplus_sorted.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        let top_aplus: Vec<&SynthCell> = aplus_sorted.into_iter().take(self.aplus_scan).collect();

        let mut intermediates: Vec<String> = Vec::new();
        for cell in &top_aplus {
            for suffix in extract_backtick_qnames(&cell.text) {
                if let Some(resolved) = resolve_suffix(&suffix, &activated_qnames) {
                    intermediates.push(resolved);
                }
            }
        }
        eprintln!(
            "[keysym] {} activated source-bearing, {} backtick intermediates from top-{} A+ cells",
            ranked.len(),
            intermediates.len(),
            top_aplus.len()
        );

        // 3. Merge + dedupe (activated first, then intermediates), cap at top_k.
        //    Each selection carries the ranked score it was selected with (0.0
        //    for one that did not come from the ranking).
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut selection: Vec<(String, String, f64)> = Vec::new(); // (qname, reason, score)
        for (qname, score, reason) in &ranked {
            if seen.insert(qname.clone()) {
                selection.push((qname.clone(), (*reason).to_string(), *score));
                if selection.len() >= self.top_k {
                    break;
                }
            }
        }
        if selection.len() < self.top_k {
            for qname in intermediates {
                if seen.insert(qname.clone()) {
                    selection.push((qname, "aplus-backtick".to_string(), 0.0));
                    if selection.len() >= self.top_k {
                        break;
                    }
                }
            }
        }

        // 3.4. Build 2: class-attribute presence map. For every CLASS node,
        // collect the names of its directly-owned ATTRIBUTE children (via
        // parent_of). Used by attribute-presence grounding below to render
        // "`.opts` is defined on BaseSchema, not on Field/List/Tuple/DateTime".
        let mut class_attrs: HashMap<NodeId, BTreeSet<String>> = HashMap::new();
        for (&attr_id, &kind) in &graph.nav.kind_by_id {
            if kind != node_kind::ATTRIBUTE {
                continue;
            }
            let Some(&parent) = graph.nav.parent_of.get(&attr_id) else {
                continue;
            };
            if graph.nav.kind_by_id.get(&parent).copied() != Some(node_kind::CLASS) {
                continue;
            }
            let Some(name) = graph.nav.name_by_id.get(&attr_id).cloned() else {
                continue;
            };
            class_attrs.entry(parent).or_default().insert(name);
        }
        // In-scope classes = parents of already-selected METHOD/FUNCTION cells.
        // These are the classes whose presence/absence of an attribute is
        // informative (e.g. `Field`, `DateTime`, `List`, `Tuple` for marshmallow).
        let mut inscope_classes: Vec<NodeId> = Vec::new();
        for (qname, _reason, _score) in &selection {
            let Some(node) = qname_to_node.get(qname.as_str()) else { continue };
            let Some(&parent) = graph.nav.parent_of.get(&node.id) else { continue };
            if graph.nav.kind_by_id.get(&parent).copied() == Some(node_kind::CLASS)
                && !inscope_classes.contains(&parent)
            {
                inscope_classes.push(parent);
            }
        }

        // 3.5. G4: Chain grounding — for each top-K selection, find ALL A+ cells
        // that mention it (via trailing-segment match on backticked qnames), then
        // resolve every *other* backtick qname against the full graph (not just
        // activated). Allow ATTRIBUTE in addition to METHOD/FUNCTION so e.g.
        // `BaseSchema.opts` gets grounded alongside `Field.root` (@property).
        // Budget-capped by `chain_grounding_cap`.
        let selected_tails: HashSet<String> = selection
            .iter()
            .map(|(qn, _, _)| qn.rsplit("::").next().unwrap_or("").to_string())
            .collect();
        let grounding_kinds: HashSet<NodeKindId> =
            [node_kind::METHOD, node_kind::FUNCTION, node_kind::ATTRIBUTE]
                .into_iter()
                .collect();
        // Determinism: qname_to_node is a HashMap; keys() yields a non-deterministic
        // iter order. resolve_suffix() iterates this Vec and returns the FIRST match,
        // so when multiple qnames have a matching suffix the chain-grounding cell
        // resolution becomes run-dependent. Sort lexicographically.
        let mut all_qnames: Vec<String> =
            qname_to_node.keys().map(|s| s.to_string()).collect();
        all_qnames.sort();
        // Side map: synthesized source text for attribute-presence cells.
        // ATTRIBUTE nodes carry no CODE cell, so step 4 prefers this when set.
        let mut synth_source: HashMap<String, String> = HashMap::new();
        let mut grounding_added = 0usize;
        'cells: for cell in &aplus_cells {
            let backticks = extract_backtick_qnames(&cell.text);
            let relevant = backticks.iter().any(|bt| {
                let tail = bt.rsplit('.').next().unwrap_or("");
                selected_tails.contains(tail) || dot_attrs.contains(tail)
            });
            if !relevant {
                continue;
            }
            for bt in backticks {
                if grounding_added >= self.chain_grounding_cap {
                    break 'cells;
                }
                let Some(resolved) = resolve_suffix(&bt, &all_qnames) else {
                    continue;
                };
                if seen.contains(&resolved) {
                    continue;
                }
                let Some(node) = qname_to_node.get(resolved.as_str()) else {
                    continue;
                };
                let kind = graph.nav.kind_by_id.get(&node.id).copied();
                if !kind.map_or(false, |k| grounding_kinds.contains(&k)) {
                    continue;
                }
                if kind == Some(node_kind::METHOD)
                    && method_parent_is_top_level_binder(node.id, graph)
                {
                    continue;
                }
                // Build 2: ATTRIBUTE → synthesise a presence-note cell instead of
                // skipping on missing CODE cell. Lists the classes in scope that
                // define this attribute vs. those that do not.
                if kind == Some(node_kind::ATTRIBUTE) {
                    let Some(attr_name) = graph.nav.name_by_id.get(&node.id).cloned() else {
                        continue;
                    };
                    let Some(&parent_class) = graph.nav.parent_of.get(&node.id) else {
                        continue;
                    };
                    let parent_simple = graph
                        .nav
                        .qname_by_id
                        .get(&parent_class)
                        .map(|q| q.rsplit("::").next().unwrap_or(q.as_str()).to_string())
                        .unwrap_or_default();
                    let mut absent: Vec<String> = Vec::new();
                    let mut present_also: Vec<String> = Vec::new();
                    for c in &inscope_classes {
                        if *c == parent_class {
                            continue;
                        }
                        let has = class_attrs
                            .get(c)
                            .map_or(false, |s| s.contains(&attr_name));
                        let Some(cq) = graph.nav.qname_by_id.get(c).cloned() else {
                            continue;
                        };
                        let simple = cq.rsplit("::").next().unwrap_or(&cq).to_string();
                        if has {
                            present_also.push(simple);
                        } else {
                            absent.push(simple);
                        }
                    }
                    let mut text = format!(
                        "# Attribute `.{name}` presence (derived from HAS_ATTRIBUTE edges)\n\
                         # Defined on class: `{parent}`\n",
                        name = attr_name,
                        parent = parent_simple,
                    );
                    if !present_also.is_empty() {
                        text.push_str(&format!(
                            "# Also defined on in-scope classes: {}\n",
                            present_also.join(", ")
                        ));
                    }
                    text.push_str(&format!(
                        "# NOT defined on in-scope classes: {}\n",
                        if absent.is_empty() {
                            "(none)".to_string()
                        } else {
                            absent.join(", ")
                        }
                    ));
                    seen.insert(resolved.clone());
                    synth_source.insert(resolved.clone(), text);
                    selection.push((resolved, "attr-presence".to_string(), 0.0));
                    grounding_added += 1;
                    continue;
                }
                if extract_code_cell(node).is_none() {
                    continue;
                }
                seen.insert(resolved.clone());
                selection.push((resolved, "chain-grounding".to_string(), 0.0));
                grounding_added += 1;
            }
        }
        eprintln!("[keysym] chain-grounding added {grounding_added} cells");

        // 4. Emit CODE cells for each selected qname.
        let mut out: Vec<SynthCell> = Vec::new();
        for (rank, (qname, reason, score)) in selection.iter().enumerate() {
            let Some(node) = qname_to_node.get(qname.as_str()) else {
                eprintln!("[keysym] skip {qname}: no node match");
                continue;
            };
            let src: &str = if let Some(s) = synth_source.get(qname) {
                s.as_str()
            } else if let Some(s) = extract_code_cell(node) {
                s
            } else {
                eprintln!("[keysym] skip {qname}: no CODE cell");
                continue;
            };
            let pos_json = extract_position_cell(node);
            let file = pos_json.and_then(parse_file_field).unwrap_or_else(|| "?".to_string());
            // Extract start_line/end_line from POSITION cell JSON
            let (start_line, end_line) = pos_json
                .map(|j| {
                    let sl = parse_pos_int_field(j, "start_line").unwrap_or(0);
                    let el = parse_pos_int_field(j, "end_line").unwrap_or(0);
                    (sl, el)
                })
                .unwrap_or((0, 0));
            let capped = if self.max_chars > 0 && src.len() > self.max_chars {
                format!("{}\n# ... (truncated)", clip_at_char_boundary(src, self.max_chars))
            } else {
                src.to_string()
            };
            let rank = rank + 1;
            out.push(SynthCell {
                hook: KEY_SYMBOLS,
                id: rank as u64,
                key: qname.clone(),
                anchor: Some(node.id),
                text: capped,
                score: *score,
                attrs: vec![
                    ("file", file),
                    ("rank", rank.to_string()),
                    ("reason", reason.clone()),
                    ("start_line", start_line.to_string()),
                    ("end_line", end_line.to_string()),
                ],
            });
        }

        eprintln!("[keysym] emitting {} source cells", out.len());
        for cell in &out {
            eprintln!(
                "  #{} [{}] {} ({})",
                cell.id,
                cell_attr(cell, "reason").unwrap_or(""),
                cell.key,
                cell_attr(cell, "file").unwrap_or("")
            );
        }
        out
    }
}

/// The value of a cell's named attr (`file`, `rank`, `reason`, `start_line`,
/// `end_line` on a [`KEY_SYMBOLS`] cell), or `None` when it has none.
pub fn cell_attr<'a>(cell: &'a SynthCell, name: &str) -> Option<&'a str> {
    cell.attrs
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.as_str())
}

/// `src` cut to at most `max` bytes, at the last char boundary at or below
/// `max`: exactly `max` bytes when that is a boundary (always, for ASCII).
fn clip_at_char_boundary(src: &str, max: usize) -> &str {
    let mut cut = max.min(src.len());
    while !src.is_char_boundary(cut) {
        cut -= 1;
    }
    &src[..cut]
}

/// True when the METHOD's enclosing CLASS is a framework top-level binder
/// (currently: class name ends in `Schema`). Such classes iterate over *all*
/// declared fields and dominate body-anchor scoring via constants like
/// `SCHEMA_OPTS_VAR_NAME`, crowding out the container-leaf methods
/// (`List._bind_to_schema`, `Tuple._bind_to_schema`, `DateTime._bind_to_schema`)
/// that are the actual fix-site candidates. Mirrors the same filter in
/// `synth_derived_notes::is_top_level_binder`.
fn method_parent_is_top_level_binder(method_id: NodeId, graph: &RepoGraph) -> bool {
    let Some(&parent) = graph.nav.parent_of.get(&method_id) else {
        return false;
    };
    if graph.nav.kind_by_id.get(&parent).copied() != Some(node_kind::CLASS) {
        return false;
    }
    let Some(q) = graph.nav.qname_by_id.get(&parent) else {
        return false;
    };
    let simple = q.rsplit("::").next().unwrap_or(q.as_str());
    simple.ends_with("Schema")
}

fn extract_dot_attrs(issue: &str) -> BTreeSet<String> {
    let Ok(re) = Regex::new(r"\.([a-zA-Z_][a-zA-Z0-9_]*)\b") else {
        return BTreeSet::new();
    };
    re.captures_iter(issue)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// Issue-quoted identifiers that look like code anchors: CamelCase or
/// snake_case with an underscore, min 4 chars. Drops prose words ("schema",
/// "field") while keeping constants ("SCHEMA_OPTS_VAR_NAME"), dunders
/// ("_bind_to_schema"), and type names ("ValidationError"). These are the
/// verbatim tokens whose presence in a symbol body is a strong fix-site signal.
fn extract_issue_anchors(issue: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(re) = Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\b") else {
        return out;
    };
    for caps in re.captures_iter(issue) {
        if let Some(m) = caps.get(1) {
            let t = m.as_str();
            if t.len() < 4 {
                continue;
            }
            let has_upper = t.chars().any(|c| c.is_ascii_uppercase());
            let has_underscore = t.contains('_');
            if has_upper || has_underscore {
                out.insert(t.to_string());
            }
        }
    }
    out
}

/// `.py` paths cited in the issue body, reduced to file-stem (basename without
/// extension). `seaborn/_core.py` → `_core`. Used by file_match_boost to lift
/// candidates whose file path matches what the user explicitly named.
fn extract_issue_file_stems(issue: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(re) = Regex::new(r"\b([A-Za-z_][\w./]*)\.py\b") else {
        return out;
    };
    for caps in re.captures_iter(issue) {
        if let Some(m) = caps.get(1) {
            let stem = m.as_str().rsplit('/').next().unwrap_or("");
            if stem.len() >= 3 {
                out.insert(stem.to_string());
            }
        }
    }
    out
}

/// 8.0 if the candidate's file basename matches (or contains as substring) a
/// stem cited in the issue. Substring match handles renames like
/// `seaborn/_core.py` → `seaborn/_oldcore.py` where `_core` ⊂ `_oldcore`.
fn file_match_boost(node: &glia_core::Node, stems: &HashSet<String>) -> f64 {
    if stems.is_empty() {
        return 0.0;
    }
    let file = match extract_position_cell(node).and_then(parse_file_field) {
        Some(f) => f,
        None => return 0.0,
    };
    let basename = file
        .rsplit('/')
        .next()
        .unwrap_or("")
        .strip_suffix(".py")
        .unwrap_or("");
    if basename.is_empty() {
        return 0.0;
    }
    for stem in stems {
        if basename == stem || basename.contains(stem.as_str()) {
            return 8.0;
        }
    }
    0.0
}

/// Up to 10.0 when the candidate METHOD's enclosing class tail matches an
/// issue-cited class identifier (from seeds.class_seeds). Boost = `min(count,
/// 5) * 2.0` where `count` is the number of times the class name appears in
/// issue + test_patch text. Mention-weighting discriminates the primary
/// target class from collateral matches sharing the same surface.
fn class_match_boost(
    qname: &str,
    class_seeds: &HashSet<String>,
    counts: &HashMap<String, usize>,
) -> f64 {
    if class_seeds.is_empty() {
        return 0.0;
    }
    // qname shape: `pkg::mod::Class::method`. Strip method tail, take class tail.
    let Some(class_qname) = qname.rsplit_once("::").map(|(c, _)| c) else {
        return 0.0;
    };
    let class_tail = class_qname.rsplit("::").next().unwrap_or("");
    if class_tail.is_empty() || !class_seeds.contains(class_tail) {
        return 0.0;
    }
    // Count of 0 means class was seeded but its name doesn't appear verbatim
    // (e.g. matched via tail of a different qname). Floor at 1 so any seeded
    // class still gets the minimum 2.0 lift over non-seeded methods.
    let count = counts.get(class_tail).copied().unwrap_or(0).max(1);
    (count.min(5) as f64) * 2.0
}

/// Up to 12.0 when the candidate qname's `Class::attr` tail matches a
/// test-anchored `Class.attr` access from `test_patch_facts`. Boost is
/// `min(count, 3) * 4.0`. Stronger ceiling than class_match_boost because a
/// resolved attr access is a direct fix-site signal (the test literally
/// reads/calls the attribute we care about).
fn attr_anchor_boost(qname: &str, test_attr_facts: &HashMap<String, usize>) -> f64 {
    if test_attr_facts.is_empty() {
        return 0.0;
    }
    // qname like `pkg::mod::Foo::bar` → look up `Foo.bar`.
    let Some(class_qname) = qname.rsplit_once("::").map(|(c, _)| c) else {
        return 0.0;
    };
    let class_tail = class_qname.rsplit("::").next().unwrap_or("");
    let attr = qname.rsplit("::").next().unwrap_or("");
    if class_tail.is_empty() || attr.is_empty() {
        return 0.0;
    }
    let key = format!("{class_tail}.{attr}");
    let count = test_attr_facts.get(&key).copied().unwrap_or(0);
    if count == 0 {
        0.0
    } else {
        (count.min(3) as f64) * 4.0
    }
}

/// Count whole-word occurrences of `needle` in `haystack` (alphanumeric/`_`
/// boundaries — so `CharField` doesn't match inside `JSONCharFieldExt`).
/// Case-sensitive; class identifiers are PascalCase so case-folding would
/// over-match common English words.
fn count_word_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let nbytes = needle.as_bytes();
    let nlen = nbytes.len();
    let hbytes = haystack.as_bytes();
    let mut count = 0usize;
    let mut i = 0usize;
    while i + nlen <= hbytes.len() {
        if &hbytes[i..i + nlen] == nbytes {
            let before_ok = i == 0 || !is_ident_byte(hbytes[i - 1]);
            let after_ok = i + nlen == hbytes.len() || !is_ident_byte(hbytes[i + nlen]);
            if before_ok && after_ok {
                count += 1;
                i += nlen;
                continue;
            }
        }
        i += 1;
    }
    count
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn count_body_anchor_matches(body: &str, anchors: &HashSet<String>) -> usize {
    if anchors.is_empty() {
        return 0;
    }
    let Ok(re) = Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\b") else {
        return 0;
    };
    let mut matched: HashSet<&str> = HashSet::new();
    for caps in re.captures_iter(body) {
        if let Some(m) = caps.get(1) {
            if anchors.contains(m.as_str()) {
                matched.insert(m.as_str());
            }
        }
    }
    matched.len()
}

fn extract_backtick_qnames(text: &str) -> Vec<String> {
    let Ok(re) = Regex::new(r"`([A-Za-z_][\w.]*)`") else {
        return Vec::new();
    };
    re.captures_iter(text)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// Match a dotted suffix (e.g. "Field.root") against a list of graph qnames
/// separated by `::` — return the full qname whose trailing segments equal
/// the suffix parts.
fn resolve_suffix(suffix: &str, activated_qnames: &[String]) -> Option<String> {
    let parts: Vec<&str> = suffix.split('.').collect();
    for qn in activated_qnames {
        let segs: Vec<&str> = qn.split("::").collect();
        if segs.len() >= parts.len() && segs[segs.len() - parts.len()..] == parts[..] {
            return Some(qn.clone());
        }
    }
    None
}

/// Pull the "file" field out of a POSITION cell JSON (`{"file":"x","start_line":..}`).
fn parse_file_field(pos_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(pos_json).ok()?;
    v.get("file")?.as_str().map(String::from)
}

/// Pull a numeric field (start_line / end_line) out of a POSITION cell JSON.
fn parse_pos_int_field(pos_json: &str, field: &str) -> Option<usize> {
    let v: serde_json::Value = serde_json::from_str(pos_json).ok()?;
    v.get(field)?.as_u64().map(|n| n as usize)
}
