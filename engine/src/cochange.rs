//! Co-change suggestions, ROSE style (CC.11a, + CC.11b): for a set of changed
//! files, the files that usually change with them — "you changed X; Y
//! usually changes with it" (Zimmermann et al., ICSE 2004) — with a
//! DIRECTIONAL confidence, the support, and whether any static link joins
//! them. Public slot, reached by module path (`glia_engine::cochange::<item>`).
//! CC.11b adds multi-antecedent rules mined from the history snapshot's
//! commits ([`cochange_multi`]) and the working tree's changed set against a
//! git rev ([`cochange_vs_rev`]).
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
//!    desc, support desc, larger antecedent, antecedent paths asc). Rows sort
//!    by (confidence desc, support desc, file, module qname).
//! 4. `link`: the best of `direct` > `bridged` > `none` between H and every
//!    query file of H's repo — LF.5c's exact test (`gaps::LinkIndex`: one
//!    edge, or a path of at most three edges through file-less nodes, over
//!    every category but CO_CHANGES / DEFINES / CONTAINS; a file that holds
//!    no node is `none`). The index is built once per answer, when the first
//!    CO_CHANGES edge touches a query MODULE (step 2 reads the consequent's
//!    file from it too), or before the ranking when only multi rules exist.
//!    `unlinked_only` keeps the `none` rows; then the list is cut to `top`.
//!
//! [`cochange_multi`] (CC.11b) adds, between steps 2 and 3, the rules whose
//! antecedent is SEVERAL query files: "commits that touched both `a.go` and
//! `a_test.go` also touched `migrations/`". Pairwise edges cannot give them,
//! so they are counted from the commits themselves: per repo of
//! `repo_roots` whose root holds a complete `.glia/history-snapshot/`
//! (`code_domain::snapshots::read_history`) and at least one query file —
//! one that exists under the root or that a commit of the snapshot touches:
//!
//! - the commit sets are the history stage's (`external::commit_file_sets`,
//!   the one rename fold: a path before a rename counts under the path it
//!   has today), keeping the commits of at most `MAX_COMMIT_FILES` (30) files
//!   — a mass reformat is no evidence — and every path in them, MODULE or
//!   not;
//! - the antecedents are Q, the repo's query files, when `|Q| >= 2`, plus
//!   every one-file-smaller subset `Q \ {x}` when `3 <= |Q| <= 8`
//!   (`MAX_LATTICE_QUERY`); a larger query uses Q only, so the lattice stays
//!   at 9 antecedents;
//! - for an antecedent A, `n_A` = the commits whose set holds all of A. When
//!   `n_A >= min_support`, every file H of those commits outside the query
//!   gives a rule A -> H: `support` = the commits holding A and H,
//!   `antecedent_commits` = `n_A`, `confidence_permille` = `1000 * support /
//!   n_A` — exact counts, with no pair floor. The same floors apply, and a
//!   file no longer under the root (deleted since) is no suggestion;
//! - H's MODULE is looked up as in step 1; a file with none (a yaml, a
//!   migration, a doc — often exactly the file worth suggesting) is kept with
//!   an empty `module_qname`. Its row's `source` is `multi`.
//!
//! The pairwise rules of step 2 are merged with them in step 3, one row per
//! consequent: on a (confidence, support) tie the larger antecedent wins.
//! [`cochange_vs_rev`] builds the working tree and takes its changed set
//! against a git rev (`git_rev::changed_files`: tracked changes, deletions
//! and both paths of a rename included, plus untracked files, `.glia/`
//! excluded) as the query of [`cochange_multi`].
//!
//! Every row is HEURISTIC: co-change is history, not proof of coupling. A row
//! with no static link carries the note "no static link joins them (a blind
//! spot, or coupling outside code)" — the case the suggestion is for.
//!
//! Absence (LD.8a, mechanism CO_CHANGES): no CO_CHANGES edge in the graph
//! (and, for [`cochange_multi`], no complete snapshot under any repo root) is
//! `no_history` (built without a `glia history sync` snapshot); otherwise an
//! empty answer is `no_match`, naming the floors (or saying no query file is
//! a MODULE, or that the working tree has no change against the rev).
//!
//! Only pairs LF.5b kept exist as edges (support >= 3 and >= 300 per mille
//! of the rarer file's commits, at most 5000 per repo), so a pairwise rule
//! whose consequent is the rarer file can be missing when its pair failed
//! LF.5b's floor; the multi rules, counted from the commits, have no such
//! floor.
//!
//! Read-only, deterministic: maps are BTreeMaps or lookup-only HashMaps, and
//! commit sets are BTreeSets. fired_on marker, once per call:
//! `[cochange-suggest] query_files=<q> unmapped=<u> candidates=<c> rows=<r> unlinked=<n> source=pairwise`
//! from [`cochange`], and from [`cochange_multi`] / [`cochange_vs_rev`]
//! `[cochange-suggest] query_files=<q> unmapped=<u> candidates=<c> rows=<r> unlinked=<n> source=multi antecedents=<a> commits_scanned=<s>`,
//! where `candidates` counts the rules of step 2 (and of the multi step)
//! before the floors, `unlinked` the returned rows whose link is `none`,
//! `antecedents` the multi-file antecedents tried over every repo and
//! `commits_scanned` the snapshot commits they were counted over.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use glia_code_domain::snapshots::read_history;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Node, NodeId};
use glia_graph::MergedGraph;

