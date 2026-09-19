// ============================================================================
// P3 answer-shaped primitives (handoff v6) — rank + locate + why, in the ENGINE
// so the CLI, pyo3/MCP, and future TUI/3d-viewer all share one implementation.
// ============================================================================

use std::collections::{BTreeMap, BTreeSet, HashMap};

use repo_graph_code_domain::{cell_type, edge_category, endpoint, node_kind};
use repo_graph_code_extractors::queues::is_framework_tag;
use repo_graph_core::{Cell, CellPayload, Edge, Node, NodeId};
use repo_graph_graph::{MergedGraph, Reach, RepoGraph};

use crate::absence::{self, Answer};
use crate::find::{self, FindOptions};

/// One node in a blast-radius answer: identity + kind + why-it's-here (`reason`)
/// + PPR `score` + `file`:`line` + `live`. Serialized straight to pyo3/CLI.
/// Produced by [`blast_radius_by_qname`], never built by a struct literal
/// outside this crate (LD.9):
///
/// ```compile_fail
/// let _ = repo_graph_engine::BlastAnswer {
///     id: 0,
///     qname: String::new(),
///     name: String::new(),
///     kind: "",
///     reason: "",
///     depth: 0,
///     score: 0.0,
///     live: false,
///     file: None,
///     line: None,
/// };
/// ```
#[derive(serde::Serialize)]
#[non_exhaustive]
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
    /// 1-based (see [`Located`]).
    pub line: Option<i64>,
}

/// Is this node an entrypoint — an externally-triggered root from which live
/// code is reachable? Routes, gRPC/WS/event handlers, CLI commands, framework
/// components, and `main`/`test*` functions.
///
/// `roles` is the node's `repo_graph_graph::roles::roles_in` (LB.3b). Since
/// the LB.3a fold an Angular `@Component` is a CLASS carrying ROLE COMPONENT,
/// not a COMPONENT node, so a COMPONENT role is an entry exactly as the
/// COMPONENT kind is. The other roles (SERVICE, HOOK, COMPOSABLE, DIRECTIVE,
/// PIPE, GUARD) were never entry kinds and stay non-entries. Pass `&[]` to ask
/// whether the kind / name alone make the node an entry.
fn is_entrypoint(
    kind: Option<repo_graph_core::NodeKindId>,
    name: &str,
    roles: &[repo_graph_core::NodeKindId],
) -> bool {
    if roles.contains(&node_kind::COMPONENT) {
        return true;
    }
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

/// Types whose liveness a live METHOD they declare implies (A7.8): the
/// method's direct `parent_of` must be one of these for the owner step to fire.
const OWNER_KINDS: [repo_graph_core::NodeKindId; 3] =
    [node_kind::CLASS, node_kind::STRUCT, node_kind::INTERFACE];

/// One liveness walk: the live set plus how each part of it got there, for
/// the `[live]` marker and the unit tests.
struct LiveWalk {
    live: std::collections::HashSet<NodeId>,
    /// Entries by kind / name.
    by_kind: usize,
    /// Entries only a ROLE cell made (LB.3b).
    by_role: usize,
    /// Types made live by a live METHOD they declare.
    owners: usize,
    /// Nodes made live by an IMPLEMENTS edge into a live node.
    implementers: usize,
    /// Every node in the merged graph.
    total: usize,
}

/// The entrypoint-reachable ("live") node set: every entrypoint plus everything
/// forward-reachable from one along semantic carry edges. A node absent from
/// this set is likely dead code. Conservative (generous entrypoint set) to avoid
/// false-dead flags — the failure mode the handoff warns about.
///
/// Two steps besides the forward carry walk (A7.8), each applied as a node is
/// popped from the queue:
/// - **Owner.** A live METHOD makes its owning CLASS / STRUCT / INTERFACE (its
///   direct `parent_of`) live. DEFINES stays out of `blast_carry_edges`, since
///   pulling in a whole container is the impact fan-out that list exists to
///   prevent. Liveness is a different question: a controller whose action is
///   route-reachable is not dead, and it is the CLASS, not the METHOD, that
///   owns the INJECTS edges to its services. One level, upward only: no climb
///   to MODULE and no walk down to sibling methods.
/// - **Implementer.** A live node makes every node with an IMPLEMENTS edge
///   into it live: the implementing CLASS of a live INTERFACE, and (A6.6's
///   method-level IMPLEMENTS) the implementing METHOD of a live interface
///   METHOD. Interface-typed DI (`UsersController -INJECTS-> IUserService`) is
///   otherwise the canonical false-dead: the real `UserService` has no inbound
///   carry edge. INHERITS_FROM stays forward-only: a live base class does not
///   make its subclasses live.
///
/// Prints `[live] entrypoints=E (kind=K role=R) owners=O implementers=I
/// reached=N/M` once per process (MCP sessions call this per query): `kind`
/// counts entries by kind / name, `role` the ones only a ROLE cell made entries
/// (LB.3b), `owners` / `implementers` the nodes each A7.8 step made live, `N`
/// the live set and `M` every node. The returned set does not depend on
/// whether the line was printed.
pub fn entrypoint_reachable(merged: &MergedGraph) -> std::collections::HashSet<NodeId> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static PRINTED: AtomicBool = AtomicBool::new(false);

    let w = live_walk(merged);
    if !PRINTED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "[live] entrypoints={} (kind={} role={}) owners={} implementers={} reached={}/{}",
            w.by_kind + w.by_role,
            w.by_kind,
            w.by_role,
            w.owners,
            w.implementers,
            w.live.len(),
            w.total
        );
    }
    w.live
}

/// The walk behind [`entrypoint_reachable`]. Every iteration is over a `Vec`
/// (`merged.graphs`, `g.nodes`, the collected edges); the maps are only looked
/// up, never iterated, so the counts are deterministic.
fn live_walk(merged: &MergedGraph) -> LiveWalk {
    use std::collections::{HashSet, VecDeque};

    let carry: HashSet<repo_graph_core::EdgeCategoryId> =
        repo_graph_graph::blast_carry_edges().into_iter().collect();
    let edges: Vec<&Edge> = merged.all_edges().collect();
    // Reverse IMPLEMENTS: interface (or interface method) -> its implementers.
    let mut implementers_of: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for e in &edges {
        if e.category == edge_category::IMPLEMENTS {
            implementers_of.entry(e.to).or_default().push(e.from);
        }
    }

    let mut w = LiveWalk {
        live: HashSet::new(),
        by_kind: 0,
        by_role: 0,
        owners: 0,
        implementers: 0,
        total: 0,
    };
    let mut owner_of: HashMap<NodeId, NodeId> = HashMap::new();
    let mut queue: VecDeque<NodeId> = VecDeque::new();
    for g in &merged.graphs {
        w.total += g.nodes.len();
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            if kind == Some(node_kind::METHOD)
                && let Some(&p) = g.nav.parent_of.get(&n.id)
                && g.nav.kind_by_id.get(&p).is_some_and(|k| OWNER_KINDS.contains(k))
            {
                owner_of.insert(n.id, p);
            }
            let name = g.nav.name_by_id.get(&n.id).map(String::as_str).unwrap_or("");
            let seeded = if is_entrypoint(kind, name, &[]) {
                &mut w.by_kind
            } else if is_entrypoint(kind, name, &repo_graph_graph::roles::roles_in(kind, &n.cells))
            {
                &mut w.by_role
            } else {
                continue;
            };
            if w.live.insert(n.id) {
                *seeded += 1;
                queue.push_back(n.id);
            }
        }
    }
    while let Some(node) = queue.pop_front() {
        if let Some(&owner) = owner_of.get(&node)
            && w.live.insert(owner)
        {
            w.owners += 1;
            queue.push_back(owner);
        }
        for &imp in implementers_of.get(&node).map(Vec::as_slice).unwrap_or(&[]) {
            if w.live.insert(imp) {
                w.implementers += 1;
                queue.push_back(imp);
            }
        }
        for e in &edges {
            if e.from == node && carry.contains(&e.category) && w.live.insert(e.to) {
                queue.push_back(e.to);
            }
        }
    }
    w
}

