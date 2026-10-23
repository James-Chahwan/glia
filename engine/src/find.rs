//! Ranked fuzzy find (LD.3b): tiered, explainable matching over name / qname
//! with a degree tie-break, located. The on-ramp the other answers resolve
//! their seeds through (LD.8a suggestions, LD.4a feature resolution, LD.2's
//! pyo3 `find`), so it lives in the engine and nowhere else.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `glia_engine::find::<item>`, never flattened into the
//! crate root.
//!
//! # Match tiers
//!
//! The first tier that applies to a node wins, and its NAME is what the record
//! reports in `match` — there is no numeric score to over-read:
//!
//! | tier | a node matches when |
//! |---|---|
//! | `exact_qname` | its qname equals the query |
//! | `exact_name` | its simple name equals the query |
//! | `exact_ci` | name or qname equals the query, case-insensitively |
//! | `qname_suffix` | its qname ends with `::` + the query (`UserService::get_user`) |
//! | `name_prefix` | its name starts with the query |
//! | `name_word` | the query starts at a word boundary inside its name: after `_` `-` `.` `:` `/` or a space, or at a lower→Upper camel step |
//! | `name_substring` | its name contains the query |
//! | `qname_substring` | only its qname contains the query (e.g. through the module path) |
//! | `subsequence` | every query char appears in order in its name (queries of 3+ chars) |
//!
//! Every tier below `exact_name` compares case-folded text. The qname tiers
//! also try the query with each `.` and `/` read as `::`, so a Python dotted
//! path or a file-ish path finds the qname.
//!
//! # Order
//!
//! Tier first. Inside `exact_qname` and `exact_name`: EXACTLY the key of
//! `MergedGraph::pick_primary` (a declaration before a container, then total
//! degree desc, then NodeId asc), so `find(q)`'s top row is the node
//! `node_id_by_qname` / `resolve_name` return whenever the match is exact —
//! the single-node resolution blast / trace seed on. Every other tier: total
//! degree desc, qname length asc, qname asc, NodeId asc. Each key ends on the
//! NodeId, so the order is total and no `HashMap` iteration order reaches it.
//!
//! # Identity classes (DD.9)
//!
//! The public answer ([`find_nodes`]) folds each FACT / DERIVED identity
//! class (`crate::identity`, Q17) to ONE row: the best-ranked matching member,
//! which carries [`FoundNode::identity`] (`tier`, `rules`, `diverged` and
//! `also_in`, every other member of the class, matched or not). The fold runs
//! on the ranked candidates BEFORE the `top_k` cut, so a class never spends
//! two slots, and keeps the ranked order. A node in no class keeps `identity`
//! `None`, which serialises to nothing: its row is byte-identical to the
//! uncollapsed one. HEURISTIC candidates (vendored copies) never fold.
//! [`FindOptions::collapse`] `Collapse::off()` is the raw per-member answer
//! (`glia find --no-collapse`). The seed resolvers (trace, tests-for,
//! governing docs, absence suggestions) call [`search`], which never folds.
//!
//! # Files for a free-text query (EB.8)
//!
//! [`find_files`] (`glia find --files`, `PyGraph.find_files`) is a separate
//! answer over FILES, not nodes: the files a free-text query (a task title,
//! a ticket line) is about, in one repo, ranked [`FIND_FILES_FUSION`] =
//! `LX[IDR,T1s]`, round 3's find table: typed id links first (the query's ids
//! routed through the working tree's task / requirement rows and trackers,
//! `intent::idr`), then the query's BM25 over past commit messages
//! (`intent::text`, from `glia history sync`'s snapshot), then the file's
//! degree, then path (rank_eval's find-shape tie rule). The node answer
//! above is untouched by it.
//!
//! EC.13 (item 11): that order is the uncalibrated one. With a calibration
//! record (`glia history calibrate`, group `find`) the files follow set
//! `SELf2t`'s pick for the repo (`calibrate::FIND_CANDIDATES`: the find
//! fusions of text, PPR from the text's top files, co-change from them, the
//! work signals and IDR, rank_eval `r3_bank_find`), ranked through the same
//! `calibrate::fusions::find_keys` the replay measured; the answer carries
//! its `selection`. `FindFilesOptions::select = false` keeps LX[IDR,T1s].

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use glia_code_domain::node_kind;
use glia_code_domain::snapshots::{HistoryMessage, HistorySnapshot, read_history, read_history_messages};
use glia_core::{NodeId, NodeKindId, RepoId};
use glia_graph::MergedGraph;

use crate::absence::{self, Answer};
use crate::calibrate::fusions::{FindHead, find_keys, find_order, find_signals_at_head};
use crate::calibrate::record::{Shape, explicit, log_selection, select};
use crate::calibrate::sets::{FIND_CANDIDATES, FIND_FALLBACK, FIND_SET};
use crate::answers::{
    Locator, entrypoint_reachable, in_scope, live_marker, project_roots, resolve_scope,
};
use crate::arch::{repo_label_for, repo_label_map};
use crate::identity::{Collapse, IdentityView, RowIdentity};
use crate::intent::defs::{Tree, read_defs};
use crate::intent::idr::{
    IdrInputs, RoutedId, ids_by_file, recent_scopes, recent_subjects, route,
};
use crate::intent::paths::{PathResolver, rename_pairs};
use crate::intent::text::{Candidates, TextAbsent, TextIndex, subject};
use crate::rank::RankCache;
use crate::review::IntentHistory;