use crate::absence::{self, Absence};
use crate::external::{self, MAX_COMMIT_FILES, signals};
use crate::gaps::{HEURISTIC, Link, LinkIndex};
use crate::git_rev;

/// The primitive name in the `[absence]` marker.
const PRIMITIVE: &str = "cochange";

/// The edge categories the answer depends on.
const MECHANISMS: &[&str] = &["CO_CHANGES"];

/// [`CochangeRow::source`] of a single-file rule.
const PAIRWISE: &str = "pairwise";

/// [`CochangeRow::source`] of a rule whose antecedent is several files.
const MULTI: &str = "multi";

/// A repo's query of more files than this uses the whole set as its only
/// multi-file antecedent (no one-file-smaller subsets): the lattice stays at
/// `1 + MAX_LATTICE_QUERY` antecedents.
const MAX_LATTICE_QUERY: usize = 8;

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
    /// Its MODULE's qname; empty for a file no MODULE names (a multi rule's
    /// yaml, migration or doc).
    pub module_qname: String,
    /// The query file(s) the rule starts from, sorted; one for a pairwise
    /// rule.
    pub antecedent: Vec<String>,
    /// Commits that touched the antecedent and `file` together.
    pub support: u32,
    /// Commits that touched the antecedent: its module churn for a pairwise
    /// rule, the commits holding every antecedent file for a multi rule.
    ///
    /// DENOMINATOR NOTE (pairwise): churn counts EVERY commit of the snapshot
    /// window that touched the file, while `support` counts only commits of
    /// 2..=30 mapped files (the history stage's `MAX_COMMIT_FILES`: a mass
    /// reformat adds no pair), so `confidence_permille` is a lower bound when
    /// mass commits touched the antecedent. A multi rule counts both over the
    /// same commits (of at most 30 files), so its confidence is exact.
    pub antecedent_commits: u32,
    /// `1000 * support / antecedent_commits`: of the commits that changed the
    /// antecedent, the share that also changed `file` — directional.
    pub confidence_permille: u32,
    /// `direct` | `bridged` | `none`: the best static link between `file` and
    /// any query file of its repo (LF.5c's test; `none` for a file that holds
    /// no node).
    pub link: &'static str,
    /// `pairwise` (one antecedent file) | `multi` (several, CC.11b).
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
struct Rule {
    repo: u64,
    file: String,
    /// The consequent's MODULE; `None` for a multi rule's file no MODULE
    /// names.
    module: Option<NodeId>,
    /// Sorted query files.
    antecedent: Vec<String>,
    support: u32,
    antecedent_commits: u32,
    confidence_permille: u32,
}

