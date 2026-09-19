//! A5.8 — anchor RPC-family marker nodes to the code that owns them.
//!
//! The cross-cutting extractors mint marker nodes (GRPC_CLIENT, WS_*, EVENT_*,
//! GRAPHQL_*, and — LA.31 — tRPC's RPC_CALL / RPC_PROCEDURE) from a needle in
//! the file text. `CodeNav::record` only writes the nav parent, so until this
//! pass those markers were graph islands: no structural edge touched them and
//! no POSITION cell located them. A trace from the function that builds a gRPC
//! stub could not cross into the gRPC hop, and `locate_node` / `glia arch`
//! could not place the marker in a file.
//!
//! Each extractor now reports an [`Anchor`] (marker id + 0-indexed line) for
//! the needle that minted the node. [`attach`] then:
//!
//! - gives the marker a one-line POSITION cell at its first anchor;
//! - emits an owner edge to the innermost METHOD / FUNCTION whose POSITION span
//!   holds an anchor ([`owner_edge`]): the method USES an outbound marker, and
//!   an inbound marker is HANDLED_BY the method. Both are blast carry edges, so
//!   `caller -CALLS-> FetchUser -USES-> grpc_client:X -GRPC_CALLS-> grpc:X`
//!   is one traversable chain;
//! - when no anchor of a marker lies inside a function (module-level
//!   `const ws = new WebSocket(...)`), falls back to a CONTAINS edge from the
//!   module. CONTAINS is structural, not a carry edge, so blast results do not
//!   widen.
//!
//! Anchor density per extractor: the gRPC client passes, the type-keyed event
//! needles and tRPC (procedure keys and call sites) anchor EVERY site (their
//! name is read from that site); the string-keyed needles anchor only the site
//! that minted the node, because the name they read is not per-site (e.g. a
//! GraphQL operation name is the file's first `gql` tag, whichever
//! `useQuery(` line matched).
//!
//! Parsers extract, the graph crate resolves: this pass reads only the file's
//! own parse (spans the language parser already attached), so its output is a
//! function of the file's content and is safe to cache with the `FileParse`.
//! Other marker families reuse this module rather than grow a parallel helper:
//! GRPC_SERVER (A5.3) finds its implementing type through [`build_span_index`]
//! and anchors through [`attach`].
//!
//! LE.4c: the queue markers (A2.8's QUEUE_PRODUCER / QUEUE_CONSUMER) are
//! marker kinds too. The queue extractor already ties every topic node to its
//! MODULE with CONTAINS (structural, so the publishing file does not fan the
//! blast radius out) and already gives it a POSITION, so [`attach`] keeps both
//! and only ADDS the owner edge: the function holding `producer.send(..)`
//! USES the producer node, and the consumer node is HANDLED_BY the function
//! holding `consumer.subscribe(..)`. Every queue call site anchors, so a topic
//! published from two functions of one file gives both a USES edge. A
//! consumer registered inside a setup function is HANDLED_BY that function,
//! not the callback it passes (`consumer.run({ eachMessage })`): binding the
//! callback needs reference extraction.
//!
//! LE.4a: the same owner index re-homes EDGES, not only markers.
//! [`rehome_to_owner`] takes a module-anchored edge category (the data
//! entities' ACCESSES_DATA; LE.4b's READS_CONFIG) plus the statement [`Site`]s
//! behind it, and moves each `module -> target` edge to the innermost
//! METHOD / FUNCTION holding its sites, with an ACCESS_MODE edge cell folded
//! from the sites' verbs. A target with a site at module scope, or one a
//! declaration names, keeps its module edge. [`access_census`] and
//! [`report_access`] give the build-level `[data-access]` marker.

use std::collections::{BTreeMap, HashMap, HashSet};

use repo_graph_code_domain::evidence::{self, Evidence};
use repo_graph_code_domain::{CodeNav, FileParse, cell_type, edge_category, node_kind};
use repo_graph_core::{
    Cell, CellPayload, Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId,
};

/// Where a marker node's needle actually fired, 0-indexed like every other
/// POSITION in the repo (`repo_graph_doc::position_json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchor {
    pub node: NodeId,
    pub line: u32,
}

/// Marker kinds a method reaches OUT through: the method USES the marker.
/// RPC_CALL (LA.31) is the tRPC hook / vanilla call, the gRPC-client shape.
/// QUEUE_PRODUCER (LE.4c) is the function that publishes to a topic.
const OUTBOUND: &[NodeKindId] = &[
    node_kind::GRPC_CLIENT,
    node_kind::WS_CLIENT,
    node_kind::EVENT_EMITTER,
    node_kind::GRAPHQL_OPERATION,
    node_kind::RPC_CALL,
    node_kind::QUEUE_PRODUCER,
];

/// Marker kinds that are an INBOUND contract: the marker is HANDLED_BY the
/// method, mirroring the HTTP side (`ROUTE --HANDLED_BY--> handler`).
/// RPC_PROCEDURE (LA.31) is a tRPC procedure key; one declared in a
/// module-level router has no enclosing function and takes the module
/// CONTAINS fallback. QUEUE_CONSUMER (LE.4c) is HANDLED_BY the function that
/// subscribes (or, for an annotation listener, the annotated method).
const INBOUND: &[NodeKindId] = &[
    node_kind::WS_HANDLER,
    node_kind::EVENT_HANDLER,
    node_kind::GRAPHQL_RESOLVER,
    node_kind::GRPC_SERVER,
    node_kind::RPC_PROCEDURE,
    node_kind::QUEUE_CONSUMER,
];

/// LE.4c: the queue markers, counted apart in [`AnchorStats`] so the
/// `[marker-anchor]` line shows the queue half fired.
fn is_queue_kind(kind: NodeKindId) -> bool {
    kind == node_kind::QUEUE_PRODUCER || kind == node_kind::QUEUE_CONSUMER
}

/// True for every kind [`attach`] anchors. RPC_CALL / RPC_PROCEDURE are shared
/// with LA.17's Connect / Twirp nodes, which are grafted post-cache with their
/// own HANDLED_BY / USES / CONTAINS edges in the directions [`owner_edge`]
/// uses, so [`census`] classifies them as anchored too. The queue kinds
/// (LE.4c) always carry A2.8's module CONTAINS, so a queue node no function
/// encloses counts as anchored to the module.
pub fn is_marker_kind(kind: NodeKindId) -> bool {
    OUTBOUND.contains(&kind) || INBOUND.contains(&kind)
}

/// 0-indexed line of `byte_offset` in `source`: the number of `\n` before it,
/// which is the index `source.lines().enumerate()` and tree-sitter rows both
/// use. An offset past the end clamps to the last line.
pub fn line_of(source: &str, byte_offset: usize) -> u32 {
    let end = byte_offset.min(source.len());
    let newlines = source.as_bytes()[..end]
        .iter()
        .filter(|&&b| b == b'\n')
        .count();
    u32::try_from(newlines).unwrap_or(u32::MAX)
}

/// `(start_line, end_line, id)` for every METHOD / FUNCTION in a file, read
/// from the POSITION cells the language parser already attached. Innermost
/// wins: sorted by (start desc, end asc, id asc), so the first span that holds
/// a line is the most deeply nested one, and ties never depend on node order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OwnerIndex(Vec<(u32, u32, NodeId)>);

