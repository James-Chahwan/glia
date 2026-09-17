use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, NodeKindId, RepoId};

pub struct EventNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
}

/// (needle, extract_name, broker_ambiguous).
///
/// `broker_ambiguous` (A2.9) marks the verbs a message-broker client shares with
/// an in-process bus: `publish(`, `.subscribe(`, `.on(`. In a file that imports
/// a broker client ([`broker_present`]) those needles are skipped outright —
/// the call is broker traffic, and `queues.rs` owns it as QUEUE_* +
/// QUEUE_FLOWS. They used to mint Weak EVENT_* nodes joined by EVENT_FLOWS, so
/// `trace` labelled a Redis/MQTT/NATS hop an in-process event, and NATS/Kafka
/// producers double-emitted. The in-process-only verbs (`.emit(`,
/// `.dispatch(`, `dispatchEvent(`, `@OnEvent(` ...) are never flagged, and a
/// file with no broker signal behaves exactly as before.
const EMITTER_PATTERNS: &[(&str, bool, bool)] = &[
    (".emit(", true, false),
    (".dispatch(", true, false),
    ("Subject.next(", true, false),
    ("EventBridge.putEvents", false, false),
    ("eventBridge.putEvents", false, false),
    ("publish(", true, true),
    (".trigger(", true, false),
    ("dispatchEvent(", true, false),
];

const HANDLER_PATTERNS: &[(&str, bool, bool)] = &[
    (".on(", true, true),
    (".addEventListener(", true, false),
    (".subscribe(", true, true),
    ("@EventPattern(", true, false),
    ("@OnEvent(", true, false),
    ("handle_event", false, false),
    (".addListener(", true, false),
];

/// Buses keyed by MESSAGE TYPE rather than by a string topic. The captured
/// token is the type name, not a literal, so `extract_event_name`'s
/// quoted-literal rule can never see them: `publisher.publishEvent(new
/// OrderPlacedEvent(id))` matches no string needle at all (`publish(` wants
/// `(` straight after `publish`), and neither does `_mediator.Publish(new
/// OrderPlaced())`.
const TYPE_EMITTER_NEEDLES: &[&str] = &[
    "publishEvent(new ", // Spring ApplicationEventPublisher
    ".publish(new ",     // NestJS CQRS EventBus, MediatR (lowercase)
    ".Publish(new ",     // MediatR (C# casing)
    ".Send(new ",        // MediatR/Mediator request shapes
    ".send(new ",
];

/// Handler-side type needles. The `char` bounds the captured token — `>` for
/// the generic forms, `)` for the decorator forms. `None` is Spring's bare
/// `@EventListener`, whose event type is the annotated method's FIRST
/// PARAMETER rather than anything inside the annotation.
const TYPE_HANDLER_NEEDLES: &[(&str, Option<char>)] = &[
    ("INotificationHandler<", Some('>')), // MediatR: type inside <>
    ("IRequestHandler<", Some('>')),
    ("@EventsHandler(", Some(')')), // NestJS CQRS: type inside ()
    ("@EventListener", None),       // Spring: type is the handler's first param
    ("@TransactionalEventListener", None),
];

/// Call sites the QUEUE extractor claims even in a file that carries no broker
/// signal: `nc.publish(` is NATS and `channel.publish(` / `basic_publish(` are
/// RabbitMQ (queues.rs QUEUE_PRODUCER needles; `channel.basic_publish` has an
/// empty gate). Per call site, so it still matters where the file-level
/// [`broker_present`] gate does not fire. Everything else a broker publishes
/// through — MQTT, Redis pub/sub, SQS/SNS, Pub/Sub, Azure SB, Kafka — is
/// handled by that gate (A2.9), which is keyed on the queue rows' own library
/// signals rather than on a second hand-written list.
const QUEUE_OWNED_PUBLISH: &[&str] = &[
    "nc.publish(",
    "channel.publish(",
    "channel.basic_publish(",
    "basic_publish(",
];

