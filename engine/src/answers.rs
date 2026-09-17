// ============================================================================
// P3 answer-shaped primitives (handoff v6) — rank + locate + why, in the ENGINE
// so the CLI, pyo3/MCP, and future TUI/3d-viewer all share one implementation.
// ============================================================================

use std::collections::{BTreeMap, BTreeSet, HashMap};

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_code_extractors::queues::is_framework_tag;
use repo_graph_core::{Cell, CellPayload, Edge, NodeId};
use repo_graph_graph::{MergedGraph, Reach};

/// One node in a blast-radius answer: identity + kind + why-it's-here (`reason`)
/// + PPR `score` + `file`:`line` + `live`. Serialized straight to pyo3/CLI.
#[derive(serde::Serialize)]
pub struct BlastAnswer {
    pub id: u64,
    pub qname: String,
    pub name: String,
    pub kind: &'static str,
    /// Edge category that first put this node in scope ("why it's in the radius").
    pub reason: &'static str,
    pub depth: usize,
    pub score: f64,
    /// Reachable from an entrypoint (route/handler/main/test/component) — `false`
    /// = likely dead. Best-effort; annotated, not filtered, unless `live_only`.
    pub live: bool,
    pub file: Option<String>,
    pub line: Option<i64>,
}

/// Is this node an entrypoint — an externally-triggered root from which live
/// code is reachable? Routes, gRPC/WS/event handlers, CLI commands, framework
/// components, and `main`/`test*` functions.
fn is_entrypoint(kind: Option<repo_graph_core::NodeKindId>, name: &str) -> bool {
    match kind {
        Some(k)
            if k == node_kind::ROUTE
                || k == node_kind::GRPC_SERVICE
                || k == node_kind::WS_HANDLER
                || k == node_kind::EVENT_HANDLER
                || k == node_kind::CLI_COMMAND
                || k == node_kind::COMPONENT =>
        {
            true
        }
        Some(k) if k == node_kind::FUNCTION || k == node_kind::METHOD => {
            name == "main" || name.starts_with("test") || name.starts_with("Test")
        }
        _ => false,
    }
}

/// The entrypoint-reachable ("live") node set: every entrypoint plus everything
/// forward-reachable from one along semantic carry edges. A node absent from
/// this set is likely dead code. Conservative (generous entrypoint set) to avoid
/// false-dead flags — the failure mode the handoff warns about.
pub fn entrypoint_reachable(merged: &MergedGraph) -> std::collections::HashSet<NodeId> {
    use std::collections::{HashSet, VecDeque};
    let carry: HashSet<repo_graph_core::EdgeCategoryId> =
        repo_graph_graph::blast_carry_edges().into_iter().collect();
    let edges: Vec<&Edge> = merged.all_edges().collect();

    let mut live: HashSet<NodeId> = HashSet::new();
    let mut queue: VecDeque<NodeId> = VecDeque::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            let name = g.nav.name_by_id.get(&n.id).map(String::as_str).unwrap_or("");
            if is_entrypoint(kind, name) && live.insert(n.id) {
                queue.push_back(n.id);
            }
        }
    }
    while let Some(node) = queue.pop_front() {
        for e in &edges {
            if e.from == node && carry.contains(&e.category) && live.insert(e.to) {
                queue.push_back(e.to);
            }
        }
    }
    live
}

/// `blast_radius`, resolved from a qname/name and fully located — the P3 answer
/// that `find`→`impact`→`activate`→`read×N` collapses to. `direction` ∈
/// {`forward`, `backward`, `both`}. Err on an unknown qname or bad direction.
///
/// `scope` (A8.3) restricts the answer to nodes whose file lives under that
/// repo-relative path, applied BEFORE `top_k` truncation so a scoped `--top-k`
/// spends its whole budget in scope instead of on whatever PPR liked globally.
/// A node with no locatable file (ENDPOINT/ROUTE/DOC_SPACE — see
/// [`node_in_scope`]) is KEPT, never dropped: those are the cross-boundary part
/// of the answer. `scope` narrows WITHIN a repo — under a multi-repo merge each
/// repo's POSITION paths are relative to its OWN root.
pub fn blast_radius_by_qname(
    merged: &MergedGraph,
    qname: &str,
    direction: &str,
    max_depth: usize,
    top_k: Option<usize>,
    live_only: bool,
    scope: Option<&str>,
) -> Result<Vec<BlastAnswer>, String> {
    let seed = merged
        .node_id_by_qname(qname)
        .or_else(|| merged.resolve_name(qname))
        .ok_or_else(|| format!("no node with qname/name `{qname}`"))?;
    let reach = match direction {
        "forward" => Reach::Forward,
        "backward" => Reach::Backward,
        "both" => Reach::Both,
        o => return Err(format!("direction must be forward|backward|both, got `{o}`")),
    };
    let live = entrypoint_reachable(merged);
    let hits = merged.blast_radius(seed, reach, max_depth, None);
    let out: Vec<BlastAnswer> = hits
        .iter()
        .filter(|h| !live_only || live.contains(&h.id))
        .map(|h| {
            let (name, qname, kind, file, line) = locate_node(merged, h.id);
            BlastAnswer {
                id: h.id.0,
                qname,
                name,
                kind,
                reason: edge_category::name(h.reason),
                depth: h.depth,
                score: h.score,
                live: live.contains(&h.id),
                file,
                line,
            }
        })
        .collect();
    // Scope BEFORE the cut: filtering after `truncate` would spend the budget
    // on out-of-scope nodes and return fewer (or zero) in-scope answers.
    let mut out = apply_scope(merged, out, scope, |a| NodeId(a.id), "blast_radius");
    if let Some(k) = top_k {
        out.truncate(k);
    }
    Ok(out)
}

