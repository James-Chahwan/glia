//! Suspected edges (CD.3b), the rows behind the HEURISTIC `suspected_edge`
//! gaps category: learned (kind, category, kind) triples over the
//! cross-service mechanisms, orphan x target candidates by channel-token
//! similarity, Adamic-Adar / resource-allocation and co-change boosts, and the
//! paste-ready `[[edge]]` stanza for the row's `draft` field. The link scores
//! are `glia_activation::algo::linkpred`. Crate-private: `gaps` calls it,
//! cross-module items are `pub(crate)`.
//!
//! 1. **Triples.** Every distinct `(from, to, category)` edge of
//!    `merged.cross_edges` whose category is a flow mechanism
//!    (`arch::FLOW_MECHANISMS`) counts one for `(kind(from), category,
//!    kind(to))`; a triple seen [`MIN_SUPPORT`] times or more is learned.
//!    Cross edges only: they are what the channel resolvers (and the overlay)
//!    paired, which is the pairing a suspected edge proposes. A parser's own
//!    intra-file flow edge (Solidity `emit`: METHOD -EVENT_FLOWS->
//!    EVENT_EMITTER) is not channel pairing, and learning it would make every
//!    method an orphan.
//! 2. **Orphans and targets.** An orphan of a learned `(kx, c, ky)` is a node
//!    of kind `kx` with no outgoing `c` edge (any edge, overlay included); a
//!    target is a node of kind `ky`, `target_unpaired` when it has no
//!    incoming `c` edge. A node whose channel the extractor could not read is
//!    neither (`<unresolved>` in it or an `unresolved:` framework tag: those
//!    are `unresolved_endpoint`'s / `tag_only_queue`'s rows), nor is a node a
//!    cell marks `"external":true` (a third-party call, CG.4a), nor a ROUTE
//!    that is a client-router page (`nav::is_nav_route`).
//! 3. **Channel.** [`tokens`] of `graph::channel_of(qname, name)`: the path
//!    cut at `?` / `#`, split on `/`; a whole-segment `${…}`, `:id`, `{id}`,
//!    `<id>` or `*` is the wildcard PARAM, a `${…}` glued to a literal is
//!    dropped (a fragment of one segment); the rest lower-cased and split on
//!    non-alphanumerics, the [`STOP`] words dropped. [`channel_score`] aligns
//!    the two lists from their ends (PARAM matches any one token) and counts
//!    word equalities up to the first mismatch: per mille of the orphan's
//!    words when the orphan starts with a PARAM (its prefix is unknown), else
//!    of the larger word count. HTTP_CALLS alone also needs the method to
//!    agree (a method-less or `ANY` route takes any). Candidates come only
//!    through a token -> targets index: an orphan scores the targets sharing
//!    a word with it, never every pair.
//! 4. **Structure.** Anchors: a node's CALLS / USES predecessors, then its
//!    HANDLED_BY successors (the orphan's owner, the target's handler), at
//!    most [`MAX_ANCHORS`], else the node itself. One `linkpred::score_pairs`
//!    call over the carry adjacency scores every anchor pair; the boost is
//!    `min(1000, AA / 2)` of the best pair (`aa` in thousandths, `ra` in
//!    millionths in the detail).
//! 5. **Co-change.** 1000 when a CO_CHANGES edge joins the files of an
//!    orphan anchor and a target anchor (the two MODULEs), else 0.
//! 6. **Score** (per mille) = `(600 channel + 150 structure + 150 co-change +
//!    100 000 [target_unpaired]) / 1000`; a pair is kept when the score and
//!    the channel are both >= 500, and an orphan keeps its best three.
//!
//! A row is the orphan (qname, kind, 1-based location), `suggest = "edge"`,
//! tier HEURISTIC, id keyed `orphan NodeId \x1f target NodeId`, and a `draft`
//! that pastes into `.glia/overlay.toml` as is: a `# gap: <id>` comment (the
//! overlay loop's gap link) and one `[[edge]]` with the two qnames as the
//! overlay edge stage binds them. Nothing here mutates the graph. Every
//! iteration is over a `Vec` or a `BTree*`; the hash maps are only looked up.
//!
//! fired_on, once per computation of the category (a report, or a
//! `graph_counts`): `[suspected] triples=<T> orphans=<O> candidates=<C>
//! kept=<K> by=<CATEGORY:n,...>` — `O` orphans with a channel word, `C`
//! pairs channel-scored, `K` rows.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use glia_activation::algo::Adjacency;
use glia_activation::algo::linkpred::{Neighbourhoods, PairScore, score_pairs};
use glia_code_domain::{edge_category, node_kind};
use glia_code_extractors::queues::is_framework_tag;
use glia_core::{Cell, CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use glia_graph::nav::is_nav_route;
use glia_graph::{MergedGraph, channel_of};

use crate::answers::Locator;
use crate::arch::FLOW_MECHANISMS;
use crate::gaps::{GapRow, HEURISTIC, Ids, SUSPECTED_EDGE};
use crate::profile::CODE_PROFILE;

/// Cross edges a triple needs before it is learned.
pub(crate) const MIN_SUPPORT: usize = 2;
/// Per-mille floor of both the channel score and the total score.
const FLOOR: u32 = 500;
/// Targets kept per orphan.
const TOP_PER_ORPHAN: usize = 3;
/// Anchors (owners / handlers) taken per node.
const MAX_ANCHORS: usize = 5;
/// Path words that name no resource.
const STOP: [&str; 13] = [
    "api", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8", "v9", "http", "https", "www",
];

/// One channel token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Tok {
    Word(String),
    /// A path parameter: matches any one token.
    Param,
}

