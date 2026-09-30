//! Time-travel graph over N revs (CD.5c): build N first-parent revs (one
//! incremental build each on the shared parse cache, working-tree identity),
//! chain their deltas through LB.6's moves into intervals, write the sidecar,
//! and answer edge history and as-of views. The interval algebra is
//! `glia_activation::algo::timeline`. Public slot, reached by module path
//! (`glia_engine::timeline::<item>`).
//!
//! # Building ([`build_timeline`])
//!
//! The window is the last [`TimelineArgs::revs`] commits of `head`'s
//! first-parent chain, oldest first. Each one is built exactly as LE.1b's
//! delta builds its rev side (`delta::graph_delta_vs_rev`): materialised
//! read-only into a temp dir, built under the WORKING TREE's identity with one
//! in-memory [`ParseCache`] loaded once, so NodeIds line up across revs and a
//! file a rev shares with the one before is a cache hit. The cache is never
//! saved: the sidecar keeps the working tree's state, so the next incremental
//! build of the tree reparses nothing.
//!
//! Consecutive built revs are chained through LB.6's move detection
//! (`detect_moves_with`, with git's renames between the two commits as the
//! declared tier) into CD.5a's `TimelineBuilder`: an edge that survives a file
//! rename is ONE span keyed by its newest endpoints, and a node's span records
//! each id it had before. Each side is LE.1b's delta side (CC.8a's
//! `side_parts` + `side`): REGION nodes and their edges are left out, because
//! a materialised rev cannot reproduce the on-disk summaries of ignored and
//! vendored directories. The builder folds presence only, so a content change
//! (the `CONTENT_CELLS` `glia delta` compares) never opens or closes a span.
//!
//! A rev that fails to materialise or build is recorded in
//! [`TimelineBuilt::skipped`] with its reason and left out of the stored
//! window; the fold continues from the last built rev. Presence is observed
//! per built rev and gaps are never bridged: an item absent at one rev and
//! back at the next is two spans. Only two builds are alive at once.
//!
//! A node span carries its kind, qname, file and 1-based line as last seen:
//! from the last rev its (final) id was present at. The sidecar is written
//! through `glia_store::write_timeline` into the repo's default layout dir
//! (`<repo>/.glia/graph/timeline.gmap`, beside a self-ignoring `.gitignore`)
//! unless [`TimelineArgs::persist`] is false. Commit subjects are user text:
//! stored only through `glia_store::timeline_subject` (first line, the A13.7
//! redaction, capped), never interpreted.
//!
//! # Answering
//!
//! [`edge_history`] lists every edge span that ever touched a node (every id
//! it had, the move chain followed), [`as_of`] is the graph at one rev as a
//! [`GraphSource`], in the ids that rev's build had. Both read the sidecar
//! ([`load_timeline`]); neither rebuilds anything. A row's tier is `derived`:
//! each end was observed by a build at that rev, and `since_window_start`
//! says when an edge was already present at the first rev (its true start is
//! before the window: said, not guessed).
//!
//! fired_on markers: `[timeline] repo=<label> revs=<N> built=<B> skipped=<S>
//! nodes=<n> edges=<e> spans=<i> closed=<c> moves=<m> wrote=<path|none>` once
//! per build, `[timeline] history <qname> rows=<r>` and
//! `[timeline] as_of <sha> nodes=<n> edges=<e>` once per query.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use glia_activation::algo::GraphSource;
use glia_activation::algo::timeline::TimelineBuilder;
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Confidence, Edge, EdgeCategoryId, NodeId, NodeKindId};
use glia_graph::MergedGraph;
use glia_graph::identity::detect_moves_with;
use glia_store::{
    TIMELINE_FILE, TIMELINE_OPEN, TimelineEdge, TimelineNode, TimelineRev, TimelineStore, read_timeline,
    timeline_subject, write_timeline,
};

