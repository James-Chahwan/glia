//! HTTP stack resolver — frontend Endpoint → backend Route by
//! (method, normalised path).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::endpoint::{is_canonical_http_path, split_owner};
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Edge, NodeId, RepoId};

use super::host::{
    AliasIndex, HostScoped, Narrowed, Owners, build_service_alias_index, hit_hosts, host_name,
    narrow_by_host, normalise_alias, raw_field,
};
use super::{CrossGraphResolver, RuleTally, weakest};
use crate::merged::MergedGraph;
use crate::nav::is_nav_route;
use crate::types::RepoGraph;

/// Pairs frontend HTTP Endpoints with backend HTTP Routes by (method,
/// normalised path) and emits `HTTP_CALLS` edges.
///
/// Matching rule:
/// - Endpoint qname `endpoint:<METHOD>:<path>` is the source side. Method comes
///   straight from the qname; path is normalised (see `normalise_http_path`).
/// - Route qname `<METHOD> <path>` — one Route node per (method, path), `ANY`
///   for a method-agnostic registration (every server parser since LB.11b).
///   The legacy per-path `route:<path>` shape, methods on stacked
///   `ROUTE_METHOD` cells, is still read (see `index_route_node`); each of its
///   (path, method) pairs is a distinct target.
/// - Cross-repo is the common case (Angular → Go gin backend), but same-repo
///   matches also link correctly (Next.js route-handlers + fetchers, etc.).
/// - Emitted edge confidence = min(endpoint_node_confidence, Strong) since
///   Routes are always Strong at v0.4.4 — i.e. the endpoint's confidence wins.
///
/// Collisions (multiple Routes with the same method+path) emit one edge per
/// target, UNLESS the endpoint's recorded host names a service: then only the
/// routes of the repos (A11.4) or nested projects (LB.4b) it names are kept
/// (`narrow_by_host`, which falls back to every target on any doubt).
///
/// Every node pairs ONCE, however many graph entries carry it (LB.4b): an
/// ENDPOINT or ROUTE id present in two language graphs of one repo, or a ROUTE
/// with two stacked `ROUTE_METHOD` cells for one verb, yields one edge per
/// (endpoint, route) pair, never one per entry.
///
/// LF.2d: [`HttpStackResolver::resolve_with_mounts`] also indexes every ROUTE
/// at the gateway paths a [`RouteMounts`] names for it. The trait's `resolve`
/// is that call with no mounts.
///
/// CG.4b (CA-13): an ENDPOINT whose every call site the engine marked
/// external (`"external":true` on each ENDPOINT_HIT, CG.4a) and none of whose
/// hosts names a service / project alias of the build (`host_names_an_alias`)
/// is a third-party call: it pairs with nothing and is left out of the
/// `[http] endpoints=` count, like `<unresolved>`. fired_on, when one was:
/// `[http-external] <n> endpoints left unpaired: every call site names a host outside the build (hosts=<k>)`.
/// The engine's `tag_synthetic_provenance` then stamps it ORIGIN `external`.
pub struct HttpStackResolver;

impl CrossGraphResolver for HttpStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        self.resolve_with_mounts(merged, &RouteMounts::default());
    }
}

/// LF.2d: the paths a gateway ALSO serves a ROUTE under, from a repo's
/// `.glia/overlay.toml` `[[route_prefix]]` stanzas. Nothing in a service's
/// source says a gateway serves it under `/orders-svc`, so a client calling
/// `/orders-svc/users` pairs with nothing, or its `${GATEWAY}/users` base-folds
/// onto every service's `/users`. A mount registers the route at
/// `prefix + path` as well, so the gateway path pairs with the mounted service
/// only.
///
/// Per ROUTE, each prefix once, with the confidence of the stanza that
/// declared it (`Origin::confidence`: never Strong). Built by the engine from
/// the build's overlays; empty (the default) is the plain resolver.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RouteMounts {
    /// `NodeId.0` -> `(prefix, confidence)`, first declaration first.
    by_route: BTreeMap<u64, Vec<(String, Confidence)>>,
}

impl RouteMounts {
    /// Mount `route` under `prefix` (a path starting with `/`). A prefix
    /// already mounted for the route keeps its slot and takes the stronger
    /// confidence.
    pub fn add(&mut self, route: NodeId, prefix: &str, conf: Confidence) {
        let slots = self.by_route.entry(route.0).or_default();
        match slots.iter_mut().find(|(p, _)| p == prefix) {
            Some((_, c)) => *c = strongest(*c, conf),
            None => slots.push((prefix.to_string(), conf)),
        }
    }

    /// No route is mounted anywhere.
    pub fn is_empty(&self) -> bool {
        self.by_route.is_empty()
    }

    /// ROUTE nodes with at least one mount.
    pub fn routes(&self) -> usize {
        self.by_route.len()
    }

    /// The `(prefix, confidence)` mounts of `route`, first declaration first.
    pub fn of(&self, route: NodeId) -> &[(String, Confidence)] {
        self.by_route.get(&route.0).map_or(&[], Vec::as_slice)
    }
}

/// [`HttpStackResolver`] bound to a set of [`RouteMounts`], as the
/// [`CrossGraphResolver`] a pass registry runs (LF.2d). Made by
/// [`HttpStackResolver::with_mounts`].
pub struct MountedHttpResolver<'a> {
    mounts: &'a RouteMounts,
}

impl CrossGraphResolver for MountedHttpResolver<'_> {
    fn resolve(&self, merged: &mut MergedGraph) {
        HttpStackResolver.resolve_with_mounts(merged, self.mounts);
    }
}

impl HttpStackResolver {
    /// CG.4b (CA-13): the ENDPOINT ids this resolver leaves out of pairing as
    /// third-party calls (see the type doc), by the one predicate the resolver
    /// applies, so a post-pass can label them without a second copy of it
    /// (the engine's `tag_synthetic_provenance` stamps ORIGIN `external` on
    /// the ones nothing else paired). Only probed: no order is promised.
    ///
    /// The alias index is built with no route owners: owners scope an alias,
    /// they never add or remove one, and only the alias NAMES are read here.
    pub fn third_party_endpoints(graphs: &[RepoGraph]) -> HashSet<NodeId> {
        let (aliases, _) = build_service_alias_index(graphs, &Owners::default());
        collect_endpoints(graphs)
            .into_iter()
            .filter(|ep| {
                parse_endpoint_qname(ep.qname).is_some_and(|(_, path)| path != UNRESOLVED_PATH)
                    && third_party_hosts(&aliases, &ep.cells).is_some()
            })
            .map(|ep| ep.id)
            .collect()
    }

