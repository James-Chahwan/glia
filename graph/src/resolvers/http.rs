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
        // Read ONCE per build. Never per lookup: the env read would show up in
        // every match and `normalise_http_path` (which is `pub`, and used by
        // fixtures and tests) must stay a pure function of its argument.
        let prefixes = api_prefixes();
        let mut stats = HttpMatchStats::default();
        let (index, stripped) = build_route_index(&merged.graphs, &prefixes, &mut stats);
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
                stats.endpoints += 1;
                let norm = normalise_http_path(raw_path);
                for (target, tier) in
                    lookup_route(&index, &stripped, &method, &norm, &prefixes)
                {
                    stats.record(tier);
                    merged.cross_edges.push(Edge {
                        from: n.id,
                        to: target.route_id,
                        category: edge_category::HTTP_CALLS,
                        // Tiers 1-3 reproduce pre-A3.1 confidence exactly
                        // (`weakest(_, Strong)` is the identity); the fuzzy
                        // tiers floor it so a consumer can tell a principled
                        // pairing from a guessed one.
                        confidence: weakest(
                            weakest(n.confidence, target.confidence),
                            tier.ceiling(),
                        ),
                    });
                }
            }
        }
        stats.report();
    }
}

/// `(METHOD, normalised_path) -> Vec<RouteTarget>`.
type RouteIndex = HashMap<(String, String), Vec<RouteTarget>>;

/// The method string the nine method-agnostic route emitters use (Rails
/// `resources`, Spring class-level `@RequestMapping`, Laravel `Route::any`,
/// Go `HandleFunc`, Clojure `ANY`, Akka `path(...)`, ...). It is a wildcard,
/// not a verb, so it must be reachable from a typed client verb.
const ANY: &str = "ANY";

/// How a client endpoint path reached a server route path.
///
/// The ladder is ordered strongest-first and the FIRST tier that yields
/// anything wins — never a union across tiers, which is where fan-out would
/// come from.
///
/// The ORDER IS LOAD-BEARING. `Exact` then `EndpointPrefix` reproduce exactly
/// what `lookup_route_with_prefix_strip` did before A3.1, so no pairing that
/// exists today can be silently retargeted: `GET /api/users` still strips to
/// `/users` and binds a typed `GET /users` route rather than an `ANY /users`
/// one. Every tier below `EndpointPrefix` fires only when the tiers above it
/// found nothing, which is what makes A3.1 purely additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchTier {
    /// `(METHOD, path)` hit the index outright.
    Exact,
    /// Up to two leading API prefixes stripped from the CLIENT path.
    EndpointPrefix,
    /// The server declared the route method-agnostic (`ANY`).
    Any,
    /// Up to two leading API prefixes stripped from the SERVER path — the
    /// symmetric half: a client calling `/users` against a server mounted at
    /// `/api/users`.
    RoutePrefix,
    /// The client path began with an interpolated segment, i.e. a base URL
    /// (`` `${environment.apiUrl}/users` `` -> `/{}/users`), which was folded
    /// away before matching.
    BaseFold,
    /// Base-URL path whose tail matched a shorter route suffix. Double-gated
    /// and the weakest thing this resolver will emit.
    Suffix,
}

impl MatchTier {
    /// Confidence ceiling for the tier.
    fn ceiling(self) -> Confidence {
        match self {
            MatchTier::Exact | MatchTier::EndpointPrefix | MatchTier::Any => Confidence::Strong,
            MatchTier::RoutePrefix | MatchTier::BaseFold => Confidence::Medium,
            MatchTier::Suffix => Confidence::Weak,
        }
    }
}

/// Fired-on counters for the `[http]` marker. `paired` counts EDGES emitted,
/// not endpoints matched — one endpoint can hit several colliding routes.
#[derive(Default)]
struct HttpMatchStats {
    routes: usize,
    endpoints: usize,
    paired: usize,
    exact: usize,
    any: usize,
    eprefix: usize,
    rprefix: usize,
    base: usize,
    suffix: usize,
}