/// One hop in a cross-stack trace: a typed edge from one entity to the next,
/// with the `mechanism` (edge category) and whether it crossed a service
/// boundary. The destination is located.
#[derive(serde::Serialize)]
pub struct TraceHop {
    pub depth: usize,
    /// Edge category name — the mechanism (`CALLS`, `HTTP_CALLS`, `QUEUE_FLOWS`…).
    pub mechanism: &'static str,
    /// True when `from` and `to` live in different repos/services.
    pub cross_service: bool,
    pub from_qname: String,
    pub to_qname: String,
    pub to_kind: &'static str,
    pub to_file: Option<String>,
    pub to_line: Option<i64>,
}

/// **cross_stack_trace** (P3): follow a feature forward across service
/// boundaries and return the ORDERED path — each hop labeled with its mechanism
/// (http/queue/grpc/call/…) and whether it crossed a service boundary — in one
/// call. Where `blast_radius` returns a ranked *set*, this returns the *sequence*
/// of typed edges, so an agent sees how a request flows end to end.
///
/// Deliberately takes NO `scope` (A8.3): a trace's entire value is that it
/// crosses service/directory boundaries, so filtering its hops would delete the
/// answer. Scope `blast_radius`/`resolve`/`governing_docs` instead.
pub fn cross_stack_trace(
    merged: &MergedGraph,
    feature: &str,
    max_depth: usize,
) -> Result<Vec<TraceHop>, String> {
    use std::collections::{HashMap, HashSet, VecDeque};
    let seed = merged
        .node_id_by_qname(feature)
        .or_else(|| merged.resolve_name(feature))
        .ok_or_else(|| format!("no node with qname/name `{feature}`"))?;

    let mut repo_of: HashMap<NodeId, u64> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            repo_of.insert(n.id, g.repo.0);
        }
    }
    let carry: HashSet<repo_graph_core::EdgeCategoryId> =
        repo_graph_graph::blast_carry_edges().into_iter().collect();
    let edges: Vec<&Edge> = merged.all_edges().collect();

    let mut hops = Vec::new();
    let mut visited: HashSet<NodeId> = HashSet::from([seed]);
    let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(seed, 0)]);
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        for e in &edges {
            if e.from != node || !carry.contains(&e.category) {
                continue;
            }
            if visited.insert(e.to) {
                let (_, from_qname, _, _, _) = locate_node(merged, e.from);
                let (_, to_qname, to_kind, to_file, to_line) = locate_node(merged, e.to);
                let cross_service = repo_of.get(&e.from) != repo_of.get(&e.to);
                hops.push(TraceHop {
                    depth: depth + 1,
                    mechanism: edge_category::name(e.category),
                    cross_service,
                    from_qname,
                    to_qname,
                    to_kind,
                    to_file,
                    to_line,
                });
                queue.push_back((e.to, depth + 1));
            }
        }
    }
    Ok(hops)
}

/// One located node in a `resolve` answer: identity + kind + PPR relevance +
/// `file`:`line`. (No `reason`/`depth` — `resolve` locates seeds, it doesn't walk.)
#[derive(serde::Serialize)]
pub struct LocatedNode {
    pub id: u64,
    pub qname: String,
    pub name: String,
    pub kind: &'static str,
    pub score: f64,
    pub file: Option<String>,
    pub line: Option<i64>,
}

/// **resolve** (P3): a failure/change signal (stacktrace, diff, test id, or
/// `auto`) → the ranked, LOCATED nodes it points at, in one call. Resolution
/// order is preserved (stacktrace frames stay in order); `score` is PPR
/// relevance seeded by the whole resolved set, so shared-context nodes rank up.
/// `kind` ∈ {`stacktrace`, `test`, `diff`, `auto`}.
///
/// `scope` (A8.3) filters the SEEDS before PPR, not the rendered result, so the
/// scores genuinely change: `resolve_frame`/`resolve_file` match POSITION files
/// by BASENAME ONLY, so an unscoped frame naming `utils.py` seeds every
/// `utils.py` in the monorepo and those bogus seeds then shape the ranking of
/// the real one. A node with no locatable file is KEPT (see [`node_in_scope`]).
/// Any golden-output test on this function must pass `scope = None`.
pub fn resolve_signal_located(
    merged: &MergedGraph,
    text: &str,
    kind: &str,
    top_k: Option<usize>,
    scope: Option<&str>,
) -> Vec<LocatedNode> {
    let seeds = merged.resolve_signal(text, kind);
    // Pre-PPR: `activate` below must only see in-scope seeds.
    let seeds = apply_scope(merged, seeds, scope, |id| *id, "resolve");
    if seeds.is_empty() {
        return Vec::new();
    }
    let mut config = repo_graph_graph::code_activation_defaults();
    config.direction = repo_graph_activation::Direction::Undirected;
    config.top_k = usize::MAX;
    let scores: HashMap<NodeId, f64> =
        merged.activate(&seeds, &config).scores.into_iter().collect();
    let mut out: Vec<LocatedNode> = seeds
        .iter()
        .map(|id| {
            let (name, qname, kind, file, line) = locate_node(merged, *id);
            LocatedNode {
                id: id.0,
                qname,
                name,
                kind,
                score: scores.get(id).copied().unwrap_or(0.0),
                file,
                line,
            }
        })
        .collect();
    if let Some(k) = top_k {
        out.truncate(k);
    }
    out
}