#[derive(serde::Deserialize)]
struct Span {
    start_line: u32,
    end_line: u32,
}

/// The first POSITION span on `node`, if it parses.
pub fn position_span(node: &Node) -> Option<(u32, u32)> {
    node.cells
        .iter()
        .filter(|c| c.kind == cell_type::POSITION)
        .find_map(|c| {
            let raw = match &c.payload {
                CellPayload::Json(s) | CellPayload::Text(s) => s,
                CellPayload::Bytes(_) => return None,
            };
            let span: Span = serde_json::from_str(raw).ok()?;
            (span.start_line <= span.end_line).then_some((span.start_line, span.end_line))
        })
}

pub fn build_owner_index(nodes: &[Node], nav: &CodeNav) -> OwnerIndex {
    build_span_index(nodes, nav, &[node_kind::METHOD, node_kind::FUNCTION])
}

/// [`build_owner_index`] over any set of node kinds, with the same
/// innermost-wins order, so [`owner_of_line`] answers "which `kinds` node
/// encloses this line". A5.3 asks it for the CLASS / STRUCT that declares a
/// gRPC base type, rather than growing a second span helper.
pub fn build_span_index(nodes: &[Node], nav: &CodeNav, kinds: &[NodeKindId]) -> OwnerIndex {
    let mut spans: Vec<(u32, u32, NodeId)> = nodes
        .iter()
        .filter(|n| nav.kind_by_id.get(&n.id).is_some_and(|k| kinds.contains(k)))
        .filter_map(|n| position_span(n).map(|(s, e)| (s, e, n.id)))
        .collect();
    spans.sort_unstable_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.0.cmp(&b.2.0))
    });
    spans.dedup();
    OwnerIndex(spans)
}

/// The innermost METHOD / FUNCTION whose span holds `line`.
pub fn owner_of_line(idx: &OwnerIndex, line: u32) -> Option<NodeId> {
    idx.0
        .iter()
        .find(|(start, end, _)| *start <= line && line <= *end)
        .map(|(_, _, id)| *id)
}

/// POSITION cell for a marker node: a one-line span at the needle, in the
/// canonical `{"file","start_line","end_line"}` key order.
pub fn position_cell(file_rel: &str, line: u32) -> Cell {
    let file = serde_json::to_string(file_rel).unwrap_or_else(|_| "\"\"".to_string());
    Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(format!(
            r#"{{"file":{file},"start_line":{line},"end_line":{line}}}"#
        )),
    }
}

/// The edge that ties `marker` to the method that owns it, by marker kind:
/// outbound kinds give `owner --USES--> marker`, inbound kinds give
/// `marker --HANDLED_BY--> owner`. `None` for any other kind.
pub fn owner_edge(marker_kind: NodeKindId, marker: NodeId, owner: NodeId) -> Option<Edge> {
    let (from, to, category) = if OUTBOUND.contains(&marker_kind) {
        (owner, marker, edge_category::USES)
    } else if INBOUND.contains(&marker_kind) {
        (marker, owner, edge_category::HANDLED_BY)
    } else {
        return None;
    };
    Some(Edge {
        from,
        to,
        category,
        confidence: Confidence::Medium,
        cells: Vec::new(),
    })
}

fn is_owner_edge(e: &Edge, marker: NodeId) -> bool {
    (e.category == edge_category::USES && e.to == marker)
        || (e.category == edge_category::HANDLED_BY && e.from == marker)
}

/// How the markers of one file (or one `attach` call) ended up.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AnchorStats {
    /// Markers with at least one owner edge to a METHOD / FUNCTION.
    pub to_method: usize,
    /// Markers no function encloses, tied to the module by CONTAINS.
    pub to_module: usize,
    /// Markers left floating: no owner edge, no module edge.
    pub unanchored: usize,
    /// LE.4c: the QUEUE_PRODUCER / QUEUE_CONSUMER share of `to_method`.
    pub queue_to_method: usize,
    /// LE.4c: the QUEUE_PRODUCER / QUEUE_CONSUMER share of `to_module`.
    pub queue_to_module: usize,
}

impl AnchorStats {
    pub fn add(&mut self, other: AnchorStats) {
        self.to_method += other.to_method;
        self.to_module += other.to_module;
        self.unanchored += other.unanchored;
        self.queue_to_method += other.queue_to_method;
        self.queue_to_module += other.queue_to_module;
    }

    /// One marker of `kind` anchored to a METHOD / FUNCTION.
    fn count_method(&mut self, kind: NodeKindId) {
        self.to_method += 1;
        if is_queue_kind(kind) {
            self.queue_to_method += 1;
        }
    }

    /// One marker of `kind` anchored to its MODULE.
    fn count_module(&mut self, kind: NodeKindId) {
        self.to_module += 1;
        if is_queue_kind(kind) {
            self.queue_to_module += 1;
        }
    }

    pub fn total(&self) -> usize {
        self.to_method + self.to_module + self.unanchored
    }
}

/// Attach POSITION to every anchored marker and emit its owner edge, or the
/// module CONTAINS fallback when none of its anchors lies inside a function.
///
/// Sorts `anchors` by `(line, node id)` first: the emitted edge order must not
/// depend on extractor iteration order, or the byte-identical store gate sees
/// flapping bytes. May be called more than once on one parse (the engine's
/// post-cache gRPC client pass does); a marker that already carries a
/// POSITION keeps it, and an edge already present is not emitted twice.
///
/// The returned stats cover the markers named by `anchors`; an anchor whose
/// node is not a marker in `fp` counts as `unanchored`.
pub fn attach(
    fp: &mut FileParse,
    path: &str,
    module_id: NodeId,
    anchors: &mut Vec<Anchor>,
) -> AnchorStats {
    let mut stats = AnchorStats::default();
    if anchors.is_empty() {
        return stats;
    }
    anchors.sort_unstable_by_key(|a| (a.line, a.node.0));
    anchors.dedup();

    let idx = build_owner_index(&fp.nodes, &fp.nav);
    let mut existing: HashSet<(NodeId, NodeId, EdgeCategoryId)> = fp
        .edges
        .iter()
        .map(|e| (e.from, e.to, e.category))
        .collect();
    let mut push_edge = |fp: &mut FileParse, e: Edge| {
        if existing.insert((e.from, e.to, e.category)) {
            fp.edges.push(e);
        }
    };

    // Markers in first-anchor order (the anchors are line-sorted, so this is
    // also each marker's lowest line).
    let mut order: Vec<(NodeId, u32, NodeKindId)> = Vec::new();
    let mut owned: HashMap<NodeId, bool> = HashMap::new();
    let mut bogus: HashSet<NodeId> = HashSet::new();
    for a in anchors.iter() {
        let Some(kind) = fp.nav.kind_by_id.get(&a.node).copied() else {
            bogus.insert(a.node);
            continue;
        };
        if !is_marker_kind(kind) {
            bogus.insert(a.node);
            continue;
        }
        let has_owner = owned.entry(a.node).or_insert_with(|| {
            order.push((a.node, a.line, kind));
            false
        });
        if let Some(owner) = owner_of_line(&idx, a.line)
            && owner != a.node
            && let Some(edge) = owner_edge(kind, a.node, owner)
        {
            *has_owner = true;
            push_edge(fp, edge);
        }
    }

    for (marker, line, kind) in order {
        if let Some(node) = fp.nodes.iter_mut().find(|n| n.id == marker)
            && !node.cells.iter().any(|c| c.kind == cell_type::POSITION)
        {
            node.cells.push(position_cell(path, line));
        }
        let has_owner = owned.get(&marker).copied().unwrap_or(false)
            || fp.edges.iter().any(|e| is_owner_edge(e, marker));
        if has_owner {
            stats.count_method(kind);
        } else {
            push_edge(
                fp,
                Edge {
                    from: module_id,
                    to: marker,
                    category: edge_category::CONTAINS,
                    confidence: Confidence::Medium,
                    cells: Vec::new(),
                },
            );
            stats.count_module(kind);
        }
    }
    stats.unanchored += bogus.len();
    stats
}

