//! The git-history stage (LF.5b): a repo's `.glia/history-snapshot/` (LF.5a,
//! written by `glia history sync`, read back through
//! `code_domain::snapshots::read_history`) becomes churn ATTN on its MODULE
//! nodes, blame-recency ATTN on its symbols and CO_CHANGES edges between the
//! modules whose files change together. History is a FACT input: both halves
//! run with or without `--no-overlay`. A repo with no snapshot directory is a
//! no-op that prints nothing; an incomplete snapshot is `read_history`'s
//! `[history] snapshot incomplete` line (once per half) and a no-op.
//!
//! Files map to MODULE nodes by the `file` of their first POSITION cell
//! (exact repo-relative match; several MODULEs on one file bind the smallest
//! NodeId). Paths with no MODULE (yaml, lockfiles, docs, deleted files) are
//! counted `unmapped` and ignored, never guessed. Renames: commits are walked
//! newest to oldest with an alias map (old path -> current path); a file entry
//! with `from = old` sets `alias[old] = current(new)`, so every commit before
//! a rename is attributed to the path the file has at the snapshot's head.
//! The fold is [`current_paths`]; [`commit_file_sets`] lends it, one path set
//! per commit, to the co-change suggestions' multi-file rules (CC.11b).
//!
//! - [`history_edges`] (a `Post` pass, through `apply_external_edges`): per
//!   commit whose mapped files number 2..=[`MAX_COMMIT_FILES`] (a mass
//!   reformat or vendoring commit is noise), every unordered pair `(a, b)`,
//!   `a < b` by path, is counted. A pair with `count >= MIN_SUPPORT` and
//!   `1000 * count / min(commits_a, commits_b) >= MIN_RATIO_PERMILLE` is kept;
//!   kept pairs sort by (count desc, a, b) and are capped at [`MAX_PAIRS`].
//!   Each is ONE edge MODULE(a) -> MODULE(b), category CO_CHANGES, Weak, with
//!   an ATTN edge cell `{"cochanges":n,"ratio_permille":r,"window_commits":w}`
//!   and EVIDENCE emitter [`EMITTER`], file `.glia/history-snapshot/meta.json`
//!   (basis `file`). CO_CHANGES is HEURISTIC-tier: it is in no carry list
//!   (blast radius, liveness, trace) and weighs 0 in activation
//!   (`code_domain::profile::CODE_TABLES`).
//! - [`history_cells`] (through `apply_external_cells`): ATTN on every MODULE
//!   the snapshot touches,
//!   `{"source":"git","commits":n,"lines_added":a,"lines_deleted":d,"first":t,"last":t,"window_commits":w,"head":"<12 hex>"}`,
//!   and, per FUNCTION / METHOD / CLASS whose POSITION span `[s0, e0]`
//!   (0-based) meets a blame run of its file (1-based: `[s0 + 1, e0 + 1]`),
//!   `{"source":"git-blame","last":t,"span_changes":<distinct times>,"head":"<12 hex>"}`.
//!   Integers only: no float and no age relative to now (the consumer computes
//!   recency). A node that already carries ATTN is skipped and counted.
//!
//! Every payload is compact JSON in the field order above (`"commits":4`, no
//! spaces). Maps are BTreeMaps and pairs are sorted, so the output never
//! depends on HashMap order.
//!
//! Marker, once per repo with a complete snapshot, printed by the cells half
//! once both halves ran (the fired_on line):
//!   `[history] ingest repo=<label> head=<12 hex> commits=<n> modules=<m> unmapped=<u> attn=<a> blame_symbols=<b> cochange_pairs=<p> (support>=3 ratio>=300 max_files=30)`
//! where `modules` counts the MODULEs the snapshot touches, `attn` the module
//! ATTN cells written, `blame_symbols` the symbol ATTN cells written and
//! `cochange_pairs` the CO_CHANGES edges of the edge half. When a node
//! already carried ATTN, one more line follows:
//!   `[history] skipped <n> node(s) already carrying ATTN repo=<label>`.

use std::collections::{BTreeMap, BTreeSet};

use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::snapshots::{HISTORY_DIR, HistorySnapshot, META_FILE, read_history};
use glia_code_domain::walk_gating::CONTROL_DIR;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, NodeKindId, RepoId};
use glia_graph::MergedGraph;

