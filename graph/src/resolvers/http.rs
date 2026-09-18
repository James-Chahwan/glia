//! HTTP stack resolver — frontend Endpoint → backend Route by
//! (method, normalised path).

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::endpoint::{is_canonical_http_path, split_owner};
use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, NodeId, RepoId};

use super::{CrossGraphResolver, weakest};
use crate::merged::MergedGraph;
use crate::nav::is_nav_route;
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
/// one edge per target, UNLESS the endpoint's recorded host names a service
/// that some of those repos declare: then only their routes are kept (A11.4,
/// `narrow_by_host`, which falls back to every target on any doubt).
pub struct HttpStackResolver;

impl CrossGraphResolver for HttpStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        // Read ONCE per build. Never per lookup: the env read would show up in
        // every match and `normalise_http_path` (which is `pub`, and used by
        // fixtures and tests) must stay a pure function of its argument.
        let prefixes = api_prefixes();
        let mut stats = HttpMatchStats::default();
        let (index, stripped, owners) = build_route_index(&merged.graphs, &prefixes, &mut stats);
        stats.report_nav_excluded();
        // A11.4: built from nodes, never from cross-edges, so where this
        // resolver sits in `run_all_resolvers` does not matter.
        let aliases = build_service_alias_index(&merged.graphs);
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ENDPOINT) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                stats.qnames.endpoint(qname);
                stats.owned_endpoints += usize::from(split_owner(qname).1.is_some());
                let Some((method, raw_path)) = parse_endpoint_qname(qname) else {
                    continue;
                };
                if raw_path == "<unresolved>" {
                    continue;
                }
                stats.endpoints += 1;
                stats.count_folds(raw_path);
                stats.count_client_normalised(&n.cells);
                let norm = normalise_http_path(raw_path);
                let mut hits = lookup_route(&index, &stripped, &method, &norm, &prefixes);
                let hosts = endpoint_hosts(&n.cells);
                if narrow_by_host(&aliases, hosts.as_deref(), &mut hits) {
                    stats.host_narrowed += 1;
                }
                for (target, tier) in hits {
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
        stats.qnames.report();
        stats.report_placeholder_folds();
        stats.report_client_normalised();
        stats.report_host_narrowed(aliases.len());
        stats.report_owners(&owners);
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
    /// Client-router NAV routes kept out of the index (A3.4).
    nav_excluded: usize,
    endpoints: usize,
    paired: usize,
    exact: usize,
    any: usize,
    eprefix: usize,
    rprefix: usize,
    base: usize,
    suffix: usize,
    /// A3.2: path SEGMENTS (route and endpoint side alike) folded to `{}` by a
    /// shape `normalise_segment` did not recognise before A3.2.
    folds_angle: usize,
    folds_splat: usize,
    folds_bracket: usize,
    /// A3.3: ENDPOINT nodes whose path the client parser rewrote, split by
    /// what the recorded `raw` literal held — a scheme+host, and/or a query or
    /// fragment. One node can count in both.
    normalised_host: usize,
    normalised_query: usize,
    /// A11.4: ENDPOINT nodes whose target list host narrowing actually cut.
    host_narrowed: usize,
    /// LB.5: the `[http-qname]` census.
    qnames: QnameCensus,
    /// LB.4a: indexed ROUTE nodes whose qname carries an owner segment
    /// (` @<project path>`), and ENDPOINT nodes likewise.
    owned_routes: usize,
    owned_endpoints: usize,
}

/// LB.5's permanent detector: ROUTE / ENDPOINT qnames whose path part is not
/// in the one canonical form (`code_domain::endpoint::canonical_http_path` —
/// a single leading `/`, or an exempt placeholder). Every emitter builds its
/// qname through that module's builders, so a non-zero count names a parser
/// that bypasses them.
///
/// ROUTEs are counted where the route index sees them, so client-router NAV
/// routes (A3.4) are out of scope exactly as they are out of the index; a
/// qname in neither route shape has no path part to judge and is counted
/// but never flagged.
#[derive(Default)]
struct QnameCensus {
    routes: usize,
    endpoints: usize,
    offenders: Vec<String>,
}

impl QnameCensus {
    /// The path part is read off the owner-free qname (LB.4a), so an owner
    /// segment never sits inside the judged path.
    fn route(&mut self, qname: &str) {
        self.routes += 1;
        let base = split_owner(qname).0;
        let path = base
            .strip_prefix("route:")
            .or_else(|| base.split_once(' ').map(|(_, p)| p));
        self.judge(qname, path);
    }

    fn endpoint(&mut self, qname: &str) {
        self.endpoints += 1;
        self.judge(qname, parse_endpoint_qname(qname).map(|(_, p)| p));
    }

    fn judge(&mut self, qname: &str, path: Option<&str>) {
        if path.is_some_and(|p| !is_canonical_http_path(p)) {
            self.offenders.push(qname.to_string());
        }
    }

    /// LB.5 fired_on marker, silent on a build with no HTTP surface (like
    /// `[http]`). With offenders it names the first three, sorted, so a
    /// regressing parser is identified without a rerun.
    fn report(&mut self) {
        if self.routes + self.endpoints == 0 {
            return;
        }
        eprintln!(
            "[http-qname] routes={} endpoints={} noncanonical={}",
            self.routes,
            self.endpoints,
            self.offenders.len(),
        );
        if !self.offenders.is_empty() {
            self.offenders.sort_unstable();
            let first = &self.offenders[..self.offenders.len().min(3)];
            eprintln!("[http-qname] noncanonical first {}: {first:?}", first.len());
        }
    }
}

impl HttpMatchStats {
    /// Count the A3.2 folds in one raw path, BEFORE normalisation — once the
    /// path is normalised every placeholder reads `{}` and the shape is gone.
    fn count_folds(&mut self, raw_path: &str) {
        let (angle, splat, bracket) = new_fold_kinds(raw_path);
        self.folds_angle += angle;
        self.folds_splat += splat;
        self.folds_bracket += bracket;
    }

    /// Count one ENDPOINT node toward the A3.3 marker. Graph build merges
    /// repeated call sites into one node with stacked ENDPOINT_HIT cells, so a
    /// node counts at most once per bucket however many of its cells carry a
    /// `raw`.
    fn count_client_normalised(&mut self, cells: &[Cell]) {
        let (mut host, mut query) = (false, false);
        for c in cells {
            if c.kind != cell_type::ENDPOINT_HIT {
                continue;
            }
            let CellPayload::Json(json) = &c.payload else {
                continue;
            };
            if let Some(raw) = raw_field(json) {
                host |= raw.contains("://");
                query |= raw.contains(['?', '#']);
            }
        }
        self.normalised_host += usize::from(host);
        self.normalised_query += usize::from(query);
    }

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