/// The rules of one answer, before the ranking, and what the absence and the
/// marker need to know about how they were found.
struct Rules {
    unmapped: Vec<String>,
    /// Query files that name a MODULE.
    mapped: usize,
    /// The graph holds a CO_CHANGES edge.
    has_edges: bool,
    links: Option<LinkIndex>,
    candidates: usize,
    /// (repo, consequent file) -> its best rule.
    best: BTreeMap<(u64, String), Rule>,
}

/// What the multi step read, for the marker and the absence.
#[derive(Default)]
struct MultiCounts {
    /// Repos whose root held a complete history snapshot.
    snapshots: usize,
    /// Multi-file antecedents tried, over every repo.
    antecedents: usize,
    /// Snapshot commits (of at most `MAX_COMMIT_FILES` files) they were
    /// counted over.
    commits_scanned: usize,
}

/// **cochange** (CC.11a): the files that usually change with `files`, by
/// pairwise rule over the graph's CO_CHANGES edges (module docs). Read-only;
/// prints the `[cochange-suggest]` marker.
pub fn cochange(merged: &MergedGraph, files: &[String], args: &CochangeArgs) -> Cochange {
    let query_files = normalise(files);
    let rules = pairwise_rules(merged, &query_files, args);
    answer(merged, query_files, rules, args, None, None)
}

/// **cochange_multi** (CC.11b): [`cochange`] plus the multi-file rules
/// counted from the history snapshot of every repo of `repo_roots`
/// (`RepoId.0` -> root, a build's `GenerateResult::repo_roots`) that holds a
/// query file (module docs). Read-only; prints the `[cochange-suggest]`
/// marker with `source=multi`.
pub fn cochange_multi(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    files: &[String],
    args: &CochangeArgs,
) -> Cochange {
    let query_files = normalise(files);
    let mut rules = pairwise_rules(merged, &query_files, args);
    let counts = multi_rules(merged, repo_roots, &query_files, args, &mut rules);
    answer(merged, query_files, rules, args, Some(counts), None)
}

/// **cochange_vs_rev** (CC.11b): [`cochange_multi`] over a fresh build of the
/// working tree at `repo_path`, for its changed files against `base` (a rev
/// git resolves to a commit): tracked changes, deletions and both paths of a
/// rename, plus untracked files; nothing under `.glia/`. Nothing is written.
/// `Err` when `repo_path` is no directory, `base` does not resolve, git fails
/// or the build fails.
pub fn cochange_vs_rev(
    repo_path: &str,
    base: &str,
    args: &CochangeArgs,
) -> Result<Cochange, String> {
    let repo = Path::new(repo_path);
    if !repo.is_dir() {
        return Err(format!("not a directory: {repo_path}"));
    }
    let rev = git_rev::resolve_rev(repo, base)?;
    let changed = git_rev::changed_files(repo, &rev)?;
    let built = crate::generate_one(repo_path)?;
    let query_files = normalise(&changed);
    let mut rules = pairwise_rules(&built.merged, &query_files, args);
    let counts = multi_rules(
        &built.merged,
        &built.repo_roots,
        &query_files,
        args,
        &mut rules,
    );
    Ok(answer(
        &built.merged,
        query_files,
        rules,
        args,
        Some(counts),
        Some(&rev.given),
    ))
}

/// Steps 1 and 2 (module docs): the query's MODULEs and the pairwise rules
/// the CO_CHANGES edges give them.
fn pairwise_rules(merged: &MergedGraph, query_files: &[String], args: &CochangeArgs) -> Rules {
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
            let confidence_permille = permille(support, commits);
            if support < args.min_support || confidence_permille < args.min_confidence_permille {
                continue;
            }
            keep_best(
                &mut best,
                Rule {
                    repo,
                    file: file.to_string(),
                    module: Some(other),
                    antecedent: vec![ante.file.to_string()],
                    support,
                    antecedent_commits: commits,
                    confidence_permille,
                },
            );
        }
    }

    Rules {
        unmapped,
        mapped: mapped.len(),
        has_edges: !co_edges.is_empty(),
        links,
        candidates,
        best,
    }
}

