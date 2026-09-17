//! Post-passes over the merged graph: the doc linker, synthetic-node
//! provenance tagging, TESTS edges, and the confidence demotions — plus the
//! deterministic cross-edge sort that locks the written bytes.

use std::collections::HashMap;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, NodeId};
use repo_graph_graph::MergedGraph;

pub(crate) fn post_passes(merged: &mut MergedGraph) {
    downgrade_test_paths(merged);
    demote_unmatched_http_nodes(merged);
    emit_tests_edges(merged);
    link_doc_sections(merged);
    link_contract_routes(merged);
    tag_synthetic_provenance(merged);
    // Deterministic cross-edge order: several resolvers emit pairs by
    // iterating HashMap indexes (per-process seed), so the edge SET was stable
    // but its Vec order — and therefore cross_stack.gmap's bytes — flapped
    // across processes and even clean-vs-incremental in one process (audit
    // 2026-06-10 #6). One sort here covers all resolvers and post-passes.
    merged
        .cross_edges
        .sort_unstable_by_key(|e| (e.from.0, e.to.0, e.category.0, e.confidence as u8));
}

/// WP-H / #7: link `.md` DOC_SECTION nodes to the code symbols they document so
/// doc nodes aren't islands. High-precision signal — backtick-quoted
/// identifiers in the markdown (`` `MyClass` ``, `` `parse_file()` ``) — matched
/// against code-symbol names. Emits DOCUMENTS cross-edges (doc → symbol), capped
/// per doc node to bound noise.
///
/// A16.3: the mention is resolved against a two-tier index rather than a bare
/// name→id map, so a qualified mention (`` `PaymentGateway.charge` ``) binds to
/// that member instead of whichever same-named symbol happened to hold the
/// lowest NodeId, and an ambiguous bare name is stamped `Weak` instead of
/// claiming `Medium` for a coin flip.
fn link_doc_sections(merged: &mut MergedGraph) {
    use repo_graph_code_domain::cell_type;
    use repo_graph_core::CellPayload;

    let mut idx = DocSymbolIndex::default();
    for g in &merged.graphs {
        for n in &g.nodes {
            let Some(kind) = g.nav.kind_by_id.get(&n.id).copied() else {
                continue;
            };
            if !is_doc_linkable_symbol(kind) {
                continue;
            }
            let Some(name) = g.nav.name_by_id.get(&n.id) else {
                continue;
            };
            idx.add(name, g.nav.qname_by_id.get(&n.id).map(String::as_str), n.id);
        }
    }
    if idx.by_name.is_empty() {
        return;
    }

    const MAX_LINKS_PER_DOC: usize = 25;
    let mut new_edges: Vec<Edge> = Vec::new();
    let (mut strong, mut medium, mut weak, mut doc_sections) = (0usize, 0usize, 0usize, 0usize);
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id).copied() != Some(node_kind::DOC_SECTION) {
                continue;
            }
            doc_sections += 1;
            let Some(text) = n.cells.iter().find_map(|c| match &c.payload {
                CellPayload::Text(s) if c.kind == cell_type::CODE => Some(s.as_str()),
                _ => None,
            }) else {
                continue;
            };
            let mut seen: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
            // Iterate RAW spans: `identifier_from_span` would throw away the
            // qualifier the doc author supplied, which is the whole signal.
            for span in backtick_spans(text) {
                let Some((sym, confidence)) = resolve_doc_mention(span, &idx) else {
                    continue;
                };
                if sym == n.id || !seen.insert(sym) {
                    continue;
                }
                match confidence {
                    Confidence::Strong => strong += 1,
                    Confidence::Medium => medium += 1,
                    Confidence::Weak => weak += 1,
                }
                new_edges.push(Edge {
                    from: n.id,
                    to: sym,
                    category: edge_category::DOCUMENTS,
                    confidence,
                });
                if seen.len() >= MAX_LINKS_PER_DOC {
                    break;
                }
            }
        }
    }
    if doc_sections > 0 {
        eprintln!(
            "[doclink] {} DOCUMENTS edges (strong={strong} qualified, medium={medium} unique, \
             weak={weak} ambiguous) over {doc_sections} doc sections",
            new_edges.len()
        );
    }
    merged.cross_edges.extend(new_edges);
}