    /// A3.4 fired_on marker. Only printed when a build actually saw a nav
    /// route, like the `[proto]` / `[contract]` markers in the engine.
    ///
    /// Printed by the resolver, not by `build_route_index`, so a second index
    /// build (`HttpRouteMatcher`, A10.2) cannot print the line twice.
    fn report_nav_excluded(&self) {
        if self.nav_excluded > 0 {
            eprintln!("[http] nav-routes excluded from route index: {}", self.nav_excluded);
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

    /// A3.2 fired_on marker. Printed only when a build folded at least one
    /// `<…>` / `*name` / `[…]` segment, like the A3.4 line above.
    fn report_placeholder_folds(&self) {
        if self.folds_angle + self.folds_splat + self.folds_bracket == 0 {
            return;
        }
        eprintln!(
            "[http] placeholder folds: angle={} splat={} bracket={}",
            self.folds_angle, self.folds_splat, self.folds_bracket,
        );
    }

    /// A3.3 fired_on marker. Printed only when a client parser actually
    /// rewrote a request path (host and/or query stripped). The durable twin
    /// of this line is the `"raw"` field on the ENDPOINT_HIT cell itself.
    fn report_client_normalised(&self) {
        if self.normalised_host + self.normalised_query == 0 {
            return;
        }
        eprintln!(
            "[http] client paths normalised: host={} query={}",
            self.normalised_host, self.normalised_query,
        );
    }

    /// LB.4a fired_on marker, graph side: what the resolver saw of the owner
    /// segments the engine's owner pass wrote. Printed only when a ROUTE or
    /// ENDPOINT carried one, so a repo without nested project roots is silent.
    /// Cross-checks the engine's `[http-owner] qualified` line.
    fn report_owners(&self, owners: &RouteOwners) {
        if self.owned_routes + self.owned_endpoints == 0 {
            return;
        }
        eprintln!(
            "[http-owner] index routes={} endpoints={} route_owners={}",
            self.owned_routes,
            self.owned_endpoints,
            owners.len(),
        );
    }

    /// A11.4 fired_on marker. Printed only when narrowing removed at least one
    /// pairing, so every build without a known service host stays silent.
    fn report_host_narrowed(&self, aliases: usize) {
        if self.host_narrowed == 0 {
            return;
        }
        eprintln!(
            "[http-host] narrowed {} endpoint pairings by service host ({aliases} aliases)",
            self.host_narrowed,
        );
    }
}

#[derive(Debug, Clone, Copy)]
struct RouteTarget {
    route_id: NodeId,
    confidence: Confidence,
    /// The repo the ROUTE lives in, which is what host narrowing (A11.4) keys
    /// on.
    repo: RepoId,
    /// LB.4a: the ROUTE's owner segment (the nested project root it lives
    /// under) as an index into the build's [`RouteOwners`], so the target
    /// stays `Copy`. `None` for a route outside every nested root.
    owner: Option<u32>,
}

/// LB.4a: the owner segments of every indexed ROUTE, interned, so a
/// [`RouteTarget`] carries a `u32` instead of a string. Built alongside the
/// route index, in index order.
#[derive(Debug, Default)]
struct RouteOwners {
    names: Vec<String>,
    by_name: HashMap<String, u32>,
}

impl RouteOwners {
    /// The index of `owner`, interning it on first sight. `None` only past
    /// `u32::MAX` distinct owners, which a repo cannot reach.
    fn intern(&mut self, owner: &str) -> Option<u32> {
        if let Some(&i) = self.by_name.get(owner) {
            return Some(i);
        }
        let i = u32::try_from(self.names.len()).ok()?;
        self.names.push(owner.to_string());
        self.by_name.insert(owner.to_string(), i);
        Some(i)
    }

    fn len(&self) -> usize {
        self.names.len()
    }
}

/// Build `(METHOD, normalised_path) → Vec<RouteTarget>` across every graph in
/// the merge. One entry per `ROUTE_METHOD` cell found on each Route node.
///
/// A ROUTE's owner segment (LB.4a) is split off first: the method and path
/// are read from the owner-free qname, so pairing ignores owners, and the
/// owner rides on the target through the returned [`RouteOwners`] table.
fn build_route_index(
    graphs: &[RepoGraph],
    prefixes: &[String],
    stats: &mut HttpMatchStats,
) -> (RouteIndex, RouteIndex, RouteOwners) {
    let mut index: RouteIndex = HashMap::new();
    let mut owners = RouteOwners::default();
    // A3.1: the SYMMETRIC half of the prefix strip. Before A3.1 the strip only
    // ever removed prefixes from the client path, so a client calling `/users`
    // against a server mounted at `/api/users` could never pair. Every route is
    // additionally registered under each of its stripped forms here, kept in a
    // second map so the strong `index` stays exactly what it was.
    let mut stripped: RouteIndex = HashMap::new();
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
            // rendered?", and LA.6a's NAVIGATES_TO links bind to it), but it
            // must never be an HTTP_CALLS target.
            //
            // A3.1 depends on this: go_router / react-router / Angular Router
            // all mint their nav entries with the method string "ANY", so the
            // moment the ANY tier below goes live, EVERY navigation entry in an
            // SPA would otherwise become an HTTP_CALLS target.
            if is_nav_route(&n.cells) {
                stats.nav_excluded += 1;
                continue;
            }
            stats.routes += 1;
            stats.qnames.route(qname);
            let (qname, owner) = split_owner(qname);
            let target = RouteTarget {
                route_id: n.id,
                confidence: n.confidence,
                repo: g.repo,
                owner: owner.and_then(|o| owners.intern(o)),
            };
            stats.owned_routes += usize::from(target.owner.is_some());
            if let Some(path) =
                index_route_node(&mut index, &mut stripped, qname, &n.cells, target, prefixes)
            {
                stats.count_folds(path);
            }
        }
    }
    (index, stripped, owners)
}

/// The resolver's ROUTE index and match ladder, reusable by a pass that pairs
/// something OTHER than a client `ENDPOINT` with the ROUTE that serves it.
/// First consumer: the engine's contract linker (A10.2), which pairs OpenAPI
/// operations with their implementing routes.
///
/// Same index (both route qname shapes, NAV routes excluded, the symmetric
/// API-prefix strip, `GLIA_API_PREFIXES` read once here) and the same tiers 1-4
/// in the same order. Tiers 5-6 are deliberately NOT offered: they infer a base
/// URL from a client-side `${…}` interpolation. A caller holding a DECLARED path
/// has no interpolation to infer from, and a leading `{param}` in a declared
/// path is a real path parameter that must not be folded away.
///
/// Building one is silent — the `[http]` markers stay the resolver's.
pub struct HttpRouteMatcher {
    index: RouteIndex,
    stripped: RouteIndex,
    prefixes: Vec<String>,
}

/// One ROUTE an [`HttpRouteMatcher`] lookup reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RouteMatch {
    pub route: NodeId,
    /// The ROUTE node's own confidence, capped at the matching tier's ceiling.
    pub confidence: Confidence,
    /// `(METHOD, path)` hit the index outright: no prefix strip on either side
    /// and no method-agnostic `ANY` fallback.
    pub exact: bool,
}