/// **governing_docs** (P3 payoff, tier-4): the doc sections that DOCUMENTS a
/// symbol — "what are the rules for X?" — located, in one call. Direct
/// `doc --DOCUMENTS--> symbol` predecessors (the conservative, precise linker
/// signal). Reuses `LocatedNode` (score = 0; docs aren't PPR-ranked here).
///
/// `scope` (A8.3) keeps only the doc sections whose own POSITION file lives
/// under that repo-relative path. A section with no locatable file is KEPT (see
/// [`node_in_scope`]).
pub fn governing_docs(
    merged: &MergedGraph,
    qname: &str,
    scope: Option<&str>,
) -> Result<Vec<LocatedNode>, String> {
    let target = merged
        .node_id_by_qname(qname)
        .or_else(|| merged.resolve_name(qname))
        .ok_or_else(|| format!("no node with qname/name `{qname}`"))?;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for e in merged.all_edges() {
        if e.to == target
            && e.category == edge_category::DOCUMENTS
            && seen.insert(e.from)
        {
            let (name, qn, kind, file, line) = locate_node(merged, e.from);
            out.push(LocatedNode {
                id: e.from.0,
                qname: qn,
                name,
                kind,
                score: 0.0,
                file,
                line,
            });
        }
    }
    let out = apply_scope(merged, out, scope, |d| NodeId(d.id), "governing_docs");
    Ok(out)
}

/// `(name, qname, kind_name, file, line)` for a node across the merged graphs.
/// `file`/`line` come from the POSITION cell; `None` for synthetic nodes
/// (ENDPOINT/DOC_SPACE) that carry no span. Shared "locate" for the primitives.
pub fn locate_node(
    merged: &MergedGraph,
    id: NodeId,
) -> (String, String, &'static str, Option<String>, Option<i64>) {
    for g in &merged.graphs {
        if !g.nav.qname_by_id.contains_key(&id) {
            continue;
        }
        let name = g.nav.name_by_id.get(&id).cloned().unwrap_or_default();
        let qname = g.nav.qname_by_id.get(&id).cloned().unwrap_or_default();
        let kind = g
            .nav
            .kind_by_id
            .get(&id)
            .map(|k| node_kind::name(*k))
            .unwrap_or("UNKNOWN");
        let (mut file, mut line) = (None, None);
        if let Some(n) = g.nodes.iter().find(|n| n.id == id) {
            for c in &n.cells {
                if c.kind != repo_graph_code_domain::cell_type::POSITION {
                    continue;
                }
                if let CellPayload::Json(s) | CellPayload::Text(s) = &c.payload
                    && let Ok(v) = serde_json::from_str::<serde_json::Value>(s)
                {
                    file = v.get("file").and_then(|f| f.as_str()).map(String::from);
                    line = v.get("start_line").and_then(serde_json::Value::as_i64);
                    // FIRST POSITION WINS — A2.8, and it is load-bearing, not
                    // cosmetic. Do NOT remove this `break`.
                    //
                    // A node can carry MORE than one POSITION cell:
                    // `merge_parses` appends the cells of every `FileParse` that
                    // minted the same NodeId, which is the normal case for a
                    // queue topic two files publish to. Without the break the
                    // LAST file parsed won, so `blast_radius` / `trace` /
                    // `resolve` reported a different file from
                    // `passes::position_file` and
                    // `projection_text::node_position`, both of which return the
                    // first. This aligns all three on the first.
                    break;
                }
            }
        }
        return (name, qname, kind, file, line);
    }
    (String::new(), format!("(unknown:{})", id.0), "UNKNOWN", None, None)
}

// ============================================================================
// A8.3 — `scope`: restrict a P3 answer to one part of a monorepo.
//
// Two rules that are load-bearing rather than cosmetic:
//   1. the filter runs BEFORE `truncate(top_k)` (blast_radius) and BEFORE
//      `activate` (resolve), so it changes the RANKING, not just the display;
//   2. a node with NO locatable file is KEPT. Dropping unlocatable nodes would
//      silently delete every ENDPOINT / ROUTE / DOC_SPACE from a scoped answer
//      and destroy the cross-service result these primitives exist for. The
//      `[scope]` marker reports the kept-unlocatable count so that is visible.
// ============================================================================

/// The repo-relative path a node should be scoped by. POSITION first (the
/// normal case), then the `ENDPOINT_HIT` cell's `file` for synthetic ENDPOINT
/// nodes that carry no span. `None` for nodes with neither — ROUTE nodes
/// included, because `ROUTE_METHOD` is a bare text cell holding only the HTTP
/// verb, not a path.
fn scope_file_of(merged: &MergedGraph, id: NodeId) -> Option<String> {
    if let (_, _, _, Some(file), _) = locate_node(merged, id) {
        return Some(file);
    }
    for g in &merged.graphs {
        let Some(n) = g.nodes.iter().find(|n| n.id == id) else {
            continue;
        };
        for c in &n.cells {
            if c.kind != repo_graph_code_domain::cell_type::ENDPOINT_HIT {
                continue;
            }
            let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
                continue;
            };
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                if let Some(f) = v.get("file").and_then(|f| f.as_str()) {
                    return Some(f.to_string());
                }
            }
        }
    }
    None
}

