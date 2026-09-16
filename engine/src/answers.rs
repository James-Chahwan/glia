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
pub fn blast_radius_by_qname(
    merged: &MergedGraph,
    qname: &str,
    direction: &str,
    max_depth: usize,
    top_k: Option<usize>,
    live_only: bool,
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
    let mut out: Vec<BlastAnswer> = hits
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
pub fn resolve_signal_located(
    merged: &MergedGraph,
    text: &str,
    kind: &str,
    top_k: Option<usize>,
) -> Vec<LocatedNode> {
    let seeds = merged.resolve_signal(text, kind);
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
pub fn governing_docs(merged: &MergedGraph, qname: &str) -> Result<Vec<LocatedNode>, String> {
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
                if let CellPayload::Json(s) | CellPayload::Text(s) = &c.payload {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                        file = v.get("file").and_then(|f| f.as_str()).map(String::from);
                        line = v.get("start_line").and_then(serde_json::Value::as_i64);
                    }
                }
            }
        }
        return (name, qname, kind, file, line);
    }
    (String::new(), format!("(unknown:{})", id.0), "UNKNOWN", None, None)
}
