//! **feature_flows** (LG.3a): LD.4b's entry flows grouped into per-feature
//! records — which entries form one feature, who calls them across services,
//! the located steps they reach with the confidence of each hop, and the data
//! sources those steps touch with the evidence tier behind each — plus the
//! deterministic writer of the `flows/<feature>.yaml` files the dogfood repos'
//! agents read ("start from the feature — load flows/<feature>.yaml"), the
//! surface `generate-repo-map.py` produced by regex.
//!
//! Module slot declared by L0.2, reached as
//! `repo_graph_engine::feature_flows::<item>`, never flattened into the crate
//! root. Named `feature_flows` because LD.4b's [`crate::trace::entry_flows`]
//! owns the word "flows" (the `glia flows` rows and the `[flows]` marker).
//!
//! # Entries
//!
//! A node is a flow entry when its kind is one of `CODE_PROFILE.tables.entry`'s
//! kinds (LD.6's reconciled set, the one liveness and LD.4b seed from; never a
//! second list), except COMPONENT. The rule's COMPONENT role and its `main` /
//! `test*` name rule are left out too: a flow describes an externally
//! triggered channel, and components show up as callers. Client navigation
//! ROUTEs are entries: a page whose route has no outgoing carry edge is listed
//! with empty steps, so the page inventory survives.
//!
//! # Feature keys ([`feature_key`])
//!
//! Read from [`channel_of`] (the literal a link travels over). ROUTE and
//! WS_HANDLER share one namespace: the first STATIC path segment, after one
//! leading `api` segment and any `v<digits>` segment are dropped, lower-cased;
//! placeholders (`:id`, `{id}`, `<id>`, `*`, `**`, `*any`, `[slug]`, `${...}`,
//! `(...)`) are not static, and no static segment left is `root`. So a nav
//! page `/orders`, the API routes `/api/orders/:id` and a `/orders/ws` socket
//! are one feature `orders` — what the script's per-feature files meant. Other
//! kinds are prefixed by mechanism: QUEUE_CONSUMER `queue-<topic>`,
//! EVENT_HANDLER `event-<channel>`, GRPC_SERVICE `grpc-<last segment of the
//! service>`, GRAPHQL_RESOLVER `graphql`, CLI_COMMAND `cli-<first word>`,
//! CRON_JOB `cron-<channel>`, anything else `<kind lower-cased>-<name>`.
//!
//! Every key is slugged to `[a-z0-9._-]`: other characters become `-`, runs of
//! `-` collapse, `-` and `.` are trimmed from both ends (no hidden file, no
//! `..`), and the key is cut to 64 bytes on a char boundary.
//!
//! [`FlowGrouping::Entry`] gives one record per entry instead, keyed by
//! LD.4b's `EntryFlow.key` (the entry's name lower-cased, spaces and hyphens
//! as `_`, the wrapper's spelling) through the same slug — `GET /api/orders`
//! is `get_-api-orders` — so `glia flows` rows and the files line up. Two
//! entries with one key get `<key>`, `<key>-2`, ... in (key, entry qname,
//! entry id) order: deterministic, never HashMap order.
//!
//! # Steps, callers, data sources
//!
//! - `steps`: the entry's forward BFS over `CODE_PROFILE.tables.carry_edges`
//!   within `depth` — the walk LD.4b's `entry_flows` makes, so an entry's
//!   step set is exactly its `EntryFlow` reach. One carry [`Adjacency`] per
//!   call serves every entry. Each step carries the category and confidence
//!   of the edge that first reached it, its kind, its service, whether that
//!   hop crossed a service, its depth and its 1-based location (one
//!   [`Locator`] per call). Ordered by (depth, qname, id).
//! - `callers`: the backward BFS over [`CALLER_CATEGORIES`] within
//!   `caller_depth` (one backward index per call): the ENDPOINT that
//!   HTTP_CALLS the route, the method that CALLS the endpoint, the component
//!   method that CALLS that. Same record, same order.
//! - `data_sources` (per feature): the targets of ACCESSES_DATA edges leaving
//!   a flow node (the entry or a step) — tier `FACT`, `via` `ACCESSES_DATA` —
//!   or leaving the MODULE that holds a flow node through DEFINES / CONTAINS
//!   (climbing a class to its file), where the cross-cutting data extractor
//!   anchors its edges today — tier `HEURISTIC` ("the file holding this step
//!   touches X"), `via` the structural edge the module holds it by. `from`
//!   names the node the ACCESSES_DATA edge leaves. Deduped by qname keeping
//!   the strongest tier (then the smaller `from`, `via`), sorted by qname.
//!   LE.4's function-level ACCESSES_DATA turns HEURISTIC rows into FACT rows
//!   with no change here.
//!
//! `service` follows LD.4b's `services` rule: under `ProjectRoots` keying (one
//! repo with manifest roots below its top) the `glia arch` service of the
//! node's file (else its qname's owner segment), otherwise the repo's label.
//! `cross_service` is LD.4b's too: the hop's ends sit in different repos, or
//! (`ProjectRoots`) in different services. The entry's own record has depth 0,
//! no `via`, confidence `strong`; `weakest` is the weakest confidence over the
//! entry's callers and steps (`strong` when it has none).
//!
//! # Files ([`render_flow_yaml`], [`write_feature_flows`])
//!
//! One `<dir>/<feature>.yaml` per record: a `# generated by glia ...` header,
//! `feature:`, `grouping:`, `services:`, then `entries:` and `data_sources:`
//! where every step and sink is ONE line holding the record as compact JSON —
//! a JSON object is a valid YAML flow mapping, so qnames holding `::`, `{id}`
//! or `#` need no quoting rules and the file greps one record per line. Plus
//! `<dir>/index.json`: `{generator, grouping, features: [{feature, file,
//! entries, callers, steps, data_sources, weakest}]}`.
//!
//! Writes go through a temp file and a rename, and a file whose bytes are
//! unchanged is not touched (its mtime stays, for watchers). A file is removed
//! only when the PREVIOUS `index.json` listed it and this write does not
//! produce it: a file glia never listed is never touched.
//!
//! The writer refuses (`InvalidInput`, naming the dir) a dir inside one of the
//! build's repos but not under that repo's `.glia/` (compared after
//! canonicalising the part that exists): every `.yaml` the walk reaches is
//! routed through the yaml sniffers (cron / config / iac / contracts) on the
//! next build, so flows written into a walked dir would feed back into the
//! graph. `.glia/` is hard-skipped by the walk (LF.1d); [`default_flows_dir`]
//! is `<repo>/.glia/graph/flows`, beside the layout. A dir outside every repo
//! is always allowed.
//!
//! # Markers
//!
//! fired_on, one line per [`feature_flows`] call (LD.4b owns `[flows]`):
//! `[feature-flows] features=<f> entries=<e> steps=<s> callers=<c> sinks=<d> (fact=<x> heuristic=<y>) grouping=<feature|entry>`
//! — grep `[feature-flows] features=`. [`write_feature_flows`] prints
//! `[feature-flows] wrote <dir> written=<w> unchanged=<u> removed=<r>`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io;
use std::path::{Component, Path, PathBuf};

