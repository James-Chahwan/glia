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
//! — grep token `[serves] mechanism=`.
//!
//! Module slot declared by L0.2; its API is reached as
//! `glia_engine::serves::<item>`, never flattened into the crate root.

use std::collections::{HashMap, HashSet};

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::{edge_category, node_kind};
use glia_code_extractors::queues::is_framework_tag;
use glia_core::{Confidence, NodeId};
use glia_graph::{HttpRouteMatcher, MergedGraph};

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

/// One node that serves the channel, located, with the code it hands off to.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct Server {
    pub id: u64,
    pub qname: String,
    /// `ROUTE` or `QUEUE_CONSUMER`.
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based (LD.1's `Locator`).
    pub line: Option<i64>,
    /// Reachable from an entrypoint (LD.6): `false` = likely dead.
    pub live: bool,
    /// HTTP: the matcher tier that reached the route — `exact`,
    /// `endpoint_prefix`, `any` (the method-agnostic fallback) or
    /// `route_prefix`. Queue: always `exact` (literal topics only).
    pub r#match: &'static str,
    /// `strong` | `medium` | `weak`. HTTP: the route's own confidence capped
    /// at its tier's ceiling; queue: the consumer node's.
    pub confidence: &'static str,
    /// The HANDLED_BY targets of the server, in edge order, located. Empty
    /// when the build bound no handler (a module-level queue consumer, a
    /// handler-less framework route).
    pub handlers: Vec<Located>,
}

/// Who serves `channel`. `mechanism` is `auto`, `http` or `queue` (any case);
/// `auto` reads a channel whose first word is an HTTP verb (`POST /orders`)
/// or that starts with `/` as HTTP, anything else as a queue topic.
///
/// HTTP: `METHOD /path` is looked up under that verb; a bare `/path` under
/// GET, POST, PUT, PATCH and DELETE in that order, deduped by route (the
/// matcher's `ANY` tier still finds method-agnostic routes under each). A
/// full URL is read for its path. Queue: every `queue_consumer:<topic>` node
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
    let results = servers(merged, live, &hits);
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
    let results = servers(merged, live, &hits);
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

/// Located servers for `hits` (in order), each with its HANDLED_BY targets.
/// One pass over the edges and one [`Locator`] per answer.
fn servers(
    merged: &MergedGraph,
    live: &HashSet<NodeId>,
    hits: &[(NodeId, &'static str, Confidence)],
) -> Vec<Server> {
    if hits.is_empty() {
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
    let rows: Vec<Server> = hits
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
            }
        })
        .collect();
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
