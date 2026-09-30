//! Communities (CD.1d): seeded Leiden over the code graph, scope-aware, with
//! structured per-community summaries (kinds, top members, entries, services,
//! effect sinks, inter-community links, cohesion) computed at query time,
//! located and tiered. The algorithm is `glia_activation::algo::community`;
//! this module answers with it. Public slot, reached by module path
//! (`glia_engine::communities::<item>`). Nothing is stored: no cell, no store
//! change, so a seed change never moves a build.
//!
//! THE VIEW. Every node under [`CommunityArgs::scope`] (a path or a project
//! label, resolved once; no scope is every node; an unlocatable node is kept,
//! as `answers::node_in_scope` keeps it), and every edge between two of them
//! whose category weighs more than 0 in the domain's `community_weights`
//! (CD.1c), graphs first, then cross edges. An edge to an id that is no node
//! is left out: it has no kind, qname or file to summarise. A node in several
//! per-language graphs is one node, read from the first graph listing it. The
//! weighted undirected view is `WeightedGraph::from_source` over that source.
//!
//! ID INDEPENDENCE. Node ids hash the repo's identity key (its git remote,
//! else its local repo, else its directory name: LB.1), and the view orders
//! its nodes by id value, which is the order Leiden's seeded shuffle permutes
//! and its ties fall back on. So the source hands the view each node's RANK
//! in the canonical order (qname, kind, file, repo label, id) as its id,
//! never the node id itself: the same code under another key (a fork, a
//! renamed checkout) gets the same partition, the same community ids and the
//! same summaries (only the located entries' `id`s differ). `ScopedPartition::node` maps a dense
//! index back to its node.
//!
//! THE PARTITION. `algo::community::communities` (Leiden up to its pair cap,
//! label propagation above), or exactly [`CommunityArgs::method`]: `leiden`,
//! or `lpa` / `label_propagation`. An unknown name is an absence `no_match`.
//! The resolution is held as `num / 1000` (gamma 1.0 is `1000 / 1000`); a
//! value that is not a finite number above 0 runs at 1.0, and the answer
//! reports the gamma it ran at. The seed is reported with the answer: the
//! partition depends on it, and another seed can escape a local optimum.
//!
//! THE SUMMARIES. Nodes with no weighted edge (strength 0) are `isolated`:
//! counted, never listed, in no community. The other communities are
//! numbered by size descending, then by their first member in the canonical
//! order; [`CommunitiesAnswer::total`] counts them all, and the first
//! [`CommunityArgs::top`] of at least [`CommunityArgs::min_size`] members
//! are summarised:
//! - `kinds`: node kind histogram, count descending, then name;
//! - `top_members`: the [`CommunityArgs::members`] members with the largest
//!   internal weighted degree (the weight of their view pairs inside the
//!   community; ties by qname), located through one `Locator`;
//! - `entries`: members that are entrypoints (the domain's entry rule over
//!   kind, name and roles, or an ENTRYPOINT cell), heaviest first, at most
//!   [`MAX_ENTRIES`];
//! - `services`: `glia arch` service histogram of the located members
//!   (`default_keying` + `service_of`), count descending, then name;
//! - `sinks`: over every weighted edge from a member to any node, the edges
//!   the domain's `effect_sink` classifies, counted per class;
//! - `links`: per other community, the view weight, edge count and category
//!   histogram of the edges between the two (either direction), the
//!   [`MAX_LINKS`] heaviest (ties by community id);
//! - `cohesion`: internal weight / (internal + boundary weight), self-loops
//!   internal;
//! - `files`: distinct member files;
//! - `label`: the longest `::`-segment prefix all the top members' qnames
//!   share, else the most frequent first two segments among them (ties to
//!   the smaller string) - mechanical, never prose.
//!
//! Every summary is tier [`HEURISTIC`]: the grouping is an optimisation
//! output, while every count under it is read off observed edges. `top` and
//! `members` of 0 keep every community / member.
//!
//! EMPTY. No weighted edge in the view is an absence `no_edges`; communities
//! found but none of `min_size` members is an absence `no_match`.
//!
//! fired_on marker, once per call:
//! `[communities] method=<leiden|label_propagation> nodes=<N> weighted_edges=<M> communities=<K> listed=<T> isolated=<I> modularity=<Q:.3> levels=<L> seed=<S> surface=<engine|cli|py>`
//! (`weighted_edges` is the view's pair count; `method=none` when the method
//! name was refused).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_activation::algo::GraphSource;
use glia_activation::algo::community::{
    self as algo, CommunityOptions, Partition, Resolution, WeightedGraph,
};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Edge, NodeId, NodeKindId};
use glia_graph::MergedGraph;
use glia_graph::roles::roles_in;