use super::RepoInputs;

/// The EVIDENCE emitter of every CO_CHANGES edge: stage `history`.
pub(crate) const EMITTER: &str = "history:cochange";

/// A commit touching more mapped files than this adds no pair.
pub(crate) const MAX_COMMIT_FILES: usize = 30;

/// A pair must co-change in at least this many commits.
pub(crate) const MIN_SUPPORT: u32 = 3;

/// ... and in at least this share (per mille) of the commits of the file
/// that changes less often.
pub(crate) const MIN_RATIO_PERMILLE: u64 = 300;

/// At most this many CO_CHANGES edges per repo, strongest first.
pub(crate) const MAX_PAIRS: usize = 5000;

/// The symbol kinds that take blame ATTN.
const SYMBOL_KINDS: [NodeKindId; 3] = [node_kind::FUNCTION, node_kind::METHOD, node_kind::CLASS];

/// One module's churn over the snapshot's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Churn {
    commits: u32,
    added: u64,
    deleted: u64,
    first: i64,
    last: i64,
}

/// One kept co-change pair, `a < b` by path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pair {
    a: String,
    b: String,
    count: u32,
    ratio_permille: u64,
}

/// What one repo's snapshot says about its graph: the same for both halves,
/// computed from the snapshot and the repo's MODULE nodes only.
struct Ingest {
    snapshot: HistorySnapshot,
    /// The head sha's first 12 characters.
    head: String,
    /// Repo-relative file -> its MODULE.
    modules: BTreeMap<String, NodeId>,
    /// Current path -> churn, for every mapped path the snapshot touches.
    churn: BTreeMap<String, Churn>,
    /// Current paths the snapshot touches that no MODULE declares.
    unmapped: BTreeSet<String>,
    /// Kept pairs, sorted (count desc, a, b), capped at [`MAX_PAIRS`].
    pairs: Vec<Pair>,
}

impl Ingest {
    fn window(&self) -> usize {
        self.snapshot.commits.len()
    }
}

/// The `source` of a module churn ATTN ([`ModuleAttn`]).
pub(super) const SOURCE_GIT: &str = "git";

/// The `source` of a symbol blame ATTN ([`SymbolAttn`]).
pub(super) const SOURCE_BLAME: &str = "git-blame";

// The three payloads are `pub(super)` so `super::signals`, their one reader
// (CC.2), round-trips these bytes in its tests.

/// The module churn payload, in this field order.
#[derive(serde::Serialize)]
pub(super) struct ModuleAttn<'a> {
    pub(super) source: &'a str,
    pub(super) commits: u32,
    pub(super) lines_added: u64,
    pub(super) lines_deleted: u64,
    pub(super) first: i64,
    pub(super) last: i64,
    pub(super) window_commits: usize,
    pub(super) head: &'a str,
}

/// The symbol blame payload, in this field order.
#[derive(serde::Serialize)]
pub(super) struct SymbolAttn<'a> {
    pub(super) source: &'a str,
    pub(super) last: i64,
    pub(super) span_changes: usize,
    pub(super) head: &'a str,
}

/// The CO_CHANGES edge-cell payload, in this field order.
#[derive(serde::Serialize)]
pub(super) struct PairAttn {
    pub(super) cochanges: u32,
    pub(super) ratio_permille: u64,
    pub(super) window_commits: usize,
}

/// Compact JSON of a payload of integers and plain strings, which cannot
/// fail to serialise; `{}` would read as an empty payload rather than lie.
pub(super) fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| String::from("{}"))
}

fn attn(payload: String) -> Cell {
    Cell { kind: cell_type::ATTN, payload: CellPayload::Json(payload) }
}

/// A node's first POSITION cell: `(file, start_line, end_line)`, 0-based rows
/// (the first POSITION wins, as everywhere a node is located).
fn position(n: &Node) -> Option<(String, Option<i64>, Option<i64>)> {
    let c = n.cells.iter().find(|c| c.kind == cell_type::POSITION)?;
    let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
        return None;
    };
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let file = v.get("file")?.as_str().filter(|f| !f.is_empty())?.to_string();
    let line = |k: &str| v.get(k).and_then(serde_json::Value::as_i64);
    Some((file, line("start_line"), line("end_line")))
}