use crate::absence::{self, Absence, Answer};
use crate::answers::Locator;
use crate::build::{GenerateResult, generate_one_as};
use crate::cache::ParseCache;
use crate::delta::{self, SideParts};
use crate::find::{self, FindOptions};
use crate::git_rev::{self, Rev, RevInfo};
use crate::persist::{default_layout_dir, write_self_ignore};

/// Revs a window holds when the caller names none.
pub const DEFAULT_REVS: usize = 20;
/// Most revs one window may hold: each one is a build.
pub const MAX_REVS: usize = 200;

/// What [`build_timeline`] builds. Start from `default()` (20 revs of
/// `HEAD`, written unless `GLIA_NO_PERSIST=1`) and set fields.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct TimelineArgs {
    /// Commits in the window, `1..=`[`MAX_REVS`].
    pub revs: usize,
    /// The rev whose first-parent chain the window ends at.
    pub head: String,
    /// Write the sidecar into the repo's default layout dir.
    pub persist: bool,
}

impl Default for TimelineArgs {
    fn default() -> Self {
        TimelineArgs {
            revs: DEFAULT_REVS,
            head: "HEAD".to_string(),
            persist: std::env::var("GLIA_NO_PERSIST").as_deref() != Ok("1"),
        }
    }
}

/// One built rev of a window: its index in the stored window (a span's rev),
/// its full commit id, committer time (unix seconds) and subject as stored
/// (`glia_store::timeline_subject`).
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug, PartialEq, Eq)]
pub struct RevRef {
    pub index: u32,
    pub sha: String,
    pub time: i64,
    pub subject: String,
}

/// What [`build_timeline`] built.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct TimelineBuilt {
    /// The built revs, oldest first; `index` is the rev a span names.
    pub revs: Vec<RevRef>,
    /// `(sha, reason)` of every rev of the window that did not build.
    pub skipped: Vec<(String, String)>,
    /// Node spans.
    pub nodes: usize,
    /// Distinct edges `(from, to, category)` over every edge span.
    pub edges: usize,
    /// Edge spans (an edge that left and came back has two).
    pub edge_spans: usize,
    /// Edge spans that closed inside the window.
    pub closed: usize,
    /// Node moves chained (LB.6 pairs the deltas applied), summed.
    pub moves: usize,
    /// The sidecar's path, `None` when nothing was written.
    pub written: Option<String>,
}

/// One edge span touching the node an [`edge_history`] asked about.
/// `direction` is `out` (the node is the edge's `from`) or `in`; `other_*`,
/// `file` and `line` (1-based) describe the other end as last seen. `until`
/// is the first rev the edge is gone at, `None` while it is present at the
/// window's last rev.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct EdgeHistoryRow {
    pub category: &'static str,
    pub direction: &'static str,
    pub other_qname: String,
    pub other_kind: &'static str,
    pub since: RevRef,
    /// Present at the window's first rev: the edge's true start is earlier or
    /// unknown.
    pub since_window_start: bool,
    pub until: Option<RevRef>,
    pub file: Option<String>,
    pub line: Option<i64>,
    /// Always `derived`: each end was observed by a build at that rev.
    pub tier: &'static str,
}

/// The graph at one rev of a timeline, in the NodeIds that rev's build had
/// (a node that moved later carries its earlier id here). A
/// [`GraphSource`], so every `activation::algo` walk runs over it. The
/// timeline records no edge confidence: every edge here is `Strong`
/// (observed by that rev's build) and carries no cells.
pub struct AsOfView {
    pub rev: RevRef,
    nodes: Vec<NodeId>,
    /// The qname of each of `nodes`, as last seen in the window.
    qnames: Vec<String>,
    edges: Vec<Edge>,
}

/// The counts of an [`AsOfView`].
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct AsOfSummary {
    pub rev: RevRef,
    pub nodes: usize,
    pub edges: usize,
    /// Edges per category name.
    pub by_category: BTreeMap<&'static str, usize>,
}