/// The multi step (module docs): every multi-file rule of every repo of
/// `repo_roots` with a complete snapshot and a query file, merged into
/// `rules.best`.
fn multi_rules(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    query_files: &[String],
    args: &CochangeArgs,
    rules: &mut Rules,
) -> MultiCounts {
    let mut counts = MultiCounts::default();
    // (repo, file) -> MODULE, built on the first rule that passes the floors.
    let mut modules: Option<BTreeMap<(u64, String), NodeId>> = None;
    for (&repo, root) in repo_roots {
        let root = Path::new(root);
        let Some(snapshot) = read_history(root) else {
            continue;
        };
        counts.snapshots += 1;
        let sets = external::commit_file_sets(&snapshot);
        let touched: BTreeSet<&str> = sets.iter().flatten().map(String::as_str).collect();
        let q: Vec<&str> = query_files
            .iter()
            .map(String::as_str)
            .filter(|f| touched.contains(f) || root.join(f).is_file())
            .collect();
        if q.is_empty() {
            continue;
        }
        let scanned: Vec<&BTreeSet<String>> = sets
            .iter()
            .filter(|s| s.len() <= MAX_COMMIT_FILES)
            .collect();
        counts.commits_scanned += scanned.len();
        let lattice = antecedents(&q);
        counts.antecedents += lattice.len();
        let q_set: BTreeSet<&str> = q.iter().copied().collect();
        for a in &lattice {
            let holding: Vec<&BTreeSet<String>> = scanned
                .iter()
                .copied()
                .filter(|s| a.iter().all(|f| s.contains(*f)))
                .collect();
            // Consequent -> the commits holding A and it.
            let mut support: BTreeMap<&str, u32> = BTreeMap::new();
            for s in &holding {
                for h in s.iter().map(String::as_str).filter(|h| !q_set.contains(h)) {
                    *support.entry(h).or_insert(0) += 1;
                }
            }
            rules.candidates += support.len();
            let n_a = u32::try_from(holding.len()).unwrap_or(u32::MAX);
            if n_a == 0 || n_a < args.min_support {
                continue;
            }
            for (h, sup) in support {
                let confidence_permille = permille(sup, n_a);
                if sup < args.min_support || confidence_permille < args.min_confidence_permille {
                    continue;
                }
                // Deleted since: no file to suggest.
                if !root.join(h).is_file() {
                    continue;
                }
                let modules = modules.get_or_insert_with(|| module_ids(merged));
                keep_best(
                    &mut rules.best,
                    Rule {
                        repo,
                        file: h.to_string(),
                        module: modules.get(&(repo, h.to_string())).copied(),
                        antecedent: a.iter().map(|f| (*f).to_string()).collect(),
                        support: sup,
                        antecedent_commits: n_a,
                        confidence_permille,
                    },
                );
            }
        }
    }
    counts
}

/// The multi-file antecedents of a repo's sorted query files `q`: `q` itself
/// when it has at least two files, plus each one-file-smaller subset when it
/// has 3..=[`MAX_LATTICE_QUERY`]. Each antecedent stays sorted.
fn antecedents<'a>(q: &[&'a str]) -> Vec<Vec<&'a str>> {
    let mut out: Vec<Vec<&'a str>> = Vec::new();
    if q.len() < 2 {
        return out;
    }
    out.push(q.to_vec());
    if (3..=MAX_LATTICE_QUERY).contains(&q.len()) {
        for skip in 0..q.len() {
            out.push(
                q.iter()
                    .enumerate()
                    .filter(|&(i, _)| i != skip)
                    .map(|(_, f)| *f)
                    .collect(),
            );
        }
    }
    out
}

