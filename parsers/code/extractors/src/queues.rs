use std::sync::OnceLock;

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, RepoId};

use crate::queue_topic::{self, TopicRule};

pub struct QueueConsumer {
    pub from: NodeId,
    pub framework: QueueFramework,
    pub identifier: String,
}

/// Qname marker for the identity-free framework-tag fallback:
/// `queue_producer:unresolved:kafka`, never `queue_producer:kafka`.
///
/// A node carrying this prefix is a COVERAGE SIGNAL — "this file talks to
/// Kafka, topic unknown" — and NEVER an identity. That distinction is the whole
/// point: two unrelated services whose topics both failed to parse used to fall
/// back to the same bare `kafka` tag, and `QueueStackResolver` joined tag to tag
/// exactly as if it were a real topic, so every such repo got a QUEUE_FLOWS edge
/// to every other such repo. `blast_radius` traverses those edges and
/// `cross_stack_trace` labels them as a real mechanism, so the false dependency
/// propagated into answers.
///
/// `graph` references THIS constant rather than a second copy of the literal —
/// the extractor and the resolver must never be able to disagree about the
/// spelling, because the failure mode of a disagreement is silent (the resolver
/// simply resumes pairing tags).
pub const UNRESOLVED_PREFIX: &str = "unresolved:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueFramework {
    Celery,
    Dramatiq,
    BullMQ,
    Sidekiq,
    Oban,
    Nats,
    RabbitMQ,
    Kafka,
    /// Raw Redis lists used as a queue (LPUSH/RPUSH ↔ BLPOP/BRPOP). Common in
    /// polyglot demos like dockersamples/example-voting-app and any "ad-hoc
    /// worker queue without a framework" pattern.
    RedisList,
    // --- pre-allocated for the rest of batch A2; no table rows yet, so these
    // --- are inert until A2.4 / A2.6 / A2.9 add needles for them.
    /// AWS SQS (`sqs.sendMessage`, `SendMessageRequest`, ...).
    Sqs,
    /// AWS SNS fan-out (`sns.publish`, `PublishRequest`, ...).
    Sns,
    /// Google Cloud Pub/Sub (`topic.publish`, `subscription.on`, ...).
    PubSub,
    /// Azure Service Bus (`ServiceBusSender`, `ServiceBusProcessor`, ...).
    AzureServiceBus,
    /// MQTT brokers (`client.publish`, `client.subscribe` over mqtt.js/paho).
    Mqtt,
    /// Redis pub/sub proper (PUBLISH/SUBSCRIBE), distinct from `RedisList`.
    RedisPubSub,
    /// JMS / ActiveMQ / Artemis (`jmsTemplate.convertAndSend`, `@JmsListener`).
    Jms,
}

impl QueueFramework {
    /// Broker family, for grouping frameworks that share a wire identity (an
    /// SQS queue URL and an SQS ARN name the same queue). Stable lowercase
    /// tags — also what [`framework_tag`] falls back to.
    pub fn family(&self) -> &'static str {
        match self {
            QueueFramework::Celery => "celery",
            QueueFramework::Dramatiq => "dramatiq",
            QueueFramework::BullMQ => "bullmq",
            QueueFramework::Sidekiq => "sidekiq",
            QueueFramework::Oban => "oban",
            QueueFramework::Nats => "nats",
            QueueFramework::RabbitMQ => "rabbitmq",
            QueueFramework::Kafka => "kafka",
            QueueFramework::RedisList | QueueFramework::RedisPubSub => "redis",
            QueueFramework::Sqs => "sqs",
            QueueFramework::Sns => "sns",
            QueueFramework::PubSub => "pubsub",
            QueueFramework::AzureServiceBus => "azureservicebus",
            QueueFramework::Mqtt => "mqtt",
            QueueFramework::Jms => "jms",
        }
    }
}

pub fn extract_queue_consumers(source: &str, from: NodeId) -> Vec<QueueConsumer> {
    let mut consumers = Vec::new();
    // One allocation per call, reused by every gate below — see
    // [`signals_present`] for why the gate reads a lowercased copy.
    let lower = source.to_ascii_lowercase();
    for (pattern, framework, signals, _rule) in CONSUMER_PATTERNS {
        if source.contains(pattern) && signals_present(&lower, signals) {
            consumers.push(QueueConsumer {
                from,
                framework: framework.clone(),
                identifier: pattern.to_string(),
            });
        }
    }
    consumers
}