    /// This resolver with `mounts` applied, as a [`CrossGraphResolver`].
    pub fn with_mounts(mounts: &RouteMounts) -> MountedHttpResolver<'_> {
        MountedHttpResolver { mounts }
    }

    /// Pair every ENDPOINT with the ROUTEs serving it, every mounted ROUTE
    /// also indexed at its gateway paths ([`RouteMounts`]). A mounted key sits
    /// in the STRONG index, so an exact gateway path pairs at the Exact tier,
    /// but its target carries the mount's confidence: a mount can never
    /// manufacture a Strong edge from an `llm` stanza.
    ///
    /// Empty `mounts` is exactly the pre-LF.2d resolver: no key, no counter
    /// and no marker changes. Otherwise one more fired_on line, after the
    /// `[http] routes=...` line (which is unchanged):
    ///   `[http] overlay mounts: routes=<r> keys=<k> paired=<p>`
    /// `routes` counts indexed ROUTE nodes that gained a mounted key, `keys`
    /// the mounted `(METHOD, path)` keys, `paired` the edges emitted through
    /// one.
    pub fn resolve_with_mounts(&self, merged: &mut MergedGraph, mounts: &RouteMounts) {
        // Read ONCE per build. Never per lookup: the env read would show up in
        // every match and `normalise_http_path` (which is `pub`, and used by
        // fixtures and tests) must stay a pure function of its argument.
        let prefixes = api_prefixes();
        let mut stats = HttpMatchStats::default();
        let mut rules = RuleTally::new("http", &MatchTier::RULES);
        let (index, stripped, owners) =
            build_route_index(&merged.graphs, &prefixes, mounts, &mut stats);
        stats.report_nav_excluded();
        // A11.4: built from nodes, never from cross-edges, so where this
        // resolver sits in the Resolve stage of CODE_PASSES does not matter.
        let (aliases, project_aliases) = build_service_alias_index(&merged.graphs, &owners);
        let mut edges = Vec::new();
        for ep in collect_endpoints(&merged.graphs) {
            stats.qnames.endpoint(ep.qname);
            stats.owned_endpoints += usize::from(split_owner(ep.qname).1.is_some());
            let Some((method, raw_path)) = parse_endpoint_qname(ep.qname) else {
                continue;
            };
            if raw_path == UNRESOLVED_PATH {
                continue;
            }
            // CA-13: a third-party call pairs with nothing, even when the app
            // serves the same path. Skipped before it is counted, like
            // `<unresolved>`.
            if let Some(external) = third_party_hosts(&aliases, &ep.cells) {
                stats.external += 1;
                stats.external_hosts.extend(external);
                continue;
            }
            stats.endpoints += 1;
            stats.count_folds(raw_path);
            stats.count_client_normalised(ep.cells.iter().copied());
            let norm = normalise_http_path(raw_path);
            let mut hits = lookup_route(&index, &stripped, &method, &norm, &prefixes);
            let hosts = hit_hosts(ep.cells.iter().copied());
            match narrow_by_host(&aliases, hosts.as_deref(), &mut hits) {
                Narrowed::No => {}
                Narrowed::Repo => stats.host_narrowed += 1,
                Narrowed::Owner => {
                    stats.host_narrowed += 1;
                    stats.owner_narrowed += 1;
                }
            }
            for (target, tier) in hits {
                stats.record(tier);
                stats.mount_hits += usize::from(target.mounted);
                // Tiers 1-3 reproduce pre-A3.1 confidence exactly
                // (`weakest(_, Strong)` is the identity); the fuzzy tiers
                // floor it so a consumer can tell a principled pairing from a
                // guessed one. LC.3c: the edge's evidence names the tier.
                let confidence =
                    weakest(weakest(ep.confidence, target.confidence), tier.ceiling());
                edges.push(
                    Edge::new(ep.id, target.route_id, edge_category::HTTP_CALLS, confidence)
                        .with_cell(rules.cell(tier.rule())),
                );
            }
        }
        merged.cross_edges.extend(edges);
        rules.report();
        stats.report();
        stats.qnames.report();
        stats.report_placeholder_folds();
        stats.report_client_normalised();
        stats.report_mounts(mounts);
        stats.report_host_narrowed(aliases.len());
        stats.report_external();
        stats.report_owner_narrowed(project_aliases);
        stats.report_owners(&owners);
    }
}

/// CA-13 (CG.4b): every call site of a client ENDPOINT is a third-party
/// call: at least one ENDPOINT_HIT, and every ENDPOINT_HIT payload carries the
/// `"external":true` mark the engine's endpoint fold writes (CG.4a: a host
/// written in the call's own literal, public, and named by no URL constant of
/// the client's repo). One unmarked site, or a non-JSON hit, and it is not.
fn all_sites_external<'c>(cells: impl IntoIterator<Item = &'c Cell>) -> bool {
    let mut seen = false;
    for c in cells {
        if c.kind != cell_type::ENDPOINT_HIT {
            continue;
        }
        match &c.payload {
            CellPayload::Json(json) if json.contains(EXTERNAL_MARK) => seen = true,
            _ => return false,
        }
    }
    seen
}

/// CA-13: the hosts of a third-party ENDPOINT, or None when it is not one.
/// It is one when [`all_sites_external`] holds for its call sites and none of
/// their hosts names a service / project of the build
/// ([`host_names_an_alias`]).
fn third_party_hosts(aliases: &AliasIndex, cells: &[&Cell]) -> Option<Vec<String>> {
    if !all_sites_external(cells.iter().copied()) {
        return None;
    }
    let hosts = hit_hosts(cells.iter().copied()).unwrap_or_default();
    (!hosts.iter().any(|h| host_names_an_alias(aliases, h))).then_some(hosts)
}

/// The key CG.4a appends to an external site's ENDPOINT_HIT payload.
const EXTERNAL_MARK: &str = "\"external\":true";

/// Host labels that name no service: a public host's `api.` / `www.` says
/// nothing about WHOSE api it is, so they are never compared with the alias
/// keys (quokka's `app` project would otherwise claim `app.example.io`).
const GENERIC_HOST_LABELS: &[&str] = &[
    "api", "www", "app", "web", "cdn", "static", "gateway", "backend", "server", "service",
];

/// CA-13: does a public `host` name a service or project of this build? The
/// whole host is looked up as [`narrow_by_host`] does, then every label of it
/// but the last (the TLD) that is not in [`GENERIC_HOST_LABELS`], each through
/// [`normalise_alias`]: `api.kinaswap.com` names a project called `kinaswap`
/// even with no URL constant; `nominatim.openstreetmap.org` names nothing in
/// a build without an `openstreetmap` / `nominatim` project.
fn host_names_an_alias(aliases: &AliasIndex, host: &str) -> bool {
    let host = host_name(host).to_ascii_lowercase();
    if aliases.contains_key(&normalise_alias(&host)) {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    let named = labels.len().saturating_sub(1);
    labels[..named]
        .iter()
        .filter(|l| !l.is_empty() && !GENERIC_HOST_LABELS.contains(l))
        .any(|l| aliases.contains_key(&normalise_alias(l)))
}

/// One client ENDPOINT node, however many graph entries carry it.
struct EndpointNode<'g> {
    id: NodeId,
    qname: &'g str,
    /// The strongest confidence of any entry.
    confidence: Confidence,
    /// Every entry's cells, so host narrowing reads every call site.
    cells: Vec<&'g Cell>,
}