/// The node a user-supplied qname or bare name means (LA.14). Candidates are
/// the nodes whose qname is exactly `q`, or — only when none is — whose simple
/// name is. With a `scope` (a path or a project label, see [`resolve_scope`])
/// and more than one candidate, the ones LOCATED under the scope are preferred:
/// `blast-radius handle --scope services/api` means the api `handle`, not the
/// busier web one whose whole radius the scope would then filter away.
///
/// Scope is a preference, never a filter: when no candidate is located under
/// it, the pick is the unscoped one, so `blast-radius shared_fn --scope
/// services/api` still answers "what in services/api uses this shared fn".
/// An unlocatable candidate does not count as in scope here — the
/// keep-unlocatable rule of [`node_in_scope`] is about not DROPPING answers,
/// and preferring a node nobody can place over one placed in scope would undo
/// the preference. Ties inside either set go through
/// [`MergedGraph::pick_primary`]. `scope = None` is exactly
/// `node_id_by_qname(q).or_else(|| resolve_name(q))`.
pub fn resolve_seed(merged: &MergedGraph, q: &str, scope: Option<&str>) -> Option<NodeId> {
    let by_qname = merged.qnames_exact(q);
    let candidates = if by_qname.is_empty() { merged.names_exact(q) } else { by_qname };
    let primary = merged.pick_primary(&candidates);
    let Some(raw) = scope.filter(|_| candidates.len() > 1) else { return primary };
    let path = resolve_scope(merged, raw);
    let loc = Locator::new(merged);
    let inside: Vec<NodeId> = candidates
        .iter()
        .copied()
        .filter(|&id| scope_file_of(&loc, id).is_some_and(|f| in_scope(&f, &path)))
        .collect();
    let pick = merged.pick_primary(&inside).or(primary);
    if let Some(id) = pick.filter(|&id| Some(id) != primary) {
        // fired_on marker (LA.14): only when the scope changed the pick.
        eprintln!(
            "[scope] seed '{q}' -> {} ({} of {} candidates in scope {path})",
            loc.locate(id).qname,
            inside.len(),
            candidates.len()
        );
    }
    pick
}

/// `blast_radius`, resolved from a qname/name and fully located — the P3 answer
/// that `find`→`impact`→`activate`→`read×N` collapses to. `direction` ∈
/// {`forward`, `backward`, `both`}. Err on an unknown qname or bad direction.
///
/// `scope` (A8.3) restricts the answer to nodes whose file lives under that
/// repo-relative path, applied BEFORE `top_k` truncation so a scoped `--top-k`
/// spends its whole budget in scope instead of on whatever PPR liked globally.
/// A node with no locatable file (DOC_SPACE, a handler-less route — see
/// [`node_in_scope`]) is KEPT, never dropped. ENDPOINT / ROUTE nodes are
/// located (A3.6) and scoped by where they are defined. `scope` narrows
/// WITHIN a repo — under a multi-repo merge each repo's POSITION paths are
/// relative to its OWN root. The seed is [`resolve_seed`] under the same
/// `scope` (LA.14): an ambiguous bare name starts from its in-scope candidate.
pub fn blast_radius_by_qname(
    merged: &MergedGraph,
    qname: &str,
    direction: &str,
    max_depth: usize,
    top_k: Option<usize>,
    live_only: bool,
    scope: Option<&str>,
) -> Result<Vec<BlastAnswer>, String> {
    let seed = resolve_seed(merged, qname, scope)
        .ok_or_else(|| format!("no node with qname/name `{qname}`"))?;
    let reach = match direction {
        "forward" => Reach::Forward,
        "backward" => Reach::Backward,
        "both" => Reach::Both,
        o => return Err(format!("direction must be forward|backward|both, got `{o}`")),
    };
    let live = entrypoint_reachable(merged);
    let hits = merged.blast_radius(seed, reach, max_depth, None);
    let loc = Locator::new(merged);
    let out: Vec<BlastAnswer> = hits
        .iter()
        .filter(|h| !live_only || live.contains(&h.id))
        .map(|h| {
            let at = loc.locate(h.id);
            BlastAnswer {
                id: h.id.0,
                qname: at.qname,
                name: at.name,
                kind: at.kind,
                reason: edge_category::name(h.reason),
                depth: h.depth,
                score: h.score,
                live: live.contains(&h.id),
                file: at.file,
                line: at.line,
            }
        })
        .collect();
    // Scope BEFORE the cut: filtering after `truncate` would spend the budget
    // on out-of-scope nodes and return fewer (or zero) in-scope answers.
    let mut out = apply_scope(&loc, out, scope, |a| NodeId(a.id), "blast_radius");
    if let Some(k) = top_k {
        out.truncate(k);
    }
    Ok(out)
}

/// One hop in a cross-stack trace: a typed edge from one entity to the next,
/// with the `mechanism` (edge category) and whether it crossed a service
/// boundary. The destination is located.
#[derive(serde::Serialize)]
#[non_exhaustive]
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
    /// 1-based (see [`Located`]).
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
    let loc = Locator::new(merged);

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
                let from = loc.locate(e.from);
                let to = loc.locate(e.to);
                let cross_service = repo_of.get(&e.from) != repo_of.get(&e.to);
                hops.push(TraceHop {
                    depth: depth + 1,
                    mechanism: edge_category::name(e.category),
                    cross_service,
                    from_qname: from.qname,
                    to_qname: to.qname,
                    to_kind: to.kind,
                    to_file: to.file,
                    to_line: to.line,
                });
                queue.push_back((e.to, depth + 1));
            }
        }
    }
    Ok(hops)
}

/// One located node in a `resolve` answer: identity + kind + PPR relevance +
/// `file`:`line`. (No `reason`/`depth` — `resolve` locates seeds, it doesn't walk.)
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct LocatedNode {
    pub id: u64,
    pub qname: String,
    pub name: String,
    pub kind: &'static str,
    pub score: f64,
    pub file: Option<String>,
    /// 1-based (see [`Located`]).
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
///
/// LD.8a: an empty answer carries its [`Absence`](crate::absence::Absence).
/// `no_signal_match` when the signal resolved to no node — its note counts
/// the frames / added lines / paths / test ids the signal held, and it names
/// no mechanism, because resolution is by POSITION file and line or by name,
/// not by edges. `no_match` when `scope` (or `top_k = Some(0)`) removed every
/// resolved node.
pub fn resolve_signal_located(
    merged: &MergedGraph,
    text: &str,
    kind: &str,
    top_k: Option<usize>,
    scope: Option<&str>,
) -> Answer<LocatedNode> {
    let seeds = merged.resolve_signal(text, kind);
    let resolved = seeds.len();
    let loc = Locator::new(merged);
    // Pre-PPR: `activate` below must only see in-scope seeds.
    let seeds = apply_scope(&loc, seeds, scope, |id| *id, "resolve");
    if seeds.is_empty() {
        return Answer::from_results(Vec::new(), || match scope {
            Some(s) if resolved > 0 => absence::scope_emptied(merged, "resolve", text, resolved, s),
            _ => absence::empty(
                merged,
                "resolve",
                text,
                "no_signal_match",
                signal_note(text, kind),
                &[],
                None,
            ),
        });
    }
    let mut config = repo_graph_graph::code_activation_defaults();
    config.direction = repo_graph_activation::Direction::Undirected;
    config.top_k = usize::MAX;
    let scores: HashMap<NodeId, f64> =
        merged.activate(&seeds, &config).scores.into_iter().collect();
    let mut out: Vec<LocatedNode> = seeds
        .iter()
        .map(|id| {
            let at = loc.locate(*id);
            LocatedNode {
                id: id.0,
                qname: at.qname,
                name: at.name,
                kind: at.kind,
                score: scores.get(id).copied().unwrap_or(0.0),
                file: at.file,
                line: at.line,
            }
        })
        .collect();
    let kept = out.len();
    if let Some(k) = top_k {
        out.truncate(k);
    }
    Answer::from_results(out, || {
        let note = format!(
            "top_k 0 kept none of the {kept} resolved {}",
            absence::plural(kept, "node", "nodes")
        );
        absence::empty(merged, "resolve", text, "no_match", note, &[], None)
    })
}

/// The `no_signal_match` note: what the signal held, counted with the shapes
/// the graph crate's resolver reads.
///
/// MIRRORS `graph/src/signal.rs` (`sniff_signal_kind`, `parse_stack_frames`,
/// `parse_diff_frames` and the plain changed-file-list fallback), which are
/// private there, while `MergedGraph::resolve_signal` returns node ids only
/// and reports no input count. Removal: when the graph crate exposes a signal
/// inventory, call it and delete this, [`sniff_kind`], [`frames_in_line`] and
/// [`added_lines`].
fn signal_note(text: &str, kind: &str) -> String {
    let (kind, auto) = if kind == "auto" {
        (sniff_kind(text), " (auto-detected)")
    } else {
        (kind, "")
    };
    let (n, one, many) = match kind {
        "stacktrace" => (
            text.lines().map(frames_in_line).sum(),
            "stack frame",
            "stack frames",
        ),
        "diff" => match added_lines(text) {
            0 => (
                text.lines()
                    .filter(|l| !l.trim().is_empty() && l.contains('.'))
                    .count(),
                "path",
                "paths",
            ),
            n => (n, "added line", "added lines"),
        },
        "test" => (
            text.split_whitespace().filter(|t| t.contains("::")).count(),
            "test id",
            "test ids",
        ),
        other => {
            return format!(
                "`{other}` is not a signal kind (stacktrace, test, diff or auto), so nothing was resolved"
            );
        }
    };
    format!(
        "the {kind} signal{auto} held {n} {}; none resolved to a node in this graph",
        absence::plural(n, one, many)
    )
}

/// MIRRORS `sniff_signal_kind` (see [`signal_note`]).
fn sniff_kind(text: &str) -> &'static str {
    if text.contains("+++ ") || text.contains("--- a/") || text.contains("\n@@ ") {
        return "diff";
    }
    if (text.contains("File \"") && text.contains("line "))
        || text.contains(".go:")
        || text.contains("\n  at ")
    {
        return "stacktrace";
    }
    let t = text.trim();
    if t.contains("::") && !t.chars().any(char::is_whitespace) {
        return "test";
    }
    "stacktrace"
}