use crate::absence::{self, Absence};
use crate::answers::{Located, Locator, in_scope, is_declared_entry, resolve_scope};
use crate::arch::{ServiceKeying, default_keying, service_of};
use crate::profile::CODE_PROFILE;

/// [`CommunityArgs::seed`] by default.
pub const DEFAULT_SEED: u64 = 42;
/// [`CommunityArgs::resolution`] by default: textbook modularity.
pub const DEFAULT_RESOLUTION: f64 = 1.0;
/// [`CommunityArgs::top`] by default.
pub const DEFAULT_TOP: usize = 30;
/// [`CommunityArgs::members`] by default.
pub const DEFAULT_MEMBERS: usize = 10;
/// [`CommunityArgs::min_size`] by default.
pub const DEFAULT_MIN_SIZE: usize = 2;
/// Entries listed per community at most.
pub const MAX_ENTRIES: usize = 10;
/// Links listed per community at most.
pub const MAX_LINKS: usize = 5;
/// [`CommunitySummary::tier`] of every summary.
pub const HEURISTIC: &str = "heuristic";
/// [`CommunityArgs::surface`] when the engine is called directly.
pub const SURFACE_ENGINE: &str = "engine";

const PRIMITIVE: &str = "communities";
/// The resolution's fixed denominator: gamma is held in thousandths.
const RESOLUTION_DEN: u32 = 1000;

/// What [`communities`] partitions and how much it summarises. Start from
/// `default()` and set fields.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct CommunityArgs {
    /// Nodes under this path or project label only.
    pub scope: Option<String>,
    /// Seeds every random choice ([`DEFAULT_SEED`]).
    pub seed: u64,
    /// Modularity's gamma ([`DEFAULT_RESOLUTION`]); above 1 favours more,
    /// smaller communities. Held as thousandths.
    pub resolution: f64,
    /// Communities summarised ([`DEFAULT_TOP`]); 0 keeps every one.
    pub top: usize,
    /// Top members per community ([`DEFAULT_MEMBERS`]); 0 keeps every one.
    pub members: usize,
    /// The smallest community summarised ([`DEFAULT_MIN_SIZE`]).
    pub min_size: usize,
    /// `leiden`, `lpa` (`label_propagation`), or `None` for Leiden up to the
    /// pair cap and label propagation above.
    pub method: Option<String>,
    /// Who asked, for the marker: [`SURFACE_ENGINE`], `cli` or `py` (empty
    /// reads as [`SURFACE_ENGINE`]).
    pub surface: &'static str,
}

impl Default for CommunityArgs {
    fn default() -> Self {
        CommunityArgs {
            scope: None,
            seed: DEFAULT_SEED,
            resolution: DEFAULT_RESOLUTION,
            top: DEFAULT_TOP,
            members: DEFAULT_MEMBERS,
            min_size: DEFAULT_MIN_SIZE,
            method: None,
            surface: SURFACE_ENGINE,
        }
    }
}

/// One member of a community.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct CommunityMember {
    pub qname: String,
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// Internal weighted degree: the weight of its view pairs inside the
    /// community.
    pub weight: u64,
}

/// The edges between a community and another one, either direction.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct CommunityLink {
    /// The other community's id.
    pub to: u32,
    /// Summed community weight of those edges.
    pub weight: u64,
    /// Those edges.
    pub edges: usize,
    /// `(category, edges)`, count descending, then name.
    pub categories: Vec<(&'static str, usize)>,
}