/// `FindOptions::default().top_k`.
pub const DEFAULT_TOP_K: usize = 20;

/// One ranked hit: identity, `live`, 1-based location (LD.1's [`Locator`])
/// and the tier that matched it. `match` is a tier name from the module doc.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FoundNode {
    pub id: u64,
    pub qname: String,
    pub name: String,
    pub kind: &'static str,
    /// Reachable from an entrypoint (LD.6, the flag `BlastAnswer::live`
    /// carries): `false` = likely dead.
    pub live: bool,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// The tier that matched, e.g. `"exact_name"` or `"name_word"`.
    pub r#match: &'static str,
    /// Set when this row stands for a FACT / DERIVED identity class (DD.9):
    /// the class's tier and rules, whether its members' contents differ, and
    /// every other member (`also_in`). Absent from the JSON when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<RowIdentity>,
}

/// What [`find_nodes`] returns and from where. Start from `default()` and set
/// fields: `#[non_exhaustive]` rules out a struct literal outside this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FindOptions {
    /// Keep the first `top_k` rows; `0` keeps every match.
    pub top_k: usize,
    /// Keep only nodes of these kinds. Applied before ranking.
    pub kinds: Option<Vec<NodeKindId>>,
    /// A repo-relative path or a project label (resolved once with
    /// `resolve_scope`), applied before `top_k` with the A8.3 rules: a
    /// `/`-boundary prefix match on the located file, and a node with no
    /// locatable file is KEPT.
    pub scope: Option<String>,
    /// Identity classes (DD.9): enabled by default, each FACT / DERIVED class
    /// is one row with [`FoundNode::identity`], folded before `top_k`.
    /// `Collapse::off()` keeps one row per member. Set the build's repo
    /// labels (`Collapse::with_labels`) so `also_in` names repos by label.
    pub collapse: Collapse,
}

impl Default for FindOptions {
    fn default() -> Self {
        FindOptions {
            top_k: DEFAULT_TOP_K,
            kinds: None,
            scope: None,
            collapse: Collapse::default(),
        }
    }
}

/// The ranked, located nodes `query` names — see the module doc for the tiers
/// and the order — as an LD.8a [`Answer`]: when nothing matches, `absence` is
/// `no_match`, and its note says whether no tier matched at all or `scope`
/// removed every match. It carries no suggestions (the subsequence tier
/// already ran) and no mechanisms (a name search follows no edge). An empty
/// (or all-whitespace) query matches nothing.
///
/// Cost: one O(V) pass of string checks, one O(V) [`Locator`] build, and one
/// O(E) degree pass, run only when more than one candidate survives the
/// filters, plus one O(V + E) liveness walk (`entrypoint_reachable`) for the
/// rows' `live` flags (LD.6); [`find_nodes_with_live`] takes the live set
/// instead. With `opts.collapse` on, one scan of the cross edges for SAME_AS
/// (the identity view, DD.9); only a graph with a collapse class pays more.
/// Prints one `[find] query=...` line (the candidates before the fold) and one
/// `[live] annotate surface=find` line per call, one `[identity] collapse
/// surface=find` line when the graph has a collapse class, and one
/// `[absence] primitive=find` line when the answer is empty.
pub fn find_nodes(merged: &MergedGraph, query: &str, opts: &FindOptions) -> Answer<FoundNode> {
    find_nodes_with_live(merged, &entrypoint_reachable(merged), query, opts)
}

/// [`find_nodes`] over a live set the caller already holds (pyo3's `PyGraph`
/// computes it once per graph): `live` must be `entrypoint_reachable` of
/// `merged`.
pub fn find_nodes_with_live(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    query: &str,
    opts: &FindOptions,
) -> Answer<FoundNode> {
    let view = IdentityView::new(merged, &opts.collapse);
    let mut found = search_with(merged, query, opts, Some(&view));
    // `folded` counts every candidate the fold dropped (also those below the
    // cut), `rows` is the answer's row count.
    view.collapse_marker("find", found.rows.len() + found.folded, found.rows.len());
    for r in &mut found.rows {
        r.live = live.contains(&NodeId(r.id));
    }
    live_marker("find", found.rows.len(), found.rows.iter().filter(|r| r.live).count());
    Answer::from_results(found.rows, || {
        if found.out_of_scope > 0
            && let Some(scope) = opts.scope.as_deref()
        {
            return absence::scope_emptied(merged, "find", query, found.out_of_scope, scope);
        }
        let q = query.trim();
        let what = match &opts.kinds {
            Some(kinds) => {
                let names: Vec<&str> = kinds.iter().map(|k| node_kind::name(*k)).collect();
                format!("no {} node", names.join(" / "))
            }
            None => "no node".to_string(),
        };
        let note = if q.is_empty() {
            "the query is empty, so it matches nothing".to_string()
        } else {
            format!("{what} matches `{q}` by name or qname in any find tier")
        };
        absence::empty(merged, "find", query, "no_match", note, &[], None)
    })
}

