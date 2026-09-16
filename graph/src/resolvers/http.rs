//! HTTP stack resolver — frontend Endpoint → backend Route by
//! (method, normalised path).

use std::collections::HashMap;

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, NodeId};

use super::{CrossGraphResolver, weakest};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

/// Pairs frontend HTTP Endpoints with backend HTTP Routes by (method,
/// normalised path) and emits `HTTP_CALLS` edges.
///
/// Matching rule:
/// - Endpoint qname `endpoint:<METHOD>:<path>` is the source side. Method comes
///   straight from the qname; path is normalised (see `normalise_http_path`).
/// - Route qname `route:<path>` — one Route node per path across all methods.
///   Methods live on stacked `ROUTE_METHOD` cells. Each (path, method) pair is
///   a distinct target.
/// - Cross-repo is the common case (Angular → Go gin backend), but same-repo
///   matches also link correctly (Next.js route-handlers + fetchers, etc.).
/// - Emitted edge confidence = min(endpoint_node_confidence, Strong) since
///   Routes are always Strong at v0.4.4 — i.e. the endpoint's confidence wins.
///
/// Collisions (multiple Routes with the same method+path across repos) emit
/// one edge per target. Rare in real corpora but cheap to handle.
pub struct HttpStackResolver;

impl CrossGraphResolver for HttpStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let index = build_route_index(&merged.graphs);
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ENDPOINT) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                let Some((method, raw_path)) = parse_endpoint_qname(qname) else {
                    continue;
                };
                if raw_path == "<unresolved>" {
                    continue;
                }
                let norm = normalise_http_path(raw_path);
                let targets = lookup_route_with_prefix_strip(&index, &method, &norm);
                for target in targets {
                    merged.cross_edges.push(Edge {
                        from: n.id,
                        to: target.route_id,
                        category: edge_category::HTTP_CALLS,
                        confidence: weakest(n.confidence, target.confidence),
                    });
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RouteTarget {
    route_id: NodeId,
    confidence: Confidence,
}

/// Build `(METHOD, normalised_path) → Vec<RouteTarget>` across every graph in
/// the merge. One entry per `ROUTE_METHOD` cell found on each Route node.
fn build_route_index(
    graphs: &[RepoGraph],
) -> HashMap<(String, String), Vec<RouteTarget>> {
    let mut index: HashMap<(String, String), Vec<RouteTarget>> = HashMap::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ROUTE) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            let target = RouteTarget {
                route_id: n.id,
                confidence: n.confidence,
            };
            index_route_node(&mut index, qname, &n.cells, target);
        }
    }
    index
}

/// Register a ROUTE node into the (METHOD, path) index. Handles both qname
/// conventions now in the repo:
///   1. parser-go / ts_routes: qname = `route:<path>`, methods live on
///      stacked ROUTE_METHOD cells (JSON payload).
///   2. parser-java / parser-csharp / parser-rust / parser-php: qname =
///      `<METHOD> <path>`, one Route node per (method, path) with a single
///      ROUTE_METHOD cell carrying the method as a plain Text payload.
///
/// Both shapes target the same downstream key space so HttpStackResolver sees
/// all routes uniformly. Migrate the non-Go parsers to shape (1) when the
/// other resolvers start needing per-path aggregation.
fn index_route_node(
    index: &mut HashMap<(String, String), Vec<RouteTarget>>,
    qname: &str,
    cells: &[Cell],
    target: RouteTarget,
) {
    if let Some(path) = qname.strip_prefix("route:") {
        let norm = normalise_http_path(path);
        for cell in cells {
            if cell.kind != cell_type::ROUTE_METHOD {
                continue;
            }
            let Some(method) = cell_method(cell) else {
                continue;
            };
            index
                .entry((method.to_ascii_uppercase(), norm.clone()))
                .or_default()
                .push(target);
        }
        return;
    }
    // Legacy shape: "<METHOD> <path>". Split on the first space.
    if let Some((method, path)) = qname.split_once(' ')
        && path.starts_with('/')
    {
        let norm = normalise_http_path(path);
        index
            .entry((method.to_ascii_uppercase(), norm))
            .or_default()
            .push(target);
    }
}

/// Extract a method name from a ROUTE_METHOD cell, handling both the JSON
/// payload used by parser-go/ts_routes and the plain Text payload used by
/// parser-java/csharp/rust/php.
fn cell_method(cell: &Cell) -> Option<String> {
    match &cell.payload {
        CellPayload::Json(json) => extract_method_field(json).map(|s| s.to_string()),
        CellPayload::Text(s) => Some(s.clone()),
        CellPayload::Bytes(_) => None,
    }
}

