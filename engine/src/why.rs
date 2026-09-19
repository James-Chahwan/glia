//! `why(edge)` (LE.5): for any two nodes, every edge between them with the
//! extractor / resolver that emitted it, its call site and confidence, tiered
//! fact / derived / heuristic; a located witness path when there is no direct
//! edge.
//!
//! The evidence is LC.3a's: one EVIDENCE cell per edge, read with
//! [`Evidence::of`] — `emitter` (`<stage>:<name>`), `rule`, `file`, 0-based
//! `line` and `basis`. This module only reads it back as an answer.
//!
//! # Endpoints
//!
//! Each side resolves through `find` to ALL nodes whose qname equals the
//! argument (a Python dotted path counts), else ALL whose simple name equals
//! it, in find's exact order (`pick_primary`'s key), capped at
//! [`MAX_ENDPOINTS`] — two per-language graphs can hold one qname, and several
//! functions can share a name. A side that names no node is an `Err` that
//! lists find's nearest qnames.
//!
//! # Tiers
//!
//! From the emitter's STAGE (the text before `:`, LC.3a's closed vocabulary
//! `code_domain::evidence::STAGES`):
//!
//! | stage | tier |
//! |---|---|
//! | `parser`, `extractor`, `docs`, `graph` | fact — read at a site in the source, or bound from an extracted reference |
//! | `resolver`, `pass` | derived — paired by name / path / topic / convention |
//! | `overlay`, `history` | heuristic — declared by a person or a model, or co-change in git |
//!
//! Exceptions, each with a row `note`: a `graph` edge bound by a name-only
//! guess (LC.3d: `graph:refs` `global_unique` / `global_unique_method`,
//! `graph:imports` `tail_unique`, `graph:rust_paths` `unique_in_crate` /
//! `tail_unique`, `graph:nav` `suffix` / `href_suffix`) is heuristic; a fact
//! whose evidence has basis `none` is derived ("no location recorded"); an
//! edge with no EVIDENCE cell (a `.gmap` from before LC.3a) is derived ("no
//! evidence recorded"); a stage outside the vocabulary is derived ("unknown
//! emitter stage"). Nothing is ever shown as fact without its evidence.
//!
//! # No direct edge
//!
//! `found` is false, `absence` is LD.8a's `no_edges` for the pair (mechanisms:
//! the requested category, else every carry category), and `path` is a
//! shortest forward path over `CODE_PROFILE.tables.carry_edges` within
//! [`MAX_PATH_HOPS`] hops — one `Adjacency`, one BFS per `from` node, the first
//! hit in `(from, to)` order — each hop explained like a row by the first edge
//! in global edge order that the walk took. A path is a witness that the two
//! are connected, never the reason one depends on the other.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `glia_engine::why::<item>`, never flattened into the
//! crate root.
//!
//! fired_on marker, one line per answered call:
//! `[why] edges=<N> path_hops=<H> found=<true|false> tiers fact=<F> derived=<D> heuristic=<H2>`
//! — grep `^\[why\] edges=`.

use std::collections::{HashMap, HashSet};

use glia_activation::algo::{Adjacency, CategorySet, Walk, reach};
use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{cell_type, edge_category};
use glia_core::{CellPayload, Confidence, Edge, EdgeCategoryId, NodeId};
use glia_graph::MergedGraph;

use crate::absence::{self, Absence, plural};
use crate::answers::Locator;
use crate::find::{self, FindOptions};
use crate::profile::CODE_PROFILE;

/// Most nodes one side of the question resolves to.
pub const MAX_ENDPOINTS: usize = 8;
/// Longest witness path, in carry hops.
pub const MAX_PATH_HOPS: usize = 6;

const FACT: &str = "fact";
const DERIVED: &str = "derived";
const HEURISTIC: &str = "heuristic";

/// `(emitter, rule)` pairs that bind by a name alone (LC.3d's handoff): the
/// only candidate with that name, not a binding the source spells out.
const NAME_ONLY_RULES: &[(&str, &str)] = &[
    ("graph:refs", "global_unique"),
    ("graph:refs", "global_unique_method"),
    ("graph:imports", "tail_unique"),
    ("graph:rust_paths", "unique_in_crate"),
    ("graph:rust_paths", "tail_unique"),
    ("graph:nav", "suffix"),
    ("graph:nav", "href_suffix"),
];

/// Where an edge was asserted: repo-relative `file` and its 1-based `line`
/// (LD.1), `None` when the evidence names the file only.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Site {
    pub file: String,
    pub line: Option<i64>,
}