/// Push one event node, deduplicated on the event name so a file matched by
/// both the type-keyed and the string-keyed pass emits one node, not two.
#[allow(clippy::too_many_arguments)]
fn push_event_node(
    nodes: &mut Vec<Node>,
    nav: &mut CodeNav,
    seen: &mut std::collections::HashSet<String>,
    event_name: &str,
    kind: NodeKindId,
    prefix: &str,
    confidence: Confidence,
    module_id: NodeId,
    repo: RepoId,
) {
    if !seen.insert(event_name.to_string()) {
        return;
    }
    let qname = format!("{prefix}{event_name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
    nodes.push(Node {
        id,
        repo,
        confidence,
        cells: vec![],
    });
    nav.record(id, event_name, &qname, kind, Some(module_id));
}

pub fn extract_event_emitter_nodes(source: &str, module_id: NodeId, repo: RepoId) -> EventNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();

    // Type-keyed FIRST: it is stronger evidence (a real type name, not a
    // framework tag), so when both passes see the same event the Medium
    // confidence is the one that lands.
    for name in scan_type_needles(source, TYPE_EMITTER_NEEDLES.iter().map(|n| (*n, None))) {
        push_event_node(
            &mut nodes,
            &mut nav,
            &mut seen,
            &name,
            node_kind::EVENT_EMITTER,
            "event_emit:",
            Confidence::Medium,
            module_id,
            repo,
        );
    }

    let mut broker = BrokerGate::default();
    for &(pattern, extract_name, ambiguous) in EMITTER_PATTERNS {
        let Some(idx) = find_needle(source, pattern) else {
            continue;
        };
        let event_name = event_name_at(source, pattern, idx, extract_name);
        if ambiguous && broker.present(source) {
            suppressed("emitter", pattern, &event_name);
            continue;
        }

        push_event_node(
            &mut nodes,
            &mut nav,
            &mut seen,
            &event_name,
            node_kind::EVENT_EMITTER,
            "event_emit:",
            Confidence::Weak,
            module_id,
            repo,
        );
    }

    EventNodes { nodes, nav }
}

pub fn extract_event_handler_nodes(source: &str, module_id: NodeId, repo: RepoId) -> EventNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();

    for name in scan_type_needles(source, TYPE_HANDLER_NEEDLES.iter().copied()) {
        push_event_node(
            &mut nodes,
            &mut nav,
            &mut seen,
            &name,
            node_kind::EVENT_HANDLER,
            "event_handle:",
            Confidence::Medium,
            module_id,
            repo,
        );
    }

    let mut broker = BrokerGate::default();
    for &(pattern, extract_name, ambiguous) in HANDLER_PATTERNS {
        let Some(idx) = find_needle(source, pattern) else {
            continue;
        };
        let event_name = event_name_at(source, pattern, idx, extract_name);
        if ambiguous && broker.present(source) {
            suppressed("handler", pattern, &event_name);
            continue;
        }

        push_event_node(
            &mut nodes,
            &mut nav,
            &mut seen,
            &event_name,
            node_kind::EVENT_HANDLER,
            "event_handle:",
            Confidence::Weak,
            module_id,
            repo,
        );
    }

    EventNodes { nodes, nav }
}

/// The event a string-keyed needle names: the quoted literal after it, else
/// the needle word itself (`publish`, `subscribe`).
fn event_name_at(source: &str, pattern: &str, idx: usize, extract_name: bool) -> String {
    if extract_name {
        extract_event_name(source, idx + pattern.len())
    } else {
        None
    }
    .unwrap_or_else(|| pattern.trim_matches('.').trim_end_matches('(').to_string())
}

/// A2.9: does this file import a message-broker client? Computed lazily, once
/// per extract call, and only when a broker-ambiguous needle actually matched
/// — so a file with no `publish(` / `.subscribe(` / `.on(` never pays for the
/// lowercase copy.
#[derive(Default)]
struct BrokerGate(Option<bool>);

impl BrokerGate {
    fn present(&mut self, source: &str) -> bool {
        *self
            .0
            .get_or_insert_with(|| broker_present(&source.to_ascii_lowercase()))
    }
}

/// True when `lower_source` carries any library signal that gates a BROKER row
/// in queues.rs (task-queue rows excluded). The list is DERIVED from those
/// rows, never re-typed here: a broker the queue extractor learns to gate is
/// suppressed on the event side in the same edit, and the two files cannot
/// disagree about which library makes a verb broker traffic.
///
/// Deliberately NOT the bare word `pubsub`: graphql-subscriptions' `PubSub`
/// is an in-process bus (`pubsub.publish('POST_ADDED', ...)`). Google Pub/Sub
/// is recognised by its package (`@google-cloud/pubsub`, `google.cloud`).
///
/// Accepted trade: a file that imports a broker client AND uses an in-process
/// emitter loses that emitter's `.on(` / `.subscribe(` / `publish(` nodes.
fn broker_present(lower_source: &str) -> bool {
    crate::queues::broker_signal_present(lower_source)
}