/// One community, summarised.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct CommunitySummary {
    /// 0-based: size descending, then first member in canonical order.
    pub id: u32,
    /// Members (isolated nodes never are).
    pub size: usize,
    /// The top members' common qname prefix (see the module doc).
    pub label: String,
    /// Always [`HEURISTIC`].
    pub tier: &'static str,
    /// Internal weight / (internal + boundary weight), in 0..=1.
    pub cohesion: f64,
    /// Distinct member files.
    pub files: usize,
    /// `(kind, members)`, count descending, then name.
    pub kinds: Vec<(&'static str, usize)>,
    pub top_members: Vec<CommunityMember>,
    /// Entrypoint members, heaviest first, at most [`MAX_ENTRIES`].
    pub entries: Vec<Located>,
    /// `(glia arch service, located members)`, count descending, then name.
    pub services: Vec<(String, usize)>,
    /// `(effect class, edges)`, count descending, then class.
    pub sinks: Vec<(&'static str, usize)>,
    /// The [`MAX_LINKS`] heaviest links to other communities.
    pub links: Vec<CommunityLink>,
}

/// The communities answer.
#[non_exhaustive]
#[derive(serde::Serialize, Clone, Debug)]
pub struct CommunitiesAnswer {
    /// `leiden` or `label_propagation` (`none` when the method was refused).
    pub method: &'static str,
    pub seed: u64,
    /// The gamma the search ran at.
    pub resolution: f64,
    /// Of the whole partition at that gamma.
    pub modularity: f64,
    /// Communities found (isolated nodes are in none).
    pub total: usize,
    /// Nodes in the view (in scope), isolated ones included.
    pub nodes: usize,
    /// Nodes with no weighted edge.
    pub isolated: usize,
    /// The summarised communities, in id order.
    pub communities: Vec<CommunitySummary>,
    /// `Some` iff `communities` is empty.
    pub absence: Option<Absence>,
}

/// Partition the code graph (or `args.scope` of it) into communities and
/// summarise them. `repo_labels` name the services exactly as `glia arch`
/// names them (`GenerateResult::repo_labels`).
pub fn communities(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    args: &CommunityArgs,
) -> CommunitiesAnswer {
    let res = resolution_of(args.resolution);
    let gamma = f64::from(res.num) / f64::from(RESOLUTION_DEN);
    if method_choice(args.method.as_deref()).is_none() {
        let name = args.method.as_deref().unwrap_or_default();
        let note =
            format!("no community method is named `{name}`; use leiden or lpa (label_propagation)");
        let absence = absence::empty(merged, PRIMITIVE, &query(args), "no_match", note, &[], None);
        let answer = CommunitiesAnswer {
            method: "none",
            seed: args.seed,
            resolution: gamma,
            modularity: 0.0,
            total: 0,
            nodes: 0,
            isolated: 0,
            communities: Vec::new(),
            absence: Some(absence),
        };
        marker(&answer, 0, 0, args);
        return answer;
    }

    let loc = Locator::new(merged);
    let mut opts = CommunityOptions::default();
    opts.seed = args.seed;
    opts.resolution = res;
    let sp = partition_of(
        merged,
        &loc,
        repo_labels,
        args.scope.as_deref(),
        &opts,
        args.method.as_deref(),
    );
    let groups = sp.groups();
    let isolated = sp.nodes.len() - groups.iter().map(Vec::len).sum::<usize>();
    let min_size = args.min_size.max(1);
    let listed: Vec<usize> = groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.len() >= min_size)
        .map(|(c, _)| c)
        .take(if args.top == 0 { usize::MAX } else { args.top })
        .collect();

    let summaries = if listed.is_empty() {
        Vec::new()
    } else {
        Summariser::new(merged, &loc, repo_labels, &sp, &groups).summarise(&listed, args)
    };
    let absence = summaries
        .is_empty()
        .then(|| no_communities(merged, args, &sp, groups.len(), min_size));
    let answer = CommunitiesAnswer {
        method: sp.partition.method.name(),
        seed: args.seed,
        resolution: gamma,
        modularity: sp.partition.modularity,
        total: groups.len(),
        nodes: sp.nodes.len(),
        isolated,
        communities: summaries,
        absence,
    };
    marker(&answer, sp.view.pair_count(), sp.partition.levels, args);
    answer
}

/// Which algorithm a [`CommunityArgs::method`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MethodChoice {
    /// `algo::community::communities`: Leiden up to the pair cap.
    Auto,
    Leiden,
    LabelPropagation,
}

/// The method `name` names (ASCII case and surrounding space ignored); `None`
/// for an unknown name.
fn method_choice(name: Option<&str>) -> Option<MethodChoice> {
    let Some(name) = name else {
        return Some(MethodChoice::Auto);
    };
    match name.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => Some(MethodChoice::Auto),
        "leiden" => Some(MethodChoice::Leiden),
        "lpa" | "label_propagation" => Some(MethodChoice::LabelPropagation),
        _ => None,
    }
}

/// Gamma `r` in thousandths, at least 1; a value that is not a finite number
/// above 0 reads as [`DEFAULT_RESOLUTION`].
fn resolution_of(r: f64) -> Resolution {
    let r = if r.is_finite() && r > 0.0 {
        r
    } else {
        DEFAULT_RESOLUTION
    };
    let num = (r * f64::from(RESOLUTION_DEN))
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32;
    Resolution {
        num,
        den: RESOLUTION_DEN,
    }
}