impl AsOfView {
    /// Node and edge counts, and edges by category.
    pub fn summary(&self) -> AsOfSummary {
        let mut by_category: BTreeMap<&'static str, usize> = BTreeMap::new();
        for e in &self.edges {
            *by_category.entry(edge_category::name(e.category)).or_insert(0) += 1;
        }
        AsOfSummary { rev: self.rev.clone(), nodes: self.nodes.len(), edges: self.edges.len(), by_category }
    }

    /// The node whose last-seen qname is `q`, else the one node whose qname
    /// ends `::q`; `None` when nothing or several match.
    pub fn node_named(&self, q: &str) -> Option<NodeId> {
        let q = q.trim();
        if let Some(i) = self.qnames.iter().position(|n| n == q) {
            return self.nodes.get(i).copied();
        }
        let suffix = format!("::{q}");
        let mut hits = self.qnames.iter().enumerate().filter(|(_, n)| n.ends_with(&suffix));
        match (hits.next(), hits.next()) {
            (Some((i, _)), None) => self.nodes.get(i).copied(),
            _ => None,
        }
    }

    /// The last-seen qname of `id`, when it is a node of this view.
    pub fn qname_of(&self, id: NodeId) -> Option<&str> {
        self.nodes.iter().position(|n| *n == id).and_then(|i| self.qnames.get(i)).map(String::as_str)
    }
}