/// fired_on marker (A2.9). It lives here but speaks in the `[queues]`
/// namespace on purpose — that is where the suppressed traffic went, and the
/// queue-side `[queues] needle '...'` line is its other half:
///   `GLIA_QUEUE_DEBUG=1 ... 2>&1 | grep '\[queues\] broker-event suppressed'`
/// The extractor is called without a path, so the line names the event, not
/// the file; the paired `[queues] needle` line carries `file=`.
fn suppressed(side: &str, needle: &str, name: &str) {
    if crate::queues::debug_enabled() {
        eprintln!("[queues] broker-event suppressed {side}='{needle}' name={name}");
    }
}

/// First occurrence of `pattern` that is not a call the queue extractor owns.
fn find_needle(source: &str, pattern: &str) -> Option<usize> {
    let mut from = 0usize;
    while let Some(rel) = source[from..].find(pattern) {
        let at = from + rel;
        if pattern != "publish(" || !queue_owned_publish(source, at) {
            return Some(at);
        }
        from = at + pattern.len();
    }
    None
}

fn queue_owned_publish(source: &str, at: usize) -> bool {
    let end = at + "publish(".len();
    QUEUE_OWNED_PUBLISH
        .iter()
        .any(|owned| source[..end].ends_with(owned))
}

/// Every occurrence of every needle, in needle order. The string-keyed path
/// calls `find` ONCE per needle, so a file publishing three event types
/// contributed one node; the type-keyed pass walks the whole file.
fn scan_type_needles<'a>(
    source: &str,
    needles: impl Iterator<Item = (&'a str, Option<char>)>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (needle, close) in needles {
        let mut from = 0usize;
        while let Some(rel) = source[from..].find(needle) {
            let at = from + rel + needle.len();
            let token = match close {
                Some(c) => extract_type_token(&source[at..], Some(c)),
                // A bracketed needle bounds its own token; a bare annotation
                // does not, and Spring's does not name the type at all.
                None if needle.starts_with('@') => extract_listener_param_type(&source[at..]),
                None => extract_type_token(&source[at..], None),
            };
            if let Some(token) = token {
                out.push(token);
            }
            from = at;
        }
    }
    out
}

/// The type token following a type-keyed needle. `close` bounds a bracketed
/// form (`<Type>` / `(Type)`); `None` reads the identifier run and stops at the
/// first delimiter. Namespaces and generic arguments fall away, so
/// `Shop.Events.OrderPlaced`, `shop::events::OrderPlaced` and
/// `OrderPlaced<Guid>` all read `OrderPlaced`.
fn extract_type_token(after: &str, close: Option<char>) -> Option<String> {
    let trimmed = after.trim_start();
    let end = match close {
        Some(c) => trimmed.find(|ch: char| ch == c || ch == ',')?,
        None => trimmed
            .find(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '.' || ch == ':'))
            .unwrap_or(trimmed.len()),
    };
    let mut raw = trimmed[..end].trim();
    raw = raw.strip_suffix(".class").unwrap_or(raw); // Java `OrderPlaced.class`
    raw = raw.rsplit("::").next().unwrap_or(raw);
    raw = raw.rsplit('.').next().unwrap_or(raw);
    raw = raw.split('<').next().unwrap_or(raw);
    let token = raw.trim();
    // Uppercase-initial plain identifier ONLY. This is a type needle, so a
    // lowercase capture means we read an argument name or a keyword
    // (`@EventListener(condition = "...")`), not an event type. The same guard
    // is what keeps the resolver's fold off string topics.
    if token.is_empty()
        || token.len() > 128
        || !token.starts_with(|c: char| c.is_uppercase())
        || !token.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(token.to_string())
}

/// Spring's `@EventListener` names no type: the event is the annotated
/// method's FIRST PARAMETER. `@EventListener(OrderPlaced.class)` names it
/// directly; the bare form needs the next signature, bounded to a few lines so
/// a dangling annotation captures nothing.
fn extract_listener_param_type(after: &str) -> Option<String> {
    let rest = if after.starts_with('(') {
        if let Some(token) = extract_type_token(&after[1..], Some(')')) {
            return Some(token);
        }
        match after.find(')') {
            Some(i) => &after[i + 1..],
            None => after,
        }
    } else {
        after
    };
    let window: String = rest.lines().take(4).collect::<Vec<_>>().join("\n");
    let open = window.find('(')?;
    let first = window[open + 1..].split([',', ')']).next()?;
    extract_type_token(first.split_whitespace().next()?, None)
}