/// Frames `parse_stack_frames` reads from one line: a Python `File "x", line
/// N` frame, else every `path.ext:N` token (see [`signal_note`]).
fn frames_in_line(line: &str) -> usize {
    if let Some(rest) = line.trim_start().strip_prefix("File \"")
        && let Some(end) = rest.find('"')
        && let Some(at) = rest[end..].find("line ")
        && rest[end + at + 5..].starts_with(|c: char| c.is_ascii_digit())
    {
        return 1;
    }
    line.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',')
        .filter(|tok| {
            let mut parts = tok.split(':');
            let path = parts.next().unwrap_or("");
            !path.is_empty()
                && path.contains('.')
                && !path.ends_with('.')
                && parts
                    .next()
                    .is_some_and(|n| n.starts_with(|c: char| c.is_ascii_digit()))
        })
        .count()
}

/// Added lines `parse_diff_frames` turns into frames: `+` lines under a
/// `+++ ` header that names a file, not `/dev/null` (see [`signal_note`]).
fn added_lines(text: &str) -> usize {
    let mut in_file = false;
    let mut n = 0;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("+++ ") {
            let p = p.split('\t').next().unwrap_or(p).trim();
            in_file = p != "/dev/null";
        } else if in_file && line.starts_with('+') {
            n += 1;
        }
    }
    n
}

/// **governing_docs** (P3 payoff, tier-4): the doc sections that DOCUMENTS a
/// symbol — "what are the rules for X?" — located, in one call. Direct
/// `doc --DOCUMENTS--> symbol` predecessors (the conservative, precise linker
/// signal). Reuses `LocatedNode` (score = 0; docs aren't PPR-ranked here).
///
/// `scope` (A8.3) keeps only the doc sections whose own POSITION file lives
/// under that repo-relative path. A section with no locatable file is KEPT (see
/// [`node_in_scope`]).
///
/// The symbol resolves through find's search (the LD.3b handoff): its first
/// row, when that row is an exact match (`exact_qname` / `exact_name`). Those
/// tiers are ordered by `pick_primary`'s key, so it is the node
/// `node_id_by_qname` / `resolve_name` return; a dotted or slashed query
/// (`app.helper`) also finds its `::` qname, as find does.
///
/// LD.8a: never an error. An empty answer's absence is `unknown_symbol`
/// (find's nearest qnames as suggestions), `no_edges` (no DOCUMENTS edge
/// reaches the symbol; caveats narrowed to the symbol's language), or
/// `no_match` (`scope` removed every section).
pub fn governing_docs(
    merged: &MergedGraph,
    qname: &str,
    scope: Option<&str>,
) -> Answer<LocatedNode> {
    // The rows are DOC_SECTIONs; the table names the edge a section reaches a
    // symbol through.
    let mechanisms = absence::mechanisms_for_kind(node_kind::DOC_SECTION);
    let near_opts = FindOptions {
        top_k: absence::SUGGESTIONS,
        ..FindOptions::default()
    };
    let near = find::search(merged, qname, &near_opts).rows;
    let Some(target) = near
        .first()
        .filter(|r| find::is_exact(r))
        .map(|r| NodeId(r.id))
    else {
        return Answer::from_results(Vec::new(), || {
            absence::unknown_symbol(merged, "governing_docs", qname, mechanisms, &near)
        });
    };
    let loc = Locator::new(merged);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for e in merged.all_edges() {
        if e.to == target
            && e.category == edge_category::DOCUMENTS
            && seen.insert(e.from)
        {
            let at = loc.locate(e.from);
            out.push(LocatedNode {
                id: e.from.0,
                qname: at.qname,
                name: at.name,
                kind: at.kind,
                score: 0.0,
                file: at.file,
                line: at.line,
            });
        }
    }
    let documented = out.len();
    let out = apply_scope(&loc, out, scope, |d| NodeId(d.id), "governing_docs");
    Answer::from_results(out, || match scope {
        Some(s) if documented > 0 => {
            absence::scope_emptied(merged, "governing_docs", qname, documented, s)
        }
        _ => {
            let at = loc.locate(target);
            let note = format!("no DOCUMENTS edge reaches `{}` in this graph", at.qname);
            absence::empty(
                merged,
                "governing_docs",
                qname,
                "no_edges",
                note,
                mechanisms,
                at.file.as_deref(),
            )
        }
    })
}

/// Identity + location of one node, shared by every answer record.
///
/// **`line` is 1-based** — the first line of the node's span as an editor
/// shows it; `None` when no tier places the node. This is the ONE line
/// convention of every answer record (`blast_radius_by_qname`,
/// `cross_stack_trace`, `resolve_signal_located`, `governing_docs`,
/// `message_contracts`) and of the pyo3 `nodes_json` span. POSITION cells keep
/// storing 0-based tree-sitter rows; [`Locator::locate`] converts, once, at its
/// single exit. A record builder copies `line` as-is and never adds 1 again.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Located {
    pub id: u64,
    pub name: String,
    pub qname: String,
    pub kind: &'static str,
    pub file: Option<String>,
    pub line: Option<i64>,
}

/// A node index over one [`MergedGraph`], built once in O(V) so an answer with
/// R located rows costs O(V + R) instead of the O(R × V) of a per-row scan.
/// Build one per answer and pass it down; never build one per row.
///
/// Placement has three tiers, first hit wins:
///
/// 1. the node's first POSITION cell ([`position_of`]) — every parsed entity,
///    and the go / ts_routes ROUTEs (a POSITION per registration);
/// 2. ROUTE / ENDPOINT only (A3.6): the ENDPOINT_HIT cell or a JSON
///    ROUTE_METHOD cell ([`endpoint::http_node_span`]), which carry the call
///    or registration site;
/// 3. ROUTE only (A3.6): the POSITION of the handler the route is HANDLED_BY,
///    for the parsers whose ROUTE_METHOD is the bare verb. "Where is this
///    route" is answered by its handler.
///
/// All three produce 0-based ROWS internally; only [`Locator::locate`] turns a
/// row into a 1-based `line`. `None` for nodes no tier places (DOC_SPACE, a
/// handler-less Django/Rails route).
pub struct Locator<'a> {
    merged: &'a MergedGraph,
    /// NodeId → index of the FIRST graph (in `merged.graphs` order) whose nav
    /// names it. Built with `or_insert` over graphs in Vec order, so the
    /// per-graph HashMap iteration order cannot change the winner.
    first_graph: HashMap<NodeId, usize>,
    /// Per graph: NodeId → index of its FIRST node in `g.nodes` with that id.
    node_at: Vec<HashMap<NodeId, usize>>,
}