/// What one computation found, printed as the `[suspected]` line.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SuspectedStats {
    pub(crate) triples: usize,
    pub(crate) orphans: usize,
    pub(crate) candidates: usize,
    pub(crate) kept: usize,
    /// Rows per edge-category name.
    pub(crate) by: BTreeMap<&'static str, usize>,
}

impl fmt::Display for SuspectedStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let by: Vec<String> = self.by.iter().map(|(c, n)| format!("{c}:{n}")).collect();
        write!(
            f,
            "[suspected] triples={} orphans={} candidates={} kept={} by={}",
            self.triples,
            self.orphans,
            self.candidates,
            self.kept,
            by.join(",")
        )
    }
}

/// A learned triple.
#[derive(Debug, Clone, Copy)]
struct Triple {
    from: NodeKindId,
    category: EdgeCategoryId,
    to: NodeKindId,
    support: usize,
}

/// One node, its first instance (graphs, then nodes, in `Vec` order).
struct NodeRef<'a> {
    id: NodeId,
    kind: NodeKindId,
    qname: &'a str,
    repo: u64,
    cells: &'a [Cell],
    /// `channel_of(qname, name)`.
    channel: String,
}

/// A channel-scored pair that cleared the channel floor.
struct Cand {
    orphan: usize,
    target: usize,
    triple: usize,
    channel: u32,
    target_unpaired: bool,
    /// Its anchor pairs in the one `score_pairs` batch.
    pairs: std::ops::Range<usize>,
}

/// Every `suspected_edge` row of `merged`, unsorted, ids handed out by `ids`
/// (key `orphan \x1f target`). Prints the `[suspected]` line.
pub(crate) fn suspected_rows(
    merged: &MergedGraph,
    loc: &Locator,
    ids: &mut Ids,
) -> (Vec<GapRow>, SuspectedStats) {
    let mut stats = SuspectedStats::default();
    let rows = compute(merged, loc, ids, &mut stats);
    eprintln!("{stats}");
    (rows, stats)
}