impl HttpMatchStats {
    fn record(&mut self, tier: MatchTier) {
        self.paired += 1;
        match tier {
            MatchTier::Exact => self.exact += 1,
            MatchTier::EndpointPrefix => self.eprefix += 1,
            MatchTier::Any => self.any += 1,
            MatchTier::RoutePrefix => self.rprefix += 1,
            MatchTier::BaseFold => self.base += 1,
            MatchTier::Suffix => self.suffix += 1,
        }
    }

    /// A3.1 fired_on marker. Silent on a build with no HTTP surface at all, so
    /// a non-web repo does not grow a line of zeroes on every generate.
    fn report(&self) {
        if self.routes == 0 && self.endpoints == 0 {
            return;
        }
        eprintln!(
            "[http] routes={} endpoints={} paired={} (exact={} any={} eprefix={} rprefix={} base={} suffix={})",
            self.routes,
            self.endpoints,
            self.paired,
            self.exact,
            self.any,
            self.eprefix,
            self.rprefix,
            self.base,
            self.suffix,
        );
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
    prefixes: &[String],
    stats: &mut HttpMatchStats,
) -> (RouteIndex, RouteIndex) {
    let mut index: RouteIndex = HashMap::new();
    // A3.1: the SYMMETRIC half of the prefix strip. Before A3.1 the strip only
    // ever removed prefixes from the client path, so a client calling `/users`
    // against a server mounted at `/api/users` could never pair. Every route is
    // additionally registered under each of its stripped forms here, kept in a
    // second map so the strong `index` stays exactly what it was.
    let mut stripped: RouteIndex = HashMap::new();
    let mut excluded = 0usize;
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ROUTE) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            // A3.4: a client-router ROUTE is a browser navigation target, not a
            // server endpoint. It stays a node (it answers "where is /dashboard
            // rendered?"), but it must never be an HTTP_CALLS target.
            //
            // A3.1 depends on this: go_router / react-router / Angular Router
            // all mint their nav entries with the method string "ANY", so the
            // moment the ANY tier below goes live, EVERY navigation entry in an
            // SPA would otherwise become an HTTP_CALLS target.
            if is_nav_route(&n.cells) {
                excluded += 1;
                continue;
            }
            stats.routes += 1;
            let target = RouteTarget {
                route_id: n.id,
                confidence: n.confidence,
            };
            index_route_node(&mut index, &mut stripped, qname, &n.cells, target, prefixes);
        }
    }
    // A3.4 fired_on marker. Only printed when a build actually saw one, like
    // the `[proto]` / `[contract]` markers in the engine.
    if excluded > 0 {
        eprintln!("[http] nav-routes excluded from route index: {excluded}");
    }
    (index, stripped)
}

/// A ROUTE node tagged `provenance: nav_route` by a client-router extractor
/// (react-router / Angular Router / vue-router / go_router). It is a browser
/// navigation target, not a server endpoint, so it must never be an
/// `HTTP_CALLS` target.
///
/// Cheap substring test — the payload is written by us (the extractors crate's
/// `nav_route_origin_cell`), not by user JSON, matching `extract_method_field`'s
/// existing tight scan and keeping serde_json out of the graph crate. Both
/// payload spellings are accepted so a future Text-payload emitter still marks.
fn is_nav_route(cells: &[Cell]) -> bool {
    cells.iter().any(|c| {
        c.kind == cell_type::ORIGIN
            && matches!(&c.payload, CellPayload::Json(j) | CellPayload::Text(j)
                        if j.contains("\"provenance\":\"nav_route\""))
    })
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
    index: &mut RouteIndex,
    stripped: &mut RouteIndex,
    qname: &str,
    cells: &[Cell],
    target: RouteTarget,
    prefixes: &[String],
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
            push_route(index, stripped, &method, &norm, target, prefixes);
        }
        return;
    }
    // Legacy shape: "<METHOD> <path>". Split on the first space.
    if let Some((method, path)) = qname.split_once(' ')
        && path.starts_with('/')
    {
        let norm = normalise_http_path(path);
        push_route(index, stripped, method, &norm, target, prefixes);
    }
}