/// One node of the view.
pub(crate) struct ViewNode<'g> {
    /// The node's own id.
    pub(crate) id: NodeId,
    pub(crate) repo: u64,
    pub(crate) qname: &'g str,
    pub(crate) kind: Option<NodeKindId>,
    /// As the `Locator` places it.
    pub(crate) file: Option<String>,
    /// An entrypoint by the domain's entry rule or an ENTRYPOINT cell.
    pub(crate) entry: bool,
}

/// A scope's weighted view and its partition: what [`communities`]
/// summarises, and the community quotient CD.2b cuts.
///
/// The view's ids are RANKS, not node ids (see the module doc): node `r` of
/// the source is `NodeId(r)`. [`ScopedPartition::node`] maps a dense index
/// back to its node.
pub(crate) struct ScopedPartition<'g> {
    /// The in-scope nodes in canonical order: rank `r` is `nodes[r]`.
    pub(crate) nodes: Vec<ViewNode<'g>>,
    /// The weighted edges between them, endpoints as ranks, cells dropped.
    pub(crate) edges: Vec<Edge>,
    pub(crate) view: WeightedGraph,
    /// Over the view's dense indices.
    pub(crate) partition: Partition,
}

impl<'g> ScopedPartition<'g> {
    /// The node at dense index `ix` of the view.
    pub(crate) fn node(&self, ix: u32) -> &ViewNode<'g> {
        &self.nodes[self.view.id(ix).0 as usize]
    }

    /// The dense index of rank `r` (an endpoint of [`Self::edges`]).
    fn ix_of(&self, r: NodeId) -> Option<u32> {
        self.view.index_of(r)
    }

    /// The communities over non-isolated nodes, each its dense indices in
    /// ascending order, numbered by size descending, then by first member.
    /// Dense order is canonical order, so the numbering is id-independent.
    fn groups(&self) -> Vec<Vec<u32>> {
        let mut by_label: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for ix in 0..self.view.len() as u32 {
            if self.view.strength(ix) == 0 {
                continue;
            }
            if let Some(&c) = self.partition.membership.get(ix as usize) {
                by_label.entry(c).or_default().push(ix);
            }
        }
        let mut groups: Vec<Vec<u32>> = by_label.into_values().collect();
        groups.sort_by(|a, b| b.len().cmp(&a.len()).then(a.first().cmp(&b.first())));
        groups
    }
}

/// The weighted view of `scope` (a path or a project label, resolved here;
/// `None` is every node) and its partition under `opts` by the method
/// `method` names as [`CommunityArgs::method`] names one. An unknown name
/// runs the default choice: [`communities`] refuses one before calling.
///
/// The seam CD.2b (splits, community quotient) reuses: `sp.partition
/// .membership` is over `sp.view`'s dense indices, and `sp.node(ix).id` is the
/// node id at dense index `ix`.
pub(crate) fn partition_of<'g>(
    merged: &'g MergedGraph,
    loc: &Locator<'g>,
    repo_labels: &BTreeMap<u64, String>,
    scope: Option<&str>,
    opts: &CommunityOptions,
    method: Option<&str>,
) -> ScopedPartition<'g> {
    let scope = scope.map(|s| resolve_scope(merged, s));
    let nodes = view_nodes(merged, loc, repo_labels, scope.as_deref());
    let rank: HashMap<NodeId, u64> = nodes
        .iter()
        .enumerate()
        .map(|(r, n)| (n.id, r as u64))
        .collect();
    let tables = &CODE_PROFILE.tables;
    let edges: Vec<Edge> = merged
        .all_edges()
        .filter(|e| tables.community_weight(e.category) > 0)
        .filter_map(|e| {
            Some(Edge {
                from: NodeId(*rank.get(&e.from)?),
                to: NodeId(*rank.get(&e.to)?),
                category: e.category,
                confidence: e.confidence,
                cells: Vec::new(),
            })
        })
        .collect();
    let source = RankSource {
        n: nodes.len() as u64,
        edges: &edges,
    };
    let view = WeightedGraph::from_source(&source, tables.community_weights);
    let partition = match method_choice(method).unwrap_or(MethodChoice::Auto) {
        MethodChoice::Auto => algo::communities(&view, opts),
        MethodChoice::Leiden => algo::leiden(&view, opts),
        MethodChoice::LabelPropagation => algo::label_propagation(&view, opts),
    };
    ScopedPartition {
        nodes,
        edges,
        view,
        partition,
    }
}