impl GraphSource for AsOfView {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

/// A node as one rev's build saw it.
struct NodeInfo {
    kind: u32,
    qname: String,
    file: Option<String>,
    /// 1-based; 0 = none.
    line: u32,
}

/// The last built rev: its build, its delta-side parts and its commit.
struct BuiltRev {
    result: GenerateResult,
    parts: SideParts,
    sha: String,
}

/// Build the timeline of the repo at `repo_path` over `args.revs` first-parent
/// revs of `args.head` (module docs), and write it unless `args.persist` is
/// false. `Err` when `repo_path` is not a directory, the window is outside
/// `1..=`[`MAX_REVS`], git is missing, `repo_path` is not in a git work tree,
/// `head` names no commit, no rev of the window builds, or the sidecar cannot
/// be written. Every temp checkout is removed before this returns.
pub fn build_timeline(repo_path: &str, args: &TimelineArgs) -> Result<TimelineBuilt, String> {
    let repo = Path::new(repo_path);
    if !repo.is_dir() {
        return Err(format!("not a directory: {repo_path}"));
    }
    if args.revs == 0 || args.revs > MAX_REVS {
        return Err(format!("a timeline window holds 1..={MAX_REVS} revs, not {}", args.revs));
    }
    let head = git_rev::resolve_rev(repo, &args.head)?;
    let log = git_rev::first_parent_log(repo, &head, args.revs)?;
    let mut cache = ParseCache::load(repo_path);
    let mut infos: HashMap<u64, NodeInfo> = HashMap::new();
    let mut built: Vec<&RevInfo> = Vec::with_capacity(log.len());
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut builder: Option<TimelineBuilder> = None;
    let mut prev: Option<BuiltRev> = None;
    let mut repo_meta: Option<(u64, String)> = None;
    for rev in &log {
        let cur = match build_rev(repo, repo_path, rev, &mut cache) {
            Ok(r) => r,
            Err(e) => {
                skipped.push((rev.sha.clone(), e));
                continue;
            }
        };
        let parts = delta::side_parts(&cur.merged);
        record_nodes(&cur.merged, parts.regions(), &mut infos);
        if let (Some(b), Some(p)) = (builder.as_mut(), prev.as_ref()) {
            let renames = git_rev::renames_between(repo, &p.sha, &rev.sha);
            let moves = detect_moves_with(&p.result.merged, &cur.merged, &renames);
            let pairs: Vec<(NodeId, NodeId)> = moves.nodes.iter().map(|m| (m.old_id, m.new_id)).collect();
            // `prev` is rebuilt from the build pushed last as `next`: the same
            // side, the rule TimelineBuilder::push trusts.
            b.push(&delta::side(&p.result.merged, &p.parts), &delta::side(&cur.merged, &parts), &pairs);
        } else {
            builder = Some(TimelineBuilder::new(&delta::side(&cur.merged, &parts)));
        }
        if repo_meta.is_none() {
            repo_meta = cur.repo_labels.iter().next().map(|(id, label)| (*id, label.clone()));
        }
        built.push(rev);
        prev = Some(BuiltRev { result: cur, parts, sha: rev.sha.clone() });
    }
    drop(prev);
    let Some(builder) = builder else {
        let why: Vec<String> = skipped.iter().map(|(sha, e)| format!("{}: {e}", short(sha))).collect();
        return Err(format!("no rev of the {} in the window built ({})", log.len(), why.join("; ")));
    };
    let tl = builder.finish();
    let (repo_id, label) = repo_meta.unwrap_or_else(|| (0, repo_path.to_string()));
    let store = to_store(repo_id, &built, &tl, &infos);

    let written = if args.persist {
        let dir = default_layout_dir(repo);
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        write_self_ignore(&dir).map_err(|e| format!("write {}/.gitignore: {e}", dir.display()))?;
        write_timeline(&dir, &store).map_err(|e| format!("write {}: {e}", dir.join(TIMELINE_FILE).display()))?;
        Some(dir.join(TIMELINE_FILE).to_string_lossy().into_owned())
    } else {
        None
    };
    let keys: BTreeSet<(u64, u64, u32)> = store.edges.iter().map(|e| (e.from, e.to, e.category)).collect();
    let out = TimelineBuilt {
        revs: (0..store.revs.len()).filter_map(|i| rev_ref(&store, i)).collect(),
        skipped,
        nodes: store.nodes.len(),
        edges: keys.len(),
        edge_spans: store.edges.len(),
        closed: store.edges.iter().filter(|e| e.until().is_some()).count(),
        moves: tl.moves_applied,
        written,
    };
    eprintln!(
        "[timeline] repo={label} revs={} built={} skipped={} nodes={} edges={} spans={} closed={} moves={} wrote={}",
        log.len(),
        out.revs.len(),
        out.skipped.len(),
        out.nodes,
        out.edges,
        out.edge_spans,
        out.closed,
        out.moves,
        out.written.as_deref().unwrap_or("none"),
    );
    Ok(out)
}

/// Materialise `rev` and build it under the working tree's identity on the
/// shared cache; the checkout is dropped before this returns.
fn build_rev(repo: &Path, repo_path: &str, rev: &RevInfo, cache: &mut ParseCache) -> Result<GenerateResult, String> {
    let r = Rev { given: rev.sha.clone(), sha: rev.sha.clone() };
    let tree = git_rev::materialize_rev(repo, &r)?;
    let tmp = tree
        .dir
        .path()
        .to_str()
        .ok_or_else(|| format!("temp dir is not valid UTF-8: {}", tree.dir.path().display()))?;
    generate_one_as(tmp, repo_path, Some(cache))
}

/// Record every non-REGION node of `m` as this rev saw it, over what an
/// earlier rev recorded for the same id.
fn record_nodes(m: &MergedGraph, regions: &BTreeSet<u64>, infos: &mut HashMap<u64, NodeInfo>) {
    let loc = Locator::new(m);
    let mut kinds: HashMap<NodeId, NodeKindId> = HashMap::new();
    for g in &m.graphs {
        for (id, k) in &g.nav.kind_by_id {
            kinds.entry(*id).or_insert(*k);
        }
    }
    let mut seen: HashSet<NodeId> = HashSet::new();
    for n in m.graphs.iter().flat_map(|g| g.nodes.iter()) {
        if regions.contains(&n.id.0) || !seen.insert(n.id) {
            continue;
        }
        let at = loc.locate(n.id);
        infos.insert(
            n.id.0,
            NodeInfo {
                kind: kinds.get(&n.id).map_or(0, |k| k.0),
                qname: at.qname,
                file: at.file,
                line: at.line.and_then(|l| u32::try_from(l).ok()).unwrap_or(0),
            },
        );
    }
}

/// Strings interned in first-use order.
#[derive(Default)]
struct Strings {
    list: Vec<String>,
    index: HashMap<String, u32>,
}

impl Strings {
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(ix) = self.index.get(s) {
            return *ix;
        }
        let ix = u32::try_from(self.list.len()).unwrap_or(u32::MAX);
        self.list.push(s.to_string());
        self.index.insert(s.to_string(), ix);
        ix
    }
}