use repo_graph_activation::algo::{Adjacency, CategorySet, Walk, reach};
use repo_graph_code_domain::endpoint::split_owner;
use repo_graph_code_domain::{edge_category as ec, node_kind as nk};
use repo_graph_core::{Confidence, EdgeCategoryId, NodeId, NodeKindId};
use repo_graph_graph::nav::nav_route_path;
use repo_graph_graph::{MergedGraph, channel_of};

use crate::VERSION_LINE;
use crate::answers::{Located, Locator, in_scope, resolve_scope};
use crate::arch::{ServiceKeying, default_keying, service_of};
use crate::persist::default_layout_dir;
use crate::profile::CODE_PROFILE;

/// `FlowOptions::default().depth`: the repo-graph wrapper's flow depth.
pub const DEFAULT_DEPTH: usize = 6;
/// `FlowOptions::default().caller_depth`.
pub const DEFAULT_CALLER_DEPTH: usize = 3;
/// The edge categories a caller walk follows backward from an entry: the
/// cross-service calls and flows, plus in-process calls and injection.
pub const CALLER_CATEGORIES: &[EdgeCategoryId] = &[
    ec::HTTP_CALLS,
    ec::GRPC_CALLS,
    ec::RPC_CALLS,
    ec::QUEUE_FLOWS,
    ec::EVENT_FLOWS,
    ec::WS_CONNECTS,
    ec::GRAPHQL_CALLS,
    ec::CLI_INVOKES,
    ec::CALLS,
    ec::INJECTS,
];
/// The index [`write_feature_flows`] writes beside the feature files.
pub const INDEX_FILE: &str = "index.json";

/// Longest feature key, in bytes.
const MAX_KEY_BYTES: usize = 64;
/// Containment hops climbed from a flow node to the MODULE holding it.
const MAX_HOLDER_HOPS: usize = 16;

/// How entries are grouped into records.
#[derive(serde::Serialize, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum FlowGrouping {
    /// One record per feature key ([`feature_key`]).
    #[default]
    Feature,
    /// One record per entry, keyed by LD.4b's `EntryFlow.key`.
    Entry,
}

impl FlowGrouping {
    /// `feature` / `entry`: the `grouping` value of the files and the marker.
    pub fn name(self) -> &'static str {
        match self {
            FlowGrouping::Feature => "feature",
            FlowGrouping::Entry => "entry",
        }
    }

    /// The grouping [`Self::name`] spells, case-insensitive.
    pub fn from_name(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "feature" => Some(FlowGrouping::Feature),
            "entry" => Some(FlowGrouping::Entry),
            _ => None,
        }
    }
}

/// What [`feature_flows`] groups and how far it walks. Start from `default()`
/// and use the `with_*` builders: `#[non_exhaustive]` rules out a struct
/// literal outside this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FlowOptions {
    pub grouping: FlowGrouping,
    /// Maximum hops of each entry's forward walk (`steps`).
    pub depth: usize,
    /// Maximum hops of each entry's backward walk (`callers`).
    pub caller_depth: usize,
    /// Keep only the record with this key (slugged like a key first).
    pub feature: Option<String>,
    /// Keep only entries whose file is under this path or project label (the
    /// A8.3 scope; an unlocatable entry is kept). Steps and callers are never
    /// scoped: a flow's value is that it crosses boundaries.
    pub scope: Option<String>,
}

