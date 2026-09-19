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

use std::collections::{HashMap, HashSet};

use glia_code_domain::node_kind;
use glia_core::{NodeId, NodeKindId};
use glia_graph::MergedGraph;

use crate::absence::{self, Answer};
use crate::answers::{Locator, entrypoint_reachable, in_scope, live_marker, resolve_scope};

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
}

impl Default for FindOptions {
    fn default() -> Self {
        FindOptions {
            top_k: DEFAULT_TOP_K,
            kinds: None,
            scope: None,
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
/// instead. Prints one `[find] query=...` line and one `[live] annotate
/// surface=find` line per call, and one `[absence] primitive=find` line when
/// the answer is empty.
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
    let mut found = search(merged, query, opts);
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
}

/// The search itself, without the envelope: what an answer that resolves its
/// seed through find (`governing_docs`) calls, so the rows it did not take
/// become its suggestions without a second pass. Every row's `live` is
/// `false` here: [`find_nodes_with_live`] sets it, and a caller that only
/// reads qnames (suggestions, the exact-match seed) pays no liveness walk.
pub(crate) fn search(merged: &MergedGraph, query: &str, opts: &FindOptions) -> Found {
    let q = Query::new(query);
    if q.raw.is_empty() {
        log_marker(&q.raw, &[], opts.top_k);
        return Found {
            rows: Vec::new(),
            out_of_scope: 0,
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

    let keep = if opts.top_k == 0 { cands.len() } else { opts.top_k };
    let rows = cands
        .iter()
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
            }
        })
        .collect();
    Found { rows, out_of_scope }
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
