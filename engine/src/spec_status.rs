//! SDD `spec_status` (LE.9b): which declared API operations are implemented,
//! which are declared but missing, and which server routes nobody declared —
//! per route and per feature, in one call. `glia spec-status` and the pyo3
//! `PyGraph.spec_status()` are its transports.
//!
//! Read-only over what the build already put in the graph:
//!
//! - a **declared op** is a DOC_SECTION whose ORIGIN is `provenance: contract`
//!   with an HTTP `method` + `path` and `source` `openapi` (A10.1) or
//!   `feature_yaml` (LE.9a). The ORIGIN is parsed here with the same keys the
//!   contract-link pass reads (`passes::contract_origin` / `http_op`), without
//!   widening that pass. Excluded on purpose: `pact` ops (A10.8) are a
//!   CONSUMER's expectation of a provider, not the provider's declared
//!   surface; AsyncAPI channel ops (A10.3) have no route; handler-annotation
//!   ops (`springdoc`, `swaggo`, `nestjs`, ...) are read off the handler they
//!   document, so they cannot be missing and cannot govern anything;
//! - an op is **implemented** when it has a DOCUMENTS edge to a ROUTE — the
//!   pairing `passes::link_contract_routes` made (A10.2). `confidence` is
//!   that edge's, which the pass caps at the ROUTE's own confidence (a route
//!   no client calls is demoted to `medium` by the http pass), and `pairing`
//!   is the pass's rule from the edge's EVIDENCE: `exact`, or `prefix` for
//!   every inferred pairing (API-prefix strip, `ANY` route, server-base
//!   retry), which is never above `medium`.
//!   One row per (op, route): two features declaring `GET /orders` give two
//!   rows on the one route, and an op two owners serve (LB.4) gives one row
//!   per ROUTE node — rows key on the ROUTE, never on the path string;
//! - **declared_missing** — the op documents no ROUTE;
//! - **undeclared** — a server ROUTE (client-router pages, ORIGIN
//!   `nav_route`, are not API) no declared op documents, in a GOVERNED
//!   service: one implementing at least one declared op. The service is
//!   `arch::service_of` (under `arch::default_keying`) on the route's handler
//!   file, else the route's own file. Routes of every other service are only
//!   counted (`ungoverned_routes`): a repo with no specs must not report every
//!   route as undeclared.
//!
//! `feature` is the op's ORIGIN `feature` (LE.9a), else the stem of the file
//! that declares it (`Path::file_stem` of its POSITION file, so a repo-wide
//! `openapi.yaml` is feature `openapi`) — never a split of the op qname, whose
//! shape LB.12 owns. Undeclared rows have none.
//!
//! Rows are sorted by (feature, status implemented < declared_missing <
//! undeclared, path, method) with undeclared (feature-less) rows last, so two
//! calls over one graph serialise byte-identically. Lines are 1-based
//! ([`crate::Located`], one [`Locator`] per answer).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Confidence, NodeId};
use glia_graph::MergedGraph;
use glia_graph::nav::is_nav_route;

use crate::answers::{Located, Locator};
use crate::arch::{ServiceKeying, default_keying, service_of};

/// The three statuses, in sort order.
pub const IMPLEMENTED: &str = "implemented";
pub const DECLARED_MISSING: &str = "declared_missing";
pub const UNDECLARED: &str = "undeclared";

/// The ORIGIN `source`s that declare a provider's surface.
const DECLARING_SOURCES: [&str; 2] = ["openapi", "feature_yaml"];