/// [`find_nodes`]' rows, plus how many matches `opts.scope` removed.
pub(crate) struct Found {
    pub(crate) rows: Vec<FoundNode>,
    /// Matches `opts.scope` filtered out (0 without a scope).
    pub(crate) out_of_scope: usize,
    /// Ranked candidates the identity fold dropped (0 without a view).
    pub(crate) folded: usize,
}

/// The search itself, without the envelope: what an answer that resolves its
/// seed through find (`governing_docs`) calls, so the rows it did not take
/// become its suggestions without a second pass. Every row's `live` is
/// `false` here: [`find_nodes_with_live`] sets it, and a caller that only
/// reads qnames (suggestions, the exact-match seed) pays no liveness walk.
/// It never folds identity classes: a seed resolver keeps every member.
pub(crate) fn search(merged: &MergedGraph, query: &str, opts: &FindOptions) -> Found {
    search_with(merged, query, opts, None)
}

/// [`search`], then, with a non-empty `view`, the identity fold on the ranked
/// candidates before the `top_k` cut (module doc): the first candidate of a
/// collapse class is its row, with `identity = view.row_identity(id, id)`,
/// and the class's later candidates are dropped. `opts.collapse` is not read
/// here: the caller built `view` from it.
pub(crate) fn search_with(
    merged: &MergedGraph,
    query: &str,
    opts: &FindOptions,
    view: Option<&IdentityView>,
) -> Found {
    let q = Query::new(query);
    if q.raw.is_empty() {
        log_marker(&q.raw, &[], opts.top_k);
        return Found {
            rows: Vec::new(),
            out_of_scope: 0,
            folded: 0,
        };
    }

    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut cands: Vec<Candidate> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            // First graph (in Vec order) to hold the id speaks for it — the
            // same graph `Locator` and `pick_primary` read the kind from.
            if !seen.insert(n.id) {
                continue;
            }
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            if let Some(kinds) = &opts.kinds
                && !kind.is_some_and(|k| kinds.contains(&k))
            {
                continue;
            }
            let name = g.nav.name_by_id.get(&n.id).map(String::as_str).unwrap_or("");
            let qname = g.nav.qname_by_id.get(&n.id).map(String::as_str).unwrap_or("");
            let Some(tier) = q.tier_of(name, qname) else {
                continue;
            };
            cands.push(Candidate {
                id: n.id,
                tier,
                declaration: kind.is_some_and(is_declaration_kind),
                qname_len: qname.chars().count(),
                qname: qname.to_string(),
                identity: None,
            });
        }
    }

    let loc = Locator::new(merged);
    let mut out_of_scope = 0usize;
    if let Some(raw) = opts.scope.as_deref() {
        let scope = resolve_scope(merged, raw);
        let before = cands.len();
        let mut unlocatable = 0usize;
        cands.retain(|c| match loc.file_of(c.id) {
            Some(f) => in_scope(&f, &scope),
            None => {
                unlocatable += 1;
                true
            }
        });
        out_of_scope = before - cands.len();
        eprintln!(
            "[scope] find scope={scope}: {before} -> {} (unlocatable={unlocatable})",
            cands.len()
        );
    }

    let degree = if cands.len() > 1 {
        degrees(merged, &cands)
    } else {
        HashMap::new()
    };
    let deg = |id: NodeId| degree.get(&id).copied().unwrap_or(0);
    cands.sort_by(|a, b| {
        a.tier.cmp(&b.tier).then_with(|| {
            if a.tier.is_exact() {
                // pick_primary's key: (is_declaration, degree, Reverse(id)) max.
                b.declaration
                    .cmp(&a.declaration)
                    .then_with(|| deg(b.id).cmp(&deg(a.id)))
                    .then_with(|| a.id.0.cmp(&b.id.0))
            } else {
                deg(b.id)
                    .cmp(&deg(a.id))
                    .then_with(|| a.qname_len.cmp(&b.qname_len))
                    .then_with(|| a.qname.cmp(&b.qname))
                    .then_with(|| a.id.0.cmp(&b.id.0))
            }
        })
    });

    log_marker(&q.raw, &cands, opts.top_k);

    let ranked = cands.len();
    let cands = match view {
        Some(view) => view.fold(cands, |c| c.id, |c, identity| c.identity = Some(identity)),
        None => cands,
    };
    let folded = ranked - cands.len();

    let keep = if opts.top_k == 0 { cands.len() } else { opts.top_k };
    let rows = cands
        .into_iter()
        .take(keep)
        .map(|c| {
            let l = loc.locate(c.id);
            FoundNode {
                id: l.id,
                qname: l.qname,
                name: l.name,
                kind: l.kind,
                live: false,
                file: l.file,
                line: l.line,
                r#match: c.tier.label(),
                identity: c.identity,
            }
        })
        .collect();
    Found {
        rows,
        out_of_scope,
        folded,
    }
}