/// True when `file` lives under `scope`. Prefix match on a `/` boundary only,
/// so `scope = "services/ap"` does NOT match `services/api/handler.py`. Both
/// sides are normalised by trimming a leading `./` or `/` and a trailing `/`;
/// an empty scope matches everything.
fn in_scope(file: &str, scope: &str) -> bool {
    let f = file.trim_start_matches("./").trim_start_matches('/');
    let s = scope.trim_start_matches("./").trim_matches('/');
    if s.is_empty() {
        return true;
    }
    f == s || f.starts_with(&format!("{s}/"))
}

/// Would this node survive `scope`? `scope = None` is always true, and so is a
/// node with no locatable file (the keep-unlocatable policy above). Public so
/// the pyo3 `find_nodes_by_qname` surface applies exactly the same rule as the
/// three scoped primitives rather than re-deriving a path guess in Python.
///
/// `scope` narrows WITHIN a repo: under a multi-repo merge each repo's POSITION
/// paths are relative to its OWN root, so a path that was passed as a separate
/// `--with` repo will not match as a scope.
pub fn node_in_scope(merged: &MergedGraph, id: NodeId, scope: Option<&str>) -> bool {
    let Some(s) = scope else { return true };
    match scope_file_of(merged, id) {
        Some(f) => in_scope(&f, s),
        None => true,
    }
}

/// One shared applier so the scoped call sites cannot drift apart. `scope =
/// None` is a strict no-op: the input is returned untouched and no marker is
/// emitted, so no existing answer, ranking or cell value changes.
fn apply_scope<T>(
    merged: &MergedGraph,
    items: Vec<T>,
    scope: Option<&str>,
    id_of: impl Fn(&T) -> NodeId,
    what: &str,
) -> Vec<T> {
    let Some(s) = scope else { return items };
    let before = items.len();
    let mut unlocatable = 0usize;
    let out: Vec<T> = items
        .into_iter()
        .filter(|it| match scope_file_of(merged, id_of(it)) {
            Some(f) => in_scope(&f, s),
            None => {
                unlocatable += 1;
                true
            }
        })
        .collect();
    eprintln!(
        "[scope] {what} scope={s}: {before} -> {} (unlocatable={unlocatable})",
        out.len()
    );
    out
}

// ============================================================================
// A12.2 — `message_contracts`: every queue topic, both sides, located, with a
// match / mismatch / unknown verdict on the payload type (A12.1's
// MESSAGE_TYPE cell).
//
// Pairing is NOT re-derived here. A producer and a consumer are a pair exactly
// when `QueueStackResolver` emitted a QUEUE_FLOWS edge between them, so the
// report inherits every rule the resolver applies — the unpairable framework
// tag (A2.3), the broker-family gate and the wildcard dialects (A2.7) — and
// cannot disagree with what `trace` / `blast_radius` walk. Topics are the
// queue qnames as the extractors minted them (A2.5 task identity, A2.6 folded
// URL/ARN topics), stripped of their `queue_producer:` / `queue_consumer:`
// prefix and nothing else.
//
// Every queue node the resolver left unpaired still gets ONE one-sided row.
// That is the unpaired-topic signal, and it is what stops a framework-tag
// topic fanning out into an N×M cross product: 3 tag producers + 4 tag
// consumers are 7 rows, never 12.
//
// fired_on marker, one line per call that produced rows:
//   `... 2>&1 | grep '^\[contracts\] topics='`
// ============================================================================

const PRODUCER_PREFIX: &str = "queue_producer:";
const CONSUMER_PREFIX: &str = "queue_consumer:";

const NOTE_TAG: &str = "topic is a framework tag, not a literal — no trustworthy pairing";
const NOTE_NO_COUNTERPART: &str = "no counterpart for this topic";
const NOTE_FAMILY_SPLIT: &str =
    "a counterpart names this topic but the resolver did not pair them (broker family differs)";
const NOTE_NO_PRODUCER_TYPE: &str = "producer side has no detectable message type";
const NOTE_NO_CONSUMER_TYPE: &str = "consumer side has no detectable message type";
const NOTE_NO_TYPES: &str = "neither side has a detectable message type";
const NOTE_CONFLICT: &str =
    "a side carries conflicting message types across its call sites — see types_seen";
const NOTE_PRIMITIVE: &str =
    "a side carries a primitive payload (a serialised body) — the types are not comparable";
const NOTE_PRIMITIVE_MATCH: &str =
    "both sides carry the same primitive payload — the real schema is serialised inside it";
const NOTE_FILE_WINDOW: &str =
    "a side's type was read from the whole file, not beside the call site";

/// Payload type names that mean "serialised body", not "schema". A12.1 keeps
/// them on purpose — by the Kafka `<Key, Value>` convention the last type
/// argument IS the value type — so they are weighed here: a primitive never
/// supports a `mismatch` (a `string` body is routinely decoded into a struct)
/// and only ever a weak `match`. Heuristic; extend as fixtures accumulate.
const PRIMITIVE_PAYLOADS: &[&str] = &[
    "string", "String", "str", "Null", "Ignore", "byte", "bytes", "Bytes", "object", "Object",
];

fn is_primitive_payload(ty: &str) -> bool {
    PRIMITIVE_PAYLOADS.contains(&ty)
}