/// One (declared op, route) pairing, one unpaired declared op, or one
/// undeclared route.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SpecStatusRow {
    /// The declaring feature; `None` on an undeclared row.
    pub feature: Option<String>,
    /// `openapi` | `feature_yaml`; `None` on an undeclared row.
    pub source: Option<&'static str>,
    /// Upper-case HTTP verb: the op's, or the route's (`ANY` for a
    /// method-agnostic registration).
    pub method: String,
    /// The declared path (server base included), or the route's path.
    pub path: String,
    /// [`IMPLEMENTED`] | [`DECLARED_MISSING`] | [`UNDECLARED`].
    pub status: &'static str,
    /// The DOCUMENTS edge's confidence (`strong` | `medium` | `weak`) on an
    /// implemented row; `None` otherwise.
    pub confidence: Option<&'static str>,
    /// How the contract-link pass paired the op with the route: `exact` |
    /// `prefix` (its EVIDENCE rule). `None` on other rows, and on a DOCUMENTS
    /// edge another emitter (an overlay stanza) asserted.
    pub pairing: Option<&'static str>,
    pub route: Option<Located>,
    /// The first function the route is HANDLED_BY.
    pub handler: Option<Located>,
    /// Where the op is declared.
    pub decl: Option<Located>,
}

/// Declared-op counts for one feature. `declared == implemented +
/// declared_missing`; an op paired with two routes counts once.
#[derive(serde::Serialize, Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FeatureTally {
    pub declared: usize,
    pub implemented: usize,
    pub declared_missing: usize,
}

/// The `spec_status` answer.
#[derive(serde::Serialize, Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SpecStatus {
    pub rows: Vec<SpecStatusRow>,
    pub by_feature: BTreeMap<String, FeatureTally>,
    /// Services implementing at least one declared op — the only ones whose
    /// routes can be undeclared. Sorted.
    pub governed_services: Vec<String>,
    /// Server routes outside every governed service (or unattributable to
    /// one), counted instead of reported.
    pub ungoverned_routes: usize,
}

impl SpecStatus {
    /// `features=<F> declared=<D> implemented=<I> declared_missing=<M>
    /// undeclared=<U> ungoverned=<G>` — op counts from `by_feature`, the
    /// undeclared row count, and `ungoverned_routes`. The body of the
    /// `[sdd] spec_status` marker and of `glia spec-status`'s totals line.
    pub fn summary(&self) -> String {
        let mut t = FeatureTally::default();
        for f in self.by_feature.values() {
            t.declared += f.declared;
            t.implemented += f.implemented;
            t.declared_missing += f.declared_missing;
        }
        let undeclared = self.rows.iter().filter(|r| r.status == UNDECLARED).count();
        format!(
            "features={} declared={} implemented={} declared_missing={} undeclared={undeclared} \
             ungoverned={}",
            self.by_feature.len(),
            t.declared,
            t.implemented,
            t.declared_missing,
            self.ungoverned_routes
        )
    }
}

/// One declared op, read off its ORIGIN cell.
struct DeclaredOp {
    id: NodeId,
    source: &'static str,
    feature: String,
    method: String,
    path: String,
}

/// One DOCUMENTS edge from a declared op: the index of the ROUTE in the
/// route list, the edge's confidence, and the contract-link rule.
struct Pairing {
    route: usize,
    confidence: Confidence,
    rule: Option<&'static str>,
}

/// One server ROUTE: the repo of the graph that first names it, and its
/// `(method, path)`.
struct ServerRoute {
    id: NodeId,
    repo: u64,
    method: String,
    path: String,
}