/// The in-scope nodes, once each (first graph wins), in canonical order:
/// qname, kind, file, repo label, id.
fn view_nodes<'g>(
    merged: &'g MergedGraph,
    loc: &Locator<'g>,
    repo_labels: &BTreeMap<u64, String>,
    scope: Option<&str>,
) -> Vec<ViewNode<'g>> {
    let entry_rule = &CODE_PROFILE.tables.entry;
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut nodes: Vec<ViewNode<'g>> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if !seen.insert(n.id) {
                continue;
            }
            let file = loc.file_of(n.id);
            if let (Some(s), Some(f)) = (scope, file.as_deref())
                && !in_scope(f, s)
            {
                continue;
            }
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            let name = g.nav.name_by_id.get(&n.id).map_or("", String::as_str);
            let entry = entry_rule.is_entry(kind, name, &roles_in(kind, &n.cells))
                || is_declared_entry(&n.cells);
            nodes.push(ViewNode {
                id: n.id,
                repo: g.repo.0,
                qname: g.nav.qname_by_id.get(&n.id).map_or("", String::as_str),
                kind,
                file,
                entry,
            });
        }
    }
    let label = |repo: u64| repo_labels.get(&repo).map_or("", String::as_str);
    nodes.sort_by(|a, b| {
        a.qname
            .cmp(b.qname)
            .then(a.kind.map(|k| k.0).cmp(&b.kind.map(|k| k.0)))
            .then(a.file.cmp(&b.file))
            .then(label(a.repo).cmp(label(b.repo)))
            .then(a.id.0.cmp(&b.id.0))
    });
    nodes
}

/// The view's source: nodes `NodeId(0)..NodeId(n)` (ranks) and the weighted
/// edges between them.
struct RankSource<'a> {
    n: u64,
    edges: &'a [Edge],
}