/// One edge, explained: its endpoints, category and confidence, the tier its
/// evidence earns and the evidence itself.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct EdgeWhy {
    pub from_id: u64,
    pub from_qname: String,
    pub to_id: u64,
    pub to_qname: String,
    /// The `edge_category::name` spelling, e.g. `"CALLS"`.
    pub category: &'static str,
    /// `strong` | `medium` | `weak`.
    pub confidence: &'static str,
    /// `fact` | `derived` | `heuristic` (module doc).
    pub tier: &'static str,
    /// `<stage>:<name>`, e.g. `graph:calls` or `resolver:http`; `None` when
    /// the edge carries no evidence.
    pub emitter: Option<String>,
    /// The branch or match tier that asserted it, e.g. `import_binding` or
    /// the HTTP resolver's `exact`.
    pub rule: Option<String>,
    /// `site` | `from_node` | `to_node` | `file` | `none`: how `site` was
    /// obtained. `site` is the asserting construct itself; the node bases
    /// are the enclosing declaration.
    pub basis: Option<&'static str>,
    pub site: Option<Site>,
    /// The two ends sit in different repos of the build.
    pub cross_repo: bool,
    /// Why the tier is not the stage's plain one, or where an overlay edge
    /// was declared.
    pub note: Option<String>,
}

/// [`why_edge`]'s answer: the direct edges, or — when there are none — the
/// witness path and the absence.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct WhyAnswer {
    /// At least one edge runs from a `from` node to a `to` node (of the
    /// requested category).
    pub found: bool,
    /// One row per edge, in `(from, to)` resolution order, then by site
    /// (file, line), then global edge order: two call sites of A -> B are two
    /// rows.
    pub edges: Vec<EdgeWhy>,
    /// When `found` is false: a shortest carry path, one row per hop, empty
    /// when none is within [`MAX_PATH_HOPS`].
    pub path: Vec<EdgeWhy>,
    /// The qnames each side resolved to, in find's exact order.
    pub from_nodes: Vec<String>,
    pub to_nodes: Vec<String>,
    pub note: Option<String>,
    /// LD.8a `no_edges`; `Some` iff `found` is false.
    pub absence: Option<Absence>,
}

/// Every edge from the node(s) `from` names to the node(s) `to` names, of
/// `category` when given (any case, an `edge_category` name), each explained
/// (module doc). With none, the witness path and the absence.
///
/// Errors: a side that names no node (with find's nearest qnames), an unknown
/// category (with the valid names).
///
/// Cost: two find searches, one O(V) Locator, one O(E) scan of every edge;
/// with no direct edge also one carry `Adjacency` (O(V + E)), one BFS per
/// `from` node, one more O(E) scan and the absence's caveat rows.
pub fn why_edge(
    merged: &MergedGraph,
    from: &str,
    to: &str,
    category: Option<&str>,
) -> Result<WhyAnswer, String> {
    let category = category.map(category_of).transpose()?;
    let (froms, from_cut) = resolve_side(merged, from)?;
    let (tos, to_cut) = resolve_side(merged, to)?;
    let loc = Locator::new(merged);
    let wanted = |e: &Edge| category.is_none_or(|c| e.category == c);

    let from_at: HashMap<NodeId, usize> =
        froms.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let to_at: HashMap<NodeId, usize> = tos.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut direct: Vec<(usize, usize, &Edge)> = Vec::new();
    let mut reverse = 0usize;
    for e in merged.all_edges().filter(|e| wanted(e)) {
        if let (Some(&fi), Some(&ti)) = (from_at.get(&e.from), to_at.get(&e.to)) {
            direct.push((fi, ti, e));
        } else if to_at.contains_key(&e.from) && from_at.contains_key(&e.to) {
            reverse += 1;
        }
    }
    let mut qnames: HashMap<NodeId, String> = HashMap::new();
    let mut explain = |e: &Edge| explain_edge(merged, &loc, &mut qnames, e);
    let mut rows: Vec<(usize, usize, EdgeWhy)> = direct
        .into_iter()
        .map(|(fi, ti, e)| (fi, ti, explain(e)))
        .collect();
    // Stable, so rows with one site (or none) keep global edge order.
    rows.sort_by(|a, b| {
        let site = |r: &EdgeWhy| r.site.as_ref().map(|s| (s.file.clone(), s.line));
        (a.0, a.1, site(&a.2)).cmp(&(b.0, b.1, site(&b.2)))
    });
    let edges: Vec<EdgeWhy> = rows.into_iter().map(|(_, _, r)| r).collect();
    let found = !edges.is_empty();

    let mut notes: Vec<String> = Vec::new();
    for (side, cut) in [(from, from_cut), (to, to_cut)] {
        if cut {
            notes.push(format!(
                "`{}` names more than {MAX_ENDPOINTS} nodes; the first {MAX_ENDPOINTS} in find's exact order are used",
                side.trim()
            ));
        }
    }

    let mut path = Vec::new();
    let mut absence = None;
    if !found {
        let hops = witness_path(merged, &froms, &tos);
        path = explain_hops(merged, &hops, &mut explain);
        let what = edge_word(category);
        notes.insert(
            0,
            if path.is_empty() {
                format!("no direct {what}; not connected within {MAX_PATH_HOPS} carry hops")
            } else {
                format!(
                    "no direct {what}; `path` is a shortest {}-hop carry path, a witness that they connect, not the reason",
                    path.len()
                )
            },
        );
        if reverse > 0 {
            notes.push(format!(
                "{reverse} {} the other way: ask why(`{}`, `{}`)",
                plural(reverse, "edge runs", "edges run"),
                to.trim(),
                from.trim()
            ));
        }
        absence = Some(no_edges(merged, &loc, from, to, category, froms[0]));
    }

    let (mut fact, mut derived, mut heuristic) = (0usize, 0usize, 0usize);
    for r in edges.iter().chain(&path) {
        match r.tier {
            FACT => fact += 1,
            HEURISTIC => heuristic += 1,
            _ => derived += 1,
        }
    }
    eprintln!(
        "[why] edges={} path_hops={} found={found} tiers fact={fact} derived={derived} heuristic={heuristic}",
        edges.len(),
        path.len()
    );

    let qname_of = |id: &NodeId| loc.locate(*id).qname;
    Ok(WhyAnswer {
        found,
        edges,
        path,
        from_nodes: froms.iter().map(qname_of).collect(),
        to_nodes: tos.iter().map(qname_of).collect(),
        note: (!notes.is_empty()).then(|| notes.join("; ")),
        absence,
    })
}