/// A10.2: pair each contract operation — a DOC_SECTION whose ORIGIN cell says
/// `provenance: contract` (A10.1's OpenAPI ops; A10.8's Pact interactions
/// carry the same shape) — with the ROUTE nodes that implement it, as
/// DOCUMENTS cross-edges **contract → ROUTE**. That direction is what
/// `governing_docs(route)` / `glia docs-for <route>` already collects, so those
/// answer with the spec'd operation with no change to the primitive.
///
/// Matching is the HTTP resolver's own route index and tiers 1-4
/// (`HttpRouteMatcher`), so a contract pairs with exactly the routes a client
/// calling the same `METHOD path` would. Confidence:
/// - `Strong` — the declared `(method, path)` hit a route outright;
/// - `Medium` — it needed an API-prefix strip (an OpenAPI `servers: /api/v1`
///   base), the route's `ANY` method, or the un-prefixed `raw_path` retry (a
///   server base that is not API-shaped, e.g. `/billing-svc`).
///
/// Neither is ever above the ROUTE node's own confidence.
///
/// Runs after the resolvers and before the cross-edge sort in `post_passes`, so
/// the new edges are covered by that sort and the written bytes stay stable.
fn link_contract_routes(merged: &mut MergedGraph) {
    let (edges, stats) = contract_route_edges(merged);
    if stats.ops > 0 {
        eprintln!(
            "[contract-link] ops={} exact={} prefix={} unmatched={} edges={}",
            stats.ops,
            stats.exact,
            stats.prefix,
            stats.unmatched,
            edges.len()
        );
    }
    merged.cross_edges.extend(edges);
}

/// Counters behind the `[contract-link]` marker. `exact` + `prefix` +
/// `unmatched` == `ops`: each op is counted once, by the tier that paired it
/// (`prefix` = every non-exact pairing, see `link_contract_routes`).
#[derive(Debug, Default, PartialEq, Eq)]
struct ContractLinkStats {
    ops: usize,
    exact: usize,
    prefix: usize,
    unmatched: usize,
}

/// The HTTP half of a contract operation, read off its ORIGIN cell.
#[derive(Debug, PartialEq, Eq)]
struct ContractOp {
    method: String,
    path: String,
    raw_path: Option<String>,
}

/// The edges `link_contract_routes` adds, without touching the graph. Split out
/// so the unit tests can assert edges and counters on a hand-built merge.
fn contract_route_edges(merged: &MergedGraph) -> (Vec<Edge>, ContractLinkStats) {
    use repo_graph_graph::HttpRouteMatcher;

    let mut stats = ContractLinkStats::default();
    // Collect the ops first: almost no build has a contract file, and those
    // builds must not pay for a second ROUTE index.
    let mut ops: Vec<(NodeId, ContractOp)> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id).copied() != Some(node_kind::DOC_SECTION) {
                continue;
            }
            if let Some(op) = contract_op(&n.cells) {
                ops.push((n.id, op));
            }
        }
    }
    if ops.is_empty() {
        return (Vec::new(), stats);
    }
    stats.ops = ops.len();

    let matcher = HttpRouteMatcher::new(&merged.graphs);
    if matcher.is_empty() {
        stats.unmatched = ops.len();
        return (Vec::new(), stats);
    }
    let mut edges: Vec<Edge> = Vec::new();
    for (op_id, op) in &ops {
        let mut hits = matcher.lookup(&op.method, &op.path);
        // Every hit comes from one tier, so the first speaks for all of them.
        let mut exact = hits.first().is_some_and(|h| h.exact);
        if hits.is_empty()
            && let Some(raw) = op.raw_path.as_deref()
            && raw != op.path
        {
            hits = matcher.lookup(&op.method, raw);
            // Dropping the server base is itself an inference.
            exact = false;
        }
        if hits.is_empty() {
            stats.unmatched += 1;
            continue;
        }
        if exact {
            stats.exact += 1;
        } else {
            stats.prefix += 1;
        }
        let mut seen: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        for h in hits {
            // One route node can carry the same method twice (stacked
            // ROUTE_METHOD cells); it is still one DOCUMENTS edge.
            if !seen.insert(h.route) {
                continue;
            }
            // Non-exact pairings are capped at Medium; a weaker route stays weaker.
            let confidence = match h.confidence {
                Confidence::Strong if !exact => Confidence::Medium,
                c => c,
            };
            edges.push(Edge {
                from: *op_id,
                to: h.route,
                category: edge_category::DOCUMENTS,
                confidence,
            });
        }
    }
    (edges, stats)
}

/// The contract operation a DOC_SECTION's ORIGIN cell declares, or `None` when
/// the node is not a contract op (markdown sections have no ORIGIN here) or the
/// op is not an HTTP one (an AsyncAPI channel op is A10.3's to pair).
fn contract_op(cells: &[repo_graph_core::Cell]) -> Option<ContractOp> {
    use repo_graph_code_domain::cell_type;
    use repo_graph_core::CellPayload;

    let json = cells.iter().find_map(|c| match &c.payload {
        CellPayload::Json(j) if c.kind == cell_type::ORIGIN => Some(j.as_str()),
        _ => None,
    })?;
    // Cheap reject before parsing: every other ORIGIN payload (nav_route,
    // region anchors, ...) takes this path.
    if !json.contains("\"contract\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v.get("provenance")?.as_str()? != "contract" {
        return None;
    }
    let method = v.get("method")?.as_str()?.to_ascii_uppercase();
    if !is_http_verb(&method) {
        return None;
    }
    let path = v.get("path")?.as_str()?.to_string();
    let raw_path = v.get("raw_path").and_then(|p| p.as_str()).map(str::to_string);
    Some(ContractOp { method, path, raw_path })
}

