// ============================================================================
// P3 answer-shaped primitives (handoff v6) — rank + locate + why, in the ENGINE
// so the CLI, pyo3/MCP, and future TUI/3d-viewer all share one implementation.
// ============================================================================

use std::collections::HashMap;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{CellPayload, Edge, NodeId};
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
