//! Who serves a channel (LD.8b): an HTTP `METHOD /path` through the HTTP
//! resolver's own route matcher, or a queue topic through its
//! `queue_consumer:<topic>` nodes. The answer is the located servers with
//! their handlers, or — when nothing serves the channel — a FACT-tier
//! `unserved_channel` absence carrying the mechanism's caveat rows and the
//! near misses an agent would ask about next.
//!
//! Before this, "what serves `POST /orders`" meant finding a ROUTE by a name
//! substring (route qnames differ by parser) and folding placeholders and API
//! prefixes by hand. [`serves`] asks [`HttpRouteMatcher`] instead: the same
//! index and tiers 1-4 the resolver pairs a client ENDPOINT with, so an answer
//! agrees with the HTTP_CALLS edges the build drew, and each server names the
//! tier that matched it (`exact` | `endpoint_prefix` | `any` | `route_prefix`)
//! — "served, but only through the `ANY` fallback" is visible.
//!
//! External servers (CJ.3): an HTTP channel is also served OUTSIDE the build
//! when a client ENDPOINT for that verb and path is one CG.4b stamped ORIGIN
//! `{"provenance":"external"}` (every call site names a public host no
//! service, project or URL constant of the build names, and nothing pairs
//! it). Such an ENDPOINT is listed after the routes, kind `ENDPOINT`, match
//! `external`, no handlers, with the hosts its sites name in
//! `external_hosts`; a channel asked as a full URL keeps only the external
//! rows whose hosts include the URL's. A channel with only external rows is
//! served, so it carries no absence. [`external_hosts`] is the one reader of
//! that verdict, shared with `effects`: an endpoint the build pairs (an alias
//! host, an overlay `[constants]` pin) is in-repo here, as in `glia gaps` and
//! engram-export. HTTP only: WS / gRPC / GraphQL clients are not marked.
//!
//! What it does not see, stated rather than guessed:
//! - The client-only tiers 5-6 (base-URL fold, suffix) — the matcher does not
//!   offer them to a caller holding a declared path.
//! - Gateway mounts from an overlay's `[[route_prefix]]` (LF.2d): the matcher
//!   indexes routes at their declared paths, so ask for the path the service
//!   itself registers.
//! - Wildcard queue subscriptions (`orders.*`): the queue resolver's pattern
//!   matcher is private, so only literal topics are matched, and the absence
//!   says so.
//!
//! No taint or auth reasoning: this answers where a channel is handled, never
//! which routes reach data without auth.
//!
//! fired_on marker, once per call:
//! `[serves] mechanism=<http|queue> channel='<c>' servers=<n> match=<tiers|->`
//! — grep token `[serves] mechanism=`. An external row's tier is `external`.
//!
//! Module slot declared by L0.2; its API is reached as
//! `glia_engine::serves::<item>`, never flattened into the crate root.

use std::collections::{BTreeSet, HashMap, HashSet};

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_code_extractors::queues::is_framework_tag;
use glia_core::{Cell, CellPayload, Confidence, NodeId};
use glia_graph::{HttpRouteMatcher, MergedGraph, normalise_http_path};

use crate::absence::{self, Answer, mechanisms_for_kind};
use crate::answers::{Located, Locator, entrypoint_reachable, live_marker};

/// The mechanisms [`serves`] accepts, in the order its error names them.
const MECHANISMS: [&str; 3] = ["auto", "http", "queue"];

/// The verbs a bare path is looked up under, and the order near misses are
/// collected in.
const VERBS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

/// Leading tokens `auto` reads as an HTTP verb (`VERBS` plus the ones a
/// route may still be registered under, and the matcher's own `ANY`).
const HTTP_VERBS: [&str; 8] = [
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "ANY",
];

/// How many near misses an `unserved_channel` absence names.
const SUGGESTIONS: usize = 5;

/// A queue consumer's qname prefix; a producer's.
const CONSUMER: &str = "queue_consumer:";
const PRODUCER: &str = "queue_producer:";

/// An ENDPOINT qname's prefix (`endpoint:<METHOD>:<path>`).
const ENDPOINT: &str = "endpoint:";

/// The match tier of a server outside the build (CJ.3).
const EXTERNAL: &str = "external";