fn compute(
    merged: &MergedGraph,
    loc: &Locator,
    ids: &mut Ids,
    stats: &mut SuspectedStats,
) -> Vec<GapRow> {
    let nodes = node_refs(merged);
    let at: HashMap<NodeId, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    let kind_of = |id: NodeId| at.get(&id).map(|i| nodes[*i].kind);

    let triples = learn(merged, &kind_of);
    stats.triples = triples.len();
    if triples.is_empty() {
        return Vec::new();
    }

    // Who has an outgoing / incoming edge of each flow category, and the
    // anchors of every node, in edge order.
    let mut out: HashSet<(NodeId, EdgeCategoryId)> = HashSet::new();
    let mut inn: HashSet<(NodeId, EdgeCategoryId)> = HashSet::new();
    let mut preds: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut handlers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut cochange: Vec<(NodeId, NodeId)> = Vec::new();
    for e in merged.all_edges() {
        let c = e.category;
        if FLOW_MECHANISMS.contains(&c) {
            out.insert((e.from, c));
            inn.insert((e.to, c));
        }
        if c == edge_category::CALLS || c == edge_category::USES {
            preds.entry(e.to).or_default().push(e.from);
        }
        if c == edge_category::HANDLED_BY {
            handlers.entry(e.from).or_default().push(e.to);
        }
        if c == edge_category::CO_CHANGES && e.from != e.to {
            cochange.push((e.from, e.to));
        }
    }
    let anchors = |id: NodeId| -> Vec<NodeId> {
        let mut a: Vec<NodeId> = Vec::new();
        for x in preds
            .get(&id)
            .into_iter()
            .flatten()
            .chain(handlers.get(&id).into_iter().flatten())
        {
            if a.len() == MAX_ANCHORS {
                break;
            }
            if *x != id && !a.contains(x) {
                a.push(*x);
            }
        }
        if a.is_empty() {
            a.push(id);
        }
        a
    };

    // Per node: can its channel pair at all (tokens are read per triple:
    // HTTP splits the method off).
    let pairable: Vec<bool> = nodes.iter().map(pairable).collect();

    let mut cands: Vec<Cand> = Vec::new();
    let mut pairs: Vec<(NodeId, NodeId)> = Vec::new();
    let mut orphans_seen: BTreeSet<usize> = BTreeSet::new();
    for (ti, t) in triples.iter().enumerate() {
        let http = t.category == edge_category::HTTP_CALLS;
        // Targets and their token index.
        let mut targets: Vec<(usize, Option<String>, Vec<Tok>)> = Vec::new();
        let mut index: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, n) in nodes.iter().enumerate() {
            if n.kind != t.to || !pairable[i] {
                continue;
            }
            let (method, toks) = split_channel(&n.channel, http);
            let slot = targets.len();
            let mut words: BTreeSet<&str> = BTreeSet::new();
            for tok in &toks {
                if let Tok::Word(w) = tok {
                    words.insert(w);
                }
            }
            for w in words {
                index.entry(w.to_string()).or_default().push(slot);
            }
            targets.push((i, method, toks));
        }
        if targets.is_empty() {
            continue;
        }
        for (i, n) in nodes.iter().enumerate() {
            if n.kind != t.from || !pairable[i] || out.contains(&(n.id, t.category)) {
                continue;
            }
            let (method, toks) = split_channel(&n.channel, http);
            let words: BTreeSet<&str> = toks
                .iter()
                .filter_map(|t| match t {
                    Tok::Word(w) => Some(w.as_str()),
                    Tok::Param => None,
                })
                .collect();
            if words.is_empty() {
                continue;
            }
            orphans_seen.insert(i);
            let mut shared: BTreeSet<usize> = BTreeSet::new();
            for w in words {
                shared.extend(index.get(w).into_iter().flatten().copied());
            }
            for slot in shared {
                let (ni, tmethod, ttoks) = &targets[slot];
                if *ni == i {
                    continue;
                }
                if http && !methods_agree(method.as_deref(), tmethod.as_deref()) {
                    continue;
                }
                stats.candidates += 1;
                let channel = channel_score(&toks, ttoks);
                if channel < FLOOR {
                    continue;
                }
                let target = &nodes[*ni];
                let start = pairs.len();
                for o in anchors(n.id) {
                    for h in anchors(target.id) {
                        pairs.push((o, h));
                    }
                }
                cands.push(Cand {
                    orphan: i,
                    target: *ni,
                    triple: ti,
                    channel,
                    target_unpaired: !inn.contains(&(target.id, t.category)),
                    pairs: start..pairs.len(),
                });
            }
        }
    }
    stats.orphans = orphans_seen.len();
    if cands.is_empty() {
        return Vec::new();
    }

    let scores: Vec<PairScore> = {
        let nb = Neighbourhoods::from_adjacency(&Adjacency::carry(merged, &CODE_PROFILE.tables));
        score_pairs(&nb, &pairs)
    };
    let cochanged = CochangeFiles::build(&cochange, loc, &at, &nodes);

    // (orphan) -> its kept candidates, scored.
    let mut kept: BTreeMap<usize, Vec<Scored>> = BTreeMap::new();
    for c in &cands {
        let best = scores[c.pairs.clone()]
            .iter()
            .copied()
            .max_by(|a, b| {
                a.adamic_adar_milli.cmp(&b.adamic_adar_milli).then(
                    a.resource_allocation_micro
                        .cmp(&b.resource_allocation_micro),
                )
            })
            .unwrap_or_default();
        let co = pairs[c.pairs.clone()]
            .iter()
            .any(|(o, h)| cochanged.joins(*o, *h));
        let structural = u32::try_from((best.adamic_adar_milli / 2).min(1000)).unwrap_or(1000);
        let score = (600 * c.channel
            + 150 * structural
            + if co { 150_000 } else { 0 }
            + if c.target_unpaired { 100_000 } else { 0 })
            / 1000;
        if score < FLOOR {
            continue;
        }
        kept.entry(c.orphan).or_default().push(Scored {
            target: c.target,
            triple: c.triple,
            score,
            channel: c.channel,
            best,
            cochange: co,
            target_unpaired: c.target_unpaired,
        });
    }

    let mut rows = Vec::new();
    for (orphan, mut list) in kept {
        list.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then(b.channel.cmp(&a.channel))
                .then_with(|| nodes[a.target].qname.cmp(nodes[b.target].qname))
                .then(nodes[a.target].id.0.cmp(&nodes[b.target].id.0))
        });
        list.truncate(TOP_PER_ORPHAN);
        let o = &nodes[orphan];
        let at_o = loc.locate(o.id);
        for s in list {
            let t = &nodes[s.target];
            let tr = &triples[s.triple];
            let cat = edge_category::name(tr.category);
            let at_t = loc.locate(t.id);
            let where_t = match (at_t.file.as_deref(), at_t.line) {
                (Some(f), Some(l)) => format!("{f}:{l}"),
                (Some(f), None) => f.to_string(),
                _ => "unlocated".to_string(),
            };
            let detail = format!(
                "{cat} -> `{}` ({where_t}) score={} channel={} aa={} ra={} cochange={} \
                 target_unpaired={} triple={}-{cat}->{} seen {}x",
                t.qname,
                milli(s.score),
                milli(s.channel),
                s.best.adamic_adar_milli,
                s.best.resource_allocation_micro,
                yes_no(s.cochange),
                yes_no(s.target_unpaired),
                node_kind::name(tr.from),
                node_kind::name(tr.to),
                tr.support,
            );
            let id = ids.id(SUSPECTED_EDGE, format!("{}\x1f{}", o.id.0, t.id.0));
            let draft = format!(
                "# gap: {id}\n[[edge]]\nfrom = \"{}\"\nto = \"{}\"\ncategory = \"{cat}\"\n\
                 note = \"glia suspected_edge score={}\"\n",
                toml_escape(o.qname),
                toml_escape(t.qname),
                milli(s.score),
            );
            *stats.by.entry(cat).or_default() += 1;
            stats.kept += 1;
            rows.push(GapRow {
                id,
                category: SUSPECTED_EDGE,
                qname: o.qname.to_string(),
                kind: at_o.kind,
                file: at_o.file.clone(),
                line: at_o.line,
                detail,
                suggest: "edge",
                tier: HEURISTIC,
                draft: Some(draft),
            });
        }
    }
    rows
}

