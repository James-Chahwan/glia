//! Co-change suggestions, ROSE style (CC.11a, + CC.11b): for a set of changed
//! files, the files that usually change with them — "you changed X; Y
//! usually changes with it" (Zimmermann et al., ICSE 2004) — with a
//! DIRECTIONAL confidence, the support, and whether any static link joins
//! them. Public slot, reached by module path (`glia_engine::cochange::<item>`).
//! CC.11b adds multi-antecedent rules mined from the history snapshot's
//! commits and the working tree's changed set against a git rev.
//!
//! Pairwise rules come from the graph alone: the CO_CHANGES edges LF.5b
//! stores (one per kept file pair, ATTN `{cochanges, ratio_permille,
//! window_commits}`) and every MODULE's churn ATTN (`commits`), both read
//! through `external::signals` (CC.2). LF.5b's `ratio_permille` is symmetric
//! (co-changes over the RARER file's commits), so it cannot say which way a
//! rule holds; this answer divides by the QUERY file's own commits instead.
//! Measured on Kina: changing `report_formatter.go` (3 commits) touched
//! `admin_service.go` every time (1.00), while `admin_service.go` (45
//! commits) touched the formatter in 0.07 of its commits.
//!
//! [`cochange`], in steps:
//!
//! 1. Query files are trimmed of a leading `./`, sorted and deduplicated.
//!    Each maps to the MODULE whose first POSITION names it exactly
//!    (repo-relative, in any repo of the graph; several MODULEs on one file
//!    bind the smallest NodeId — the history stage's mapping rule). A file no
//!    MODULE names goes to [`Cochange::unmapped`], never guessed.
//! 2. Every CO_CHANGES edge with a query MODULE Q (file F) at either end
//!    gives a candidate rule F -> H, H the other end's file (never a query
//!    file): `support` = the edge's `cochanges`, `antecedent_commits` =
//!    Q's churn `commits`, `confidence_permille` = `1000 * support /
//!    antecedent_commits`. Rules under `min_support` or
//!    `min_confidence_permille` are dropped.
//! 3. One row per consequent file (per repo): its best rule by (confidence
//!    desc, support desc, antecedent path asc). Rows sort by (confidence
//!    desc, support desc, file, module qname).
//! 4. `link`: the best of `direct` > `bridged` > `none` between H and every
//!    query file of H's repo — LF.5c's exact test (`gaps::LinkIndex`: one
//!    edge, or a path of at most three edges through file-less nodes, over
//!    every category but CO_CHANGES / DEFINES / CONTAINS). The index is
//!    built once per answer, when the first CO_CHANGES edge touches a query
//!    MODULE (step 2 reads the consequent's file from it too).
//!    `unlinked_only` keeps the `none` rows; then the list is cut to `top`.
//!
//! Every row is HEURISTIC: co-change is history, not proof of coupling. A row
//! with no static link carries the note "no static link joins them (a blind
//! spot, or coupling outside code)" — the case the suggestion is for.
//!
//! Absence (LD.8a, mechanism CO_CHANGES): no CO_CHANGES edge in the graph is
//! `no_history` (built without a `glia history sync` snapshot); otherwise an
//! empty answer is `no_match`, naming the floors (or saying no query file is
//! a MODULE).
//!
//! Only pairs LF.5b kept exist as edges (support >= 3 and >= 300 per mille
//! of the rarer file's commits, at most 5000 per repo), so a rule whose
//! consequent is the rarer file can be missing when its pair failed LF.5b's
//! floor; CC.11b's commits path has no such floor.
//!
//! Read-only, deterministic: maps are BTreeMaps or lookup-only HashMaps.
//! fired_on marker, once per call:
//! `[cochange-suggest] query_files=<q> unmapped=<u> candidates=<c> rows=<r> unlinked=<n> source=pairwise`
//! where `candidates` counts the rules of step 2 before the floors and
//! `unlinked` the returned rows whose link is `none`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, NodeId};
use glia_graph::MergedGraph;

use crate::absence::{self, Absence};
use crate::external::signals;
use crate::gaps::{HEURISTIC, Link, LinkIndex};

/// The primitive name in the `[absence]` marker.
const PRIMITIVE: &str = "cochange";