impl Default for FlowOptions {
    fn default() -> Self {
        FlowOptions {
            grouping: FlowGrouping::Feature,
            depth: DEFAULT_DEPTH,
            caller_depth: DEFAULT_CALLER_DEPTH,
            feature: None,
            scope: None,
        }
    }
}

impl FlowOptions {
    pub fn with_grouping(mut self, grouping: FlowGrouping) -> Self {
        self.grouping = grouping;
        self
    }

    pub fn with_depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    pub fn with_caller_depth(mut self, caller_depth: usize) -> Self {
        self.caller_depth = caller_depth;
        self
    }

    pub fn with_feature(mut self, feature: impl Into<String>) -> Self {
        self.feature = Some(feature.into());
        self
    }

    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }
}

/// One feature: its entries, the services they touch and the data sources
/// their steps reach.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FeatureFlow {
    /// The record's key, `[a-z0-9._-]`: also its file name, `<feature>.yaml`.
    pub feature: String,
    /// Every service an entry, caller or step sits in, sorted.
    pub services: Vec<String>,
    /// Ordered by (entry qname, entry id).
    pub entries: Vec<FlowEntry>,
    /// Sorted by qname.
    pub data_sources: Vec<FlowSink>,
}

/// One entry point of a feature, with who calls it and what it reaches.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FlowEntry {
    /// The entry itself: depth 0, no `via`, confidence `strong`.
    pub entry: FlowStep,
    /// The weakest confidence over `callers` and `steps` (`strong` if none).
    pub weakest: &'static str,
    /// The backward walk over [`CALLER_CATEGORIES`], by (depth, qname).
    pub callers: Vec<FlowStep>,
    /// The forward walk over the carry edges, by (depth, qname).
    pub steps: Vec<FlowStep>,
}

/// One located node of a flow and the hop that reached it.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FlowStep {
    pub qname: String,
    pub kind: &'static str,
    /// The node's service (module doc); `None` when it cannot be placed.
    pub service: Option<String>,
    /// Category of the edge that first reached the node; `None` on the entry.
    pub via: Option<&'static str>,
    /// That edge's confidence: `strong`, `medium` or `weak`.
    pub confidence: &'static str,
    /// The hop's two ends sit in different services (module doc).
    pub cross_service: bool,
    /// Hops from the entry.
    pub depth: usize,
    pub file: Option<String>,
    /// 1-based (see [`Located`]).
    pub line: Option<i64>,
}

/// A data source a feature's steps reach, and on what evidence.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FlowSink {
    pub qname: String,
    pub kind: &'static str,
    /// `ACCESSES_DATA` for a FACT; for a HEURISTIC, the structural edge
    /// (`DEFINES` / `CONTAINS`) the module holds the step by.
    pub via: &'static str,
    /// `FACT` (a flow node's own edge) or `HEURISTIC` (its module's edge).
    pub tier: &'static str,
    /// Qname of the node the ACCESSES_DATA edge leaves.
    pub from: String,
}

/// What one [`write_feature_flows`] call did to the feature files (the index
/// is not counted).
#[derive(serde::Serialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct FlowsWritten {
    /// Files created or rewritten.
    pub written: usize,
    /// Files whose bytes were already right, left untouched.
    pub unchanged: usize,
    /// Files the previous index listed that this write no longer produces.
    pub removed: usize,
}

/// Where a repo's flow files live by default: `<repo>/.glia/graph/flows`, the
/// layout dir ([`default_layout_dir`]) joined with `flows`. Nothing is created.
pub fn default_flows_dir(repo: &Path) -> PathBuf {
    default_layout_dir(repo).join("flows")
}