/// The sidecar's store for a finished fold: the built revs, and the spans in
/// the builder's order with their strings interned in that order, so the
/// bytes are a pure function of the history.
fn to_store(
    repo: u64,
    built: &[&RevInfo],
    tl: &glia_activation::algo::timeline::Timeline,
    infos: &HashMap<u64, NodeInfo>,
) -> TimelineStore {
    let revs = built
        .iter()
        .map(|r| TimelineRev { sha: r.sha.clone(), time: r.time, subject: timeline_subject(&r.subject) })
        .collect();
    let mut strings = Strings::default();
    let nodes = tl
        .nodes
        .iter()
        .map(|s| {
            let info = infos.get(&s.id.0);
            let qname = match info {
                Some(i) => strings.intern(&i.qname),
                None => strings.intern(&format!("(unknown:{})", s.id.0)),
            };
            let file = info.and_then(|i| i.file.as_deref()).map_or(0, |f| strings.intern(f).saturating_add(1));
            TimelineNode {
                id: s.id.0,
                kind: info.map_or(0, |i| i.kind),
                qname,
                file,
                line: info.map_or(0, |i| i.line),
                from_rev: s.from_rev,
                until_rev: s.until_rev.unwrap_or(TIMELINE_OPEN),
                prior: s.prior_ids.iter().map(|(rev, id)| (*rev, id.0)).collect(),
            }
        })
        .collect();
    let edges = tl
        .edges
        .iter()
        .map(|s| TimelineEdge {
            from: s.key.from.0,
            to: s.key.to.0,
            category: s.key.category.0,
            from_rev: s.from_rev,
            until_rev: s.until_rev.unwrap_or(TIMELINE_OPEN),
        })
        .collect();
    TimelineStore { repo, revs, strings: strings.list, nodes, edges }
}

/// The first 7 chars of a commit id.
fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// Rev `i` of `store` as a [`RevRef`].
fn rev_ref(store: &TimelineStore, i: usize) -> Option<RevRef> {
    let r = store.revs.get(i)?;
    Some(RevRef { index: u32::try_from(i).ok()?, sha: r.sha.clone(), time: r.time, subject: r.subject.clone() })
}

/// The timeline sidecar of the repo at `repo_path` (its default layout dir).
/// `Err` naming `glia timeline build` when there is none, and the store's
/// reason when the file is old, future or damaged.
pub fn load_timeline(repo_path: &str) -> Result<TimelineStore, String> {
    let dir = default_layout_dir(Path::new(repo_path));
    match read_timeline(&dir) {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err(format!(
            "no timeline at {}: run `glia timeline build {repo_path}` first",
            dir.join(TIMELINE_FILE).display()
        )),
        Err(e) => Err(format!(
            "{}: {e} (run `glia timeline build {repo_path}` to rebuild it)",
            dir.join(TIMELINE_FILE).display()
        )),
    }
}

/// Where each id a node span ever had was that span's id: raw id ->
/// `(span index, first rev, first rev after)`.
struct IdIndex {
    by_id: HashMap<u64, Vec<(usize, u32, u32)>>,
}

