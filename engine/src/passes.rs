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
/// A10.3: an AsyncAPI channel op (`action` + `channel`, no `method`) pairs the
/// same way with the queue node named for its channel — see
/// [`channel_edges`]. It prints its own `[contract-link] channels=` line so
/// neither half ever rewrites the other's marker.
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
            edges.len() - stats.channel_edges
        );
    }
    // A10.3 fired_on marker: `... 2>&1 | grep '^\[contract-link\] channels='`
    if stats.channels > 0 {
        eprintln!(
            "[contract-link] channels={} paired={} crossed={} unmatched={} edges={}",
            stats.channels,
            stats.paired,
            stats.crossed,
            stats.channel_unmatched,
            stats.channel_edges
        );
    }
    merged.cross_edges.extend(edges);
}

/// Counters behind the `[contract-link]` markers.
///
/// HTTP half: `exact` + `prefix` + `unmatched` == `ops`: each op is counted
/// once, by the tier that paired it (`prefix` = every non-exact pairing, see
/// `link_contract_routes`).
///
/// Channel half (A10.3): `paired` + `channel_unmatched` == `channels`;
/// `crossed` ⊆ `paired` counts the ops that only paired with the OTHER side's
/// queue node; `channel_edges` is how many of the returned edges are theirs.
#[derive(Debug, Default, PartialEq, Eq)]
struct ContractLinkStats {
    ops: usize,
    exact: usize,
    prefix: usize,
    unmatched: usize,
    channels: usize,
    paired: usize,
    crossed: usize,
    channel_unmatched: usize,
    channel_edges: usize,
}

/// The HTTP half of a contract operation, read off its ORIGIN cell.
#[derive(Debug, PartialEq, Eq)]
struct ContractOp {
    method: String,
    path: String,
    raw_path: Option<String>,
}

/// A10.3: the message half of a contract operation, read off its ORIGIN cell.
#[derive(Debug, PartialEq, Eq)]
struct ChannelOp {
    /// `true` for `publish` (pairs with producers first), `false` for
    /// `subscribe` (consumers first).
    publish: bool,
    channel: String,
}

/// The edges `link_contract_routes` adds, without touching the graph. Split out
/// so the unit tests can assert edges and counters on a hand-built merge.
fn contract_route_edges(merged: &MergedGraph) -> (Vec<Edge>, ContractLinkStats) {
    use repo_graph_graph::HttpRouteMatcher;

    let mut stats = ContractLinkStats::default();
    // Collect the ops first: almost no build has a contract file, and those
    // builds must not pay for a ROUTE or queue index.
    let mut ops: Vec<(NodeId, ContractOp)> = Vec::new();
    let mut channels: Vec<(NodeId, ChannelOp)> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id).copied() != Some(node_kind::DOC_SECTION) {
                continue;
            }
            let Some(origin) = contract_origin(&n.cells) else {
                continue;
            };
            if let Some(op) = http_op(&origin) {
                ops.push((n.id, op));
            } else if let Some(op) = channel_op(&origin) {
                channels.push((n.id, op));
            }
        }
    }
    let mut edges: Vec<Edge> = Vec::new();
    if !channels.is_empty() {
        channel_edges(merged, &channels, &mut stats, &mut edges);
    }
    if ops.is_empty() {
        return (edges, stats);
    }
    stats.ops = ops.len();

    let matcher = HttpRouteMatcher::new(&merged.graphs);
    if matcher.is_empty() {
        stats.unmatched = ops.len();
        return (edges, stats);
    }
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

/// A10.3: queue nodes keyed by their normalised topic, one map per side. Only
/// ever LOOKED UP, never iterated (CODE_RULES §3).
#[derive(Default)]
struct QueueSides<'a> {
    producers: HashMap<&'a str, Vec<(NodeId, Confidence)>>,
    consumers: HashMap<&'a str, Vec<(NodeId, Confidence)>>,
}

/// The one normalisation channel matching allows: surrounding `/` trimmed.
/// Deliberately NOT case-folded — queue topics are case-sensitive.
fn norm_channel(s: &str) -> &str {
    s.trim_matches('/')
}