/// **feature_flows** (LG.3a): every flow entry of `merged`, grouped into
/// records — see the module doc. `repo_labels` is the build's
/// (`GenerateResult::repo_labels`); services are named by it. Records are
/// sorted by key. Never an error: a graph with no entries gives no records.
pub fn feature_flows(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    opts: &FlowOptions,
) -> Vec<FeatureFlow> {
    let mut ctx = Ctx::new(merged, repo_labels);

    let mut entries = flow_entries(merged);
    if let Some(raw) = opts.scope.as_deref() {
        let scope = resolve_scope(merged, raw);
        entries.retain(|e| ctx.loc.file_of(e.id).is_none_or(|f| in_scope(&f, &scope)));
    }
    let wanted = opts.feature.as_deref().map(slug_key);
    let mut groups: BTreeMap<String, Vec<EntryNode<'_>>> = BTreeMap::new();
    for (key, e) in keyed(entries, opts.grouping) {
        if wanted.as_ref().is_none_or(|w| *w == key) {
            groups.entry(key).or_default().push(e);
        }
    }

    let carry = Adjacency::carry(merged, &CODE_PROFILE.tables);
    let callers_adj = Adjacency::build(merged, &CategorySet::of(CALLER_CATEGORIES));
    let mut flows: Vec<FeatureFlow> = Vec::with_capacity(groups.len());
    for (feature, mut members) in groups {
        members.sort_by(|a, b| a.qname.cmp(b.qname).then_with(|| a.id.0.cmp(&b.id.0)));
        let mut services: BTreeSet<String> = BTreeSet::new();
        // Entry and step ids: where the data sources are read from.
        let mut flow_nodes: Vec<NodeId> = Vec::new();
        let mut out: Vec<FlowEntry> = Vec::with_capacity(members.len());
        for e in &members {
            let entry = ctx.step(e.id, 0, None, Confidence::Strong, false);
            let (reached, steps): (Vec<NodeId>, Vec<FlowStep>) = ctx
                .walk(&carry, e.id, Walk::Forward, opts.depth)
                .into_iter()
                .unzip();
            let callers: Vec<FlowStep> = ctx
                .walk(&callers_adj, e.id, Walk::Backward, opts.caller_depth)
                .into_iter()
                .map(|(_, s)| s)
                .collect();
            let weakest = callers
                .iter()
                .chain(&steps)
                .map(|s| rank_of(s.confidence))
                .min()
                .map_or("strong", name_of_rank);
            services.extend(
                std::iter::once(&entry)
                    .chain(&callers)
                    .chain(&steps)
                    .filter_map(|s| s.service.clone()),
            );
            flow_nodes.push(e.id);
            flow_nodes.extend(reached);
            out.push(FlowEntry {
                entry,
                weakest,
                callers,
                steps,
            });
        }
        let data_sources = ctx.sinks(&flow_nodes);
        flows.push(FeatureFlow {
            feature,
            services: services.into_iter().collect(),
            entries: out,
            data_sources,
        });
    }

    let entries: usize = flows.iter().map(|f| f.entries.len()).sum();
    let steps: usize = flows
        .iter()
        .flat_map(|f| &f.entries)
        .map(|e| e.steps.len())
        .sum();
    let callers: usize = flows
        .iter()
        .flat_map(|f| &f.entries)
        .map(|e| e.callers.len())
        .sum();
    let sinks: Vec<&FlowSink> = flows.iter().flat_map(|f| &f.data_sources).collect();
    let fact = sinks.iter().filter(|s| s.tier == TIER_FACT).count();
    eprintln!(
        "[feature-flows] features={} entries={entries} steps={steps} callers={callers} sinks={} (fact={fact} heuristic={}) grouping={}",
        flows.len(),
        sinks.len(),
        sinks.len() - fact,
        opts.grouping.name()
    );
    flows
}

const TIER_FACT: &str = "FACT";
const TIER_HEURISTIC: &str = "HEURISTIC";

/// A feature key for an entry of `kind` — see the module doc's "Feature
/// keys". Pure: reads the qname and name only.
pub fn feature_key(kind: NodeKindId, qname: &str, name: &str) -> String {
    let channel = channel_of(qname, name);
    if kind == nk::ROUTE || kind == nk::WS_HANDLER {
        let path = nav_route_path(qname).unwrap_or(channel.as_str());
        return path_key(path);
    }
    let bare = strip_kind_prefix(&channel);
    if kind == nk::QUEUE_CONSUMER {
        return prefixed("queue", bare);
    }
    if kind == nk::EVENT_HANDLER {
        return prefixed("event", bare);
    }
    if kind == nk::GRPC_SERVICE {
        let last = bare
            .rsplit(['.', '/'])
            .find(|s| !s.is_empty())
            .unwrap_or(bare);
        return prefixed("grpc", last);
    }
    if kind == nk::GRAPHQL_RESOLVER {
        return "graphql".to_string();
    }
    if kind == nk::CLI_COMMAND {
        return prefixed("cli", bare.split_whitespace().next().unwrap_or(""));
    }
    if kind == nk::CRON_JOB {
        return prefixed("cron", bare);
    }
    let kind_name = nk::name(kind).to_ascii_lowercase();
    let label = if name.is_empty() { bare } else { name };
    prefixed(&kind_name, label)
}

/// The YAML file of one record — see the module doc's "Files".
pub fn render_flow_yaml(flow: &FeatureFlow, grouping: FlowGrouping) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# generated by glia {VERSION_LINE} - glia flows; do not edit"
    );
    let _ = writeln!(out, "feature: {}", json(&flow.feature));
    let _ = writeln!(out, "grouping: {}", grouping.name());
    let _ = writeln!(out, "services: {}", json(&flow.services));
    if flow.entries.is_empty() {
        out.push_str("entries: []\n");
    } else {
        out.push_str("entries:\n");
        for e in &flow.entries {
            let _ = writeln!(out, "  - entry: {}", json(&e.entry));
            let _ = writeln!(out, "    weakest: {}", e.weakest);
            yaml_list(&mut out, "    ", "callers", &e.callers);
            yaml_list(&mut out, "    ", "steps", &e.steps);
        }
    }
    yaml_list(&mut out, "", "data_sources", &flow.data_sources);
    out
}