/// One node that serves the channel, located, with the code it hands off to.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct Server {
    pub id: u64,
    pub qname: String,
    /// `ROUTE` or `QUEUE_CONSUMER`; `ENDPOINT` for an `external` row (the
    /// build's client of a server outside it).
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based (LD.1's `Locator`).
    pub line: Option<i64>,
    /// Reachable from an entrypoint (LD.6): `false` = likely dead.
    pub live: bool,
    /// HTTP: the matcher tier that reached the route — `exact`,
    /// `endpoint_prefix`, `any` (the method-agnostic fallback) or
    /// `route_prefix` — or `external` for a client ENDPOINT whose server is
    /// outside the build (CJ.3). Queue: always `exact` (literal topics only).
    pub r#match: &'static str,
    /// `strong` | `medium` | `weak`. HTTP: the route's own confidence capped
    /// at its tier's ceiling (an `external` row: the endpoint node's); queue:
    /// the consumer node's.
    pub confidence: &'static str,
    /// The HANDLED_BY targets of the server, in edge order, located. Empty
    /// when the build bound no handler (a module-level queue consumer, a
    /// handler-less framework route), and for every `external` row (the
    /// handler is outside the build).
    pub handlers: Vec<Located>,
    /// The hosts outside the build this row's client calls (match
    /// `external`), sorted; empty for every in-repo server.
    pub external_hosts: Vec<String>,
}

/// Who serves `channel`. `mechanism` is `auto`, `http` or `queue` (any case);
/// `auto` reads a channel whose first word is an HTTP verb (`POST /orders`)
/// or that starts with `/` as HTTP, anything else as a queue topic.
///
/// HTTP: `METHOD /path` is looked up under that verb; a bare `/path` under
/// GET, POST, PUT, PATCH and DELETE in that order, deduped by route (the
/// matcher's `ANY` tier still finds method-agnostic routes under each). A
/// full URL is read for its path. After the routes come the `external` rows:
/// the ENDPOINTs CG.4b stamped external for those verbs and that path, in
/// verb order then build order (a full URL keeps those naming its host).
/// Queue: every `queue_consumer:<topic>` node
/// (owner segment ignored), graphs in build order; a framework tag
/// (`unresolved:<framework>`) is refused, not matched.
///
/// Empty results carry an [`Absence`](crate::absence::Absence):
/// `unserved_channel` with near misses — HTTP: the same path's routes under
/// the other verbs, then the parent path's routes; queue: the topic's
/// producers, then consumed topics differing only by case or separator — or
/// `no_match` for a refused framework tag.
///
/// `Err` only for an unknown `mechanism`. Each server's `live` is read off
/// one [`entrypoint_reachable`] walk; [`serves_with_live`] takes the set.
pub fn serves(
    merged: &MergedGraph,
    channel: &str,
    mechanism: &str,
) -> Result<Answer<Server>, String> {
    let mech = mechanism_of(channel, mechanism)?;
    Ok(answer(merged, &entrypoint_reachable(merged), channel, mech))
}

/// [`serves`] over a live set the caller already holds (pyo3's `PyGraph`
/// caches one per graph): `live` must be [`entrypoint_reachable`] of `merged`.
pub fn serves_with_live(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    channel: &str,
    mechanism: &str,
) -> Result<Answer<Server>, String> {
    let mech = mechanism_of(channel, mechanism)?;
    Ok(answer(merged, live, channel, mech))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mechanism {
    Http,
    Queue,
}

impl Mechanism {
    fn name(self) -> &'static str {
        match self {
            Mechanism::Http => "http",
            Mechanism::Queue => "queue",
        }
    }
}

/// The mechanism `mechanism` names for `channel` (`auto` decides by shape).
fn mechanism_of(channel: &str, mechanism: &str) -> Result<Mechanism, String> {
    match mechanism.trim().to_ascii_lowercase().as_str() {
        "http" => Ok(Mechanism::Http),
        "queue" => Ok(Mechanism::Queue),
        "auto" => {
            let c = channel.trim();
            Ok(if c.starts_with('/') || split_verb(c).is_some() {
                Mechanism::Http
            } else {
                Mechanism::Queue
            })
        }
        _ => Err(format!(
            "unknown mechanism '{}'; valid: {}",
            mechanism.trim(),
            MECHANISMS.join(", ")
        )),
    }
}

/// `(VERB, rest)` when `c`'s first word is an HTTP verb (any case) and
/// something follows it.
fn split_verb(c: &str) -> Option<(&'static str, &str)> {
    let (head, rest) = c.split_once(char::is_whitespace)?;
    let verb = HTTP_VERBS
        .iter()
        .copied()
        .find(|v| v.eq_ignore_ascii_case(head))?;
    let rest = rest.trim();
    (!rest.is_empty()).then_some((verb, rest))
}