/// `at` is the byte offset just past the matched needle.
fn extract_event_name(source: &str, at: usize) -> Option<String> {
    let after = source.get(at..)?;
    let trimmed = after.trim_start();
    let (quote, rest) = if let Some(rest) = trimmed.strip_prefix('\'') {
        ('\'', rest)
    } else if let Some(rest) = trimmed.strip_prefix('"') {
        ('"', rest)
    } else {
        return None;
    };
    let end = rest.find(quote)?;
    let lit = &rest[..end];
    if lit.is_empty() || lit.len() > 128 {
        return None;
    }
    Some(lit.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }
    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test")
    }

    #[test]
    fn detects_emit() {
        let source = "emitter.emit('user.created', data);";
        let result = extract_event_emitter_nodes(source, module_id(), repo());
        assert!(!result.nodes.is_empty());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_emit:user.created")
        );
    }

    #[test]
    fn detects_handler() {
        let source = "emitter.on('user.created', handler);";
        let result = extract_event_handler_nodes(source, module_id(), repo());
        assert!(!result.nodes.is_empty());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_handle:user.created")
        );
    }

    #[test]
    fn detects_nest_event_pattern() {
        let source = "@EventPattern('order.placed')\nasync handleOrder(data) {}";
        let result = extract_event_handler_nodes(source, module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_handle:order.placed")
        );
    }

    #[test]
    fn detects_spring_publish_event_type() {
        let source = "publisher.publishEvent(new OrderPlacedEvent(id));";
        let result = extract_event_emitter_nodes(source, module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_emit:OrderPlacedEvent"),
            "{:?}",
            result.nav.qname_by_id
        );
        assert!(
            result
                .nodes
                .iter()
                .all(|n| n.confidence == Confidence::Medium)
        );
    }

    #[test]
    fn detects_spring_event_listener_param_type() {
        let source = "@EventListener\npublic void onOrderPlaced(OrderPlacedEvent event) {}";
        let result = extract_event_handler_nodes(source, module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_handle:OrderPlacedEvent"),
            "{:?}",
            result.nav.qname_by_id
        );
    }

    #[test]
    fn detects_mediatr_notification_handler() {
        let source = "public class Emailer : INotificationHandler<OrderPlaced>\n{\n}";
        let result = extract_event_handler_nodes(source, module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_handle:OrderPlaced"),
            "{:?}",
            result.nav.qname_by_id
        );
    }

    #[test]
    fn detects_nest_events_handler_decorator() {
        let source = "@EventsHandler(OrderPlacedEvent)\nexport class H {}";
        let result = extract_event_handler_nodes(source, module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_handle:OrderPlacedEvent"),
            "{:?}",
            result.nav.qname_by_id
        );
    }

    #[test]
    fn type_scan_is_multi_occurrence_and_strips_namespaces() {
        // The string-keyed path calls `find` once per needle; this one walks
        // the whole file, and `Shop.Events.X` reduces to `X`.
        let source = "_mediator.Publish(new Shop.Events.OrderPlaced());\n                      _mediator.Publish(new Shop.Events.OrderShipped());";
        let result = extract_event_emitter_nodes(source, module_id(), repo());
        let mut names: Vec<_> = result.nav.qname_by_id.values().cloned().collect();
        names.sort();
        assert_eq!(
            names,
            vec!["event_emit:OrderPlaced", "event_emit:OrderShipped"]
        );
    }

    #[test]
    fn type_needle_rejects_lowercase_and_keyword_captures() {
        // `condition` is an annotation argument, not an event type.
        let source = "@EventListener(condition = \"#e.ok\")\npublic void on(int n) {}";
        let result = extract_event_handler_nodes(source, module_id(), repo());
        assert!(
            !result
                .nav
                .qname_by_id
                .values()
                .any(|q| q.starts_with("event_handle:c")),
            "{:?}",
            result.nav.qname_by_id
        );
    }

    #[test]
    fn queue_owned_publish_does_not_double_emit() {
        // `nc.publish("orders", p)` is a NATS QUEUE_PRODUCER; the generic
        // `publish(` needle must not mint a phantom EVENT_EMITTER beside it.
        let source = "func pub(nc conn, p []byte) { nc.publish(\"orders\", p) }";
        let result = extract_event_emitter_nodes(source, module_id(), repo());
        assert!(
            result.nodes.is_empty(),
            "expected no event node, got {:?}",
            result.nav.qname_by_id
        );

        // An in-process bus that happens to use the same verb still fires.
        let bus = "bus.publish(\"user.created\", u);";
        let result = extract_event_emitter_nodes(bus, module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_emit:user.created")
        );
    }

    // ---- A2.9: broker pub/sub leaves the event bus -------------------------

    fn emitted(source: &str) -> Vec<String> {
        let mut v: Vec<String> = extract_event_emitter_nodes(source, module_id(), repo())
            .nav
            .qname_by_id
            .into_values()
            .collect();
        v.sort();
        v
    }

    fn handled(source: &str) -> Vec<String> {
        let mut v: Vec<String> = extract_event_handler_nodes(source, module_id(), repo())
            .nav
            .qname_by_id
            .into_values()
            .collect();
        v.sort();
        v
    }

    #[test]
    fn redis_publish_is_not_an_event_emitter() {
        let publisher = "import redis\nr = redis.Redis()\nr.publish('notifications', payload)\n";
        assert!(emitted(publisher).is_empty(), "{:?}", emitted(publisher));
        let subscriber = "import redis\np = redis.Redis().pubsub()\np.subscribe('notifications')\n";
        assert!(handled(subscriber).is_empty(), "{:?}", handled(subscriber));
    }

    #[test]
    fn plain_emitter_still_emits_without_broker_signal() {
        // Regression guard: the same verbs with no broker import are an
        // in-process bus, exactly as before A2.9.
        assert_eq!(
            emitted("bus.publish('user.created', u);"),
            vec!["event_emit:user.created"]
        );
        assert_eq!(
            handled("bus.subscribe('user.created', h);"),
            vec!["event_handle:user.created"]
        );
        assert_eq!(
            handled("emitter.on('user.created', h);"),
            vec!["event_handle:user.created"]
        );
    }

    #[test]
    fn broker_suppression_covers_every_measured_shape() {
        // mqtt.js, both sides, and its `client.on('connect')` lifecycle hook.
        let mqtt_sub = "import mqtt from 'mqtt';\nclient.on('connect', () => { client.subscribe('sensors/temp'); });";
        assert!(handled(mqtt_sub).is_empty(), "{:?}", handled(mqtt_sub));
        assert!(emitted("import mqtt from 'mqtt';\nclient.publish('sensors/temp', v);").is_empty());
        // kafkajs object argument: used to mint the method-named `event_handle:subscribe`.
        let kafkajs =
            "import { Kafka } from 'kafkajs';\nawait consumer.subscribe({ topic: 'orders' });";
        assert!(handled(kafkajs).is_empty(), "{:?}", handled(kafkajs));
        // Spring: a method DECLARATION `publish(` in a broker file.
        let spring = "import org.springframework.kafka.core.KafkaTemplate;\npublic void publish(String p) { kafkaTemplate.send(\"orders\", p); }";
        assert!(emitted(spring).is_empty(), "{:?}", emitted(spring));
        // AWS SDK v2 SQS (the java/sqs_sns cell) and SNS via boto3.
        let sqs = "import software.amazon.awssdk.services.sqs.SqsClient;\npublic void publish(String p) {}";
        assert!(emitted(sqs).is_empty(), "{:?}", emitted(sqs));
        assert!(emitted("import boto3\nsns.publish(TopicArn=arn, Message=m)").is_empty());
        // Google Pub/Sub by package, both sides.
        let gcp = "from google.cloud import pubsub_v1\npublisher.publish(topic_path, data)\nsubscriber.subscribe(path, callback=cb)";
        assert!(emitted(gcp).is_empty() && handled(gcp).is_empty());
    }

    #[test]
    fn in_process_verbs_and_buses_are_never_suppressed() {
        // `.emit(` is not broker-ambiguous: kept even in a broker file.
        assert_eq!(
            emitted("import redis\nemitter.emit('cache.flushed', k);"),
            vec!["event_emit:cache.flushed"]
        );
        // graphql-subscriptions' `PubSub` is an in-process bus; the bare word
        // `pubsub` is deliberately not a broker signal.
        assert_eq!(
            emitted(
                "import { PubSub } from 'graphql-subscriptions';\npubsub.publish('POST_ADDED', p);"
            ),
            vec!["event_emit:POST_ADDED"]
        );
        // A task-queue library is not a broker: BullMQ workers are EventEmitters.
        assert_eq!(
            handled("import { Worker } from 'bullmq';\nworker.on('completed', done);"),
            vec!["event_handle:completed"]
        );
        // Type-keyed buses (NestJS CQRS) stay, broker import or not.
        assert_eq!(
            emitted(
                "import { Kafka } from 'kafkajs';\nthis.eventBus.publish(new OrderPlaced(id));"
            ),
            vec!["event_emit:OrderPlaced"]
        );
    }
}