/// Write `flows` to `dir` as `<feature>.yaml` files plus [`INDEX_FILE`] — see
/// the module doc's "Files". `repo_roots` are the build's repos, for the
/// walked-dir refusal. An `InvalidInput` error, with nothing written, for a
/// refused dir, a feature that is not a plain `[a-z0-9._-]` file stem, or two
/// records with one feature.
pub fn write_feature_flows(
    repo_roots: &[&Path],
    dir: &Path,
    flows: &[FeatureFlow],
    grouping: FlowGrouping,
) -> io::Result<FlowsWritten> {
    refuse_walked(repo_roots, dir)?;
    let mut produced: BTreeSet<String> = BTreeSet::new();
    for f in flows {
        if !is_key(&f.feature) {
            return Err(invalid(format!(
                "feature flows: `{}` is not a file-safe feature key; nothing written to {}",
                f.feature,
                dir.display()
            )));
        }
        if !produced.insert(format!("{}.yaml", f.feature)) {
            return Err(invalid(format!(
                "feature flows: feature `{}` appears twice; nothing written to {}",
                f.feature,
                dir.display()
            )));
        }
    }
    std::fs::create_dir_all(dir)?;
    let previous = listed_files(dir);

    let mut stats = FlowsWritten::default();
    let mut rows: Vec<IndexRow<'_>> = Vec::with_capacity(flows.len());
    for f in flows {
        let file = format!("{}.yaml", f.feature);
        if write_if_changed(dir, &file, render_flow_yaml(f, grouping).as_bytes())? {
            stats.written += 1;
        } else {
            stats.unchanged += 1;
        }
        let entries = &f.entries;
        rows.push(IndexRow {
            feature: &f.feature,
            file,
            entries: entries.len(),
            callers: entries.iter().map(|e| e.callers.len()).sum(),
            steps: entries.iter().map(|e| e.steps.len()).sum(),
            data_sources: f.data_sources.len(),
            weakest: entries
                .iter()
                .map(|e| rank_of(e.weakest))
                .min()
                .map_or("strong", name_of_rank),
        });
    }
    for old in previous.difference(&produced) {
        match std::fs::remove_file(dir.join(old)) {
            Ok(()) => stats.removed += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let index = IndexFile {
        generator: format!("glia {VERSION_LINE}"),
        grouping: grouping.name(),
        features: rows,
    };
    let mut text = serde_json::to_string_pretty(&index).map_err(io::Error::other)?;
    text.push('\n');
    write_if_changed(dir, INDEX_FILE, text.as_bytes())?;
    eprintln!(
        "[feature-flows] wrote {} written={} unchanged={} removed={}",
        dir.display(),
        stats.written,
        stats.unchanged,
        stats.removed
    );
    Ok(stats)
}

// ============================================================================
// entries and keys
// ============================================================================

/// One flow entry: the node, its kind, name and qname.
struct EntryNode<'a> {
    id: NodeId,
    kind: NodeKindId,
    name: &'a str,
    qname: &'a str,
}

/// Every flow entry of `merged` (module doc), in graph then node order, each
/// id once.
fn flow_entries(merged: &MergedGraph) -> Vec<EntryNode<'_>> {
    let rule = &CODE_PROFILE.tables.entry;
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut out: Vec<EntryNode<'_>> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let Some(kind) = g.nav.kind_by_id.get(&n.id).copied() else {
                continue;
            };
            if kind == nk::COMPONENT || !rule.kinds.contains(&kind) || !seen.insert(n.id) {
                continue;
            }
            out.push(EntryNode {
                id: n.id,
                kind,
                name: text_of(&g.nav.name_by_id, n.id),
                qname: text_of(&g.nav.qname_by_id, n.id),
            });
        }
    }
    out
}

/// `id`'s entry in a nav name / qname map, `""` when it has none.
fn text_of(m: &HashMap<NodeId, String>, id: NodeId) -> &str {
    m.get(&id).map(String::as_str).unwrap_or("")
}

/// Each entry with its record key under `grouping`.
fn keyed(entries: Vec<EntryNode<'_>>, grouping: FlowGrouping) -> Vec<(String, EntryNode<'_>)> {
    match grouping {
        FlowGrouping::Feature => entries
            .into_iter()
            .map(|e| (feature_key(e.kind, e.qname, e.name), e))
            .collect(),
        FlowGrouping::Entry => {
            let mut based: Vec<(String, EntryNode<'_>)> = entries
                .into_iter()
                .map(|e| {
                    let base = slug_key(&entry_flow_key(e.name));
                    (
                        if base.is_empty() {
                            "root".to_string()
                        } else {
                            base
                        },
                        e,
                    )
                })
                .collect();
            based.sort_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| a.1.qname.cmp(b.1.qname))
                    .then_with(|| a.1.id.0.cmp(&b.1.id.0))
            });
            let mut used: HashSet<String> = HashSet::new();
            based
                .into_iter()
                .map(|(base, e)| {
                    let mut key = base.clone();
                    let mut n = 1;
                    while used.contains(&key) {
                        n += 1;
                        key = format!("{base}-{n}");
                    }
                    used.insert(key.clone());
                    (key, e)
                })
                .collect()
        }
    }
}

/// LD.4b's `EntryFlow.key` for an entry called `name`: lower-cased, spaces
/// and hyphens as `_` (`trace::slug`, crate-private there; the
/// `entry_grouping_gives_one_record_per_entry` test pins the two together
/// through the files' keys).
fn entry_flow_key(name: &str) -> String {
    name.to_lowercase().replace([' ', '-'], "_")
}

/// A ROUTE / WS_HANDLER path's key: its first static segment (module doc).
fn path_key(raw: &str) -> String {
    let path = strip_kind_prefix(raw);
    // A legacy `<METHOD> <path>` display name, when the qname was not one.
    let path = match path.split_once(' ') {
        Some((m, rest)) if !m.is_empty() && m.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => path,
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    let mut dropped_api = false;
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        let seg = seg.to_lowercase();
        if is_placeholder(&seg) || is_version(&seg) {
            continue;
        }
        if seg == "api" && !dropped_api {
            dropped_api = true;
            continue;
        }
        let key = slug_key(&seg);
        if !key.is_empty() {
            return key;
        }
    }
    "root".to_string()
}