/// One of the verbs an OpenAPI path item may declare — literally the allow-list
/// the contract extractor gates emission on.
fn is_http_verb(method: &str) -> bool {
    repo_graph_code_extractors::contracts::METHODS
        .iter()
        .any(|m| m.eq_ignore_ascii_case(method))
}

/// Doc→code mention index. `by_tail2` keys the last two qname segments so a
/// qualified mention (`Class.method`) binds to the right member; `by_name` keeps
/// today's bare-name lookup but carries the collision count so an ambiguous hit
/// can be graded instead of silently picking the lowest NodeId. Both maps are
/// only ever LOOKED UP, never iterated (CODE_RULES §3).
#[derive(Default)]
struct DocSymbolIndex {
    /// `name`            → (lowest id, how many symbols share it)
    by_name: HashMap<String, (NodeId, usize)>,
    /// `"Class::method"` → (lowest id, how many symbols share it)
    by_tail2: HashMap<String, (NodeId, usize)>,
}

impl DocSymbolIndex {
    fn record(map: &mut HashMap<String, (NodeId, usize)>, key: String, id: NodeId) {
        map.entry(key)
            .and_modify(|(cur, n)| {
                if id.0 < cur.0 {
                    *cur = id;
                }
                *n += 1;
            })
            .or_insert((id, 1));
    }

    fn add(&mut self, name: &str, qname: Option<&str>, id: NodeId) {
        Self::record(&mut self.by_name, name.to_string(), id);
        // Same identifier rules as the lookup side, so a key is never stored
        // that `resolve_doc_mention` could not ask for.
        if let Some(q) = qname {
            let mut segs = q.rsplit("::");
            if let (Some(last), Some(prev)) = (segs.next(), segs.next())
                && is_identifier(prev)
                && is_identifier(last)
            {
                Self::record(&mut self.by_tail2, format!("{prev}::{last}"), id);
            }
        }
    }
}

/// (target, confidence) for one inline-code span, or `None` when it names
/// nothing in the graph.
fn resolve_doc_mention(span: &str, idx: &DocSymbolIndex) -> Option<(NodeId, Confidence)> {
    let norm = span.trim().trim_end_matches("()").replace('.', "::");
    // Tier 1 — qualified mention, e.g. `PaymentGateway.charge` / `mod::Thing`.
    if norm.contains("::") {
        let mut segs = norm.rsplit("::");
        if let (Some(last), Some(prev)) = (segs.next(), segs.next())
            && is_identifier(prev)
            && is_identifier(last)
            && let Some(&(id, n)) = idx.by_tail2.get(&format!("{prev}::{last}"))
        {
            return Some((id, if n == 1 { Confidence::Strong } else { Confidence::Medium }));
        }
    }
    // Tiers 2/3 — bare tail name (the pre-A16.3 behaviour), graded by ambiguity.
    let ident = identifier_from_span(span)?;
    let &(id, n) = idx.by_name.get(&ident)?;
    Some((id, if n == 1 { Confidence::Medium } else { Confidence::Weak }))
}

/// Node kinds a doc section can meaningfully document.
fn is_doc_linkable_symbol(kind: repo_graph_core::NodeKindId) -> bool {
    use repo_graph_code_domain::node_kind as nk;
    kind == nk::FUNCTION
        || kind == nk::METHOD
        || kind == nk::CLASS
        || kind == nk::STRUCT
        || kind == nk::INTERFACE
        || kind == nk::ENUM
        || kind == nk::COMPONENT
        || kind == nk::SERVICE
        || kind == nk::STATE_VAR
        || kind == nk::DATA_ENTITY
}

/// Contents of single-backtick inline-code spans in markdown, unreduced. Triple-
/// backtick fenced blocks fall on even split segments and are skipped.
fn backtick_spans(text: &str) -> Vec<&str> {
    text.split('`')
        .enumerate()
        .filter_map(|(i, seg)| (i % 2 == 1).then_some(seg))
        .collect()
}

/// Bare identifiers inside inline-code spans — the pre-A16.3 reduction, kept as
/// the reference behaviour the tier-2/3 fallback must stay identical to.
#[cfg(test)]
fn backtick_identifiers(text: &str) -> Vec<String> {
    backtick_spans(text)
        .into_iter()
        .filter_map(identifier_from_span)
        .collect()
}