impl GraphSource for RankSource<'_> {
    fn node_ids(&self) -> Vec<NodeId> {
        (0..self.n).map(NodeId).collect()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

/// Everything the summaries read, computed once over the whole view.
struct Summariser<'s, 'g> {
    merged: &'g MergedGraph,
    loc: &'s Locator<'g>,
    labels: &'s BTreeMap<u64, String>,
    sp: &'s ScopedPartition<'g>,
    groups: &'s [Vec<u32>],
    /// Community of each dense index; `u32::MAX` for an isolated node.
    comm: Vec<u32>,
    /// Internal weighted degree of each dense index.
    inner: Vec<u64>,
    keying: ServiceKeying,
}

/// Accumulated edges from one community to another.
#[derive(Default)]
struct LinkAcc {
    weight: u64,
    edges: usize,
    categories: BTreeMap<&'static str, usize>,
}

impl<'s, 'g> Summariser<'s, 'g> {
    fn new(
        merged: &'g MergedGraph,
        loc: &'s Locator<'g>,
        labels: &'s BTreeMap<u64, String>,
        sp: &'s ScopedPartition<'g>,
        groups: &'s [Vec<u32>],
    ) -> Self {
        let n = sp.view.len();
        let mut comm = vec![u32::MAX; n];
        for (c, g) in groups.iter().enumerate() {
            for &ix in g {
                comm[ix as usize] = c as u32;
            }
        }
        let inner = (0..n as u32)
            .map(|ix| {
                let c = comm[ix as usize];
                sp.view
                    .neighbours(ix)
                    .iter()
                    .filter(|&&(u, _)| comm[u as usize] == c)
                    .fold(0u64, |s, &(_, w)| s.saturating_add(w))
            })
            .collect();
        Summariser {
            merged,
            loc,
            labels,
            sp,
            groups,
            comm,
            inner,
            keying: default_keying(merged),
        }
    }

    /// The summaries of the `listed` communities, in that order.
    fn summarise(&self, listed: &[usize], args: &CommunityArgs) -> Vec<CommunitySummary> {
        let wanted: BTreeSet<u32> = listed.iter().map(|&c| c as u32).collect();
        let links = self.links(&wanted);
        let sinks = self.sinks(&wanted);
        listed
            .iter()
            .map(|&c| {
                let cid = c as u32;
                self.summary(
                    cid,
                    args,
                    links.get(&cid).map_or(&[][..], Vec::as_slice),
                    sinks.get(&cid),
                )
            })
            .collect()
    }

    fn summary(
        &self,
        c: u32,
        args: &CommunityArgs,
        links: &[CommunityLink],
        sinks: Option<&BTreeMap<&'static str, usize>>,
    ) -> CommunitySummary {
        let members = &self.groups[c as usize];
        let mut ranked: Vec<u32> = members.clone();
        ranked.sort_by(|&a, &b| {
            self.inner[b as usize]
                .cmp(&self.inner[a as usize])
                .then(self.sp.node(a).qname.cmp(self.sp.node(b).qname))
                .then(a.cmp(&b))
        });
        let cap = |k: usize| if k == 0 { usize::MAX } else { k };
        let top_members: Vec<CommunityMember> = ranked
            .iter()
            .take(cap(args.members))
            .map(|&ix| {
                let at = self.loc.locate(self.sp.node(ix).id);
                CommunityMember {
                    qname: at.qname,
                    kind: at.kind,
                    file: at.file,
                    line: at.line,
                    weight: self.inner[ix as usize],
                }
            })
            .collect();
        let entries: Vec<Located> = ranked
            .iter()
            .filter(|&&ix| self.sp.node(ix).entry)
            .take(MAX_ENTRIES)
            .map(|&ix| self.loc.locate(self.sp.node(ix).id))
            .collect();

        let mut kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut services: BTreeMap<String, usize> = BTreeMap::new();
        let mut files: BTreeSet<&str> = BTreeSet::new();
        let (mut internal, mut boundary) = (0u64, 0u64);
        for &ix in members {
            let node = self.sp.node(ix);
            *kinds
                .entry(node.kind.map_or("UNKNOWN", node_kind::name))
                .or_default() += 1;
            if let Some(f) = node.file.as_deref() {
                files.insert(f);
                *services
                    .entry(service_of(f, node.repo, &self.keying, self.labels))
                    .or_default() += 1;
            }
            // A pair inside is met from both ends; the diagonal is twice the
            // self-loop weight. Both halve below.
            internal = internal
                .saturating_add(self.inner[ix as usize])
                .saturating_add(self.sp.view.self_weight(ix));
            let out = self
                .sp
                .view
                .strength(ix)
                .saturating_sub(self.sp.view.self_weight(ix))
                .saturating_sub(self.inner[ix as usize]);
            boundary = boundary.saturating_add(out);
        }
        let internal = internal / 2;
        let cohesion = if internal + boundary == 0 {
            0.0
        } else {
            internal as f64 / (internal + boundary) as f64
        };
        let top_qnames: Vec<&str> = top_members.iter().map(|m| m.qname.as_str()).collect();
        CommunitySummary {
            id: c,
            size: members.len(),
            label: label_of(&top_qnames),
            tier: HEURISTIC,
            cohesion,
            files: files.len(),
            kinds: by_count(kinds),
            top_members,
            entries,
            services: by_count(services),
            sinks: sinks.cloned().map(by_count).unwrap_or_default(),
            links: links.to_vec(),
        }
    }

    /// Per wanted community, its [`MAX_LINKS`] heaviest links, from the view
    /// edges between two communities (either direction).
    fn links(&self, wanted: &BTreeSet<u32>) -> BTreeMap<u32, Vec<CommunityLink>> {
        let tables = &CODE_PROFILE.tables;
        let mut acc: BTreeMap<(u32, u32), LinkAcc> = BTreeMap::new();
        for e in &self.sp.edges {
            let (Some(a), Some(b)) = (self.sp.ix_of(e.from), self.sp.ix_of(e.to)) else {
                continue;
            };
            let (ca, cb) = (self.comm[a as usize], self.comm[b as usize]);
            if ca == cb || ca == u32::MAX || cb == u32::MAX {
                continue;
            }
            let w = u64::from(tables.community_weight(e.category));
            let name = edge_category::name(e.category);
            for (from, to) in [(ca, cb), (cb, ca)] {
                if !wanted.contains(&from) {
                    continue;
                }
                let link = acc.entry((from, to)).or_default();
                link.weight = link.weight.saturating_add(w);
                link.edges += 1;
                *link.categories.entry(name).or_default() += 1;
            }
        }
        let mut out: BTreeMap<u32, Vec<CommunityLink>> = BTreeMap::new();
        for ((from, to), link) in acc {
            out.entry(from).or_default().push(CommunityLink {
                to,
                weight: link.weight,
                edges: link.edges,
                categories: by_count(link.categories),
            });
        }
        for links in out.values_mut() {
            links.sort_by(|a, b| b.weight.cmp(&a.weight).then(a.to.cmp(&b.to)));
            links.truncate(MAX_LINKS);
        }
        out
    }

    /// Per wanted community, the effect sinks its members reach: every
    /// weighted edge (any graph, cross edges too) from a member to a node the
    /// domain's `effect_sink` classifies with that edge's category, in scope
    /// or not.
    fn sinks(&self, wanted: &BTreeSet<u32>) -> BTreeMap<u32, BTreeMap<&'static str, usize>> {
        let tables = &CODE_PROFILE.tables;
        let mut member_of: HashMap<NodeId, u32> = HashMap::new();
        for &c in wanted {
            for &ix in &self.groups[c as usize] {
                member_of.insert(self.sp.node(ix).id, c);
            }
        }
        let sink_via: BTreeSet<u32> = tables
            .effect_sinks
            .iter()
            .flat_map(|s| s.via.iter().map(|c| c.0))
            .collect();
        let mut out: BTreeMap<u32, BTreeMap<&'static str, usize>> = BTreeMap::new();
        for e in self.merged.all_edges() {
            if !sink_via.contains(&e.category.0) || tables.community_weight(e.category) == 0 {
                continue;
            }
            let Some(&c) = member_of.get(&e.from) else {
                continue;
            };
            let Some(kind) = kind_of(self.merged, e.to) else {
                continue;
            };
            if let Some((_, sink)) = tables.effect_sink(kind, e.category) {
                *out.entry(c).or_default().entry(sink.class).or_default() += 1;
            }
        }
        out
    }
}