/// Every ENDPOINT node of the merge, once per NodeId, in first-seen order.
///
/// An id can sit in several graphs of one repo (a TypeScript and a Dart client
/// calling one canonical path, LB.5). Pairing each entry on its own emitted the
/// same HTTP_CALLS edge once per entry and counted the node once per entry, so
/// the entries are merged here, the way `grpc.rs` `pair_servers` keys on
/// NodeId: the strongest entry confidence, and the union of the entries'
/// cells, so one call site with no host still blocks narrowing.
fn collect_endpoints(graphs: &[RepoGraph]) -> Vec<EndpointNode<'_>> {
    let mut out: Vec<EndpointNode<'_>> = Vec::new();
    let mut at: HashMap<NodeId, usize> = HashMap::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ENDPOINT) {
                continue;
            }
            if let Some(e) = at.get(&n.id).and_then(|&i| out.get_mut(i)) {
                e.confidence = strongest(e.confidence, n.confidence);
                e.cells.extend(&n.cells);
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            at.insert(n.id, out.len());
            out.push(EndpointNode {
                id: n.id,
                qname,
                confidence: n.confidence,
                cells: n.cells.iter().collect(),
            });
        }
    }
    out
}

/// The stronger of two confidences (the dual of [`weakest`]).
fn strongest(a: Confidence, b: Confidence) -> Confidence {
    if weakest(a, b) == a { b } else { a }
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
    /// Every tier's evidence rule, in ladder order: the `[evidence-rules]`
    /// order for `resolver=http`.
    const RULES: [&'static str; 6] = [
        "exact",
        "endpoint_prefix",
        "any",
        "route_prefix",
        "base_fold",
        "suffix",
    ];

    /// LC.3c: the rule an HTTP_CALLS edge's evidence names for this tier —
    /// the variant's snake_case name.
    fn rule(self) -> &'static str {
        match self {
            MatchTier::Exact => "exact",
            MatchTier::EndpointPrefix => "endpoint_prefix",
            MatchTier::Any => "any",
            MatchTier::RoutePrefix => "route_prefix",
            MatchTier::BaseFold => "base_fold",
            MatchTier::Suffix => "suffix",
        }
    }

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
    /// LB.4b: the subset of `host_narrowed` where the cut fell INSIDE one
    /// repo (a kept and a dropped target share a repo), i.e. the host named a
    /// nested project rather than a whole repo.
    owner_narrowed: usize,
    /// LB.5: the `[http-qname]` census.
    qnames: QnameCensus,
    /// LB.4a: indexed ROUTE nodes whose qname carries an owner segment
    /// (` @<project path>`), and ENDPOINT nodes likewise.
    owned_routes: usize,
    owned_endpoints: usize,
    /// LF.2d: indexed ROUTE nodes that gained a mounted key, the mounted
    /// `(METHOD, path)` keys, and the edges emitted through one.
    mount_routes: usize,
    mount_keys: usize,
    mount_hits: usize,
    /// CA-13 (CG.4b): ENDPOINT nodes left unpaired because every call site
    /// names a public host outside the build, and those hosts.
    external: usize,
    external_hosts: BTreeSet<String>,
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
///
/// LB.11b: `pathonly` is the second permanent detector — ROUTE qnames (owner
/// split off) in the per-path `route:<path>` shape. Since LB.11a (go) and
/// LB.11b (ts_routes) every server parser keys a route `<METHOD> <path>`, so a
/// non-zero count names a parser that regressed to the path-only shape.
#[derive(Default)]
struct QnameCensus {
    routes: usize,
    endpoints: usize,
    offenders: Vec<String>,
    pathonly: Vec<String>,
}