/// A path segment that is a parameter, not a name.
fn is_placeholder(seg: &str) -> bool {
    seg.starts_with([':', '{', '<', '*', '[', '$', '('])
}

/// `v<digits>`.
fn is_version(seg: &str) -> bool {
    seg.strip_prefix('v')
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// The owner segment and a kind's own qname prefix, off a channel literal
/// [`channel_of`] fell back to the qname for.
fn strip_kind_prefix(s: &str) -> &str {
    let s = split_owner(s).0;
    for p in ["page:", "route:", "ws:", "event_handle:", "cli:"] {
        if let Some(rest) = s.strip_prefix(p) {
            return rest;
        }
    }
    s
}

/// `<prefix>-<slug of rest>`, or the bare prefix when `rest` slugs to nothing.
fn prefixed(prefix: &str, rest: &str) -> String {
    let rest = slug_key(rest);
    if rest.is_empty() {
        slug_key(prefix)
    } else {
        slug_key(&format!("{prefix}-{rest}"))
    }
}

/// Slug to `[a-z0-9._-]`: lower-cased, other characters as `-`, runs of `-`
/// collapsed, `-` / `.` trimmed from both ends, cut to [`MAX_KEY_BYTES`] on a
/// char boundary (then trimmed again).
fn slug_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
            c
        } else {
            '-'
        };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    let mut end = out.len().min(MAX_KEY_BYTES);
    while !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out.trim_matches(['-', '.']).to_string()
}

/// A key [`write_feature_flows`] accepts as a file stem: non-empty,
/// `[a-z0-9._-]`, not starting with `.` (never hidden, never `..`).
fn is_key(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

// ============================================================================
// walking and locating
// ============================================================================

fn rank_of(confidence: &str) -> u8 {
    match confidence {
        "weak" => 0,
        "medium" => 1,
        _ => 2,
    }
}

fn name_of_rank(rank: u8) -> &'static str {
    match rank {
        0 => "weak",
        1 => "medium",
        _ => "strong",
    }
}

fn confidence_name(c: Confidence) -> &'static str {
    match c {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    }
}

/// The per-call indexes every record reads, each built once in O(V + E).
struct Ctx<'a> {
    loc: Locator<'a>,
    labels: &'a BTreeMap<u64, String>,
    repo_of: HashMap<NodeId, u64>,
    kind_of: HashMap<NodeId, NodeKindId>,
    keying: ServiceKeying,
    /// `service_of` keys with no repo-label prefix: one repo, so only
    /// equality and the bare key matter.
    no_labels: BTreeMap<u64, String>,
    /// `(from, to, category)` -> the confidence of the first such edge in
    /// global edge order: the edge a BFS walks.
    confidence: HashMap<(NodeId, NodeId, EdgeCategoryId), Confidence>,
    /// ACCESSES_DATA targets per source, in edge order.
    access: HashMap<NodeId, Vec<NodeId>>,
    /// The first DEFINES / CONTAINS parent of each node, in edge order.
    holder: HashMap<NodeId, (NodeId, EdgeCategoryId)>,
    located: HashMap<NodeId, Located>,
    service_key: HashMap<NodeId, Option<String>>,
}