/// One side of a message contract: the queue node, where it lives, and the
/// payload type it says it carries.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MessageContractSide {
    pub node_id: u64,
    /// `RepoId.0`; `GenerateResult::repo_labels` maps it to a human label.
    pub repo_id: u64,
    pub qname: String,
    /// The topic as THIS node names it. It differs from the row's `topic` only
    /// for a wildcard subscriber (`orders.*`) the resolver matched by pattern.
    pub topic: String,
    /// Qualified name of the parent MODULE — the file the call site is in.
    pub module: Option<String>,
    /// The node's own POSITION (A2.8: first call site in the first file that
    /// uses the topic), else the parent MODULE's.
    pub file: Option<String>,
    pub line: Option<i64>,
    /// The best MESSAGE_TYPE `type`: beside-the-call-site before whole-file,
    /// schema before primitive, then first-seen.
    pub message_type: Option<String>,
    /// The type as written (`pb.OrderCreated`).
    pub message_type_raw: Option<String>,
    /// `"generic"` | `"struct_literal"`.
    pub form: Option<String>,
    /// `"near"` | `"file"`.
    pub window: Option<String>,
    /// Every distinct `type` across the node's MESSAGE_TYPE cells, first-seen
    /// order. A topic used from N files carries up to N cells.
    pub types_seen: Vec<String>,
    /// More than one distinct non-primitive type was read beside this node's
    /// call sites (whole-file guesses count only when there is no near one).
    pub conflicting: bool,
}

/// One row of the contract report.
///
/// `status` is `"match"`, `"mismatch"` or `"unknown"`. `confidence` is
/// `"strong"` or `"weak"` for a verdict and `"none"` for `unknown`. `note` says
/// why whenever the row is anything other than a strong verdict.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MessageContractRow {
    pub topic: String,
    pub topic_is_tag: bool,
    /// The consumer subscribes by a wildcard pattern the resolver matched (A2.7).
    pub pattern: bool,
    pub producer: Option<MessageContractSide>,
    pub consumer: Option<MessageContractSide>,
    pub status: &'static str,
    pub confidence: &'static str,
    pub note: Option<&'static str>,
}

/// A queue node folded across every graph it appears in. One NodeId can sit in
/// several per-language graphs of one repo — the id hashes repo + kind +
/// qname, not language — and each copy carries its own files' cells.
struct QueueNodeAcc<'a> {
    producer: bool,
    repo: u64,
    qname: &'a str,
    topic: &'a str,
    parent: Option<NodeId>,
    types: Vec<&'a Cell>,
}

/// One MESSAGE_TYPE cell, parsed. A cell that is not JSON or has no `type`
/// parses to nothing — never a panic.
struct ParsedType {
    ty: String,
    raw: Option<String>,
    form: Option<String>,
    window: Option<String>,
}

impl ParsedType {
    fn near(&self) -> bool {
        self.window.as_deref() == Some("near")
    }
}

fn parse_message_type(c: &Cell) -> Option<ParsedType> {
    let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
        return None;
    };
    let v = serde_json::from_str::<serde_json::Value>(s).ok()?;
    let field = |k: &str| v.get(k).and_then(serde_json::Value::as_str).map(String::from);
    Some(ParsedType {
        ty: field("type").filter(|t| !t.is_empty())?,
        raw: field("raw"),
        form: field("form"),
        window: field("window"),
    })
}

fn collect_queue_nodes(merged: &MergedGraph) -> BTreeMap<u64, QueueNodeAcc<'_>> {
    let mut out: BTreeMap<u64, QueueNodeAcc<'_>> = BTreeMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let (producer, prefix) = match g.nav.kind_by_id.get(&n.id) {
                Some(k) if *k == node_kind::QUEUE_PRODUCER => (true, PRODUCER_PREFIX),
                Some(k) if *k == node_kind::QUEUE_CONSUMER => (false, CONSUMER_PREFIX),
                _ => continue,
            };
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(topic) = qname.strip_prefix(prefix) else { continue };
            let acc = out.entry(n.id.0).or_insert_with(|| QueueNodeAcc {
                producer,
                repo: n.repo.0,
                qname,
                topic,
                parent: None,
                types: Vec::new(),
            });
            if acc.parent.is_none() {
                acc.parent = g.nav.parent_of.get(&n.id).copied();
            }
            acc.types
                .extend(n.cells.iter().filter(|c| c.kind == cell_type::MESSAGE_TYPE));
        }
    }
    out
}

fn contract_side(merged: &MergedGraph, id: u64, acc: &QueueNodeAcc<'_>) -> MessageContractSide {
    let parsed: Vec<ParsedType> = acc.types.iter().filter_map(|c| parse_message_type(c)).collect();
    // `min_by_key` keeps the FIRST of equal keys, so first-seen breaks ties.
    let best = parsed
        .iter()
        .min_by_key(|t| (!t.near(), is_primitive_payload(&t.ty)));
    let mut types_seen: Vec<String> = Vec::new();
    for t in &parsed {
        if !types_seen.contains(&t.ty) {
            types_seen.push(t.ty.clone());
        }
    }
    // Conflict is judged on the near reads when there are any: a whole-file
    // guess is too weak to contradict a type read beside the call site.
    let any_near = parsed.iter().any(ParsedType::near);
    let mut schemas: Vec<&str> = Vec::new();
    for t in parsed.iter().filter(|t| t.near() || !any_near) {
        if !is_primitive_payload(&t.ty) && !schemas.contains(&t.ty.as_str()) {
            schemas.push(&t.ty);
        }
    }

    let (_, _, _, mut file, mut line) = locate_node(merged, NodeId(id));
    if file.is_none()
        && let Some(p) = acc.parent
    {
        (_, _, _, file, line) = locate_node(merged, p);
    }
    let module = acc
        .parent
        .and_then(|p| merged.graphs.iter().find_map(|g| g.nav.qname_by_id.get(&p).cloned()));

    MessageContractSide {
        node_id: id,
        repo_id: acc.repo,
        qname: acc.qname.to_string(),
        topic: acc.topic.to_string(),
        module,
        file,
        line,
        message_type: best.map(|t| t.ty.clone()),
        message_type_raw: best.and_then(|t| t.raw.clone()),
        form: best.and_then(|t| t.form.clone()),
        window: best.and_then(|t| t.window.clone()),
        types_seen,
        conflicting: schemas.len() > 1,
    }
}