/// The registry id of a category name, any case; unknown lists every name.
fn category_of(name: &str) -> Result<EdgeCategoryId, String> {
    let want = name.trim();
    edge_category::ALL
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(want))
        .map(|(id, _)| *id)
        .ok_or_else(|| {
            let names: Vec<&str> = edge_category::ALL.iter().map(|(_, n)| *n).collect();
            format!(
                "unknown edge category `{want}`; valid: {}",
                names.join(", ")
            )
        })
}

/// The nodes `query` names: find's `exact_qname` rows, else its `exact_name`
/// rows, at most [`MAX_ENDPOINTS`], and whether more were cut. None: an error
/// naming find's nearest qnames.
fn resolve_side(merged: &MergedGraph, query: &str) -> Result<(Vec<NodeId>, bool), String> {
    let opts = FindOptions {
        top_k: MAX_ENDPOINTS + 1,
        ..FindOptions::default()
    };
    let rows = find::search(merged, query, &opts).rows;
    let Some(tier) = rows.iter().find(|r| find::is_exact(r)).map(|r| r.r#match) else {
        let near: Vec<String> = rows
            .iter()
            .take(absence::SUGGESTIONS)
            .map(|r| format!("`{}`", r.qname))
            .collect();
        let hint = if near.is_empty() {
            String::new()
        } else {
            format!("; nearest: {}", near.join(", "))
        };
        return Err(format!("no node with qname/name `{}`{hint}", query.trim()));
    };
    let mut ids: Vec<NodeId> = rows
        .iter()
        .filter(|r| r.r#match == tier)
        .map(|r| NodeId(r.id))
        .collect();
    let cut = ids.len() > MAX_ENDPOINTS;
    ids.truncate(MAX_ENDPOINTS);
    Ok((ids, cut))
}

/// A shortest forward carry path from a `froms` node to a `tos` node: one
/// `Adjacency` and one BFS per `from`, the first hit in `(from, to)` order.
/// Each hop is `(from, to, category)`; empty when none is within
/// [`MAX_PATH_HOPS`]. A node on both sides is no path to itself.
fn witness_path(
    merged: &MergedGraph,
    froms: &[NodeId],
    tos: &[NodeId],
) -> Vec<(NodeId, NodeId, EdgeCategoryId)> {
    let adj = Adjacency::build(merged, &CategorySet::of(CODE_PROFILE.tables.carry_edges));
    for &f in froms {
        let reached = reach::bfs(&adj, &[f], Walk::Forward, MAX_PATH_HOPS).reached;
        let entered: HashMap<NodeId, (EdgeCategoryId, NodeId)> =
            reached.iter().map(|r| (r.id, (r.via, r.parent))).collect();
        for &t in tos.iter().filter(|&&t| t != f) {
            if !entered.contains_key(&t) {
                continue;
            }
            // Every parent was discovered before its child and the chain ends
            // at the seed `f`, so this walks at most `reached.len()` steps.
            let mut hops = Vec::new();
            let mut cur = t;
            while cur != f {
                let Some(&(via, parent)) = entered.get(&cur) else {
                    break;
                };
                hops.push((parent, cur, via));
                cur = parent;
            }
            if cur == f {
                hops.reverse();
                return hops;
            }
        }
    }
    Vec::new()
}

/// Each hop explained by the first edge in global edge order with its
/// `(from, to, category)` — the incidence the BFS took. One O(E) scan.
fn explain_hops(
    merged: &MergedGraph,
    hops: &[(NodeId, NodeId, EdgeCategoryId)],
    explain: &mut impl FnMut(&Edge) -> EdgeWhy,
) -> Vec<EdgeWhy> {
    if hops.is_empty() {
        return Vec::new();
    }
    let wanted: HashSet<(NodeId, NodeId, EdgeCategoryId)> = hops.iter().copied().collect();
    let mut first: HashMap<(NodeId, NodeId, EdgeCategoryId), &Edge> = HashMap::new();
    for e in merged.all_edges() {
        let key = (e.from, e.to, e.category);
        if wanted.contains(&key) {
            first.entry(key).or_insert(e);
        }
    }
    hops.iter()
        .filter_map(|k| first.get(k).map(|e| explain(e)))
        .collect()
}

/// The LD.8a absence for a pair with no direct edge.
fn no_edges(
    merged: &MergedGraph,
    loc: &Locator<'_>,
    from: &str,
    to: &str,
    category: Option<EdgeCategoryId>,
    seed: NodeId,
) -> Absence {
    let mechanisms: Vec<&'static str> = match category {
        Some(c) => vec![edge_category::name(c)],
        None => CODE_PROFILE
            .tables
            .carry_edges
            .iter()
            .map(|c| edge_category::name(*c))
            .collect(),
    };
    let what = edge_word(category);
    let note = format!(
        "no {what} runs from `{}` to `{}` in this graph",
        from.trim(),
        to.trim()
    );
    let query = format!("{} -> {}", from.trim(), to.trim());
    let seed_file = loc.file_of(seed);
    absence::empty(
        merged,
        "why",
        &query,
        "no_edges",
        note,
        &mechanisms,
        seed_file.as_deref(),
    )
}