impl<'a> Locator<'a> {
    /// Index every graph of `merged`. Prints the LD.1 fired_on marker once per
    /// process (a Locator is built per answer, so a per-build line would flood
    /// a long-running MCP server's stderr).
    pub fn new(merged: &'a MergedGraph) -> Self {
        let mut first_graph: HashMap<NodeId, usize> = HashMap::new();
        let mut node_at: Vec<HashMap<NodeId, usize>> = Vec::with_capacity(merged.graphs.len());
        for (gi, g) in merged.graphs.iter().enumerate() {
            for id in g.nav.qname_by_id.keys() {
                first_graph.entry(*id).or_insert(gi);
            }
            let mut at: HashMap<NodeId, usize> = HashMap::with_capacity(g.nodes.len());
            for (ni, n) in g.nodes.iter().enumerate() {
                at.entry(n.id).or_insert(ni);
            }
            node_at.push(at);
        }
        static BUILT: std::sync::Once = std::sync::Once::new();
        BUILT.call_once(|| {
            eprintln!(
                "[locate] locator built: nodes={} graphs={} line_base=1",
                first_graph.len(),
                merged.graphs.len()
            );
        });
        Locator { merged, first_graph, node_at }
    }

    /// Identity + 1-based location of `id`. An id no graph names comes back as
    /// `qname: "(unknown:<id>)"`, `kind: "UNKNOWN"`, unlocated. A node named
    /// by a graph's nav but absent from its node list keeps its name and qname
    /// and is unlocated.
    pub fn locate(&self, id: NodeId) -> Located {
        let Some((gi, g)) = self.graph_of(id) else {
            return Located {
                id: id.0,
                name: String::new(),
                qname: format!("(unknown:{})", id.0),
                kind: "UNKNOWN",
                file: None,
                line: None,
            };
        };
        let name = g.nav.name_by_id.get(&id).cloned().unwrap_or_default();
        let qname = g.nav.qname_by_id.get(&id).cloned().unwrap_or_default();
        let kind_id = g.nav.kind_by_id.get(&id).copied();
        let (file, row) = self.place(gi, g, id, &qname);
        Located {
            id: id.0,
            name,
            qname,
            kind: kind_id.map(node_kind::name).unwrap_or("UNKNOWN"),
            file,
            // THE conversion: a 0-based stored row becomes a 1-based line here
            // and nowhere else. Saturating, so a corrupt i64::MAX row cannot
            // overflow-panic a debug build.
            line: row.map(|r| r.saturating_add(1)),
        }
    }

    /// The repo-relative file [`Locator::locate`] reports for `id` — what the
    /// A8.3 scope filter keys on — without cloning the name and qname.
    pub(crate) fn file_of(&self, id: NodeId) -> Option<String> {
        let (gi, g) = self.graph_of(id)?;
        let qname = g.nav.qname_by_id.get(&id).map(String::as_str).unwrap_or("");
        self.place(gi, g, id, qname).0
    }

    fn graph_of(&self, id: NodeId) -> Option<(usize, &'a RepoGraph)> {
        let gi = *self.first_graph.get(&id)?;
        Some((gi, self.merged.graphs.get(gi)?))
    }