/// The kind of `id` in the first graph naming it.
fn kind_of(merged: &MergedGraph, id: NodeId) -> Option<NodeKindId> {
    merged
        .graphs
        .iter()
        .find_map(|g| g.nav.kind_by_id.get(&id).copied())
}

/// A histogram as `(key, count)`, count descending, then key.
fn by_count<K: Ord>(h: BTreeMap<K, usize>) -> Vec<(K, usize)> {
    let mut rows: Vec<(K, usize)> = h.into_iter().collect();
    // Stable, so equal counts keep the map's key order.
    rows.sort_by_key(|r| std::cmp::Reverse(r.1));
    rows
}

/// The longest `::`-segment prefix every qname shares, else the most
/// frequent first two segments (ties to the smaller string); empty for no
/// qname.
fn label_of(qnames: &[&str]) -> String {
    let split: Vec<Vec<&str>> = qnames.iter().map(|q| q.split("::").collect()).collect();
    let Some(first) = split.first() else {
        return String::new();
    };
    let mut common = first.len();
    for segs in &split[1..] {
        common = common.min(
            first
                .iter()
                .zip(segs)
                .take_while(|(a, b)| a == b && !a.is_empty())
                .count(),
        );
    }
    if common > 0 {
        return first[..common].join("::");
    }
    let mut heads: BTreeMap<String, usize> = BTreeMap::new();
    for segs in &split {
        *heads
            .entry(segs[..segs.len().min(2)].join("::"))
            .or_default() += 1;
    }
    by_count(heads)
        .into_iter()
        .next()
        .map(|(h, _)| h)
        .unwrap_or_default()
}

/// The query an absence names.
fn query(args: &CommunityArgs) -> String {
    let scope = args
        .scope
        .as_deref()
        .map(|s| format!(" scope={s}"))
        .unwrap_or_default();
    format!(
        "method={} seed={} resolution={} min_size={}{scope}",
        args.method.as_deref().unwrap_or("auto"),
        args.seed,
        args.resolution,
        args.min_size
    )
}

/// Why no community is listed: no weighted edge in the view (`no_edges`),
/// or none of `min_size` members (`no_match`).
fn no_communities(
    merged: &MergedGraph,
    args: &CommunityArgs,
    sp: &ScopedPartition<'_>,
    total: usize,
    min_size: usize,
) -> Absence {
    let scope = args
        .scope
        .as_deref()
        .map(|s| format!(" under scope `{s}`"))
        .unwrap_or_default();
    let mechanisms: Vec<&'static str> = CODE_PROFILE
        .tables
        .community_weights
        .iter()
        .map(|&(c, _)| edge_category::name(c))
        .collect();
    let (reason, note) = if sp.view.pair_count() == 0 {
        (
            "no_edges",
            format!(
                "no weighted edge joins two of the {} nodes{scope}, so there is no community to find",
                sp.nodes.len()
            ),
        )
    } else {
        (
            "no_match",
            format!("{total} communities{scope}, none with {min_size} or more members"),
        )
    };
    absence::empty(
        merged,
        PRIMITIVE,
        &query(args),
        reason,
        note,
        &mechanisms,
        None,
    )
}