impl HttpRouteMatcher {
    pub fn new(graphs: &[RepoGraph]) -> Self {
        let prefixes = api_prefixes();
        let mut scratch = HttpMatchStats::default();
        let (index, stripped, _owners) = build_route_index(graphs, &prefixes, &mut scratch);
        Self { index, stripped, prefixes }
    }

    /// No server route anywhere in the build. (`stripped` is only ever filled
    /// alongside `index`, so this one check covers both.)
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Routes serving `method path`, from the FIRST tier that yields anything —
    /// never a union across tiers, so every hit shares one `exact` value.
    /// `path` is raw; it is normalised here with [`normalise_http_path`].
    pub fn lookup(&self, method: &str, path: &str) -> Vec<RouteMatch> {
        let norm = normalise_http_path(path);
        lookup_direct(&self.index, &self.stripped, method, &norm, &self.prefixes)
            .unwrap_or_default()
            .into_iter()
            .map(|(t, tier)| RouteMatch {
                route: t.route_id,
                confidence: weakest(t.confidence, tier.ceiling()),
                exact: tier == MatchTier::Exact,
            })
            .collect()
    }
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
///
/// Returns the raw path the qname carried (once per node, however many
/// methods it stacks), or `None` for a qname neither shape describes — the
/// A3.2 fold counter reads it. `qname` is owner-free: [`build_route_index`]
/// splits the LB.4a owner segment off before it calls here.
fn index_route_node<'q>(
    index: &mut RouteIndex,
    stripped: &mut RouteIndex,
    qname: &'q str,
    cells: &[Cell],
    target: RouteTarget,
    prefixes: &[String],
) -> Option<&'q str> {
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
        return Some(path);
    }
    // Legacy shape: "<METHOD> <path>". Split on the first space. Every
    // emitter now builds a canonical path (LB.5, counted by the `[http-qname]`
    // census); the `/` guard stays as the safety net for one that does not.
    if let Some((method, path)) = qname.split_once(' ')
        && path.starts_with('/')
    {
        let norm = normalise_http_path(path);
        push_route(index, stripped, method, &norm, target, prefixes);
        return Some(path);
    }
    None
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

/// `endpoint:<METHOD>:<path>[ @<owner>]` -> `(METHOD upper-cased, path)`. The
/// LB.4a owner segment is split off first, so the path never carries it.
pub(crate) fn parse_endpoint_qname(qname: &str) -> Option<(String, &str)> {
    let rest = split_owner(qname).0.strip_prefix("endpoint:")?;
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

/// The still-escaped `raw` value of an ENDPOINT_HIT payload (A3.3), or None.
fn raw_field(json: &str) -> Option<&str> {
    str_field(json, "raw")
}

/// The still-escaped value of the string field `key` in an ENDPOINT_HIT
/// payload, or None.
///
/// All three writers (`code_domain::endpoint`, the TS parser's serde_json and
/// the engine's endpoint fold) emit compact JSON, so the key is matched WITH
/// its `:"`. Inside any escaped value every `"` is preceded by `\`, so a path
/// that is literally `raw` (`"path":"raw"`) cannot be mistaken for the key, and
/// `"host":"` cannot match inside `"hosts":[`.
fn str_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":\"");
    let rest = &json[json.find(&pat)? + pat.len()..];
    quoted_body(rest)
}

/// `rest` starts just after an opening quote: everything up to the first
/// unescaped quote, or None when it is unterminated.
fn quoted_body(rest: &str) -> Option<&str> {
    let mut escaped = false;
    for (i, b) in rest.bytes().enumerate() {
        match b {
            b'\\' if !escaped => escaped = true,
            b'"' if !escaped => return Some(&rest[..i]),
            _ => escaped = false,
        }
    }
    None
}

// ============================================================================
// A11.4 — host narrowing
// ============================================================================

/// `normalise_alias(name) -> every repo that declares a service by that name`.
type AliasIndex = HashMap<String, HashSet<RepoId>>;

/// The INFRA_RESOURCE kinds that NAME a service (`infra:<kind>:<name>`, see
/// `parsers/code/extractors/src/iac.rs`). ConfigMaps, secrets, jobs and
/// ingresses are not addressable as an HTTP host, so they stay out.
const ALIAS_KINDS: &[&str] = &["service", "deployment", "statefulset", "image"];

/// What `dockerfile_image_name` returns for a Dockerfile at a repo's root. It
/// names nothing, and every such repo would share it.
const DEGENERATE_IMAGE: &str = "image";

/// Name endings that do not tell two services apart: `users-service`,
/// `users-svc` and `users-api` are the same service. `_` is folded to `-`
/// before these are tried, so `users_service` is covered too.
const SERVICE_SUFFIXES: &[&str] = &["-service", "-svc", "-api", "-server"];

/// Cluster-internal DNS tails. Only a name ending in one of these is cut back
/// to its first label (`users.default.svc.cluster.local` -> `users`). A dotted
/// PUBLIC hostname is left whole: cutting `api.example.com` to `api` would
/// alias it onto any compose service that happens to be called `api`.
/// (`.svc.cluster.local` ends in `.local`.)
const CLUSTER_DNS_TAILS: &[&str] = &[".svc", ".local"];

/// Every service alias in the merge, keyed by [`normalise_alias`]. A name
/// declared in several repos maps to all of them. That is a real ambiguity,
/// and narrowing then keeps the targets in every one of them.
///
/// The IacResolver builds its index the same way, and this one stays separate
/// on purpose: that one pairs verbatim qnames, this one keys on a normalised
/// NAME and only for the service-naming kinds.
fn build_service_alias_index(graphs: &[RepoGraph]) -> AliasIndex {
    let mut index = AliasIndex::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::INFRA_RESOURCE) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            let Some((kind, name)) = qname
                .strip_prefix("infra:")
                .and_then(|rest| rest.split_once(':'))
            else {
                continue;
            };
            if !ALIAS_KINDS.contains(&kind) || (kind == "image" && name == DEGENERATE_IMAGE) {
                continue;
            }
            let alias = normalise_alias(name);
            if !alias.is_empty() {
                index.entry(alias).or_default().insert(g.repo);
            }
        }
    }
    index
}

/// One spelling for a service name, applied identically to the alias and to
/// the host so `users-service` (compose) and `users-svc` (a client's base URL)
/// meet at `users`:
/// lowercase, `_` -> `-`, a cluster DNS name cut to its first label, then
/// [`SERVICE_SUFFIXES`] stripped for as long as one matches and leaves a
/// non-empty stem (so `users-api-service` and `users-api` also meet).
fn normalise_alias(name: &str) -> String {
    let mut s = name.trim().to_ascii_lowercase().replace('_', "-");
    if CLUSTER_DNS_TAILS.iter().any(|t| s.ends_with(t))
        && let Some((first, _)) = s.split_once('.')
    {
        s = first.to_string();
    }
    while let Some(stem) = SERVICE_SUFFIXES
        .iter()
        .find_map(|suf| s.strip_suffix(suf).filter(|stem| !stem.is_empty()))
    {
        s = stem.to_string();
    }
    s
}