impl<'a> Ctx<'a> {
    fn new(merged: &'a MergedGraph, labels: &'a BTreeMap<u64, String>) -> Self {
        let mut repo_of: HashMap<NodeId, u64> = HashMap::new();
        let mut kind_of: HashMap<NodeId, NodeKindId> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                repo_of.insert(n.id, g.repo.0);
            }
            for (id, k) in &g.nav.kind_by_id {
                kind_of.entry(*id).or_insert(*k);
            }
        }
        let walked = CategorySet::of(CODE_PROFILE.tables.carry_edges);
        let callers = CategorySet::of(CALLER_CATEGORIES);
        let mut confidence = HashMap::new();
        let mut access: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut holder = HashMap::new();
        for e in merged.all_edges() {
            if walked.contains(e.category) || callers.contains(e.category) {
                confidence
                    .entry((e.from, e.to, e.category))
                    .or_insert(e.confidence);
            }
            if e.category == ec::ACCESSES_DATA {
                access.entry(e.from).or_default().push(e.to);
            } else if e.category == ec::DEFINES || e.category == ec::CONTAINS {
                holder.entry(e.to).or_insert((e.from, e.category));
            }
        }
        Ctx {
            loc: Locator::new(merged),
            labels,
            repo_of,
            kind_of,
            keying: default_keying(merged),
            no_labels: BTreeMap::new(),
            confidence,
            access,
            holder,
            located: HashMap::new(),
            service_key: HashMap::new(),
        }
    }

    fn located(&mut self, id: NodeId) -> &Located {
        let loc = &self.loc;
        self.located.entry(id).or_insert_with(|| loc.locate(id))
    }

    /// One walk from `seed` over `adj`, as `(node, record)` ordered by
    /// (depth, qname, id). Forward, a reached node's edge runs
    /// `parent -> node`; backward, `node -> parent`.
    fn walk(
        &mut self,
        adj: &Adjacency,
        seed: NodeId,
        walk: Walk,
        depth: usize,
    ) -> Vec<(NodeId, FlowStep)> {
        let mut rows: Vec<(NodeId, FlowStep)> = reach::bfs(adj, &[seed], walk, depth)
            .reached
            .iter()
            .map(|r| {
                let edge = match walk {
                    Walk::Backward => (r.id, r.parent, r.via),
                    _ => (r.parent, r.id, r.via),
                };
                let conf = self
                    .confidence
                    .get(&edge)
                    .copied()
                    .unwrap_or(Confidence::Weak);
                let cross = self.cross(r.parent, r.id);
                (r.id, self.step(r.id, r.depth, Some(r.via), conf, cross))
            })
            .collect();
        rows.sort_by(|(ia, a), (ib, b)| {
            a.depth
                .cmp(&b.depth)
                .then_with(|| a.qname.cmp(&b.qname))
                .then_with(|| ia.0.cmp(&ib.0))
        });
        rows
    }

    fn step(
        &mut self,
        id: NodeId,
        depth: usize,
        via: Option<EdgeCategoryId>,
        conf: Confidence,
        cross_service: bool,
    ) -> FlowStep {
        let service = self.service_label(id);
        let at = self.located(id).clone();
        FlowStep {
            qname: at.qname,
            kind: at.kind,
            service,
            via: via.map(ec::name),
            confidence: confidence_name(conf),
            cross_service,
            depth,
            file: at.file,
            line: at.line,
        }
    }

    /// LD.4b's `services` label (module doc).
    fn service_label(&mut self, id: NodeId) -> Option<String> {
        let repo = *self.repo_of.get(&id)?;
        if matches!(self.keying, ServiceKeying::ProjectRoots(_)) {
            return self.service_key(id);
        }
        Some(service_of("", repo, &ServiceKeying::PerRepo, self.labels))
    }

    /// The `glia arch` service of `id` under `ProjectRoots` keying: its
    /// located file, else its qname's owner segment.
    fn service_key(&mut self, id: NodeId) -> Option<String> {
        if let Some(k) = self.service_key.get(&id) {
            return k.clone();
        }
        let file = match self.loc.file_of(id) {
            Some(f) => Some(f),
            None => split_owner(&self.located(id).qname).1.map(str::to_string),
        };
        let repo = self.repo_of.get(&id).copied().unwrap_or_default();
        let key = file.map(|f| service_of(&f, repo, &self.keying, &self.no_labels));
        self.service_key.insert(id, key.clone());
        key
    }

    /// LD.4b's `cross_service` for a hop between `a` and `b` (module doc).
    fn cross(&mut self, a: NodeId, b: NodeId) -> bool {
        if self.repo_of.get(&a) != self.repo_of.get(&b) {
            return true;
        }
        if !matches!(self.keying, ServiceKeying::ProjectRoots(_)) {
            return false;
        }
        matches!((self.service_key(a), self.service_key(b)), (Some(x), Some(y)) if x != y)
    }

    /// The MODULE holding `id` through DEFINES / CONTAINS, and the category
    /// of the edge it holds the next node down by. `None` for a MODULE (its
    /// own edges are facts) and for a node no module holds.
    fn module_of(&self, id: NodeId) -> Option<(NodeId, EdgeCategoryId)> {
        if self.kind_of.get(&id) == Some(&nk::MODULE) {
            return None;
        }
        let mut cur = id;
        for _ in 0..MAX_HOLDER_HOPS {
            let &(parent, via) = self.holder.get(&cur)?;
            if self.kind_of.get(&parent) == Some(&nk::MODULE) {
                return Some((parent, via));
            }
            cur = parent;
        }
        None
    }

    /// The data sources of one feature's flow nodes (module doc).
    fn sinks(&mut self, flow_nodes: &[NodeId]) -> Vec<FlowSink> {
        // (source, holder category) per candidate: `None` = FACT.
        let mut cands: Vec<(NodeId, NodeId, Option<EdgeCategoryId>)> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        for &n in flow_nodes {
            if !seen.insert(n) {
                continue;
            }
            for &t in self.access.get(&n).into_iter().flatten() {
                cands.push((t, n, None));
            }
            if let Some((m, via)) = self.module_of(n) {
                for &t in self.access.get(&m).into_iter().flatten() {
                    cands.push((t, m, Some(via)));
                }
            }
        }
        let mut best: BTreeMap<String, FlowSink> = BTreeMap::new();
        for (target, from, held) in cands {
            let at = self.located(target).clone();
            let from_q = self.located(from).qname.clone();
            let sink = FlowSink {
                qname: at.qname,
                kind: at.kind,
                via: held.map_or(ec::name(ec::ACCESSES_DATA), ec::name),
                tier: if held.is_none() {
                    TIER_FACT
                } else {
                    TIER_HEURISTIC
                },
                from: from_q,
            };
            let order = |s: &FlowSink| (s.tier != TIER_FACT, s.from.clone(), s.via);
            match best.get(&sink.qname) {
                Some(have) if order(have) <= order(&sink) => {}
                _ => {
                    best.insert(sink.qname.clone(), sink);
                }
            }
        }
        best.into_values().collect()
    }
}

// ============================================================================
// files
// ============================================================================

fn json<T: serde::Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())
}