fn answer(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    channel: &str,
    mech: Mechanism,
) -> Answer<Server> {
    let channel = channel.trim();
    let answer = match mech {
        Mechanism::Http => http(merged, live, channel),
        Mechanism::Queue => queue(merged, live, channel),
    };
    let mut tiers: Vec<&str> = Vec::new();
    for s in &answer.results {
        if !tiers.contains(&s.r#match) {
            tiers.push(s.r#match);
        }
    }
    let tiers = if tiers.is_empty() {
        "-".to_string()
    } else {
        tiers.join(",")
    };
    eprintln!(
        "[serves] mechanism={} channel='{channel}' servers={} match={tiers}",
        mech.name(),
        answer.results.len()
    );
    answer
}

// ---- HTTP ------------------------------------------------------------------

fn http(merged: &MergedGraph, live: &HashSet<NodeId>, channel: &str) -> Answer<Server> {
    let (verbs, raw_path): (Vec<&str>, &str) = match split_verb(channel) {
        Some((verb, rest)) => (vec![verb], rest),
        None => (VERBS.to_vec(), channel),
    };
    let path = request_path(raw_path);
    let host = request_host(raw_path);
    let matcher = HttpRouteMatcher::new(&merged.graphs);

    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut hits: Vec<(NodeId, &'static str, Confidence)> = Vec::new();
    for verb in &verbs {
        for h in matcher.lookup(verb, &path) {
            if seen.insert(h.route) {
                hits.push((h.route, h.tier, h.confidence));
            }
        }
    }
    let external = external_servers(merged, &verbs, &path, host.as_deref());
    let results = servers(merged, live, &hits, &external);
    Answer::from_results(results, || {
        let mechanisms = mechanisms_for_kind(node_kind::ROUTE);
        let asked = if verbs.len() == 1 {
            format!("`{} {path}`", verbs[0])
        } else {
            format!("`{path}` under {}", VERBS.join(" / "))
        };
        let note = if matcher.is_empty() {
            format!("no route serves {asked}: the build holds no server ROUTE")
        } else {
            format!("no route serves {asked} in this graph")
        };
        let near = http_near_misses(merged, &matcher, &verbs, &path);
        absence::unserved_channel(merged, "serves", channel, note, mechanisms, near)
    })
}

/// The request path of `raw`: a full URL's scheme and authority, a query
/// string and a fragment dropped, a leading `/` ensured. Placeholder and
/// slash normalisation are the matcher's.
fn request_path(raw: &str) -> String {
    let mut p = raw.trim();
    if let Some(i) = p.find(['?', '#']) {
        p = &p[..i];
    }
    if let Some((_, after_scheme)) = p.split_once("://") {
        p = after_scheme.find('/').map_or("", |i| &after_scheme[i..]);
    }
    if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    }
}

/// The host a full URL names: its authority with any `user@` and a numeric
/// `:port` stripped, lower-cased, a trailing root `.` dropped; `None` for a
/// bare path or an empty authority.
fn request_host(raw: &str) -> Option<String> {
    let (_, after_scheme) = raw.trim().split_once("://")?;
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = host_name(authority);
    (!host.is_empty()).then_some(host)
}

/// `[user@]host[:port]` -> `host`, lower-cased, a trailing root `.` dropped:
/// the shape the endpoint fold judges a host in, so a recorded `host` and an
/// asked one compare equal whatever their case or port.
fn host_name(authority: &str) -> String {
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// CG.4b's verdict on one ENDPOINT id, read from every cell of every entry of
/// it (an ENDPOINT id can sit in several graphs of a repo, LB.5, and CG.4b
/// stamps every entry): `Some(hosts)` when some ORIGIN cell parses to
/// `provenance == "external"`, `hosts` being the distinct `host` strings of
/// the ENDPOINT_HIT payloads marked `"external": true` (CG.4a), sorted.
/// `None` for an in-repo endpoint, including an external call the build pairs
/// because its host names a project alias, or an overlay pin configured its
/// site: CG.4b never stamps those. The one reader `effects` and `serves`
/// share, so the two answers agree with `glia gaps` and engram-export.
pub(crate) fn external_hosts<'c>(cells: impl IntoIterator<Item = &'c Cell>) -> Option<Vec<String>> {
    let mut external = false;
    let mut hosts: BTreeSet<String> = BTreeSet::new();
    for c in cells {
        let (CellPayload::Json(j) | CellPayload::Text(j)) = &c.payload else {
            continue;
        };
        if c.kind == cell_type::ORIGIN {
            external |= serde_json::from_str::<serde_json::Value>(j)
                .is_ok_and(|v| v.get("provenance").and_then(|p| p.as_str()) == Some(EXTERNAL));
        } else if c.kind == cell_type::ENDPOINT_HIT
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(j)
            && v.get("external").and_then(|e| e.as_bool()) == Some(true)
            && let Some(h) = v.get("host").and_then(|h| h.as_str())
        {
            hosts.insert(h.to_string());
        }
    }
    external.then(|| hosts.into_iter().collect())
}

/// `(id, confidence, hosts)` of every ENDPOINT CG.4b stamped external whose
/// owner-free qname is `endpoint:<verb>:<path>` for one of `verbs` (paths
/// compared through the matcher's [`normalise_http_path`]), keeping only
/// those whose hosts include `host` (in [`host_name`] form) when the channel
/// named one. In verb order, then graph and node build order; the first entry
/// of an id fixes its place and its confidence. One pass over the nodes.
fn external_servers(
    merged: &MergedGraph,
    verbs: &[&str],
    path: &str,
    host: Option<&str>,
) -> Vec<(NodeId, Confidence, Vec<String>)> {
    let want = normalise_http_path(path);
    // Per id: (verb index, confidence of the first entry, every entry's cells).
    let mut order: Vec<NodeId> = Vec::new();
    let mut found: HashMap<NodeId, (usize, Confidence, Vec<&Cell>)> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ENDPOINT) {
                continue;
            }
            if let Some(entry) = found.get_mut(&n.id) {
                entry.2.extend(n.cells.iter());
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            let Some((verb, p)) = split_owner(qname)
                .0
                .strip_prefix(ENDPOINT)
                .and_then(|rest| rest.split_once(':'))
            else {
                continue;
            };
            let Some(vi) = verbs.iter().position(|v| v.eq_ignore_ascii_case(verb)) else {
                continue;
            };
            if normalise_http_path(p) != want {
                continue;
            }
            order.push(n.id);
            found.insert(n.id, (vi, n.confidence, n.cells.iter().collect()));
        }
    }
    let mut rows: Vec<(usize, NodeId, Confidence, Vec<String>)> = Vec::new();
    for id in order {
        let Some((vi, confidence, cells)) = found.remove(&id) else {
            continue;
        };
        let Some(hosts) = external_hosts(cells) else {
            continue;
        };
        if host.is_some_and(|h| !hosts.iter().any(|x| host_name(x) == h)) {
            continue;
        }
        rows.push((vi, id, confidence, hosts));
    }
    // Stable: build order within a verb.
    rows.sort_by_key(|r| r.0);
    rows.into_iter()
        .map(|(_, id, confidence, hosts)| (id, confidence, hosts))
        .collect()
}

