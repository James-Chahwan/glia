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
