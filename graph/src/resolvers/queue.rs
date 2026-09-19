//! Queue stack resolver — producer → consumer by topic name, gated on broker
//! family, with broker-dialect wildcard subscriptions (A2.7).

use std::collections::HashMap;
use std::str::Split;

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_code_extractors::queues::{QueueFramework, UNRESOLVED_PREFIX};
use glia_core::{Cell, CellPayload, Confidence, Edge, NodeId};

use super::{CrossGraphResolver, RuleTally, weakest};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

// ============================================================================
// QueueStackResolver — matches producer → consumer by topic name
// ============================================================================

pub struct QueueStackResolver;

impl CrossGraphResolver for QueueStackResolver {
    /// BREAKING (A2.3): a node whose topic is the framework-tag fallback
    /// (`queue_producer:unresolved:kafka`) is NOT joinable.
    ///
    /// The tag is minted by the extractor when no occurrence of a needle named a
    /// topic, so it carries no identity at all — it only says "this file talks to
    /// Kafka". Joining it like a real topic meant every repo whose topics failed
    /// to parse acquired a QUEUE_FLOWS edge to every OTHER such repo: an
    /// all-to-all false cross-service dependency, which `blast_radius` then
    /// traverses (QUEUE_FLOWS is a carry category) and `cross_stack_trace` labels
    /// as a real mechanism. The node is kept — it is a genuine coverage signal —
    /// but it is now structurally unpairable. Both sides are guarded.
    ///
    /// BREAKING (A2.7): the join is family-gated. A topic string is only an
    /// identity WITHIN a broker family — a Sidekiq `jobs` and a Kafka `jobs` are
    /// different queues — so a pair whose family sets are both known and
    /// disjoint is dropped (`family_mismatch`). A node with no family cell pairs
    /// as before. After the exact pass, wildcard subscribers (`orders.*`,
    /// `sensors/+/temp`, Kafka `^orders\..*`) are matched in their family's
    /// dialect; those edges are always `Weak` — a pattern match is an
    /// inference, not a literal identity. A subscription with no literal
    /// segment (`>`, `#`, `*.*`, `^.*`) pairs with nothing (`skipped_catchall`).
    ///
    /// fired_on marker (one line per build, only when a counter is non-zero):
    ///   `... 2>&1 | grep '^\[queues\] resolver paired='`
    fn resolve(&self, merged: &mut MergedGraph) {
        let index = build_queue_index(&merged.graphs);
        let mut skipped_unresolved = index.skipped_unresolved;
        let (mut paired, mut family_mismatch, mut wildcard) = (0usize, 0usize, 0usize);
        // LC.3c: `exact` for a consumer keyed on the producer's topic,
        // `pattern` for a compiled wildcard subscription that covers it.
        let mut rules = RuleTally::new("queue", &["exact", "pattern"]);
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::QUEUE_PRODUCER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                // LB.8: the owner segment names the producing project, not the
                // topic; every owner of a topic pairs every consumer of it.
                let Some(topic) = split_owner(qname).0.strip_prefix("queue_producer:") else {
                    continue;
                };
                if topic.starts_with(UNRESOLVED_PREFIX) {
                    skipped_unresolved += 1;
                    continue;
                }
                let families = node_families(&n.cells);
                for t in index.exact.get(topic).into_iter().flatten() {
                    if !families_compatible(&families, &t.families) {
                        family_mismatch += 1;
                        continue;
                    }
                    let confidence = weakest(n.confidence, t.confidence);
                    merged.cross_edges.push(
                        Edge::new(n.id, t.id, edge_category::QUEUE_FLOWS, confidence)
                            .with_cell(rules.cell("exact")),
                    );
                    paired += 1;
                }
                // Rare path: O(producers × wildcard consumers). A consumer whose
                // pattern string equals the topic was already decided above.
                for w in index.wildcard.iter().filter(|w| w.topic != topic) {
                    let mut hits = w.arms.iter().filter(|(_, p)| p.matches(w.topic, topic)).peekable();
                    if hits.peek().is_none() {
                        continue;
                    }
                    if !hits.any(|(fam, _)| families_compatible(&families, &[*fam])) {
                        family_mismatch += 1;
                        continue;
                    }
                    merged.cross_edges.push(
                        Edge::new(n.id, w.id, edge_category::QUEUE_FLOWS, Confidence::Weak)
                            .with_cell(rules.cell("pattern")),
                    );
                    paired += 1;
                    wildcard += 1;
                }
            }
        }
        let skipped_catchall = index.skipped_catchall;
        // One line per BUILD (not per file), and only when this resolver had
        // anything to say — the `[ws-resolve]` house style, so the fixtures with
        // no queue nodes at all stay silent. `wildcard` is a subset of `paired`.
        if paired + skipped_unresolved + family_mismatch + skipped_catchall > 0 {
            eprintln!(
                "[queues] resolver paired={paired} skipped_unresolved={skipped_unresolved} \
                 family_mismatch={family_mismatch} wildcard={wildcard} \
                 skipped_catchall={skipped_catchall}"
            );
        }
        rules.report();
    }
}