impl QnameCensus {
    /// The path part is read off the owner-free qname (LB.4a), so an owner
    /// segment never sits inside the judged path.
    fn route(&mut self, qname: &str) {
        self.routes += 1;
        let base = split_owner(qname).0;
        let legacy = base.strip_prefix("route:");
        if legacy.is_some() {
            self.pathonly.push(qname.to_string());
        }
        let path = legacy.or_else(|| base.split_once(' ').map(|(_, p)| p));
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
    /// regressing parser is identified without a rerun; LB.11b's `pathonly=`
    /// names its first three the same way.
    fn report(&mut self) {
        if self.routes + self.endpoints == 0 {
            return;
        }
        eprintln!(
            "[http-qname] routes={} endpoints={} noncanonical={} pathonly={}",
            self.routes,
            self.endpoints,
            self.offenders.len(),
            self.pathonly.len(),
        );
        if !self.offenders.is_empty() {
            self.offenders.sort_unstable();
            let first = &self.offenders[..self.offenders.len().min(3)];
            eprintln!("[http-qname] noncanonical first {}: {first:?}", first.len());
        }
        if !self.pathonly.is_empty() {
            self.pathonly.sort_unstable();
            let first = &self.pathonly[..self.pathonly.len().min(3)];
            eprintln!("[http-qname] pathonly first {}: {first:?}", first.len());
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
    fn count_client_normalised<'c>(&mut self, cells: impl IntoIterator<Item = &'c Cell>) {
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

    /// LF.2d fired_on marker, a line of its own so the A3.1 line above never
    /// moves. Printed only when the build carries mounts.
    fn report_mounts(&self, mounts: &RouteMounts) {
        if mounts.is_empty() {
            return;
        }
        eprintln!(
            "[http] overlay mounts: routes={} keys={} paired={}",
            self.mount_routes, self.mount_keys, self.mount_hits,
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
    fn report_owners(&self, owners: &Owners) {
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

    /// CG.4b fired_on marker. Printed only when an endpoint was left out of
    /// pairing as a third-party call, so every build without one is silent.
    fn report_external(&self) {
        if self.external == 0 {
            return;
        }
        eprintln!(
            "[http-external] {} endpoints left unpaired: every call site names a host outside the build (hosts={})",
            self.external,
            self.external_hosts.len(),
        );
    }

    /// LB.4b fired_on marker. Printed only when a host cut a target list
    /// inside one repo, so a build whose hosts name only whole repos (or
    /// nothing) stays silent. `aliases` counts the alias names a nested
    /// project contributed (its label, its directory name, or an IaC
    /// resource declared inside it).
    fn report_owner_narrowed(&self, aliases: usize) {
        if self.owner_narrowed == 0 {
            return;
        }
        eprintln!(
            "[http-owner] narrowed={} (host -> project, aliases={aliases})",
            self.owner_narrowed,
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
    /// under) as an index into the build's [`Owners`], so the target
    /// stays `Copy`. `None` for a route outside every nested root.
    owner: Option<u32>,
    /// LF.2d: registered under a [`RouteMounts`] gateway path, not its own.
    mounted: bool,
}

/// Host narrowing (A11.4 / LB.4b) reads a target's repo and owner; the
/// narrowing itself is shared with the other channel resolvers (`host.rs`).
impl HostScoped for (RouteTarget, MatchTier) {
    fn repo(&self) -> RepoId {
        self.0.repo
    }
    fn owner(&self) -> Option<u32> {
        self.0.owner
    }
}

/// Build `(METHOD, normalised_path) → Vec<RouteTarget>` across every graph in
/// the merge. One target per (method, path) a Route node serves: a node's
/// entries in several graphs, and stacked `ROUTE_METHOD` cells naming one verb
/// (gin's `router.GET("/user/2fa")` beside `userGroup.GET("/2fa")`), all land
/// on the same target (LB.4b, [`push_target`]). The counters count nodes.
///
/// A ROUTE's owner segment (LB.4a) is split off first: the method and path
/// are read from the owner-free qname, so pairing ignores owners, and the
/// owner rides on the target through the returned [`Owners`] table.
///
/// LF.2d: a ROUTE `mounts` names is ALSO registered, after its own keys,
/// under every `(METHOD, prefix + path)` in the strong `index` (never the
/// stripped one), with a `mounted` target whose confidence is the weaker of
/// the route's and the mount's ([`index_mounted`]).
fn build_route_index(
    graphs: &[RepoGraph],
    prefixes: &[String],
    mounts: &RouteMounts,
    stats: &mut HttpMatchStats,
) -> (RouteIndex, RouteIndex, Owners) {
    let mut index: RouteIndex = HashMap::new();
    let mut owners = Owners::default();
    // A3.1: the SYMMETRIC half of the prefix strip. Before A3.1 the strip only
    // ever removed prefixes from the client path, so a client calling `/users`
    // against a server mounted at `/api/users` could never pair. Every route is
    // additionally registered under each of its stripped forms here, kept in a
    // second map so the strong `index` stays exactly what it was.
    let mut stripped: RouteIndex = HashMap::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut mounted_seen: HashSet<NodeId> = HashSet::new();
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
            let first = seen.insert(n.id);
            if first {
                stats.routes += 1;
                stats.qnames.route(qname);
            }
            let (qname, owner) = split_owner(qname);
            let target = RouteTarget {
                route_id: n.id,
                confidence: n.confidence,
                repo: g.repo,
                owner: owner.and_then(|o| owners.intern(o)),
                mounted: false,
            };
            // Every entry is indexed (another graph's entry can stack a verb
            // the first did not), but counted once.
            let path = index_route_node(&mut index, &mut stripped, qname, &n.cells, target, prefixes);
            if first {
                stats.owned_routes += usize::from(target.owner.is_some());
                if let Some(path) = path {
                    stats.count_folds(path);
                }
            }
            for (prefix, conf) in mounts.of(n.id) {
                let mounted = RouteTarget {
                    confidence: weakest(n.confidence, *conf),
                    mounted: true,
                    ..target
                };
                let keys = index_mounted(&mut index, qname, &n.cells, mounted, prefix);
                stats.mount_keys += keys;
                if keys > 0 && mounted_seen.insert(n.id) {
                    stats.mount_routes += 1;
                }
            }
        }
    }
    (index, stripped, owners)
}

/// LF.2d: register a mounted `target` under every `(METHOD, prefix + path)`
/// the owner-free route `qname` serves (its own method, or each
/// `ROUTE_METHOD` cell's for the legacy `route:<path>` shape), in the STRONG
/// index only: a gateway path is matched as given, never prefix-stripped
/// again. Returns how many keys gained the target; a key that already holds
/// the route (its own path equals the mounted one) keeps its slot.
fn index_mounted(
    index: &mut RouteIndex,
    qname: &str,
    cells: &[Cell],
    target: RouteTarget,
    prefix: &str,
) -> usize {
    let Some((method, path)) = split_route_qname(qname) else {
        return 0;
    };
    let key_path = normalise_http_path(&format!("{prefix}/{path}"));
    let methods: Vec<String> = match method {
        Some(m) => vec![m.to_ascii_uppercase()],
        None => cells
            .iter()
            .filter(|c| c.kind == cell_type::ROUTE_METHOD)
            .filter_map(cell_method)
            .map(|m| m.to_ascii_uppercase())
            .collect(),
    };
    let mut added = 0;
    for m in methods {
        let slot = index.entry((m, key_path.clone())).or_default();
        let before = slot.len();
        push_target(slot, target);
        added += slot.len() - before;
    }
    added
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
    /// The ladder tier that matched, by its snake_case name — the same string
    /// an HTTP_CALLS edge's evidence names (LC.3c's `MatchTier::rule`):
    /// `exact` | `endpoint_prefix` | `any` | `route_prefix`. (`base_fold` and
    /// `suffix` are client-only tiers this matcher never offers.) LD.8b: lets
    /// an answer say "served, but only through the `ANY` fallback".
    pub tier: &'static str,
}

impl HttpRouteMatcher {
    pub fn new(graphs: &[RepoGraph]) -> Self {
        let prefixes = api_prefixes();
        let mut scratch = HttpMatchStats::default();
        // A contract pairs DECLARED paths: a gateway mount is not part of an
        // OpenAPI path, so the matcher never sees one (LF.2d).
        let (index, stripped, _owners) =
            build_route_index(graphs, &prefixes, &RouteMounts::default(), &mut scratch);
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
                tier: tier.rule(),
            })
            .collect()
    }
}

/// Register a ROUTE node into the (METHOD, path) index. Handles both qname
/// conventions:
///   1. LEGACY ONLY, no emitter since LB.11a (go) / LB.11b (ts_routes):
///      qname = `route:<path>`, methods live on stacked ROUTE_METHOD cells
///      (JSON payload). Kept so a hand-built or pre-0.5.0 graph still pairs;
///      the `[http-qname] pathonly=` census counts any that appear.
///   2. Every server parser: qname = `<METHOD> <path>` (`ANY` for a
///      method-agnostic registration), one Route node per (method, path),
///      its ROUTE_METHOD cell a bare-verb Text payload or a located JSON one.
///
/// Both shapes target the same downstream key space so HttpStackResolver sees
/// all routes uniformly.
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
    let (method, path) = split_route_qname(qname)?;
    let norm = normalise_http_path(path);
    match method {
        Some(method) => push_route(index, stripped, method, &norm, target, prefixes),
        None => {
            for cell in cells {
                if cell.kind != cell_type::ROUTE_METHOD {
                    continue;
                }
                let Some(method) = cell_method(cell) else {
                    continue;
                };
                push_route(index, stripped, &method, &norm, target, prefixes);
            }
        }
    }
    Some(path)
}

/// The one reader of the two ROUTE qname shapes (owner-free):
/// `route:<path>` (legacy, no emitter since LB.11b) -> `(None, path)`, methods
/// on stacked `ROUTE_METHOD` cells; `<METHOD> <path>` -> `(Some(METHOD), path)`.
/// `None` for neither.
fn split_route_qname(qname: &str) -> Option<(Option<&str>, &str)> {
    if let Some(path) = qname.strip_prefix("route:") {
        return Some((None, path));
    }
    // Legacy shape: "<METHOD> <path>". Split on the first space. Every
    // emitter now builds a canonical path (LB.5, counted by the `[http-qname]`
    // census); the `/` guard stays as the safety net for one that does not.
    match qname.split_once(' ') {
        Some((method, path)) if path.starts_with('/') => Some((Some(method), path)),
        _ => None,
    }
}

/// The raw request path a ROUTE qname names, in either shape, with the LB.4a
/// owner segment split off first: `route:/ws @services/chat` -> `/ws`,
/// `GET /users/{id}` -> `/users/{id}`. `None` for a qname neither shape
/// describes. Shared with the WebSocket resolver (LA.18b), which gives a
/// path-less upgrade handler the paths of the routes that reach it.
pub(crate) fn route_path(qname: &str) -> Option<&str> {
    split_route_qname(split_owner(qname).0).map(|(_, path)| path)
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
    push_target(index.entry((method.clone(), norm.to_string())).or_default(), target);
    // Every depth, not just the deepest: a route at `/api/v1/orders` must be
    // reachable from a client that says `/v1/orders` as well as one that says
    // `/orders`. Bounded at 2 segments, so this adds at most two entries.
    for cand in strip_api_prefixes(norm, prefixes) {
        push_target(stripped.entry((method.clone(), cand)).or_default(), target);
    }
}

/// Add `target` under one key, once per ROUTE id (LB.4b). A second sighting
/// of the id (another graph's entry, or a second `ROUTE_METHOD` cell for the
/// same verb) keeps the first slot, so hit order is unchanged, and takes the
/// stronger confidence. A key holds only the routes sharing one (method, path),
/// so the scan is short.
fn push_target(targets: &mut Vec<RouteTarget>, target: RouteTarget) {
    match targets.iter_mut().find(|t| t.route_id == target.route_id) {
        Some(t) => t.confidence = strongest(t.confidence, target.confidence),
        None => targets.push(target),
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
    use super::super::host::{AliasIndex, AliasScope};
    use super::*;

    #[test]
    fn route_path_reads_both_qname_shapes() {
        // LA.18b: the one reader of the ROUTE qname shape, shared with the
        // WebSocket resolver. The LB.4a owner segment never reaches the path.
        assert_eq!(route_path("route:/ws"), Some("/ws"));
        assert_eq!(route_path("GET /users/{id}"), Some("/users/{id}"));
        assert_eq!(route_path("route:/ws @services/chat"), Some("/ws"));
        assert_eq!(route_path("POST /orders @api"), Some("/orders"));
        // An `@` inside a segment is not an owner separator.
        assert_eq!(route_path("route:/pkg/@scope/x"), Some("/pkg/@scope/x"));
        // Neither shape: a nav page, a method with no path, a bare word.
        assert_eq!(route_path("page:/users"), None);
        assert_eq!(route_path("GET users"), None);
        assert_eq!(route_path("ws"), None);
        // index_route_node reads the same split: method from the legacy
        // qname, none (cells carry it) from the per-path one.
        assert_eq!(split_route_qname("route:/a"), Some((None, "/a")));
        assert_eq!(split_route_qname("GET /a"), Some((Some("GET"), "/a")));
    }

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
        use glia_code_domain::{CodeNav, GRAPH_TYPE};
        use glia_core::Node;

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
    /// so an owner-qualified client with no host reaches both (LB.4b narrows
    /// only on a host), in both route shapes, and the owners ride on the
    /// targets.
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
        let (index, _, owners) =
            build_route_index(std::slice::from_ref(&g), &[], &RouteMounts::default(), &mut stats);
        let targets = index
            .get(&("GET".to_string(), "/health".to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut got: Vec<(u64, Option<&str>)> = targets
            .iter()
            .map(|t| (t.route_id.0, t.owner.and_then(|i| owners.name(i))))
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

    #[test]
    fn match_tier_rules_follow_the_ladder() {
        // LC.3c: the `[evidence-rules]` order is the ladder order, and each
        // rule is its variant's snake_case name.
        let ladder = [
            MatchTier::Exact,
            MatchTier::EndpointPrefix,
            MatchTier::Any,
            MatchTier::RoutePrefix,
            MatchTier::BaseFold,
            MatchTier::Suffix,
        ];
        assert_eq!(ladder.map(MatchTier::rule), MatchTier::RULES);
    }

    /// A10.2 — one RepoGraph holding both ROUTE qname shapes, a NAV route and
    /// an `ANY` route, for the public matcher.
    fn matcher_graph() -> (RepoGraph, [NodeId; 4]) {
        use glia_code_domain::{CodeNav, GRAPH_TYPE};
        use glia_core::Node;

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
        let hit = |route, confidence, exact, tier| {
            vec![RouteMatch { route, confidence, exact, tier }]
        };

        // Tier 1, legacy `<METHOD> <path>` shape. Method is case-folded.
        assert_eq!(m.lookup("get", "/users"), hit(users, Confidence::Strong, true, "exact"));
        // Tier 1, `route:` shape; any param spelling normalises the same way.
        assert_eq!(
            m.lookup("POST", "/api/orders/{id}"),
            hit(orders, Confidence::Strong, true, "exact")
        );
        // Tier 2: an OpenAPI `servers: /api/v1` base is a caller-side prefix.
        assert_eq!(
            m.lookup("GET", "/api/v1/users"),
            hit(users, Confidence::Strong, false, "endpoint_prefix")
        );
        // Tier 4: route mounted under /api, caller declares the bare path.
        assert_eq!(
            m.lookup("POST", "/orders/{oid}"),
            hit(orders, Confidence::Medium, false, "route_prefix")
        );
        // Tier 3: an ANY route serves every verb, but is not an exact hit, and
        // the route's own Medium confidence is kept.
        assert_eq!(m.lookup("DELETE", "/health"), hit(health, Confidence::Medium, false, "any"));
    }

    /// LD.8b — `RouteMatch::tier` names the tier, so `serves` can say a route
    /// answered only through the method-agnostic fallback: a typed `GET
    /// /users` is `exact`, an `ANY /posts` reached by GET is `any`.
    #[test]
    fn route_match_names_its_tier() {
        use glia_code_domain::{CodeNav, GRAPH_TYPE};
        use glia_core::Node;

        let r = crate::test_support::repo();
        let mut nav = CodeNav::default();
        let mut nodes = Vec::new();
        for (qname, verb) in [("GET /users", "GET"), ("ANY /posts", "ANY")] {
            let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::ROUTE, qname);
            nav.record(id, qname, qname, node_kind::ROUTE, None);
            let cell = Cell { kind: cell_type::ROUTE_METHOD, payload: CellPayload::Text(verb.into()) };
            nodes.push(Node { id, repo: r, confidence: Confidence::Strong, cells: vec![cell] });
        }
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
        let m = HttpRouteMatcher::new(std::slice::from_ref(&g));
        let tier = |method, path| m.lookup(method, path).first().map(|h| (h.tier, h.exact));
        assert_eq!(tier("GET", "/users"), Some(("exact", true)));
        assert_eq!(tier("GET", "/posts"), Some(("any", false)));
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

    // ---- A11.4 host narrowing ------------------------------------------

    /// One repo holding `(kind, qname, cells)` nodes, and their ids in order.
    fn repo_graph(
        repo: RepoId,
        specs: Vec<(glia_core::NodeKindId, &str, Vec<Cell>)>,
    ) -> (RepoGraph, Vec<NodeId>) {
        use glia_code_domain::{CodeNav, GRAPH_TYPE};
        use glia_core::Node;

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

    /// One target `n` in `repo`, owned by project index `owner`.
    fn target(n: u64, repo: u64, owner: Option<u32>) -> (RouteTarget, MatchTier) {
        (
            RouteTarget {
                route_id: NodeId(n),
                confidence: Confidence::Strong,
                repo: RepoId(repo),
                owner,
                mounted: false,
            },
            MatchTier::Exact,
        )
    }

    fn alias_index(rows: &[(&str, &[AliasScope])]) -> AliasIndex {
        rows.iter()
            .map(|(name, scopes)| ((*name).to_string(), scopes.iter().copied().collect()))
            .collect()
    }

    fn host(h: &str) -> Vec<String> {
        vec![h.to_string()]
    }

    fn kept(hits: &[(RouteTarget, MatchTier)]) -> Vec<u64> {
        hits.iter().map(|(t, _)| t.route_id.0).collect()
    }

    /// Straight at the function: an alias whose repo holds none of the
    /// targets, and a single target, both leave `hits` untouched.
    #[test]
    fn narrow_by_host_never_empties_or_touches_a_single_hit() {
        let t = |n: u64, repo: u64| target(n, repo, None);
        let aliases = alias_index(&[
            ("payments", &[(RepoId(9), None)]),
            ("users", &[(RepoId(1), None)]),
        ]);

        let mut hits = vec![t(1, 1), t(2, 2)];
        assert_eq!(narrow_by_host(&aliases, Some(&host("payments:80")), &mut hits), Narrowed::No);
        assert_eq!(hits.len(), 2);

        let mut hits = vec![t(2, 2)];
        assert_eq!(narrow_by_host(&aliases, Some(&host("users")), &mut hits), Narrowed::No);
        assert_eq!(hits.len(), 1, "a lone hit in another repo is kept");

        let mut hits = vec![t(1, 1), t(2, 2), t(3, 1)];
        assert_eq!(narrow_by_host(&aliases, Some(&host("users-svc:80")), &mut hits), Narrowed::Repo);
        assert_eq!(kept(&hits), vec![1, 3], "order is kept");
        assert!(hits.iter().all(|(_, tier)| *tier == MatchTier::Exact));
    }

    /// LB.4b: two projects of ONE repo serve the path; the host names the
    /// users project, so only its route is kept, and the cut is owner-level.
    #[test]
    fn narrow_by_owner_keeps_the_named_project() {
        let (admin, users) = (0, 1);
        let aliases = alias_index(&[
            ("admin", &[(RepoId(7), Some(admin))]),
            ("users", &[(RepoId(7), Some(users))]),
        ]);
        let mut hits = vec![target(1, 7, Some(admin)), target(2, 7, Some(users))];
        assert_eq!(narrow_by_host(&aliases, Some(&host("users-svc:8080")), &mut hits), Narrowed::Owner);
        assert_eq!(kept(&hits), vec![2]);

        // A root-level route of the same repo (no owner) is not in the users
        // project either.
        let mut hits = vec![target(1, 7, None), target(2, 7, Some(users)), target(3, 7, Some(admin))];
        assert_eq!(narrow_by_host(&aliases, Some(&host("users")), &mut hits), Narrowed::Owner);
        assert_eq!(kept(&hits), vec![2]);

        // Hosts naming both projects keep both: nothing is cut.
        let mut hits = vec![target(1, 7, Some(admin)), target(2, 7, Some(users))];
        let both = vec!["users".to_string(), "admin-api".to_string()];
        assert_eq!(narrow_by_host(&aliases, Some(&both), &mut hits), Narrowed::No);
        assert_eq!(hits.len(), 2);

        // The same project index in ANOTHER repo is a different project.
        let mut hits = vec![target(1, 8, Some(users)), target(2, 8, Some(admin))];
        assert_eq!(narrow_by_host(&aliases, Some(&host("users")), &mut hits), Narrowed::No);
        assert_eq!(hits.len(), 2);
    }

    /// A repo-level alias (an IaC resource outside every nested project)
    /// keeps every owner of its repo, and only a cut across repos counts as
    /// `Repo`.
    #[test]
    fn repo_level_alias_keeps_every_owner() {
        let aliases = alias_index(&[("users", &[(RepoId(7), None)])]);
        let mut hits =
            vec![target(1, 7, Some(0)), target(2, 8, None), target(3, 7, Some(1)), target(4, 7, None)];
        assert_eq!(narrow_by_host(&aliases, Some(&host("users-service")), &mut hits), Narrowed::Repo);
        assert_eq!(kept(&hits), vec![1, 3, 4]);

        // A repo-level scope and a project scope of another repo: the union.
        let aliases = alias_index(&[("users", &[(RepoId(7), None), (RepoId(8), Some(2))])]);
        let mut hits = vec![target(1, 7, Some(0)), target(2, 8, Some(2)), target(3, 8, Some(3))];
        assert_eq!(narrow_by_host(&aliases, Some(&host("users")), &mut hits), Narrowed::Owner);
        assert_eq!(kept(&hits), vec![1, 2]);
    }

    /// Unknown hosts, a known name with no scope, and every other shortfall
    /// of evidence keep the whole list.
    #[test]
    fn unknown_host_keeps_all() {
        let aliases = alias_index(&[
            ("users", &[(RepoId(7), Some(1))]),
            // A project that serves no route: known, scoped to nothing.
            ("web", &[]),
        ]);
        let three = || vec![target(1, 7, Some(0)), target(2, 7, Some(1)), target(3, 8, None)];
        for hosts in [
            vec!["billing:9000".to_string()],
            vec!["users".to_string(), "billing".to_string()],
            vec!["users".to_string(), String::new()],
            vec!["web".to_string()],
        ] {
            let mut hits = three();
            assert_eq!(narrow_by_host(&aliases, Some(&hosts), &mut hits), Narrowed::No, "{hosts:?}");
            assert_eq!(hits.len(), 3, "{hosts:?}");
        }
        let mut hits = three();
        assert_eq!(narrow_by_host(&aliases, None, &mut hits), Narrowed::No);
        assert_eq!(hits.len(), 3);
        // A known-but-empty name does not block the other host's evidence.
        let mut hits = three();
        let web_or_users = vec!["web".to_string(), "users".to_string()];
        assert_eq!(narrow_by_host(&aliases, Some(&web_or_users), &mut hits), Narrowed::Owner);
        assert_eq!(kept(&hits), vec![2]);
    }

    /// LB.4b end to end: a monorepo (PROJECT graph + two owned routes + an
    /// owned client whose host names `users-svc`) pairs only with the users
    /// route.
    #[test]
    fn host_naming_a_project_pairs_only_with_its_route() {
        let r = RepoId(90);
        let hit_users = hit(r#"{"method":"GET","path":"/health","host":"users-svc:8080"}"#);
        let (code, ids) = repo_graph(
            r,
            vec![
                (node_kind::ENDPOINT, "endpoint:GET:/health @web", vec![hit_users]),
                (node_kind::ROUTE, "GET /health @services/admin", vec![text_get()]),
                (node_kind::ROUTE, "GET /health @services/users", vec![text_get()]),
            ],
        );
        let (mut projects, pids) = repo_graph(
            r,
            vec![
                (node_kind::PROJECT, "project:services/admin", vec![]),
                (node_kind::PROJECT, "project:services/users", vec![]),
                (node_kind::PROJECT, "project:web", vec![]),
            ],
        );
        for (id, label) in pids.iter().zip(["admin", "users", "web"]) {
            projects.nav.name_by_id.insert(*id, label.to_string());
        }
        let mut merged = MergedGraph::new(vec![code, projects]);
        HttpStackResolver.resolve(&mut merged);
        let to: Vec<NodeId> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::HTTP_CALLS)
            .map(|e| e.to)
            .collect();
        assert_eq!(to, vec![ids[2]]);
    }

    /// LB.4b id-dedup: an ENDPOINT id in two graphs of one repo, and a ROUTE
    /// with its verb stacked twice (and itself in two graphs), pair ONCE,
    /// with the strongest entry confidence; the hosts are the union of both
    /// entries' cells.
    #[test]
    fn a_node_in_two_graphs_pairs_once() {
        let r = RepoId(91);
        let twice: Vec<Cell> = get_route().into_iter().chain(get_route()).collect();
        let (mut ts, ids) = repo_graph(
            r,
            vec![
                (node_kind::ENDPOINT, "endpoint:GET:/user/2fa", vec![hit(r#"{"host":"users"}"#)]),
                (node_kind::ROUTE, "route:/user/2fa", twice),
            ],
        );
        let (mut dart, _) = repo_graph(
            r,
            vec![
                (node_kind::ENDPOINT, "endpoint:GET:/user/2fa", vec![hit(r#"{"path":"/user/2fa"}"#)]),
                (node_kind::ROUTE, "route:/user/2fa", get_route()),
            ],
        );
        ts.nodes[0].confidence = Confidence::Weak;
        dart.nodes[0].confidence = Confidence::Medium;
        let mut merged = MergedGraph::new(vec![ts, dart]);
        HttpStackResolver.resolve(&mut merged);
        let calls: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::HTTP_CALLS)
            .collect();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!((calls[0].from, calls[0].to), (ids[0], ids[1]));
        assert_eq!(calls[0].confidence, Confidence::Medium, "the strongest entry wins");

        let endpoints = collect_endpoints(&merged.graphs);
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].id, ids[0]);
        assert_eq!(endpoints[0].confidence, Confidence::Medium);
        assert_eq!(endpoints[0].cells.len(), 2, "both entries' hits");
        // The second entry's hit names no host, so the union is no evidence.
        assert_eq!(hit_hosts(endpoints[0].cells.iter().copied()), None);
        // The route is counted, and indexed, once.
        let mut stats = HttpMatchStats::default();
        let (index, _, _) = build_route_index(&merged.graphs, &[], &RouteMounts::default(), &mut stats);
        assert_eq!(stats.routes, 1);
        assert_eq!(index.get(&("GET".to_string(), "/user/2fa".to_string())).map(Vec::len), Some(1));
    }

    /// CG.4b (CA-13): a web repo whose ENDPOINT `endpoint:GET:/search` carries
    /// `hits`, and a server repo serving `GET /search` beside a nested
    /// PROJECT labelled `project`. Returns the HTTP_CALLS count and what
    /// [`third_party_hosts`] said of the endpoint over the same alias index.
    fn pair_search(hits: Vec<Cell>, project: &str) -> (usize, Option<Vec<String>>) {
        let (web, _) = repo_graph(RepoId(501), vec![(node_kind::ENDPOINT, "endpoint:GET:/search", hits)]);
        let (mut server, ids) = repo_graph(
            RepoId(502),
            vec![
                (node_kind::ROUTE, "GET /search", vec![text_get()]),
                (node_kind::PROJECT, "project:apps/backend", vec![]),
            ],
        );
        server.nav.name_by_id.insert(ids[1], project.to_string());
        let mut merged = MergedGraph::new(vec![web, server]);
        HttpStackResolver.resolve(&mut merged);
        let calls = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::HTTP_CALLS)
            .count();
        let (aliases, _) = build_service_alias_index(&merged.graphs, &Owners::default());
        let endpoints = collect_endpoints(&merged.graphs);
        let verdict = third_party_hosts(&aliases, &endpoints[0].cells);
        assert_eq!(
            HttpStackResolver::third_party_endpoints(&merged.graphs).contains(&endpoints[0].id),
            verdict.is_some(),
            "the public set is the resolver's own predicate"
        );
        (calls, verdict)
    }

    /// CG.4b: an endpoint whose every call site is marked external, with a
    /// host naming nothing in the build, pairs with nothing although the app
    /// serves the path; unmarked, or with a host whose label names a project
    /// of the build, it pairs as before.
    #[test]
    fn external_endpoint_is_left_unpaired() {
        let nominatim = r#"{"method":"GET","path":"/search","host":"nominatim.openstreetmap.org","external":true}"#;
        let (calls, hosts) = pair_search(vec![hit(nominatim), hit(nominatim)], "backend");
        assert_eq!(calls, 0, "a third-party /search is not the app's GET /search");
        assert_eq!(hosts, Some(vec!["nominatim.openstreetmap.org".to_string()]), "hosts once");

        // The same site without the mark pairs.
        let plain = r#"{"method":"GET","path":"/search","host":"nominatim.openstreetmap.org"}"#;
        assert_eq!(pair_search(vec![hit(plain)], "backend"), (1, None));

        // One marked site and one unmarked: not every site is external.
        assert_eq!(pair_search(vec![hit(nominatim), hit(plain)], "backend"), (1, None));

        // A marked host whose non-generic label names a project of the build
        // (`api.shop.io` -> `shop`) is the build's own backend.
        let shop = r#"{"method":"GET","path":"/search","host":"api.shop.io","external":true}"#;
        assert_eq!(pair_search(vec![hit(shop)], "shop"), (1, None));
        // ... and a project named like the generic label claims nothing.
        assert_eq!(pair_search(vec![hit(shop)], "api").0, 0);

        // `<unresolved>` is skipped before the mark is read, as it always was.
        let (g, _) = repo_graph(
            RepoId(503),
            vec![(node_kind::ENDPOINT, "endpoint:GET:<unresolved>", vec![hit(nominatim)])],
        );
        assert!(HttpStackResolver::third_party_endpoints(&[g]).is_empty());

        // A non-JSON hit is never a marked site.
        let text = Cell { kind: cell_type::ENDPOINT_HIT, payload: CellPayload::Text("external".into()) };
        assert!(!all_sites_external(&[text]));
        assert!(!all_sites_external(&[]), "no hit, no mark");
    }

    #[test]
    fn host_names_an_alias_reads_non_generic_labels() {
        let aliases = alias_index(&[("shop", &[]), ("users", &[(RepoId(1), None)])]);
        assert!(host_names_an_alias(&aliases, "api.shop.io"));
        assert!(host_names_an_alias(&aliases, "www.SHOP.com:443"));
        assert!(host_names_an_alias(&aliases, "users-api.example.io"), "normalised: users-api -> users");
        assert!(host_names_an_alias(&aliases, "users"), "the whole host, as narrowing reads it");
        assert!(!host_names_an_alias(&aliases, "nominatim.openstreetmap.org"));
        assert!(!host_names_an_alias(&aliases, "api.example.shop"), "the TLD is never a name");
        let generic = alias_index(&[("app", &[]), ("api", &[])]);
        assert!(!host_names_an_alias(&generic, "app.acme.io"));
        assert!(!host_names_an_alias(&generic, "api.acme.io"));
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
        // LB.11b: the path-only shape is counted whether or not its path is
        // canonical, and only for ROUTEs.
        assert_eq!(c.pathonly, vec!["route:/items", "route:items"]);

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
        // The owner is split off before the path-only test, too.
        assert_eq!(c.pathonly, vec!["route:/items @web"]);
    }

    // ---- LF.2d route mounts ----------------------------------------------

    /// A mount registers the orders route at `/orders-svc/users`: the gateway
    /// path pairs with it alone (billing serves `/users` too), at the Exact
    /// tier but with the mount's confidence. Without the mount it pairs with
    /// nothing, and the contract matcher never sees a mount.
    #[test]
    fn mounted_key_pairs_only_the_mounted_route() {
        let build = || {
            let (web, _) = repo_graph(
                RepoId(501),
                vec![
                    (node_kind::ENDPOINT, "endpoint:GET:/orders-svc/users", vec![hit("{}")]),
                    (node_kind::ENDPOINT, "endpoint:GET:/orders-svc/orders", vec![hit("{}")]),
                    (node_kind::ENDPOINT, "endpoint:GET:/users", vec![hit("{}")]),
                ],
            );
            let (orders, o) = repo_graph(
                RepoId(502),
                vec![
                    (node_kind::ROUTE, "GET /users", vec![]),
                    // The legacy path-only shape reads its verbs off the cell.
                    (node_kind::ROUTE, "route:/orders", get_route()),
                ],
            );
            let (billing, b) = repo_graph(RepoId(503), vec![(node_kind::ROUTE, "GET /users", vec![])]);
            (MergedGraph::new(vec![web, orders, billing]), o, b[0])
        };
        let pairs = |m: &MergedGraph| -> Vec<(String, NodeId, Confidence, Option<String>)> {
            let mut out: Vec<_> = m
                .cross_edges
                .iter()
                .filter(|e| e.category == edge_category::HTTP_CALLS)
                .map(|e| {
                    let from = m.graphs[0].nav.qname_by_id.get(&e.from).cloned().unwrap_or_default();
                    let rule = glia_code_domain::evidence::Evidence::of(e).and_then(|ev| ev.rule);
                    (from, e.to, e.confidence, rule)
                })
                .collect();
            out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
            out
        };

        // No mounts: the gateway paths pair with nothing; `/users` fans out.
        let (mut plain, o, billing) = build();
        HttpStackResolver.resolve(&mut plain);
        let got = pairs(&plain);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.iter().all(|p| p.0 == "endpoint:GET:/users"), "{got:?}");

        let mut mounts = RouteMounts::default();
        assert!(mounts.is_empty());
        mounts.add(o[0], "/orders-svc", Confidence::Weak);
        mounts.add(o[1], "/orders-svc/", Confidence::Weak);
        mounts.add(o[1], "/orders-svc/", Confidence::Medium);
        assert_eq!(mounts.routes(), 2);
        assert_eq!(mounts.of(o[1]), [("/orders-svc/".to_string(), Confidence::Medium)]);
        assert!(mounts.of(billing).is_empty());

        let (mut mounted, _, _) = build();
        HttpStackResolver::with_mounts(&mounts).resolve(&mut mounted);
        let got = pairs(&mounted);
        let exact = Some("exact".to_string());
        assert_eq!(
            got,
            vec![
                ("endpoint:GET:/orders-svc/orders".to_string(), o[1], Confidence::Medium, exact.clone()),
                ("endpoint:GET:/orders-svc/users".to_string(), o[0], Confidence::Weak, exact.clone()),
                ("endpoint:GET:/users".to_string(), o[0], Confidence::Strong, exact.clone()),
                ("endpoint:GET:/users".to_string(), billing, Confidence::Strong, exact),
            ],
            "the mounted keys pair the orders routes only, at the mount's confidence; \
             the routes' own keys are untouched"
        );

        // The mounted index: keys land in the strong index only, counted.
        let mut stats = HttpMatchStats::default();
        let (index, stripped, _) = build_route_index(&mounted.graphs, &["api".to_string()], &mounts, &mut stats);
        assert_eq!((stats.mount_routes, stats.mount_keys), (2, 2));
        let key = ("GET".to_string(), "/orders-svc/users".to_string());
        let hit = index.get(&key).expect("mounted key");
        assert_eq!(hit.len(), 1);
        assert!(hit[0].mounted && hit[0].route_id == o[0] && hit[0].confidence == Confidence::Weak);
        assert!(!stripped.contains_key(&key));
        assert!(index.get(&("GET".to_string(), "/users".to_string())).is_some_and(|t| t.iter().all(|t| !t.mounted)));

        // The contract matcher pairs declared paths only.
        assert!(HttpRouteMatcher::new(&mounted.graphs).lookup("GET", "/orders-svc/users").is_empty());
    }
}