/// Register one (method, path) route into the exact index and, for each API
/// prefix it is mounted behind, into the route-side stripped index.
fn push_route(
    index: &mut RouteIndex,
    stripped: &mut RouteIndex,
    method: &str,
    norm: &str,
    target: RouteTarget,
    prefixes: &[String],
) {
    let method = method.to_ascii_uppercase();
    index
        .entry((method.clone(), norm.to_string()))
        .or_default()
        .push(target);
    // Every depth, not just the deepest: a route at `/api/v1/orders` must be
    // reachable from a client that says `/v1/orders` as well as one that says
    // `/orders`. Bounded at 2 segments, so this adds at most two entries.
    for cand in strip_api_prefixes(norm, prefixes) {
        stripped
            .entry((method.clone(), cand))
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

pub(crate) fn parse_endpoint_qname(qname: &str) -> Option<(String, &str)> {
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

/// Default API mount prefixes stripped from either side of a path when
/// matching. `pub(crate)` so the SDD slice-1c work — which wants
/// `build_route_index` / the matcher promoted to `pub` — has a stable name to
/// promote later. Override per build with `GLIA_API_PREFIXES`.
pub(crate) const API_PREFIXES: &[&str] =
    &["protected", "api", "public", "internal", "v1", "v2", "v3"];

/// The prefix list for one build. `GLIA_API_PREFIXES="api,v1,gateway"` replaces
/// the default outright (env-var config is the established glia idiom —
/// `GLIA_NO_PERSIST`, `GLIA_STORE_VERBOSE`).
///
/// Called ONCE per `resolve`, never per lookup: a per-lookup env read would
/// cost on every match, and it must never move into `normalise_http_path`,
/// which is `pub` and has to stay a pure function of its argument.
fn api_prefixes() -> Vec<String> {
    match std::env::var("GLIA_API_PREFIXES") {
        Ok(s) if !s.trim().is_empty() => s
            .split(',')
            .map(|p| p.trim().trim_matches('/').to_ascii_lowercase())
            .filter(|p| !p.is_empty())
            .collect(),
        _ => API_PREFIXES.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// Every prefix-stripped form of `norm_path`, shallowest first, stopping at the
/// first segment that is not an API prefix. Bounded at two segments and never
/// strips the last segment — the same `1..=2.min(len-1)` bound the pre-A3.1
/// endpoint-side loop used.
fn strip_api_prefixes(norm_path: &str, prefixes: &[String]) -> Vec<String> {
    let segments: Vec<&str> = norm_path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let mut out = Vec::new();
    for strip in 1..=2.min(segments.len().saturating_sub(1)) {
        if !prefixes.iter().any(|p| p == segments[strip - 1]) {
            break;
        }
        out.push(format!("/{}", segments[strip..].join("/")));
    }
    out
}

/// Tier 6 lives behind this one flag. If multi-service fan-out ever shows up in
/// a real corpus, flipping it to `false` is the whole revert.
const SUFFIX_TIER: bool = true;

/// The tier ladder. Returns the targets from the FIRST tier that yields
/// anything, tagged with that tier — never a union across tiers.
fn lookup_route(
    index: &RouteIndex,
    stripped: &RouteIndex,
    method: &str,
    norm_path: &str,
    prefixes: &[String],
) -> Vec<(RouteTarget, MatchTier)> {
    // Tiers 1-4.
    if let Some(hit) = lookup_direct(index, stripped, method, norm_path, prefixes) {
        return hit;
    }

    // Tiers 5-6 are gated on a LEADING `{}` segment. `` `${environment.apiUrl}/users` ``
    // — the commonest Angular/React client shape — normalises to `/{}/users`,
    // and a first segment that came from an interpolation is a base URL, not a
    // resource. An ordinary path can never reach these tiers, so `/users/{}`
    // cannot suffix-match its way onto an unrelated route.
    let Some(folded) = norm_path.strip_prefix("/{}") else {
        return Vec::new();
    };
    let folded = if folded.is_empty() { "/" } else { folded };

    // Tier 5 — base fold: re-run tiers 1-4 against the folded path. The tier
    // REPORTED is BaseFold whatever matched inside, because the fold itself is
    // the inference being made.
    if let Some(hit) = lookup_direct(index, stripped, method, folded, prefixes) {
        return retier(hit, MatchTier::BaseFold);
    }

    // Tier 6 — suffix fallback, double-gated: the leading-`{}` condition above,
    // plus a suffix that retains at least one literal segment, plus an
    // ambiguity bail. Emitting nothing beats fanning out.
    if SUFFIX_TIER {
        for cand in literal_suffixes(folded) {
            if let Some(hit) = lookup_direct(index, stripped, method, &cand, prefixes) {
                if hit.len() > MAX_SUFFIX_TARGETS {
                    return Vec::new();
                }
                return retier(hit, MatchTier::Suffix);
            }
        }
    }
    Vec::new()
}

/// A suffix key matching more routes than this is ambiguous; tier 6 abandons
/// rather than emitting them all.
const MAX_SUFFIX_TARGETS: usize = 3;

/// Tiers 1-4 against one candidate path, in the mandated order:
/// exact → endpoint-side strip → ANY → route-side strip.
///
/// Exact-then-endpoint-strip is pre-A3.1 behaviour verbatim, so no existing
/// pairing can be retargeted by the tiers below it.
fn lookup_direct(
    index: &RouteIndex,
    stripped: &RouteIndex,
    method: &str,
    norm_path: &str,
    prefixes: &[String],
) -> Option<Vec<(RouteTarget, MatchTier)>> {
    let method = method.to_ascii_uppercase();
    let eps = strip_api_prefixes(norm_path, prefixes);

    let mut probes: Vec<(&RouteIndex, &str, String, MatchTier)> = Vec::new();
    // 1. exact
    probes.push((index, &method, norm_path.to_string(), MatchTier::Exact));
    // 2. endpoint-side strip — client says `/api/users`, server says `/users`
    probes.extend(
        eps.iter()
            .map(|c| (index, method.as_str(), c.clone(), MatchTier::EndpointPrefix)),
    );
    // 3. ANY — the server declared the route method-agnostic
    probes.push((index, ANY, norm_path.to_string(), MatchTier::Any));
    probes.extend(eps.iter().map(|c| (index, ANY, c.clone(), MatchTier::Any)));
    // 4. route-side strip — client says `/users`, server mounts `/api/users`
    for m in [method.as_str(), ANY] {
        probes.push((stripped, m, norm_path.to_string(), MatchTier::RoutePrefix));
        probes.extend(
            eps.iter()
                .map(|c| (stripped, m, c.clone(), MatchTier::RoutePrefix)),
        );
    }

    for (map, m, path, tier) in probes {
        if let Some(targets) = map.get(&(m.to_string(), path)).filter(|t| !t.is_empty()) {
            return Some(targets.iter().map(|t| (*t, tier)).collect());
        }
    }
    None
}

/// Suffixes of `path`, longest first, excluding `path` itself (tier 5 already
/// tried that) and excluding any suffix made only of `{}` wildcards — those
/// would match essentially anything.
fn literal_suffixes(path: &str) -> Vec<String> {
    let segs: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    (1..segs.len())
        .filter(|start| segs[*start..].iter().any(|s| *s != "{}"))
        .map(|start| format!("/{}", segs[start..].join("/")))
        .collect()
}

/// Relabel an inner tier-1..4 hit with the outer tier that reached it.
fn retier(
    hit: Vec<(RouteTarget, MatchTier)>,
    tier: MatchTier,
) -> Vec<(RouteTarget, MatchTier)> {
    hit.into_iter().map(|(t, _)| (t, tier)).collect()
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
    fn strip_api_prefixes_is_bounded_and_configurable() {
        let default: Vec<String> = API_PREFIXES.iter().map(|s| (*s).to_string()).collect();
        // Shallowest first, so `/v1/orders` is reachable as well as `/orders`.
        assert_eq!(
            strip_api_prefixes("/api/v1/orders", &default),
            vec!["/v1/orders".to_string(), "/orders".to_string()]
        );
        // Stops at the first non-prefix segment.
        assert_eq!(strip_api_prefixes("/users/api", &default), Vec::<String>::new());
        // Never strips the last segment — `/api` stays `/api`.
        assert_eq!(strip_api_prefixes("/api", &default), Vec::<String>::new());
        // Bounded at two segments, exactly as the pre-A3.1 loop was.
        assert_eq!(
            strip_api_prefixes("/api/v1/v2/orders", &default),
            vec!["/v1/v2/orders".to_string(), "/v2/orders".to_string()]
        );
        // Configurability: the list is a parameter, so GLIA_API_PREFIXES can
        // replace it without this function reading the environment.
        let custom = vec!["gateway".to_string()];
        assert_eq!(
            strip_api_prefixes("/gateway/orders", &custom),
            vec!["/orders".to_string()]
        );
        assert_eq!(strip_api_prefixes("/api/orders", &custom), Vec::<String>::new());
    }

    #[test]
    fn api_prefixes_defaults_to_the_const_list() {
        // The env override is deliberately NOT exercised here: env is
        // process-global and these tests run on parallel threads.
        // `strip_api_prefixes` above proves the list is a parameter.
        assert!(api_prefixes().contains(&"api".to_string()));
        assert_eq!(api_prefixes().len(), API_PREFIXES.len());
    }

    #[test]
    fn literal_suffixes_skips_self_and_all_wildcard_tails() {
        assert_eq!(
            literal_suffixes("/a/b/users"),
            vec!["/b/users".to_string(), "/users".to_string()]
        );
        // A tail of nothing but `{}` would match essentially any route.
        assert_eq!(literal_suffixes("/users/{}"), Vec::<String>::new());
        assert_eq!(literal_suffixes("/a/{}/users"), vec!["/{}/users".to_string(), "/users".to_string()]);
        // Single segment: no proper suffix at all.
        assert_eq!(literal_suffixes("/users"), Vec::<String>::new());
    }

    #[test]
    fn match_tier_ceilings_keep_pre_a31_tiers_unfloored() {
        // weakest(x, Strong) == x, so tiers 1-3 cannot change any edge that
        // exists today.
        assert_eq!(MatchTier::Exact.ceiling(), Confidence::Strong);
        assert_eq!(MatchTier::EndpointPrefix.ceiling(), Confidence::Strong);
        assert_eq!(MatchTier::Any.ceiling(), Confidence::Strong);
        assert_eq!(MatchTier::RoutePrefix.ceiling(), Confidence::Medium);
        assert_eq!(MatchTier::BaseFold.ceiling(), Confidence::Medium);
        assert_eq!(MatchTier::Suffix.ceiling(), Confidence::Weak);
    }

    #[test]
    fn extract_method_field_handles_ordering_and_whitespace() {
        let json = r#"{"method":"POST","handler":"h","file":"x.go","line":1,"col":2}"#;
        assert_eq!(extract_method_field(json), Some("POST"));
        let spaced = r#"{ "method" : "GET" , "line" : 0 }"#;
        assert_eq!(extract_method_field(spaced), Some("GET"));
    }
}