/// The edge categories the answer depends on.
const MECHANISMS: &[&str] = &["CO_CHANGES"];

/// [`CochangeRow::source`] of a single-file rule.
const PAIRWISE: &str = "pairwise";

/// The note on a row no static link explains.
const UNLINKED_NOTE: &str = "no static link joins them (a blind spot, or coupling outside code)";

/// The floors and the cut of [`cochange`]. Start from `default()` and set
/// fields: `#[non_exhaustive]` rules out a struct literal outside this crate.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct CochangeArgs {
    /// A rule's confidence floor, per mille of its antecedent's commits
    /// (default 300).
    pub min_confidence_permille: u32,
    /// A rule's support floor, in co-changing commits (default 3).
    pub min_support: u32,
    /// At most this many rows (default 20).
    pub top: usize,
    /// Keep only the rows no static link joins to the query (default false).
    pub unlinked_only: bool,
}

impl Default for CochangeArgs {
    fn default() -> Self {
        CochangeArgs {
            min_confidence_permille: 300,
            min_support: 3,
            top: 20,
            unlinked_only: false,
        }
    }
}

/// One suggested file: the best rule "antecedent -> file" (module docs).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct CochangeRow {
    /// The consequent: a repo-relative file that usually changes with the
    /// antecedent.
    pub file: String,
    /// Its MODULE's qname.
    pub module_qname: String,
    /// The query file(s) the rule starts from; one for a pairwise rule.
    pub antecedent: Vec<String>,
    /// Commits that touched the antecedent and `file` together.
    pub support: u32,
    /// Commits that touched the antecedent (its module churn).
    ///
    /// DENOMINATOR NOTE: churn counts EVERY commit of the snapshot window that
    /// touched the file, while `support` counts only commits of 2..=30 mapped
    /// files (the history stage's `MAX_COMMIT_FILES`: a mass reformat adds no
    /// pair), so `confidence_permille` is a lower bound when mass commits
    /// touched the antecedent.
    pub antecedent_commits: u32,
    /// `1000 * support / antecedent_commits`: of the commits that changed the
    /// antecedent, the share that also changed `file` — directional.
    pub confidence_permille: u32,
    /// `direct` | `bridged` | `none`: the best static link between `file` and
    /// any query file of its repo (LF.5c's test).
    pub link: &'static str,
    /// `pairwise` (one antecedent file) | `multi` (CC.11b).
    pub source: &'static str,
    /// Always `heuristic`: co-change is history, not proof of coupling.
    pub tier: &'static str,
    /// Set when `link` is `none`: a blind spot, or coupling outside code.
    pub note: Option<String>,
}

/// The co-change suggestions for a set of changed files (module docs).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct Cochange {
    /// The query, trimmed of a leading `./`, sorted and deduplicated.
    pub query_files: Vec<String>,
    /// Query files no MODULE of the graph names, sorted.
    pub unmapped: Vec<String>,
    pub rows: Vec<CochangeRow>,
    /// `Some` exactly when `rows` is empty.
    pub absence: Option<Absence>,
}

/// A query MODULE: the file it maps, its repo and its churn commits.
struct Antecedent<'a> {
    repo: u64,
    file: &'a str,
    commits: Option<u32>,
}

/// A rule that passed the floors, before the link.
struct Rule<'a> {
    repo: u64,
    file: String,
    module: NodeId,
    antecedent: &'a str,
    support: u32,
    antecedent_commits: u32,
    confidence_permille: u32,
}