/// A kept candidate.
struct Scored {
    target: usize,
    triple: usize,
    score: u32,
    channel: u32,
    best: PairScore,
    cochange: bool,
    target_unpaired: bool,
}

/// Every node's first instance, with its channel.
fn node_refs(merged: &MergedGraph) -> Vec<NodeRef<'_>> {
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut out = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if seen.contains(&n.id) {
                continue;
            }
            let (Some(kind), Some(qname)) =
                (g.nav.kind_by_id.get(&n.id), g.nav.qname_by_id.get(&n.id))
            else {
                continue;
            };
            seen.insert(n.id);
            let name = g
                .nav
                .name_by_id
                .get(&n.id)
                .map(String::as_str)
                .unwrap_or("");
            out.push(NodeRef {
                id: n.id,
                kind: *kind,
                qname,
                repo: g.repo.0,
                cells: &n.cells,
                channel: channel_of(qname, name),
            });
        }
    }
    out
}

/// The learned triples, in (from kind, category, to kind) order.
fn learn(merged: &MergedGraph, kind_of: &dyn Fn(NodeId) -> Option<NodeKindId>) -> Vec<Triple> {
    let edges: BTreeSet<(u64, u64, u32)> = merged
        .cross_edges
        .iter()
        .filter(|e| FLOW_MECHANISMS.contains(&e.category) && e.from != e.to)
        .map(|e| (e.from.0, e.to.0, e.category.0))
        .collect();
    let mut support: BTreeMap<(u32, u32, u32), usize> = BTreeMap::new();
    for (from, to, cat) in edges {
        let (Some(kf), Some(kt)) = (kind_of(NodeId(from)), kind_of(NodeId(to))) else {
            continue;
        };
        *support.entry((kf.0, cat, kt.0)).or_default() += 1;
    }
    support
        .into_iter()
        .filter(|(_, n)| *n >= MIN_SUPPORT)
        .map(|((f, c, t), n)| Triple {
            from: NodeKindId(f),
            category: EdgeCategoryId(c),
            to: NodeKindId(t),
            support: n,
        })
        .collect()
}