/// Is `row` an exact match (`exact_qname` / `exact_name`)? Those two tiers are
/// ordered by `pick_primary`'s key, so the first exact row of an answer is the
/// node `node_id_by_qname` / `resolve_name` return — the one a primitive that
/// resolves its seed through find takes.
pub(crate) fn is_exact(row: &FoundNode) -> bool {
    row.r#match == Tier::ExactQname.label() || row.r#match == Tier::ExactName.label()
}

/// A matched node, before ranking.
struct Candidate {
    id: NodeId,
    tier: Tier,
    declaration: bool,
    qname_len: usize,
    qname: String,
    /// Set by the identity fold on the row that stands for its class.
    identity: Option<RowIdentity>,
}

/// Match tiers, best first; the derived `Ord` IS the tier order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    ExactQname,
    ExactName,
    ExactCi,
    QnameSuffix,
    NamePrefix,
    NameWord,
    NameSubstring,
    QnameSubstring,
    Subsequence,
}

impl Tier {
    const ALL: [Tier; 9] = [
        Tier::ExactQname,
        Tier::ExactName,
        Tier::ExactCi,
        Tier::QnameSuffix,
        Tier::NamePrefix,
        Tier::NameWord,
        Tier::NameSubstring,
        Tier::QnameSubstring,
        Tier::Subsequence,
    ];

    fn label(self) -> &'static str {
        match self {
            Tier::ExactQname => "exact_qname",
            Tier::ExactName => "exact_name",
            Tier::ExactCi => "exact_ci",
            Tier::QnameSuffix => "qname_suffix",
            Tier::NamePrefix => "name_prefix",
            Tier::NameWord => "name_word",
            Tier::NameSubstring => "name_substring",
            Tier::QnameSubstring => "qname_substring",
            Tier::Subsequence => "subsequence",
        }
    }

    /// The two tiers ordered by `pick_primary`'s key.
    fn is_exact(self) -> bool {
        matches!(self, Tier::ExactQname | Tier::ExactName)
    }
}

/// Characters after which a name starts a new word (`name_word`).
const WORD_SEPARATORS: [char; 6] = ['_', '-', '.', ':', '/', ' '];

/// The query, normalised once per call.
struct Query {
    /// Trimmed, case kept: the `exact_qname` / `exact_name` form.
    raw: String,
    /// `raw`, plus `raw` with `.` and `/` read as `::` when that differs.
    qname_forms: Vec<String>,
    /// Case-folded `raw`, as chars (the name tiers).
    folded: Vec<char>,
    folded_str: String,
    /// Case-folded `qname_forms`.
    folded_qname_forms: Vec<String>,
    /// `::` + each of `folded_qname_forms` (`qname_suffix`).
    qname_suffixes: Vec<String>,
}

impl Query {
    fn new(query: &str) -> Self {
        let raw = query.trim().to_string();
        let mut qname_forms = vec![raw.clone()];
        let pathlike = raw.replace(['.', '/'], "::");
        if pathlike != raw {
            qname_forms.push(pathlike);
        }
        let folded_qname_forms: Vec<String> = qname_forms.iter().map(|f| fold(f)).collect();
        let qname_suffixes = folded_qname_forms.iter().map(|f| format!("::{f}")).collect();
        let folded_str = fold(&raw);
        Query {
            folded: folded_str.chars().collect(),
            folded_str,
            raw,
            qname_forms,
            folded_qname_forms,
            qname_suffixes,
        }
    }

    /// The best tier `(name, qname)` matches, or `None`.
    fn tier_of(&self, name: &str, qname: &str) -> Option<Tier> {
        if self.qname_forms.iter().any(|f| f == qname) {
            return Some(Tier::ExactQname);
        }
        if name == self.raw {
            return Some(Tier::ExactName);
        }
        let fname = fold(name);
        let fqname = fold(qname);
        if fname == self.folded_str || self.folded_qname_forms.contains(&fqname) {
            return Some(Tier::ExactCi);
        }
        if self.qname_suffixes.iter().any(|s| fqname.ends_with(s.as_str())) {
            return Some(Tier::QnameSuffix);
        }
        if fname.starts_with(&self.folded_str) {
            return Some(Tier::NamePrefix);
        }
        if starts_a_word(name, &self.folded) {
            return Some(Tier::NameWord);
        }
        if fname.contains(&self.folded_str) {
            return Some(Tier::NameSubstring);
        }
        if self.folded_qname_forms.iter().any(|f| fqname.contains(f.as_str())) {
            return Some(Tier::QnameSubstring);
        }
        if self.folded.len() >= 3 && is_subsequence(&self.folded, &fname) {
            return Some(Tier::Subsequence);
        }
        None
    }
}