/// `host[:port]` -> `host`. A bracketed IPv6 literal keeps its brackets and
/// never matches an alias, which is the right answer for it.
fn host_name(authority: &str) -> &str {
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    }
}

const HOSTS_KEY: &str = "\"hosts\":[";

/// The `hosts` array of an ENDPOINT_HIT payload: every authority the client's
/// base URL can take, one per deployment binding, `""` for a binding with
/// none. None when the array is malformed.
fn hosts_list(json: &str) -> Option<Vec<&str>> {
    let mut rest = &json[json.find(HOSTS_KEY)? + HOSTS_KEY.len()..];
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.starts_with(']') {
            return Some(out);
        }
        rest = rest.strip_prefix('"')?;
        let value = quoted_body(rest)?;
        out.push(value);
        rest = rest[value.len() + 1..].trim_start();
        rest = rest.strip_prefix(',').unwrap_or(rest);
    }
}

/// Every host an ENDPOINT's calls may go to, or None when any call site gives
/// no evidence.
///
/// Graph build stacks one ENDPOINT_HIT cell per call site on the node, and two
/// call sites on one path can name different services, so the answer is the
/// union over the cells. A cell with a `hosts` array (A11.4's engine side:
/// the base URL is bound differently per deployment) contributes the whole
/// array; otherwise its `host` (A11.2). A cell with neither means one call site
/// goes somewhere unknown, and then nothing may be narrowed.
fn endpoint_hosts(cells: &[Cell]) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut seen_hit = false;
    for c in cells {
        if c.kind != cell_type::ENDPOINT_HIT {
            continue;
        }
        let CellPayload::Json(json) = &c.payload else {
            return None;
        };
        seen_hit = true;
        let hosts = if json.contains(HOSTS_KEY) {
            hosts_list(json)?
        } else {
            vec![str_field(json, "host")?]
        };
        for h in hosts {
            if !out.iter().any(|o| o == h) {
                out.push(h.to_string());
            }
        }
    }
    (seen_hit && !out.is_empty()).then_some(out)
}

/// Drop the targets that live outside the service the client named. Returns
/// true only when it removed at least one.
///
/// It acts only on positive evidence and otherwise leaves `hits` alone:
/// - no host, or any host (deployment) that is not a known alias: the client
///   may call something the map does not know;
/// - no target in an owning repo: the map is incomplete, and losing an edge
///   to it would be worse than keeping a collision.
///
/// With several hosts the owners are their union, so a client whose dev and
/// prod bases name different services keeps both services' routes.
///
/// MONOREPO IS OUT OF SCOPE, deliberately. This keys on the ROUTE's RepoId. Two
/// services in ONE repo serving the same path collapse into a single
/// `route:<path>` node with two HANDLED_BY edges, so there is nothing here to
/// choose between. Closing that case needs an owner segment in ROUTE qnames, a
/// route-identity change for the route workstream. Do not bolt a path-prefix
/// variant onto this index to get it.
fn narrow_by_host(
    aliases: &AliasIndex,
    hosts: Option<&[String]>,
    hits: &mut Vec<(RouteTarget, MatchTier)>,
) -> bool {
    let Some(hosts) = hosts else {
        return false;
    };
    if hits.len() < 2 {
        return false;
    }
    let mut owners: HashSet<RepoId> = HashSet::new();
    for h in hosts {
        let Some(repos) = aliases.get(&normalise_alias(host_name(h))) else {
            return false;
        };
        owners.extend(repos.iter().copied());
    }
    let kept = hits.iter().filter(|(t, _)| owners.contains(&t.repo)).count();
    if kept == 0 || kept == hits.len() {
        return false;
    }
    hits.retain(|(t, _)| owners.contains(&t.repo));
    true
}

/// Collapse path param syntaxes into a stable form so a frontend endpoint's
/// `/users/${id}` matches a backend route's `/users/:id` or `/users/{id}`.
/// Rules:
/// - Leading slash normalised to exactly one.
/// - Trailing slash stripped (except on the root).
/// - A segment that is a parameter in any framework's syntax → `{}` (see
///   `normalise_segment`): `:x`, `{x}`, any segment containing `${`, and since
///   A3.2 `<x>` / `<int:x>`, a named splat `*x`, and `[x]` / `[...x]`.
/// - Empty segments collapse (so `//foo` → `/foo`).
///
/// Route and endpoint paths both pass through here, so a fold applies
/// identically on both sides of a match. It happens at index/lookup time only;
/// no stored qname changes shape.
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

/// Collapse one path segment to `{}` if it is a parameter in ANY framework's
/// syntax. The three pre-A3.2 shapes are tested first and unchanged.
fn normalise_segment(seg: &str) -> String {
    if is_classic_param(seg) || fold_kind(seg).is_some() {
        "{}".to_string()
    } else {
        seg.to_string()
    }
}

/// The pre-A3.2 parameter shapes: `:id` / `:id?` (Express, Rails, gin),
/// `{id}` / `{id:int}` / `{path...}` (Spring, ASP.NET, chi, Go 1.22), and any
/// segment holding a `${…}` template interpolation.
fn is_classic_param(seg: &str) -> bool {
    seg.starts_with(':') || (seg.starts_with('{') && seg.ends_with('}')) || seg.contains("${")
}

/// A parameter shape `normalise_segment` learned in A3.2. Kept apart from the
/// classic shapes so the `[http] placeholder folds` marker counts only the
/// folds A3.2 added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaceholderFold {
    /// `<uid>`, `<int:uid>`, `<path:sub>` — Flask/Werkzeug, Django, Rocket.
    Angle,
    /// `*path` — a NAMED splat (Rails, Express 5).
    Splat,
    /// `[id]`, `[...slug]`, `[[...slug]]` — Next.js, Nuxt, SvelteKit.
    Bracket,
}

/// The sentinel the TS parser writes for a path it could not read. It is an
/// unknown path, not a parameter: folding it would let an unreadable call pair
/// with any single-segment `/{}` route. `resolve` skips it before normalising;
/// this keeps `normalise_http_path` safe for a caller that does not.
const UNRESOLVED_PATH: &str = "<unresolved>";

/// Classify a segment as one of the A3.2 shapes, or `None`.
///
/// Deliberately conservative on `*`: a NAMED splat is a parameter, but a bare
/// `*` or `**` is a catch-all (`app.get('*', h)` is everywhere in Express).
/// Folding it to `{}` would pair every single-segment interpolated client call
/// with that catch-all, so it stays literal and unpairable. The name must be an
/// identifier, so a glob like `*.js` or `***` stays literal too.
///
/// The empty brackets `<>` and `[]` are literals, not parameters.
fn fold_kind(seg: &str) -> Option<PlaceholderFold> {
    if seg.len() > 2 && seg.starts_with('<') && seg.ends_with('>') && seg != UNRESOLVED_PATH {
        return Some(PlaceholderFold::Angle);
    }
    if seg.len() > 2 && seg.starts_with('[') && seg.ends_with(']') {
        return Some(PlaceholderFold::Bracket);
    }
    let name = seg.strip_prefix('*')?;
    let mut chars = name.chars();
    let starts_ident = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if starts_ident && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Some(PlaceholderFold::Splat);
    }
    None
}