/// Reduce an inline-code span to a bare identifier: drop trailing `()`, take the
/// last `.`/`::` segment, require an identifier ≥3 chars. `None` if not one.
fn identifier_from_span(span: &str) -> Option<String> {
    let s = span.trim().trim_end_matches("()");
    let s = s.rsplit(|c| c == '.' || c == ':').next().unwrap_or(s);
    is_identifier(s).then(|| s.to_string())
}

/// ≥3 chars, first char ascii-alpha or `_`, rest ascii-alphanumeric or `_`.
/// Extracted verbatim from `identifier_from_span` so tier 1 can never match
/// something the pre-A16.3 path would have rejected.
fn is_identifier(s: &str) -> bool {
    s.len() >= 3
        && s.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Tag the substrate-only synthetic node kinds with an `ORIGIN` cell so
/// consumers (the engram exporter, neuropil) can filter them by coordinate
/// rather than string-matching keys. These nodes are real and load-bearing for
/// the cross-repo resolvers (`PackageResolver` pairs `package:npm:*`,
/// `EventBusResolver` pairs `event_*`), so they are NOT dropped here — only
/// categorised. (glia-v2 G6/G9/G11)
fn tag_synthetic_provenance(merged: &mut MergedGraph) {
    use repo_graph_code_domain::{cell_type, node_kind};
    use repo_graph_core::{Cell, CellPayload};

    for g in &mut merged.graphs {
        let nodes = &mut g.nodes;
        let nav = &g.nav;
        for n in nodes.iter_mut() {
            // Don't double-tag (region anchors are tagged at creation).
            if n.cells.iter().any(|c| c.kind == cell_type::ORIGIN) {
                continue;
            }
            let kind = nav.kind_by_id.get(&n.id).copied();
            let qname = nav.qname_by_id.get(&n.id).map(String::as_str).unwrap_or("");
            let file = position_file(&n.cells).unwrap_or_default();
            let provenance = if matches!(kind, Some(node_kind::PACKAGE_DEP)) {
                // npm/cargo/etc. dependency pseudo-nodes (G9 hub).
                "dependency"
            } else if matches!(
                kind,
                Some(node_kind::EVENT_EMITTER) | Some(node_kind::EVENT_HANDLER)
            ) {
                // event emitter/handler name pseudo-nodes (G6).
                "synthetic"
            } else if qname.contains("ExampleInstrumentedTest")
                || qname.starts_with("androidTest::")
            {
                // Framework-generated test stubs — Capacitor's
                // ExampleInstrumentedTest, anything under androidTest (G11).
                "generated"
            } else if is_generated_proto(&file) {
                // protobuf-generated reflection code — `chatpb::*::Reset` etc.
                // swamp recall on matching trigrams. Distinct from `generated`
                // so engram can opt it back in independently. (glia-v3 #5)
                "generated_proto"
            } else if is_test_fixture(&file, qname) {
                // test files + seeders/load-testers — droppable by default at
                // recall, opt-in via engram's --include-tests. (glia-v3 #6)
                "test_fixture"
            } else {
                continue;
            };
            n.cells.push(Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(format!(r#"{{"provenance":"{provenance}"}}"#)),
            });
        }
    }
}

/// Pull the `file` path out of a node's POSITION cell. Lightweight string
/// scan of the `{"file":"...","start_line":..}` payload — avoids a serde_json
/// dependency in the engine crate.
fn position_file(cells: &[repo_graph_core::Cell]) -> Option<String> {
    use repo_graph_code_domain::cell_type;
    use repo_graph_core::CellPayload;
    for c in cells {
        if c.kind != cell_type::POSITION {
            continue;
        }
        if let CellPayload::Json(j) = &c.payload
            && let Some(rest) = j.split("\"file\":\"").nth(1)
            && let Some(end) = rest.find('"')
        {
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// Generated-protobuf source: codegen file extensions across the languages
/// quokka-stack mixes (Go / Dart / TS / Python). The reflection-method noise
/// (`Reset`/`String`/`ProtoReflect`/`Marshal`…) lives in these files. (glia-v3 #5)
fn is_generated_proto(file: &str) -> bool {
    file.ends_with(".pb.go")
        || file.ends_with(".pb-grpc.go")
        || file.ends_with(".pb.dart")
        || file.ends_with(".pbjson.dart")
        || file.ends_with(".pbenum.dart")
        || file.ends_with(".pbserver.dart")
        || file.ends_with(".pb.ts")
        || file.ends_with("_pb2.py")
        || file.ends_with("_pb2_grpc.py")
        || file.contains(".pb.")
}

/// Test / fixture / seeder code, by file path or qname shape. (glia-v3 #6)
fn is_test_fixture(file: &str, qname: &str) -> bool {
    file.ends_with("_test.go")
        || file.ends_with("_test.dart")
        || file.ends_with(".spec.ts")
        || file.ends_with(".test.ts")
        || file.ends_with(".spec.js")
        || file.ends_with(".test.js")
        || file.ends_with("_test.py")
        || file.ends_with("_spec.rb")
        || file.contains("/tests/")
        || file.contains("/__tests__/")
        || file.contains("/test/")
        || is_test_qname(qname)
}

// ----------------------------------------------------------------------------
// Post passes
// ----------------------------------------------------------------------------

fn emit_tests_edges(merged: &mut MergedGraph) {
    let mut modules_by_tail: HashMap<String, Vec<(NodeId, String)>> = HashMap::new();
    let mut module_info: Vec<(NodeId, String)> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id).copied() != Some(node_kind::MODULE) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            module_info.push((n.id, qname.clone()));
            if let Some(tail) = qname.rsplit("::").next() {
                modules_by_tail
                    .entry(tail.to_string())
                    .or_default()
                    .push((n.id, qname.clone()));
            }
        }
    }
    for (from_id, qname) in &module_info {
        if !is_test_qname(qname) {
            continue;
        }
        let Some(tail) = qname.rsplit("::").next() else { continue };
        let stripped = strip_test_affixes(tail);
        if stripped.is_empty() || stripped == tail {
            continue;
        }
        let Some(candidates) = modules_by_tail.get(stripped) else { continue };
        for to_id in select_test_targets(*from_id, qname, candidates) {
            merged.cross_edges.push(Edge {
                from: *from_id,
                to: to_id,
                category: edge_category::TESTS,
                confidence: Confidence::Strong,
            });
        }
    }
}

fn select_test_targets(
    from_id: NodeId,
    test_qname: &str,
    candidates: &[(NodeId, String)],
) -> Vec<NodeId> {
    const MAX_TEST_TARGETS: usize = 3;
    let test_parent: Vec<&str> = qname_parent_segments(test_qname);
    let mut scored: Vec<(usize, NodeId)> = candidates
        .iter()
        .filter(|(id, _)| *id != from_id)
        .map(|(id, qn)| {
            let cand_parent = qname_parent_segments(qn);
            (common_prefix_len(&test_parent, &cand_parent), *id)
        })
        .collect();
    if scored.is_empty() {
        return Vec::new();
    }
    let max_score = scored.iter().map(|(s, _)| *s).max().unwrap_or(0);
    scored.retain(|(s, _)| *s == max_score);
    scored.truncate(MAX_TEST_TARGETS);
    scored.into_iter().map(|(_, id)| id).collect()
}

fn qname_parent_segments(qname: &str) -> Vec<&str> {
    let mut segs: Vec<&str> = qname.split("::").collect();
    segs.pop();
    segs
}

fn common_prefix_len(a: &[&str], b: &[&str]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

fn downgrade_test_paths(merged: &mut MergedGraph) {
    for g in &mut merged.graphs {
        for n in &mut g.nodes {
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            if qname.starts_with("route:") {
                continue;
            }
            if qname_is_noncritical_path(qname) {
                n.confidence = Confidence::Weak;
            }
        }
    }
}

fn demote_unmatched_http_nodes(merged: &mut MergedGraph) {
    use std::collections::HashSet;
    let mut matched: HashSet<NodeId> = HashSet::new();
    for e in &merged.cross_edges {
        if e.category == edge_category::HTTP_CALLS {
            matched.insert(e.from);
            matched.insert(e.to);
        }
    }
    for g in &mut merged.graphs {
        for n in &mut g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            let is_http_node = matches!(kind, Some(k) if k == node_kind::ROUTE || k == node_kind::ENDPOINT);
            if !is_http_node {
                continue;
            }
            if matches!(n.confidence, Confidence::Weak) {
                continue;
            }
            if !matched.contains(&n.id) {
                n.confidence = Confidence::Medium;
            }
        }
    }
}

fn qname_is_noncritical_path(qname: &str) -> bool {
    const NONCRITICAL: &[&str] = &[
        "tests", "test", "__tests__", "spec", "specs",
        "fixtures", "fixture", "examples", "example",
        "e2e", "__mocks__", "mocks", "testdata",
    ];
    qname.split("::").any(|seg| {
        let lowered = seg.to_ascii_lowercase();
        NONCRITICAL.contains(&lowered.as_str())
    })
}

fn is_test_qname(qname: &str) -> bool {
    let lowered = qname.to_ascii_lowercase();
    if lowered.contains("::tests::")
        || lowered.contains("::test::")
        || lowered.contains("::__tests__::")
        || lowered.contains("::spec::")
        || lowered.starts_with("tests::")
        || lowered.starts_with("test::")
        || lowered.starts_with("spec::")
    {
        return true;
    }
    let Some(tail) = qname.rsplit("::").next() else {
        return false;
    };
    let t = tail.to_ascii_lowercase();
    t.starts_with("test_")
        || t.ends_with("_test")
        || t.ends_with("_spec")
        || t.ends_with(".test")
        || t.ends_with(".spec")
}

fn strip_test_affixes(name: &str) -> &str {
    let lowered = name.to_ascii_lowercase();
    if let Some(rest) = lowered.strip_prefix("test_") {
        return &name[name.len() - rest.len()..];
    }
    for suffix in ["_test", "_spec", ".test", ".spec"] {
        if lowered.ends_with(suffix) {
            return &name[..name.len() - suffix.len()];
        }
    }
    name
}

#[cfg(test)]
mod passes_tests {
    use super::*;

    /// A16.3 — hand-built index, no MergedGraph needed. Mirrors the doc-link
    /// fixture: `PaymentGateway::charge` is a unique member, `charge` is a
    /// unique bare name, `save` is shared by two classes.
    fn fixture_index() -> DocSymbolIndex {
        let mut idx = DocSymbolIndex::default();
        idx.add("charge", Some("ordering::PaymentGateway::charge"), NodeId(7));
        idx.add("get_user", Some("users::get_user"), NodeId(3));
        idx.add("save", Some("ordering::OrderService::save"), NodeId(11));
        idx.add("save", Some("users::UserRepo::save"), NodeId(4));
        idx
    }

    #[test]
    fn doc_mention_qualified_is_strong() {
        let idx = fixture_index();
        assert_eq!(idx.by_tail2["PaymentGateway::charge"], (NodeId(7), 1));
        assert_eq!(
            resolve_doc_mention("PaymentGateway.charge", &idx),
            Some((NodeId(7), Confidence::Strong))
        );
        // The `::` spelling and a trailing `()` reach the same tier-1 answer.
        assert_eq!(
            resolve_doc_mention("PaymentGateway::charge()", &idx),
            Some((NodeId(7), Confidence::Strong))
        );
    }

    #[test]
    fn doc_mention_bare_unique_is_medium() {
        let idx = fixture_index();
        assert_eq!(
            resolve_doc_mention("get_user", &idx),
            Some((NodeId(3), Confidence::Medium))
        );
    }

    #[test]
    fn doc_mention_bare_ambiguous_is_weak() {
        let idx = fixture_index();
        // Two symbols named `save`: same lowest-id target as the pre-A16.3
        // path picked, but no longer claiming Medium for a coin flip.
        assert_eq!(idx.by_name["save"], (NodeId(4), 2));
        assert_eq!(
            resolve_doc_mention("save", &idx),
            Some((NodeId(4), Confidence::Weak))
        );
    }

    #[test]
    fn doc_mention_qualified_miss_falls_back_to_bare() {
        let idx = fixture_index();
        // No `Invoice::charge` member — tier 1 misses, tiers 2/3 answer with
        // the unique bare `charge`.
        assert!(!idx.by_tail2.contains_key("Invoice::charge"));
        assert_eq!(
            resolve_doc_mention("Invoice.charge", &idx),
            Some((NodeId(7), Confidence::Medium))
        );
        // Ambiguous bare fallback still degrades to Weak.
        assert_eq!(
            resolve_doc_mention("Whatever.save", &idx),
            Some((NodeId(4), Confidence::Weak))
        );
    }

    #[test]
    fn doc_mention_unknown_is_none() {
        let idx = fixture_index();
        assert_eq!(resolve_doc_mention("npm install", &idx), None);
        assert_eq!(resolve_doc_mention("--flag", &idx), None);
        assert_eq!(resolve_doc_mention("nothing_here", &idx), None);
    }

    // ------------------------------------------------------------------
    // A10.2 — link_contract_routes, on a hand-built merge
    // ------------------------------------------------------------------

    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type};
    use repo_graph_core::{Cell, CellPayload, Node, NodeKindId, RepoId};
    use repo_graph_graph::RepoGraph;

    /// A merge under construction: nodes + nav for one repo.
    struct Hand {
        repo: RepoId,
        nodes: Vec<Node>,
        nav: CodeNav,
    }

    impl Hand {
        fn new(canonical: &str) -> Self {
            Hand { repo: RepoId::from_canonical(canonical), nodes: vec![], nav: CodeNav::default() }
        }

        fn add(&mut self, kind: NodeKindId, name: &str, qname: &str, cells: Vec<Cell>) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, self.repo, kind, qname);
            self.nav.record(id, name, qname, kind, None);
            self.nodes.push(Node { id, repo: self.repo, confidence: Confidence::Strong, cells });
            id
        }

        /// A ROUTE in the legacy `<METHOD> <path>` shape (flask, spring, ...).
        fn route(&mut self, method: &str, path: &str) -> NodeId {
            let qname = format!("{method} {path}");
            let cell = Cell { kind: cell_type::ROUTE_METHOD, payload: CellPayload::Text(method.into()) };
            self.add(node_kind::ROUTE, &qname, &qname, vec![cell])
        }

        /// A DOC_SECTION carrying `origin` as its ORIGIN payload.
        fn doc(&mut self, qname: &str, origin: &str) -> NodeId {
            let cell = Cell { kind: cell_type::ORIGIN, payload: CellPayload::Json(origin.into()) };
            self.add(node_kind::DOC_SECTION, qname, qname, vec![cell])
        }

        fn graph(self) -> RepoGraph {
            RepoGraph {
                repo: self.repo,
                nodes: self.nodes,
                edges: vec![],
                nav: self.nav,
                symbols: Default::default(),
                unresolved_calls: vec![],
                unresolved_refs: vec![],
                properties: Default::default(),
            }
        }
    }

    /// The exact ORIGIN payload A10.1's extractor writes (see
    /// contracts.rs `server_prefix_is_joined_onto_the_path`).
    fn op_origin(method: &str, path: &str, raw_path: &str) -> String {
        format!(
            r#"{{"provenance":"contract","source":"openapi","method":"{method}","path":"{path}","raw_path":"{raw_path}","operation_id":"op"}}"#
        )
    }

    fn documents(edges: &[Edge], from: NodeId) -> Vec<(NodeId, Confidence)> {
        edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::DOCUMENTS)
            .map(|e| (e.to, e.confidence))
            .collect()
    }

    #[test]
    fn contract_link_strong_on_exact_medium_on_prefix_none_on_docs() {
        let mut h = Hand::new("test://contract-link/one");
        let get_users = h.route("GET", "/users");
        let exact = h.doc("contract::openapi::GET:/users", &op_origin("GET", "/users", "/users"));
        let prefixed = h.doc(
            "contract::openapi::GET:/api/v1/users",
            &op_origin("GET", "/api/v1/users", "/users"),
        );
        // A markdown section may carry the same fields; only a contract pairs.
        let prose = h.doc(
            "README::users",
            r#"{"provenance":"documentation","method":"GET","path":"/users"}"#,
        );
        let merged = MergedGraph::new(vec![h.graph()]);

        let (edges, stats) = contract_route_edges(&merged);
        assert_eq!(documents(&edges, exact), vec![(get_users, Confidence::Strong)]);
        assert_eq!(documents(&edges, prefixed), vec![(get_users, Confidence::Medium)]);
        assert!(documents(&edges, prose).is_empty());
        assert_eq!(edges.len(), 2);
        assert_eq!(stats, ContractLinkStats { ops: 2, exact: 1, prefix: 1, unmatched: 0 });
    }

    #[test]
    fn contract_link_retries_raw_path_for_a_non_api_server_base() {
        let mut h = Hand::new("test://contract-link/raw");
        let get_orders = h.route("GET", "/orders");
        // `billing-svc` is not an API prefix, so only the raw path pairs — and
        // having dropped the declared base, it is not Strong.
        let op = h.doc(
            "contract::billing::GET:/billing-svc/orders",
            &op_origin("GET", "/billing-svc/orders", "/orders"),
        );
        // The method is part of the key: POST pairs with nothing, and in
        // particular not with GET /orders.
        let miss = h.doc(
            "contract::billing::POST:/billing-svc/orders",
            &op_origin("POST", "/billing-svc/orders", "/orders"),
        );
        let merged = MergedGraph::new(vec![h.graph()]);

        let (edges, stats) = contract_route_edges(&merged);
        assert_eq!(documents(&edges, op), vec![(get_orders, Confidence::Medium)]);
        assert!(documents(&edges, miss).is_empty());
        assert_eq!(edges.len(), 1);
        assert_eq!(stats, ContractLinkStats { ops: 2, exact: 0, prefix: 1, unmatched: 1 });
    }

    #[test]
    fn contract_link_pairs_across_repos_and_lands_in_cross_edges() {
        let mut spec = Hand::new("test://contract-link/spec");
        let op = spec.doc("contract::api::DELETE:/users/{id}", &op_origin("delete", "/users/{id}", "/users/{id}"));
        let mut svc = Hand::new("test://contract-link/svc");
        let del = svc.route("DELETE", "/users/:id");
        let mut merged = MergedGraph::new(vec![spec.graph(), svc.graph()]);

        link_contract_routes(&mut merged);
        // Method is case-folded; `{id}` and `:id` normalise to one key.
        assert_eq!(documents(&merged.cross_edges, op), vec![(del, Confidence::Strong)]);
    }

    #[test]
    fn contract_link_is_silent_without_contract_ops_or_routes() {
        // No contract op: nothing, and no route index is built.
        let mut h = Hand::new("test://contract-link/none");
        h.route("GET", "/users");
        h.doc("README::intro", r#"{"provenance":"test_fixture"}"#);
        let (edges, stats) = contract_route_edges(&MergedGraph::new(vec![h.graph()]));
        assert!(edges.is_empty());
        assert_eq!(stats, ContractLinkStats::default());

        // Ops but no routes: every op is unmatched.
        let mut h = Hand::new("test://contract-link/no-routes");
        h.doc("contract::api::GET:/users", &op_origin("GET", "/users", "/users"));
        let (edges, stats) = contract_route_edges(&MergedGraph::new(vec![h.graph()]));
        assert!(edges.is_empty());
        assert_eq!(stats, ContractLinkStats { ops: 1, exact: 0, prefix: 0, unmatched: 1 });
    }

    #[test]
    fn contract_op_reads_only_http_contract_origins() {
        let origin = |j: &str| vec![Cell { kind: cell_type::ORIGIN, payload: CellPayload::Json(j.into()) }];
        // operation_id is OMITTED, not null, when the spec has none.
        assert_eq!(
            contract_op(&origin(
                r#"{"provenance":"contract","source":"openapi","method":"get","path":"/api/v1/users","raw_path":"/users"}"#
            )),
            Some(ContractOp {
                method: "GET".into(),
                path: "/api/v1/users".into(),
                raw_path: Some("/users".into()),
            })
        );
        // An AsyncAPI channel op is not an HTTP op.
        assert_eq!(
            contract_op(&origin(r#"{"provenance":"contract","source":"asyncapi","method":"publish","path":"orders"}"#)),
            None
        );
        // "contract" as some OTHER field's value passes the cheap substring
        // reject, and the parsed provenance still refuses it.
        assert_eq!(
            contract_op(&origin(
                r#"{"provenance":"documentation","source":"contract","method":"GET","path":"/x"}"#
            )),
            None
        );
        // Malformed JSON and a Text payload are both ignored, never a panic.
        assert_eq!(contract_op(&origin(r#"{"provenance":"contract""#)), None);
        let text = vec![Cell {
            kind: cell_type::ORIGIN,
            payload: CellPayload::Text(op_origin("GET", "/users", "/users")),
        }];
        assert_eq!(contract_op(&text), None);
    }

    #[test]
    fn backtick_identifiers_extract_inline_code(){
        let md = "Use `parse_config` and `WidgetFactory.build()`.\n\
                  Run `npm install` (ignored). `x` too short.\n\
                  ```\nfenced `not_this`\n```";
        let ids = backtick_identifiers(md);
        assert!(ids.contains(&"parse_config".to_string()));
        // method span reduces to the trailing identifier.
        assert!(ids.contains(&"build".to_string()));
        // "npm install" has a space → not an identifier; "x" too short.
        assert!(!ids.iter().any(|s| s.contains(' ')));
        assert!(!ids.contains(&"x".to_string()));
    }

    #[test]
    fn identifier_from_span_normalises() {
        assert_eq!(identifier_from_span("parse_config()"), Some("parse_config".into()));
        assert_eq!(identifier_from_span("mod::Thing"), Some("Thing".into()));
        assert_eq!(identifier_from_span("a.b.method"), Some("method".into()));
        assert_eq!(identifier_from_span("--flag"), None);
        assert_eq!(identifier_from_span("ab"), None); // too short
    }

    #[test]
    fn proto_and_test_fixture_detection() {
        // generated_proto: codegen extensions across languages (glia-v3 #5).
        assert!(is_generated_proto("chatpb/chat.pb.go"));
        assert!(is_generated_proto("gen/chat.pb-grpc.go"));
        assert!(is_generated_proto("lib/proto/chat.pbjson.dart"));
        assert!(is_generated_proto("proto/chat_pb2.py"));
        assert!(!is_generated_proto("src/chat.go"));
        assert!(!is_generated_proto("src/app/chat.component.ts"));
        // test_fixture: paths + qname shapes (glia-v3 #6).
        assert!(is_test_fixture("services/auth_test.go", "turps::auth"));
        assert!(is_test_fixture("app/login.spec.ts", "quokka_web::login"));
        assert!(is_test_fixture("pkg/foo.go", "pkg::tests::seed_users"));
        assert!(!is_test_fixture("services/auth.go", "turps::auth::HashPassword"));
    }

    #[test]
    fn position_file_extraction() {
        use repo_graph_code_domain::cell_type;
        use repo_graph_core::{Cell, CellPayload};
        let cells = vec![Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(
                r#"{"file":"src/app/a.ts","start_line":3,"end_line":9}"#.into(),
            ),
        }];
        assert_eq!(position_file(&cells).as_deref(), Some("src/app/a.ts"));
        assert_eq!(position_file(&[]), None);
    }
}