/// Case fold, char by char, so the name tiers and the word-boundary walk
/// (which must keep each folded char aligned with its source char) agree on
/// every string. Never slices bytes, so non-ASCII names cannot panic.
fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// Does `q` (folded chars) occur in `name` at a word start other than
/// position 0 (that is `name_prefix`)? A word starts after a
/// [`WORD_SEPARATORS`] char or at a lower→Upper camel step, both judged on
/// the ORIGINAL chars; a char whose fold expands keeps the flag on its first
/// folded char only.
fn starts_a_word(name: &str, q: &[char]) -> bool {
    let orig: Vec<char> = name.chars().collect();
    let mut chars: Vec<char> = Vec::with_capacity(orig.len());
    let mut starts: Vec<bool> = Vec::with_capacity(orig.len());
    for (i, &c) in orig.iter().enumerate() {
        let start = i > 0 && {
            let prev = orig[i - 1];
            WORD_SEPARATORS.contains(&prev) || (prev.is_lowercase() && c.is_uppercase())
        };
        for (k, lc) in c.to_lowercase().enumerate() {
            chars.push(lc);
            starts.push(start && k == 0);
        }
    }
    if q.is_empty() || q.len() > chars.len() {
        return false;
    }
    (1..=chars.len() - q.len()).any(|i| starts[i] && chars[i..i + q.len()] == *q)
}

/// Every char of `q` appears in `hay`, in order.
fn is_subsequence(q: &[char], hay: &str) -> bool {
    let mut it = hay.chars();
    q.iter().all(|c| it.any(|h| h == *c))
}

/// MIRRORS `MergedGraph::is_declaration` (private to the graph crate): false
/// for the container / anchor kinds, true for every other kind. It is the
/// first key of `pick_primary`, and the `exact_*` tiers must order exactly
/// as `pick_primary` does. `engine/tests/find_ranked.rs` pins the parity
/// (a container outranking a same-named declaration on degree). Removal: when
/// the graph crate exposes its pick-primary key, call that and delete this.
fn is_declaration_kind(k: NodeKindId) -> bool {
    const CONTAINERS: [NodeKindId; 5] = [
        node_kind::MODULE,
        node_kind::PACKAGE,
        node_kind::PROJECT,
        node_kind::REGION,
        node_kind::DOC_SPACE,
    ];
    !CONTAINERS.contains(&k)
}

/// Total degree (in + out, intra + cross) of each candidate in ONE pass over
/// `all_edges` — the count `MergedGraph::degree` takes per node, which is
/// O(E) each. A self-loop counts once, as it does there. Lookups only: this
/// map is never iterated.
fn degrees(merged: &MergedGraph, cands: &[Candidate]) -> HashMap<NodeId, usize> {
    let mut deg: HashMap<NodeId, usize> = cands.iter().map(|c| (c.id, 0)).collect();
    for e in merged.all_edges() {
        if let Some(d) = deg.get_mut(&e.from) {
            *d += 1;
        }
        if e.to != e.from
            && let Some(d) = deg.get_mut(&e.to)
        {
            *d += 1;
        }
    }
    deg
}

/// The LD.3b fired_on marker, one line per call:
/// `[find] query='<q>' matched=<n> tiers=<tier:count,...> top_k=<k>`, the
/// tiers listed in tier order, non-zero only (`-` when nothing matched).
fn log_marker(query: &str, cands: &[Candidate], top_k: usize) {
    let tiers: Vec<String> = Tier::ALL
        .iter()
        .filter_map(|t| {
            let n = cands.iter().filter(|c| c.tier == *t).count();
            (n > 0).then(|| format!("{}:{n}", t.label()))
        })
        .collect();
    let tiers = if tiers.is_empty() { "-".to_string() } else { tiers.join(",") };
    eprintln!(
        "[find] query='{query}' matched={} tiers={tiers} top_k={top_k}",
        cands.len()
    );
}

// ---- EB.8: the files a free-text query is about -------------------------

/// The fusion [`find_files`] ranks by (round 3's find table): typed id links
/// first, then the query's BM25 over past commit messages, then the file's
/// degree, then path.
pub const FIND_FILES_FUSION: &str = "LX[IDR,T1s]";

/// What [`find_files`] keeps and how it reports. Start from `default()` and
/// set fields: `#[non_exhaustive]` rules out a struct literal outside this
/// crate.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct FindFilesOptions {
    /// Keep the first `top_k` rows ([`DEFAULT_TOP_K`]); `0` keeps every row.
    pub top_k: usize,
    /// The surface the markers name (`cli`, `py`, `lib`).
    pub surface: &'static str,
    /// The holder's ranking cache (EA.7: a PyGraph's, the CLI's); `None`
    /// builds a transient one over the call's repo roots. Only its
    /// [`crate::rank::FileIndex`], its co-change, PPR indexes and
    /// calibration record are read.
    pub rank_cache: Option<Arc<RankCache>>,
    /// Rank by the repo's calibrated `SELf2t` pick (EC.13; the default);
    /// false ranks [`FIND_FILES_FUSION`] as an explicit choice.
    pub select: bool,
}