/// (needle, framework, framework-presence signals, topic rule). The signals list gates
/// emission: if NONE of the substrings appears in the same source file
/// (case-INSENSITIVELY — every signal literal here MUST be lowercase, see
/// [`signals_present`]), the pattern is skipped — this stops e.g. Express `res.send('Hello')` from
/// being mis-classified as a Dramatiq producer. An empty signals list means
/// the needle is unique enough to stand alone (Sidekiq's `perform_async`,
/// `Oban.insert`, etc.).
///
/// The 4th element says HOW that needle spells its topic — see
/// [`crate::queue_topic::TopicRule`]. `ArgLiteral` is the historical behaviour
/// (first quoted literal in positional arg #0); `KeyedOrArg` additionally reads
/// the kafkajs object form `send({ topic: 'x' })` and the Go struct field
/// `kafka.Message{Topic: "x"}`, which the old scanner could not see at all.
const CONSUMER_PATTERNS: &[(&str, QueueFramework, &[&str], TopicRule)] = &[
    ("@celery.task", QueueFramework::Celery, &[], TopicRule::ArgLiteral),
    ("@shared_task", QueueFramework::Celery, &[], TopicRule::ArgLiteral),
    ("@dramatiq.actor", QueueFramework::Dramatiq, &[], TopicRule::ArgLiteral),
    // `new Worker(` is a common JS shape (web workers, BullMQ, etc.); require
    // BullMQ presence.
    ("new Worker(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"], TopicRule::ArgLiteral),
    ("BullModule", QueueFramework::BullMQ, &[], TopicRule::ArgLiteral),
    ("include Sidekiq::Worker", QueueFramework::Sidekiq, &[], TopicRule::ArgLiteral),
    ("include Sidekiq::Job", QueueFramework::Sidekiq, &[], TopicRule::ArgLiteral),
    ("use Oban.Worker", QueueFramework::Oban, &[], TopicRule::ArgLiteral),
    ("use Oban.Pro.Worker", QueueFramework::Oban, &[], TopicRule::ArgLiteral),
    // `nc.subscribe` collides with Backbone events / Redis pubsub vars; gate.
    ("nc.subscribe", QueueFramework::Nats, &["nats"], TopicRule::ArgLiteral),
    // Go's nats.go exports Capitalized APIs (`nc.Subscribe`, `nc.QueueSubscribe`);
    // the lowercase JS needles never match Go source, so Go queues went blind.
    ("nc.Subscribe", QueueFramework::Nats, &["nats"], TopicRule::ArgLiteral),
    ("nc.QueueSubscribe", QueueFramework::Nats, &["nats"], TopicRule::ArgLiteral),
    ("channel.consume", QueueFramework::RabbitMQ, &["amqp", "amqplib", "rabbitmq"], TopicRule::ArgLiteral),
    ("KafkaConsumer", QueueFramework::Kafka, &[], TopicRule::ArgLiteral),
    // `consumer.subscribe` is generic; require kafka library presence.
    ("consumer.subscribe", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::KeyedOrArg(&["topic"])),
    // Go Kafka consumers: segmentio `reader.ReadMessage`, confluent `consumer.ReadMessage`.
    ("reader.ReadMessage", QueueFramework::Kafka, &["kafka", "segmentio"], TopicRule::ArgLiteral),
    ("consumer.ReadMessage", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::ArgLiteral),
    // Redis-as-queue consumer side. BLPOP/BRPOP block until message; LPOP/RPOP
    // are non-blocking pops. .NET driver uses ListLeftPop/ListRightPop.
    (".blpop(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    (".brpop(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    (".lpop(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    (".rpop(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    ("ListLeftPop(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListLeftPopAsync(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListRightPop(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListRightPopAsync(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    // ---- A2.2: receiver-agnostic needles ---------------------------------
    // A leading-dot needle matches ANY receiver name, so C#'s `consumer.`,
    // Go's `r.` and Rust's `consumer.` all hit the same row. The LIBRARY gate
    // (never the bare framework word) is what keeps Rx/NATS/RxJS out.
    // C# — Confluent.Kafka: `consumer.Subscribe("orders")`.
    (".Subscribe(", QueueFramework::Kafka, &["confluent.kafka"], TopicRule::ArgLiteral),
    // Java/Kotlin — Spring: `@KafkaListener(topics = "orders")`.
    ("@KafkaListener", QueueFramework::Kafka, &["springframework.kafka"], TopicRule::Keyed(&["topics", "topic"])),
    // Go — segmentio/kafka-go: the topic is a ReaderConfig struct field.
    ("kafka.NewReader(", QueueFramework::Kafka, &["kafka-go", "segmentio"], TopicRule::Keyed(&["topic"])),
    ("kafka.ReaderConfig{", QueueFramework::Kafka, &["kafka-go"], TopicRule::Keyed(&["topic"])),
    // Go — `r.ReadMessage(ctx)` proves the consumer is live but names no topic.
    // NoIdentity is the one rule that emits NOTHING rather than falling back to
    // a framework tag, so it cannot shadow the ReaderConfig row above.
    (".ReadMessage(", QueueFramework::Kafka, &["kafka"], TopicRule::NoIdentity),
    // Scala — Alpakka / akka-stream-kafka: `Subscriptions.topics("orders")`.
    ("Subscriptions.topics(", QueueFramework::Kafka, &["akka.kafka"], TopicRule::ArgLiteral),
    // Rust — rdkafka: `consumer.subscribe(&["orders"])`.
    (".subscribe(&[", QueueFramework::Kafka, &["rdkafka"], TopicRule::ArgLiteral),
    // Python — pika / amqp: `ch.queue_declare(queue="orders")`, receiver-free.
    ("queue_declare(", QueueFramework::RabbitMQ, &["pika", "amqp"], TopicRule::KeyedOrArg(&["queue"])),
    ("basic_consume(", QueueFramework::RabbitMQ, &["pika", "amqp"], TopicRule::KeyedOrArg(&["queue"])),
    // C# — RabbitMQ.Client: `channel.QueueDeclare(queue: "orders", ...)`.
    (".QueueDeclare(", QueueFramework::RabbitMQ, &["rabbitmq.client"], TopicRule::KeyedOrArg(&["queue"])),
];

const PRODUCER_PATTERNS: &[(&str, QueueFramework, &[&str], TopicRule)] = &[
    // `.delay(` collides with `setTimeout.delay`, jQuery `.delay`, Carrierwave,
    // and many JS animation libs; require Celery presence.
    (".delay(", QueueFramework::Celery, &["celery", "@shared_task"], TopicRule::ArgLiteral),
    (".apply_async(", QueueFramework::Celery, &[], TopicRule::ArgLiteral),
    // `.send(` is wildly overloaded (`res.send`, `socket.send`, ...). Require
    // Dramatiq import — `import dramatiq` or `@dramatiq.actor`.
    (".send(", QueueFramework::Dramatiq, &["dramatiq"], TopicRule::ArgLiteral),
    // `queue.add(` — generic var name; require BullMQ context.
    ("queue.add(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"], TopicRule::ArgLiteral),
    // `new Queue(` — also generic; require BullMQ.
    ("new Queue(", QueueFramework::BullMQ, &["bullmq", "@nestjs/bullmq"], TopicRule::ArgLiteral),
    ("perform_async", QueueFramework::Sidekiq, &[], TopicRule::ArgLiteral),
    ("perform_in", QueueFramework::Sidekiq, &[], TopicRule::ArgLiteral),
    ("Oban.insert", QueueFramework::Oban, &[], TopicRule::ArgLiteral),
    ("nc.publish", QueueFramework::Nats, &["nats"], TopicRule::ArgLiteral),
    // Go's nats.go exports Capitalized `nc.Publish`; the lowercase JS needle
    // never matches Go source, so Go NATS producers went blind.
    ("nc.Publish", QueueFramework::Nats, &["nats"], TopicRule::ArgLiteral),
    ("channel.publish", QueueFramework::RabbitMQ, &["amqp", "amqplib", "rabbitmq"], TopicRule::ArgLiteral),
    ("channel.basic_publish", QueueFramework::RabbitMQ, &[], TopicRule::ArgLiteral),
    ("producer.send", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::KeyedOrArg(&["topic"])),
    ("producer.produce", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::ArgLiteral),
    // Go Kafka producers: confluent `producer.Produce`, segmentio `writer.WriteMessages`.
    // A2.2: the trailing `(` is REQUIRED — without it this needle also swallows
    // C#'s `_producer.ProduceAsync(`, whose topic sits where this rule cannot
    // read it, manufacturing a `queue_producer:unresolved:kafka` tag beside the
    // real node.
    ("producer.Produce(", QueueFramework::Kafka, &["kafka", "confluent"], TopicRule::KeyedOrArg(&["topic"])),
    ("writer.WriteMessages", QueueFramework::Kafka, &["kafka", "segmentio"], TopicRule::KeyedOrArg(&["topic"])),
    // Redis-as-queue producer side. .lpush / .rpush both push items onto a
    // list; consumers BLPOP/BRPOP off the other end.
    (".lpush(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    (".rpush(", QueueFramework::RedisList, &["redis"], TopicRule::ArgLiteral),
    ("ListLeftPush(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListLeftPushAsync(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListRightPush(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    ("ListRightPushAsync(", QueueFramework::RedisList, &["stackexchange.redis"], TopicRule::ArgLiteral),
    // ---- A2.2: receiver-agnostic needles ---------------------------------
    // C# — Confluent.Kafka: `_producer.ProduceAsync("orders", msg)`.
    (".ProduceAsync(", QueueFramework::Kafka, &["confluent.kafka"], TopicRule::ArgLiteral),
    (".Produce(", QueueFramework::Kafka, &["confluent.kafka"], TopicRule::ArgLiteral),
    // Java/Kotlin — Spring: `kafkaTemplate.send("orders", payload)`. Carrying
    // `Template.` keeps this off a bare `.send(` while matching any spelling of
    // the field (kafkaTemplate / KafkaTemplate / ordersTemplate).
    ("Template.send(", QueueFramework::Kafka, &["springframework.kafka"], TopicRule::ArgLiteral),
    // Java/Scala — plain client, diamond form: `new ProducerRecord<>("orders", v)`.
    // KNOWN MISS: the explicit-generics form `new ProducerRecord<String,String>(`
    // is unreachable because the argument-region walker cannot step over `<...>`.
    ("ProducerRecord<>(", QueueFramework::Kafka, &["kafka"], TopicRule::ArgLiteral),
    // Go — segmentio/kafka-go: `w.WriteMessages(ctx, kafka.Message{Topic: "x"})`.
    (".WriteMessages(", QueueFramework::Kafka, &["kafka-go", "segmentio"], TopicRule::Keyed(&["topic"])),
    ("kafka.NewWriter(", QueueFramework::Kafka, &["kafka-go", "segmentio"], TopicRule::Keyed(&["topic"])),
    // Rust — rdkafka: `FutureRecord::to("orders")`.
    ("FutureRecord::to(", QueueFramework::Kafka, &["rdkafka"], TopicRule::ArgLiteral),
    // PHP — rdkafka: the topic is named on `$producer->newTopic("orders")`;
    // `$topic->produce(PARTITION, flags, $payload)` is liveness only: arg #1 is
    // the msgflags int and NO argument ever holds the topic, which is bound to
    // the `$topic` object. NoIdentity (not ArgIndex(1), as first drafted) —
    // otherwise this row's empty topic list falls back to a
    // `queue_producer:unresolved:kafka` tag standing beside the real node the
    // `newTopic(` row just emitted.
    ("newTopic(", QueueFramework::Kafka, &["rdkafka"], TopicRule::ArgLiteral),
    ("->produce(", QueueFramework::Kafka, &["rdkafka"], TopicRule::NoIdentity),
    // Ruby — WaterDrop / Karafka: `produce_async(topic: "orders", payload: p)`.
    (".produce_async(", QueueFramework::Kafka, &["waterdrop", "karafka", "rdkafka"], TopicRule::Keyed(&["topic"])),
    // C / C++ — librdkafka: `rd_kafka_topic_new(rk, "orders", NULL)`.
    ("rd_kafka_topic_new(", QueueFramework::Kafka, &["rdkafka", "librdkafka"], TopicRule::ArgIndex(1)),
    // Clojure — `(send! producer {:topic "orders" :value v})`. KNOWN MISS: the
    // separator-less EDN map is unreadable to the keyed scanner, so this row is
    // framework-tag liveness until the scanner learns `{:k v}`.
    ("send!", QueueFramework::Kafka, &["kafka"], TopicRule::Keyed(&["topic"])),
    // Java/Kotlin — Spring AMQP: `rabbitTemplate.convertAndSend("orders", msg)`.
    ("Template.convertAndSend(", QueueFramework::RabbitMQ, &["springframework.amqp"], TopicRule::ArgLiteral),
    // C# — RabbitMQ.Client: `BasicPublish(exchange, routingKey, props, body)`.
    (".BasicPublish(", QueueFramework::RabbitMQ, &["rabbitmq.client"], TopicRule::ArgIndex(1)),
    // Elixir — AMQP: `publish(chan, exchange, routing_key, payload)` — arg #2 is
    // the routing key; arg #1 is the exchange, which is usually "".
    ("AMQP.Basic.publish(", QueueFramework::RabbitMQ, &["amqp"], TopicRule::ArgIndex(2)),
    // Python — pika, receiver-free: `ch.basic_publish(routing_key="orders")`.
    // Positional arg #0 is deliberately NOT read: for pika it is the exchange.
    ("basic_publish(", QueueFramework::RabbitMQ, &["pika", "amqp"], TopicRule::Keyed(&["routing_key", "queue"])),
];

/// True when `signals` is empty (always pass) or any signal substring appears in
/// `lower_source`. Lets distinct framework names gate their broad-needle patterns.
///
/// `lower_source` MUST already be `to_ascii_lowercase`d, and every signal literal
/// in both tables MUST be spelled lowercase (`signal_lists_are_lowercase` asserts
/// it) — that is what makes the GATE case-insensitive. It used to compare the raw
/// source, so `using Confluent.Kafka;` failed a `["kafka", "confluent"]` gate and
/// a whole C# Kafka file emitted nothing at all.
///
/// The NEEDLES stay case-sensitive: API names are (`Produce` is not `produce`).
fn signals_present(lower_source: &str, signals: &[&str]) -> bool {
    signals.is_empty() || signals.iter().any(|s| lower_source.contains(s))
}

pub struct QueueNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
}

/// Queue-consumer nodes for one file — one node per DISTINCT (topic, framework).
///
/// `path` is the file the source came from; it is only used by the
/// `GLIA_QUEUE_DEBUG` marker today (A2.8 attaches it to the node).
pub fn extract_queue_consumer_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> QueueNodes {
    emit_queue_nodes(
        source,
        path,
        module_id,
        repo,
        CONSUMER_PATTERNS,
        node_kind::QUEUE_CONSUMER,
        "queue_consumer:",
    )
}

/// Queue-producer nodes for one file — one node per DISTINCT (topic, framework).
pub fn extract_queue_producer_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> QueueNodes {
    emit_queue_nodes(
        source,
        path,
        module_id,
        repo,
        PRODUCER_PATTERNS,
        node_kind::QUEUE_PRODUCER,
        "queue_producer:",
    )
}

/// Shared emit loop for both sides.
///
/// Was: one topic per needle, read from the FIRST occurrence of that needle in
/// the file. Now: every occurrence is scanned and each distinct topic gets its
/// own node, so a file that publishes to `orders` and `payments` stops hiding
/// one of them. The framework-tag fallback is emitted only when NO occurrence
/// of that needle named a topic.
fn emit_queue_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    patterns: &[(&str, QueueFramework, &[&str], TopicRule)],
    kind: repo_graph_core::NodeKindId,
    prefix: &str,
) -> QueueNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();
    // ONE allocation per emit call (two per file, both sides) — cheap beside the
    // tree-sitter parse that already ran, and it is what makes the gate
    // case-insensitive for every row at once.
    let lower = source.to_ascii_lowercase();

    for (pattern, framework, signals, rule) in patterns {
        if !source.contains(pattern) || !signals_present(&lower, signals) {
            continue;
        }
        let hits = queue_topic::scan(source, pattern, *rule);
        let topics: Vec<String> = hits.into_iter().filter_map(|h| h.topic).collect();
        if debug_enabled() && !topics.is_empty() {
            eprintln!(
                "[queues] scan needle='{pattern}' rule={rule:?} hits={} path={path} topics={}",
                topics.len(),
                topics.join(",")
            );
        }
        for topic in &topics {
            if push_node(
                &mut nodes, &mut nav, &mut seen, topic, framework, module_id, repo, kind, prefix,
                Confidence::Medium,
            ) {
                fired_on(pattern, framework, topic, path);
            }
        }
        // A needle whose rule is `NoIdentity` NEVER names a topic (it is a
        // liveness signal — Go's `r.ReadMessage(ctx)`), so falling back to a
        // topic-less framework tag here would manufacture exactly the all-to-all
        // tag pairing A2.1 removed. Every other rule keeps the fallback.
        if topics.is_empty() && !matches!(rule, TopicRule::NoIdentity) {
            // A2.3: `Weak`, not `Medium`. The tag proves the framework is live in
            // this file and nothing else; ranking it level with a node that names
            // a real topic overstated what was actually read off the source. The
            // `seen` key is `{topic}:{framework:?}`, so one tag per (framework,
            // direction) per file however many needles of that framework fired.
            let tag = framework_tag(framework);
            if push_node(
                &mut nodes, &mut nav, &mut seen, &tag, framework, module_id, repo, kind, prefix,
                Confidence::Weak,
            ) {
                fired_on(pattern, framework, &tag, path);
            }
        }
    }

    QueueNodes { nodes, nav }
}

#[allow(clippy::too_many_arguments)]
fn push_node(
    nodes: &mut Vec<Node>,
    nav: &mut CodeNav,
    seen: &mut std::collections::HashSet<String>,
    topic: &str,
    framework: &QueueFramework,
    module_id: NodeId,
    repo: RepoId,
    kind: repo_graph_core::NodeKindId,
    prefix: &str,
    confidence: Confidence,
) -> bool {
    if !seen.insert(format!("{topic}:{framework:?}")) {
        return false;
    }
    let qname = format!("{prefix}{topic}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
    nodes.push(Node {
        id,
        repo,
        confidence,
        cells: vec![],
    });
    nav.record(id, topic, &qname, kind, Some(module_id));
    true
}

/// Identity-free fallback when no occurrence of a needle named a topic.
///
/// BREAKING (A2.3): prefixed with [`UNRESOLVED_PREFIX`], so the tag is
/// structurally unpairable — `queue_producer:unresolved:kafka`, not
/// `queue_producer:kafka`. The bare spelling was indistinguishable from a repo
/// that genuinely publishes to a topic literally named `kafka`, which is what
/// let the resolver pair it. [`QueueFramework::family`] is deliberately NOT used
/// for the suffix — it folds `RedisList`/`RedisPubSub` together and would
/// silently merge two distinct coverage signals.
fn framework_tag(f: &QueueFramework) -> String {
    format!("{UNRESOLVED_PREFIX}{}", format!("{f:?}").to_lowercase())
}

/// Grep-able proof that a needle passed its gate and produced a node.
/// `GLIA_QUEUE_DEBUG=1 cargo test -p repo-graph-code-extractors -- --nocapture
///  2>&1 | grep "\\[queues\\] needle '"`
fn fired_on(needle: &str, framework: &QueueFramework, topic: &str, path: &str) {
    if debug_enabled() {
        eprintln!(
            "[queues] needle '{needle}' framework={framework:?} gate=ok topic={topic} file={path}"
        );
    }
}

/// `GLIA_QUEUE_DEBUG=1` turns on the `[queues] scan needle=` marker, read once.
fn debug_enabled() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("GLIA_QUEUE_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0")
    })
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

    const PATH: &str = "src/test.rs";

    fn qnames(r: &QueueNodes) -> Vec<String> {
        let mut v: Vec<String> = r.nav.qname_by_id.values().cloned().collect();
        v.sort();
        v
    }

    #[test]
    fn detects_celery() {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test");
        let refs = extract_queue_consumers("@celery.task\ndef process():", id);
        assert!(refs.iter().any(|r| r.framework == QueueFramework::Celery));
    }

    #[test]
    fn detects_bullmq() {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test");
        let refs = extract_queue_consumers(
            "import { Worker } from 'bullmq';\nconst worker = new Worker('queue', handler);",
            id,
        );
        assert!(refs.iter().any(|r| r.framework == QueueFramework::BullMQ));
    }

    #[test]
    fn detects_sidekiq() {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test");
        let refs = extract_queue_consumers("include Sidekiq::Worker", id);
        assert!(refs.iter().any(|r| r.framework == QueueFramework::Sidekiq));
    }

    #[test]
    fn consumer_nodes_with_topic() {
        let source = "import { Worker } from 'bullmq';\nconst worker = new Worker('emails', handler);";
        let result = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert_eq!(result.nodes.len(), 1);
        let qname = result.nav.qname_by_id.values().next().unwrap();
        assert_eq!(qname, "queue_consumer:emails");
    }

    #[test]
    fn producer_nodes() {
        let source = "from celery import Celery\nsend_email.delay('hello')";
        let result = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nav.kind_by_id[&result.nodes[0].id], node_kind::QUEUE_PRODUCER);
    }

    #[test]
    fn express_res_send_does_not_match_dramatiq_producer() {
        // Express response handler — `.send(` is not a Dramatiq producer
        // because the file has no `dramatiq` import. Was the dominant false
        // positive in the 2026-05-05 substrate eval (16/18 producers were
        // HTTP response strings).
        let source = r#"
const express = require('express');
const app = express();
app.get('/', (req, res) => {
    res.send('Hello World');
});
app.get('/users', (req, res) => {
    res.send('<p>Users online: 42</p>');
});
"#;
        let result = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert!(
            result.nodes.is_empty(),
            "express res.send must not emit a Dramatiq producer (no dramatiq import)"
        );
    }

    #[test]
    fn dramatiq_send_emits_when_import_present() {
        // Same `.send(` pattern, but file imports dramatiq → emit.
        let source = r#"
import dramatiq

@dramatiq.actor
def greet(name):
    pass

greet.send('alice')
"#;
        let result = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert!(!result.nodes.is_empty(), "dramatiq import unlocks .send pattern");
    }

    #[test]
    fn redis_list_as_queue_pair() {
        // Voting-app shape: Python pushes on `votes`, .NET pops from `votes`.
        // Same topic across services → SHARES_QUEUE on cross-graph join.
        let producer = "import redis\nr = redis.Redis()\nr.rpush('votes', vote)";
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        assert_eq!(pr.nodes.len(), 1);
        let pq = pr.nav.qname_by_id.values().next().unwrap();
        assert_eq!(pq, "queue_producer:votes");

        let consumer = r#"using StackExchange.Redis;
var entry = db.ListLeftPop("votes");"#;
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(cr.nodes.len(), 1);
        let cq = cr.nav.qname_by_id.values().next().unwrap();
        assert_eq!(cq, "queue_consumer:votes");
    }

    #[test]
    fn jquery_delay_does_not_match_celery_producer() {
        let source = "$('#el').fadeIn().delay(500).fadeOut();";
        let result = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert!(
            result.nodes.is_empty(),
            "jQuery .delay() must not emit a Celery producer (no celery import)"
        );
    }

    #[test]
    fn go_nats_capitalized_producer_and_consumer_with_topic() {
        // Go's nats.go exports Capitalized `nc.Publish` / `nc.Subscribe`, and the
        // topic is the first call argument after `(` — earlier lowercase-only
        // needles + a literal scanner that stopped at `(` left Go queues blind.
        let producer = r#"
import "github.com/nats-io/nats.go"

func PublishOrder(nc *nats.Conn, payload []byte) error {
	return nc.Publish("orders", payload)
}
"#;
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        assert_eq!(pr.nodes.len(), 1);
        assert_eq!(pr.nav.kind_by_id[&pr.nodes[0].id], node_kind::QUEUE_PRODUCER);
        let pq = pr.nav.qname_by_id.values().next().unwrap();
        assert_eq!(pq, "queue_producer:orders");

        let consumer = r#"
import "github.com/nats-io/nats.go"

func SubscribeOrders(nc *nats.Conn) (*nats.Subscription, error) {
	return nc.Subscribe("orders", handle)
}
"#;
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(cr.nodes.len(), 1);
        assert_eq!(cr.nav.kind_by_id[&cr.nodes[0].id], node_kind::QUEUE_CONSUMER);
        let cq = cr.nav.qname_by_id.values().next().unwrap();
        // Topic must be the subject "orders", not the framework tag "nats".
        assert_eq!(cq, "queue_consumer:orders");
    }

    #[test]
    fn kafka_producer_and_consumer() {
        // BREAKING (A2.1): the producer half used to collapse to
        // `queue_producer:unresolved:kafka` because the object form was unreadable; it now
        // carries the real topic. The consumer half still reads as the tag: the
        // only needle that fires is `KafkaConsumer`, whose occurrences (an
        // import and a bare `new KafkaConsumer()`) name no topic — the topic
        // lives on `c.subscribe(...)`, and `c` is not the `consumer.` receiver
        // the table matches. A2.x can widen that needle; this asserts today.
        let consumer = "import { KafkaConsumer } from 'kafkajs';\nconst c = new KafkaConsumer();\nc.subscribe('user-events')";
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:unresolved:kafka".to_string()]);

        let producer = "import { Kafka } from 'kafkajs';\nproducer.send({ topic: 'user-events' })";
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:user-events".to_string()]);
    }

    // ---- A2.1: every occurrence, real argument forms ----------------------

    #[test]
    fn multi_topic_one_file() {
        // THE packet's reason to exist: `source.find(needle)` saw only the
        // FIRST send, so `payments` was invisible to the graph.
        let source = r#"
import { Kafka } from 'kafkajs';
const producer = kafka.producer();

export async function publishOrder(o) {
  await producer.send({ topic: 'orders', messages: [{ value: o }] });
}
export async function publishPayment(p) {
  await producer.send({ topic: 'payments', messages: [{ value: p }] });
}
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(pr.nodes.len(), 2);
        assert_eq!(
            qnames(&pr),
            vec![
                "queue_producer:orders".to_string(),
                "queue_producer:payments".to_string()
            ]
        );
    }

    #[test]
    fn kafkajs_object_form() {
        let source =
            "import { Kafka } from 'kafkajs';\nawait consumer.subscribe({ topic: 'orders', fromBeginning: true });";
        let cr = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:orders".to_string()]);
    }

    #[test]
    fn java_arrays_aslist() {
        let source = r#"
import org.apache.kafka.clients.consumer.KafkaConsumer;
consumer.subscribe(Arrays.asList("orders"));
"#;
        let cr = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert!(
            qnames(&cr).contains(&"queue_consumer:orders".to_string()),
            "Arrays.asList(\"orders\") must resolve to the topic, got {:?}",
            qnames(&cr)
        );
    }

    #[test]
    fn go_struct_topic_field() {
        let source = r#"
import "github.com/segmentio/kafka-go"

func Publish(w *kafka.Writer, v []byte) error {
	return writer.WriteMessages(ctx, kafka.Message{Topic: "orders", Value: v})
}
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
    }

    #[test]
    fn backtick_literal() {
        let source = "import { Queue } from 'bullmq';\nqueue.add(`emails`, job);";
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:emails".to_string()]);
    }

    #[test]
    fn payload_literal_in_arg_one_is_not_a_topic() {
        // PRECISION REGRESSION GUARD: fails the moment the region scan is
        // widened past positional arg #0.
        let source = "import { Kafka } from 'kafkajs';\nproducer.send(topicVar, \"payload\");";
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(
            qnames(&pr),
            vec!["queue_producer:unresolved:kafka".to_string()],
            "an unreadable topic must fall back to the framework tag, never to the payload"
        );
    }

    #[test]
    fn unparseable_topic_emits_weak_unresolved_node() {
        // A2.3: the topic is an env var, so nothing readable names it. The node
        // must still exist (it is the coverage signal "this file talks to Kafka,
        // topic unknown") but it must be spelled `unresolved:` so the resolver
        // can refuse to pair it, and it must be Weak, not Medium.
        let source = "import { Kafka } from 'kafkajs';\nconst topic = process.env.T;\nproducer.send({ topic, messages: [] });";
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(
            qnames(&pr),
            vec!["queue_producer:unresolved:kafka".to_string()],
            "an unreadable topic must be a self-declaring sentinel, not a bare framework name"
        );
        assert_eq!(pr.nodes.len(), 1);
        assert_eq!(
            pr.nodes[0].confidence,
            Confidence::Weak,
            "the tag proves the framework is live, nothing more"
        );
        // The topic-bearing sibling stays Medium — this is the contrast the
        // confidence split exists to express.
        let named = "import { Kafka } from 'kafkajs';\nproducer.send({ topic: 'orders' });";
        let ok = extract_queue_producer_nodes(named, PATH, module_id(), repo());
        assert_eq!(qnames(&ok), vec!["queue_producer:orders".to_string()]);
        assert_eq!(ok.nodes[0].confidence, Confidence::Medium);
    }

    #[test]
    fn unresolved_prefix_is_not_a_legal_topic_shape() {
        // Guards the one way the sentinel could collide with a real topic: the
        // resolver keys on this exact prefix, so it must stay in lockstep with
        // what `framework_tag` actually emits.
        assert!(framework_tag(&QueueFramework::Kafka).starts_with(UNRESOLVED_PREFIX));
        assert_eq!(framework_tag(&QueueFramework::Kafka), "unresolved:kafka");
    }

    #[test]
    fn tag_fallback_is_emitted_once_per_framework() {
        // Three `nc.Publish` calls, none with a literal subject: exactly one
        // tag node, not three.
        let source = r#"
import "github.com/nats-io/nats.go"
nc.Publish(subjectA, a)
nc.Publish(subjectB, b)
nc.Publish(subjectC, c)
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:unresolved:nats".to_string()]);
    }

    // ---- A2.2: receiver-agnostic needles + case-insensitive gates ---------

    #[test]
    fn signal_lists_are_lowercase() {
        // The gate compares against a `to_ascii_lowercase`d copy of the source,
        // so ONE upper-case letter in a signal literal is a permanently dead
        // gate — the row can never fire again. Asserted, not commented.
        for (needle, _, signals, _) in CONSUMER_PATTERNS.iter().chain(PRODUCER_PATTERNS) {
            for s in *signals {
                assert_eq!(
                    s.to_string(),
                    s.to_ascii_lowercase(),
                    "signal {s:?} gating needle {needle:?} must be spelled lowercase"
                );
            }
        }
    }

    #[test]
    fn csharp_confluent_gate_is_case_insensitive() {
        // THE packet's reason to exist. The only Kafka evidence in either file
        // is `using Confluent.Kafka;` — there is no lowercase "kafka" anywhere
        // (no bootstrap.servers host). Under the old case-SENSITIVE gate both
        // halves of this service emitted NOTHING AT ALL.
        let producer = r#"using System.Threading.Tasks;
using Confluent.Kafka;

public sealed class OrderProducer
{
    private readonly IProducer<Null, string> _producer;

    public async Task PublishAsync(string payload)
    {
        await _producer.ProduceAsync("orders", new Message<Null, string> { Value = payload });
    }
}"#;
        assert!(!producer.contains("kafka"), "fixture must carry no lowercase 'kafka'");
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);

        let consumer = r#"using System.Threading;
using Confluent.Kafka;

public sealed class OrderConsumer
{
    public void Run(ConsumerConfig config, CancellationToken ct)
    {
        var consumer = new ConsumerBuilder<Ignore, string>(config).Build();
        consumer.Subscribe("orders");
        var result = consumer.Consume(ct);
    }
}"#;
        assert!(!consumer.contains("kafka"), "fixture must carry no lowercase 'kafka'");
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        assert_eq!(qnames(&cr), vec!["queue_consumer:orders".to_string()]);
    }

    #[test]
    fn rx_subscribe_without_kafka_gate_emits_nothing() {
        // PRECISION GUARD for the receiver-agnostic `.Subscribe(` needle: the
        // gate is the ONLY thing keeping Rx/ReactiveX out. Fails the moment the
        // `confluent.kafka` signal list is dropped or widened to "kafka".
        let source = r#"using System;
using System.Reactive.Linq;

public sealed class Ticker
{
    public IDisposable Start(IObservable<long> source)
    {
        return source.Subscribe("ignored");
    }
}"#;
        let cr = extract_queue_consumer_nodes(source, PATH, module_id(), repo());
        assert!(
            cr.nodes.is_empty(),
            "Rx .Subscribe( must not emit a Kafka consumer, got {:?}",
            qnames(&cr)
        );
    }

    #[test]
    fn spring_kafka_template_producer() {
        // `kafkaTemplate.send(` matched NO needle before: the table's row was
        // the receiver-bound `producer.send`.
        let source = r#"
import org.springframework.kafka.core.KafkaTemplate;

public class OrderProducer {
    private final KafkaTemplate<String, String> kafkaTemplate;

    public void publish(String payload) {
        kafkaTemplate.send("orders", payload);
    }
}
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
    }

    #[test]
    fn go_segmentio_writer_and_reader() {
        // Idiomatic short receivers `w` / `r` — the old needles were spelled
        // `writer.WriteMessages` / `reader.ReadMessage` and saw nothing here.
        let producer = r#"
import "github.com/segmentio/kafka-go"

func Publish(ctx context.Context, w *kafka.Writer, body []byte) error {
	return w.WriteMessages(ctx, kafka.Message{Topic: "orders", Value: body})
}
"#;
        let pr = extract_queue_producer_nodes(producer, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);

        let consumer = r#"
import "github.com/segmentio/kafka-go"

func Consume(ctx context.Context) error {
	r := kafka.NewReader(kafka.ReaderConfig{
		Brokers: []string{"localhost:9092"},
		Topic:   "orders",
		GroupID: "svc",
	})
	m, err := r.ReadMessage(ctx)
	_ = m
	return err
}
"#;
        let cr = extract_queue_consumer_nodes(consumer, PATH, module_id(), repo());
        // `.ReadMessage(` is NoIdentity: it proves the consumer is live but
        // names no topic, so it must NOT add a `queue_consumer:unresolved:kafka` tag
        // beside the real topic the ReaderConfig row already named.
        assert_eq!(qnames(&cr), vec!["queue_consumer:orders".to_string()]);
    }

    #[test]
    fn rust_rdkafka_future_record() {
        let source = r#"
use rdkafka::producer::{FutureProducer, FutureRecord};

async fn publish(p: &FutureProducer, payload: &str) {
    let _ = p.send(FutureRecord::to("orders").payload(payload), Timeout::Never).await;
}
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
    }

    #[test]
    fn php_rdkafka_new_topic() {
        // `RdKafka\Producer` lowercases to `rdkafka\producer`, so this is a
        // second case-insensitivity proof. `->produce(` is NoIdentity, so the
        // real topic stands alone with no `queue_producer:unresolved:kafka` tag beside it.
        let source = r#"<?php
$producer = new RdKafka\Producer();
$topic = $producer->newTopic("orders");
$topic->produce(RD_KAFKA_PARTITION_UA, 0, $payload);
"#;
        let pr = extract_queue_producer_nodes(source, PATH, module_id(), repo());
        assert_eq!(qnames(&pr), vec!["queue_producer:orders".to_string()]);
    }
}