    fn node(&self, gi: usize, g: &'a RepoGraph, id: NodeId) -> Option<&'a Node> {
        g.nodes.get(*self.node_at.get(gi)?.get(&id)?)
    }

    /// `(file, 0-based row)` through the three tiers.
    fn place(
        &self,
        gi: usize,
        g: &'a RepoGraph,
        id: NodeId,
        qname: &str,
    ) -> (Option<String>, Option<i64>) {
        let Some(n) = self.node(gi, g, id) else {
            return (None, None);
        };
        let placed = position_of(n);
        if placed.0.is_some() {
            return placed;
        }
        let kind_id = g.nav.kind_by_id.get(&id).copied();
        let is_route = kind_id == Some(node_kind::ROUTE);
        if !is_route && kind_id != Some(node_kind::ENDPOINT) {
            return placed;
        }
        if let Some((f, l)) = endpoint::http_node_span(&n.cells) {
            log_http_locate_once("cell", qname);
            return (Some(f), l);
        }
        if is_route && let Some((f, l)) = self.handler_position(gi, g, id) {
            log_http_locate_once("handled_by", qname);
            return (Some(f), l);
        }
        placed
    }

    /// Tier 3: the POSITION of the first handler (in edge order) that `route`
    /// is HANDLED_BY and that carries a file. Confined to the route's own repo
    /// graph — a ROUTE's HANDLED_BY never crosses repos — so it costs one scan
    /// of that repo's edges, and only for a route tiers 1–2 could not place.
    /// The handler is found through the node index, not a node scan.
    fn handler_position(
        &self,
        gi: usize,
        g: &'a RepoGraph,
        route: NodeId,
    ) -> Option<(String, Option<i64>)> {
        g.edges
            .iter()
            .filter(|e| e.from == route && e.category == edge_category::HANDLED_BY)
            .find_map(|e| match position_of(self.node(gi, g, e.to)?) {
                (Some(f), l) => Some((f, l)),
                (None, _) => None,
            })
    }
}

/// [`Located`] for one node — a one-off [`Locator`]. O(V) per call: a caller
/// locating more than one node builds a `Locator` once and calls
/// [`Locator::locate`] instead.
pub fn locate_node(merged: &MergedGraph, id: NodeId) -> Located {
    Locator::new(merged).locate(id)
}

/// `(file, start_line)` from a node's FIRST parseable POSITION cell;
/// `(None, None)` when it has none. `start_line` is the stored 0-based row.
///
/// FIRST POSITION WINS — A2.8, and it is load-bearing, not cosmetic. A node
/// can carry MORE than one POSITION cell: `merge_parses` appends the cells of
/// every `FileParse` that minted the same NodeId, which is the normal case
/// for a queue topic two files publish to. Returning on the first parseable
/// cell is what keeps `blast_radius` / `trace` / `resolve` on the same file
/// as `passes::position_file` and `projection_text::node_position`, both of
/// which return the first. Scanning on would let the LAST file parsed win.
fn position_of(n: &Node) -> (Option<String>, Option<i64>) {
    for c in &n.cells {
        if c.kind != cell_type::POSITION {
            continue;
        }
        if let CellPayload::Json(s) | CellPayload::Text(s) = &c.payload
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(s)
        {
            let file = v.get("file").and_then(|f| f.as_str()).map(String::from);
            let line = v.get("start_line").and_then(serde_json::Value::as_i64);
            return (file, line);
        }
    }
    (None, None)
}

/// The A3.6 fired_on marker, once per process PER TIER: [`Locator::locate`]
/// runs inside the P3 answer loops, so a per-call line would flood stderr,
/// while one line per process would hide
/// whichever tier fired second. `source` is `cell` (tier 2) or `handled_by`
/// (tier 3), so a run prints at most two lines.
fn log_http_locate_once(source: &'static str, qname: &str) {
    static CELL: std::sync::Once = std::sync::Once::new();
    static HANDLED_BY: std::sync::Once = std::sync::Once::new();
    let once = if source == "cell" { &CELL } else { &HANDLED_BY };
    once.call_once(|| {
        eprintln!("[locate] http span fallback fired: source={source} node={qname}");
    });
}

// ============================================================================
// A8.3 — `scope`: restrict a P3 answer to one part of a monorepo.
//
// Two rules that are load-bearing rather than cosmetic:
//   1. the filter runs BEFORE `truncate(top_k)` (blast_radius) and BEFORE
//      `activate` (resolve), so it changes the RANKING, not just the display;
//   2. a node with NO locatable file is KEPT. Dropping unlocatable nodes would
//      silently delete every DOC_SPACE (and, before A3.6 located them, every
//      ENDPOINT / ROUTE) from a scoped answer and destroy the cross-service
//      result these primitives exist for. The `[scope]` marker reports the
//      kept-unlocatable count so that is visible.
// ============================================================================

/// The repo-relative path a node should be scoped by: exactly the file
/// [`Locator::locate`] reports. Since A3.6 that places ENDPOINT nodes by their
/// ENDPOINT_HIT call site and ROUTE nodes by their JSON ROUTE_METHOD cell or
/// their HANDLED_BY handler, so an HTTP node is scoped where it is defined
/// instead of being kept as unlocatable. `None` only for the nodes no tier
/// places (DOC_SPACE, a handler-less route) — those fall under the
/// keep-unlocatable rule above.
fn scope_file_of(loc: &Locator<'_>, id: NodeId) -> Option<String> {
    loc.file_of(id)
}

/// True when `file` lives under `scope`. Prefix match on a `/` boundary only,
/// so `scope = "services/ap"` does NOT match `services/api/handler.py`. Both
/// sides are normalised by trimming a leading `./` or `/` and a trailing `/`;
/// an empty scope matches everything, and so does `.` — the path the ROOT
/// project resolves to (A8.6), which otherwise matched nothing at all.
pub(crate) fn in_scope(file: &str, scope: &str) -> bool {
    let f = file.trim_start_matches("./").trim_start_matches('/');
    let s = scope.trim_start_matches("./").trim_matches('/');
    if s.is_empty() || s == "." {
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
///
/// `scope` here is a PATH. A caller holding a user-supplied scope that may be
/// a project label resolves it ONCE with [`resolve_scope`] before looping —
/// this runs per node, and resolving per node would walk the graph N times.
///
/// Builds a one-off [`Locator`] when `scope` is set: O(V) per call, the same
/// order as the node scan it replaced. A caller filtering many nodes builds
/// one `Locator` and filters on its `file` instead.
pub fn node_in_scope(merged: &MergedGraph, id: NodeId, scope: Option<&str>) -> bool {
    let Some(s) = scope else { return true };
    match scope_file_of(&Locator::new(merged), id) {
        Some(f) => in_scope(&f, s),
        None => true,
    }
}

/// One shared applier so the scoped call sites cannot drift apart. `scope =
/// None` is a strict no-op: the input is returned untouched and no marker is
/// emitted, so no existing answer, ranking or cell value changes.
///
/// A8.6: the scope is label-resolved HERE, once per call, so all three scoped
/// primitives — and the pyo3 / CLI surfaces over them — accept `@shop/web` as
/// well as `apps/web` without any of them knowing about labels. Resolution is
/// a pass-through for anything that is not a project, so every A8.3 path
/// scope behaves exactly as before.
fn apply_scope<T>(
    loc: &Locator<'_>,
    items: Vec<T>,
    scope: Option<&str>,
    id_of: impl Fn(&T) -> NodeId,
    what: &str,
) -> Vec<T> {
    let Some(raw) = scope else { return items };
    let resolved = resolve_scope(loc.merged, raw);
    let s = resolved.as_str();
    let before = items.len();
    let mut unlocatable = 0usize;
    let out: Vec<T> = items
        .into_iter()
        .filter(|it| match scope_file_of(loc, id_of(it)) {
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
// A8.6 — project roots as the `scope` vocabulary.
//
// A8.3's scope is a repo-relative path, which is exactly what a human or an
// agent does not know in a monorepo: you know the service is `@shop/web`, not
// that it lives at `apps/web`. A8.5 put both in the graph as PROJECT anchors;
// this reads them back. Nothing is cached and nothing rides on a side field,
// so a freshly generated graph and one reopened from a `.gmap` answer alike.
// ============================================================================

/// One manifest-rooted sub-project: a PROJECT anchor (A8.5) decoded from its
/// ORIGIN cell. `path` is repo-relative, `.` for the repo root.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProjectInfo {
    pub qname: String,
    pub label: String,
    pub ecosystem: String,
    pub manifest: String,
    pub path: String,
}

/// Every PROJECT anchor in the graph, sorted by `path` (then `qname`, for the
/// several `.` roots of a `--with` merge) so callers and tests see a stable
/// order. A node whose ORIGIN cell is missing or will not parse is SKIPPED,
/// not fatal — the same degrade-don't-fail rule `locate_node` applies to a
/// bad POSITION cell.
pub fn project_roots(merged: &MergedGraph) -> Vec<ProjectInfo> {
    let mut out = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::PROJECT) {
                continue;
            }
            let origin = n.cells.iter().filter(|c| c.kind == cell_type::ORIGIN).find_map(|c| {
                match &c.payload {
                    CellPayload::Json(s) | CellPayload::Text(s) => {
                        serde_json::from_str::<serde_json::Value>(s).ok()
                    }
                    CellPayload::Bytes(_) => None,
                }
            });
            let Some(v) = origin.filter(serde_json::Value::is_object) else { continue };
            let field = |k: &str| v.get(k).and_then(serde_json::Value::as_str).map(String::from);
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            // The walker records the root as `""`; `.` is what the qname says
            // and what a human types.
            let path = field("path")
                .or_else(|| qname.strip_prefix("project:").map(String::from))
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| ".".to_string());
            let label = field("label")
                .or_else(|| g.nav.name_by_id.get(&n.id).cloned())
                .unwrap_or_default();
            out.push(ProjectInfo {
                qname,
                label,
                ecosystem: field("ecosystem").unwrap_or_default(),
                manifest: field("manifest").unwrap_or_default(),
                path,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.qname.cmp(&b.qname)));
    out
}

/// Turn a user-supplied scope into a repo-relative path.
///
/// 1. A project's own path (`apps/web`, or `.`) is returned UNCHANGED — so
///    this is idempotent: resolving an already-resolved scope is a no-op, and
///    a path wins over a label that happens to spell the same string.
/// 2. A project's full qname (`project:apps/web`) resolves to its path.
/// 3. An exact `label` match (`@shop/web`) resolves to its path. When several
///    projects share the label (two Maven modules with one artifactId), the
///    lexicographically smallest path wins and a `[scope] ambiguous` warning
///    names all of them — never a panic, never a silent pick.
/// 4. Anything else passes through untouched: a literal path scope.
///
/// O(nodes) per call; the scoped primitives call it once, not per node.
pub fn resolve_scope(merged: &MergedGraph, scope: &str) -> String {
    let roots = project_roots(merged);
    if roots.iter().any(|p| p.path == scope) {
        return scope.to_string();
    }
    if let Some(p) = roots.iter().find(|p| p.qname == scope) {
        eprintln!("[scope] resolved qname '{scope}' -> {}", p.path);
        return p.path.clone();
    }
    // `roots` is sorted by path, so the first match is the smallest path.
    let hits: Vec<&ProjectInfo> = roots.iter().filter(|p| p.label == scope).collect();
    let Some(first) = hits.first() else {
        return scope.to_string();
    };
    if hits.len() > 1 {
        let paths: Vec<&str> = hits.iter().map(|p| p.path.as_str()).collect();
        eprintln!(
            "[scope] ambiguous label '{scope}' matches {} projects ({}); using {}",
            hits.len(),
            paths.join(", "),
            first.path
        );
    }
    // fired_on marker (A8.6). Label path only — a literal path scope never
    // prints it, so it cannot be confused with A8.3's `[scope] <what>` line.
    eprintln!("[scope] resolved label '{scope}' -> {}", first.path);
    first.path.clone()
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
#[non_exhaustive]
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
    /// uses the topic), else the parent MODULE's. `line` is 1-based (see
    /// [`Located`]).
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
#[non_exhaustive]
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
    /// The full qname, LB.8 owner segment included, so a side still says
    /// which project it is.
    qname: &'a str,
    /// The bare topic: the owner segment is not part of the channel.
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
            let Some(topic) = endpoint::split_owner(qname).0.strip_prefix(prefix) else { continue };
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

fn contract_side(loc: &Locator<'_>, id: u64, acc: &QueueNodeAcc<'_>) -> MessageContractSide {
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

    let mut at = loc.locate(NodeId(id));
    if at.file.is_none()
        && let Some(p) = acc.parent
    {
        at = loc.locate(p);
    }
    let (file, line) = (at.file, at.line);
    let module = acc
        .parent
        .and_then(|p| {
            loc.merged.graphs.iter().find_map(|g| g.nav.qname_by_id.get(&p).cloned())
        });

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
    let loc = Locator::new(merged);
    let sides: BTreeMap<u64, MessageContractSide> = nodes
        .iter()
        .map(|(id, acc)| (*id, contract_side(&loc, *id, acc)))
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

    /// A2.8 — FIRST POSITION WINS, and `position_of` returning on the first
    /// parseable cell is what makes it so. `merge_parses` appends the cells of every `FileParse` that
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
        let at = locate_node(&MergedGraph::new(vec![g]), id);
        assert_eq!(at.id, id.0);
        assert_eq!(at.name, "orders");
        assert_eq!(at.qname, "queue_producer:orders");
        assert_eq!(at.kind, "QUEUE_PRODUCER");
        assert_eq!(at.file.as_deref(), Some("a.go"));
        // Stored row 4 (0-based) is reported as line 5 (1-based, LD.1).
        assert_eq!(at.line, Some(5));
    }

    /// A3.6 tier 3 — a bare-verb ROUTE borrows the POSITION of the handler it
    /// is HANDLED_BY. The first handler (edge order) that carries a file
    /// wins, a handler with no POSITION is skipped, and the handler's own
    /// POSITION keeps A2.8's first-wins. A route with no handler, and a
    /// non-HTTP node carrying an HTTP-shaped cell, stay unlocated.
    #[test]
    fn bare_verb_route_is_located_by_its_first_positioned_handler() {
        use repo_graph_code_domain::edge_category;
        use repo_graph_core::Edge;
        let repo = RepoId::from_canonical("test://locate-route");
        let id = |kind, q: &str| NodeId::from_parts(GRAPH_TYPE, repo, kind, q);
        let pos = |file: &str, line: u32| Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":{line},"end_line":{line}}}"#
            )),
        };
        let verb = Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Text("GET".into()),
        };
        let hit = Cell {
            kind: cell_type::ENDPOINT_HIT,
            payload: CellPayload::Json(r#"{"file":"x.ts","line":3}"#.into()),
        };
        let route = id(node_kind::ROUTE, "GET /users");
        let orphan = id(node_kind::ROUTE, "ANY /posts");
        let bare = id(node_kind::FUNCTION, "app::no_span");
        let handler = id(node_kind::FUNCTION, "app::list_users");
        let func_with_hit = id(node_kind::FUNCTION, "app::odd");
        let node = |id, cells| Node { id, repo, confidence: Confidence::Strong, cells };
        let mut nav = CodeNav::default();
        for (nid, q, k) in [
            (route, "GET /users", node_kind::ROUTE),
            (orphan, "ANY /posts", node_kind::ROUTE),
            (bare, "app::no_span", node_kind::FUNCTION),
            (handler, "app::list_users", node_kind::FUNCTION),
            (func_with_hit, "app::odd", node_kind::FUNCTION),
        ] {
            nav.record(nid, q, q, k, None);
        }
        let handled_by = |to| Edge {
            from: route,
            to,
            category: edge_category::HANDLED_BY,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        };
        let g = RepoGraph {
            repo,
            nodes: vec![
                node(route, vec![verb.clone()]),
                node(orphan, vec![verb]),
                node(bare, vec![]),
                node(handler, vec![pos("app.py", 6), pos("other.py", 40)]),
                node(func_with_hit, vec![hit]),
            ],
            edges: vec![handled_by(bare), handled_by(handler)],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let m = MergedGraph::new(vec![g]);
        let at = |nid| {
            let at = locate_node(&m, nid);
            (at.file, at.line)
        };
        // The handler's stored row 6 is line 7 (LD.1).
        assert_eq!(at(route), (Some("app.py".to_string()), Some(7)));
        assert_eq!(at(orphan), (None, None), "no handler, no span");
        assert_eq!(at(func_with_hit), (None, None), "tiers 2-3 are HTTP-only");
    }

    /// LD.1 parity: the Locator index keeps HEAD's scan semantics. The FIRST
    /// graph (Vec order) whose nav names an id owns it, even when a later graph
    /// carries a POSITION for the same id; a nav entry with no node in that
    /// graph keeps its name and qname and is unlocated; an id no graph names is
    /// `(unknown:<id>)`.
    #[test]
    fn locator_keeps_the_first_graph_rule() {
        use super::Locator;
        let repo = RepoId::from_canonical("test://locate-parity");
        let id = |q: &str| NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, q);
        let pos = |file: &str, line: u32| Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":{line},"end_line":{line}}}"#
            )),
        };
        let (shared, nav_only) = (id("m::shared"), id("m::nav_only"));
        let graph = |nodes: Vec<Node>, named: &[NodeId]| {
            let mut nav = CodeNav::default();
            for nid in named {
                let q = if *nid == shared { "m::shared" } else { "m::nav_only" };
                nav.record(*nid, q, q, node_kind::FUNCTION, None);
            }
            RepoGraph {
                repo,
                nodes,
                edges: vec![],
                nav,
                symbols: SymbolTable::default(),
                unresolved_calls: vec![],
                unresolved_refs: vec![],
                properties: Default::default(),
            }
        };
        let node = |nid, cells| Node { id: nid, repo, confidence: Confidence::Strong, cells };
        let m = MergedGraph::new(vec![
            graph(vec![node(shared, vec![pos("a.py", 2)])], &[shared, nav_only]),
            graph(
                vec![node(shared, vec![pos("b.py", 9)]), node(nav_only, vec![pos("c.py", 5)])],
                &[shared, nav_only],
            ),
        ]);
        let loc = Locator::new(&m);
        let at = loc.locate(shared);
        assert_eq!((at.file.as_deref(), at.line), (Some("a.py"), Some(3)), "first graph wins");
        let at = loc.locate(nav_only);
        assert_eq!(at.qname, "m::nav_only");
        assert_eq!((at.file, at.line), (None, None), "nav-only in the owning graph: unlocated");
        let ghost = id("m::ghost");
        let at = loc.locate(ghost);
        assert_eq!(at.qname, format!("(unknown:{})", ghost.0));
        assert_eq!((at.kind, at.name.as_str(), at.line), ("UNKNOWN", "", None));
        assert_eq!(loc.file_of(shared).as_deref(), Some("a.py"), "file_of agrees with locate");
        assert_eq!(locate_node(&m, shared), loc.locate(shared), "the one-off wrapper agrees");
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
        // So is `.` — the root project's path (A8.6).
        assert!(in_scope("web/client.py", "."));
        assert!(in_scope("main.go", "./"));
    }
}