impl<'a> QueueSides<'a> {
    /// Index every QUEUE_PRODUCER / QUEUE_CONSUMER by the topic in its qname,
    /// minus the LB.8 owner segment (every owner of a topic documents under
    /// its one channel). A framework-tag node
    /// (`queue_producer:unresolved:kafka`) names no topic and is left out, so
    /// a channel literally called `kafka` cannot pair with every Kafka file
    /// whose topic failed to parse.
    fn build(merged: &'a MergedGraph) -> Self {
        use repo_graph_code_domain::endpoint::split_owner;
        use repo_graph_code_extractors::queues::UNRESOLVED_PREFIX;

        let mut ix = QueueSides::default();
        for g in &merged.graphs {
            for n in &g.nodes {
                let (map, prefix) = match g.nav.kind_by_id.get(&n.id).copied() {
                    Some(node_kind::QUEUE_PRODUCER) => (&mut ix.producers, "queue_producer:"),
                    Some(node_kind::QUEUE_CONSUMER) => (&mut ix.consumers, "queue_consumer:"),
                    _ => continue,
                };
                let Some(topic) = g
                    .nav
                    .qname_by_id
                    .get(&n.id)
                    .and_then(|q| split_owner(q).0.strip_prefix(prefix))
                else {
                    continue;
                };
                if topic.starts_with(UNRESOLVED_PREFIX) {
                    continue;
                }
                let key = norm_channel(topic);
                if key.is_empty() {
                    continue;
                }
                map.entry(key).or_default().push((n.id, n.confidence));
            }
        }
        ix
    }
}

/// A10.3: pair each AsyncAPI channel op with the queue node(s) named for its
/// channel, as DOCUMENTS cross-edges **contract → queue node**.
/// - `Strong` — the same side (`publish` → QUEUE_PRODUCER, `subscribe` →
///   QUEUE_CONSUMER) has a node with exactly that topic;
/// - `Medium` — only the other side does. AsyncAPI v2 documents a channel from
///   the publisher's viewpoint (and v2's `publish` famously means "others
///   publish to me"), so a repo holding only the opposite half still has this
///   contract as its documentation — but that reading is an inference;
/// - nothing — neither side names the channel. Never a fan-out.
///
/// Matching is exact on the channel string after [`norm_channel`]. Neither
/// tier is ever above the queue node's own confidence.
fn channel_edges(
    merged: &MergedGraph,
    channels: &[(NodeId, ChannelOp)],
    stats: &mut ContractLinkStats,
    edges: &mut Vec<Edge>,
) {
    stats.channels = channels.len();
    let sides = QueueSides::build(merged);
    for (op_id, op) in channels {
        let key = norm_channel(&op.channel);
        let (same, other) = if op.publish {
            (&sides.producers, &sides.consumers)
        } else {
            (&sides.consumers, &sides.producers)
        };
        let (hits, tier) = match same.get(key) {
            Some(h) => (h, Confidence::Strong),
            None => match other.get(key) {
                Some(h) => {
                    stats.crossed += 1;
                    (h, Confidence::Medium)
                }
                None => {
                    stats.channel_unmatched += 1;
                    continue;
                }
            },
        };
        stats.paired += 1;
        for &(to, node_conf) in hits {
            edges.push(Edge {
                from: *op_id,
                to,
                category: edge_category::DOCUMENTS,
                confidence: weaker(tier, node_conf),
            });
            stats.channel_edges += 1;
        }
    }
}

fn weaker(a: Confidence, b: Confidence) -> Confidence {
    // Declaration order is Strong < Medium < Weak.
    if (a as u8) >= (b as u8) { a } else { b }
}

/// The parsed ORIGIN payload of a contract op, or `None` when the node is not
/// one (markdown sections have no ORIGIN here; other ORIGIN payloads say a
/// different provenance).
fn contract_origin(cells: &[repo_graph_core::Cell]) -> Option<serde_json::Value> {
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
    Some(v)
}

/// The HTTP operation a contract ORIGIN declares, or `None` when it is not an
/// HTTP one (an AsyncAPI channel op has no HTTP `method`).
fn http_op(v: &serde_json::Value) -> Option<ContractOp> {
    let method = v.get("method")?.as_str()?.to_ascii_uppercase();
    if !is_http_verb(&method) {
        return None;
    }
    let path = v.get("path")?.as_str()?.to_string();
    let raw_path = v.get("raw_path").and_then(|p| p.as_str()).map(str::to_string);
    Some(ContractOp { method, path, raw_path })
}