/// `(status, confidence, note)` for one resolver-made pair.
fn contract_verdict(
    p: &MessageContractSide,
    c: &MessageContractSide,
) -> (&'static str, &'static str, Option<&'static str>) {
    const UNKNOWN: (&str, &str) = ("unknown", "none");
    let (pt, ct) = match (p.message_type.as_deref(), c.message_type.as_deref()) {
        (Some(pt), Some(ct)) => (pt, ct),
        (None, None) => return (UNKNOWN.0, UNKNOWN.1, Some(NOTE_NO_TYPES)),
        (None, Some(_)) => return (UNKNOWN.0, UNKNOWN.1, Some(NOTE_NO_PRODUCER_TYPE)),
        (Some(_), None) => return (UNKNOWN.0, UNKNOWN.1, Some(NOTE_NO_CONSUMER_TYPE)),
    };
    if p.conflicting || c.conflicting {
        return (UNKNOWN.0, UNKNOWN.1, Some(NOTE_CONFLICT));
    }
    let primitive = is_primitive_payload(pt) || is_primitive_payload(ct);
    let whole_file = p.window.as_deref() != Some("near") || c.window.as_deref() != Some("near");
    if pt == ct {
        let note = if primitive {
            Some(NOTE_PRIMITIVE_MATCH)
        } else {
            whole_file.then_some(NOTE_FILE_WINDOW)
        };
        return ("match", if note.is_some() { "weak" } else { "strong" }, note);
    }
    if primitive {
        return (UNKNOWN.0, UNKNOWN.1, Some(NOTE_PRIMITIVE));
    }
    let note = whole_file.then_some(NOTE_FILE_WINDOW);
    ("mismatch", if whole_file { "weak" } else { "strong" }, note)
}

/// Every queue topic in `merged` as producer/consumer contract rows — one per
/// resolver-made pair, plus one one-sided row per queue node the resolver left
/// unpaired. Sorted by (topic, producer file/id, consumer file/id), so two
/// calls over the same graph serialise byte-identically.
///
/// Read-only: no node, edge or cell is added or changed. A graph whose
/// resolvers never ran has no QUEUE_FLOWS edges, so every row is one-sided.
pub fn message_contracts(merged: &MergedGraph) -> Vec<MessageContractRow> {
    let nodes = collect_queue_nodes(merged);
    let is_tag_id = |id: &u64| nodes.get(id).is_some_and(|a| is_framework_tag(a.topic));
    // A node folded from two graphs is walked twice by the resolver, so its
    // edges can repeat: the set dedups them. Tags are refused even if some
    // future resolver pairs them.
    let pairs: BTreeSet<(u64, u64)> = merged
        .all_edges()
        .filter(|e| e.category == edge_category::QUEUE_FLOWS)
        .map(|e| (e.from.0, e.to.0))
        .filter(|(f, t)| {
            nodes.get(f).is_some_and(|a| a.producer)
                && nodes.get(t).is_some_and(|a| !a.producer)
                && !is_tag_id(f)
                && !is_tag_id(t)
        })
        .collect();
    let sides: BTreeMap<u64, MessageContractSide> = nodes
        .iter()
        .map(|(id, acc)| (*id, contract_side(merged, *id, acc)))
        .collect();

    let mut rows: Vec<MessageContractRow> = Vec::new();
    let mut paired: BTreeSet<u64> = BTreeSet::new();
    for (p, c) in &pairs {
        let (Some(ps), Some(cs)) = (sides.get(p), sides.get(c)) else { continue };
        paired.insert(*p);
        paired.insert(*c);
        let (status, confidence, note) = contract_verdict(ps, cs);
        rows.push(MessageContractRow {
            topic: ps.topic.clone(),
            topic_is_tag: false,
            pattern: ps.topic != cs.topic,
            producer: Some(ps.clone()),
            consumer: Some(cs.clone()),
            status,
            confidence,
            note,
        });
    }

    // Which directions name each topic, to tell "nobody on the other side"
    // from "somebody the resolver refused" (the family gate).
    let mut directions: HashMap<&str, (bool, bool)> = HashMap::new();
    for acc in nodes.values() {
        let d = directions.entry(acc.topic).or_default();
        if acc.producer {
            d.0 = true;
        } else {
            d.1 = true;
        }
    }
    for (id, acc) in &nodes {
        if paired.contains(id) {
            continue;
        }
        let Some(side) = sides.get(id) else { continue };
        let tag = is_framework_tag(acc.topic);
        let note = if tag {
            NOTE_TAG
        } else {
            let (has_p, has_c) = directions.get(acc.topic).copied().unwrap_or_default();
            if (acc.producer && has_c) || (!acc.producer && has_p) {
                NOTE_FAMILY_SPLIT
            } else {
                NOTE_NO_COUNTERPART
            }
        };
        let (producer, consumer) = if acc.producer {
            (Some(side.clone()), None)
        } else {
            (None, Some(side.clone()))
        };
        rows.push(MessageContractRow {
            topic: acc.topic.to_string(),
            topic_is_tag: tag,
            pattern: false,
            producer,
            consumer,
            status: "unknown",
            confidence: "none",
            note: Some(note),
        });
    }

    fn side_key(s: &Option<MessageContractSide>) -> Option<(Option<&str>, u64)> {
        s.as_ref().map(|s| (s.file.as_deref(), s.node_id))
    }
    rows.sort_by(|a, b| {
        (a.topic.as_str(), side_key(&a.producer), side_key(&a.consumer)).cmp(&(
            b.topic.as_str(),
            side_key(&b.producer),
            side_key(&b.consumer),
        ))
    });

    if !rows.is_empty() {
        let count = |s: &str| rows.iter().filter(|r| r.status == s).count();
        let topics: BTreeSet<(&str, bool)> =
            rows.iter().map(|r| (r.topic.as_str(), r.topic_is_tag)).collect();
        let tag_topics = topics.iter().filter(|(_, tag)| *tag).count();
        eprintln!(
            "[contracts] topics={} match={} mismatch={} unknown={} literal={} tag={} rows={}",
            topics.len(),
            count("match"),
            count("mismatch"),
            count("unknown"),
            topics.len() - tag_topics,
            tag_topics,
            rows.len()
        );
    }
    rows
}