/// **cochange** (CC.11a): the files that usually change with `files`, by
/// pairwise rule over the graph's CO_CHANGES edges (module docs). Read-only;
/// prints the `[cochange-suggest]` marker.
pub fn cochange(merged: &MergedGraph, files: &[String], args: &CochangeArgs) -> Cochange {
    let query_files = normalise(files);
    let query_set: BTreeSet<&str> = query_files.iter().map(String::as_str).collect();

    let modules = query_modules(merged, &query_set);
    let mapped: BTreeSet<&str> = modules.values().map(|a| a.file).collect();
    let unmapped: Vec<String> = query_files
        .iter()
        .filter(|f| !mapped.contains(f.as_str()))
        .cloned()
        .collect();

    let co_edges: Vec<_> = merged
        .all_edges()
        .filter(|e| e.category == edge_category::CO_CHANGES)
        .collect();

    let mut links: Option<LinkIndex> = None;
    let mut candidates = 0usize;
    // (repo, consequent file) -> its best rule.
    let mut best: BTreeMap<(u64, String), Rule> = BTreeMap::new();
    for e in &co_edges {
        for (q, other) in [(e.from, e.to), (e.to, e.from)] {
            let Some(ante) = modules.get(&q.0) else {
                continue;
            };
            if modules.contains_key(&other.0) {
                continue;
            }
            let ix = links.get_or_insert_with(|| LinkIndex::build(merged));
            let Some((repo, file)) = ix.files.first_file(other) else {
                continue;
            };
            if repo != ante.repo || query_set.contains(file) {
                continue;
            }
            candidates += 1;
            let (Some(pair), Some(commits)) = (signals::pair_counts(&e.cells), ante.commits) else {
                continue;
            };
            if commits == 0 {
                continue;
            }
            let support = pair.cochanges;
            let confidence = (1000 * u64::from(support) / u64::from(commits)).min(1000);
            let confidence_permille = u32::try_from(confidence).unwrap_or(1000);
            if support < args.min_support || confidence_permille < args.min_confidence_permille {
                continue;
            }
            let rule = Rule {
                repo,
                file: file.to_string(),
                module: other,
                antecedent: ante.file,
                support,
                antecedent_commits: commits,
                confidence_permille,
            };
            match best.get_mut(&(repo, rule.file.clone())) {
                Some(slot) if !better(&rule, slot) => {}
                Some(slot) => *slot = rule,
                None => {
                    best.insert((repo, rule.file.clone()), rule);
                }
            }
        }
    }

    let qnames = qnames_of(merged, best.values().map(|r| r.module));
    let mut ranked: Vec<(Rule, String)> = best
        .into_values()
        .map(|r| {
            let q = qnames.get(&r.module.0).cloned().unwrap_or_default();
            (r, q)
        })
        .collect();
    ranked.sort_by(|(x, xq), (y, yq)| {
        y.confidence_permille
            .cmp(&x.confidence_permille)
            .then_with(|| y.support.cmp(&x.support))
            .then_with(|| x.file.cmp(&y.file))
            .then_with(|| xq.cmp(yq))
    });

    let mut rows: Vec<CochangeRow> = Vec::new();
    for (r, module_qname) in ranked {
        if rows.len() >= args.top {
            break;
        }
        let link = match &links {
            Some(ix) => best_link(ix, r.repo, &r.file, &query_files),
            None => Link::None,
        };
        if args.unlinked_only && link != Link::None {
            continue;
        }
        rows.push(CochangeRow {
            module_qname,
            file: r.file,
            antecedent: vec![r.antecedent.to_string()],
            support: r.support,
            antecedent_commits: r.antecedent_commits,
            confidence_permille: r.confidence_permille,
            link: link_name(link),
            source: PAIRWISE,
            tier: HEURISTIC,
            note: (link == Link::None).then(|| UNLINKED_NOTE.to_string()),
        });
    }

    let unlinked = rows
        .iter()
        .filter(|r| r.link == link_name(Link::None))
        .count();
    eprintln!(
        "[cochange-suggest] query_files={} unmapped={} candidates={candidates} rows={} unlinked={unlinked} source={PAIRWISE}",
        query_files.len(),
        unmapped.len(),
        rows.len()
    );

    let absence = rows.is_empty().then(|| {
        let query = query_files.join(", ");
        let (reason, note) = if co_edges.is_empty() {
            (
                "no_history",
                "no CO_CHANGES edge in this graph: it was built without a git-history \
                 snapshot (run `glia history sync <repo>`, then rebuild)"
                    .to_string(),
            )
        } else {
            ("no_match", no_match_note(&query_files, mapped.len(), args))
        };
        absence::empty(merged, PRIMITIVE, &query, reason, note, MECHANISMS, None)
    });

    Cochange {
        query_files,
        unmapped,
        rows,
        absence,
    }
}