/// `repo`'s MODULE nodes by the file of their POSITION; several on one file
/// bind the smallest NodeId.
fn module_files(merged: &MergedGraph, repo: RepoId) -> BTreeMap<String, NodeId> {
    let mut out: BTreeMap<String, NodeId> = BTreeMap::new();
    for g in merged.graphs.iter().filter(|g| g.repo == repo) {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                continue;
            }
            let Some((file, _, _)) = position(n) else { continue };
            out.entry(file)
                .and_modify(|id| {
                    if n.id.0 < id.0 {
                        *id = n.id;
                    }
                })
                .or_insert(n.id);
        }
    }
    out
}

/// Every file entry of every commit of `snapshot` under the path it has at
/// the snapshot's head: `out[i][j]` is `commits[i].files[j]`'s current path.
/// The one rename fold (module docs): commits are walked newest to oldest and
/// a file entry with `from = old` sets `alias[old] = current(new)`, each entry
/// resolved before its own alias is recorded.
fn current_paths(snapshot: &HistorySnapshot) -> Vec<Vec<String>> {
    let mut alias: BTreeMap<String, String> = BTreeMap::new();
    snapshot
        .commits
        .iter()
        .map(|commit| {
            commit
                .files
                .iter()
                .map(|f| {
                    let current = alias.get(&f.p).cloned().unwrap_or_else(|| f.p.clone());
                    if let Some(old) = &f.from {
                        alias.insert(old.clone(), current.clone());
                    }
                    current
                })
                .collect()
        })
        .collect()
}

/// Each commit's files under their current paths (the rename fold of
/// [`current_paths`]), one set per commit, newest first. Every path is kept,
/// MODULE or not (a deleted file included): the consumer filters. Read by the
/// co-change suggestions' multi-file rules (`cochange::cochange_multi`,
/// CC.11b).
pub(crate) fn commit_file_sets(snapshot: &HistorySnapshot) -> Vec<BTreeSet<String>> {
    current_paths(snapshot).into_iter().map(|paths| paths.into_iter().collect()).collect()
}

/// Read `input`'s snapshot and fold it over the repo's modules. `None` when
/// the repo has no complete snapshot.
fn ingest(merged: &MergedGraph, input: &RepoInputs) -> Option<Ingest> {
    let snapshot = read_history(&input.root)?;
    let modules = module_files(merged, input.repo);
    let head = snapshot.meta.head.get(..12).unwrap_or(&snapshot.meta.head).to_string();

    let mut churn: BTreeMap<String, Churn> = BTreeMap::new();
    let mut unmapped: BTreeSet<String> = BTreeSet::new();
    let mut pair_counts: BTreeMap<(String, String), u32> = BTreeMap::new();

    for (commit, paths) in snapshot.commits.iter().zip(current_paths(&snapshot)) {
        let mut touched: BTreeSet<String> = BTreeSet::new();
        for (f, current) in commit.files.iter().zip(paths) {
            if !modules.contains_key(&current) {
                unmapped.insert(current);
                continue;
            }
            let c = churn.entry(current.clone()).or_insert(Churn {
                commits: 0,
                added: 0,
                deleted: 0,
                first: commit.t,
                last: commit.t,
            });
            c.added += u64::from(f.a.unwrap_or(0));
            c.deleted += u64::from(f.d.unwrap_or(0));
            touched.insert(current);
        }
        for path in &touched {
            if let Some(c) = churn.get_mut(path) {
                c.commits += 1;
                c.first = c.first.min(commit.t);
                c.last = c.last.max(commit.t);
            }
        }
        if (2..=MAX_COMMIT_FILES).contains(&touched.len()) {
            let files: Vec<&String> = touched.iter().collect();
            for (i, a) in files.iter().enumerate() {
                for b in files.iter().skip(i + 1) {
                    *pair_counts.entry(((*a).clone(), (*b).clone())).or_insert(0) += 1;
                }
            }
        }
    }

    let commits_of = |p: &str| churn.get(p).map_or(0, |c| c.commits);
    let mut pairs: Vec<Pair> = pair_counts
        .into_iter()
        .filter_map(|((a, b), count)| {
            let least = u64::from(commits_of(&a).min(commits_of(&b)));
            if count < MIN_SUPPORT || least == 0 {
                return None;
            }
            let ratio_permille = 1000 * u64::from(count) / least;
            (ratio_permille >= MIN_RATIO_PERMILLE).then_some(Pair { a, b, count, ratio_permille })
        })
        .collect();
    pairs.sort_by(|x, y| y.count.cmp(&x.count).then_with(|| x.a.cmp(&y.a)).then_with(|| x.b.cmp(&y.b)));
    pairs.truncate(MAX_PAIRS);

    Some(Ingest { snapshot, head, modules, churn, unmapped, pairs })
}