#[cfg(test)]
mod locate_tests {
    use super::locate_node;
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
    use repo_graph_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};
    use repo_graph_graph::{MergedGraph, RepoGraph, SymbolTable};

    /// A2.8 — FIRST POSITION WINS, and the `break` in `locate_node` is what
    /// makes it so. `merge_parses` appends the cells of every `FileParse` that
    /// minted the same NodeId, so a queue topic published from two files
    /// carries TWO POSITION cells. Without the break the LAST file parsed won,
    /// which disagreed with `passes::position_file` and
    /// `projection_text::node_position` — both of which return the first.
    ///
    /// A3.6 refactors these lines: this test is the contract. If it starts
    /// failing with `b.go`, first-wins was reverted.
    #[test]
    fn locate_node_uses_first_position() {
        let repo = RepoId::from_canonical("test://locate");
        let id = NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::QUEUE_PRODUCER,
            "queue_producer:orders",
        );
        let pos = |file: &str, line: u32| Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":{line},"end_line":{line}}}"#
            )),
        };
        let mut nav = CodeNav::default();
        nav.record(
            id,
            "orders",
            "queue_producer:orders",
            node_kind::QUEUE_PRODUCER,
            None,
        );
        let g = RepoGraph {
            repo,
            nodes: vec![Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: vec![pos("a.go", 4), pos("b.go", 11)],
            }],
            edges: vec![],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let (name, qname, kind, file, line) = locate_node(&MergedGraph::new(vec![g]), id);
        assert_eq!(name, "orders");
        assert_eq!(qname, "queue_producer:orders");
        assert_eq!(kind, "QUEUE_PRODUCER");
        assert_eq!(file.as_deref(), Some("a.go"));
        assert_eq!(line, Some(4));
    }
}

#[cfg(test)]
mod scope_tests {
    use super::in_scope;

    #[test]
    fn in_scope_matches_on_segment_boundaries_only() {
        assert!(in_scope("services/api/handler.py", "services/api"));
        assert!(in_scope("./services/api/handler.py", "/services/api/"));
        assert!(in_scope("services/api", "services/api"));
        // The boundary rule: a prefix that stops mid-segment must not match.
        assert!(!in_scope("services/api/handler.py", "services/ap"));
        assert!(!in_scope("services-api/handler.py", "services"));
        assert!(!in_scope("web/client.py", "services/api"));
        // An empty scope is "everything", not "nothing".
        assert!(in_scope("web/client.py", ""));
        assert!(in_scope("web/client.py", "/"));
    }
}

#[cfg(test)]
mod contracts_tests {
    use super::{MessageContractSide, contract_side, contract_verdict, message_contracts};
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
    use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
    use repo_graph_graph::{MergedGraph, RepoGraph, SymbolTable};

    fn msg(ty: &str, window: &str) -> Cell {
        Cell {
            kind: cell_type::MESSAGE_TYPE,
            payload: CellPayload::Json(format!(
                r#"{{"type":"{ty}","raw":"pb.{ty}","form":"generic","window":"{window}"}}"#
            )),
        }
    }