impl IdIndex {
    fn new(store: &TimelineStore) -> Self {
        let mut by_id: HashMap<u64, Vec<(usize, u32, u32)>> = HashMap::new();
        for (i, n) in store.nodes.iter().enumerate() {
            let mut start = n.from_rev;
            for &(changed, before) in &n.prior {
                by_id.entry(before).or_default().push((i, start, changed));
                start = changed;
            }
            by_id.entry(n.id).or_default().push((i, start, n.until_rev));
        }
        IdIndex { by_id }
    }

    /// The node span whose id at `rev` is `id`.
    fn span_at(&self, id: u64, rev: u32) -> Option<usize> {
        self.by_id.get(&id)?.iter().find(|(_, from, until)| *from <= rev && rev < *until).map(|(i, _, _)| *i)
    }
}

/// The id node span `n` had at `rev`.
fn id_at(n: &TimelineNode, rev: u32) -> u64 {
    n.prior.iter().find(|(changed, _)| rev < *changed).map_or(n.id, |(_, before)| *before)
}

/// The last rev edge span `e` is present at.
fn last_rev(e: &TimelineEdge, revs: usize) -> u32 {
    match e.until() {
        Some(u) => u.saturating_sub(1),
        None => u32::try_from(revs).unwrap_or(u32::MAX).saturating_sub(1),
    }
}

/// The node spans `q` names: those whose qname is `q`, else those of the one
/// qname ending `::q`. `Err` lists the several qnames a suffix matched
/// (empty when nothing matched).
fn resolve_spans(store: &TimelineStore, q: &str) -> Result<BTreeSet<usize>, Vec<String>> {
    if q.is_empty() {
        return Err(Vec::new());
    }
    let with = |name: &str| -> BTreeSet<usize> {
        store.nodes.iter().enumerate().filter(|(_, n)| store.node_qname(n) == Some(name)).map(|(i, _)| i).collect()
    };
    let exact = with(q);
    if !exact.is_empty() {
        return Ok(exact);
    }
    let suffix = format!("::{q}");
    let names: BTreeSet<&str> =
        store.nodes.iter().filter_map(|n| store.node_qname(n)).filter(|n| n.ends_with(&suffix)).collect();
    match names.len() {
        1 => Ok(names.into_iter().next().map(with).unwrap_or_default()),
        _ => Err(names.into_iter().map(String::from).collect()),
    }
}

/// The edge category a history filter names (case-insensitive), `Err` with
/// the name when it names none.
fn category_named(name: &str) -> Result<EdgeCategoryId, String> {
    let n = name.trim();
    edge_category::ALL
        .iter()
        .find(|(_, c)| c.eq_ignore_ascii_case(n))
        .map(|(id, _)| *id)
        .ok_or_else(|| n.to_string())
}

/// Every edge span of `store` that touched the node `qname` names, in any id
/// it had over the window (module docs), optionally of one `category`
/// (case-insensitive). The node resolves among the timeline's node spans by
/// exact qname, else the unique qname ending `::<qname>`. `merged` is the
/// CURRENT graph (the caller's `load_or_rebuild`): an unknown or ambiguous
/// name is an `unknown_symbol` absence with find's nearest qnames in it as the
/// suggestions, and a node with no such edge a `no_edges` absence with its
/// coverage caveats. Rows are sorted by `(since, category, other_qname)`.
pub fn edge_history(
    merged: &MergedGraph,
    store: &TimelineStore,
    qname: &str,
    category: Option<&str>,
) -> Answer<EdgeHistoryRow> {
    let q = qname.trim();
    let asked = category.map(category_named);
    let answer = match resolve_spans(store, q) {
        Ok(spans) => history_rows(merged, store, q, &spans, asked.as_ref()),
        Err(candidates) => Answer::from_results(Vec::new(), || unknown_node(merged, store, q, &candidates, asked.as_ref())),
    };
    eprintln!("[timeline] history {q} rows={}", answer.results.len());
    answer
}