/// `path` with its last segment stripped; `None` for a one-segment path (its
/// parent is `/`, which is every path's ancestor and so no near miss).
fn parent_path(path: &str) -> Option<&str> {
    let (parent, _) = path.trim_end_matches('/').rsplit_once('/')?;
    (!parent.is_empty()).then_some(parent)
}

/// Near misses for an unserved HTTP channel, all through the matcher: the
/// routes `path` has under the verbs not asked, then the parent path's routes
/// under every verb. Deduped, at most [`SUGGESTIONS`], as qnames.
fn http_near_misses(
    merged: &MergedGraph,
    matcher: &HttpRouteMatcher,
    asked: &[&str],
    path: &str,
) -> Vec<String> {
    let mut ids: Vec<NodeId> = Vec::new();
    let mut take = |verb: &str, p: &str| {
        for h in matcher.lookup(verb, p) {
            if !ids.contains(&h.route) {
                ids.push(h.route);
            }
        }
    };
    for verb in VERBS.iter().filter(|v| !asked.contains(v)) {
        take(verb, path);
    }
    if let Some(parent) = parent_path(path) {
        for verb in VERBS {
            take(verb, parent);
        }
    }
    ids.truncate(SUGGESTIONS);
    ids.into_iter()
        .filter_map(|id| qname_of(merged, id))
        .collect()
}

// ---- queue -----------------------------------------------------------------