/// Can this node take part: its channel was read (no `<unresolved>`, no
/// `unresolved:` framework tag), no cell marks it external, and it is not a
/// client-router page.
fn pairable(n: &NodeRef<'_>) -> bool {
    let bare = n
        .channel
        .split_once(' ')
        .filter(|(m, _)| is_method(m))
        .map_or(n.channel.as_str(), |(_, rest)| rest);
    !(bare.contains("<unresolved>")
        || is_framework_tag(bare)
        || is_external(n.cells)
        || (n.kind == node_kind::ROUTE && is_nav_route(n.cells)))
}

/// A cell (CG.4a's ENDPOINT_HIT) says the node is a third-party call.
fn is_external(cells: &[Cell]) -> bool {
    cells.iter().any(|c| {
        matches!(&c.payload, CellPayload::Json(j) | CellPayload::Text(j)
            if j.contains("\"external\":true"))
    })
}

fn is_method(m: &str) -> bool {
    !m.is_empty() && m.bytes().all(|b| b.is_ascii_uppercase())
}

/// `(method, tokens)` of a channel; the method split off only for HTTP.
fn split_channel(channel: &str, http: bool) -> (Option<String>, Vec<Tok>) {
    if http && let Some((m, rest)) = channel.split_once(' ').filter(|(m, _)| is_method(m)) {
        return (Some(m.to_string()), tokens(rest));
    }
    (None, tokens(channel))
}

/// HTTP: equal methods, or a side with none / `ANY`.
fn methods_agree(orphan: Option<&str>, target: Option<&str>) -> bool {
    match (orphan, target) {
        (Some(a), Some(b)) => a == b || a == "ANY" || b == "ANY",
        _ => true,
    }
}

/// The channel tokens of `path` (see the module doc).
pub(crate) fn tokens(path: &str) -> Vec<Tok> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut out = Vec::new();
    for seg in path.split('/') {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        if is_param_segment(seg) {
            out.push(Tok::Param);
            continue;
        }
        let stripped = strip_placeholders(seg);
        let words: Vec<&str> = stripped
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        if words.is_empty() && seg.contains("${") {
            // Only placeholders (`${a}${b}`): one parameter segment.
            out.push(Tok::Param);
            continue;
        }
        for w in words {
            let w = w.to_lowercase();
            if !STOP.contains(&w.as_str()) {
                out.push(Tok::Word(w));
            }
        }
    }
    out
}

/// A whole segment that is a parameter: `${…}`, `:id`, `{id}`, `<id>`, `*`.
fn is_param_segment(seg: &str) -> bool {
    seg == "*"
        || seg.starts_with(':')
        || (seg.starts_with('{') && seg.ends_with('}'))
        || (seg.starts_with('<') && seg.ends_with('>'))
        || (seg.starts_with("${")
            && seg.ends_with('}')
            && seg[2..].find('}') == Some(seg.len() - 3))
}

/// `seg` with every `${…}` placeholder removed.
fn strip_placeholders(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    let mut rest = seg;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(end) => rest = &rest[start + end + 1..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Suffix-aligned similarity of two token lists, per mille (module doc, 3).
pub(crate) fn channel_score(orphan: &[Tok], target: &[Tok]) -> u32 {
    let words = |ts: &[Tok]| ts.iter().filter(|t| matches!(t, Tok::Word(_))).count();
    let (known, known_t) = (words(orphan), words(target));
    if known == 0 {
        return 0;
    }
    let mut matches = 0usize;
    for (a, b) in orphan.iter().rev().zip(target.iter().rev()) {
        match (a, b) {
            (Tok::Param, _) | (_, Tok::Param) => {}
            (Tok::Word(x), Tok::Word(y)) if x == y => matches += 1,
            _ => break,
        }
    }
    let denom = if orphan.first() == Some(&Tok::Param) {
        known
    } else {
        known.max(known_t)
    };
    u32::try_from(1000 * matches / denom).unwrap_or(1000)
}

/// The (repo, file) pairs CO_CHANGES edges join, both ways round.
struct CochangeFiles<'a> {
    pairs: HashSet<((u64, String), (u64, String))>,
    loc: &'a Locator<'a>,
    at: &'a HashMap<NodeId, usize>,
    nodes: &'a [NodeRef<'a>],
}