/// The spec status of every declared op and governed route in `merged`.
/// `repo_labels` feed `arch::service_of` (service ids of merged repos are
/// prefixed with them). `feature` restricts `rows` and `by_feature` to that
/// feature and drops the (feature-less) undeclared rows;
/// `governed_services` and `ungoverned_routes` stay whole-graph facts.
///
/// Prints the `[sdd] spec_status ...` fired_on marker.
pub fn spec_status(
    merged: &MergedGraph,
    repo_labels: &BTreeMap<u64, String>,
    feature: Option<&str>,
) -> SpecStatus {
    let loc = Locator::new(merged);
    let keying = default_keying(merged);

    let mut ops: Vec<DeclaredOp> = Vec::new();
    let mut routes: Vec<ServerRoute> = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            if kind == Some(node_kind::DOC_SECTION) {
                if let Some(op) = declared_op(n.id, &n.cells, &loc)
                    && seen.insert(n.id)
                {
                    ops.push(op);
                }
            } else if kind == Some(node_kind::ROUTE) && !is_nav_route(&n.cells) {
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                if seen.insert(n.id) {
                    let (method, path) = route_method_path(qname, &n.cells);
                    routes.push(ServerRoute {
                        id: n.id,
                        repo: g.repo.0,
                        method,
                        path,
                    });
                }
            }
        }
    }

    // DOCUMENTS op -> ROUTE, and the first HANDLED_BY per route, in edge
    // order (first wins, so HashMap iteration never picks a winner).
    let op_ids: HashSet<NodeId> = ops.iter().map(|o| o.id).collect();
    let route_ix: HashMap<NodeId, usize> =
        routes.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
    let mut paired: HashMap<NodeId, Vec<Pairing>> = HashMap::new();
    let mut handler_of: HashMap<NodeId, NodeId> = HashMap::new();
    for e in merged.all_edges() {
        if e.category == edge_category::DOCUMENTS && op_ids.contains(&e.from) {
            if let Some(&ri) = route_ix.get(&e.to) {
                let hits = paired.entry(e.from).or_default();
                if !hits.iter().any(|h| h.route == ri) {
                    hits.push(Pairing {
                        route: ri,
                        confidence: e.confidence,
                        rule: pairing_rule(&e.cells),
                    });
                }
            }
        } else if e.category == edge_category::HANDLED_BY && route_ix.contains_key(&e.from) {
            handler_of.entry(e.from).or_insert(e.to);
        }
    }

    // Per route: its located handler and route, and the service they place it in.
    let mut placed: Vec<(Located, Option<Located>, Option<String>)> =
        Vec::with_capacity(routes.len());
    for r in &routes {
        let route = loc.locate(r.id);
        let handler = handler_of.get(&r.id).map(|h| loc.locate(*h));
        let file = handler
            .as_ref()
            .and_then(|h| h.file.as_deref())
            .or(route.file.as_deref());
        let service = match file {
            Some(f) => Some(service_of(f, r.repo, &keying, repo_labels)),
            // One service per repo needs no file.
            None if keying == ServiceKeying::PerRepo => {
                Some(service_of("", r.repo, &keying, repo_labels))
            }
            None => None,
        };
        placed.push((route, handler, service));
    }

    let mut governed: BTreeSet<String> = BTreeSet::new();
    let mut documented: HashSet<usize> = HashSet::new();
    for hits in paired.values() {
        for h in hits {
            documented.insert(h.route);
            if let Some(svc) = &placed[h.route].2 {
                governed.insert(svc.clone());
            }
        }
    }

    let mut out = SpecStatus::default();
    for op in &ops {
        if feature.is_some_and(|f| f != op.feature) {
            continue;
        }
        let tally = out.by_feature.entry(op.feature.clone()).or_default();
        tally.declared += 1;
        let decl = Some(loc.locate(op.id));
        let row = |status, confidence, pairing, route, handler| SpecStatusRow {
            feature: Some(op.feature.clone()),
            source: Some(op.source),
            method: op.method.clone(),
            path: op.path.clone(),
            status,
            confidence,
            pairing,
            route,
            handler,
            decl: decl.clone(),
        };
        match paired.get(&op.id) {
            Some(hits) => {
                tally.implemented += 1;
                for h in hits {
                    let (route, handler, _) = &placed[h.route];
                    out.rows.push(row(
                        IMPLEMENTED,
                        Some(confidence_name(h.confidence)),
                        h.rule,
                        Some(route.clone()),
                        handler.clone(),
                    ));
                }
            }
            None => {
                tally.declared_missing += 1;
                out.rows.push(row(DECLARED_MISSING, None, None, None, None));
            }
        }
    }
    for (ri, r) in routes.iter().enumerate() {
        if documented.contains(&ri) {
            continue;
        }
        let (route, handler, service) = &placed[ri];
        if !service.as_ref().is_some_and(|s| governed.contains(s)) {
            out.ungoverned_routes += 1;
            continue;
        }
        if feature.is_some() {
            continue;
        }
        out.rows.push(SpecStatusRow {
            feature: None,
            source: None,
            method: r.method.clone(),
            path: r.path.clone(),
            status: UNDECLARED,
            confidence: None,
            pairing: None,
            route: Some(route.clone()),
            handler: handler.clone(),
            decl: None,
        });
    }
    out.rows.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    out.governed_services = governed.into_iter().collect();

    // fired_on marker: `glia spec-status <repo> 2>&1 | grep '^\[sdd\] spec_status'`
    eprintln!("[sdd] spec_status {}", out.summary());
    out
}