/// `edge`, or `<CATEGORY> edge` when the question names one.
fn edge_word(category: Option<EdgeCategoryId>) -> String {
    category.map_or_else(
        || "edge".to_string(),
        |c| format!("{} edge", edge_category::name(c)),
    )
}

/// One row: the edge's endpoints, category, confidence and evidence, tiered.
fn explain_edge(
    merged: &MergedGraph,
    loc: &Locator<'_>,
    qnames: &mut HashMap<NodeId, String>,
    e: &Edge,
) -> EdgeWhy {
    let mut qname = |id: NodeId| {
        qnames
            .entry(id)
            .or_insert_with(|| loc.locate(id).qname)
            .clone()
    };
    let ev = Evidence::of(e);
    let (tier, note) = tier_of(ev.as_ref(), e);
    let site = ev.as_ref().and_then(|ev| {
        let file = match &ev.file {
            Some(f) => Some(f.clone()),
            // A site line the fill pass never placed: the caller's file.
            None if ev.line.is_some() => loc.file_of(e.from),
            None => None,
        }?;
        Some(Site {
            file,
            line: ev.line.map(|l| i64::from(l) + 1),
        })
    });
    EdgeWhy {
        from_id: e.from.0,
        from_qname: qname(e.from),
        to_id: e.to.0,
        to_qname: qname(e.to),
        category: edge_category::name(e.category),
        confidence: confidence_name(e.confidence),
        tier,
        emitter: ev.as_ref().map(|ev| ev.emitter.clone()),
        rule: ev.as_ref().and_then(|ev| ev.rule.clone()),
        basis: ev.as_ref().map(|ev| basis_name(ev.basis)),
        site,
        cross_repo: cross_repo(merged, e.from, e.to),
        note,
    }
}