/// Classify every marker node in `fp` by the edges the parse now carries:
/// owner edge → `to_method`, CONTAINS from a MODULE → `to_module`, neither →
/// `unanchored`. Reads the finished parse rather than `attach`'s return value,
/// so a cache-served `FileParse` counts exactly like a fresh one.
pub fn census(fp: &FileParse) -> AnchorStats {
    let mut stats = AnchorStats::default();
    let mut owner_linked: HashSet<NodeId> = HashSet::new();
    let mut module_linked: HashSet<NodeId> = HashSet::new();
    for e in &fp.edges {
        match e.category {
            c if c == edge_category::USES => {
                owner_linked.insert(e.to);
            }
            c if c == edge_category::HANDLED_BY => {
                owner_linked.insert(e.from);
            }
            c if c == edge_category::CONTAINS
                && fp.nav.kind_by_id.get(&e.from) == Some(&node_kind::MODULE) =>
            {
                module_linked.insert(e.to);
            }
            _ => {}
        }
    }
    let mut seen: HashSet<NodeId> = HashSet::new();
    for n in &fp.nodes {
        let Some(kind) = fp.nav.kind_by_id.get(&n.id).copied() else {
            continue;
        };
        if !is_marker_kind(kind) || !seen.insert(n.id) {
            continue;
        }
        if owner_linked.contains(&n.id) {
            stats.count_method(kind);
        } else if module_linked.contains(&n.id) {
            stats.count_module(kind);
        } else {
            stats.unanchored += 1;
        }
    }
    stats
}

/// The fired_on marker line for `stats`, or `None` when the repo holds no
/// marker node:
///   `[marker-anchor] {a} anchored to methods, {m} to module, {u} unanchored repo=<label>`
/// LE.4c: when a queue marker was anchored, ` (queue: {q} to methods, {qm} to
/// module)` goes before ` repo=` (grep token `(queue: `); a repo without queue
/// nodes prints the line unchanged.
pub fn report_line(stats: AnchorStats, repo_label: &str) -> Option<String> {
    if stats.total() == 0 {
        return None;
    }
    let queue = if stats.queue_to_method + stats.queue_to_module > 0 {
        format!(
            " (queue: {} to methods, {} to module)",
            stats.queue_to_method, stats.queue_to_module
        )
    } else {
        String::new()
    };
    Some(format!(
        "[marker-anchor] {} anchored to methods, {} to module, {} unanchored{queue} repo={repo_label}",
        stats.to_method, stats.to_module, stats.unanchored
    ))
}

/// Print [`report_line`], once per repo that holds a marker node.
pub fn report(stats: AnchorStats, repo_label: &str) {
    if let Some(line) = report_line(stats, repo_label) {
        eprintln!("{line}");
    }
}

/// LE.4a: one statement site of a `module -> target` edge: the 0-indexed
/// line that names `target`, and the single-site mode (`"read"` /
/// `"write"`, `None` when the statement does not say).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    pub target: NodeId,
    pub line: u32,
    pub mode: Option<&'static str>,
}

/// LE.4a: how the data-access edges of one file (or one repo) ended up.
/// Returned by [`rehome_to_owner`] for the edges it touched, and recounted
/// from a finished parse by [`access_census`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AccessStats {
    /// Function / method edges with an access site (re-homed, or a parser's
    /// own edge that took a mode).
    pub to_fn: usize,
    /// Module edges kept: a site at module scope, or a declaration.
    pub module_kept: usize,
    /// ... of `to_fn`, by ACCESS_MODE.
    pub read: usize,
    pub write: usize,
    pub read_write: usize,
    /// ... of `to_fn`, with no ACCESS_MODE (every site's verb unknown).
    pub unknown: usize,
}

impl AccessStats {
    pub fn add(&mut self, other: AccessStats) {
        self.to_fn += other.to_fn;
        self.module_kept += other.module_kept;
        self.read += other.read;
        self.write += other.write;
        self.read_write += other.read_write;
        self.unknown += other.unknown;
    }

    /// One function edge whose ACCESS_MODE is `mode`.
    fn count_fn(&mut self, mode: Option<&str>) {
        self.to_fn += 1;
        match mode {
            Some("read") => self.read += 1,
            Some("write") => self.write += 1,
            Some("read_write") => self.read_write += 1,
            _ => self.unknown += 1,
        }
    }
}

/// Fold two ACCESS_MODE values: equal stays, read + write is `read_write`,
/// `None` (unknown) adds nothing.
fn fold_mode(a: Option<&'static str>, b: Option<&'static str>) -> Option<&'static str> {
    match (a, b) {
        (None, m) | (m, None) => m,
        (Some(x), Some(y)) if x == y => Some(x),
        _ => Some("read_write"),
    }
}

/// The ACCESS_MODE text an edge carries, as its `'static` spelling.
fn mode_of(e: &Edge) -> Option<&'static str> {
    let cell = e.cell(cell_type::ACCESS_MODE)?;
    let (CellPayload::Text(t) | CellPayload::Json(t)) = &cell.payload else {
        return None;
    };
    ["read", "write", "read_write"]
        .into_iter()
        .find(|m| *m == t.as_str())
}

/// Put `mode` on `e` as its one ACCESS_MODE cell, folded with the one it
/// already carries (a second call on one parse never downgrades it).
fn set_mode(e: &mut Edge, mode: &'static str) {
    let folded = fold_mode(mode_of(e), Some(mode)).unwrap_or(mode);
    let cell = Cell {
        kind: cell_type::ACCESS_MODE,
        payload: CellPayload::Text(folded.to_string()),
    };
    match e
        .cells
        .iter()
        .position(|c| c.kind == cell_type::ACCESS_MODE)
    {
        Some(i) => e.cells[i] = cell,
        None => e.cells.push(cell),
    }
}