/// Row order: feature (feature-less undeclared rows last), status rank, path,
/// method, then the located ids so rows equal on all four still sort stably.
fn sort_key(r: &SpecStatusRow) -> (bool, &Option<String>, u8, &str, &str, u64, u64) {
    let rank = match r.status {
        IMPLEMENTED => 0,
        DECLARED_MISSING => 1,
        _ => 2,
    };
    (
        r.feature.is_none(),
        &r.feature,
        rank,
        &r.path,
        &r.method,
        r.route.as_ref().map_or(0, |l| l.id),
        r.decl.as_ref().map_or(0, |l| l.id),
    )
}

/// The declared op a DOC_SECTION's cells carry: the first JSON ORIGIN cell
/// (the cell `passes::contract_origin` reads), `provenance: contract`, a
/// declaring `source`, an HTTP verb `method` and a `path`.
fn declared_op(id: NodeId, cells: &[Cell], loc: &Locator<'_>) -> Option<DeclaredOp> {
    let json = cells.iter().find_map(|c| match &c.payload {
        CellPayload::Json(j) if c.kind == cell_type::ORIGIN => Some(j.as_str()),
        _ => None,
    })?;
    if !json.contains("\"contract\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v.get("provenance")?.as_str()? != "contract" {
        return None;
    }
    let source = v.get("source")?.as_str()?;
    let source = *DECLARING_SOURCES.iter().find(|s| **s == source)?;
    let method = v.get("method")?.as_str()?.to_ascii_uppercase();
    if !glia_code_extractors::contracts::METHODS
        .iter()
        .any(|m| m.eq_ignore_ascii_case(&method))
    {
        return None;
    }
    let path = v.get("path")?.as_str()?.to_string();
    let feature = match v.get("feature").and_then(|f| f.as_str()) {
        Some(f) if !f.is_empty() => f.to_string(),
        _ => loc
            .file_of(id)
            .and_then(|f| {
                Path::new(&f)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "(unplaced)".to_string()),
    };
    Some(DeclaredOp {
        id,
        source,
        feature,
        method,
        path,
    })
}

/// `(METHOD, path)` of a server ROUTE, owner segment (LB.4a) split off:
/// `<METHOD> <path>` as named; legacy `route:<path>` with the verbs of its
/// ROUTE_METHOD cells (`,`-joined in cell order, `ANY` when none). Any other
/// qname is its own path under `ANY`.
fn route_method_path(qname: &str, cells: &[Cell]) -> (String, String) {
    let q = split_owner(qname).0;
    if let Some(path) = q.strip_prefix("route:") {
        let mut verbs: Vec<String> = Vec::new();
        for c in cells.iter().filter(|c| c.kind == cell_type::ROUTE_METHOD) {
            let verb = match &c.payload {
                CellPayload::Json(j) => serde_json::from_str::<serde_json::Value>(j)
                    .ok()
                    .and_then(|v| v.get("method")?.as_str().map(str::to_ascii_uppercase)),
                CellPayload::Text(t) => Some(t.trim().to_ascii_uppercase()),
                _ => None,
            };
            if let Some(v) = verb.filter(|v| !v.is_empty() && !verbs.contains(v)) {
                verbs.push(v);
            }
        }
        let method = if verbs.is_empty() {
            "ANY".to_string()
        } else {
            verbs.join(",")
        };
        return (method, path.to_string());
    }
    match q.split_once(' ') {
        Some((m, p)) if p.starts_with('/') => (m.to_ascii_uppercase(), p.to_string()),
        _ => ("ANY".to_string(), q.to_string()),
    }
}

/// The contract-link pass's rule on a DOCUMENTS edge (`exact` | `prefix`), or
/// `None` when another emitter asserted the edge.
fn pairing_rule(cells: &[Cell]) -> Option<&'static str> {
    let ev = Evidence::read(cells)?;
    if ev.emitter != "pass:contract_link" {
        return None;
    }
    match ev.rule.as_deref()? {
        "exact" => Some("exact"),
        "prefix" => Some("prefix"),
        _ => None,
    }
}