fn parse_endpoint_qname(qname: &str) -> Option<(String, &str)> {
    let rest = qname.strip_prefix("endpoint:")?;
    let (method, path) = rest.split_once(':')?;
    Some((method.to_uppercase(), path))
}

/// Extract the `method` string field from a `ROUTE_METHOD` cell's JSON payload.
/// Minimal parse — the payload is a flat object written by parser-go, not
/// arbitrary user JSON, so a tight scan is enough and keeps us off serde_json
/// as a graph-crate dependency.
fn extract_method_field(json: &str) -> Option<&str> {
    let key = "\"method\"";
    let idx = json.find(key)?;
    let after = &json[idx + key.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// Collapse path param syntaxes into a stable form so a frontend endpoint's
/// `/users/${id}` matches a backend route's `/users/:id` or `/users/{id}`.
/// Rules:
/// - Leading slash normalised to exactly one.
/// - Trailing slash stripped (except on the root).
/// - Segment matching `:x`, `{x}`, `${…}` (tree-sitter substitution marker),
///   or any segment containing `${` → `{}`.
/// - Empty segments collapse (so `//foo` → `/foo`).
pub fn normalise_http_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "/".to_string();
    }
    let body = trimmed.trim_matches('/');
    if body.is_empty() {
        return "/".to_string();
    }
    let segs: Vec<String> = body
        .split('/')
        .filter(|s| !s.is_empty())
        .map(normalise_segment)
        .collect();
    format!("/{}", segs.join("/"))
}

fn normalise_segment(seg: &str) -> String {
    if seg.starts_with(':')
        || (seg.starts_with('{') && seg.ends_with('}'))
        || seg.contains("${")
    {
        "{}".to_string()
    } else {
        seg.to_string()
    }
}

// Hardcoded for now — move to config.yaml if a real codebase needs a custom prefix.
const API_PREFIXES: &[&str] = &["protected", "api", "public", "internal", "v1", "v2", "v3"];

fn lookup_route_with_prefix_strip<'a>(
    index: &'a HashMap<(String, String), Vec<RouteTarget>>,
    method: &str,
    norm_path: &str,
) -> &'a [RouteTarget] {
    let key = (method.to_string(), norm_path.to_string());
    if let Some(targets) = index.get(&key) {
        return targets;
    }
    let segments: Vec<&str> = norm_path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    for strip in 1..=2.min(segments.len().saturating_sub(1)) {
        if !API_PREFIXES.contains(&segments[strip - 1]) {
            break;
        }
        let stripped = format!("/{}", segments[strip..].join("/"));
        let key = (method.to_string(), stripped);
        if let Some(targets) = index.get(&key) {
            return targets;
        }
    }
    &[]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_http_path_collapses_all_param_syntaxes() {
        assert_eq!(normalise_http_path("/users/:id"), "/users/{}");
        assert_eq!(normalise_http_path("/users/{id}"), "/users/{}");
        assert_eq!(normalise_http_path("/users/${…}"), "/users/{}");
        assert_eq!(normalise_http_path("/api/users/:id/posts/:pid"), "/api/users/{}/posts/{}");
        assert_eq!(normalise_http_path("users/list"), "/users/list");
        assert_eq!(normalise_http_path("/users/list/"), "/users/list");
        assert_eq!(normalise_http_path("//double//slash"), "/double/slash");
        assert_eq!(normalise_http_path("/"), "/");
        assert_eq!(normalise_http_path(""), "/");
    }

    #[test]
    fn parse_endpoint_qname_splits_method_and_path() {
        assert_eq!(
            parse_endpoint_qname("endpoint:GET:/users"),
            Some(("GET".to_string(), "/users"))
        );
        assert_eq!(
            parse_endpoint_qname("endpoint:POST:/api/login"),
            Some(("POST".to_string(), "/api/login"))
        );
        assert_eq!(parse_endpoint_qname("route:/users"), None);
    }

    #[test]
    fn extract_method_field_handles_ordering_and_whitespace() {
        let json = r#"{"method":"POST","handler":"h","file":"x.go","line":1,"col":2}"#;
        assert_eq!(extract_method_field(json), Some("POST"));
        let spaced = r#"{ "method" : "GET" , "line" : 0 }"#;
        assert_eq!(extract_method_field(spaced), Some("GET"));
    }
}