fn queue(merged: &MergedGraph, live: &HashSet<NodeId>, topic: &str) -> Answer<Server> {
    let mechanisms = mechanisms_for_kind(node_kind::QUEUE_CONSUMER);
    if is_framework_tag(topic) {
        return Answer::from_results(Vec::new(), || {
            let note = format!(
                "`{topic}` is the tag an unresolved topic is extracted as, and a framework tag is not a topic: \
                 nothing is matched against it (`glia gaps` lists the tag-only queue nodes)"
            );
            absence::empty(merged, "serves", topic, "no_match", note, mechanisms, None)
        });
    }
    let hits: Vec<(NodeId, &'static str, Confidence)> =
        queue_nodes(merged, node_kind::QUEUE_CONSUMER, CONSUMER)
            .into_iter()
            .filter(|(_, t, _)| *t == topic)
            .map(|(id, _, confidence)| (id, "exact", confidence))
            .collect();
    let results = servers(merged, live, &hits, &[]);
    Answer::from_results(results, || {
        let producers: Vec<(NodeId, &str, Confidence)> =
            queue_nodes(merged, node_kind::QUEUE_PRODUCER, PRODUCER)
                .into_iter()
                .filter(|(_, t, _)| *t == topic)
                .collect();
        let published = match producers.len() {
            0 => String::new(),
            n => format!(
                " ({n} {} publish to it)",
                absence::plural(n, "producer", "producers")
            ),
        };
        let note = format!(
            "no consumer subscribes to topic `{topic}` in this graph{published}; \
             wildcard subscribers are not matched by serves"
        );
        let mut near: Vec<NodeId> = producers.iter().map(|(id, _, _)| *id).collect();
        let folded = fold_topic(topic);
        for (id, t, _) in queue_nodes(merged, node_kind::QUEUE_CONSUMER, CONSUMER) {
            if t != topic && !is_framework_tag(t) && fold_topic(t) == folded && !near.contains(&id)
            {
                near.push(id);
            }
        }
        near.truncate(SUGGESTIONS);
        let near = near
            .into_iter()
            .filter_map(|id| qname_of(merged, id))
            .collect();
        absence::unserved_channel(merged, "serves", topic, note, mechanisms, near)
    })
}

/// `(id, topic, confidence)` of every `kind` node whose owner-free qname is
/// `prefix<topic>`, once per id, graphs and nodes in build order.
fn queue_nodes<'m>(
    merged: &'m MergedGraph,
    kind: glia_core::NodeKindId,
    prefix: &str,
) -> Vec<(NodeId, &'m str, Confidence)> {
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut out = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&kind) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            let Some(topic) = split_owner(qname).0.strip_prefix(prefix) else {
                continue;
            };
            if seen.insert(n.id) {
                out.push((n.id, topic, n.confidence));
            }
        }
    }
    out
}