/// A family that pairs with every family. Nothing emits it today —
/// `QueueFramework::family()` gives Celery/Dramatiq/Oban their own tags — but
/// it is the documented escape hatch for a framework whose broker is
/// configuration rather than code.
const GENERIC_FAMILY: &str = "generic";

struct Consumer<'a> {
    id: NodeId,
    confidence: Confidence,
    families: Vec<&'a str>,
}

struct WildcardConsumer<'a> {
    id: NodeId,
    topic: &'a str,
    /// One arm per family under whose dialect `topic` is a pattern.
    arms: Vec<(&'a str, Pattern)>,
}

struct QueueIndex<'a> {
    exact: HashMap<&'a str, Vec<Consumer<'a>>>,
    wildcard: Vec<WildcardConsumer<'a>>,
    skipped_unresolved: usize,
    skipped_catchall: usize,
}

/// Topic → consumers (every resolvable consumer, so literal equality keeps
/// today's behaviour), plus the wildcard subscribers in a separate Vec so the
/// exact hot path stays one HashMap lookup.
fn build_queue_index(graphs: &[RepoGraph]) -> QueueIndex<'_> {
    let mut ix = QueueIndex {
        exact: HashMap::new(),
        wildcard: Vec::new(),
        skipped_unresolved: 0,
        skipped_catchall: 0,
    };
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::QUEUE_CONSUMER) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(topic) = split_owner(qname).0.strip_prefix("queue_consumer:") else {
                continue;
            };
            if topic.starts_with(UNRESOLVED_PREFIX) {
                ix.skipped_unresolved += 1;
                continue;
            }
            let families = node_families(&n.cells);
            let mut arms = Vec::new();
            let mut catchall = false;
            for fam in &families {
                match compile_pattern(topic, fam) {
                    Compiled::Literal => {}
                    Compiled::Catchall => catchall = true,
                    Compiled::Pattern(p) => arms.push((*fam, p)),
                }
            }
            ix.skipped_catchall += usize::from(catchall);
            if !arms.is_empty() {
                ix.wildcard.push(WildcardConsumer { id: n.id, topic, arms });
            }
            ix.exact.entry(topic).or_default().push(Consumer {
                id: n.id,
                confidence: n.confidence,
                families,
            });
        }
    }
    ix
}

/// Every distinct `family` named by the node's A2.8 CODE cells. A topic used
/// from N files carries N cells, possibly of different frameworks, so this is
/// a set. Any cell that does not parse contributes nothing (= family absent).
fn node_families(cells: &[Cell]) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for c in cells {
        let (true, CellPayload::Json(json)) = (c.kind == cell_type::CODE, &c.payload) else {
            continue;
        };
        if let Some(f) = family_field(json).filter(|f| !out.contains(f)) {
            out.push(f);
        }
    }
    out
}