    /// One queue node per `(kind, topic, cells)`, all in one repo, plus the
    /// given producer→consumer QUEUE_FLOWS cross edges (by index).
    fn graph(nodes: Vec<(bool, &str, Vec<Cell>)>, flows: &[(usize, usize)]) -> (MergedGraph, Vec<NodeId>) {
        let repo = RepoId::from_canonical("test://contracts");
        let mut nav = CodeNav::default();
        let mut out = Vec::new();
        let mut ids = Vec::new();
        for (producer, topic, cells) in nodes {
            let (kind, prefix) = if producer {
                (node_kind::QUEUE_PRODUCER, "queue_producer:")
            } else {
                (node_kind::QUEUE_CONSUMER, "queue_consumer:")
            };
            let qname = format!("{prefix}{topic}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
            nav.record(id, topic, &qname, kind, None);
            out.push(Node { id, repo, confidence: Confidence::Medium, cells });
            ids.push(id);
        }
        let g = RepoGraph {
            repo,
            nodes: out,
            edges: vec![],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let mut merged = MergedGraph::new(vec![g]);
        for (p, c) in flows {
            merged.cross_edges.push(Edge {
                from: ids[*p],
                to: ids[*c],
                category: edge_category::QUEUE_FLOWS,
                confidence: Confidence::Medium,
            });
        }
        (merged, ids)
    }

    fn side_of(merged: &MergedGraph, id: NodeId) -> MessageContractSide {
        let nodes = super::collect_queue_nodes(merged);
        contract_side(merged, id.0, &nodes[&id.0])
    }

    #[test]
    fn whole_file_type_makes_the_verdict_weak() {
        let (m, ids) = graph(
            vec![
                (true, "orders", vec![msg("OrderCreated", "file")]),
                (false, "orders", vec![msg("OrderCreated", "near")]),
            ],
            &[(0, 1)],
        );
        let (p, c) = (side_of(&m, ids[0]), side_of(&m, ids[1]));
        let (status, confidence, note) = contract_verdict(&p, &c);
        assert_eq!((status, confidence), ("match", "weak"));
        assert!(note.is_some_and(|n| n.contains("whole file")));
    }

    #[test]
    fn near_type_beats_an_earlier_whole_file_and_primitive_read() {
        let (m, ids) = graph(
            vec![(
                true,
                "orders",
                vec![msg("string", "near"), msg("Legacy", "file"), msg("OrderCreated", "near")],
            )],
            &[],
        );
        let s = side_of(&m, ids[0]);
        assert_eq!(s.message_type.as_deref(), Some("OrderCreated"));
        assert_eq!(s.types_seen, vec!["string", "Legacy", "OrderCreated"]);
        // `Legacy` is a whole-file guess and `string` a primitive: neither
        // contradicts the near schema read.
        assert!(!s.conflicting);
    }

    #[test]
    fn two_near_schema_types_are_a_conflict_not_a_verdict() {
        let (m, _) = graph(
            vec![
                (true, "orders", vec![msg("OrderCreated", "near"), msg("OrderPlaced", "near")]),
                (false, "orders", vec![msg("OrderCreated", "near")]),
            ],
            &[(0, 1)],
        );
        let rows = message_contracts(&m);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].producer.as_ref().unwrap().conflicting);
        assert_eq!(rows[0].status, "unknown");
        assert!(rows[0].note.is_some_and(|n| n.contains("conflicting")));
    }

    #[test]
    fn malformed_payload_is_no_type_and_no_panic() {
        let bad = Cell {
            kind: cell_type::MESSAGE_TYPE,
            payload: CellPayload::Json("{not json".into()),
        };
        let untyped = Cell {
            kind: cell_type::MESSAGE_TYPE,
            payload: CellPayload::Json(r#"{"raw":"x"}"#.into()),
        };
        let (m, ids) = graph(
            vec![(true, "orders", vec![bad, untyped]), (false, "orders", vec![])],
            &[(0, 1)],
        );
        let rows = message_contracts(&m);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].producer.as_ref().unwrap().message_type, None);
        assert_eq!(rows[0].status, "unknown");
        assert_eq!(rows[0].note, Some(super::NOTE_NO_TYPES));
        assert_eq!(side_of(&m, ids[0]).types_seen, Vec::<String>::new());
    }

    /// Same topic, no QUEUE_FLOWS edge = the resolver refused the pair (the
    /// family gate). The report must say so, not claim nobody is listening,
    /// and must not pair them itself.
    #[test]
    fn unpaired_same_topic_is_named_as_a_family_split() {
        let (m, _) = graph(
            vec![
                (true, "jobs", vec![msg("Job", "near")]),
                (false, "jobs", vec![msg("Job", "near")]),
            ],
            &[],
        );
        let rows = message_contracts(&m);
        assert_eq!(rows.len(), 2, "two one-sided rows, never a pair the resolver refused");
        assert!(rows.iter().all(|r| r.status == "unknown"));
        assert!(rows.iter().all(|r| r.note == Some(super::NOTE_FAMILY_SPLIT)));
    }

    /// A wildcard subscriber the resolver matched keeps its own pattern topic;
    /// the row is keyed by the producer's concrete topic.
    #[test]
    fn pattern_pair_is_keyed_by_the_producer_topic() {
        let (m, _) = graph(
            vec![
                (true, "orders.created", vec![msg("OrderCreated", "near")]),
                (false, "orders.*", vec![msg("OrderCreated", "near")]),
            ],
            &[(0, 1)],
        );
        let rows = message_contracts(&m);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].topic, "orders.created");
        assert!(rows[0].pattern);
        assert_eq!(rows[0].consumer.as_ref().unwrap().topic, "orders.*");
        assert_eq!((rows[0].status, rows[0].confidence), ("match", "strong"));
    }

    /// Belt and braces: even if an edge between two tags existed, the report
    /// refuses to pair them.
    #[test]
    fn tag_edge_is_never_a_pair() {
        let (m, _) = graph(
            vec![
                (true, "unresolved:kafka", vec![msg("A", "near")]),
                (false, "unresolved:kafka", vec![msg("A", "near")]),
            ],
            &[(0, 1)],
        );
        let rows = message_contracts(&m);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.topic_is_tag && r.note == Some(super::NOTE_TAG)));
    }
}