/// Steps 3 and 4 (module docs): rank the rules, link them, cut, and answer.
/// `multi` is the multi step's counts (`None` for [`cochange`]); `base` the
/// rev of [`cochange_vs_rev`].
fn answer(
    merged: &MergedGraph,
    query_files: Vec<String>,
    rules: Rules,
    args: &CochangeArgs,
    multi: Option<MultiCounts>,
    base: Option<&str>,
) -> Cochange {
    let Rules {
        unmapped,
        mapped,
        has_edges,
        mut links,
        candidates,
        best,
    } = rules;
    // A multi-only answer has not built the index yet.
    if !best.is_empty() && links.is_none() {
        links = Some(LinkIndex::build(merged));
    }

    let qnames = qnames_of(merged, best.values().filter_map(|r| r.module));
    let mut ranked: Vec<(Rule, String)> = best
        .into_values()
        .map(|r| {
            let q = r
                .module
                .and_then(|m| qnames.get(&m.0).cloned())
                .unwrap_or_default();
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
        let source = if r.antecedent.len() >= 2 {
            MULTI
        } else {
            PAIRWISE
        };
        rows.push(CochangeRow {
            module_qname,
            file: r.file,
            antecedent: r.antecedent,
            support: r.support,
            antecedent_commits: r.antecedent_commits,
            confidence_permille: r.confidence_permille,
            link: link_name(link),
            source,
            tier: HEURISTIC,
            note: (link == Link::None).then(|| UNLINKED_NOTE.to_string()),
        });
    }

    let unlinked = rows
        .iter()
        .filter(|r| r.link == link_name(Link::None))
        .count();
    let source = match &multi {
        Some(m) => format!(
            "{MULTI} antecedents={} commits_scanned={}",
            m.antecedents, m.commits_scanned
        ),
        None => PAIRWISE.to_string(),
    };
    eprintln!(
        "[cochange-suggest] query_files={} unmapped={} candidates={candidates} rows={} unlinked={unlinked} source={source}",
        query_files.len(),
        unmapped.len(),
        rows.len()
    );

    let absence = rows.is_empty().then(|| {
        let query = match base {
            Some(b) => format!("rev {b}"),
            None => query_files.join(", "),
        };
        let has_history = has_edges || multi.as_ref().is_some_and(|m| m.snapshots > 0);
        let (reason, note) = if !has_history {
            (
                "no_history",
                "no CO_CHANGES edge in this graph: it was built without a git-history \
                 snapshot (run `glia history sync <repo>`, then rebuild)"
                    .to_string(),
            )
        } else {
            (
                "no_match",
                no_match_note(&query_files, mapped, args, multi.as_ref(), base),
            )
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

/// `1000 * support / commits`, at most 1000; `commits` is non-zero.
fn permille(support: u32, commits: u32) -> u32 {
    let confidence = (1000 * u64::from(support) / u64::from(commits.max(1))).min(1000);
    u32::try_from(confidence).unwrap_or(1000)
}

/// Keep `rule` when it beats the rule `best` holds for its consequent.
fn keep_best(best: &mut BTreeMap<(u64, String), Rule>, rule: Rule) {
    match best.entry((rule.repo, rule.file.clone())) {
        Entry::Vacant(slot) => {
            slot.insert(rule);
        }
        Entry::Occupied(mut slot) => {
            if better(&rule, slot.get()) {
                slot.insert(rule);
            }
        }
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

/// Per (repo, file), the MODULE whose first POSITION names the file, the
/// smallest NodeId on a tie (the history stage's rule), for every file `keep`
/// accepts.
fn modules_by_file<'g>(
    merged: &'g MergedGraph,
    keep: impl Fn(&str) -> bool,
) -> BTreeMap<(u64, String), &'g Node> {
    let mut by_file: BTreeMap<(u64, String), &'g Node> = BTreeMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                continue;
            }
            let Some(file) = first_position_file(&n.cells) else {
                continue;
            };
            if !keep(&file) {
                continue;
            }
            by_file
                .entry((g.repo.0, file))
                .and_modify(|slot| {
                    if n.id.0 < slot.id.0 {
                        *slot = n;
                    }
                })
                .or_insert(n);
        }
    }
    by_file
}

/// Node id -> the query MODULE it is: per (repo, query file), the MODULE of
/// [`modules_by_file`], with its churn commits.
fn query_modules<'a>(
    merged: &MergedGraph,
    query: &BTreeSet<&'a str>,
) -> HashMap<u64, Antecedent<'a>> {
    modules_by_file(merged, |f| query.contains(f))
        .into_iter()
        .filter_map(|((repo, file), n)| {
            let &file = query.get(file.as_str())?;
            let commits = signals::module_churn(&n.cells).map(|c| c.commits);
            Some((
                n.id.0,
                Antecedent {
                    repo,
                    file,
                    commits,
                },
            ))
        })
        .collect()
}