/// The plain tier of an emitter stage; `None` for a stage outside the
/// documented vocabulary.
fn stage_tier(stage: &str) -> Option<&'static str> {
    match stage {
        "parser" | "extractor" | "docs" | "graph" => Some(FACT),
        "resolver" | "pass" => Some(DERIVED),
        "overlay" | "history" => Some(HEURISTIC),
        _ => None,
    }
}

/// `(tier, note)` for an edge's evidence (module doc).
fn tier_of(ev: Option<&Evidence>, e: &Edge) -> (&'static str, Option<String>) {
    let Some(ev) = ev else {
        return (DERIVED, Some("no evidence recorded".to_string()));
    };
    let stage = ev.emitter.split(':').next().unwrap_or("");
    let Some(tier) = stage_tier(stage) else {
        return (DERIVED, Some(format!("unknown emitter stage `{stage}`")));
    };
    let rule = ev.rule.as_deref();
    if tier == FACT
        && let Some(rule) = rule
        && NAME_ONLY_RULES
            .iter()
            .any(|&(em, r)| em == ev.emitter && r == rule)
    {
        return (HEURISTIC, Some(format!("name-only binding ({rule})")));
    }
    if tier == FACT && ev.basis == Basis::None {
        return (DERIVED, Some("no location recorded".to_string()));
    }
    let note = match stage {
        "overlay" => Some(overlay_note(ev, e)),
        "history" => Some("files change together in git history; not a code reference".to_string()),
        _ => None,
    };
    (tier, note)
}

/// Where and by whom an overlay edge was declared (LF.2b): the stanza's line
/// and the ORIGIN edge cell's provenance.
fn overlay_note(ev: &Evidence, e: &Edge) -> String {
    if ev.emitter != "overlay:edge" {
        let rule = ev
            .rule
            .as_deref()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        return format!("matched by a declared `.glia/overlay.toml` stanza{rule}");
    }
    let provenance = e
        .cells
        .iter()
        .filter(|c| c.kind == cell_type::ORIGIN)
        .find_map(|c| {
            let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
                return None;
            };
            let v: serde_json::Value = serde_json::from_str(s).ok()?;
            v.get("provenance")?.as_str().map(str::to_string)
        })
        .unwrap_or_else(|| "overlay".to_string());
    let at = match (&ev.file, ev.line) {
        (Some(f), Some(l)) => format!("{f}:{}", u64::from(l) + 1),
        (Some(f), None) => f.clone(),
        _ => ".glia/overlay.toml".to_string(),
    };
    format!("declared in {at} by {provenance}")
}

/// The ends sit in different repos: the first graph (in merge order) holding
/// each id names its repo. An unknown end is not cross-repo.
fn cross_repo(merged: &MergedGraph, from: NodeId, to: NodeId) -> bool {
    let repo_of = |id: NodeId| {
        merged
            .graphs
            .iter()
            .find(|g| g.nav.kind_by_id.contains_key(&id))
            .map(|g| g.repo)
    };
    matches!((repo_of(from), repo_of(to)), (Some(a), Some(b)) if a != b)
}

fn confidence_name(c: Confidence) -> &'static str {
    match c {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    }
}

fn basis_name(b: Basis) -> &'static str {
    match b {
        Basis::Site => "site",
        Basis::FromNode => "from_node",
        Basis::ToNode => "to_node",
        Basis::File => "file",
        Basis::None => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::evidence::STAGES;

    #[test]
    fn every_documented_stage_has_a_tier() {
        // A new stage in LC.3a's vocabulary must be tiered here explicitly;
        // until it is, its edges read as derived with an "unknown" note.
        for s in STAGES {
            assert!(stage_tier(s).is_some(), "stage `{s}` has no tier");
        }
        assert_eq!(stage_tier("oracle"), None);
    }

    #[test]
    fn name_only_rules_belong_to_fact_stages() {
        for (emitter, _) in NAME_ONLY_RULES {
            let stage = emitter.split(':').next().unwrap_or("");
            assert_eq!(stage_tier(stage), Some(FACT), "{emitter}");
        }
    }

    #[test]
    fn category_names_resolve_in_any_case() {
        assert_eq!(category_of("calls"), Ok(edge_category::CALLS));
        assert_eq!(category_of(" HTTP_CALLS "), Ok(edge_category::HTTP_CALLS));
        let err = category_of("nope").expect_err("unknown");
        assert!(
            err.starts_with("unknown edge category `nope`; valid: "),
            "{err}"
        );
    }
}