impl<'a> CochangeFiles<'a> {
    fn build(
        edges: &[(NodeId, NodeId)],
        loc: &'a Locator<'a>,
        at: &'a HashMap<NodeId, usize>,
        nodes: &'a [NodeRef<'a>],
    ) -> Self {
        let mut s = CochangeFiles {
            pairs: HashSet::new(),
            loc,
            at,
            nodes,
        };
        for (a, b) in edges {
            if let (Some(fa), Some(fb)) = (s.file(*a), s.file(*b))
                && fa != fb
            {
                s.pairs.insert((fa.clone(), fb.clone()));
                s.pairs.insert((fb, fa));
            }
        }
        s
    }

    fn file(&self, id: NodeId) -> Option<(u64, String)> {
        let repo = self.nodes[*self.at.get(&id)?].repo;
        Some((repo, self.loc.file_of(id)?))
    }

    /// A CO_CHANGES edge joins the files of `a` and `b`.
    fn joins(&self, a: NodeId, b: NodeId) -> bool {
        if self.pairs.is_empty() {
            return false;
        }
        match (self.file(a), self.file(b)) {
            (Some(fa), Some(fb)) => self.pairs.contains(&(fa, fb)),
            _ => false,
        }
    }
}

/// Per mille as `0.87`.
fn milli(v: u32) -> String {
    format!("{}.{:02}", v / 1000, (v % 1000) / 10)
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// `s` as the body of a TOML basic string.
fn toml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Tok {
        Tok::Word(s.to_string())
    }

    #[test]
    fn tokens_mark_params_and_drop_stop_words() {
        assert_eq!(tokens("${…}/read-all"), [Tok::Param, w("read"), w("all")]);
        assert_eq!(
            tokens("/notifications/:id/read"),
            [w("notifications"), Tok::Param, w("read")]
        );
        assert_eq!(
            tokens("/api/v2/Users/{id}?page=1"),
            [w("users"), Tok::Param]
        );
        assert_eq!(tokens("${…}/smr${…}"), [Tok::Param, w("smr")]);
        assert_eq!(tokens("<int:id>/x/*"), [Tok::Param, w("x"), Tok::Param]);
        assert_eq!(tokens("orders.created"), [w("orders"), w("created")]);
        assert_eq!(tokens("${a}${b}/x"), [Tok::Param, w("x")]);
    }

    #[test]
    fn channel_scores_align_from_the_end() {
        let s = |a: &str, b: &str| channel_score(&tokens(a), &tokens(b));
        assert_eq!(s("${…}/read-all", "/notifications/read-all"), 1000);
        assert_eq!(s("${…}/${…}/read", "/notifications/:id/read"), 1000);
        assert_eq!(s("${…}/smr${…}", "/smr"), 1000);
        assert_eq!(
            s("/notifications/${…}/read", "/notifications/:id/read"),
            1000
        );
        assert_eq!(s("/orders-svc/orders", "/orders"), 333);
        assert_eq!(s("/users", "/admin/users"), 500);
        assert_eq!(s("${…}/read-all", "/notifications/read-all/extra"), 0);
        assert_eq!(s("${…}", "/anything"), 0, "no word: skipped");
    }

    #[test]
    fn methods_and_escaping() {
        assert!(methods_agree(Some("POST"), Some("POST")));
        assert!(methods_agree(Some("POST"), Some("ANY")));
        assert!(methods_agree(Some("POST"), None));
        assert!(!methods_agree(Some("POST"), Some("GET")));
        assert_eq!(
            split_channel("POST ${…}/read-all", true),
            (
                Some("POST".to_string()),
                vec![Tok::Param, w("read"), w("all")]
            )
        );
        assert_eq!(split_channel("orders created", false).0, None);
        assert_eq!(toml_escape("a\"b\\c\n"), "a\\\"b\\\\c\\n");
        assert_eq!(milli(700), "0.70");
        assert_eq!(milli(1000), "1.00");
        assert_eq!(milli(875), "0.87");
    }

    #[test]
    fn empty_graph_learns_nothing() {
        let m = MergedGraph::new(Vec::new());
        let loc = Locator::new(&m);
        let mut ids = Ids::default();
        let (rows, stats) = suspected_rows(&m, &loc, &mut ids);
        assert!(rows.is_empty());
        assert_eq!(stats, SuspectedStats::default());
        assert_eq!(
            stats.to_string(),
            "[suspected] triples=0 orphans=0 candidates=0 kept=0 by="
        );
    }
}