impl Default for FindFilesOptions {
    fn default() -> Self {
        FindFilesOptions {
            top_k: DEFAULT_TOP_K,
            surface: "lib",
            rank_cache: None,
            select: true,
        }
    }
}

/// One ranked file of [`find_files`].
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct FoundFile {
    /// 1-based.
    pub rank: usize,
    /// Repo-relative path.
    pub file: String,
    /// Typed id routing over the working tree (`intent::idr`): named + cited.
    pub idr: f64,
    /// BM25 of the query's subject over past commit messages
    /// (`intent::text`), 0 when the history has no messages.
    pub text: f64,
    /// [`crate::rank::FileIndex::degree`]: the tie key after the two scores.
    pub degree: f64,
    /// The routed ids that reached the file, in query order.
    pub ids: Vec<String>,
}

/// The files `query` is about, in one repo: [`FIND_FILES_FUSION`] (rank_eval's
/// find shape `LX[IDR,T1s]` with its tie rule, keys `[idr, text, degree]`
/// descending, then path). See [`find_files_with_ids`]; this drops the
/// routed ids.
pub fn find_files(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    query: &str,
    opts: &FindFilesOptions,
) -> Result<Answer<FoundFile>, String> {
    find_files_with(merged, repo_roots, query, opts, None, None)
}

/// [`find_files`] over a caller's history and with a forced pick (EG.7):
/// `history` replaces every read of the root's `.glia/history-snapshot/`
/// (its `exclude`d commits dropped from the snapshot before any signal reads
/// it; `rev` / `git` route the ids at that revision instead of the working
/// tree; `vocab` replaces the root's `[ids]`), and `pick: Some(name)` ranks
/// that `SELf2t` candidate (source `explicit`) whatever `opts.select` says.
/// `find_files_with(.., None, None)` is [`find_files`].
///
/// For the rank gate's harness (`engine/examples/rank_eval.rs`); unstable.
#[doc(hidden)]
pub fn find_files_with(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    query: &str,
    opts: &FindFilesOptions,
    history: Option<&IntentHistory>,
    pick: Option<&str>,
) -> Result<Answer<FoundFile>, String> {
    find_files_core(merged, repo_roots, query, opts, history, pick).map(|(a, _)| a)
}

/// [`find_files`] plus the ids the query routed (`intent::idr::RoutedId`,
/// one row per resolved key, unresolved mentions with their reason), what
/// `glia find --files` prints above the files.
///
/// - Candidates: the repo's slice of the [`crate::rank::FileIndex`]
///   (`intent::text::Candidates`, paths ascending).
/// - IDR: `intent::idr::route` of the whole query over the WORKING tree's
///   definitions (its docs, trackers and Tier-4 sections, the repo's
///   `[ids]` vocabulary, the history's rename pairs), the recent scopes read
///   from the history snapshot's subjects.
/// - Text: `TextIndex::scores(subject(query))` over the snapshot's messages
///   (zeros when there is none).
/// - Order (EC.13): the selected fusion's keys
///   (`calibrate::fusions::find_keys`, set `SELf2t` on shape `find`, the
///   record of the repo's root; [`FIND_FILES_FUSION`] uncalibrated or with
///   `select` off), `fuse::lex_order` with tie = path position. Rows are the
///   files with a positive first or second key, cut at `top_k`; uncalibrated
///   that is `[idr, text, degree]`, the files with `idr > 0` or `text > 0`.
///
/// Err unless `repo_roots` has exactly one repo (no multi-repo text ranking
/// was measured). An empty answer is a FACT `no_match` absence whose note
/// names the missing inputs; it walked no mechanism, so it has no caveats.
/// Query-time: reads `.glia/history-snapshot/` and
/// `.glia/calibration-snapshot/`, writes nothing. Prints `[select]
/// surface=find_files set=SELf2t shape=find pick=<name> source=<s> ..`, then
/// `[find-files] repo=<label> surface=<s> fusion=<pick> ids=<n>
/// idr_files=<i> text=<present|absent> text_files=<t> rows=<r>`, after the
/// `[intent] text`, `[ids] vocab`, `[id-defs]`, `[id-ns]`, `[id-paths]` and
/// `[idr]` lines of the same surface.
pub fn find_files_with_ids(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    query: &str,
    opts: &FindFilesOptions,
) -> Result<(Answer<FoundFile>, Vec<RoutedId>), String> {
    find_files_core(merged, repo_roots, query, opts, None, None)
}

/// The history a [`find_files_with`] call reads: `history`'s snapshot with
/// its `exclude`d commits dropped (messages kept row-aligned), else the
/// root's on disk.
fn find_history(root: &Path, history: Option<&IntentHistory>) -> Option<(HistorySnapshot, Option<Vec<HistoryMessage>>)> {
    let Some(h) = history else {
        let snap = read_history(root)?;
        let msgs = read_history_messages(root, &snap);
        return Some((snap, msgs));
    };
    if h.exclude.is_empty() {
        return Some((h.snapshot.clone(), h.messages.clone()));
    }
    let keep: Vec<bool> = h.snapshot.commits.iter().map(|c| !h.exclude.contains(&c.c)).collect();
    let commits = h
        .snapshot
        .commits
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(c, _)| c.clone())
        .collect();
    let snap = HistorySnapshot {
        meta: h.snapshot.meta.clone(),
        commits,
        blame: h.snapshot.blame.clone(),
    };
    let msgs = h.messages.as_ref().map(|m| {
        m.iter()
            .filter(|r| !h.exclude.contains(&r.c))
            .cloned()
            .collect()
    });
    Some((snap, msgs))
}