/// The CD.1d fired_on line.
fn marker(a: &CommunitiesAnswer, weighted_edges: usize, levels: u32, args: &CommunityArgs) {
    let surface = if args.surface.is_empty() {
        SURFACE_ENGINE
    } else {
        args.surface
    };
    eprintln!(
        "[communities] method={} nodes={} weighted_edges={weighted_edges} communities={} listed={} isolated={} modularity={:.3} levels={levels} seed={} surface={surface}",
        a.method,
        a.nodes,
        a.total,
        a.communities.len(),
        a.isolated,
        a.modularity,
        a.seed,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_the_common_prefix_else_the_commonest_head() {
        assert_eq!(label_of(&["a::b::c", "a::b::d", "a::b"]), "a::b");
        assert_eq!(label_of(&["a::b::c"]), "a::b::c");
        assert_eq!(
            label_of(&["x::y::f", "z::w::g", "z::w::h", "x::y::k"]),
            "x::y"
        );
        assert_eq!(label_of(&["p::q", "r::s", "r::s::t"]), "r::s");
        assert_eq!(label_of(&[]), "");
    }

    #[test]
    fn resolution_is_held_in_thousandths() {
        assert_eq!(
            resolution_of(1.0),
            Resolution {
                num: 1000,
                den: 1000
            }
        );
        assert_eq!(resolution_of(0.25).num, 250);
        assert_eq!(resolution_of(0.0001).num, 1, "at least one thousandth");
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(resolution_of(bad).num, 1000, "{bad} reads as 1.0");
        }
    }

    #[test]
    fn method_names() {
        assert_eq!(method_choice(None), Some(MethodChoice::Auto));
        assert_eq!(method_choice(Some(" Leiden ")), Some(MethodChoice::Leiden));
        assert_eq!(
            method_choice(Some("lpa")),
            Some(MethodChoice::LabelPropagation)
        );
        assert_eq!(
            method_choice(Some("label_propagation")),
            Some(MethodChoice::LabelPropagation)
        );
        assert_eq!(method_choice(Some("louvain")), None);
    }

    #[test]
    fn histograms_order_by_count_then_key() {
        let h: BTreeMap<&str, usize> = [("b", 2), ("a", 2), ("c", 5)].into_iter().collect();
        assert_eq!(by_count(h), [("c", 5), ("a", 2), ("b", 2)]);
    }

    /// `partition_of` (CD.2b's seam) partitions the same view `communities`
    /// summarises, and `ids` maps dense indices back to node ids.
    #[test]
    fn partition_of_is_the_answer_view() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut a = String::new();
        let mut b = String::new();
        for i in 0..4 {
            a.push_str(&format!(
                "\ndef a{i}():\n    a{}()\n    a{}()\n\n",
                (i + 1) % 4,
                (i + 2) % 4
            ));
            b.push_str(&format!(
                "\ndef b{i}():\n    b{}()\n    b{}()\n\n",
                (i + 1) % 4,
                (i + 2) % 4
            ));
        }
        for (rel, src) in [("one/a.py", a), ("two/b.py", b)] {
            let p = tmp.path().join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            std::fs::write(p, src).expect("write");
        }
        let r = crate::generate_one(tmp.path().to_str().expect("utf-8")).expect("generate_one");
        let loc = Locator::new(&r.merged);
        let mut opts = CommunityOptions::default();
        opts.seed = DEFAULT_SEED;
        opts.resolution = resolution_of(DEFAULT_RESOLUTION);
        let sp = partition_of(&r.merged, &loc, &r.repo_labels, None, &opts, None);
        let comm_of = |q: &str| {
            let ix = (0..sp.view.len() as u32)
                .find(|&ix| loc.locate(sp.node(ix).id).qname == q)
                .unwrap_or_else(|| panic!("{q} in the view"));
            sp.partition.membership[ix as usize]
        };
        assert_eq!(comm_of("one::a::a0"), comm_of("one::a::a3"));
        assert_eq!(comm_of("two::b::b0"), comm_of("two::b::b2"));
        assert_ne!(comm_of("one::a::a0"), comm_of("two::b::b0"));
        let answer = communities(&r.merged, &r.repo_labels, &CommunityArgs::default());
        assert_eq!(answer.total, 2);
        assert!((answer.modularity - sp.partition.modularity).abs() < 1e-12);
        assert!(
            sp.edges
                .iter()
                .all(|e| CODE_PROFILE.tables.community_weight(e.category) > 0)
        );
    }
}