/// Tight scan for `"family":"<tag>"` — the payload is written by the
/// extractors crate (`queues::finish`), not user JSON, so this keeps
/// serde_json out of the graph crate (same call as `http::extract_method_field`).
fn family_field(json: &str) -> Option<&str> {
    let rest = &json[json.find("\"family\":\"")? + "\"family\":\"".len()..];
    let tag = &rest[..rest.find('"')?];
    (!tag.is_empty()).then_some(tag)
}

/// Pair when either side is unknown or generic, or the sets intersect.
fn families_compatible(a: &[&str], b: &[&str]) -> bool {
    a.is_empty()
        || b.is_empty()
        || a.contains(&GENERIC_FAMILY)
        || b.contains(&GENERIC_FAMILY)
        || a.iter().any(|f| b.contains(f))
}

// ---- wildcard dialects ----------------------------------------------------

/// Token syntax of a segmented subscription.
#[derive(Clone, Copy)]
struct Segmented {
    sep: char,
    /// Exactly one token.
    one: &'static str,
    /// This token and every following one (NATS / ActiveMQ `>`); must be last.
    rest_some: Option<&'static str>,
    /// Zero or more tokens, anywhere (AMQP / Artemis / MQTT `#`).
    rest_any: &'static str,
}

impl Segmented {
    fn is_wild(&self, seg: &str) -> bool {
        seg == self.one || seg == self.rest_any || Some(seg) == self.rest_some
    }
}

/// NATS, RabbitMQ topic exchanges, JMS (ActiveMQ `>` and Artemis `#`).
const DOTTED: Segmented = Segmented { sep: '.', one: "*", rest_some: Some(">"), rest_any: "#" };
/// MQTT topic filters.
const SLASHED: Segmented = Segmented { sep: '/', one: "+", rest_some: None, rest_any: "#" };

enum Pattern {
    Segments(Segmented),
    /// Kafka `subscribe(Pattern)`: the regex's literal prefix. `exact` when the
    /// whole regex was literal. No regex dependency — an over-approximation,
    /// which is why every wildcard edge is Weak.
    Prefix { prefix: String, exact: bool },
}

enum Compiled {
    Literal,
    Catchall,
    Pattern(Pattern),
}

impl Pattern {
    /// Does the concrete `topic` fall under the subscription `pattern` (the
    /// consumer's own topic string, which this arm was compiled from)?
    fn matches(&self, pattern: &str, topic: &str) -> bool {
        match self {
            Pattern::Segments(syn) => seg_walk(topic.split(syn.sep), pattern.split(syn.sep), *syn),
            Pattern::Prefix { prefix, exact: true } => topic == prefix,
            Pattern::Prefix { prefix, exact: false } => topic.starts_with(prefix.as_str()),
        }
    }
}

/// Classify `topic` in the dialect of `family` (families are named through
/// `QueueFramework::family()`, the single source of truth — never a literal).
fn compile_pattern(topic: &str, family: &str) -> Compiled {
    let is = |f: QueueFramework| f.family() == family;
    if is(QueueFramework::Kafka) {
        // Kafka topics are `[A-Za-z0-9._-]`, so a regex shows itself by an
        // anchor or a metacharacter no topic can hold. `$` and `{}` alone stay
        // literal: they are far more often an unexpanded `${env}` template.
        if !topic.starts_with('^') && !topic.contains(['*', '+', '?', '[', '(', '|', '\\']) {
            return Compiled::Literal;
        }
        return match kafka_literal_prefix(topic) {
            (p, _) if p.is_empty() => Compiled::Catchall,
            (prefix, exact) => Compiled::Pattern(Pattern::Prefix { prefix, exact }),
        };
    }
    let syn = if is(QueueFramework::Mqtt) {
        SLASHED
    } else if [QueueFramework::Nats, QueueFramework::RabbitMQ, QueueFramework::Jms]
        .into_iter()
        .any(is)
    {
        DOTTED
    } else {
        return Compiled::Literal;
    };
    let mut segs = topic.split(syn.sep);
    if !segs.clone().any(|s| syn.is_wild(s)) {
        Compiled::Literal
    } else if !segs.any(|s| !s.is_empty() && !syn.is_wild(s)) {
        Compiled::Catchall
    } else {
        Compiled::Pattern(Pattern::Segments(syn))
    }
}