/// The rows of a resolved node (the spans `spans`), or its `no_edges`
/// absence.
fn history_rows(
    merged: &MergedGraph,
    store: &TimelineStore,
    q: &str,
    spans: &BTreeSet<usize>,
    asked: Option<&Result<EdgeCategoryId, String>>,
) -> Answer<EdgeHistoryRow> {
    let idx = IdIndex::new(store);
    // Fallback for an endpoint no span holds at the edge's last rev.
    let ids: HashSet<u64> = spans
        .iter()
        .filter_map(|&i| store.nodes.get(i))
        .flat_map(|n| std::iter::once(n.id).chain(n.prior.iter().map(|(_, id)| *id)))
        .collect();
    let ours = |id: u64, at: Option<usize>| at.map_or_else(|| ids.contains(&id), |s| spans.contains(&s));
    let mut rows: Vec<EdgeHistoryRow> = Vec::new();
    for e in &store.edges {
        match asked {
            Some(Ok(c)) if e.category != c.0 => continue,
            Some(Err(_)) => break,
            _ => {}
        }
        let t = last_rev(e, store.revs.len());
        let (from_span, to_span) = (idx.span_at(e.from, t), idx.span_at(e.to, t));
        let (direction, other_id, other_span) = if ours(e.from, from_span) {
            ("out", e.to, to_span)
        } else if ours(e.to, to_span) {
            ("in", e.from, from_span)
        } else {
            continue;
        };
        let Some(since) = rev_ref(store, e.from_rev as usize) else {
            continue;
        };
        let other = other_span.and_then(|s| store.nodes.get(s));
        rows.push(EdgeHistoryRow {
            category: edge_category::name(EdgeCategoryId(e.category)),
            direction,
            other_qname: other
                .and_then(|n| store.node_qname(n))
                .map_or_else(|| format!("(unknown:{other_id})"), String::from),
            other_kind: other.map_or("UNKNOWN", |n| node_kind::name(NodeKindId(n.kind))),
            since,
            since_window_start: e.from_rev == 0,
            until: e.until().and_then(|u| rev_ref(store, u as usize)),
            file: other.and_then(|n| store.node_file(n)).map(String::from),
            line: other.and_then(TimelineNode::line).map(i64::from),
            tier: "derived",
        });
    }
    rows.sort_by(|a, b| {
        let until = |r: &EdgeHistoryRow| r.until.as_ref().map_or(u32::MAX, |u| u.index);
        (a.since.index, a.category, &a.other_qname, a.direction, until(a))
            .cmp(&(b.since.index, b.category, &b.other_qname, b.direction, until(b)))
    });
    Answer::from_results(rows, || {
        let node = spans.iter().next().and_then(|&i| store.nodes.get(i));
        let name = node.and_then(|n| store.node_qname(n)).unwrap_or(q);
        let (mechanisms, note): (Vec<&'static str>, String) = match asked {
            Some(Err(c)) => (Vec::new(), format!("`{c}` is not an edge category")),
            Some(Ok(c)) => {
                let cat = edge_category::name(*c);
                (vec![cat], format!("no {cat} edge touches `{name}` in {}", window(store)))
            }
            None => {
                let kind = NodeKindId(node.map_or(0, |n| n.kind));
                (absence::mechanisms_for_kind(kind).to_vec(), format!("no edge touches `{name}` in {}", window(store)))
            }
        };
        absence::empty(merged, "timeline", q, "no_edges", note, &mechanisms, node.and_then(|n| store.node_file(n)))
    })
}

/// "the timeline's N revs (<first>..<last>)".
fn window(store: &TimelineStore) -> String {
    match (store.revs.first(), store.revs.last()) {
        (Some(a), Some(b)) => format!("the timeline's {} revs ({}..{})", store.revs.len(), short(&a.sha), short(&b.sha)),
        _ => "the timeline (no revs)".to_string(),
    }
}