/// LE.4a: re-home the `module_id -> target` edges of `category` to the code
/// that holds their statement sites.
///
/// Per target (sites grouped by target id, so output never depends on site
/// order): every site is mapped to the innermost METHOD / FUNCTION whose
/// POSITION span holds its line ([`owner_of_line`]). When EVERY site has an
/// owner and `target` is not in `keep_module` (a declaration named it), the
/// module edge is removed; otherwise it stays, untouched. Each owner gets one
/// edge `owner -> target` whose ACCESS_MODE folds its sites' modes (`read` +
/// `write` = `read_write`; only unknown sites give no cell). When the parse
/// already holds that edge (a language parser's own function-level edge:
/// Go GORM, Python SQLAlchemy, Ruby ActiveRecord), the mode cell goes on it
/// and it keeps its parser evidence; otherwise a new edge is appended with
/// EVIDENCE `emitter` at `path` and the owner's first site line (basis
/// `site`). New edges are appended sorted by `(from, to)`.
///
/// The site lines come from the same file as the spans, so the result is a
/// function of the file's content and is safe to cache with the parse.
/// Generic over the category on purpose: LE.4b re-homes READS_CONFIG here.
pub fn rehome_to_owner(
    fp: &mut FileParse,
    path: &str,
    module_id: NodeId,
    category: EdgeCategoryId,
    sites: &[Site],
    keep_module: &[NodeId],
    emitter: &str,
) -> AccessStats {
    let mut stats = AccessStats::default();
    if sites.is_empty() {
        return stats;
    }
    let idx = build_owner_index(&fp.nodes, &fp.nav);
    // First edge of `category` per (from, to): lookups only, never iterated,
    // so one pass over the edges serves every site. Indices stay valid: new
    // edges wait in `fresh` and the module edges are dropped last.
    let mut at: HashMap<(NodeId, NodeId), usize> = HashMap::new();
    for (i, e) in fp.edges.iter().enumerate() {
        if e.category == category {
            at.entry((e.from, e.to)).or_insert(i);
        }
    }
    let mut by_target: BTreeMap<u64, (NodeId, Vec<&Site>)> = BTreeMap::new();
    for site in sites {
        by_target
            .entry(site.target.0)
            .or_insert_with(|| (site.target, Vec::new()))
            .1
            .push(site);
    }

    let mut drop_module: HashSet<NodeId> = HashSet::new();
    let mut fresh: Vec<Edge> = Vec::new();
    for (target, group) in by_target.into_values() {
        // owner id -> (owner, folded mode, first site line)
        let mut owners: BTreeMap<u64, (NodeId, Option<&'static str>, u32)> = BTreeMap::new();
        let mut at_module = false;
        for site in &group {
            match owner_of_line(&idx, site.line).filter(|o| *o != target) {
                Some(owner) => {
                    let slot = owners
                        .entry(owner.0)
                        .or_insert((owner, site.mode, site.line));
                    slot.1 = fold_mode(slot.1, site.mode);
                    slot.2 = slot.2.min(site.line);
                }
                None => at_module = true,
            }
        }
        if at.contains_key(&(module_id, target)) {
            if at_module || owners.is_empty() || keep_module.contains(&target) {
                stats.module_kept += 1;
            } else {
                drop_module.insert(target);
            }
        }
        for (owner, mode, line) in owners.into_values() {
            match at.get(&(owner, target)).and_then(|&i| fp.edges.get_mut(i)) {
                Some(e) => {
                    if let Some(m) = mode {
                        set_mode(e, m);
                    }
                    stats.count_fn(mode_of(e));
                }
                None => {
                    let mut e = Edge::new(owner, target, category, Confidence::Medium);
                    if let Some(m) = mode {
                        set_mode(&mut e, m);
                    }
                    evidence::attach(&mut e, Evidence::emitter(emitter).at(path, line));
                    stats.count_fn(mode);
                    fresh.push(e);
                }
            }
        }
    }
    if !drop_module.is_empty() {
        fp.edges.retain(|e| {
            !(e.from == module_id && e.category == category && drop_module.contains(&e.to))
        });
    }
    fresh.sort_by_key(|e| (e.from.0, e.to.0));
    fp.edges.extend(fresh);
    stats
}

/// The emitter an edge's EVIDENCE names, if any.
fn emitter_of(e: &Edge) -> Option<String> {
    Evidence::of(e).map(|ev| ev.emitter)
}

/// LE.4a: recount the data-access edges of a finished parse, so a
/// cache-served `FileParse` counts exactly like a fresh one (the
/// [`census`] pattern). Counted: ACCESSES_DATA edges into a DATA_ENTITY
/// that the `extractor:data_entities` stage emitted, plus parser edges that
/// took an ACCESS_MODE. A FUNCTION / METHOD source is `to_fn` (by its mode),
/// a MODULE source `module_kept`. Provider buckets (`data_source:*`) are not
/// DATA_ENTITY nodes and never count.
pub fn access_census(fp: &FileParse) -> AccessStats {
    let mut stats = AccessStats::default();
    for e in &fp.edges {
        if e.category != edge_category::ACCESSES_DATA
            || fp.nav.kind_by_id.get(&e.to) != Some(&node_kind::DATA_ENTITY)
        {
            continue;
        }
        let mode = mode_of(e);
        if mode.is_none() && emitter_of(e).as_deref() != Some(DATA_ENTITIES_EMITTER) {
            continue;
        }
        match fp.nav.kind_by_id.get(&e.from) {
            Some(k) if *k == node_kind::FUNCTION || *k == node_kind::METHOD => {
                stats.count_fn(mode);
            }
            Some(k) if *k == node_kind::MODULE => stats.module_kept += 1,
            _ => {}
        }
    }
    stats
}

/// The evidence emitter of the data-entities extractor's edges: its module
/// edges (stamped by the engine's `run_with_edges!`) and the edges
/// [`rehome_to_owner`] adds for it.
pub const DATA_ENTITIES_EMITTER: &str = "extractor:data_entities";

/// The LE.4a fired_on marker line for `stats`, or `None` when the repo holds
/// no counted data-access edge:
///   `[data-access] rehomed fn={F} module_kept={M} modes read={R} write={W} read_write={X} unknown={U} repo=<label>`
pub fn report_access_line(stats: AccessStats, repo_label: &str) -> Option<String> {
    if stats.to_fn + stats.module_kept == 0 {
        return None;
    }
    Some(format!(
        "[data-access] rehomed fn={} module_kept={} modes read={} write={} read_write={} unknown={} repo={repo_label}",
        stats.to_fn, stats.module_kept, stats.read, stats.write, stats.read_write, stats.unknown
    ))
}

/// Print [`report_access_line`], once per repo that holds a data-access edge.
pub fn report_access(stats: AccessStats, repo_label: &str) {
    if let Some(line) = report_access_line(stats, repo_label) {
        eprintln!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::GRAPH_TYPE;
    use repo_graph_core::RepoId;

    fn repo() -> RepoId {
        RepoId(7)
    }

    fn id(kind: NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn node(id: NodeId, cells: Vec<Cell>) -> Node {
        Node {
            id,
            repo: repo(),
            confidence: Confidence::Strong,
            cells,
        }
    }

    fn span_cell(start: u32, end: u32) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"a.ts","start_line":{start},"end_line":{end}}}"#
            )),
        }
    }

    /// `(kind, qname, POSITION span)` for one node of a test parse.
    type Entry<'a> = (NodeKindId, &'a str, Option<(u32, u32)>);

    /// A parse with a MODULE plus the given entries.
    fn parse_with(entries: &[Entry<'_>]) -> (FileParse, NodeId) {
        let mut fp = FileParse::default();
        let module = id(node_kind::MODULE, "a");
        fp.nodes.push(node(module, vec![span_cell(0, 100)]));
        fp.nav.record(module, "a", "a", node_kind::MODULE, None);
        for (kind, qname, span) in entries {
            let nid = id(*kind, qname);
            let cells = span.map(|(s, e)| vec![span_cell(s, e)]).unwrap_or_default();
            fp.nodes.push(node(nid, cells));
            fp.nav.record(nid, qname, qname, *kind, Some(module));
        }
        (fp, module)
    }

    fn position_of(fp: &FileParse, nid: NodeId) -> Option<String> {
        fp.nodes
            .iter()
            .find(|n| n.id == nid)?
            .cells
            .iter()
            .find(|c| c.kind == cell_type::POSITION)
            .and_then(|c| match &c.payload {
                CellPayload::Json(s) => Some(s.clone()),
                _ => None,
            })
    }

    #[test]
    fn line_of_counts_newlines_before_the_offset() {
        let src = "a\nbb\nccc";
        assert_eq!(line_of(src, 0), 0);
        assert_eq!(line_of(src, 2), 1);
        assert_eq!(line_of(src, 5), 2);
        assert_eq!(line_of(src, 999), 2, "past the end clamps to the last line");
        let lines: Vec<_> = src.lines().collect();
        assert_eq!(lines[line_of(src, 6) as usize], "ccc");
    }

    #[test]
    fn owner_index_picks_innermost_span() {
        let (fp, _) = parse_with(&[
            (node_kind::FUNCTION, "outer", Some((2, 20))),
            (node_kind::FUNCTION, "inner", Some((5, 9))),
            (node_kind::METHOD, "sibling", Some((12, 14))),
            // Not an owner kind: a CLASS spanning everything never wins.
            (node_kind::CLASS, "Klass", Some((1, 30))),
            // No POSITION: cannot own anything.
            (node_kind::FUNCTION, "unlocated", None),
        ]);
        let idx = build_owner_index(&fp.nodes, &fp.nav);
        assert_eq!(
            owner_of_line(&idx, 6),
            Some(id(node_kind::FUNCTION, "inner"))
        );
        assert_eq!(
            owner_of_line(&idx, 5),
            Some(id(node_kind::FUNCTION, "inner"))
        );
        assert_eq!(
            owner_of_line(&idx, 3),
            Some(id(node_kind::FUNCTION, "outer"))
        );
        assert_eq!(
            owner_of_line(&idx, 13),
            Some(id(node_kind::METHOD, "sibling"))
        );
        assert_eq!(
            owner_of_line(&idx, 20),
            Some(id(node_kind::FUNCTION, "outer"))
        );
        assert_eq!(
            owner_of_line(&idx, 25),
            None,
            "a class body line has no owner"
        );
        assert_eq!(owner_of_line(&idx, 0), None);
    }

    #[test]
    fn owner_index_breaks_identical_span_ties_by_id() {
        let (mut fp, _) = parse_with(&[
            (node_kind::FUNCTION, "b", Some((3, 3))),
            (node_kind::FUNCTION, "a", Some((3, 3))),
        ]);
        let first = owner_of_line(&build_owner_index(&fp.nodes, &fp.nav), 3);
        fp.nodes.reverse();
        let second = owner_of_line(&build_owner_index(&fp.nodes, &fp.nav), 3);
        assert_eq!(first, second, "node order must not change the owner");
    }

    #[test]
    fn owner_edge_direction_by_kind() {
        let owner = id(node_kind::METHOD, "m");
        for kind in [
            node_kind::GRPC_CLIENT,
            node_kind::WS_CLIENT,
            node_kind::EVENT_EMITTER,
            node_kind::GRAPHQL_OPERATION,
            node_kind::QUEUE_PRODUCER,
        ] {
            let marker = id(kind, "x");
            let e = owner_edge(kind, marker, owner).expect("outbound kinds anchor");
            assert_eq!(
                (e.from, e.to, e.category),
                (owner, marker, edge_category::USES)
            );
            assert_eq!(e.confidence, Confidence::Medium);
        }
        for kind in [
            node_kind::WS_HANDLER,
            node_kind::EVENT_HANDLER,
            node_kind::GRAPHQL_RESOLVER,
            node_kind::GRPC_SERVER,
            node_kind::QUEUE_CONSUMER,
        ] {
            let marker = id(kind, "x");
            let e = owner_edge(kind, marker, owner).expect("inbound kinds anchor");
            assert_eq!(
                (e.from, e.to, e.category),
                (marker, owner, edge_category::HANDLED_BY)
            );
        }
        for kind in [
            node_kind::CRON_JOB,
            node_kind::ROUTE,
            node_kind::METHOD,
            node_kind::GRPC_SERVICE,
        ] {
            assert!(owner_edge(kind, id(kind, "x"), owner).is_none(), "{kind:?}");
        }
    }

    #[test]
    fn rpc_kinds_are_marker_kinds() {
        assert!(is_marker_kind(node_kind::RPC_CALL));
        assert!(is_marker_kind(node_kind::RPC_PROCEDURE));
        let owner = id(node_kind::FUNCTION, "Users");
        let call = id(node_kind::RPC_CALL, "rpc_call:user.list");
        let e = owner_edge(node_kind::RPC_CALL, call, owner).expect("RPC_CALL is outbound");
        assert_eq!(
            (e.from, e.to, e.category),
            (owner, call, edge_category::USES),
            "the calling function USES the call"
        );
        let procedure = id(node_kind::RPC_PROCEDURE, "rpc:user.list");
        let e = owner_edge(node_kind::RPC_PROCEDURE, procedure, owner)
            .expect("RPC_PROCEDURE is inbound");
        assert_eq!(
            (e.from, e.to, e.category),
            (procedure, owner, edge_category::HANDLED_BY),
            "the procedure is HANDLED_BY its enclosing function"
        );
    }

    #[test]
    fn marker_inside_a_function_gets_position_and_owner_edge() {
        let (mut fp, module) = parse_with(&[
            (node_kind::FUNCTION, "FetchUser", Some((10, 23))),
            (node_kind::GRPC_CLIENT, "grpc_client:UserService", None),
        ]);
        let client = id(node_kind::GRPC_CLIENT, "grpc_client:UserService");
        let mut anchors = vec![Anchor {
            node: client,
            line: 17,
        }];
        let stats = attach(&mut fp, "client/main.go", module, &mut anchors);
        assert_eq!(
            stats,
            AnchorStats {
                to_method: 1,
                to_module: 0,
                unanchored: 0,
                ..AnchorStats::default()
            }
        );
        assert_eq!(
            position_of(&fp, client).as_deref(),
            Some(r#"{"file":"client/main.go","start_line":17,"end_line":17}"#)
        );
        assert_eq!(
            fp.edges,
            vec![Edge {
                from: id(node_kind::FUNCTION, "FetchUser"),
                to: client,
                category: edge_category::USES,
                confidence: Confidence::Medium,
                cells: Vec::new(),
            }]
        );
        assert_eq!(census(&fp), stats);
    }

    #[test]
    fn module_level_marker_falls_back_to_contains() {
        let (mut fp, module) = parse_with(&[
            (node_kind::FUNCTION, "ServeWs", Some((14, 27))),
            (node_kind::WS_HANDLER, "ws:ws", None),
        ]);
        let handler = id(node_kind::WS_HANDLER, "ws:ws");
        let mut anchors = vec![Anchor {
            node: handler,
            line: 8,
        }];
        let stats = attach(&mut fp, "hub.go", module, &mut anchors);
        assert_eq!(
            stats,
            AnchorStats {
                to_method: 0,
                to_module: 1,
                unanchored: 0,
                ..AnchorStats::default()
            }
        );
        assert_eq!(
            fp.edges,
            vec![Edge {
                from: module,
                to: handler,
                category: edge_category::CONTAINS,
                confidence: Confidence::Medium,
                cells: Vec::new(),
            }]
        );
        assert!(position_of(&fp, handler).is_some_and(|p| p.contains("\"start_line\":8")));
        assert_eq!(census(&fp), stats);
    }

    #[test]
    fn module_fallback_only_when_no_anchor_has_an_owner() {
        let (mut fp, module) = parse_with(&[
            (node_kind::FUNCTION, "a", Some((10, 12))),
            (node_kind::FUNCTION, "b", Some((20, 22))),
            (node_kind::GRPC_CLIENT, "grpc_client:X", None),
        ]);
        let client = id(node_kind::GRPC_CLIENT, "grpc_client:X");
        // A module-level site at line 2, plus a site in each function.
        let mut anchors = vec![
            Anchor {
                node: client,
                line: 21,
            },
            Anchor {
                node: client,
                line: 2,
            },
            Anchor {
                node: client,
                line: 11,
            },
        ];
        let stats = attach(&mut fp, "x.go", module, &mut anchors);
        assert_eq!(stats.to_method, 1);
        assert_eq!(stats.to_module, 0);
        let uses: Vec<NodeId> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::USES)
            .map(|e| e.from)
            .collect();
        assert_eq!(
            uses,
            vec![id(node_kind::FUNCTION, "a"), id(node_kind::FUNCTION, "b")],
            "one USES per owning function, in line order"
        );
        assert!(
            !fp.edges
                .iter()
                .any(|e| e.category == edge_category::CONTAINS)
        );
        // POSITION is the lowest anchored line.
        assert!(position_of(&fp, client).is_some_and(|p| p.contains("\"start_line\":2,")));
    }

    #[test]
    fn attach_is_idempotent_and_keeps_an_existing_position() {
        let (mut fp, module) = parse_with(&[
            (node_kind::FUNCTION, "f", Some((1, 5))),
            (node_kind::EVENT_EMITTER, "event_emit:x", Some((3, 3))),
        ]);
        let marker = id(node_kind::EVENT_EMITTER, "event_emit:x");
        let mut anchors = vec![Anchor {
            node: marker,
            line: 4,
        }];
        attach(&mut fp, "a.ts", module, &mut anchors.clone());
        attach(&mut fp, "a.ts", module, &mut anchors);
        assert_eq!(
            fp.edges.len(),
            1,
            "no duplicate owner edge on a second call"
        );
        let positions = fp.nodes.iter().find(|n| n.id == marker).map(|n| {
            n.cells
                .iter()
                .filter(|c| c.kind == cell_type::POSITION)
                .count()
        });
        assert_eq!(positions, Some(1));
        assert!(position_of(&fp, marker).is_some_and(|p| p.contains("\"start_line\":3")));
    }

    #[test]
    fn non_marker_anchor_is_ignored_and_counted() {
        let (mut fp, module) = parse_with(&[(node_kind::FUNCTION, "f", Some((1, 5)))]);
        let f = id(node_kind::FUNCTION, "f");
        let ghost = id(node_kind::WS_CLIENT, "ws_client:nowhere");
        let mut anchors = vec![
            Anchor { node: f, line: 2 },
            Anchor {
                node: ghost,
                line: 2,
            },
        ];
        let stats = attach(&mut fp, "a.ts", module, &mut anchors);
        assert_eq!(
            stats,
            AnchorStats {
                to_method: 0,
                to_module: 0,
                unanchored: 2,
                ..AnchorStats::default()
            }
        );
        assert!(fp.edges.is_empty());
        assert!(
            fp.nodes
                .iter()
                .find(|n| n.id == f)
                .is_some_and(|n| n.cells.len() == 1)
        );
    }

    #[test]
    fn census_counts_a_marker_nobody_anchored() {
        let (fp, _) = parse_with(&[(node_kind::GRAPHQL_RESOLVER, "graphql_resolver:Q", None)]);
        assert_eq!(
            census(&fp),
            AnchorStats {
                to_method: 0,
                to_module: 0,
                unanchored: 1,
                ..AnchorStats::default()
            }
        );
    }

    #[test]
    fn attach_edge_order_is_independent_of_anchor_order() {
        let build = |order: &[usize]| {
            let (mut fp, module) = parse_with(&[
                (node_kind::FUNCTION, "f1", Some((1, 3))),
                (node_kind::FUNCTION, "f2", Some((5, 7))),
                (node_kind::METHOD, "m3", Some((9, 11))),
                (node_kind::WS_CLIENT, "ws_client:/a", None),
                (node_kind::EVENT_HANDLER, "event_handle:b", None),
                (node_kind::GRAPHQL_OPERATION, "graphql_op:c", None),
                (node_kind::EVENT_EMITTER, "event_emit:d", None),
            ]);
            let all = [
                Anchor {
                    node: id(node_kind::WS_CLIENT, "ws_client:/a"),
                    line: 2,
                },
                Anchor {
                    node: id(node_kind::EVENT_HANDLER, "event_handle:b"),
                    line: 6,
                },
                Anchor {
                    node: id(node_kind::GRAPHQL_OPERATION, "graphql_op:c"),
                    line: 10,
                },
                Anchor {
                    node: id(node_kind::EVENT_EMITTER, "event_emit:d"),
                    line: 20,
                },
                Anchor {
                    node: id(node_kind::WS_CLIENT, "ws_client:/a"),
                    line: 6,
                },
            ];
            let mut anchors: Vec<Anchor> = order.iter().map(|&i| all[i]).collect();
            attach(&mut fp, "a.ts", module, &mut anchors);
            fp
        };
        let a = build(&[0, 1, 2, 3, 4]);
        let b = build(&[4, 3, 2, 1, 0]);
        let c = build(&[2, 0, 4, 1, 3]);
        assert_eq!(a.edges, b.edges);
        assert_eq!(a.edges, c.edges);
        assert_eq!(a.nodes, b.nodes);
        assert_eq!(a.nodes, c.nodes);
        assert_eq!(a.edges.len(), 5, "4 owner edges + 1 module fallback");
    }

    /// LE.4c: a queue parse as the queue extractor leaves it — each topic
    /// node already carries its A2.8 POSITION and module CONTAINS.
    fn queue_parse() -> (FileParse, NodeId, NodeId, NodeId) {
        let (mut fp, module) = parse_with(&[
            (node_kind::FUNCTION, "publishOrder", Some((6, 11))),
            (node_kind::FUNCTION, "listen", Some((13, 20))),
            (
                node_kind::QUEUE_PRODUCER,
                "queue_producer:orders",
                Some((7, 7)),
            ),
            (
                node_kind::QUEUE_CONSUMER,
                "queue_consumer:payments",
                Some((14, 14)),
            ),
        ]);
        let producer = id(node_kind::QUEUE_PRODUCER, "queue_producer:orders");
        let consumer = id(node_kind::QUEUE_CONSUMER, "queue_consumer:payments");
        for to in [consumer, producer] {
            fp.edges.push(Edge {
                from: module,
                to,
                category: edge_category::CONTAINS,
                confidence: Confidence::Medium,
                cells: Vec::new(),
            });
        }
        (fp, module, producer, consumer)
    }

    #[test]
    fn queue_markers_gain_owner_edges_and_keep_contains_and_position() {
        assert!(is_marker_kind(node_kind::QUEUE_PRODUCER));
        assert!(is_marker_kind(node_kind::QUEUE_CONSUMER));
        let (mut fp, module, producer, consumer) = queue_parse();
        let before = fp.edges.clone();
        let mut anchors = vec![
            Anchor {
                node: consumer,
                line: 14,
            },
            Anchor {
                node: producer,
                line: 7,
            },
        ];
        let stats = attach(&mut fp, "svc/bus.ts", module, &mut anchors);
        assert_eq!(
            stats,
            AnchorStats {
                to_method: 2,
                to_module: 0,
                unanchored: 0,
                queue_to_method: 2,
                queue_to_module: 0,
            }
        );
        // A2.8's CONTAINS pair stays first and untouched; the owner edges are
        // ADDED after it, in line order.
        assert_eq!(fp.edges[..2], before[..]);
        let added: Vec<_> = fp.edges[2..]
            .iter()
            .map(|e| (e.from, e.to, e.category))
            .collect();
        assert_eq!(
            added,
            vec![
                (
                    id(node_kind::FUNCTION, "publishOrder"),
                    producer,
                    edge_category::USES
                ),
                (
                    consumer,
                    id(node_kind::FUNCTION, "listen"),
                    edge_category::HANDLED_BY
                ),
            ]
        );
        // The extractor's POSITION is kept, not replaced by the anchor's.
        assert!(position_of(&fp, producer).is_some_and(|p| p.contains("\"file\":\"a.ts\"")));
        assert_eq!(census(&fp), stats);
    }

    #[test]
    fn queue_marker_at_module_level_keeps_its_one_contains() {
        let (mut fp, module, producer, _) = queue_parse();
        let mut anchors = vec![Anchor {
            node: producer,
            line: 2,
        }];
        let stats = attach(&mut fp, "svc/bus.ts", module, &mut anchors);
        assert_eq!(
            stats,
            AnchorStats {
                to_method: 0,
                to_module: 1,
                unanchored: 0,
                queue_to_method: 0,
                queue_to_module: 1,
            }
        );
        assert_eq!(
            fp.edges.len(),
            2,
            "no second module CONTAINS, no owner edge"
        );
    }

    #[test]
    fn queue_topic_sent_from_two_functions_is_used_by_both() {
        let (mut fp, module, producer, _) = queue_parse();
        let mut anchors = vec![
            Anchor {
                node: producer,
                line: 15,
            },
            Anchor {
                node: producer,
                line: 8,
            },
        ];
        attach(&mut fp, "svc/bus.ts", module, &mut anchors);
        let users: Vec<NodeId> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::USES && e.to == producer)
            .map(|e| e.from)
            .collect();
        assert_eq!(
            users,
            vec![
                id(node_kind::FUNCTION, "publishOrder"),
                id(node_kind::FUNCTION, "listen")
            ]
        );
    }

    #[test]
    fn report_line_appends_the_queue_share_only_when_queues_anchored() {
        let rpc = AnchorStats {
            to_method: 3,
            to_module: 1,
            ..AnchorStats::default()
        };
        assert_eq!(
            report_line(rpc, "r").as_deref(),
            Some("[marker-anchor] 3 anchored to methods, 1 to module, 0 unanchored repo=r"),
            "a repo without queue markers prints the pre-LE.4c line"
        );
        let mut both = rpc;
        both.add(AnchorStats {
            to_method: 2,
            queue_to_method: 2,
            ..AnchorStats::default()
        });
        assert_eq!(
            report_line(both, "r").as_deref(),
            Some(
                "[marker-anchor] 5 anchored to methods, 1 to module, 0 unanchored (queue: 2 to methods, 0 to module) repo=r"
            )
        );
        assert_eq!(report_line(AnchorStats::default(), "r"), None);
    }

    // ------------------------------------------------------------------
    // LE.4a: rehome_to_owner / access_census
    // ------------------------------------------------------------------

    /// A parse with a MODULE `a`, the given functions / methods, the
    /// DATA_ENTITY `orders` and `audit`, and the extractor's module edges to
    /// both (stamped like the engine stamps them).
    fn access_parse(owners: &[Entry<'_>]) -> (FileParse, NodeId, NodeId, NodeId) {
        let (mut fp, module) = parse_with(owners);
        let orders = id(node_kind::DATA_ENTITY, "data_entity:sql:orders");
        let audit = id(node_kind::DATA_ENTITY, "data_entity:sql:audit");
        for (t, q) in [(orders, "orders"), (audit, "audit")] {
            fp.nodes.push(node(t, vec![]));
            fp.nav.record(t, q, q, node_kind::DATA_ENTITY, Some(module));
            let mut e = Edge::new(module, t, edge_category::ACCESSES_DATA, Confidence::Medium);
            evidence::attach(&mut e, Evidence::emitter(DATA_ENTITIES_EMITTER));
            fp.edges.push(e);
        }
        (fp, module, orders, audit)
    }

    fn site(target: NodeId, line: u32, mode: Option<&'static str>) -> Site {
        Site { target, line, mode }
    }

    fn access_edges(fp: &FileParse, to: NodeId) -> Vec<(NodeId, Option<&'static str>)> {
        fp.edges
            .iter()
            .filter(|e| e.to == to && e.category == edge_category::ACCESSES_DATA)
            .map(|e| (e.from, mode_of(e)))
            .collect()
    }

    fn rehome(fp: &mut FileParse, module: NodeId, sites: &[Site], keep: &[NodeId]) -> AccessStats {
        rehome_to_owner(
            fp,
            "a.ts",
            module,
            edge_category::ACCESSES_DATA,
            sites,
            keep,
            DATA_ENTITIES_EMITTER,
        )
    }

    #[test]
    fn rehome_moves_module_edge_to_innermost_fn() {
        let (mut fp, module, orders, _) =
            access_parse(&[(node_kind::FUNCTION, "save", Some((3, 6)))]);
        let save = id(node_kind::FUNCTION, "save");
        let stats = rehome(&mut fp, module, &[site(orders, 4, Some("write"))], &[]);
        assert_eq!(access_edges(&fp, orders), vec![(save, Some("write"))]);
        let e = fp
            .edges
            .iter()
            .find(|e| e.from == save && e.to == orders)
            .unwrap();
        let ev = Evidence::of(e).unwrap();
        assert_eq!(ev.emitter, DATA_ENTITIES_EMITTER);
        assert_eq!((ev.file.as_deref(), ev.line), (Some("a.ts"), Some(4)));
        assert_eq!(ev.basis, evidence::Basis::Site);
        assert_eq!(
            stats,
            AccessStats {
                to_fn: 1,
                write: 1,
                ..AccessStats::default()
            }
        );
    }

    #[test]
    fn module_scope_site_keeps_module_edge() {
        let (mut fp, module, orders, audit) =
            access_parse(&[(node_kind::FUNCTION, "load", Some((5, 8)))]);
        let load = id(node_kind::FUNCTION, "load");
        let stats = rehome(
            &mut fp,
            module,
            &[
                // audit: only a module-scope site (line 1).
                site(audit, 1, Some("read")),
                // orders: one site in `load`, one at module scope.
                site(orders, 6, Some("read")),
                site(orders, 2, Some("read")),
            ],
            &[],
        );
        assert_eq!(
            access_edges(&fp, audit),
            vec![(module, None)],
            "no mode on a module edge"
        );
        let mut got = access_edges(&fp, orders);
        got.sort_by_key(|(f, _)| f.0);
        let mut want = vec![(module, None), (load, Some("read"))];
        want.sort_by_key(|(f, _)| f.0);
        assert_eq!(
            got, want,
            "the function edge is added, the module edge stays"
        );
        assert_eq!((stats.to_fn, stats.module_kept), (1, 2));
    }

    #[test]
    fn declared_target_keeps_module_edge() {
        let (mut fp, module, orders, _) =
            access_parse(&[(node_kind::FUNCTION, "save", Some((3, 6)))]);
        let save = id(node_kind::FUNCTION, "save");
        rehome(
            &mut fp,
            module,
            &[site(orders, 4, Some("write"))],
            &[orders],
        );
        let mut got = access_edges(&fp, orders);
        got.sort_by_key(|(f, _)| f.0);
        let mut want = vec![(module, None), (save, Some("write"))];
        want.sort_by_key(|(f, _)| f.0);
        assert_eq!(
            got, want,
            "a declaration's module edge stays beside the function's"
        );
    }

    #[test]
    fn nested_fn_wins() {
        let (mut fp, module, orders, _) = access_parse(&[
            (node_kind::FUNCTION, "outer", Some((2, 20))),
            (node_kind::FUNCTION, "inner", Some((5, 9))),
        ]);
        rehome(&mut fp, module, &[site(orders, 7, Some("read"))], &[]);
        assert_eq!(
            access_edges(&fp, orders),
            vec![(id(node_kind::FUNCTION, "inner"), Some("read"))]
        );
    }

    #[test]
    fn modes_fold_to_read_write() {
        let (mut fp, module, orders, _) =
            access_parse(&[(node_kind::METHOD, "archive", Some((3, 9)))]);
        let stats = rehome(
            &mut fp,
            module,
            &[
                site(orders, 4, Some("read")),
                site(orders, 5, None),
                site(orders, 6, Some("write")),
            ],
            &[],
        );
        assert_eq!(
            access_edges(&fp, orders),
            vec![(id(node_kind::METHOD, "archive"), Some("read_write"))]
        );
        assert_eq!(stats.read_write, 1);
        assert_eq!(fold_mode(Some("read"), Some("read")), Some("read"));
        assert_eq!(fold_mode(None, Some("write")), Some("write"));
        assert_eq!(
            fold_mode(Some("read_write"), Some("read")),
            Some("read_write")
        );
    }

    #[test]
    fn unknown_mode_attaches_no_cell() {
        let (mut fp, module, orders, _) =
            access_parse(&[(node_kind::FUNCTION, "open", Some((3, 6)))]);
        let open = id(node_kind::FUNCTION, "open");
        let stats = rehome(&mut fp, module, &[site(orders, 4, None)], &[]);
        let e = fp
            .edges
            .iter()
            .find(|e| e.from == open && e.to == orders)
            .unwrap();
        assert!(e.cell(cell_type::ACCESS_MODE).is_none());
        assert!(Evidence::of(e).is_some(), "still stamped");
        assert_eq!((stats.to_fn, stats.unknown), (1, 1));
    }

    #[test]
    fn existing_parser_edge_gets_mode_not_duplicate() {
        let (mut fp, module, orders, _) =
            access_parse(&[(node_kind::FUNCTION, "get_users", Some((3, 6)))]);
        let get = id(node_kind::FUNCTION, "get_users");
        let mut parser_edge = Edge::new(
            get,
            orders,
            edge_category::ACCESSES_DATA,
            Confidence::Medium,
        );
        evidence::attach(&mut parser_edge, Evidence::emitter("parser:go").line(4));
        fp.edges.push(parser_edge);
        let stats = rehome(&mut fp, module, &[site(orders, 5, Some("write"))], &[]);
        assert_eq!(
            access_edges(&fp, orders),
            vec![(get, Some("write"))],
            "one edge, no module edge"
        );
        let e = fp
            .edges
            .iter()
            .find(|e| e.from == get && e.to == orders)
            .unwrap();
        assert_eq!(
            Evidence::of(e).unwrap().emitter,
            "parser:go",
            "the parser keeps its evidence"
        );
        assert_eq!(stats.to_fn, 1);
        // A second pass folds, never duplicates or downgrades.
        rehome(&mut fp, module, &[site(orders, 5, Some("read"))], &[]);
        assert_eq!(access_edges(&fp, orders), vec![(get, Some("read_write"))]);
    }

    #[test]
    fn output_order_is_sorted() {
        let entries = [
            (node_kind::FUNCTION, "f1", Some((1, 3))),
            (node_kind::FUNCTION, "f2", Some((5, 7))),
            (node_kind::FUNCTION, "f3", Some((9, 11))),
        ];
        let (fp0, module, orders, audit) = access_parse(&entries);
        let sites = [
            site(audit, 10, Some("read")),
            site(orders, 2, Some("write")),
            site(orders, 6, Some("read")),
            site(audit, 2, None),
            site(orders, 10, Some("read")),
        ];
        let mut a = fp0.clone();
        rehome(&mut a, module, &sites, &[]);
        let mut reversed = sites;
        reversed.reverse();
        let mut b = fp0;
        rehome(&mut b, module, &reversed, &[]);
        assert_eq!(a.edges, b.edges, "site order must not reach the output");
        let added: Vec<(u64, u64)> = a
            .edges
            .iter()
            .filter(|e| e.from != module)
            .map(|e| (e.from.0, e.to.0))
            .collect();
        let mut sorted = added.clone();
        sorted.sort_unstable();
        assert_eq!(added, sorted);
        assert_eq!(added.len(), 5);
    }

    #[test]
    fn access_census_recounts_a_finished_parse() {
        let (mut fp, module, orders, audit) = access_parse(&[
            (node_kind::FUNCTION, "save", Some((3, 6))),
            (node_kind::FUNCTION, "load", Some((8, 9))),
        ]);
        let returned = rehome(
            &mut fp,
            module,
            &[
                site(orders, 4, Some("write")),
                site(orders, 8, Some("read")),
                site(audit, 1, Some("read")),
            ],
            &[],
        );
        let counted = access_census(&fp);
        assert_eq!(
            counted, returned,
            "a cache-served parse counts like a fresh one"
        );
        assert_eq!(
            report_access_line(counted, "r").as_deref(),
            Some(
                "[data-access] rehomed fn=2 module_kept=1 modes read=1 write=1 read_write=0 unknown=0 repo=r"
            )
        );
        assert_eq!(report_access_line(AccessStats::default(), "r"), None);
    }
}