/// `(angle, splat, bracket)`: how many segments of `raw` fold to `{}` only
/// because of A3.2. A segment a classic shape already folds is not counted.
fn new_fold_kinds(raw: &str) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    // Same trim and split as `normalise_http_path`, so the counter sees exactly
    // the segments the normaliser folds.
    for seg in raw.trim().split('/').filter(|s| !s.is_empty()) {
        if is_classic_param(seg) {
            continue;
        }
        match fold_kind(seg) {
            Some(PlaceholderFold::Angle) => counts.0 += 1,
            Some(PlaceholderFold::Splat) => counts.1 += 1,
            Some(PlaceholderFold::Bracket) => counts.2 += 1,
            None => {}
        }
    }
    counts
}

/// Default API mount prefixes stripped from either side of a path when
/// matching. `pub(crate)` so the SDD slice-1c work has a stable name to promote
/// later; the index and matcher themselves are already public as
/// `HttpRouteMatcher` (A10.2). Override per build with `GLIA_API_PREFIXES`.
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

        // A3.2 — Flask / Django / Rocket converters.
        assert_eq!(normalise_http_path("/users/<int:uid>"), "/users/{}");
        assert_eq!(normalise_http_path("/files/<path:sub>"), "/files/{}");
        assert_eq!(normalise_http_path("/users/<uid>/posts"), "/users/{}/posts");
        // A3.2 — Rails / Express 5 named splat.
        assert_eq!(normalise_http_path("/files/*path"), "/files/{}");
        assert_eq!(normalise_http_path("/files/*rest_of/edit"), "/files/{}/edit");
        // A3.2 — Next.js / SvelteKit dynamic segments.
        assert_eq!(normalise_http_path("/blog/[slug]"), "/blog/{}");
        assert_eq!(normalise_http_path("/blog/[...slug]"), "/blog/{}");
        assert_eq!(normalise_http_path("/docs/[[...slug]]"), "/docs/{}");
        // Every spelling of one parameter lands on the same key.
        for p in ["/u/:id", "/u/{id}", "/u/${…}", "/u/<int:id>", "/u/*id", "/u/[id]"] {
            assert_eq!(normalise_http_path(p), "/u/{}", "{p}");
        }
    }

    #[test]
    fn normalise_http_path_keeps_catch_alls_and_non_params_literal() {
        // A bare `*` / `**` is a catch-all, not a parameter: folding it would
        // pair every single-segment interpolated call with it.
        assert_eq!(normalise_http_path("/static/*"), "/static/*");
        assert_eq!(normalise_http_path("/a/**"), "/a/**");
        assert_eq!(normalise_http_path("*"), "/*");
        // A splat's name must be an identifier: globs stay literal.
        assert_eq!(normalise_http_path("/assets/*.js"), "/assets/*.js");
        assert_eq!(normalise_http_path("/a/***"), "/a/***");
        // Empty brackets are literals.
        assert_eq!(normalise_http_path("/a/[]"), "/a/[]");
        assert_eq!(normalise_http_path("/a/<>"), "/a/<>");
        // A bracket that does not span the whole segment is not a parameter.
        assert_eq!(normalise_http_path("/a/[id].json"), "/a/[id].json");
        assert_eq!(normalise_http_path("/a/<id>.json"), "/a/<id>.json");
        // The TS parser's unreadable-path sentinel is not a parameter.
        assert_eq!(normalise_http_path("<unresolved>"), "/<unresolved>");
    }

    #[test]
    fn new_fold_kinds_counts_only_the_a32_shapes() {
        assert_eq!(new_fold_kinds("/users/<int:uid>"), (1, 0, 0));
        assert_eq!(new_fold_kinds("/files/*path"), (0, 1, 0));
        assert_eq!(new_fold_kinds("/blog/[...slug]/<id>"), (1, 0, 1));
        // Classic shapes fold, but they are not A3.2's to count.
        assert_eq!(new_fold_kinds("/u/:id/{pid}/${…}"), (0, 0, 0));
        // Neither are the literals A3.2 deliberately leaves alone.
        assert_eq!(new_fold_kinds("/static/*/**/[]/<unresolved>"), (0, 0, 0));
        // The counter trims exactly as the normaliser does.
        assert_eq!(new_fold_kinds("  /users/<uid>  "), (1, 0, 0));
    }

    /// A3.2 end to end through the resolver: a template client pairs with a
    /// Flask converter route, a named splat and a Next.js segment, while a
    /// bare Express catch-all stays unpaired.
    #[test]
    fn resolver_pairs_client_templates_with_folded_route_placeholders() {
        use repo_graph_code_domain::{CodeNav, GRAPH_TYPE};
        use repo_graph_core::Node;

        let r = crate::test_support::repo();
        let mut nav = CodeNav::default();
        let mut nodes = Vec::new();
        let mut add = |kind, qname: &str, cells: Vec<Cell>| {
            let id = NodeId::from_parts(GRAPH_TYPE, r, kind, qname);
            nav.record(id, qname, qname, kind, None);
            nodes.push(Node { id, repo: r, confidence: Confidence::Strong, cells });
            id
        };
        let get = || vec![Cell { kind: cell_type::ROUTE_METHOD, payload: CellPayload::Text("GET".into()) }];
        let flask = add(node_kind::ROUTE, "GET /users/<int:uid>", get());
        let rails = add(node_kind::ROUTE, "GET /files/*path", get());
        let next = add(node_kind::ROUTE, "GET /blog/[slug]", get());
        let catch_all = add(node_kind::ROUTE, "GET /static/*", get());
        let user_call = add(node_kind::ENDPOINT, "endpoint:GET:/users/${…}", vec![]);
        let file_call = add(node_kind::ENDPOINT, "endpoint:GET:/files/${…}", vec![]);
        let blog_call = add(node_kind::ENDPOINT, "endpoint:GET:/blog/${…}", vec![]);
        let static_call = add(node_kind::ENDPOINT, "endpoint:GET:/static/${…}", vec![]);
        let g = RepoGraph {
            repo: r,
            nodes,
            edges: vec![],
            symbols: Default::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let mut merged = MergedGraph::new(vec![g]);
        HttpStackResolver.resolve(&mut merged);

        let pairs: Vec<(NodeId, NodeId)> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::HTTP_CALLS)
            .map(|e| (e.from, e.to))
            .collect();
        let got: std::collections::HashSet<_> = pairs.iter().copied().collect();
        let want: std::collections::HashSet<_> =
            [(user_call, flask), (file_call, rails), (blog_call, next)].into_iter().collect();
        // Exactly these three, once each: no cross-pairing between the folded
        // keys, and nothing reaches the bare `*` catch-all.
        assert_eq!(pairs.len(), 3);
        assert_eq!(got, want);
        assert!(!pairs.iter().any(|(from, to)| *from == static_call || *to == catch_all));
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
        // LB.4a: the owner segment is split off before the path is read.
        assert_eq!(
            parse_endpoint_qname("endpoint:GET:/health @web"),
            Some(("GET".to_string(), "/health"))
        );
        assert_eq!(
            parse_endpoint_qname("endpoint:get:${…}/users @apps/@shop/web"),
            Some(("GET".to_string(), "${…}/users"))
        );
        assert_eq!(parse_endpoint_qname("route:/users @api"), None);
    }

    /// LB.4a: two services of one repo serving `/health` are two ROUTE ids
    /// (each qualified with its project). Pairing reads the owner-free path,
    /// so an owner-qualified client reaches both (narrowing by project is
    /// LB.4b), in both route shapes, and the owners ride on the targets.
    #[test]
    fn owner_segments_are_ignored_by_pairing_and_carried_on_targets() {
        let repo = RepoId(77);
        let (g, ids) = repo_graph(
            repo,
            vec![
                (node_kind::ENDPOINT, "endpoint:GET:/health @web", vec![hit(r#"{"method":"GET"}"#)]),
                (node_kind::ROUTE, "GET /health @services/admin", vec![text_get()]),
                (node_kind::ROUTE, "route:/health @services/users", get_route()),
            ],
        );
        let mut stats = HttpMatchStats::default();
        let (index, _, owners) = build_route_index(std::slice::from_ref(&g), &[], &mut stats);
        let targets = index
            .get(&("GET".to_string(), "/health".to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut got: Vec<(u64, Option<&str>)> = targets
            .iter()
            .map(|t| (t.route_id.0, t.owner.and_then(|i| owners.names.get(i as usize)).map(String::as_str)))
            .collect();
        got.sort();
        let mut want = vec![(ids[1].0, Some("services/admin")), (ids[2].0, Some("services/users"))];
        want.sort();
        assert_eq!(got, want);
        assert_eq!((stats.owned_routes, owners.len()), (2, 2));
        assert!(stats.qnames.offenders.is_empty(), "the owner is not judged as path");

        let mut merged = MergedGraph::new(vec![g]);
        HttpStackResolver.resolve(&mut merged);
        let mut to: Vec<u64> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::HTTP_CALLS && e.from == ids[0])
            .map(|e| e.to.0)
            .collect();
        to.sort_unstable();
        let mut both = vec![ids[1].0, ids[2].0];
        both.sort_unstable();
        assert_eq!(to, both);
    }

    fn text_get() -> Cell {
        Cell { kind: cell_type::ROUTE_METHOD, payload: CellPayload::Text("GET".into()) }
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

    /// A10.2 — one RepoGraph holding both ROUTE qname shapes, a NAV route and
    /// an `ANY` route, for the public matcher.
    fn matcher_graph() -> (RepoGraph, [NodeId; 4]) {
        use repo_graph_code_domain::{CodeNav, GRAPH_TYPE};
        use repo_graph_core::Node;

        let r = crate::test_support::repo();
        let mut nav = CodeNav::default();
        let mut nodes = Vec::new();
        let mut add = |qname: &str, confidence: Confidence, cells: Vec<Cell>| {
            let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::ROUTE, qname);
            nav.record(id, qname, qname, node_kind::ROUTE, None);
            nodes.push(Node { id, repo: r, confidence, cells });
            id
        };
        let text = |m: &str| Cell { kind: cell_type::ROUTE_METHOD, payload: CellPayload::Text(m.into()) };
        let json = |m: &str| Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Json(format!(r#"{{"method":"{m}"}}"#)),
        };
        let users = add("GET /users", Confidence::Strong, vec![text("GET")]);
        let orders = add("route:/api/orders/:id", Confidence::Strong, vec![json("POST")]);
        let health = add("ANY /health", Confidence::Medium, vec![text("ANY")]);
        let nav_route = add(
            "route:/dashboard",
            Confidence::Strong,
            vec![
                json("GET"),
                Cell {
                    kind: cell_type::ORIGIN,
                    payload: CellPayload::Json(r#"{"provenance":"nav_route"}"#.into()),
                },
            ],
        );
        let g = RepoGraph {
            repo: r,
            nodes,
            edges: vec![],
            symbols: Default::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        (g, [users, orders, health, nav_route])
    }

    #[test]
    fn route_matcher_flags_exact_and_caps_confidence_by_tier() {
        let (g, [users, orders, health, _]) = matcher_graph();
        let m = HttpRouteMatcher::new(std::slice::from_ref(&g));
        assert!(!m.is_empty());
        let hit = |route, confidence, exact| vec![RouteMatch { route, confidence, exact }];

        // Tier 1, legacy `<METHOD> <path>` shape. Method is case-folded.
        assert_eq!(m.lookup("get", "/users"), hit(users, Confidence::Strong, true));
        // Tier 1, `route:` shape; any param spelling normalises the same way.
        assert_eq!(m.lookup("POST", "/api/orders/{id}"), hit(orders, Confidence::Strong, true));
        // Tier 2: an OpenAPI `servers: /api/v1` base is a caller-side prefix.
        assert_eq!(m.lookup("GET", "/api/v1/users"), hit(users, Confidence::Strong, false));
        // Tier 4: route mounted under /api, caller declares the bare path.
        assert_eq!(m.lookup("POST", "/orders/{oid}"), hit(orders, Confidence::Medium, false));
        // Tier 3: an ANY route serves every verb, but is not an exact hit, and
        // the route's own Medium confidence is kept.
        assert_eq!(m.lookup("DELETE", "/health"), hit(health, Confidence::Medium, false));
    }

    #[test]
    fn route_matcher_misses_other_verbs_nav_routes_and_base_folds() {
        let (g, _) = matcher_graph();
        let m = HttpRouteMatcher::new(std::slice::from_ref(&g));
        // The method is part of the key.
        assert!(m.lookup("POST", "/users").is_empty());
        // A3.4: a NAV route is never a server route, for this caller either.
        assert!(m.lookup("GET", "/dashboard").is_empty());
        // Tiers 5-6 are client-only: a declared leading `{tenant}` is a real
        // path parameter, so `/{tenant}/users` must not fold onto `/users`.
        assert!(m.lookup("GET", "/{tenant}/users").is_empty());
        // An empty build is empty.
        assert!(HttpRouteMatcher::new(&[]).is_empty());
    }

    #[test]
    fn extract_method_field_handles_ordering_and_whitespace() {
        let json = r#"{"method":"POST","handler":"h","file":"x.go","line":1,"col":2}"#;
        assert_eq!(extract_method_field(json), Some("POST"));
        let spaced = r#"{ "method" : "GET" , "line" : 0 }"#;
        assert_eq!(extract_method_field(spaced), Some("GET"));
    }

    /// A3.3 — `raw` is read only as a KEY, and to the first unescaped quote.
    #[test]
    fn raw_field_matches_the_key_not_a_value() {
        let with = r#"{"method":"GET","path":"/users","confidence":"strong","raw":"https://a/users?x=1"}"#;
        assert_eq!(raw_field(with), Some("https://a/users?x=1"));
        let without = r#"{"method":"GET","path":"/users","file":"a.ts","line":1,"col":1,"confidence":"strong"}"#;
        assert_eq!(raw_field(without), None);
        // A path that is literally `raw` is a value, not the key.
        let value = r#"{"method":"GET","path":"raw","file":"a.ts"}"#;
        assert_eq!(raw_field(value), None);
        // An escaped quote inside the value does not end it.
        let esc = r##"{"path":"/q","raw":"/q?s=\"x\"#f"}"##;
        assert_eq!(raw_field(esc), Some(r##"/q?s=\"x\"#f"##));
        // Unterminated → None, never a panic.
        assert_eq!(raw_field(r#"{"raw":"abc"#), None);
    }

    // ---- A11.4 host narrowing ------------------------------------------

    /// One repo holding `(kind, qname, cells)` nodes, and their ids in order.
    fn repo_graph(
        repo: RepoId,
        specs: Vec<(repo_graph_core::NodeKindId, &str, Vec<Cell>)>,
    ) -> (RepoGraph, Vec<NodeId>) {
        use repo_graph_code_domain::{CodeNav, GRAPH_TYPE};
        use repo_graph_core::Node;

        let mut nav = CodeNav::default();
        let mut nodes = Vec::new();
        let mut ids = Vec::new();
        for (kind, qname, cells) in specs {
            let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, qname);
            nav.record(id, qname, qname, kind, None);
            nodes.push(Node { id, repo, confidence: Confidence::Strong, cells });
            ids.push(id);
        }
        let g = RepoGraph {
            repo,
            nodes,
            edges: vec![],
            symbols: Default::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        (g, ids)
    }

    fn get_route() -> Vec<Cell> {
        vec![Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Json(r#"{"method":"GET","handler":"h"}"#.into()),
        }]
    }

    fn hit(json: &str) -> Cell {
        Cell { kind: cell_type::ENDPOINT_HIT, payload: CellPayload::Json(json.into()) }
    }

    /// web (one ENDPOINT carrying `hits`), users (`route:/users` + the
    /// `infra:service:users-service` alias) and orders (`route:/users`, no
    /// alias). Returns the HTTP_CALLS targets and the two route ids.
    fn pair_web_users_orders(hits: Vec<Cell>, orders_infra: &[&str]) -> (Vec<NodeId>, NodeId, NodeId) {
        let (web, _) = repo_graph(
            RepoId(401),
            vec![(node_kind::ENDPOINT, "endpoint:GET:/users", hits)],
        );
        let (users, u) = repo_graph(
            RepoId(402),
            vec![
                (node_kind::ROUTE, "route:/users", get_route()),
                (node_kind::INFRA_RESOURCE, "infra:service:users-service", vec![]),
            ],
        );
        let mut orders_specs = vec![(node_kind::ROUTE, "route:/users", get_route())];
        orders_specs.extend(orders_infra.iter().map(|q| (node_kind::INFRA_RESOURCE, *q, vec![])));
        let (orders, o) = repo_graph(RepoId(403), orders_specs);
        let mut merged = MergedGraph::new(vec![web, users, orders]);
        HttpStackResolver.resolve(&mut merged);
        let mut to: Vec<NodeId> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::HTTP_CALLS)
            .map(|e| e.to)
            .collect();
        to.sort_by_key(|id| id.0);
        (to, u[0], o[0])
    }

    #[test]
    fn host_naming_a_service_keeps_only_that_services_route() {
        let (to, users, orders) = pair_web_users_orders(
            vec![hit(r#"{"method":"GET","path":"/users","host":"users-service:8080"}"#)],
            &[],
        );
        assert_ne!(users, orders, "RepoId is part of the route id");
        assert_eq!(to, vec![users]);
    }

    #[test]
    fn no_host_keeps_every_colliding_route() {
        let (to, users, orders) =
            pair_web_users_orders(vec![hit(r#"{"method":"GET","path":"/users"}"#)], &[]);
        let mut want = vec![users, orders];
        want.sort_by_key(|id| id.0);
        assert_eq!(to, want, "today's behaviour, bit for bit");
    }

    /// Every way the evidence can fall short leaves both edges in place.
    #[test]
    fn narrowing_falls_back_to_all_targets_without_positive_evidence() {
        let both = |hits: Vec<Cell>, orders_infra: &[&str]| {
            let (to, _, _) = pair_web_users_orders(hits, orders_infra);
            to.len()
        };
        // A host no repo declares.
        assert_eq!(both(vec![hit(r#"{"host":"billing:9000"}"#)], &[]), 2);
        // A public dotted host is never cut down to its first label.
        assert_eq!(both(vec![hit(r#"{"host":"users.example.com"}"#)], &[]), 2);
        // The alias is declared by both repos: a genuine ambiguity.
        assert_eq!(both(vec![hit(r#"{"host":"users-service"}"#)], &["infra:image:users"]), 2);
        // One call site names users, a second names nothing.
        assert_eq!(
            both(vec![hit(r#"{"host":"users-service"}"#), hit(r#"{"path":"/users"}"#)], &[]),
            2
        );
        // One deployment names users, another has a relative base.
        assert_eq!(both(vec![hit(r#"{"host":"users-service","hosts":["users-service",""]}"#)], &[]), 2);
        // A non-JSON hit is no evidence.
        assert_eq!(
            both(
                vec![Cell { kind: cell_type::ENDPOINT_HIT, payload: CellPayload::Text("users-service".into()) }],
                &[]
            ),
            2
        );
    }

    /// Owners are the union over hosts: two call sites (or two deployments)
    /// naming the two services keep both, one naming users twice keeps one.
    #[test]
    fn several_hosts_narrow_to_the_union_of_their_owners() {
        let (to, _, _) = pair_web_users_orders(
            vec![hit(r#"{"host":"users-service"}"#), hit(r#"{"host":"orders"}"#)],
            &["infra:deployment:orders-svc"],
        );
        assert_eq!(to.len(), 2);
        let (to, _, _) = pair_web_users_orders(
            vec![hit(r#"{"host":"users-svc","hosts":["users-svc","users.default.svc.cluster.local:80"]}"#)],
            &["infra:deployment:orders-svc"],
        );
        assert_eq!(to.len(), 1);
    }

    /// Straight at the function: an alias whose repo holds none of the
    /// targets, and a single target, both leave `hits` untouched.
    #[test]
    fn narrow_by_host_never_empties_or_touches_a_single_hit() {
        let t = |n: u64, repo: u64| {
            (
                RouteTarget {
                    route_id: NodeId(n),
                    confidence: Confidence::Strong,
                    repo: RepoId(repo),
                    owner: None,
                },
                MatchTier::Exact,
            )
        };
        let aliases: AliasIndex = [
            ("payments".to_string(), HashSet::from([RepoId(9)])),
            ("users".to_string(), HashSet::from([RepoId(1)])),
        ]
        .into_iter()
        .collect();
        let host = |h: &str| vec![h.to_string()];

        let mut hits = vec![t(1, 1), t(2, 2)];
        assert!(!narrow_by_host(&aliases, Some(&host("payments:80")), &mut hits));
        assert_eq!(hits.len(), 2);

        let mut hits = vec![t(2, 2)];
        assert!(!narrow_by_host(&aliases, Some(&host("users")), &mut hits));
        assert_eq!(hits.len(), 1, "a lone hit in another repo is kept");

        let mut hits = vec![t(1, 1), t(2, 2), t(3, 1)];
        assert!(narrow_by_host(&aliases, Some(&host("users-svc:80")), &mut hits));
        let kept: Vec<u64> = hits.iter().map(|(x, _)| x.route_id.0).collect();
        assert_eq!(kept, vec![1, 3], "order is kept");
        assert!(hits.iter().all(|(_, tier)| *tier == MatchTier::Exact));
    }

    /// The degenerate root-Dockerfile image name aliases nothing.
    #[test]
    fn root_dockerfile_image_is_not_an_alias() {
        let (g, _) = repo_graph(
            RepoId(1),
            vec![
                (node_kind::INFRA_RESOURCE, "infra:image:image", vec![]),
                (node_kind::INFRA_RESOURCE, "infra:configmap:users", vec![]),
                (node_kind::INFRA_RESOURCE, "infra:statefulset:Users_DB", vec![]),
            ],
        );
        let idx = build_service_alias_index(std::slice::from_ref(&g));
        assert_eq!(idx.keys().collect::<Vec<_>>(), vec!["users-db"]);
    }

    #[test]
    fn normalise_alias_meets_compose_k8s_and_client_spellings() {
        for s in [
            "users",
            "Users",
            "users-service",
            "users_service",
            "users-svc",
            "users_svc",
            "users-api",
            "users-server",
            "users-api-service",
            "users-service.default.svc.cluster.local",
            "users.default.svc",
            "users.local",
        ] {
            assert_eq!(normalise_alias(s), "users", "{s}");
        }
        // A public hostname stays whole, so it cannot alias onto `api`.
        assert_eq!(normalise_alias("api.example.com"), "api.example.com");
        assert_eq!(normalise_alias("api"), "api");
        // A bare suffix is a name, not an empty stem.
        assert_eq!(normalise_alias("service"), "service");
        assert_eq!(normalise_alias("-svc"), "-svc");
        assert_eq!(normalise_alias(""), "");
    }

    #[test]
    fn host_name_drops_port_and_userinfo_only() {
        assert_eq!(host_name("users-service:8080"), "users-service");
        assert_eq!(host_name("users-service"), "users-service");
        assert_eq!(host_name("u:p@users:80"), "users");
        assert_eq!(host_name("[::1]:8080"), "[::1]");
        assert_eq!(host_name("[::1]"), "[::1]");
        assert_eq!(host_name(""), "");
    }

    #[test]
    fn endpoint_hit_host_fields_are_read_as_keys() {
        let h = |json: &str| endpoint_hosts(&[hit(json)]);
        assert_eq!(h(r#"{"path":"/u","host":"a:1"}"#), Some(vec!["a:1".to_string()]));
        // `hosts` wins and is the whole set; `host` is its first entry.
        assert_eq!(
            h(r#"{"host":"a","hosts":["a", "b" ,"a"]}"#),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(h(r#"{"hosts":[]}"#), None);
        // A value that is literally `host` is not the key.
        assert_eq!(h(r#"{"path":"host","raw":"\"host\":\"x\""}"#), None);
        // Malformed arrays are no evidence, never a panic.
        assert_eq!(h(r#"{"host":"a","hosts":["a""#), None);
        assert_eq!(h(r#"{"host":"a","hosts":[1]}"#), None);
        assert_eq!(endpoint_hosts(&[]), None);
        assert_eq!(str_field(r#"{"host":"a"}"#, "host"), Some("a"));
        assert_eq!(str_field(r#"{"hosts":["a"]}"#, "host"), None);
    }

    /// A3.3 — the marker counts NODES per bucket, not cells: a node with two
    /// stacked query-bearing hits counts once, and a host+query raw counts in
    /// both buckets.
    #[test]
    fn client_normalised_counts_nodes_per_bucket() {
        let hit = |json: &str| Cell {
            kind: cell_type::ENDPOINT_HIT,
            payload: CellPayload::Json(json.to_string()),
        };
        let mut stats = HttpMatchStats::default();
        stats.count_client_normalised(&[
            hit(r#"{"path":"/users","raw":"https://api/users?active=1"}"#),
            hit(r#"{"path":"/users","raw":"https://api/users?page=2"}"#),
        ]);
        stats.count_client_normalised(&[hit(r#"{"path":"/users","raw":"https://api/users"}"#)]);
        stats.count_client_normalised(&[hit(r#"{"path":"/users","raw":"/users#top"}"#)]);
        // No raw, a non-JSON cell, and a raw on the wrong cell kind: ignored.
        stats.count_client_normalised(&[
            hit(r#"{"path":"/users"}"#),
            Cell { kind: cell_type::ENDPOINT_HIT, payload: CellPayload::Text("raw".into()) },
            Cell {
                kind: cell_type::ROUTE_METHOD,
                payload: CellPayload::Json(r#"{"raw":"https://x/y?z"}"#.into()),
            },
        ]);
        assert_eq!((stats.normalised_host, stats.normalised_query), (2, 2));
    }

    /// LB.5 — the `[http-qname]` census judges the path part of all three
    /// qname shapes, exempts the placeholder shapes, and never flags a qname
    /// it cannot split.
    #[test]
    fn qname_census_flags_only_relative_paths() {
        let mut c = QnameCensus::default();
        for q in [
            "GET /users",
            "route:/items",
            "ANY /",
            "GET widgets",
            "route:items",
            "odd",
        ] {
            c.route(q);
        }
        for q in [
            "endpoint:GET:/users",
            "endpoint:GET:${…}/users",
            "endpoint:POST:<unresolved>",
            "endpoint:DELETE:protected/x",
        ] {
            c.endpoint(q);
        }
        assert_eq!((c.routes, c.endpoints), (6, 4));
        let mut got = c.offenders.clone();
        got.sort_unstable();
        assert_eq!(
            got,
            vec!["GET widgets", "endpoint:DELETE:protected/x", "route:items"]
        );

        // LB.4a: the owner segment is stripped before judging, both ways: it
        // never makes a canonical path an offender, and never hides one.
        let mut c = QnameCensus::default();
        c.route("GET /users @services/api");
        c.route("route:/items @web");
        c.route("GET widgets @api");
        c.endpoint("endpoint:GET:/users @web");
        c.endpoint("endpoint:GET:users @web");
        let mut got = c.offenders.clone();
        got.sort_unstable();
        assert_eq!(got, vec!["GET widgets @api", "endpoint:GET:users @web"]);
    }
}