/// The `unknown_symbol` absence of a name no node span (or several) holds.
fn unknown_node(
    merged: &MergedGraph,
    store: &TimelineStore,
    q: &str,
    candidates: &[String],
    asked: Option<&Result<EdgeCategoryId, String>>,
) -> Absence {
    let mut mechanisms: Vec<&'static str> = vec!["CALLS"];
    if let Some(Ok(c)) = asked {
        let cat = edge_category::name(*c);
        if cat != "CALLS" {
            mechanisms.push(cat);
        }
    }
    let opts = FindOptions { top_k: absence::SUGGESTIONS, ..FindOptions::default() };
    let near = find::find_nodes(merged, q, &opts).results;
    let mut a = absence::unknown_symbol(merged, "timeline", q, &mechanisms, &near);
    a.note = if candidates.is_empty() {
        format!("no node of {} has the qname `{q}` or a qname ending `::{q}`", window(store))
    } else {
        let shown: Vec<&str> = candidates.iter().take(5).map(String::as_str).collect();
        format!(
            "`{q}` matches {} qnames in {} ({}{}): give the full qname",
            candidates.len(),
            window(store),
            shown.join(", "),
            if candidates.len() > shown.len() { ", ..." } else { "" }
        )
    };
    a
}

/// The index `rev` names in `store`: a commit id prefix of at least 7 hex
/// chars (unique among the window's revs), else a rev index.
fn rev_index(store: &TimelineStore, rev: &str) -> Result<u32, String> {
    let t = rev.trim();
    let n = store.revs.len();
    if n == 0 {
        return Err("the timeline holds no revs".to_string());
    }
    if t.len() >= 7 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
        let lower = t.to_ascii_lowercase();
        let hits: Vec<usize> = store.revs.iter().enumerate().filter(|(_, r)| r.sha.starts_with(&lower)).map(|(i, _)| i).collect();
        match hits.as_slice() {
            [i] => return u32::try_from(*i).map_err(|e| e.to_string()),
            [] => {}
            _ => return Err(format!("`{t}` names {} revs of the timeline: give more of the commit id", hits.len())),
        }
    } else if let Ok(i) = t.parse::<usize>()
        && i < n
    {
        return u32::try_from(i).map_err(|e| e.to_string());
    }
    Err(format!(
        "no rev `{t}` in {}: give an index 0..={} or a commit id prefix of at least 7 hex chars",
        window(store),
        n - 1
    ))
}

/// The graph at `rev` of `store` (module docs): a rev index or a commit id
/// prefix of at least 7 hex chars. `Err` when it names no rev of the window,
/// or several.
pub fn as_of(store: &TimelineStore, rev: &str) -> Result<AsOfView, String> {
    let r = rev_index(store, rev)?;
    let rref = rev_ref(store, r as usize).ok_or_else(|| format!("no rev {r} in the timeline"))?;
    let idx = IdIndex::new(store);
    let mut nodes: Vec<NodeId> = Vec::new();
    let mut qnames: Vec<String> = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();
    for n in store.nodes.iter().filter(|n| n.covers(r)) {
        let id = id_at(n, r);
        if seen.insert(id) {
            nodes.push(NodeId(id));
            qnames.push(store.node_qname(n).unwrap_or_default().to_string());
        }
    }
    // An edge is keyed by its ends as last seen: take each end back to the id
    // its node had at `r`.
    let end = |id: u64, t: u32| -> u64 {
        idx.span_at(id, t).and_then(|s| store.nodes.get(s)).filter(|n| n.covers(r)).map_or(id, |n| id_at(n, r))
    };
    let edges: Vec<Edge> = store
        .edges
        .iter()
        .filter(|e| e.covers(r))
        .map(|e| {
            let t = last_rev(e, store.revs.len());
            Edge::new(NodeId(end(e.from, t)), NodeId(end(e.to, t)), EdgeCategoryId(e.category), Confidence::Strong)
        })
        .collect();
    eprintln!("[timeline] as_of {} nodes={} edges={}", rref.sha, nodes.len(), edges.len());
    Ok(AsOfView { rev: rref, nodes, qnames, edges })
}