fn find_files_core(
    merged: &MergedGraph,
    repo_roots: &BTreeMap<u64, String>,
    query: &str,
    opts: &FindFilesOptions,
    history: Option<&IntentHistory>,
    pick: Option<&str>,
) -> Result<(Answer<FoundFile>, Vec<RoutedId>), String> {
    let (repo, root) = match (repo_roots.len(), repo_roots.iter().next()) {
        (1, Some((r, p))) => (*r, p.clone()),
        (n, _) => return Err(format!("find --files ranks one repo's files; got {n} repos")),
    };
    let surface = opts.surface;
    let cache = opts.rank_cache.clone().unwrap_or_else(|| {
        let labels = repo_label_map(&[(repo, root.clone())]);
        Arc::new(RankCache::new("answer", repo_roots, &labels))
    });
    cache.guard(merged);
    let label = cache
        .repo_labels()
        .get(&repo)
        .cloned()
        .unwrap_or_else(|| repo_label_for(&root));
    let files = cache.files(merged);
    let cands = Candidates::of(&files, repo);
    let n = cands.len();
    let root_path = Path::new(&root);

    // The history: the caller's (its excluded commits dropped), else the root's.
    let held = find_history(root_path, history);
    let snap = held.as_ref().map(|h| &h.0);
    let msgs = held.as_ref().and_then(|h| h.1.as_deref());

    // T1s: the subject's BM25 over past commit messages, nothing excluded.
    let subj = subject(query).to_string();
    let (text, text_state) = match (snap, msgs) {
        (Some(s), Some(m)) => {
            let idx = TextIndex::build(s, m, &cands, &BTreeSet::new());
            idx.report(&label, surface, &subj);
            (idx.scores(&subj), "present")
        }
        (s, _) => {
            let why = if s.is_none() { TextAbsent::History } else { TextAbsent::Messages };
            why.report(&label, surface);
            (vec![0.0; n], "absent")
        }
    };

    // IDR over the working tree (or the caller's revision).
    let git: &Path = history.and_then(|h| h.git.as_deref()).unwrap_or(root_path);
    let tree = match history.and_then(|h| h.rev.as_deref()) {
        Some(rev) => Tree::Rev {
            repo: git,
            sha: rev.to_string(),
        },
        None => Tree::Work(root_path),
    };
    let at = match &tree {
        Tree::Rev { sha, .. } => format!("{}@{sha}", git.display()),
        _ => "worktree".to_string(),
    };
    let vocab = IntentHistory::vocab_for(history, root_path);
    let routed = IntentHistory::routed(history, ("find", at, query.to_string(), n), || {
        let projects = project_roots(merged);
        let dirs: Vec<&str> = projects
            .iter()
            .map(|p| p.path.as_str())
            .filter(|p| *p != ".")
            .collect();
        let renames = snap.map(rename_pairs).unwrap_or_default();
        let defs = read_defs(&tree, &dirs, &vocab, Some((merged, RepoId(repo))));
        let ns = defs.namespaces(&vocab.local_words);
        let inputs = IdrInputs {
            defs,
            ns,
            paths: PathResolver::new(&cands, &renames),
        };
        let side = inputs.side(&cands, &vocab, &tree).labelled(&label, surface);
        let recent = match (snap, msgs) {
            (Some(s), Some(m)) => recent_scopes(
                &recent_subjects(s, m, &cands, &BTreeSet::new()),
                &inputs.ns,
                &vocab,
            ),
            _ => Vec::new(),
        };
        let routed = route(query, &side, &recent);
        let ids_of = ids_by_file(&routed, &inputs, &vocab, &tree, &cands);
        (routed, ids_of)
    });
    let (routed, ids_of) = (&routed.0, &routed.1);
    let idr: Vec<f64> = (0..n).map(|i| routed.idr.get(i).copied().unwrap_or(0.0)).collect();

    // EC.13: SELf2t's calibrated pick, else LX[IDR,T1s]; the find shape's
    // tie rule (the degree in an unused third key), then path.
    let selection = if let Some(name) = pick {
        explicit(FIND_SET, Shape::Find, name)
    } else if opts.select {
        select(
            cache.calibration().read(),
            FIND_SET,
            Shape::Find,
            &FIND_CANDIDATES,
            FIND_FALLBACK,
        )
    } else {
        explicit(FIND_SET, Shape::Find, FIND_FILES_FUSION)
    };
    log_selection("find_files", &selection);
    let head = FindHead::new(merged, &cache, &cands, snap.map(|s| (s, msgs.unwrap_or(&[]))));
    let keyed = |name: &str| {
        let sig = find_signals_at_head(&head, name, text.clone(), idr.clone());
        find_keys(name, &sig)
    };
    let (fusion, keys) = match keyed(&selection.pick) {
        Some(k) if k.len() == n => (selection.pick.clone(), k),
        _ => (
            FIND_FILES_FUSION.to_string(),
            keyed(FIND_FILES_FUSION).unwrap_or_default(),
        ),
    };
    let mut rows: Vec<FoundFile> = Vec::new();
    for i in find_order(&keys) {
        if opts.top_k > 0 && rows.len() >= opts.top_k {
            break;
        }
        rows.push(FoundFile {
            rank: rows.len() + 1,
            file: cands.paths[i].clone(),
            idr: idr[i],
            text: text[i],
            degree: cands.deg[i],
            ids: ids_of.get(i).cloned().unwrap_or_default(),
        });
    }
    let nz = |v: &[f64]| v.iter().filter(|&&w| w > 0.0).count();
    let (idr_files, text_files) = (nz(&idr), nz(&text));
    eprintln!(
        "[find-files] repo={label} surface={surface} fusion={fusion} ids={} idr_files={idr_files} text={text_state} text_files={text_files} rows={}",
        routed.ids.len(),
        rows.len()
    );
    let mut answer = Answer::from_results(rows, || {
        let mut why: Vec<&str> = Vec::new();
        if idr_files == 0 {
            why.push(if routed.has_id {
                "the query's ids reach no file of this repo"
            } else {
                "no id in the query resolved to a defining row"
            });
        }
        if text_files == 0 {
            why.push(if text_state == "absent" {
                "no history messages (run glia history sync)"
            } else {
                "no past commit message matches the query"
            });
        }
        let note = format!("no file of `{label}` ranks for `{}`: {}", query.trim(), why.join("; "));
        absence::empty(merged, "find_files", query, "no_match", note, &[], None)
    });
    answer.selection = Some(selection);
    Ok((answer, routed.ids.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(q: &str, name: &str, qname: &str) -> Option<&'static str> {
        Query::new(q).tier_of(name, qname).map(Tier::label)
    }

    #[test]
    fn each_tier_fires_on_its_own_shape() {
        assert_eq!(tier("m::f", "f", "m::f"), Some("exact_qname"));
        assert_eq!(tier("f", "f", "m::f"), Some("exact_name"));
        assert_eq!(tier("GETUSER", "getUser", "m::getUser"), Some("exact_ci"));
        assert_eq!(tier("M::GETUSER", "getUser", "m::getUser"), Some("exact_ci"));
        assert_eq!(tier("Svc::run", "run", "m::Svc::run"), Some("qname_suffix"));
        assert_eq!(tier("svc", "Service", "m::Service"), Some("subsequence"));
        assert_eq!(tier("serv", "Service", "m::Service"), Some("name_prefix"));
        assert_eq!(tier("user", "get_user", "m::get_user"), Some("name_word"));
        assert_eq!(tier("user", "getUser", "m::getUser"), Some("name_word"));
        assert_eq!(tier("users", "GET /users", "endpoint:GET:/users"), Some("name_word"));
        assert_eq!(tier("ser", "user", "m::user"), Some("name_substring"));
        assert_eq!(tier("mod", "load", "mod::load"), Some("qname_substring"));
        assert_eq!(tier("usr", "users", "users"), Some("subsequence"));
        assert_eq!(tier("us", "unused", "m::unused"), Some("name_substring"));
        // Subsequence needs 3+ query chars.
        assert_eq!(tier("ud", "used", "x::used"), None);
    }

    #[test]
    fn dotted_and_slashed_queries_are_tried_as_qnames() {
        assert_eq!(tier("m.Svc.run", "run", "m::Svc::run"), Some("exact_qname"));
        assert_eq!(tier("Svc/run", "run", "m::Svc::run"), Some("qname_suffix"));
        // The raw form still matches a qname that really has a `/`.
        assert_eq!(tier("/users", "GET /users", "endpoint:GET:/users"), Some("name_word"));
    }

    #[test]
    fn camel_steps_are_word_starts_but_acronym_runs_are_not() {
        assert!(starts_a_word("getUserById", &['b', 'y']));
        assert!(!starts_a_word("HTTPServer", &['s', 'e', 'r']));
        assert!(!starts_a_word("user", &['u', 's', 'e', 'r']), "position 0 is name_prefix");
    }

    #[test]
    fn non_ascii_names_fold_without_panicking() {
        // 'İ' folds to two chars; the boundary flags must stay aligned.
        assert!(starts_a_word("xİ_größe", &['g', 'r', 'ö']));
        assert_eq!(tier("GRÖSSE", "größe", "m::größe"), None);
        assert_eq!(tier("ΣΟΦΊΑ", "σοφία", "m::σοφία"), Some("exact_ci"));
        assert!(!starts_a_word("", &['a']));
    }
}