#[cfg(test)]
mod project_scope_tests {
    use super::{project_roots, resolve_scope};
    use repo_graph_code_domain::project_roots::ProjectRoot;
    use repo_graph_core::{CellPayload, RepoId};
    use repo_graph_graph::MergedGraph;

    fn graph(roots: &[ProjectRoot]) -> MergedGraph {
        MergedGraph::new(vec![crate::walk::build_project_graph(roots, RepoId(1))])
    }

    /// Two Maven modules sharing an artifactId: the smallest path wins,
    /// deterministically, whatever order the anchors were emitted in.
    #[test]
    fn ambiguous_label_resolves_to_the_smallest_path() {
        let label = || Some("billing".to_string());
        let m = graph(&[
            ProjectRoot::new("svc/zeta".into(), "maven", "pom.xml", label()),
            ProjectRoot::new("svc/alpha".into(), "maven", "pom.xml", label()),
        ]);
        assert_eq!(resolve_scope(&m, "billing"), "svc/alpha");
        // Both paths still resolve to themselves.
        assert_eq!(resolve_scope(&m, "svc/zeta"), "svc/zeta");
        assert_eq!(resolve_scope(&m, "svc/alpha"), "svc/alpha");
    }

    /// A PROJECT whose ORIGIN will not parse is skipped, not fatal, and the
    /// root's `""` walker path surfaces as `.`.
    #[test]
    fn unparseable_origin_is_skipped_and_root_is_dot() {
        let mut m = graph(&[
            ProjectRoot::new(String::new(), "npm", "package.json", Some("mono".into())),
            ProjectRoot::new("apps/web".into(), "npm", "package.json", Some("web".into())),
        ]);
        let broken = m.graphs[0]
            .nodes
            .iter_mut()
            .find(|n| {
                matches!(&n.cells[0].payload, CellPayload::Json(s) if s.contains("apps/web"))
            })
            .expect("apps/web anchor");
        broken.cells[0].payload = CellPayload::Json("{not json".into());
        let roots = project_roots(&m);
        assert_eq!(roots.len(), 1, "the broken anchor is skipped");
        assert_eq!((roots[0].path.as_str(), roots[0].label.as_str()), (".", "mono"));
        assert_eq!(roots[0].qname, "project:.");
        assert_eq!(resolve_scope(&m, "mono"), ".");
        assert_eq!(resolve_scope(&m, "web"), "web", "a skipped label passes through");
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
                cells: Vec::new(),
            });
        }
        (merged, ids)
    }

    fn side_of(merged: &MergedGraph, id: NodeId) -> MessageContractSide {
        let nodes = super::collect_queue_nodes(merged);
        contract_side(&super::Locator::new(merged), id.0, &nodes[&id.0])
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

#[cfg(test)]
mod role_live_tests {
    use super::{entrypoint_reachable, is_entrypoint};
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
    use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
    use repo_graph_graph::{MergedGraph, RepoGraph, SymbolTable};

    /// `app::Page` -INJECTS-> `app::Api`, both CLASS; `Page` carries a ROLE
    /// cell with `roles` when given. Returns the graph and `(page, api)`.
    fn page_injects_api(roles: Option<&str>) -> (MergedGraph, NodeId, NodeId) {
        let repo = RepoId::from_canonical("test://role-live");
        let id = |q: &str| NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, q);
        let (page, api) = (id("app::Page"), id("app::Api"));
        let mut nav = CodeNav::default();
        nav.record(page, "Page", "app::Page", node_kind::CLASS, None);
        nav.record(api, "Api", "app::Api", node_kind::CLASS, None);
        let cells = roles
            .map(|r| Cell {
                kind: cell_type::ROLE,
                payload: CellPayload::Json(format!(r#"{{"roles":[{r}]}}"#)),
            })
            .into_iter()
            .collect();
        let node = |nid, cells| Node { id: nid, repo, confidence: Confidence::Strong, cells };
        let g = RepoGraph {
            repo,
            nodes: vec![node(page, cells), node(api, vec![])],
            edges: vec![Edge {
                from: page,
                to: api,
                category: edge_category::INJECTS,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            }],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        (MergedGraph::new(vec![g]), page, api)
    }

    /// LB.3b: a CLASS folded from an `@Component` seeds liveness through its
    /// ROLE cell; without the cell, or with a non-entry role, it does not.
    #[test]
    fn component_role_is_an_entrypoint() {
        let (m, page, api) = page_injects_api(Some(r#""COMPONENT""#));
        let live = entrypoint_reachable(&m);
        assert!(live.contains(&page), "the COMPONENT-role class is an entry");
        assert!(live.contains(&api), "what it injects is live");

        let (m, page, api) = page_injects_api(None);
        let live = entrypoint_reachable(&m);
        assert!(!live.contains(&page) && !live.contains(&api), "no role, no entry");

        let (m, _, api) = page_injects_api(Some(r#""SERVICE""#));
        assert!(!entrypoint_reachable(&m).contains(&api), "SERVICE is not an entry role");

        // The kind / name arms are unchanged and need no roles.
        let class = Some(node_kind::CLASS);
        assert!(is_entrypoint(class, "Page", &[node_kind::COMPONENT]));
        assert!(!is_entrypoint(class, "Page", &[node_kind::SERVICE, node_kind::HOOK]));
        assert!(is_entrypoint(Some(node_kind::COMPONENT), "Card", &[]));
        assert!(is_entrypoint(Some(node_kind::FUNCTION), "main", &[]));
        assert!(!is_entrypoint(class, "Page", &[]));
    }
}

#[cfg(test)]
mod live_tests {
    //! A7.8 — liveness past the entry set: a live METHOD makes its owning
    //! type live, a live interface makes its implementers live, and neither
    //! step turns into "everything is live".

    use super::{entrypoint_reachable, is_entrypoint, live_walk};
    use repo_graph_code_domain::{
        CallQualifier, CodeNav, FileParse, GRAPH_TYPE, UnresolvedRef, edge_category, node_kind,
    };
    use repo_graph_core::{Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId};
    use repo_graph_graph::roles::roles_in;
    use repo_graph_graph::{MergedGraph, RepoGraph, SymbolTable, build_typescript};

    fn repo() -> RepoId {
        RepoId::from_canonical("test://live")
    }

    /// Hand-built graph: nodes recorded with kind, qname and parent; the simple
    /// name is the last `::` segment of the qname.
    #[derive(Default)]
    struct G {
        nodes: Vec<Node>,
        edges: Vec<Edge>,
        nav: CodeNav,
    }

    impl G {
        fn node(&mut self, kind: NodeKindId, qname: &str, parent: Option<NodeId>) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            self.nav.record(id, name, qname, kind, parent);
            self.nodes.push(Node {
                id,
                repo: repo(),
                confidence: Confidence::Strong,
                cells: vec![],
            });
            id
        }

        fn edge(&mut self, from: NodeId, to: NodeId, category: EdgeCategoryId) {
            self.edges.push(Edge {
                from,
                to,
                category,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
        }

        fn merged(self) -> MergedGraph {
            MergedGraph::new(vec![RepoGraph {
                repo: repo(),
                nodes: self.nodes,
                edges: self.edges,
                nav: self.nav,
                symbols: SymbolTable::default(),
                unresolved_calls: vec![],
                unresolved_refs: vec![],
                properties: Default::default(),
            }])
        }
    }

    /// LB.3 folds the `@Component` overlay into its CLASS; the surviving CLASS
    /// is an entrypoint through its ROLE cell and what it injects is live.
    /// Built through `build_typescript`, so the fold itself runs.
    #[test]
    fn merged_component_seeds_its_injects() {
        let id = |k, q: &str| NodeId::from_parts(GRAPH_TYPE, repo(), k, q);
        let node = |nid| Node {
            id: nid,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![],
        };
        let parse = |nodes, edges, refs, nav| FileParse {
            nodes,
            edges,
            imports: vec![],
            calls: vec![],
            refs,
            nav,
            properties: Default::default(),
        };
        // m.ts: `@Injectable() class UserService {}` (no entry role).
        let (m, svc_class, svc_overlay) = (
            id(node_kind::MODULE, "m"),
            id(node_kind::CLASS, "m::UserService"),
            id(node_kind::SERVICE, "m::UserService"),
        );
        let mut nav = CodeNav::default();
        nav.record(m, "m", "m", node_kind::MODULE, None);
        nav.record(
            svc_class,
            "UserService",
            "m::UserService",
            node_kind::CLASS,
            Some(m),
        );
        nav.record(
            svc_overlay,
            "UserService",
            "m::UserService",
            node_kind::SERVICE,
            Some(m),
        );
        let service = parse(
            vec![node(m), node(svc_class), node(svc_overlay)],
            vec![Edge {
                from: m,
                to: svc_class,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            }],
            vec![],
            nav,
        );
        // n.ts: `@Component() class UsersComponent { constructor(s: UserService) }`.
        let (n, comp_class, comp_overlay) = (
            id(node_kind::MODULE, "n"),
            id(node_kind::CLASS, "n::UsersComponent"),
            id(node_kind::COMPONENT, "n::UsersComponent"),
        );
        let mut nav = CodeNav::default();
        nav.record(n, "n", "n", node_kind::MODULE, None);
        nav.record(
            comp_class,
            "UsersComponent",
            "n::UsersComponent",
            node_kind::CLASS,
            Some(n),
        );
        nav.record(
            comp_overlay,
            "UsersComponent",
            "n::UsersComponent",
            node_kind::COMPONENT,
            Some(n),
        );
        let component = parse(
            vec![node(n), node(comp_class), node(comp_overlay)],
            vec![Edge {
                from: n,
                to: comp_class,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            }],
            vec![UnresolvedRef {
                from: comp_class,
                from_module: n,
                qualifier: CallQualifier::Bare("UserService".into()),
                category: edge_category::INJECTS,
                line: 0,
            }],
            nav,
        );
        let g = build_typescript(repo(), vec![service, component], |_, _| None).expect("build");

        // The fold left one node per declaration, the CLASS, with the role.
        assert!(
            g.nodes
                .iter()
                .all(|x| x.id != comp_overlay && x.id != svc_overlay)
        );
        let comp = g
            .nodes
            .iter()
            .find(|x| x.id == comp_class)
            .expect("the CLASS survives");
        let roles = roles_in(Some(node_kind::CLASS), &comp.cells);
        assert!(is_entrypoint(
            Some(node_kind::CLASS),
            "UsersComponent",
            &roles
        ));
        assert!(g.edges.iter().any(|e| e.from == comp_class
            && e.to == svc_class
            && e.category == edge_category::INJECTS));

        let w = live_walk(&MergedGraph::new(vec![g]));
        assert!(
            w.live.contains(&comp_class),
            "the folded component is an entry"
        );
        assert!(w.live.contains(&svc_class), "what it injects is live");
        assert_eq!(
            (w.by_kind, w.by_role, w.owners, w.implementers),
            (0, 1, 0, 0)
        );
    }

    /// ROUTE -HANDLED_BY-> METHOD, whose CLASS -INJECTS-> a service: the
    /// method makes the class live, and the class carries the injection.
    #[test]
    fn method_liveness_propagates_to_owning_class() {
        let mut g = G::default();
        let module = g.node(node_kind::MODULE, "app", None);
        let ctrl = g.node(node_kind::CLASS, "app::ReportsController", Some(module));
        let get = g.node(node_kind::METHOD, "app::ReportsController::Get", Some(ctrl));
        let route = g.node(node_kind::ROUTE, "GET /reports", None);
        let svc = g.node(node_kind::CLASS, "app::ReportService", Some(module));
        let point = g.node(node_kind::STRUCT, "app::Point", Some(module));
        let norm = g.node(node_kind::METHOD, "app::Point::norm", Some(point));
        g.edge(module, ctrl, edge_category::DEFINES);
        g.edge(ctrl, get, edge_category::DEFINES);
        g.edge(route, get, edge_category::HANDLED_BY);
        g.edge(ctrl, svc, edge_category::INJECTS);
        g.edge(get, norm, edge_category::CALLS);

        let w = live_walk(&g.merged());
        for (id, what) in [
            (ctrl, "the controller"),
            (svc, "its injected service"),
            (point, "a STRUCT"),
        ] {
            assert!(w.live.contains(&id), "{what} is live");
        }
        assert!(!w.live.contains(&module), "no climb to MODULE");
        assert_eq!((w.by_kind, w.owners, w.implementers), (1, 2, 0));
        assert_eq!(w.total, 7);
    }

    /// The guard against "everything is live": a CLASS with no live method and
    /// no entry role stays dead, and a live method's class does not make its
    /// sibling methods live (upward only).
    #[test]
    fn unrelated_class_stays_dead() {
        let mut g = G::default();
        let ctrl = g.node(node_kind::CLASS, "app::Ctrl", None);
        let get = g.node(node_kind::METHOD, "app::Ctrl::get", Some(ctrl));
        let helper = g.node(node_kind::METHOD, "app::Ctrl::helper", Some(ctrl));
        let route = g.node(node_kind::ROUTE, "GET /x", None);
        let other = g.node(node_kind::CLASS, "app::Unused", None);
        let run = g.node(node_kind::METHOD, "app::Unused::run", Some(other));
        let dep = g.node(node_kind::CLASS, "app::Dep", None);
        g.edge(ctrl, get, edge_category::DEFINES);
        g.edge(ctrl, helper, edge_category::DEFINES);
        g.edge(route, get, edge_category::HANDLED_BY);
        g.edge(other, run, edge_category::DEFINES);
        g.edge(other, dep, edge_category::INJECTS);

        let live = entrypoint_reachable(&g.merged());
        assert!(live.contains(&ctrl));
        for (id, what) in [
            (helper, "a sibling of the live method"),
            (other, "an unrelated class"),
            (run, "its method"),
            (dep, "what it injects"),
        ] {
            assert!(!live.contains(&id), "{what} stays dead");
        }
    }

    /// A class that is live as a route handler DEFINES a method nothing calls:
    /// DEFINES still does not carry, so the method stays dead.
    #[test]
    fn defines_still_does_not_carry() {
        let mut g = G::default();
        let ctrl = g.node(node_kind::CLASS, "app::Ctrl", None);
        let unused = g.node(node_kind::METHOD, "app::Ctrl::unused", Some(ctrl));
        let route = g.node(node_kind::ROUTE, "ANY /api", None);
        g.edge(route, ctrl, edge_category::HANDLED_BY);
        g.edge(ctrl, unused, edge_category::DEFINES);

        let w = live_walk(&g.merged());
        assert!(w.live.contains(&ctrl));
        assert!(!w.live.contains(&unused), "DEFINES is not a carry edge");
        assert_eq!((w.owners, w.implementers), (0, 0));
    }

    /// Interface-typed DI: the controller injects `IRepo` and calls
    /// `IRepo::find`; the implementing class and its `find` are live, and the
    /// implementation's own callees with them. INHERITS_FROM stays forward-only
    /// and an implementer of a dead interface stays dead.
    #[test]
    fn implementer_of_live_interface_is_live() {
        let mut g = G::default();
        let route = g.node(node_kind::ROUTE, "GET /items", None);
        let ctrl = g.node(node_kind::CLASS, "app::Ctrl", None);
        let get = g.node(node_kind::METHOD, "app::Ctrl::get", Some(ctrl));
        let irepo = g.node(node_kind::INTERFACE, "app::IRepo", None);
        let ifind = g.node(node_kind::METHOD, "app::IRepo::find", Some(irepo));
        let repo_c = g.node(node_kind::CLASS, "app::Repo", None);
        let find = g.node(node_kind::METHOD, "app::Repo::find", Some(repo_c));
        let load = g.node(node_kind::METHOD, "app::Repo::load", Some(repo_c));
        let cached = g.node(node_kind::CLASS, "app::CachedRepo", None);
        let idead = g.node(node_kind::INTERFACE, "app::IDead", None);
        let dead_impl = g.node(node_kind::CLASS, "app::DeadImpl", None);
        g.edge(route, get, edge_category::HANDLED_BY);
        g.edge(ctrl, irepo, edge_category::INJECTS);
        g.edge(get, ifind, edge_category::CALLS);
        g.edge(repo_c, irepo, edge_category::IMPLEMENTS);
        g.edge(find, ifind, edge_category::IMPLEMENTS);
        g.edge(find, load, edge_category::CALLS);
        g.edge(cached, repo_c, edge_category::INHERITS_FROM);
        g.edge(dead_impl, idead, edge_category::IMPLEMENTS);

        let w = live_walk(&g.merged());
        for (id, what) in [
            (repo_c, "the implementing class"),
            (find, "the implementing method"),
            (load, "the implementation's callee"),
        ] {
            assert!(w.live.contains(&id), "{what} is live");
        }
        assert!(!w.live.contains(&cached), "INHERITS_FROM is forward-only");
        assert!(!w.live.contains(&idead) && !w.live.contains(&dead_impl));
        // Implementers: Repo (via IRepo) and Repo::find (via IRepo::find).
        // Owners: Ctrl (via get); IRepo and Repo were already live.
        assert_eq!((w.owners, w.implementers), (1, 2));
    }
}