/// `files` trimmed of a leading `./`, empties dropped, sorted, deduplicated.
fn normalise(files: &[String]) -> Vec<String> {
    let set: BTreeSet<String> = files
        .iter()
        .map(|f| {
            let mut f = f.trim();
            while let Some(rest) = f.strip_prefix("./") {
                f = rest;
            }
            f.to_string()
        })
        .filter(|f| !f.is_empty())
        .collect();
    set.into_iter().collect()
}

/// Node id -> the query MODULE it is: per (repo, query file), the MODULE whose
/// first POSITION names the file, the smallest NodeId on a tie (the history
/// stage's rule), with its churn commits.
fn query_modules<'a>(
    merged: &MergedGraph,
    query: &BTreeSet<&'a str>,
) -> HashMap<u64, Antecedent<'a>> {
    let mut by_file: BTreeMap<(u64, &'a str), (NodeId, Option<u32>)> = BTreeMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                continue;
            }
            let Some(file) = first_position_file(&n.cells) else {
                continue;
            };
            let Some(&file) = query.get(file.as_str()) else {
                continue;
            };
            let commits = signals::module_churn(&n.cells).map(|c| c.commits);
            by_file
                .entry((g.repo.0, file))
                .and_modify(|slot| {
                    if n.id.0 < slot.0.0 {
                        *slot = (n.id, commits);
                    }
                })
                .or_insert((n.id, commits));
        }
    }
    by_file
        .into_iter()
        .map(|((repo, file), (id, commits))| {
            (
                id.0,
                Antecedent {
                    repo,
                    file,
                    commits,
                },
            )
        })
        .collect()
}

/// The non-empty `file` of the first POSITION cell, as the history stage
/// reads it.
fn first_position_file(cells: &[Cell]) -> Option<String> {
    let c = cells.iter().find(|c| c.kind == cell_type::POSITION)?;
    let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
        return None;
    };
    serde_json::from_str::<serde_json::Value>(s)
        .ok()?
        .get("file")?
        .as_str()
        .filter(|f| !f.is_empty())
        .map(str::to_string)
}

/// `rule` beats `slot`: (confidence, support) higher, then the smaller
/// antecedent path.
fn better(rule: &Rule, slot: &Rule) -> bool {
    rule.confidence_permille
        .cmp(&slot.confidence_permille)
        .then_with(|| rule.support.cmp(&slot.support))
        .then_with(|| slot.antecedent.cmp(rule.antecedent))
        .is_gt()
}

/// The qname of each of `ids`, looked up in graph order (the first graph that
/// names an id wins).
fn qnames_of(merged: &MergedGraph, ids: impl Iterator<Item = NodeId>) -> HashMap<u64, String> {
    let mut out: HashMap<u64, String> = HashMap::new();
    for id in ids {
        if out.contains_key(&id.0) {
            continue;
        }
        if let Some(q) = merged
            .graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id))
        {
            out.insert(id.0, q.clone());
        }
    }
    out
}

/// The best link between `file` and any query file of `repo`: direct >
/// bridged > none.
fn best_link(ix: &LinkIndex, repo: u64, file: &str, query: &[String]) -> Link {
    let mut best = Link::None;
    for q in query {
        match ix.link(repo, file, q) {
            Link::Direct => return Link::Direct,
            Link::Bridged => best = Link::Bridged,
            Link::None => {}
        }
    }
    best
}

fn link_name(link: Link) -> &'static str {
    match link {
        Link::Direct => "direct",
        Link::Bridged => "bridged",
        Link::None => "none",
    }
}

/// Why no row came back, when the graph does hold co-change history.
fn no_match_note(query: &[String], mapped: usize, args: &CochangeArgs) -> String {
    if query.is_empty() {
        return "no query file was given".to_string();
    }
    if mapped == 0 {
        return format!(
            "none of the {} query {} is a MODULE in this graph",
            query.len(),
            absence::plural(query.len(), "file", "files")
        );
    }
    let mut note = format!(
        "no co-change rule from the {mapped} mapped query {} passes min_support={} min_confidence_permille={}",
        absence::plural(mapped, "file", "files"),
        args.min_support,
        args.min_confidence_permille
    );
    if args.unlinked_only {
        note.push_str(" unlinked_only");
    }
    if args.top == 0 {
        note.push_str(" top=0");
    }
    note
}