/// (repo, file) -> the MODULE of [`modules_by_file`], for every file.
fn module_ids(merged: &MergedGraph) -> BTreeMap<(u64, String), NodeId> {
    modules_by_file(merged, |_| true)
        .into_iter()
        .map(|(k, n)| (k, n.id))
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

/// `rule` beats `slot`: (confidence, support) higher, then the larger
/// antecedent, then the smaller antecedent paths.
fn better(rule: &Rule, slot: &Rule) -> bool {
    rule.confidence_permille
        .cmp(&slot.confidence_permille)
        .then_with(|| rule.support.cmp(&slot.support))
        .then_with(|| rule.antecedent.len().cmp(&slot.antecedent.len()))
        .then_with(|| slot.antecedent.cmp(&rule.antecedent))
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

/// Why no row came back, when the graph or a snapshot does hold co-change
/// history.
fn no_match_note(
    query: &[String],
    mapped: usize,
    args: &CochangeArgs,
    multi: Option<&MultiCounts>,
    base: Option<&str>,
) -> String {
    if query.is_empty() {
        return match base {
            Some(b) => format!("the working tree has no change against {b}"),
            None => "no query file was given".to_string(),
        };
    }
    let mut note = match multi {
        Some(m) => format!(
            "no co-change rule from the {} query {} ({mapped} a MODULE; {} multi-file {} over {} {}) passes min_support={} min_confidence_permille={}",
            query.len(),
            absence::plural(query.len(), "file", "files"),
            m.antecedents,
            absence::plural(m.antecedents, "antecedent", "antecedents"),
            m.commits_scanned,
            absence::plural(m.commits_scanned, "commit", "commits"),
            args.min_support,
            args.min_confidence_permille
        ),
        None if mapped == 0 => {
            return format!(
                "none of the {} query {} is a MODULE in this graph",
                query.len(),
                absence::plural(query.len(), "file", "files")
            );
        }
        None => format!(
            "no co-change rule from the {mapped} mapped query {} passes min_support={} min_confidence_permille={}",
            absence::plural(mapped, "file", "files"),
            args.min_support,
            args.min_confidence_permille
        ),
    };
    if args.unlinked_only {
        note.push_str(" unlinked_only");
    }
    if args.top == 0 {
        note.push_str(" top=0");
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lattice_is_the_set_and_its_one_smaller_subsets_up_to_the_cap() {
        let files = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
        let sizes: Vec<usize> = (0..=files.len())
            .map(|n| antecedents(&files[..n]).len())
            .collect();
        // 0 and 1 file: no multi-file antecedent; 2: the pair; 3..=8: the set
        // and its n subsets; 9: the set only.
        assert_eq!(sizes, [0, 0, 1, 4, 5, 6, 7, 8, 9, 1]);
        assert_eq!(
            antecedents(&files[..3]),
            [
                vec!["a", "b", "c"],
                vec!["b", "c"],
                vec!["a", "c"],
                vec!["a", "b"]
            ]
        );
    }

    #[test]
    fn permille_is_capped_and_floors() {
        assert_eq!(
            (
                permille(4, 4),
                permille(6, 10),
                permille(1, 3),
                permille(5, 4)
            ),
            (1000, 600, 333, 1000)
        );
    }
}