/// The CO_CHANGES half: push one edge per kept pair of `input`'s snapshot
/// onto `merged.cross_edges`. Returns the number of edges pushed.
pub(super) fn history_edges(merged: &mut MergedGraph, input: &RepoInputs) -> usize {
    let Some(ing) = ingest(merged, input) else {
        return 0;
    };
    let window_commits = ing.window();
    let mut evidence = Evidence::emitter(EMITTER);
    evidence.file = Some(format!("{CONTROL_DIR}/{HISTORY_DIR}/{META_FILE}"));
    evidence.basis = Basis::File;
    let mut pushed = 0usize;
    for p in &ing.pairs {
        let (Some(&from), Some(&to)) = (ing.modules.get(&p.a), ing.modules.get(&p.b)) else {
            continue;
        };
        let cell = PairAttn { cochanges: p.count, ratio_permille: p.ratio_permille, window_commits };
        merged.cross_edges.push(
            Edge::new(from, to, edge_category::CO_CHANGES, Confidence::Weak)
                .with_cell(attn(json(&cell)))
                .with_cell(evidence.to_cell()),
        );
        pushed += 1;
    }
    pushed
}

/// The ATTN half: churn on `input`'s modules, blame recency on its symbols,
/// then the stage marker. True when a cell was written.
pub(super) fn history_cells(merged: &mut MergedGraph, input: &RepoInputs) -> bool {
    let Some(ing) = ingest(merged, input) else {
        return false;
    };
    let window_commits = ing.window();

    // Node id -> the ATTN payload it takes; modules and symbols kept apart
    // for the marker's counts.
    let mut module_cells: BTreeMap<u64, String> = BTreeMap::new();
    for (path, c) in &ing.churn {
        let Some(id) = ing.modules.get(path) else { continue };
        let payload = ModuleAttn {
            source: SOURCE_GIT,
            commits: c.commits,
            lines_added: c.added,
            lines_deleted: c.deleted,
            first: c.first,
            last: c.last,
            window_commits,
            head: &ing.head,
        };
        module_cells.insert(id.0, json(&payload));
    }
    let symbol_cells = blame_cells(merged, input.repo, &ing);

    let mut attn_written = 0usize;
    let mut blame_written = 0usize;
    let mut skipped = 0usize;
    for g in merged.graphs.iter_mut().filter(|g| g.repo == input.repo) {
        for n in g.nodes.iter_mut() {
            let (payload, count) = match (module_cells.get(&n.id.0), symbol_cells.get(&n.id.0)) {
                (Some(p), _) => (p, &mut attn_written),
                (None, Some(p)) => (p, &mut blame_written),
                (None, None) => continue,
            };
            if n.cells.iter().any(|c| c.kind == cell_type::ATTN) {
                skipped += 1;
                continue;
            }
            n.cells.push(attn(payload.clone()));
            *count += 1;
        }
    }

    eprintln!(
        "[history] ingest repo={} head={} commits={} modules={} unmapped={} attn={attn_written} blame_symbols={blame_written} cochange_pairs={} (support>={MIN_SUPPORT} ratio>={MIN_RATIO_PERMILLE} max_files={MAX_COMMIT_FILES})",
        input.label,
        ing.head,
        window_commits,
        ing.churn.len(),
        ing.unmapped.len(),
        ing.pairs.len(),
    );
    if skipped > 0 {
        eprintln!("[history] skipped {skipped} node(s) already carrying ATTN repo={}", input.label);
    }
    attn_written + blame_written > 0
}