fn confidence_name(c: Confidence) -> &'static str {
    match c {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(kind: glia_core::CellTypeId, s: &str) -> Cell {
        Cell {
            kind,
            payload: CellPayload::Text(s.to_string()),
        }
    }

    fn json(kind: glia_core::CellTypeId, s: &str) -> Cell {
        Cell {
            kind,
            payload: CellPayload::Json(s.to_string()),
        }
    }

    #[test]
    fn route_method_path_reads_every_server_shape() {
        assert_eq!(
            route_method_path("GET /orders @services/api", &[]),
            ("GET".to_string(), "/orders".to_string())
        );
        let cells = [
            json(
                cell_type::ROUTE_METHOD,
                r#"{"method":"post","file":"a.go"}"#,
            ),
            text(cell_type::ROUTE_METHOD, "GET"),
            text(cell_type::ROUTE_METHOD, "POST"),
        ];
        assert_eq!(
            route_method_path("route:/ws", &cells),
            ("POST,GET".to_string(), "/ws".to_string())
        );
        assert_eq!(
            route_method_path("route:/ws", &[]),
            ("ANY".to_string(), "/ws".to_string())
        );
        assert_eq!(
            route_method_path("weird", &[]),
            ("ANY".to_string(), "weird".to_string())
        );
    }

    #[test]
    fn only_openapi_and_feature_yaml_http_ops_declare() {
        let merged = MergedGraph::new(Vec::new());
        let loc = Locator::new(&merged);
        let origin = |s: &str| [json(cell_type::ORIGIN, s)];
        let op = declared_op(
            NodeId(1),
            &origin(
                r#"{"provenance":"contract","source":"feature_yaml","feature":"activities","method":"post","path":"/a"}"#,
            ),
            &loc,
        )
        .expect("a feature_yaml op declares");
        assert_eq!(
            (op.source, op.feature.as_str(), op.method.as_str()),
            ("feature_yaml", "activities", "POST")
        );
        for rejected in [
            r#"{"provenance":"contract","source":"pact","method":"GET","path":"/users"}"#,
            r#"{"provenance":"contract","source":"swaggo","method":"GET","path":"/users"}"#,
            r#"{"provenance":"contract","source":"asyncapi","action":"publish","channel":"orders"}"#,
            r#"{"provenance":"contract","source":"openapi","method":"TRACEX","path":"/x"}"#,
            r#"{"provenance":"nav_route","source":"openapi","method":"GET","path":"/x"}"#,
        ] {
            assert!(
                declared_op(NodeId(2), &origin(rejected), &loc).is_none(),
                "{rejected}"
            );
        }
        // No `feature` and no POSITION file: placed under one named bucket.
        let op = declared_op(
            NodeId(3),
            &origin(r#"{"provenance":"contract","source":"openapi","method":"GET","path":"/x"}"#),
            &loc,
        )
        .expect("an openapi op declares");
        assert_eq!(op.feature, "(unplaced)");
    }

    #[test]
    fn summary_counts_ops_and_undeclared_rows() {
        let empty = SpecStatus::default();
        assert_eq!(
            empty.summary(),
            "features=0 declared=0 implemented=0 declared_missing=0 undeclared=0 ungoverned=0"
        );
    }
}