/// A topic with case and separators (`.` `_` `-` `/` `:` whitespace) folded,
/// so `Orders_Created` and `orders.created` compare equal.
fn fold_topic(topic: &str) -> String {
    topic
        .chars()
        .map(|c| match c {
            '_' | '-' | '/' | ':' => '.',
            c if c.is_whitespace() => '.',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

// ---- shared ----------------------------------------------------------------

/// Located servers for `hits` (in order), each with its HANDLED_BY targets,
/// then the `external` rows for `external` (in order, no handlers). One pass
/// over the edges and one [`Locator`] per answer.
fn servers(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    hits: &[(NodeId, &'static str, Confidence)],
    external: &[(NodeId, Confidence, Vec<String>)],
) -> Vec<Server> {
    if hits.is_empty() && external.is_empty() {
        return Vec::new();
    }
    let wanted: HashSet<NodeId> = hits.iter().map(|(id, _, _)| *id).collect();
    let mut handlers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for e in merged.all_edges() {
        if e.category == edge_category::HANDLED_BY && wanted.contains(&e.from) {
            let list = handlers.entry(e.from).or_default();
            if !list.contains(&e.to) {
                list.push(e.to);
            }
        }
    }
    let loc = Locator::new(merged);
    let mut rows: Vec<Server> = hits
        .iter()
        .map(|&(id, tier, confidence)| {
            let at = loc.locate(id);
            Server {
                id: at.id,
                qname: at.qname,
                kind: at.kind,
                file: at.file,
                line: at.line,
                live: live.contains(&id),
                r#match: tier,
                confidence: confidence_name(confidence),
                handlers: handlers
                    .get(&id)
                    .map(|hs| hs.iter().map(|h| loc.locate(*h)).collect())
                    .unwrap_or_default(),
                external_hosts: Vec::new(),
            }
        })
        .collect();
    rows.extend(external.iter().map(|(id, confidence, hosts)| {
        let at = loc.locate(*id);
        Server {
            id: at.id,
            qname: at.qname,
            kind: at.kind,
            file: at.file,
            line: at.line,
            live: live.contains(id),
            r#match: EXTERNAL,
            confidence: confidence_name(*confidence),
            handlers: Vec::new(),
            external_hosts: hosts.clone(),
        }
    }));
    live_marker("serves", rows.len(), rows.iter().filter(|r| r.live).count());
    rows
}

fn qname_of(merged: &MergedGraph, id: NodeId) -> Option<String> {
    merged
        .graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
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

    #[test]
    fn auto_reads_a_verb_or_a_slash_as_http() {
        let m = |c| mechanism_of(c, "auto").expect("auto");
        assert_eq!(m("POST /orders"), Mechanism::Http);
        assert_eq!(m("post  /orders"), Mechanism::Http);
        assert_eq!(m("/orders"), Mechanism::Http);
        assert_eq!(m("orders"), Mechanism::Queue);
        assert_eq!(m("orders.created"), Mechanism::Queue);
        // A verb alone is a topic name, not a request.
        assert_eq!(m("delete"), Mechanism::Queue);
        assert_eq!(mechanism_of("orders", "HTTP"), Ok(Mechanism::Http));
        let err = mechanism_of("x", "smtp").expect_err("unknown");
        assert_eq!(err, "unknown mechanism 'smtp'; valid: auto, http, queue");
    }

    #[test]
    fn request_path_reads_a_url_and_drops_the_query() {
        assert_eq!(request_path("/orders?x=1#top"), "/orders");
        assert_eq!(
            request_path("https://api.example.com/v1/orders"),
            "/v1/orders"
        );
        assert_eq!(request_path("https://api.example.com"), "/");
        assert_eq!(request_path("orders/{id}"), "/orders/{id}");
    }

    #[test]
    fn request_host_reads_the_authority() {
        assert_eq!(
            request_host("https://u@Api.Example.org:8443/x?q").as_deref(),
            Some("api.example.org")
        );
        assert_eq!(
            request_host("http://api.example.org.").as_deref(),
            Some("api.example.org")
        );
        assert_eq!(request_host("/x"), None);
        assert_eq!(request_host("orders/{id}"), None);
        assert_eq!(request_host("https:///x"), None);
        // A non-numeric suffix is no port.
        assert_eq!(host_name("svc:http"), "svc:http");
    }

    fn cell(kind: glia_core::CellTypeId, json: &str) -> Cell {
        Cell {
            kind,
            payload: CellPayload::Json(json.to_string()),
        }
    }

    #[test]
    fn external_hosts_reads_the_origin_verdict() {
        let origin = cell(cell_type::ORIGIN, r#"{"provenance":"external"}"#);
        let site = |host: &str, external: bool| {
            let mark = if external { r#","external":true"# } else { "" };
            cell(
                cell_type::ENDPOINT_HIT,
                &format!(r#"{{"method":"GET","path":"/search","host":"{host}"{mark}}}"#),
            )
        };
        // Two entries of one id: hosts deduped and sorted, unmarked sites skipped.
        let cells = [
            site("z.example.org", true),
            origin.clone(),
            site("a.example.org", true),
            site("z.example.org", true),
            site("in.example.org", false),
        ];
        assert_eq!(
            external_hosts(&cells),
            Some(vec![
                "a.example.org".to_string(),
                "z.example.org".to_string()
            ])
        );
        // Marked sites without the ORIGIN stamp: paired, so in-repo.
        assert_eq!(external_hosts(&[site("a.example.org", true)]), None);
        // Another provenance is not external.
        let fixture = cell(cell_type::ORIGIN, r#"{"provenance":"test_fixture"}"#);
        assert_eq!(
            external_hosts(&[fixture, site("a.example.org", true)]),
            None
        );
        assert_eq!(external_hosts(&[origin]), Some(Vec::new()));
    }

    #[test]
    fn parent_path_stops_above_the_root() {
        assert_eq!(parent_path("/orders/{id}"), Some("/orders"));
        assert_eq!(parent_path("/orders/{id}/"), Some("/orders"));
        assert_eq!(parent_path("/orders"), None);
        assert_eq!(parent_path("/"), None);
    }

    #[test]
    fn topics_fold_case_and_separators_only() {
        assert_eq!(fold_topic("Orders_Created"), fold_topic("orders.created"));
        assert_eq!(fold_topic("orders-created"), fold_topic("orders:created"));
        assert_ne!(fold_topic("orderscreated"), fold_topic("orders.created"));
    }
}