/// `^orders\..*` → ("orders.", false); `^orders$` → ("orders", true). Stops at
/// the first unescaped metacharacter; a literal made optional by the next char
/// (`s?`, `s*`, `s{0,}`) is not part of the required prefix. Alternation has
/// no prefix at all, so it is reported as a catch-all.
fn kafka_literal_prefix(pattern: &str) -> (String, bool) {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    let mut out = String::new();
    if body.contains('|') {
        return (out, false);
    }
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        let lit = match c {
            '\\' => match chars.next() {
                Some(e) if !e.is_ascii_alphanumeric() => e,
                _ => return (out, false), // `\d`, `\w`: a class
            },
            '$' if chars.peek().is_none() => return (out, true),
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' => {
                return (out, false);
            }
            c => c,
        };
        if matches!(chars.peek(), Some('*' | '?' | '{')) {
            return (out, false);
        }
        out.push(lit);
    }
    (out, true)
}

/// Segment walk, no allocation beyond `split`. `#` backtracks over the
/// remaining topic tokens; topics are a handful of tokens, so this is cheap.
fn seg_walk(mut t: Split<'_, char>, mut p: Split<'_, char>, syn: Segmented) -> bool {
    while let Some(ps) = p.next() {
        if ps == syn.rest_any {
            loop {
                if seg_walk(t.clone(), p.clone(), syn) {
                    return true;
                }
                if t.next().is_none() {
                    return false;
                }
            }
        }
        let Some(ts) = t.next() else { return false };
        if Some(ps) == syn.rest_some {
            return p.next().is_none();
        }
        if ps != syn.one && ps != ts {
            return false;
        }
    }
    t.next().is_none()
}

#[cfg(test)]
mod tests {
    use glia_code_domain::{edge_category, node_kind};

    use super::super::tests::{channel_graph, cross_pairs};
    use super::*;

    /// LB.8: two owners on each side of one topic pair all-to-all (2 x 2), and
    /// an owner-qualified framework tag stays unpairable.
    #[test]
    fn owner_qualified_sides_pair_by_the_bare_topic() {
        let g = channel_graph(
            "queue-owner",
            &[
                (node_kind::QUEUE_PRODUCER, "queue_producer:orders.created @services/orders"),
                (node_kind::QUEUE_PRODUCER, "queue_producer:orders.created @services/returns"),
                (node_kind::QUEUE_CONSUMER, "queue_consumer:orders.created @services/billing"),
                (node_kind::QUEUE_CONSUMER, "queue_consumer:orders.created @services/audit"),
                (node_kind::QUEUE_PRODUCER, "queue_producer:unresolved:kafka @services/orders"),
                (node_kind::QUEUE_CONSUMER, "queue_consumer:unresolved:kafka @services/audit"),
            ],
        );
        let mut m = MergedGraph::new(vec![g]);
        QueueStackResolver.resolve(&mut m);
        let p = |a: &str, b: &str| {
            (format!("queue_producer:orders.created @services/{a}"), format!("queue_consumer:orders.created @services/{b}"))
        };
        assert_eq!(
            cross_pairs(&m, edge_category::QUEUE_FLOWS),
            [p("orders", "audit"), p("orders", "billing"), p("returns", "audit"), p("returns", "billing")]
        );
    }
}