/// A10.3: the channel operation a contract ORIGIN declares. An ORIGIN with a
/// `method` is an HTTP op (or malformed) and is never read as a channel.
fn channel_op(v: &serde_json::Value) -> Option<ChannelOp> {
    if v.get("method").is_some() {
        return None;
    }
    let publish = match v.get("action")?.as_str()? {
        "publish" => true,
        "subscribe" => false,
        _ => return None,
    };
    let channel = v.get("channel")?.as_str()?;
    if norm_channel(channel).is_empty() {
        return None;
    }
    Some(ChannelOp { publish, channel: channel.to_string() })
}

/// The HTTP contract operation a DOC_SECTION's cells declare (test seam for
/// [`contract_origin`] + [`http_op`]).
#[cfg(test)]
fn contract_op(cells: &[repo_graph_core::Cell]) -> Option<ContractOp> {
    http_op(&contract_origin(cells)?)
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

/// Module-level TESTS edges emitted by one run, split by which affix family
/// paired the test module with its target (A6.7).
#[derive(Debug, Default, PartialEq, Eq)]
struct TestsEdgeStats {
    /// `test_x` / `x_test` / `x_spec` / `x.test` / `x.spec` — the pre-A6.7 rule.
    snake: usize,
    /// `XTest` / `XTests` / `XTestCase` / `XSpec` / `XSpecs` / `TestX`.
    camel: usize,
}

fn emit_tests_edges(merged: &mut MergedGraph) {
    let (edges, stats) = tests_module_edges(merged);
    // A6.7 fired_on marker: `... 2>&1 | grep '^\[tests\] module TESTS edges:'`
    if stats.snake + stats.camel > 0 {
        eprintln!(
            "[tests] module TESTS edges: {} (snake={} camel={})",
            stats.snake + stats.camel,
            stats.snake,
            stats.camel
        );
    }
    merged.cross_edges.extend(edges);
}

/// Pair each test MODULE with the module(s) it tests by stripping the test
/// affix off its qname tail and looking the stem up among module tails.
fn tests_module_edges(merged: &MergedGraph) -> (Vec<Edge>, TestsEdgeStats) {
    let mut edges = Vec::new();
    let mut stats = TestsEdgeStats::default();
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
        if !is_test_module_qname(qname) {
            continue;
        }
        let Some(tail) = qname.rsplit("::").next() else { continue };
        let stripped = strip_test_affixes(tail);
        if stripped.is_empty() || stripped == tail {
            continue;
        }
        let Some(candidates) = modules_by_tail.get(stripped) else { continue };
        let snake = strip_snake_test_affixes(tail) != tail;
        for to_id in select_test_targets(*from_id, qname, candidates) {
            if snake {
                stats.snake += 1;
            } else {
                stats.camel += 1;
            }
            edges.push(Edge {
                from: *from_id,
                to: to_id,
                category: edge_category::TESTS,
                confidence: Confidence::Strong,
            });
        }
    }
    (edges, stats)
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

/// Test-module gate for TESTS-edge emission only (A6.7). Deliberately distinct
/// from `is_test_qname`, which also drives the ORIGIN `test_fixture`
/// provenance cell via `is_test_fixture` — broadening that would change the
/// existing cell on every Java/C#/Swift test class. This adds only the
/// CamelCase tails (`CalcTest`, `CalcTests`, `TestCalc`, ...) that sit outside
/// any test/ directory, e.g. a flat or PSR-4 layout.
fn is_test_module_qname(qname: &str) -> bool {
    if is_test_qname(qname) {
        return true;
    }
    let Some(tail) = qname.rsplit("::").next() else { return false };
    strip_test_affixes(tail) != tail
}

/// Strip a test affix off a module tail: the snake_case arms first, so every
/// name the pre-A6.7 rule stripped strips identically, then the CamelCase arms.
/// Returns `name` unchanged when neither family matches.
fn strip_test_affixes(name: &str) -> &str {
    let snake = strip_snake_test_affixes(name);
    if snake != name {
        return snake;
    }
    strip_camel_test_affixes(name)
}

/// `test_x`, `x_test`, `x_spec`, `x.test`, `x.spec` (ASCII case-insensitive).
fn strip_snake_test_affixes(name: &str) -> &str {
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

/// CamelCase conventions: JUnit/xUnit/NUnit `FooTest`/`FooTests`, ScalaTest
/// and Quick `FooSpec`/`FooSpecs`, XCTest `FooTests`, JUnit `FooTestCase`, and
/// the prefix form `TestFoo`. Matched case-SENSITIVELY on the capital, and the
/// remaining stem must itself start uppercase, so a production name like
/// `Fastest`, `Manifest` or `Latest` never strips. Order matters: `TestCase`
/// and `Tests` before `Test`, `Specs` before `Spec`.
fn strip_camel_test_affixes(name: &str) -> &str {
    const CAMEL_SUFFIXES: [&str; 5] = ["TestCase", "Tests", "Test", "Specs", "Spec"];
    for suffix in CAMEL_SUFFIXES {
        if let Some(stem) = name.strip_suffix(suffix)
            && stem.chars().next().is_some_and(char::is_uppercase)
        {
            return stem;
        }
    }
    if let Some(rest) = name.strip_prefix("Test")
        && rest.chars().next().is_some_and(char::is_uppercase)
    {
        return rest;
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
        assert_eq!(stats, ContractLinkStats { ops: 2, exact: 1, prefix: 1, unmatched: 0, ..Default::default() });
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
        assert_eq!(stats, ContractLinkStats { ops: 2, exact: 0, prefix: 1, unmatched: 1, ..Default::default() });
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
        assert_eq!(stats, ContractLinkStats { ops: 1, exact: 0, prefix: 0, unmatched: 1, ..Default::default() });
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

    // ------------------------------------------------------------------
    // A10.3 — AsyncAPI channel ops → queue nodes
    // ------------------------------------------------------------------

    /// The exact ORIGIN payload A10.3's extractor writes (see contracts.rs
    /// `asyncapi_v2_channels_yield_one_op_per_publish_subscribe`).
    fn chan_origin(action: &str, channel: &str) -> String {
        format!(
            r#"{{"provenance":"contract","source":"asyncapi","action":"{action}","channel":"{channel}","operation_id":"op"}}"#
        )
    }

    impl Hand {
        fn producer(&mut self, topic: &str) -> NodeId {
            let q = format!("queue_producer:{topic}");
            self.add(node_kind::QUEUE_PRODUCER, topic, &q, vec![])
        }
        fn consumer(&mut self, topic: &str) -> NodeId {
            let q = format!("queue_consumer:{topic}");
            self.add(node_kind::QUEUE_CONSUMER, topic, &q, vec![])
        }
    }

    #[test]
    fn channel_link_same_side_strong_other_side_medium_miss_none() {
        let mut h = Hand::new("test://contract-link/chan");
        let prod_orders = h.producer("orders");
        let cons_orders = h.consumer("orders");
        let cons_audit = h.consumer("audit/events");
        // Same side exists: Strong to it, and NOT to the other side.
        let pub_orders = h.doc("contract::asyncapi::publish:orders", &chan_origin("publish", "orders"));
        let sub_orders = h.doc("contract::asyncapi::subscribe:orders", &chan_origin("subscribe", "orders"));
        // Only the other side exists: Medium; `/`-trimmed on both sides.
        let pub_audit = h.doc("contract::asyncapi::publish:/audit/events/", &chan_origin("publish", "/audit/events/"));
        // Neither side: no edge, never a fan-out.
        let miss = h.doc("contract::asyncapi::publish:payments", &chan_origin("publish", "payments"));
        // Case-sensitive: `Orders` is not `orders`.
        let cased = h.doc("contract::asyncapi::subscribe:Orders", &chan_origin("subscribe", "Orders"));
        let merged = MergedGraph::new(vec![h.graph()]);

        let (edges, stats) = contract_route_edges(&merged);
        assert_eq!(documents(&edges, pub_orders), vec![(prod_orders, Confidence::Strong)]);
        assert_eq!(documents(&edges, sub_orders), vec![(cons_orders, Confidence::Strong)]);
        assert_eq!(documents(&edges, pub_audit), vec![(cons_audit, Confidence::Medium)]);
        assert!(documents(&edges, miss).is_empty());
        assert!(documents(&edges, cased).is_empty());
        assert_eq!(edges.len(), 3);
        assert_eq!(
            stats,
            ContractLinkStats {
                channels: 5,
                paired: 3,
                crossed: 1,
                channel_unmatched: 2,
                channel_edges: 3,
                ..Default::default()
            }
        );
    }

    #[test]
    fn channel_link_skips_framework_tag_nodes_and_pairs_across_repos() {
        // A tag-fallback node names no topic: a channel literally called
        // `unresolved:kafka` (or `kafka`) must not pair with it.
        let mut svc = Hand::new("test://contract-link/chan-svc");
        svc.add(node_kind::QUEUE_PRODUCER, "kafka", "queue_producer:unresolved:kafka", vec![]);
        let weak_cons = {
            let id = svc.consumer("orders");
            if let Some(n) = svc.nodes.iter_mut().find(|n| n.id == id) {
                n.confidence = Confidence::Weak;
            }
            id
        };
        let mut spec = Hand::new("test://contract-link/chan-spec");
        let tag = spec.doc("contract::api::publish:unresolved:kafka", &chan_origin("publish", "unresolved:kafka"));
        let bare = spec.doc("contract::api::publish:kafka", &chan_origin("publish", "kafka"));
        let sub = spec.doc("contract::api::subscribe:orders", &chan_origin("subscribe", "orders"));
        let mut merged = MergedGraph::new(vec![spec.graph(), svc.graph()]);

        link_contract_routes(&mut merged);
        assert!(documents(&merged.cross_edges, tag).is_empty());
        assert!(documents(&merged.cross_edges, bare).is_empty());
        // Cross-repo, and never above the queue node's own confidence.
        assert_eq!(documents(&merged.cross_edges, sub), vec![(weak_cons, Confidence::Weak)]);
    }

    #[test]
    fn channel_and_http_halves_count_separately() {
        let mut h = Hand::new("test://contract-link/both");
        let get_users = h.route("GET", "/users");
        let prod = h.producer("orders");
        let http = h.doc("contract::openapi::GET:/users", &op_origin("GET", "/users", "/users"));
        let chan = h.doc("contract::asyncapi::publish:orders", &chan_origin("publish", "orders"));
        // An ORIGIN carrying BOTH an HTTP method and a channel is an HTTP op
        // (or malformed) — never read as a channel.
        let hybrid = h.doc(
            "contract::x::hybrid",
            r#"{"provenance":"contract","method":"PATCH","path":"/nope","action":"publish","channel":"orders"}"#,
        );
        let merged = MergedGraph::new(vec![h.graph()]);

        let (edges, stats) = contract_route_edges(&merged);
        assert_eq!(documents(&edges, http), vec![(get_users, Confidence::Strong)]);
        assert_eq!(documents(&edges, chan), vec![(prod, Confidence::Strong)]);
        assert!(documents(&edges, hybrid).is_empty());
        assert_eq!(
            stats,
            ContractLinkStats {
                ops: 2,
                exact: 1,
                unmatched: 1,
                channels: 1,
                paired: 1,
                channel_edges: 1,
                ..Default::default()
            }
        );
        // The HTTP line's `edges=` is edges.len() - channel_edges.
        assert_eq!(edges.len() - stats.channel_edges, 1);

        // Channel ops with no queue nodes and no HTTP ops: all unmatched, and
        // the HTTP half stays silent (ops == 0).
        let mut h = Hand::new("test://contract-link/no-queues");
        h.doc("contract::asyncapi::subscribe:orders", &chan_origin("subscribe", "orders"));
        let (edges, stats) = contract_route_edges(&MergedGraph::new(vec![h.graph()]));
        assert!(edges.is_empty());
        assert_eq!(
            stats,
            ContractLinkStats { channels: 1, channel_unmatched: 1, ..Default::default() }
        );
    }

    #[test]
    fn channel_op_reads_only_asyncapi_shaped_origins() {
        let parse = |j: &str| {
            let cells = vec![Cell { kind: cell_type::ORIGIN, payload: CellPayload::Json(j.into()) }];
            contract_origin(&cells).and_then(|v| channel_op(&v))
        };
        assert_eq!(
            parse(&chan_origin("subscribe", "user/signedup")),
            Some(ChannelOp { publish: false, channel: "user/signedup".into() })
        );
        // v3 verbs are mapped by the extractor; raw ones are not accepted here.
        assert_eq!(parse(&chan_origin("send", "orders")), None);
        // An empty / all-slash channel names nothing.
        assert_eq!(parse(&chan_origin("publish", "/")), None);
        // Not a contract provenance.
        assert_eq!(
            parse(r#"{"provenance":"documentation","source":"contract","action":"publish","channel":"orders"}"#),
            None
        );
        // An OpenAPI op is not a channel op.
        assert_eq!(parse(&op_origin("GET", "/users", "/users")), None);
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
    fn strip_test_affixes_camel_and_snake() {
        // A6.7 CamelCase arms.
        assert_eq!(strip_test_affixes("UserServiceTest"), "UserService");
        assert_eq!(strip_test_affixes("UserServiceTests"), "UserService");
        assert_eq!(strip_test_affixes("UserServiceTestCase"), "UserService");
        assert_eq!(strip_test_affixes("UserServiceSpec"), "UserService");
        assert_eq!(strip_test_affixes("UserServiceSpecs"), "UserService");
        assert_eq!(strip_test_affixes("TestUserService"), "UserService");
        // Must-not-strip: a lowercase `test` inside a production word, the bare
        // affix itself, and a lowercase stem or remainder.
        assert_eq!(strip_test_affixes("Fastest"), "Fastest");
        assert_eq!(strip_test_affixes("Manifest"), "Manifest");
        assert_eq!(strip_test_affixes("Latest"), "Latest");
        assert_eq!(strip_test_affixes("Test"), "Test");
        assert_eq!(strip_test_affixes("Tests"), "Tests");
        assert_eq!(strip_test_affixes("Testimonial"), "Testimonial");
        assert_eq!(strip_test_affixes("userServiceTest"), "userServiceTest");
        // The snake_case rule is unchanged and still wins first.
        assert_eq!(strip_test_affixes("test_calc"), "calc");
        assert_eq!(strip_test_affixes("calc_test"), "calc");
        assert_eq!(strip_test_affixes("math.test"), "math");
        assert_eq!(strip_test_affixes("Login.spec"), "Login");
    }

    #[test]
    fn camel_test_module_gate_leaves_provenance_alone() {
        // The ORIGIN `test_fixture` cell reads is_test_fixture -> is_test_qname,
        // and A6.7 must not move it: a flat-layout `FooTest` stays unflagged.
        assert!(!is_test_fixture("src/Foo.java", "src::FooTest"));
        assert!(!is_test_qname("src::FooTest"));
        // ...while the TESTS-only gate accepts it.
        assert!(is_test_module_qname("src::FooTest"));
        assert!(is_test_module_qname("CalcTests"));
        assert!(is_test_module_qname("src::test::java::helpers"));
        assert!(!is_test_module_qname("src::Manifest"));
        assert!(!is_test_module_qname("src::Calc"));
    }

    #[test]
    fn tests_module_edges_pair_camel_and_snake_modules() {
        let mut h = Hand::new("test://tests-edges/one");
        let mut module = |q: &str| {
            let tail = q.rsplit("::").next().unwrap_or(q).to_string();
            h.add(node_kind::MODULE, &tail, q, vec![])
        };
        // Flat JUnit/PHPUnit layout: only the CamelCase tail marks the test.
        let calc = module("Calc");
        let calc_test = module("CalcTest");
        // Maven layout: the path already marks it; the affix used to block it.
        let user_service = module("src::main::java::UserService");
        let user_service_test = module("src::test::java::UserServiceTest");
        // Prefix form under a tests/ dir.
        let cart = module("src::Cart");
        let test_cart = module("tests::TestCart");
        // Pre-A6.7 snake cases, unchanged.
        let py_calc = module("pkg::calc");
        let py_test = module("pkg::test_calc");
        let math = module("math");
        let math_test = module("math.test");
        // A production name ending in lowercase `test` pairs with nothing.
        let _fas = module("Fas");
        let _fastest = module("Fastest");
        let merged = MergedGraph::new(vec![h.graph()]);

        let (edges, stats) = tests_module_edges(&merged);
        let mut got: Vec<(NodeId, NodeId)> = edges
            .iter()
            .inspect(|e| assert_eq!(e.category, edge_category::TESTS))
            .map(|e| (e.from, e.to))
            .collect();
        got.sort_by_key(|(f, t)| (f.0, t.0));
        let mut want = vec![
            (calc_test, calc),
            (user_service_test, user_service),
            (test_cart, cart),
            (py_test, py_calc),
            (math_test, math),
        ];
        want.sort_by_key(|(f, t)| (f.0, t.0));
        assert_eq!(got, want);
        assert_eq!(stats, TestsEdgeStats { snake: 2, camel: 3 });
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