/// `<indent><key>: []`, or `<indent><key>:` and one `<indent>  - <json>`
/// line per item.
fn yaml_list<T: serde::Serialize>(out: &mut String, indent: &str, key: &str, items: &[T]) {
    if items.is_empty() {
        let _ = writeln!(out, "{indent}{key}: []");
        return;
    }
    let _ = writeln!(out, "{indent}{key}:");
    for item in items {
        let _ = writeln!(out, "{indent}  - {}", json(item));
    }
}

/// `index.json`, field order = output order.
#[derive(serde::Serialize)]
struct IndexFile<'a> {
    generator: String,
    grouping: &'static str,
    features: Vec<IndexRow<'a>>,
}

#[derive(serde::Serialize)]
struct IndexRow<'a> {
    feature: &'a str,
    file: String,
    entries: usize,
    callers: usize,
    steps: usize,
    data_sources: usize,
    weakest: &'static str,
}

/// The feature files the index in `dir` lists: plain `<key>.yaml` names only,
/// so a hand-edited index can never point the prune outside `dir`. No index,
/// or one that does not parse, lists nothing.
fn listed_files(dir: &Path) -> BTreeSet<String> {
    let Ok(bytes) = std::fs::read(dir.join(INDEX_FILE)) else {
        return BTreeSet::new();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return BTreeSet::new();
    };
    v.get("features")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("file").and_then(serde_json::Value::as_str))
        .filter(|f| f.strip_suffix(".yaml").is_some_and(is_key))
        .map(str::to_string)
        .collect()
}

/// Write `bytes` to `dir/name` through a temp file and a rename, unless the
/// file already holds exactly them. `true` when it wrote.
fn write_if_changed(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<bool> {
    let path = dir.join(name);
    if std::fs::read(&path).is_ok_and(|have| have == bytes) {
        return Ok(false);
    }
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(true)
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// Refuse a `dir` inside a repo root but outside that root's `.glia/` (module
/// doc).
fn refuse_walked(repo_roots: &[&Path], dir: &Path) -> io::Result<()> {
    let out = lenient_canonical(dir)?;
    for root in repo_roots {
        let root_c = lenient_canonical(root)?;
        if out.starts_with(&root_c) && !out.starts_with(root_c.join(".glia")) {
            return Err(invalid(format!(
                "feature flows: refusing to write to {}: it is inside the repo {} and not under its .glia/, so the next build would walk the files as sources; use {} or a dir outside the repo",
                dir.display(),
                root.display(),
                default_flows_dir(root).display()
            )));
        }
    }
    Ok(())
}

/// `p` made absolute, its longest existing prefix canonicalised (symlinks
/// resolved), the rest applied lexically (`..` pops, `.` is dropped).
fn lenient_canonical(p: &Path) -> io::Result<PathBuf> {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    let comps: Vec<Component<'_>> = abs.components().collect();
    for split in (1..=comps.len()).rev() {
        let prefix: PathBuf = comps[..split].iter().collect();
        let Ok(mut out) = prefix.canonicalize() else {
            continue;
        };
        for c in &comps[split..] {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::Normal(s) => out.push(s),
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            }
        }
        return Ok(out);
    }
    Ok(abs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_builders() {
        let o = FlowOptions::default();
        assert_eq!(
            (
                o.grouping,
                o.depth,
                o.caller_depth,
                o.feature.as_deref(),
                o.scope.as_deref()
            ),
            (FlowGrouping::Feature, 6, 3, None, None)
        );
        let o = o
            .with_grouping(FlowGrouping::Entry)
            .with_depth(2)
            .with_caller_depth(1)
            .with_feature("orders")
            .with_scope("web");
        assert_eq!(
            (
                o.grouping,
                o.depth,
                o.caller_depth,
                o.feature.as_deref(),
                o.scope.as_deref()
            ),
            (FlowGrouping::Entry, 2, 1, Some("orders"), Some("web"))
        );
        assert_eq!(
            FlowGrouping::from_name(" Entry "),
            Some(FlowGrouping::Entry)
        );
        assert_eq!(
            FlowGrouping::from_name("feature"),
            Some(FlowGrouping::Feature)
        );
        assert_eq!(FlowGrouping::from_name("flows"), None);
    }

    #[test]
    fn slug_rules() {
        assert_eq!(slug_key("GET /api/Orders"), "get-api-orders");
        assert_eq!(slug_key("..hidden"), "hidden");
        assert_eq!(slug_key("a  ::  b"), "a-b");
        assert_eq!(slug_key("~~~"), "");
        assert_eq!(entry_flow_key("POST /orders"), "post_/orders");
        assert_eq!(slug_key(&entry_flow_key("POST /orders")), "post_-orders");
        let long = "x".repeat(100);
        assert_eq!(slug_key(&long).len(), MAX_KEY_BYTES);
        assert!(is_key("orders") && is_key("queue-orders.created"));
        assert!(!is_key("") && !is_key(".x") && !is_key("..") && !is_key("a/b") && !is_key("A"));
        assert!(is_version("v2") && !is_version("v") && !is_version("vx1"));
    }

    #[test]
    fn lenient_canonical_resolves_the_missing_tail_lexically() {
        let td = tempfile::tempdir().expect("tempdir");
        let base = td.path().canonicalize().expect("canonical tempdir");
        let got = lenient_canonical(&td.path().join("a/../b/./c")).expect("canonical");
        assert_eq!(got, base.join("b").join("c"));
    }
}