/// Node id -> blame ATTN payload, for every FUNCTION / METHOD / CLASS of
/// `repo` whose POSITION span meets a blame run of its file.
fn blame_cells(merged: &MergedGraph, repo: RepoId, ing: &Ingest) -> BTreeMap<u64, String> {
    let mut out = BTreeMap::new();
    if ing.snapshot.blame.is_empty() {
        return out;
    }
    let runs: BTreeMap<&str, &[[i64; 3]]> =
        ing.snapshot.blame.iter().map(|b| (b.p.as_str(), b.runs.as_slice())).collect();
    for g in merged.graphs.iter().filter(|g| g.repo == repo) {
        for n in &g.nodes {
            if !g.nav.kind_by_id.get(&n.id).is_some_and(|k| SYMBOL_KINDS.contains(k)) {
                continue;
            }
            let Some((file, Some(s0), end)) = position(n) else { continue };
            let Some(file_runs) = runs.get(file.as_str()) else { continue };
            // POSITION rows are 0-based, blame lines 1-based: convert once.
            let (lo, hi) = (s0 + 1, end.unwrap_or(s0).max(s0) + 1);
            let times: BTreeSet<i64> = file_runs
                .iter()
                .filter(|[start, stop, _]| *start <= hi && *stop >= lo)
                .map(|[_, _, t]| *t)
                .collect();
            let Some(&last) = times.last() else { continue };
            let payload = SymbolAttn { source: SOURCE_BLAME, last, span_changes: times.len(), head: &ing.head };
            out.insert(n.id.0, json(&payload));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use glia_code_domain::snapshots::{BlameFile, HistoryCommit, HistoryFile, HistoryMeta};

    use super::*;

    fn commit(sha: &str, t: i64, files: &[(&str, u32, Option<&str>)]) -> HistoryCommit {
        HistoryCommit {
            c: sha.to_string(),
            t,
            files: files
                .iter()
                .map(|(p, a, from)| HistoryFile {
                    p: p.to_string(),
                    a: Some(*a),
                    d: Some(0),
                    from: from.map(str::to_string),
                })
                .collect(),
        }
    }

    fn snapshot(commits: Vec<HistoryCommit>, blame: Vec<BlameFile>) -> HistorySnapshot {
        HistorySnapshot {
            meta: HistoryMeta::new("abcdef0123456789".into(), 2000, None, String::new()),
            commits,
            blame,
        }
    }

    /// [`ingest`] over `snap` (written to a temp dir named by `tag`) and one
    /// MODULE per file of `files`.
    fn fold(tag: &str, snap: HistorySnapshot, files: &[&str]) -> Ingest {
        let modules: BTreeMap<String, NodeId> =
            files.iter().enumerate().map(|(i, f)| (f.to_string(), NodeId(i as u64 + 1))).collect();
        let snapshot = snap;
        let dir = std::env::temp_dir().join(format!("glia_history_fold_{}_{tag}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        glia_code_domain::snapshots::write_history(&dir, snapshot.meta.clone(), &snapshot.commits, &snapshot.blame)
            .expect("write snapshot");
        let repo = RepoId::from_canonical("test://history-fold");
        let merged = module_graph(repo, &modules);
        let input = RepoInputs { repo, root: dir.clone(), label: "fold".into(), config: None };
        let ing = ingest(&merged, &input).expect("a complete snapshot");
        std::fs::remove_dir_all(&dir).ok();
        ing
    }

    /// One graph of MODULE nodes with the given ids, each located at its file.
    fn module_graph(repo: RepoId, modules: &BTreeMap<String, NodeId>) -> MergedGraph {
        let mut g = glia_graph::RepoGraph {
            repo,
            nodes: Vec::new(),
            edges: Vec::new(),
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: Default::default(),
        };
        for (file, id) in modules {
            g.nodes.push(Node {
                id: *id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![Cell {
                    kind: cell_type::POSITION,
                    payload: CellPayload::Json(format!(r#"{{"file":"{file}","start_line":0,"end_line":1}}"#)),
                }],
            });
            g.nav.kind_by_id.insert(*id, node_kind::MODULE);
        }
        MergedGraph::new(vec![g])
    }

    #[test]
    fn pairs_need_support_and_ratio() {
        // a+b together 3x and a alone 7x more: the ratio divides by the
        // rarer file (b, 3 commits), 1000*3/3 = 1000, kept. e+f once: below
        // support.
        let mut commits = Vec::new();
        for i in 0..3 {
            commits.push(commit(&format!("ab{i}"), 100 + i, &[("a.py", 1, None), ("b.py", 1, None)]));
        }
        for i in 0..7 {
            commits.push(commit(&format!("a{i}"), 200 + i, &[("a.py", 1, None)]));
        }
        commits.push(commit("ef", 300, &[("e.py", 1, None), ("f.py", 1, None)]));
        let ing = fold("support", snapshot(commits, vec![]), &["a.py", "b.py", "e.py", "f.py"]);
        assert_eq!(ing.pairs, [Pair { a: "a.py".into(), b: "b.py".into(), count: 3, ratio_permille: 1000 }]);
        assert_eq!(ing.churn["a.py"].commits, 10);
        assert_eq!(ing.head, "abcdef012345");
    }

    #[test]
    fn low_ratio_pair_is_dropped() {
        // x+y together 3x, but each changes 11x: 1000*3/11 = 272 < 300.
        let mut commits = Vec::new();
        for i in 0..3 {
            commits.push(commit(&format!("xy{i}"), i, &[("x.py", 1, None), ("y.py", 1, None)]));
        }
        for i in 0..8 {
            commits.push(commit(&format!("x{i}"), 10 + i, &[("x.py", 1, None)]));
            commits.push(commit(&format!("y{i}"), 30 + i, &[("y.py", 1, None)]));
        }
        let ing = fold("ratio", snapshot(commits, vec![]), &["x.py", "y.py"]);
        assert!(ing.pairs.is_empty(), "{:?}", ing.pairs);
        assert_eq!(ing.churn["x.py"].commits, 11);
    }

    #[test]
    fn unmapped_paths_are_counted_not_guessed() {
        let commits = vec![commit("c1", 1, &[("a.py", 1, None), ("Cargo.lock", 9, None), ("docs/x.md", 2, None)])];
        let ing = fold("unmapped", snapshot(commits, vec![]), &["a.py"]);
        assert_eq!(ing.unmapped.iter().map(String::as_str).collect::<Vec<_>>(), ["Cargo.lock", "docs/x.md"]);
        assert_eq!(ing.churn.len(), 1);
    }

    #[test]
    fn rename_chain_resolves_to_the_head_path() {
        // Newest first: y -> z, then x -> y, then x created.
        let commits = vec![
            commit("c3", 3, &[("z.py", 1, Some("y.py"))]),
            commit("c2", 2, &[("y.py", 1, Some("x.py"))]),
            commit("c1", 1, &[("x.py", 5, None)]),
        ];
        let ing = fold("rename", snapshot(commits, vec![]), &["z.py"]);
        let z = ing.churn["z.py"];
        assert_eq!((z.commits, z.added, z.first, z.last), (3, 7, 1, 3));
        assert!(ing.unmapped.is_empty(), "{:?}", ing.unmapped);
    }

    #[test]
    fn commit_file_sets_fold_renames_and_keep_every_path() {
        // Newest first: y -> z, then x -> y beside a doc, then x with a lockfile.
        let commits = vec![
            commit("c3", 3, &[("z.py", 1, Some("y.py"))]),
            commit("c2", 2, &[("y.py", 1, Some("x.py")), ("docs/x.md", 1, None)]),
            commit("c1", 1, &[("x.py", 5, None), ("Cargo.lock", 9, None)]),
        ];
        let sets = commit_file_sets(&snapshot(commits, vec![]));
        let names: Vec<Vec<&str>> = sets.iter().map(|s| s.iter().map(String::as_str).collect()).collect();
        assert_eq!(names, [vec!["z.py"], vec!["docs/x.md", "z.py"], vec!["Cargo.lock", "z.py"]]);
    }

    #[test]
    fn position_reads_the_first_cell() {
        let n = Node {
            id: NodeId(1),
            repo: RepoId::from_canonical("test://pos"),
            confidence: Confidence::Strong,
            cells: vec![
                Cell { kind: cell_type::POSITION, payload: CellPayload::Json(r#"{"file":"a.py","start_line":3,"end_line":5}"#.into()) },
                Cell { kind: cell_type::POSITION, payload: CellPayload::Json(r#"{"file":"b.py","start_line":0,"end_line":0}"#.into()) },
            ],
        };
        assert_eq!(position(&n), Some(("a.py".into(), Some(3), Some(5))));
    }
}
